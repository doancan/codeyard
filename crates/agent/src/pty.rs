//! Pseudo-terminal: `openpty` + shell. No dependencies (libc), to keep the agent small.

#[cfg(unix)]
use std::fs::File;
use std::io::{self, Write};
#[cfg(unix)]
use std::process::Child;

#[cfg(windows)]
use crate::Locked;

/// What to start in a new terminal.
#[derive(Debug, Clone, Default)]
pub struct Spawn {
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<String>,
    /// Program (`None`: the user's shell).
    pub shell: Option<String>,
    /// Arguments (`None`: a login shell, `-l`).
    pub args: Option<Vec<String>>,
    /// Added to the environment.
    pub env: Vec<(String, String)>,
    /// Set up shell integration (see `shell_integration`).
    pub integration: bool,
}

/// What is running in a terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub busy: Option<bool>,
    pub name: Option<String>,
    pub cwd: Option<String>,
}

/// Longest program name or folder reported.
const MAX_STATUS_TEXT: usize = 1024;

fn clip(s: String) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_STATUS_TEXT).collect()
}

/// The shells this machine offers: `/etc/shells` (existing, executable, no `nologin`), at most 64.
#[cfg(unix)]
pub fn shells() -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let text = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.contains("nologin") || out.iter().any(|s| s == line) {
            continue;
        }
        if std::fs::metadata(line).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
            out.push(line.to_string());
        }
        if out.len() >= 64 {
            break;
        }
    }
    out
}

#[cfg(windows)]
pub fn shells() -> Vec<String> {
    let mut out = Vec::new();
    for name in ["pwsh.exe", "powershell.exe", "cmd.exe", "wsl.exe"] {
        if std::process::Command::new("where").arg(name).output().is_ok_and(|o| o.status.success()) {
            out.push(name.to_string());
        }
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        let bash = std::path::Path::new(&pf).join("Git").join("bin").join("bash.exe");
        if bash.is_file() {
            out.push(bash.to_string_lossy().into_owned());
        }
    }
    out
}

#[cfg(unix)]
pub struct Pty {
    master: File,
    child: Child,
    /// Keyboard input is written on a separate thread: a full terminal (e.g. a big paste into a stopped shell) must not block the dispatch
    /// thread and the `ptys` lock.
    input: std::sync::mpsc::SyncSender<Vec<u8>>,
}

#[cfg(windows)]
pub struct Pty {
    master: std::sync::Arc<std::sync::Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>>,
    /// Keyboard input is written on a separate thread (see unix `Pty::input`).
    input: std::sync::mpsc::SyncSender<Vec<u8>>,
    child: std::sync::Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
}

#[cfg(windows)]
mod windows {
    use super::*;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    fn size(cols: u16, rows: u16) -> PtySize {
        PtySize { cols: cols.max(1), rows: rows.max(1), pixel_width: 0, pixel_height: 0 }
    }

    pub fn spawn_with(spec: Spawn) -> io::Result<(Pty, Box<dyn io::Read + Send>)> {
        let Spawn { cols, rows, cwd, shell, args, env, .. } = spec;
        let pair = native_pty_system().openpty(size(cols, rows)).map_err(io::Error::other)?;
        let shell = shell.filter(|s| !s.is_empty()).map(std::ffi::OsString::from).or_else(|| std::env::var_os("COMSPEC")).unwrap_or_else(|| "cmd.exe".into());
        let mut cmd = CommandBuilder::new(shell);
        for a in args.unwrap_or_default() {
            cmd.arg(a);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let cwd = cwd.as_deref();
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "codeyard");
        if let Some(dir) = cwd.filter(|d| std::path::Path::new(d).is_dir()) {
            cmd.cwd(dir);
        }
        // Open the streams before spawning, so a setup failure cannot orphan a shell.
        let reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let mut writer = pair.master.take_writer().map_err(io::Error::other)?;
        let child = std::sync::Arc::new(std::sync::Mutex::new(pair.slave.spawn_command(cmd).map_err(io::Error::other)?));
        drop(pair.slave);
        let master = std::sync::Arc::new(std::sync::Mutex::new(Some(pair.master)));
        let (monitor_child, monitor_master) = (child.clone(), master.clone());
        std::thread::spawn(move || {
            loop {
                if !matches!(monitor_child.locked().try_wait(), Ok(None)) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            // ConPTY keeps its output pipe open until the pseudo-console closes,
            // even after the shell exits. Close it while the output thread drains.
            let master = monitor_master.locked().take();
            drop(master);
        });
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(512);
        std::thread::spawn(move || {
            while let Ok(chunk) = rx.recv() {
                if writer.write_all(&chunk).and_then(|_| writer.flush()).is_err() {
                    break;
                }
            }
        });
        Ok((Pty { master, input: tx, child }, reader))
    }

    pub fn resize(pty: &Pty, cols: u16, rows: u16) -> io::Result<()> {
        let master = pty.master.locked();
        master.as_ref().ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?.resize(size(cols, rows)).map_err(io::Error::other)
    }
}

#[cfg(windows)]
pub use windows::{resize, spawn_with};

#[cfg(unix)]
mod imp {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use super::*;

