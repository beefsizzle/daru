use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use amane::Service;

use crate::panels::{self, Which};

/*
 * the bar's battery icon (see bar.rs), from amane's Battery service, which reads /sys/class/power_supply every 5 s,
 * and what the battery dropdown (batpanel.rs) shows beyond the charge: the charge over the last two hours, how fast
 * it charges or runs down, and how worn it is
 */

const POWER_SUPPLIES: &str = "/sys/class/power_supply";

// the charge is noted once a minute, and two hours of it kept
const SAMPLE_EVERY: Duration = Duration::from_secs(60);
pub const SAMPLES: usize = 120;

// the draw on battery is the charge used over up to the last ten minutes, and needs five to say anything
const DRAW_OVER: Duration = Duration::from_secs(10 * 60);
const DRAW_AFTER: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, PartialEq)]
pub struct Sample {
    pub at: Instant,
    pub percent: u8,
    pub energy: Option<f64>, // Wh left
    pub draining: bool,      // on battery, as opposed to charging or plugged in
    pub after_sleep: bool,   // the first since the computer was suspended, see `History::listen`
}

/*
 * the charge once a minute, for the graph and the draw. this runs whether the dropdown is open or not, so both are
 * there the moment it opens: a few file reads a minute, and 120 samples of memory
 */
#[derive(Default)]
pub struct History {
    pub samples: VecDeque<Sample>, // oldest first
}

impl Service for History {
    fn new() -> Self {
        Self::default()
    }

    /*
     * `at` is an Instant, which doesn't count time spent suspended, so a night asleep would look like one minute and
     * the charge lost over it like a huge draw. the wall clock does count it: a sample more than two minutes after the
     * last by the wall clock follows a suspend, and the draw starts again from it
     */
    fn listen() {
        let Some(path) = find() else { return };

        let mut last: Option<SystemTime> = None;

        loop {
            let status = read(&path, "status");

            let wall = SystemTime::now();
            let after_sleep = last.is_some_and(|last| wall.duration_since(last).unwrap_or_default() > 2 * SAMPLE_EVERY);

            last = Some(wall);

            let sample = Sample {
                at: Instant::now(),
                percent: number(&path, "capacity").map_or(0, |percent| percent.clamp(0.0, 100.0) as u8),
                energy: watt_hours(&path, "now"),
                draining: status == "Discharging",
                after_sleep,
            };

            {
                let mut history = Self::write();

                history.samples.push_back(sample);

                while history.samples.len() > SAMPLES {
                    history.samples.pop_front();
                }
            }

            std::thread::sleep(SAMPLE_EVERY);
        }
    }
}

/*
 * what the battery uses, in watts, and over how long, from the charge it lost while it ran down without a break
 * (up to DRAW_OVER back). None until it has been on battery for DRAW_AFTER.
 *
 * the kernel's own power_now isn't used on battery: it can't be trusted everywhere. one laptop's aftermarket battery
 * never reads below its charge divided by ten hours, so above about 45% it says ~5 W at idle against a real 2.6-2.9 W
 * (worked out from the charge used over time). charging, power_now is real, see `Details`
 */
pub fn draw(samples: &VecDeque<Sample>) -> Option<(f32, Duration)> {
    let newest = samples.back().filter(|sample| sample.draining)?;

    let mut oldest = None;

    for sample in samples.iter().rev() {
        if !sample.draining || sample.energy.is_none() || newest.at.duration_since(sample.at) > DRAW_OVER {
            break;
        }

        oldest = Some(sample);

        // what it lost while asleep isn't what it draws awake
        if sample.after_sleep {
            break;
        }
    }

    let oldest = oldest?;

    let span = newest.at.duration_since(oldest.at);

    if span < DRAW_AFTER {
        return None;
    }

    let used = oldest.energy? - newest.energy?;

    Some(((used / (span.as_secs_f64() / 3600.0)).max(0.0) as f32, span))
}

/*
 * what the kernel says beyond the charge, read every two seconds while the battery dropdown is open and not at all
 * otherwise. each is None when the battery doesn't say
 */
#[derive(Default, Clone, PartialEq)]
pub struct Details {
    pub status: String,        // Charging, Discharging, Full or "Not charging"
    pub charging: Option<f32>, // watts going in, only while it charges
    pub now: Option<f64>,      // Wh left
    pub full: Option<f64>,     // Wh it holds when full
    pub design: Option<f64>,   // Wh it held new
    pub cycles: Option<u32>,   // full charges so far; 0 means the battery doesn't count them
    pub volts: Option<f32>,
}

impl Details {
    /// how much it holds when full, as a percent of what it held new
    pub fn health(&self) -> Option<u8> {
        self.full.zip(self.design).filter(|(_, new)| *new > 0.0).map(|(full, new)| (full / new * 100.0).round().min(255.0) as u8)
    }
}

impl Service for Details {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {
        let Some(path) = find() else { return };

