use std::collections::HashSet;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use amane::{
    Center, Column, End, Horizontal, Image, LayerWindow, Monitor, Notification, Notifications, Parent, Rectangle, Row, ScrollArea, Service,
    Start, Urgency, Vertical, Widget, Zone, children,
};

use crate::panels::{self, GAP, RADIUS, Which};
use crate::theme::{Theme, alpha, mix, theme};
use crate::ui::{ago, bold, chip, hover, icon, label, round_button, set_hover};

/*
 * popups and do-not-disturb. the `Notifications` service keeps every notification until it's dismissed,
 * which is the notification center. this service decides which of them are shown as a toast.
 */
pub struct Notify {
    pub dnd: bool,
    pub toasts: Vec<u32>,
    pub monitor: String, // the monitor the popups appear on: where you were when they arrived
    hidden: HashSet<u32>,
}

const TOAST_SECONDS: u64 = 6;
const SLOTS: usize = 3;

// the most a picture from a sender is decoded at: 40 px drawn, at a 1.5 scale, with room to spare
const PICTURE_SIZE: u32 = 64;

impl Service for Notify {
    fn new() -> Self {
        Self { dnd: false, toasts: Vec::new(), monitor: String::new(), hidden: HashSet::new() }
    }

    /*
     * runs outside `update()`, which holds the service locked: asking niri which monitor has focus takes a process
     * spawn, and the toast windows read this service while drawing. between rounds it sleeps until the first toast on
     * screen is due to go, or until the toast windows see something new (`noticed`), so an idle shell never wakes it
     */
    fn listen() {
        loop {
            let due = refresh();

            wait_for_news(due);
        }
    }
}

// picks the notifications shown as toasts, and says how long until the first of them is due to go
fn refresh() -> Option<Duration> {
    let (dnd, hidden, previous) = {
        let notify = Notify::read();

        (notify.dnd, notify.hidden.clone(), notify.toasts.clone())
    };

    // each with how long it has left
    let mut shown = Vec::new();

    if !dnd {
        let notifications = Notifications::read();

        for notification in notifications.list().iter().rev() {
            if hidden.contains(&notification.id()) {
                continue;
            }

            let limit = if notification.urgency() == Urgency::Critical { 20 } else { TOAST_SECONDS };

            let age = SystemTime::now().duration_since(notification.received()).unwrap_or_default();

            if age.as_secs() < limit {
                shown.push((notification.id(), Duration::from_secs(limit).saturating_sub(age)));
            }
        }
    }

    shown.truncate(SLOTS);

    let due = shown.iter().map(|(_, left)| *left).min();
    let ids: Vec<u32> = shown.into_iter().map(|(id, _)| id).collect();

    // a new popup appears on the monitor you're looking at
    let monitor = ids.iter().any(|id| !previous.contains(id)).then(crate::panels::focused_output);

    if ids == previous && monitor.is_none() {
        return due;
    }

    let mut notify = Notify::write();

    // one that was dismissed while this round was running stays dismissed
    let kept: Vec<u32> = ids.into_iter().filter(|id| !notify.hidden.contains(id)).collect();

    notify.toasts = kept;

    if let Some(monitor) = monitor {
        notify.monitor = monitor;
    }

    due
}

/*
 * what the toast windows last saw: how many notifications there are, when the latest arrived (a replacement keeps its
 * id and place but arrives again), do-not-disturb, and how many were dismissed. `toast()` runs whenever either service
 * changes, even with no toast on screen, so it's where the toast thread learns there's something new to look at
 */
#[derive(PartialEq)]
struct Seen {
    count: usize,
    latest: Option<SystemTime>,
    dnd: bool,
    hidden: usize,
}

struct Watch {
    seen: Option<Seen>,
    news: bool, // something changed since the toast thread last looked
}

static WATCH: Mutex<Watch> = Mutex::new(Watch { seen: None, news: false });
static NEWS: Condvar = Condvar::new();