    fn winsize(cols: u16, rows: u16) -> libc::winsize {
        libc::winsize { ws_row: rows.max(1), ws_col: cols.max(1), ws_xpixel: 0, ws_ypixel: 0 }
    }

    /// Start the shell; the returned `File` is for reading output (a copy of the master).
    pub fn spawn_with(spec: Spawn) -> io::Result<(Pty, File)> {
        let Spawn { cols, rows, cwd, shell, args, mut env, integration } = spec;
        let (cwd, shell) = (cwd.as_deref(), shell.as_deref());
        let (mut master, mut slave) = (0, 0);
        let mut ws = winsize(cols, rows);
        // SAFETY: `master`, `slave` and `ws` are valid for the call. Null name and termios mean "don't return a name" and "default
        // termios". `&mut ws` coerces to whichever pointer mutability this platform's libc declares, so no cast is needed.
        let rc = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openpty succeeded, so both are open fds that nothing else owns; ownership moves to the OwnedFds.
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        // The shell must not inherit the master (other children spawned meanwhile would keep the pty open). The slave ends are
        // dup2'd onto 0-2 in the child, which clears the flag on the copies.
        for fd in [&master, &slave] {
            // SAFETY: the fd is open (owned by `fd`); F_SETFD only changes the descriptor flags.
            unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(fd), libc::F_SETFD, libc::FD_CLOEXEC) };
        }

