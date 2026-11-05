//! All mutations use a kernel process descriptor, never a numeric kill fallback.
use crate::system::Process;
use std::{
    fs, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub pid: u32,
    pub start: u64,
}
impl Identity {
    pub fn of(process: &Process) -> Self {
        Self {
            pid: process.pid,
            start: process.start_ticks(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Action {
    Terminate,
    Kill,
    Suspend,
    Resume,
}
impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Terminate => "TERMINATE",
            Self::Kill => "FORCE KILL",
            Self::Suspend => "SUSPEND",
            Self::Resume => "RESUME",
        }
    }
    pub fn signal(self) -> i32 {
        match self {
            Self::Terminate => libc::SIGTERM,
            Self::Kill => libc::SIGKILL,
            Self::Suspend => libc::SIGSTOP,
            Self::Resume => libc::SIGCONT,
        }
    }
    pub fn warning(self) -> &'static str {
        match self {
            Self::Terminate => "Requests exit; unsaved work may be lost.",
            Self::Kill => "Immediate exit. Unsaved work WILL be lost.",
            Self::Suspend => "Freezes the process; held locks stay held.",
            Self::Resume => "Continues a stopped process.",
        }
    }
}
pub struct Pending {
    pub identity: Identity,
    pub name: String,
    pub action: Action,
    fd: OwnedFd,
}
impl Pending {
    pub fn new(process: &Process, action: Action) -> io::Result<Self> {
        Self::open(Identity::of(process), process.name.clone(), action)
    }
    fn open(identity: Identity, name: String, action: Action) -> io::Result<Self> {
        let proc_self = fs::read_link("/proc/self")?;
        if proc_self.to_str().and_then(|s| s.parse::<u32>().ok()) != Some(std::process::id()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Process control disabled: /proc belongs to a different PID namespace",
            ));
        }
        if identity.pid <= 1 || identity.pid == std::process::id() || identity.pid > i32::MAX as u32
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "PID 0, PID 1, and ktop itself are protected",
            ));
        }
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // pidfd_open returns a CLOEXEC descriptor. OwnedFd closes on every error path.
        let fd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
        if start_ticks(identity.pid)? != identity.start {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Process identity changed; select it again",
            ));
        }
        Ok(Self {
            identity,
            name,
            action,
            fd,
        })
    }
    pub fn confirm(self) -> io::Result<()> {
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.fd.as_raw_fd(),
                self.action.signal(),
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
fn start_ticks(pid: u32) -> io::Result<u64> {
    let stat = fs::read(format!("/proc/{pid}/stat"))?;
    let end = stat
        .iter()
        .rposition(|c| *c == b')')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid process stat"))?;
    let field = stat[end + 1..]
        .split(|b| b.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
        .nth(19)
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|s| s.parse().ok());
    field.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing process start time"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::process::ExitStatusExt,
        process::{Child, Command},
        time::{Duration, Instant},
    };
    struct Guard(Child);
    impl Guard {
        fn spawn() -> Self {
            Self(Command::new("sleep").arg("30").spawn().unwrap())
        }
        fn identity(&self) -> Identity {
            Identity {
                pid: self.0.id(),
                start: start_ticks(self.0.id()).unwrap(),
            }
        }
        fn action(&self, action: Action) -> Pending {
            Pending::open(self.identity(), "owned test child".into(), action).unwrap()
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn stopped(pid: u32) -> bool {
        let stat = fs::read(format!("/proc/{pid}/stat")).unwrap();
        stat[stat.iter().rposition(|b| *b == b')').unwrap() + 2] == b'T'
    }
    fn wait_state(pid: u32, expected: bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while stopped(pid) != expected {
            assert!(Instant::now() < deadline, "child state transition");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn protected_and_stale_identities_are_never_signalled() {
        for pid in [0, 1, std::process::id(), u32::MAX] {
            assert_eq!(
                Pending::open(Identity { pid, start: 0 }, String::new(), Action::Kill)
                    .err()
                    .expect("protected process was accepted")
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        let child = Guard::spawn();
        let mut wrong = child.identity();
        wrong.start = wrong.start.wrapping_add(1);
        assert_eq!(
            Pending::open(wrong, String::new(), Action::Kill)
                .err()
                .expect("stale identity was accepted")
                .kind(),
            io::ErrorKind::NotFound
        );
        assert!(!stopped(child.0.id()));
    }
    #[test]
    fn owned_child_suspend_resume_and_terminate() {
        let mut child = Guard::spawn();
        child.action(Action::Suspend).confirm().unwrap();
        wait_state(child.0.id(), true);
        child.action(Action::Resume).confirm().unwrap();
        wait_state(child.0.id(), false);
        child.action(Action::Terminate).confirm().unwrap();
        assert_eq!(child.0.wait().unwrap().signal(), Some(libc::SIGTERM));
    }
    #[test]
    fn kill_and_exited_descriptor_do_not_target_a_replacement() {
        let mut child = Guard::spawn();
        let pending = child.action(Action::Kill);
        child.action(Action::Kill).confirm().unwrap();
        assert_eq!(child.0.wait().unwrap().signal(), Some(libc::SIGKILL));
        assert_eq!(
            pending.confirm().unwrap_err().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}