fn noticed(notify: &Notify, notifications: &Notifications) {
    let list = notifications.list();

    let seen = Seen {
        count: list.len(),
        latest: list.iter().map(Notification::received).max(),
        dnd: notify.dnd,
        hidden: notify.hidden.len(),
    };

    let mut watch = crate::sync::lock(&WATCH);

    // the toast thread's own changes (which toasts, which monitor) aren't in `Seen`, so they don't wake it again
    if watch.seen.as_ref() != Some(&seen) {
        watch.seen = Some(seen);
        watch.news = true;

        NEWS.notify_one();
    }
}

// news that came while the toast thread was busy is kept in `news`, so it returns at once instead of missing it
fn wait_for_news(due: Option<Duration>) {
    let watch = crate::sync::lock(&WATCH);

    let mut watch = match due {
        Some(due) => NEWS.wait_timeout_while(watch, due, |watch| !watch.news).unwrap_or_else(PoisonError::into_inner).0,
        None => NEWS.wait_while(watch, |watch| !watch.news).unwrap_or_else(PoisonError::into_inner),
    };

    watch.news = false;
}

// ---------------------------------------------------------------- toasts

pub const TOAST_WIDTH: f32 = 372.0;
pub const TOAST_HEIGHT: f32 = 84.0;

pub fn toast_0(monitor: &Monitor) -> LayerWindow {
    toast(0, monitor)
}

pub fn toast_1(monitor: &Monitor) -> LayerWindow {
    toast(1, monitor)
}

pub fn toast_2(monitor: &Monitor) -> LayerWindow {
    toast(2, monitor)
}

fn toast(slot: usize, monitor: &Monitor) -> LayerWindow {
    let t = theme();
    let notify = Notify::read();
    let notifications = Notifications::read();

    noticed(&notify, &notifications);

    // while the center is open the same notifications are already on screen
    // and dropdowns open in the same corner
    let center_open = [Which::Notifications, Which::Power, Which::Network, Which::TrayMenu, Which::Audio, Which::Bluetooth, Which::Clipboard].iter().any(|which| panels::shown(*which, monitor));
    let here = notify.monitor.is_empty() || notify.monitor == monitor.name;

    let shown = notify.toasts.get(slot).and_then(|id| notifications.list().iter().find(|item| item.id() == *id));

    let top = GAP + slot as i32 * (TOAST_HEIGHT as i32 + GAP);

    let window = LayerWindow::new()
        .width(TOAST_WIDTH)
        .height(TOAST_HEIGHT)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Right)
        .margin(amane::Margin { top, right: GAP, bottom: 0, left: 0 })
        .layer(amane::Layer::Overlay)
        .space(Zone::Respect)
        .namespace("amane-panel")
        .visible(shown.is_some() && !center_open && here);

    match shown {
        Some(notification) => window.child(card(&t, notification, true)),
        None => window.child(Rectangle::new().width(Parent).height(Parent)),
    }
}

// ---------------------------------------------------------------- cards

fn avatar(t: &Theme, notification: &Notification) -> Rectangle {
    let base = Rectangle::new().width(40.0).height(40.0).radius(0.0);

    /*
     * a sender's own picture (an avatar) wins over the app icon. pictures a sender gives are decoded small, at about
     * the size they're drawn: a full-size decode stays as long as the notification is in the center's history, and a
     * screenshot is 24 MB at 3000 x 2000. small copies are kept for good, a few KB each
     */
    if let Some(image) = crate::icons::resolve(notification.image()) {
        return base.fill(Image::cover(image).thumbnail(PICTURE_SIZE, PICTURE_SIZE));
    }

    let app_icon = crate::icons::resolve(notification.icon()).or_else(|| crate::icons::resolve(&notification.app_name().to_lowercase()));

    if let Some(path) = app_icon {
        // a sender can give a file of its own as the icon, like a screenshot tool showing the screenshot
        let own_file = notification.icon().trim().trim_start_matches("file://").starts_with('/');

        let picture = if own_file { Image::contain(&path).thumbnail(PICTURE_SIZE, PICTURE_SIZE) } else { crate::ui::app_icon(&path) };

        return base.fill(t.surface_hi).padding(7.0).child(Rectangle::new().width(26.0).height(26.0).fill(picture));
    }

    let critical = notification.urgency() == Urgency::Critical;

    base.fill(if critical { alpha(t.red, 0.2) } else { t.accent_soft })
        .align_child(Center, Center)
        .child(icon("\u{f0f3}", 16.0, if critical { t.red } else { t.accent }))
}

