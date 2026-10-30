use std::time::Duration;

use amane::{
    Animation, Color, Full, Horizontal, Layer, LayerWindow, Margin, Monitor, Parent, Rectangle, Service, Vertical, Zone,
};

use crate::theme::{Theme, theme};

// how long a panel takes to fade in. closing is instant: niri's blur behind a window that fades out smears
const FADE: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Dashboard,
    Calendar,
    Notifications,
    TrayMenu,
    Wallpapers,
    Launcher,
    Power,
    Network,
    Audio,
    Bluetooth,
    Clipboard,
}

pub const COUNT: usize = 11;

pub struct Panels {
    pub open: [bool; COUNT], // indexed by `Which`
    pub monitor: String,     // the monitor they open on, empty for all of them
    pub armed: Option<&'static str>, // power action waiting for a second click
    generation: u64,                 // counts every change, so a late answer can tell it's out of date
    fades: [Animation; COUNT],       // how far each open panel has faded in, 0 to 1
}

impl Service for Panels {
    fn new() -> Self {
        Self {
            open: [false; COUNT],
            monitor: String::new(),
            armed: None,
            generation: 0,
            fades: std::array::from_fn(|_| Animation::new(0.0).duration(FADE)),
        }
    }

    fn interval() -> Duration {
        Duration::from_secs(3600)
    }
}

pub fn is_open(which: Which) -> bool {
    Panels::read().open[which as usize]
}

/*
 * the threads that only work while a panel is open (the system numbers, the network details, the audio devices,
 * Bluetooth, the clipboard) wait here while it's closed, so a closed panel costs no wakeups at all. `changed` wakes
 * them whenever a panel opens or closes, and each looks again
 */
static OPENING: std::sync::Condvar = std::sync::Condvar::new();
static OPENING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// waits until one of these panels is open, and returns at once if one already is
pub fn wait_until_open(which: &[Which]) {
    let mut waiting = crate::sync::lock(&OPENING_LOCK);

    // the lock is held from the look to the wait, and `changed` takes it to wake us, so an opening can't slip between
    while !which.iter().any(|which| is_open(*which)) {
        waiting = OPENING.wait(waiting).unwrap_or_else(std::sync::PoisonError::into_inner);
    }
}

/// is this panel open, and on this monitor
pub fn shown(which: Which, monitor: &Monitor) -> bool {
    let panels = Panels::read();

    panels.open[which as usize] && (panels.monitor.is_empty() || panels.monitor == monitor.name)
}

/// the colours to draw an open panel with, faded while it's just opening. None when it isn't open on this monitor
pub fn look(which: Which, monitor: &Monitor) -> Option<Theme> {
    let fade = {
        let panels = Panels::read();

        let here = panels.monitor.is_empty() || panels.monitor == monitor.name;

        if !(panels.open[which as usize] && here) {
            return None;
        }

        panels.fades[which as usize].value()
    };

    Some(theme().faded(fade))
}

// the first monitor a bar was built for, the answer when the compositor can't say which one has focus
static PRIMARY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn note_monitor(monitor: &Monitor) {
    let _ = PRIMARY.set(monitor.name.clone());
}

/*
 * the monitor with keyboard focus, which is where a keybind should open things. read from the workspaces niri
 * reports (the focused workspace is on the focused monitor, and every switch is an event), so there's nothing to
 * wait for. this runs in ipc handlers, which run on the thread that draws every window
 */
pub fn focused_output() -> String {
    let workspaces = amane::Workspaces::read();

    workspaces
        .list()
        .iter()
        .find(|workspace| workspace.focused())
        .and_then(|workspace| workspace.output())
        .map(str::to_string)
        .or_else(|| PRIMARY.get().cloned())
        .unwrap_or_default()
}

