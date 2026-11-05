mod hardware;
mod system;

use hardware::Hardware;
use std::{
    fmt::Write as _,
    io::{self, IsTerminal, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use system::{Process, System};

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    Cpu,
    Memory,
    Pid,
}
impl Sort {
    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "RSS",
            Self::Pid => "PID",
        }
    }
}
struct Options {
    interval: Duration,
    once: bool,
}

fn options() -> Result<Option<Options>, String> {
    let mut args = std::env::args().skip(1);
    let mut interval = Duration::from_secs(1);
    let mut once = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "ktop {} - small, read-only Linux system monitor\n\nUsage: ktop [--interval SECONDS] [--once]\n\n  --interval SECONDS   Refresh period, 0.2 to 60 (default 1)\n  --once               Print one local snapshot after a sampling interval\n  --version            Print version\n\nKeys: q quit | 1 overview | 2 hardware | 3 processes\n      c CPU sort | m memory sort | p PID sort | j/k or arrows scroll\n      Space pause | +/- faster/slower | g top\n\nCPU: one busy core is 100% per process; memory uses resident pages.\nUnavailable hardware counters are never estimated. No network requests or logs.",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(None);
            }
            "--version" | "-V" => {
                println!("ktop {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--once" => once = true,
            "--interval" => {
                let value = args
                    .next()
                    .ok_or("--interval requires seconds")?
                    .parse::<f64>()
                    .map_err(|_| "Invalid interval")?;
                if !value.is_finite() || !(0.2..=60.0).contains(&value) {
                    return Err("Interval must be between 0.2 and 60 seconds".into());
                }
                interval = Duration::from_secs_f64(value);
            }
            _ => return Err(format!("Unknown argument: {arg}; use --help")),
        }
    }
    Ok(Some(Options { interval, once }))
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ktop: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let Some(opts) = options()? else {
        return Ok(());
    };
    if !cfg!(target_os = "linux") {
        return Err("Linux /proc and sysfs are required".into());
    }
    if !opts.once && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
        return Err("Interactive mode needs a terminal; use --once for text output".into());
    }
    let mut system = System::new();
    system.sample();
    if opts.once {
        std::thread::sleep(opts.interval);
        system.sample();
        let hardware = Hardware::collect();
        for line in render(
            &system,
            &hardware,
            100,
            60,
            Sort::Cpu,
            0,
            1,
            false,
            opts.interval,
        ) {
            println!("{}", clean(&line, 100));
        }
        return Ok(());
    }
    let mut terminal = Terminal::enter().map_err(|e| e.to_string())?;
    let mut hardware = Hardware::collect();
    let mut hardware_at = Instant::now();
    let mut interval = opts.interval;
    let mut next = Instant::now() + interval;
    let mut sort = Sort::Cpu;
    let mut offset: usize = 0;
    let mut page = 1;
    let mut paused = false;
    let mut dirty = true;
    let mut size = dimensions();
    let mut escape = 0;
    while !STOP.load(Ordering::Relaxed) {
        let now = Instant::now();
        if !paused && now >= next {
            system.sample();
            if hardware_at.elapsed() >= Duration::from_secs(2) {
                hardware = Hardware::collect();
                hardware_at = now;
            }
            next = now + interval;
            dirty = true;
        }
        let current = dimensions();
        if current != size {
            size = current;
            dirty = true;
        }
        if dirty {
            offset = offset.min(if page == 2 {
                hardware.gpus.len() + hardware.sensors.len()
            } else {
                system.processes.len().saturating_sub(1)
            });
            let rows = render(
                &system, &hardware, size.0, size.1, sort, offset, page, paused, interval,
            );
            terminal
                .draw(&rows, size.0, size.1)
                .map_err(|e| e.to_string())?;
            dirty = false;
        }
        let mut poll = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // Short bounded wait handles signals and resize even while paused.
        let ready = unsafe { libc::poll(&mut poll, 1, 100) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error().to_string());
        }
        if ready == 0 {
            continue;
        }
        if poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            break;
        }
        let mut input = [0u8; 32];
        let count =
            unsafe { libc::read(libc::STDIN_FILENO, input.as_mut_ptr().cast(), input.len()) };
        if count <= 0 {
            break;
        }
        for key in &input[..count as usize] {
            if escape == 1 {
                escape = if *key == b'[' { 2 } else { 0 };
                continue;
            }
            if escape == 2 {
                match key {
                    b'A' => offset = offset.saturating_sub(1),
                    b'B' => offset = offset.saturating_add(1),
                    _ => {}
                }
                escape = 0;
                dirty = true;
                continue;
            }
            match key {
                b'q' | b'Q' | 3 | 4 => {
                    STOP.store(true, Ordering::Relaxed);
                    break;
                }
                27 => escape = 1,
                b'1' | b'2' | b'3' => {
                    page = key - b'0';
                    offset = 0;
                }
                b'c' => {
                    sort = Sort::Cpu;
                    offset = 0;
                }
                b'm' => {
                    sort = Sort::Memory;
                    offset = 0;
                }
                b'p' => {
                    sort = Sort::Pid;
                    offset = 0;
                }
                b'j' => offset = offset.saturating_add(1),
                b'k' => offset = offset.saturating_sub(1),
                b'g' => offset = 0,
                b' ' => {
                    paused = !paused;
                    if !paused {
                        next = Instant::now();
                    }
                }
                b'+' | b'=' => {
                    interval = interval.div_f64(1.5).max(Duration::from_millis(200));
                    next = Instant::now() + interval;
                }
                b'-' => {
                    interval = interval.mul_f64(1.5).min(Duration::from_secs(60));
                    next = Instant::now() + interval;
                }
                _ => {}
            }
            dirty = true;
        }
    }
    Ok(())
}

fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut n = value as f64;
    let mut unit = 0;
    while n >= 1024.0 && unit < UNITS.len() - 1 {
        n /= 1024.0;
        unit += 1;
    }
    format!("{n:.1} {}", UNITS[unit])
}
fn bar(percent: f64, width: usize) -> String {
    let used = ((percent.clamp(0.0, 100.0) / 100.0 * width as f64).round() as usize).min(width);
    format!("[{}{}]", "|".repeat(used), " ".repeat(width - used))
}
fn percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 * 100.0 / total as f64
    }
}

#[allow(clippy::too_many_arguments)]
fn render(
    s: &System,
    h: &Hardware,
    width: usize,
    height: usize,
    sort: Sort,
    offset: usize,
    page: u8,
    paused: bool,
    interval: Duration,
) -> Vec<String> {
    let mut rows = Vec::with_capacity(height);
    if width < 42 || height < 12 {
        rows.push("ktop - terminal too small".into());
        rows.push("Resize to at least 42 columns x 12 rows.".into());
        rows.push("q: quit".into());
        return rows;
    }
    rows.push(format!(
        "KTOP {} | {} | {:.1}s | {}",
        env!("CARGO_PKG_VERSION"),
        match page {
            2 => "HARDWARE",
            3 => "PROCESSES",
            _ => "OVERVIEW",
        },
        interval.as_secs_f64(),
        if paused { "PAUSED" } else { "LIVE" }
    ));
    rows.push(format!(
        "Up {}d {:02}:{:02}:{:02} | Load {} | {} processes",
        s.uptime / 86400,
        s.uptime / 3600 % 24,
        s.uptime / 60 % 60,
        s.uptime % 60,
        s.load,
        s.processes.len()
    ));
    if let Some(error) = &s.error {
        rows.push(format!("WARNING: {error}"));
    }
    if page == 2 {
        rows.push("GPU / HWMON SENSORS (j/k scroll)".into());
        let items: Vec<&str> = h
            .gpus
            .iter()
            .chain(h.sensors.iter())
            .map(String::as_str)
            .collect();
        if items.is_empty() {
            rows.push("No readable hardware counters; kernel/driver support required.".into());
        }
        for line in items
            .into_iter()
            .skip(offset)
            .take(height.saturating_sub(rows.len() + 1))
        {
            rows.push(line.into());
        }
    } else {
        if page == 1 {
            let cpu = s.cpu.first().copied().unwrap_or(0.0);
            rows.push(format!(
                "CPU {} {:5.1}% | {} cores{}",
                bar(cpu, width.saturating_sub(42).clamp(8, 32)),
                cpu,
                s.cpu.len().saturating_sub(1),
                s.mhz
                    .map(|v| format!(" | CPU0 {v} MHz"))
                    .unwrap_or_default()
            ));
            if height >= 24 {
                let per_row = (width / 14).max(1);
                for (chunk, cores) in s
                    .cpu
                    .get(1..)
                    .unwrap_or(&[])
                    .chunks(per_row)
                    .take(2)
                    .enumerate()
                {
                    let mut row = String::new();
                    for (i, usage) in cores.iter().enumerate() {
                        let _ = write!(row, " c{:<3} {:5.1}% ", chunk * per_row + i, usage);
                    }
                    rows.push(row);
                }
            }
            rows.push(format!(
                "RAM {} / {} ({:.1}%) | Swap {} / {}",
                bytes(s.memory_used),
                bytes(s.memory_total),
                percent(s.memory_used, s.memory_total),
                bytes(s.swap_used),
                bytes(s.swap_total)
            ));
            if let Some((used, total)) = s.disk_space {
                rows.push(format!(
                    "Root {} / {} ({:.1}%)",
                    bytes(used),
                    bytes(total),
                    percent(used, total)
                ));
            }
            if height >= 20 {
                for n in s.network.iter().take(2) {
                    rows.push(format!(
                        "NET {:12} RX {}/s  TX {}/s",
                        n.name,
                        bytes(n.read as u64),
                        bytes(n.write as u64)
                    ));
                }
                for d in s.disks.iter().take(2) {
                    rows.push(format!(
                        "I/O {:12} R  {}/s  W  {}/s",
                        d.name,
                        bytes(d.read as u64),
                        bytes(d.write as u64)
                    ));
                }
                if let Some(gpu) = h.gpus.first() {
                    rows.push(format!("GPU {gpu}"));
                }
                if h.gpus.is_empty() {
                    rows.push("GPU counters unavailable (no readable DRM device)".into());
                }
                for sensor in h.sensors.iter().take(if height >= 32 { 4 } else { 1 }) {
                    rows.push(sensor.clone());
                }
            }
            rows.push(String::new());
        }
        rows.push(format!(
            "PROCESSES | sort {} | rows {}+ | CPU: 100% = one core",
            sort.label(),
            offset + 1
        ));
        rows.push(format!(
            "{:>7} S {:>7} {:>10} {:>5}  NAME",
            "PID", "CPU%", "RSS", "THR"
        ));
        let mut processes: Vec<&Process> = s.processes.values().collect();
        processes.sort_unstable_by(|a, b| match sort {
            Sort::Cpu => b
                .cpu
                .total_cmp(&a.cpu)
                .then_with(|| b.rss.cmp(&a.rss))
                .then_with(|| a.pid.cmp(&b.pid)),
            Sort::Memory => b.rss.cmp(&a.rss).then_with(|| a.pid.cmp(&b.pid)),
            Sort::Pid => a.pid.cmp(&b.pid),
        });
        let capacity = height.saturating_sub(rows.len() + 1);
        for p in processes.into_iter().skip(offset).take(capacity) {
            rows.push(format!(
                "{:>7} {} {:>7.1} {:>10} {:>5}  {}",
                p.pid,
                p.state,
                p.cpu,
                bytes(p.rss),
                p.threads,
                p.name
            ));
        }
    }
    rows.truncate(height.saturating_sub(1));
    while rows.len() < height - 1 {
        rows.push(String::new());
    }
    rows.push("q quit | 1/2/3 pages | c/m/p sort | j/k scroll | Space pause | +/- rate".into());
    rows
}

