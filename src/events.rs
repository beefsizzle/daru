use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use amane::Service;

use crate::calendar::days_in;
use crate::clock::Clock;

/*
 * events from calendars the shell can read but never change. each line of ~/.config/amane-shell/calendars is the
 * link to a calendar's .ics file, like the "Secret address in iCal format" Google Calendar gives every calendar in its
 * settings (Nextcloud, Outlook and iCloud have the same thing under other names). no Google account or API key: the
 * file is downloaded like the weather, every 15 minutes, and read here. that link is a password to the calendar, so
 * it only lives in that file (chmod 600 it), and it's handed to curl on stdin, never on its command line where `ps`
 * would show it to anyone on the machine
 */
#[derive(Clone, PartialEq)]
pub struct Event {
    pub day: i64,             // days since 1 January 1970, local time
    pub last: i64,            // the last day it covers, the same as `day` unless it's an all-day event over several
    pub time: Option<String>, // "14:00" local, none for an all-day event
    pub title: String,
}

#[derive(Default)]
pub struct Events {
    pub list: Vec<Event>, // by day, then time, all-day ones first
}

impl Events {
    pub fn on(&self, day: i64) -> bool {
        self.list.iter().any(|event| event.day <= day && day <= event.last)
    }
}

impl Service for Events {
    fn new() -> Self {
        Self::default()
    }

    /*
     * the downloads run before the lock is taken, like the weather's. each calendar keeps what it last read, so one
     * that can't be reached for a while (the network is down, Google is slow) doesn't empty the panel
     */
    fn listen() {
        let mut read: HashMap<String, Vec<Event>> = HashMap::new();

        loop {
            let links = links();

            let mut failed = false;

            for link in &links {
                match download(link) {
                    Some(text) => {
                        let clock = Clock::read();

                        let today = days_from_civil(clock.year, clock.month, clock.day);

                        drop(clock);

                        read.insert(link.clone(), events(&text, today));
                    }
                    None => failed = true,
                }
            }

            read.retain(|link, _| links.contains(link));

            let mut list: Vec<Event> = read.values().flatten().cloned().collect();

            list.sort_by(|a, b| (a.day, &a.time).cmp(&(b.day, &b.time)));

            if list != Self::read().list {
                Self::write().list = list;
            }

            // with no calendars set up, look for the file every minute so a new one shows up soon
            let wait = if links.is_empty() || failed { 60 } else { 900 };

            std::thread::sleep(Duration::from_secs(wait));
        }
    }
}

fn links() -> Vec<String> {
    let text = std::fs::read_to_string(crate::cache::config_dir().join("calendars")).unwrap_or_default();

    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        // Apple's webcal:// links are https underneath
        .map(|line| line.strip_prefix("webcal://").map_or(line.to_string(), |rest| format!("https://{rest}")))
        // they go inside quotes in curl's config, so nothing that could end the quotes early
        .filter(|link| link.starts_with("https://") && !link.contains(|letter: char| letter == '"' || letter == '\\' || letter.is_whitespace() || letter.is_control()))
        .collect()
}

fn download(link: &str) -> Option<String> {
    let mut curl = Command::new("curl")
        .args(["-fsSL", "--max-time", "30", "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    curl.stdin.take()?.write_all(format!("url = \"{link}\"\n").as_bytes()).ok()?;

    let output = curl.wait_with_output().ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout).into_owned();

    text.contains("BEGIN:VCALENDAR").then_some(text)
}

// ---------------------------------------------------------------- reading .ics

// how far around today repeating events are worked out: the months the arrows will reasonably reach
const BEFORE: i64 = 400;
const AFTER: i64 = 400;

// a date, or a date and time, as the file wrote it: in its own time zone, not yet in yours
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Stamp {
    day: i64,
    minute: Option<u32>, // minutes past midnight, none for a whole day
}

#[derive(Clone, PartialEq)]
enum Zone {
    Local, // "floating" times and whole days, the same wherever you are
    Utc,
    Named(String), // like Europe/Oslo
}