/// one notification. `toast` cards sit alone on the desktop, the others sit in the center
fn card(t: &Theme, notification: &Notification, toast: bool) -> Rectangle {
    let id = notification.id();
    let hot = hover(&format!("n{id}"));
    let key = format!("n{id}");

    let width = if toast { TOAST_WIDTH - 28.0 } else { 380.0 - 28.0 - 28.0 };

    let text_width = width - 40.0 - 12.0 - 26.0;

    let head = Row::new(children![
        Rectangle::new()
            .width(text_width - 36.0)
            .height(14.0)
            .child(label(notification.app_name().to_uppercase(), 10.0, t.faint).elide()),
        Rectangle::new().width(36.0).height(14.0).align_child(End, Start).child(label(ago(notification.received()), 10.5, t.faint)),
    ]);

    let summary = Rectangle::new().width(text_width).height(17.0).child(bold(notification.summary(), 12.5, t.fg).elide());

    let body = Rectangle::new().width(text_width).height(30.0).child(label(strip_markup(notification.body()), 11.5, t.dim).wrap().max_lines(2));

    let content = Row::new(children![
        avatar(t, notification),
        Column::new(children![head, summary, body]).gap(1.0),
        close_button(t, id),
    ])
    .gap(12.0);

    let fill = if toast { alpha(t.panel, 0.0) } else { mix(t.surface, t.surface_hi, hot) };

    let rectangle = Rectangle::new()
        .width(if toast { TOAST_WIDTH } else { 380.0 - 28.0 })
        .height(if toast { TOAST_HEIGHT } else { 80.0 })
        .radius(RADIUS)
        .fill(fill)
        .padding(if toast { 13.0 } else { 12.0 })
        .align_child(Start, Center)
        .cursor(amane::Pointer)
        .on_hover(move |inside| set_hover(&key, inside))
        .on_click(move |_| {
            Notify::write().hide(id);

            Notifications::click(id);
        });

    if toast {
        // toasts are their own window, so they carry their own panel look
        rectangle.fill(t.panel).border(1.0, t.border).child(content)
    } else {
        rectangle.child(content)
    }
}

impl Notify {
    pub fn hide(&mut self, id: u32) {
        self.hidden.insert(id);
        self.toasts.retain(|shown| *shown != id);
    }
}

fn close_button(t: &Theme, id: u32) -> Rectangle {
    round_button(&format!("x{id}"), t, "\u{f00d}", t.faint, move |_| {
        Notify::write().hide(id);

        Notifications::dismiss(id);
    })
}

// notification bodies may hold <b> and <a href> markup and entities like &amp; and &#39;
pub fn strip_markup(text: &str) -> String {
    let mut tagless = String::new();
    let mut inside = false;

    for letter in text.chars() {
        match letter {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => tagless.push(letter),
            _ => {}
        }
    }

    unescape(&tagless)
}

// one pass, so "&amp;lt;" becomes "&lt;" (what the sender meant) and not "<"
fn unescape(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find('&') {
        result.push_str(&rest[..start]);

        let after = &rest[start + 1..];

        let decoded = after.find(';').filter(|end| *end <= 8).and_then(|end| {
            let name = &after[..end];

            let letter = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name
                    .strip_prefix('#')
                    .and_then(|number| match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => number.parse().ok(),
                    })
                    .and_then(char::from_u32),
            }?;

            Some((letter, end))
        });

        match decoded {
            Some((letter, end)) => {
                result.push(letter);

                rest = &after[end + 1..];
            }
            None => {
                result.push('&');

                rest = after;
            }
        }
    }

    result.push_str(rest);

    result
}

