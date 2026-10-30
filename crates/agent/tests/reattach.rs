//! Terminals taken back after a client restart, with a real daemon process: the client's connection is dropped (as when
//! the app is killed), a new connection to the same session attaches to the same shell with its token, a wrong or
//! foreign token is refused, only the newest connection may type, output lost while away is counted, and an orphaned
//! session's shells end after the grace period, then the idle daemon exits.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use proto::{Frame, Request, Response};

struct Conn {
    w: UnixStream,
    r: BufReader<UnixStream>,
    next: u32,
}

impl Conn {
    fn open(sock: &Path, session: &str) -> (Conn, bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match UnixStream::connect(sock) {
                Ok(s) => break s,
                Err(e) => assert!(Instant::now() < deadline, "no daemon: {e}"),
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut c = Conn { r: BufReader::new(stream.try_clone().unwrap()), w: stream, next: 10 };
        let hello = Request::Hello { client_version: proto::VERSION, session: Some(session.into()) };
        proto::write_frame(&mut c.w, &Frame { id: 1, body: hello }).unwrap();
        let resumed = match proto::read_frame::<Response>(&mut c.r).unwrap().unwrap().body {
            Response::Hello(info) => info.resumed,
            other => panic!("{other:?}"),
        };
        (c, resumed)
    }

    /// A request and its answer; events read meanwhile are added to `seen`.
    fn call(&mut self, body: Request, seen: &mut String) -> Response {
        self.next += 1;
        let id = self.next;
        proto::write_frame(&mut self.w, &Frame { id, body }).unwrap();
        loop {
            let frame = proto::read_frame::<Response>(&mut self.r).unwrap().unwrap();
            match frame.body {
                Response::PtyOutput { data, .. } => seen.push_str(&String::from_utf8_lossy(&data)),
                body if frame.id == id => return body,
                _ => {}
            }
        }
    }

    fn input(&mut self, pty: u32, text: &str) {
        proto::write_frame(&mut self.w, &Frame { id: 0, body: Request::PtyInput { pty, data: text.as_bytes().to_vec() } }).unwrap();
        self.w.flush().unwrap();
    }

    /// One more frame of output (or nothing within 200 ms).
    fn read_some(&mut self, seen: &mut String) {
        self.w.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        if let Ok(Some(Frame { body: Response::PtyOutput { data, .. }, .. })) = proto::read_frame::<Response>(&mut self.r) {
            seen.push_str(&String::from_utf8_lossy(&data));
        }
        self.w.set_read_timeout(None).unwrap();
    }

    /// Output until it contains `want`.
    fn read_until(&mut self, seen: &mut String, want: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        self.w.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        while !seen.contains(want) {
            assert!(Instant::now() < deadline, "waited for {want:?}, got {seen:?}");
            match proto::read_frame::<Response>(&mut self.r) {
                Ok(Some(Frame { body: Response::PtyOutput { data, .. }, .. })) => seen.push_str(&String::from_utf8_lossy(&data)),
                Ok(Some(_)) | Err(_) => {}
                Ok(None) => panic!("connection closed; got {seen:?}"),
            }
        }
        self.w.set_read_timeout(None).unwrap();
    }
}

fn daemon(name: &str, grace: u64, idle: u64) -> (Child, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("codeyard-reattach-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("t.sock");
    let child = Command::new(env!("CARGO_BIN_EXE_codeyard-agent"))
        .arg("--daemon")
        .arg(&sock)
        .env("CODEYARD_DAEMON_GRACE", grace.to_string())
        .env("CODEYARD_DAEMON_IDLE", idle.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    (child, dir, sock)
}

fn spawn_shell(c: &mut Conn) -> u32 {
    match c.call(Request::PtySpawn { cols: 80, rows: 24, cwd: None, shell: Some("/bin/sh".into()) }, &mut String::new()) {
        Response::PtySpawned { pty } => pty,
        other => panic!("{other:?}"),
    }
}

fn token(c: &mut Conn, pty: u32) -> (String, String) {
    match c.call(Request::PtyToken { pty }, &mut String::new()) {
        Response::PtyToken { instance, secret } => (instance, secret),
        other => panic!("{other:?}"),
    }
}

/// The shell's pid, printed by the shell itself.
fn shell_pid(c: &mut Conn, pty: u32, tag: &str) -> String {
    // The echoed command line shows `{tag}=$$`; the answer is `{tag}=<digits>` followed by a line end.
    let mut seen = String::new();
    c.input(pty, &format!("echo {tag}=$$\n"));
    let marker = format!("{tag}=");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for (at, _) in seen.match_indices(&marker) {
            let rest = &seen[at + marker.len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty() && rest[digits.len()..].starts_with(['\r', '\n']) {
                return digits;
            }
        }
        assert!(Instant::now() < deadline, "no pid in {seen:?}");
        c.read_some(&mut seen);
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn a_restarted_client_takes_its_terminal_back_with_its_token_only() {
    let (mut d, dir, sock) = daemon("restart", 300, 600);
    let (mut c1, resumed) = Conn::open(&sock, "window-a");
    assert!(!resumed);
    let pty = spawn_shell(&mut c1);
    let before = shell_pid(&mut c1, pty, "before");
    let (instance, secret) = token(&mut c1, pty);
    assert_eq!((instance.len(), secret.len()), (32, 32), "128-bit random hex");
    // Output produced while no client is attached, then the client "dies".
    c1.input(pty, "sleep 0.4; echo while-away\n");
    drop(c1);
    std::thread::sleep(Duration::from_millis(900));

    let (mut c2, resumed) = Conn::open(&sock, "window-a");
    assert!(resumed, "the session outlived its client");
    let mut seen = String::new();
    // Foreign or guessed tokens are refused.
    let wrong = c2.call(Request::PtyAttach { pty, instance: instance.clone(), secret: "0".repeat(32) }, &mut seen);
    assert!(matches!(wrong, Response::Error(proto::Error::PermissionDenied(_))), "{wrong:?}");
    let foreign = c2.call(Request::PtyAttach { pty, instance: "f".repeat(32), secret: secret.clone() }, &mut seen);
    assert!(matches!(foreign, Response::Error(proto::Error::PermissionDenied(_))), "{foreign:?}");
    let missing = c2.call(Request::PtyAttach { pty: pty + 7, instance: instance.clone(), secret: secret.clone() }, &mut seen);
    assert!(matches!(missing, Response::Error(proto::Error::NotFound(_))), "{missing:?}");
    match c2.call(Request::PtyAttach { pty, instance, secret }, &mut seen) {
        Response::PtyAttached { pty: p, sequence, dropped } => assert!(p == pty && sequence > 0 && dropped == 0, "{sequence} {dropped}"),
        other => panic!("{other:?}"),
    }
    c2.read_until(&mut seen, "while-away");
    // The same shell, and it still takes input.
    assert_eq!(shell_pid(&mut c2, pty, "after"), before, "the same process");
    assert!(matches!(c2.call(Request::PtyList, &mut String::new()), Response::PtyList(ids) if ids == vec![pty]));

    // Another connection to the session takes it over: the replaced one can no longer type.
    let (mut c3, resumed) = Conn::open(&sock, "window-a");
    assert!(resumed);
    let stale = dir.join("from-the-replaced-connection");
    c2.input(pty, &format!("touch {}\n", stale.display()));
    let fresh = dir.join("from-the-owner");
    c3.input(pty, &format!("touch {}\n", fresh.display()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fresh.exists() {
        assert!(Instant::now() < deadline, "the owner's input did not arrive");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(!stale.exists(), "a replaced connection must not type into the terminal");
    drop((c2, c3));
    let _ = d.kill();
    let _ = d.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// An exclusive session (a window's terminals) is refused to a second connection while the first one lives, and given
/// to it once the first is gone.
#[test]
fn an_exclusive_session_is_not_taken_from_a_live_connection() {
    let (mut d, dir, sock) = daemon("exclusive", 300, 600);
    let session = format!("{}window-x", proto::EXCLUSIVE_SESSION);
    let (mut c1, _) = Conn::open(&sock, &session);
    let pty = spawn_shell(&mut c1);
    let mut s = UnixStream::connect(&sock).unwrap();
    proto::write_frame(&mut s, &Frame { id: 1, body: Request::Hello { client_version: proto::VERSION, session: Some(session.clone()) } }).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    let refused = proto::read_frame::<Response>(&mut r).unwrap().unwrap().body;
    assert!(matches!(refused, Response::Error(proto::Error::PermissionDenied(_))), "{refused:?}");
    // The first connection still owns its terminal.
    let marker = dir.join("still-owned");
    c1.input(pty, &format!("touch {}\n", marker.display()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the first connection lost its terminal");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Its owner gone, the session goes to the next connection.
    drop(c1);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut s = UnixStream::connect(&sock).unwrap();
        proto::write_frame(&mut s, &Frame { id: 1, body: Request::Hello { client_version: proto::VERSION, session: Some(session.clone()) } }).unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        if let Response::Hello(info) = proto::read_frame::<Response>(&mut r).unwrap().unwrap().body {
            assert!(info.resumed);
            break;
        }
        assert!(Instant::now() < deadline, "the session was not freed after its owner left");
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = d.kill();
    let _ = d.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn output_lost_while_away_is_counted() {
    let (mut d, dir, sock) = daemon("gap", 300, 600);
    let (mut c1, _) = Conn::open(&sock, "window-gap");
    let pty = spawn_shell(&mut c1);
    let (instance, secret) = token(&mut c1, pty);
    // More than the 256 KB backlog while detached.
    c1.input(pty, "sleep 0.3; head -c 600000 /dev/zero | tr '\\0' x; echo; echo gap-done\n");
    drop(c1);
    std::thread::sleep(Duration::from_millis(1500));
    let (mut c2, resumed) = Conn::open(&sock, "window-gap");
    assert!(resumed);
    let mut seen = String::new();
    match c2.call(Request::PtyAttach { pty, instance, secret }, &mut seen) {
        Response::PtyAttached { dropped, sequence, .. } => assert!(dropped > 0 && sequence >= 600_000, "dropped {dropped}, sequence {sequence}"),
        other => panic!("{other:?}"),
    }
    let _ = d.kill();
    let _ = d.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn orphaned_terminals_end_after_the_grace_period_and_the_idle_daemon_exits() {
    let (mut d, dir, sock) = daemon("orphan", 1, 1);
    let (mut c1, _) = Conn::open(&sock, "window-orphan");
    let pty = spawn_shell(&mut c1);
    let pid: i32 = shell_pid(&mut c1, pty, "orphan").parse().unwrap();
    assert!(alive(pid));
    drop(c1);
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) {
        assert!(Instant::now() < deadline, "the orphaned shell {pid} still runs after the grace period");
        std::thread::sleep(Duration::from_millis(50));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if d.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "the idle daemon did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!sock.exists(), "an exiting daemon removes its socket");
    let _ = std::fs::remove_dir_all(&dir);
}
