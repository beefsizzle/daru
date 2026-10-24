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

// where icons live, and which themes to look in under each. Flatpak apps export their icons to their own folders
fn roots() -> Vec<(String, &'static [&'static str])> {
    let home = std::env::var("HOME").unwrap_or_default();

    vec![
        (format!("{home}/.local/share/icons"), &THEMES),
        (String::from("/usr/share/icons"), &THEMES),
        (format!("{home}/.local/share/flatpak/exports/share/icons"), &["hicolor"]),
        (String::from("/var/lib/flatpak/exports/share/icons"), &["hicolor"]),
    ]
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
    ["png", "svg"].iter().find_map(|extension| check(format!("/usr/share/pixmaps/{name}.{extension}")))
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