        loop {
            panels::wait_until_open(&[Which::Battery]);

            let fresh = read_details(&path);

            if fresh != *Self::read() {
                *Self::write() = fresh;
            }

            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

// the same battery amane's Battery service picks: the first that powers the computer, not a mouse's
fn find() -> Option<PathBuf> {
    fs::read_dir(POWER_SUPPLIES).ok()?.flatten().map(|entry| entry.path()).find(|path| read(path, "type") == "Battery" && read(path, "scope") != "Device")
}

fn read(folder: &Path, name: &str) -> String {
    fs::read_to_string(folder.join(name)).unwrap_or_default().trim().to_string()
}

fn number(folder: &Path, name: &str) -> Option<f64> {
    read(folder, name).parse().ok()
}

/*
 * energy_<which> in Wh. batteries report in energy (µWh) or in charge (µAh); a charge is turned into energy at the
 * battery's rated voltage, which is what makers do for the Wh printed on the battery
 */
fn watt_hours(path: &Path, which: &str) -> Option<f64> {
    number(path, &format!("energy_{which}"))
        .or_else(|| number(path, &format!("charge_{which}")).zip(number(path, "voltage_min_design")).map(|(charge, volts)| charge * volts / 1e6))
        .map(|microwatt_hours| microwatt_hours / 1e6)
}

fn read_details(path: &Path) -> Details {
    let status = read(path, "status");

    // power_now, or current times voltage on a battery that counts charge
    let charging = number(path, "power_now")
        .or_else(|| number(path, "current_now").zip(number(path, "voltage_now")).map(|(current, voltage)| current * voltage / 1e6))
        .map(|microwatts| (microwatts / 1e6) as f32)
        .filter(|watts| status == "Charging" && *watts > 0.0);

    Details {
        charging,
        now: watt_hours(path, "now"),
        full: watt_hours(path, "full"),
        design: watt_hours(path, "full_design"),
        cycles: number(path, "cycle_count").map(|count| count as u32).filter(|count| *count > 0),
        volts: number(path, "voltage_now").map(|microvolts| (microvolts / 1e6) as f32).filter(|volts| *volts > 0.0),
        status,
    }
}

/// at or below this a battery that's running down shows in red
pub const LOW: u8 = 15;

/// the icon for a charge from 0 to 100: the level in tenths while it runs down, a bolt in it while it charges, and a
/// plug when it's plugged in and full
pub fn glyph(percent: u8, charging: bool, full: bool) -> &'static str {
    if full {
        return "\u{f06a5}";
    }

    if charging {
        // the font has no charging icon for 50 or 70
        return match percent {
            95.. => "\u{f0085}",
            90..95 => "\u{f008b}",
            80..90 => "\u{f008a}",
            60..80 => "\u{f0089}",
            40..60 => "\u{f0088}",
            30..40 => "\u{f0087}",
            20..30 => "\u{f0086}",
            _ => "\u{f089c}",
        };
    }

    const TENTHS: [&str; 9] = [
        "\u{f007a}", "\u{f007b}", "\u{f007c}", "\u{f007d}", "\u{f007e}", "\u{f007f}", "\u{f0080}", "\u{f0081}", "\u{f0082}",
    ];

    match percent {
        95.. => "\u{f0079}",
        10..95 => TENTHS[usize::from(percent / 10) - 1],
        _ => "\u{f0083}",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    use super::{Sample, draw, glyph};

    // a sample a minute, from oldest to newest: (Wh left, on battery)
    fn minutes(readings: &[(f64, bool)]) -> VecDeque<Sample> {
        let start = Instant::now();

        readings
            .iter()
            .enumerate()
            .map(|(minute, &(energy, draining))| Sample { at: start + Duration::from_secs(60 * minute as u64), percent: 50, energy: Some(energy), draining, after_sleep: false })
            .collect()
    }

    #[test]
    fn the_draw_is_the_charge_used_over_the_last_ten_minutes() {
        // 0.05 Wh a minute is 3 W; the minutes before the last ten don't count
        let mut readings = vec![(40.0, true); 5];

        readings.extend((0..=10).map(|minute| (30.0 - 0.05 * f64::from(minute), true)));

        let (watts, over) = draw(&minutes(&readings)).unwrap();

        assert!((watts - 3.0).abs() < 0.01);
        assert_eq!(over, Duration::from_secs(600));
    }

    #[test]
    fn the_draw_waits_for_five_minutes_on_battery() {
        // four minutes since it was unplugged
        let readings = [(30.0, false), (30.0, false), (30.0, true), (29.95, true), (29.9, true), (29.85, true), (29.8, true)];

        assert!(draw(&minutes(&readings)).is_none());

        // and says nothing while it charges
        assert!(draw(&minutes(&[(30.0, false); 12])).is_none());
    }

    #[test]
    fn the_draw_starts_again_after_a_suspend() {
        // 3 W for ten minutes, a night asleep that cost 4 Wh but looks like one minute, then 3 W for six more
        let mut readings: Vec<(f64, bool)> = (0..=10).map(|minute| (40.0 - 0.05 * f64::from(minute), true)).collect();

        readings.extend((0..=6).map(|minute| (35.5 - 0.05 * f64::from(minute), true)));

        let mut samples = minutes(&readings);

        samples[11].after_sleep = true;

        let (watts, over) = draw(&samples).unwrap();

        assert!((watts - 3.0).abs() < 0.01);
        assert_eq!(over, Duration::from_secs(360));

        // and says nothing for its first five minutes awake
        samples.truncate(15);

        assert!(draw(&samples).is_none());
    }

    #[test]
    fn running_down_shows_the_level_in_tenths() {
        assert_eq!(glyph(100, false, false), "\u{f0079}");
        assert_eq!(glyph(57, false, false), "\u{f007e}");
        assert_eq!(glyph(10, false, false), "\u{f007a}");
    }

    #[test]
    fn nearly_empty_shows_a_warning() {
        assert_eq!(glyph(9, false, false), "\u{f0083}");
        assert_eq!(glyph(0, false, false), "\u{f0083}");
    }

    #[test]
    fn charging_shows_a_bolt_and_full_on_the_charger_a_plug() {
        assert_eq!(glyph(99, true, false), "\u{f0085}");
        assert_eq!(glyph(55, true, false), "\u{f0088}");
        assert_eq!(glyph(3, true, false), "\u{f089c}");
        assert_eq!(glyph(100, false, true), "\u{f06a5}");
    }
}
