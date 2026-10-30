use amane::{
    Center, Column, End, Full, Layer, LayerWindow, Monitor, Parent, Rectangle, Row, Service, Start,
    Vertical, Widget, Workspaces, Zone, Audio, Battery, Cpu, Link, Memory, Network, Notifications, children,
    Padding,
};

use crate::clock::Clock;
use crate::focus::Focus;
use crate::panels::{self, Which};
use crate::weather::{Weather, describe};
use crate::theme::{Theme, alpha, theme};
use crate::ui::{bold, chip, icon, label};
use crate::volume::Level;

pub const HEIGHT: f32 = 36.0;

// the sizes the chips on the right are laid out with. `right_edge` adds them up to find where a dropdown goes
const PADDING: f32 = 8.0;
const GAP: f32 = 6.0;
const SQUARE: f32 = 34.0;
const STAT: f32 = 62.0;
const BELL_WITH_COUNT: f32 = 52.0;

pub fn view(monitor: &Monitor) -> LayerWindow {
    panels::note_monitor(monitor);

    let _scope = crate::ui::Scope::enter(&monitor.name);

    let t = theme();
    let name = monitor.name.clone();

    let left = Row::new(children![
        launcher(&t, &name),
        workspaces(&t, monitor),
        title(&t),
    ])
    .gap(10.0)
    .align(Center);

    // a Row places its children at the top, so the clock gets the same vertical centering as the sides
    let middle = Rectangle::new().width(clock_width()).height(Parent).align_child(Center, Center).child(clock(&t, &name));

    let mut chips: Vec<Box<dyn Widget>> = children![
        tray(&t, &name),
        cpu(&t, &name),
        memory(&t, &name),
        audio(&t, &name),
        bluetooth(&t, &name),
        clipboard(&t, &name),
        network(&t, &name),
        bell(&t, &name),
    ];

    if let Some(battery) = battery(&t) {
        chips.push(Box::new(battery));
    }

    chips.push(Box::new(power(&t, &name)));

    let right = Row::new(chips).gap(GAP).align(Center);

    let zones = Row::new(children![
        Rectangle::new()
            .width(Parent)
            .height(Parent)
            .padding(Padding { top: 0.0, right: 0.0, bottom: 0.0, left: 8.0 })
            .align_child(Start, Center)
            .child(left),
        middle,
        Rectangle::new()
            .width(Parent)
            .height(Parent)
            .padding(Padding { top: 0.0, right: PADDING, bottom: 0.0, left: 0.0 })
            .align_child(End, Center)
            .child(right),
    ])
    .width(Parent)
    .height(Parent);

    let line = Rectangle::new().width(Parent).height(1.0).fill(t.border);

    LayerWindow::new()
        .width(Full)
        .height(HEIGHT)
        .anchor_vertical(Vertical::Top)
        .layer(Layer::Top)
        .space(Zone::Reserve)
        .namespace("amane-bar")
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .fill(t.glass)
                .child(Column::new(children![
                    Rectangle::new().width(Parent).height(Parent).child(zones),
                    line,
                ])),
        )
}

// the distro's logo, named by LOGO= in /etc/os-release (CachyOS puts it straight in /usr/share/icons, most others in
// an icon theme). looked up once, the bar is drawn far more often than the distro changes
fn distro_logo() -> Option<&'static str> {
    static LOGO: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

    LOGO.get_or_init(|| {
        let release = std::fs::read_to_string("/etc/os-release").ok()?;

        let name = release.lines().find_map(|line| line.strip_prefix("LOGO="))?.trim_matches('"').to_string();

        ["svg", "png"]
            .iter()
            .map(|extension| format!("/usr/share/icons/{name}.{extension}"))
            .find(|path| std::path::Path::new(path).exists())
            .or_else(|| crate::icons::resolve(&name))
    })
    .as_deref()
}

fn launcher(t: &Theme, name: &str) -> Rectangle {
    let logo = match distro_logo() {
        Some(path) => Rectangle::new().width(18.0).height(18.0).fill(crate::ui::app_icon(path)),
        // a distro without one gets Tux
        None => Rectangle::new().width(18.0).height(18.0).align_child(Center, Center).child(icon("\u{f17c}", 14.0, t.fg)),
    };

    chip("launcher", t, SQUARE, logo).on_click(toggle_on(name, Which::Dashboard))
}

