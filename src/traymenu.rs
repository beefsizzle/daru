use amane::{
    Center, Column, Horizontal, Layer, LayerWindow, Margin, MenuEntry, MenuKind, Monitor, Padding, Pointer, Rectangle, Row, ScrollArea, Service,
    Start, Tray, Vertical, Widget, Zone, children,
};

use crate::panels::{self, GAP, Which};
use crate::theme::{RADIUS_SMALL, Theme, alpha, mix};
use crate::ui::{bold, hover, icon, label, set_hover};

/*
 * the popup menu of a tray icon. amane's Tray service reads the menu from the app when it's asked for, since many apps
 * only fill it in then. this draws it, and keeps track of which submenu is showing
 */
#[derive(Default)]
struct TrayMenu {
    item: u32,       // the tray icon whose menu was asked for
    stack: Vec<i32>, // the submenus walked into
}

impl Service for TrayMenu {
    fn new() -> Self {
        Self::default()
    }

    // it only changes when a menu is opened or clicked
    fn listen() {}
}

/// opens a tray icon's menu, from a right click or a left click on an item that's only a menu. the panel opens at once
/// and stays empty until the app has sent its menu, which can take a second
pub fn open(id: u32, monitor: &str) {
    {
        let mut menu = TrayMenu::write();

        menu.item = id;
        menu.stack.clear();
    }

    panels::show(Which::TrayMenu, monitor);

    Tray::open_menu(id);
}

/// is the menu on screen the one of this tray icon
pub fn is_menu_of(id: u32) -> bool {
    TrayMenu::read().item == id
}

fn find(entries: &[MenuEntry], id: i32) -> Option<&MenuEntry> {
    entries.iter().find_map(|entry| if entry.id() == id { Some(entry) } else { find(entry.children(), id) })
}

fn click(id: i32, enabled: bool, submenu: bool) {
    if !enabled {
        return;
    }

    if submenu {
        TrayMenu::write().stack.push(id);

        // read again, since apps may only fill a submenu as it opens
        Tray::open_submenu(id);

        return;
    }

    Tray::click_menu(id);

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

fn row(t: &Theme, entry: &MenuEntry) -> Box<dyn Widget> {
    if entry.kind() == MenuKind::Separator {
        let line = Rectangle::new().width(WIDTH - 40.0).height(1.0).fill(t.border);

        return Box::new(Rectangle::new().width(ROW_WIDTH).height(9.0).align_child(Center, Center).child(line));
    }

    let (id, enabled, submenu) = (entry.id(), entry.enabled(), entry.has_submenu());

    let key = format!("tm{id}");
    let hot = if enabled { hover(&key) } else { 0.0 };

    let color = if enabled { t.fg } else { t.faint };

    // an empty tick keeps every label lined up
    let lead = match entry.kind() {
        MenuKind::Check(true) | MenuKind::Radio(true) => icon("\u{f00c}", 11.0, t.accent),
        _ => icon("", 11.0, t.accent),
    };

    let trail: Box<dyn Widget> = if submenu { Box::new(icon("\u{f054}", 10.0, t.dim)) } else { Box::new(Rectangle::new().width(1.0).height(1.0)) };

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
            .on_click(move |_| click(id, enabled, submenu))
            .child(
                Row::new(vec![
                    Box::new(Rectangle::new().width(14.0).height(14.0).align_child(Center, Center).child(lead)) as Box<dyn Widget>,
                    Box::new(Rectangle::new().width(LABEL_WIDTH).height(18.0).child(label(entry.label(), 12.5, color).elide())),
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
    let tray = Tray::read();

    // nothing to draw until the app has sent the menu that was asked for
    let Some(entries) = tray.menu().filter(|shown| shown.item() == menu.item).map(|shown| shown.entries()) else {
        return panels::hidden("amane-panel");
    };

    // right edge of the popup lines up with the right edge of the tray
    let margin = Margin { top: GAP, right: crate::bar::right_edge(crate::bar::Slot::Tray), bottom: 0, left: 0 };

    // the submenu walked into, when the app still has it
    let current = menu.stack.last().and_then(|id| find(entries, *id));

    let entries = current.map_or(entries, MenuEntry::children);

    let mut rows: Vec<Box<dyn Widget>> = Vec::new();

    let mut height = 0.0;

    for child in entries {
        height += if child.kind() == MenuKind::Separator { 9.0 } else { ROW };

        rows.push(row(&t, child));
    }

    let gaps = rows.len().saturating_sub(1) as f32 * 2.0;

    let list_height = (height + gaps).min(MAX_LIST);

    let mut sections: Vec<Box<dyn Widget>> = Vec::new();
    let mut total = 28.0 + list_height;

    if let Some(current) = current {
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
                    Row::new(children![icon("\u{f053}", 10.0, t.accent), bold(current.label(), 12.5, t.fg)])
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
