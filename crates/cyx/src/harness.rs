//! `cyx test`: the built `extension.wasm` in the real `codeyard-exthost`, against a scripted app. It activates the
//! extension, runs each declared command and draws each declared view, and answers every call the way the app's broker
//! would: checked against the manifest's permissions, with dismissed pickers and no network, files or programs. A
//! refused call fails the test with its fix; so do a trap, an error from `activate` and a request left unanswered.

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use ext_api::value::{Completion, Denied, EnvInfo, Error as ApiError, Notify, Platform, Request, Response, Start, ThemeKind, Why};
use ext_manifest::{Code, Diagnostic, Manifest};
use ext_proto::{Hello, Reader, ToApp, ToHost};
use sha2::{Digest, Sha256};

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

/// Run the built component of the extension in `dir` (`extension.wasm`) with `exthost`.
pub fn run(dir: &Path, manifest: &Manifest, exthost: &Path) -> Result<Report, String> {
    let dir = &std::fs::canonicalize(dir).map_err(|e| format!("error: {}: {e}", dir.display()))?;
    let wasm = std::fs::read(dir.join(ext_pkg::COMPONENT)).map_err(|e| format!("error: cannot read {}: {e}", ext_pkg::COMPONENT))?;
    let component =
        ext_pkg::check_component(&wasm, manifest).map_err(|errors| errors.iter().map(|d| d.render(ext_pkg::COMPONENT, "")).collect::<Vec<_>>().join("\n"))?;
    // One folder per run: two runs in one process (tests) must not share it.
    static RUNS: AtomicU64 = AtomicU64::new(0);
    let run = RUNS.fetch_add(1, Ordering::Relaxed);
    let work = std::env::temp_dir().join(format!("cyx-test-{}-{}-{run}", std::process::id(), crate::now()));
    std::fs::create_dir_all(work.join("data")).map_err(|e| format!("error: {}: {e}", work.display()))?;
    let result = run_in(&work, dir, manifest, exthost, &wasm, component.interfaces);
    let _ = std::fs::remove_dir_all(&work);
    // A TypeScript extension's stack traces name its bundle; show the .ts file and line instead.
    let map = manifest.code.as_ref().filter(|c| c.language != ext_manifest::CodeLanguage::Rust).and_then(|_| crate::sourcemap::load(dir));
    result.map(|mut report| {
        if let Some(map) = map {
            let apply = |s: &mut String| *s = map.apply(crate::typescript::BUNDLE, s);
            report.lines.iter_mut().for_each(apply);
            report.failures.iter_mut().for_each(apply);
        }
        report
    })
}

fn run_in(work: &Path, dir: &Path, manifest: &Manifest, exthost: &Path, wasm: &[u8], granted: Vec<String>) -> Result<Report, String> {
    let compiled = compile(exthost, wasm)?;
    let compiled_path = work.join("extension.cwasm");
    std::fs::write(&compiled_path, &compiled).map_err(|e| format!("error: {e}"))?;
    // `\\?\C:\x` (Windows' canonical form) becomes `file:///C:/x`.
    let path = dir.display().to_string().replace('\\', "/").trim_start_matches("//?/").to_string();
    let folder = if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") };
    let settings = manifest.contributions.settings.iter().map(|s| (s.key.clone(), s.default.as_ref().and_then(|v| serde_json::to_string(v).ok()))).collect();
    let id = manifest.id.as_str();
    let local = |full: &str| full.strip_prefix(&format!("{id}.")).unwrap_or(full).to_string();
    let first = manifest.contributions.commands.first().map(|c| format!("command:{}", local(&c.id))).unwrap_or_else(|| "workspace-contains".into());
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
        reason: first.clone(),
    };

    let mut app = App::start(exthost, hello, manifest.clone(), folder)?;
    let mut report = Report::default();
    match app.wait(&mut report, |m| matches!(m, ToApp::Activated(_))) {
        Some(ToApp::Activated(Ok(()))) => report.pass(format!("activates ({first})")),
        Some(ToApp::Activated(Err(e))) => report.fail(format!("activate failed: {e}")),
        _ => report.fail("activate did not return".into()),
    }
    let commands = manifest.contributions.commands.iter().map(|c| (true, local(&c.id)));
    let views = manifest.contributions.views.iter().map(|v| (false, local(&v.id)));
    for (command, name) in commands.chain(views) {
        // After a trap or a failed activate the rest cannot run.
        if app.dead || !report.failures.is_empty() {
            break;
        }
        let request = if command { Request::Command { command: name.clone(), args_json: String::new() } } else { Request::Render { view: name.clone() } };
        let what = if command { format!("command {name}") } else { format!("view {name}") };
        match (command, app.request(&mut report, request)) {
            (_, None) if app.dead => {}
            (_, None) => report.fail(format!("{what} did not answer within {} s", WAIT.as_secs())),
            (_, Some(Err(ApiError::Denied(d)))) => report.fail(format!("{what} was denied: {}", fix(&d))),
            (true, Some(Ok(Response::Json(_) | Response::None))) => report.pass(format!("{what} answers")),
            // Without network, files or programs a command may fail; it still answered.
            (true, Some(Err(e))) => report.pass(format!("{what} answers (with an error: {e})")),
            (false, Some(Ok(Response::View(tree)))) => match serde_json::from_str::<serde_json::Value>(&tree) {
                Ok(_) => report.pass(format!("{what} draws")),
                Err(e) => report.fail(format!("{what} drew something that is not JSON: {e}")),
            },
            (false, Some(Err(e))) => report.fail(format!("{what} failed: {e}")),
            (_, Some(Ok(other))) => report.fail(format!("{what} answered {other:?}")),
        }
    }
    app.shutdown();
    // Every refusal fails the test, even one the extension caught.
    for d in std::mem::take(&mut app.denied) {
        report.failures.push(d.clone());
        report.lines.push(d);
    }
    Ok(report)
}