fn workspaces(t: &Theme, monitor: &Monitor) -> Rectangle {
    let workspaces = Workspaces::read();

    let mut dots: Vec<Box<dyn Widget>> = Vec::new();

    for workspace in workspaces.list() {
        if workspace.output() != Some(monitor.name.as_str()) {
            continue;
        }

        let focused = workspace.focused() || workspace.active();
        let occupied = workspace.windows() > 0;

        let (width, fill) = if focused {
            (22.0, t.accent)
        } else if occupied {
            (8.0, t.dim)
        } else {
            (8.0, alpha(t.fg, 0.20))
        };

        let id = workspace.id();

        dots.push(Box::new(
            Rectangle::new()
                .width(width)
                .height(8.0)
                .radius(crate::theme::CHIP_RADIUS)
                .fill(fill)
                .cursor(amane::Pointer)
                .on_click(move |_| Workspaces::focus(id)),
        ));
    }

    let width = 20.0 + dots.len() as f32 * 14.0 + 14.0;

    chip("workspaces", t, width, Row::new(dots).gap(6.0).align(Center))
}

fn title(t: &Theme) -> Rectangle {
    let focus = Focus::read();

    Rectangle::new()
        .width(320.0)
        .height(HEIGHT - 1.0)
        .align_child(Start, Center)
        .child(label(&focus.title, 12.5, t.dim).elide())
}

// the clock pill: time and date, and the weather on its right once there is some
const CLOCK_WIDTH: f32 = 150.0;
const CLOCK_WIDTH_WITH_WEATHER: f32 = 226.0;

fn clock_width() -> f32 {
    if Weather::read().ready { CLOCK_WIDTH_WITH_WEATHER } else { CLOCK_WIDTH }
}

fn clock(t: &Theme, name: &str) -> Rectangle {
    let clock = Clock::read();
    let weather = Weather::read();

    let mut parts: Vec<Box<dyn Widget>> = vec![Box::new(bold(&clock.time, 13.0, t.fg)), Box::new(label(&clock.date, 12.0, t.dim))];

    if weather.ready {
        let (glyph, _) = describe(weather.code, weather.day);

        parts.push(Box::new(Rectangle::new().width(1.0).height(14.0).fill(t.border)));
        parts.push(Box::new(icon(glyph, 14.0, t.accent)));
        parts.push(Box::new(label(crate::weather::degrees(weather.temp), 12.0, t.fg)));
    }

    chip(
        "clock",
        t,
        if weather.ready { CLOCK_WIDTH_WITH_WEATHER } else { CLOCK_WIDTH },
        Row::new(parts).gap(10.0).align(Center),
    )
    .on_click(toggle_on(name, Which::Calendar))
}

fn tray(t: &Theme, name: &str) -> Rectangle {
    let tray = crate::tray::Tray::read();

    let mut icons: Vec<Box<dyn Widget>> = Vec::new();

    for item in &tray.items {
        let target = item.clone();

        let picture = if item.icon.is_empty() {
            Rectangle::new().width(16.0).height(16.0).radius(0.0).fill(t.faint)
        } else {
            Rectangle::new().width(16.0).height(16.0).fill(amane::Image::contain(&item.icon))
        };

        icons.push(Box::new(
            Rectangle::new()
                .width(22.0)
                .height(22.0)
                .align_child(Center, Center)
                .cursor(amane::Pointer)
                .on_click({
                    let monitor = name.to_string();

                    move |button| crate::tray::click(&target, button, &monitor)
                })
                .child(picture),
        ));
    }

    // an empty tray takes no room
    if icons.is_empty() {
        return Rectangle::new().width(0.0).height(1.0);
    }

    let width = icons.len() as f32 * 22.0 + (icons.len() as f32 - 1.0) * 4.0 + 18.0;

    chip("tray", t, width, Row::new(icons).gap(4.0).align(Center))
}

fn cpu(t: &Theme, name: &str) -> Rectangle {
    let cpu = Cpu::read();

    chip(
        "cpu",
        t,
        STAT,
        Row::new(children![icon("\u{f4bc}", 13.0, t.accent), label(format!("{}%", cpu.percent()), 12.0, t.fg)])
            .gap(6.0)
            .align(Center),
    )
    .on_click(toggle_on(name, Which::System))
}

fn memory(t: &Theme, name: &str) -> Rectangle {
    let memory = Memory::read();

    chip(
        "memory",
        t,
        STAT,
        Row::new(children![icon("\u{f035b}", 13.0, t.accent), label(format!("{}%", memory.percent()), 12.0, t.fg)])
            .gap(6.0)
            .align(Center),
    )
    .on_click(toggle_on(name, Which::System))
}

fn audio(t: &Theme, name: &str) -> Rectangle {
    let audio = Audio::read();

    let glyph = if Level::Output.muted(audio.muted()) { "\u{f026}" } else { "\u{f028}" };

    chip(
        "audio",
        t,
        STAT,
        Row::new(children![icon(glyph, 13.0, t.accent), label(format!("{}%", Level::Output.percent(audio.volume(), audio.muted())), 12.0, t.fg)])
            .gap(6.0)
            .align(Center),
    )
    .on_click(toggle_on(name, Which::Audio))
    // up (negative) raises it, five percent a notch however many small events the notch is made of
    .on_scroll(|scroll| crate::volume::scroll_output(crate::ui::wheel_steps(-scroll.y)))
}

