use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use amane::{Center, Column, Horizontal, Image, Key, Keyboard, LayerWindow, Monitor, Parent, Rectangle, Row, Service, Start, TextInput, Widget, children};

use crate::cache;
use crate::panels::{self, Which};
use crate::theme::{Theme, alpha, mix};
use crate::ui::{Wheel, chip, hover, icon, label, round_button, set_hover};

/*
 * the clipboard history, kept by cliphist. two `wl-paste --watch cliphist store` processes (started by niri, see the
 * config) save everything that's copied, text and images, in a database that survives restarts and works while this
 * shell isn't running. wl-paste tells cliphist when a password manager marked the copy as secret, and cliphist skips
 * those. the shell only reads the list while the dropdown is open and asks cliphist to decode, delete and wipe
 */
#[derive(Clone, PartialEq)]
pub struct Entry {
    line: String,         // what cliphist printed: "<id>\t<preview>", which is also what it takes back
    id: String,
    pub preview: String,
    pub image: bool,
    pub thumbnail: String, // a small jpeg of an image entry, empty until it's made
}

pub struct Clipboard {
    pub entries: Vec<Entry>, // newest first
    pub available: bool,     // false when cliphist isn't installed
    pub loaded: bool,        // the first look at the history has finished
    pub query: String,
    pub selected: usize, // an index into what matches the query
    pub offset: usize,   // the first row of those that is on screen
    wheel: Wheel,        // slides the rows
}

const KEPT: usize = 200;

fn cliphist(arguments: &[&str]) -> Option<std::process::Output> {
    Command::new("cliphist").args(arguments).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()
}

fn thumbnail(entry: &Entry) -> Option<PathBuf> {
    let file = cache::runtime_dir()?.join(format!("clip-{}.jpg", entry.id));

    if file.exists() {
        return Some(file);
    }

    // cliphist decode writes the image, ImageMagick shrinks it. a screenshot can be 4K, far too big to draw in a row
    let mut decode = Command::new("cliphist").arg("decode").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;

    let made = shrink(&mut decode, entry, &file);

    // whatever happened above (ImageMagick missing, a failed write), the child is collected and doesn't linger
    let _ = decode.wait();

    made.then_some(file)
}

fn shrink(decode: &mut std::process::Child, entry: &Entry, file: &std::path::Path) -> bool {
    let Some(mut stdin) = decode.stdin.take() else { return false };

    if std::io::Write::write_all(&mut stdin, entry.line.as_bytes()).is_err() {
        return false;
    }

    drop(stdin);

    let Some(output) = decode.stdout.take() else { return false };

    let partial = file.with_extension("part");

    let made = Command::new("magick")
        .args(["-", "-auto-orient", "-thumbnail", "96x96^", "-gravity", "center", "-extent", "96x96", "-quality", "80"])
        .arg(format!("jpeg:{}", partial.display()))
        .stdin(Stdio::from(output))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());

    // renamed when whole, so a half-made thumbnail is never drawn
    made && std::fs::rename(&partial, file).is_ok()
}

/// the history as cliphist has it, without thumbnails. quick, so the list can be shown at once
fn read_list() -> Option<Vec<Entry>> {
    let output = cliphist(&["-preview-width", "120", "list"])?;

    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .take(KEPT)
            .filter_map(|line| {
                let (id, preview) = line.split_once('\t')?;

                Some(Entry {
                    line: line.to_string(),
                    id: id.to_string(),
                    preview: preview.to_string(),
                    image: preview.starts_with("[[ binary data"),
                    thumbnail: String::new(),
                })
            })
            .collect(),
    )
}

impl Service for Clipboard {
    fn new() -> Self {
        Self { entries: Vec::new(), available: true, loaded: false, query: String::new(), selected: 0, offset: 0, wheel: Wheel::new() }
    }

