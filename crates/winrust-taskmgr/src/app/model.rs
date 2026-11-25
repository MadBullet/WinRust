//! What the Task Manager shows, and the thread that samples it.
//!
//! The UI thread never blocks: one background thread refreshes processes, I/O
//! counters, GPU and system CPU and hands over a finished [`Snapshot`].
//! Process identity is `(pid, creation time)`, so recycled PIDs are not
//! confused when rows move between frames.
//!
//! Nothing here draws: [`Row`] holds a process and its six metric values, and
//! [`Snapshot`] is one finished sample.  The ordering rules below are model
//! rules too - what "running first" means is not a painting decision.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winrust_core::gpu;
use winrust_core::process::ProcessInfo;
use winrust_core::system::{CpuSample, MemoryStatus};
use winrust_core::ts;

use super::format::{mb, rate};
use super::SortBy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Metric {
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
    GpuEngine,
}

impl Metric {
    fn caption(self) -> &'static str {
        match self {
            Metric::Cpu => ts("CPU"),
            Metric::Memory => ts("Memory"),
            Metric::Disk => ts("Disk"),
            Metric::Network => ts("Network"),
            Metric::Gpu => ts("GPU"),
            Metric::GpuEngine => ts("GPU engine"),
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Metric::Cpu => "▦",
            Metric::Memory => "▤",
            Metric::Disk => "💽",
            Metric::Network => "🌐",
            Metric::Gpu => "🎮",
            Metric::GpuEngine => "◫",
        }
    }
    fn short(self) -> &'static str {
        match self {
            Metric::Cpu => "Ц",
            Metric::Memory => "П",
            Metric::Disk => "Д",
            Metric::Network => "С",
            Metric::Gpu => "GPU",
            Metric::GpuEngine => "Я",
        }
    }
    const ALL: [Metric; 6] = [
        Metric::Cpu,
        Metric::Memory,
        Metric::Disk,
        Metric::Network,
        Metric::Gpu,
        Metric::GpuEngine,
    ];
}

/// Row data for one process plus its six metric values.
#[derive(Debug, Clone, Default)]
pub(super) struct Row {
    pub(super) info: ProcessInfo,
    /// Task Manager's per-process CPU share of the whole machine.
    pub(super) cpu_percent: f32,
    pub(super) disk_bps: f64,
    pub(super) net_bps: f64,
    pub(super) gpu_percent: f32,
    pub(super) gpu_engine: String,
    /// Application grouping ("Приложения" vs "Фоновые процессы").
    pub(super) group: String,
    /// Application name shown instead of the exe (Task Manager shows
    /// "Проводник", we show the file description from the version resource).
    pub(super) app_name: String,
    /// Aggregate of the whole group, drawn on the group header.
    pub(super) group_cpu: f32,
    pub(super) group_mem: u64,
    pub(super) group_gpu: f32,
    pub(super) group_count: usize,
}

