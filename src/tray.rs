use amane::{Button, Service, Tray, TrayItem, TrayStatus};

use crate::panels::{self, Which};

/*
 * the system tray. amane's Tray service is the StatusNotifierWatcher: apps register with it, and it reads their icons
 * and menus. what's left here is which icons the bar shows and what a click on one does
 */

/// the icons to show. an app with nothing to tell (Passive) asks to be hidden
pub fn shown(tray: &Tray) -> impl Iterator<Item = &TrayItem> {
    tray.items().iter().filter(|item| item.status() != TrayStatus::Passive)
}

pub fn click(id: u32, button: Button, monitor: &str) {
    let item = Tray::read().items().iter().find(|item| item.id() == id).map(|item| (item.has_menu(), item.is_menu()));

    // the app quit between the bar being drawn and the click
    let Some((has_menu, is_menu)) = item else { return };

    let wants_menu = matches!(button, Button::Right) || (matches!(button, Button::Left) && is_menu);

    if wants_menu {
        // a second click on the same icon closes its menu. a click on another icon opens that one's instead
        if panels::is_open(Which::TrayMenu) && crate::traymenu::is_menu_of(id) {
            panels::close();

            return;
        }

        if has_menu {
            crate::traymenu::open(id, monitor);

            return;
        }

        // no dbusmenu: ContextMenu below asks the app to draw its own
    }

    match button {
        Button::Left => Tray::activate(id),
        Button::Right => Tray::context_menu(id),
        Button::Middle => Tray::secondary_activate(id),
    }
}
