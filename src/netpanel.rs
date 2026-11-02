use amane::{
    AccessPoint, Center, Column, Horizontal, Key, Keyboard, LayerWindow, Link, Monitor, Network, Parent, Rectangle, Row,
    ScrollArea, Service, Start, TextInput, Widget, children,
};

use crate::netinfo::Netinfo;
use crate::panels::{self, Which};
use crate::sys::{Sys, format_rate};
use crate::theme::{Theme, alpha, mix};
use crate::ui::{bold, chip, hover, icon, label, round_button, section, set_hover, sparkline};
use crate::wifi::{self, Wifi};

const WIDTH: f32 = 372.0;
const INNER: f32 = WIDTH - 28.0;

fn bytes(count: u64) -> String {
    let value = count as f64;

    if value >= 1e12 {
        format!("{:.2} TB", value / 1e12)
    } else if value >= 1e9 {
        format!("{:.1} GB", value / 1e9)
    } else {
        format!("{:.0} MB", value / 1e6)
    }
}

fn speed_card(t: &Theme, caption: &str, glyph: &str, rate: f64, history: &[f32]) -> Rectangle {
    Rectangle::new()
        .width((INNER - 10.0) / 2.0)
        .height(88.0)
        .fill(t.surface)
        .border(1.0, t.border)
        .padding(12.0)
        .align_child(Start, Start)
        .child(
            Column::new(children![
                Row::new(children![icon(glyph, 11.0, t.accent), section(caption, t)]).gap(7.0).align(Center),
                bold(format_rate(rate), 18.0, t.fg),
                // at least 100 KB/s tall, so an idle line stays flat
                sparkline(t, (INNER - 10.0) / 2.0 - 24.0, 26.0, history, t.accent, 100_000.0),
            ])
            .gap(5.0),
        )
}

const DETAIL_ROW: f32 = 18.0;

fn detail(t: &Theme, name: &str, value: String) -> Box<dyn Widget> {
    Box::new(
        Rectangle::new().width(INNER - 24.0).height(DETAIL_ROW).align_child(Start, Center).child(
            Row::new(children![
                label(name, 12.0, t.dim),
                Rectangle::new().width(Parent).height(1.0),
                label(value, 12.0, t.fg),
            ])
            .align(Center)
            .width(INNER - 24.0),
        ),
    )
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::Network, monitor) else {
        return panels::hidden("amane-panel");
    };
    let network = Network::read();
    let info = Netinfo::read();
    let sys = Sys::read();

    let (glyph, title) = match network.link() {
        Link::Wired => ("\u{f0200}", String::from("Wired connection")),
        Link::Wifi => (wifi::glyph(network.strength(), wifi::internet(&network)), format!("Wi-Fi \u{b7} {}", network.ssid())),
        Link::Offline => ("\u{f05aa}", String::from("Offline")),
        _ => ("\u{f0200}", String::from("Connected")),
    };

    let mut subtitle = if info.interface.is_empty() { String::from("No active interface") } else { format!("{} \u{b7} {}", info.connection, info.interface) };

    if network.connected() && !wifi::internet(&network) {
        subtitle = format!("No internet \u{b7} {subtitle}");
    }

    let header = Row::new(children![
        Rectangle::new()
            .width(40.0)
            .height(40.0)
            .fill(t.accent_soft)
            .align_child(Center, Center)
            .child(icon(glyph, 18.0, if network.connected() { t.accent } else { t.faint })),
        Column::new(children![bold(title, 14.0, t.fg), label(subtitle, 11.5, t.dim)]).gap(2.0),
    ])
    .gap(12.0)
    .align(Center);

    let speeds = Row::new(children![
        speed_card(&t, "Download", "\u{f063}", sys.rx, &sys.rx_history),
        speed_card(&t, "Upload", "\u{f062}", sys.tx, &sys.tx_history),
    ])
    .gap(10.0);

    let mut details: Vec<Box<dyn Widget>> = Vec::new();

    if !info.ip.is_empty() {
        details.push(detail(&t, "IP address", info.ip.clone()));
    }

    if !info.ip6.is_empty() {
        details.push(detail(&t, "IPv6", info.ip6.clone()));
    }

    if !info.gateway.is_empty() {
        details.push(detail(&t, "Gateway", info.gateway.clone()));
    }

    if !info.dns.is_empty() {
        details.push(detail(&t, "DNS", info.dns.iter().take(2).cloned().collect::<Vec<_>>().join(", ")));
    }

    if !info.mac.is_empty() {
        details.push(detail(&t, "MAC address", info.mac.to_lowercase()));
    }

    if network.link() == Link::Wifi {
        details.push(detail(&t, "Signal", format!("{}%", network.strength())));
    } else if !info.speed.is_empty() {
        details.push(detail(&t, "Link speed", info.speed.clone()));
    }

    if let Some(ping) = info.ping {
        details.push(detail(&t, "Latency to gateway", format!("{ping:.1} ms")));
    }

    details.push(detail(&t, "Received", bytes(sys.rx_total)));
    details.push(detail(&t, "Sent", bytes(sys.tx_total)));

    let detail_height = details.len() as f32 * DETAIL_ROW + (details.len() as f32 - 1.0) * 6.0 + 24.0;

    let card = Rectangle::new()
        .width(INNER)
        .height(detail_height)
        .fill(t.surface)
        .border(1.0, t.border)
        .padding(12.0)
        .child(Column::new(details).gap(6.0));

    let mut sections: Vec<Box<dyn Widget>> = vec![Box::new(header)];

    let mut height = 28.0 + 40.0;

    // the networks in range come first, they're what this dropdown is opened for on a laptop
    if info.has_wifi {
        let (block, block_height) = wifi_section(&t, &network);

        sections.push(block);

        height += 14.0 + block_height;
    }

    sections.push(Box::new(speeds));
    sections.push(Box::new(card));

    height += 14.0 + 88.0 + 14.0 + detail_height;

    let window = panels::window_at(WIDTH, height, Horizontal::Right, crate::bar::right_edge(crate::bar::Slot::Network))
        .child(panels::body(&t).padding(14.0).child(Column::new(sections).gap(14.0)));

    // typing a password needs the keyboard, the rest of the time the dropdown leaves it alone
    if Wifi::read().prompt.is_some() {
        window.keyboard(Keyboard::Exclusive).on_key(|key| {
            if key == Key::Escape {
                wifi::cancel();
            }
        })
    } else {
        window
    }
}

