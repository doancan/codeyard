//! codeyard-exthost as the app sees it: a child process speaking `ext-proto`, running testdata/probe.wasm
//! (tests/guests/probe). Each test plays the app's side of the conversation.

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use ext_api::value::{Completion, EnvInfo, LogLevel, Notify, Platform, Request, Response, Start, ThemeKind};
use ext_proto::{Hello, Reader, ToApp, ToHost};
use sha2::{Digest, Sha256};

const EXE: &str = env!("CARGO_BIN_EXE_codeyard-exthost");
// The probe with its sandbox escape attempts (a superset of testdata/probe.wasm).
const PROBE: &[u8] = include_bytes!("../testdata/probe-escape.wasm");

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("exthost-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `--compile` the probe once per test folder; returns the compiled file and its hash.
fn compile(dir: &Path) -> (PathBuf, [u8; 32]) {
    let mut c = Command::new(EXE).arg("--compile").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(PROBE).unwrap();
    let out = c.wait_with_output().unwrap();
    assert!(out.status.success());
    let path = dir.join("probe.cwasm");
    std::fs::write(&path, &out.stdout).unwrap();
    (path, Sha256::digest(&out.stdout).into())
}

fn hello(dir: &Path, compiled: &Path, sha: [u8; 32], granted: &[&str], untrusted: bool) -> Hello {
    Hello {
        version: ext_proto::VERSION,
        build: ext_proto::BUILD.into(),
        extension: "you.probe".into(),
        package_dir: dir.to_string_lossy().into_owned(),
        compiled: compiled.to_string_lossy().into_owned(),
        compiled_sha256: sha,
        data_dir: dir.join("data").to_string_lossy().into_owned(),
        granted: granted.iter().map(|s| s.to_string()).collect(),
        untrusted,
        env: EnvInfo {
            codeyard_version: "0.5.0".into(),
            api_version: ext_api::VERSION.to_string(),
            language: "en".into(),
            theme: ThemeKind::Dark,
            platform: Platform::Linux,
            remote: false,
            workspace_trusted: !untrusted,
        },
        settings: vec![("interval".into(), Some("60".into()))],
        folders: vec!["file:///work".into()],
        reason: "command:echo".into(),
        // Windows sandboxes the exthost from the parent (an AppContainer), which these tests do not do.
        sandbox: cfg!(unix),
    }
}

struct Host {
    child: Child,
    stdin: ChildStdin,
    reader: Reader<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Host {
    fn start(h: Hello) -> Host {
        let mut child = Command::new(EXE).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let reader = Reader::limited(BufReader::new(child.stdout.take().unwrap()), Instant::now());
        ext_proto::write(&mut stdin, &ToHost::Hello(Box::new(h))).unwrap();
        Host { child, stdin, reader, next_id: 1 }
    }

    fn next(&mut self) -> ToApp {
        self.reader.read(Instant::now()).unwrap()
    }

    fn ready(mut self) -> Host {
        assert!(matches!(self.next(), ToApp::Ready { .. }));
        loop {
            match self.next() {
                ToApp::Activated(r) => {
                    r.unwrap();
                    return self;
                }
                ToApp::Notify(Notify::Log { .. }) => {}
                other => panic!("{other:?}"),
            }
        }
    }

    fn send(&mut self, m: &ToHost) {
        ext_proto::write(&mut self.stdin, m).unwrap();
    }

    fn command(&mut self, command: &str, args: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request::Command { command: command.into(), args_json: args.into() };
        self.send(&ToHost::Handle { id, deadline_ms: 0, request });
        id
    }

    /// The next answer to `id`, collecting other frames.
    fn answer(&mut self, id: u64) -> (Result<Response, ext_api::value::Error>, Vec<ToApp>) {
        let mut seen = Vec::new();
        loop {
            match self.next() {
                ToApp::Notify(Notify::Respond { id: i, result }) if i == id => return (result, seen),
                other => seen.push(other),
            }
        }
    }

    fn json(&mut self, command: &str, args: &str) -> String {
        let id = self.command(command, args);
        match self.answer(id).0 {
            Ok(Response::Json(j)) => j,
            other => panic!("{other:?}"),
        }
    }

    fn exit(mut self) -> std::process::ExitStatus {
        drop(self.stdin);
        self.child.wait().unwrap()
    }
}

#[test]
fn commands_run_and_tickets_complete() {
    let d = dir("basic");
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &[], false)).ready();
    assert_eq!(h.json("echo", "[1,2]"), "[1,2]");
    assert_eq!(h.json("env", ""), ext_api::VERSION.to_string());
    assert_eq!(h.json("settings", ""), "Some(\"60\")");
    // A notice leaves as a Start frame; its completion comes back as the answer.
    let id = h.command("notice", "");
    let ticket = match h.next() {
        ToApp::Start { ticket, call: Start::ShowNotice { actions, .. } } => {
            assert_eq!(actions, ["A", "B"]);
            ticket
        }
        other => panic!("{other:?}"),
    };
    h.send(&ToHost::Complete { ticket, completion: Completion::Chosen(vec![1]) });
    assert_eq!(h.answer(id).0, Ok(Response::Json("chosen [1]".into())));
    h.send(&ToHost::Shutdown);
    assert!(h.exit().success());
    // `deactivate` ran and storage is on disk.
    assert!(d.join("data/storage").join(hex("deactivated")).exists());
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn a_permission_not_granted_is_refused_without_leaving_the_exthost() {
    let d = dir("refused");
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &["net"], false)).ready();
    let id = h.command("read", "");
    let (answer, other) = h.answer(id);
    assert_eq!(answer, Ok(Response::Json("denied read Why::RevokedByUser".into())));
    assert!(!other.iter().any(|m| matches!(m, ToApp::Start { .. })), "nothing reached the app: {other:?}");
    // A granted one does leave.
    h.command("fetch", "https://api.example.com/x");
    assert!(matches!(h.next(), ToApp::Start { call: Start::Fetch { .. }, .. }));
}

