use std::time::Duration;

use amane::Service;

pub struct Hour {
    pub time: String,
    pub temp: f32,
    pub code: u32,
    pub day: bool,
}

pub struct Day {
    pub name: String,
    pub code: u32,
    pub high: f32,
    pub low: f32,
}

#[derive(Default)]
pub struct Weather {
    pub ready: bool,
    pub temp: f32,
    pub feels: f32,
    pub humidity: u32,
    pub code: u32,
    pub wind: f32,
    pub day: bool,
    pub hours: Vec<Hour>,
    pub days: Vec<Day>,
    pub sunrise: String,
    pub sunset: String,
}

// "latitude longitude" (like "48.85 2.35") in ~/.config/amane-shell/location, so coordinates never end up in the code
fn location() -> Option<(String, String)> {
    let text = std::fs::read_to_string(crate::cache::config_dir().join("location")).ok()?;

    let mut parts = text.split_whitespace();

    // numbers only: these end up inside a shell command
    let latitude: f64 = parts.next()?.parse().ok()?;
    let longitude: f64 = parts.next()?.parse().ok()?;

    Some((format!("{latitude:.4}"), format!("{longitude:.4}")))
}

const QUERY: &str = r#"
. as $r | $r.current as $c |
"C|\($c.temperature_2m)|\($c.apparent_temperature)|\($c.relative_humidity_2m)|\($c.weather_code)|\($c.wind_speed_10m)|\($c.is_day)",
( ($r.hourly.time | map(. >= ($c.time[:13] + ":00")) | index(true)) as $i
  | range($i; $i + 18; 3)
  | "H|\($r.hourly.time[.][11:16])|\($r.hourly.temperature_2m[.])|\($r.hourly.weather_code[.])|\($r.hourly.is_day[.])" ),
( range(0; 7) as $d
  | "D|\($r.daily.time[$d] | strptime("%Y-%m-%d") | mktime | strftime("%a"))|\($r.daily.weather_code[$d])|\($r.daily.temperature_2m_max[$d])|\($r.daily.temperature_2m_min[$d])" ),
"S|\($r.daily.sunrise[0][11:16])|\($r.daily.sunset[0][11:16])"
"#;

fn fetch() -> Weather {
    let Some((latitude, longitude)) = location() else { return Weather::default() };

    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={latitude}&longitude={longitude}\
         &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,is_day\
         &hourly=temperature_2m,weather_code,is_day\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset\
         &timezone=auto&forecast_days=7&wind_speed_unit=ms"
    );

    let text = amane::output(&format!("curl -fsS --max-time 15 '{url}' | jq -r '{QUERY}'"));

    let mut weather = Weather::default();

    for line in text.lines() {
        let fields: Vec<&str> = line.split('|').collect();

        let number = |index: usize| fields.get(index).and_then(|value| value.parse::<f32>().ok()).unwrap_or(0.0);

        match fields.first().copied() {
            Some("C") if fields.len() >= 7 => {
                weather.ready = true;
                weather.temp = number(1);
                weather.feels = number(2);
                weather.humidity = number(3) as u32;
                weather.code = number(4) as u32;
                weather.wind = number(5);
                weather.day = fields[6] == "1";
            }
            Some("H") if fields.len() >= 5 => weather.hours.push(Hour {
                time: fields[1].to_string(),
                temp: number(2),
                code: number(3) as u32,
                day: fields[4] == "1",
            }),
            Some("D") if fields.len() >= 5 => weather.days.push(Day {
                name: fields[1].to_string(),
                code: number(2) as u32,
                high: number(3),
                low: number(4),
            }),
            Some("S") if fields.len() >= 3 => {
                weather.sunrise = fields[1].to_string();
                weather.sunset = fields[2].to_string();
            }
            _ => {}
        }
    }

    weather
}

impl Service for Weather {
    fn new() -> Self {
        Self::default()
    }

    /*
     * the request takes up to 15 seconds, so it runs before the lock is taken. `update()` runs with the service
     * locked, and the bar reads this one on every frame, so a slow request there froze every window
     */
    fn listen() {
        loop {
            let fresh = fetch();

            let wait = if fresh.ready {
                *Self::write() = fresh;

                // fresh data every 15 minutes
                Duration::from_secs(900)
            } else if Self::read().ready {
                // what we have is old, the network is probably down, try again soon
                Duration::from_secs(60)
            } else {
                Duration::from_secs(10)
            };

            std::thread::sleep(wait);
        }
    }
}

/// nerd font weather glyph and a short description for a WMO weather code
pub fn describe(code: u32, day: bool) -> (&'static str, &'static str) {
    match code {
        0 => (if day { "\u{e30d}" } else { "\u{e32b}" }, "Clear"),
        1 => (if day { "\u{e30d}" } else { "\u{e32b}" }, "Mostly clear"),
        2 => (if day { "\u{e302}" } else { "\u{e37e}" }, "Partly cloudy"),
        3 => ("\u{e312}", "Overcast"),
        45 | 48 => ("\u{e313}", "Fog"),
        51..=57 => ("\u{e319}", "Drizzle"),
        61..=67 => ("\u{e318}", "Rain"),
        71..=77 | 85 | 86 => ("\u{e31a}", "Snow"),
        80..=82 => ("\u{e319}", "Showers"),
        95..=99 => ("\u{e31d}", "Thunderstorm"),
        _ => ("\u{e312}", "Cloudy"),
    }
}

// -0.3 should read 0, not -0
pub fn degrees(value: f32) -> String {
    let rounded = value.round();

    format!("{}\u{b0}", if rounded == 0.0 { 0.0 } else { rounded })
}
