use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use amane::{Full, Image, Layer, LayerWindow, Parent, Rectangle, Service, Zone};

use crate::compositor::Shot;

/*
 * a region screenshot dragged out over a frozen copy of the screen, as niri's own screenshot tool does, for the
 * compositors that have none (Umbriel). each monitor is grabbed with grim first and shown in an overlay window drawn
 * by the compositor from shared memory, like the wallpaper, so it costs no gpu memory. slurp runs on top of it as
 * before, and grim then takes the region from the frozen picture, which is exactly what was on screen when the
 * screenshot was asked for
 */
#[derive(Default)]
pub struct Freeze {
    // the frozen picture of each monitor, by name; empty while nothing is frozen
    pictures: HashMap<String, PathBuf>,

    // shown, but not yet known to be on screen, see FRAMES
    waiting: bool,
}

impl Service for Freeze {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

// every monitor a freeze window was made for, which is every monitor the shell has seen. amane has no list of them
static MONITORS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/*
 * the frames each freeze window drew since it was shown, as the wallpaper's fade counts them: once a monitor has drawn
 * a second frame, the first, with the picture attached, is on screen. slurp mustn't start before that, or the live
 * screen would show for a moment under its first drag
 */
static FRAMES: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

// one screenshot at a time: a second keypress while dragging is ignored rather than freezing over the freeze
static TAKING: Mutex<()> = Mutex::new(());

// each freeze gets fresh file names: amane keeps decoded pictures by path, and an old one would show again
static COUNT: AtomicU32 = AtomicU32::new(0);

// how long the grabs may take to decode, and the windows to show them, before the drag goes ahead on the live screen
const DECODE_WITHIN: Duration = Duration::from_secs(1);
const SHOW_WITHIN: Duration = Duration::from_millis(300);

pub fn view(monitor: &amane::Monitor) -> LayerWindow {
    crate::sync::lock(&MONITORS).insert(monitor.name.clone());

    let (picture, waiting) = {
        let freeze = Freeze::read();

        (freeze.pictures.get(&monitor.name).cloned(), freeze.waiting)
    };

    if picture.is_some() && waiting {
        if let Some(frames) = crate::sync::lock(&FRAMES).as_mut() {
            *frames.entry(monitor.name.clone()).or_insert(0) += 1;
        }

        amane::request_frame();
    }

    let window = LayerWindow::new()
        .width(Full)
        .height(Full)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("daru-freeze")
        .click_through()
        .visible(picture.is_some());

    // the child only shows on a compositor without viewports, where the picture can't be shown, and then shows nothing
    let window = match &picture {
        Some(path) => window.picture(path),
        None => window,
    };

    window.child(Rectangle::new().width(Parent).height(Parent))
}

/// a region screenshot over a frozen screen, or one to draw on in satty. it runs on a thread of its own
pub fn region(shot: Shot) {
    std::thread::spawn(move || {
        let Ok(_taking) = TAKING.try_lock() else { return };

        let frozen = freeze();

        // slurp prints the region as "x,y wxh", and nothing when the drag is cancelled with Escape
        let region = Command::new("slurp").stderr(Stdio::null()).output().ok().filter(|output| output.status.success());
        let region = region.map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string()).filter(|region| is_region(region));

        let Some(region) = region else {
            thaw(&frozen);

            return;
        };

        // taken while the screen is still frozen, then the picture is let go before anything else opens
        let capture = runtime().join(format!("daru-screenshot-{}.png", COUNT.fetch_add(1, Ordering::Relaxed)));
        let taken = Command::new("grim").arg("-g").arg(&region).arg(&capture).status().is_ok_and(|status| status.success());

        thaw(&frozen);

        if !taken {
            return;
        }

        let capture = capture.display().to_string();

        match shot {
            // saved, copied and announced as every other screenshot is (see compositor.rs)
            Shot::Region => amane::spawn(&format!(
                "{}; mv \"{capture}\" \"$file\" && {}",
                crate::compositor::SAVE,
                crate::compositor::DONE
            )),
            // satty saves, copies and notifies by itself, and the capture is left for it to read
            Shot::Annotate => amane::spawn(&format!("satty --filename \"{capture}\"")),
            // a whole monitor needs no freeze, compositor::take never sends one here
            Shot::Screen => {}
        }
    });
}

