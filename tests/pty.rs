//! Real terminal journeys. Only test-owned children may receive a signal.
#![cfg(target_os = "linux")]
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::ExitStatusExt,
    },
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct OwnedChild(Child);
impl OwnedChild {
    fn sleep() -> Self {
        Self(Command::new("sleep").arg("30").spawn().unwrap())
    }
    fn pid(&self) -> u32 {
        self.0.id()
    }
    fn state(&self) -> u8 {
        let stat = fs::read(format!("/proc/{}/stat", self.pid())).unwrap();
        stat[stat.iter().rposition(|b| *b == b')').unwrap() + 2]
    }
    fn wait_state(&self, stopped: bool) {
        let end = Instant::now() + Duration::from_secs(3);
        while (self.state() == b'T') != stopped {
            assert!(Instant::now() < end, "owned child did not change state");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct App {
    child: Child,
    master: File,
    slave: File,
    original: libc::termios,
    received: Vec<u8>,
}
impl App {
    fn new() -> Self {
        let (mut master, mut slave) = (-1, -1);
        let size = libc::winsize {
            ws_row: 30,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    &size,
                )
            },
            0
        );
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        let mut original = std::mem::MaybeUninit::<libc::termios>::zeroed();
        assert_eq!(
            unsafe { libc::tcgetattr(slave.as_raw_fd(), original.as_mut_ptr()) },
            0
        );
        let child = Command::new(env!("CARGO_BIN_EXE_ktop"))
            .arg("--interval")
            .arg("0.2")
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()))
            .spawn()
            .unwrap();
        let mut app = Self {
            child,
            master,
            slave,
            original: unsafe { original.assume_init() },
            received: Vec::new(),
        };
        app.until("ktop");
        app
    }
    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }
    fn read_for(&mut self, duration: Duration) {
        let end = Instant::now() + duration;
        while Instant::now() < end {
            let mut p = libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let millis = end
                .saturating_duration_since(Instant::now())
                .as_millis()
                .clamp(1, 50) as i32;
            if unsafe { libc::poll(&mut p, 1, millis) } <= 0 {
                continue;
            }
            let mut bytes = [0; 16384];
            match self.master.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => self.received.extend_from_slice(&bytes[..n]),
                Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                Err(e) => panic!("PTY read: {e}"),
            }
        }
    }
    fn until(&mut self, text: &str) {
        let end = Instant::now() + Duration::from_secs(5);
        while !self
            .received
            .windows(text.len())
            .any(|w| w == text.as_bytes())
        {
            assert!(Instant::now() < end, "expected terminal state: {text}");
            self.read_for(Duration::from_millis(20));
        }
    }
    fn select(&mut self, pid: u32) {
        self.received.clear();
        self.send(format!("3/{pid}\r").as_bytes());
        self.until("FILTER:");
        self.read_for(Duration::from_millis(100));
    }
    fn request(&mut self, key: u8, label: &str, pid: u32) {
        self.received.clear();
        self.send(&[key]);
        self.until(&format!("{label} PID {pid}"));
        self.read_for(Duration::from_millis(100));
    }
    fn resize(&mut self, width: u16, height: u16) {
        let size = libc::winsize {
            ws_row: height,
            ws_col: width,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe { libc::ioctl(self.slave.as_raw_fd(), libc::TIOCSWINSZ, &size) },
            0
        );
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGWINCH);
        }
        self.read_for(Duration::from_millis(150));
    }
    fn finish(&mut self) {
        self.send(b"q");
        let end = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < end, "ktop did not quit");
            self.read_for(Duration::from_millis(20));
        }
        let mut current = std::mem::MaybeUninit::<libc::termios>::zeroed();
        assert_eq!(
            unsafe { libc::tcgetattr(self.slave.as_raw_fd(), current.as_mut_ptr()) },
            0
        );
        let current = unsafe { current.assume_init() };
        assert_eq!(current.c_lflag, self.original.c_lflag);
        assert_eq!(current.c_iflag, self.original.c_iflag);
        assert_eq!(current.c_oflag, self.original.c_oflag);
        assert_eq!(current.c_cc, self.original.c_cc);
    }
}
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn selected_owned_process_actions_require_visible_confirmation() {
    let mut target = OwnedChild::sleep();
    let mut app = App::new();
    app.select(target.pid());
    app.request(b's', "SUSPEND", target.pid());
    assert_ne!(target.state(), b'T');
    app.send(b"y");
    target.wait_state(true);
    app.read_for(Duration::from_millis(100));
    app.request(b'r', "RESUME", target.pid());
    app.send(b"y");
    target.wait_state(false);
    app.read_for(Duration::from_millis(100));
    app.request(b't', "TERMINATE", target.pid());
    app.send(b"y");
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = target.0.try_wait().unwrap() {
            assert_eq!(status.signal(), Some(libc::SIGTERM));
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    app.finish();
}
#[test]
fn queued_and_fragmented_pastes_cannot_confirm_or_hide_target() {
    let mut target = OwnedChild::sleep();
    let mut app = App::new();
    app.select(target.pid());
    app.received.clear();
    app.send(b"t\x1b[200~");
    app.read_for(Duration::from_millis(100));
    app.send(b"y\x1b[201~");
    app.read_for(Duration::from_millis(150));
    assert!(target.0.try_wait().unwrap().is_none());
    app.send(b"y");
    app.read_for(Duration::from_millis(100));
    assert!(target.0.try_wait().unwrap().is_none());
    app.send(b"n");
    app.read_for(Duration::from_millis(100));
    app.request(b'K', "FORCE KILL", target.pid());
    app.send(b"\x1b[200~text\x1b[201~y\x1b[201~");
    app.read_for(Duration::from_millis(150));
    app.send(b"y");
    app.read_for(Duration::from_millis(100));
    assert!(target.0.try_wait().unwrap().is_none());
    app.received.clear();
    app.send(b"ty");
    app.until(&format!("TERMINATE PID {}", target.pid()));
    app.read_for(Duration::from_millis(150));
    assert!(target.0.try_wait().unwrap().is_none());
    app.send(b"n");
    app.read_for(Duration::from_millis(100));
    app.received.clear();
    app.send(b"K");
    app.until(&format!("FORCE KILL PID {}", target.pid()));
    app.send(b"n");
    app.read_for(Duration::from_millis(150));
    app.send(b"y");
    app.read_for(Duration::from_millis(100));
    assert!(target.0.try_wait().unwrap().is_none());
    app.request(b'K', "FORCE KILL", target.pid());
    app.resize(5, 6);
    app.send(b"y");
    app.read_for(Duration::from_millis(100));
    assert!(target.0.try_wait().unwrap().is_none());
    app.resize(100, 30);
    app.request(b'K', "FORCE KILL", target.pid());
    app.send(b"y");
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = target.0.try_wait().unwrap() {
            assert_eq!(status.signal(), Some(libc::SIGKILL));
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    app.finish();
}
#[test]
fn paste_interruption_suspend_resume_and_snapshot_termination() {
    let mut app = App::new();
    app.send(b"\x1b[200~");
    unsafe {
        libc::kill(app.child.id() as i32, libc::SIGTSTP);
    }
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let stat = fs::read(format!("/proc/{}/stat", app.child.id())).unwrap();
        if stat[stat.iter().rposition(|b| *b == b')').unwrap() + 2] == b'T' {
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    app.send(b"\x1b[201~");
    unsafe {
        libc::kill(app.child.id() as i32, libc::SIGCONT);
    }
    app.read_for(Duration::from_millis(150));
    app.send(b"\x1bq");
    let end = Instant::now() + Duration::from_secs(3);
    while app.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < end);
        app.read_for(Duration::from_millis(20));
    }
    let mut once = Command::new(env!("CARGO_BIN_EXE_ktop"))
        .args(["--once", "--interval", "60"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    unsafe {
        libc::kill(once.id() as i32, libc::SIGTERM);
    }
    let end = Instant::now() + Duration::from_secs(2);
    while once.try_wait().unwrap().is_none() {
        if Instant::now() >= end {
            let _ = once.kill();
            let _ = once.wait();
            panic!("snapshot ignored termination");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn safe_cancel_and_quit_survive_the_activation_read_tail() {
    let mut target = OwnedChild::sleep();
    let mut app = App::new();
    app.select(target.pid());
    app.send(b"tn");
    app.read_for(Duration::from_millis(150));
    app.send(b"y");
    app.read_for(Duration::from_millis(100));
    assert!(target.0.try_wait().unwrap().is_none());
    app.send(b"t\x03");
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "queued Ctrl-C did not quit");
        app.read_for(Duration::from_millis(20));
    }
    assert!(target.0.try_wait().unwrap().is_none());
    app.finish();
}
