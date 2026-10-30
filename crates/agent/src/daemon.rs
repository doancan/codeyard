//! Daemon mode: sessions (terminals, watching) outlive the ssh connection.
//! Each ssh connection is an `--attach` bridge: stdio <-> a per-user unix socket.
//! A dropped session waits `GRACE`; with no sessions left the daemon exits after `IDLE_EXIT`.
//! On Windows the same daemon listens on a named pipe only the current user can open (`daemon_windows.rs`); it keeps
//! the app's local terminals for the next start.

use std::collections::HashMap;
use std::io::{self, BufReader, Read, Write};
#[cfg(any(unix, windows))]
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use proto::{Error, Frame, Request, Response};

use crate::{Locked, Session, hello};

/// A dropped session waits this long (terminals keep running).
pub const GRACE: Duration = Duration::from_secs(300);
/// The daemon exits this long after the last session is gone.
pub const IDLE_EXIT: Duration = Duration::from_secs(600);

struct Entry {
    session: Arc<Session>,
    detached_since: Option<Instant>,
}

#[derive(Default)]
pub struct Registry {
    sessions: Mutex<HashMap<String, Entry>>,
}

fn new_session_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let seed = format!("{nanos}-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
    format!("{:016x}", proto::content_hash(seed.as_bytes()))
}

impl Registry {
    /// Drive one connection: the first frame must be `Hello { session }`. If the same session
    /// id exists, attach to it (`resumed`), otherwise open a new one.
    pub fn serve_connection(self: &Arc<Self>, input: impl Read, mut output: impl Write + Send + 'static) -> io::Result<()> {
        let mut input = BufReader::new(input);
        let Some(Frame { id, body }) = proto::read_frame::<Request>(&mut input)? else { return Ok(()) };
        let Request::Hello { client_version, session } = body else {
            let err = Response::Error(Error::Other("expected a hello first".into()));
            return proto::write_frame(&mut output, &Frame { id, body: &err });
        };
        let mut info = match hello(client_version, false) {
            Ok(Response::Hello(info)) => info,
            Ok(other) => return proto::write_frame(&mut output, &Frame { id, body: &other }),
            Err(e) => return proto::write_frame(&mut output, &Frame { id, body: &Response::Error(e) }),
        };
        let key = session.unwrap_or_else(new_session_id);
        let (sess, resumed) = {
            let mut map = self.sessions.locked();
            match map.get_mut(&key) {
                // An exclusive session is not taken from a connection that is still there.
                Some(e) if key.starts_with(proto::EXCLUSIVE_SESSION) && e.session.out.is_attached() => {
                    let refused = Response::Error(Error::PermissionDenied("the session is in use by another connection".into()));
                    return proto::write_frame(&mut output, &Frame { id, body: &refused });
                }
                Some(e) => {
                    e.detached_since = None;
                    (e.session.clone(), true)
                }
                None => {
                    let s = Arc::new(Session::default());
                    map.insert(key.clone(), Entry { session: s.clone(), detached_since: None });
                    (s, false)
                }
            }
        };
        info.resumed = resumed;
        let result = sess.run(&mut input, Box::new(output), Some(Frame { id, body: Response::Hello(info) }), None);
        // A session closed with `Bye` is removed without waiting for grace (the shells are already closed).
        if sess.is_stopped() {
            self.sessions.locked().remove(&key);
            return result;
        }
        // Connection ended: if no other connection took over, the session is disconnected (or ends now, if its client
        // asked not to keep it).
        if !sess.out.is_attached() {
            let mut map = self.sessions.locked();
            if sess.keep_for.load(Ordering::Relaxed) == 0 {
                if let Some(e) = map.remove(&key) {
                    e.session.shutdown();
                }
            } else if let Some(e) = map.get_mut(&key) {
                e.detached_since.get_or_insert_with(Instant::now);
            }
        }
        result
    }

    /// Close disconnected sessions whose grace (their own `KeepFor`, else `grace`) has expired. Returns whether any
    /// session is left.
    pub fn reap(&self, grace: Duration) -> bool {
        let mut map = self.sessions.locked();
        map.retain(|_, e| match e.detached_since {
            Some(t) if t.elapsed() >= Some(e.session.keep_for.load(Ordering::Relaxed)).filter(|s| *s != u64::MAX).map_or(grace, Duration::from_secs) => {
                e.session.shutdown();
                false
            }
            _ => true,
        });
        !map.is_empty()
    }
}

