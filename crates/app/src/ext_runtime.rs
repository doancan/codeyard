//! Running code extensions (docs/plugin-architecture.md §3): one `codeyard-exthost` process per active extension,
//! started on its first event, with its component compiled once into the cache. No UI and no permission decisions
//! here: frames from the exthost go to the workbench, which asks the broker (`ext_broker`).
//!
//! The app never links wasmtime: compiling and running happen in the exthost binary that ships beside the app.

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ext_api::value::{EnvInfo, Request, Response};
use ext_proto::{Hello, Reader, ToApp, ToHost};
use sha2::{Digest, Sha256};

/// Three crashes within this window disable the extension until the user acts (§6.3).
pub const CRASH_WINDOW: Duration = Duration::from_secs(60);
pub const CRASHES: usize = 3;
/// An exthost with no call or timer for this long is stopped; the next event starts it again.
pub const IDLE: Duration = Duration::from_secs(5 * 60);

/// `codeyard-exthost` beside the running app (or `CODEYARD_EXTHOST`). Tests find the one cargo built.
pub fn exthost_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CODEYARD_EXTHOST") {
        return Some(PathBuf::from(p));
    }
    let name = if cfg!(windows) { "codeyard-exthost.exe" } else { "codeyard-exthost" };
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    // Beside the app; in a test binary (target/<profile>/deps/), one folder up.
    [dir.join(name), dir.parent()?.join(name)].into_iter().find(|p| p.is_file())
}

#[cfg(test)]
thread_local! {
    static LOAD_WITHOUT_EXTHOST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Tests whose extension never runs (its requests are recorded, not sent) load code extensions on a machine without
/// the exthost or its sandbox, such as a Windows test runner, while the returned guard lives.
#[cfg(test)]
pub fn load_without_exthost_for_test() -> impl Drop {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            LOAD_WITHOUT_EXTHOST.with(|c| c.set(false));
        }
    }
    LOAD_WITHOUT_EXTHOST.with(|c| c.set(true));
    Reset
}

/// Whether code extensions can run in this app: `Err` says why not, naming the extension.
pub fn code_runs_here(id: &dyn std::fmt::Display) -> Result<(), String> {
    #[cfg(test)]
    if LOAD_WITHOUT_EXTHOST.with(|c| c.get()) {
        return Ok(());
    }
    match (exthost_path(), sandbox()) {
        (None, _) => Err(crate::i18n::tf("{} contains code, but codeyard-exthost is missing beside Codeyard", &[id])),
        (Some(_), Err(why)) => Err(crate::i18n::tf("{0} contains code, which needs a sandbox this system does not provide: {1}", &[id, &why])),
        (Some(_), Ok(())) => Ok(()),
    }
}

/// Whether installed code extensions load at startup: only what costs nothing, so they start no process then. The
/// sandbox check runs a process, so it waits for the first install or start (`code_runs_here`); once it has answered,
/// its answer counts here too.
pub fn code_loads_here(id: &dyn std::fmt::Display) -> Result<(), String> {
    #[cfg(test)]
    if LOAD_WITHOUT_EXTHOST.with(|c| c.get()) {
        return Ok(());
    }
    // Looked up once per process: a reload with many code extensions installed asks once, not once each.
    static PRESENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    match SANDBOX.get() {
        Some(_) => code_runs_here(id),
        None if !*PRESENT.get_or_init(|| exthost_path().is_some()) => {
            Err(crate::i18n::tf("{} contains code, but codeyard-exthost is missing beside Codeyard", &[id]))
        }
        None => Ok(()),
    }
}

static SANDBOX: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();

/// `codeyard-exthost --check-sandbox`, asked once per process: installed code runs only where the OS sandbox works.
pub fn sandbox() -> Result<(), String> {
    SANDBOX
        .get_or_init(|| {
            let exe = exthost_path().ok_or_else(|| "codeyard-exthost is missing".to_string())?;
            let out = Command::new(exe).arg("--check-sandbox").output().map_err(|e| e.to_string())?;
            let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() { Ok(()) } else { Err(line.strip_prefix("unavailable: ").unwrap_or(&line).to_string()) }
        })
        .clone()
}

