use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use amane::{Argument, Bus, Service, Value};

use crate::sync::lock;

/*
 * the system tray. the shell is the StatusNotifierWatcher, so tray apps register here,
 * and also scans the bus for items that were running before it started.
 */
#[derive(Clone, PartialEq, Default)]
pub struct TrayItem {
    pub destination: String,
    pub path: String,
    pub title: String,
    pub icon: String, // a PNG or SVG file, empty when the item has none we can draw
    pub menu: String, // dbusmenu object path, empty when the app has no menu
    pub is_menu: bool, // left click should open the menu too
}

pub struct Tray {
    pub items: Vec<TrayItem>,
}

static REGISTERED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

// set when any item says its icon changed, so pixmaps (which are big) are only fetched again then
static ICONS_STALE: AtomicBool = AtomicBool::new(true);

// items whose properties are being read right now. an app that doesn't answer keeps its entry, so a hung one costs one
// waiting thread and not a new one every round
static BUSY: Mutex<Vec<String>> = Mutex::new(Vec::new());

// how long a round waits for apps to answer. one that is slower keeps showing what it showed before
const ANSWER_WITHIN: Duration = Duration::from_millis(1500);

/*
 * a round of asking every app runs when something says it changed: an app registering, one of the item signals below,
 * or an app's name coming or going on the bus. it used to run every 2 seconds, about 20 D-Bus calls and a thread per app
 * each time, which made the tray the busiest thing in an idle shell. an app that changes something without saying so
 * still shows it within a minute
 */
const AT_LEAST_EVERY: Duration = Duration::from_secs(60);

// an app starting sends a few of these at once, and they make one round
const SETTLE: Duration = Duration::from_millis(150);

// what an item sends when something we show changes (NewIcon also marks its pixmap stale, see ICONS_STALE)
const ITEM_SIGNALS: [&str; 5] = ["NewTitle", "NewStatus", "NewToolTip", "NewAttentionIcon", "NewOverlayIcon"];

// the biggest icon we'll read: 512 x 512 x 4 bytes. an app can send any size it likes
const LARGEST_ICON: u64 = 512;

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";

impl Service for Tray {
    fn new() -> Self {
        Self { items: Vec::new() }
    }

    fn listen() {
        let bus = Bus::session();

        // watch first, then take the name, so the first calls aren't missed
        let watcher = bus.methods("/StatusNotifierWatcher", WATCHER);
        let properties = bus.methods("/StatusNotifierWatcher", "org.freedesktop.DBus.Properties");

        let owner = bus.own(WATCHER);

        bus.own(&format!("org.kde.StatusNotifierHost-{}", std::process::id()));

        let (changed, changes) = mpsc::channel();

        std::thread::spawn(move || poll(&changes));

        {
            let changed = changed.clone();

            std::thread::spawn(move || {
                for _ in Bus::session().signals(ITEM, "NewIcon") {
                    ICONS_STALE.store(true, Ordering::Relaxed);

                    let _ = changed.send(());
                }
            });
        }

        for name in ITEM_SIGNALS {
            let changed = changed.clone();

            std::thread::spawn(move || {
                for _ in Bus::session().signals(ITEM, name) {
                    let _ = changed.send(());
                }
            });
        }

        {
            let changed = changed.clone();

            std::thread::spawn(move || follow_names(&changed));
        }

        if !owner {
            // another watcher exists, we only read its list in `poll`
            return;
        }

        std::thread::spawn(move || answer_properties(properties));

        for method in watcher {
            match method.name() {
                "RegisterStatusNotifierItem" => {
                    let service = method.arguments().first().map_or("", Value::text).to_string();

                    let (destination, path) = registration(&service, method.sender());

                    if !destination.is_empty() {
                        let mut registered = lock(&REGISTERED);

                        if !registered.iter().any(|(known, _)| *known == destination) {
                            registered.push((destination.clone(), path));
                        }
                    }

                    method.reply(&[]);

                    Bus::session().emit(
                        "/StatusNotifierWatcher",
                        WATCHER,
                        "StatusNotifierItemRegistered",
                        &[Argument::Text(destination)],
                    );

                    let _ = changed.send(());
                }
                _ => method.reply(&[]),
            }
        }
    }
}