        let shell = shell.map(str::to_string).or_else(|| std::env::var("SHELL").ok()).unwrap_or_else(|| "/bin/sh".into());
        // A login shell with integration: the shell's own way to load the hook scripts.
        let args = match (args, integration) {
            (None, true) => crate::shell_integration::setup(&shell, &mut env),
            (args, _) => args,
        };
        let mut cmd = Command::new(&shell);
        match args {
            Some(args) => cmd.args(args),
            None => cmd.arg("-l"),
        };
        cmd.envs(env);
        cmd.env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "codeyard")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        if let Some(dir) = cwd.filter(|d| std::path::Path::new(d).is_dir()) {
            cmd.current_dir(dir);
        }
        // SAFETY: the closure runs between fork and exec, so it may only make async-signal-safe calls: setsid and ioctl are, and
        // `last_os_error` reads errno without allocating. It captures nothing.
        unsafe {
            cmd.pre_exec(|| {
                // New session + this PTY as controlling terminal: so Ctrl+C and job control work.
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        let master = File::from(master);
        let reader = master.try_clone()?;
        let mut writer = master.try_clone()?;
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(512);
        std::thread::spawn(move || {
            while let Ok(chunk) = rx.recv() {
                if writer.write_all(&chunk).is_err() {
                    break;
                }
            }
        });
        Ok((Pty { master, child, input: tx }, reader))
    }

    pub fn resize(pty: &Pty, cols: u16, rows: u16) -> io::Result<()> {
        let ws = winsize(cols, rows);
        let fd = std::os::fd::AsRawFd::as_raw_fd(&pty.master);
        // SAFETY: `fd` is the open master, and `&ws` points to a live winsize, which is what TIOCSWINSZ reads.
        if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(unix)]
pub use imp::{resize, spawn_with};

/// A login shell (the agent's own tests and the old request).
#[cfg(test)]
pub fn spawn(cols: u16, rows: u16, cwd: Option<&str>, shell: Option<&str>) -> io::Result<(Pty, impl io::Read)> {
    spawn_with(Spawn { cols, rows, cwd: cwd.map(str::to_string), shell: shell.map(str::to_string), ..Spawn::default() })
}

impl Pty {
    /// Enqueue (doesn't block); `WouldBlock` if the queue (512 chunks) is full.
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        use std::sync::mpsc::TrySendError;
        self.input.try_send(data.to_vec()).map_err(|e| match e {
            TrySendError::Full(_) => io::Error::new(io::ErrorKind::WouldBlock, "the terminal is not reading its input"),
            TrySendError::Disconnected(_) => io::ErrorKind::BrokenPipe.into(),
        })
    }

    /// The foreground program and the shell's folder.
    #[cfg(unix)]
    pub fn status(&self) -> Status {
        let shell = self.child.id() as libc::pid_t;
        let fd = std::os::fd::AsRawFd::as_raw_fd(&self.master);
        // SAFETY: `fd` is the open master of this terminal; tcgetpgrp only reads its foreground process group.
        let fg = unsafe { libc::tcgetpgrp(fd) };
        let busy = (fg > 0).then_some(fg != shell);
        let name = busy.filter(|b| *b).and_then(|_| process_name(fg)).map(clip);
        Status { busy, name, cwd: process_cwd(shell).map(clip) }
    }

    #[cfg(windows)]
    pub fn status(&self) -> Status {
        Status::default()
    }

    pub fn kill(&mut self) {
        #[cfg(unix)]
        let _ = self.child.kill();
        #[cfg(windows)]
        let _ = self.child.locked().kill();
    }

    /// Exit code (called after the read end closes).
    pub fn wait(&mut self) -> Option<i32> {
        #[cfg(unix)]
        {
            self.child.wait().ok().and_then(|s| s.code())
        }
        #[cfg(windows)]
        {
            self.child.locked().wait().ok().map(|s| s.exit_code() as i32)
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Without waiting, every closed session would leave a zombie shell.
        self.kill();
        #[cfg(unix)]
        let _ = self.child.wait();
        #[cfg(windows)]
        let _ = self.child.locked().wait();
    }
}

#[cfg(target_os = "linux")]
fn process_name(pid: libc::pid_t) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(target_os = "linux")]
fn process_cwd(pid: libc::pid_t) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok().map(|p| p.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn process_name(pid: libc::pid_t) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a writable buffer of the given size; proc_name writes at most that many bytes and returns the length.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

#[cfg(target_os = "macos")]
fn process_cwd(pid: libc::pid_t) -> Option<String> {
    // SAFETY: an all-zero proc_vnodepathinfo is a valid value of this plain C struct.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: `info` is writable and `size` is its exact size, as PROC_PIDVNODEPATHINFO expects.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size) };
    if n != size {
        return None;
    }
    let path = &info.pvi_cdir.vip_path;
    // SAFETY: the kernel fills `vip_path` with a NUL-terminated C string inside the array.
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), std::mem::size_of_val(path)) };
    let end = bytes.iter().position(|b| *b == 0)?;
    Some(String::from_utf8_lossy(&bytes[..end]).into_owned()).filter(|s| !s.is_empty())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn process_name(_: libc::pid_t) -> Option<String> {
    None
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn process_cwd(_: libc::pid_t) -> Option<String> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Read;

    fn wait_for(p: &Pty, what: &str, want: impl Fn(&Status) -> bool) -> Status {
        let start = std::time::Instant::now();
        loop {
            let s = p.status();
            if want(&s) {
                return s;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "{what}: {s:?}");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn the_foreground_program_and_the_folder_are_reported() {
        let dir = std::env::temp_dir().join(format!("codeyard-pty-status-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap().to_string_lossy().into_owned();
        let spec = Spawn {
            cols: 80,
            rows: 24,
            cwd: Some(dir.clone()),
            shell: Some("/bin/sh".into()),
            args: Some(Vec::new()),
            env: vec![("CODEYARD_T".into(), "1".into())],
            integration: false,
        };
        let (mut p, mut reader) = spawn_with(spec).unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while reader.read(&mut buf).is_ok_and(|n| n > 0) {}
        });
        let idle = wait_for(&p, "idle shell", |s| s.busy == Some(false));
        assert_eq!(idle.cwd.as_deref(), Some(dir.as_str()), "the shell's folder");
        p.write(b"sleep 30\n").unwrap();
        let busy = wait_for(&p, "a program in the foreground", |s| s.busy == Some(true));
        assert_eq!(busy.name.as_deref(), Some("sleep"));
        p.kill();
    }

    #[test]
    fn listed_shells_exist() {
        for s in shells() {
            assert!(std::path::Path::new(&s).is_file(), "{s}");
        }
    }
}