/// End sessions whose grace has expired, and the daemon itself once it has had no sessions for `IDLE_EXIT` (`at_exit`
/// runs first). Tests can shorten the durations (seconds): CODEYARD_DAEMON_GRACE / CODEYARD_DAEMON_IDLE.
#[cfg(any(unix, windows))]
fn start_reaper(registry: Arc<Registry>, at_exit: impl FnOnce() + Send + 'static) {
    let secs = |name: &str, default: Duration| std::env::var(name).ok().and_then(|v| v.parse().ok()).map_or(default, Duration::from_secs);
    let (grace, idle) = (secs("CODEYARD_DAEMON_GRACE", GRACE), secs("CODEYARD_DAEMON_IDLE", IDLE_EXIT));
    let tick = Duration::from_secs(10).min(grace.max(Duration::from_secs(1)));
    std::thread::spawn(move || {
        let mut empty_since: Option<Instant> = None;
        loop {
            std::thread::sleep(tick);
            if registry.reap(grace) {
                empty_since = None;
            } else if empty_since.get_or_insert_with(Instant::now).elapsed() >= idle {
                at_exit();
                std::process::exit(0);
            }
        }
    });
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use super::*;

    /// Unix socket paths are limited to ~104 bytes (macOS 104, Linux 108). For a long path,
    /// chdir into the socket's directory, bind/listen by relative name, then chdir back
    /// (terminals start in the working directory). Only call while single-threaded.
    fn short<T>(socket: &Path, f: impl FnOnce(&Path) -> io::Result<T>) -> io::Result<T> {
        if socket.as_os_str().len() < 100 {
            return f(socket);
        }
        let (Some(dir), Some(name)) = (socket.parent(), socket.file_name()) else { return f(socket) };
        let back = std::env::current_dir()?;
        std::env::set_current_dir(dir)?;
        let result = f(Path::new(name));
        std::env::set_current_dir(back)?;
        result
    }

    fn connect(socket: &Path) -> io::Result<UnixStream> {
        short(socket, |p| UnixStream::connect(p))
    }

    /// Daemon: listen on the socket. Returns immediately if a daemon is already running.
    pub fn run(socket: &Path) -> io::Result<()> {
        if let Some(dir) = socket.parent() {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        // If two bridges start for the first time at once, one must not take the other's new socket for dead and delete it: the "alive? / delete / bind"
        // steps are serialized with a lock file (held until `listener` exists).
        let _lock = SocketLock::acquire(socket)?;
        if socket.exists() {
            if connect(socket).is_ok() {
                return Ok(());
            }
            let _ = std::fs::remove_file(socket); // stale, dead socket
        }
        let listener = short(socket, |p| UnixListener::bind(p))?;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
        let registry = Arc::new(Registry::default());
        let sock = socket.to_path_buf();
        start_reaper(registry.clone(), move || {
            let _ = std::fs::remove_file(&sock);
        });

        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let registry = registry.clone();
            std::thread::spawn(move || {
                if let Ok(reader) = stream.try_clone() {
                    let _ = registry.serve_connection(reader, stream);
                }
            });
        }
        Ok(())
    }

    /// `flock` on `<socket>.lock` (the OS releases it when the process dies).
    pub(super) struct SocketLock {
        _file: std::fs::File,
    }

    impl SocketLock {
        pub(super) fn acquire(socket: &Path) -> io::Result<Self> {
            use std::os::fd::AsRawFd;
            let mut name = socket.as_os_str().to_os_string();
            name.push(".lock");
            let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).mode_600().open(Path::new(&name))?;
            // SAFETY: `file` owns the fd and lives past the call. flock touches no memory; the lock is kept for as long as the
            // open file description lives (`SocketLock` holds it) and the kernel drops it when the process dies.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(SocketLock { _file: file })
        }
    }

    trait Mode600 {
        fn mode_600(&mut self) -> &mut Self;
    }
    impl Mode600 for std::fs::OpenOptions {
        fn mode_600(&mut self) -> &mut Self {
            use std::os::unix::fs::OpenOptionsExt;
            self.mode(0o600)
        }
    }

    /// A connection to the daemon on `socket`, started first (this executable with `--daemon`, a detached process) if
    /// none answers.
    pub fn connect_or_start(socket: &Path) -> io::Result<UnixStream> {
        if let Ok(s) = connect(socket) {
            return Ok(s);
        }
        spawn_daemon(socket)?;
        let mut last = None;
        for _ in 0..100 {
            match connect(socket) {
                Ok(s) => return Ok(s),
                Err(e) => last = Some(e),
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        Err(last.unwrap_or_else(|| io::Error::other("daemon did not start")))
    }

    /// Bridge: stdio <-> socket. Starts the daemon if missing (detached process, doesn't die with ssh).
    pub fn attach(socket: &Path) -> io::Result<()> {
        let stream = connect_or_start(socket)?;
        let mut from_daemon = stream.try_clone()?;
        std::thread::spawn(move || {
            // stdout is line-buffered: flush after every chunk for binary frames.
            let mut out = io::stdout().lock();
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = from_daemon.read(&mut buf) {
                if n == 0 || out.write_all(&buf[..n]).and_then(|_| out.flush()).is_err() {
                    break;
                }
            }
            std::process::exit(0);
        });
        let mut to_daemon = stream;
        let mut input = io::stdin().lock();
        let mut buf = vec![0u8; 64 * 1024];
        while let Ok(n) = input.read(&mut buf) {
            if n == 0 || to_daemon.write_all(&buf[..n]).is_err() {
                break;
            }
        }
        // ssh dropped (stdin closed): the daemon side marks the session disconnected, shells live on.
        let _ = to_daemon.shutdown(std::net::Shutdown::Both);
        Ok(())
    }

    fn spawn_daemon(socket: &Path) -> io::Result<()> {
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.arg("--daemon").arg(socket).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        // SAFETY: the closure runs between fork and exec, where only async-signal-safe calls are allowed. It makes just
        // setsid and signal, captures nothing, and allocates nothing.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                // "Ignore" dispositions are inherited across exec: a daemon started
                // from a background job ignored SIGTERM and `kill` didn't stop it.
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                Ok(())
            });
        }
        cmd.spawn().map(|_| ())
    }
}

