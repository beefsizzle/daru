use amane::{Center, Column, Horizontal, LayerWindow, Monitor, Padding, Parent, Rectangle, Row, Service, SpaceBetween, Widget, children};

use crate::clock::Clock;
use crate::panels::{self, Which};
use crate::theme::{Theme, alpha};
use crate::ui::{bold, icon, label, section};
use crate::weather::{Weather, degrees, describe};

const WIDTH: f32 = 372.0;
const INNER: f32 = WIDTH - 28.0;

pub fn view(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::Calendar, monitor) else {
        return panels::hidden("amane-panel");
    };
    let weather = Weather::read();
    let clock = Clock::read();

    let rows = month_rows(clock.year, clock.month);

    let calendar_height = 24.0 + 8.0 + 18.0 + 4.0 + rows as f32 * 32.0;

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

    sections.push(Box::new(month(&t, &clock, rows)));

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

fn days_in(year: i32, month: u32) -> u32 {
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

fn cell(t: &Theme, content: impl Widget + 'static, highlight: bool) -> Box<dyn Widget> {
    let base = Rectangle::new().width(INNER / 7.0).height(32.0).align_child(Center, Center);

    if highlight {
        return Box::new(base.child(
            Rectangle::new().width(28.0).height(28.0).radius(0.0).fill(t.accent).align_child(Center, Center).child(content),
        ));
    }

    Box::new(base.child(content))
}

fn month(t: &Theme, clock: &Clock, rows: u32) -> Column {
    let first = weekday(clock.year, clock.month, 1);
    let count = days_in(clock.year, clock.month);

    let title = Row::new(children![
        bold(MONTHS[clock.month as usize - 1], 14.0, t.fg),
        label(clock.year.to_string(), 14.0, t.faint),
    ])
    .gap(6.0)
    .align(Center)
    .height(24.0);

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
                cells.push(cell(t, label("", 12.0, t.faint), false));

                continue;
            }

            let day = index - first + 1;
            let today = day == clock.day;

            let color = if today {
                t.on_accent
            } else if column >= 5 {
                alpha(t.fg, 0.55)
            } else {
                t.fg
            };

            let text = if today { bold(day.to_string(), 12.0, color) } else { label(day.to_string(), 12.0, color) };

            cells.push(cell(t, text, today));
        }

        grid.push(Box::new(Row::new(cells)));
    }

    Column::new(children![title, Row::new(header), Column::new(grid)]).gap(4.0)
}

#[cfg(test)]
mod tests {
    use super::{days_in, month_rows, weekday};

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