#[derive(Default)]
struct Raw {
    uid: String,
    title: String,
    start: Option<(Stamp, Zone)>,
    end: Option<Stamp>,
    rule: Option<String>,
    skipped: Vec<Stamp>,     // EXDATE: occurrences of a repeating event that were deleted
    replaces: Option<Stamp>, // RECURRENCE-ID: this one is a moved or changed occurrence of a repeating event
    cancelled: bool,
}

// the events in one .ics file, those around today, in your own time zone
fn events(text: &str, today: i64) -> Vec<Event> {
    let raws = parse(text);

    // a changed occurrence is its own VEVENT, and the repeating one has to leave that occurrence out
    let mut replaced: HashMap<&str, HashSet<Stamp>> = HashMap::new();

    for raw in &raws {
        if let Some(stamp) = raw.replaces {
            replaced.entry(raw.uid.as_str()).or_default().insert(stamp);
        }
    }

    let first = today - BEFORE;
    let last = today + AFTER;

    // each occurrence, and the zone its time is in
    let mut found: Vec<(Stamp, i64, Zone, &str)> = Vec::new();

    for raw in &raws {
        let Some((start, zone)) = &raw.start else {
            continue;
        };

        if raw.cancelled {
            continue;
        }

        // whole days the event covers past its first, for a holiday or a trip over several days
        let extra = match (start.minute, raw.end) {
            (None, Some(end)) => (end.day - start.day - 1).max(0),
            _ => 0,
        };

        let skipped: HashSet<Stamp> = raw
            .skipped
            .iter()
            .copied()
            .chain(if raw.replaces.is_none() { replaced.get(raw.uid.as_str()).into_iter().flatten().copied().collect() } else { Vec::new() })
            .collect();

        let days = match (&raw.rule, raw.replaces) {
            (Some(rule), None) => repeat(start.day, rule, first - extra, last),
            _ => vec![start.day],
        };

        for day in days {
            let stamp = Stamp { day, minute: start.minute };

            if skipped.contains(&stamp) || day + extra < first || day > last {
                continue;
            }

            found.push((stamp, extra, zone.clone(), raw.title.as_str()));
        }
    }

    let local = to_local(&found.iter().map(|(stamp, _, zone, _)| (*stamp, zone.clone())).collect::<Vec<_>>());

    found
        .iter()
        .zip(local)
        .map(|((_, extra, _, title), stamp)| Event {
            day: stamp.day,
            last: stamp.day + extra,
            time: stamp.minute.map(|minute| format!("{:02}:{:02}", minute / 60, minute % 60)),
            title: if title.is_empty() { String::from("(no title)") } else { title.to_string() },
        })
        .collect()
}

fn parse(text: &str) -> Vec<Raw> {
    let mut raws = Vec::new();
    let mut current: Option<Raw> = None;

    // how deep inside an event's own parts (its alarms) a line is, whose SUMMARY isn't the event's
    let mut inside = 0;

    // a long line goes on in the next ones, each starting with a space or a tab
    let mut lines: Vec<String> = Vec::new();

    for line in text.lines() {
        let line = line.trim_end_matches('\r');

        match (line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')), lines.last_mut()) {
            (Some(rest), Some(previous)) => previous.push_str(rest),
            _ => lines.push(line.to_string()),
        }
    }

    for line in &lines {
        let Some((name, parameters, value)) = split(line) else {
            continue;
        };

        if name == "BEGIN" && value == "VEVENT" {
            current = Some(Raw::default());

            continue;
        }

        if name == "END" && value == "VEVENT" {
            raws.extend(current.take());

            inside = 0;

            continue;
        }

        // properties of anything other than an event (its alarms, the time zone definitions) don't count
        let Some(raw) = current.as_mut() else {
            continue;
        };

        match name {
            "BEGIN" => inside += 1,
            "END" => inside -= 1,
            _ => {}
        }

        if inside > 0 || name == "END" {
            continue;
        }

        match name {
            "UID" => raw.uid = value.to_string(),
            "SUMMARY" => raw.title = unescape(value),
            "DTSTART" => raw.start = stamp(parameters, value),
            "DTEND" => raw.end = stamp(parameters, value).map(|(stamp, _)| stamp),
            "RRULE" => raw.rule = Some(value.to_string()),
            "EXDATE" => raw.skipped.extend(value.split(',').filter_map(|one| stamp(parameters, one)).map(|(stamp, _)| stamp)),
            "RECURRENCE-ID" => raw.replaces = stamp(parameters, value).map(|(stamp, _)| stamp),
            "STATUS" => raw.cancelled = value == "CANCELLED",
            _ => {}
        }
    }

    raws
}

