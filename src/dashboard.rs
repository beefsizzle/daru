use amane::{
    Audio, Button, Center, Column, Horizontal, LayerWindow, Monitor, Network, Parent, Rectangle, Row, Service, SpaceBetween, Start, Widget, children,
};

use crate::btstate::BtState;
use crate::notify::Notify;
use crate::panels::{self, Panels, Which};
use crate::sys::{Sys, format_rate};
use crate::theme::{Theme, alpha, mix};
use crate::ui::{bold, hover, icon, label, meter, round_button, set_hover, slider};
use crate::volume::{self, Level};

const WIDTH: f32 = 372.0;
const INNER: f32 = WIDTH - 28.0;
const TILE: f32 = 108.0;
const TILE_HEIGHT: f32 = 70.0;

pub fn view(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::Dashboard, monitor) else {
        return panels::hidden("amane-panel");
    };
    // header 40, tiles 2 rows, volume 30, system 125, and the gaps between them
    let height = 28.0 + 40.0 + 14.0 + (TILE_HEIGHT * 2.0 + 10.0) + 14.0 + 30.0 + 14.0 + 125.0;

    let sections: Vec<Box<dyn Widget>> = vec![
        Box::new(header(&t)),
        Box::new(tiles(&t)),
        Box::new(volume(&t)),
        Box::new(system(&t)),
    ];

    panels::window(WIDTH, height, Horizontal::Left).child(
        panels::body(&t).padding(14.0).child(Column::new(sections).gap(14.0)),
    )
}

// ---------------------------------------------------------------- header

// what's left of the header's width for the text: the avatar, five 30px buttons and the six 6px gaps between them all
const TEXT_WIDTH: f32 = INNER - 40.0 - 5.0 * 30.0 - 6.0 * 6.0;

fn header(t: &Theme) -> Row {
    let user = std::env::var("USER").unwrap_or_else(|_| String::from("user"));
    let initial = user.chars().next().unwrap_or('?').to_uppercase().to_string();

    // read once and let go: the buttons below read `Panels` too, and a second read held by the same thread can deadlock
    // when another thread is waiting to write (a std RwLock lets a waiting writer block new readers)
    let armed = Panels::read().armed;
    let uptime = Sys::read().uptime.clone();

    // the name and uptime, or while a power button waits for its second click, what that click will do. the text
    // has a box of its own, as wide as the room left of the buttons, so a longer line can't push them aside
    let (top, bottom) = match armed {
        Some(action) => (bold("Click again", 14.0, t.red), label(format!("to {action}"), 11.5, t.red)),
        None => (bold(user, 14.0, t.fg), label(format!("up {uptime}"), 11.5, t.dim)),
    };

    let line = |text: amane::Text, height: f32| Rectangle::new().width(TEXT_WIDTH).height(height).child(text.elide());

    let text = Column::new(children![line(top, 18.0), line(bottom, 15.0)]).gap(2.0);

    let avatar = Rectangle::new()
        .width(40.0)
        .height(40.0)
        .radius(0.0)
        .fill(t.accent_soft)
        .border(1.5, alpha(t.accent, 0.5))
        .align_child(Center, Center)
        .child(bold(initial, 17.0, t.accent));

    Row::new(children![
        avatar,
        text,
        round_button("lock", t, "\u{f023}", t.dim, |_| {
            panels::close();

            amane::Lock::start();
        }),
        power_button("sleep", t, armed, "\u{f186}", "sleep", crate::power::SUSPEND),
        power_button("logout", t, armed, "\u{f08b}", "log out", crate::compositor::log_out()),
        power_button("reboot", t, armed, "\u{f021}", "restart", crate::power::REBOOT),
        power_button("shutdown", t, armed, "\u{f011}", "shut down", crate::power::POWER_OFF),
    ])
    .gap(6.0)
    .align(Center)
    .width(INNER)
}

// the first click only arms the button, the second one runs it
fn power_button(id: &'static str, t: &Theme, armed_action: Option<&'static str>, glyph: &str, action: &'static str, command: &'static str) -> Rectangle {
    let color = if armed_action == Some(action) { t.red } else { t.dim };

    round_button(id, t, glyph, color, move |_: Button| {
        let mut panels = Panels::write();

        if panels.armed == Some(action) {
            panels.armed = None;

            amane::spawn(command);
        } else {
            panels.armed = Some(action);
        }
    })
}

// ---------------------------------------------------------------- toggles

fn tile_text(content: impl Widget + 'static, height: f32) -> Box<dyn Widget> {
    Box::new(Rectangle::new().width(TILE - 22.0).height(height).child(content))
}

fn tile(
    id: &str,
    t: &Theme,
    glyph: &str,
    title: &str,
    subtitle: &str,
    active: bool,
    on_click: impl Fn(Button) + 'static,
) -> Rectangle {
    let hot = hover(id);

    let fill = if active { mix(t.accent_soft, alpha(t.accent, 0.34), hot) } else { mix(t.surface, t.surface_hi, hot) };

    let tint = if active { t.accent } else { t.dim };

    let id = id.to_string();

    Rectangle::new()
        .width(TILE)
        .height(TILE_HEIGHT)
        .radius(0.0)
        .fill(fill)
        .border(1.0, if active { alpha(t.accent, 0.35) } else { t.border })
        .padding(11.0)
        .align_child(Start, Start)
        .cursor(amane::Pointer)
        .on_hover(move |inside| set_hover(&id, inside))
        .on_click(on_click)
        .child(
            Column::new(vec![
                Box::new(icon(glyph, 17.0, tint)),
                Box::new(Rectangle::new().width(1.0).height(7.0)),
                tile_text(bold(title, 12.5, t.fg).elide(), 16.0),
                tile_text(label(subtitle, 10.5, t.dim).elide(), 14.0),
            ]),
        )
}

