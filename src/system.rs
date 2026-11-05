use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read},
    time::Instant,
};

#[derive(Default, Clone, Copy)]
struct CpuTime {
    total: u64,
    idle: u64,
}

#[derive(Default)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub state: char,
    pub cpu: f64,
    pub rss: u64,
    pub threads: u64,
    ticks: u64,
    start: u64,
    generation: u64,
}

#[derive(Default)]
pub struct IoRate {
    pub name: String,
    pub read: f64,
    pub write: f64,
    previous: (u64, u64),
    generation: u64,
}

pub struct System {
    pub cpu: Vec<f64>,
    pub memory_total: u64,
    pub memory_used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub uptime: u64,
    pub load: String,
    pub processes: HashMap<u32, Process>,
    pub network: Vec<IoRate>,
    pub disks: Vec<IoRate>,
    pub disk_space: Option<(u64, u64)>,
    pub mhz: Option<u64>,
    pub error: Option<String>,
    previous_cpu: Vec<CpuTime>,
    previous: Option<Instant>,
    generation: u64,
    hz: f64,
    page_size: u64,
    buffer: String,
}

fn read_into(path: impl AsRef<std::path::Path>, buffer: &mut String) -> io::Result<()> {
    buffer.clear();
    File::open(path)?.read_to_string(buffer)?;
    Ok(())
}

fn number(s: Option<&&str>) -> u64 {
    s.and_then(|s| s.parse().ok()).unwrap_or(0)
}

impl System {
    pub fn new() -> Self {
        // sysconf returns -1 on error; use conservative Linux defaults then.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let pages = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        Self {
            cpu: Vec::new(),
            memory_total: 0,
            memory_used: 0,
            swap_total: 0,
            swap_used: 0,
            uptime: 0,
            load: String::new(),
            processes: HashMap::new(),
            network: Vec::new(),
            disks: Vec::new(),
            disk_space: None,
            mhz: None,
            error: None,
            previous_cpu: Vec::new(),
            previous: None,
            generation: 0,
            hz: if hz > 0 { hz as f64 } else { 100.0 },
            page_size: if pages > 0 { pages as u64 } else { 4096 },
            buffer: String::with_capacity(8192),
        }
    }

    pub fn sample(&mut self) {
        let now = Instant::now();
        let elapsed = self.previous.map(|t| now.duration_since(t).as_secs_f64());
        self.generation = self.generation.wrapping_add(1);
        self.error = None;
        if let Err(e) = self.read_cpu() {
            self.error = Some(format!("CPU counters: {e}"));
            self.cpu.clear();
        }
        if let Err(e) = self.read_memory() {
            self.error = Some(format!("Memory counters: {e}"));
        }
        if read_into("/proc/uptime", &mut self.buffer).is_ok() {
            self.uptime = self
                .buffer
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0) as u64;
        }
        if read_into("/proc/loadavg", &mut self.buffer).is_ok() {
            self.load.clear();
            for part in self.buffer.split_whitespace().take(3) {
                if !self.load.is_empty() {
                    self.load.push(' ');
                }
                self.load.push_str(part);
            }
        }
        self.read_processes(elapsed);
        self.read_network(elapsed);
        self.read_disks(elapsed);
        self.disk_space = root_space();
        self.mhz = fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|khz| khz / 1000);
        self.previous = Some(now);
    }

    fn read_cpu(&mut self) -> io::Result<()> {
        read_into("/proc/stat", &mut self.buffer)?;
        self.cpu.clear();
        let mut count = 0;
        for line in self.buffer.lines().take_while(|l| l.starts_with("cpu")) {
            let mut parts = line.split_whitespace();
            parts.next();
            let mut current = CpuTime::default();
            for (i, value) in parts.take(8).enumerate() {
                let value = value.parse::<u64>().unwrap_or(0);
                current.total = current.total.saturating_add(value);
                if i == 3 || i == 4 {
                    current.idle = current.idle.saturating_add(value);
                }
            }
            let usage = self
                .previous_cpu
                .get(count)
                .and_then(|old| {
                    let total = current.total.checked_sub(old.total)?;
                    let idle = current.idle.checked_sub(old.idle)?;
                    (total > 0).then(|| {
                        (100.0 * (total.saturating_sub(idle)) as f64 / total as f64)
                            .clamp(0.0, 100.0)
                    })
                })
                .unwrap_or(0.0);
            self.cpu.push(usage);
            if let Some(old) = self.previous_cpu.get_mut(count) {
                *old = current;
            } else {
                self.previous_cpu.push(current);
            }
            count += 1;
        }
        self.previous_cpu.truncate(count);
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "no CPU counters",
            ));
        }
        Ok(())
    }

    fn read_memory(&mut self) -> io::Result<()> {
        read_into("/proc/meminfo", &mut self.buffer)?;
        let (mut total, mut available, mut swap, mut free_swap) = (None, None, 0, 0);
        for line in self.buffer.lines() {
            let mut p = line.split_whitespace();
            let key = p.next().unwrap_or("");
            let bytes = p
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
                .saturating_mul(1024);
            match key {
                "MemTotal:" => total = Some(bytes),
                "MemAvailable:" => available = Some(bytes),
                "SwapTotal:" => swap = bytes,
                "SwapFree:" => free_swap = bytes,
                _ => {}
            }
        }
        self.memory_total =
            total.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "MemTotal missing"))?;
        self.memory_used =
            self.memory_total.saturating_sub(available.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "MemAvailable missing")
            })?);
        self.swap_total = swap;
        self.swap_used = swap.saturating_sub(free_swap);
        Ok(())
    }

    fn read_processes(&mut self, elapsed: Option<f64>) {
        let Ok(entries) = fs::read_dir("/proc") else {
            self.error = Some("Cannot read /proc processes".into());
            return;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if read_into(entry.path().join("stat"), &mut self.buffer).is_err() {
                continue;
            }
            let Some(stat) = parse_stat(&self.buffer) else {
                continue;
            };
            let p = self.processes.entry(pid).or_default();
            p.cpu = match elapsed {
                Some(dt) if dt > 0.0 && p.start == stat.start && p.generation != 0 => {
                    stat.ticks.saturating_sub(p.ticks) as f64 * 100.0 / self.hz / dt
                }
                _ => 0.0,
            };
            p.pid = pid;
            if p.name != stat.name {
                p.name.clear();
                p.name.push_str(stat.name);
            }
            p.state = stat.state;
            p.rss = stat.rss.saturating_mul(self.page_size);
            p.threads = stat.threads;
            p.ticks = stat.ticks;
            p.start = stat.start;
            p.generation = self.generation;
        }
        self.processes
            .retain(|_, p| p.generation == self.generation);
    }

    fn read_network(&mut self, elapsed: Option<f64>) {
        if read_into("/proc/net/dev", &mut self.buffer).is_err() {
            self.network.clear();
            return;
        }
        for line in self.buffer.lines().skip(2) {
            let Some((name, values)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if name == "lo" {
                continue;
            }
            let mut p = values.split_whitespace();
            let rx = p.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let tx = p.nth(7).and_then(|s| s.parse().ok()).unwrap_or(0);
            update_rate(&mut self.network, name, (rx, tx), elapsed, self.generation);
        }
        self.network.retain(|r| r.generation == self.generation);
    }

    fn read_disks(&mut self, elapsed: Option<f64>) {
        if read_into("/proc/diskstats", &mut self.buffer).is_err() {
            self.disks.clear();
            return;
        }
        for line in self.buffer.lines() {
            let mut p = line.split_whitespace();
            p.next();
            p.next();
            let Some(name) = p.next() else {
                continue;
            };
            if name.starts_with("loop")
                || name.starts_with("ram")
                || name.starts_with("dm-")
                || !std::path::Path::new("/sys/block").join(name).exists()
            {
                continue;
            }
            let read = p
                .nth(2)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
                .saturating_mul(512);
            let write = p
                .nth(3)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
                .saturating_mul(512);
            update_rate(
                &mut self.disks,
                name,
                (read, write),
                elapsed,
                self.generation,
            );
        }
        self.disks.retain(|r| r.generation == self.generation);
    }
}

