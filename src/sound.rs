use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use amane::{Apps, Audio, Button, Center, Column, Horizontal, LayerWindow, Monitor, Rectangle, Row, Service, Start, Widget, children};

use crate::panels::{self, Which};
use crate::sync::lock;
use crate::theme::{RADIUS_SMALL, Theme, alpha, mix};
use crate::ui::{app_icon, bold, hover, icon, label, round_button, section, set_hover, slider};
use crate::volume::{self, Level};

/*
 * the sound dropdown: volume and mute for the speakers and the microphone, and which device each one uses, then a
 * row for every app that's playing sound, with its own volume, mute and speaker. volumes come from Amane's Audio
 * service, the devices and the apps from pactl (PipeWire answers it too)
 */
const WIDTH: f32 = 372.0;
const INNER: f32 = WIDTH - 28.0;
const ROW: f32 = 30.0;
const MAX_DEVICES: usize = 4;
const MAX_APPS: usize = 6;

// an app row: its name over its slider
const APP_NAME: f32 = 20.0;
const APP_HEIGHT: f32 = APP_NAME + 6.0 + 30.0;

// how wide the button naming an app's speaker is
const SPEAKER_CHIP: f32 = 130.0;

#[derive(Clone, Default, PartialEq)]
pub struct Device {
    pub index: u32,          // the number pactl gives it, which is how an app's stream says where it plays
    pub name: String,        // what pactl calls it
    pub description: String, // what a person calls it
}

/*
 * an app that's playing sound. pactl lists streams, and one app can have several (a browser with two tabs playing,
 * Discord's call and its notification sounds), so they're gathered under the app's name and whatever is done to the
 * row is done to all of them. the level and mute shown are the first stream's
 */
#[derive(Clone, PartialEq)]
pub struct App {
    pub name: String,
    pub icon: Option<String>, // a file, found once per app and remembered in ICONS
    pub streams: Vec<u32>,
    pub percent: u8,
    pub muted: bool,
    pub sink: u32, // the Device::index it plays on
}

#[derive(Default, PartialEq)]
pub struct Devices {
    pub sinks: Vec<Device>,
    pub sources: Vec<Device>,
    pub default_sink: String,
    pub default_source: String,
    pub apps: Vec<App>,
}

// `pactl list sinks` prints a block per device: "Sink #89", then "\tName: ..." and "\tDescription: ..." lines
fn list(kind: &str) -> Vec<Device> {
    let text = amane::output(&format!("LC_ALL=C pactl list {kind}s"));

    let heading = format!("{} #", kind[..1].to_uppercase() + &kind[1..]);

    let mut devices: Vec<Device> = Vec::new();

    for line in text.lines() {
        if let Some(index) = line.strip_prefix(&heading) {
            devices.push(Device { index: index.trim().parse().unwrap_or(u32::MAX), ..Device::default() });
        } else if let Some(device) = devices.last_mut() {
            let line = line.trim();

            if let Some(name) = line.strip_prefix("Name: ") {
                device.name = name.to_string();
            } else if let Some(description) = line.strip_prefix("Description: ") {
                device.description = description.to_string();
            }
        }
    }

    // a source that only mirrors a speaker isn't a microphone
    devices.retain(|device| !device.name.is_empty() && !device.name.ends_with(".monitor"));

    devices
}

// one stream as `pactl list sink-inputs` prints it
#[derive(Default)]
struct Stream {
    index: u32,
    sink: u32,
    percent: u8,
    muted: bool,
    name: String,   // application.name, like "Firefox"
    icon: String,   // application.icon_name, which few apps set
    binary: String, // application.process.binary, like "firefox" or ".Discord-wrapped"
    event: bool,    // a click or notification sound (media.role "event"), over before it could be clicked
}

/*
 * a block per stream: "Sink Input #288", then "\tSink: 63", "\tMute: no", "\tVolume: front-left: 0 /   0% / -inf dB,
 * ..." and, under "Properties:", lines like `application.name = "paplay"`
 */
