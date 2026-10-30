use amane::{Argument, Bus, Service};

/*
 * the power profile, as power-profiles-daemon keeps it: power-saver, balanced or performance, the same three GNOME and
 * KDE offer. the daemon tunes the CPU for each (on intel_pstate or amd-pstate, the energy preference the CPU is given;
 * the governor itself stays powersave), so a separate governor switch would only fight it. switching needs no password
 * for whoever sits at the machine, polkit allows it for the active session.
 *
 * read when the power dropdown opens and after a pick, never polled, so it costs nothing while the dropdown is closed.
 * a change made elsewhere (powerprofilesctl, a laptop's own key) shows the next time the dropdown opens
 */
#[derive(Default, Clone, PartialEq)]
pub struct Profiles {
    // the one in use, empty until the first read or when the daemon isn't running
    pub active: String,

    // what the daemon offers here, from saving to fast; empty without the daemon, and the switcher hides
    pub available: Vec<String>,
}

impl Service for Profiles {
    fn new() -> Self {
        Self::default()
    }

    // the first read happens when the service starts, which is the first time the dropdown is drawn
    fn listen() {
        publish(read());
    }
}

// the daemon's newer name; older versions only answer to net.hadess.PowerProfiles, which the newer ones keep too
const NAMES: [(&str, &str); 2] = [
    ("org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles"),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"),
];

// the order they're shown in, whatever order the daemon lists them in
const ORDER: [&str; 3] = ["power-saver", "balanced", "performance"];

fn read() -> Profiles {
    for (name, path) in NAMES {
        let bus = Bus::system();

        let active = bus.property(name, path, name, "ActiveProfile");

        if active.text().is_empty() {
            continue;
        }

        // a list of maps, each naming one profile under "Profile" (and its drivers, which aren't shown)
        let offered = bus.property(name, path, name, "Profiles");

        let available = ORDER.iter().filter(|wanted| offered.list().iter().any(|profile| profile.get("Profile").text() == **wanted)).map(|name| name.to_string()).collect();

        return Profiles { active: active.text().to_string(), available };
    }

    Profiles::default()
}

fn publish(fresh: Profiles) {
    if fresh != *Profiles::read() {
        *Profiles::write() = fresh;
    }
}

/// read again without waiting, as the dropdown opens
pub fn refresh() {
    std::thread::spawn(|| publish(read()));
}

/// switches to a profile. the dropdown shows it at once, and the daemon's answer is read back after, so a refused
/// switch (no daemon, or polkit said no) shows the profile that's really in use
pub fn pick(profile: &str) {
    if !Profiles::read().available.iter().any(|name| name == profile) {
        return;
    }

    Profiles::write().active = profile.to_string();

    let profile = profile.to_string();

    std::thread::spawn(move || {
        for (name, path) in NAMES {
            let bus = Bus::system();

            if bus.property(name, path, name, "ActiveProfile").text().is_empty() {
                continue;
            }

            bus.set_property(name, path, name, "ActiveProfile", Argument::from(profile.as_str()));

            break;
        }

        publish(read());
    });
}

/// one step towards saving (-1) or speed (+1), for the arrow keys; it stops at either end
pub fn step(by: i32) {
    let (active, available) = {
        let profiles = Profiles::read();

        (profiles.active.clone(), profiles.available.clone())
    };

    let Some(at) = available.iter().position(|name| *name == active) else { return };

    let next = at as i32 + by;

    if next < 0 || next >= available.len() as i32 {
        return;
    }

    pick(&available[next as usize]);
}