#[cfg(unix)]
pub use unix::{attach, connect_or_start, run};

#[cfg(windows)]
#[path = "daemon_windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::{Pipe, connect, connect_or_start, pipe_name, run};

#[cfg(test)]
mod tests {
    use super::*;

    struct Conn {
        w: io::PipeWriter,
        r: BufReader<io::PipeReader>,
    }

    fn connect(reg: &Arc<Registry>, session: Option<&str>) -> (Conn, proto::HostInfo) {
        let (req_r, req_w) = io::pipe().unwrap();
        let (resp_r, resp_w) = io::pipe().unwrap();
        let reg = reg.clone();
        std::thread::spawn(move || reg.serve_connection(req_r, resp_w));
        let mut c = Conn { w: req_w, r: BufReader::new(resp_r) };
        let hello = Request::Hello { client_version: proto::VERSION, session: session.map(str::to_string) };
        proto::write_frame(&mut c.w, &Frame { id: 1, body: hello }).unwrap();
        let Some(Frame { body: Response::Hello(info), .. }) = proto::read_frame::<Response>(&mut c.r).unwrap() else { panic!("hello") };
        (c, info)
    }

    #[cfg(unix)]
    #[test]
    fn terminal_survives_a_dropped_connection_and_output_is_replayed() {
        let reg = Arc::new(Registry::default());
        let (mut c, info) = connect(&reg, Some("oturum-1"));
        assert!(!info.resumed);
        proto::write_frame(&mut c.w, &Frame { id: 2, body: Request::PtySpawn { cols: 80, rows: 24, cwd: None, shell: Some("/bin/sh".into()) } }).unwrap();
        let pty = loop {
            match proto::read_frame::<Response>(&mut c.r).unwrap().unwrap().body {
                Response::PtySpawned { pty } => break pty,
                _ => continue,
            }
        };
        // A command whose output arrives only after the connection is lost.
        proto::write_frame(&mut c.w, &Frame { id: 0, body: Request::PtyInput { pty, data: b"sleep 0.4; echo sonra-geldi\n".to_vec() } }).unwrap();
        drop(c); // ssh dropped
        std::thread::sleep(Duration::from_millis(900));

        let (mut c2, info) = connect(&reg, Some("oturum-1"));
        assert!(info.resumed, "must resume the same session");
        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !String::from_utf8_lossy(&out).contains("sonra-geldi") {
            assert!(Instant::now() < deadline, "buffered output did not arrive: {}", String::from_utf8_lossy(&out));
            if let Response::PtyOutput { data, .. } = proto::read_frame::<Response>(&mut c2.r).unwrap().unwrap().body {
                out.extend(data);
            }
        }
        // The shell is still alive: new input works.
        proto::write_frame(&mut c2.w, &Frame { id: 0, body: Request::PtyInput { pty, data: b"echo hala-burada\n".to_vec() } }).unwrap();
        let mut out = Vec::new();
        while !String::from_utf8_lossy(&out).contains("hala-burada") {
            if let Response::PtyOutput { data, .. } = proto::read_frame::<Response>(&mut c2.r).unwrap().unwrap().body {
                out.extend(data);
            }
        }

        let (_c3, other) = connect(&reg, Some("baska"));
        assert!(!other.resumed, "different id: new session");
    }

