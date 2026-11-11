use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use amane::{
    Apps, Center, Column, DesktopApp, Horizontal, Key, Keyboard, Layer, LayerWindow, Margin, Monitor, Parent, Rectangle, Row, Service, Start,
    TextInput, Vertical, Widget, Zone, children,
};

use crate::panels::{self, Which};
use crate::theme::{alpha, mix};
use crate::ui::{Wheel, bold, icon, label};

/*
 * the app launcher: type to filter, up/down to choose, enter to start.
 * apps you start often float to the top, the counts are kept in ~/.local/state/amane-shell/launch-counts
 */
pub struct Launcher {
    pub query: String,
    pub selected: usize,
    pub grid: bool,        // the "all apps" grid instead of the ranked list
    pub grid_offset: usize, // the first grid row that is on screen
    wheel: Wheel,           // slides the grid
    counts: HashMap<String, u32>,
}

const WIDTH: f32 = 640.0;
const ROW: f32 = 46.0;
const MAX_ROWS: usize = 8;

// the all-apps grid: icons in columns, a few rows on screen, the rest slides under the wheel
const COLUMNS: usize = 6;
const GRID_ROWS: usize = 4;
const CELL_WIDTH: f32 = 96.0;
const CELL_HEIGHT: f32 = 100.0;

// the button next to the search box that opens the grid
const SWITCH: f32 = 112.0;

// how often each app was started. state, not cache: a cache cleaner would reset the order of the list
fn counts_file() -> std::path::PathBuf {
    crate::cache::state_dir().join("launch-counts")
}

impl Service for Launcher {
    fn new() -> Self {
        let counts = std::fs::read_to_string(counts_file())
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let (count, name) = line.split_once('\t')?;

                Some((name.to_string(), count.parse().ok()?))
            })
            .collect();

        Self { query: String::new(), selected: 0, grid: false, grid_offset: 0, wheel: Wheel::new(), counts }
    }

    fn interval() -> Duration {
        Duration::from_secs(3600)
    }
}

impl Launcher {
    fn open_grid(&mut self, grid: bool) {
        self.grid = grid;
        self.selected = 0;
        self.grid_offset = 0;
    }

    // keeps the chosen cell's row on screen
    fn show_selected(&mut self) {
        let row = self.selected / COLUMNS;

        if row < self.grid_offset {
            self.grid_offset = row;
        } else if row >= self.grid_offset + GRID_ROWS {
            self.grid_offset = row + 1 - GRID_ROWS;
        }
    }

    /// the wheel slides the grid one row per notch, down goes towards Z
    fn scroll_grid(&mut self, amount: f32, rows: usize) {
        let last = rows.saturating_sub(GRID_ROWS);

        self.grid_offset = (self.grid_offset as isize + self.wheel.steps(amount) as isize).clamp(0, last as isize) as usize;
    }

    fn remember(&mut self, name: &str) {
        *self.counts.entry(name.to_string()).or_insert(0) += 1;

        let text: String = self.counts.iter().map(|(name, count)| format!("{count}\t{name}\n")).collect();

        // written on another thread (this runs under the lock, on the drawing thread) and under another name, then renamed,
        // so a crash halfway through can't leave half a file and lose the counts
        std::thread::spawn(move || {
            let file = counts_file();

            if let Some(folder) = file.parent() {
                let _ = std::fs::create_dir_all(folder);
            }

            let partial = file.with_extension("part");

            if std::fs::write(&partial, text).is_ok() {
                let _ = std::fs::rename(&partial, &file);
            }
        });
    }
}

/// (for the ipc command and tests) opens the grid directly
pub fn open_grid_now(selected: usize) {
    let mut launcher = Launcher::write();

    launcher.open_grid(true);
    launcher.selected = selected;
    launcher.show_selected();
}

/// opening the launcher starts from a clean search
pub fn reset() {
    {
        let mut launcher = Launcher::write();

        launcher.query.clear();
        launcher.open_grid(false);
    }

    TextInput::set_text("launcher-query", "");
}