fn streams() -> Vec<Stream> {
    let text = amane::output("LC_ALL=C pactl list sink-inputs");

    let mut streams: Vec<Stream> = Vec::new();

    for line in text.lines() {
        if let Some(index) = line.strip_prefix("Sink Input #") {
            streams.push(Stream { index: index.trim().parse().unwrap_or(u32::MAX), ..Stream::default() });

            continue;
        }

        let Some(stream) = streams.last_mut() else { continue };

        let line = line.trim();

        if let Some(sink) = line.strip_prefix("Sink: ") {
            stream.sink = sink.trim().parse().unwrap_or(u32::MAX);
        } else if let Some(mute) = line.strip_prefix("Mute: ") {
            stream.muted = mute.trim() == "yes";
        } else if let Some(volume) = line.strip_prefix("Volume: ") {
            // the first channel's percent: the number in front of the first '%'
            let percent = volume.split('%').next().and_then(|before| before.rsplit(' ').next()).and_then(|number| number.parse::<u16>().ok());

            stream.percent = percent.unwrap_or(0).min(255) as u8;
        } else if let Some((key, value)) = line.split_once(" = ") {
            let value = value.trim_matches('"').to_string();

            match key {
                "application.name" => stream.name = value,
                "application.icon_name" => stream.icon = value,
                "application.process.binary" => stream.binary = value,
                "media.role" => stream.event = value == "event",
                _ => {}
            }
        }
    }

    streams.retain(|stream| stream.index != u32::MAX && !stream.event);

    streams
}

// what a program is called on disk: NixOS starts many through a wrapper, which leaves ".Discord-wrapped"
fn plain_binary(binary: &str) -> String {
    binary.trim_start_matches('.').trim_end_matches("-wrapped").to_lowercase()
}

// app name -> icon file, found once. a miss is remembered too, so an app without one doesn't search every second
static ICONS: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/*
 * the icon the app names, else the one of the installed app with the same name or program (the launcher's list),
 * else an icon called like the program or the app
 */
fn app_icon_file(stream: &Stream) -> Option<String> {
    if let Some(known) = lock(&ICONS).get_or_insert_with(HashMap::new).get(&stream.name) {
        return known.clone();
    }

    let binary = plain_binary(&stream.binary);

    let installed = {
        let apps = Apps::read();

        apps.list()
            .iter()
            .find(|app| {
                let program = app.exec().split_whitespace().next().unwrap_or_default();

                let program = program.rsplit('/').next().unwrap_or_default();

                app.name().eq_ignore_ascii_case(&stream.name) || (!binary.is_empty() && plain_binary(program) == binary)
            })
            .and_then(|app| app.icon_path().map(|path| path.to_string_lossy().into_owned()))
    };

    let found = crate::icons::resolve(&stream.icon)
        .or(installed)
        .or_else(|| crate::icons::resolve(&binary))
        .or_else(|| crate::icons::resolve(&stream.name.to_lowercase()));

    lock(&ICONS).get_or_insert_with(HashMap::new).insert(stream.name.clone(), found.clone());

    found
}

// the streams gathered under their app, in the order they started
fn apps() -> Vec<App> {
    let mut apps: Vec<App> = Vec::new();

    for mut stream in streams() {
        if stream.name.is_empty() {
            stream.name = if stream.binary.is_empty() { String::from("Unknown") } else { plain_binary(&stream.binary) };
        }

        if let Some(app) = apps.iter_mut().find(|app| app.name == stream.name) {
            app.streams.push(stream.index);

            continue;
        }

        apps.push(App {
            icon: app_icon_file(&stream),
            name: stream.name,
            streams: vec![stream.index],
            percent: stream.percent,
            muted: stream.muted,
            sink: stream.sink,
        });
    }

    apps
}

fn read_all() -> Devices {
    Devices {
        sinks: list("sink"),
        sources: list("source"),
        default_sink: amane::output("pactl get-default-sink").trim().to_string(),
        default_source: amane::output("pactl get-default-source").trim().to_string(),
        apps: apps(),
    }
}

