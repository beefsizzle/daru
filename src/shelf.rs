use std::cell::RefCell;
use std::collections::HashMap;

use amane::{
    Apps, Button, Center, Column, DesktopApp, Horizontal, LayerWindow, Monitor, Padding, Pointer, Rectangle, Row,
    Service, Start, Widget, Workspaces,
};

use crate::focus::Windows;
use crate::panels::{self, Which};
use crate::theme::{RADIUS_SMALL, Theme, alpha, mix};
use crate::ui::{app_icon, bold, chip, hover, label, set_hover};

/*
 * the shelf in the bar: the pinned apps, then every other app with a window open, on any workspace or monitor. a click
 * on an open app brings its window forward wherever it is, again goes through its other windows, and a click on a
 * pinned app that isn't open starts it. a right click opens its menu: pin or unpin, close, and move to another
 * workspace. pins are kept in ~/.config/amane-shell/pinned
 */
const ITEM: f32 = 30.0;
const ITEM_GAP: f32 = 4.0;
const ICON: f32 = 18.0;

// ---------------------------------------------------------------- pins

struct Pins {
    list: Vec<String>,
}

fn pins_file() -> std::path::PathBuf {
    crate::cache::config_dir().join("pinned")
}

impl Service for Pins {
    fn new() -> Self {
        let list = std::fs::read_to_string(pins_file())
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect();

        Self { list }
    }

    // only this shell changes the list, so nothing polls the file
    fn listen() {}
}

fn toggle_pin(key: &str) {
    let list = {
        let mut pins = Pins::write();

        match pins.list.iter().position(|pin| pin.eq_ignore_ascii_case(key)) {
            Some(at) => drop(pins.list.remove(at)),
            None => pins.list.push(key.to_string()),
        }

        pins.list.clone()
    };

    let file = pins_file();

    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }

    let _ = std::fs::write(&file, list.join("\n") + "\n");
}

// ---------------------------------------------------------------- apps

/*
 * the installed app a window belongs to. Wayland only gives the window's app id, which is usually the desktop file's
 * name ("org.gnome.Nautilus") or the program's ("firefox"), and the app's icon is mostly named after one of those
 */
fn app_for(apps: &[DesktopApp], app_id: &str) -> Option<DesktopApp> {
    let wanted = app_id.to_lowercase();

    let last = wanted.rsplit('.').next().unwrap_or(&wanted).to_string();

    let program = |app: &DesktopApp| {
        app.exec().split_whitespace().next().and_then(|first| first.rsplit('/').next()).unwrap_or("").to_lowercase()
    };

    apps.iter()
        .find(|app| app.icon().is_some_and(|icon| icon.to_lowercase() == wanted))
        .or_else(|| apps.iter().find(|app| program(app) == wanted))
        .or_else(|| apps.iter().find(|app| app.name().to_lowercase() == wanted))
        .or_else(|| apps.iter().find(|app| program(app) == last || app.name().to_lowercase() == last))
        .cloned()
}

#[derive(Clone)]
struct Found {
    app: Option<DesktopApp>,
    icon: Option<String>,
}

/*
 * the app and icon for each app id, worked out once: the bar is drawn on every change it shows, on each monitor, and
 * matching goes through every installed app. forgotten when the installed apps change
 */
fn found(apps: &[DesktopApp], app_id: &str) -> Found {
    thread_local! {
        static FOUND: RefCell<(usize, HashMap<String, Found>)> = RefCell::new((0, HashMap::new()));
    }

    FOUND.with_borrow_mut(|(installed, known)| {
        if *installed != apps.len() {
            *installed = apps.len();
            known.clear();
        }

        known
            .entry(app_id.to_lowercase())
            .or_insert_with(|| {
                let app = app_for(apps, app_id);

                let icon = app
                    .as_ref()
                    .and_then(|app| app.icon_path().map(|path| path.to_string_lossy().into_owned()))
                    .or_else(|| crate::icons::resolve(app_id))
                    .or_else(|| crate::icons::resolve(&app_id.to_lowercase()));

                Found { app, icon }
            })
            .clone()
    })
}

struct Item {
    key: String, // the app id, or the pinned name
    found: Found,
    windows: Vec<String>,
    focused: bool,
}

fn items() -> Vec<Item> {
    let apps = Apps::read();
    let apps = apps.list();

    let mut items: Vec<Item> = Pins::read()
        .list
        .iter()
        .map(|pin| Item { key: pin.clone(), found: found(apps, pin), windows: Vec::new(), focused: false })
        .collect();

    for window in &Windows::read().list {
        if window.app_id.is_empty() {
            continue;
        }

        let index = match items.iter().position(|item| item.key.eq_ignore_ascii_case(&window.app_id)) {
            Some(index) => index,
            None => {
                items.push(Item {
                    key: window.app_id.clone(),
                    found: found(apps, &window.app_id),
                    windows: Vec::new(),
                    focused: false,
                });

                items.len() - 1
            }
        };

        items[index].windows.push(window.id.clone());
        items[index].focused |= window.focused;
    }

    items
}