// lower is better. None means it doesn't match at all
fn score(query: &str, app: &DesktopApp) -> Option<u32> {
    rank(query, app.name(), &format!("{} {}", app.description().unwrap_or(""), app.exec()))
}

// `query` is lowercase. `extra` is the text besides the name that a search may also find (description, command)
fn rank(query: &str, name: &str, extra: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }

    let name = name.to_lowercase();

    if name.starts_with(query) {
        return Some(0);
    }

    if name.contains(&format!(" {query}")) {
        return Some(1);
    }

    if name.contains(query) {
        return Some(2);
    }

    if extra.to_lowercase().contains(query) {
        return Some(4);
    }

    // letters in order, so "ffx" finds Firefox
    let mut letters = query.chars();
    let mut wanted = letters.next();

    for letter in name.chars() {
        if Some(letter) == wanted {
            wanted = letters.next();
        }
    }

    wanted.is_none().then_some(6)
}

// the ranked list: best match first, then the apps you start most, then A to Z
fn matches(query: &str, apps: &[DesktopApp], counts: &HashMap<String, u32>) -> Vec<DesktopApp> {
    let mut found: Vec<(u32, std::cmp::Reverse<u32>, String, &DesktopApp)> = Vec::new();

    for app in apps {
        if let Some(rank) = score(query, app) {
            let used = counts.get(app.name()).copied().unwrap_or(0);

            found.push((rank, std::cmp::Reverse(used), app.name().to_lowercase(), app));
        }
    }

    found.sort_by(|a, b| (a.0, &a.1, &a.2).cmp(&(b.0, &b.1, &b.2)));

    found.into_iter().map(|(_, _, _, app)| app.clone()).collect()
}

/// every app that matches the search, A to Z, for the grid
fn alphabetical(query: &str, apps: &[DesktopApp]) -> Vec<DesktopApp> {
    let mut found: Vec<(String, &DesktopApp)> =
        apps.iter().filter(|app| score(query, app).is_some()).map(|app| (app.name().to_lowercase(), app)).collect();

    found.sort_by(|a, b| a.0.cmp(&b.0));

    found.into_iter().map(|(_, app)| app.clone()).collect()
}

// what a list was made from: the search, list or grid, the installed apps (Amane replaces its list when they change,
// so its address and length tell) and how many launches have been counted
#[derive(PartialEq)]
struct Source {
    query: String,
    grid: bool,
    apps: (usize, usize),
    launches: u32,
}

thread_local! {
    static LISTED: RefCell<Option<(Source, Rc<Vec<DesktopApp>>)>> = const { RefCell::new(None) };
}

/*
 * what's on screen, in the order the keyboard walks through it. ranking every installed app is the same work on every
 * frame (hover fades redraw at display rate) and for every key, so the list is kept until what it was made from changes
 */
fn current() -> Rc<Vec<DesktopApp>> {
    let apps = Apps::read();
    let launcher = Launcher::read();

    let source = Source {
        query: launcher.query.to_lowercase(),
        grid: launcher.grid,
        apps: (apps.list().as_ptr() as usize, apps.list().len()),
        launches: launcher.counts.values().sum(),
    };

    LISTED.with(|listed| {
        let mut listed = listed.borrow_mut();

        if let Some((made_from, list)) = listed.as_ref()
            && *made_from == source
        {
            return Rc::clone(list);
        }

        let list = Rc::new(if source.grid { alphabetical(&source.query, apps.list()) } else { matches(&source.query, apps.list(), &launcher.counts) });

        *listed = Some((source, Rc::clone(&list)));

        list
    })
}

fn start(app: &DesktopApp) {
    Launcher::write().remember(app.name());

    app.launch();

    panels::close();
}

fn start_selected() {
    let selected = Launcher::read().selected;

    if let Some(app) = current().get(selected) {
        start(app);
    }
}