// "DTSTART;TZID=Europe/Oslo:20261030T140000" into its name, its parameters and its value
fn split(line: &str) -> Option<(&str, &str, &str)> {
    let mut quoted = false;

    // a quoted parameter may hold a colon of its own
    let colon = line.char_indices().find(|(_, letter)| {
        if *letter == '"' {
            quoted = !quoted;
        }

        *letter == ':' && !quoted
    })?;

    let (head, value) = (&line[..colon.0], &line[colon.0 + 1..]);

    let (name, parameters) = head.split_once(';').unwrap_or((head, ""));

    Some((name, parameters, value))
}

// "20261030", "20261030T140000Z", or "20261030T140000" in the zone its TZID parameter names
fn stamp(parameters: &str, value: &str) -> Option<(Stamp, Zone)> {
    let number = |range: std::ops::Range<usize>| value.get(range)?.parse::<u32>().ok();

    let day = days_from_civil(number(0..4)? as i32, number(4..6)?, number(6..8)?);

    if value.len() == 8 {
        return Some((Stamp { day, minute: None }, Zone::Local));
    }

    if value.get(8..9) != Some("T") {
        return None;
    }

    let minute = Some(number(9..11)? * 60 + number(11..13)?);

    let zone = if value.ends_with('Z') {
        Zone::Utc
    } else {
        parameters
            .split(';')
            .find_map(|parameter| parameter.strip_prefix("TZID="))
            .map(|name| name.trim_matches('"'))
            .filter(|name| known_zone(name))
            .map_or(Zone::Local, |name| Zone::Named(name.to_string()))
    };

    Some((Stamp { day, minute }, zone))
}

// only names `date` will understand, so one odd line can't throw off the answers for all the others
fn known_zone(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("..")
        && name.chars().all(|letter| letter.is_ascii_alphanumeric() || "_+-/".contains(letter))
        && std::path::Path::new("/usr/share/zoneinfo").join(name).is_file()
}

fn unescape(text: &str) -> String {
    let mut plain = String::new();
    let mut letters = text.chars();

    while let Some(letter) = letters.next() {
        if letter != '\\' {
            plain.push(letter);

            continue;
        }

        match letters.next() {
            Some('n' | 'N') => plain.push(' '),
            Some(other) => plain.push(other),
            None => {}
        }
    }

    plain
}

/*
 * times in UTC or another time zone, moved into yours. `date` knows every zone and when each one changes for summer,
 * so all of them go through one `date -f -`, a line each. whole days and floating times stay as they are, and if
 * `date` doesn't answer one line for each it was given, everything stays as written rather than risk a mix-up
 */
fn to_local(stamps: &[(Stamp, Zone)]) -> Vec<Stamp> {
    let mut asked: Vec<usize> = Vec::new();
    let mut input = String::new();

    for (index, (stamp, zone)) in stamps.iter().enumerate() {
        let (Some(minute), name) = (stamp.minute, match zone {
            Zone::Local => continue,
            Zone::Utc => "UTC",
            Zone::Named(name) => name.as_str(),
        }) else {
            continue;
        };

        let (year, month, day) = civil_from_days(stamp.day);

        input.push_str(&format!("TZ=\"{name}\" {year:04}-{month:02}-{day:02} {:02}:{:02}\n", minute / 60, minute % 60));

        asked.push(index);
    }

    let mut local: Vec<Stamp> = stamps.iter().map(|(stamp, _)| *stamp).collect();

    if asked.is_empty() {
        return local;
    }

    let answers = run_date(&input);

    if answers.len() != asked.len() {
        return local;
    }

    for (index, answer) in asked.into_iter().zip(answers) {
        if let Some((stamp, _)) = stamp("", &answer) {
            local[index] = stamp;
        }
    }

    local
}