/*
 * where a registered item lives: "name", "name/path", or only "/path" on the caller's own connection, which apps built
 * on libappindicator send (Steam, for one), as they own no name of their own
 */
fn registration(service: &str, sender: &str) -> (String, String) {
    if service.starts_with('/') {
        return (sender.to_string(), service.to_string());
    }

    match service.split_once('/') {
        Some((name, path)) => (name.to_string(), format!("/{path}")),
        None => (service.to_string(), String::from("/StatusNotifierItem")),
    }
}

// clients ask the watcher whether a host exists before they bother registering
fn answer_properties(properties: impl Iterator<Item = amane::Method>) {
    for method in properties {
        let registered: Vec<String> = lock(&REGISTERED).iter().map(|(name, _)| name.clone()).collect();

        let value = |name: &str| -> Option<Argument> {
            match name {
                "IsStatusNotifierHostRegistered" => Some(Argument::Bool(true)),
                "ProtocolVersion" => Some(Argument::Int(0)),
                "RegisteredStatusNotifierItems" => Some(Argument::TextList(registered.clone())),
                _ => None,
            }
        };

        match method.name() {
            "Get" => {
                let name = method.arguments().get(1).map_or("", Value::text);

                match value(name) {
                    Some(argument) => method.reply(&[Argument::Variant(Box::new(argument))]),
                    None => method.reply(&[]),
                }
            }
            "GetAll" => {
                let mut map = std::collections::BTreeMap::new();

                for name in ["IsStatusNotifierHostRegistered", "ProtocolVersion", "RegisteredStatusNotifierItems"] {
                    if let Some(argument) = value(name) {
                        map.insert(name.to_string(), argument);
                    }
                }

                method.reply(&[Argument::Map(map)]);
            }
            _ => method.reply(&[]),
        }
    }
}

/*
 * an app's name coming or going: one that quit has to leave the tray, and one that started with a well-known item name
 * may never register. every connection that comes and goes is announced, so only names that could be an item count
 */
fn follow_names(changed: &Sender<()>) {
    for signal in Bus::session().signals("org.freedesktop.DBus", "NameOwnerChanged") {
        let name = signal.arguments().first().map_or("", Value::text);

        let an_item_name = name.starts_with("org.kde.StatusNotifierItem-") || name.starts_with("org.freedesktop.StatusNotifierItem-");
        let shown = || Tray::read().items.iter().any(|item| item.destination == name);
        let registered = || lock(&REGISTERED).iter().any(|(destination, _)| destination == name);

        if (an_item_name || shown() || registered()) && changed.send(()).is_err() {
            return;
        }
    }
}

// the next round's moment: something changed, and has settled, or a minute has gone by
fn wait_for_change(changes: &Receiver<()>) {
    match changes.recv_timeout(AT_LEAST_EVERY) {
        Ok(()) => while changes.recv_timeout(SETTLE).is_ok() {},
        Err(RecvTimeoutError::Timeout) => {}
        // nothing can say so any more (no session bus): rounds go on at the slow pace
        Err(RecvTimeoutError::Disconnected) => std::thread::sleep(AT_LEAST_EVERY),
    }
}

// every app that might have an icon: registered ones first (in the order they registered, with the path they gave),
// then ones another watcher knows, then the rest found on the bus by name. a stable order, so icons don't shuffle
fn candidates(bus: &Bus, names: &[String]) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = lock(&REGISTERED).clone();

    for name in bus.property(WATCHER, "/StatusNotifierWatcher", WATCHER, "RegisteredStatusNotifierItems").list() {
        let service = name.text();

        found.push(match service.split_once('/') {
            Some((name, path)) => (name.to_string(), format!("/{path}")),
            None => (service.to_string(), String::from("/StatusNotifierItem")),
        });
    }

    let mut scanned: Vec<&String> =
        names.iter().filter(|name| name.starts_with("org.kde.StatusNotifierItem-") || name.starts_with("org.freedesktop.StatusNotifierItem-")).collect();

    scanned.sort();

    found.extend(scanned.into_iter().map(|name| (name.clone(), String::from("/StatusNotifierItem"))));

    // the first entry for an app wins, which is the path it registered with
    let mut seen = std::collections::HashSet::new();

    found.retain(|(destination, _)| seen.insert(destination.clone()));

    found
}

