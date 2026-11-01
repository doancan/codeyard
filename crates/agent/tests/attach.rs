//! ssh drop with real processes: the `--attach` bridge is killed, a new one attaches to the same
//! session; the daemon lives on as a detached process and the shell keeps running.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(unix)]

use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use proto::{Frame, Request, Response};

fn attach(sock: &std::path::Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_codeyard-agent")).arg("--attach").arg(sock).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let w = child.stdin.take().unwrap();
    let r = BufReader::new(child.stdout.take().unwrap());
    (child, w, r)
}

fn hello(w: &mut ChildStdin, r: &mut BufReader<ChildStdout>, session: &str) -> proto::HostInfo {
    let h = Request::Hello { client_version: proto::VERSION, session: Some(session.into()) };
    proto::write_frame(w, &Frame { id: 1, body: h }).unwrap();
    w.flush().unwrap();
    match proto::read_frame::<Response>(r).unwrap().unwrap().body {
        Response::Hello(info) => info,
        other => panic!("{other:?}"),
    }
}

#[test]
fn killing_the_bridge_keeps_the_session() {
    let dir = std::env::temp_dir().join(format!("codeyard-attach-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");

    let (mut c1, mut w1, mut r1) = attach(&sock);
    assert!(!hello(&mut w1, &mut r1, "pencere").resumed);
    let spawn = Request::PtySpawn { cols: 80, rows: 24, cwd: None, shell: Some("/bin/sh".into()) };
    proto::write_frame(&mut w1, &Frame { id: 2, body: spawn }).unwrap();
    let pty = loop {
        if let Response::PtySpawned { pty } = proto::read_frame::<Response>(&mut r1).unwrap().unwrap().body {
            break pty;
        }
    };
    let input = Request::PtyInput { pty, data: b"sleep 0.5; echo kopukken-uretildi\n".to_vec() };
    proto::write_frame(&mut w1, &Frame { id: 0, body: input }).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    c1.kill().unwrap(); // ssh dropped
    let _ = c1.wait();
    std::thread::sleep(Duration::from_millis(900));

    let (mut c2, mut w2, mut r2) = attach(&sock);
    assert!(hello(&mut w2, &mut r2, "pencere").resumed, "daemon must keep the session");
    let mut out = String::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !out.contains("kopukken-uretildi") {
        assert!(Instant::now() < deadline, "output: {out}");
        if let Response::PtyOutput { data, .. } = proto::read_frame::<Response>(&mut r2).unwrap().unwrap().body {
            out.push_str(&String::from_utf8_lossy(&data));
        }
    }
    c2.kill().unwrap();
    let _ = c2.wait();
    // Clean up the daemon (so the test doesn't leave it idle for 10 minutes).
    let _ = Command::new("pkill").arg("-f").arg(format!("codeyard-agent --daemon {}", sock.display())).status();
}

/// A dropped session closes when its grace time runs out, and the daemon exits once no sessions remain
/// (so no process lives forever on the server). Durations are shortened in the test.
#[test]
fn idle_daemon_exits_by_itself() {
    let dir = std::env::temp_dir().join(format!("codeyard-idle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_codeyard-agent"))
        .arg("--attach")
        .arg(&sock)
        .env("CODEYARD_DAEMON_GRACE", "1")
        .env("CODEYARD_DAEMON_IDLE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut w = child.stdin.take().unwrap();
    let mut r = BufReader::new(child.stdout.take().unwrap());
    hello(&mut w, &mut r, "gecici");
    // As the app does: set up watching (starts the background thread).
    proto::write_frame(&mut w, &Frame { id: 2, body: Request::Watch { paths: vec![dir.to_string_lossy().into_owned()] } }).unwrap();
    w.flush().unwrap();
    let _ = proto::read_frame::<Response>(&mut r).unwrap();
    drop(w); // ssh dropped
    let _ = child.wait();
    let deadline = Instant::now() + Duration::from_secs(20);
    while sock.exists() {
        assert!(Instant::now() < deadline, "daemon did not exit when idle");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Even when started from a background job (a parent that ignores SIGTERM) the daemon
/// must be stoppable with `kill`.
#[test]
fn daemon_stops_on_sigterm_even_if_parent_ignored_it() {
    let dir = std::env::temp_dir().join(format!("codeyard-term-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    // `trap '' TERM`: the shell ignores SIGTERM, its children inherit that.
    let script = format!("trap '' TERM; echo | {} --attach {} >/dev/null", env!("CARGO_BIN_EXE_codeyard-agent"), sock.display());
    assert!(Command::new("/bin/sh").arg("-c").arg(&script).status().unwrap().success() || sock.exists());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !sock.exists() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    let pids = Command::new("pgrep").arg("-f").arg(format!("codeyard-agent --daemon {}", sock.display())).output().unwrap();
    let pid = String::from_utf8_lossy(&pids.stdout).trim().lines().next().unwrap_or_default().to_string();
    assert!(!pid.is_empty());
    assert!(Command::new("kill").arg(&pid).status().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(5);
    while Command::new("kill").arg("-0").arg(&pid).status().unwrap().success() {
        assert!(Instant::now() < deadline, "daemon did not stop on SIGTERM");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Started from a thread that blocks SIGTERM (as macOS dispatch threads do, where the app starts its terminal daemon),
/// the daemon still stops on SIGTERM: the signal mask is inherited across exec, so the daemon clears it.
#[test]
fn daemon_stops_on_sigterm_even_if_its_starter_blocked_it() {
    let dir = std::env::temp_dir().join(format!("codeyard-mask-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("d.sock");
    let agent = env!("CARGO_BIN_EXE_codeyard-agent").to_string();
    let s = sock.clone();
    // A thread of its own, so the mask stays with it.
    std::thread::spawn(move || {
        // SAFETY: plain libc calls on a set on this thread's stack; the mask is this thread's only.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGTERM);
            libc::sigaddset(&mut set, libc::SIGINT);
            assert_eq!(libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()), 0);
        }
        let mut c = Command::new(&agent).arg("--attach").arg(&s).stdin(Stdio::piped()).stdout(Stdio::null()).spawn().unwrap();
        drop(c.stdin.take());
        let _ = c.wait();
    })
    .join()
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !sock.exists() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    let pids = Command::new("pgrep").arg("-f").arg(format!("codeyard-agent --daemon {}", sock.display())).output().unwrap();
    let pid = String::from_utf8_lossy(&pids.stdout).trim().lines().next().unwrap_or_default().to_string();
    assert!(!pid.is_empty());
    assert!(Command::new("kill").arg(&pid).status().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(5);
    while Command::new("kill").arg("-0").arg(&pid).status().unwrap().success() {
        if Instant::now() >= deadline {
            let _ = Command::new("kill").arg("-9").arg(&pid).status();
            panic!("daemon did not stop on SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Also works when the unix socket path limit (~104 bytes) is exceeded (long home directory or
/// CODEYARD_REMOTE_DIR). Caught by the real ssh test.
#[test]
fn long_socket_path_works() {
    let base = std::env::temp_dir().join(format!("codeyard-uzun-{}", std::process::id()));
    let deep = base.join("a".repeat(40)).join("b".repeat(40));
    std::fs::create_dir_all(&deep).unwrap();
    let sock = deep.join("agent-v7-0123456789ab.sock");
    assert!(sock.as_os_str().len() > 110, "{}", sock.display());
    let mut child = Command::new(env!("CARGO_BIN_EXE_codeyard-agent"))
        .arg("--attach")
        .arg(&sock)
        .env("CODEYARD_DAEMON_GRACE", "1")
        .env("CODEYARD_DAEMON_IDLE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut w = child.stdin.take().unwrap();
    let mut r = BufReader::new(child.stdout.take().unwrap());
    let info = hello(&mut w, &mut r, "uzun");
    assert!(!info.resumed);
    drop(w);
    let _ = child.wait();
    let deadline = Instant::now() + Duration::from_secs(20);
    while sock.exists() {
        assert!(Instant::now() < deadline, "daemon did not exit");
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = std::fs::remove_dir_all(&base);
}
