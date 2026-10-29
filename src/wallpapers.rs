use std::collections::HashMap;

use amane::{
    Center, Column, Full, Key, Keyboard, Layer, LayerWindow, Monitor, Parent, Rectangle, Row, Service, Vertical, Widget, Zone, children,
};

use crate::panels::{self, Which};
use crate::theme::{alpha};
use crate::ui::{Wheel, bold, hover, icon, label, set_hover};
use crate::wallpaper::Wallpaper;

/*
 * the wallpaper picker: a strip at the bottom of the screen with a thumbnail per image.
 * thumbnails are made once with ImageMagick, because decoding a few dozen 4K pngs
 * every time the picker opens would stall the shell.
 */
pub struct Wallpapers {
    pub files: Vec<String>,
    // wallpaper -> its finished thumbnail. checked once here, so drawing the strip touches no files
    pub ready: HashMap<String, String>,
    pub selected: usize,
    wheel: Wheel,  // moves the selection
}

const THUMB_WIDTH: u32 = 480;
const THUMB_HEIGHT: u32 = 270;

fn directory() -> String {
    // ~/.config/amane-shell/wallpaper-dir overrides the default folder
    std::fs::read_to_string(crate::cache::config_dir().join("wallpaper-dir"))
        .map(|text| text.trim().to_string())
        .ok()
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| format!("{}/Pictures/Wallpapers", std::env::var("HOME").unwrap_or_default()))
}

// the copy's name includes the file's size and modification time, so replacing a wallpaper with new content under
// the same name makes new copies instead of serving the old ones for ever
fn cache(folder: &str, path: &str) -> String {
    let (size, modified) = std::fs::metadata(path)
        .ok()
        .map(|metadata| (metadata.len(), metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |time| time.as_secs())))
        .unwrap_or((0, 0));

    let key = crate::cache::fnv(format!("{path}\0{size}\0{modified}").as_bytes());

    crate::cache::cache_dir().join(folder).join(format!("{key:x}.jpg")).to_string_lossy().into_owned()
}

fn thumbnail_path(path: &str) -> String {
    cache("thumbs", path)
}

/*
 * what the desktop actually draws. Amane only reads real png, jpeg and svg data, and a wallpaper can be a webp with
 * a .jpg name, or a 36 megapixel scan that takes seconds to decode and a lot of memory. so every wallpaper gets a
 * jpeg at the biggest screen's size, made once with ImageMagick
 */
const DISPLAY: &str = "2560x1440";
const DISPLAY_SIZE: (u32, u32) = (2560, 1440);

fn display_path(path: &str) -> String {
    cache("display", path)
}

/*
 * a copy counts as ready only when it is a whole jpeg: it ends with the end-of-image marker. a file that merely
 * exists could be half written (the shell was restarted mid-conversion, or something is converting it right now)
 */
fn complete_jpeg(path: &str) -> bool {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else { return false };

    let mut end = [0u8; 2];

    file.seek(SeekFrom::End(-2)).is_ok() && file.read_exact(&mut end).is_ok() && end == [0xFF, 0xD9]
}

pub fn display_if_ready(path: &str) -> Option<String> {
    let display = display_path(path);

    complete_jpeg(&display).then_some(display)
}

pub fn thumbnail_if_ready(path: &str) -> Option<String> {
    let thumbnail = thumbnail_path(path);

    complete_jpeg(&thumbnail).then_some(thumbnail)
}

fn quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/*
 * every run writes its own temporary file and renames it when whole, so two runs on one image can't corrupt each
 * other. but two runs on one image would be wasted work (the background thread and a wallpaper you just picked can
 * want the same file), so the second waits for the first. nothing is held while ImageMagick runs, so a wallpaper you
 * pick never queues behind the background conversions of other images
 */
static RUNNING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/*
 * a jpeg can be decoded straight at 1/8, 2/8 ... 7/8 of its size, which takes a fraction of the memory: a 6000 pixel
 * photo decoded whole takes hundreds of megabytes for a moment, only to be shrunk. ImageMagick's jpeg:size picks the
 * scale below the size asked for, not above, so the exact size of the smallest scale that still covers `fill` is
 * worked out here from the picture's own size. None for a jpeg that can't shrink (or anything else): decode it whole
 */