// ---------------------------------------------------------------- wi-fi

const ROW: f32 = 44.0;
const MAX_ROWS: usize = 6;
const HEADER: f32 = 26.0;
const NOTE_HEIGHT: f32 = 70.0;

// joined first, then the ones that join with a click, then by signal
fn order(points: &[AccessPoint]) -> Vec<&AccessPoint> {
    let mut sorted: Vec<&AccessPoint> = points.iter().collect();

    sorted.sort_by(|a, b| b.active().cmp(&a.active()).then(b.saved().cmp(&a.saved())).then(b.strength().cmp(&a.strength())));

    sorted
}

fn status(point: &AccessPoint, wifi: &Wifi, connecting: bool, internet: bool) -> (String, bool) {
    let ssid = point.ssid();

    if wifi.joining.as_deref() == Some(ssid) {
        return (String::from(if connecting { "Connecting\u{2026}" } else { "Starting\u{2026}" }), true);
    }

    if point.active() {
        return (String::from(if internet { "Connected" } else { "Connected, no internet" }), true);
    }

    if let Some(failure) = wifi.failed.as_ref().filter(|failure| failure.ssid == ssid) {
        let text = match (failure.refused, failure.typed) {
            (true, true) => "Wrong password, try again",
            (true, false) => "Saved password refused, forget it to retry",
            (false, _) => "Couldn\u{2019}t connect",
        };

        return (String::from(text), false);
    }

    if wifi::enterprise(point) && !point.saved() {
        return (String::from("Needs a username, not supported here"), false);
    }

    let text = match (point.saved(), point.secured()) {
        (true, _) => "Saved",
        (false, true) => "Secured",
        (false, false) => "Open",
    };

    (String::from(text), false)
}

fn network_row(t: &Theme, index: usize, point: &AccessPoint, wifi: &Wifi, network: &Network) -> Rectangle {
    let id = format!("wifi-{index}");
    let hot = hover(&id);

    let ssid = point.ssid().to_string();
    let (active, saved, secured) = (point.active(), point.saved(), point.secured());
    let busy = wifi.joining.is_some();

    // an enterprise network needs a username the panel can't ask for, so only a profile made elsewhere joins it
    let unsupported = wifi::enterprise(point) && !saved;

    let (text, lit) = status(point, wifi, network.connecting(), wifi::internet(network));

    let action = if active {
        "Disconnect"
    } else if busy || unsupported {
        ""
    } else if saved || !secured {
        "Connect"
    } else {
        "Join"
    };

    let target = ssid.clone();

    // a saved network can be forgotten, which is also the way out of a saved password that stopped working
    let forget: Box<dyn Widget> = if saved && !busy {
        Box::new(round_button(&format!("wifi-x-{index}"), t, "\u{f1f8}", t.faint, move |_| wifi::forget(&target)))
    } else {
        Box::new(Rectangle::new().width(1.0).height(1.0))
    };

    let glyph_color = if active { t.accent } else { t.dim };

    Rectangle::new()
        .width(INNER)
        .height(ROW)
        .fill(if active { alpha(t.accent, 0.14) } else { mix(alpha(t.surface, 0.0), t.surface_hi, hot) })
        .padding(amane::Padding { top: 0.0, right: 8.0, bottom: 0.0, left: 12.0 })
        .align_child(Start, Center)
        .cursor(amane::Pointer)
        .on_hover(move |inside| set_hover(&id, inside))
        .on_click(move |_| {
            if active {
                Network::disconnect();
            } else if busy || unsupported {
                // one join at a time, and none the panel can't finish
            } else if saved || !secured {
                wifi::join(&ssid, None);
            } else {
                wifi::ask(&ssid);
            }
        })
        .child(
            Row::new(vec![
                Box::new(Rectangle::new().width(22.0).height(22.0).align_child(Center, Center).child(icon(wifi::glyph(point.strength(), true), 15.0, glyph_color)))
                    as Box<dyn Widget>,
                Box::new(
                    Column::new(children![
                        Rectangle::new().width(INNER - 196.0).height(17.0).child(bold(point.ssid(), 12.5, t.fg).elide()),
                        label(text, 11.0, if lit { t.accent } else { t.dim }),
                    ])
                    .gap(1.0),
                ),
                Box::new(Rectangle::new().width(Parent).height(1.0)),
                Box::new(icon(if secured { "\u{f023}" } else { "" }, 10.0, t.faint)),
                Box::new(label(action, 11.0, t.faint)),
                forget,
            ])
            .gap(10.0)
            .align(Center)
            .width(INNER - 20.0),
        )
}

