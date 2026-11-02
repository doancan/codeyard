//! `cyx test`: the built `extension.wasm` in the real `codeyard-exthost`, against a scripted app. It activates the
//! extension, runs each declared command, draws each declared view (checking its tree as Codeyard does), asks each
//! declared provider once about a sample document and each pull request provider for a list, and answers every call
//! the way the app's broker would: checked against the manifest's permissions with the same rules
//! (`ext_manifest::allow`, `ext_manifest::calls`), with dismissed pickers, files and HTTP responses only from the
//! fixtures, and no programs. A refused call fails the test with its fix; so do a trap, an error from `activate`, a
//! view Codeyard would not draw, a request left unanswered, and an error that is not the fixtures' doing (an
//! exception, a handler that is not registered). After a trap or a timeout the exthost is started again, so one run
//! reports every problem.
//! Each watch the extension starts (API 1.1) gets a document or file of its own to see open, change and be saved, or be
//! created, changed and deleted.

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use ext_api::value::{
    Completion, Denied, Document, DocumentChange, DocumentEvent, EnvInfo, Error as ApiError, FileChange, FileEvent, FileResult, FileStat, FormatOptions,
    HttpHead, Notify, Platform, Position, PullAnswer, PullQuery, PullRemote, Range, Request, Response, Start, StreamEvent, ThemeKind, Why,
};
use ext_manifest::{Code, Diagnostic, Manifest};
use ext_proto::{Hello, Reader, ToApp, ToHost};
use sha2::{Digest, Sha256};

use crate::fixtures::Fixtures;

/// How long one request (or `activate`) may take before the test fails.
const WAIT: Duration = Duration::from_secs(20);

/// `CODEYARD_EXTHOST`, then `codeyard-exthost` beside `cyx`, then on PATH.
pub fn exthost() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CODEYARD_EXTHOST") {
        return Some(PathBuf::from(p));
    }
    let exe = if cfg!(windows) { "codeyard-exthost.exe" } else { "codeyard-exthost" };
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(exe))).filter(|p| p.is_file());
    let on_path = || std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(exe)).find(|p| p.is_file()));
    beside.or_else(on_path)
}

/// What a test run found. `failures` empty means it passed.
#[derive(Debug, Default)]
pub struct Report {
    pub lines: Vec<String>,
    pub failures: Vec<String>,
}

impl Report {
    fn pass(&mut self, s: String) {
        self.lines.push(format!("  ✓ {s}"));
    }

    fn fail(&mut self, s: String) {
        self.lines.push(format!("  ✗ {s}"));
        self.failures.push(s);
    }
}

/// The file `core:editor.open`'s argument names, when it is one the command takes.
fn editor_open_target(args_json: &str) -> Option<String> {
    ext_manifest::file_location(&serde_json::from_str(args_json).unwrap_or_default()).map(|(uri, _)| uri)
}

/// Run the built component of the extension in `dir` (`extension.wasm`) with `exthost`; `fixtures` answer its
/// requests.
pub fn run(dir: &Path, manifest: &Manifest, exthost: &Path, fixtures: &Fixtures) -> Result<Report, String> {
    let dir = &std::fs::canonicalize(dir).map_err(|e| format!("error: {}: {e}", dir.display()))?;
    let wasm = std::fs::read(dir.join(ext_pkg::COMPONENT)).map_err(|e| format!("error: cannot read {}: {e}", ext_pkg::COMPONENT))?;
    let component =
        ext_pkg::check_component(&wasm, manifest).map_err(|errors| errors.iter().map(|d| d.render(ext_pkg::COMPONENT, "")).collect::<Vec<_>>().join("\n"))?;
    // One folder per run: two runs in one process (tests) must not share it.
    static RUNS: AtomicU64 = AtomicU64::new(0);
    let run = RUNS.fetch_add(1, Ordering::Relaxed);
    let work = std::env::temp_dir().join(format!("cyx-test-{}-{}-{run}", std::process::id(), crate::now()));
    std::fs::create_dir_all(work.join("data")).map_err(|e| format!("error: {}: {e}", work.display()))?;
    let result = run_in(&work, dir, manifest, exthost, &wasm, component.interfaces, fixtures);
    let _ = std::fs::remove_dir_all(&work);
    // A TypeScript extension's stack traces name its bundle; show the .ts file and line instead.
    let map = manifest.code.as_ref().filter(|c| c.language != ext_manifest::CodeLanguage::Rust).and_then(|_| crate::sourcemap::load(dir));
    result.map(|mut report| {
        let apply = |s: &mut String| {
            if let Some(map) = &map {
                *s = map.apply(crate::typescript::BUNDLE, s);
            }
            *s = without_build_folders(s);
        };
        report.lines.iter_mut().for_each(apply);
        report.failures.iter_mut().for_each(apply);
        report
    })
}

