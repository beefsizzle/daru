mod bar;
mod battery;
mod btpanel;
mod btstate;
mod cache;
mod clipboard;
mod compositor;
mod calendar;
mod clock;
mod dashboard;
mod events;
mod focus;
mod freeze;
mod icons;
mod launcher;
mod lockscreen;
mod logind;
mod notify;
mod netinfo;
mod netpanel;
mod panels;
mod recolor;
mod power;
mod sound;
mod sync;
mod sys;
mod sysinfo;
mod system;
mod theme;
mod volume;
mod tray;
mod traymenu;
mod ui;
mod wallpaper;
mod wallpapers;
mod weather;
mod wifi;

use amane::{App, Layer, LayerWindow, Palette, Parent, Rectangle, Zone, Image, Service};

use crate::theme::TEXT_FONT;

// glibc's malloc tuning, see main
#[cfg(target_env = "gnu")]
unsafe extern "C" {
    fn mallopt(param: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
}

#[cfg(target_env = "gnu")]
const M_ARENA_MAX: std::ffi::c_int = -8;

fn main() {
    /*
     * glibc gives threads their own malloc arenas, up to eight per core, and each keeps the memory it once held. the
     * shell runs dozens of mostly idle threads (services, D-Bus, decoding), so two arenas between them all is plenty,
     * and keeps about 10 MB less resident
     */
    #[cfg(target_env = "gnu")]
    // SAFETY: a plain glibc setting, made before any other thread exists
    unsafe {
        mallopt(M_ARENA_MAX, 2);
    }

    // Amane would otherwise also set up OpenGL next to Vulkan, which loads Mesa's whole GL stack for nothing.
    // set before any other thread exists, and only when nobody chose a backend themselves
    if std::env::var_os("WGPU_BACKEND").is_none() {
        // SAFETY: this is the first thing main does, no other thread is reading the environment yet
        unsafe { std::env::set_var("WGPU_BACKEND", "vulkan") };
    }

    // starts listening for lock requests and suspend
    drop(logind::Logind::read());

    // the lock screen also unlocks with an enrolled fingerprint (fprintd), next to the password
    amane::Lock::use_fingerprint();

    let (path, shown) = {
        let wallpaper = wallpaper::Wallpaper::read();

        (wallpaper.path.clone(), wallpaper.shown.clone())
    };

    // the theme comes from the small thumbnail when there is one
    if !shown.is_empty() {
        let source = wallpapers::thumbnail_if_ready(&path).unwrap_or(shown);

        Palette::write().open(&source, 16);
    }

    // other apps' colour files catch up with the wallpaper, in case it changed while the shell wasn't running
    recolor::export();

    App::new()
        .font(TEXT_FONT)
        .ipc("toggle", panels::ipc)
        .ipc("wallpaper", wallpaper::ipc)
        // `amane ipc call recolor` writes the colours for other apps again, after one was set up (sddm/install.sh)
        .ipc("recolor", |_| {
            recolor::export();

            String::from("ok")
        })
        // `amane ipc call screenshot region|screen|annotate`, for the compositor's keybinds (see compositor.rs)
        .ipc("screenshot", |arguments| {
            let Some(shot) = arguments.first().and_then(|name| compositor::Shot::parse(name)) else {
                return String::from("usage: screenshot region|screen|annotate");
            };

            compositor::take(shot);

            String::from("ok")
        })
        // `amane ipc call launcher-grid [n]` opens the launcher on the all-apps grid, with the nth app chosen
        .ipc("launcher-grid", |arguments| {
            panels::show_here(panels::Which::Launcher);

            launcher::open_grid_now(arguments.first().and_then(|text| text.parse().ok()).unwrap_or(0));

            String::from("ok")
        })
        // `amane ipc call launcher-search fire` opens the launcher with a search already typed
        .ipc("launcher-search", |arguments| {
            let text = arguments.join(" ");

            panels::show_here(panels::Which::Launcher);

            {
                let mut launcher = launcher::Launcher::write();

                launcher.query = text.clone();
                launcher.selected = 0;
            }

            amane::TextInput::set_text("launcher-query", &text);

            String::from("ok")
        })
        // `amane ipc call wallpaper-step 1` moves the picker's selection, like the arrow keys or the wheel
        .ipc("wallpaper-step", |arguments| {
            let by: isize = arguments.first().and_then(|text| text.parse().ok()).unwrap_or(1);

            wallpapers::Wallpapers::write().step(by);

            String::from("ok")
        })
        // `amane ipc call clipboard-step 20` moves the clipboard panel's selection, like the arrow keys
        .ipc("clipboard-step", |arguments| {
            clipboard::step(arguments.first().and_then(|text| text.parse().ok()).unwrap_or(1));

            String::from("ok")
        })
        .ipc("lock", |_| {
            amane::Lock::start();

            String::from("locked")
        })
        .ipc("sleep", |_| {
            amane::spawn(power::SUSPEND);

            String::from("suspending")
        })
        // `amane ipc call tray-menu 0` opens the menu of the first tray icon
        .ipc("tray-menu", |arguments| {
            let index: usize = arguments.first().and_then(|text| text.parse().ok()).unwrap_or(0);

            let item = tray::Tray::read().items.get(index).cloned();

            match item {
                Some(item) if traymenu::open(&item, &panels::focused_output()) => String::from("ok"),
                Some(_) => String::from("that icon has no menu"),
                None => String::from("no such tray icon"),
            }
        })
        .window_per_monitor(wallpaper_window)
        .window_per_monitor(bar::view)
        .window_per_monitor(panels::catcher)
        .window_per_monitor(dashboard::view)
        .window_per_monitor(calendar::view)
        .window_per_monitor(notify::center)
        .window_per_monitor(traymenu::view)
        .window_per_monitor(wallpapers::view)
        .window_per_monitor(launcher::view)
        .window_per_monitor(power::view)
        .window_per_monitor(netpanel::view)
        .window_per_monitor(sound::view)
        .window_per_monitor(btpanel::view)
        .window_per_monitor(clipboard::view)
        .window_per_monitor(system::view)
        .lock(lockscreen::view)
        .window_per_monitor(notify::toast_0)
        .window_per_monitor(notify::toast_1)
        .window_per_monitor(notify::toast_2)
        // last, so a frozen screen covers the shell's other overlays too; slurp still opens above it
        .window_per_monitor(freeze::view)
        .run();
}

// how strongly a wallpaper waiting for its fade is drawn: under a quarter of one shade out of 255, so nothing shows
const WAITING_OPACITY: f32 = 0.001;

/*
 * the wallpaper is drawn by the compositor (LayerWindow::picture): one buffer in shared memory for every monitor,
 * and no gpu memory of the shell's own. that was about 170 MB of swapchains and textures for two monitors. only while
 * a new wallpaper fades in does the window draw with the gpu, and it gives that back afterwards.
 * no `.thumbnail()` here on purpose: Amane keeps small copies for good, and the file is already a screen-sized copy
 * (see wallpapers.rs)
 */
fn wallpaper_window(monitor: &amane::Monitor) -> LayerWindow {
    let (shown, incoming, fade, waiting) = {
        let wallpaper = wallpaper::Wallpaper::read();

        (wallpaper.shown.clone(), wallpaper.incoming.clone(), wallpaper.fade.value(), wallpaper.waiting)
    };

    // a fade that can't start until its first frame is on screen keeps the window drawing, see wallpaper.rs
    if waiting {
        wallpaper::drawn_waiting(&monitor.name);

        amane::request_frame();
    }

    let base = Rectangle::new().width(Parent).height(Parent);

    let window = LayerWindow::new()
        .width(amane::Full)
        .height(amane::Full)
        .layer(Layer::Background)
        .space(Zone::Ignore)
        .namespace("amane-wallpaper")
        .visible(!shown.is_empty() || incoming.is_some());

    match &incoming {
        // a new wallpaper fades in on top of the old one (black, the very first time), drawn by the gpu for as long
        // as that takes
        Some(incoming) => {
            let under = if shown.is_empty() { base.fill(amane::Color::BLACK) } else { base.fill(Image::cover(&shown)) };

            /*
             * while it waits to start, the new picture is drawn too faint to see rather than not at all: Amane leaves
             * out anything at no opacity, and the point of waiting is that this frame sends it to the gpu
             */
            let opacity = if waiting { WAITING_OPACITY } else { fade };

            window.child(under.child(Rectangle::new().width(Parent).height(Parent).fill(Image::cover(incoming)).opacity(opacity)))
        }
        // the compositor draws it. the child only shows while the file is first decoding, and must not name the
        // picture itself, or the shell would decode its own copy again
        None if !shown.is_empty() => window.picture(&shown).child(base.fill(amane::Color::BLACK)),
        None => window.child(base.fill(amane::Color::BLACK)),
    }
}