/*
 * every change to the open panels goes through here: put them on the monitor (panels left open on another one
 * are closed), let `edit` decide which are open, forget a half-confirmed power button, and run the hooks for
 * whatever just opened or closed.
 *
 * only one panel is open at a time, like the dropdowns of other bars: opening one closes the one that was open.
 * (they used to be combinable when they didn't overlap, but a bar where some dropdowns replace each other and
 * others pile up is harder to predict than one rule.) `toggle all` still opens the dashboard, calendar,
 * notifications and wallpaper picker together
 */
fn update(monitor: Option<&str>, edit: impl FnOnce(&mut [bool; COUNT])) {
    update_since(None, monitor, edit);
}

// the same, but only when nothing changed after `since` (a `generation()`). false when something did
fn update_since(since: Option<u64>, monitor: Option<&str>, edit: impl FnOnce(&mut [bool; COUNT])) -> bool {
    let mut panels = Panels::write();

    if since.is_some_and(|since| since != panels.generation) {
        return false;
    }

    panels.generation += 1;

    let before = panels.open;

    if let Some(monitor) = monitor
        && panels.monitor != monitor
    {
        panels.open = [false; COUNT];
        panels.monitor = monitor.to_string();
    }

    edit(&mut panels.open);

    panels.armed = None;

    let after = panels.open;

    for index in 0..COUNT {
        if before[index] == after[index] {
            continue;
        }

        // a panel that opens fades in from nothing, one that closes is gone at once
        panels.fades[index] = Animation::new(0.0).duration(FADE);

        if after[index] {
            panels.fades[index].to(1.0);
        }
    }

    drop(panels);

    changed(&before, &after);

    true
}

/// a panel that has just been opened starts fresh
fn opened(which: Which) {
    match which {
        Which::Launcher => crate::launcher::reset(),
        Which::Clipboard => crate::clipboard::reset(),
        // the calendar opens on this month, whichever one the arrows were left on
        Which::Calendar => crate::calendar::reset(),
        // the picker starts on the wallpaper that's showing
        Which::Wallpapers => crate::wallpapers::Wallpapers::write().select_current(),
        _ => {}
    }
}

/// opens or closes a panel on the given monitor. a panel opened on another monitor moves the others there too
pub fn toggle(which: Which, monitor: &str) {
    let mut opening = false;

    update(Some(monitor), |open| {
        opening = !open[which as usize];

        if opening {
            *open = [false; COUNT];
        }

        open[which as usize] = opening;
    });

    if opening {
        opened(which);
    }
}

/// opens a panel on the given monitor, closing whatever else was open. unlike `toggle` it never closes it
pub fn show(which: Which, monitor: &str) {
    update(Some(monitor), |open| {
        *open = [false; COUNT];
        open[which as usize] = true;
    });

    opened(which);
}

/// what the panels look like now, for `show_since`
pub fn generation() -> u64 {
    Panels::read().generation
}

/// `show`, for an answer that took a while: only when no panel was opened or closed after `since`, so a panel the
/// user dismissed (or replaced) in the meantime doesn't pop up late. returns whether it opened
pub fn show_since(since: u64, which: Which, monitor: &str) -> bool {
    let opened_now = update_since(Some(since), Some(monitor), |open| {
        *open = [false; COUNT];
        open[which as usize] = true;
    });

    if opened_now {
        opened(which);
    }

    opened_now
}

/// for keybinds and ipc, where nothing says which monitor was meant
pub fn toggle_here(which: Which) {
    toggle(which, &focused_output());
}

pub fn show_here(which: Which) {
    show(which, &focused_output());
}

pub fn close() {
    update(None, |open| *open = [false; COUNT]);
}