// ---------------------------------------------------------------- center

const CENTER_WIDTH: f32 = 380.0;

pub fn center(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::Notifications, monitor) else {
        return panels::hidden("amane-panel");
    };
    let notifications = Notifications::read();
    let notify = Notify::read();

    let count = notifications.list().len();

    let header = Row::new(children![
        bold("Notifications", 15.0, t.fg),
        Rectangle::new().width(Parent).height(1.0),
        chip("clear", &t, 76.0, label("Clear all", 11.5, t.dim)).on_click(|_| {
            Notifications::clear();
        }),
        chip("dnd", &t, 34.0, icon(if notify.dnd { "\u{f1f6}" } else { "\u{f0f3}" }, 12.5, if notify.dnd { t.accent } else { t.dim }))
            .on_click(|_| {
                let on = Notify::read().dnd;

                Notify::write().dnd = !on;
            }),
    ])
    .gap(6.0)
    .align(Center)
    .width(CENTER_WIDTH - 28.0);

    let mut cards: Vec<Box<dyn Widget>> = Vec::new();

    for notification in notifications.list().iter().rev() {
        cards.push(Box::new(card(&t, notification, false)));
    }

    let list_height = if count == 0 { 150.0 } else { (count.min(6) as f32) * 80.0 + (count.min(6) as f32 - 1.0) * 8.0 };

    let list: Box<dyn Widget> = if count == 0 {
        Box::new(
            Rectangle::new()
                .width(CENTER_WIDTH - 28.0)
                .height(list_height)
                .align_child(Center, Center)
                .child(Column::new(children![
                    icon("\u{f1f6}", 30.0, t.faint),
                    label("You're all caught up", 12.5, t.faint),
                ])
                .gap(10.0)
                .align(Center)),
        )
    } else {
        Box::new(
            Rectangle::new()
                .width(CENTER_WIDTH - 28.0)
                .height(list_height)
                .clip()
                .child(ScrollArea::new("notification-list", Column::new(cards).gap(8.0))),
        )
    };

    let height = 28.0 + 30.0 + 12.0 + list_height;

    panels::window(CENTER_WIDTH, height, Horizontal::Right)
        .child(panels::body(&t).padding(14.0).child(Column::new(vec![Box::new(header), list]).gap(12.0)))
}

#[cfg(test)]
mod tests {
    use super::strip_markup;

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(strip_markup("Battery at 20%"), "Battery at 20%");
    }

    #[test]
    fn tags_are_removed() {
        assert_eq!(strip_markup("<b>Bold</b> and <a href=\"https://example.com\">a link</a>"), "Bold and a link");
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(strip_markup("Tom &amp; Jerry &lt;3 &quot;quotes&quot; &apos;x&apos;"), "Tom & Jerry <3 \"quotes\" 'x'");
        assert_eq!(strip_markup("it&#39;s &#x41;&#X42; &#128512;"), "it's AB \u{1f600}");
    }

    #[test]
    fn decoding_happens_once() {
        assert_eq!(strip_markup("&amp;lt;"), "&lt;");
        assert_eq!(strip_markup("&amp;amp;"), "&amp;");
    }

    #[test]
    fn decoded_markup_is_not_stripped() {
        // the sender escaped it, so it's text and not a tag
        assert_eq!(strip_markup("a &lt;b&gt; c"), "a <b> c");
    }

    #[test]
    fn stray_ampersands_stay() {
        assert_eq!(strip_markup("R&D"), "R&D");
        assert_eq!(strip_markup("a & b"), "a & b");
        assert_eq!(strip_markup("&nosuch; &#xZZ; &#99999999999;"), "&nosuch; &#xZZ; &#99999999999;");
        assert_eq!(strip_markup("trailing &"), "trailing &");
    }
}