// a plain icon: showing whether something is connected would keep Bluetooth polled all the time (see btstate.rs)
fn bluetooth(t: &Theme, name: &str) -> Rectangle {
    chip("bluetooth", t, SQUARE, icon("\u{f293}", 13.0, t.dim)).on_click(toggle_on(name, Which::Bluetooth))
}

fn clipboard(t: &Theme, name: &str) -> Rectangle {
    chip("clipboard", t, SQUARE, icon("\u{f0ea}", 13.0, t.dim)).on_click(toggle_on(name, Which::Clipboard))
}

fn network(t: &Theme, name: &str) -> Rectangle {
    let network = Network::read();

    // on Wi-Fi the bars follow the signal, with a warning mark while the network doesn't reach the internet, and an
    // empty outline means a network is being joined
    let (glyph, color) = match network.link() {
        Link::Wifi => (crate::wifi::glyph(network.strength(), crate::wifi::internet(&network)), if crate::wifi::internet(&network) { t.accent } else { t.dim }),
        Link::Offline if network.connecting() => ("\u{f092f}", t.dim),
        Link::Offline if !network.wifi_enabled() => ("\u{f092e}", t.faint),
        Link::Offline => ("\u{f05aa}", t.faint),
        _ => ("\u{f0200}", if crate::wifi::internet(&network) { t.accent } else { t.dim }),
    };

    chip("network", t, SQUARE, icon(glyph, 14.0, color))
        .on_click(toggle_on(name, Which::Network))
}

fn bell(t: &Theme, name: &str) -> Rectangle {
    let count = Notifications::read().list().len();

    chip(
        "bell",
        t,
        bell_width(count),
        Row::new(children![
            icon("\u{f0f3}", 13.0, if count > 0 { t.accent } else { t.dim }),
            label(if count > 0 { count.to_string() } else { String::new() }, 12.0, t.fg),
        ])
        .gap(6.0)
        .align(Center),
    )
    .on_click(toggle_on(name, Which::Notifications))
}

// the charge left, on a computer with a battery. a desktop has none, and no room is left for one
fn battery(t: &Theme) -> Option<Rectangle> {
    let battery = Battery::read();

    if !battery.present() {
        return None;
    }

    let (percent, charging, full) = (battery.percent(), battery.charging(), battery.full());

    let low = percent <= crate::battery::LOW && !charging && !full;

    let (glyph_color, text_color) = if low { (t.red, t.red) } else { (t.accent, t.fg) };

    Some(chip(
        "battery",
        t,
        STAT,
        Row::new(children![icon(crate::battery::glyph(percent, charging, full), 13.0, glyph_color), label(format!("{percent}%"), 12.0, text_color)])
            .gap(6.0)
            .align(Center),
    ))
}

fn power(t: &Theme, name: &str) -> Rectangle {
    chip("power", t, SQUARE, icon("\u{f011}", 13.0, t.red)).on_click(toggle_on(name, Which::Power))
}

fn toggle_on(name: &str, which: Which) -> impl Fn(amane::Button) + 'static {
    let name = name.to_string();

    move |_| panels::toggle(which, &name)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Slot {
    Tray,
    Audio,
    Bluetooth,
    Clipboard,
    Network,
    Power,
    System, // the CPU and RAM chips both open it, lined up under the RAM chip
}

fn bell_width(count: usize) -> f32 {
    if count > 0 { BELL_WITH_COUNT } else { SQUARE }
}

/// distance from the screen's right edge to the right edge of a chip, so a dropdown can line up under it
pub fn right_edge(slot: Slot) -> i32 {
    // the chips on the right in the order `view` puts them, last one first. the ones without a slot have no
    // dropdown of their own (the bell opens the notification center on the other side)
    let battery = Battery::read().present();

    let from_the_right = [
        (Some(Slot::Power), SQUARE),
        // only on a computer with a battery (see `battery`): without one, -GAP cancels the gap added below
        (None, if battery { STAT } else { -GAP }),
        (None, bell_width(Notifications::read().list().len())),
        (Some(Slot::Network), SQUARE),
        (Some(Slot::Clipboard), SQUARE),
        (Some(Slot::Bluetooth), SQUARE),
        (Some(Slot::Audio), STAT),
        (Some(Slot::System), STAT), // memory
        (None, STAT),               // cpu
        (Some(Slot::Tray), 0.0), // as wide as its icons, but nothing is placed to its left
    ];

    let mut edge = PADDING;

    for (item, width) in from_the_right {
        if item == Some(slot) {
            break;
        }

        edge += width + GAP;
    }

    edge as i32
}
