use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use amane::{
    Argument, Bus, Center, Column, Horizontal, Layer, LayerWindow, Margin, Monitor, Padding, Parent, Pointer, Rectangle, Row, ScrollArea,
    Service, Start, Value, Vertical, Widget, Zone, children,
};

use crate::panels::{self, GAP, Which};
use crate::theme::{RADIUS_SMALL, Theme, alpha, mix};
use crate::tray::TrayItem;
use crate::ui::{bold, hover, icon, label, set_hover};

/*
 * the popup menu of a tray icon. apps describe their menu over dbusmenu, as a tree of entries, and the shell draws it.
 * it's read when it opens, since many apps only fill it in then
 */
const MENU: &str = "com.canonical.dbusmenu";

#[derive(Clone, Default)]
struct Node {
    id: i32,
    label: String,
    enabled: bool,
    separator: bool,
    visible: bool,         // apps hide entries with `visible: false`
    checked: Option<bool>, // Some for checkmark and radio entries
    children: Vec<Node>,
    submenu: bool,
}

#[derive(Default)]
struct TrayMenu {
    root: Option<Node>,
    stack: Vec<i32>, // the submenus walked into
    destination: String,
    path: String,
}

impl Service for TrayMenu {
    fn new() -> Self {
        Self::default()
    }

    // it only changes when a menu is opened or clicked
    fn listen() {}
}

fn parse(value: &Value) -> Node {
    let fields = value.list();

    let props = fields.get(1).cloned().unwrap_or(Value::Nothing);

    let text = |key: &str| props.get(key).text().to_string();

    let flag = |key: &str, default: bool| match props.get(key) {
        Value::Bool(value) => *value,
        _ => default,
    };

    let children: Vec<Node> = fields.get(2).map(|list| list.list().iter().map(parse).collect()).unwrap_or_default();

    let checked = match text("toggle-type").as_str() {
        "checkmark" | "radio" => Some(props.get("toggle-state").number() == 1.0),
        _ => None,
    };

    // a single underscore marks the keyboard shortcut letter
    let label = text("label").replace("__", "\u{0}").replace('_', "").replace('\u{0}', "_");

    Node {
        id: fields.first().map_or(0, |id| id.number() as i32),
        label,
        enabled: flag("enabled", true),
        separator: text("type") == "separator",
        visible: flag("visible", true),
        checked,
        submenu: text("children-display") == "submenu" || !children.is_empty(),
        children: children.into_iter().filter(|child| child.visible && (child.separator || !child.label.is_empty())).collect(),
    }
}

fn fetch(destination: &str, path: &str) -> Option<Node> {
    let bus = Bus::session();

    let answer = bus.call(destination, path, MENU, "GetLayout", &[Argument::Int(0), Argument::Int(-1), Argument::TextList(Vec::new())]);

    // (revision, layout)
    let layout = answer.list().get(1)?;

    Some(parse(layout))
}

// counts menus asked for, so only the latest one is shown
static REQUEST: AtomicU64 = AtomicU64::new(0);

/// opens a tray icon's menu, from a right click or a left click on an item that's only a menu. false when the app has
/// no dbusmenu, so the caller can ask the app to draw its own
pub fn open(item: &TrayItem, monitor: &str) -> bool {
    if item.menu.is_empty() {
        return false;
    }

    let (destination, path) = (item.destination.clone(), item.menu.clone());
    let monitor = monitor.to_string();

    // an app can take a second to send its menu. by then the user may have clicked another icon, or dismissed it all
    let request = REQUEST.fetch_add(1, Ordering::SeqCst) + 1;
    let since = panels::generation();

    std::thread::spawn(move || {
        // some apps only fill their menu when it's about to be shown
        Bus::session().call(&destination, &path, MENU, "AboutToShow", &[Argument::Int(0)]);

        let Some(root) = fetch(&destination, &path) else { return };

        {
            let mut menu = TrayMenu::write();

            // checked under the lock, so an older answer can't replace the menu of a newer click
            if REQUEST.load(Ordering::SeqCst) != request || panels::generation() != since {
                return;
            }

            menu.root = Some(root);
            menu.stack.clear();
            menu.destination = destination;
            menu.path = path;
        }

        panels::show_since(since, Which::TrayMenu, &monitor);
    });

    true
}

/// is the menu on screen the one of this tray item
pub fn is_menu_of(destination: &str) -> bool {
    TrayMenu::read().destination == destination
}

fn find(node: &Node, id: i32) -> Option<&Node> {
    if node.id == id {
        return Some(node);
    }

    node.children.iter().find_map(|child| find(child, id))
}

fn click(node: &Node) {
    if !node.enabled {
        return;
    }

    if node.submenu {
        let id = node.id;

        let (destination, path) = {
            let mut menu = TrayMenu::write();

            menu.stack.push(id);

            (menu.destination.clone(), menu.path.clone())
        };

        // read again, since apps may only fill a submenu as it opens
        std::thread::spawn(move || {
            Bus::session().call(&destination, &path, MENU, "AboutToShow", &[Argument::Int(id)]);

            if let Some(root) = fetch(&destination, &path) {
                TrayMenu::write().root = Some(root);
            }
        });

        return;
    }

    let (destination, path) = {
        let menu = TrayMenu::read();

        (menu.destination.clone(), menu.path.clone())
    };

    let id = node.id;

    std::thread::spawn(move || {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs() as u32);

        Bus::session().call(
            &destination,
            &path,
            MENU,
            "Event",
            &[Argument::Int(id), Argument::Text(String::from("clicked")), Argument::Variant(Box::new(Argument::Int(0))), Argument::Unsigned(stamp)],
        );
    });

    panels::close();
}

