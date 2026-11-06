use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use amane::{Button, Center, Full, Image, Key, Keyboard, Layer, LayerWindow, Parent, Rectangle, Service, Stack, Widget, Zone};

use crate::compositor::{self, Shot};
use crate::panels;
use crate::theme::{alpha, theme};
use crate::ui::label;

/*
 * a region screenshot chosen over a frozen copy of the screen, as niri's own screenshot tool does, for the compositors
 * that have none (Umbriel). each monitor is grabbed with grim first and shown in an overlay window drawn by the
 * compositor from shared memory, like the wallpaper, so it costs no gpu memory. a second window on top draws the
 * selection, so moving it never redraws the picture. the screenshot is cropped out of the frozen picture, which is
 * exactly what was on screen when it was asked for, and has none of the selection's own drawing in it
 */
#[derive(Default)]
pub struct Freeze {
    // the frozen picture of each monitor, by name; empty while nothing is frozen
    pictures: HashMap<String, PathBuf>,

    // shown, but not yet known to be on screen, see FRAMES
    waiting: bool,

    // the selection being made, once the pictures are on screen
    selecting: Option<Selecting>,
}

impl Service for Freeze {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

// a part of a monitor, in the same units as its window
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Area {
    fn between(a: (f32, f32), b: (f32, f32)) -> Self {
        Self { x: a.0.min(b.0), y: a.1.min(b.1), width: (a.0 - b.0).abs(), height: (a.1 - b.1).abs() }
    }

    fn contains(self, point: (f32, f32)) -> bool {
        point.0 >= self.x && point.0 <= self.x + self.width && point.1 >= self.y && point.1 <= self.y + self.height
    }

    // moved back onto a monitor of this size, and cut down to it if it's too big
    fn kept_in(self, size: (f32, f32)) -> Self {
        let (width, height) = (self.width.min(size.0), self.height.min(size.1));

        Self { x: self.x.clamp(0.0, size.0 - width), y: self.y.clamp(0.0, size.1 - height), width, height }
    }

    // never down to nothing: a selection that small couldn't be seen or grabbed again
    fn resized(self, width: f32, height: f32, size: (f32, f32)) -> Self {
        Self { width: (self.width + width).max(SMALLEST), height: (self.height + height).max(SMALLEST), ..self }.kept_in(size)
    }
}

// what a press without a drag selects, around where it was
const SMALLEST: f32 = 32.0;

// how far a key moves or resizes the selection
const STEP: f32 = 10.0;

#[derive(Clone, Copy)]
enum Drag {
    // a new selection, from where the press was
    New { from: (f32, f32) },

    // the selection moving, held this far from its corner
    Move { held: (f32, f32) },
}

struct Selecting {
    monitor: String,
    size: (f32, f32),

    // nothing until the first drag: a box already there would only be in the way of drawing a new one
    area: Option<Area>,

    // the one window that takes the keys: several asking for them would fight over them
    keys: String,

    drag: Option<Drag>,

    // the drag was told about the last pointer move, see `pointer_moved`
    told: bool,

    answer: Sender<Answer>,
}

impl Selecting {
    fn drag_to(&mut self, monitor: &str, size: (f32, f32), point: (f32, f32)) {
        let point = (point.0.clamp(0.0, size.0), point.1.clamp(0.0, size.1));

        self.told = true;

        match (self.drag, self.area) {
            // the first call of a drag is the press. inside the selection it picks it up, anywhere else starts anew
            (None, Some(area)) if self.monitor == monitor && area.contains(point) => {
                self.drag = Some(Drag::Move { held: (point.0 - area.x, point.1 - area.y) });
            }
            (Some(Drag::New { from }), _) => self.area = Some(Area::between(from, point)),
            (Some(Drag::Move { held }), Some(area)) => {
                self.area = Some(Area { x: point.0 - held.0, y: point.1 - held.1, ..area }.kept_in(self.size));
            }
            _ => {
                self.monitor = monitor.to_string();
                self.size = size;
                self.area = Some(Area::between(point, point));
                self.drag = Some(Drag::New { from: point });
            }
        }
    }

