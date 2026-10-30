use amane::{
    Center, Column, Cursor, Horizontal, LayerWindow, Monitor, Padding, Parent, Rectangle, Row, Service, SpaceBetween, Start, Widget,
    children,
};

use crate::clock::Clock;
use crate::events::{Event, Events, civil_from_days, days_from_civil, weekday_of};
use crate::panels::{self, Which};
use crate::theme::{Theme, alpha};
use crate::ui::{Wheel, bold, icon, label, round_button, section};
use crate::weather::{Weather, degrees, describe};

const WIDTH: f32 = 372.0;
const INNER: f32 = WIDTH - 28.0;

/*
 * which month the calendar shows, as months away from this one. the arrows and the wheel move it, clicking the month's
 * name comes back, and it's back on this month whenever the calendar opens
 */
#[derive(Default)]
pub struct Browse {
    months: i32,
    wheel: Wheel,
}

impl Service for Browse {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

pub fn reset() {
    if Browse::read().months != 0 {
        Browse::write().months = 0;
    }
}

// the year and month `months` away from this one
fn shift(year: i32, month: u32, months: i32) -> (i32, u32) {
    let index = year * 12 + month as i32 - 1 + months;

    (index.div_euclid(12), index.rem_euclid(12) as u32 + 1)
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::Calendar, monitor) else {
        return panels::hidden("amane-panel");
    };
    let weather = Weather::read();
    let clock = Clock::read();
    let events = Events::read();
    let months = Browse::read().months;

    let (year, month) = shift(clock.year, clock.month, months);

    let rows = month_rows(year, month);

    let calendar_height = 30.0 + 8.0 + 18.0 + 4.0 + rows as f32 * 32.0;

    let mut sections: Vec<Box<dyn Widget>> = Vec::new();
    let mut height = 28.0 + calendar_height;

    if weather.ready {
        sections.push(Box::new(current(&t, &weather)));
        sections.push(Box::new(hourly(&t, &weather)));
        sections.push(Box::new(week(&t, &weather)));

        height += 64.0 + 14.0 + 78.0 + 14.0 + 18.0 + 14.0 + DAY_HEIGHT + 14.0;
    } else {
        sections.push(Box::new(
            Rectangle::new()
                .width(INNER)
                .height(56.0)
                .radius(0.0)
                .fill(t.surface)
                .align_child(Center, Center)
                .child(label("Weather unavailable. Is ~/.config/amane-shell/location set?", 11.5, t.dim)),
        ));

        height += 56.0 + 14.0;
    }

    sections.push(Box::new(self::month(&t, &clock, &events, year, month, rows)));

    if let Some((list, list_height)) = agenda(&t, &events, &clock, year, month, months) {
        sections.push(Box::new(list));

        height += 14.0 + list_height;
    }

    panels::window(WIDTH, height, Horizontal::Middle)
        .child(panels::body(&t).padding(14.0).child(Column::new(sections).gap(14.0)))
}

// ---------------------------------------------------------------- weather

fn current(t: &Theme, weather: &Weather) -> Row {
    let (glyph, description) = describe(weather.code, weather.day);

    let stat = |name: &str, value: String| {
        Row::new(children![label(name, 11.0, t.faint), Rectangle::new().width(Parent).height(1.0), label(value, 11.5, t.fg)])
            .width(118.0)
            .align(Center)
    };

    Row::new(children![
        Rectangle::new().width(52.0).height(52.0).align_child(Center, Center).child(icon(glyph, 40.0, t.accent)),
        Column::new(children![
            bold(degrees(weather.temp), 34.0, t.fg),
            label(description, 12.5, t.dim),
        ])
        .gap(0.0),
        Rectangle::new().width(Parent).height(1.0),
        Column::new(children![
            stat("Feels like", degrees(weather.feels)),
            stat("Humidity", format!("{}%", weather.humidity)),
            stat("Wind", format!("{:.1} m/s", weather.wind)),
        ])
        .gap(4.0),
    ])
    .gap(12.0)
    .align(Center)
    .width(INNER)
}

