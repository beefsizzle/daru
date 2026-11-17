use amane::{
    Center, Color, Column, Fingerprint, Image, LayerWindow, Lock, Monitor, Parent, Rectangle, Service, Stack, TextInput,
    children,
};

use crate::clock::Clock;
use crate::theme::{alpha, dark};
use crate::ui::{bold, icon, label};
use crate::wallpaper::Wallpaper;

// the lock screen. the password is checked through PAM's `login` service, like a tty login, and an enrolled
// fingerprint unlocks it too while the password field stays usable (see `Lock::use_fingerprint` in main.rs)

pub fn view(_: &Monitor) -> LayerWindow {
    // the dark colours whatever the mode: the text here lies on the dimmed wallpaper, not on a panel
    let t = dark();
    let lock = Lock::read();
    let clock = Clock::read();

    let user = std::env::var("USER").unwrap_or_else(|_| String::from("user"));
    let initial = user.chars().next().unwrap_or('?').to_uppercase().to_string();

    let fingerprint = lock.fingerprint();
    let reader = matches!(fingerprint, Fingerprint::Listening | Fingerprint::Retry | Fingerprint::NoMatch);

    let (status, status_color) = if lock.checking() {
        ("Checking\u{2026}", t.dim)
    } else if lock.failed() {
        ("Wrong password, try again", t.red)
    } else if fingerprint == Fingerprint::TooManyTries {
        ("Too many attempts, use your password", t.red)
    } else if fingerprint == Fingerprint::NoMatch {
        ("Fingerprint not recognised, try again", t.red)
    } else if fingerprint == Fingerprint::Retry {
        ("Couldn't read that, place your finger again", t.dim)
    } else if reader {
        ("Touch the reader or enter your password", t.dim)
    } else {
        ("Enter your password to unlock", t.dim)
    };

    // a big fingerprint under the line while the reader is listening, red when the finger wasn't recognised.
    // its room is kept when there's no reader too, so the rest doesn't move when it comes and goes
    let reader_mark = Rectangle::new().width(64.0).height(64.0).align_child(Center, Center).child(icon(
        if reader { "\u{f0237}" } else { "" },
        52.0,
        if fingerprint == Fingerprint::NoMatch { t.red } else { t.accent },
    ));

    let wallpaper = Wallpaper::read().shown.clone();

    // a tiny copy, blurred once when it loads, stretched over the whole screen
    let background = if wallpaper.is_empty() {
        Rectangle::new().width(Parent).height(Parent).fill(t.panel)
    } else {
        Rectangle::new().width(Parent).height(Parent).fill(Image::cover(wallpaper).thumbnail(192, 108).blurred(5))
    };

    let shade = Rectangle::new().width(Parent).height(Parent).fill(alpha(Color::BLACK, 0.48));

    let avatar = Rectangle::new()
        .width(76.0)
        .height(76.0)
        .fill(t.accent_soft)
        .border(1.5, alpha(t.accent, 0.6))
        .align_child(Center, Center)
        .child(bold(initial, 32.0, t.accent));

    let field = Rectangle::new()
        .width(340.0)
        .height(46.0)
        .fill(alpha(Color::BLACK, 0.35))
        .border(1.0, if lock.failed() { t.red } else { alpha(t.fg, 0.25) })
        .padding(amane::Padding { top: 0.0, right: 14.0, bottom: 0.0, left: 14.0 })
        .align_child(Center, Center)
        .child(
            TextInput::new("lock-password")
                .placeholder("Password")
                .password()
                .focused()
                .size(16.0)
                .color(t.fg)
                .width(312.0)
                .on_submit(|password| {
                    Lock::unlock(&password);

                    TextInput::set_text("lock-password", "");
                }),
        );

    let column = Column::new(children![
        label(&clock.time, 128.0, t.fg).weight(250),
        label(&clock.long_date, 22.0, t.dim),
        Rectangle::new().width(1.0).height(48.0),
        avatar,
        bold(user, 17.0, t.fg),
        Rectangle::new().width(1.0).height(6.0),
        field,
        label(status, 12.5, status_color),
        reader_mark,
    ])
    .gap(12.0)
    .align(Center);

    let footer = Rectangle::new()
        .width(Parent)
        .height(Parent)
        .padding(amane::Padding { top: 0.0, right: 0.0, bottom: 28.0, left: 0.0 })
        .align_child(Center, amane::End)
        .child(amane::Row::new(children![icon("\u{f023}", 12.0, t.faint), label("Locked", 12.0, t.faint)]).gap(8.0).align(Center));

    // the compositor sizes lock screens to the monitor, so this size is never used
    LayerWindow::new().width(1.0).height(1.0).child(Stack::new(children![
        background,
        shade,
        Rectangle::new().width(Parent).height(Parent).align_child(Center, Center).child(column),
        footer,
    ]))
}