fn on_key(key: Key) {
    if key == Key::Escape {
        panels::close();

        return;
    }

    let grid = Launcher::read().grid;

    // the list wraps around its few rows, the grid walks the whole alphabet. Left and Right belong to the search box,
    // so the grid moves by row with Up and Down and by cell with Tab
    let by = match (key, grid) {
        (Key::Down, true) => COLUMNS as isize,
        (Key::Up, true) => -(COLUMNS as isize),
        (Key::Down | Key::Tab, _) => 1,
        (Key::Up, false) => -1,
        _ => return,
    };

    let total = if grid { current().len() } else { current().len().min(MAX_ROWS) };

    if total == 0 {
        return;
    }

    let mut launcher = Launcher::write();

    let next = launcher.selected as isize + by;

    launcher.selected = if grid {
        // by row: stop at the ends instead of wrapping to the other side of the alphabet
        next.clamp(0, total as isize - 1) as usize
    } else {
        next.rem_euclid(total as isize) as usize
    };

    launcher.show_selected();
}

fn toggle_grid() {
    let mut launcher = Launcher::write();

    let grid = !launcher.grid;

    launcher.open_grid(grid);
}

// one icon with its name under it
fn cell(t: &crate::theme::Theme, index: usize, app: &DesktopApp, chosen: bool) -> Rectangle {
    let launched = app.clone();

    let name = app.name();

    let picture = {
        let base = Rectangle::new().width(44.0).height(44.0);

        match app.icon_path() {
            Some(path) => base.fill(crate::ui::app_icon(path)),
            // an app without an icon gets its first letter
            None => base
                .fill(t.surface_hi)
                .align_child(Center, Center)
                .child(bold(name.chars().next().unwrap_or('?').to_uppercase().to_string(), 18.0, t.dim)),
        }
    };

    // short names are centred, long ones are cut off with an ellipsis (which fills the width, so they start at the left)
    let caption = if name.chars().count() <= 13 {
        Rectangle::new().width(CELL_WIDTH - 8.0).height(28.0).align_child(Center, Start).child(label(name, 11.5, t.fg))
    } else {
        Rectangle::new().width(CELL_WIDTH - 8.0).height(28.0).child(label(name, 11.5, t.fg).elide())
    };

    Rectangle::new()
        .width(CELL_WIDTH)
        .height(CELL_HEIGHT)
        .fill(if chosen { alpha(t.accent, 0.20) } else { alpha(t.surface, 0.0) })
        .border(if chosen { 1.0 } else { 0.0 }, alpha(t.accent, 0.45))
        .padding(amane::Padding { top: 10.0, right: 4.0, bottom: 0.0, left: 4.0 })
        .align_child(Center, Start)
        .cursor(amane::Pointer)
        // the choice follows the pointer only when it moves (see the list rows)
        .on_move(move |_| {
            if Launcher::read().selected != index {
                Launcher::write().selected = index;
            }
        })
        .on_click(move |_| start(&launched))
        .child(Column::new(children![picture, caption]).gap(6.0).align(Center))
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::Launcher, monitor) else {
        return panels::hidden("amane-panel");
    };

    let (selected, grid, grid_offset, empty_query) = {
        let launcher = Launcher::read();

        (launcher.selected, launcher.grid, launcher.grid_offset, launcher.query.is_empty())
    };

    // ---- the search field and the button that switches between the list and the grid
    let field = Rectangle::new()
        .width(WIDTH - 28.0 - SWITCH - 8.0)
        .height(48.0)
        .fill(mix(t.surface, t.surface_hi, 0.4))
        .border(1.0, alpha(t.accent, 0.5))
        .padding(amane::Padding { top: 0.0, right: 16.0, bottom: 0.0, left: 16.0 })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                icon("\u{f002}", 14.0, t.accent),
                TextInput::new("launcher-query")
                    .placeholder("Search apps")
                    .focused()
                    .size(16.0)
                    .color(t.fg)
                    .width(WIDTH - 28.0 - SWITCH - 8.0 - 32.0 - 14.0 - 14.0)
                    .on_change(|text| {
                        let mut launcher = Launcher::write();

                        launcher.query = text;
                        launcher.selected = 0;
                        launcher.grid_offset = 0;
                    })
                    .on_submit(|_| start_selected()),
            ])
            .gap(14.0)
            .align(Center),
        );

    let hot = crate::ui::hover("launcher-grid");

    let switch = Rectangle::new()
        .width(SWITCH)
        .height(48.0)
        .fill(if grid { alpha(t.accent, 0.22) } else { mix(t.surface, t.surface_hi, 0.4 + 0.6 * hot) })
        .border(1.0, if grid { alpha(t.accent, 0.6) } else { alpha(t.fg, 0.14) })
        .align_child(Center, Center)
        .cursor(amane::Pointer)
        .on_hover(|inside| crate::ui::set_hover("launcher-grid", inside))
        .on_click(|_| toggle_grid())
        .child(
            Row::new(children![
                icon("\u{f00a}", 14.0, if grid { t.accent } else { t.dim }),
                label("All apps", 12.5, if grid { t.fg } else { t.dim }),
            ])
            .gap(8.0)
            .align(Center),
        );

    let top = Row::new(children![field, switch]).gap(8.0);

    // ---- the body: the ranked list, or every app A to Z
    let (heading, body, body_height): (String, Box<dyn Widget>, f32);

    if grid {
        let apps = current();

        let total_rows = apps.len().div_ceil(COLUMNS);

        // the rows slide over the whole alphabet, so every app can be reached
        let offset = grid_offset.min(total_rows.saturating_sub(GRID_ROWS));

        let mut rows: Vec<Box<dyn Widget>> = Vec::new();

        for row in offset..(offset + GRID_ROWS).min(total_rows) {
            let mut cells: Vec<Box<dyn Widget>> = Vec::new();

            for column in 0..COLUMNS {
                let index = row * COLUMNS + column;

                match apps.get(index) {
                    Some(app) => cells.push(Box::new(cell(&t, index, app, index == selected))),
                    None => cells.push(Box::new(Rectangle::new().width(CELL_WIDTH).height(CELL_HEIGHT))),
                }
            }

            rows.push(Box::new(Row::new(cells)));
        }

        body_height = GRID_ROWS as f32 * CELL_HEIGHT;

        heading = format!("All apps \u{b7} {}", apps.len());

        let content: Box<dyn Widget> = if apps.is_empty() {
            Box::new(Rectangle::new().width(WIDTH - 28.0).height(body_height).align_child(Center, Center).child(label("No matching apps", 12.5, t.faint)))
        } else {
            let grid_body = Column::new(rows);

            // a thin scrollbar on the right, only when there's more than fits
            let bar: Box<dyn Widget> = if total_rows > GRID_ROWS {
                let thumb = (body_height * GRID_ROWS as f32 / total_rows as f32).max(24.0);
                let free = body_height - thumb;
                let top = free * offset as f32 / (total_rows - GRID_ROWS) as f32;

                Box::new(
                    Rectangle::new()
                        .width(4.0)
                        .height(body_height)
                        .fill(alpha(t.fg, 0.06))
                        .padding(amane::Padding { top, right: 0.0, bottom: 0.0, left: 0.0 })
                        .child(Rectangle::new().width(4.0).height(thumb).fill(alpha(t.fg, 0.35))),
                )
            } else {
                Box::new(Rectangle::new().width(4.0).height(body_height))
            };

            Box::new(
                Row::new(vec![
                    Box::new(
                        Rectangle::new()
                            .width(CELL_WIDTH * COLUMNS as f32)
                            .height(body_height)
                            .on_scroll(move |scroll| Launcher::write().scroll_grid(scroll.y, total_rows))
                            .child(grid_body),
                    ) as Box<dyn Widget>,
                    Box::new(Rectangle::new().width(Parent).height(1.0)),
                    bar,
                ])
                .width(WIDTH - 28.0),
            )
        };

        body = content;
    } else {
        let found = current();
        let shown: Vec<&DesktopApp> = found.iter().take(MAX_ROWS).collect();

        let mut rows: Vec<Box<dyn Widget>> = Vec::new();

        for (index, app) in shown.iter().enumerate() {
            let chosen = index == selected;

            let picture = {
                let base = Rectangle::new().width(30.0).height(30.0);

                match app.icon_path() {
                    Some(path) => base.fill(crate::ui::app_icon(path)),
                    None => base,
                }
            };

            let launched = (*app).clone();

            let text = Column::new(children![
                bold(app.name(), 13.5, t.fg),
                Rectangle::new().width(WIDTH - 28.0 - 24.0 - 30.0 - 14.0 - 40.0).height(15.0).child(label(app.description().unwrap_or(""), 11.0, t.dim).elide()),
            ])
            .gap(1.0);

            let enter: Box<dyn Widget> = if chosen { Box::new(icon("\u{f0311}", 13.0, t.accent)) } else { Box::new(Rectangle::new().width(1.0).height(1.0)) };

            rows.push(Box::new(
                Rectangle::new()
                    .width(WIDTH - 28.0)
                    .height(ROW)
                    .fill(if chosen { alpha(t.accent, 0.20) } else { alpha(t.surface, 0.0) })
                    .border(if chosen { 1.0 } else { 0.0 }, alpha(t.accent, 0.45))
                    .padding(amane::Padding { top: 0.0, right: 12.0, bottom: 0.0, left: 12.0 })
                    .align_child(Start, Center)
                    .cursor(amane::Pointer)
                    // the choice follows the pointer only when it moves, so results changing under a parked pointer
                    // don't take the choice away from the keyboard (Enter would start the wrong app)
                    .on_move(move |_| {
                        if Launcher::read().selected != index {
                            Launcher::write().selected = index;
                        }
                    })
                    .on_click(move |_| start(&launched))
                    .child(
                        Row::new(vec![
                            Box::new(picture) as Box<dyn Widget>,
                            Box::new(text),
                            Box::new(Rectangle::new().width(Parent).height(1.0)),
                            enter,
                        ])
                        .gap(14.0)
                        .align(Center)
                        .width(WIDTH - 28.0 - 24.0),
                    ),
            ));
        }

        if shown.is_empty() {
            rows.push(Box::new(
                Rectangle::new().width(WIDTH - 28.0).height(ROW).align_child(Center, Center).child(label("No matching apps", 12.5, t.faint)),
            ));
        }

        let count = shown.len().max(1) as f32;

        body_height = count * ROW + (count - 1.0) * 2.0;

        heading = String::from(if empty_query { "Apps" } else { "Results" });

        body = Box::new(Column::new(rows).gap(2.0));
    }

    let total = 28.0 + 48.0 + 10.0 + 14.0 + 10.0 + body_height;

    LayerWindow::new()
        .width(WIDTH)
        .height(total)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Middle)
        .margin(Margin { top: 190, right: 0, bottom: 0, left: 0 })
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("amane-panel")
        .keyboard(Keyboard::Exclusive)
        .on_key(on_key)
        .child(
            panels::body(&t).padding(14.0).child(
                Column::new(vec![
                    Box::new(top) as Box<dyn Widget>,
                    Box::new(label(heading.to_uppercase(), 10.0, t.faint).weight(amane::Weight::Bold)),
                    body,
                ])
                .gap(10.0),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::rank;

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(rank("", "Firefox", ""), Some(0));
    }

    #[test]
    fn better_matches_rank_lower() {
        let prefix = rank("fire", "Firefox", "");
        let word = rank("fox", "Red Fox", "");
        let inside = rank("ire", "Firefox", "");
        let extra = rank("browser", "Firefox", "Web Browser firefox %u");
        let letters = rank("ffx", "Firefox", "");

        assert_eq!(prefix, Some(0));
        assert_eq!(word, Some(1));
        assert_eq!(inside, Some(2));
        assert_eq!(extra, Some(4));
        assert_eq!(letters, Some(6));
    }

    #[test]
    fn names_are_compared_in_lowercase() {
        assert_eq!(rank("fire", "FIREFOX", ""), Some(0));
        assert_eq!(rank("web", "Firefox", "WEB BROWSER"), Some(4));
    }

    #[test]
    fn letters_have_to_come_in_order() {
        assert_eq!(rank("xff", "Firefox", ""), None);
        assert_eq!(rank("zzz", "Firefox", "browser"), None);
    }
}