fn fix(d: &Denied) -> String {
    match d.why {
        Why::NotDeclared => format!("add to [permissions]:  {}", d.permission),
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
    next_id: u64,
    /// Refusals, rendered with their fix.
    denied: Vec<String>,
    /// The exthost trapped or closed its pipe.
    dead: bool,
}

impl App {
    fn start(exthost: &Path, hello: Hello, manifest: Manifest, folder: String) -> Result<App, String> {
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
        let app = App { child, stdin, frames, manifest, folder, next_id: 1, denied: Vec::new(), dead: false };
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
        self.send(&ToHost::Handle { id, deadline_ms: 0, request });
        match self.wait(report, |m| matches!(m, ToApp::Notify(Notify::Respond { id: i, .. }) if *i == id))? {
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
                    let completion = self.answer(report, call);
                    self.send(&ToHost::Complete { ticket, completion });
                }
                ToApp::Notify(Notify::Log { message, .. }) => {
                    // The TypeScript SDK refuses a call to an interface the manifest does not grant (the component
                    // does not import it) and logs it; it counts as a refusal.
                    if message.starts_with("denied: ") {
                        self.denied.push(message.clone());
                    }
                    report.lines.push(format!("    log: {message}"));
                }
                ToApp::Trap(t) => {
                    let mut msg = format!("the extension trapped: {}", t.message);
                    for f in t.frames.iter().take(8) {
                        msg.push_str(&format!("\n        at {f}"));
                    }
                    report.fail(msg);
                    self.dead = true;
                    return None;
                }
                _ => {}
            }
        }
    }

    /// What the app's broker would answer, in a sandbox: no network, files or programs.
    fn answer(&mut self, report: &mut Report, call: Start) -> Completion {
        let p = &self.manifest.permissions;
        let rel = |uri: &str| uri.strip_prefix(&self.folder).unwrap_or(uri).trim_start_matches('/').to_string();
        let refused = match &call {
            Start::ReadFile { uri } | Start::Stat { uri } if !p.allows_read(&rel(uri)) => Some(format!(r#"read = ["{}"]"#, rel(uri))),
            Start::FindFiles { glob, .. } if p.read.is_empty() => Some(format!(r#"read = ["{glob}"]"#)),
            Start::WriteFile { uri, .. } | Start::Delete { uri } | Start::CreateDirectory { uri } if !p.allows_write(&rel(uri)) => {
                Some(format!(r#"write = ["{}"]"#, rel(uri)))
            }
            Start::Fetch { request } => {
                let host = host_of(&request.url);
                (!p.allows_net(&host)).then(|| format!(r#"net = ["{host}"]"#))
            }
            Start::Session { provider, .. } | Start::AuthFetch { provider, .. } if !p.auth.contains(provider) => Some(format!(r#"auth = ["{provider}"]"#)),
            Start::Spawn { command } if !p.allows_run(&command.program) => Some(format!(r#"run = ["{}"]"#, command.program)),
            _ => None,
        };
        if let Some(line) = refused {
            let what = call.wit_name();
            let d = Diagnostic::new(Code::NotDeclared, format!("{what} needs a permission the manifest does not declare"))
                .fix(format!("add to [permissions]:  {line}"));
            self.denied.push(d.render(ext_manifest::FILE_NAME, ""));
            return Completion::Failed(ApiError::Denied(Denied { permission: line, why: Why::NotDeclared }));
        }
        let offline = |what: &str| Completion::Failed(ApiError::Other(format!("cyx test does not {what}; use the mock host's fixtures in unit tests")));
        match call {
            Start::ShowNotice { message, .. } => {
                report.lines.push(format!("    notice: {message}"));
                Completion::Chosen(Vec::new())
            }
            Start::QuickPick { .. } => Completion::Chosen(Vec::new()),
            Start::InputBox { .. } | Start::SecretGet { .. } | Start::ReadClipboard => Completion::Text(None),
            Start::Execute { .. } => Completion::Json("null".into()),
            Start::ActiveDocument | Start::ReadDocument { .. } => Completion::Document(None),
            Start::Selections => Completion::Ranges(Vec::new()),
            Start::ReadFile { uri } | Start::Stat { uri } => Completion::Failed(ApiError::NotFound(uri)),
            Start::ReadMany { uris } => {
                Completion::Files(uris.into_iter().map(|uri| ext_api::value::FileResult { contents: Err(ApiError::NotFound(uri.clone())), uri }).collect())
            }
            Start::FindFiles { .. } => Completion::Uris(Vec::new()),
            Start::Session { provider, .. } => Completion::Account(ext_api::value::Account { provider, label: "cyx-test".into() }),
            Start::Fetch { .. } | Start::AuthFetch { .. } => offline("reach the network"),
            Start::Spawn { .. } => offline("run programs"),
            _ => Completion::Done,
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

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}