fn hourly(t: &Theme, weather: &Weather) -> Rectangle {
    let mut slots: Vec<Box<dyn Widget>> = Vec::new();

    for (index, hour) in weather.hours.iter().enumerate() {
        let (glyph, _) = describe(hour.code, hour.day);

        let name = if index == 0 { String::from("Now") } else { hour.time.clone() };

        slots.push(Box::new(
            Column::new(children![
                label(name, 10.5, t.dim),
                icon(glyph, 17.0, if index == 0 { t.accent } else { t.fg }),
                bold(degrees(hour.temp), 12.0, t.fg),
            ])
            .gap(7.0)
            .align(Center),
        ));
    }

    Rectangle::new()
        .width(INNER)
        .height(78.0)
        .radius(0.0)
        .fill(t.surface)
        .padding(Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 })
        .align_child(Center, Center)
        .child(Row::new(slots).justify(SpaceBetween).width(INNER - 20.0))
}

/*
 * the week ahead in one block like the hourly strip above it, today first and lit up like "Now": the day, its weather,
 * and the high over the low
 */
const DAY_HEIGHT: f32 = 92.0;

fn week(t: &Theme, weather: &Weather) -> Column {
    let mut days: Vec<Box<dyn Widget>> = Vec::new();

    for (index, day) in weather.days.iter().enumerate() {
        let (glyph, _) = describe(day.code, true);

        let name = if index == 0 { String::from("Today") } else { day.name.clone() };

        days.push(Box::new(
            Column::new(children![
                label(name, 10.5, t.dim),
                icon(glyph, 17.0, if index == 0 { t.accent } else { t.fg }),
                Column::new(children![bold(degrees(day.high), 12.0, t.fg), label(degrees(day.low), 10.5, t.dim)])
                    .gap(2.0)
                    .align(Center),
            ])
            .gap(7.0)
            .align(Center),
        ));
    }

    let sun = Row::new(children![
        icon("\u{e34c}", 14.0, t.dim),
        label(&weather.sunrise, 11.5, t.dim),
        Rectangle::new().width(Parent).height(1.0),
        icon("\u{e34d}", 14.0, t.dim),
        label(&weather.sunset, 11.5, t.dim),
    ])
    .gap(6.0)
    .align(Center)
    .width(INNER);

    Column::new(children![
        sun,
        Rectangle::new()
            .width(INNER)
            .height(DAY_HEIGHT)
            .radius(0.0)
            .fill(t.surface)
            .padding(Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 })
            .align_child(Center, Center)
            .child(Row::new(days).justify(SpaceBetween).width(INNER - 20.0)),
    ])
    .gap(14.0)
}

// ---------------------------------------------------------------- calendar

const MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

pub fn days_in(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

// 0 = Monday
fn weekday(year: i32, month: u32, day: u32) -> u32 {
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];

    let year = if month < 3 { year - 1 } else { year };

    let sunday_first = (year + year / 4 - year / 100 + year / 400 + OFFSETS[month as usize - 1] + day as i32) % 7;

    ((sunday_first + 6) % 7) as u32
}

fn month_rows(year: i32, month: u32) -> u32 {
    (weekday(year, month, 1) + days_in(year, month)).div_ceil(7)
}

// a day of the month, with a small square under it when it has events
fn cell(t: &Theme, content: impl Widget + 'static, highlight: bool, dot: bool) -> Box<dyn Widget> {
    let base = Rectangle::new().width(INNER / 7.0).height(32.0).align_child(Center, Center);

    let mark = Rectangle::new().width(4.0).height(4.0).radius(0.0).fill(match (dot, highlight) {
        (false, _) => alpha(t.accent, 0.0),
        (true, true) => t.on_accent,
        (true, false) => t.accent,
    });

    let content = Column::new(children![content, mark]).gap(2.0).align(Center);

    if highlight {
        return Box::new(base.child(
            Rectangle::new().width(28.0).height(28.0).radius(0.0).fill(t.accent).align_child(Center, Center).child(content),
        ));
    }

    Box::new(base.child(content))
}

