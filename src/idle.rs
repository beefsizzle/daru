use std::time::Duration;

use crate::compositor::{self, Compositor};

/*
 * what happens when you step away (App::idle, in main.rs): the session locks after LOCK and the screens go off after
 * SCREENS_OFF, both counted from the last input. a playing video holds both off, since niri and Umbriel tell the shell
 * nothing while something inhibits idle
 */
pub const LOCK: Duration = Duration::from_secs(5 * 60);
pub const SCREENS_OFF: Duration = Duration::from_secs(10 * 60);

pub fn screens_off() {
    match compositor::running() {
        Compositor::Niri => amane::spawn("niri msg action power-off-monitors"),
        Compositor::Umbriel => amane::spawn("umbriel msg dpms-off"),
        Compositor::Other => {}
    }
}

// niri turns them back on by itself at the first input, Umbriel is asked to
pub fn screens_on() {
    if compositor::running() == Compositor::Umbriel {
        amane::spawn("umbriel msg dpms-on");
    }
}
