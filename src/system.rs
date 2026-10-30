use amane::{Center, Column, End, Horizontal, LayerWindow, Monitor, Parent, Rectangle, Row, Service, SpaceBetween, Start, Text, Widget, children};

use crate::dashboard::{gauge, severity};
use crate::panels::{self, Which};
use crate::sysinfo::{SysInfo, bytes};
use crate::theme::Theme;
use crate::ui::{bold, label, meter, section};

/*
 * the System dropdown, from the bar's CPU and RAM chips: the dashboard's meters with more behind them. every core,
 * the graphics card, memory and temperatures side by side, the drives, and what's keeping the processor busy. the
 * numbers come from sysinfo.rs, which only reads them while this is open
 */
const WIDTH: f32 = 372.0;
const INNER: f32 = WIDTH - 28.0;

const GAUGE: f32 = 96.0; // a dashboard gauge: its number, 53 of meter, its caption
const METER: f32 = 53.0; // 12 blocks of 3.5 with a gap of 1 between them
const ROW: f32 = 16.0;
const ROW_GAP: f32 = 6.0;

pub fn view(monitor: &Monitor) -> LayerWindow {
    let Some(t) = panels::look(Which::System, monitor) else {
        return panels::hidden("amane-panel");
    };

    let info = SysInfo::read();

    let mut parts: Vec<Box<dyn Widget>> = Vec::new();

    // the title, and the processor's name under it
    parts.push(Box::new(
        Column::new(children![
            Row::new(children![
                bold("System", 15.0, t.fg),
                Rectangle::new().width(Parent).height(1.0),
                label(if info.uptime.is_empty() { String::new() } else { format!("up {}", info.uptime) }, 11.5, t.dim),
            ])
            .align(Center)
            .width(INNER),
            label(&info.model, 11.5, t.dim),
        ])
        .gap(2.0),
    ));

    let mut height = 28.0 + 32.0;

    if info.ready {
        parts.push(Box::new(gauges(&t, &info)));
        parts.push(Box::new(cores(&t, &info)));

        let (tiles, tiles_height) = tiles(&t, &info);

        parts.push(Box::new(tiles));

        height += 14.0 + GAUGE + 14.0 + cores_height() + 14.0 + tiles_height;

        if !info.disks.is_empty() {
            parts.push(Box::new(disks(&t, &info)));

            height += 14.0 + block_height(info.disks.len());
        }

        if !info.processes.is_empty() {
            parts.push(Box::new(processes(&t, &info)));

            height += 14.0 + list_height(info.processes.len() + 1);
        }
    }

    panels::window_at(WIDTH, height, Horizontal::Right, crate::bar::right_edge(crate::bar::Slot::System))
        .child(panels::body(&t).padding(14.0).child(Column::new(parts).gap(14.0)))
}

// the dashboard's row of meters, with the graphics card's two when there is one
fn gauges(t: &Theme, info: &SysInfo) -> Row {
    let memory = &info.memory;

    let percent = |used: u64, total: u64| if total == 0 { 0.0 } else { used as f32 / total as f32 * 100.0 };

    let mut list: Vec<Box<dyn Widget>> = children![
        gauge(t, "CPU", info.total, format!("{:.0}", info.total)),
        gauge(t, "RAM", percent(memory.used, memory.total), format!("{:.0}", percent(memory.used, memory.total))),
    ];

    if let Some(gpu) = &info.gpu {
        let vram = percent(gpu.vram_used, gpu.vram_total);

        list.push(Box::new(gauge(t, "GPU", gpu.busy, format!("{:.0}", gpu.busy))));
        list.push(Box::new(gauge(t, "VRAM", vram, format!("{vram:.0}"))));
    }

    list.push(Box::new(gauge(t, "Swap", percent(memory.swap_used, memory.swap_total), format!("{:.0}", percent(memory.swap_used, memory.swap_total)))));

    if let Some((_, degrees)) = info.temps.first() {
        list.push(Box::new(gauge(t, "Temp", *degrees, format!("{degrees:.0}\u{b0}"))));
    }

    Row::new(list).justify(SpaceBetween).width(INNER)
}

fn cores_height() -> f32 {
    24.0 + 14.0 + 8.0 + METER
}

// a thin meter for every thread, with the clock and the load average above them
fn cores(t: &Theme, info: &SysInfo) -> Rectangle {
    let count = info.cores.len().max(1) as f32;
    let gap = 4.0;
    let width = ((INNER - 24.0 - gap * (count - 1.0)) / count).floor();

    let meters: Vec<Box<dyn Widget>> =
        info.cores.iter().map(|load| Box::new(meter(t, width, load / 100.0, severity(t, *load))) as Box<dyn Widget>).collect();

    let [one, five, fifteen] = info.load;

    let heading = Row::new(children![
        section(&format!("{} threads", info.cores.len()), t),
        Rectangle::new().width(Parent).height(1.0),
        label(format!("{:.2} GHz   load {one:.2} {five:.2} {fifteen:.2}", info.ghz), 11.0, t.dim),
    ])
    .align(Center)
    .width(INNER - 24.0);

    surface(t, INNER, cores_height()).child(Column::new(children![heading, Row::new(meters).gap(gap)]).gap(8.0))
}