fn decode_size(width: u32, height: u32, rotated: bool, fill: (u32, u32)) -> Option<(u32, u32)> {
    // a picture stored on its side is turned after decoding, so its stored width becomes the height
    let (fill_width, fill_height) = if rotated { (fill.1, fill.0) } else { fill };

    if width == 0 || height == 0 {
        return None;
    }

    // the eighths needed to cover the fill on both sides
    let eighths = |side: u32, wanted: u32| (u64::from(wanted) * 8).div_ceil(u64::from(side));

    let scale = eighths(width, fill_width).max(eighths(height, fill_height)).max(1);

    if scale >= 8 {
        return None;
    }

    let at = |side: u32| (u64::from(side) * scale).div_ceil(8) as u32;

    Some((at(width), at(height)))
}

// "-define jpeg:size=WxH " for a jpeg that can be decoded smaller, read from its header without decoding it
fn decode_option(source: &str, fill: (u32, u32)) -> String {
    let header = amane::output(&format!("magick identify -ping -format '%m %w %h %[orientation]' {}[0]", quote(source)));

    let mut fields = header.split_whitespace();

    if fields.next() != Some("JPEG") {
        return String::new();
    }

    let mut number = || fields.next().and_then(|text| text.parse().ok()).unwrap_or(0);

    let (width, height) = (number(), number());

    // LeftTop, RightTop, RightBottom and LeftBottom are turned a quarter
    let rotated = matches!(header.split_whitespace().nth(3), Some("LeftTop" | "RightTop" | "RightBottom" | "LeftBottom"));

    decode_size(width, height, rotated, fill).map_or_else(String::new, |(w, h)| format!("-define jpeg:size={w}x{h} "))
}

/// runs ImageMagick from `source` to `target`, written under another name and renamed when it's whole.
/// slow, so never on the drawing thread. `fill` is the size the result is shrunk to cover, so a jpeg can be decoded
/// smaller to begin with (see `decode_size`)
fn convert(source: &str, target: &str, fill: (u32, u32), options: &str) -> bool {
    if complete_jpeg(target) {
        return true;
    }

    let mine = {
        let mut running = crate::sync::lock(&RUNNING);

        let free = !running.iter().any(|other| other == target);

        if free {
            running.push(target.to_string());
        }

        free
    };

    if !mine {
        // someone else is making it: wait for them (up to a minute, a 36 megapixel image takes a few seconds)
        for _ in 0..600 {
            std::thread::sleep(std::time::Duration::from_millis(100));

            if !crate::sync::lock(&RUNNING).iter().any(|other| other == target) {
                break;
            }
        }

        return complete_jpeg(target);
    }

    if let Some(folder) = std::path::Path::new(target).parent() {
        let _ = std::fs::create_dir_all(folder);
    }

    let partial = format!("{target}.{}.part", COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));

    let decode = decode_option(source, fill);

    amane::output(&format!("nice -n 19 magick {decode}{}[0] -auto-orient {options} jpeg:{}", quote(source), quote(&partial)));

    let made = complete_jpeg(&partial) && std::fs::rename(&partial, target).is_ok();

    if !made {
        let _ = std::fs::remove_file(&partial);
    }

    crate::sync::lock(&RUNNING).retain(|other| other != target);

    made
}