    #[cfg(unix)]
    #[test]
    fn bye_closes_the_session_at_once_instead_of_waiting_for_the_grace_period() {
        let reg = Arc::new(Registry::default());
        let (mut c, _) = connect(&reg, Some("kapanacak"));
        proto::write_frame(&mut c.w, &Frame { id: 2, body: Request::PtySpawn { cols: 80, rows: 24, cwd: None, shell: Some("/bin/sh".into()) } }).unwrap();
        loop {
            if let Response::PtySpawned { .. } = proto::read_frame::<Response>(&mut c.r).unwrap().unwrap().body {
                break;
            }
        }
        proto::write_frame(&mut c.w, &Frame { id: 0, body: Request::Bye }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !reg.sessions.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the session was not removed after Bye");
            std::thread::sleep(Duration::from_millis(10));
        }
        // The same id is a brand-new session now (nothing to resume).
        let (_c, info) = connect(&reg, Some("kapanacak"));
        assert!(!info.resumed);
    }

    #[cfg(unix)]
    #[test]
    fn the_socket_lock_serialises_two_starters() {
        let dir = std::env::temp_dir().join(format!("codeyard-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("d.sock");
        let first = unix::SocketLock::acquire(&sock).unwrap();
        let s2 = sock.clone();
        let started = Instant::now();
        let second = std::thread::spawn(move || {
            let _l = unix::SocketLock::acquire(&s2).unwrap();
            started.elapsed()
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(!second.is_finished(), "the second starter must wait for the first");
        drop(first);
        assert!(second.join().unwrap() >= Duration::from_millis(250));
    }

    #[test]
    fn expired_sessions_are_reaped() {
        let reg = Arc::new(Registry::default());
        let (c, _) = connect(&reg, Some("gecici"));
        drop(c);
        std::thread::sleep(Duration::from_millis(100));
        assert!(reg.reap(GRACE), "must stay until the grace expires");
        assert!(!reg.reap(Duration::ZERO), "must be reaped once the grace expires");
        let (_c, info) = connect(&reg, Some("gecici"));
        assert!(!info.resumed);
    }

    #[cfg(unix)]
    #[test]
    fn real_unix_socket() {
        use std::os::unix::net::UnixStream;
        let dir = std::env::temp_dir().join(format!("codeyard-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sock = dir.join("a.sock");
        let s2 = sock.clone();
        std::thread::spawn(move || run(&s2));
        let mut stream = None;
        for _ in 0..100 {
            if let Ok(s) = UnixStream::connect(&sock) {
                stream = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut s = stream.expect("daemon must listen");
        let hello = Request::Hello { client_version: proto::VERSION, session: None };
        proto::write_frame(&mut s, &Frame { id: 7, body: hello }).unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let f: Frame<Response> = proto::read_frame(&mut r).unwrap().unwrap();
        assert!(matches!(f.body, Response::Hello(_)));
        // bind() makes the socket connectable before the daemon applies its mode.
        // The handshake is the readiness barrier; the parent is already owner-only.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777, 0o600, "owner only");
    }
}
