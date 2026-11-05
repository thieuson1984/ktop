use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Read},
    time::Instant,
};

const MAX_PROC_BYTES: u64 = 16 * 1024 * 1024;
const MAX_STAT_BYTES: u64 = 64 * 1024;

#[derive(Default, Clone, Copy)]
struct CpuTime {
    total: u64,
    idle: u64,
    generation: u64,
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

impl Process {
    pub fn start_ticks(&self) -> u64 {
        self.start
    }
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
    /// Kernel CPU IDs corresponding to `cpu[1..]`; hotplug does not renumber them.
    pub cpu_ids: Vec<u32>,
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
    previous_cpu: HashMap<Option<u32>, CpuTime>,
    previous: Option<Instant>,
    generation: u64,
    hz: f64,
    page_size: u64,
    buffer: String,
    process_buffer: Vec<u8>,
}

fn read_into(path: impl AsRef<std::path::Path>, buffer: &mut String) -> io::Result<()> {
    buffer.clear();
    File::open(path)?
        .take(MAX_PROC_BYTES + 1)
        .read_to_string(buffer)?;
    if buffer.len() > MAX_PROC_BYTES as usize {
        buffer.clear();
        return Err(invalid_data("proc counters exceed read limit"));
    }
    Ok(())
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl System {
    pub fn new() -> Self {
        // sysconf returns -1 on error; use conservative Linux defaults then.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let pages = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        Self {
            cpu: Vec::new(),
            cpu_ids: Vec::new(),
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
            previous_cpu: HashMap::new(),
            previous: None,
            generation: 0,
            hz: if hz > 0 { hz as f64 } else { 100.0 },
            page_size: if pages > 0 { pages as u64 } else { 4096 },
            buffer: String::with_capacity(8192),
            process_buffer: Vec::with_capacity(1024),
        }
    }

    pub fn sample(&mut self) {
        let now = Instant::now();
        let elapsed = self.previous.map(|t| now.duration_since(t).as_secs_f64());
        self.generation = self.generation.wrapping_add(1);
        self.error = None;
        if let Err(e) = self.read_cpu() {
            self.record_error(&format!("CPU counters: {e}"));
            self.cpu.clear();
            self.cpu_ids.clear();
            self.previous_cpu.clear();
        }
        self.memory_total = 0;
        self.memory_used = 0;
        self.swap_total = 0;
        self.swap_used = 0;
        if let Err(e) = self.read_memory() {
            self.record_error(&format!("Memory counters: {e}"));
        }
        self.uptime = 0;
        match read_into("/proc/uptime", &mut self.buffer)
            .and_then(|()| parse_uptime(&self.buffer).ok_or_else(|| invalid_data("invalid uptime")))
        {
            Ok(uptime) => self.uptime = uptime,
            Err(e) => self.record_error(&format!("Uptime: {e}")),
        }
        self.load.clear();
        match read_into("/proc/loadavg", &mut self.buffer) {
            Ok(()) => {
                if let Some(load) = parse_load(&self.buffer) {
                    for (index, part) in load.into_iter().enumerate() {
                        if index != 0 {
                            self.load.push(' ');
                        }
                        self.load.push_str(part);
                    }
                } else {
                    self.record_error("Load: invalid counters");
                }
            }
            Err(e) => self.record_error(&format!("Load: {e}")),
        }
        self.read_processes(elapsed);
        self.read_network(elapsed);
        self.read_disks(elapsed);
        self.disk_space = root_space();
        if self.disk_space.is_none() {
            self.record_error("Root filesystem counters unavailable");
        }
        self.mhz = read_into(
            "/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq",
            &mut self.buffer,
        )
        .ok()
        .and_then(|()| self.buffer.trim().parse::<u64>().ok())
        .map(|khz| khz / 1000);
        self.previous = Some(now);
        // Release unusual high-water marks after workloads disappear, while
        // retaining ordinary buffers/maps so steady-state samples reuse storage.
        if self.processes.capacity() > self.processes.len().saturating_mul(4).max(256) {
            self.processes
                .shrink_to(self.processes.len().saturating_mul(2).max(128));
        }
        if self.previous_cpu.capacity() > self.previous_cpu.len().saturating_mul(4).max(256) {
            self.previous_cpu
                .shrink_to(self.previous_cpu.len().saturating_mul(2).max(128));
        }
        if self.cpu.capacity() > self.cpu.len().saturating_mul(4).max(256) {
            self.cpu
                .shrink_to(self.cpu.len().saturating_mul(2).max(128));
            self.cpu_ids
                .shrink_to(self.cpu_ids.len().saturating_mul(2).max(128));
        }
        for rows in [&mut self.network, &mut self.disks] {
            if rows.capacity() > rows.len().saturating_mul(4).max(64) {
                rows.shrink_to(rows.len().saturating_mul(2).max(32));
            }
        }
        if self.buffer.capacity() > 64 * 1024 {
            self.buffer.shrink_to(8192);
        }
        if self.process_buffer.capacity() > 8192 {
            self.process_buffer.shrink_to(1024);
        }
    }

    fn record_error(&mut self, message: &str) {
        if let Some(error) = &mut self.error {
            error.push_str("; ");
            error.push_str(message);
        } else {
            self.error = Some(message.to_owned());
        }
    }

    fn read_cpu(&mut self) -> io::Result<()> {
        read_into("/proc/stat", &mut self.buffer)?;
        collect_cpu(
            &self.buffer,
            &mut self.cpu,
            &mut self.cpu_ids,
            &mut self.previous_cpu,
            self.generation,
        )
    }

    fn read_memory(&mut self) -> io::Result<()> {
        read_into("/proc/meminfo", &mut self.buffer)?;
        let (total, used, swap, used_swap) =
            parse_memory(&self.buffer).ok_or_else(|| invalid_data("invalid memory counters"))?;
        self.memory_total = total;
        self.memory_used = used;
        self.swap_total = swap;
        self.swap_used = used_swap;
        Ok(())
    }

    fn read_processes(&mut self, elapsed: Option<f64>) {
        let Ok(entries) = fs::read_dir("/proc") else {
            self.record_error("Cannot read /proc processes");
            self.processes.clear();
            return;
        };
        let mut path = std::path::PathBuf::with_capacity(64);
        path.push("/proc");
        let mut incomplete = false;
        for entry in entries {
            let Ok(entry) = entry else {
                incomplete = true;
                continue;
            };
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            self.process_buffer.clear();
            path.push(&name);
            path.push("stat");
            let read = File::open(&path).and_then(|file| {
                file.take(MAX_STAT_BYTES + 1)
                    .read_to_end(&mut self.process_buffer)
            });
            path.pop();
            path.pop();
            if let Err(error) = read {
                // Exiting processes routinely disappear between read_dir/open.
                incomplete |= error.kind() != io::ErrorKind::NotFound;
                continue;
            }
            if self.process_buffer.len() > MAX_STAT_BYTES as usize {
                incomplete = true;
                continue;
            }
            let Some(stat) = parse_stat(&self.process_buffer) else {
                incomplete = true;
                continue;
            };
            if stat.pid != pid {
                incomplete = true;
                continue;
            }
            let Some(rss) = stat.rss.checked_mul(self.page_size) else {
                incomplete = true;
                continue;
            };
            let p = self.processes.entry(pid).or_default();
            update_process(p, &stat, rss, elapsed, self.hz, self.generation);
        }
        self.processes
            .retain(|_, p| p.generation == self.generation);
        if incomplete {
            self.record_error("Some process counters unavailable or invalid");
        }
    }

    fn read_network(&mut self, elapsed: Option<f64>) {
        if read_into("/proc/net/dev", &mut self.buffer).is_err() {
            self.record_error("Network counters unavailable");
            self.network.clear();
            return;
        }
        let mut incomplete = self.buffer.lines().take(2).count() < 2;
        let mut index = 0;
        for line in self.buffer.lines().skip(2) {
            let Some((name, values)) = line.split_once(':') else {
                incomplete = true;
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                incomplete = true;
                continue;
            }
            if name == "lo" {
                continue;
            }
            let Some(counters) = parse_network_counters(values) else {
                incomplete = true;
                continue;
            };
            update_rate(
                &mut self.network,
                name,
                counters,
                elapsed,
                self.generation,
                1.0,
                index,
            );
            index += 1;
        }
        self.network.retain(|r| r.generation == self.generation);
        if incomplete {
            self.record_error("Some network counters invalid");
        }
    }

    fn read_disks(&mut self, elapsed: Option<f64>) {
        if read_into("/proc/diskstats", &mut self.buffer).is_err() {
            self.record_error("Disk counters unavailable");
            self.disks.clear();
            return;
        }
        let Ok(entries) = fs::read_dir("/sys/block") else {
            self.record_error("Block device inventory unavailable");
            self.disks.clear();
            return;
        };
        let devices: HashSet<_> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        let mut incomplete = false;
        let mut index = 0;
        for line in self.buffer.lines() {
            let mut p = line.split_whitespace();
            p.next();
            p.next();
            let Some(name) = p.next() else {
                incomplete = true;
                continue;
            };
            if name.starts_with("loop")
                || name.starts_with("ram")
                || name.starts_with("dm-")
                || !devices.contains(std::ffi::OsStr::new(name))
            {
                continue;
            }
            let Some(read) = p.nth(2).and_then(|s| s.parse::<u64>().ok()) else {
                incomplete = true;
                continue;
            };
            let Some(write) = p.nth(3).and_then(|s| s.parse::<u64>().ok()) else {
                incomplete = true;
                continue;
            };
            update_rate(
                &mut self.disks,
                name,
                (read, write),
                elapsed,
                self.generation,
                512.0,
                index,
            );
            index += 1;
        }
        self.disks.retain(|r| r.generation == self.generation);
        if incomplete {
            self.record_error("Some disk counters invalid");
        }
    }
}

fn collect_cpu(
    text: &str,
    cpu: &mut Vec<f64>,
    ids: &mut Vec<u32>,
    previous: &mut HashMap<Option<u32>, CpuTime>,
    generation: u64,
) -> io::Result<()> {
    cpu.clear();
    ids.clear();
    let result = (|| {
        for line in text.lines().take_while(|line| line.starts_with("cpu")) {
            let mut parts = line.split_whitespace();
            let label = parts
                .next()
                .ok_or_else(|| invalid_data("missing CPU label"))?;
            let id = if label == "cpu" {
                if !cpu.is_empty() {
                    return Err(invalid_data("duplicate aggregate CPU counters"));
                }
                None
            } else {
                if cpu.is_empty() {
                    return Err(invalid_data("aggregate CPU counters missing"));
                }
                Some(
                    label
                        .strip_prefix("cpu")
                        .and_then(|id| id.parse::<u32>().ok())
                        .ok_or_else(|| invalid_data("invalid CPU ID"))?,
                )
            };
            let mut current = CpuTime {
                generation,
                ..CpuTime::default()
            };
            let mut count = 0;
            // Guest time is already included in user/nice; sum only through steal.
            for (index, value) in parts.take(8).enumerate() {
                let value = value
                    .parse::<u64>()
                    .map_err(|_| invalid_data("invalid CPU counter"))?;
                current.total = current
                    .total
                    .checked_add(value)
                    .ok_or_else(|| invalid_data("CPU counter overflow"))?;
                if index == 3 || index == 4 {
                    current.idle = current
                        .idle
                        .checked_add(value)
                        .ok_or_else(|| invalid_data("CPU idle counter overflow"))?;
                }
                count += 1;
            }
            if count < 4 {
                return Err(invalid_data("incomplete CPU counters"));
            }
            let old = previous.insert(id, current);
            if old.is_some_and(|old| old.generation == generation) {
                return Err(invalid_data("duplicate CPU ID"));
            }
            let usage = old
                .filter(|old| old.generation == generation.wrapping_sub(1))
                .and_then(|old| {
                    let total = current.total.checked_sub(old.total)?;
                    let idle = current.idle.checked_sub(old.idle)?;
                    let busy = total.checked_sub(idle)?;
                    (total > 0).then(|| 100.0 * busy as f64 / total as f64)
                })
                .unwrap_or(0.0);
            cpu.push(usage);
            if let Some(id) = id {
                ids.push(id);
            }
        }
        if cpu.is_empty() {
            return Err(invalid_data("no CPU counters"));
        }
        Ok(())
    })();
    if result.is_err() {
        cpu.clear();
        ids.clear();
        previous.clear();
    } else {
        previous.retain(|_, time| time.generation == generation);
    }
    result
}

fn parse_memory(text: &str) -> Option<(u64, u64, u64, u64)> {
    let (mut total, mut available, mut swap, mut free_swap) = (None, None, None, None);
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let target = match parts.next().unwrap_or("") {
            "MemTotal:" => &mut total,
            "MemAvailable:" => &mut available,
            "SwapTotal:" => &mut swap,
            "SwapFree:" => &mut free_swap,
            _ => continue,
        };
        let value = parts.next()?.parse::<u64>().ok()?.checked_mul(1024)?;
        if parts.next()? != "kB" || parts.next().is_some() || target.replace(value).is_some() {
            return None;
        }
    }
    let total = total.filter(|total| *total > 0)?;
    let swap = swap?;
    Some((
        total,
        total.checked_sub(available?)?,
        swap,
        swap.checked_sub(free_swap?)?,
    ))
}

