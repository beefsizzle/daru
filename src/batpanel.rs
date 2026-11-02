use amane::{Battery, Center, Color, Column, Horizontal, LayerWindow, Monitor, Parent, Rectangle, Row, Service, Start, Widget, children};

use crate::battery::{Details, History, SAMPLES, draw};
use crate::panels::{self, Which};
use crate::theme::{Theme, alpha};
use crate::ui::{bold, icon, label, section};

/*
 * the battery dropdown, from the bar's battery chip, laid out like the network one: the charge and what the battery
 * is doing at the top, its charge over the last two hours, two cards for how fast it charges or runs down and how
 * worn it is, and the rest in a card of details. the graph and the draw come from battery.rs's minute samples, the
 * rest from its Details, read only while this is open
 */
const WIDTH: f32 = 372.0;
const INNER: f32 = WIDTH - 28.0;

const HEADER: f32 = 40.0;
const GRAPH: f32 = 44.0;
const CHARGE_CARD: f32 = 24.0 + 14.0 + 6.0 + GRAPH + 4.0 + 12.0;
const CARD: f32 = 88.0;
const DETAIL_ROW: f32 = 18.0;

// "1h 5m", "45m"
fn span(hours: f64) -> String {
    let minutes = (hours * 60.0).round().max(1.0) as u64;

    if minutes >= 60 { format!("{}h {}m", minutes / 60, minutes % 60) } else { format!("{minutes}m") }
}

/*
 * the charge once a minute as thin bars on a fixed 0-100 scale, the oldest on the left. slots from before the shell
 * started stay empty, so the time along the bottom is always two hours. charging shows in the accent, running down
 * a shade lighter, and low in red
 */
fn graph(t: &Theme, history: &History, width: f32) -> Row {
    let gap = 0.6;
    let bar = (width - gap * (SAMPLES - 1) as f32) / SAMPLES as f32;

    let missing = SAMPLES.saturating_sub(history.samples.len());

    let mut bars: Vec<Box<dyn Widget>> = Vec::new();

    for slot in 0..SAMPLES {
        let track = Rectangle::new().width(bar).height(GRAPH).fill(alpha(t.fg, 0.04)).align_child(Center, amane::End);

        let Some(sample) = slot.checked_sub(missing).and_then(|index| history.samples.get(index)) else {
            bars.push(Box::new(track));

            continue;
        };

        let color: Color = if sample.draining && sample.percent <= crate::battery::LOW {
            t.red
        } else if sample.draining {
            alpha(t.accent, 0.55)
        } else {
            t.accent
        };

        bars.push(Box::new(track.child(Rectangle::new().width(bar).height((GRAPH * f32::from(sample.percent) / 100.0).max(1.0)).fill(color))));
    }

    Row::new(bars).gap(gap)
}

fn charge_card(t: &Theme, history: &History, glyph: &str) -> Rectangle {
    let inner = INNER - 24.0;

    Rectangle::new().width(INNER).height(CHARGE_CARD).fill(t.surface).border(1.0, t.border).padding(12.0).align_child(Start, Start).child(
        Column::new(children![
            Row::new(children![
                Row::new(children![icon(glyph, 11.0, t.accent), section("Charge", t)]).gap(7.0).align(Center),
                Rectangle::new().width(Parent).height(1.0),
                label("last 2 hours", 10.5, t.faint),
            ])
            .align(Center)
            .width(inner),
            graph(t, history, inner),
            Row::new(children![label("past", 10.0, t.faint), Rectangle::new().width(Parent).height(1.0), label("now", 10.0, t.faint)]).align(Center).width(inner),
        ])
        .gap(6.0),
    )
}

// a half-width card: a small heading, a big number, and a line under it, as the network dropdown's speed cards
fn card(t: &Theme, glyph: &str, caption: &str, value: String, note: String) -> Rectangle {
    Rectangle::new().width((INNER - 10.0) / 2.0).height(CARD).fill(t.surface).border(1.0, t.border).padding(12.0).align_child(Start, Start).child(
        Column::new(children![
            Row::new(children![icon(glyph, 11.0, t.accent), section(caption, t)]).gap(7.0).align(Center),
            bold(value, 18.0, t.fg),
            label(note, 11.5, t.dim),
        ])
        .gap(5.0),
    )
}