// Names from proc/sysfs are untrusted terminal content. Printable ASCII only:
// strips escape/control sequences and gives deterministic column clipping.
fn clean(line: &str, width: usize) -> String {
    line.chars()
        .take(width)
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect()
}
fn dimensions() -> (usize, usize) {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0 {
        let size = unsafe { size.assume_init() };
        if size.ws_col > 0 && size.ws_row > 0 {
            return (
                (size.ws_col as usize).min(512),
                (size.ws_row as usize).min(200),
            );
        }
    }
    (80, 24)
}
struct Terminal {
    original: libc::termios,
    output: String,
}
impl Terminal {
    fn enter() -> io::Result<Self> {
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let terminal = Self {
            original,
            output: String::with_capacity(8192),
        };
        std::panic::set_hook(Box::new(move |_| {
            restore(&original);
            eprintln!("ktop: unexpected failure; terminal restored");
        }));
        unsafe {
            libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
            libc::signal(libc::SIGHUP, stop as *const () as libc::sighandler_t);
        }
        io::stdout().write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J")?;
        io::stdout().flush()?;
        Ok(terminal)
    }
    fn draw(&mut self, rows: &[String], width: usize, height: usize) -> io::Result<()> {
        self.output.clear();
        for (i, row) in rows.iter().take(height).enumerate() {
            let _ = write!(self.output, "\x1b[{};1H\x1b[2K", i + 1);
            if i == 0 || i == height - 1 {
                self.output.push_str("\x1b[1;36m");
            }
            // Reserve the final column to avoid automatic terminal wrapping.
            self.output.push_str(&clean(row, width.saturating_sub(1)));
            self.output.push_str("\x1b[0m");
        }
        self.output.push_str("\x1b[J");
        let mut stdout = io::stdout().lock();
        stdout.write_all(self.output.as_bytes())?;
        stdout.flush()
    }
}
fn restore(original: &libc::termios) {
    unsafe {
        libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, original);
    }
    let _ = io::stdout().write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
    let _ = io::stdout().flush();
}
impl Drop for Terminal {
    fn drop(&mut self) {
        restore(&self.original);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_names_cannot_inject_escape_sequences() {
        assert_eq!(clean("bad\x1b[2J\n\t\u{202e}name", 80), "bad?[2J???name");
        assert_eq!(clean("abcdef", 3), "abc");
    }
}