// when an app's level or mute was last set from here, see `listen`
static LAST_SET: Mutex<Option<Instant>> = Mutex::new(None);

// how long a level just set from here wins over what pactl reads, as the main sliders do (see volume.rs)
const CONFIRM_WITHIN: Duration = Duration::from_millis(600);

impl Service for Devices {
    fn new() -> Self {
        Self::default()
    }

    /*
     * a few process spawns a second, so only while the dropdown is on screen. a level dragged to is written here at
     * once, and pactl may not have it yet when the next read comes round, so a read that soon after is thrown away
     * rather than making the slider jump back for a moment
     */
    fn listen() {
        loop {
            panels::wait_until_open(&[Which::Audio]);

            let fresh = read_all();

            let settling = lock(&LAST_SET).is_some_and(|at| at.elapsed() < CONFIRM_WITHIN);

            if !settling && fresh != *Self::read() {
                *Self::write() = fresh;
            }

            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

// a device name is data from the system, so it's an argument and never part of a shell command
fn run(arguments: &'static [&'static str], name: String) {
    std::thread::spawn(move || {
        let _ = Command::new("pactl").args(arguments).arg(name).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    });
}

// several pactl calls in a row, off the main thread
fn run_all(calls: Vec<Vec<String>>) {
    std::thread::spawn(move || {
        for arguments in calls {
            let _ = Command::new("pactl").args(arguments).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    });
}

// ---------------------------------------------------------------- what the app rows do

/*
 * a drag sends a new level every frame, and a pactl process for each would pile up. so the newest level per app waits
 * here, and one thread sends whatever is newest until nothing is left: levels passed over meanwhile are never sent
 */
static WANTED: Mutex<Option<HashMap<String, (Vec<u32>, u8)>>> = Mutex::new(None);

static SENDING: AtomicBool = AtomicBool::new(false);

fn send_levels() {
    loop {
        let next = {
            let mut wanted = lock(&WANTED);

            let wanted = wanted.get_or_insert_with(HashMap::new);

            let name = wanted.keys().next().cloned();

            name.and_then(|name| wanted.remove(&name))
        };

        let Some((streams, percent)) = next else {
            SENDING.store(false, Ordering::SeqCst);

            // a level that came in between the look above and the store would otherwise wait for the next drag
            let more = lock(&WANTED).as_ref().is_some_and(|wanted| !wanted.is_empty());

            if more && !SENDING.swap(true, Ordering::SeqCst) {
                continue;
            }

            return;
        };

        for stream in streams {
            let stream = stream.to_string();

            for arguments in [["set-sink-input-volume", &stream, &format!("{percent}%")], ["set-sink-input-mute", &stream, "0"]] {
                let _ = Command::new("pactl").args(arguments).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
        }
    }
}

// changes the app's row straight away, so the view doesn't wait for pactl
fn update_app(name: &str, change: impl FnOnce(&mut App)) {
    *lock(&LAST_SET) = Some(Instant::now());

    if let Some(app) = Devices::write().apps.iter_mut().find(|app| app.name == name) {
        change(app);
    }
}

// a level for every stream of the app, which also unmutes it, as moving the main sliders does
fn set_app_level(name: &str, percent: u8) {
    let Some(streams) = Devices::read().apps.iter().find(|app| app.name == name).map(|app| app.streams.clone()) else { return };

    update_app(name, |app| {
        app.percent = percent;
        app.muted = false;
    });

    lock(&WANTED).get_or_insert_with(HashMap::new).insert(name.to_string(), (streams, percent));

    if !SENDING.swap(true, Ordering::SeqCst) {
        std::thread::spawn(send_levels);
    }
}

// a wheel notch is the same 5% as on the main sliders, counted from what's shown. a muted app counts from zero
fn step_app_level(name: &str, notches: i32) {
    let Some((percent, muted)) = Devices::read().apps.iter().find(|app| app.name == name).map(|app| (app.percent, app.muted)) else { return };

    let from = if muted { 0 } else { i32::from(percent) };

    let next = from + notches * 5;

    if notches == 0 || (muted && next <= 0) {
        return;
    }

    set_app_level(name, next.clamp(0, 100) as u8);
}

fn toggle_app_mute(name: &str) {
    let Some((streams, muted)) = Devices::read().apps.iter().find(|app| app.name == name).map(|app| (app.streams.clone(), app.muted)) else { return };

    update_app(name, |app| app.muted = !muted);

    let value = if muted { "0" } else { "1" };

    run_all(streams.iter().map(|stream| vec![String::from("set-sink-input-mute"), stream.to_string(), value.to_string()]).collect());
}

fn move_app(name: &str, sink: &Device) {
    let Some(streams) = Devices::read().apps.iter().find(|app| app.name == name).map(|app| app.streams.clone()) else { return };

    let index = sink.index;

    update_app(name, |app| app.sink = index);

    *Picker::write() = Picker { open: None };

    run_all(streams.iter().map(|stream| vec![String::from("move-sink-input"), stream.to_string(), sink.name.clone()]).collect());
}

// which app's list of speakers is unfolded, by name. a service only so that changing it redraws the dropdown
#[derive(Default)]
struct Picker {
    open: Option<String>,
}

impl Service for Picker {
    fn new() -> Self {
        Self::default()
    }

    fn interval() -> Duration {
        Duration::from_secs(3600)
    }
}

fn toggle_picker(name: &str) {
    let mut picker = Picker::write();

    picker.open = if picker.open.as_deref() == Some(name) { None } else { Some(name.to_string()) };
}

fn device_row(t: &Theme, id: &str, device: &Device, active: bool, on_click: impl Fn() + 'static) -> Rectangle {
    let hot = hover(id);

    let key = id.to_string();

    Rectangle::new()
        .width(INNER)
        .height(ROW)
        .fill(if active { alpha(t.accent, 0.16) } else { mix(alpha(t.surface, 0.0), t.surface_hi, hot) })
        .padding(amane::Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 })
        .align_child(Start, Center)
        .cursor(amane::Pointer)
        .on_hover(move |inside| set_hover(&key, inside))
        .on_click(move |_: Button| on_click())
        .child(
            Row::new(vec![
                Box::new(Rectangle::new().width(14.0).height(14.0).align_child(Center, Center).child(icon(if active { "\u{f00c}" } else { "" }, 11.0, t.accent))) as Box<dyn Widget>,
                Box::new(Rectangle::new().width(INNER - 20.0 - 14.0 - 12.0).height(16.0).child(label(&device.description, 12.0, if active { t.fg } else { t.dim }).elide())),
            ])
            .gap(12.0)
            .align(Center),
        )
}

// what one half of the panel (output or input) shows and does
struct Block<'a> {
    title: &'a str,
    id: &'a str,
    glyph_on: &'a str,
    glyph_off: &'a str,
    percent: u8,
    muted: bool,
    mute: fn(),
    changed: fn(f32),
    scrolled: fn(i32),
    devices: &'a [Device],
    default: &'a str,
    kind: &'static [&'static str],
}

// the slider row and the devices under it
fn block(t: &Theme, block: &Block) -> (Column, f32) {
    let Block { title, id, glyph_on, glyph_off, percent, muted, mute, changed, scrolled, devices, default, kind } = *block;

    let level = f32::from(percent) / 100.0;

    let controls = Row::new(children![
        round_button(&format!("{id}-mute"), t, if muted { glyph_off } else { glyph_on }, if muted { t.faint } else { t.accent }, move |_| mute()),
        slider(t, 250.0, if muted { 0.0 } else { level }, changed, scrolled),
        Rectangle::new().width(44.0).height(20.0).align_child(amane::End, Center).child(label(format!("{percent}%"), 12.0, t.dim)),
    ])
    .gap(10.0)
    .align(Center)
    .width(INNER);

    let mut rows: Vec<Box<dyn Widget>> = Vec::new();

    // the one in use first, so it's on screen however many devices there are
    let mut ordered: Vec<&Device> = devices.iter().collect();

    ordered.sort_by_key(|device| device.name != default);

    for (index, device) in ordered.into_iter().take(MAX_DEVICES).enumerate() {
        let name = device.name.clone();

        rows.push(Box::new(device_row(t, &format!("{id}-{index}"), device, device.name == default, move || run(kind, name.clone()))));
    }

    let count = rows.len() as f32;

    let height = 14.0 + 8.0 + 30.0 + 8.0 + if count > 0.0 { count * ROW + (count - 1.0) * 2.0 } else { 0.0 };

    let mut parts: Vec<Box<dyn Widget>> = vec![Box::new(section(title, t)), Box::new(controls)];

    if !rows.is_empty() {
        parts.push(Box::new(Column::new(rows).gap(2.0)));
    }

    (Column::new(parts).gap(8.0), height)
}

// the button at the right of an app's name: the speaker it plays on, and an arrow for the list under it
fn speaker_chip(t: &Theme, id: &str, speaker: &str, open: bool, app: String) -> Rectangle {
    let id = format!("{id}-speaker");

    let hot = hover(&id);

    Rectangle::new()
        .width(SPEAKER_CHIP)
        .height(APP_NAME)
        .radius(RADIUS_SMALL)
        .fill(if open { alpha(t.accent, 0.16) } else { mix(alpha(t.surface, 0.0), t.surface_hi, hot) })
        .padding(amane::Padding { top: 0.0, right: 6.0, bottom: 0.0, left: 8.0 })
        .align_child(Start, Center)
        .cursor(amane::Pointer)
        .on_hover(move |inside| set_hover(&id, inside))
        .on_click(move |_: Button| toggle_picker(&app))
        .child(
            Row::new(children![
                Rectangle::new().width(SPEAKER_CHIP - 14.0 - 6.0 - 10.0).height(14.0).child(label(speaker, 11.0, t.dim).elide()),
                icon(if open { "\u{f077}" } else { "\u{f078}" }, 9.0, t.dim),
            ])
            .gap(6.0)
            .align(Center),
        )
}

/*
 * one app: its icon and name, with the speaker it plays on at the right, over its mute button, slider and level.
 * the speaker button unfolds the speakers under the row, and only shows when there's more than one to pick from
 */
fn app_row(t: &Theme, index: usize, app: &App, sinks: &[Device], open: bool) -> (Column, f32) {
    let id = format!("snd-app-{index}");

    let picker = sinks.len() > 1;

    let picture: Box<dyn Widget> = match &app.icon {
        Some(path) => Box::new(Rectangle::new().width(18.0).height(18.0).fill(app_icon(path))),
        None => Box::new(Rectangle::new().width(18.0).height(18.0).align_child(Center, Center).child(icon("\u{f001}", 12.0, t.dim))),
    };

    let name_width = INNER - 18.0 - 10.0 - if picker { SPEAKER_CHIP + 10.0 } else { 0.0 };

    let mut top: Vec<Box<dyn Widget>> = vec![picture, Box::new(Rectangle::new().width(name_width).height(16.0).child(label(&app.name, 12.0, t.fg).elide()))];

    if picker {
        let speaker = sinks.iter().find(|sink| sink.index == app.sink).map_or("", |sink| sink.description.as_str());

        top.push(Box::new(speaker_chip(t, &id, speaker, open, app.name.clone())));
    }

    let percent = if app.muted { 0 } else { app.percent };

    let (mute, drag, scroll) = (app.name.clone(), app.name.clone(), app.name.clone());

    let controls = Row::new(children![
        round_button(&format!("{id}-mute"), t, if app.muted { "\u{f026}" } else { "\u{f028}" }, if app.muted { t.faint } else { t.accent }, move |_| toggle_app_mute(&mute)),
        slider(t, 250.0, f32::from(percent) / 100.0, move |value| set_app_level(&drag, (value * 100.0).round() as u8), move |notches| step_app_level(&scroll, notches)),
        // an app can raise itself past 100%, which the slider can't show, so the number says it in the accent
        Rectangle::new().width(44.0).height(20.0).align_child(amane::End, Center).child(label(format!("{percent}%"), 12.0, if percent > 100 { t.accent } else { t.dim })),
    ])
    .gap(10.0)
    .align(Center)
    .width(INNER);

    let mut parts: Vec<Box<dyn Widget>> = vec![Box::new(Row::new(top).gap(10.0).align(Center).width(INNER)), Box::new(controls)];

    let mut height = APP_HEIGHT;

    if open && picker {
        let mut rows: Vec<Box<dyn Widget>> = Vec::new();

        for (number, sink) in sinks.iter().take(MAX_DEVICES).enumerate() {
            let (name, sink_for_click) = (app.name.clone(), sink.clone());

            rows.push(Box::new(device_row(t, &format!("{id}-sink-{number}"), sink, sink.index == app.sink, move || move_app(&name, &sink_for_click))));
        }

        let count = rows.len() as f32;

        height += 6.0 + count * ROW + (count - 1.0) * 2.0;

        parts.push(Box::new(Column::new(rows).gap(2.0)));
    }

    (Column::new(parts).gap(6.0), height)
}

// every app playing sound, or nothing when none is
fn apps_block(t: &Theme, devices: &Devices) -> Option<(Column, f32)> {
    if devices.apps.is_empty() {
        return None;
    }

    let open = Picker::read().open.clone();

    let mut rows: Vec<Box<dyn Widget>> = Vec::new();

    // the section title and the gap under it
    let mut height = 14.0 + 8.0;

    for (index, app) in devices.apps.iter().take(MAX_APPS).enumerate() {
        let (row, row_height) = app_row(t, index, app, &devices.sinks, open.as_deref() == Some(app.name.as_str()));

        height += row_height + if index > 0 { 12.0 } else { 0.0 };

        rows.push(Box::new(row));
    }

    Some((Column::new(vec![Box::new(section("Apps", t)) as Box<dyn Widget>, Box::new(Column::new(rows).gap(12.0))]).gap(8.0), height))
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::Audio, monitor) else {
        return panels::hidden("amane-panel");
    };
    let audio = Audio::read();
    let devices = Devices::read();