fn run_in(work: &Path, dir: &Path, manifest: &Manifest, exthost: &Path, wasm: &[u8], granted: Vec<String>, fixtures: &Fixtures) -> Result<Report, String> {
    let compiled = compile(exthost, wasm)?;
    let compiled_path = work.join("extension.cwasm");
    std::fs::write(&compiled_path, &compiled).map_err(|e| format!("error: {e}"))?;
    // `\\?\C:\x` (Windows' canonical form) becomes `file:///C:/x`.
    let path = dir.display().to_string().replace('\\', "/").trim_start_matches("//?/").to_string();
    let folder = if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") };
    let settings = manifest.contributions.settings.iter().map(|s| (s.key.clone(), s.default.as_ref().and_then(|v| serde_json::to_string(v).ok()))).collect();
    let id = manifest.id.as_str();
    let local = |full: &str| full.strip_prefix(&format!("{id}.")).unwrap_or(full).to_string();
    let first = manifest
        .contributions
        .commands
        .first()
        .map(|c| format!("command:{}", local(&c.id)))
        .or_else(|| manifest.contributions.pull_requests.first().map(|p| format!("pull-requests:{}", local(&p.id))))
        .unwrap_or_else(|| "workspace-contains".into());
    let hello = Hello {
        version: ext_proto::VERSION,
        build: ext_proto::BUILD.into(),
        extension: id.to_string(),
        package_dir: dir.display().to_string(),
        compiled: compiled_path.display().to_string(),
        compiled_sha256: Sha256::digest(&compiled).into(),
        data_dir: work.join("data").display().to_string(),
        granted,
        untrusted: false,
        env: EnvInfo {
            codeyard_version: env!("CARGO_PKG_VERSION").into(),
            api_version: ext_api::VERSION.to_string(),
            language: "en".into(),
            theme: ThemeKind::Dark,
            platform: if cfg!(target_os = "macos") {
                Platform::Macos
            } else if cfg!(windows) {
                Platform::Windows
            } else {
                Platform::Linux
            },
            remote: false,
            workspace_trusted: true,
        },
        settings,
        folders: vec![folder.clone()],
        status_items: manifest.status_item_ids(),
        reason: first.clone(),
        // Run it as users will, in the OS sandbox; Windows' AppContainer is not verified yet (#100), so there the
        // author's build runs behind Wasm and the exthost's own checks, as in `cyx dev`.
        sandbox: !cfg!(windows),
    };

    let mut report = Report::default();
    let Some(mut app) = activate(exthost, &hello, manifest, &folder, fixtures, &mut report, true)? else { return Ok(report) };
    for step in steps(manifest, fixtures, &folder) {
        // An `async activate` that rejected: every request fails with its error, so nothing more to learn.
        if app.late_activate {
            break;
        }
        // A trap or a request left unanswered stops this exthost; the rest runs in a new one, so one run reports every
        // problem.
        if app.dead {
            app.stop(&mut report);
            report.lines.push("    (the extension host was started again)".into());
            match activate(exthost, &hello, manifest, &folder, fixtures, &mut report, false)? {
                Some(a) => app = a,
                None => return Ok(report),
            }
        }
        run_step(&mut app, &mut report, step);
    }
    if !app.dead && !app.late_activate {
        app.exercise_watches(&mut report);
    }
    app.stop(&mut report);
    Ok(report)
}

/// Start an exthost and wait for `activate`; none when it failed (reported), so nothing else can run.
fn activate(
    exthost: &Path,
    hello: &Hello,
    manifest: &Manifest,
    folder: &str,
    fixtures: &Fixtures,
    report: &mut Report,
    first: bool,
) -> Result<Option<App>, String> {
    let mut app = App::start(exthost, hello.clone(), manifest.clone(), folder.to_string(), fixtures.clone())?;
    let ok = match app.wait(report, |m| matches!(m, ToApp::Activated(_))) {
        Some(ToApp::Activated(Ok(()))) => true,
        Some(ToApp::Activated(Err(e))) => {
            report.fail(format!("activate failed: {e}"));
            false
        }
        _ if app.dead => false,
        _ => {
            report.fail(format!("activate did not return within {} s", WAIT.as_secs()));
            false
        }
    };
    if !ok {
        app.stop(report);
        return Ok(None);
    }
    if first {
        report.pass(format!("activates ({})", hello.reason));
    }
    Ok(Some(app))
}

/// One request `cyx test` sends, and what it expects back.
struct Step {
    what: String,
    request: Request,
    expect: Expect,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    Command,
    View,
    Edits,
    CodeActions,
    Hover,
    Completions,
    CodeLenses,
    InlayHints,
    Tasks,
    Scm,
    PullRequests,
}