/// things that run while a panel is open. looking for Bluetooth devices drains the adapter and the battery of
/// whatever is nearby, so it only happens while its panel is on screen
pub fn changed(before: &[bool; COUNT], after: &[bool; COUNT]) {
    {
        let _waking = crate::sync::lock(&OPENING_LOCK);

        OPENING.notify_all();
    }

    let bluetooth = Which::Bluetooth as usize;

    if before[bluetooth] != after[bluetooth] {
        crate::btstate::scan_with_panel();
    }

    // the dashboard shows the adapter's state and the system numbers
    let dashboard = Which::Dashboard as usize;

    if !before[dashboard] && after[dashboard] {
        crate::btstate::refresh();
        crate::sys::refresh();
    }

    let network = Which::Network as usize;

    if !before[network] && after[network] {
        crate::sys::refresh();
        crate::netinfo::refresh();
        crate::wifi::opened();
    }
}

pub fn ipc(arguments: &[String]) -> String {
    let which = match arguments.first().map(String::as_str) {
        Some("dashboard") => Which::Dashboard,
        Some("calendar") => Which::Calendar,
        Some("notifications") => Which::Notifications,
        Some("wallpapers") => Which::Wallpapers,
        Some("launcher") => Which::Launcher,
        Some("power") => Which::Power,
        Some("network") => Which::Network,
        Some("audio") => Which::Audio,
        Some("bluetooth") => Which::Bluetooth,
        Some("clipboard") => Which::Clipboard,
        Some("all") => {
            update(Some(&focused_output()), |open| {
                *open = [false; COUNT];

                for which in [Which::Dashboard, Which::Calendar, Which::Notifications, Which::Wallpapers] {
                    open[which as usize] = true;
                }
            });

            opened(Which::Wallpapers);

            return String::from("ok");
        }
        Some("close") => {
            close();

            return String::from("closed");
        }
        _ => return String::from("usage: toggle dashboard|calendar|notifications|wallpapers|launcher|power|network|audio|bluetooth|clipboard|all|close"),
    };

    toggle_here(which);

    String::from("ok")
}

pub const GAP: i32 = 8;
pub const RADIUS: f32 = 0.0;

/// the empty window every popup panel is built on, anchored under the bar
pub fn window(width: f32, height: f32, side: Horizontal) -> LayerWindow {
    window_at(width, height, side, GAP)
}

/// the same, with its own gap to the screen edge (dropdowns line up under their bar chip)
pub fn window_at(width: f32, height: f32, side: Horizontal, edge: i32) -> LayerWindow {
    let (left, right) = match side {
        Horizontal::Left => (edge, 0),
        Horizontal::Right => (0, edge),
        _ => (0, 0),
    };

    LayerWindow::new()
        .width(width)
        .height(height)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(side)
        .margin(Margin { top: GAP, right, bottom: 0, left })
        .layer(Layer::Overlay)
        .space(Zone::Respect)
        .namespace("amane-panel")
}

/// the bordered body of a panel
pub fn body(t: &Theme) -> Rectangle {
    Rectangle::new()
        .width(Parent)
        .height(Parent)
        .radius(RADIUS)
        .fill(t.panel)
        .border(1.0, t.border)
}

/// a transparent window under the panels that closes them when you click outside
pub fn catcher(monitor: &Monitor) -> LayerWindow {
    let panels = Panels::read();

    let visible = panels.open.iter().any(|open| *open) && (panels.monitor.is_empty() || panels.monitor == monitor.name);

    drop(panels);

    LayerWindow::new()
        .width(Full)
        .height(Full)
        .layer(Layer::Top)
        .space(Zone::Respect)
        .namespace("amane-catcher")
        .visible(visible)
        // the compositor shows one stretched transparent pixel, so a fullscreen catcher costs no gpu memory
        .solid(Color::TRANSPARENT)
        .child(Rectangle::new().width(Parent).height(Parent).on_click(|_| close()))
}

/// what a panel's window is while it's closed. building the real panel would read the services behind it,
/// which keeps them polling (network commands, system stats) for a panel nobody has open.
/// the namespace is set here too, because it's read once, when the window opens
pub fn hidden(namespace: &'static str) -> LayerWindow {
    LayerWindow::new()
        .width(1.0)
        .height(1.0)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace(namespace)
        .visible(false)
        .child(Rectangle::new().width(Parent).height(Parent))
}
