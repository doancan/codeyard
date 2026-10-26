//! Running commands (git) and piped long-lived processes (language servers).
//! If no program path is given, it is looked up through the user's login shell: when the app
//! is launched from the Dock or an ssh command, PATH does not include
//! directories like ~/.cargo/bin.

use std::io::{self, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Cap on one command's output (a huge `git diff`).
const MAX_OUTPUT: u64 = 32 * 1024 * 1024;

/// If `cwd` is given it must exist: otherwise silently running the command in the agent's own directory (`$HOME` over ssh) would, for example, run `git`
/// against a different repository.
pub fn command(program: &str, args: &[String], cwd: Option<&str>) -> std::io::Result<Command> {
    #[cfg(unix)]
    let mut cmd = if program.contains('/') {
        Command::new(program)
    } else {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
        let mut c = Command::new(shell);
        // $0 = program, $@ = arguments: avoids quoting.
        c.args(["-lc", "exec \"$0\" \"$@\"", program]);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        // Command searches .exe automatically, but language servers installed by npm
        // commonly use .cmd launchers. Let Rust quote batch-file arguments safely.
        let resolved = resolve_program(program);
        let mut c = Command::new(resolved);
        c.creation_flags(0x08000000); // CREATE_NO_WINDOW: no console flash for Git/LSP.
        c
    };
    cmd.args(args);
    if let Some(dir) = cwd {
        if !std::path::Path::new(dir).is_dir() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("the working directory {dir} does not exist")));
        }
        cmd.current_dir(dir);
    }
    // Keep git's own pager/colors/prompts from kicking in.
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GIT_PAGER", "cat").env("LC_ALL", "C");
    Ok(cmd)
}

#[cfg(windows)]
fn resolve_program(program: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(program);
    if path.extension().is_none() {
        let dirs: Vec<_> = if path.components().count() > 1 {
            vec![std::path::PathBuf::new()]
        } else {
            std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
        };
        for dir in dirs {
            for extension in ["exe", "com", "cmd", "bat"] {
                let candidate = dir.join(path).with_extension(extension);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    path.to_path_buf()
}

pub struct Output {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Longest time for a single command: killed if exceeded (a hung network operation, git asking for credentials).
/// Only a safety net; normally we return when the command finishes.
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(600);

/// Grace period for collecting data left in the pipes after the command finishes. Background processes
/// the command started (a daemon in the login shell's profile, git's
/// background maintenance) may keep the pipes open: if we waited for EOF, `exec` would wait as long as that process lives.
const DRAIN_GRACE: Duration = Duration::from_millis(100);

/// Run and wait for it to finish. If `stdin` is given, it is written and closed.
pub fn exec(program: &str, args: &[String], cwd: Option<&str>, stdin: Option<Vec<u8>>) -> io::Result<Output> {
    exec_with_timeout(program, args, cwd, stdin, EXEC_TIMEOUT)
}

/// A stdio handle requested with `Stdio::piped()`; `None` is not expected, but is an error rather than a panic.
fn piped<T>(handle: Option<T>) -> io::Result<T> {
    handle.ok_or_else(|| io::Error::other("child stdio was not piped"))
}

pub fn exec_with_timeout(program: &str, args: &[String], cwd: Option<&str>, stdin: Option<Vec<u8>>, timeout: Duration) -> io::Result<Output> {
    let mut child = command(program, args, cwd)?
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(data) = stdin {
        let mut w = piped(child.stdin.take())?;
        std::thread::spawn(move || {
            let _ = w.write_all(&data);
        });
    }
    let stdout = piped(child.stdout.take())?;
    let stderr = piped(child.stderr.take())?;
    let (stdout, stderr, status) = collect(&mut child, stdout, stderr, timeout)?;
    Ok(Output { code: exit_code(status), stdout, stderr })
}

/// Read both pipes together (so one filling up doesn't stall the child); when the child exits, drain the remaining data
/// and return, even if the pipes stay open in another process.
#[cfg(unix)]
fn collect(
    child: &mut Child,
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
    timeout: Duration,
) -> io::Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus)> {
    use std::os::fd::AsRawFd;
    let fds = [stdout.as_raw_fd(), stderr.as_raw_fd()];
    for fd in fds {
        // SAFETY: `fd` belongs to `stdout`/`stderr`, which outlive this function body, so it is open; fcntl on an open fd only reads
        // and rewrites the status flags (no memory is passed).
        unsafe { libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) };
    }
    let mut bufs = [Vec::new(), Vec::new()];
    let mut open = [true, true];
    let started = Instant::now();
    let mut exited: Option<(std::process::ExitStatus, Instant)> = None;
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        // Finished and the pipes closed (normal path), or the grace period expired after finishing.
        if let Some((status, at)) = exited
            && (!open[0] && !open[1] || at.elapsed() >= DRAIN_GRACE)
        {
            return Ok((std::mem::take(&mut bufs[0]), std::mem::take(&mut bufs[1]), status));
        }
        if exited.is_none() && started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!("timed out after {} s", timeout.as_secs())));
        }
        let mut pfds: Vec<libc::pollfd> = (0..2).filter(|&i| open[i]).map(|i| libc::pollfd { fd: fds[i], events: libc::POLLIN, revents: 0 }).collect();
        // SAFETY: `pfds` is a live, non-empty Vec of initialised `pollfd`s and the count passed is its length; the fds stay
        // open because `stdout`/`stderr` are still owned here.
        let ready = if pfds.is_empty() { 0 } else { unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as _, 20) } };
        if pfds.is_empty() {
            std::thread::sleep(Duration::from_millis(5));
        }
        if ready > 0 {
            for pfd in &pfds {
                if pfd.revents == 0 {
                    continue;
                }
                let i = if pfd.fd == fds[0] { 0 } else { 1 };
                loop {
                    // SAFETY: `chunk` is writable for `chunk.len()` bytes (the count passed), and the fd is open (see above).
                    let n = unsafe { libc::read(fds[i], chunk.as_mut_ptr().cast(), chunk.len()) };
                    if n > 0 {
                        if (bufs[i].len() as u64) < MAX_OUTPUT {
                            bufs[i].extend_from_slice(&chunk[..n as usize]);
                        }
                    } else if n == 0 {
                        open[i] = false; // EOF
                        break;
                    } else {
                        let e = io::Error::last_os_error();
                        if e.kind() != io::ErrorKind::WouldBlock && e.kind() != io::ErrorKind::Interrupted {
                            open[i] = false;
                        }
                        break;
                    }
                }
            }
        }
        if exited.is_none()
            && let Some(status) = child.try_wait()?
        {
            exited = Some((status, Instant::now()));
        }
    }
}