fn detail(t: &Theme, name: &str, value: String) -> Box<dyn Widget> {
    Box::new(
        Row::new(children![label(name, 12.0, t.dim), Rectangle::new().width(Parent).height(1.0), label(value, 12.0, t.fg)])
            .align(Center)
            .width(INNER - 24.0)
            .height(DETAIL_ROW),
    )
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::Battery, monitor) else {
        return panels::hidden("amane-panel");
    };

    let (percent, charging, full) = {
        let battery = Battery::read();

        (battery.percent(), battery.charging(), battery.full())
    };

    let details = Details::read().clone();
    let history = History::read();

    let used = draw(&history.samples);

    // how long until it's full, from the rate it charges at, or until it's empty, from the rate it ran down at
    let to_full = details.charging.zip(details.full.zip(details.now)).map(|(watts, (full, now))| (full - now).max(0.0) / f64::from(watts));
    let to_empty = used.zip(details.now).filter(|((watts, _), _)| *watts > 0.0).map(|((watts, _), now)| now / f64::from(watts));

    let glyph = crate::battery::glyph(percent, charging, full);

    let low = percent <= crate::battery::LOW && !charging && !full;

    let subtitle = match details.status.as_str() {
        "Charging" => to_full.map_or(String::from("Charging"), |hours| format!("Charging \u{b7} full in about {}", span(hours))),
        "Discharging" => to_empty.map_or(String::from("On battery"), |hours| format!("On battery \u{b7} about {} left", span(hours))),
        "Full" => String::from("Fully charged"),
        "Not charging" => String::from("Plugged in, not charging"),
        _ => String::new(),
    };

    let header = Row::new(children![
        Rectangle::new().width(HEADER).height(HEADER).fill(t.accent_soft).align_child(Center, Center).child(icon(glyph, 18.0, if low { t.red } else { t.accent })),
        Column::new(children![bold(format!("Battery \u{b7} {percent}%"), 14.0, if low { t.red } else { t.fg }), label(subtitle, 11.5, t.dim)]).gap(2.0),
    ])
    .gap(12.0)
    .align(Center);

    // what goes in or comes out. running down, the number needs five minutes on battery, see battery::draw
    let rate = match (details.status.as_str(), details.charging, used) {
        ("Charging", Some(watts), _) => card(&t, "\u{f0e7}", "Charging", format!("{watts:.1} W"), String::from("adding ions")),
        ("Discharging", _, Some((watts, over))) => card(&t, "\u{f0e7}", "Using", format!("{watts:.1} W"), format!("Iavg draw last {}", span(over.as_secs_f64() / 3600.0))),
        ("Discharging", _, None) => card(&t, "\u{f0e7}", "Using", String::from("\u{2013}"), String::from("after 5 min on battery")),
        _ => card(&t, "\u{f1e6}", "Power", String::from("Plugged in"), String::from("not charging")),
    };

    let health = card(
        &t,
        "\u{f004}",
        "Health",
        details.health().map_or(String::from("\u{2013}"), |health| format!("{health}%")),
        details.cycles.map_or(String::new(), |cycles| format!("{cycles} cycles")),
    );

    let mut rows: Vec<Box<dyn Widget>> = Vec::new();

    if let Some((now, full)) = details.now.zip(details.full) {
        rows.push(detail(&t, "Charge left", format!("{now:.1} of {full:.1} Wh")));
    }

    if let Some(design) = details.design {
        rows.push(detail(&t, "Capacity when new", format!("{design:.1} Wh")));
    }

    if let Some(volts) = details.volts {
        rows.push(detail(&t, "Voltage", format!("{volts:.2} V")));
    }

    let mut parts: Vec<Box<dyn Widget>> =
        vec![Box::new(header), Box::new(charge_card(&t, &history, glyph)), Box::new(Row::new(children![rate, health]).gap(10.0))];

    let mut height = 28.0 + HEADER + 14.0 + CHARGE_CARD + 14.0 + CARD;

    if !rows.is_empty() {
        let card_height = rows.len() as f32 * DETAIL_ROW + (rows.len() as f32 - 1.0) * 6.0 + 24.0;

        parts.push(Box::new(Rectangle::new().width(INNER).height(card_height).fill(t.surface).border(1.0, t.border).padding(12.0).child(Column::new(rows).gap(6.0))));

        height += 14.0 + card_height;
    }

    panels::window_at(WIDTH, height, Horizontal::Right, crate::bar::right_edge(crate::bar::Slot::Battery))
        .child(panels::body(&t).padding(14.0).child(Column::new(parts).gap(14.0)))
}

#[cfg(test)]
mod tests {
    use super::span;

    #[test]
    fn spans_read_in_hours_and_minutes() {
        assert_eq!(span(1.0 + 5.0 / 60.0), "1h 5m");
        assert_eq!(span(0.75), "45m");
        assert_eq!(span(0.0), "1m");
    }
}