impl Row {
    fn metric(&self, m: Metric) -> String {
        match m {
            Metric::Cpu => format!("{:.1}%", self.cpu_percent),
            Metric::Memory => mb(self.info.private_bytes),
            Metric::Disk => rate(self.disk_bps),
            Metric::Network => rate(self.net_bps),
            Metric::Gpu => format!("{:.1}%", self.gpu_percent),
            Metric::GpuEngine => {
                if self.gpu_engine.is_empty() {
                    "—".into()
                } else {
                    self.gpu_engine.clone()
                }
            }
        }
    }
    fn metric_heat(&self, m: Metric) -> f32 {
        match m {
            Metric::Cpu => self.cpu_percent,
            Metric::Memory => {
                (self.info.private_bytes as f64 / (512.0 * 1024.0 * 1024.0)) as f32 * 100.0
            }
            Metric::Disk => (self.disk_bps / (1024.0 * 1024.0)) as f32 * 100.0,
            Metric::Network => (self.net_bps / (1024.0 * 1024.0)) as f32 * 100.0,
            Metric::Gpu => self.gpu_percent,
            Metric::GpuEngine => 0.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct Snapshot {
    pub(super) rows: Vec<Row>,
    pub(super) memory: MemoryStatus,
    pub(super) cpu_percent: f32,
    pub(super) cpu: CpuSample,
    pub(super) gpu: gpu::GpuSample,
    pub(super) uptime_ms: u64,
    pub(super) took: Duration,
}

pub(super) struct Sampler {
    pub(super) rx: Receiver<Snapshot>,
    pub(super) stop: Arc<AtomicBool>,
}

/// Start the sampling thread.  Any panic inside is caught so a driver quirk
/// cannot take the whole program down.
pub(super) fn spawn_sampler(interval: Duration) -> Sampler {
    let (tx, rx) = channel::<Snapshot>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        // A panicking sampler used to abort the process; now it degrades to
        // "no data" and the UI keeps running.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut prev_cpu = system::cpu_sample();
            let mut prev_proc: HashMap<u32, (u64, u64)> = HashMap::new();
            let mut prev_io: HashMap<u32, u64> = HashMap::new();
            let mut prev_net: HashMap<u32, u64> = HashMap::new();
            let mut prev_wall = Instant::now();
            let cores = system::cpu_count().max(1) as f64;
            let mut gpu = GpuQuery::open();
            let mut app_names: HashMap<String, String> = HashMap::new();

            while !stop2.load(Ordering::Relaxed) {
                let t0 = Instant::now();
                let now_cpu = system::cpu_sample();
                let cpu_percent = system::cpu_usage_between(prev_cpu, now_cpu);
                let procs = process::enumerate().unwrap_or_default();
                let wall = prev_wall.elapsed().as_secs_f64().max(0.001);
                let net_totals = net::net_bytes_per_pid();

                // GPU: engine utilisation grouped by owning PID.
                let mut per_pid_gpu: HashMap<u32, (f32, String)> = HashMap::new();
                let mut gpu_sample = gpu::GpuSample::default();
                if let Some(q) = gpu.as_mut() {
                    if let Some(engines) = q.sample() {
                        let mut by_adapter: HashMap<u32, f32> = HashMap::new();
                        for (name, pct) in &engines {
                            // PDH hands out doubles; the UI works in f32.
                            let pct = *pct as f32;
                            let pid = pid_from_engine(name).unwrap_or(0);
                            let adapter = gpu::adapter_of(name);
                            let e = by_adapter.entry(adapter).or_insert(0.0);
                            if pct > *e {
                                *e = pct;
                            }
                            if pid != 0 {
                                let entry = per_pid_gpu.entry(pid).or_insert((0.0, String::new()));
                                if pct > entry.0 {
                                    entry.0 = pct;
                                    entry.1 = engine_short(name);
                                }
                            }
                        }
                        gpu_sample.by_adapter = by_adapter.into_iter().collect();
                        gpu_sample.overall = gpu_sample
                            .by_adapter
                            .iter()
                            .map(|(_, v)| *v)
                            .fold(0.0_f32, f32::max);
                    }
                }

                let mut rows = Vec::with_capacity(procs.len());
                let mut new_proc = HashMap::with_capacity(procs.len());
                let mut new_io: HashMap<u32, u64> = HashMap::with_capacity(procs.len());
                let mut new_net: HashMap<u32, u64> = HashMap::with_capacity(procs.len());
                for p in procs {
                    new_proc.insert(p.pid, (p.created_100ns, p.cpu_time_100ns));
                    let cpu_percent = match prev_proc.get(&p.pid).copied() {
                        Some((pc, pu)) if p.created_100ns == pc => {
                            let delta = p.cpu_time_100ns.saturating_sub(pu) as f64;
                            (delta / 10_000_000.0 / wall / cores * 100.0) as f32
                        }
                        _ => 0.0,
                    };
                    let io_total = p.io_read_bytes.saturating_add(p.io_write_bytes);
                    let disk_bps = match prev_io.get(&p.pid) {
                        Some(prev) => io_total.saturating_sub(*prev) as f64 / wall,
                        None => 0.0,
                    };
                    new_io.insert(p.pid, io_total);
                    let net_total = net_totals.get(&p.pid).copied().unwrap_or(0);
                    let net_bps = match prev_net.get(&p.pid) {
                        Some(prev) => net_total.saturating_sub(*prev) as f64 / wall,
                        None => 0.0,
                    };
                    new_net.insert(p.pid, net_total);

                    // Human application name, resolved once per image path.
                    let app_name = if p.exe_path.is_empty() {
                        p.name.clone()
                    } else {
                        app_names
                            .entry(p.exe_path.clone())
                            .or_insert_with(|| {
                                let vi = shell::version_info(&p.exe_path);
                                if vi.description.is_empty() {
                                    p.name.clone()
                                } else {
                                    vi.description
                                }
                            })
                            .clone()
                    };
                    let (gpu_percent, gpu_engine) = per_pid_gpu
                        .get(&p.pid)
                        .cloned()
                        .map(|(pct, eng)| (pct, eng))
                        .unwrap_or((0.0, String::new()));
                    let group = group_key(&p);
                    rows.push(Row {
                        info: p,
                        cpu_percent,
                        disk_bps,
                        net_bps,
                        gpu_percent,
                        gpu_engine,
                        group,
                        app_name,
                        ..Default::default()
                    });
                }
                prev_proc = new_proc;
                prev_io = new_io;
                prev_net = new_net;
                prev_cpu = now_cpu;
                prev_wall = Instant::now();

                // Windows groups a child process under the application that
                // started it: the helpers of Explorer belong to Explorer, not to
                // a group of their own.  Walk the parent chain to the topmost
                // ancestor that is itself an application row and use its name as
                // the group key; everything else keeps its own name.
                {
                    let mut info: HashMap<u32, (u32, String, bool)> = HashMap::new();
                    for r in &rows {
                        info.insert(
                            r.info.pid,
                            (r.info.parent_pid, r.app_name.clone(), is_app_row(r)),
                        );
                    }
                    let mut resolved: HashMap<u32, String> = HashMap::new();
                    for r in &rows {
                        let mut pid = r.info.pid;
                        let mut group = r.app_name.clone();
                        // Depth-limited so a corrupt parent link cannot loop.
                        for _ in 0..32 {
                            let Some((parent, name, is_app)) = info.get(&pid) else {
                                break;
                            };
                            if *is_app {
                                group = name.clone();
                                break;
                            }
                            if *parent == 0 || *parent == pid {
                                break;
                            }
                            pid = *parent;
                        }
                        resolved.insert(r.info.pid, group);
                    }
                    for r in &mut rows {
                        if let Some(g) = resolved.get(&r.info.pid) {
                            r.group = g.to_lowercase();
                        }
                    }
                }

                // Group aggregates for the "Приложения (N)" headers.
                let mut groups: HashMap<String, (f32, u64, usize, f32)> = HashMap::new();
                for r in &rows {
                    let e = groups.entry(r.group.clone()).or_default();
                    e.0 += r.cpu_percent;
                    e.1 += r.info.private_bytes;
                    e.2 += 1;
                    e.3 += r.gpu_percent;
                }
                for r in &mut rows {
                    if let Some((cpu, mem, count, gpu)) = groups.get(&r.group) {
                        r.group_cpu = *cpu;
                        r.group_mem = *mem;
                        r.group_count = *count;
                        r.group_gpu = *gpu;
                    }
                }
                rows.sort_by(|a, b| {
                    b.cpu_percent
                        .partial_cmp(&a.cpu_percent)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(b.info.private_bytes.cmp(&a.info.private_bytes))
                });

                let snap = Snapshot {
                    rows,
                    memory: system::memory_status(),
                    cpu_percent,
                    cpu: now_cpu,
                    gpu: gpu_sample,
                    uptime_ms: system::uptime_ms(),
                    took: t0.elapsed(),
                };
                if tx.send(snap).is_err() {
                    break;
                }
                let elapsed = t0.elapsed();
                if elapsed < interval {
                    std::thread::sleep(interval - elapsed);
                }
            }
        }));
        if result.is_err() {
            // No `log` crate in this binary: the sampler thread panicked, but the
            // UI keeps drawing whatever the last good snapshot was.
            eprintln!("winrust-taskmgr: process sampler stopped after a panic; UI keeps running");
        }
    });
    Sampler { rx, stop }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// `pid_1234_luid_0x00000000_0x0000A1B2_phys_0_eng_3_engtype_3D`
