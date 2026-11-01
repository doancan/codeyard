//! codeyard-exthost as the app sees it: a child process speaking `ext-proto`, running testdata/probe.wasm
//! (tests/guests/probe). Each test plays the app's side of the conversation.

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use ext_api::value::{
    Completion, DocumentChange, DocumentEvent, EnvInfo, FileChange, FileEvent, LogLevel, Notify, Platform, Request, Response, Start, ThemeKind,
};
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
    compile_wasm(dir, PROBE)
}

fn compile_wasm(dir: &Path, wasm: &[u8]) -> (PathBuf, [u8; 32]) {
    let mut c = Command::new(EXE).arg("--compile").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(wasm).unwrap();
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
        status_items: vec!["state".into()],
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

/// An answer over `CALL_BYTES` fails its request with `invalid` and the exthost goes on; before, it was dropped and the
/// request waited until its deadline.
#[test]
fn an_answer_over_the_call_limit_fails_its_request() {
    let d = dir("huge");
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &[], false)).ready();
    let id = h.command("huge", "");
    assert_eq!(h.answer(id).0, Err(ext_api::value::Error::Invalid("the result is 20 MiB; at most 8 MiB".into())));
    assert_eq!(h.json("echo", "[1]"), "[1]");
}

/// `set-status-item` refuses what the app would not show, with the app's words: an id not in `[[status-bar]]`, a text
/// longer than `STATUS_TEXT_LEN`.
#[test]
fn a_status_item_is_checked_as_the_app_checks_it() {
    let d = dir("status");
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &[], false)).ready();
    let id = h.command("status", "");
    let (answer, seen) = h.answer(id);
    assert_eq!(answer, Ok(Response::Json("Ok(())".into())), "{seen:?}");
    assert!(seen.iter().any(|m| matches!(m, ToApp::Notify(Notify::SetStatusItem { item }) if item.id == "state")), "{seen:?}");
    let id = h.command("status-undeclared", "");
    let (answer, seen) = h.answer(id);
    assert!(
        matches!(&answer, Ok(Response::Json(j)) if j.starts_with("Err(") && j.contains("status item") && j.contains("other") && j.contains("is not declared")),
        "{answer:?}"
    );
    assert!(!seen.iter().any(|m| matches!(m, ToApp::Notify(Notify::SetStatusItem { .. }))), "{seen:?}");
}

/// A component built against API 0.1.0 (the probe as it was before 1.0, kept as testdata/probe-0.1.wasm) still runs:
/// `--compile` renames its imports to the current version and it links, activates and answers like a 1.0 build.
#[test]
fn a_component_built_against_0_1_still_runs() {
    let old = include_bytes!("../testdata/probe-0.1.wasm");
    assert!(old.windows(b"codeyard:api/net@0.1.0".len()).any(|w| w == b"codeyard:api/net@0.1.0"), "the fixture is a 0.1.0 build");
    let d = dir("transition");
    let (c, sha) = compile_wasm(&d, old);
    let mut h = Host::start(hello(&d, &c, sha, &[], false)).ready();
    assert_eq!(h.json("echo", "[1,2]"), "[1,2]");
    assert_eq!(h.json("env", ""), ext_api::VERSION.to_string());
    h.send(&ToHost::Shutdown);
    assert!(h.exit().success());
}

/// A component built against API 1.0.0 (the probe as it was before 1.1, kept as testdata/probe-1.0.wasm) runs unchanged
/// on this host: its `@1.0.0` imports bind to 1.1, it has no `events` export, and events sent to it are dropped.
#[test]
fn a_component_built_against_1_0_runs_unchanged() {
    let old = include_bytes!("../testdata/probe-1.0.wasm");
    assert!(old.windows(b"codeyard:api/guest@1.0.0".len()).any(|w| w == b"codeyard:api/guest@1.0.0"), "the fixture is a 1.0.0 build");
    assert!(!old.windows(b"codeyard:api/events@".len()).any(|w| w == b"codeyard:api/events@"));
    let d = dir("api-1-0");
    let (c, sha) = compile_wasm(&d, old);
    let mut h = Host::start(hello(&d, &c, sha, &["workspace"], false)).ready();
    assert_eq!(h.json("echo", "[1,2]"), "[1,2]");
    h.send(&ToHost::Files { watch: 1, events: vec![FileEvent { change: FileChange::Created, uri: "file:///work/a.txt".into() }] });
    assert_eq!(h.json("env", ""), ext_api::VERSION.to_string());
    h.send(&ToHost::Shutdown);
    assert!(h.exit().success());
}

