mod control;
mod hardware;
mod input;
mod system;
mod tasks;
mod ui;

use control::{Action, Pending};
use hardware::Hardware;
use input::{Decoder, Key};
use std::{
    io::{self, IsTerminal, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use system::System;
use tasks::Tasks;
use ui::{Frame, View};

static STOP: AtomicBool = AtomicBool::new(false);
static SUSPEND: AtomicBool = AtomicBool::new(false);
static RESIZE: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
extern "C" fn suspend(_: libc::c_int) {
    SUSPEND.store(true, Ordering::Relaxed);
}
extern "C" fn resize(_: libc::c_int) {
    RESIZE.store(true, Ordering::Relaxed);
}
fn install_signals() {
    unsafe {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
            libc::signal(signal, stop as *const () as libc::sighandler_t);
        }
        libc::signal(libc::SIGTSTP, suspend as *const () as libc::sighandler_t);
        libc::signal(libc::SIGWINCH, resize as *const () as libc::sighandler_t);
    }
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
                    "ktop {} - compact Linux monitor and process manager\n\nUsage: ktop [--interval SECONDS] [--once]\n\n  --interval SECONDS   Refresh period, 0.2 to 60 (default 1)\n  --once               Print one read-only local snapshot\n  --version            Print version\n\nKeys: q quit | 1 overview | 2 hardware | 3 processes\n      c/m/p CPU/RSS/PID sort | j/k or arrows scroll/select\n      / filter by name or exact PID | g/G first/last process\n      t terminate | K force kill | s suspend | r resume (process view)\n      y confirm action | Esc cancel | Space pause | +/- refresh rate\n      Ctrl-Z suspend ktop safely; shell fg resumes it\n\nActions require confirmation and normal kernel signal permissions.\nPID 0, PID 1, and ktop itself are protected. No privilege escalation.\nCPU: one busy core is 100% per process. NO_COLOR disables styling.\nNo network requests, task database, or persistent logs.",
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
            _ => return Err("Unknown argument; use --help".into()),
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
struct Notice {
    text: String,
    expires: Instant,
    error: bool,
}
impl Notice {
    fn new(text: String, error: bool) -> Self {
        Self {
            text,
            error,
            expires: Instant::now() + Duration::from_secs(8),
        }
    }
}

fn discard_input(decoder: &mut Decoder, bytes: &[u8]) -> bool {
    let mut cancel = false;
    for byte in bytes {
        for key in decoder.feed(*byte).into_iter().flatten() {
            match key {
                Key::Quit | Key::Char(b'q') => STOP.store(true, Ordering::Relaxed),
                Key::Suspend => SUSPEND.store(true, Ordering::Relaxed),
                Key::Escape | Key::Char(b'n') => cancel = true,
                _ => {}
            }
        }
    }
    cancel
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
    let mut tasks = Tasks::default();
    tasks.refresh(&system, Sort::Cpu);
    if opts.once {
        std::thread::sleep(opts.interval);
        if STOP.load(Ordering::Relaxed) {
            return Ok(());
        }
        system.sample();
        tasks.refresh(&system, Sort::Cpu);
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
                tasks: &tasks,
            },
        );
        let mut output = String::new();
        frame.write_plain(&mut output);
        // Broken pipes are normal for snapshots piped to a reader that exits early.
        if let Err(e) = io::stdout().write_all(output.as_bytes()) {
            if e.kind() != io::ErrorKind::BrokenPipe {
                return Err(e.to_string());
            }
        }
        return Ok(());
    }
    install_signals();
    if STOP.load(Ordering::Relaxed) {
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
    let mut checked_size = Instant::now();
    let mut frame = Frame::new(size.0.saturating_sub(1), size.1);
    let mut decoder = Decoder::default();
    let mut pending: Option<Pending> = None;
    let mut arming = false;
    let mut editor: Option<String> = None;
    let mut notice: Option<Notice> = None;
    while !STOP.load(Ordering::Relaxed) {
        if SUSPEND.swap(false, Ordering::Relaxed) {
            pending = None;
            terminal.suspend().map_err(|e| e.to_string())?;
            decoder = Decoder::default();
            arming = false;
            frame.invalidate();
            RESIZE.store(true, Ordering::Relaxed);
            dirty = true;
        }
        let now = Instant::now();
        if !paused && now >= next {
            system.sample();
            tasks.refresh(&system, sort);
            if hardware_at.elapsed() >= Duration::from_secs(2) {
                hardware = Hardware::collect();
                hardware_at = now;
            }
            next = Instant::now() + interval;
            dirty = true;
        }
        if RESIZE.swap(false, Ordering::Relaxed) || checked_size.elapsed() >= Duration::from_secs(1)
        {
            let current = dimensions();
            checked_size = now;
            if current != size {
                if pending.take().is_some() {
                    notice = Some(Notice::new(
                        "Terminal resized; action cancelled".into(),
                        true,
                    ));
                }
                size = current;
                dirty = true;
            }
        }
        if notice.as_ref().is_some_and(|n| n.expires <= now) {
            notice = None;
            dirty = true;
        }
        if dirty {
            frame.resize(size.0.saturating_sub(1), size.1);
            offset = if page == 2 {
                offset.min(ui::hardware_row_count(&system, &hardware).saturating_sub(1))
            } else if page == 3 {
                tasks.offset(offset, frame.visible_processes(page))
            } else {
                offset.min(
                    tasks
                        .len()
                        .saturating_sub(frame.visible_processes(page).max(1)),
                )
            };
            frame.render(
                &system,
                &hardware,
                View {
                    sort,
                    offset,
                    page,
                    paused,
                    interval,
                    tasks: &tasks,
                },
            );
            if let Some(n) = &notice {
                frame.message(&n.text, n.error);
            }
            if let Some(query) = &editor {
                frame.message(
                    &format!("/ PID or name: {query}_   Enter apply | Esc cancel"),
                    false,
                );
            }
            if let Some(action) = &pending {
                frame.confirmation(action);
            }
            terminal.draw(&mut frame).map_err(|e| e.to_string())?;
            dirty = false;
        }
        let wait = if decoder.waiting_escape() || arming {
            50
        } else if paused {
            1000
        } else {
            next.saturating_duration_since(Instant::now())
                .as_millis()
                .clamp(1, 1000) as i32
        };
        let mut poll = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, wait) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error().to_string());
        }
        if poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            break;
        }
        let mut input = [0u8; 128];
        let count = if ready == 0 {
            0
        } else {
            unsafe { libc::read(libc::STDIN_FILENO, input.as_mut_ptr().cast(), input.len()) }
        };
        if count < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error().to_string());
        }
        if ready > 0 && count == 0 {
            break;
        }
        let escaped = if ready == 0 {
            decoder.finish_escape()
        } else {
            None
        };
        let discard = arming && ready > 0;
        if ready == 0 {
            arming = false;
        }
        // The extra iteration delivers a timed standalone Escape without inventing
        // a byte that could enter a filter or dispatch a destructive shortcut.
        'batch: for i in 0..(count as usize + usize::from(escaped.is_some())) {
            let events = if i < count as usize {
                decoder.feed(input[i])
            } else {
                [escaped, None]
            };
            for key in events.into_iter().flatten() {
                if matches!(key, Key::Quit) || (editor.is_none() && key == Key::Char(b'q')) {
                    STOP.store(true, Ordering::Relaxed);
                    break 'batch;
                }
                if key == Key::Suspend {
                    SUSPEND.store(true, Ordering::Relaxed);
                    break 'batch;
                }
                if pending.is_some() && matches!(key, Key::Escape | Key::Char(b'n')) {
                    pending = None;
                    arming = false;
                    dirty = true;
                    continue;
                }
                if discard {
                    continue;
                }
                dirty = true;
                if pending.is_some() {
                    if let Key::Char(b'y') = key {
                        let current = dimensions();
                        if current != size || current.0 < 42 || current.1 < 12 {
                            pending = None;
                            size = current;
                            notice = Some(Notice::new(
                                "Resize to at least 42x12; action cancelled".into(),
                                true,
                            ));
                            discard_input(
                                &mut decoder,
                                &input[(i + 1).min(count as usize)..count as usize],
                            );
                            break 'batch;
                        }
                        let action = pending.take().unwrap();
                        let pid = action.identity.pid;
                        let label = action.action.label();
                        notice = Some(match action.confirm() {
                            Ok(()) => {
                                Notice::new(format!("{label} signal sent to PID {pid}"), false)
                            }
                            Err(e) => Notice::new(format!("PID {pid}: {e}"), true),
                        });
                        system.sample();
                        tasks.refresh(&system, sort);
                        next = Instant::now() + interval;
                        discard_input(
                            &mut decoder,
                            &input[(i + 1).min(count as usize)..count as usize],
                        );
                        break 'batch;
                    }
                    continue;
                }
                if let Some(query) = &mut editor {
                    match key {
                        Key::Escape => editor = None,
                        Key::Enter => {
                            tasks.filter = editor.take().unwrap();
                            tasks.filtered(&system, sort);
                            offset = 0;
                        }
                        Key::Backspace => {
                            query.pop();
                        }
                        Key::Clear => query.clear(),
                        Key::Char(c) if query.len() < 64 => query.push(c as char),
                        _ => {}
                    }
                    continue;
                }
                match key {
                    Key::Char(b'1'..=b'3') => {
                        if let Key::Char(n) = key {
                            page = n - b'0';
                            offset = 0;
                        }
                    }
                    Key::Char(b'c' | b'm' | b'p') => {
                        sort = match key {
                            Key::Char(b'm') => Sort::Memory,
                            Key::Char(b'p') => Sort::Pid,
                            _ => Sort::Cpu,
                        };
                        tasks.refresh(&system, sort);
                        offset = 0;
                    }
                    Key::Char(b'j') | Key::Down => {
                        if page == 3 {
                            tasks.step(1);
                        } else {
                            offset = offset.saturating_add(1);
                        }
                    }
                    Key::Char(b'k') | Key::Up => {
                        if page == 3 {
                            tasks.step(-1);
                        } else {
                            offset = offset.saturating_sub(1);
                        }
                    }
                    Key::PageDown => {
                        if page == 3 {
                            tasks.step(frame.visible_processes(page).max(1) as isize);
                        } else {
                            offset = offset.saturating_add(10);
                        }
                    }
                    Key::PageUp => {
                        if page == 3 {
                            tasks.step(-(frame.visible_processes(page).max(1) as isize));
                        } else {
                            offset = offset.saturating_sub(10);
                        }
                    }
                    Key::Char(b'g') | Key::Home => {
                        tasks.first();
                        offset = 0;
                    }
                    Key::Char(b'G') | Key::End => {
                        tasks.last();
                        offset = if page == 2 {
                            ui::hardware_row_count(&system, &hardware).saturating_sub(1)
                        } else {
                            tasks.len().saturating_sub(1)
                        };
                    }
                    Key::Char(b'/') => {
                        page = 3;
                        editor = Some(tasks.filter.clone());
                    }
                    Key::Char(b't' | b'K' | b's' | b'r') if page == 3 => {
                        let current = dimensions();
                        if current != size || current.0 < 42 || current.1 < 12 {
                            size = current;
                            notice = Some(Notice::new(
                                "Resize to at least 42x12 before process actions".into(),
                                true,
                            ));
                            continue;
                        }
                        let action = match key {
                            Key::Char(b't') => Action::Terminate,
                            Key::Char(b'K') => Action::Kill,
                            Key::Char(b's') => Action::Suspend,
                            _ => Action::Resume,
                        };
                        if let Some(process) = tasks.process(&system) {
                            match Pending::new(process, action) {
                                Ok(action) => pending = Some(action),
                                Err(e) => notice = Some(Notice::new(e.to_string(), true)),
                            }
                        } else {
                            notice = Some(Notice::new(
                                "No selected live process; use j/k to select".into(),
                                true,
                            ));
                        }
                        // Preserve framing while discarding queued activation input.
                        // Arm only after the prompt is drawn and input has gone quiet.
                        if discard_input(
                            &mut decoder,
                            &input[(i + 1).min(count as usize)..count as usize],
                        ) {
                            pending = None;
                        }
                        arming = pending.is_some();
                        break 'batch;
                    }
                    Key::Char(b' ') => {
                        paused = !paused;
                        if !paused {
                            next = Instant::now();
                        }
                    }
                    Key::Char(b'+' | b'=') => {
                        interval = interval.div_f64(1.5).max(Duration::from_millis(200));
                        next = Instant::now() + interval;
                    }
                    Key::Char(b'-') => {
                        interval = interval.mul_f64(1.5).min(Duration::from_secs(60));
                        next = Instant::now() + interval;
                    }
                    _ => {}
                }
            }
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
        let mut original = std::mem::MaybeUninit::<libc::termios>::zeroed();
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { original.assume_init() };
        let terminal = Self {
            original,
            output: String::with_capacity(8192),
        };
        std::panic::set_hook(Box::new(move |_| {
            restore(&original);
            eprintln!("ktop: unexpected failure; terminal restored");
        }));
        terminal.activate()?;
        Ok(terminal)
    }
    fn activate(&self) -> io::Result<()> {
        let mut raw = self.original;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        io::stdout().write_all(b"\x1b[?1049h\x1b[?25l\x1b[?2004h\x1b[2J")?;
        io::stdout().flush()
    }
    fn suspend(&self) -> io::Result<()> {
        restore(&self.original);
        unsafe {
            libc::raise(libc::SIGSTOP);
        }
        self.activate()
    }
    fn draw(&mut self, frame: &mut Frame) -> io::Result<()> {
        self.output.clear();
        frame.write_ansi(&mut self.output);
        let budget = frame.output_budget();
        if self.output.capacity() > budget * 2 {
            self.output.shrink_to(budget.max(self.output.len()));
        }
        if self.output.is_empty() {
            return Ok(());
        }
        let mut stdout = io::stdout().lock();
        stdout.write_all(self.output.as_bytes())?;
        stdout.flush()
    }
}
fn restore(original: &libc::termios) {
    unsafe {
        libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, original);
    }
    let _ = io::stdout().write_all(b"\x1b[0m\x1b[?2004l\x1b[?25h\x1b[?1049l");
    let _ = io::stdout().flush();
}
impl Drop for Terminal {
    fn drop(&mut self) {
        restore(&self.original);
    }
}
