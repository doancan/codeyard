//! Workbench: running code extensions (docs/plugin-architecture.md §3, §6). Their commands join the palette; the first
//! one starts the extension's exthost (compiling its component once). Frames from exthosts arrive here: file, search
//! and process calls go to the broker on a background thread, calls that need the window are answered here, and
//! everything is checked against what the extension may do in this window before anything happens.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};

use ext_api::value::{self as v, Completion, LogLevel, Notify, Start};
use ext_proto::{ToApp, ToHost};
use futures::StreamExt;

use super::*;
use crate::ext_broker::{self, Scope, Sink};
use crate::ext_runtime::{self, Crashes, Event, Pending, Process};

/// Lines of each extension's log kept for the Inspector (#108).
const LOG_LINES: usize = 500;

pub(super) struct CodeState {
    tx: mpsc::Sender<(String, Event)>,
    rx: Option<mpsc::Receiver<(String, Event)>>,
    pub(super) procs: HashMap<String, Arc<Process>>,
    /// Requests waiting for their exthost to start.
    queued: HashMap<String, Vec<ToHost>>,
    pub(super) pending: Arc<Mutex<Pending>>,
    crashes: Crashes,
    /// Crashed three times within a minute: not started again this session.
    pub(super) disabled: HashSet<String>,
    cancels: HashMap<(String, u64), Arc<AtomicBool>>,
    /// The request that is a user action right now, per extension (`activeTab`, `open-external`).
    pub(super) user_action: HashMap<String, u64>,
    pub(super) logs: HashMap<String, VecDeque<(LogLevel, String)>>,
    pub(super) last_trap: HashMap<String, ext_proto::Trap>,
    pump: Option<gpui::Task<()>>,
}

impl CodeState {
    /// Started, or starting.
    pub(super) fn running(&self, id: &str) -> bool {
        self.procs.contains_key(id) || self.queued.contains_key(id)
    }
}

impl Default for CodeState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        CodeState {
            tx,
            rx: Some(rx),
            procs: HashMap::new(),
            queued: HashMap::new(),
            pending: Arc::default(),
            crashes: Crashes::default(),
            disabled: HashSet::new(),
            cancels: HashMap::new(),
            user_action: HashMap::new(),
            logs: HashMap::new(),
            last_trap: HashMap::new(),
            pump: None,
        }
    }
}

/// Streams a ticket's data back to its exthost.
struct ToExthost {
    proc: Arc<Process>,
    ticket: u64,
}

impl Sink for ToExthost {
    fn event(&self, event: v::StreamEvent) {
        let _ = self.proc.send(&ToHost::Event { ticket: self.ticket, event });
    }
}

impl Workbench {
    /// Palette entries for the commands of active code extensions.
    pub(super) fn extension_commands(&self) -> Vec<Command> {
        let reg = crate::ext::current();
        let mut out = Vec::new();
        for e in reg.active.iter().filter(|e| e.manifest.code.is_some()) {
            for c in &e.manifest.contributions.commands {
                // The manifest stores full ids (`you.ext.refresh`); the extension is asked by its local id.
                let local = c.id.strip_prefix(e.id()).and_then(|r| r.strip_prefix('.')).unwrap_or(&c.id);
                out.push(Command { label: c.title.clone(), run: Run::Extension { extension: e.id().to_string(), command: local.to_string() } });
            }
        }
        out
    }

    /// What `extension` may do in this window.
    fn extension_scope(&self, active: &crate::ext::Active, cx: &App) -> Scope {
        let roots = self.all_roots(cx);
        let trusted = roots.iter().all(|r| self.is_trusted_root(r, cx));
        let trust = if trusted { ext_core::trust::Trust::Trusted } else { ext_core::trust::Trust::Untrusted };
        let eff = ext_core::trust::effective(&active.manifest, &ext_manifest::Permissions::default(), trust);
        Scope {
            extension: active.id().to_string(),
            permissions: eff.permissions,
            roots,
            trusted,
            dependencies: active.manifest.dependencies.iter().map(|(d, _)| d.to_string()).collect(),
        }
    }