    let (output, output_height) = block(
        &t,
        &Block {
            title: "Output",
            id: "snd-out",
            glyph_on: "\u{f028}",
            glyph_off: "\u{f026}",
            percent: Level::Output.percent(audio.volume(), audio.muted()),
            muted: Level::Output.muted(audio.muted()),
            mute: Audio::toggle_mute,
            changed: volume::drag_output,
            scrolled: volume::scroll_output,
            devices: &devices.sinks,
            default: &devices.default_sink,
            kind: &["set-default-sink"],
        },
    );

    let (input, input_height) = block(
        &t,
        &Block {
            title: "Input",
            id: "snd-in",
            glyph_on: "\u{f130}",
            glyph_off: "\u{f131}",
            percent: Level::Input.percent(audio.microphone_volume(), audio.microphone_muted()),
            muted: Level::Input.muted(audio.microphone_muted()),
            mute: Audio::toggle_microphone_mute,
            changed: volume::drag_input,
            scrolled: volume::scroll_input,
            devices: &devices.sources,
            default: &devices.default_source,
            kind: &["set-default-source"],
        },
    );

    let mut height = 28.0 + 22.0 + 14.0 + output_height + 14.0 + 1.0 + 14.0 + input_height;

    let divider = || Rectangle::new().width(INNER).height(1.0).fill(t.border);

    let mut parts: Vec<Box<dyn Widget>> = vec![Box::new(bold("Sound", 15.0, t.fg)), Box::new(output), Box::new(divider()), Box::new(input)];

    if let Some((apps, apps_height)) = apps_block(&t, &devices) {
        height += 14.0 + 1.0 + 14.0 + apps_height;

        parts.push(Box::new(divider()));
        parts.push(Box::new(apps));
    }

    panels::window_at(WIDTH, height, Horizontal::Right, crate::bar::right_edge(crate::bar::Slot::Audio))
        .child(
            panels::body(&t).padding(14.0).child(
                Column::new(parts).gap(14.0),
            ),
        )
}