// "20261030T140000" for each line, in the order they were given
fn run_date(input: &str) -> Vec<String> {
    let Ok(mut date) = Command::new("date")
        .args(["-f", "-", "+%Y%m%dT%H%M00"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };

    // written from another thread: with many lines, `date` would fill its output pipe and wait for us to read it
    let Some(mut stdin) = date.stdin.take() else {
        return Vec::new();
    };

    let input = input.to_string();

    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });

    let output = date.wait_with_output();

    let _ = writer.join();

    output.map(|output| String::from_utf8_lossy(&output.stdout).lines().map(String::from).collect()).unwrap_or_default()
}

// ---------------------------------------------------------------- repeating events

/*
 * the days a repeating event falls on, from its rule ("FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20270101T000000Z"), up to
 * `last`. it counts from the event's first day, since COUNT includes the occurrences before `first` too. the rules
 * calendars actually write are covered: every n days, weeks on some weekdays, months on a date or on "the second
 * Tuesday" or "the last Friday", and years. anything rarer falls back to the plainest reading of its FREQ
 */
fn repeat(start: i64, rule: &str, first: i64, last: i64) -> Vec<i64> {
    let mut frequency = "";
    let mut interval: i64 = 1;
    let mut count: Option<usize> = None;
    let mut until: Option<i64> = None;
    let mut weekdays: Vec<(i32, u32)> = Vec::new(); // (which one in the month, 0 for all; weekday, 0 = Monday)
    let mut month_days: Vec<i32> = Vec::new();

    for part in rule.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };

        match key {
            "FREQ" => frequency = value,
            "INTERVAL" => interval = value.parse().unwrap_or(1).max(1),
            "COUNT" => count = value.parse().ok(),
            "UNTIL" => until = stamp("", value).map(|(stamp, _)| stamp.day),
            "BYDAY" => weekdays = value.split(',').filter_map(weekday_rule).collect(),
            "BYMONTHDAY" => month_days = value.split(',').filter_map(|day| day.parse().ok()).collect(),
            _ => {}
        }
    }

    let end = until.map_or(last, |until| until.min(last));

    let mut days = Vec::new();
    let mut made = 0;

    // every day the rule lands on, in order, until it ends or passes `end`
    let mut keep = |day: i64, days: &mut Vec<i64>| -> bool {
        if day < start {
            return true;
        }

        if day > end || count.is_some_and(|count| made >= count) {
            return false;
        }

        made += 1;

        if day >= first {
            days.push(day);
        }

        true
    };

    let (year, month, date) = civil_from_days(start);

    // far more than 800 days around today ever needs, for an event that started long ago
    for step in 0..20_000i64 {
        let more = match frequency {
            "DAILY" => keep(start + step * interval, &mut days),
            "WEEKLY" => {
                let monday = start - weekday_of(start) as i64 + step * interval * 7;

                let mut on: Vec<u32> = weekdays.iter().map(|(_, weekday)| *weekday).collect();

                if on.is_empty() {
                    on.push(weekday_of(start));
                }

                on.sort_unstable();
                on.dedup();

                on.into_iter().all(|weekday| keep(monday + weekday as i64, &mut days))
            }
            "MONTHLY" => {
                let months = (month as i64 - 1) + step * interval;
                let (year, month) = (year + months.div_euclid(12) as i32, months.rem_euclid(12) as u32 + 1);

                let mut on = in_month(year, month, &weekdays, &month_days, date);

                on.sort_unstable();

                on.into_iter().all(|day| keep(day, &mut days))
            }
            "YEARLY" => {
                let year = year + (step * interval) as i32;

                // the 29th of February only comes back in leap years
                if date > days_in(year, month) {
                    true
                } else {
                    keep(days_from_civil(year, month, date), &mut days)
                }
            }
            // a rule this doesn't know (every hour, say) shows its first time only
            _ => {
                keep(start, &mut days);

                false
            }
        };

        if !more {
            break;
        }
    }

    days
}

// "MO", "2TU", "-1FR"
fn weekday_rule(text: &str) -> Option<(i32, u32)> {
    let split = text.len().checked_sub(2)?;

    let weekday = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"].iter().position(|name| Some(*name) == text.get(split..))? as u32;

    let which = if split == 0 { 0 } else { text.get(..split)?.parse().ok()? };

    Some((which, weekday))
}