// reads one app's properties. None when it asks to be hidden (Passive)
fn fetch_item(destination: &str, path: &str, refresh_icon: bool, previous: Option<&TrayItem>) -> Option<TrayItem> {
    let bus = Bus::session();

    let text = |name: &str| bus.property(destination, path, ITEM, name).text().to_string();

    if text("Status") == "Passive" {
        return None;
    }

    let mut title = text("Title");

    if title.is_empty() {
        title = text("Id");
    }

    let icon = icon_for(&bus, destination, path, &text("IconName"), &text("IconThemePath"), refresh_icon, previous.map(|item| item.icon.as_str()));

    let menu = text("Menu");
    let is_menu = bus.property(destination, path, ITEM, "ItemIsMenu").bool();

    Some(TrayItem { destination: destination.to_string(), path: path.to_string(), title, icon, menu, is_menu })
}

fn poll(changes: &Receiver<()>) {
    let bus = Bus::session();

    // apps whose pixmap has to be read again. a NewIcon doesn't say who sent it, so it marks every app, and an app
    // stays marked until it has answered: one that was slow or busy in that round would otherwise keep its old icon
    let mut stale: std::collections::HashSet<String> = std::collections::HashSet::new();

    loop {
        let names: Vec<String> = bus
            .call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames", &[])
            .list()
            .iter()
            .map(|name| name.text().to_string())
            .collect();

        let everyone = candidates(&bus, &names);

        if ICONS_STALE.swap(false, Ordering::Relaxed) {
            stale.extend(everyone.iter().map(|(destination, _)| destination.clone()));
        }

        stale.retain(|destination| names.contains(destination));

        let before: Vec<TrayItem> = Tray::read().items.clone();

        // each app is asked on a thread of its own. a D-Bus call has no timeout, so asking one after another would let a
        // single frozen app stall every icon forever
        let (sender, receiver) = std::sync::mpsc::channel::<(usize, String, Option<TrayItem>)>();

        let mut slots: Vec<Option<TrayItem>> = Vec::new();
        let mut waiting = 0;

        for (destination, path) in everyone {
            // an app that quit
            if !names.contains(&destination) {
                lock(&REGISTERED).retain(|(known, _)| *known != destination);

                continue;
            }

            let previous = before.iter().find(|item| item.destination == destination).cloned();

            let slot = slots.len();

            slots.push(previous.clone());

            // still being asked from an earlier round (it's hung): keep what we had, don't ask again yet
            if lock(&BUSY).contains(&destination) {
                continue;
            }

            lock(&BUSY).push(destination.clone());

            waiting += 1;

            let sender = sender.clone();
            let refresh_icon = stale.contains(&destination);

            std::thread::spawn(move || {
                let item = fetch_item(&destination, &path, refresh_icon, previous.as_ref());

                lock(&BUSY).retain(|busy| *busy != destination);

                let _ = sender.send((slot, destination, item));
            });
        }

        drop(sender);

        let deadline = Instant::now() + ANSWER_WITHIN;

        for _ in 0..waiting {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { break };

            // an app that doesn't answer in time keeps its old entry in `slots`
            let Ok((slot, destination, item)) = receiver.recv_timeout(left) else { break };

            slots[slot] = item;

            stale.remove(&destination);
        }

        let items: Vec<TrayItem> = slots.into_iter().flatten().collect();

        // an icon that changes every poll (a spinner, a progress badge) would otherwise leave a file behind each time
        let keep: std::collections::HashSet<std::path::PathBuf> = items.iter().map(|item| std::path::PathBuf::from(&item.icon)).collect();

        crate::cache::prune("tray-", &keep);

        if before != items {
            Tray::write().items = items;
        }

        wait_for_change(changes);
    }
}