fn month(t: &Theme, clock: &Clock, events: &Events, year: i32, month: u32, rows: u32) -> Rectangle {
    let first = weekday(year, month, 1);
    let count = days_in(year, month);
    let start = days_from_civil(year, month, 1);

    let away = Browse::read().months != 0;

    let title = Row::new(children![
        Rectangle::new()
            .width(Parent)
            .height(30.0)
            .align_child(Start, Center)
            .cursor(if away { Cursor::Pointer } else { Cursor::Default })
            .on_click(|_| reset())
            .child(Row::new(children![bold(MONTHS[month as usize - 1], 14.0, t.fg), label(year.to_string(), 14.0, t.faint)]).gap(6.0)),
        round_button("calendar-back", t, "\u{f053}", t.dim, |_| Browse::write().months -= 1),
        round_button("calendar-on", t, "\u{f054}", t.dim, |_| Browse::write().months += 1),
    ])
    .gap(2.0)
    .align(Center)
    .width(INNER);

    let mut header: Vec<Box<dyn Widget>> = Vec::new();

    for name in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] {
        header.push(Box::new(
            Rectangle::new().width(INNER / 7.0).height(18.0).align_child(Center, Center).child(section(name, t)),
        ));
    }

    let mut grid: Vec<Box<dyn Widget>> = Vec::new();

    for row in 0..rows {
        let mut cells: Vec<Box<dyn Widget>> = Vec::new();

        for column in 0..7 {
            let index = row * 7 + column;

            if index < first || index >= first + count {
                cells.push(cell(t, label("", 12.0, t.faint), false, false));

                continue;
            }

            let day = index - first + 1;
            let today = (year, month, day) == (clock.year, clock.month, clock.day);

            let color = if today {
                t.on_accent
            } else if column >= 5 {
                alpha(t.fg, 0.55)
            } else {
                t.fg
            };

            let text = if today { bold(day.to_string(), 12.0, color) } else { label(day.to_string(), 12.0, color) };

            cells.push(cell(t, text, today, events.on(start + day as i64 - 1)));
        }

        grid.push(Box::new(Row::new(cells)));
    }

    // the wheel moves through the months too, a notch at a time on a touchpad as well
    Rectangle::new()
        .width(INNER)
        .height(30.0 + 8.0 + 18.0 + 4.0 + rows as f32 * 32.0)
        .on_scroll(|scroll| {
            let mut browse = Browse::write();

            let steps = browse.wheel.steps(scroll.y);

            browse.months += steps;
        })
        .child(Column::new(children![title, Row::new(header), Column::new(grid)]).gap(4.0))
}

// ---------------------------------------------------------------- events

const AGENDA_ROWS: usize = 4;
const AGENDA_ROW: f32 = 18.0;
const AGENDA_GAP: f32 = 8.0;

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/*
 * the next few events from the calendars in ~/.config/amane-shell/calendars (see events.rs), or the first few of the
 * month the arrows moved to. nothing at all when there are none
 */