// starts the app when nothing of it is open, otherwise brings its windows forward one after another
fn activate(app: Option<&DesktopApp>, windows: &[String]) {
    if windows.is_empty() {
        if let Some(app) = app {
            app.launch();
        }

        return;
    }

    let focused = {
        let open = Windows::read();

        windows.iter().position(|id| open.list.iter().any(|window| &window.id == id && window.focused))
    };

    let next = focused.map_or(0, |at| (at + 1) % windows.len());

    // Umbriel's window ids are hex, nothing for the shell to misread
    amane::spawn(&format!("umbriel msg window-focus {}", windows[next]));
}

// ---------------------------------------------------------------- drawing

fn button(t: &Theme, item: Item, place: usize, output: &str) -> Box<dyn Widget> {
    let picture = Rectangle::new().width(ICON).height(ICON);

    let picture = match &item.found.icon {
        Some(path) => picture.fill(app_icon(path)),
        // an app with no icon anywhere gets its first letter
        None => picture.fill(alpha(t.fg, 0.16)).align_child(Center, Center).child(bold(
            item.key.chars().next().map(|first| first.to_uppercase().to_string()).unwrap_or_default(),
            11.0,
            t.fg,
        )),
    };

    // a short line under an open app, a longer one in the accent colour under the one in focus
    let (width, fill) = if item.focused {
        (14.0, t.accent)
    } else if item.windows.is_empty() {
        (0.0, t.dim)
    } else {
        (5.0, t.dim)
    };

    let marker = Rectangle::new().width(width).height(2.0).fill(fill);

    let content = Column::new(vec![Box::new(picture) as Box<dyn Widget>, Box::new(marker)]).gap(2.0).align(Center);

    let Item { key, found, windows, .. } = item;

    let output = output.to_string();

    Box::new(chip(&format!("shelf/{key}"), t, ITEM, content).on_click(move |button| match button {
        Button::Right => open_menu(&key, place, &output),
        _ => activate(found.app.as_ref(), &windows),
    }))
}

pub fn view(t: &Theme, output: &str) -> Row {
    let buttons: Vec<Box<dyn Widget>> =
        items().into_iter().enumerate().map(|(place, item)| button(t, item, place, output)).collect();

    Row::new(buttons).gap(ITEM_GAP).align(Center)
}

// ---------------------------------------------------------------- the menu

#[derive(Default)]
struct Menu {
    key: String,  // the app it's for
    place: usize, // where its button is on the shelf, which the menu lines up under
}

impl Service for Menu {
    fn new() -> Self {
        Self::default()
    }

    // it only changes when a menu is opened
    fn listen() {}
}

// a second right click on the same app closes its menu again
fn open_menu(key: &str, place: usize, output: &str) {
    let same = panels::is_open(Which::AppMenu) && Menu::read().key == key;

    if same {
        panels::close();

        return;
    }

    *Menu::write() = Menu { key: key.to_string(), place };

    panels::show(Which::AppMenu, output);
}

fn close_all(windows: &[String]) {
    for id in windows {
        amane::spawn(&format!("umbriel msg window-close {id}"));
    }
}

/*
 * Umbriel only moves the focused window, so each one is focused first, one after another in a single shell so the
 * steps can't overtake each other. a window on a workspace that isn't showing takes the screen there while it moves
 */
fn move_all(windows: &[String], index: u32, output: &str) {
    let steps: Vec<String> = windows
        .iter()
        .map(|id| {
            let focus = format!("umbriel msg window-focus {id}");

            format!("{focus} && umbriel msg window-move-to-workspace-silent {index} --output {output}")
        })
        .collect();

    // monitor names are like DP-1 or eDP-1, nothing for the shell to misread
    amane::spawn(&steps.join(" && "));
}

const MENU_WIDTH: f32 = 220.0;
const MENU_PADDING: f32 = 6.0;
const ROW: f32 = 30.0;
const ROW_WIDTH: f32 = MENU_WIDTH - 2.0 * MENU_PADDING;
const SPOT: f32 = 26.0;

// a line of the menu that does something when clicked, and closes the menu
fn entry(t: &Theme, id: &str, text: String, action: impl Fn() + 'static) -> Box<dyn Widget> {
    let id = format!("appmenu/{id}");

    let hot = hover(&id);

    Box::new(
        Rectangle::new()
            .width(ROW_WIDTH)
            .height(ROW)
            .radius(RADIUS_SMALL)
            .fill(mix(alpha(t.surface, 0.0), t.surface_hi, hot))
            .padding(Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 })
            .align_child(Start, Center)
            .cursor(Pointer)
            .on_hover(move |inside| set_hover(&id, inside))
            .on_click(move |_| {
                action();

                panels::close();
            })
            .child(label(text, 12.5, t.fg)),
    )
}