// the days in one month a monthly rule picks
fn in_month(year: i32, month: u32, weekdays: &[(i32, u32)], month_days: &[i32], date: u32) -> Vec<i64> {
    let length = days_in(year, month);
    let first = days_from_civil(year, month, 1);

    if !weekdays.is_empty() {
        let mut days = Vec::new();

        for (which, weekday) in weekdays {
            // every one of that weekday in the month
            let all: Vec<i64> =
                (0..length as i64).map(|offset| first + offset).filter(|day| weekday_of(*day) == *weekday).collect();

            match *which {
                0 => days.extend(all),
                which if which > 0 => days.extend(all.get(which as usize - 1)),
                which => days.extend(all.len().checked_sub(which.unsigned_abs() as usize).and_then(|index| all.get(index))),
            }
        }

        return days;
    }

    let wanted: Vec<i32> = if month_days.is_empty() { vec![date as i32] } else { month_days.to_vec() };

    wanted
        .into_iter()
        .filter_map(|day| match day {
            1.. if day as u32 <= length => Some(day as u32),
            ..=-1 if day.unsigned_abs() <= length => Some(length + 1 - day.unsigned_abs()),
            _ => None,
        })
        .map(|day| days_from_civil(year, month, day))
        .collect()
}

// ---------------------------------------------------------------- days

// days since 1 January 1970, the other way round from `civil_from_days` (Howard Hinnant's algorithm)
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year } as i64;
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * ((month as i64 + 9) % 12) + 2) / 5 + day as i64 - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;

    era * 146_097 + of_cycle - 719_468
}

pub fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days - era * 146_097;
    let of_cycle = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * of_cycle + of_cycle / 4 - of_cycle / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = (of_year - (153 * shifted + 2) / 5 + 1) as u32;
    let month = if shifted < 10 { shifted + 3 } else { shifted - 9 } as u32;
    let year = (of_cycle + era * 400) as i32 + i32::from(month <= 2);

    (year, month, day)
}

// 0 = Monday. 1 January 1970 was a Thursday
pub fn weekday_of(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, date: u32) -> i64 {
        days_from_civil(year, month, date)
    }

    #[test]
    fn days_round_trip() {
        assert_eq!(day(1970, 1, 1), 0);
        assert_eq!(day(2000, 3, 1), 11_017);
        assert_eq!(weekday_of(day(2026, 10, 30)), 4); // a Friday

        for days in [-1000, 0, 59, 60, 11_016, 20_000, 30_000] {
            let (year, month, date) = civil_from_days(days);

            assert_eq!(day(year, month, date), days);
        }
    }

    #[test]
    fn weekly_on_some_weekdays() {
        let start = day(2026, 10, 5); // a Monday

        let days = repeat(start, "FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4", start, start + 100);

        assert_eq!(days, vec![start, start + 2, start + 7, start + 9]);
    }

    #[test]
    fn every_other_week_until_a_date() {
        let start = day(2026, 10, 1);

        let days = repeat(start, "FREQ=WEEKLY;INTERVAL=2;UNTIL=20261030T000000Z", start, start + 100);

        assert_eq!(days, vec![start, start + 14, start + 28]);
    }

    #[test]
    fn monthly_on_the_last_friday_and_the_second_tuesday() {
        let start = day(2026, 1, 1);

        let last_friday = repeat(start, "FREQ=MONTHLY;BYDAY=-1FR;COUNT=2", start, start + 400);

        assert_eq!(last_friday, vec![day(2026, 1, 30), day(2026, 2, 27)]);

        let second_tuesday = repeat(start, "FREQ=MONTHLY;BYDAY=2TU;COUNT=2", start, start + 400);

        assert_eq!(second_tuesday, vec![day(2026, 1, 13), day(2026, 2, 10)]);
    }

    #[test]
    fn the_31st_skips_shorter_months() {
        let start = day(2026, 1, 31);

        let days = repeat(start, "FREQ=MONTHLY;COUNT=3", start, start + 400);

        assert_eq!(days, vec![day(2026, 1, 31), day(2026, 3, 31), day(2026, 5, 31)]);
    }

    #[test]
    fn a_birthday_on_the_leap_day() {
        let start = day(2024, 2, 29);

        let days = repeat(start, "FREQ=YEARLY", start, day(2033, 1, 1));

        assert_eq!(days, vec![day(2024, 2, 29), day(2028, 2, 29), day(2032, 2, 29)]);
    }

    #[test]
    fn count_includes_occurrences_before_the_window() {
        let start = day(2026, 1, 1);

        let days = repeat(start, "FREQ=DAILY;COUNT=10", start + 5, start + 100);

        assert_eq!(days, (5..10).map(|offset| start + offset).collect::<Vec<_>>());
    }

    const FILE: &str = "BEGIN:VCALENDAR\r
