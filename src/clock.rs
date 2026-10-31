use std::sync::{Condvar, Mutex};
use std::time::Duration;

use amane::{Bus, Service, Value};

// set when the computer wakes from a suspend, see `listen`
static WOKE: Mutex<bool> = Mutex::new(false);
static WAKING: Condvar = Condvar::new();

pub struct Clock {
    pub time: String,   // 17:52
    pub date: String,   // Sun 18 Oct
    pub long_date: String, // Sunday 18 October
    pub year: i32,
    pub month: u32,
    pub day: u32,
    seconds: u32,
}

fn read() -> Clock {
    // one `date` call, so every field comes from the same instant
    let line = amane::output("date '+%H:%M|%a %d %b|%Y|%m|%d|%A %d %B|%S'");

    let mut parts = line.trim().split('|');

    let time = parts.next().unwrap_or("").to_string();
    let date = parts.next().unwrap_or("").to_string();
    let year = parts.next().and_then(|v| v.parse().ok()).unwrap_or(2026);
    let month = parts.next().and_then(|v| v.parse().ok()).unwrap_or(1);
    let day = parts.next().and_then(|v| v.parse().ok()).unwrap_or(1);
    let long_date = parts.next().unwrap_or("").to_string();
    let seconds = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);

    Clock { time, date, long_date, year, month, day, seconds }
}

impl Service for Clock {
    fn new() -> Self {
        read()
    }

    /*
     * `date` runs before the lock is taken (`update()` holds it, and every bar reads the clock), once at each minute
     * change. the timer doesn't count time spent asleep, so after a suspend it would come up to a minute late:
     * logind says when the computer wakes, and the clock is read again right then
     */
    fn listen() {
        std::thread::spawn(|| {
            loop {
                for signal in Bus::system().signals("org.freedesktop.login1.Manager", "PrepareForSleep") {
                    // true going to sleep, false on waking
                    if !signal.arguments().first().is_some_and(Value::bool) {
                        *crate::sync::lock(&WOKE) = true;

                        WAKING.notify_all();
                    }
                }

                // no system bus (yet): the clock still keeps time, it's only late after a suspend
                std::thread::sleep(Duration::from_secs(60));
            }
        });

        loop {
            let now = read();

            let seconds_left = 60u64.saturating_sub(u64::from(now.seconds)).clamp(1, 60);

            let changed = {
                let current = Self::read();

                now.time != current.time || now.day != current.day
            };

            if changed {
                *Self::write() = now;
            }

            // `date` cuts the seconds off, so this never wakes before the minute turns, at most a second after
            let woke = crate::sync::lock(&WOKE);
            let (mut woke, _) = WAKING
                .wait_timeout_while(woke, Duration::from_secs(seconds_left), |woke| !*woke)
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            *woke = false;
        }
    }
}