fn back() {
    TrayMenu::write().stack.pop();
}

// ---------------------------------------------------------------- view

const WIDTH: f32 = 320.0;
const ROW: f32 = 30.0;
const MAX_LIST: f32 = 440.0;

// a row's width inside the panel's padding, and its label's: less the row's own padding, the tick, the arrow and the gaps
const ROW_WIDTH: f32 = WIDTH - 28.0;
const LABEL_WIDTH: f32 = ROW_WIDTH - 20.0 - 14.0 - 14.0 - 16.0;

fn row(t: &Theme, node: &Node) -> Box<dyn Widget> {
    if node.separator {
        let line = Rectangle::new().width(WIDTH - 40.0).height(1.0).fill(t.border);

        return Box::new(Rectangle::new().width(ROW_WIDTH).height(9.0).align_child(Center, Center).child(line));
    }

    let key = format!("tm{}", node.id);
    let hot = if node.enabled { hover(&key) } else { 0.0 };

    let color = if node.enabled { t.fg } else { t.faint };

    // an empty tick keeps every label lined up
    let lead = match node.checked {
        Some(true) => icon("\u{f00c}", 11.0, t.accent),
        _ => icon("", 11.0, t.accent),
    };

    let trail: Box<dyn Widget> = if node.submenu { Box::new(icon("\u{f054}", 10.0, t.dim)) } else { Box::new(Rectangle::new().width(1.0).height(1.0)) };

    let target = node.clone();

    Box::new(
        Rectangle::new()
            .width(ROW_WIDTH)
            .height(ROW)
            .radius(RADIUS_SMALL)
            .fill(mix(alpha(t.surface, 0.0), t.surface_hi, hot))
            .padding(Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 })
            .align_child(Start, Center)
            .cursor(Pointer)
            .on_hover(move |inside| set_hover(&key, inside))
            .on_click(move |_| click(&target))
            .child(
                Row::new(vec![
                    Box::new(Rectangle::new().width(14.0).height(14.0).align_child(Center, Center).child(lead)) as Box<dyn Widget>,
                    Box::new(Rectangle::new().width(LABEL_WIDTH).height(18.0).child(label(&node.label, 12.5, color).elide())),
                    trail,
                ])
                .gap(8.0)
                .align(Center),
            ),
    )
}

fn window(height: f32, margin: Margin) -> LayerWindow {
    LayerWindow::new()
        .width(WIDTH)
        .height(height)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Right)
        .margin(margin)
        .layer(Layer::Overlay)
        .space(Zone::Respect)
        .namespace("amane-panel")
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::TrayMenu, monitor) else {
        return panels::hidden("amane-panel");
    };
    let menu = TrayMenu::read();

    // right edge of the popup lines up with the right edge of the tray
    let margin = Margin { top: GAP, right: crate::bar::right_edge(crate::bar::Slot::Tray), bottom: 0, left: 0 };

    let Some(root) = menu.root.as_ref() else {
        return window(40.0, margin).child(Rectangle::new().width(Parent).height(Parent));
    };

    let current = menu.stack.last().and_then(|id| find(root, *id)).unwrap_or(root);

    let mut rows: Vec<Box<dyn Widget>> = Vec::new();

    let mut height = 0.0;

    for child in &current.children {
        height += if child.separator { 9.0 } else { ROW };

        rows.push(row(&t, child));
    }

    let gaps = rows.len().saturating_sub(1) as f32 * 2.0;

    let list_height = (height + gaps).min(MAX_LIST);

    let mut sections: Vec<Box<dyn Widget>> = Vec::new();
    let mut total = 28.0 + list_height;

    if !menu.stack.is_empty() {
        sections.push(Box::new(
            Rectangle::new()
                .width(ROW_WIDTH)
                .height(ROW)
                .radius(RADIUS_SMALL)
                .cursor(Pointer)
                .on_click(|_| back())
                .align_child(Start, Center)
                .padding(Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 })
                .child(
                    Row::new(children![icon("\u{f053}", 10.0, t.accent), bold(&current.label, 12.5, t.fg)])
                        .gap(10.0)
                        .align(Center),
                ),
        ));

        total += ROW + 8.0;
    }

    // its own scroll position at every level, so a submenu doesn't open scrolled like the menu it came from
    // (the id has to be a literal, so deeper levels share the last one)
    let scroll_id = ["tray-menu-0", "tray-menu-1", "tray-menu-2", "tray-menu-3"][menu.stack.len().min(3)];

    sections.push(Box::new(
        Rectangle::new()
            .width(ROW_WIDTH)
            .height(list_height)
            .clip()
            .child(ScrollArea::new(scroll_id, Column::new(rows).gap(2.0))),
    ));

    window(total, margin).child(panels::body(&t).padding(14.0).child(Column::new(sections).gap(8.0)))
}
