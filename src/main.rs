mod hardware;
mod system;
mod ui;

use hardware::Hardware;
use std::{
    io::{self, IsTerminal, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use system::System;
use ui::{Frame, View};

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
        let mut frame = Frame::new(100, 40);
        frame.render(
            &system,
            &hardware,
            View {
                sort: Sort::Cpu,
                offset: 0,
                page: 1,
                paused: false,
                interval: opts.interval,
            },
        );
        let mut output = String::new();
        frame.write_plain(&mut output);
        print!("{output}");
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
    let mut frame = Frame::new(size.0.saturating_sub(1), size.1);
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
                ui::hardware_row_count(&system, &hardware).saturating_sub(1)
            } else {
                system.processes.len().saturating_sub(1)
            });
            frame.resize(size.0.saturating_sub(1), size.1);
            frame.render(
                &system,
                &hardware,
                View {
                    sort,
                    offset,
                    page,
                    paused,
                    interval,
                },
            );
            terminal.draw(&frame).map_err(|e| e.to_string())?;
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
    fn draw(&mut self, frame: &Frame) -> io::Result<()> {
        self.output.clear();
        frame.write_ansi(&mut self.output);
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