fn parse_uptime(text: &str) -> Option<u64> {
    let value = text.split_whitespace().next()?;
    let (whole, fraction) = value.split_once('.').unwrap_or((value, "0"));
    if whole.is_empty()
        || fraction.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    whole.parse().ok()
}

fn parse_load(text: &str) -> Option<[&str; 3]> {
    let mut parts = text.split_whitespace();
    let load = [parts.next()?, parts.next()?, parts.next()?];
    load.iter()
        .all(|part| {
            part.parse::<f64>()
                .is_ok_and(|value| value.is_finite() && value >= 0.0)
        })
        .then_some(load)
}

fn parse_network_counters(values: &str) -> Option<(u64, u64)> {
    let mut parts = values.split_whitespace();
    let rx = parts.next()?.parse().ok()?;
    let tx = parts.nth(7)?.parse().ok()?;
    Some((rx, tx))
}

fn update_process(
    process: &mut Process,
    stat: &Stat<'_>,
    rss: u64,
    elapsed: Option<f64>,
    hz: f64,
    generation: u64,
) {
    let pid = stat.pid;
    process.cpu = elapsed
        .filter(|dt| {
            dt.is_finite()
                && *dt > 0.0
                && process.pid == pid
                && process.start == stat.start
                && process.generation == generation.wrapping_sub(1)
        })
        .and_then(|dt| {
            stat.ticks
                .checked_sub(process.ticks)
                .map(|ticks| ticks as f64 * 100.0 / hz / dt)
        })
        .unwrap_or(0.0);
    process.pid = pid;
    let name = String::from_utf8_lossy(stat.name);
    if process.name != name {
        process.name.clear();
        process.name.push_str(&name);
    }
    process.state = stat.state;
    process.rss = rss;
    process.threads = stat.threads;
    process.ticks = stat.ticks;
    process.start = stat.start;
    process.generation = generation;
}

