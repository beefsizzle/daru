use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use amane::Service;

use crate::panels::{self, Which};

/*
 * the numbers behind the System dropdown (system.rs), which the bar's CPU and RAM chips open: every core, the GPU,
 * memory and swap, temperatures, disks and the busiest processes. all of it comes from /proc and /sys, read every two
 * seconds, and only while the dropdown is open. the thread sleeps until then, so a shell with it closed pays nothing
 */
#[derive(Default, Clone, PartialEq)]
pub struct SysInfo {
    pub ready: bool,
    pub uptime: String,    // "3h 7m"
    pub model: String,     // "AMD Ryzen 7 9800X3D 8-Core Processor"
    pub cores: Vec<f32>,   // each thread's load, 0 to 100
    pub total: f32,        // all of them together
    pub ghz: f32,          // the average clock right now
    pub load: [f32; 3],    // load average over 1, 5 and 15 minutes
    pub memory: Memory,
    pub gpu: Option<Gpu>,  // only an AMD card says how busy it is through /sys
    pub temps: Vec<(String, f32)>, // ("CPU", 45.0), ("GPU", 40.0), ...
    pub disks: Vec<Disk>,
    pub processes: Vec<Process>,
}

#[derive(Default, Clone, PartialEq)]
pub struct Memory {
    pub total: u64, // bytes
    pub used: u64,  // what programs hold: everything but what the kernel could hand back at once
    pub cached: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Clone, PartialEq)]
pub struct Gpu {
    pub busy: f32, // percent
    pub vram_used: u64,
    pub vram_total: u64,
}

#[derive(Clone, PartialEq)]
pub struct Disk {
    pub place: String, // where it's mounted, the shortest of its places
    pub used: u64,
    pub size: u64,
}

#[derive(Clone, PartialEq)]
pub struct Process {
    pub name: String,
    pub cpu: f32, // percent of one core, like top: a busy program on four cores reads 400
    pub memory: u64,
}

impl Service for SysInfo {
    fn new() -> Self {
        Self::default()
    }

    /*
     * load is the difference between two readings, so after a pause the first round only takes a baseline and the
     * second, half a second later rather than two, fills the dropdown. everything is read before the lock is taken
     */
    fn listen() {
        let model = model();
        let gpu = gpu_folder();

        loop {
            panels::wait_until_open(&[Which::System]);

            let mut before = Sample::take();
            let mut round = 0u32;
            let mut disks = Vec::new();

            std::thread::sleep(Duration::from_millis(500));

            while panels::is_open(Which::System) {
                let now = Sample::take();

                // the disks change slowly and cost a process, so when it opens and then every half minute
                if round.is_multiple_of(15) {
                    disks = self::disks();
                }

                let info = SysInfo {
                    ready: true,
                    uptime: crate::sys::uptime(),
                    model: model.clone(),
                    cores: now.cores(&before),
                    total: now.total(&before),
                    ghz: ghz(),
                    load: load(),
                    memory: memory(),
                    gpu: gpu.as_deref().map(read_gpu),
                    temps: temps(gpu.as_deref()),
                    disks: disks.clone(),
                    processes: now.busiest(&before, PROCESSES),
                };

                if info != *Self::read() {
                    *Self::write() = info;
                }

                before = now;
                round += 1;

                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

// ---------------------------------------------------------------- cpu and processes

// the counters load is worked out from, at one moment
struct Sample {
    at: Instant,
    cores: Vec<(u64, u64)>, // each thread's (busy, all) time, in ticks
    all: (u64, u64),
    processes: HashMap<u32, (String, u64, u64)>, // pid: (name, ticks of cpu so far, bytes in memory)
}

// ticks per second, which /proc counts cpu time in. 100 on every Linux build that matters
const TICKS: f32 = 100.0;

const PAGE: u64 = 4096;

// how many of the busiest processes the dropdown lists
const PROCESSES: usize = 5;

impl Sample {
    fn take() -> Self {
        let text = fs::read_to_string("/proc/stat").unwrap_or_default();

        let mut cores = Vec::new();
        let mut all = (0, 0);

        for line in text.lines().take_while(|line| line.starts_with("cpu")) {
            let mut fields = line.split_whitespace();

            let name = fields.next().unwrap_or("");

            let ticks: Vec<u64> = fields.filter_map(|field| field.parse().ok()).collect();

            // user nice system idle iowait irq softirq steal: idle and iowait are the time spent doing nothing
            let total: u64 = ticks.iter().take(8).sum();
            let idle = ticks.get(3).unwrap_or(&0) + ticks.get(4).unwrap_or(&0);

            if name == "cpu" {
                all = (total - idle, total);
            } else {
                cores.push((total - idle, total));
            }
        }

        Self { at: Instant::now(), cores, all, processes: processes() }
    }

    fn cores(&self, before: &Self) -> Vec<f32> {
        self.cores.iter().zip(&before.cores).map(|(now, then)| share(*now, *then)).collect()
    }

    fn total(&self, before: &Self) -> f32 {
        share(self.all, before.all)
    }

    // processes with the same name count as one, the way a browser's many helpers add up to the browser
    fn busiest(&self, before: &Self, count: usize) -> Vec<Process> {
        let seconds = self.at.duration_since(before.at).as_secs_f32().max(0.1);

        let mut by_name: HashMap<&str, Process> = HashMap::new();

        for (pid, (name, ticks, memory)) in &self.processes {
            // a process that started since the last look has no earlier reading to compare with
            let cpu = before.processes.get(pid).map_or(0.0, |(_, earlier, _)| ticks.saturating_sub(*earlier) as f32 / TICKS / seconds * 100.0);

            let entry = by_name.entry(name.as_str()).or_insert_with(|| Process { name: name.clone(), cpu: 0.0, memory: 0 });

            entry.cpu += cpu;
            entry.memory += memory;
        }

        let mut list: Vec<Process> = by_name.into_values().collect();

        list.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory)));
        list.truncate(count);

        list
    }
}

fn share((busy, all): (u64, u64), (busy_before, all_before): (u64, u64)) -> f32 {
    let all = all.saturating_sub(all_before);

    if all == 0 {
        return 0.0;
    }

    (busy.saturating_sub(busy_before) as f32 / all as f32 * 100.0).clamp(0.0, 100.0)
}

// every process's name, cpu time and memory. a few hundred small files, only while the dropdown is open
fn processes() -> HashMap<u32, (String, u64, u64)> {
    let mut found = HashMap::new();

    let Ok(entries) = fs::read_dir("/proc") else {
        return found;
    };

    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };

        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };

        if let Some(process) = parse_stat(&stat) {
            found.insert(pid, process);
        }
    }

    found
}