/// `watch` (API 1.1): a watch is announced to the app, refused when `read` is not granted or past the limit, and the
/// app's events reach the guest's `events` export.
#[test]
fn watches_reach_the_app_and_their_events_reach_the_guest() {
    let d = dir("watch");
    let (c, sha) = compile_wasm(&d, include_bytes!("../testdata/probe.wasm"));
    // Without `workspace` (read) granted, the watch is refused in the exthost and the app hears of the denial.
    let mut h = Host::start(hello(&d, &c, sha, &[], false)).ready();
    let id = h.command("watch-docs", "");
    let (answer, other) = h.answer(id);
    assert_eq!(answer, Ok(Response::Json(r#"Err(Error::Denied(Denied { permission: "read", why: Why::RevokedByUser }))"#.into())));
    assert!(other.iter().any(|m| matches!(m, ToApp::Denied { call, .. } if call == "watch/documents")), "{other:?}");
    h.send(&ToHost::Shutdown);
    assert!(h.exit().success());

    let mut h = Host::start(hello(&d, &c, sha, &["workspace", "watch"], false)).ready();
    let id = h.command("watch-docs", "\"text\"");
    let (answer, other) = h.answer(id);
    assert_eq!(answer, Ok(Response::Json("Ok(1)".into())));
    assert!(other.contains(&ToApp::Notify(Notify::WatchDocuments { id: 1, text: true })), "{other:?}");
    let id = h.command("watch-files", "\"src/**/*.rs\"");
    let (answer, other) = h.answer(id);
    assert_eq!(answer, Ok(Response::Json("Ok(2)".into())));
    assert!(other.contains(&ToApp::Notify(Notify::WatchFiles { id: 2, glob: "src/**/*.rs".into() })), "{other:?}");
    // A glob `read` could not have is refused before it reaches the app.
    assert!(h.json("watch-files", "\"../secret\"").starts_with("Err(Error::Invalid("), "a bad glob");

    let doc =
        DocumentEvent { change: DocumentChange::Saved, uri: "file:///work/a.txt".into(), language: "plaintext".into(), version: 3, text: Some("hi".into()) };
    h.send(&ToHost::Documents { watch: 1, events: vec![doc] });
    h.send(&ToHost::Files { watch: 2, events: vec![FileEvent { change: FileChange::Deleted, uri: "file:///work/src/b.rs".into() }] });
    assert_eq!(h.json("watched", ""), "1 saved file:///work/a.txt plaintext v3 Some(\"hi\")\n2 deleted file:///work/src/b.rs\n");

    // Stopping one tells the app; the limit counts only the watches still running.
    let id = h.command("unwatch", "\"2\"");
    let (_, other) = h.answer(id);
    assert!(other.contains(&ToApp::Notify(Notify::StopWatch { id: 2 })), "{other:?}");
    for _ in 1..ext_manifest::limits::WATCHES {
        assert!(h.json("watch-docs", "").starts_with("Ok("));
    }
    assert!(h.json("watch-docs", "").starts_with("Err(Error::Invalid("), "past the limit");
    h.send(&ToHost::Shutdown);
    assert!(h.exit().success());
}

/// `request.hover` and `request.completion` cross the component boundary both ways, with every field of an item.
#[test]
fn hover_and_completion_answer_through_the_guest() {
    use ext_api::value::{CompletionKind, Document, Position};
    let d = dir("hover");
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &[], false)).ready();
    let document = Document { uri: "file:///w/a.toml".into(), language: "toml".into(), version: 3, text: "a = 1\n".into() };
    let position = Position { line: 0, character: 2 };
    h.send(&ToHost::Handle { id: 50, deadline_ms: 0, request: Request::Hover { document: document.clone(), position } });
    match h.answer(50).0 {
        Ok(Response::Hover(Some(hover))) => assert_eq!((hover.contents.as_str(), hover.range), ("**Probe** hover at 0:2", None)),
        other => panic!("{other:?}"),
    }
    h.send(&ToHost::Handle { id: 51, deadline_ms: 0, request: Request::Completion { document, position, trigger: Some(".".into()) } });
    match h.answer(51).0 {
        Ok(Response::Completions(list)) => {
            let labels: Vec<&str> = list.items.iter().map(|i| i.label.as_str()).collect();
            assert_eq!(labels, ["probe-item", "probe-dot"]);
            assert_eq!(list.items[0].kind, CompletionKind::Keyword);
            assert_eq!(list.items[0].kind.lsp(), 14, "the Language Server Protocol's number");
            assert_eq!(list.items[0].documentation.as_deref(), Some("**probe-item** from the probe"));
            assert!(!list.incomplete);
        }
        other => panic!("{other:?}"),
    }
    h.send(&ToHost::Shutdown);
    assert!(h.exit().success());
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
fn a_permission_turned_off_is_refused_from_the_next_call() {
    let d = dir("grants");
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &["net"], false)).ready();
    h.command("fetch", "https://api.example.com/x");
    assert!(matches!(h.next(), ToApp::Start { call: Start::Fetch { .. }, .. }));
    // The user turns `net` off on the extension page: no restart, the next call is refused and the app is told.
    h.send(&ToHost::Grants(Vec::new()));
    let id = h.command("fetch", "https://api.example.com/x");
    let (answer, other) = h.answer(id);
    assert_eq!(answer, Ok(Response::Json("denied net Why::RevokedByUser".into())));
    let reported = other.iter().find_map(|m| match m {
        ToApp::Denied { call, denied } => Some((call.as_str(), denied.permission.as_str(), denied.why)),
        _ => None,
    });
    assert_eq!(reported, Some(("net/fetch", "net", ext_api::value::Why::RevokedByUser)));
    // And on again.
    h.send(&ToHost::Grants(vec!["net".into()]));
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
fn pull_request_queries_reach_the_provider_and_its_forge_calls_go_through_auth() {
    use ext_api::value::{ChangedFile, FileStatus, PullAnswer, PullQuery, PullRemote, StreamEvent};
    let d = dir("pulls");
    let (c, sha) = compile(&d);
    let mut h = Host::start(hello(&d, &c, sha, &["auth"], false)).ready();
    let remote = PullRemote {
        provider: "acme".into(),
        name: "origin".into(),
        url: "git@git.acme.example.com:team/app.git".into(),
        host: "git.acme.example.com".into(),
        path: "team/app".into(),
        account: Some("acme".into()),
    };
    let ask = |h: &mut Host, query: PullQuery| {
        let id = h.next_id;
        h.next_id += 1;
        h.send(&ToHost::Handle { id, deadline_ms: 20_000, request: Request::PullRequests { query } });
        id
    };
    // `list` calls the forge through `auth.fetch`: the app signs it, the guest gets the body.
    let id = ask(&mut h, PullQuery::List { remote: remote.clone() });
    let ticket = match h.next() {
        ToApp::Start { ticket, call: Start::AuthFetch { provider, request } } => {
            assert_eq!((provider.as_str(), request.url.as_str()), ("acme", "https://api.acme.example.com/repos/team/app/pulls"));
            assert!(request.headers.iter().all(|(k, _)| !k.eq_ignore_ascii_case("authorization")));
            ticket
        }
        other => panic!("{other:?}"),
    };
    h.send(&ToHost::Event { ticket, event: StreamEvent::Data(b"7|Fix it|ada|topic|main\n".to_vec()) });
    h.send(&ToHost::Complete { ticket, completion: Completion::Done });
    match h.answer(id).0 {
        Ok(Response::PullRequests(PullAnswer::List(l))) => {
            assert_eq!((l[0].id.as_str(), l[0].reference.as_str(), l[0].title.as_str(), l[0].source_branch.as_str()), ("7", "!7", "Fix it", "topic"))
        }
        other => panic!("{other:?}"),
    }
    // Files and contents come back as typed answers.
    let id = ask(&mut h, PullQuery::Files { remote: remote.clone(), id: "7".into() });
    assert!(matches!(h.answer(id).0, Ok(Response::PullRequests(PullAnswer::Files(f))) if f.len() == 2 && f[1].status == FileStatus::Added));
    let file = ChangedFile { path: "a.txt".into(), old_path: None, status: FileStatus::Modified };
    let id = ask(&mut h, PullQuery::Contents { remote: remote.clone(), id: "7".into(), file });
    match h.answer(id).0 {
        Ok(Response::PullRequests(PullAnswer::Contents(c))) => {
            assert_eq!((c.base.as_deref(), c.head.as_deref()), (Some("one\n"), Some("one\ntwo from a.txt\n")))
        }
        other => panic!("{other:?}"),
    }
    let id = ask(&mut h, PullQuery::Checkout { remote, id: "7".into() });
    match h.answer(id).0 {
        Ok(Response::PullRequests(PullAnswer::Checkout(c))) => assert_eq!((c.r#ref.as_str(), c.branch.as_str()), ("refs/pull/7/head", "pr-7")),
        other => panic!("{other:?}"),
    }
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

/// A source control provider (#144): the host asks for a folder's state and a file's original text, the answers
/// travel back as `response.scm` and `response.text`, and `scm.refresh` reaches the app.
#[test]
fn a_source_control_provider_answers_and_asks_for_a_refresh() {
    let d = dir("scm");
    let (c, sha) = compile_wasm(&d, include_bytes!("../testdata/probe-scm.wasm"));
    let mut h = Host::start(hello(&d, &c, sha, &["workspace"], false)).ready();
    let request = Request::ScmStatus { provider: "toy".into(), root: "file:///work".into() };
    h.send(&ToHost::Handle { id: 100, deadline_ms: 0, request });
    let ticket = match h.next() {
        ToApp::Start { ticket, call: Start::ReadFile { uri } } => {
            assert_eq!(uri, "file:///work/.toyvcs/changes");
            ticket
        }
        other => panic!("{other:?}"),
    };
    h.send(&ToHost::Complete { ticket, completion: Completion::Bytes(b"changes M a.txt\nstaged A b.txt\n".to_vec()) });
    let Ok(Response::Scm(state)) = h.answer(100).0 else { panic!("no state") };
    let files: Vec<(&str, &str, ext_api::value::FileState)> =
        state.groups.iter().flat_map(|g| g.resources.iter().map(move |r| (g.id.as_str(), r.uri.as_str(), r.state))).collect();
    assert_eq!(
        files,
        [("staged", "file:///work/b.txt", ext_api::value::FileState::Added), ("changes", "file:///work/a.txt", ext_api::value::FileState::Modified)]
    );
    assert_eq!(state.input.accept.map(|a| a.command).as_deref(), Some("scm-commit"));

    let request = Request::ScmOriginal { provider: "toy".into(), uri: "file:///work/a.txt".into() };
    h.send(&ToHost::Handle { id: 101, deadline_ms: 0, request });
    let ticket = match h.next() {
        ToApp::Start { ticket, call: Start::ReadFile { uri } } => {
            assert_eq!(uri, "file:///work/.toyvcs/base/a.txt");
            ticket
        }
        other => panic!("{other:?}"),
    };
    h.send(&ToHost::Complete { ticket, completion: Completion::Bytes(b"old".to_vec()) });
    assert_eq!(h.answer(101).0, Ok(Response::Text(Some("old".into()))));

    let request = Request::ScmValidate { provider: "toy".into(), root: "file:///work".into(), value: " ".into() };
    h.send(&ToHost::Handle { id: 102, deadline_ms: 0, request });
    assert_eq!(h.answer(102).0, Ok(Response::Text(Some("Write a message first".into()))));

    let id = h.command("scm-refresh", "");
    let (answer, seen) = h.answer(id);
    assert_eq!(answer, Ok(Response::Json("refreshed".into())));
    assert!(seen.iter().any(|m| matches!(m, ToApp::Notify(Notify::ScmRefresh { provider }) if provider == "toy")), "{seen:?}");
}