// copies of wallpapers that were replaced or deleted (their names come from the file's size and time) would pile up.
// the wallpaper in use is kept too: it can live outside the folder (`amane ipc call wallpaper <path>`)
fn sweep_orphans() {
    let mut files = Wallpapers::read().files.clone();

    files.push(Wallpaper::read().path.clone());

    for folder in ["thumbs", "display", "blurred"] {
        let expected: std::collections::HashSet<std::ffi::OsString> =
            files.iter().filter_map(|path| std::path::Path::new(&cache(folder, path)).file_name().map(std::ffi::OsStr::to_os_string)).collect();

        let Some(directory) = std::path::Path::new(&cache(folder, "")).parent().map(std::path::Path::to_path_buf) else { continue };
        let Ok(entries) = std::fs::read_dir(directory) else { continue };

        for entry in entries.flatten() {
            let path = entry.path();

            let is_copy = path.extension().is_some_and(|extension| extension == "jpg");

            if is_copy && !expected.contains(entry.file_name().as_os_str()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

// conversions that were cut short by a restart leave `.part` files behind
fn sweep_partials() {
    for folder in ["thumbs", "display", "blurred"] {
        let Some(directory) = std::path::Path::new(&cache(folder, "")).parent().map(std::path::Path::to_path_buf) else { continue };
        let Ok(entries) = std::fs::read_dir(directory) else { continue };

        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|extension| extension == "part") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// makes the screen-sized copy if it doesn't exist yet
pub fn make_display(path: &str) -> Option<String> {
    // "^>" fills the size and only ever shrinks. flattened onto black, since a wallpaper has no use for transparency
    let options = format!("-background black -alpha remove -alpha off -resize '{DISPLAY}^>' -sampling-factor 4:4:4 -quality 94");

    let display = display_path(path);

    convert(path, &display, DISPLAY_SIZE, &options).then_some(display)
}

/*
 * the login screen's background (see recolor::login_screen), blurred like the lock screen, made from the screen-sized
 * copy and only for the wallpaper in use. a small copy blurred and enlarged again is as soft as a big blur over the
 * whole picture and far quicker, and enlarging a blur shows no pixels. none until the screen-sized copy exists
 */
pub fn make_blurred(path: &str) -> Option<String> {
    let display = display_if_ready(path)?;

    let blurred = cache("blurred", path);

    // decoded at twice the size it's blurred at, and enlarged only to 1280: the login screen scales a blur up seamlessly.
    // on one thread, since nothing waits for it
    convert(&display, &blurred, (960, 540), "-limit thread 1 -resize 480x -blur 0x8 -resize 1280x -quality 90").then_some(blurred)
}

fn make_thumbnail(path: &str) -> Option<String> {
    let options = format!("-thumbnail {THUMB_WIDTH}x{THUMB_HEIGHT}^ -gravity center -extent {THUMB_WIDTH}x{THUMB_HEIGHT} -quality 82");

    let thumbnail = thumbnail_path(path);

    // decoded at twice the thumbnail's size at least, so the last shrink is ImageMagick's own and stays sharp
    convert(path, &thumbnail, (THUMB_WIDTH * 2, THUMB_HEIGHT * 2), &options).then_some(thumbnail)
}

fn list() -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(directory())
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| matches!(extension.to_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp"))
                })
                .map(|path| path.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();

    files.sort_by_key(|path| path.to_lowercase());

    files
}

impl Service for Wallpapers {
    fn new() -> Self {
        let files = list();

        let ready = files.iter().filter_map(|path| Some((path.clone(), thumbnail_if_ready(path)?))).collect();

        Self { files, ready, selected: 0, wheel: Wheel::new() }
    }

    // thumbnails and screen-sized copies are made in the background, one image at a time, at low priority.
    // the wallpaper showing now goes first
    fn listen() {
        sweep_partials();

        let current = Wallpaper::read().path.clone();

        let mut todo: Vec<String> = {
            let wallpapers = Self::read();

            wallpapers
                .files
                .iter()
                .filter(|path| !wallpapers.ready.contains_key(*path) || display_if_ready(path).is_none())
                .cloned()
                .collect()
        };

        // the wallpaper in use may come from somewhere else than the folder, and needs its copy all the same
        if !current.is_empty() && !todo.contains(&current) && display_if_ready(&current).is_none() && std::path::Path::new(&current).exists() {
            todo.push(current.clone());
        }

        todo.sort_by_key(|path| *path != current);

        for path in todo {
            if let Some(thumbnail) = make_thumbnail(&path) {
                Self::write().ready.insert(path.clone(), thumbnail);
            }

            let display = make_display(&path);

            // the wallpaper that was waiting for its copy can be drawn now
            if let Some(display) = display {
                crate::wallpaper::display_ready(&path, &display);
            }
        }

        sweep_orphans();
    }
}

impl Wallpapers {
    /// start on the wallpaper that's showing now
    pub fn select_current(&mut self) {
        let current = Wallpaper::read().path.clone();

        self.selected = self.files.iter().position(|path| *path == current).unwrap_or(0);
    }

    /// the wheel moves one wallpaper per notch, down or right goes forward
    fn scroll(&mut self, amount: f32) {
        let steps = self.wheel.steps(amount);

        if steps != 0 {
            self.step(steps as isize);
        }
    }

    pub fn step(&mut self, by: isize) {
        if self.files.is_empty() {
            return;
        }

        let count = self.files.len() as isize;

        self.selected = (self.selected as isize + by).rem_euclid(count) as usize;
    }
}

// ---------------------------------------------------------------- view

const BIG: (f32, f32) = (336.0, 189.0);
const SMALL: (f32, f32) = (200.0, 112.5);
const VISIBLE: isize = 3; // thumbnails on each side of the selected one
const HEIGHT: f32 = 300.0;

fn pick(path: String) {
    crate::wallpaper::apply(&path);
}

fn on_key(key: Key) {
    match key {
        Key::Escape => panels::close(),
        Key::Left | Key::Character('h') => Wallpapers::write().step(-1),
        Key::Right | Key::Character('l') => Wallpapers::write().step(1),
        Key::Home => Wallpapers::write().selected = 0,
        Key::End => {
            let mut wallpapers = Wallpapers::write();

            wallpapers.selected = wallpapers.files.len().saturating_sub(1);
        }
        Key::Enter | Key::Space => {
            let path = {
                let wallpapers = Wallpapers::read();

                wallpapers.files.get(wallpapers.selected).cloned()
            };

            if let Some(path) = path {
                pick(path);
            }
        }
        _ => {}
    }
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let wallpapers = Wallpapers::read();

    // reading the service above is what starts its thumbnail work at launch, so it stays. but a closed picker shouldn't
    // build seven tiles (and look at seven files) on every change
    let Some(t) = panels::look(Which::Wallpapers, monitor) else {
        return panels::hidden("amane-dock");
    };
    let current = Wallpaper::read().path.clone();

    let window = LayerWindow::new()
        .width(Full)
        .height(HEIGHT)
        .anchor_vertical(Vertical::Bottom)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("amane-dock")
        .keyboard(Keyboard::Exclusive)
        .on_key(on_key);

    let count = wallpapers.files.len() as isize;

    if count == 0 {
        return window.child(
            panels::body(&t).align_child(Center, Center).child(label("No wallpapers found in ~/Pictures/Wallpapers", 13.0, t.dim)),
        );
    }

    let selected = wallpapers.selected as isize;

    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    for offset in -VISIBLE..=VISIBLE {
        // wraps around, so the strip never runs out
        let index = (selected + offset).rem_euclid(count) as usize;

        // with few images, don't show the same one twice
        if count < VISIBLE * 2 + 1 && (selected + offset < 0 || selected + offset >= count) {
            continue;
        }

        let path = wallpapers.files[index].clone();

        let (width, height) = if offset == 0 { BIG } else { SMALL };

        let key = format!("wp{index}");
        let hot = hover(&key);

        let is_current = path == current;

        // a plain tile until the thumbnail has been made
        let tile = Rectangle::new().width(width).height(height);

        let tile = match wallpapers.ready.get(&path) {
            Some(thumbnail) => tile.fill(amane::Image::cover(thumbnail)),
            None => tile.fill(t.surface_hi),
        };

        // the side tiles are dimmed by a veil of the panel's colour rather than with `Rectangle::opacity`, which Amane
        // draws on a window-sized texture of its own. the panel's colour at 38% over the picture looks about the same
        // as the picture at 62% over the panel
        let veil = if offset == 0 { 0.0 } else { 0.38 * (1.0 - hot) };

        let tile = if veil > 0.0 {
            tile.child(Rectangle::new().width(Parent).height(Parent).fill(alpha(t.panel, veil)))
        } else {
            tile
        };

        let tile = tile.cursor(amane::Pointer);

        let border = if offset == 0 { t.accent } else { alpha(t.fg, 0.0) };

        let target = path.clone();

        let framed = Rectangle::new()
            .width(width + 8.0)
            .height(height + 8.0)
            .border(if offset == 0 { 2.0 } else { 0.0 }, border)
            .padding(4.0)
            .align_child(Center, Center)
            .cursor(amane::Pointer)
            .on_hover(move |inside| set_hover(&key, inside))
            .on_click(move |_| {
                Wallpapers::write().selected = index;

                pick(target.clone());
            })
            .child(tile);

        // a small tag on the wallpaper that is showing now
        let tagged: Box<dyn Widget> = if is_current {
            Box::new(
                amane::Stack::new(children![
                    framed,
                    Rectangle::new()
                        .width(22.0)
                        .height(22.0)
                        .fill(t.accent)
                        .align_child(Center, Center)
                        .translate(width + 8.0 - 26.0, 4.0)
                        .child(icon("\u{f00c}", 11.0, t.on_accent)),
                ]),
            )
        } else {
            Box::new(framed)
        };

        items.push(tagged);
    }

    let name = wallpapers.files[wallpapers.selected]
        .rsplit('/')
        .next()
        .unwrap_or("")
        .rsplit_once('.')
        .map_or_else(String::new, |(stem, _)| stem.to_string());

    let strip = Row::new(items).gap(14.0).align(Center).justify(Center).width(Parent).height(BIG.1 + 16.0);

    let caption = Row::new(children![
        bold(name, 13.0, t.fg),
        label(format!("{} / {}", wallpapers.selected + 1, count), 12.0, t.faint),
        label("\u{2190} \u{2192} choose     enter apply     esc close", 11.5, t.faint),
    ])
    .gap(18.0)
    .align(Center);

    let content = Column::new(children![strip, caption]).gap(14.0).align(Center);

    window.child(
        Rectangle::new()
            .width(Parent)
            .height(Parent)
            // the bar's glass rather than a panel's background: the strip is mostly pictures, not text to read, and
            // sits as see-through as the bar and the windows around it
            .fill(t.glass)
            .align_child(Center, Center)
            .on_scroll(|scroll| Wallpapers::write().scroll(if scroll.y.abs() >= scroll.x.abs() { scroll.y } else { scroll.x }))
            .child(content),
    )
}

#[cfg(test)]
mod tests {
    use super::decode_size;

    // the size ImageMagick decodes at when asked for `size`: the eighth below it, as jpeg:size does
    fn decoded(width: u32, height: u32, size: (u32, u32)) -> (u32, u32) {
        let eighths = (size.0 * 8 / width).min(size.1 * 8 / height);

        ((width * eighths).div_ceil(8), (height * eighths).div_ceil(8))
    }

    #[test]
    fn a_4k_picture_is_decoded_at_six_eighths_to_cover_1440p() {
        let size = decode_size(3840, 2160, false, (2560, 1440)).unwrap();

        assert_eq!(size, (2880, 1620));
        assert_eq!(decoded(3840, 2160, size), (2880, 1620));
    }

    #[test]
    fn the_decoded_picture_always_covers_the_fill() {
        for (width, height) in [(3840, 2160), (6016, 6016), (6067, 3467), (5120, 2880), (6000, 3375), (2561, 1441), (7000, 1500)] {
            for fill in [(2560, 1440), (960, 540)] {
                if let Some(size) = decode_size(width, height, false, fill) {
                    let (w, h) = decoded(width, height, size);

                    assert!(w >= fill.0 && h >= fill.1, "{width}x{height} for {fill:?} decoded at {w}x{h}");
                }
            }
        }
    }

    #[test]
    fn a_picture_stored_on_its_side_covers_the_turned_fill() {
        // a portrait photo stored 4000 wide, shown 3000 wide and 4000 high
        let size = decode_size(4000, 3000, true, (2560, 1440)).unwrap();
        let (w, h) = decoded(4000, 3000, size);

        assert!(h >= 2560 && w >= 1440, "{w}x{h}");
    }

    #[test]
    fn a_picture_too_small_to_shrink_is_decoded_whole() {
        assert_eq!(decode_size(2560, 1440, false, (2560, 1440)), None);
        assert_eq!(decode_size(1920, 1080, false, (2560, 1440)), None);
        assert_eq!(decode_size(0, 0, false, (2560, 1440)), None);
    }
}