fn icon_for(bus: &Bus, destination: &str, path: &str, name: &str, theme_path: &str, refresh: bool, previous: Option<&str>) -> String {
    if !name.is_empty() {
        for extension in ["svg", "png"] {
            let file = format!("{theme_path}/{name}.{extension}");

            if !theme_path.is_empty() && std::path::Path::new(&file).exists() {
                return file;
            }
        }

        if let Some(file) = crate::icons::resolve(name) {
            return file;
        }
    }

    // raw pixels (Electron, Steam) arrive as one value per byte, a quarter of a million of them for a big icon. so they
    // are only fetched when the app says the icon changed, or when we never had one
    if !refresh && let Some(previous) = previous.filter(|previous| !previous.is_empty()) {
        return previous.to_string();
    }

    pixmap_file(bus, destination, path)
}

// the biggest pixmap an item offers, written as a png
fn pixmap_file(bus: &Bus, destination: &str, path: &str) -> String {
    let pixmaps = bus.property(destination, path, ITEM, "IconPixmap");

    let mut best: Option<(u32, u32, Vec<u8>)> = None;

    for pixmap in pixmaps.list() {
        let fields = pixmap.list();

        if fields.len() < 3 {
            continue;
        }

        let (width, height) = (fields[0].number().max(0.0) as u64, fields[1].number().max(0.0) as u64);

        // sizes come from any app on the bus: no zero, nothing huge, and the arithmetic below can't overflow
        if width == 0 || height == 0 || width > LARGEST_ICON || height > LARGEST_ICON {
            continue;
        }

        if best.as_ref().is_some_and(|(w, h, _)| u64::from(*w) * u64::from(*h) >= width * height) {
            continue;
        }

        let bytes: Vec<u8> = fields[2].list().iter().map(|byte| byte.number() as u8).collect();

        if bytes.len() as u64 == width * height * 4 {
            best = Some((width as u32, height as u32, bytes));
        }
    }

    let Some((width, height, argb)) = best else { return String::new() };

    // not /tmp: see cache::runtime_dir
    let Some(directory) = crate::cache::runtime_dir() else { return String::new() };

    let file = directory.join(format!("tray-{:x}.png", crate::cache::fnv(&argb)));

    if !file.exists() {
        // the wire format is ARGB, png wants RGBA
        let mut rgba = Vec::with_capacity(argb.len());

        for pixel in argb.chunks_exact(4) {
            rgba.extend_from_slice(&[pixel[1], pixel[2], pixel[3], pixel[0]]);
        }

        // written under another name and renamed, so a half-written icon is never drawn
        let partial = file.with_extension("part");

        if std::fs::write(&partial, png(width, height, &rgba)).is_err() || std::fs::rename(&partial, &file).is_err() {
            return String::new();
        }
    }

    file.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------- a tiny png writer

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;

    for byte in data {
        crc ^= u32::from(*byte);

        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }

    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());

    let mut body = kind.to_vec();
    body.extend_from_slice(data);

    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);

    for row in rgba.chunks_exact(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    // zlib with uncompressed blocks, which is plenty for an icon
    let mut zlib = vec![0x78, 0x01];

    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();

    for (index, block) in blocks.iter().enumerate() {
        let last = index == blocks.len() - 1;

        zlib.push(u8::from(last));
        zlib.extend_from_slice(&(block.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(block);
    }

    let (mut a, mut b) = (1u32, 0u32);

    for byte in &raw {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }

    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);

    out
}

// ---------------------------------------------------------------- clicks

pub fn click(item: &TrayItem, button: amane::Button, monitor: &str) {
    use crate::panels::{self, Which};

    let wants_menu = matches!(button, amane::Button::Right) || (matches!(button, amane::Button::Left) && item.is_menu);

    if wants_menu {
        // a second click on the same icon closes its menu. a click on another icon opens that one's instead
        if panels::is_open(Which::TrayMenu) && crate::traymenu::is_menu_of(&item.destination) {
            panels::close();

            return;
        }

        // an app without a dbusmenu is asked to draw its own
        if crate::traymenu::open(item, monitor) {
            return;
        }
    }

    let method = match button {
        amane::Button::Left => "Activate",
        amane::Button::Right => "ContextMenu",
        _ => "SecondaryActivate",
    };

    let (destination, path) = (item.destination.clone(), item.path.clone());

    // the app may take a while to answer, so never wait for it on the drawing thread
    std::thread::spawn(move || {
        Bus::session().call(&destination, &path, ITEM, method, &[Argument::Int(0), Argument::Int(0)]);
    });
}

#[cfg(test)]
mod tests {
    use super::{crc32, png, registration};

    #[test]
    fn registrations_in_every_form() {
        let item = |service| registration(service, ":1.7");

        assert_eq!(item("org.kde.StatusNotifierItem-5-1"), (String::from("org.kde.StatusNotifierItem-5-1"), String::from("/StatusNotifierItem")));
        assert_eq!(item(":1.9/org/ayatana/NotificationItem/app"), (String::from(":1.9"), String::from("/org/ayatana/NotificationItem/app")));
        assert_eq!(item("/org/ayatana/NotificationItem/steam"), (String::from(":1.7"), String::from("/org/ayatana/NotificationItem/steam")));
    }

    #[test]
    fn crc32_matches_the_published_values() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    // walks the chunks of a png, checking each checksum, and returns them
    fn chunks(file: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(&file[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

        let mut found = Vec::new();
        let mut at = 8;

        while at < file.len() {
            let length = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = file[at + 4..at + 8].try_into().unwrap();
            let data = file[at + 8..at + 8 + length].to_vec();
            let checksum = u32::from_be_bytes(file[at + 8 + length..at + 12 + length].try_into().unwrap());

            assert_eq!(checksum, crc32(&file[at + 4..at + 8 + length]), "bad checksum on {}", String::from_utf8_lossy(&kind));

            found.push((kind, data));

            at += 12 + length;
        }

        found
    }

    // the bytes inside a zlib stream made of stored blocks, with its checksum verified
    fn unstore(zlib: &[u8]) -> Vec<u8> {
        assert_eq!(&zlib[..2], &[0x78, 0x01]);

        let mut raw = Vec::new();
        let mut at = 2;

        loop {
            let last = zlib[at] == 1;
            let length = u16::from_le_bytes([zlib[at + 1], zlib[at + 2]]);

            assert_eq!(u16::from_le_bytes([zlib[at + 3], zlib[at + 4]]), !length);

            raw.extend_from_slice(&zlib[at + 5..at + 5 + length as usize]);

            at += 5 + length as usize;

            if last {
                break;
            }
        }

        let (mut a, mut b) = (1u32, 0u32);

        for byte in &raw {
            a = (a + u32::from(*byte)) % 65521;
            b = (b + a) % 65521;
        }

        assert_eq!(&zlib[at..], &((b << 16) | a).to_be_bytes());

        raw
    }

    #[test]
    fn small_image_round_trips() {
        let pixels: Vec<u8> = (0..2 * 3 * 4).map(|value| value as u8).collect();

        let file = png(2, 3, &pixels);
        let parts = chunks(&file);

        assert_eq!(parts.iter().map(|(kind, _)| kind.as_slice()).collect::<Vec<_>>(), [b"IHDR".as_slice(), b"IDAT", b"IEND"]);

        // 2x3, 8 bits per channel, RGBA
        assert_eq!(parts[0].1, [0, 0, 0, 2, 0, 0, 0, 3, 8, 6, 0, 0, 0]);

        let raw = unstore(&parts[1].1);

        // every row starts with the "no filter" byte
        assert_eq!(raw.len(), 3 * (1 + 2 * 4));

        for (index, row) in raw.chunks(9).enumerate() {
            assert_eq!(row[0], 0);
            assert_eq!(&row[1..], &pixels[index * 8..index * 8 + 8]);
        }
    }

    #[test]
    fn big_images_use_several_blocks() {
        // 200 x 100 x 4 bytes is more than the 65535 a stored block holds
        let pixels = vec![0x7f; 200 * 100 * 4];

        let parts = chunks(&png(200, 100, &pixels));

        let raw = unstore(&parts[1].1);

        assert_eq!(raw.len(), 100 * (1 + 200 * 4));
    }
}