fn update_rate(
    rows: &mut Vec<IoRate>,
    name: &str,
    current: (u64, u64),
    elapsed: Option<f64>,
    generation: u64,
) {
    let index = rows.iter().position(|r| r.name == name).unwrap_or_else(|| {
        rows.push(IoRate {
            name: name.to_owned(),
            ..Default::default()
        });
        rows.len() - 1
    });
    let row = &mut rows[index];
    let dt = elapsed.filter(|dt| *dt > 0.0 && row.generation != 0);
    row.read = dt
        .and_then(|dt| current.0.checked_sub(row.previous.0).map(|n| n as f64 / dt))
        .unwrap_or(0.0);
    row.write = dt
        .and_then(|dt| current.1.checked_sub(row.previous.1).map(|n| n as f64 / dt))
        .unwrap_or(0.0);
    row.previous = current;
    row.generation = generation;
}

struct Stat<'a> {
    name: &'a str,
    state: char,
    ticks: u64,
    start: u64,
    rss: u64,
    threads: u64,
}

fn parse_stat(text: &str) -> Option<Stat<'_>> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close <= open {
        return None;
    }
    let mut fields = [""; 22];
    let mut count = 0;
    for (slot, field) in fields
        .iter_mut()
        .zip(text.get(close + 1..)?.split_whitespace())
    {
        *slot = field;
        count += 1;
    }
    if count < 22 {
        return None;
    }
    Some(Stat {
        name: &text[open + 1..close],
        state: fields[0].chars().next()?,
        ticks: number(fields.get(11)).saturating_add(number(fields.get(12))),
        start: fields[19].parse().ok()?,
        rss: fields[21].parse::<i64>().ok()?.max(0) as u64,
        threads: fields[17].parse().ok()?,
    })
}

fn root_space() -> Option<(u64, u64)> {
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // Fixed NUL-terminated path and correctly sized output. No allocation.
    if unsafe { libc::statvfs(c"/".as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    let size = stat.f_frsize;
    Some((
        stat.f_blocks
            .saturating_sub(stat.f_bfree)
            .saturating_mul(size),
        stat.f_blocks.saturating_mul(size),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_names_can_contain_spaces_and_parentheses() {
        let s = parse_stat(
            "42 (a ) tricky (name)) R 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21",
        )
        .unwrap();
        assert_eq!(s.name, "a ) tricky (name)");
        assert_eq!((s.ticks, s.threads, s.start, s.rss), (23, 17, 19, 21));
        assert!(parse_stat("42 (gone) R 1").is_none());
    }
    #[test]
    fn counter_resets_do_not_make_negative_rates() {
        let mut rows = Vec::new();
        update_rate(&mut rows, "net", (200, 300), None, 1);
        update_rate(&mut rows, "net", (600, 500), Some(2.0), 2);
        assert_eq!((rows[0].read, rows[0].write), (200.0, 100.0));
        update_rate(&mut rows, "net", (1, 1), Some(1.0), 3);
        assert_eq!((rows[0].read, rows[0].write), (0.0, 0.0));
    }
}