/*
 * "1234 (some name) S 1 ...": the name is between the first ( and the last ), since it may hold spaces and brackets
 * of its own. after it, utime and stime are the 12th and 13th fields and rss (in pages) the 22nd
 */
fn parse_stat(stat: &str) -> Option<(String, u64, u64)> {
    let name = stat.get(stat.find('(')? + 1..stat.rfind(')')?)?.to_string();

    let fields: Vec<&str> = stat.get(stat.rfind(')')? + 2..)?.split_whitespace().collect();

    let number = |index: usize| fields.get(index).and_then(|field| field.parse::<u64>().ok());

    Some((name, number(11)? + number(12)?, number(21)? * PAGE))
}

fn model() -> String {
    let text = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();

    text.lines()
        .find_map(|line| line.strip_prefix("model name").and_then(|rest| rest.split_once(':')).map(|(_, name)| name.trim().to_string()))
        .unwrap_or_default()
}

fn ghz() -> f32 {
    let text = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();

    let clocks: Vec<f32> = text
        .lines()
        .filter_map(|line| line.strip_prefix("cpu MHz").and_then(|rest| rest.split_once(':')).and_then(|(_, value)| value.trim().parse().ok()))
        .collect();

    if clocks.is_empty() { 0.0 } else { clocks.iter().sum::<f32>() / clocks.len() as f32 / 1000.0 }
}

fn load() -> [f32; 3] {
    let text = fs::read_to_string("/proc/loadavg").unwrap_or_default();

    let mut numbers = text.split_whitespace().filter_map(|field| field.parse().ok());

    [numbers.next().unwrap_or(0.0), numbers.next().unwrap_or(0.0), numbers.next().unwrap_or(0.0)]
}

// ---------------------------------------------------------------- memory

fn memory() -> Memory {
    let text = fs::read_to_string("/proc/meminfo").unwrap_or_default();

    let field = |name: &str| -> u64 {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };

    let total = field("MemTotal");
    let swap_total = field("SwapTotal");

    Memory {
        total,
        used: total.saturating_sub(field("MemAvailable")),
        cached: field("Cached") + field("Buffers") + field("SReclaimable"),
        swap_total,
        swap_used: swap_total.saturating_sub(field("SwapFree")),
    }
}

// ---------------------------------------------------------------- gpu and temperatures

// the AMD card with the most video memory, so a desktop's graphics card wins over the processor's own graphics
fn gpu_folder() -> Option<PathBuf> {
    let entries = fs::read_dir("/sys/class/drm").ok()?;

    entries
        .flatten()
        .map(|entry| entry.path().join("device"))
        .filter(|device| device.join("gpu_busy_percent").exists())
        .filter_map(|device| Some((number(&device.join("mem_info_vram_total"))?, device)))
        .max_by_key(|(vram, _)| *vram)
        .map(|(_, device)| device)
}