fn update_rate(
    rows: &mut Vec<IoRate>,
    name: &str,
    current: (u64, u64),
    elapsed: Option<f64>,
    generation: u64,
    unit: f64,
    hint: usize,
) {
    // Keep already-sampled rows at the front in source order. Stable inventories
    // take one name comparison per row, rather than a quadratic full-vector scan.
    let hint = hint.min(rows.len());
    let existing = if rows.get(hint).is_some_and(|row| row.name == name) {
        Some(hint)
    } else {
        rows[hint..]
            .iter()
            .position(|row| row.name == name)
            .map(|index| hint + index)
    };
    let index = existing.unwrap_or_else(|| {
        rows.push(IoRate {
            name: name.to_owned(),
            ..Default::default()
        });
        rows.len() - 1
    });
    rows.swap(index, hint);
    let row = &mut rows[hint];
    let dt = elapsed.filter(|dt| {
        dt.is_finite()
            && *dt > 0.0
            && existing.is_some()
            && row.generation == generation.wrapping_sub(1)
    });
    row.read = dt
        .and_then(|dt| {
            current
                .0
                .checked_sub(row.previous.0)
                .map(|n| n as f64 * unit / dt)
        })
        .unwrap_or(0.0);
    row.write = dt
        .and_then(|dt| {
            current
                .1
                .checked_sub(row.previous.1)
                .map(|n| n as f64 * unit / dt)
        })
        .unwrap_or(0.0);
    row.previous = current;
    row.generation = generation;
}