#[cfg(windows)]
fn collect(
    child: &mut Child,
    mut stdout: std::process::ChildStdout,
    mut stderr: std::process::ChildStderr,
    timeout: Duration,
) -> io::Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED};
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    let mut bufs = [Vec::new(), Vec::new()];
    let mut open = [true, true];
    let started = Instant::now();
    let mut exited: Option<(std::process::ExitStatus, Instant)> = None;
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        if let Some((status, at)) = exited
            && (!open[0] && !open[1] || at.elapsed() >= DRAIN_GRACE)
        {
            return Ok((std::mem::take(&mut bufs[0]), std::mem::take(&mut bufs[1]), status));
        }
        if exited.is_none() && started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!("timed out after {} s", timeout.as_secs())));
        }
        let mut read_any = false;
        for i in 0..2 {
            if !open[i] {
                continue;
            }
            let handle = if i == 0 { stdout.as_raw_handle() } else { stderr.as_raw_handle() };
            let mut available = 0;
            // SAFETY: `handle` comes from `stdout`/`stderr`, still owned here, so it is an open pipe handle. The null out-pointers
            // are the documented way to skip those outputs; `available` is a valid u32. Peeking first means neither stream can
            // block the other or bypass the timeout.
            if unsafe { PeekNamedPipe(handle, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut available, std::ptr::null_mut()) } == 0 {
                let error = io::Error::last_os_error();
                if matches!(error.raw_os_error(), Some(e) if e == ERROR_BROKEN_PIPE as i32 || e == ERROR_PIPE_NOT_CONNECTED as i32) {
                    open[i] = false;
                    continue;
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            let count = (available as usize).min(chunk.len());
            if count > 0 {
                read_any = true;
                let n = if i == 0 { stdout.read(&mut chunk[..count])? } else { stderr.read(&mut chunk[..count])? };
                if n == 0 {
                    open[i] = false;
                }
                let keep = n.min((MAX_OUTPUT as usize).saturating_sub(bufs[i].len()));
                bufs[i].extend_from_slice(&chunk[..keep]);
            }
        }
        if exited.is_none()
            && let Some(status) = child.try_wait()?
        {
            exited = Some((status, Instant::now()));
        }
        if !read_any {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Long-lived process: written to via stdin, read via stdout (stderr is discarded). On Unix it leads its own process
/// group, so stopping it also stops what it started (a test runner's test binaries, an adapter's debuggee).
pub struct Proc {
    child: Child,
    waited: bool,
    /// Queue for the thread that writes to stdin: a stopped process's full pipe buffer must not block `write` (and with it the dispatch
    /// thread, `ProcKill`).
    input: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
}

pub fn spawn(program: &str, args: &[String], cwd: Option<&str>) -> io::Result<(Proc, impl Read + Send + 'static)> {
    let mut cmd = command(program, args, cwd)?;
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let stdin = child.stdin.take();
    let stdout = piped(child.stdout.take())?;
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(512);
    std::thread::spawn(move || {
        let Some(mut stdin) = stdin else { return };
        while let Ok(chunk) = rx.recv() {
            if stdin.write_all(&chunk).and_then(|_| stdin.flush()).is_err() {
                break;
            }
        }
        // When the queue closes (`wait`/`drop`) `stdin` is dropped: the process sees EOF.
    });
    Ok((Proc { child, waited: false, input: Some(tx) }, stdout))
}

impl Proc {
    /// Enqueue; never blocks. `WouldBlock` if the process isn't reading and the queue (512 chunks) is full.
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        use std::sync::mpsc::TrySendError;
        match self.input.as_ref() {
            Some(tx) => tx.try_send(data.to_vec()).map_err(|e| match e {
                TrySendError::Full(_) => io::Error::new(io::ErrorKind::WouldBlock, "the process is not reading its input"),
                TrySendError::Disconnected(_) => io::ErrorKind::BrokenPipe.into(),
            }),
            None => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }

    pub fn wait(&mut self) -> Option<i32> {
        self.input = None;
        self.waited = true;
        self.child.wait().ok().and_then(exit_code)
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.input = None;
        // The whole group, while its leader still runs (so the group id cannot belong to anyone else yet).
        #[cfg(unix)]
        if !self.waited
            && matches!(self.child.try_wait(), Ok(None))
            && let Ok(pid) = i32::try_from(self.child.id())
        {
            // SAFETY: kill has no memory-safety preconditions; a negative pid names the group this child leads.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Exit code; for a process killed by a signal, 128 + signal as in the shell (SIGSEGV → 139).
/// `None` only if waiting failed: the client treats this as a lost connection.
fn exit_code(status: std::process::ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return Some(128 + sig);
        }
    }
    status.code()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn stopping_a_process_stops_what_it_started() {
        let (p, mut out) = spawn("/bin/sh", &["-c".into(), "sleep 600 & echo $!; wait".into()], None).unwrap();
        let mut line = Vec::new();
        let mut b = [0u8; 1];
        while out.read(&mut b).unwrap() == 1 && b[0] != b'\n' {
            line.push(b[0]);
        }
        let child: i32 = String::from_utf8(line).unwrap().trim().parse().unwrap();
        drop(p);
        let started = Instant::now();
        // SAFETY: signal 0 only checks that the process exists.
        while unsafe { libc::kill(child, 0) } == 0 {
            assert!(started.elapsed() < Duration::from_secs(10), "the grandchild {child} outlived its parent");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(windows)]
    fn fixture(name: &str, stdin: Option<Vec<u8>>, timeout: Duration) -> io::Result<Output> {
        exec_with_timeout(
            &std::env::current_exe()?.to_string_lossy(),
            &["--exact".into(), format!("proc::tests::{name}"), "--ignored".into(), "--nocapture".into()],
            None,
            stdin,
            timeout,
        )
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "subprocess fixture"]
    fn windows_fixture_stdio() {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input).unwrap();
        print!("{input}");
        eprintln!("stderr-marker");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "subprocess fixture"]
    fn windows_fixture_sleep() {
        std::thread::sleep(Duration::from_secs(3));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "subprocess fixture"]
    fn windows_fixture_flood() {
        let data = vec![b'x'; MAX_OUTPUT as usize + 65536];
        let writer = std::thread::spawn(move || io::stderr().write_all(&data).unwrap());
        io::stdout().write_all(&vec![b'y'; MAX_OUTPUT as usize + 65536]).unwrap();
        writer.join().unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "subprocess fixture"]
    fn windows_fixture_background() {
        let _child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "proc::tests::windows_fixture_sleep", "--ignored", "--nocapture"])
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        println!("parent-exited");
    }

    #[cfg(windows)]
    #[test]
    fn windows_exec_roundtrips_unicode_stdin_and_stderr() {
        let out = fixture("windows_fixture_stdio", Some("Türkçe: ıİşğüöç\nsecond line\n".as_bytes().to_vec()), Duration::from_secs(5)).unwrap();
        assert_eq!(out.code, Some(0));
        assert!(String::from_utf8_lossy(&out.stdout).contains("Türkçe: ıİşğüöç\nsecond line"));
        assert!(String::from_utf8_lossy(&out.stderr).contains("stderr-marker"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_exec_enforces_timeout() {
        let start = Instant::now();
        let err = fixture("windows_fixture_sleep", None, Duration::from_millis(200)).err().expect("timeout");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(windows)]
    #[test]
    fn windows_exec_drains_both_streams_past_the_output_limit() {
        let out = fixture("windows_fixture_flood", None, Duration::from_secs(30)).unwrap();
        assert_eq!(out.code, Some(0));
        assert_eq!(out.stdout.len(), MAX_OUTPUT as usize);
        assert_eq!(out.stderr.len(), MAX_OUTPUT as usize);
    }

    #[cfg(windows)]
    #[test]
    fn windows_exec_does_not_wait_for_inherited_background_pipes() {
        let start = Instant::now();
        let out = fixture("windows_fixture_background", None, Duration::from_secs(5)).unwrap();
        assert_eq!(out.code, Some(0));
        assert!(String::from_utf8_lossy(&out.stdout).contains("parent-exited"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(windows)]
    #[test]
    fn windows_batch_launcher_preserves_spaces_unicode_and_literal_arguments() {
        let dir = std::env::temp_dir().join(format!("codeyard-cmd-şablon boşluk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("language-server.cmd");
        std::fs::write(&script, "@echo off\r\nchcp 65001 >nul\r\necho \"%~1\"\r\ncd\r\n").unwrap();
        let out = exec(&script.with_extension("").to_string_lossy(), &["Türkçe & literal".into()], Some(&dir.to_string_lossy()), None).unwrap();
        assert_eq!(out.code, Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("\"Türkçe & literal\""), "{text}");
        assert!(text.contains(&*dir.to_string_lossy()), "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn writing_to_a_process_that_never_reads_does_not_block() {
        // `sleep` never reads stdin: its pipe fills up. The old code blocked the dispatch thread (and the proc table lock)
        // in `write_all`; now the input is queued and the call returns, finally with WouldBlock.
        let (mut p, _out) = spawn("sleep", &["30".to_string()], None).unwrap();
        let chunk = vec![b'x'; 64 * 1024];
        let started = std::time::Instant::now();
        let mut refused = None;
        for _ in 0..20_000 {
            if let Err(e) = p.write(&chunk) {
                refused = Some(e.kind());
                break;
            }
        }
        assert!(started.elapsed() < Duration::from_secs(10), "write blocked for {:?}", started.elapsed());
        assert_eq!(refused, Some(io::ErrorKind::WouldBlock), "a full queue is reported, not waited for");
        // Killing still works immediately (the writer thread is stuck, the process is not).
        let t = std::time::Instant::now();
        drop(p);
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_missing_working_directory_is_an_error_not_a_silent_fallback() {
        let Err(e) = exec("pwd", &[], Some("/definitely/not/a/dir"), None) else { panic!("must fail") };
        assert!(e.to_string().contains("does not exist"), "{e}");
        assert!(spawn("cat", &[], Some("/definitely/not/a/dir")).is_err());
    }

    /// The command finished but a process it left in the background (a daemon started by the login shell,
    /// git's background maintenance) still holds stderr open: `exec` must not wait for it.
    #[test]
    #[cfg(unix)]
    fn exec_returns_when_the_command_exits_even_if_a_background_child_holds_its_pipes() {
        let t = Instant::now();
        let out = exec("/bin/sh", &["-c".into(), "sleep 20 >/dev/null & echo tamam; echo uyarı >&2".into()], None, None).unwrap();
        let took = t.elapsed();
        assert_eq!((out.code, out.stdout.as_slice(), out.stderr.as_slice()), (Some(0), "tamam\n".as_bytes(), "uyarı\n".as_bytes()));
        assert!(took < Duration::from_secs(3), "{took:?}: waiting for the background process");
        // The same goes for whoever holds stdout.
        let t = Instant::now();
        let out = exec("/bin/sh", &["-c".into(), "sleep 20 2>/dev/null & echo tamam".into()], None, None).unwrap();
        assert_eq!(out.stdout, b"tamam\n");
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }

    /// A command that never finishes: killed when the time runs out, returns an error.
    #[test]
    #[cfg(unix)]
    fn a_command_that_never_ends_is_killed_at_the_timeout() {
        let t = Instant::now();
        let err = exec_with_timeout("/bin/sleep", &["30".into()], None, None, Duration::from_millis(300)).err().expect("timeout");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    /// A command given stdin (git commit -F -) goes through the same path.
    #[test]
    #[cfg(unix)]
    fn stdin_is_written_and_closed() {
        let out = exec("/bin/cat", &[], None, Some(b"ileti\nikinci satir\n".to_vec())).unwrap();
        assert_eq!(out.stdout, b"ileti\nikinci satir\n");
    }

    /// Lots of output (exceeding the pipe's 64 KB) is read completely.
    #[test]
    #[cfg(unix)]
    fn large_output_is_complete() {
        let out = exec("/bin/sh", &["-c".into(), "i=0; while [ $i -lt 20000 ]; do echo satır-$i; i=$((i+1)); done; echo son >&2".into()], None, None).unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert_eq!(text.lines().count(), 20000);
        assert_eq!(text.lines().last(), Some("satır-19999"));
        assert_eq!(out.stderr, b"son\n");
    }
}
