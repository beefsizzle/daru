use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/*
 * icon name -> file, for notifications and the tray. looked up on demand in the installed themes and remembered.
 * an index of every icon on the system would cost tens of thousands of strings to find the few dozen that get asked for
 */
// a miss is remembered for a while only: an icon can be installed after the app that asks for it started
// what was found for a name (or None when nothing was) and when it was looked up
type Lookups = HashMap<String, (Option<String>, Instant)>;

static CACHE: OnceLock<Mutex<Lookups>> = OnceLock::new();

const MISS_LASTS: Duration = Duration::from_secs(60);

const THEMES: [&str; 4] = ["hicolor", "breeze-dark", "breeze", "Adwaita"];
const CONTEXTS: [&str; 6] = ["apps", "status", "devices", "panel", "categories", "actions"];
// the scalable ones first (some apps put theirs in hicolor's "symbolic"), then the biggest bitmaps
const SIZES: [&str; 9] = ["scalable", "symbolic", "256x256", "128x128", "96x96", "64x64", "48x48", "32x32", "22x22"];
const NUMBERS: [&str; 6] = ["256", "128", "64", "48", "32", "22"];

/*
 * the data folders icons live under, the user's first: XDG_DATA_HOME (or ~/.local/share), then each folder in
 * XDG_DATA_DIRS, which means /usr/local/share and /usr/share when it's unset, as the spec says. NixOS has no
 * /usr/share at all: its icons sit in the profiles XDG_DATA_DIRS names (/run/current-system/sw/share,
 * /etc/profiles/per-user/<user>/share), so a fixed /usr/share/icons found nothing there
 */
pub fn data_dirs() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();

    let own = std::env::var("XDG_DATA_HOME").ok().filter(|dir| !dir.is_empty()).unwrap_or_else(|| format!("{home}/.local/share"));

    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|dirs| !dirs.is_empty()).unwrap_or_else(|| String::from("/usr/local/share:/usr/share"));

    let mut dirs = vec![own];

    for dir in system.split(':') {
        if !dir.is_empty() && !dirs.iter().any(|seen| seen == dir) {
            dirs.push(dir.to_string());
        }
    }

    dirs
}

// where icons live, and which themes to look in under each. Flatpak apps export their icons to their own folders,
// which XDG_DATA_DIRS only names when Flatpak's profile script ran, so they're looked in either way
fn roots() -> Vec<(String, &'static [&'static str])> {
    let home = std::env::var("HOME").unwrap_or_default();

    let themes: &'static [&'static str] = &THEMES;

    let mut roots: Vec<(String, &'static [&'static str])> = data_dirs().into_iter().map(|dir| (format!("{dir}/icons"), themes)).collect();

    roots.push((format!("{home}/.local/share/flatpak/exports/share/icons"), &["hicolor"]));
    roots.push((String::from("/var/lib/flatpak/exports/share/icons"), &["hicolor"]));

    roots
}

fn find(name: &str) -> Option<String> {
    let check = |path: String| Path::new(&path).exists().then_some(path);

    for (root, themes) in roots() {
        for &theme in themes {
            for context in CONTEXTS {
                for extension in ["svg", "png"] {
                    // hicolor and Adwaita: <size>/<context>/<name>
                    for size in SIZES {
                        if let Some(path) = check(format!("{root}/{theme}/{size}/{context}/{name}.{extension}")) {
                            return Some(path);
                        }
                    }

                    // breeze: <context>/<number>/<name>
                    for number in NUMBERS {
                        if let Some(path) = check(format!("{root}/{theme}/{context}/{number}/{name}.{extension}")) {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }

    // apps that ship one loose icon
    data_dirs().iter().find_map(|dir| ["png", "svg"].iter().find_map(|extension| check(format!("{dir}/pixmaps/{name}.{extension}"))))
}

/// an icon name, a path, or a file:// link, as a path to a PNG or SVG
pub fn resolve(name: &str) -> Option<String> {
    let name = name.trim();

    if name.is_empty() {
        return None;
    }

    let name = name.strip_prefix("file://").unwrap_or(name);

    if name.starts_with('/') {
        return Path::new(name).exists().then(|| name.to_string());
    }

    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Some((known, at)) = crate::sync::lock(cache).get(name)
        && (known.is_some() || at.elapsed() < MISS_LASTS)
    {
        return known.clone();
    }

    let found = find(name);

    crate::sync::lock(cache).insert(name.to_string(), (found.clone(), Instant::now()));

    found
}