/// Each declared command, view and provider, once: providers with a sample document, the first `files` fixture when
/// there is one (in the provider's first language), else an empty one.
fn steps(manifest: &Manifest, fixtures: &Fixtures, folder: &str) -> Vec<Step> {
    let c = &manifest.contributions;
    let id = manifest.id.as_str();
    let local = |full: &str| full.strip_prefix(&format!("{id}.")).unwrap_or(full).to_string();
    let mut out = Vec::new();
    for cmd in &c.commands {
        let name = local(&cmd.id);
        let args_json = fixtures.command_args(&name);
        out.push(Step { what: format!("command {name}"), request: Request::Command { command: name, args_json }, expect: Expect::Command });
    }
    for v in &c.views {
        let name = local(&v.id);
        out.push(Step { what: format!("view {name}"), request: Request::Render { view: name }, expect: Expect::View });
    }
    let document = |languages: &[String]| {
        let language = languages.first().cloned().unwrap_or_else(|| "plaintext".into());
        let (path, text) = fixtures.files.first().map_or(("untitled".to_string(), String::new()), |(p, t)| (p.clone(), t.clone()));
        Document { uri: format!("{}/{path}", folder.trim_end_matches('/')), language, version: 1, text }
    };
    let whole = |d: &Document| {
        let lines = d.text.split('\n').count() as u32;
        Range { start: Position { line: 0, character: 0 }, end: Position { line: lines, character: 0 } }
    };
    let start = Position { line: 0, character: 0 };
    let mut provider = |what: &str, request: Request, expect: Expect| out.push(Step { what: what.to_string(), request, expect });
    if let Some(p) = c.formatters.first() {
        let options = FormatOptions { tab_size: 4, insert_spaces: true };
        provider("formatter", Request::Format { document: document(&p.languages), range: None, options }, Expect::Edits);
    }
    if let Some(p) = c.code_actions.first() {
        let d = document(&p.languages);
        let range = Range { start, end: start };
        provider("code actions", Request::CodeActions { document: d, range, diagnostics: Vec::new() }, Expect::CodeActions);
    }
    if let Some(p) = c.hover.first() {
        provider("hover", Request::Hover { document: document(&p.languages), position: start }, Expect::Hover);
    }
    if let Some(p) = c.completion.first() {
        provider("completion", Request::Completion { document: document(&p.languages), position: start, trigger: None }, Expect::Completions);
    }
    if let Some(p) = c.code_lenses.first() {
        let d = document(&p.languages);
        let range = whole(&d);
        provider("code lenses", Request::CodeLens { document: d, range }, Expect::CodeLenses);
    }
    if let Some(p) = c.inlay_hints.first() {
        let d = document(&p.languages);
        let range = whole(&d);
        provider("inlay hints", Request::InlayHints { document: d, range }, Expect::InlayHints);
    }
    if !c.tasks.is_empty() {
        provider("tasks", Request::ProvideTasks, Expect::Tasks);
    }
    for p in &c.scm {
        let name = local(&p.id);
        provider(&format!("source control {name}"), Request::ScmStatus { provider: name.clone(), root: folder.to_string() }, Expect::Scm);
    }
    // Each pull request provider lists a remote on its first host (`https://<host>/example/repo.git`).
    for p in &c.pull_requests {
        let name = local(&p.id);
        let host = p.hosts.first().map_or("example.com", |h| h.trim_start_matches("*.")).to_string();
        let remote = PullRemote {
            provider: name.clone(),
            name: "origin".into(),
            url: format!("https://{host}/example/repo.git"),
            host,
            path: "example/repo".into(),
            account: p.auth.clone(),
        };
        provider(&format!("pull requests {name}"), Request::PullRequests { query: PullQuery::List { remote } }, Expect::PullRequests);
    }
    out
}

fn run_step(app: &mut App, report: &mut Report, step: Step) {
    let Step { what, request, expect } = step;
    app.current = what.clone();
    let answer = app.request(report, request);
    let offline = std::mem::take(&mut app.offline);
    match (expect, answer) {
        (_, None) if app.dead => {}
        (_, None) => {
            report.fail(format!("{what} did not answer within {} s", WAIT.as_secs()));
            // It may still be working, or waiting on something that never comes: the next step gets a new exthost.
            app.dead = true;
        }
        (_, Some(Err(ApiError::Denied(d)))) => report.fail(format!("{what} was denied: {}", fix(&d))),
        // Without the network, files or programs a handler may fail; that is an answer. Any other error is a bug:
        // an exception, a handler that is not registered, an answer too large.
        (_, Some(Err(e))) => match offline.first() {
            Some(why) => report.pass(format!("{what} answers (with an error, after {why}: {e})")),
            // A command that needs arguments refuses to run without them: say where they go.
            None if expect == Expect::Command && matches!(e, ApiError::Invalid(_)) && !e.to_string().contains("the result is ") => {
                let id = what.trim_start_matches("command ");
                report
                    .fail(format!("{what} failed: {e}\n        = fix: if it needs arguments, give them in test/fixtures.json: \"commands\": {{ \"{id}\": … }}"))
            }
            None => report.fail(format!("{what} failed: {e}")),
        },
        // The same check Codeyard makes before drawing it, so "draws" means Codeyard draws it.
        (Expect::View, Some(Ok(Response::View(tree)))) => match ext_ui::decode_json(&tree) {
            Ok(_) => report.pass(format!("{what} draws")),
            Err(e) => report.fail(format!("{what} is refused by Codeyard: {e}")),
        },
        (Expect::PullRequests, Some(Ok(Response::PullRequests(PullAnswer::List(l))))) => report.pass(format!("{what} lists ({} open)", l.len())),
        (Expect::Command, Some(Ok(Response::Json(_) | Response::None)))
        | (Expect::Edits, Some(Ok(Response::Edits(_))))
        | (Expect::CodeActions, Some(Ok(Response::CodeActions(_))))
        | (Expect::Hover, Some(Ok(Response::Hover(_))))
        | (Expect::Completions, Some(Ok(Response::Completions(_))))
        | (Expect::CodeLenses, Some(Ok(Response::CodeLenses(_))))
        | (Expect::InlayHints, Some(Ok(Response::InlayHints(_))))
        | (Expect::Tasks, Some(Ok(Response::Tasks(_))))
        | (Expect::Scm, Some(Ok(Response::Scm(_)))) => report.pass(format!("{what} answers")),
        (_, Some(Ok(other))) => report.fail(format!("{what} answered {other:?}")),
    }
}