fn tiles(t: &Theme) -> Column {
    let network = Network::read();
    let bluetooth = BtState::read();
    let audio = Audio::read();
    let notify = Notify::read();

    let (net_glyph, net_sub) = match network.link() {
        amane::Link::Wired if !crate::wifi::internet(&network) => ("\u{f0200}", String::from("Wired \u{b7} No internet")),
        amane::Link::Wired => ("\u{f0200}", String::from("Wired")),
        amane::Link::Wifi if !crate::wifi::internet(&network) => (crate::wifi::glyph(network.strength(), false), format!("{} \u{b7} No internet", network.ssid())),
        amane::Link::Wifi => (crate::wifi::glyph(network.strength(), true), network.ssid().to_string()),
        amane::Link::Offline => ("\u{f05aa}", String::from("Offline")),
        _ => ("\u{f0200}", String::from("Connected")),
    };

    let bluetooth_sub = if bluetooth.adapter.is_none() {
        "No adapter"
    } else if bluetooth.powered {
        "On"
    } else {
        "Off"
    };

    let first = Row::new(children![
        tile("t-net", t, net_glyph, "Network", &net_sub, network.connected(), |_| {
            let on = Network::read().wifi_enabled();

            Network::set_wifi(!on);
        }),
        tile("t-bt", t, "\u{f293}", "Bluetooth", bluetooth_sub, bluetooth.powered, |_| {
            let on = BtState::read().powered;

            crate::btstate::set_powered(!on);
        }),
        tile(
            "t-dnd",
            t,
            if notify.dnd { "\u{f1f6}" } else { "\u{f0f3}" },
            "Quiet",
            if notify.dnd { "On" } else { "Off" },
            notify.dnd,
            |_| {
                let on = Notify::read().dnd;

                Notify::write().dnd = !on;
            },
        ),
    ])
    .gap(10.0);

    let second = Row::new(children![
        tile(
            "t-mic",
            t,
            if Level::Input.muted(audio.microphone_muted()) { "\u{f131}" } else { "\u{f130}" },
            "Microphone",
            if Level::Input.muted(audio.microphone_muted()) { "Muted" } else { "Live" },
            !Level::Input.muted(audio.microphone_muted()),
            |_| Audio::toggle_microphone_mute(),
        ),
        tile(
            "t-mute",
            t,
            if Level::Output.muted(audio.muted()) { "\u{f026}" } else { "\u{f028}" },
            "Speakers",
            if Level::Output.muted(audio.muted()) { "Muted" } else { "On" },
            !Level::Output.muted(audio.muted()),
            |_| Audio::toggle_mute(),
        ),
        tile("t-shot", t, "\u{f030}", "Screenshot", "Region", false, |_| {
            panels::close();

            // a moment for the dashboard to close first, so it isn't in the picture, or in the frozen screen
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(300));

                crate::compositor::take(crate::compositor::Shot::Region);
            });
        }),
    ])
    .gap(10.0);

    Column::new(children![first, second]).gap(10.0)
}

// ---------------------------------------------------------------- volume

fn volume(t: &Theme) -> Row {
    let audio = Audio::read();

    let muted = Level::Output.muted(audio.muted());
    let percent = Level::Output.percent(audio.volume(), audio.muted());
    let level = f32::from(percent) / 100.0;

    let glyph = if muted {
        "\u{f026}"
    } else if level < 0.4 {
        "\u{f027}"
    } else {
        "\u{f028}"
    };

    Row::new(children![
        round_button("v-mute", t, glyph, if muted { t.faint } else { t.accent }, |_| Audio::toggle_mute()),
        slider(t, 250.0, level, volume::drag_output, volume::scroll_output),
        Rectangle::new()
            .width(44.0)
            .height(20.0)
            .align_child(amane::End, Center)
            .child(label(format!("{percent}%"), 12.0, t.dim)),
    ])
    .gap(10.0)
    .align(Center)
    .width(INNER)
}

// ---------------------------------------------------------------- system

pub fn severity(t: &Theme, percent: f32) -> amane::Color {
    if percent > 90.0 {
        t.red
    } else if percent > 75.0 {
        t.yellow
    } else {
        t.accent
    }
}

pub fn gauge(t: &Theme, caption: &str, percent: f32, shown: String) -> Column {
    Column::new(children![
        bold(shown, 12.5, t.fg),
        meter(t, 24.0, percent / 100.0, severity(t, percent)),
        label(caption, 10.5, t.dim),
    ])
    .gap(6.0)
    .align(Center)
}

fn system(t: &Theme) -> Column {
    let cpu = amane::Cpu::read();
    let memory = amane::Memory::read();
    let sys = Sys::read();

    let rings = Row::new(children![
        gauge(t, "CPU", cpu.percent() as f32, format!("{}", cpu.percent())),
        gauge(t, "RAM", memory.percent() as f32, format!("{}", memory.percent())),
        gauge(t, "Disk", sys.disk as f32, format!("{}", sys.disk)),
        gauge(t, "Temp", sys.temp, format!("{:.0}\u{b0}", sys.temp)),
    ])
    .justify(SpaceBetween)
    .width(INNER);

    let speeds = Row::new(children![
        icon("\u{f063}", 11.0, t.accent),
        label(format_rate(sys.rx), 11.5, t.dim),
        Rectangle::new().width(Parent).height(1.0),
        icon("\u{f062}", 11.0, t.accent),
        label(format_rate(sys.tx), 11.5, t.dim),
    ])
    .gap(6.0)
    .align(Center)
    .width(INNER);

    Column::new(children![rings, speeds]).gap(10.0)
}