// the password field that opens under a secured network
fn password_row(t: &Theme, typed: usize) -> Rectangle {
    let ready = typed >= 8;

    Rectangle::new()
        .width(INNER)
        .height(ROW)
        .fill(mix(t.surface, t.surface_hi, 0.4))
        .border(1.0, alpha(t.accent, 0.5))
        .padding(amane::Padding { top: 0.0, right: 8.0, bottom: 0.0, left: 12.0 })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                icon("\u{f084}", 12.0, t.accent),
                TextInput::new(wifi::PASSWORD_FIELD)
                    .placeholder("Password")
                    .password()
                    .focused()
                    .size(13.0)
                    .color(t.fg)
                    .width(INNER - 20.0 - 12.0 - 60.0 - 20.0)
                    .on_change(|text| Wifi::write().password = text)
                    .on_submit(|_| wifi::submit()),
                chip("wifi-join", t, 60.0, label("Join", 11.5, if ready { t.accent } else { t.faint })).on_click(|_| wifi::submit()),
            ])
            .gap(10.0)
            .align(Center)
            .width(INNER - 20.0),
        )
}

// the Wi-Fi switch and the networks in range, and how tall they are
fn wifi_section(t: &Theme, network: &Network) -> (Box<dyn Widget>, f32) {
    let wifi = Wifi::read();
    let enabled = network.wifi_enabled();

    let busy = if wifi.joining.is_some() {
        "Connecting\u{2026}"
    } else if enabled && network.access_points().is_empty() {
        "Searching\u{2026}"
    } else {
        ""
    };

    let header = Row::new(children![
        section("Wi-Fi networks", t),
        Rectangle::new().width(Parent).height(1.0),
        label(busy, 11.0, t.faint),
        chip("wifi-power", t, 52.0, label(if enabled { "On" } else { "Off" }, 11.5, if enabled { t.accent } else { t.dim }))
            .on_click(|_| Network::set_wifi(!Network::read().wifi_enabled())),
    ])
    .gap(10.0)
    .align(Center)
    .width(INNER);

    let note = |text: &str| -> (Box<dyn Widget>, f32) {
        (Box::new(Rectangle::new().width(INNER).height(NOTE_HEIGHT).align_child(Center, Center).child(label(text, 12.5, t.faint))), NOTE_HEIGHT)
    };

    let (body, body_height) = if !enabled {
        note("Wi-Fi is off")
    } else if network.access_points().is_empty() {
        note("Looking for networks\u{2026}")
    } else {
        let mut rows: Vec<Box<dyn Widget>> = Vec::new();

        for (index, point) in order(network.access_points()).into_iter().enumerate() {
            rows.push(Box::new(network_row(t, index, point, &wifi, network)));

            if wifi.prompt.as_deref() == Some(point.ssid()) {
                rows.push(Box::new(password_row(t, wifi.password.chars().count())));
            }
        }

        // the password field adds a row, so the network it belongs to doesn't scroll away
        let shown = rows.len().min(MAX_ROWS + usize::from(wifi.prompt.is_some())) as f32;

        let list_height = shown * ROW + (shown - 1.0) * 2.0;

        let list = Rectangle::new().width(INNER).height(list_height).clip().child(ScrollArea::new("wifi-list", Column::new(rows).gap(2.0)));

        (Box::new(list) as Box<dyn Widget>, list_height)
    };

    let block = Column::new(vec![Box::new(Rectangle::new().width(INNER).height(HEADER).align_child(Start, Center).child(header)) as Box<dyn Widget>, body]).gap(10.0);

    (Box::new(block), HEADER + 10.0 + body_height)
}
