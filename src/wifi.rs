use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use amane::{Bus, Network, Service, TextInput};

/*
 * the Wi-Fi part of the network dropdown: which network asks for its password, which one is being joined, and
 * which one just failed. NetworkManager does the joining (through amane's `Network`), this only keeps what the
 * panel needs to say about it
 */
#[derive(Default)]
pub struct Wifi {
    pub prompt: Option<String>,  // the secured network whose password field is open
    pub password: String,        // what's typed in it so far
    pub joining: Option<String>, // the network being joined from the panel
    pub failed: Option<Failure>, // the last network that didn't join
}

#[derive(Clone, PartialEq)]
pub struct Failure {
    pub ssid: String,
    pub refused: bool, // NetworkManager said the password was wrong, rather than the network not answering
    pub typed: bool,   // the password was typed in the panel, as opposed to a saved one
}

impl Service for Wifi {
    fn new() -> Self {
        Self::default()
    }

    fn interval() -> Duration {
        Duration::from_secs(3600)
    }
}

pub const PASSWORD_FIELD: &str = "wifi-password";

// the bar and panel glyph for a signal strength, 0 to 100, with a warning mark on a network without internet
pub fn glyph(strength: u8, internet: bool) -> &'static str {
    let bars = match strength {
        0..=19 => 0,
        20..=44 => 1,
        45..=69 => 2,
        70..=84 => 3,
        _ => 4,
    };

    match (bars, internet) {
        (0, true) => "\u{f092f}", // outline, no bars
        (1, true) => "\u{f091f}",
        (2, true) => "\u{f0922}",
        (3, true) => "\u{f0925}",
        (_, true) => "\u{f0928}", // full
        (0, false) => "\u{f092b}",
        (1, false) => "\u{f0920}",
        (2, false) => "\u{f0923}",
        (3, false) => "\u{f0926}",
        (_, false) => "\u{f0929}",
    }
}

/// the panel opened: a fresh look at the networks around, and no leftovers from last time
pub fn opened() {
    Network::scan();

    let mut wifi = Wifi::write();

    wifi.prompt = None;
    wifi.password.clear();

    if wifi.joining.is_none() {
        wifi.failed = None;
    }
}

/// opens the password field under a secured network, or closes it when it's already open there
pub fn ask(ssid: &str) {
    let mut wifi = Wifi::write();

    if wifi.prompt.as_deref() == Some(ssid) {
        wifi.prompt = None;
    } else {
        wifi.prompt = Some(ssid.to_string());
        wifi.failed = None;
    }

    wifi.password.clear();

    drop(wifi);

    TextInput::set_text(PASSWORD_FIELD, "");
}

pub fn cancel() {
    let mut wifi = Wifi::write();

    wifi.prompt = None;
    wifi.password.clear();

    drop(wifi);

    TextInput::set_text(PASSWORD_FIELD, "");
}

/// joins with what's typed in the password field
pub fn submit() {
    let (ssid, password) = {
        let wifi = Wifi::read();

        let Some(ssid) = wifi.prompt.clone() else { return };

        (ssid, wifi.password.clone())
    };

    // WPA needs at least eight characters, NetworkManager would refuse it anyway
    if password.chars().count() < 8 {
        return;
    }

    join(&ssid, Some(password));
}

/// joins a network: a saved or open one with `None`, a new secured one with its password
pub fn join(ssid: &str, password: Option<String>) {
    {
        let mut wifi = Wifi::write();

        if wifi.joining.is_some() {
            return;
        }

        wifi.prompt = None;
        wifi.password.clear();
        wifi.joining = Some(ssid.to_string());
        wifi.failed = None;
    }

    TextInput::set_text(PASSWORD_FIELD, "");

    listen_for_failures();

    let typed = password.is_some();

    Network::connect(ssid, password.as_deref());

    let ssid = ssid.to_string();

    std::thread::spawn(move || watch(ssid, typed));
}

/// forgets a saved network, password and all
pub fn forget(ssid: &str) {
    Network::forget(ssid);

    let mut wifi = Wifi::write();

    if wifi.failed.as_ref().is_some_and(|failure| failure.ssid == ssid) {
        wifi.failed = None;
    }
}

#[derive(Debug, PartialEq)]
enum Step {
    Wait,
    Joined,
    Failed,
}

// NetworkManager starts working on it within a few seconds, and a WPA handshake is over well within the limit
const START_WITHIN: Duration = Duration::from_secs(10);
const GIVE_UP_AFTER: Duration = Duration::from_secs(45);

/*
 * where a join stands, from what `Network` shows: the Wi-Fi card on the network (`joined`) and done joining is
 * success, having been busy joining and stopped without getting there is failure. the network that was up before (if
 * any) stays up until NetworkManager starts, so nothing counts before it's been seen busy, or the limit passed.
 * `joined` is the card's own network, not `Network::ssid()`, which is empty while a cable carries the internet
 */