#[test]
fn an_untrusted_folder_refuses_sending() {
    let d = dir("untrusted");
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &["net", "workspace"], true)).ready();
    assert_eq!(h.json("fetch", "https://api.example.com/x"), "denied net Why::UntrustedWorkspace");
    // Reading what it declared is still allowed there.
    h.command("read", "");
    assert!(matches!(h.next(), ToApp::Start { call: Start::ReadFile { .. }, .. }));
}

#[test]
fn storage_persists_and_prints_reach_the_log() {
    let d = dir("storage");
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &[], false)).ready();
    assert_eq!(h.json("store", "v1"), "Ok(()) [\"k\"]");
    let id = h.command("print", "");
    assert_eq!(h.answer(id).0, Ok(Response::None));
    // What it printed is sent when the call returns, before the next call's frames.
    let id = h.command("echo", "");
    let (_, frames) = h.answer(id);
    let logs: Vec<_> = frames
        .iter()
        .filter_map(|m| match m {
            ToApp::Notify(Notify::Log { level, message }) => Some((*level, message.as_str())),
            _ => None,
        })
        .collect();
    assert!(logs.contains(&(LogLevel::Info, "hello from stdout")), "{logs:?}");
    assert!(logs.contains(&(LogLevel::Warn, "hello from stderr")), "{logs:?}");
    drop(h);
    let mut h = Host::start(hello(&d, &c, sha, &[], false)).ready();
    assert_eq!(h.json("load", ""), "Some(Ok(\"v1\"))");
}

fn trap(command: &str) -> (ext_proto::Trap, Duration) {
    let d = dir(command);
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &[], false)).ready();
    let t = Instant::now();
    h.command(command, "");
    let trap = loop {
        match h.next() {
            ToApp::Trap(t) => break t,
            ToApp::Notify(Notify::Log { .. }) => {}
            other => panic!("{other:?}"),
        }
    };
    let took = t.elapsed();
    assert!(!h.exit().success());
    (trap, took)
}

#[test]
fn a_guest_that_never_returns_is_stopped() {
    let (t, took) = trap("spin");
    assert!(t.message.contains("ran for more than 2000 ms"), "{}", t.message);
    assert!(took < Duration::from_secs(10), "{took:?}");
}

#[test]
fn a_guest_that_eats_memory_is_stopped() {
    let (t, _) = trap("grow");
    assert!(!t.message.is_empty());
}

#[test]
fn a_panic_is_a_trap_with_frames() {
    let (t, _) = trap("trap");
    assert!(t.message.contains("unreachable"), "{}", t.message);
    assert!(!t.frames.is_empty());
}

#[test]
fn tampered_compiled_code_is_not_run() {
    let d = dir("tampered");
    let (c, mut sha) = compile(&d);
    sha[0] ^= 1;
    let mut h = Host::start(hello(&d, &c, sha, &[], false));
    assert!(matches!(h.next(), ToApp::Ready { .. }));
    assert!(matches!(h.reader.read::<ToApp>(Instant::now()), Err(ext_proto::Error::Closed)));
    assert!(!h.exit().success());
}

#[test]
fn another_build_is_turned_away() {
    let d = dir("build");
    let (c, sha) = compile(&d);
    let mut h = Host::start(Hello { build: "other".into(), ..hello(&d, &c, sha, &[], false) });
    assert!(matches!(h.next(), ToApp::Mismatch { .. }));
    assert!(!h.exit().success());
}

#[test]
fn a_guest_cannot_reach_files_sockets_or_processes() {
    let d = dir("escape");
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &[], false)).ready();
    // Even a file the exthost process itself may read is out of the guest's reach: WASI has no folders here.
    let answer = h.json("escape", &c.to_string_lossy());
    assert_eq!(answer.lines().count(), 6, "{answer}");
    assert!(answer.lines().all(|l| l.ends_with(": refused")), "{answer}");
}

#[test]
fn cyx_dev_runs_without_the_os_sandbox() {
    let d = dir("dev");
    let (c, sha) = compile(&d);
    let mut h = Host::start(Hello { sandbox: false, ..hello(&d, &c, sha, &[], false) }).ready();
    assert_eq!(h.json("echo", "dev"), "dev");
}

/// Spawn to `Activated`, with and without the sandbox: the "Activation, cold (spawn + sandbox + load)" perf row.
///   cargo test --release -p ext-host --test exthost cold_start -- --ignored --nocapture
#[test]
#[ignore = "a measurement, not a check"]
fn cold_start() {
    let d = dir("cold");
    let (c, sha) = compile(&d);
    for sandbox in [false, true, false, true] {
        let mut times: Vec<Duration> = (0..20)
            .map(|_| {
                let t = Instant::now();
                let h = Host::start(Hello { sandbox, ..hello(&d, &c, sha, &[], false) }).ready();
                let took = t.elapsed();
                drop(h);
                took
            })
            .collect();
        times.sort();
        println!("sandbox {sandbox:5}: median {:?}, p95 {:?}", times[10], times[18]);
    }
}
