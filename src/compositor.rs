use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use amane::{Argument, Bus};

/*
 * what differs between the compositors the shell runs on: how to log out, how to take a screenshot, and where the
 * focused window's title comes from (see focus.rs). each sets its own variable for the programs it starts
 */
#[derive(Clone, Copy, PartialEq)]
pub enum Compositor {
    Niri,
    Umbriel,
    Other,
}

/*
 * Umbriel's event stream for these families (see its ipc docs), straight from its socket: `each` gets every line, the
 * state as it is first and then a whole new snapshot on every change. `umbriel subscribe` does the same, but kept a
 * process of about 9 MB running for each family followed. returns when Umbriel goes away or `each` says to stop
 */
pub fn umbriel_events(families: &[&str], mut each: impl FnMut(String) -> bool) {
    use std::io::{BufRead, BufReader, Write};

    let Some(path) = std::env::var_os("UMBRIEL_SOCKET") else { return };
    let Ok(mut socket) = std::os::unix::net::UnixStream::connect(path) else { return };

    let names: Vec<String> = families.iter().map(|family| format!("\"{family}\"")).collect();

    if socket.write_all(format!("{{\"cmd\":\"subscribe\",\"events\":[{}]}}\n", names.join(",")).as_bytes()).is_err() {
        return;
    }

    for line in BufReader::new(socket).lines() {
        let Ok(line) = line else { return };

        // a reply to the request rather than an event
        if line.starts_with("{\"ok\"") || line.starts_with("{\"err\"") {
            continue;
        }

        if !each(line) {
            return;
        }
    }
}

pub fn running() -> Compositor {
    if std::env::var_os("NIRI_SOCKET").is_some() {
        Compositor::Niri
    } else if std::env::var_os("UMBRIEL_SOCKET").is_some() {
        Compositor::Umbriel
    } else {
        Compositor::Other
    }
}

/*
 * ends the session without asking. Umbriel takes an action's arguments as separate words (it used to join them with a
 * colon, and `umbriel msg` now refuses that). logind ends the session if Umbriel can't: right after an update, the
 * new `umbriel` command can't talk to a compositor that's still the old one until you log in again
 */
pub fn log_out() -> &'static str {
    match running() {
        Compositor::Niri => "niri msg action quit --skip-confirmation",
        Compositor::Umbriel => "umbriel msg session-quit skip-confirmation || loginctl terminate-session \"$XDG_SESSION_ID\"",
        Compositor::Other => "loginctl terminate-session \"$XDG_SESSION_ID\"",
    }
}

/// what `amane ipc call screenshot region|screen|annotate` takes
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shot {
    Region,   // a region you choose
    Screen,   // the monitor in focus
    Annotate, // a region, opened in satty to draw on first
}

impl Shot {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "region" => Some(Self::Region),
            "screen" => Some(Self::Screen),
            "annotate" => Some(Self::Annotate),
            _ => None,
        }
    }
}

/// where the next screenshot goes, as niri saves them
pub fn new_file() -> Option<PathBuf> {
    let folder = PathBuf::from(std::env::var_os("HOME")?).join("Pictures/Screenshots");

    std::fs::create_dir_all(&folder).ok()?;

    let now = amane::output("date '+%Y-%m-%d %H-%M-%S'");

    Some(folder.join(format!("Screenshot from {now}.png")))
}

/// puts a picture on the clipboard. wl-copy stays behind to hand it out, and has read the file by the time it returns
pub fn copy(file: &Path) -> bool {
    let Ok(picture) = File::open(file) else { return false };

    Command::new("wl-copy")
        .args(["--type", "image/png"])
        .stdin(picture)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// a notification from the screenshot tool, straight to the notification server, which is usually this shell
pub fn announce(picture: &str, summary: &str, body: &str) {
    Bus::session().call(
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        &[
            Argument::from("Screenshot"),
            Argument::Unsigned(0),
            Argument::from(picture),
            Argument::from(summary),
            Argument::from(body),
            Argument::TextList(Vec::new()),
            Argument::Map(BTreeMap::new()),
            Argument::Int(-1),
        ],
    );
}

/// a saved screenshot is copied too, and announced with the picture in the notification, as niri does
pub fn finish(file: &Path) -> bool {
    let copied = copy(file);

    if copied {
        announce(&file.display().to_string(), "Screenshot taken", "Saved in Pictures/Screenshots and copied");
    }

    copied
}

/// the monitor in focus, where the compositor says
pub fn focused_output() -> Option<String> {
    (running() == Compositor::Umbriel)
        .then(|| amane::output("umbriel outputs --json | jq -r '.[] | select(.focused) | .name'"))
        .filter(|name| !name.is_empty())
}

// a whole monitor, or all of them when none is named
fn screen(output: Option<String>) {
    let Some(file) = new_file() else { return };

    let mut grim = Command::new("grim");

    if let Some(output) = output {
        grim.args(["-o", &output]);
    }

    if grim.arg(&file).stderr(Stdio::null()).status().is_ok_and(|status| status.success()) {
        finish(&file);
    }
}

/*
 * takes a screenshot without waiting for it. niri's own tool saves, copies and notifies by itself, and freezes the
 * screen while a region is chosen. elsewhere a region is chosen over a frozen screen (freeze.rs), also for one to draw
 * on in satty, and a whole monitor is taken with grim
 */
pub fn take(shot: Shot) {
    match (running(), shot) {
        (Compositor::Niri, Shot::Region) => amane::spawn("niri msg action screenshot"),
        (Compositor::Niri, Shot::Screen) => amane::spawn("niri msg action screenshot-screen"),
        (_, Shot::Screen) => {
            std::thread::spawn(|| screen(focused_output()));
        }
        (_, Shot::Region | Shot::Annotate) => crate::freeze::region(shot),
    }
}

#[cfg(test)]
mod tests {
    use super::Shot;

    #[test]
    fn shots_by_name() {
        assert_eq!(Shot::parse("region"), Some(Shot::Region));
        assert_eq!(Shot::parse("screen"), Some(Shot::Screen));
        assert_eq!(Shot::parse("annotate"), Some(Shot::Annotate));
        assert_eq!(Shot::parse("window"), None);
    }
}