BEGIN:VEVENT\r
UID:weekly\r
DTSTART;VALUE=DATE:20261102\r
DTEND;VALUE=DATE:20261103\r
RRULE:FREQ=WEEKLY;COUNT=3\r
EXDATE;VALUE=DATE:20261109\r
SUMMARY:Bins out\\, both\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:trip\r
DTSTART;VALUE=DATE:20261120\r
DTEND;VALUE=DATE:20261123\r
SUMMARY:A trip with a very long\r
  name\r
BEGIN:VALARM\r
SUMMARY:not the event's\r
END:VALARM\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:gone\r
DTSTART;VALUE=DATE:20261105\r
STATUS:CANCELLED\r
SUMMARY:Cancelled\r
END:VEVENT\r
END:VCALENDAR\r
";

    #[test]
    fn reads_whole_day_events() {
        let today = day(2026, 10, 30);

        let mut events = events(FILE, today);

        events.sort_by_key(|event| event.day);

        let titles: Vec<(&str, i64, i64)> =
            events.iter().map(|event| (event.title.as_str(), event.day - today, event.last - today)).collect();

        // the 9th was deleted, the trip covers three days, the cancelled one is gone
        assert_eq!(titles, vec![("Bins out, both", 3, 3), ("Bins out, both", 17, 17), ("A trip with a very long name", 21, 23)]);

        assert!(events.iter().all(|event| event.time.is_none()));
    }

    #[test]
    fn a_moved_occurrence_replaces_the_original() {
        let file = "BEGIN:VEVENT
UID:standup
DTSTART:20261102T090000
RRULE:FREQ=DAILY;COUNT=3
SUMMARY:Standup
END:VEVENT
BEGIN:VEVENT
UID:standup
RECURRENCE-ID:20261103T090000
DTSTART:20261103T110000
SUMMARY:Standup, later
END:VEVENT
";
        let today = day(2026, 11, 1);

        let mut events = events(file, today);

        events.sort_by(|a, b| (a.day, &a.time).cmp(&(b.day, &b.time)));

        let seen: Vec<(i64, &str, &str)> =
            events.iter().map(|event| (event.day - today, event.time.as_deref().unwrap_or(""), event.title.as_str())).collect();

        assert_eq!(seen, vec![(1, "09:00", "Standup"), (2, "11:00", "Standup, later"), (3, "09:00", "Standup")]);
    }

    #[test]
    fn utc_times_move_into_the_local_zone() {
        let stamps = [
            (Stamp { day: day(2026, 7, 1), minute: Some(12 * 60) }, Zone::Named(String::from("Asia/Tokyo"))),
            (Stamp { day: day(2026, 7, 1), minute: Some(23 * 60) }, Zone::Utc),
        ];

        let local = to_local(&stamps);

        // in whatever zone the tests run, both are moved by a whole number of quarter hours
        assert_eq!(local.len(), 2);
        assert!(local.iter().all(|stamp| stamp.minute.is_some_and(|minute| minute % 15 == 0)));

        // noon in Tokyo is 03:00 UTC: never later than 23:00 UTC the same day, wherever you are
        assert!((local[0].day, local[0].minute) < (local[1].day, local[1].minute));
    }

    #[test]
    fn odd_zone_names_are_read_as_local() {
        assert!(!known_zone("../../etc/passwd"));
        assert!(!known_zone("W. Europe Standard Time"));
        assert!(known_zone("UTC"));
    }
}