struct Stat<'a> {
    pid: u32,
    name: &'a [u8],
    state: char,
    ticks: u64,
    start: u64,
    rss: u64,
    threads: u64,
}

fn parse_stat(text: &[u8]) -> Option<Stat<'_>> {
    let open = text.iter().position(|byte| *byte == b'(')?;
    let close = text.iter().rposition(|byte| *byte == b')')?;
    if close <= open {
        return None;
    }
    let pid = std::str::from_utf8(text.get(..open)?)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let remaining = std::str::from_utf8(text.get(close + 1..)?).ok()?;
    let mut fields = [""; 22];
    let mut count = 0;
    for (slot, field) in fields.iter_mut().zip(remaining.split_whitespace()) {
        *slot = field;
        count += 1;
    }
    if count < 22 {
        return None;
    }
    if fields[0].len() != 1 || !fields[0].as_bytes()[0].is_ascii_graphic() {
        return None;
    }
    Some(Stat {
        pid,
        name: text.get(open + 1..close)?,
        state: fields[0].as_bytes()[0] as char,
        ticks: fields[11]
            .parse::<u64>()
            .ok()?
            .checked_add(fields[12].parse().ok()?)?,
        start: fields[19].parse().ok()?,
        rss: fields[21].parse().ok()?,
        threads: fields[17].parse().ok()?,
    })
}