fn fix(d: &Denied) -> String {
    // A path outside the workspace, an IP address: no manifest line allows it. The SDKs say the same.
    if let Some((_, reason)) = ext_manifest::parse_ungrantable_line(&d.permission) {
        return format!("none: this cannot be granted, {reason}");
    }
    match d.why {
        Why::NotDeclared => ext_manifest::oauth_host_fix(&d.permission)
            .or_else(|| ext_manifest::user_host_fix(&d.permission))
            .unwrap_or_else(|| format!("add to [permissions]:  {}", d.permission)),
        why => format!("{} ({})", d.permission, why.as_str()),
    }
}

fn compile(exthost: &Path, wasm: &[u8]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new(exthost);
    cmd.arg("--compile");
    // Cranelift's incremental cache: a rebuilt extension recompiles only what changed (a TypeScript one embeds the
    // same JavaScript engine every time, so a second run compiles in about half the time).
    if let Ok(dir) = crate::typescript::cache_dir()
        && std::fs::create_dir_all(&dir).is_ok()
    {
        cmd.arg("--cache").arg(dir.join("exthost-incremental.bin"));
    }
    let mut c = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("error: cannot start {}: {e}", exthost.display()))?;
    c.stdin.take().ok_or("error: no stdin")?.write_all(wasm).map_err(|e| format!("error: {e}"))?;
    let out = c.wait_with_output().map_err(|e| format!("error: {e}"))?;
    if !out.status.success() {
        return Err(format!("error: the extension host could not compile {}: {}", ext_pkg::COMPONENT, String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

/// The scripted app: one exthost, its frames read on a thread so every wait has a deadline.
struct App {
    child: Child,
    stdin: ChildStdin,
    frames: Receiver<Result<ToApp, String>>,
    manifest: Manifest,
    folder: String,
    fixtures: Fixtures,
    next_id: u64,
    /// Refusals, rendered with their fix.
    denied: Vec<String>,
    /// The exthost trapped, closed its pipe, or left a request unanswered.
    dead: bool,
    /// What the current request asked for that `cyx test` cannot give offline (a fixture, a file, a program): an
    /// error after one of these is an answer, any other error a failure.
    offline: Vec<String>,
    /// The current request is a user action (`Request::is_user_action`), so it may open links.
    user_action: bool,
    /// `activate` returned a Promise that rejected (the TypeScript SDK logs `activate failed: …`).
    late_activate: bool,
    /// What runs now (`command hello`), to name it in a trap.
    current: String,
    /// Watches the extension started: (id, the glob of a file watch, or none for a document watch, and whether it wants
    /// the text).
    watches: Vec<(u64, Option<String>, bool)>,
    /// The files the extension has diagnostics on now (a uri each), for the limit on how many.
    diagnostic_files: std::collections::BTreeSet<String>,
    /// The reasons `set-diagnostics` was refused for, reported once each.
    refused_diagnostics: std::collections::BTreeSet<String>,
}

impl App {
    fn start(exthost: &Path, hello: Hello, manifest: Manifest, folder: String, fixtures: Fixtures) -> Result<App, String> {
        let mut child = Command::new(exthost)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("error: cannot start {}: {e}", exthost.display()))?;
        let mut stdin = child.stdin.take().ok_or("error: no stdin")?;
        let stdout = child.stdout.take().ok_or("error: no stdout")?;
        let (tx, frames) = channel();
        std::thread::spawn(move || {
            let mut reader = Reader::limited(BufReader::new(stdout), Instant::now());
            loop {
                let frame = reader.read::<ToApp>(Instant::now()).map_err(|e| e.to_string());
                let end = frame.is_err();
                if tx.send(frame).is_err() || end {
                    return;
                }
            }
        });
        ext_proto::write(&mut stdin, &ToHost::Hello(Box::new(hello))).map_err(|e| format!("error: {e}"))?;
        let app = App {
            child,
            stdin,
            frames,
            manifest,
            folder,
            fixtures,
            next_id: 1,
            denied: Vec::new(),
            dead: false,
            offline: Vec::new(),
            user_action: false,
            late_activate: false,
            current: "activate".into(),
            watches: Vec::new(),
            diagnostic_files: Default::default(),
            refused_diagnostics: Default::default(),
        };
        match app.frames.recv_timeout(WAIT) {
            Ok(Ok(ToApp::Ready { .. })) => Ok(app),
            Ok(Ok(ToApp::Mismatch { build, .. })) => Err(format!(
                "error: the extension host is from another build ({build}, cyx is {})\n   = fix: use the codeyard-exthost that came with this cyx, or set CODEYARD_EXTHOST",
                ext_proto::BUILD
            )),
            other => Err(format!("error: the extension host did not start: {other:?}")),
        }
    }

    fn send(&mut self, m: &ToHost) {
        let _ = ext_proto::write(&mut self.stdin, m);
    }

    /// Send a request and wait for its answer, answering the calls it makes on the way.
    fn request(&mut self, report: &mut Report, request: Request) -> Option<Result<Response, ApiError>> {
        let id = self.next_id;
        self.next_id += 1;
        self.offline.clear();
        self.user_action = request.is_user_action();
        self.send(&ToHost::Handle { id, deadline_ms: 0, request });
        let answer = self.wait(report, |m| matches!(m, ToApp::Notify(Notify::Respond { id: i, .. }) if *i == id));
        self.user_action = false;
        match answer? {
            ToApp::Notify(Notify::Respond { result, .. }) => Some(result),
            _ => None,
        }
    }

    /// Answer frames until one matches `done`; none on a trap, a closed pipe or the deadline.
    fn wait(&mut self, report: &mut Report, done: impl Fn(&ToApp) -> bool) -> Option<ToApp> {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let frame = match self.frames.recv_timeout(left) {
                Ok(Ok(f)) => f,
                Ok(Err(e)) => {
                    report.fail(format!("the extension host stopped: {e}"));
                    self.dead = true;
                    return None;
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
            };
            if done(&frame) {
                return Some(frame);
            }
            match frame {
                ToApp::Start { ticket, call } => {
                    let (events, completion) = self.answer(report, call);
                    for event in events {
                        self.send(&ToHost::Event { ticket, event });
                    }
                    self.send(&ToHost::Complete { ticket, completion });
                }
                ToApp::Notify(Notify::Log { message, .. }) => {
                    // The TypeScript SDK refuses a call to an interface the manifest does not grant (the component
                    // does not import it) and logs it; it counts as a refusal.
                    if message.starts_with("denied: ") {
                        self.denied.push(message.clone());
                    }
                    report.lines.push(format!("    log: {message}"));
                    // An `async activate` that rejected after its first `await`: activation failed.
                    if let Some(why) = message.strip_prefix("activate failed: ")
                        && !self.late_activate
                    {
                        self.late_activate = true;
                        report.fail(format!("activate failed: {why}"));
                    }
                }
                ToApp::Notify(Notify::SetDiagnostics { uri, diagnostics }) => self.set_diagnostics(report, &uri, &diagnostics),
                ToApp::Notify(Notify::ClearDiagnostics { uri }) => match uri {
                    Some(u) => {
                        self.diagnostic_files.remove(&u);
                    }
                    None => self.diagnostic_files.clear(),
                },
                ToApp::Notify(Notify::WatchDocuments { id, text }) => self.watches.push((id, None, text)),
                ToApp::Notify(Notify::WatchFiles { id, glob }) => self.watches.push((id, Some(glob), false)),
                ToApp::Notify(Notify::StopWatch { id }) => self.watches.retain(|(w, ..)| *w != id),
                ToApp::Trap(t) => {
                    report.fail(format!("{}: {}", self.current, trap_message(&t)));
                    self.dead = true;
                    return None;
                }
                _ => {}
            }
        }
    }

    /// What the app's broker would answer, with the same checks (`ext_manifest`'s), in a sandbox: files only from
    /// the fixtures, the network only through them, no programs. The events streamed before the completion come
    /// first.
    fn answer(&mut self, report: &mut Report, call: Start) -> (Vec<StreamEvent>, Completion) {
        if let Some(e) = self.invalid(&call) {
            return (Vec::new(), Completion::Failed(e));
        }
        if let Some(line) = self.refused(&call) {
            let what = call.wit_name();
            let denied = Denied { permission: line, why: Why::NotDeclared };
            let why = if ext_manifest::parse_ungrantable_line(&denied.permission).is_some() {
                "is refused, and no permission can allow it"
            } else {
                "needs a permission the manifest does not declare"
            };
            let d = Diagnostic::new(Code::NotDeclared, format!("{what} {why}")).fix(fix(&denied));
            self.denied.push(d.render(ext_manifest::FILE_NAME, ""));
            return (Vec::new(), Completion::Failed(ApiError::Denied(denied)));
        }
        let request = match &call {
            Start::Fetch { request } | Start::AuthFetch { request, .. } => Some(request),
            _ => None,
        };
        if let Some(r) = request {
            return match self.fixtures.find(&r.method, &r.url) {
                Some(f) => {
                    let events = vec![StreamEvent::HttpHead(HttpHead { status: f.status, headers: f.headers.clone() }), StreamEvent::Data(f.body.clone())];
                    (events, Completion::Done)
                }
                None => {
                    let file = self.fixtures.source.as_ref().map_or_else(|| crate::fixtures::DEFAULT.to_string(), |p| p.display().to_string());
                    self.offline.push(format!("no fixture for {} {}", r.method, r.url));
                    let msg = format!("cyx test does not reach the network and has no fixture for {} {}; add one to {file}", r.method, r.url);
                    (Vec::new(), Completion::Failed(ApiError::Other(msg)))
                }
            };
        }
        let done = |c: Completion| (Vec::new(), c);
        done(match call {
            Start::ShowNotice { message, .. } => {
                report.lines.push(format!("    notice: {message}"));
                Completion::Chosen(Vec::new())
            }
            Start::QuickPick { .. } => Completion::Chosen(Vec::new()),
            Start::InputBox { .. } | Start::SecretGet { .. } | Start::ReadClipboard => Completion::Text(None),
            Start::Execute { .. } => Completion::Json("null".into()),
            Start::ActiveDocument | Start::ReadDocument { .. } => Completion::Document(None),
            Start::Selections => Completion::Ranges(Vec::new()),
            Start::ReadFile { uri } => match self.file(&uri) {
                Some(text) => Completion::Bytes(text.into_bytes()),
                None => Completion::Failed(self.missing(uri)),
            },
            Start::Stat { uri } => match self.file(&uri) {
                Some(text) => Completion::Stat(FileStat { is_dir: false, size: text.len() as u64, modified: 0 }),
                None => Completion::Failed(self.missing(uri)),
            },
            Start::ReadMany { uris } => Completion::Files(
                uris.into_iter()
                    .map(|uri| {
                        let contents = match self.file(&uri) {
                            Some(text) if self.allows_read(&uri) => Ok(text.into_bytes()),
                            Some(_) => Err(ApiError::Denied(Denied { permission: format!(r#"read = ["{}"]"#, self.rel(&uri)), why: Why::NotDeclared })),
                            None => Err(self.missing(uri.clone())),
                        };
                        FileResult { uri, contents }
                    })
                    .collect(),
            ),
            Start::FindFiles { glob, max } => {
                let p = &self.manifest.permissions;
                let found =
                    self.fixtures.files.iter().map(|(path, _)| path).filter(|path| ext_manifest::glob::path_matches(&glob, path) && p.allows_read(path));
                Completion::Uris(found.take(max as usize).map(|path| format!("{}/{path}", self.folder.trim_end_matches('/'))).collect())
            }
            Start::Session { provider, .. } => Completion::Account(ext_api::value::Account { provider, label: "cyx-test".into() }),
            Start::OpenExternal { .. } if !self.user_action => {
                Completion::Failed(ApiError::Denied(Denied { permission: "a user action (a command or a link click)".into(), why: Why::NotDeclared }))
            }
            Start::Spawn { command } => {
                self.offline.push(format!("running {}", command.program));
                Completion::Failed(ApiError::Other("cyx test does not run programs; use the mock host's process fixtures in unit tests".into()))
            }
            _ => Completion::Done,
        })
    }

    /// `languages.set-diagnostics`, with the app's rules (`ext_manifest::calls`): refused, or with messages it would
    /// drop, the diagnostics never show, so each is a failure.
    fn set_diagnostics(&mut self, report: &mut Report, uri: &str, diagnostics: &[ext_api::value::Diagnostic]) {
        let languages = &self.manifest.contributions.languages;
        let language = ext_manifest::language_id(languages, &ext_manifest::normalize(uri).unwrap_or_default());
        let declared: Vec<Vec<String>> = self.manifest.contributions.diagnostics.iter().map(|d| d.languages.clone()).collect();
        let reachable = ext_manifest::relative(std::slice::from_ref(&self.folder), uri).is_some();
        let others = self.diagnostic_files.iter().filter(|u| u.as_str() != uri).count();
        if let Err(why) = ext_manifest::calls::check_diagnostics_target(Some(&declared), &language, reachable, others) {
            let fix = if declared.is_empty() {
                format!("\n        = fix: add [[diagnostics]] with languages = [\"{language}\"] to {}", ext_manifest::FILE_NAME)
            } else if why.contains("does not list its language") {
                format!("\n        = fix: add \"{language}\" to the languages of [[diagnostics]]")
            } else {
                String::new()
            };
            // Past the limit every further file is refused for the same reason: report it once.
            if self.refused_diagnostics.insert(why.clone()) {
                report.fail(format!("set-diagnostics on {uri} is refused by Codeyard: {why}{fix}"));
            }
            return;
        }
        let shown: Vec<_> = diagnostics.iter().filter_map(|d| ext_manifest::calls::shown_diagnostic_message(&d.message)).collect();
        if shown.len() < diagnostics.len() {
            report.fail(format!(
                "set-diagnostics on {uri}: Codeyard drops {} of {} diagnostics, whose message is empty or has bidirectional or zero-width characters",
                diagnostics.len() - shown.len(),
                diagnostics.len()
            ));
        }
        if shown.iter().any(|(_, cut)| *cut) {
            report.lines.push(format!("    set-diagnostics on {uri}: a message is cut at {} characters", ext_manifest::limits::DIAGNOSTIC_MESSAGE_LEN));
        }
        if shown.is_empty() {
            self.diagnostic_files.remove(uri);
        } else {
            self.diagnostic_files.insert(uri.to_string());
        }
    }

    /// The workspace-relative path of `uri` in the folder (`ext_manifest::relative`).
    fn rel(&self, uri: &str) -> String {
        ext_manifest::relative(std::slice::from_ref(&self.folder), uri).unwrap_or_else(|| uri.to_string())
    }

    fn allows_read(&self, uri: &str) -> bool {
        ext_manifest::relative(std::slice::from_ref(&self.folder), uri).is_some_and(|r| self.manifest.permissions.allows_read(&r))
    }

    /// The `files` fixture at `uri`.
    fn file(&self, uri: &str) -> Option<String> {
        let rel = ext_manifest::relative(std::slice::from_ref(&self.folder), uri)?;
        self.fixtures.file(&rel).map(str::to_string)
    }

    /// A file the fixtures do not have: not found, as offline as a missing HTTP fixture.
    fn missing(&mut self, uri: String) -> ApiError {
        self.offline.push(format!("no file {} in the fixtures", self.rel(&uri)));
        ApiError::NotFound(uri)
    }

    /// The `invalid` error the app answers before any permission check: a URL Codeyard never fetches
    /// (`ext_manifest::calls::check_url`), an `authorization` header on a signed request.
    fn invalid(&self, call: &Start) -> Option<ApiError> {
        if let Start::Execute { command, args_json } = call
            && command == "core:editor.open"
            && editor_open_target(args_json).is_none()
        {
            return ext_manifest::core_command(command).map(|c| ApiError::Invalid(c.bad_argument()));
        }
        let request = match call {
            Start::Fetch { request } | Start::AuthFetch { request, .. } => request,
            _ => return None,
        };
        // An IP address is refused as a denial instead (`refused`).
        if let Err(e) = ext_manifest::calls::check_url(&request.url)
            && let Ok(message) = e.refusal()
        {
            return Some(ApiError::Invalid(message));
        }
        let signed = matches!(call, Start::AuthFetch { .. });
        (signed && request.headers.iter().any(|(n, _)| n.trim().eq_ignore_ascii_case("authorization")))
            .then(|| ApiError::Invalid("the authorization header is added by Codeyard; remove it from the request".into()))
    }

    /// The manifest line that would allow `call`, when the broker refuses it.
    fn refused(&self, call: &Start) -> Option<String> {
        let p = &self.manifest.permissions;
        let folders = std::slice::from_ref(&self.folder);
        // A path outside the folder (`..`, `%2e%2e`, another folder) is refused whatever the globs say.
        let path = |uri: &str, kind: &str, ok: &dyn Fn(&str) -> bool| match ext_manifest::relative(folders, uri) {
            None => Some(ext_manifest::outside_line(kind, uri)),
            Some(rel) if !ok(&rel) => Some(format!(r#"{kind} = ["{rel}"]"#)),
            Some(_) => None,
        };
        match call {
            Start::ReadFile { uri } | Start::Stat { uri } => path(uri, "read", &|r| p.allows_read(r)),
            Start::FindFiles { glob, .. } if p.read.is_empty() => Some(format!(r#"read = ["{glob}"]"#)),
            Start::WriteFile { uri, .. } | Start::Delete { uri } | Start::CreateDirectory { uri } => path(uri, "write", &|r| p.allows_write(r)),
            Start::Rename { from, to } => path(from, "write", &|r| p.allows_write(r)).or_else(|| path(to, "write", &|r| p.allows_write(r))),
            // An IP address: no `net` entry can allow it.
            Start::Fetch { request } | Start::AuthFetch { request, .. } if ext_manifest::calls::check_url(&request.url).is_err() => {
                ext_manifest::calls::check_url(&request.url).err().and_then(|e| e.refusal().err())
            }
            // `net-from-setting`: the host in the setting's default stands in for the one the user allows.
            Start::Fetch { request } => {
                let host = host_of(&request.url);
                let allowed = p.allows_net(&host) || self.setting_host().is_some_and(|h| h == host);
                let line = if p.net_from_setting.is_some() { ext_manifest::user_host_line(&host) } else { format!(r#"net = ["{host}"]"#) };
                (!allowed).then_some(line)
            }
            Start::Session { provider, .. } | Start::AuthFetch { provider, .. } if !p.auth.contains(provider) => Some(format!(r#"auth = ["{provider}"]"#)),
            // As the app does: an account signs requests to its own hosts only.
            Start::AuthFetch { provider, request } => {
                let host = host_of(&request.url);
                ext_manifest::account_hosts(&self.manifest, provider)
                    .filter(|hosts| !ext_manifest::net_allows(hosts, &host))
                    .map(|_| ext_manifest::oauth_host_line(provider, &host))
            }
            Start::Spawn { command } if !p.allows_run(&command.program) => Some(format!(r#"run = ["{}"]"#, command.program)),
            // A core command is listed in `permissions.commands`; `core:editor.open` also needs `read` on its file.
            Start::Execute { command, args_json } if command.starts_with("core:") => {
                if !p.commands.contains(command) {
                    return Some(format!(r#"commands = ["{command}"]"#));
                }
                let uri = (command == "core:editor.open").then(|| editor_open_target(args_json)).flatten()?;
                path(&uri, "read", &|r| p.allows_read(r))
            }
            _ => None,
        }
    }

    /// Stop the exthost; its refusals join the report (every refusal fails the test, even one the extension caught).
    fn stop(&mut self, report: &mut Report) {
        self.shutdown();
        // The same refusal, repeated, is told once.
        let mut seen = std::collections::HashSet::new();
        for d in std::mem::take(&mut self.denied).into_iter().filter(|d| seen.insert(d.clone())) {
            report.failures.push(d.clone());
            report.lines.push(d);
        }
    }

    /// The host named by the default of the setting `net-from-setting` names, if any.
    fn setting_host(&self) -> Option<String> {
        let key = self.manifest.permissions.net_from_setting.as_ref()?;
        let setting = self.manifest.contributions.settings.iter().find(|s| &s.key == key)?;
        let value = setting.default.as_ref()?.as_str()?.trim();
        let value = if value.contains("://") { value.to_string() } else { format!("https://{value}") };
        Some(host_of(&value)).filter(|h| !h.is_empty())
    }

    /// Give each watch the extension started a file of its own: a document watch sees it open, change and be saved, a
    /// file watch sees it created, changed and deleted. Each then answers a request, so a trap in a handler fails here.
    fn exercise_watches(&mut self, report: &mut Report) {
        let first_read = self.manifest.permissions.read.first().cloned().unwrap_or_default();
        for (id, glob, text) in std::mem::take(&mut self.watches) {
            if self.dead {
                return;
            }
            let path = example_path(glob.as_deref().unwrap_or(&first_read));
            if !self.manifest.permissions.allows_read(&path) {
                report.lines.push(format!("    watch {id}: no example file that `read` allows; not exercised"));
                continue;
            }
            let uri = format!("{}/{path}", self.folder.trim_end_matches('/'));
            let label = match &glob {
                Some(glob) => {
                    for change in [FileChange::Created, FileChange::Changed, FileChange::Deleted] {
                        self.send(&ToHost::Files { watch: id, events: vec![FileEvent { change, uri: uri.clone() }] });
                    }
                    format!("file watch {glob} handles {path} created, changed and deleted")
                }
                None => {
                    for (version, change) in [(1, DocumentChange::Opened), (2, DocumentChange::Changed), (2, DocumentChange::Saved)] {
                        let text = text.then(|| "TODO: example\n".to_string());
                        self.send(&ToHost::Documents {
                            watch: id,
                            events: vec![DocumentEvent { change, uri: uri.clone(), language: "plaintext".into(), version, text }],
                        });
                    }
                    format!("document watch handles {path} opened, changed and saved")
                }
            };
            // Answered at once by the SDKs and changes nothing: the events before it have been handled.
            match self.request(report, Request::CancelProgress { id: u64::MAX }) {
                Some(_) => report.pass(label),
                None if self.dead => {}
                None => report.fail(format!("{label}: the extension did not answer afterwards within {} s", WAIT.as_secs())),
            }
        }
    }

    fn shutdown(&mut self) {
        self.send(&ToHost::Shutdown);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A trap with the frames that show where: the Rust runtime's panic handling (`abort`, `std::panicking`,
/// `core::panicking`, the indexing that panicked, …) sits innermost and is left out, so the extension's own frames come
/// first.
fn trap_message(t: &ext_proto::Trap) -> String {
    const RUNTIME: &[&str] = &["std::", "core::", "alloc::", "__rustc::", "__rust", "rust_panic", "rust_begin_unwind", "panic_abort::", "panic_unwind::"];
    let runtime = |f: &str| {
        let name = f.trim_start_matches(['<', '&']);
        name == "abort" || RUNTIME.iter().any(|p| name.starts_with(p)) || [" as core::", " as alloc::", " as std::"].iter().any(|p| name.contains(p))
    };
    // A trap with no frame of the extension's own keeps them all.
    let hidden = match t.frames.iter().take_while(|f| runtime(f)).count() {
        n if n == t.frames.len() => 0,
        n => n,
    };
    let mut msg = format!("the extension trapped: {}", t.message);
    for f in t.frames.iter().skip(hidden).take(8) {
        msg.push_str(&format!("\n        at {f}"));
    }
    if hidden > 0 {
        msg.push_str(&format!("\n        ({hidden} frames of the Rust runtime's panic handling left out)"));
    }
    msg
}

/// The JavaScript engine's `initializer.js` without the temporary folder it was built in (`/var/folders/…/T/…/`).
fn without_build_folders(s: &str) -> String {
    const FILE: &str = "initializer.js:";
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find(FILE) {
        let head = &rest[..at];
        // The path starts after the last space, `(` or `@` before the file name.
        let path_start = head.rfind([' ', '(', '@', '\n']).map_or(0, |i| i + 1);
        out.push_str(&head[..path_start]);
        out.push_str(FILE);
        rest = &rest[at + FILE.len()..];
    }
    out.push_str(rest);
    out
}

/// A workspace path that `glob` matches, for a watch to see: each wildcard part becomes `example`.
fn example_path(glob: &str) -> String {
    let parts: Vec<String> =
        glob.trim_start_matches("./").split('/').filter(|p| *p != "**" && !p.is_empty()).map(|p| p.replace('*', "example").replace('?', "x")).collect();
    if parts.is_empty() { "example.txt".into() } else { parts.join("/") }
}

/// The host of a URL that passed `ext_manifest::calls::check_url` (others are refused before anyone asks).
fn host_of(url: &str) -> String {
    ext_manifest::calls::check_url(url).map(|t| t.host).unwrap_or_default()
}