fn pid_from_engine(instance: &str) -> Option<u32> {
    let rest = instance.strip_prefix("pid_")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// `3D`, `Copy`, `VideoDecode` — the part Task Manager prints in "GPU engine".
fn engine_short(instance: &str) -> String {
    match instance.split("engtype_").nth(1) {
        Some(t) => {
            let end = t.find('_').unwrap_or(t.len());
            t[..end].to_string()
        }
        None => String::new(),
    }
}

fn group_key(p: &ProcessInfo) -> String {
    p.name.to_lowercase()
}

/// Order rows for the Status column: running first, then suspended, then
/// "access denied" rows.  Sorting status alphabetically would be useless.
pub(super) fn status_rank(r: &Row) -> u8 {
    if r.info.suspended {
        1
    } else if r.info.access == winrust_core::process::AccessLevel::Denied {
        2
    } else {
        0
    }
}

/// "Apps" are processes that own a visible top-level window; without a window
/// list the cheapest good proxy is "has a version description and is not a
/// Windows host process".  Background processes are the rest.
pub(super) fn is_app_row(r: &Row) -> bool {
    let n = r.info.name.to_lowercase();
    let host = n.starts_with("svchost")
        || n.starts_with("conhost")
        || n.starts_with("dllhost")
        || n.starts_with("runtimebroker")
        || n.starts_with("backgroundtaskhost")
        || n.starts_with("searchhost")
        || n.starts_with("taskhostw")
        || n.starts_with("sihost")
        || n.starts_with("ctfmon");
    if host {
        return false;
    }
    !r.app_name.is_empty()
}

pub(super) fn sort_rows(rows: &mut [Row], by: SortBy, desc: bool) {
    let cmp = |a: &Row, b: &Row| -> std::cmp::Ordering {
        let ord = match by {
            SortBy::Name => a.info.name.to_lowercase().cmp(&b.info.name.to_lowercase()),
            SortBy::Cpu => a
                .cpu_percent
                .partial_cmp(&b.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortBy::Memory => a.info.private_bytes.cmp(&b.info.private_bytes),
            SortBy::Disk => a
                .disk_bps
                .partial_cmp(&b.disk_bps)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortBy::Network => a
                .net_bps
                .partial_cmp(&b.net_bps)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortBy::Gpu => a
                .gpu_percent
                .partial_cmp(&b.gpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortBy::GpuEngine => a
                .gpu_engine
                .to_lowercase()
                .cmp(&b.gpu_engine.to_lowercase()),
            SortBy::Status => status_rank(a).cmp(&status_rank(b)),
            SortBy::Priority => a.info.priority.rank().cmp(&b.info.priority.rank()),
            SortBy::Pid => a.info.pid.cmp(&b.info.pid),
            SortBy::UserName => a
                .info
                .user_name
                .to_lowercase()
                .cmp(&b.info.user_name.to_lowercase()),
            SortBy::Path => a
                .info
                .exe_path
                .to_lowercase()
                .cmp(&b.info.exe_path.to_lowercase()),
        };
        if desc {
            ord.reverse()
        } else {
            ord
        }
    };
    rows.sort_by(cmp);
}

/// First token of a startup command line, with quotes stripped.
pub(super) fn exe_from_command(cmd: &str) -> String {
    let t = cmd.trim();
    if let Some(rest) = t.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return winrust_core::i18n::head(rest, end).to_string();
        }
        return rest.to_string();
    }
    t.split_whitespace().next().unwrap_or("").to_string()
}