/// `<data>/extensions/compiled/<build>/<wasm sha256>.cwasm`, with a `.sha256` sidecar of the compiled file.
/// Compiled code from other builds is deleted (`gc`), since the exthost refuses it anyway.
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(extensions_data: &Path) -> Cache {
        Cache { dir: extensions_data.join("compiled") }
    }

    fn build_dir(&self) -> PathBuf {
        let safe: String = ext_proto::BUILD.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '-' }).collect();
        self.dir.join(safe)
    }

    /// The compiled code of `wasm`, compiling it with `exthost` when it is not cached.
    pub fn compiled(&self, exthost: &Path, wasm: &[u8]) -> Result<(PathBuf, [u8; 32]), String> {
        compile_into(&self.build_dir(), exthost, wasm, None)
    }

    /// The extension in development (`cyx dev`): compiled with Cranelift's incremental cache, so a rebuild recompiles
    /// only the functions that changed, into `dev/<id>/`, which keeps only the latest build.
    pub fn compiled_dev(&self, exthost: &Path, wasm: &[u8], id: &str) -> Result<(PathBuf, [u8; 32]), String> {
        let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
        let dir = self.build_dir().join("dev").join(safe);
        let incremental = dir.join("incremental.bin");
        let (path, sha) = compile_into(&dir, exthost, wasm, Some(&incremental))?;
        // Earlier builds: a running exthost has loaded its code already, and on Windows an open file stays.
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let keep = path.file_stem().map(|s| s.to_os_string()).unwrap_or_default();
            for e in rd.flatten() {
                let p = e.path();
                if p != incremental && p.file_stem().is_some_and(|s| s != keep) {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        Ok((path, sha))
    }

    /// Delete compiled code of other builds.
    pub fn gc(&self) {
        let keep = self.build_dir();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                if e.path() != keep {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
}

/// `<dir>/<wasm sha256>.cwasm` with its `.sha256` sidecar, compiled with `exthost` unless it is there already.
fn compile_into(dir: &Path, exthost: &Path, wasm: &[u8], incremental: Option<&Path>) -> Result<(PathBuf, [u8; 32]), String> {
    let key = hex(&Sha256::digest(wasm));
    let path = dir.join(format!("{key}.cwasm"));
    let sidecar = dir.join(format!("{key}.sha256"));
    if let (Ok(code), Ok(want)) = (std::fs::read(&path), std::fs::read_to_string(&sidecar))
        && hex(&Sha256::digest(&code)) == want.trim()
    {
        return Ok((path, Sha256::digest(&code).into()));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exthost);
    cmd.arg("--compile");
    if let Some(cache) = incremental {
        cmd.arg("--cache").arg(cache);
    }
    let mut child =
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("cannot start {}: {e}", exthost.display()))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let input = wasm.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let tmp = dir.join(format!(".{key}.tmp"));
    std::fs::write(&tmp, &out.stdout).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| e.to_string())?;
    let sha: [u8; 32] = Sha256::digest(&out.stdout).into();
    std::fs::write(&sidecar, hex(&sha)).map_err(|e| e.to_string())?;
    Ok((path, sha))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What the workbench hears from one exthost.
#[derive(Debug)]
pub enum Event {
    Message(ToApp),
    /// The process ended: `crashed` unless it was asked to stop.
    Exited {
        crashed: bool,
        reason: String,
    },
}

/// One running exthost.
pub struct Process {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    stopping: Arc<std::sync::atomic::AtomicBool>,
    /// Why the app killed it, when that counts as a crash (over its memory cap): the reason its exit reports.
    killed_for: Arc<Mutex<Option<String>>>,
    pid: u32,
    /// Replaced by a new build (hot reload): its end is not reported, since the extension runs on.
    retired: Arc<std::sync::atomic::AtomicBool>,
    pub last_used: Mutex<Instant>,
}

/// Every frame the app sent to an exthost, in tests: what an extension could ever see.
#[cfg(test)]
pub static SENT_FOR_TEST: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

impl Process {
    /// Start an exthost for one extension and complete the handshake. Frames after `Ready` arrive at `events`,
    /// tagged with the extension id, from a reader thread.
    pub fn start(exthost: &Path, hello: Hello, events: mpsc::Sender<(String, Event)>) -> Result<Process, String> {
        let id = hello.extension.clone();
        let mut child = Command::new(exthost)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", exthost.display()))?;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take();
        ext_proto::write(&mut stdin, &ToHost::Hello(Box::new(hello))).map_err(|e| e.to_string())?;
        let mut reader = Reader::limited(BufReader::new(stdout), Instant::now());
        match reader.read::<ToApp>(Instant::now()) {
            Ok(ToApp::Ready { .. }) => {}
            Ok(ToApp::Mismatch { build, .. }) => {
                let _ = child.kill();
                return Err(format!("the extension host is build {build}, not {}", ext_proto::BUILD));
            }
            Ok(other) => return Err(format!("unexpected first frame {other:?}")),
            Err(e) => return Err(e.to_string()),
        }
        let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = stopping.clone();
        let killed_for: Arc<Mutex<Option<String>>> = Arc::default();
        let killed = killed_for.clone();
        let retired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let replaced = retired.clone();
        std::thread::Builder::new()
            .name(format!("exthost {id}"))
            .spawn(move || {
                let reason = loop {
                    match reader.read::<ToApp>(Instant::now()) {
                        Ok(m) => {
                            let trap = match &m {
                                ToApp::Trap(t) => Some(t.message.clone()),
                                _ => None,
                            };
                            if events.send((id.clone(), Event::Message(m))).is_err() {
                                return;
                            }
                            if let Some(message) = trap {
                                break message;
                            }
                        }
                        // Too large or too fast: the app stops listening and kills it (§6.2).
                        Err(ext_proto::Error::Closed) => break String::new(),
                        Err(e) => break e.to_string(),
                    }
                };
                let mut reason = reason;
                if let Some(why) = killed.lock().unwrap_or_else(|p| p.into_inner()).take() {
                    reason = why;
                }
                if let Some(mut err) = stderr {
                    let mut s = String::new();
                    let _ = std::io::Read::read_to_string(&mut err, &mut s);
                    if reason.is_empty() {
                        reason = s.trim().to_string();
                    }
                }
                let crashed = !stop.load(std::sync::atomic::Ordering::Relaxed);
                if !replaced.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = events.send((id, Event::Exited { crashed, reason }));
                }
            })
            .map_err(|e| e.to_string())?;
        let pid = child.id();
        Ok(Process { stdin: Mutex::new(stdin), child: Mutex::new(child), stopping, killed_for, pid, retired, last_used: Mutex::new(Instant::now()) })
    }

    pub fn send(&self, msg: &ToHost) -> Result<(), String> {
        #[cfg(test)]
        {
            let mut frame = Vec::new();
            let _ = ext_proto::write(&mut frame, msg);
            SENT_FOR_TEST.lock().unwrap_or_else(|p| p.into_inner()).push(frame);
        }
        *self.last_used.lock().unwrap_or_else(|p| p.into_inner()) = Instant::now();
        let mut w = self.stdin.lock().unwrap_or_else(|p| p.into_inner());
        ext_proto::write(&mut *w, msg).map_err(|e| e.to_string())
    }

    pub fn idle_for(&self) -> Duration {
        self.last_used.lock().unwrap_or_else(|p| p.into_inner()).elapsed()
    }

    /// Ask it to deactivate and exit; kill it if it does not within a second.
    pub fn stop(&self) {
        self.stopping.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.send(&ToHost::Shutdown);
        let mut child = self.child.lock().unwrap_or_else(|p| p.into_inner());
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            // A hot reload waits for this; an exthost exits within milliseconds of `Shutdown`.
            std::thread::sleep(Duration::from_millis(2));
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    /// The exthost's process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Kill at once and report it as a crash with `reason` (over its memory cap).
    pub fn kill_as_crash(&self, reason: String) {
        *self.killed_for.lock().unwrap_or_else(|p| p.into_inner()) = Some(reason);
        let mut child = self.child.lock().unwrap_or_else(|p| p.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Stop it for a new build of the same extension (hot reload): `deactivate` runs, so the SDK can keep its state,
    /// and its end is not reported as the extension stopping.
    pub fn retire(&self) {
        self.retired.store(true, std::sync::atomic::Ordering::Relaxed);
        self.stop();
    }

    /// Kill at once (a frame over its limits, a misbehaving guest).
    pub fn kill(&self) {
        self.stopping.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut child = self.child.lock().unwrap_or_else(|p| p.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Crash history per extension: three within a minute disable it for the session.
#[derive(Default)]
pub struct Crashes(HashMap<String, Vec<Instant>>);

impl Crashes {
    /// Record a crash at `now`; true when the extension must now be disabled.
    pub fn record(&mut self, id: &str, now: Instant) -> bool {
        let list = self.0.entry(id.to_string()).or_default();
        list.retain(|t| now.duration_since(*t) < CRASH_WINDOW);
        list.push(now);
        list.len() >= CRASHES
    }
}

/// Requests sent to exthosts, waiting for `requests.respond`.
#[derive(Default)]
pub struct Pending {
    next: u64,
    waiting: HashMap<(String, u64), mpsc::Sender<Result<Response, ext_api::value::Error>>>,
}

impl Pending {
    pub fn add(&mut self, extension: &str) -> (u64, mpsc::Receiver<Result<Response, ext_api::value::Error>>) {
        self.next += 1;
        let (tx, rx) = mpsc::channel();
        self.waiting.insert((extension.to_string(), self.next), tx);
        (self.next, rx)
    }

    /// An answer from the exthost; ids it was never sent are ignored.
    pub fn answer(&mut self, extension: &str, id: u64, result: Result<Response, ext_api::value::Error>) {
        if let Some(tx) = self.waiting.remove(&(extension.to_string(), id)) {
            let _ = tx.send(result);
        }
    }

    /// The exthost went away: everything it owed fails.
    pub fn fail_all(&mut self, extension: &str, why: &str) {
        let keys: Vec<_> = self.waiting.keys().filter(|(e, _)| e == extension).cloned().collect();
        for k in keys {
            if let Some(tx) = self.waiting.remove(&k) {
                let _ = tx.send(Err(ext_api::value::Error::Other(why.to_string())));
            }
        }
    }
}

/// The `Hello` for one extension in one window.
pub struct Start<'a> {
    pub id: &'a str,
    pub package_dir: &'a Path,
    pub compiled: (PathBuf, [u8; 32]),
    pub data_dir: PathBuf,
    pub granted: Vec<String>,
    pub untrusted: bool,
    pub env: EnvInfo,
    pub settings: Vec<(String, Option<String>)>,
    pub folders: Vec<String>,
    pub reason: String,
    /// False only for the extension in development (`cyx dev`): the author's own folder, behind Wasm and the broker.
    pub sandbox: bool,
}

impl Start<'_> {
    pub fn hello(self) -> Hello {
        Hello {
            version: ext_proto::VERSION,
            build: ext_proto::BUILD.into(),
            extension: self.id.to_string(),
            package_dir: self.package_dir.to_string_lossy().into_owned(),
            compiled: self.compiled.0.to_string_lossy().into_owned(),
            compiled_sha256: self.compiled.1,
            data_dir: self.data_dir.to_string_lossy().into_owned(),
            granted: self.granted,
            untrusted: self.untrusted,
            env: self.env,
            settings: self.settings,
            folders: self.folders,
            reason: self.reason,
            sandbox: self.sandbox,
        }
    }
}

/// The `codeyard:api` interfaces whose permission `p` grants (what the exthost links for real).
pub fn granted_interfaces(p: &ext_manifest::Permissions) -> Vec<String> {
    ext_api::INTERFACES.iter().filter(|i| i.need != ext_api::Need::Nothing && i.need.granted(p)).map(|i| i.name.to_string()).collect()
}

/// A command request, as `guest.handle` receives it.
pub fn command(command: &str, args_json: &str) -> Request {
    Request::Command { command: command.to_string(), args_json: args_json.to_string() }
}

#[cfg(test)]
#[path = "ext_runtime_tests.rs"]
mod tests;