fn agenda(t: &Theme, events: &Events, clock: &Clock, year: i32, month: u32, months: i32) -> Option<(Rectangle, f32)> {
    let today = days_from_civil(clock.year, clock.month, clock.day);

    let shown: Vec<&Event> = if months == 0 {
        // what's still to come: today's timed events drop off once their time has passed
        events
            .list
            .iter()
            .filter(|event| event.last > today || (event.last == today && event.time.as_ref().is_none_or(|time| *time >= clock.time)))
            .take(AGENDA_ROWS)
            .collect()
    } else {
        let first = days_from_civil(year, month, 1);
        let last = first + days_in(year, month) as i64 - 1;

        events.list.iter().filter(|event| event.day <= last && event.last >= first).take(AGENDA_ROWS).collect()
    };

    if shown.is_empty() {
        return None;
    }

    // a column for times only when one of them has a time, so a list of whole days has no gap
    let time_width = if shown.iter().any(|event| event.time.is_some()) { 40.0 } else { 0.0 };

    let mut rows: Vec<Box<dyn Widget>> = Vec::new();

    for event in &shown {
        let now = months == 0 && event.day <= today;

        let when = if now {
            String::from("Today")
        } else if event.day == today + 1 {
            String::from("Tomorrow")
        } else {
            let (_, month, day) = civil_from_days(event.day);

            format!("{} {day} {}", WEEKDAYS[weekday_of(event.day) as usize], &MONTHS[month as usize - 1][..3])
        };

        rows.push(Box::new(
            Row::new(children![
                Rectangle::new().width(78.0).height(AGENDA_ROW).child(if now { bold(when, 11.5, t.fg) } else { label(when, 11.5, t.dim) }),
                Rectangle::new().width(time_width).height(AGENDA_ROW).child(label(event.time.clone().unwrap_or_default(), 11.5, t.dim)),
                Rectangle::new().width(INNER - 24.0 - 78.0 - time_width - 16.0).height(AGENDA_ROW).child(label(&event.title, 12.0, t.fg).elide()),
            ])
            .gap(if time_width > 0.0 { 8.0 } else { 4.0 })
            .align(Center),
        ));
    }

    let height = shown.len() as f32 * (AGENDA_ROW + AGENDA_GAP) - AGENDA_GAP + 24.0;

    let list = Rectangle::new().width(INNER).height(height).radius(0.0).fill(t.surface).padding(12.0).child(Column::new(rows).gap(AGENDA_GAP));

    Some((list, height))
}

#[cfg(test)]
mod tests {
    use super::{days_in, month_rows, shift, weekday};

    #[test]
    fn arrows_cross_the_year() {
        assert_eq!(shift(2026, 10, 0), (2026, 10));
        assert_eq!(shift(2026, 10, 3), (2027, 1));
        assert_eq!(shift(2026, 1, -1), (2025, 12));
        assert_eq!(shift(2026, 10, -22), (2024, 12));
    }

    #[test]
    fn month_lengths() {
        assert_eq!(days_in(2023, 1), 31);
        assert_eq!(days_in(2023, 4), 30);
        assert_eq!(days_in(2023, 12), 31);
    }

    #[test]
    fn february_follows_the_leap_year_rules() {
        assert_eq!(days_in(2023, 2), 28);
        assert_eq!(days_in(2024, 2), 29);
        assert_eq!(days_in(1900, 2), 28); // divisible by 100
        assert_eq!(days_in(2000, 2), 29); // divisible by 400
    }

    #[test]
    fn weekdays_start_on_monday() {
        assert_eq!(weekday(2026, 10, 19), 0); // a Monday
        assert_eq!(weekday(1970, 1, 1), 3); // Thursday
        assert_eq!(weekday(2000, 1, 1), 5); // Saturday
        assert_eq!(weekday(2024, 2, 29), 3); // the leap day, Thursday
        assert_eq!(weekday(2024, 3, 1), 4); // and the day after it, Friday
        assert_eq!(weekday(2023, 10, 1), 6); // Sunday
    }

    #[test]
    fn rows_cover_the_month() {
        assert_eq!(month_rows(2027, 2), 4); // starts on a Monday and has 28 days: exactly four weeks
        assert_eq!(month_rows(2026, 2), 5); // the same month starting on a Sunday spills into a fifth
        assert_eq!(month_rows(2023, 10), 6); // a Sunday start and 31 days needs six
        assert_eq!(month_rows(2024, 2), 5);
    }
}
