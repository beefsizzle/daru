use std::process::{Command, Stdio};
use std::time::Duration;

use amane::{Audio, Button, Center, Column, Horizontal, LayerWindow, Monitor, Rectangle, Row, Service, Start, Widget, children};

use crate::panels::{self, Which};
use crate::theme::{Theme, alpha, mix};
use crate::ui::{bold, hover, icon, label, round_button, section, set_hover, slider};
use crate::volume::{self, Level};

/*
 * the sound dropdown: volume and mute for the speakers and the microphone, and which device each one uses.
 * volumes come from Amane's Audio service, the list of devices from pactl (PipeWire answers it too)
 */
const WIDTH: f32 = 372.0;
const INNER: f32 = WIDTH - 28.0;
const ROW: f32 = 30.0;
const MAX_DEVICES: usize = 4;

#[derive(Clone, Default, PartialEq)]
pub struct Device {
    pub name: String,        // what pactl calls it
    pub description: String, // what a person calls it
}

#[derive(Default, PartialEq)]
pub struct Devices {
    pub sinks: Vec<Device>,
    pub sources: Vec<Device>,
    pub default_sink: String,
    pub default_source: String,
}

// `pactl list sinks` prints a block per device: "Sink #89", then "\tName: ..." and "\tDescription: ..." lines
fn list(kind: &str) -> Vec<Device> {
    let text = amane::output(&format!("LC_ALL=C pactl list {kind}s"));

    let heading = format!("{} #", kind[..1].to_uppercase() + &kind[1..]);

    let mut devices: Vec<Device> = Vec::new();

    for line in text.lines() {
        if line.starts_with(&heading) {
            devices.push(Device::default());
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

fn read_all() -> Devices {
    Devices {
        sinks: list("sink"),
        sources: list("source"),
        default_sink: amane::output("pactl get-default-sink").trim().to_string(),
        default_source: amane::output("pactl get-default-source").trim().to_string(),
    }
}

impl Service for Devices {
    fn new() -> Self {
        Self::default()
    }

    // a few process spawns a second, so only while the dropdown is on screen
    fn listen() {
        loop {
            panels::wait_until_open(&[Which::Audio]);

            let fresh = read_all();

            if fresh != *Self::read() {
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

fn device_row(t: &Theme, id: &str, device: &Device, active: bool, kind: &'static [&'static str]) -> Rectangle {
    let hot = hover(id);

    let key = id.to_string();

    let name = device.name.clone();

    Rectangle::new()
        .width(INNER)
        .height(ROW)
        .fill(if active { alpha(t.accent, 0.16) } else { mix(alpha(t.surface, 0.0), t.surface_hi, hot) })
        .padding(amane::Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 })
        .align_child(Start, Center)
        .cursor(amane::Pointer)
        .on_hover(move |inside| set_hover(&key, inside))
        .on_click(move |_: Button| run(kind, name.clone()))
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
        rows.push(Box::new(device_row(t, &format!("{id}-{index}"), device, device.name == default, kind)));
    }

    let count = rows.len() as f32;

    let height = 14.0 + 8.0 + 30.0 + 8.0 + if count > 0.0 { count * ROW + (count - 1.0) * 2.0 } else { 0.0 };

    let mut parts: Vec<Box<dyn Widget>> = vec![Box::new(section(title, t)), Box::new(controls)];

    if !rows.is_empty() {
        parts.push(Box::new(Column::new(rows).gap(2.0)));
    }

    (Column::new(parts).gap(8.0), height)
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
            percent: Level::Output.shown(audio.volume()),
            muted: audio.muted(),
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
            percent: Level::Input.shown(audio.microphone_volume()),
            muted: audio.microphone_muted(),
            mute: Audio::toggle_microphone_mute,
            changed: volume::drag_input,
            scrolled: volume::scroll_input,
            devices: &devices.sources,
            default: &devices.default_source,
            kind: &["set-default-source"],
        },
    );

    let height = 28.0 + 22.0 + 14.0 + output_height + 14.0 + 1.0 + 14.0 + input_height;

    let divider = Rectangle::new().width(INNER).height(1.0).fill(t.border);

    panels::window_at(WIDTH, height, Horizontal::Right, crate::bar::right_edge(crate::bar::Slot::Audio))
        .child(
            panels::body(&t).padding(14.0).child(
                Column::new(vec![Box::new(bold("Sound", 15.0, t.fg)) as Box<dyn Widget>, Box::new(output), Box::new(divider), Box::new(input)]).gap(14.0),
            ),
        )
}

