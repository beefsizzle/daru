use std::time::Duration;

use amane::Service;

use crate::panels::{self, Which};

// the details NetworkManager knows about the connection that carries your traffic
#[derive(Clone, Default, PartialEq)]
pub struct Netinfo {
    pub interface: String,
    pub kind: String,       // ethernet, wifi
    pub connection: String, // the profile name
    pub ip: String,
    pub ip6: String,
    pub gateway: String,
    pub dns: Vec<String>,
    pub mac: String,
    pub speed: String, // negotiated link speed of a wired port
    pub ping: Option<f32>, // milliseconds to the gateway
    pub has_wifi: bool,    // the machine has a Wi-Fi device, whether it's in use or not
}

// runs a program with arguments and returns what it printed. no shell: an interface name or a gateway address is
// data from the system, not something to paste into a command line
fn run(program: &str, arguments: &[&str]) -> String {
    std::process::Command::new(program)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

fn default_interface() -> String {
    // "default via 10.0.0.1 dev enp5s0 proto dhcp metric 100"
    let route = run("ip", &["route", "show", "default"]);

    let mut words = route.split_whitespace();

    while let Some(word) = words.next() {
        if word == "dev" {
            return words.next().unwrap_or("").to_string();
        }
    }

    String::new()
}

fn speed_text(interface: &str) -> String {
    let megabits: i64 = std::fs::read_to_string(format!("/sys/class/net/{interface}/speed"))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(-1);

    match megabits {
        ..=0 => String::new(),
        1..=999 => format!("{megabits} Mbit/s"),
        _ => format!("{:.1} Gbit/s", megabits as f64 / 1000.0).replace(".0 ", " "),
    }
}

// what NetworkManager says now. the ping is measured separately, see `keep_ping`
fn read() -> Netinfo {
    let interface = default_interface();

    // "wifi", "ethernet", "loopback", one device per line
    let has_wifi = run("nmcli", &["-t", "-f", "TYPE", "device"]).lines().any(|kind| kind == "wifi");

    let mut info = Netinfo { interface: interface.clone(), has_wifi, ..Netinfo::default() };

    if interface.is_empty() {
        return info;
    }

    let mut dns6: Vec<String> = Vec::new();

    // nmcli -t escapes colons inside values as "\:"
    let text = run("nmcli", &["-t", "dev", "show", &interface]);

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };

        let value = value.replace("\\:", ":");

        match key.split('[').next().unwrap_or(key) {
            "GENERAL.TYPE" => info.kind = value,
            "GENERAL.CONNECTION" => info.connection = value,
            "GENERAL.HWADDR" => info.mac = value,
            "IP4.ADDRESS" if info.ip.is_empty() => info.ip = value,
            "IP4.GATEWAY" => info.gateway = value,
            "IP4.DNS" => info.dns.push(value),
            "IP6.DNS" => dns6.push(value),
            "IP6.ADDRESS" if info.ip6.is_empty() && !value.starts_with("fe80") => info.ip6 = value,
            _ => {}
        }
    }

    // routers often only hand out an IPv6 resolver
    if info.dns.is_empty() {
        info.dns = dns6;
    }

    info.speed = speed_text(&interface);

    info
}

// a ping is taken every other round, and between them the last one still stands, but only while it was to the same
// gateway. after the cable is pulled or the network changed it says nothing about the connection on screen
fn keep_ping(fresh: &mut Netinfo, previous: &Netinfo) {
    if !fresh.gateway.is_empty() && fresh.gateway == previous.gateway {
        fresh.ping = previous.ping;
    }
}

/// a first look right when the panel opens
pub fn refresh() {
    std::thread::spawn(|| {
        let mut fresh = read();

        keep_ping(&mut fresh, &Netinfo::read());

        if fresh != *Netinfo::read() {
            *Netinfo::write() = fresh;
        }
    });
}

impl Service for Netinfo {
    fn new() -> Self {
        Self::default()
    }

    /*
     * nmcli and a one second ping run before the lock is taken: `update()` runs with the service locked, and
     * the network panel reads this one while drawing. only a real change is written, so an idle panel doesn't redraw.
     * and only while the network panel is open: it's a few process spawns every few seconds
     */
    fn listen() {
        let mut round = 0u32;

        loop {
            if !panels::is_open(Which::Network) {
                round = 0;

                panels::wait_until_open(&[Which::Network]);

                continue;
            }

            round += 1;

            let previous = Self::read().clone();

            let mut fresh = read();

            keep_ping(&mut fresh, &previous);

            // a ping takes up to a second, so only every other round
            if round.is_multiple_of(2) && !fresh.gateway.is_empty() {
                let output = run("ping", &["-c1", "-W1", &fresh.gateway]);

                fresh.ping = output.split("time=").nth(1).and_then(|rest| rest.split_whitespace().next()).and_then(|number| number.parse().ok());
            }

            // NetworkManager scans by itself now and then, a dropdown left open looks again every fifteen seconds
            if fresh.has_wifi && round.is_multiple_of(5) {
                amane::Network::scan();
            }

            if fresh != previous {
                *Self::write() = fresh;
            }

            std::thread::sleep(Duration::from_secs(3));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Netinfo, keep_ping};

    fn on(gateway: &str, ping: Option<f32>) -> Netinfo {
        Netinfo { gateway: gateway.to_string(), ping, ..Netinfo::default() }
    }

    #[test]
    fn ping_is_kept_for_the_same_gateway() {
        let mut fresh = on("10.0.0.1", None);

        keep_ping(&mut fresh, &on("10.0.0.1", Some(0.4)));

        assert_eq!(fresh.ping, Some(0.4));
    }

    #[test]
    fn ping_is_dropped_when_the_gateway_changes_or_goes() {
        let mut moved = on("192.168.1.1", None);
        let mut offline = on("", None);

        keep_ping(&mut moved, &on("10.0.0.1", Some(0.4)));
        keep_ping(&mut offline, &on("10.0.0.1", Some(0.4)));

        assert_eq!(moved.ping, None);
        assert_eq!(offline.ping, None);
    }
}