// a name on the left and its value on the right, the way the weather's details are laid out
fn stat(t: &Theme, name: &str, value: String, width: f32) -> Row {
    Row::new(children![label(name, 11.5, t.dim), Rectangle::new().width(Parent).height(1.0), label(value, 11.5, t.fg)])
        .align(Center)
        .width(width)
        .height(ROW)
}

// memory on the left, temperatures on the right
fn tiles(t: &Theme, info: &SysInfo) -> (Row, f32) {
    let width = (INNER - 10.0) / 2.0;
    let inside = width - 24.0;

    let memory = &info.memory;

    let memory_rows: Vec<Box<dyn Widget>> = children![
        stat(t, "Used", format!("{} / {}", short(memory.used), bytes(memory.total)), inside),
        stat(t, "Cached", bytes(memory.cached), inside),
        stat(t, "Swap", format!("{} / {}", short(memory.swap_used), bytes(memory.swap_total)), inside),
    ];

    let temperature_rows: Vec<Box<dyn Widget>> =
        info.temps.iter().map(|(name, degrees)| Box::new(stat(t, name, format!("{degrees:.0}\u{b0}"), inside)) as Box<dyn Widget>).collect();

    let rows = memory_rows.len().max(temperature_rows.len());
    let height = block_height(rows);

    let tile = |title: &str, rows: Vec<Box<dyn Widget>>| {
        surface(t, width, height).child(Column::new(children![section(title, t), Column::new(rows).gap(ROW_GAP)]).gap(8.0))
    };

    (Row::new(children![tile("Memory", memory_rows), tile("Temperatures", temperature_rows)]).gap(10.0), height)
}

// "12.3" for the part before " / 31.2 GB", so the unit is said once
fn short(amount: u64) -> String {
    format!("{:.1}", amount as f64 / (1u64 << 30) as f64)
}

// each drive: where it's mounted, how full, and how much of how much
fn disks(t: &Theme, info: &SysInfo) -> Rectangle {
    let track = 120.0;

    let rows: Vec<Box<dyn Widget>> = info
        .disks
        .iter()
        .map(|disk| {
            let full = if disk.size == 0 { 0.0 } else { disk.used as f32 / disk.size as f32 };

            let bar = Rectangle::new()
                .width(track)
                .height(4.0)
                .radius(0.0)
                .fill(t.surface_hi)
                .align_child(Start, Center)
                .child(Rectangle::new().width((track * full).max(4.0)).height(4.0).radius(0.0).fill(severity(t, full * 100.0)));

            Box::new(
                Row::new(children![
                    Rectangle::new().width(INNER - 24.0 - track - 110.0 - 20.0).height(ROW).child(label(&disk.place, 11.5, t.fg).elide()),
                    bar,
                    Rectangle::new().width(110.0).height(ROW).align_child(End, Center).child(label(format!("{} / {}", bytes(disk.used), bytes(disk.size)), 11.5, t.dim)),
                ])
                .gap(10.0)
                .align(Center),
            ) as Box<dyn Widget>
        })
        .collect();

    surface(t, INNER, block_height(info.disks.len())).child(Column::new(children![section("Drives", t), Column::new(rows).gap(ROW_GAP)]).gap(8.0))
}

// the busiest few, with how much of a core each one keeps busy and the memory it holds
fn processes(t: &Theme, info: &SysInfo) -> Rectangle {
    let name_width = INNER - 24.0 - 56.0 - 70.0 - 16.0;

    let row = |name: Text, cpu: Text, memory: Text| -> Box<dyn Widget> {
        Box::new(
            Row::new(children![
                Rectangle::new().width(name_width).height(ROW).child(name),
                Rectangle::new().width(56.0).height(ROW).align_child(End, Center).child(cpu),
                Rectangle::new().width(70.0).height(ROW).align_child(End, Center).child(memory),
            ])
            .gap(8.0)
            .align(Center),
        )
    };

    // the column headings take the place a section title has in the other blocks
    let mut rows: Vec<Box<dyn Widget>> = vec![row(section("Processes", t), section("CPU", t), section("Memory", t))];

    for process in &info.processes {
        rows.push(row(
            label(&process.name, 11.5, t.fg).elide(),
            label(format!("{:.0}%", process.cpu), 11.5, if process.cpu >= 50.0 { t.fg } else { t.dim }),
            label(bytes(process.memory), 11.5, t.dim),
        ));
    }

    surface(t, INNER, list_height(rows.len())).child(Column::new(rows).gap(ROW_GAP))
}

// a block of `rows` rows
fn list_height(rows: usize) -> f32 {
    24.0 + rows as f32 * (ROW + ROW_GAP) - ROW_GAP
}

// the same with a section title above them
fn block_height(rows: usize) -> f32 {
    list_height(rows) + 14.0 + 8.0
}

fn surface(t: &Theme, width: f32, height: f32) -> Rectangle {
    Rectangle::new().width(width).height(height).radius(0.0).fill(t.surface).padding(12.0)
}