fn read_gpu(device: &Path) -> Gpu {
    Gpu {
        busy: number(&device.join("gpu_busy_percent")).unwrap_or(0) as f32,
        vram_used: number(&device.join("mem_info_vram_used")).unwrap_or(0),
        vram_total: number(&device.join("mem_info_vram_total")).unwrap_or(0),
    }
}

fn number(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

// a sensor's first reading, in degrees
fn degrees(folder: &Path, input: &str) -> Option<f32> {
    number(&folder.join(input)).map(|millidegrees| millidegrees as f32 / 1000.0).filter(|degrees| *degrees > 0.0)
}

/*
 * the processor (k10temp on AMD, coretemp on Intel), the graphics card from the same folder its load came from
 * (edge is the card's surface, junction its hottest spot), and each NVMe drive
 */
fn temps(gpu: Option<&Path>) -> Vec<(String, f32)> {
    let mut found = Vec::new();

    let Ok(entries) = fs::read_dir("/sys/class/hwmon") else {
        return found;
    };

    let mut drives = 0;

    let mut folders: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();

    folders.sort();

    for folder in folders {
        let name = fs::read_to_string(folder.join("name")).unwrap_or_default();

        match name.trim() {
            "k10temp" | "coretemp" => found.extend(degrees(&folder, "temp1_input").map(|reading| (String::from("CPU"), reading))),
            "nvme" => {
                drives += 1;

                found.extend(degrees(&folder, "temp1_input").map(|reading| (if drives == 1 { String::from("NVMe") } else { format!("NVMe {drives}") }, reading)));
            }
            _ => {}
        }
    }

    // the card's own sensors live under its device folder
    let card = gpu.and_then(|device| fs::read_dir(device.join("hwmon")).ok()?.flatten().next().map(|entry| entry.path()));

    if let Some(card) = card {
        found.extend(degrees(&card, "temp1_input").map(|reading| (String::from("GPU"), reading)));
        found.extend(degrees(&card, "temp2_input").map(|reading| (String::from("GPU hotspot"), reading)));
    }

    // processor first, then the card, then the drives
    found.sort_by_key(|(name, _)| match name.as_str() {
        "CPU" => 0,
        name if name.starts_with("GPU") => 1,
        _ => 2,
    });

    found
}

// ---------------------------------------------------------------- disks

/*
 * each drive once. btrfs mounts one drive in several places (/, /home, /var/log...), so they're told apart by the
 * device, and the shortest place names it. small ones like /boot are left out
 */
fn disks() -> Vec<Disk> {
    let text = amane::output("df -B1 -x tmpfs -x devtmpfs -x efivarfs -x overlay -x squashfs --output=source,target,used,size");

    let mut by_source: Vec<(String, Disk)> = Vec::new();

    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();

        let [source, place, used, size] = fields[..] else {
            continue;
        };

        let (Ok(used), Ok(size)) = (used.parse::<u64>(), size.parse::<u64>()) else {
            continue;
        };

        if size < 8 << 30 {
            continue;
        }

        match by_source.iter_mut().find(|(known, _)| known == source) {
            Some((_, disk)) if place.len() < disk.place.len() => disk.place = place.to_string(),
            Some(_) => {}
            None => by_source.push((source.to_string(), Disk { place: place.to_string(), used, size })),
        }
    }

    by_source.into_iter().map(|(_, disk)| disk).collect()
}

/// "12.3 GB", "512 MB"
pub fn bytes(amount: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    const TB: f64 = (1u64 << 40) as f64;

    let amount = amount as f64;

    if amount >= TB {
        format!("{:.1} TB", amount / TB)
    } else if amount >= GB {
        format!("{:.1} GB", amount / GB)
    } else {
        format!("{:.0} MB", amount / (1u64 << 20) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_process_whose_name_has_spaces_and_brackets() {
        let stat = "4321 (Web (Content) 2) S 1 4321 4321 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 30 0 1000 900000000 2048 18446744073709551615";

        let (name, ticks, memory) = parse_stat(stat).expect("failed to read the stat line");

        assert_eq!(name, "Web (Content) 2");
        assert_eq!(ticks, 300);
        assert_eq!(memory, 2048 * PAGE);
    }

    #[test]
    fn load_is_the_busy_share_of_the_time_between_two_readings() {
        assert_eq!(share((150, 400), (100, 300)), 50.0);
        assert_eq!(share((100, 300), (100, 300)), 0.0);
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(bytes(512 << 20), "512 MB");
        assert_eq!(bytes(12 << 30), "12.0 GB");
        assert_eq!(bytes(2 << 40), "2.0 TB");
    }
}
