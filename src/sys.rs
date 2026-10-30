use std::fs;
use std::time::{Duration, Instant};

use amane::Service;

use crate::panels::{self, Which};

// numbers the built-in services don't cover: temperature, disk, network speed, uptime
pub struct Sys {
    pub temp: f32,
    pub disk: u8,
    pub rx: f64, // bytes per second
    pub tx: f64,
    pub rx_history: Vec<f32>, // the last 40 samples, two seconds apart
    pub tx_history: Vec<f32>,
    pub rx_total: u64, // bytes since boot
    pub tx_total: u64,
    pub uptime: String,
}

fn temperature() -> f32 {
    let Ok(entries) = fs::read_dir("/sys/class/hwmon") else { return 0.0 };

    let mut fallback = 0.0;

    for entry in entries.flatten() {
        let path = entry.path();

        let name = fs::read_to_string(path.join("name")).unwrap_or_default();

        let reading = fs::read_to_string(path.join("temp1_input"))
            .ok()
            .and_then(|text| text.trim().parse::<f32>().ok())
            .map_or(0.0, |millidegrees| millidegrees / 1000.0);

        // k10temp on AMD, coretemp on Intel
        if matches!(name.trim(), "k10temp" | "coretemp") && reading > 0.0 {
            return reading;
        }

        if reading > fallback {
            fallback = reading;
        }
    }

    fallback
}

fn disk() -> u8 {
    amane::output("df --output=pcent /").lines().nth(1).and_then(|line| line.trim().trim_end_matches('%').parse().ok()).unwrap_or(0)
}

fn counters() -> (u64, u64) {
    let text = fs::read_to_string("/proc/net/dev").unwrap_or_default();

    let mut rx = 0;
    let mut tx = 0;

    for line in text.lines().skip(2) {
        let Some((name, rest)) = line.split_once(':') else { continue };

        let name = name.trim();

        // loopback and interfaces that carry traffic another interface already counted: containers, VMs, VPN tunnels
        const VIRTUAL: [&str; 11] = ["lo", "veth", "docker", "br-", "virbr", "vnet", "tun", "tap", "wg", "tailscale", "zt"];

        if VIRTUAL.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }

        let fields: Vec<u64> = rest.split_whitespace().filter_map(|field| field.parse().ok()).collect();

        if fields.len() >= 9 {
            rx += fields[0];
            tx += fields[8];
        }
    }

    (rx, tx)
}

pub fn uptime() -> String {
    let seconds: f64 = fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| text.split_whitespace().next().and_then(|field| field.parse().ok()))
        .unwrap_or(0.0);

    format_uptime(seconds)
}

fn format_uptime(seconds: f64) -> String {
    let minutes = (seconds / 60.0) as u64;

    let (days, hours, minutes) = (minutes / 1440, (minutes / 60) % 24, minutes % 60);

    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

pub fn format_rate(bytes: f64) -> String {
    if bytes >= 1_048_576.0 {
        format!("{:.1} MB/s", bytes / 1_048_576.0)
    } else {
        format!("{:.0} KB/s", bytes / 1024.0)
    }
}

/// a first look right when a panel opens, so the numbers aren't blank until the next round of the loop
pub fn refresh() {
    std::thread::spawn(|| {
        let (rx, tx) = counters();

        let (temp, uptime, disk) = (temperature(), uptime(), disk());

        let mut sys = Sys::write();

        sys.temp = temp;
        sys.uptime = uptime;
        sys.disk = disk;
        sys.rx_total = rx;
        sys.tx_total = tx;
    });
}

impl Service for Sys {
    fn new() -> Self {
        Self { temp: 0.0, disk: 0, rx: 0.0, tx: 0.0, rx_history: vec![0.0; 40], tx_history: vec![0.0; 40], rx_total: 0, tx_total: 0, uptime: String::new() }
    }

    /*
     * only while the dashboard or the network panel is open, the two places these numbers are shown. everything is
     * read before the lock is taken (`update()` would hold it, and `df` is a process). speeds are a difference between
     * two readings, so after a pause the first reading only sets the baseline
     */
    fn listen() {
        let mut baseline: Option<(u64, u64, Instant)> = None;
        let mut round = 0u32;

        loop {
            if !(panels::is_open(Which::Dashboard) || panels::is_open(Which::Network)) {
                baseline = None;
                round = 0;

                panels::wait_until_open(&[Which::Dashboard, Which::Network]);

                continue;
            }

            let (rx, tx) = counters();
            let now = Instant::now();

            let rates = baseline.map(|(before_rx, before_tx, at)| {
                let seconds = now.duration_since(at).as_secs_f64().max(0.001);

                (rx.saturating_sub(before_rx) as f64 / seconds, tx.saturating_sub(before_tx) as f64 / seconds)
            });

            baseline = Some((rx, tx, now));

            let temp = temperature();
            let uptime = uptime();

            // the disk changes slowly and costs a process, so about every half minute
            let disk = (round.is_multiple_of(15)).then(disk);

            {
                let mut sys = Self::write();

                sys.temp = temp;
                sys.uptime = uptime;
                sys.rx_total = rx;
                sys.tx_total = tx;

                if let Some(disk) = disk {
                    sys.disk = disk;
                }

                if let Some((rate_rx, rate_tx)) = rates {
                    sys.rx = rate_rx;
                    sys.tx = rate_tx;

                    sys.rx_history.remove(0);
                    sys.rx_history.push(rate_rx as f32);
                    sys.tx_history.remove(0);
                    sys.tx_history.push(rate_tx as f32);
                }
            }

            round += 1;

            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{format_rate, format_uptime};

    #[test]
    fn uptime_shows_the_two_biggest_units() {
        assert_eq!(format_uptime(0.0), "0m");
        assert_eq!(format_uptime(59.9), "0m");
        assert_eq!(format_uptime(60.0 * 45.0), "45m");
        assert_eq!(format_uptime(3600.0 * 3.0 + 60.0 * 7.0), "3h 7m");
        assert_eq!(format_uptime(86400.0 * 2.0 + 3600.0 * 5.0 + 600.0), "2d 5h");
        assert_eq!(format_uptime(86400.0 - 1.0), "23h 59m");
    }

    #[test]
    fn rates_switch_to_megabytes() {
        assert_eq!(format_rate(0.0), "0 KB/s");
        assert_eq!(format_rate(2048.0), "2 KB/s");
        assert_eq!(format_rate(1_048_576.0 * 1.5), "1.5 MB/s");
    }
}