// "x,y wxh", in whole numbers; anything else isn't passed on, even though slurp is the only one to write it
fn is_region(text: &str) -> bool {
    let Some((place, size)) = text.split_once(' ') else { return false };

    let number = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-');

    place.split_once(',').is_some_and(|(x, y)| number(x) && number(y))
        && size.split_once('x').is_some_and(|(width, height)| number(width) && number(height))
}

fn runtime() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from)
}

/*
 * grabs every monitor and shows the grabs, returning once they're on screen. a monitor that can't be grabbed (it
 * was unplugged) is left live, and when nothing decodes in time the drag simply goes ahead on the live screen
 */
fn freeze() -> Vec<PathBuf> {
    let monitors: Vec<String> = crate::sync::lock(&MONITORS).iter().cloned().collect();
    let count = COUNT.fetch_add(1, Ordering::Relaxed);

    // all at once: on two monitors the second grab mustn't be a frame later than the first
    let grabs: Vec<_> = monitors
        .into_iter()
        .filter_map(|name| {
            let path = runtime().join(format!("daru-freeze-{name}-{count}.png"));

            // no compression: the grab is read back at once and thrown away, and decoding is what holds things up
            let grim = Command::new("grim").args(["-l", "0", "-o", &name]).arg(&path).stderr(Stdio::null()).spawn().ok()?;

            Some((name, path, grim))
        })
        .collect();

    let grabbed: Vec<(String, PathBuf)> = grabs
        .into_iter()
        .filter_map(|(name, path, mut grim)| grim.wait().is_ok_and(|status| status.success()).then_some((name, path)))
        .collect();

    let paths: Vec<PathBuf> = grabbed.iter().map(|(_, path)| path.clone()).collect();

    // decoded before the windows show, so they never stand there empty
    let started = Instant::now();

    while !paths.iter().all(Image::loaded) {
        if started.elapsed() > DECODE_WITHIN {
            thaw(&paths);

            return Vec::new();
        }

        std::thread::sleep(Duration::from_millis(4));
    }

    *crate::sync::lock(&FRAMES) = Some(HashMap::new());

    {
        let mut freeze = Freeze::write();

        freeze.pictures = grabbed.into_iter().collect();
        freeze.waiting = true;
    }

    let shown = Instant::now();

    while !all_shown(paths.len()) && shown.elapsed() < SHOW_WITHIN {
        std::thread::sleep(Duration::from_millis(4));
    }

    *crate::sync::lock(&FRAMES) = None;

    Freeze::write().waiting = false;

    paths
}

// every frozen monitor has drawn its second frame
fn all_shown(monitors: usize) -> bool {
    crate::sync::lock(&FRAMES).as_ref().is_some_and(|frames| frames.len() >= monitors && frames.values().all(|count| *count >= 2))
}

// hides the frozen pictures and lets go of them, decoded copies and files both
fn thaw(paths: &[PathBuf]) {
    {
        let mut freeze = Freeze::write();

        freeze.pictures.clear();
        freeze.waiting = false;
    }

    for path in paths {
        Image::forget(path);

        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::is_region;

    #[test]
    fn only_what_slurp_prints_is_taken_as_a_region() {
        assert!(is_region("120,45 800x600"));

        // a region on a monitor left of or above the first
        assert!(is_region("-1920,0 300x200"));

        for text in ["", "120,45", "120,45 800x", "1,2 3x4; rm -rf ~", "$(reboot) 1x1", "1,2  3x4"] {
            assert!(!is_region(text), "{text:?}");
        }
    }
}