fn root_space() -> Option<(u64, u64)> {
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // Fixed NUL-terminated path and correctly sized output. No allocation.
    if unsafe { libc::statvfs(c"/".as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    let size = stat.f_frsize;
    if size == 0 {
        return None;
    }
    Some((
        stat.f_blocks.checked_sub(stat.f_bfree)?.checked_mul(size)?,
        stat.f_blocks.checked_mul(size)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_names_can_contain_spaces_and_parentheses() {
        let s = parse_stat(
            b"42 (a ) tricky (name)) R 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21",
        )
        .unwrap();
        assert_eq!(s.name, b"a ) tricky (name)");
        assert_eq!((s.ticks, s.threads, s.start, s.rss), (23, 17, 19, 21));
        assert!(parse_stat(b"42 (gone) R 1").is_none());
    }
    #[test]
    fn counter_resets_do_not_make_negative_rates() {
        let mut rows = Vec::new();
        update_rate(&mut rows, "net", (200, 300), None, 1, 1.0, 0);
        update_rate(&mut rows, "net", (600, 500), Some(2.0), 2, 1.0, 0);
        assert_eq!((rows[0].read, rows[0].write), (200.0, 100.0));
        update_rate(&mut rows, "net", (1, 1), Some(1.0), 3, 1.0, 0);
        assert_eq!((rows[0].read, rows[0].write), (0.0, 0.0));
    }

    fn stat_fixture(name: &[u8], ticks: u64, start: u64) -> Vec<u8> {
        let mut fields = vec!["0".to_owned(); 22];
        fields[0] = "R".to_owned();
        fields[11] = ticks.to_string();
        fields[17] = "2".to_owned();
        fields[19] = start.to_string();
        fields[21] = "3".to_owned();
        let mut text = b"42 (".to_vec();
        text.extend_from_slice(name);
        text.extend_from_slice(b") ");
        text.extend_from_slice(fields.join(" ").as_bytes());
        text
    }

    #[test]
    fn non_utf8_process_names_do_not_hide_valid_counters() {
        let text = stat_fixture(b"worker ) \xff (", 125, 9);
        let stat = parse_stat(&text).unwrap();
        let mut process = Process::default();
        update_process(&mut process, &stat, 12_288, Some(1.0), 100.0, 1);
        assert_eq!(process.name, "worker ) \u{fffd} (");
        assert_eq!(
            (process.pid, process.start_ticks(), process.rss),
            (42, 9, 12_288)
        );
        assert_eq!(process.cpu, 0.0);
    }

    #[test]
    fn malformed_process_counters_are_not_fake_zero_samples() {
        let valid = String::from_utf8(stat_fixture(b"worker", 125, 9)).unwrap();
        assert!(parse_stat(valid.replace(" 125 ", " invalid ").as_bytes()).is_none());
        assert!(parse_stat(valid.replace(" R ", " RUNNING ").as_bytes()).is_none());
        assert!(parse_stat(valid.replace(" 3", " -3").as_bytes()).is_none());
        let overflow = valid.replace(" 125 0 ", " 18446744073709551615 1 ");
        assert!(parse_stat(overflow.as_bytes()).is_none());
    }

    #[test]
    fn process_rates_require_adjacent_samples_and_matching_start_time() {
        let mut process = Process::default();
        for (generation, ticks, start, expected) in [
            (1, 100, 9, 0.0),
            (2, 150, 9, 25.0),
            (4, 350, 9, 0.0),
            (5, 450, 10, 0.0),
            (6, 500, 10, 25.0),
            (7, 1, 10, 0.0),
        ] {
            let text = stat_fixture(b"worker", ticks, start);
            update_process(
                &mut process,
                &parse_stat(&text).unwrap(),
                12_288,
                Some(2.0),
                100.0,
                generation,
            );
            assert_eq!(process.cpu, expected);
        }
    }

    #[test]
    fn new_and_interrupted_io_counters_do_not_spike() {
        let mut rows = Vec::new();
        update_rate(&mut rows, "net", (1000, 2000), Some(1.0), 1, 1.0, 0);
        assert_eq!((rows[0].read, rows[0].write), (0.0, 0.0));
        update_rate(&mut rows, "net", (9000, 9000), Some(1.0), 3, 1.0, 0);
        assert_eq!((rows[0].read, rows[0].write), (0.0, 0.0));
        update_rate(&mut rows, "net", (9200, 9400), Some(2.0), 4, 1.0, 0);
        assert_eq!((rows[0].read, rows[0].write), (100.0, 200.0));
        update_rate(&mut rows, "net", (9300, 9500), Some(f64::NAN), 5, 1.0, 0);
        assert_eq!((rows[0].read, rows[0].write), (0.0, 0.0));
    }

    #[test]
    fn disk_sector_deltas_are_scaled_after_subtraction() {
        let mut rows = Vec::new();
        let count = u64::MAX / 512 + 100;
        update_rate(&mut rows, "disk", (count, count), None, 1, 512.0, 0);
        update_rate(
            &mut rows,
            "disk",
            (count + 8, count + 16),
            Some(2.0),
            2,
            512.0,
            0,
        );
        assert_eq!((rows[0].read, rows[0].write), (2048.0, 4096.0));
    }

    #[test]
    fn malformed_network_fields_are_not_zero_counters() {
        assert_eq!(
            parse_network_counters("12 0 0 0 0 0 0 0 34"),
            Some((12, 34))
        );
        assert_eq!(parse_network_counters("bad 0 0 0 0 0 0 0 34"), None);
        assert_eq!(parse_network_counters("12 0 0 0"), None);
    }

    #[test]
    fn cpu_hotplug_preserves_ids_and_invalidates_reappearing_cores() {
        let (mut cpu, mut ids, mut previous) = (Vec::new(), Vec::new(), HashMap::new());
        collect_cpu(
            "cpu 100 0 0 100\ncpu0 20 0 0 80\ncpu2 50 0 0 50\n",
            &mut cpu,
            &mut ids,
            &mut previous,
            1,
        )
        .unwrap();
        collect_cpu(
            "cpu 110 0 0 110\ncpu2 60 0 0 60\n",
            &mut cpu,
            &mut ids,
            &mut previous,
            2,
        )
        .unwrap();
        assert_eq!(ids, [2]);
        assert_eq!(cpu, [50.0, 50.0]);
        collect_cpu(
            "cpu 120 0 0 120\ncpu0 30 0 0 90\ncpu2 70 0 0 70\n",
            &mut cpu,
            &mut ids,
            &mut previous,
            3,
        )
        .unwrap();
        assert_eq!(ids, [0, 2]);
        assert_eq!(cpu, [50.0, 0.0, 50.0]);
    }

    #[test]
    fn failed_cpu_samples_clear_values_and_baselines() {
        let (mut cpu, mut ids, mut previous) = (Vec::new(), Vec::new(), HashMap::new());
        collect_cpu("cpu 100 0 0 100\n", &mut cpu, &mut ids, &mut previous, 1).unwrap();
        assert!(collect_cpu("cpu bad 0 0 100\n", &mut cpu, &mut ids, &mut previous, 2).is_err());
        assert!(cpu.is_empty() && ids.is_empty() && previous.is_empty());
        collect_cpu("cpu 300 0 0 300\n", &mut cpu, &mut ids, &mut previous, 3).unwrap();
        assert_eq!(cpu, [0.0]);
        assert!(collect_cpu("cpu 1 2\n", &mut cpu, &mut ids, &mut previous, 4).is_err());
    }

    #[test]
    fn cpu_guest_ticks_are_not_counted_twice() {
        let (mut cpu, mut ids, mut previous) = (Vec::new(), Vec::new(), HashMap::new());
        collect_cpu(
            "cpu 100 20 30 50 0 0 0 0 100 20\n",
            &mut cpu,
            &mut ids,
            &mut previous,
            1,
        )
        .unwrap();
        collect_cpu(
            "cpu 110 20 30 60 0 0 0 0 110 20\n",
            &mut cpu,
            &mut ids,
            &mut previous,
            2,
        )
        .unwrap();
        assert_eq!(cpu, [50.0]);
    }

    #[test]
    fn memory_samples_require_complete_consistent_kibibyte_values() {
        let valid = "MemTotal: 100 kB\nMemAvailable: 40 kB\nSwapTotal: 20 kB\nSwapFree: 5 kB\n";
        assert_eq!(parse_memory(valid), Some((102_400, 61_440, 20_480, 15_360)));
        for invalid in [
            valid.replace("40 kB", "invalid kB"),
            valid.replace("40 kB", "101 kB"),
            valid.replace("5 kB", "21 kB"),
            valid.replace("100 kB", "18446744073709551615 kB"),
            valid.replace("100 kB", "100 MB"),
            valid.replace("MemAvailable: 40 kB\n", ""),
        ] {
            assert_eq!(parse_memory(&invalid), None);
        }
    }

    #[test]
    fn uptime_and_load_reject_nonfinite_or_missing_metrics() {
        assert_eq!(parse_uptime("123.99 45.0\n"), Some(123));
        for invalid in ["NaN 0", "-1.0 0", "1.bad 0", "", "18446744073709551616.0 0"] {
            assert_eq!(parse_uptime(invalid), None);
        }
        assert_eq!(
            parse_load("  0.10  1.25  2.50 1/2 42\n"),
            Some(["0.10", "1.25", "2.50"])
        );
        for invalid in ["0.1 0.2", "NaN 0 0", "0 inf 0", "0 -1 0"] {
            assert_eq!(parse_load(invalid), None);
        }
    }

    #[test]
    fn independent_collection_errors_remain_visible_together() {
        let mut system = System::new();
        system.record_error("CPU counters unavailable");
        system.record_error("Memory counters unavailable");
        assert_eq!(
            system.error.as_deref(),
            Some("CPU counters unavailable; Memory counters unavailable")
        );
    }

    #[test]
    fn io_inventory_reordering_does_not_exchange_device_baselines() {
        let mut rows = Vec::new();
        update_rate(&mut rows, "first", (100, 200), None, 1, 1.0, 0);
        update_rate(&mut rows, "second", (1000, 2000), None, 1, 1.0, 1);
        update_rate(&mut rows, "second", (1100, 2400), Some(2.0), 2, 1.0, 0);
        update_rate(&mut rows, "first", (300, 800), Some(2.0), 2, 1.0, 1);
        assert_eq!(rows[0].name, "second");
        assert_eq!((rows[0].read, rows[0].write), (50.0, 200.0));
        assert_eq!(rows[1].name, "first");
        assert_eq!((rows[1].read, rows[1].write), (100.0, 300.0));
    }

    #[test]
    fn io_generations_remain_valid_across_wraparound() {
        let mut rows = Vec::new();
        update_rate(&mut rows, "net", (100, 100), None, u64::MAX, 1.0, 0);
        update_rate(&mut rows, "net", (200, 300), Some(2.0), 0, 1.0, 0);
        assert_eq!((rows[0].read, rows[0].write), (50.0, 100.0));
    }
}