    // only while the dropdown is open (the first read starts it)
    fn listen() {
        // images that couldn't be converted, so they aren't decoded again every second
        let mut failed: HashSet<String> = HashSet::new();

        loop {
            panels::wait_until_open(&[Which::Clipboard]);

            refresh(&mut failed);

            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

/*
 * the list is published as soon as cliphist has it, and the thumbnails of the images near the top follow one at a time:
 * each takes a decode and an ImageMagick run, which for a few screenshots is seconds
 */
fn refresh(failed: &mut HashSet<String>) {
    let Some(mut entries) = read_list() else {
        let mut clipboard = Clipboard::write();

        clipboard.available = false;
        clipboard.loaded = true;

        return;
    };

    let changed = {
        let clipboard = Clipboard::read();

        // thumbnails that were already made stay on their entries
        for entry in &mut entries {
            if let Some(known) = clipboard.entries.iter().find(|known| known.id == entry.id) {
                entry.thumbnail = known.thumbnail.clone();
            }
        }

        clipboard.entries != entries || !clipboard.loaded || !clipboard.available
    };

    if changed {
        let mut clipboard = Clipboard::write();

        clipboard.entries = entries;
        clipboard.available = true;
        clipboard.loaded = true;
    }

    fill_thumbnails(failed);
}

fn fill_thumbnails(failed: &mut HashSet<String>) {
    // the images on screen now, wherever the rows have been scrolled to
    let offset = Clipboard::read().offset;

    let wanted: Vec<Entry> = matching()
        .into_iter()
        .skip(offset)
        .take(ROWS)
        .filter(|entry| entry.image && entry.thumbnail.is_empty() && !failed.contains(&entry.id))
        .collect();

    for entry in wanted {
        match thumbnail(&entry) {
            Some(file) => {
                if let Some(shown) = Clipboard::write().entries.iter_mut().find(|shown| shown.id == entry.id) {
                    shown.thumbnail = file.to_string_lossy().into_owned();
                }
            }
            None => {
                failed.insert(entry.id.clone());
            }
        }
    }

    let (keep, present): (HashSet<PathBuf>, HashSet<String>) = {
        let clipboard = Clipboard::read();

        (
            clipboard.entries.iter().filter(|entry| !entry.thumbnail.is_empty()).map(|entry| PathBuf::from(&entry.thumbnail)).collect(),
            clipboard.entries.iter().map(|entry| entry.id.clone()).collect(),
        )
    };

    cache::prune("clip-", &keep);

    // an id that comes back (the history was wiped) gets another try
    failed.retain(|id| present.contains(id));
}

/// opening it starts from an empty search
pub fn reset() {
    {
        let mut clipboard = Clipboard::write();

        clipboard.query.clear();
        clipboard.selected = 0;
        clipboard.offset = 0;
    }

    TextInput::set_text("clipboard-query", "");
}

fn copy(entry: &Entry) {
    let line = entry.line.clone();

    std::thread::spawn(move || {
        let Ok(mut decode) = Command::new("cliphist").arg("decode").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else { return };

        let Some(mut stdin) = decode.stdin.take() else { return };
        let _ = std::io::Write::write_all(&mut stdin, line.as_bytes());
        drop(stdin);

        let Some(output) = decode.stdout.take() else { return };

        // wl-copy works out the type (text or image) from what it's given, and stays around to serve the paste
        let _ = Command::new("wl-copy").stdin(Stdio::from(output)).stdout(Stdio::null()).stderr(Stdio::null()).status();

        let _ = decode.wait();
    });

    panels::close();
}

fn forget(entry: &Entry) {
    let line = entry.line.clone();

    // gone from the list at once, the database follows
    Clipboard::write().entries.retain(|other| other.line != line);

    std::thread::spawn(move || {
        if let Ok(mut delete) = Command::new("cliphist").arg("delete").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            if let Some(mut stdin) = delete.stdin.take() {
                let _ = std::io::Write::write_all(&mut stdin, line.as_bytes());
            }

            let _ = delete.wait();
        }
    });
}

fn wipe() {
    Clipboard::write().entries.clear();

    std::thread::spawn(|| {
        let _ = Command::new("cliphist").arg("wipe").stdout(Stdio::null()).stderr(Stdio::null()).status();
    });
}

fn matching() -> Vec<Entry> {
    let clipboard = Clipboard::read();

    let query = clipboard.query.to_lowercase();

    clipboard.entries.iter().filter(|entry| query.is_empty() || entry.preview.to_lowercase().contains(&query)).cloned().collect()
}

fn copy_selected() {
    let selected = Clipboard::read().selected;

    if let Some(entry) = matching().get(selected) {
        copy(entry);
    }
}

// how many rows fit on screen. the history is longer, the rows slide over it
const ROWS: usize = 8;

impl Clipboard {
    // keeps the chosen row on screen
    fn show_selected(&mut self) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + ROWS {
            self.offset = self.selected + 1 - ROWS;
        }
    }

    fn choose(&mut self, index: usize) {
        self.selected = index;
        self.show_selected();
    }

    /// the wheel moves the rows one notch at a time, down goes to older entries
    fn scroll(&mut self, amount: f32, total: usize) {
        let last = total.saturating_sub(ROWS);

        self.offset = (self.offset as isize + self.wheel.steps(amount) as isize).clamp(0, last as isize) as usize;
    }
}

/// moves the chosen row up or down, around the ends
pub fn step(by: isize) {
    let total = matching().len() as isize;

    if total == 0 {
        return;
    }

    let mut clipboard = Clipboard::write();

    let next = (clipboard.selected as isize + by).rem_euclid(total) as usize;

    clipboard.choose(next);
}

fn on_key(key: Key) {
    match key {
        Key::Escape => panels::close(),
        Key::Down | Key::Tab => step(1),
        Key::Up => step(-1),
        _ => {}
    }
}

const WIDTH: f32 = 420.0;
const INNER: f32 = WIDTH - 28.0;
const ROW: f32 = 44.0;

fn row(t: &Theme, index: usize, entry: &Entry, chosen: bool) -> Rectangle {
    let id = format!("clip-{index}");
    let hot = hover(&id);

    let copied = entry.clone();
    let removed = entry.clone();

    // an image gets its thumbnail, a text its first line
    let lead: Box<dyn Widget> = if entry.image {
        if entry.thumbnail.is_empty() {
            Box::new(Rectangle::new().width(30.0).height(30.0).fill(t.surface_hi).align_child(Center, Center).child(icon("\u{f03e}", 13.0, t.faint)))
        } else {
            Box::new(Rectangle::new().width(30.0).height(30.0).fill(Image::cover(&entry.thumbnail)))
        }
    } else {
        Box::new(Rectangle::new().width(30.0).height(30.0).align_child(Center, Center).child(icon("\u{f0f6}", 13.0, t.faint)))
    };

    // cliphist describes an image as "[[ binary data 4 KiB png 1200x800 ]]"
    let text = if entry.image { entry.preview.trim_start_matches("[[ binary data").trim_end_matches("]]").trim().to_string() } else { entry.preview.clone() };

    let text = if entry.image { format!("Image \u{b7} {text}") } else { text };

    Rectangle::new()
        .width(INNER)
        .height(ROW)
        .fill(if chosen { alpha(t.accent, 0.20) } else { mix(alpha(t.surface, 0.0), t.surface_hi, hot) })
        .border(if chosen { 1.0 } else { 0.0 }, alpha(t.accent, 0.45))
        .padding(amane::Padding { top: 0.0, right: 4.0, bottom: 0.0, left: 8.0 })
        .align_child(Start, Center)
        .cursor(amane::Pointer)
        .on_hover(move |inside| set_hover(&id, inside))
        // the choice follows the pointer only when it moves, so rows sliding under a parked pointer (typing, scrolling)
        // don't take the choice away from the keyboard. a write marks the service changed even when nothing changed,
        // and a pointer sends many events, so look first
        .on_move(move |_| {
            if Clipboard::read().selected != index {
                Clipboard::write().selected = index;
            }
        })
        .on_click(move |_| copy(&copied))
        .child(
            Row::new(vec![
                lead,
                Box::new(Rectangle::new().width(INNER - 8.0 - 4.0 - 30.0 - 30.0 - 30.0).height(18.0).child(label(text, 12.5, t.fg).elide())),
                Box::new(Rectangle::new().width(Parent).height(1.0)),
                Box::new(round_button(&format!("clip-x-{index}"), t, "\u{f00d}", t.faint, move |_| forget(&removed))),
            ])
            .gap(10.0)
            .align(Center)
            .width(INNER - 12.0),
        )
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::Clipboard, monitor) else {
        return panels::hidden("amane-panel");
    };

