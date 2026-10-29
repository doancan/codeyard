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
    procs: HashMap<String, Arc<Process>>,
    /// Requests waiting for their exthost to start.
    queued: HashMap<String, Vec<ToHost>>,
    pending: Arc<Mutex<Pending>>,
    crashes: Crashes,
    /// Crashed three times within a minute: not started again this session.
    pub(super) disabled: HashSet<String>,
    cancels: HashMap<(String, u64), Arc<AtomicBool>>,
    /// The request that is a user action right now, per extension (`activeTab`, `open-external`).
    user_action: HashMap<String, u64>,
    pub(super) logs: HashMap<String, VecDeque<(LogLevel, String)>>,
    /// The last command answer of each extension, for the Inspector (#108) and tests.
    pub(super) last_answer: HashMap<String, String>,
    pub(super) last_trap: HashMap<String, ext_proto::Trap>,
    pump: Option<gpui::Task<()>>,
    /// `timers.every`: dropping a task stops its timer.
    timers: HashMap<(String, u64), gpui::Task<()>>,
    /// `workspace-contains` checks already made, per extension and workspace roots, so a reload does not repeat them.
    checked_workspace: HashSet<(String, Vec<String>)>,
    /// The setting values each running exthost has, to send only changes.
    sent_settings: HashMap<String, Vec<(String, Option<String>)>>,
    /// Progress shown for each extension: id → (title, message, percent).
    pub(super) progress: HashMap<(String, u64), (String, Option<String>, Option<u8>)>,
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
            last_answer: HashMap::new(),
            last_trap: HashMap::new(),
            pump: None,
            timers: HashMap::new(),
            sent_settings: HashMap::new(),
            checked_workspace: HashSet::new(),
            progress: HashMap::new(),
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
        self.send_to_extension(&active, &format!("command:{command}"), Some(msg), window, cx);
        let ext = extension.to_string();
        let answer = crate::ui::bridge(answer);
        cx.spawn_in(window, async move |this, cx| {
            let mut answer = answer;
            let result = answer.next().await;
            let _ = this.update(cx, |this, cx| {
                if this.cfg.exts.code.user_action.get(&ext) == Some(&id) {
                    this.cfg.exts.code.user_action.remove(&ext);
                }
                if let Some(Ok(v::Response::Json(j))) = &result {
                    this.cfg.exts.code.last_answer.insert(ext.clone(), j.clone());
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
    /// `msg` is `None` to only activate it (`workspace-contains`).
    fn send_to_extension(&mut self, active: &crate::ext::Active, reason: &str, msg: Option<ToHost>, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_extension_pump(window, cx);
        let id = active.id().to_string();
        if let Some(p) = self.cfg.exts.code.procs.get(&id) {
            match &msg {
                None => return,
                Some(m) if p.send(m).is_ok() => return,
                Some(_) => {}
            }
        }
        let starting = self.cfg.exts.code.queued.contains_key(&id);
        self.cfg.exts.code.queued.entry(id.clone()).or_default().extend(msg);
        if starting {
            return;
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
        let settings_now = extension_settings(active, cx);
        self.cfg.exts.code.sent_settings.insert(id.clone(), settings_now.clone());
        let package_dir = active.dir.clone();
        let wasm_path = active.file(ext_pkg::COMPONENT);
        let granted = ext_runtime::granted_interfaces(&scope.permissions);
        let untrusted = !scope.trusted;
        let tx = self.cfg.exts.code.tx.clone();
        let reason = reason.to_string();
        // The extension in development runs without the OS sandbox (its author's folder); everything installed runs in it.
        let sandbox = crate::ext::dev_dir().is_none_or(|d| d != active.dir);
        let task = cx.background_executor().spawn(async move {
            let wasm = std::fs::read(&wasm_path).map_err(|e| format!("{}: {e}", wasm_path.display()))?;
            // The sandbox lets the exthost write only here, so it must exist first.
            std::fs::create_dir_all(data.join("data").join(&id)).map_err(|e| e.to_string())?;
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
                settings: settings_now,
                folders,
                reason,
                sandbox,
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
            Event::Message(ToApp::Notify(n)) => self.on_extension_notify(id, n, window, cx),
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

    fn log_extension(&mut self, id: &str, level: LogLevel, line: String) {
        let log = self.cfg.exts.code.logs.entry(id.to_string()).or_default();
        if log.len() >= LOG_LINES {
            log.pop_front();
        }
        log.push_back((level, line));
    }

    fn on_extension_notify(&mut self, id: &str, n: Notify, window: &mut Window, cx: &mut Context<Self>) {
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
            Notify::StartProgress { id: p, title, .. } => {
                self.cfg.exts.code.progress.insert((id.to_string(), p), (title, None, None));
                self.show_extension_progress(id, p, cx);
            }
            Notify::ReportProgress { id: p, message, percent } => {
                if let Some(entry) = self.cfg.exts.code.progress.get_mut(&(id.to_string(), p)) {
                    entry.1 = message;
                    entry.2 = percent;
                }
                self.show_extension_progress(id, p, cx);
            }
            Notify::EndProgress { id: p } => {
                if self.cfg.exts.code.progress.remove(&(id.to_string(), p)).is_some()
                    && matches!(&self.notice, Some(Notice::Info(t)) if t.starts_with(&format!("{id}: ")))
                {
                    self.notice = None;
                    cx.notify();
                }
            }
            Notify::Every { id: t, seconds } => self.start_extension_timer(id, t, seconds, window, cx),
            Notify::ClearTimer { id: t } => {
                self.cfg.exts.code.timers.remove(&(id.to_string(), t));
            }
            // Status items, views, diagnostics and decorations land with their surfaces (#103, #104); until then
            // they are logged so an author sees them arrive.
            other => self.log_extension(id, LogLevel::Debug, format!("{} (not shown in this version)", other.wit_name())),
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
            Start::QuickPick { items, options } => self.extension_quick_pick(id, ticket, items, options, window, cx),
            Start::InputBox { options } => {
                if options.password {
                    let msg = "password input is not available; accounts go through `auth`, which never shows the extension a token";
                    complete(&proc, Completion::Failed(v::Error::Invalid(msg.into())));
                } else {
                    let placeholder = format!("{id}: {}", options.prompt.clone().or(options.title.clone()).unwrap_or_default());
                    let purpose = InputPurpose::ExtInput { extension: id.to_string(), ticket };
                    self.show_input(purpose, "OK", &placeholder, options.value.as_deref().unwrap_or(""), window, cx);
                }
            }
            Start::ActiveDocument | Start::Selections | Start::ReadDocument { .. } | Start::ApplyEdits { .. } | Start::SetSelections { .. } => {
                let c = self.extension_document_call(id, &scope, &call, cx);
                complete(&proc, c);
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
        self.send_to_extension(&target, &format!("command:{local}"), Some(msg), window, cx);
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

    /// After a settings reload: stop the exthosts of extensions that were uninstalled or turned off, and tell the
    /// others about their settings that changed (`request.settings-changed`).
    pub(super) fn sync_running_extensions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reg = crate::ext::current();
        for e in reg.active.iter() {
            let id = e.id().to_string();
            let Some(p) = self.cfg.exts.code.procs.get(&id).cloned() else { continue };
            let now = extension_settings(e, cx);
            let before = self.cfg.exts.code.sent_settings.get(&id).cloned().unwrap_or_default();
            if now == before {
                continue;
            }
            let keys: Vec<String> = now.iter().filter(|kv| !before.contains(kv)).map(|(k, _)| k.clone()).collect();
            let _ = p.send(&ToHost::Settings(now.clone()));
            let (req, _answer) = self.cfg.exts.code.pending.lock().unwrap_or_else(|e| e.into_inner()).add(&id);
            let _ = p.send(&ToHost::Handle { id: req, deadline_ms: 0, request: v::Request::SettingsChanged { keys } });
            self.cfg.exts.code.sent_settings.insert(id, now);
        }
        self.stop_removed_extensions();
        self.activate_for_workspace(window, cx);
    }

    /// `[activation] workspace-contains`: start an extension when a workspace folder has a matching file. The check
    /// runs once per extension and set of folders, in the background, on the window's host.
    fn activate_for_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let roots = self.all_roots(cx);
        if roots.is_empty() {
            return;
        }
        let reg = crate::ext::current();
        for e in reg.active.iter().filter(|e| e.manifest.code.is_some()) {
            let globs: Vec<String> = e
                .manifest
                .activation
                .iter()
                .filter_map(|a| match a {
                    ext_manifest::Activation::WorkspaceContains(g) => Some(g.clone()),
                    _ => None,
                })
                .collect();
            let key = (e.id().to_string(), roots.clone());
            if globs.is_empty() || self.cfg.exts.code.running(e.id()) || !self.cfg.exts.code.checked_workspace.insert(key) {
                continue;
            }
            let host = self.remote.host.clone();
            let roots = roots.clone();
            let id = e.id().to_string();
            let found = cx.background_executor().spawn(async move {
                roots
                    .iter()
                    .any(|root| host.list_files(root, 20_000).is_ok_and(|l| l.paths.iter().any(|p| globs.iter().any(|g| crate::globs::path_matches(g, p)))))
            });
            cx.spawn_in(window, async move |this, cx| {
                if !found.await {
                    return;
                }
                let _ = this.update_in(cx, |this, window, cx| {
                    let reg = crate::ext::current();
                    if let Some(active) = reg.active.iter().find(|e| e.id() == id) {
                        this.send_to_extension(active, "workspace-contains", None, window, cx);
                    }
                });
            })
            .detach();
        }
    }

    /// Stop the exthosts of extensions that were uninstalled, turned off or replaced since they started.
    fn stop_removed_extensions(&mut self) {
        let reg = crate::ext::current();
        let gone: Vec<String> = self.cfg.exts.code.procs.keys().filter(|id| !reg.active.iter().any(|e| e.id() == id.as_str())).cloned().collect();
        for id in gone {
            if let Some(p) = self.cfg.exts.code.procs.remove(&id) {
                std::thread::spawn(move || p.stop());
            }
        }
    }
}

impl Workbench {
    /// Complete a ticket of `extension` from the window (a pick, an input).
    fn complete_extension_ticket(&mut self, extension: &str, ticket: u64, completion: Completion) {
        if let Some(p) = self.cfg.exts.code.procs.get(extension) {
            let _ = p.send(&ToHost::Complete { ticket, completion });
        }
    }

    fn extension_quick_pick(&mut self, id: &str, ticket: u64, items: &[v::PickItem], options: &v::PickOptions, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let rows =
            items.iter().take(10_000).map(|i| PickItem { label: i.label.clone().into(), detail: i.detail.clone().map(Into::into), keys: None }).collect();
        // The publisher is named in the prompt, so an extension cannot pose as Codeyard.
        let prompt = format!("{id}: {}", options.placeholder.clone().or(options.title.clone()).unwrap_or_default());
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items: rows }], &prompt, "", None, cx));
        self.show_overlay(picker, OverlayKind::ExtPick { extension: id.to_string(), ticket }, window, cx);
    }

    /// Picks and inputs of extensions: the answer completes their ticket; Escape answers "nothing".
    pub(super) fn on_extension_overlay_event(&mut self, event: &PickerEvent, window: &mut Window, cx: &mut Context<Self>) {
        let answer = match event {
            PickerEvent::Confirm(Pick::Item { index, .. }) => Some(Completion::Chosen(vec![*index as u32])),
            PickerEvent::Confirm(Pick::Input) => {
                let text = self.overlay.as_ref().map(|o| o.picker.read(cx).query().to_string());
                Some(Completion::Text(text))
            }
            PickerEvent::Dismiss => None,
            _ => return,
        };
        let Some(ov) = self.close_overlay(window, cx) else { return };
        let (ext, ticket, nothing) = match ov.kind {
            OverlayKind::ExtPick { extension, ticket } => (extension, ticket, Completion::Chosen(Vec::new())),
            OverlayKind::Input(InputPurpose::ExtInput { extension, ticket }) => (extension, ticket, Completion::Text(None)),
            _ => return,
        };
        self.complete_extension_ticket(&ext, ticket, answer.unwrap_or(nothing));
    }

    fn show_extension_progress(&mut self, id: &str, p: u64, cx: &mut Context<Self>) {
        let Some((title, message, percent)) = self.cfg.exts.code.progress.get(&(id.to_string(), p)) else { return };
        let mut text = format!("{id}: {title}");
        if let Some(m) = message {
            text.push_str(&format!(" — {m}"));
        }
        if let Some(pc) = percent {
            text.push_str(&format!(" ({pc}%)"));
        }
        self.notice = Some(Notice::Info(text.into()));
        cx.notify();
    }

    /// `timers.every`: a request every `seconds` (at least `limits::TIMER_MIN_SECONDS`), skipped while the window is
    /// not focused. An active timer keeps the exthost from being stopped as idle.
    fn start_extension_timer(&mut self, id: &str, timer: u64, seconds: u32, window: &mut Window, cx: &mut Context<Self>) {
        let every = std::time::Duration::from_secs(seconds.max(ext_manifest::limits::TIMER_MIN_SECONDS) as u64);
        let ext = id.to_string();
        let task = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(every).await;
                let alive = this.update_in(cx, |this, window, _| {
                    if !window.is_window_active() && !cfg!(test) {
                        return;
                    }
                    let Some(p) = this.cfg.exts.code.procs.get(&ext).cloned() else { return };
                    let (req, _answer) = this.cfg.exts.code.pending.lock().unwrap_or_else(|e| e.into_inner()).add(&ext);
                    let _ = p.send(&ToHost::Handle { id: req, deadline_ms: 0, request: v::Request::Timer { id: timer } });
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        self.cfg.exts.code.timers.insert((id.to_string(), timer), task);
    }

    /// Documents: the active editor's, or an open one. A command the user started may read and edit the active
    /// document and set its selections (Chrome's `activeTab`); anything else needs `read` (and `write` to edit) on the
    /// document's path.
    fn extension_document_call(&mut self, id: &str, scope: &Scope, call: &Start, cx: &mut Context<Self>) -> Completion {
        let user = self.cfg.exts.code.user_action.contains_key(id);
        let active = self.active_editor().cloned();
        let active_path = active.as_ref().map(|e| e.read(cx).path.clone());
        let editor_for = |this: &Self, uri: &str| -> Option<(Entity<crate::editor::EditorView>, String)> {
            let path = ext_broker::normalize(uri)?;
            this.tabs.editors.iter().find(|e| ext_broker::normalize(&e.read(cx).path).as_deref() == Some(path.as_str())).map(|e| (e.clone(), path))
        };
        let may = |path: &str, write: bool| -> Result<(), v::Error> {
            if user && active_path.as_deref().and_then(ext_broker::normalize).as_deref() == Some(path) {
                return Ok(());
            }
            if write { scope.may_write(path).map(|_| ()) } else { scope.may_read(path).map(|_| ()) }
        };
        let document = |e: &Entity<crate::editor::EditorView>| {
            let ed = e.read(cx);
            // The language id authors match on (`rust`, `python`), as language servers get it; else the lowercase name.
            let language = crate::lsp::language_of(&ed.path).map_or_else(|| ed.language().to_ascii_lowercase(), |(id, _)| id.to_string());
            v::Document { uri: ext_broker::uri(&ed.path), language, version: ed.lsp_buffer_version() as u32, text: ed.text() }
        };
        let range16 =
            |r: crate::lsp::Range16| v::Range { start: v::Position { line: r.0.0, character: r.0.1 }, end: v::Position { line: r.1.0, character: r.1.1 } };
        let to16 = |r: &v::Range| -> crate::lsp::Range16 { ((r.start.line, r.start.character), (r.end.line, r.end.character)) };
        match call {
            Start::ActiveDocument => match &active {
                None => Completion::Document(None),
                Some(e) => match may(&ext_broker::normalize(&e.read(cx).path).unwrap_or_default(), false) {
                    Ok(()) => Completion::Document(Some(document(e))),
                    Err(err) => Completion::Failed(err),
                },
            },
            Start::Selections => match &active {
                None => Completion::Ranges(Vec::new()),
                Some(e) => match may(&ext_broker::normalize(&e.read(cx).path).unwrap_or_default(), false) {
                    Ok(()) => Completion::Ranges(vec![range16(e.read(cx).lsp_selection())]),
                    Err(err) => Completion::Failed(err),
                },
            },
            Start::ReadDocument { uri } => match editor_for(self, uri) {
                None => Completion::Document(None),
                Some((e, path)) => match may(&path, false) {
                    Ok(()) => Completion::Document(Some(document(&e))),
                    Err(err) => Completion::Failed(err),
                },
            },
            Start::ApplyEdits { uri, version, edits } => match editor_for(self, uri) {
                None => Completion::Failed(v::Error::NotFound(format!("{uri} is not open"))),
                Some((e, path)) => match may(&path, true) {
                    Err(err) => Completion::Failed(err),
                    Ok(()) => {
                        let edits: Vec<_> = edits.iter().map(|t| (to16(&t.range), t.new_text.clone())).collect();
                        let current = e.read(cx).lsp_buffer_version();
                        let applied = (current as u32 == *version) && e.update(cx, |ed, cx| ed.apply_lsp_edits_if_unchanged(current, &edits, cx));
                        if applied {
                            Completion::Done
                        } else {
                            Completion::Failed(v::Error::Invalid("the document changed since that version; read it again".into()))
                        }
                    }
                },
            },
            Start::SetSelections { uri, selections } => match editor_for(self, uri) {
                None => Completion::Failed(v::Error::NotFound(format!("{uri} is not open"))),
                Some((e, path)) => match may(&path, false) {
                    Err(err) => Completion::Failed(err),
                    Ok(()) => {
                        if let Some(r) = selections.first() {
                            e.update(cx, |ed, cx| ed.set_lsp_selection(to16(r), cx));
                        }
                        Completion::Done
                    }
                },
            },
            _ => Completion::Failed(v::Error::Invalid("not a document call".into())),
        }
    }
}

/// The extension's own settings, as JSON: the user's value (`ext.<id>.<key>` in settings.jsonc) or the default.
fn extension_settings(active: &crate::ext::Active, cx: &App) -> Vec<(String, Option<String>)> {
    let user = &cx.global::<Settings>().ext_settings;
    active
        .manifest
        .contributions
        .settings
        .iter()
        .map(|s| {
            let value = user
                .get(&format!("ext.{}.{}", active.id(), s.key))
                .map(|v| v.to_string())
                .or_else(|| s.default.as_ref().and_then(|d| serde_json::to_string(d).ok()));
            (s.key.clone(), value)
        })
        .collect()
}

/// An API error in plain words, with its fix when it is a denial.
pub(super) fn explain(e: &v::Error) -> String {
    match e {
        v::Error::Denied(d) => crate::i18n::tf("denied ({0}); add to [permissions]: {1}", &[&d.why.as_str(), &d.permission]),
        other => other.to_string(),
    }
}