    /// Run a command of an extension (palette, menu, key binding): start its exthost if needed, then ask it.
    pub(super) fn run_extension_command(&mut self, extension: &str, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == extension).cloned() else { return };
        if self.cfg.exts.code.disabled.contains(extension) {
            let msg = crate::i18n::tf("{} crashed three times and is turned off until Codeyard restarts.", &[&extension]);
            self.notice = Some(Notice::Error(msg.into()));
            cx.notify();
            return;
        }
        let (id, answer) = self.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).add(extension);
        self.cfg.exts.code.user_action.insert(extension.to_string(), id);
        let msg = ToHost::Handle { id, deadline_ms: 0, request: ext_runtime::command(command, "null") };
        self.send_to_extension(&active, &format!("command:{command}"), msg, window, cx);
        let ext = extension.to_string();
        let answer = crate::ui::bridge(answer);
        cx.spawn_in(window, async move |this, cx| {
            let mut answer = answer;
            let result = answer.next().await;
            let _ = this.update(cx, |this, cx| {
                if this.cfg.exts.code.user_action.get(&ext) == Some(&id) {
                    this.cfg.exts.code.user_action.remove(&ext);
                }
                // Turned off by this very crash: that notice says more than the failed command.
                if let Some(Err(e)) = result.filter(|_| !this.cfg.exts.code.disabled.contains(&ext)) {
                    this.notice = Some(Notice::Error(format!("{ext}: {}", explain(&e)).into()));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Send `msg` to the extension's exthost, starting it first (compile and handshake run in the background).
    pub(super) fn send_to_extension(&mut self, active: &crate::ext::Active, reason: &str, msg: ToHost, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_extension_pump(window, cx);
        let id = active.id().to_string();
        if let Some(p) = self.cfg.exts.code.procs.get(&id)
            && p.send(&msg).is_ok()
        {
            return;
        }
        let queue = self.cfg.exts.code.queued.entry(id.clone()).or_default();
        queue.push(msg);
        if queue.len() > 1 {
            return; // already starting
        }
        let Some(exthost) = ext_runtime::exthost_path() else {
            self.cfg.exts.code.queued.remove(&id);
            self.notice = Some(Notice::Error(crate::i18n::tf("{} cannot run: codeyard-exthost is missing beside Codeyard.", &[&id]).into()));
            cx.notify();
            return;
        };
        let scope = self.extension_scope(active, cx);
        let Some(cfg_dir) = self.cfg.dir.clone() else {
            self.cfg.exts.code.queued.remove(&id);
            return;
        };
        let data = crate::ext::data_root(&cfg_dir);
        let dark = matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark);
        let env = v::EnvInfo {
            codeyard_version: env!("CARGO_PKG_VERSION").into(),
            api_version: ext_api::VERSION.to_string(),
            language: crate::i18n::LANGUAGES[crate::i18n::current()].code.to_string(),
            theme: if dark { v::ThemeKind::Dark } else { v::ThemeKind::Light },
            platform: if cfg!(target_os = "macos") {
                v::Platform::Macos
            } else if cfg!(windows) {
                v::Platform::Windows
            } else {
                v::Platform::Linux
            },
            remote: self.remote.host.is_remote(),
            workspace_trusted: scope.trusted,
        };
        let folders: Vec<String> = scope.roots.iter().map(|r| ext_broker::uri(r)).collect();
        let package_dir = active.dir.clone();
        let wasm_path = active.file(ext_pkg::COMPONENT);
        let granted = ext_runtime::granted_interfaces(&scope.permissions);
        let untrusted = !scope.trusted;
        let tx = self.cfg.exts.code.tx.clone();
        let reason = reason.to_string();
        let task = cx.background_executor().spawn(async move {
            let wasm = std::fs::read(&wasm_path).map_err(|e| format!("{}: {e}", wasm_path.display()))?;
            let cache = ext_runtime::Cache::new(&data);
            cache.gc();
            let compiled = cache.compiled(&exthost, &wasm)?;
            let start = ext_runtime::Start {
                id: &id,
                package_dir: &package_dir,
                compiled,
                data_dir: data.join("data").join(&id),
                granted,
                untrusted,
                env,
                settings: Vec::new(),
                folders,
                reason,
            };
            let hello = start.hello();
            Process::start(&exthost, hello, tx).map(|p| (id, p))
        });
        let ext = active.id().to_string();
        cx.spawn_in(window, async move |this, cx| {
            let started = task.await;
            let _ = this.update(cx, |this, cx| {
                let code = &mut this.cfg.exts.code;
                let queued = code.queued.remove(&ext).unwrap_or_default();
                match started {
                    Ok((id, p)) => {
                        let p = Arc::new(p);
                        for m in &queued {
                            let _ = p.send(m);
                        }
                        code.procs.insert(id, p);
                    }
                    Err(e) => {
                        code.pending.lock().unwrap_or_else(|p| p.into_inner()).fail_all(&ext, &e);
                        this.notice = Some(Notice::Error(crate::i18n::tf("{0} could not start: {1}", &[&ext, &e]).into()));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Start listening to exthosts once per window.
    fn ensure_extension_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rx) = self.cfg.exts.code.rx.take() else { return };
        let mut events = crate::ui::bridge(rx);
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some((id, event)) = events.next().await {
                if this.update_in(cx, |this, window, cx| this.on_extension_event(&id, event, window, cx)).is_err() {
                    break;
                }
            }
        });
        self.cfg.exts.code.pump = Some(task);
        // Stop exthosts nobody used for a while; the next event starts them again.
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(60)).await;
                if this.update(cx, |this, _| this.stop_idle_extensions()).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn stop_idle_extensions(&mut self) {
        let idle: Vec<String> = self.cfg.exts.code.procs.iter().filter(|(_, p)| p.idle_for() > ext_runtime::IDLE).map(|(id, _)| id.clone()).collect();
        for id in idle {
            if let Some(p) = self.cfg.exts.code.procs.remove(&id) {
                std::thread::spawn(move || p.stop());
            }
        }
    }

    fn on_extension_event(&mut self, id: &str, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Message(ToApp::Start { ticket, call }) => self.on_extension_call(id, ticket, call, window, cx),
            Event::Message(ToApp::Notify(n)) => self.on_extension_notify(id, n, cx),
            Event::Message(ToApp::Activated(Err(e))) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("{0} failed to start: {1}", &[&id, &e]).into()));
                cx.notify();
            }
            Event::Message(ToApp::Trap(t)) => {
                self.log_extension(id, LogLevel::Error, format!("trap: {} {}", t.message, t.frames.join(" ← ")));
                self.cfg.exts.code.last_trap.insert(id.to_string(), t);
            }
            Event::Message(_) => {}
            Event::Exited { crashed, reason } => {
                self.on_extension_exited(id, crashed, cx);
                let code = &mut self.cfg.exts.code;
                code.procs.remove(id);
                code.pending.lock().unwrap_or_else(|p| p.into_inner()).fail_all(id, &reason);
                if crashed {
                    if code.crashes.record(id, std::time::Instant::now()) {
                        code.disabled.insert(id.to_string());
                        let msg = crate::i18n::tf("{} crashed three times in a minute and was turned off.", &[&id]);
                        self.notice = Some(Notice::Error(msg.into()));
                    } else {
                        self.log_extension(id, LogLevel::Error, format!("stopped: {reason}"));
                    }
                    cx.notify();
                }
            }
        }
    }

    pub(super) fn log_extension(&mut self, id: &str, level: LogLevel, line: String) {
        let log = self.cfg.exts.code.logs.entry(id.to_string()).or_default();
        if log.len() >= LOG_LINES {
            log.pop_front();
        }
        log.push_back((level, line));
    }

    fn on_extension_notify(&mut self, id: &str, n: Notify, cx: &mut Context<Self>) {
        match n {
            Notify::Log { level, message } => self.log_extension(id, level, message),
            Notify::Respond { id: req, result } => {
                self.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).answer(id, req, result);
            }
            Notify::Cancel { ticket } => {
                if let Some(c) = self.cfg.exts.code.cancels.remove(&(id.to_string(), ticket)) {
                    c.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            // Status items, diagnostics and decorations (#104).
            n @ (Notify::SetStatusItem { .. }
            | Notify::RemoveStatusItem { .. }
            | Notify::SetDiagnostics { .. }
            | Notify::ClearDiagnostics { .. }
            | Notify::SetDecorations { .. }) => self.on_provider_notify(id, n, cx),
            // Progress, timers and views land with their surfaces (#103); until then they are logged so an author sees
            // them arrive.
            other => {
                self.log_extension(id, LogLevel::Debug, format!("{} (not shown in this version)", other.wit_name()));
                let _ = cx;
            }
        }
    }

    /// A host call that returns a ticket: checked, then run on a background thread or answered from the window.
    fn on_extension_call(&mut self, id: &str, ticket: u64, call: Start, window: &mut Window, cx: &mut Context<Self>) {
        let Some(proc) = self.cfg.exts.code.procs.get(id).cloned() else { return };
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == id).cloned() else { return };
        let scope = self.extension_scope(&active, cx);
        let complete = |p: &Arc<Process>, c: Completion| {
            let _ = p.send(&ToHost::Complete { ticket, completion: c });
        };
        match &call {
            Start::ShowNotice { severity, message, .. } => {
                let text = format!("{id}: {message}");
                self.notice = Some(if *severity == v::Severity::Error { Notice::Error(text.into()) } else { Notice::Info(text.into()) });
                cx.notify();
                complete(&proc, Completion::Chosen(Vec::new()));
            }
            Start::OpenExternal { url } => {
                let user = self.cfg.exts.code.user_action.contains_key(id);
                let c = if !url.starts_with("https://") {
                    Completion::Failed(v::Error::Invalid("only https: links open".into()))
                } else if !user {
                    Completion::Failed(v::Error::Denied(v::Denied { permission: "a user action (a command or a link click)".into(), why: v::Why::NotDeclared }))
                } else {
                    cx.open_url(url);
                    Completion::Done
                };
                complete(&proc, c);
            }
            Start::Execute { command, args_json } => match scope.may_execute(command) {
                Err(e) => complete(&proc, Completion::Failed(e)),
                Ok(()) => self.execute_for_extension(id, ticket, command, args_json, proc, window, cx),
            },
            Start::WriteClipboard { text } => {
                let c = if scope.permissions.clipboard.writes() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                    Completion::Done
                } else {
                    Completion::Failed(v::Error::Denied(v::Denied { permission: "clipboard = \"write\"".into(), why: v::Why::NotDeclared }))
                };
                complete(&proc, c);
            }
            Start::ReadClipboard => {
                let c = if scope.permissions.clipboard.reads() {
                    Completion::Text(cx.read_from_clipboard().and_then(|i| i.text()))
                } else {
                    Completion::Failed(v::Error::Denied(v::Denied { permission: "clipboard = \"read\"".into(), why: v::Why::NotDeclared }))
                };
                complete(&proc, c);
            }
            _ => {
                let host = self.remote.host.clone();
                let cancel = Arc::new(AtomicBool::new(false));
                self.cfg.exts.code.cancels.insert((id.to_string(), ticket), cancel.clone());
                let p = proc.clone();
                cx.background_executor()
                    .spawn(async move {
                        let sink = ToExthost { proc: p.clone(), ticket };
                        let c = ext_broker::run(host.as_ref(), &scope, &call, &sink, &cancel)
                            .unwrap_or_else(|| Completion::Failed(v::Error::Other(format!("{} is not available in this version", call.wit_name()))));
                        let _ = p.send(&ToHost::Complete { ticket, completion: c });
                    })
                    .detach();
            }
        }
    }

    /// `commands.execute` for an extension's own or a dependency's command: forwarded to that extension, and its
    /// answer completes the ticket.
    #[allow(clippy::too_many_arguments)]
    fn execute_for_extension(
        &mut self,
        caller: &str,
        ticket: u64,
        command: &str,
        args_json: &str,
        proc: Arc<Process>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reg = crate::ext::current();
        let target = reg.active.iter().find(|e| command.strip_prefix(e.id()).is_some_and(|r| r.starts_with('.'))).cloned();
        let Some(target) = target else {
            let c = Completion::Failed(v::Error::NotFound(format!("{command} is not available")));
            let _ = proc.send(&ToHost::Complete { ticket, completion: c });
            return;
        };
        let local = command[target.id().len() + 1..].to_string();
        let (id, answer) = self.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).add(target.id());
        let msg = ToHost::Handle { id, deadline_ms: 0, request: ext_runtime::command(&local, args_json) };
        self.send_to_extension(&target, &format!("command:{local}"), msg, window, cx);
        let _ = caller;
        let answer = crate::ui::bridge(answer);
        cx.background_executor()
            .spawn(async move {
                let mut answer = answer;
                let c = match answer.next().await {
                    Some(Ok(v::Response::Json(j))) => Completion::Json(j),
                    Some(Ok(_)) => Completion::Json("null".into()),
                    Some(Err(e)) => Completion::Failed(e),
                    None => Completion::Failed(v::Error::Cancelled),
                };
                let _ = proc.send(&ToHost::Complete { ticket, completion: c });
            })
            .detach();
    }

    /// Stop the exthosts of extensions that were uninstalled, turned off or replaced since they started.
    pub(super) fn stop_removed_extensions(&mut self) {
        let reg = crate::ext::current();
        let gone: Vec<String> = self.cfg.exts.code.procs.keys().filter(|id| !reg.active.iter().any(|e| e.id() == id.as_str())).cloned().collect();
        for id in gone {
            if let Some(p) = self.cfg.exts.code.procs.remove(&id) {
                std::thread::spawn(move || p.stop());
            }
        }
    }
}

/// An API error in plain words, with its fix when it is a denial.
pub(super) fn explain(e: &v::Error) -> String {
    match e {
        v::Error::Denied(d) => crate::i18n::tf("denied ({0}); add to [permissions]: {1}", &[&d.why.as_str(), &d.permission]),
        other => other.to_string(),
    }
}
