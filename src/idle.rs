use std::time::Duration;

use crate::compositor;

/*
 * what happens when you step away (App::idle, in main.rs): the session locks after LOCK and the screens go off after
 * SCREENS_OFF, both counted from the last input. a playing video holds both off, since the compositor tells the shell
 * nothing while something inhibits idle. turning the screens off is Umbriel's to do, so they stay on anywhere else
 */
pub const LOCK: Duration = Duration::from_secs(5 * 60);
pub const SCREENS_OFF: Duration = Duration::from_secs(10 * 60);

pub fn screens_off() {
    if compositor::umbriel() {
        amane::spawn("umbriel msg dpms-off");
    }
}

// asked for at the first input: Umbriel only wakes the screens by itself when every one of them is off
pub fn screens_on() {
    if compositor::umbriel() {
        amane::spawn("umbriel msg dpms-on");
    }
}
