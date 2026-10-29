use std::sync::mpsc;
use std::time::{Duration, Instant};

use ext_api::value::{Notify, Platform, Response, ThemeKind};
use ext_proto::ToApp;

use super::*;

const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe.wasm");

/// The exthost cargo built beside the test binary; `cargo test --workspace` (CI) always builds it.
fn exthost() -> Option<PathBuf> {
    let p = exthost_path();
    if p.is_none() {
        eprintln!("skipped: codeyard-exthost is not built (cargo build -p ext-host)");
    }
    p
}

fn env() -> EnvInfo {
    EnvInfo {
        codeyard_version: "0.5.0".into(),
        api_version: ext_api::VERSION.to_string(),
        language: "en".into(),
        theme: ThemeKind::Dark,
        platform: Platform::Macos,
        remote: false,
        workspace_trusted: true,
    }
}

fn start(name: &str) -> Option<(Process, mpsc::Receiver<(String, Event)>, PathBuf)> {
    let exe = exthost()?;
    let dir = std::env::temp_dir().join(format!("runtime-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = Cache::new(&dir);
    let compiled = cache.compiled(&exe, PROBE).unwrap();
    let (tx, rx) = mpsc::channel();
    let s = Start {
        id: "you.probe",
        package_dir: &dir,
        compiled,
        data_dir: dir.join("data"),
        granted: vec![],
        untrusted: false,
        env: env(),
        settings: vec![],
        folders: vec![],
        reason: "command:echo".into(),
    };
    let p = Process::start(&exe, s.hello(), tx).unwrap();
    Some((p, rx, dir))
}

fn next(rx: &mpsc::Receiver<(String, Event)>) -> Event {
    rx.recv_timeout(Duration::from_secs(20)).unwrap().1
}

#[test]
fn an_extension_starts_answers_and_stops() {
    let Some((p, rx, dir)) = start("basic") else { return };
    loop {
        match next(&rx) {
            Event::Message(ToApp::Activated(r)) => {
                r.unwrap();
                break;
            }
            Event::Message(ToApp::Notify(Notify::Log { .. })) => {}
            other => panic!("{other:?}"),
        }
    }
    p.send(&ToHost::Handle { id: 7, deadline_ms: 0, request: command("echo", "\"hi\"") }).unwrap();
    match next(&rx) {
        Event::Message(ToApp::Notify(Notify::Respond { id: 7, result })) => assert_eq!(result, Ok(Response::Json("\"hi\"".into()))),
        other => panic!("{other:?}"),
    }
    p.stop();
    assert!(matches!(next(&rx), Event::Exited { crashed: false, .. }));
    // Compiled once: a second lookup is a cache hit with the same file.
    let cache = Cache::new(&dir);
    let t = Instant::now();
    let again = cache.compiled(&exthost().unwrap(), PROBE).unwrap();
    assert!(again.0.exists());
    assert!(t.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_trap_is_a_crash() {
    let Some((p, rx, _)) = start("trap") else { return };
    while !matches!(next(&rx), Event::Message(ToApp::Activated(_))) {}
    p.send(&ToHost::Handle { id: 1, deadline_ms: 0, request: command("trap", "") }).unwrap();
    let mut trapped = false;
    loop {
        match next(&rx) {
            Event::Message(ToApp::Trap(t)) => {
                assert!(t.message.contains("unreachable"), "{}", t.message);
                trapped = true;
            }
            Event::Exited { crashed, .. } => {
                assert!(crashed && trapped);
                break;
            }
            _ => {}
        }
    }
}

#[test]
fn three_crashes_in_a_minute_disable() {
    let mut c = Crashes::default();
    let t = Instant::now();
    assert!(!c.record("a.b", t));
    assert!(!c.record("a.b", t + Duration::from_secs(10)));
    assert!(c.record("a.b", t + Duration::from_secs(20)));
    let mut c = Crashes::default();
    assert!(!c.record("a.b", t));
    assert!(!c.record("a.b", t + Duration::from_secs(30)));
    assert!(!c.record("a.b", t + Duration::from_secs(95)), "the first one is over a minute old");
}

#[test]
fn pending_answers_go_to_their_waiter_once() {
    let mut p = Pending::default();
    let (id, rx) = p.add("a.b");
    p.answer("c.d", id, Ok(Response::None));
    assert!(rx.try_recv().is_err(), "another extension cannot answer");
    p.answer("a.b", id, Ok(Response::Json("1".into())));
    assert_eq!(rx.try_recv().unwrap(), Ok(Response::Json("1".into())));
    let (_, rx) = p.add("a.b");
    p.fail_all("a.b", "stopped");
    assert!(rx.try_recv().unwrap().is_err());
}

#[test]
fn granted_interfaces_follow_permissions() {
    let p = ext_manifest::Permissions { net: vec!["x.dev".into()], ..Default::default() };
    assert_eq!(granted_interfaces(&p), vec!["net".to_string()]);
}
