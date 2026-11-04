use std::collections::HashMap;

use amane::{
    Center, Color, Column, Full, Key, Keyboard, Layer, LayerWindow, Margin, Monitor, Parent, Rectangle, Row, Service, SpaceBetween,
    Start, Vertical, Widget, Zone, children,
};

use crate::panels::{self, Which};
use crate::schemes::{self, SCHEMES};
use crate::theme::{self, Theme, alpha};
use crate::ui::{Wheel, bold, hover, label, section, set_hover};
use crate::wallpaper::Wallpaper;

/*
 * the wallpaper picker: a panel at the bottom of the screen with a thumbnail per image, and under it the colour
 * schemes that can replace the wallpaper's colours (see schemes.rs). thumbnails are made once with ImageMagick,
 * because decoding a few dozen 4K pngs every time the picker opens would stall the shell.
 */
pub struct Wallpapers {
    pub files: Vec<String>,
    // wallpaper -> its finished thumbnail. checked once here, so drawing the strip touches no files
    pub ready: HashMap<String, String>,
    pub selected: usize,
    pub scheme: usize, // the scheme chosen in the picker: 0 for the wallpaper's colours, then `SCHEMES` in order
    pub focus: Focus,  // which row the keys move
    wheel: Wheel,      // moves the selection
    scheme_wheel: Wheel,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Wallpapers,
    Schemes,
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

// for a path inside a shell command: single quotes, and any single quote in it closed, escaped and reopened
pub fn quote(path: &str) -> String {
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

        Self { files, ready, selected: 0, scheme: 0, focus: Focus::Wallpapers, wheel: Wheel::new(), scheme_wheel: Wheel::new() }
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
    /// start on the wallpaper that's showing now and the colours in use
    pub fn select_current(&mut self) {
        let current = Wallpaper::read().path.clone();

        self.selected = self.files.iter().position(|path| *path == current).unwrap_or(0);

        self.scheme = schemes::chosen().and_then(|chosen| SCHEMES.iter().position(|scheme| scheme.name == chosen.name)).map_or(0, |index| index + 1);

        self.focus = Focus::Wallpapers;
    }

    fn scroll_schemes(&mut self, amount: f32) {
        let steps = self.scheme_wheel.steps(amount);

        if steps != 0 {
            self.step_scheme(steps as isize);
        }
    }

    // the schemes are a list with two ends, not a loop like the wallpapers
    fn step_scheme(&mut self, by: isize) {
        self.scheme = (self.scheme as isize).saturating_add(by).clamp(0, SCHEMES.len() as isize) as usize;
    }

    fn step_focused(&mut self, by: isize) {
        match self.focus {
            Focus::Wallpapers if by.abs() > self.files.len() as isize => self.selected = if by < 0 { 0 } else { self.files.len().saturating_sub(1) },
            Focus::Wallpapers => self.step(by),
            Focus::Schemes => self.step_scheme(by),
        }
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

const PAD: f32 = 18.0;
const HEADER: f32 = 16.0;

// every thumbnail is the same size, the selected one is framed
const THUMB: (f32, f32) = (224.0, 126.0);
const THUMB_GAP: f32 = 10.0;

// the colour schemes, each drawn in its own colours
const SWATCH: (f32, f32) = (144.0, 56.0);
const SWATCH_GAP: f32 = 8.0;

// the selection's border, 2 wide, sits this far out from what it frames
const FRAME: f32 = 5.0;

// the bar along the bottom of the wallpaper and the scheme in use
const IN_USE: f32 = 3.0;

const HEIGHT: f32 =
    PAD + HEADER + 12.0 + THUMB.1 + 2.0 * FRAME + 18.0 + 1.0 + 18.0 + HEADER + 12.0 + SWATCH.1 + 2.0 * FRAME + PAD;

fn pick(path: String) {
    crate::wallpaper::apply(&path);
}

// 0 is the wallpaper's own colours, then the schemes in order
fn pick_scheme(index: usize) {
    schemes::choose(index.checked_sub(1).and_then(|index| SCHEMES.get(index)));
}

fn on_key(key: Key) {
    let focus = Wallpapers::read().focus;

    match key {
        Key::Escape => panels::close(),
        Key::Up | Key::Character('k') => Wallpapers::write().focus = Focus::Wallpapers,
        Key::Down | Key::Character('j') => Wallpapers::write().focus = Focus::Schemes,
        Key::Tab => {
            let mut wallpapers = Wallpapers::write();

            wallpapers.focus = if focus == Focus::Wallpapers { Focus::Schemes } else { Focus::Wallpapers };
        }
        Key::Left | Key::Character('h') => Wallpapers::write().step_focused(-1),
        Key::Right | Key::Character('l') => Wallpapers::write().step_focused(1),
        Key::Home => Wallpapers::write().step_focused(isize::MIN / 2),
        Key::End => Wallpapers::write().step_focused(isize::MAX / 2),
        Key::Enter | Key::Space => {
            let (path, scheme) = {
                let wallpapers = Wallpapers::read();

                (wallpapers.files.get(wallpapers.selected).cloned(), wallpapers.scheme)
            };

            match focus {
                Focus::Wallpapers => {
                    if let Some(path) = path {
                        pick(path);
                    }
                }
                Focus::Schemes => pick_scheme(scheme),
            }
        }
        _ => {}
    }
}

// how far the panel has faded in, for colours that don't come from its own theme
fn shown(t: &Theme) -> f32 {
    f32::from(t.fg.alpha()) / 255.0
}

// a colour at `amount` of its own opacity
fn faded(color: Color, amount: f32) -> Color {
    alpha(color, f32::from(color.alpha()) / 255.0 * amount)
}

/// draws a 2 wide border around something selected: the accent in the row the keys move, faint in the other
fn framed(t: &Theme, selected: bool, focused: bool, content: Rectangle, size: (f32, f32)) -> Rectangle {
    let color = if focused { t.accent } else { faded(alpha(t.fg, 0.28), shown(t)) };

    Rectangle::new()
        .width(size.0 + 2.0 * FRAME)
        .height(size.1 + 2.0 * FRAME)
        .border(if selected { 2.0 } else { 0.0 }, color)
        .align_child(Center, Center)
        .cursor(amane::Pointer)
        .child(content)
}

// the row's name, in the accent while the keys move it, then what's chosen in it
fn header(t: &Theme, name: &str, focused: bool, chosen: String, extra: Option<String>) -> Row {
    let mut items: Vec<Box<dyn Widget>> = vec![
        Box::new(section(name, t).color(if focused { t.accent } else { t.faint })),
        Box::new(bold(chosen, 12.5, t.fg)),
    ];

    if let Some(extra) = extra {
        items.push(Box::new(label(extra, 12.0, t.faint)));
    }

    Row::new(items).gap(12.0).align(Center).height(HEADER)
}

fn strip(t: &Theme, wallpapers: &Wallpapers, current: &str, width: f32) -> Rectangle {
    let area = Rectangle::new()
        .width(width)
        .height(THUMB.1 + 2.0 * FRAME)
        .align_child(Center, Center)
        .on_scroll(|scroll| Wallpapers::write().scroll(if scroll.y.abs() >= scroll.x.abs() { scroll.y } else { scroll.x }));

    let count = wallpapers.files.len() as isize;

    if count == 0 {
        return area.child(label(format!("No wallpapers found in {}", directory()), 13.0, t.dim));
    }

    // as many as fit, an odd number so the selected one is in the middle
    let fit = ((width + THUMB_GAP) / (THUMB.0 + 2.0 * FRAME + THUMB_GAP)).floor() as isize;
    let side = ((fit - 1) / 2).max(0);

    let selected = wallpapers.selected as isize;
    let focused = wallpapers.focus == Focus::Wallpapers;

    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    for offset in -side..=side {
        // wraps around, so the strip never runs out
        let index = (selected + offset).rem_euclid(count) as usize;

        // with few images, don't show the same one twice
        if count < side * 2 + 1 && (selected + offset < 0 || selected + offset >= count) {
            continue;
        }

        let path = wallpapers.files[index].clone();

        let key = format!("wp{index}");
        let hot = hover(&key);

        // a plain tile until the thumbnail has been made
        let tile = Rectangle::new().width(THUMB.0).height(THUMB.1);

        let tile = match wallpapers.ready.get(&path) {
            Some(thumbnail) => tile.fill(amane::Image::cover(thumbnail)),
            None => tile.fill(t.surface_hi),
        };

        // the others are dimmed by a veil of the panel's colour rather than with `Rectangle::opacity`, which Amane
        // draws on a window-sized texture of its own
        let veil = if offset == 0 { 0.0 } else { 0.45 * (1.0 - hot) };

        let marker = Rectangle::new().width(Parent).height(IN_USE).fill(if path == *current { t.accent } else { Color::TRANSPARENT });

        let tile = tile
            .align_child(Center, amane::End)
            .child(Rectangle::new().width(Parent).height(Parent).fill(alpha(t.panel, veil)).align_child(Center, amane::End).child(marker));

        let target = path.clone();

        items.push(Box::new(
            framed(t, offset == 0, focused, tile, THUMB).on_hover(move |inside| set_hover(&key, inside)).on_click(move |_| {
                {
                    let mut wallpapers = Wallpapers::write();

                    wallpapers.selected = index;
                    wallpapers.focus = Focus::Wallpapers;
                }

                pick(target.clone());
            }),
        ));
    }

    area.child(Row::new(items).gap(THUMB_GAP).align(Center))
}

// one scheme, drawn in its own colours: its name over its accent and signal colours
fn swatch(t: &Theme, index: usize, name: &str, colors: &Theme, in_use: bool, selected: bool, focused: bool) -> Rectangle {
    let key = format!("sc{index}");
    let hot = hover(&key);

    let amount = shown(t);

    let background = Color::rgb(colors.panel.red(), colors.panel.green(), colors.panel.blue());

    let dots: Vec<Box<dyn Widget>> = [colors.accent, colors.red, colors.yellow, colors.green, colors.blue]
        .into_iter()
        .map(|color| Box::new(Rectangle::new().width(10.0).height(10.0).fill(faded(color, amount))) as Box<dyn Widget>)
        .collect();

    let content = Rectangle::new()
        .width(SWATCH.0)
        .height(SWATCH.1 - IN_USE)
        .padding(10.0)
        .align_child(Start, Center)
        .child(Column::new(children![label(name, 12.0, faded(colors.fg, amount)), Row::new(dots).gap(4.0)]).gap(8.0));

    let marker = Rectangle::new().width(SWATCH.0).height(IN_USE).fill(if in_use { faded(colors.accent, amount) } else { Color::TRANSPARENT });

    let chip = Rectangle::new()
        .width(SWATCH.0)
        .height(SWATCH.1)
        .fill(faded(background, amount))
        .border(1.0, faded(alpha(colors.fg, 0.10 + 0.25 * hot), amount))
        .child(Column::new(children![content, marker]));

    framed(t, selected, focused, chip, SWATCH).on_hover(move |inside| set_hover(&key, inside)).on_click(move |_| {
        {
            let mut wallpapers = Wallpapers::write();

            wallpapers.scheme = index;
            wallpapers.focus = Focus::Schemes;
        }

        pick_scheme(index);
    })
}

fn swatches(t: &Theme, wallpapers: &Wallpapers, width: f32) -> Rectangle {
    let total = SCHEMES.len() + 1;

    // as many as fit, sliding along so the selected one is always among them
    let fit = (((width + SWATCH_GAP) / (SWATCH.0 + 2.0 * FRAME + SWATCH_GAP)).floor() as usize).clamp(1, total);
    let first = wallpapers.scheme.saturating_sub(fit / 2).min(total - fit);

    let in_use = schemes::chosen().map_or(0, |chosen| SCHEMES.iter().position(|scheme| scheme.name == chosen.name).map_or(0, |index| index + 1));
    let focused = wallpapers.focus == Focus::Schemes;

    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    for index in first..first + fit {
        let (name, colors) = match index.checked_sub(1).and_then(|index| SCHEMES.get(index)) {
            Some(scheme) => (scheme.name, scheme.theme()),
            None => ("Wallpaper", theme::from_wallpaper()),
        };

        items.push(Box::new(swatch(t, index, name, &colors, index == in_use, index == wallpapers.scheme, focused)));
    }

    Rectangle::new()
        .width(width)
        .height(SWATCH.1 + 2.0 * FRAME)
        .align_child(Center, Center)
        .on_scroll(|scroll| Wallpapers::write().scroll_schemes(if scroll.y.abs() >= scroll.x.abs() { scroll.y } else { scroll.x }))
        .child(Row::new(items).gap(SWATCH_GAP).align(Center))
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let wallpapers = Wallpapers::read();

    // reading the service above is what starts its thumbnail work at launch, so it stays. but a closed picker shouldn't
    // build its tiles (and look at their files) on every change
    let Some(t) = panels::look(Which::Wallpapers, monitor) else {
        return panels::hidden("amane-dock");
    };

    let current = Wallpaper::read().path.clone();

    let width = monitor.width as f32 - 2.0 * panels::GAP as f32 - 2.0 * PAD;

    let window = LayerWindow::new()
        .width(Full)
        .height(HEIGHT)
        .anchor_vertical(Vertical::Bottom)
        .margin(Margin { top: 0, right: panels::GAP, bottom: panels::GAP, left: panels::GAP })
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("amane-dock")
        .keyboard(Keyboard::Exclusive)
        .on_key(on_key);

    let count = wallpapers.files.len();

    let name = wallpapers
        .files
        .get(wallpapers.selected)
        .and_then(|path| path.rsplit('/').next())
        .map(|file| file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string())
        .unwrap_or_default();

    let scheme_name = wallpapers.scheme.checked_sub(1).and_then(|index| SCHEMES.get(index)).map_or("Wallpaper", |scheme| scheme.name);

    let top = Row::new(children![
        header(&t, "Wallpaper", wallpapers.focus == Focus::Wallpapers, name, (count > 0).then(|| format!("{} / {count}", wallpapers.selected + 1))),
        label("\u{2190} \u{2192} choose     \u{2191} \u{2193} wallpaper or colours     enter apply     esc close", 11.5, t.faint),
    ])
    .justify(SpaceBetween)
    .align(Center)
    .width(width);

    let content = Column::new(children![
        top,
        Rectangle::new().width(width).height(12.0),
        strip(&t, &wallpapers, &current, width),
        Rectangle::new().width(width).height(18.0),
        Rectangle::new().width(width).height(1.0).fill(t.border),
        Rectangle::new().width(width).height(18.0),
        header(&t, "Colours", wallpapers.focus == Focus::Schemes, scheme_name.to_string(), None),
        Rectangle::new().width(width).height(12.0),
        swatches(&t, &wallpapers, width),
    ]);

    window.child(panels::body(&t).padding(PAD).align_child(Start, Start).child(content))
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