fn step(joined: bool, connecting: bool, seen_connecting: bool, elapsed: Duration) -> Step {
    if joined && !connecting {
        return Step::Joined;
    }

    if seen_connecting && !connecting {
        return Step::Failed;
    }

    if elapsed > GIVE_UP_AFTER || (!seen_connecting && elapsed > START_WITHIN) {
        return Step::Failed;
    }

    Step::Wait
}

/*
 * follows a join to its end. a password typed in the panel that NetworkManager refused leaves a profile with that
 * password behind, which would be tried again on the next click, so it's forgotten and the field opens again. any
 * other failure (out of range, no address) keeps the profile, and the next click tries again
 */
fn watch(ssid: String, typed: bool) {
    let started = Instant::now();

    let mut seen_connecting = false;

    loop {
        std::thread::sleep(Duration::from_millis(400));

        let (joined, connecting) = {
            let network = Network::read();

            let joined = network.access_points().iter().any(|point| point.ssid() == ssid && point.active());

            (joined, network.connecting())
        };

        seen_connecting |= connecting;

        match step(joined, connecting, seen_connecting, started.elapsed()) {
            Step::Wait => continue,
            Step::Joined => {
                Wifi::write().joining = None;

                return;
            }
            Step::Failed => {
                let refused = failure_since(started).is_some_and(|reason| matches!(reason, NO_SECRETS | SUPPLICANT_DISCONNECT));

                let retype = typed && refused;

                if retype {
                    Network::forget(&ssid);
                }

                let mut wifi = Wifi::write();

                wifi.joining = None;
                wifi.failed = Some(Failure { ssid: ssid.clone(), refused, typed });

                if retype {
                    wifi.prompt = Some(ssid);
                    wifi.password.clear();

                    drop(wifi);

                    TextInput::set_text(PASSWORD_FIELD, "");
                }

                return;
            }
        }
    }
}

// ---------------------------------------------------------------- why a join failed

/*
 * NetworkManager says why a device stopped joining only in the signal it sends as it gives up, so that's listened
 * for from the first join on. a refused password ends as "no secrets" (it asks for a new password, and nobody is
 * there to give one), or now and then as the supplicant dropping out of the handshake
 */
const FAILED: u32 = 120;
const NO_SECRETS: u32 = 7;
const SUPPLICANT_DISCONNECT: u32 = 8;

// when a device last failed, and why
static LAST_FAILURE: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

fn listen_for_failures() {
    static STARTED: Once = Once::new();

    STARTED.call_once(|| {
        std::thread::spawn(|| {
            for signal in Bus::system().signals("org.freedesktop.NetworkManager.Device", "StateChanged") {
                // the new state, the old one, and the reason
                let arguments = signal.arguments();

                let (Some(state), Some(reason)) = (arguments.first(), arguments.get(2)) else { continue };

                if state.number() as u32 == FAILED {
                    *crate::sync::lock(&LAST_FAILURE) = Some((Instant::now(), reason.number() as u32));
                }
            }
        });
    });
}

fn failure_since(started: Instant) -> Option<u32> {
    crate::sync::lock(&LAST_FAILURE).filter(|(at, _)| *at >= started).map(|(_, reason)| reason)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Step, glyph, step};

    const SOON: Duration = Duration::from_secs(2);

    #[test]
    fn being_on_the_network_and_done_is_joined() {
        assert_eq!(step(true, false, true, SOON), Step::Joined);

        // a saved network can come up between two looks
        assert_eq!(step(true, false, false, SOON), Step::Joined);
    }

    #[test]
    fn being_on_it_but_still_busy_is_not_done() {
        assert_eq!(step(true, true, true, SOON), Step::Wait);
    }

    #[test]
    fn the_old_network_is_not_a_failure_before_networkmanager_starts() {
        assert_eq!(step(false, false, false, SOON), Step::Wait);
        assert_eq!(step(false, true, true, SOON), Step::Wait);
    }

    #[test]
    fn stopping_without_getting_there_is_a_failure() {
        assert_eq!(step(false, false, true, SOON), Step::Failed);
    }

    #[test]
    fn a_join_that_never_starts_or_never_ends_gives_up() {
        assert_eq!(step(false, false, false, Duration::from_secs(11)), Step::Failed);
        assert_eq!(step(false, true, true, Duration::from_secs(46)), Step::Failed);
    }

    #[test]
    fn more_signal_draws_more_bars() {
        assert_eq!(glyph(5, true), "\u{f092f}");
        assert_eq!(glyph(30, true), "\u{f091f}");
        assert_eq!(glyph(60, true), "\u{f0922}");
        assert_eq!(glyph(80, true), "\u{f0925}");
        assert_eq!(glyph(100, true), "\u{f0928}");
    }

    #[test]
    fn no_internet_keeps_the_bars_and_adds_a_warning() {
        assert_eq!(glyph(5, false), "\u{f092b}");
        assert_eq!(glyph(60, false), "\u{f0923}");
        assert_eq!(glyph(100, false), "\u{f0929}");
    }
}
