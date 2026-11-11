use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use amane::{Argument, Bus};

/*
 * what the shell asks of the compositor: logging out, screenshots, and Umbriel's event stream, which the focused
 * window's title (focus.rs) and the layout chip (layouts.rs) follow. the shell is made for Umbriel. on another
 * compositor it still runs, without the parts that need Umbriel
 */

/// whether Umbriel is the compositor: it sets this for the programs it starts
pub fn umbriel() -> bool {
    std::env::var_os("UMBRIEL_SOCKET").is_some()
}

/*
 * Umbriel's event stream for these families (see its ipc docs), straight from its socket: `each` gets every line, the
 * state as it is first and then a whole new snapshot on every change. `umbriel subscribe` does the same, but costs a
 * process of about 9 MB for each family followed. returns when Umbriel goes away or `each` says to stop
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

/*
 * ends the session without asking. logind ends it if Umbriel can't: right after an update, the new `umbriel` command
 * can't talk to a compositor that's still the old one until you log in again
 */
pub fn log_out() -> &'static str {
    if umbriel() {
        "umbriel msg session-quit skip-confirmation || loginctl terminate-session \"$XDG_SESSION_ID\""
    } else {
        "loginctl terminate-session \"$XDG_SESSION_ID\""
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

/// where the next screenshot goes: ~/Pictures/Screenshots, named after the time it was taken
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

/// a saved screenshot is copied too, and announced with the picture in the notification
pub fn finish(file: &Path) -> bool {
    let copied = copy(file);

    if copied {
        announce(&file.display().to_string(), "Screenshot taken", "Saved in Pictures/Screenshots and copied");
    }

    copied
}

/// the monitor in focus, where the compositor says
pub fn focused_output() -> Option<String> {
    umbriel()
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
 * takes a screenshot without waiting for it. a region is chosen over a frozen screen (freeze.rs), also for one to draw
 * on in satty, and a whole monitor is taken with grim
 */
pub fn take(shot: Shot) {
    match shot {
        Shot::Screen => {
            std::thread::spawn(|| screen(focused_output()));
        }
        Shot::Region | Shot::Annotate => crate::freeze::region(shot),
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