/*
 * a row of numbered squares for one monitor's workspaces, the way the bar's dots show them. the empty one Umbriel
 * keeps at the end is a +, a new workspace
 */
fn workspace_row(t: &Theme, output: &str, named: bool, windows: &[String]) -> Box<dyn Widget> {
    let workspaces = Workspaces::read();

    let here: Vec<_> = workspaces.list().iter().filter(|workspace| workspace.output() == Some(output)).collect();

    let mut spots: Vec<Box<dyn Widget>> = Vec::new();

    if named {
        let name = label(output, 11.5, t.dim);

        spots.push(Box::new(Rectangle::new().width(52.0).height(SPOT).align_child(Start, Center).child(name)));
    }

    for (at, workspace) in here.iter().enumerate() {
        let index = workspace.index();

        let text = if at + 1 == here.len() && workspace.windows() == 0 {
            String::from("+")
        } else {
            workspace.name().map_or_else(|| index.to_string(), str::to_string)
        };

        let id = format!("appmenu/ws/{output}/{index}");
        let hot = hover(&id);

        let (windows, output) = (windows.to_vec(), output.to_string());

        spots.push(Box::new(
            Rectangle::new()
                .width(SPOT)
                .height(SPOT)
                .radius(RADIUS_SMALL)
                .fill(mix(alpha(t.surface, 0.45), t.surface_hi, hot))
                .align_child(Center, Center)
                .cursor(Pointer)
                .on_hover(move |inside| set_hover(&id, inside))
                .on_click(move |_| {
                    move_all(&windows, index, &output);

                    panels::close();
                })
                .child(label(text, 12.0, t.fg)),
        ));
    }

    Box::new(
        Rectangle::new()
            .width(ROW_WIDTH)
            .height(SPOT)
            .padding(Padding { top: 0.0, right: 0.0, bottom: 0.0, left: 10.0 })
            .child(Row::new(spots).gap(4.0).align(Center)),
    )
}

pub fn menu(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::AppMenu, monitor) else {
        return panels::hidden("amane-panel");
    };

    let (key, place) = {
        let menu = Menu::read();

        (menu.key.clone(), menu.place)
    };

    // the app as the shelf has it now, which may have lost its windows while the menu was open
    let item = items().into_iter().find(|item| item.key.eq_ignore_ascii_case(&key));

    let (name, windows) = match &item {
        Some(item) => (
            item.found.app.as_ref().map_or_else(|| item.key.clone(), |app| app.name().to_string()),
            item.windows.clone(),
        ),
        None => (key.clone(), Vec::new()),
    };

    let pinned = Pins::read().list.iter().any(|pin| pin.eq_ignore_ascii_case(&key));

    let mut rows: Vec<Box<dyn Widget>> = vec![Box::new(
        Rectangle::new()
            .width(ROW_WIDTH)
            .height(ROW)
            .padding(Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 })
            .align_child(Start, Center)
            .child(bold(name, 12.5, t.fg).elide()),
    )];

    let mut height = ROW;

    let pin_key = key.clone();

    rows.push(entry(&t, "pin", String::from(if pinned { "Unpin" } else { "Pin" }), move || toggle_pin(&pin_key)));

    height += ROW;

    if !windows.is_empty() {
        let text = if windows.len() == 1 { String::from("Close") } else { format!("Close {} windows", windows.len()) };

        let closing = windows.clone();

        rows.push(entry(&t, "close", text, move || close_all(&closing)));

        rows.push(Box::new(
            Rectangle::new()
                .width(ROW_WIDTH)
                .height(ROW)
                .padding(Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 })
                .align_child(Start, Center)
                .child(label("Move to", 11.5, t.dim)),
        ));

        height += 2.0 * ROW;

        let mut outputs: Vec<String> = Vec::new();

        for workspace in Workspaces::read().list() {
            if let Some(output) = workspace.output()
                && !outputs.iter().any(|known| known == output)
            {
                outputs.push(output.to_string());
            }
        }

        for output in &outputs {
            rows.push(workspace_row(&t, output, outputs.len() > 1, &windows));

            height += SPOT;
        }
    }

    let gaps = (rows.len() - 1) as f32 * 2.0;

    let left = crate::bar::shelf_left(&monitor.name) + place as f32 * (ITEM + ITEM_GAP);

    panels::window_at(MENU_WIDTH, height + gaps + 2.0 * MENU_PADDING, Horizontal::Left, left as i32)
        .child(panels::body(&t).padding(MENU_PADDING).child(Column::new(rows).gap(2.0)))
}