    let (selected, offset, empty_history, available, loaded) = {
        let clipboard = Clipboard::read();

        (clipboard.selected, clipboard.offset, clipboard.entries.is_empty(), clipboard.available, clipboard.loaded)
    };

    let found = matching();

    // the rows slide over the whole list, so every entry can be reached
    let offset = offset.min(found.len().saturating_sub(ROWS));

    let mut rows: Vec<Box<dyn Widget>> = Vec::new();

    for (position, entry) in found.iter().enumerate().skip(offset).take(ROWS) {
        rows.push(Box::new(row(&t, position, entry, position == selected)));
    }

    let list_height;

    if rows.is_empty() {
        list_height = 80.0;

        rows.push(Box::new(
            Rectangle::new().width(INNER).height(list_height).align_child(Center, Center).child(label(
                if !loaded {
                    "Loading\u{2026}"
                } else if !available {
                    "cliphist isn't installed"
                } else if empty_history {
                    "Nothing copied yet"
                } else {
                    "No matches"
                },
                12.5,
                t.faint,
            )),
        ));
    } else {
        let count = rows.len() as f32;

        list_height = count * ROW + (count - 1.0) * 2.0;
    }

    let field = Rectangle::new()
        .width(INNER)
        .height(44.0)
        .fill(mix(t.surface, t.surface_hi, 0.4))
        .border(1.0, alpha(t.accent, 0.5))
        .padding(amane::Padding { top: 0.0, right: 14.0, bottom: 0.0, left: 14.0 })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                icon("\u{f0ea}", 14.0, t.accent),
                TextInput::new("clipboard-query")
                    .placeholder("Search the clipboard history")
                    .focused()
                    .size(15.0)
                    .color(t.fg)
                    .width(INNER - 28.0 - 14.0 - 14.0)
                    .on_change(|text| {
                        let mut clipboard = Clipboard::write();

                        clipboard.query = text;
                        clipboard.selected = 0;
                        clipboard.offset = 0;
                    })
                    .on_submit(|_| copy_selected()),
            ])
            .gap(14.0)
            .align(Center),
        );

    let position = if found.is_empty() { String::new() } else { format!("{} / {}", selected.min(found.len() - 1) + 1, found.len()) };

    let footer = Row::new(children![
        label("\u{2191} \u{2193} choose     enter copy     esc close", 11.0, t.faint),
        Rectangle::new().width(Parent).height(1.0),
        label(position, 11.0, t.faint),
        chip("clip-clear", &t, 70.0, label("Clear all", 11.0, t.dim)).on_click(|_| wipe()),
    ])
    .gap(12.0)
    .align(Center)
    .width(INNER);

    let height = 28.0 + 44.0 + 10.0 + list_height + 10.0 + 26.0;

    let total = found.len();

    panels::window_at(WIDTH, height, Horizontal::Right, crate::bar::right_edge(crate::bar::Slot::Clipboard))
        .keyboard(Keyboard::Exclusive)
        .on_key(on_key)
        .child(
            panels::body(&t).padding(14.0).child(
                Column::new(vec![Box::new(field) as Box<dyn Widget>, Box::new(
                        Rectangle::new()
                            .width(INNER)
                            .height(list_height)
                            .on_scroll(move |scroll| Clipboard::write().scroll(scroll.y, total))
                            .child(Column::new(rows).gap(2.0)),
                    ),
                    Box::new(footer),
                ])
                .gap(10.0),
            ),
        )
}