    fn release(&mut self) {
        let dragged = self.drag.take().is_some();

        if dragged
            && let Some(area) = self.area
            && (area.width < 1.0 || area.height < 1.0)
        {
            let half = SMALLEST / 2.0;

            self.area = Some(Area { x: area.x - half, y: area.y - half, width: SMALLEST, height: SMALLEST }.kept_in(self.size));
        }
    }

    /*
     * amane tells a drag that it moved, then the window that the pointer moved, but it doesn't tell a drag that it
     * ended, and the click that says so only comes when the button goes up over the window it went down in. dragged
     * past the edge onto another monitor it never comes, and the next press would carry on the old drag. so a pointer
     * move that the drag wasn't told about means the button is already up
     */
    fn pointer_moved(&mut self) {
        if !std::mem::take(&mut self.told) {
            self.release();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Save,
    Copy,
}

struct Answer {
    action: Action,
    monitor: String,
    area: Area,
}

// every monitor a freeze window was made for, which is every monitor the shell has seen, and its size. amane has no
// list of them
static MONITORS: Mutex<BTreeMap<String, (f32, f32)>> = Mutex::new(BTreeMap::new());

/*
 * the frames each freeze window drew since it was shown, as the wallpaper's fade counts them: once a monitor has drawn
 * a second frame, the first, with the picture attached, is on screen. the selection mustn't open before that, or the
 * live screen would show for a moment under its first drag
 */
static FRAMES: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

// one screenshot at a time: a second keypress while choosing is ignored rather than freezing over the freeze
static TAKING: Mutex<()> = Mutex::new(());

// each freeze gets fresh file names: amane keeps decoded pictures by path, and an old one would show again
static COUNT: AtomicU32 = AtomicU32::new(0);

// how long the grabs may take to decode, and the windows to show them, before the screenshot gives up or goes ahead
const DECODE_WITHIN: Duration = Duration::from_secs(1);
const SHOW_WITHIN: Duration = Duration::from_millis(300);

pub fn view(monitor: &amane::Monitor) -> LayerWindow {
    crate::sync::lock(&MONITORS).insert(monitor.name.clone(), (monitor.width as f32, monitor.height as f32));

    let (picture, waiting) = {
        let freeze = Freeze::read();

        (freeze.pictures.get(&monitor.name).cloned(), freeze.waiting)
    };

    if picture.is_some() && waiting {
        if let Some(frames) = crate::sync::lock(&FRAMES).as_mut() {
            *frames.entry(monitor.name.clone()).or_insert(0) += 1;
        }

        amane::request_frame();
    }

    let window = LayerWindow::new()
        .width(Full)
        .height(Full)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("daru-freeze")
        .click_through()
        .visible(picture.is_some());

    // the child only shows on a compositor without viewports, where the picture can't be shown, and then shows nothing
    let window = match &picture {
        Some(path) => window.picture(path),
        None => window,
    };

    window.child(Rectangle::new().width(Parent).height(Parent))
}

const HINTS: (&str, f32) = ("enter save     c copy     arrows or h j k l move     shift + h j k l resize     esc cancel", 640.0);
const HINTS_EMPTY: (&str, f32) = ("drag to select     esc cancel", 240.0);
const HINTS_HEIGHT: f32 = 34.0;
const HINTS_MARGIN: f32 = 24.0;
const BORDER: f32 = 2.0;

// a rectangle somewhere on the window, as everything drawn over the frozen picture is
fn placed(x: f32, y: f32, width: f32, height: f32) -> Rectangle {
    Rectangle::new().width(width).height(height).translate(x, y)
}

/// what's selected, drawn over the frozen pictures: everything else dimmed, and how to go on
pub fn selection(monitor: &amane::Monitor) -> LayerWindow {
    let freeze = Freeze::read();

    let shown = freeze.selecting.as_ref().filter(|_| freeze.pictures.contains_key(&monitor.name));

    let Some(selecting) = shown else {
        return panels::hidden("daru-selection");
    };

    let t = theme();
    let dim = alpha(amane::Color::rgb(0, 0, 0), 0.5);

    let size = (monitor.width as f32, monitor.height as f32);

    let mut layers: Vec<Box<dyn Widget>> = Vec::new();

    let here = selecting.monitor == monitor.name;

    match selecting.area.filter(|_| here) {
        Some(area) => {
            let (right, bottom) = (area.x + area.width, area.y + area.height);

            layers.push(Box::new(placed(0.0, 0.0, size.0, area.y).fill(dim)));
            layers.push(Box::new(placed(0.0, bottom, size.0, size.1 - bottom).fill(dim)));
            layers.push(Box::new(placed(0.0, area.y, area.x, area.height).fill(dim)));
            layers.push(Box::new(placed(right, area.y, size.0 - right, area.height).fill(dim)));

            layers.push(Box::new(
                placed(area.x - BORDER, area.y - BORDER, area.width + 2.0 * BORDER, area.height + 2.0 * BORDER)
                    .border(BORDER, t.accent)
                    .cursor(amane::Move),
            ));
        }
        None => layers.push(Box::new(placed(0.0, 0.0, size.0, size.1).fill(dim))),
    }

    // out of the way while the selection is being dragged about
    if here && selecting.drag.is_none() {
        let (hints, width) = if selecting.area.is_some() { HINTS } else { HINTS_EMPTY };

        layers.push(Box::new(
            placed((size.0 - width) / 2.0, size.1 - HINTS_HEIGHT - HINTS_MARGIN, width, HINTS_HEIGHT)
                .fill(t.panel)
                .border(1.0, t.border)
                .align_child(Center, Center)
                .child(label(hints, 12.0, t.dim)),
        ));
    }

    let keyboard = if selecting.keys == monitor.name { Keyboard::Exclusive } else { Keyboard::None };

    let name = monitor.name.clone();

    let surface = Rectangle::new()
        .width(Parent)
        .height(Parent)
        .cursor(amane::Crosshair)
        .on_drag(move |point| {
            if let Some(selecting) = Freeze::write().selecting.as_mut() {
                selecting.drag_to(&name, size, (point.x, point.y));
            }
        })
        .on_move(|_| {
            // a pointer that's only passing over the selection shouldn't redraw it
            let dragging = Freeze::read().selecting.as_ref().is_some_and(|selecting| selecting.drag.is_some());

            if dragging && let Some(selecting) = Freeze::write().selecting.as_mut() {
                selecting.pointer_moved();
            }
        })
        .on_click(|button| match button {
            Button::Left => {
                if let Some(selecting) = Freeze::write().selecting.as_mut() {
                    selecting.release();
                }
            }
            // no keyboard is needed to get out again
            Button::Right => cancel(),
            _ => {}
        })
        .child(Stack::new(layers));

    LayerWindow::new()
        .width(Full)
        .height(Full)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("daru-selection")
        .keyboard(keyboard)
        .on_key(on_key)
        .child(surface)
}

fn on_key(key: Key) {
    match key {
        Key::Escape => cancel(),
        Key::Enter | Key::Space => finish(Action::Save),
        Key::Character('c') => finish(Action::Copy),
        Key::Left | Key::Character('h') => nudge(-STEP, 0.0),
        Key::Right | Key::Character('l') => nudge(STEP, 0.0),
        Key::Up | Key::Character('k') => nudge(0.0, -STEP),
        Key::Down | Key::Character('j') => nudge(0.0, STEP),
        Key::Character('H') => resize(-STEP, 0.0),
        Key::Character('L') => resize(STEP, 0.0),
        Key::Character('K') => resize(0.0, -STEP),
        Key::Character('J') => resize(0.0, STEP),
        _ => {}
    }
}

fn nudge(x: f32, y: f32) {
    if let Some(selecting) = Freeze::write().selecting.as_mut() {
        let size = selecting.size;

        selecting.area = selecting.area.map(|area| Area { x: area.x + x, y: area.y + y, ..area }.kept_in(size));
    }
}

fn resize(width: f32, height: f32) {
    if let Some(selecting) = Freeze::write().selecting.as_mut() {
        let size = selecting.size;

        selecting.area = selecting.area.map(|area| area.resized(width, height, size));
    }
}

// closes the selection at once, and leaves the rest to the thread that's waiting for it. with nothing selected yet
// there's nothing to save, and it stays open
fn finish(action: Action) {
    let mut freeze = Freeze::write();

    let Some(area) = freeze.selecting.as_ref().and_then(|selecting| selecting.area) else { return };
    let Some(selecting) = freeze.selecting.take() else { return };

    drop(freeze);

    let _ = selecting.answer.send(Answer { action, monitor: selecting.monitor, area });
}

// the thread waiting for an answer sees that none will come
fn cancel() {
    Freeze::write().selecting = None;
}

// the monitor that starts out with the keys and the hints: the one in focus, of those that were frozen
fn first_monitor(focused: Option<String>, frozen: &BTreeMap<String, (f32, f32)>) -> Option<(String, (f32, f32))> {
    let (monitor, size) = focused.and_then(|name| frozen.get_key_value(&name)).or_else(|| frozen.iter().next())?;

    Some((monitor.clone(), *size))
}

// opens the selection on what was frozen, and says whether there was anything to open it on
fn open(answer: Sender<Answer>) -> bool {
    let sizes = crate::sync::lock(&MONITORS).clone();

    let frozen: BTreeMap<String, (f32, f32)> = {
        let freeze = Freeze::read();

        sizes.into_iter().filter(|(name, _)| freeze.pictures.contains_key(name)).collect()
    };

    let Some((monitor, size)) = first_monitor(compositor::focused_output(), &frozen) else {
        return false;
    };

    Freeze::write().selecting = Some(Selecting { keys: monitor.clone(), monitor, size, area: None, drag: None, told: false, answer });

    true
}

/// a region screenshot over a frozen screen, or one to draw on in satty. it runs on a thread of its own
pub fn region(shot: Shot) {
    std::thread::spawn(move || {
        let Ok(_taking) = TAKING.try_lock() else { return };

        let frozen = freeze();

        let (answer, answered) = mpsc::channel();

        if !open(answer) {
            thaw(&frozen);
            compositor::announce("", "Screenshot failed", "The screen couldn't be captured");

            return;
        }

        // no answer means it was cancelled
        let answer = answered.recv().ok();

        let picture = answer.as_ref().and_then(|answer| Freeze::read().pictures.get(&answer.monitor).cloned());

        // the screen comes back before the picture is worked on, and the file stays until that's done
        hide();

        let result = match (answer, picture) {
            (Some(answer), Some(picture)) => Some(deliver(shot, &answer, &picture)),
            _ => None,
        };

        forget(&frozen);

        if result == Some(false) {
            compositor::announce("", "Screenshot failed", "The picture couldn't be saved");
        }
    });
}

// saved, copied and announced as every other screenshot is (see compositor.rs). says if it went well
fn deliver(shot: Shot, answer: &Answer, picture: &Path) -> bool {
    let Some(size) = crate::sync::lock(&MONITORS).get(&answer.monitor).copied() else { return false };

    let Some(pixels) = png_size(picture) else { return false };

    let geometry = crop_geometry(crop(answer.area, pixels, size));

    // not /tmp: see cache::runtime_dir
    let temporary = || crate::cache::runtime_dir().map(|folder| folder.join(format!("daru-screenshot-{}.png", COUNT.fetch_add(1, Ordering::Relaxed))));

    match (shot, answer.action) {
        // satty saves, copies and notifies by itself, and the capture is left for it to read
        (Shot::Annotate, Action::Save) => {
            let Some(file) = temporary().filter(|file| cut(picture, &geometry, file)) else { return false };

            amane::spawn(&format!("satty --filename {}", crate::wallpapers::quote(&file.display().to_string())));

            true
        }
        (_, Action::Copy) => {
            let Some(file) = temporary().filter(|file| cut(picture, &geometry, file)) else { return false };

            let copied = compositor::copy(&file);

            let _ = std::fs::remove_file(&file);

            if copied {
                compositor::announce("", "Screenshot copied", "Copied to the clipboard");
            }

            copied
        }
        _ => compositor::new_file().is_some_and(|file| cut(picture, &geometry, &file) && compositor::finish(&file)),
    }
}

fn cut(picture: &Path, geometry: &str, file: &Path) -> bool {
    Command::new("magick")
        .arg(picture)
        .args(["-crop", geometry, "+repage"])
        .arg(file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

// the width and height a png says it has, which come first in the file
fn png_size(path: &Path) -> Option<(u32, u32)> {
    let mut head = [0; 24];

    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;

    let number = |at: usize| u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);

    head.starts_with(b"\x89PNG\r\n\x1a\n").then(|| (number(16), number(20)))
}

/*
 * the part of the picture a selection covers, as x, y, width and height. the selection is in the window's units and
 * the picture in the monitor's real pixels: on a screen scaled by 1.5 a selection 100 wide is 150 pixels. both edges
 * are rounded rather than the size, so a selection reaching the side of the monitor reaches the side of the picture
 */
fn crop(area: Area, picture: (u32, u32), monitor: (f32, f32)) -> (u32, u32, u32, u32) {
    let (across, down) = (picture.0 as f32 / monitor.0, picture.1 as f32 / monitor.1);

    let edge = |place: f32, scale: f32, limit: u32| ((place * scale).round() as u32).min(limit);

    let (left, top) = (edge(area.x, across, picture.0), edge(area.y, down, picture.1));
    let (right, bottom) = (edge(area.x + area.width, across, picture.0), edge(area.y + area.height, down, picture.1));

    (left, top, right.saturating_sub(left).max(1), bottom.saturating_sub(top).max(1))
}

fn crop_geometry((x, y, width, height): (u32, u32, u32, u32)) -> String {
    format!("{width}x{height}+{x}+{y}")
}

/*
 * grabs every monitor and shows the grabs, returning once they're on screen. a monitor that can't be grabbed (it
 * was unplugged) is left live, and when nothing decodes in time there's nothing to choose from
 */
fn freeze() -> Vec<PathBuf> {
    let Some(directory) = crate::cache::runtime_dir() else { return Vec::new() };

    let monitors: Vec<String> = crate::sync::lock(&MONITORS).keys().cloned().collect();
    let count = COUNT.fetch_add(1, Ordering::Relaxed);

    // all at once: on two monitors the second grab mustn't be a frame later than the first
    let grabs: Vec<_> = monitors
        .into_iter()
        .enumerate()
        .filter_map(|(index, name)| {
            // numbered, not named after the monitor: the name comes from the compositor and stays out of paths
            let path = directory.join(format!("daru-freeze-{count}-{index}.png"));

            // no compression: the grab is read back at once and thrown away, and decoding is what holds things up
            let grim = Command::new("grim").args(["-l", "0", "-o", &name]).arg(&path).stderr(Stdio::null()).spawn().ok()?;

            Some((name, path, grim))
        })
        .collect();

    let grabbed: Vec<(String, PathBuf)> = grabs
        .into_iter()
        .filter_map(|(name, path, mut grim)| grim.wait().is_ok_and(|status| status.success()).then_some((name, path)))
        .collect();

    let paths: Vec<PathBuf> = grabbed.iter().map(|(_, path)| path.clone()).collect();

    // decoded before the windows show, so they never stand there empty
    let started = Instant::now();

    while !paths.iter().all(Image::loaded) {
        if started.elapsed() > DECODE_WITHIN {
            thaw(&paths);

            return Vec::new();
        }

        std::thread::sleep(Duration::from_millis(4));
    }

    *crate::sync::lock(&FRAMES) = Some(HashMap::new());

    {
        let mut freeze = Freeze::write();

        freeze.pictures = grabbed.into_iter().collect();
        freeze.waiting = true;
    }

    let shown = Instant::now();

    while !all_shown(paths.len()) && shown.elapsed() < SHOW_WITHIN {
        std::thread::sleep(Duration::from_millis(4));
    }

    *crate::sync::lock(&FRAMES) = None;

    Freeze::write().waiting = false;

    paths
}

// every frozen monitor has drawn its second frame
fn all_shown(monitors: usize) -> bool {
    crate::sync::lock(&FRAMES).as_ref().is_some_and(|frames| frames.len() >= monitors && frames.values().all(|count| *count >= 2))
}

// hides the frozen pictures and lets go of them, decoded copies and files both
fn thaw(paths: &[PathBuf]) {
    hide();
    forget(paths);
}

fn hide() {
    let mut freeze = Freeze::write();

    freeze.pictures.clear();
    freeze.waiting = false;
    freeze.selecting = None;
}

fn forget(paths: &[PathBuf]) {
    for path in paths {
        Image::forget(path);

        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::mpsc;

    use super::{Answer, Area, Drag, Selecting, crop, crop_geometry, first_monitor};

    const SCREEN: (f32, f32) = (2000.0, 1333.0);

    fn selecting(area: Option<Area>) -> (Selecting, mpsc::Receiver<Answer>) {
        let (answer, answered) = mpsc::channel();

        (
            Selecting { monitor: String::from("A"), size: SCREEN, area, keys: String::from("A"), drag: None, told: false, answer },
            answered,
        )
    }

    fn area(x: f32, y: f32, width: f32, height: f32) -> Area {
        Area { x, y, width, height }
    }

    #[test]
    fn a_drag_selects_between_its_ends_whichever_way_it_goes() {
        assert_eq!(Area::between((100.0, 200.0), (40.0, 260.0)), area(40.0, 200.0, 60.0, 60.0));
    }

    #[test]
    fn a_selection_stays_on_its_monitor() {
        assert_eq!(area(-20.0, 1300.0, 100.0, 100.0).kept_in(SCREEN), area(0.0, 1233.0, 100.0, 100.0));
        assert_eq!(area(10.0, 10.0, 3000.0, 100.0).kept_in(SCREEN), area(0.0, 10.0, 2000.0, 100.0));
    }

    #[test]
    fn resizing_never_makes_a_selection_vanish() {
        assert_eq!(area(10.0, 10.0, 40.0, 40.0).resized(-100.0, 0.0, SCREEN), area(10.0, 10.0, 32.0, 40.0));
    }

    #[test]
    fn a_press_outside_starts_a_new_selection_and_inside_moves_it() {
        let (mut selecting, _answered) = selecting(Some(area(100.0, 100.0, 200.0, 200.0)));

        selecting.drag_to("A", SCREEN, (150.0, 150.0));

        assert!(matches!(selecting.drag, Some(Drag::Move { .. })));

        selecting.drag_to("A", SCREEN, (160.0, 170.0));

        assert_eq!(selecting.area, Some(area(110.0, 120.0, 200.0, 200.0)));

        selecting.release();
        selecting.drag_to("A", SCREEN, (900.0, 900.0));
        selecting.drag_to("A", SCREEN, (1000.0, 950.0));

        assert_eq!(selecting.area, Some(area(900.0, 900.0, 100.0, 50.0)));
    }

    #[test]
    fn a_press_on_another_monitor_moves_the_selection_there() {
        let (mut selecting, _answered) = selecting(Some(area(100.0, 100.0, 200.0, 200.0)));

        // the same place, but on the other monitor it's outside the selection
        selecting.drag_to("B", (2560.0, 1440.0), (150.0, 150.0));
        selecting.drag_to("B", (2560.0, 1440.0), (2600.0, 300.0));

        assert_eq!(selecting.monitor, "B");
        assert_eq!(selecting.area, Some(area(150.0, 150.0, 2410.0, 150.0)));
    }

    #[test]
    fn a_press_without_a_drag_selects_a_small_box_around_it() {
        let (mut selecting, _answered) = selecting(Some(area(100.0, 100.0, 200.0, 200.0)));

        selecting.drag_to("A", SCREEN, (1000.0, 1000.0));
        selecting.release();

        assert_eq!(selecting.area, Some(area(984.0, 984.0, 32.0, 32.0)));
    }

    #[test]
    fn a_drag_let_go_past_the_edge_ends_at_the_next_pointer_move() {
        let (mut selecting, _answered) = selecting(Some(area(100.0, 100.0, 200.0, 200.0)));

        // while the button is down, every pointer move reaches the drag first
        selecting.drag_to("A", SCREEN, (900.0, 900.0));
        selecting.drag_to("A", SCREEN, (2000.0, 1000.0));
        selecting.pointer_moved();

        assert!(selecting.drag.is_some());

        // and once it's up, none does
        selecting.pointer_moved();

        assert!(selecting.drag.is_none());
    }

    #[test]
    fn the_keys_start_on_the_monitor_in_focus_if_it_was_frozen() {
        let frozen = BTreeMap::from([(String::from("A"), (1000.0, 800.0)), (String::from("B"), (2000.0, 1000.0))]);

        assert_eq!(first_monitor(Some(String::from("B")), &frozen), Some((String::from("B"), (2000.0, 1000.0))));

        // no monitor in focus is known, or it wasn't frozen, so any frozen one will do
        assert_eq!(first_monitor(None, &frozen).map(|(monitor, _)| monitor), Some(String::from("A")));
        assert_eq!(first_monitor(Some(String::from("C")), &frozen).map(|(monitor, _)| monitor), Some(String::from("A")));

        assert_eq!(first_monitor(Some(String::from("B")), &BTreeMap::new()), None);
    }

    #[test]
    fn the_first_press_starts_a_selection_where_there_was_none() {
        let (mut selecting, _answered) = selecting(None);

        selecting.drag_to("A", SCREEN, (100.0, 100.0));
        selecting.drag_to("A", SCREEN, (300.0, 200.0));

        assert_eq!(selecting.area, Some(area(100.0, 100.0, 200.0, 100.0)));
    }

    #[test]
    fn the_crop_is_in_the_pictures_own_pixels() {
        // a 3000x2000 screen at scale 1.5
        let scaled = crop(area(100.0, 50.0, 200.0, 100.0), (3000, 2000), (2000.0, 1333.33));

        assert_eq!(crop_geometry(scaled), "300x150+150+75");

        // at scale 1 nothing changes
        assert_eq!(crop(area(10.0, 20.0, 30.0, 40.0), (2560, 1440), (2560.0, 1440.0)), (10, 20, 30, 40));
    }

    #[test]
    fn a_selection_to_the_edge_reaches_the_edge_of_the_picture() {
        assert_eq!(crop(area(0.0, 0.0, 2000.0, 1333.33), (3000, 2000), (2000.0, 1333.33)), (0, 0, 3000, 2000));
    }

    #[test]
    fn a_crop_is_never_empty() {
        assert_eq!(crop(area(10.0, 10.0, 0.0, 0.0), (100, 100), (100.0, 100.0)), (10, 10, 1, 1));
    }
}
