//! Debugger UI (F5): breakpoints, session, stopped line, call stack,
//! variables and the debug console. The protocol lives in `dap.rs`, configuration in `debug_config.rs`.
//!
//! Flow: F5 → find a configuration (launch.json / project) → (if any) pre-build → start the adapter →
//! `begin` (including breakpoints) → events are handled in `on_dap_event`. When stopped, the stack and
//! variables are requested in the background and the file is opened at the stopped line.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, mpsc};

use futures::StreamExt;
use gpui::{AnyElement, Context, Entity, Window, div, prelude::*, px};

use super::{Bottom, Notice, OverlayKind, Workbench};
use crate::dap::{self, DapEvent, Frame, Scope, Variable};
use crate::debug_config::{self, LaunchConfig};
use crate::input::{FieldEvent, TextField};
use crate::picker::{PickItem, Picker, Source as PickSource};
use crate::ui::{self, Palette, c, icon, layout, text};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum DebugStatus {
    Starting,
    Running,
    /// Reason for stopping ("breakpoint", "step", "exception"…).
    Stopped(String),
    Ended(String),
}

pub(super) struct DebugState {
    /// Session id: answers and events carry it, so those of a session that is gone are dropped.
    pub id: u64,
    pub config: LaunchConfig,
    pub session: Option<Arc<dap::Session>>,
    pub status: DebugStatus,
    pub thread: i64,
    pub frames: Vec<Frame>,
    /// Selected frame.
    pub frame: usize,
    pub scopes: Vec<Scope>,
    /// `variablesReference` → children (including scopes).
    pub children: HashMap<i64, Vec<Variable>>,
    pub expanded: HashSet<i64>,
    /// (category, line): "stdout", "stderr", "console".
    pub console: Vec<(String, String)>,
    /// Last output chunk not terminated by a newline (category, text): moves to the console once the line is complete.
    pub partial: Option<(String, String)>,
    /// Whether the session has stopped at least once (the user is told if no breakpoint was ever hit).
    pub stopped_once: bool,
    /// How many breakpoints there were at session start.
    pub breakpoints_set: usize,
    /// Stopped location: (absolute path, 0-based line).
    pub location: Option<(String, usize)>,
    pub input: Entity<TextField>,
    /// Latest values of the watch expressions (same order as `Workbench::watch_exprs`).
    pub watch_values: Vec<String>,
    /// The program's threads and the (selected) thread `thread` reported by the stop.
    pub threads: Vec<(i64, String)>,
    /// Data breakpoints (bound to the session): (dataId, description).
    pub data_breakpoints: Vec<(String, String)>,
}

/// Most sessions kept; the oldest ended one goes first.
const MAX_SESSIONS: usize = 8;

impl super::state_glue::DebugUi {
    /// The session the controls, the toolbar and the editors follow: the focused one, else the newest.
    pub(super) fn cur(&self) -> Option<&DebugState> {
        self.focused.and_then(|id| self.by_id(id)).or(self.sessions.last())
    }

    pub(super) fn cur_mut(&mut self) -> Option<&mut DebugState> {
        let id = self.cur()?.id;
        self.by_id_mut(id)
    }

    /// Id of the current session (0: none, which no session has).
    pub(super) fn cur_id(&self) -> u64 {
        self.cur().map_or(0, |d| d.id)
    }

    pub(super) fn by_id(&self, id: u64) -> Option<&DebugState> {
        self.sessions.iter().find(|d| d.id == id)
    }

    pub(super) fn by_id_mut(&mut self, id: u64) -> Option<&mut DebugState> {
        self.sessions.iter_mut().find(|d| d.id == id)
    }

    pub(super) fn has_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }

    /// Sessions that can take requests (started, not ended).
    pub(super) fn live(&self) -> impl Iterator<Item = Arc<dap::Session>> + '_ {
        self.sessions.iter().filter(|d| !matches!(d.status, DebugStatus::Ended(_) | DebugStatus::Starting)).filter_map(|d| d.session.clone())
    }
}

impl DebugState {
    /// The one way a line reaches the console: a line is cut to `LINE_LIMIT` characters and the console keeps `CONSOLE_LIMIT` lines.
    fn push_console(&mut self, category: &str, line: impl AsRef<str>) {
        console_push(&mut self.console, category, line.as_ref());
    }

    /// Output chunk (the adapter may split anywhere): full lines go to the console, the rest waits for the next chunk.
    fn append_output(&mut self, category: &str, text: &str) {
        console_append(&mut self.console, &mut self.partial, category, text);
    }

    fn flush_partial(&mut self) {
        if let Some((c, p)) = self.partial.take() {
            self.push_console(&c, p);
        }
    }
}

type ConsoleLines = Vec<(String, String)>;

fn console_push(console: &mut ConsoleLines, category: &str, line: &str) {
    console.push((category.to_string(), crate::lsp::clip_text(line, LINE_LIMIT)));
    let extra = console.len().saturating_sub(CONSOLE_LIMIT);
    console.drain(..extra);
}

/// A rest longer than `PARTIAL_LIMIT` bytes is moved to the console as it is, so a program that never ends its line cannot grow it.
fn console_append(console: &mut ConsoleLines, partial: &mut Option<(String, String)>, category: &str, text: &str) {
    let mut buf = match partial.take() {
        Some((c, p)) if c == category => p,
        Some((c, p)) => {
            console_push(console, &c, &p);
            String::new()
        }
        None => String::new(),
    };
    buf.push_str(text);
    let mut lines: Vec<&str> = buf.split('\n').collect();
    let mut rest = lines.pop().unwrap_or_default();
    for line in lines {
        console_push(console, category, line.trim_end_matches('\r'));
    }
    if rest.len() > PARTIAL_LIMIT {
        console_push(console, category, rest);
        rest = "";
    }
    if !rest.is_empty() {
        *partial = Some((category.to_string(), rest.to_string()));
    }
}

/// Display form of the adapter's stop reason (DAP `reason`); unknown reasons pass through (code compares the raw `reason`).
fn stop_reason(reason: &str) -> String {
    use crate::i18n::{t, tr};
    let known = [
        tr("breakpoint"),
        tr("step"),
        tr("pause"),
        tr("exception"),
        tr("entry"),
        tr("goto"),
        tr("function breakpoint"),
        tr("data breakpoint"),
        tr("instruction breakpoint"),
    ];
    known.iter().find(|k| **k == reason).map_or_else(|| reason.to_string(), |k| t(k).to_string())
}

gpui::actions!(workbench, [ToggleBreakpointEnabled, EnableAllBreakpoints, DisableAllBreakpoints, ToggleBreakpointsActive, AddFunctionBreakpoint]);

const CONSOLE_LIMIT: usize = 5000;
/// Longest console line (characters) and longest unfinished line (bytes) kept.
const LINE_LIMIT: usize = 4000;
const PARTIAL_LIMIT: usize = 64 * 1024;
/// Debug output (bytes) that may wait for the UI before more is dropped.
const OUTPUT_BACKLOG: usize = 8 * 1024 * 1024;
/// Number of lines last drawn in the panel.
const CONSOLE_SHOWN: usize = 300;

/// Launch error: (message, pre-build output if any).
type StartError = (String, String);

/// In the background: pre-build + adapter process.
fn launch_session(host: Arc<dyn host::Host>, config: &LaunchConfig, root: &str, tx: mpsc::Sender<DapEvent>) -> Result<Arc<dap::Session>, StartError> {
    if let Some(cmd) = &config.pre_launch {
        // On local Windows the shell is `cmd`; everywhere else (including SSH servers) it is `sh`.
        let (shell, flag) = if cfg!(windows) && !host.is_remote() { ("cmd", "/C") } else { ("sh", "-c") };
        let out = host.exec(shell, &[flag, cmd], Some(root), None).map_err(|e| (crate::i18n::tf("Could not run “{}”: {}", &[&cmd, &e]), String::new()))?;
        if !out.ok() {
            return Err((crate::i18n::tf("Build failed: {}", &[&cmd]), format!("{}{}", out.stdout_str(), out.stderr_str())));
        }
    }
    let adapter = config.adapter.clone().map_err(|e| (e, String::new()))?;
    if let Some((host_name, port)) = &adapter.connect {
        return dap::Session::attach_socket(host, host_name, *port, tx).map_err(|e| (e, String::new()));
    }
    // lldb resolves a relative source path to an absolute one if the file exists in the adapter's cwd, which
    // breaks matching against debug info built with the logical path. So the adapter starts outside the project
    // (the program's own `cwd` goes in the `launch` arguments).
    let neutral = if cfg!(windows) && !host.is_remote() { std::env::temp_dir().to_string_lossy().into_owned() } else { "/".to_string() };
    let adapter_cwd = if adapter.id == "lldb-dap" { neutral } else { root.to_string() };
    dap::Session::start(host, &adapter.command, adapter.args, Some(adapter_cwd), tx)
        .map_err(|e| (crate::i18n::tf("Could not start the debug adapter: {}", &[&e]), String::new()))
}

/// Run window-bound work from an async context (e.g. opening a file).
fn with_window(this: &gpui::WeakEntity<Workbench>, cx: &mut gpui::AsyncApp, f: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) {
    let Ok(handle) = this.update(cx, |t, _| t.window_handle) else { return };
    let this = this.clone();
    cx.update_window(handle, |_, window, cx| {
        this.update(cx, |t, cx| f(t, window, cx)).ok();
    })
    .ok();
}

impl Workbench {
    // --- Breakpoints ---

    /// Take an editor's breakpoints into the store (their lines move with edits); saved when they changed.
    pub(super) fn sync_breakpoints_from(&mut self, ed: &Entity<crate::editor::EditorView>, cx: &mut Context<Self>) -> bool {
        let (path, list) = {
            let e = ed.read(cx);
            (e.path.clone(), e.breakpoints())
        };
        if list.is_empty() && self.dbg.bps.file(&path).is_empty() {
            return false;
        }
        let changed = self.dbg.bps.set_file(&path, list);
        if changed {
            self.save_breakpoints(&path, cx);
        }
        changed
    }

    /// A breakpoint changed in an editor: update the store and, if a session is open, send the file's list.
    pub(super) fn on_breakpoints_changed(&mut self, ed: &Entity<crate::editor::EditorView>, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        self.sync_breakpoints_from(ed, cx);
        self.push_breakpoints(&path);
        cx.notify();
    }

    /// Report a file's breakpoints to every running session.
    fn push_breakpoints(&self, path: &str) {
        // Main thread, no waiting for the reply: a later `continue` can't overtake this request.
        let wire = self.dbg.bps.wire(path);
        for session in self.dbg.live() {
            session.set_breakpoints_async(path, &wire);
        }
    }

    /// The store changed a file's breakpoints (list actions, adapter results): show them in its open editors, send them
    /// and save them.
    pub(super) fn breakpoints_changed_in_store(&mut self, path: &str, send: bool, cx: &mut Context<Self>) {
        let list = self.dbg.bps.file(path).to_vec();
        let editors: Vec<_> = self.all_editors().into_iter().filter(|e| e.read(cx).path == path).collect();
        for ed in editors {
            ed.update(cx, |e, cx| e.set_breakpoints(&list, cx));
        }
        if send {
            self.push_breakpoints(path);
            self.save_breakpoints(path, cx);
        }
        cx.notify();
    }

    /// Give a newly opened editor this file's breakpoints and the stopped line.
    pub(super) fn apply_breakpoints_to(&mut self, ed: &Entity<crate::editor::EditorView>, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        let list = self.dbg.bps.file(&path).to_vec();
        if !list.is_empty() {
            ed.update(cx, |e, cx| e.set_breakpoints(&list, cx));
        }
        self.apply_debug_line_all(cx);
    }

    /// Writes one file's breakpoints to the project's saved state (files outside the folders are not kept).
    fn save_breakpoints(&self, path: &str, cx: &gpui::App) {
        let (Some(s), Some(ws)) = (&self.persist.state, self.persist.workspace_id) else { return };
        let Some(rel) = self.rel(path, cx) else { return };
        let list: Vec<state::SavedBreakpoint> = self
            .dbg
            .bps
            .file(path)
            .iter()
            .map(|(line, m)| state::SavedBreakpoint {
                rel_path: rel.clone(),
                line: *line,
                enabled: !m.disabled,
                condition: m.condition.clone(),
                hit: m.hit.clone(),
                log: m.log.clone(),
            })
            .collect();
        let _ = s.save_file_breakpoints(ws, &rel, &list);
    }

    /// Function breakpoints and the "active" switch live in the key-value store.
    fn save_breakpoint_settings(&self) {
        let (Some(s), Some(ws)) = (&self.persist.state, self.persist.workspace_id) else { return };
        let names: Vec<&str> = self.dbg.bps.functions.iter().map(|f| f.name.as_str()).collect();
        let _ = s.set(&format!("dbg.functions:{ws}"), &names.join("\n"));
        let _ = s.set(&format!("dbg.inactive:{ws}"), if self.dbg.bps.active { "" } else { "1" });
    }

    /// A folder was opened: its watches, exception filters and breakpoints come back.
    pub(super) fn load_debug_state(&mut self, cx: &mut Context<Self>) {
        self.load_watches();
        self.load_exception_filters();
        let old = self.dbg.bps.remove_all();
        self.dbg.bps = crate::debug_model::BreakpointStore::default();
        if let (Some(s), Some(ws), Some(root)) = (&self.persist.state, self.persist.workspace_id, self.root(cx)) {
            let mut files: BTreeMap<String, Vec<(usize, crate::editor::BpMeta)>> = BTreeMap::new();
            for b in s.breakpoints(ws) {
                let meta = crate::editor::BpMeta { condition: b.condition, log: b.log, hit: b.hit, disabled: !b.enabled, ..Default::default() };
                files.entry(ui::join(&root, &b.rel_path)).or_default().push((b.line, meta));
            }
            for (path, list) in files {
                self.dbg.bps.set_file(&path, list);
            }
            for name in s.get(&format!("dbg.functions:{ws}")).unwrap_or_default().lines() {
                self.dbg.bps.add_function(name);
            }
            self.dbg.bps.active = s.get(&format!("dbg.inactive:{ws}")).is_none_or(|v| v.is_empty());
        }
        for path in old.into_iter().chain(self.dbg.bps.paths()) {
            self.breakpoints_changed_in_store(&path, false, cx);
        }
    }

    /// "Debug: Enable/Disable Breakpoint": the breakpoint on the cursor's line.
    pub(super) fn toggle_breakpoint_enabled(&mut self, _: &ToggleBreakpointEnabled, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let (path, line) = {
            let e = ed.read(cx);
            (e.path.clone(), e.cursor_position().0 - 1)
        };
        self.sync_breakpoints_from(&ed, cx);
        let Some(on) = self.dbg.bps.file(&path).iter().find(|(l, _)| *l == line).map(|(_, m)| m.disabled) else {
            self.notice = Some(Notice::Info(crate::i18n::t("There is no breakpoint on this line (F9 adds one).").into()));
            cx.notify();
            return;
        };
        self.set_breakpoint_enabled(path, line, on, cx);
    }

    pub(super) fn set_breakpoint_enabled(&mut self, path: String, line: usize, on: bool, cx: &mut Context<Self>) {
        self.dbg.bps.set_enabled(&path, line, on);
        self.breakpoints_changed_in_store(&path, true, cx);
    }

    fn set_all_breakpoints_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            self.sync_breakpoints_from(&ed, cx);
        }
        self.dbg.bps.set_all_enabled(on);
        for path in self.dbg.bps.paths() {
            self.breakpoints_changed_in_store(&path, true, cx);
        }
        self.push_function_breakpoints();
    }

    pub(super) fn enable_all_breakpoints(&mut self, _: &EnableAllBreakpoints, _: &mut Window, cx: &mut Context<Self>) {
        self.set_all_breakpoints_enabled(true, cx);
    }

    pub(super) fn disable_all_breakpoints(&mut self, _: &DisableAllBreakpoints, _: &mut Window, cx: &mut Context<Self>) {
        self.set_all_breakpoints_enabled(false, cx);
    }

    /// "Debug: Deactivate/Activate Breakpoints": the list stays, but nothing is sent while deactivated.
    pub(super) fn toggle_breakpoints_active(&mut self, _: &ToggleBreakpointsActive, _: &mut Window, cx: &mut Context<Self>) {
        self.dbg.bps.active = !self.dbg.bps.active;
        for path in self.dbg.bps.paths() {
            self.push_breakpoints(&path);
        }
        self.push_function_breakpoints();
        self.save_breakpoint_settings();
        self.notice = Some(Notice::Info(if self.dbg.bps.active {
            crate::i18n::t("Breakpoints are active.").into()
        } else {
            crate::i18n::t("Breakpoints are deactivated: the program runs past them.").into()
        }));
        cx.notify();
    }

    pub(super) fn add_function_breakpoint(&mut self, _: &AddFunctionBreakpoint, window: &mut Window, cx: &mut Context<Self>) {
        self.show_input(super::InputPurpose::FunctionBreakpoint, crate::i18n::t("Add"), crate::i18n::t("function name, e.g. main or app::run"), "", window, cx);
    }

    pub(super) fn finish_function_breakpoint(&mut self, name: String, cx: &mut Context<Self>) {
        if self.dbg.bps.add_function(&name) {
            self.push_function_breakpoints();
            self.save_breakpoint_settings();
            cx.notify();
        }
    }

    #[expect(dead_code, reason = "used by the Run and Debug view")]
    pub(super) fn remove_function_breakpoint(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.dbg.bps.functions.len() {
            self.dbg.bps.functions.remove(index);
            self.push_function_breakpoints();
            self.save_breakpoint_settings();
            cx.notify();
        }
    }

    #[expect(dead_code, reason = "used by the Run and Debug view")]
    pub(super) fn set_function_breakpoint_enabled(&mut self, index: usize, on: bool, cx: &mut Context<Self>) {
        if let Some(f) = self.dbg.bps.functions.get_mut(index) {
            f.disabled = !on;
            self.push_function_breakpoints();
            cx.notify();
        }
    }

    fn push_function_breakpoints(&self) {
        let wire = self.dbg.bps.wire_functions();
        for session in self.dbg.live() {
            session.set_function_breakpoints_async(&wire);
        }
    }

    /// "Debug: Edit Breakpoint…": condition, `log: message` or `hit: count` for the cursor's line.
    pub(super) fn edit_breakpoint(&mut self, _: &super::EditBreakpoint, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let (path, line) = {
            let e = ed.read(cx);
            (e.path.clone(), e.cursor_position().0 - 1)
        };
        self.start_edit_breakpoint(path, line, window, cx);
    }

    pub(super) fn start_edit_breakpoint(&mut self, path: String, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.dbg.bps.file(&path).iter().find(|(l, _)| *l == line).map(|(_, m)| m.clone()).unwrap_or_default();
        let initial = crate::debug_model::breakpoint_meta_text(&current);
        self.show_input(
            super::InputPurpose::EditBreakpoint { path, line },
            crate::i18n::t("Set"),
            crate::i18n::t("condition  ·  log: message with {expr}  ·  hit: 5  ·  empty: plain breakpoint"),
            &initial,
            window,
            cx,
        );
    }

    pub(super) fn finish_edit_breakpoint(&mut self, path: String, line: usize, text: String, cx: &mut Context<Self>) {
        let meta = crate::debug_model::parse_breakpoint_meta(&text);
        self.dbg.bps.set_meta(&path, line, meta);
        self.breakpoints_changed_in_store(&path, true, cx);
    }

    /// "Debug: Install JavaScript Debugger": downloads Microsoft's js-debug adapter (MIT) into the settings folder.
    pub(super) fn install_js_debug(&mut self, _: &super::InstallJsDebug, _: &mut Window, cx: &mut Context<Self>) {
        if debug_config::find_js_debug().is_some() {
            self.notice = Some(Notice::Info(crate::i18n::t("The JavaScript debugger is already installed.").into()));
            cx.notify();
            return;
        }
        self.notice = Some(Notice::Info(crate::i18n::t("Downloading Microsoft's js-debug (about 1.2 MB)…").into()));
        cx.notify();
        let dir = debug_config::adapters_dir();
        let task = cx.background_executor().spawn(async move {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            // `curl` and `tar` ship with macOS, Linux and Windows 10+; no shell pipe.
            let archive = dir.join("js-debug-dap.tar.gz");
            // Cap time and size so a stalled network or a huge response can't hang the app.
            let fetched = std::process::Command::new("curl")
                .args(["-fsSL", "--max-time", "120", "--max-filesize", "20000000", "-o"])
                .arg(&archive)
                .arg(debug_config::JS_DEBUG_URL)
                .output()
                .map_err(|e| format!("curl: {e}"))?;
            if !fetched.status.success() {
                let _ = std::fs::remove_file(&archive);
                return Err(String::from_utf8_lossy(&fetched.stderr).trim().to_string());
            }
            let result = debug_config::install_js_debug_archive(&dir, &archive, debug_config::JS_DEBUG_SHA256);
            let _ = std::fs::remove_file(&archive);
            result
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.notice = Some(match result {
                    Ok(()) if debug_config::find_js_debug().is_some() => {
                        Notice::Info(crate::i18n::t("JavaScript debugger installed: F5 now works for Node.").into())
                    }
                    Ok(()) => Notice::Error(crate::i18n::t("The download finished but js-debug was not found in it.").into()),
                    Err(e) => Notice::Error(crate::i18n::tf("Could not download js-debug: {}", &[&e]).into()),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // --- Exception breakpoints ---

    /// "Debug: Break on Exceptions…": filters offered by the adapter (while a session is open), toggled on selection.
    pub(super) fn break_on_exceptions(&mut self, _: &super::BreakOnExceptions, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.dbg.cur().and_then(|d| d.session.clone()) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Start a debug session first (F5): the adapter says which exceptions it can break on.").into()));
            cx.notify();
            return;
        };
        let available = session.exception_filters();
        if available.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("This debug adapter has no exception breakpoints.").into()));
            cx.notify();
            return;
        }
        let selected = self.selected_exception_filters(&available);
        let items = available
            .iter()
            .map(|(id, label, _)| PickItem {
                label: format!("{} {label}", if selected.contains(id) { "✓" } else { "·" }).into(),
                detail: Some(id.clone().into()),
                keys: None,
            })
            .collect();
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Toggle a break-on-exception filter"), "", None, cx));
        self.show_overlay(picker, OverlayKind::ExceptionFilters(available.into_iter().map(|(id, label, _)| (id, label)).collect()), window, cx);
    }

    /// Selected filters: the user's choice if any, otherwise the adapter's defaults.
    fn selected_exception_filters(&self, available: &[(String, String, bool)]) -> Vec<String> {
        match &self.dbg.exception_filters {
            Some(p) => p.clone(),
            None => available.iter().filter(|(.., d)| *d).map(|(id, ..)| id.clone()).collect(),
        }
    }

    pub(super) fn toggle_exception_filter(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(session) = self.dbg.cur().and_then(|d| d.session.clone()) else { return };
        let available = session.exception_filters();
        let mut selected = self.selected_exception_filters(&available);
        match selected.iter().position(|f| *f == id) {
            Some(i) => {
                selected.remove(i);
            }
            None => selected.push(id),
        }
        session.set_exception_filters(&selected);
        self.dbg.exception_filters = Some(selected.clone());
        if let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) {
            let _ = s.set(&format!("exfilters:{ws}"), &selected.join("\n"));
            let _ = s.set(&format!("exfilters-set:{ws}"), "1");
        }
        self.notice = Some(Notice::Info(if selected.is_empty() {
            crate::i18n::t("Not breaking on exceptions.").into()
        } else {
            crate::i18n::tf("Breaking on: {}", &[&selected.join(", ")]).into()
        }));
        cx.notify();
    }

    pub(super) fn load_exception_filters(&mut self) {
        self.dbg.exception_filters = match (self.persist.workspace_id, &self.persist.state) {
            (Some(ws), Some(s)) if s.get(&format!("exfilters-set:{ws}")).is_some() => {
                Some(s.get(&format!("exfilters:{ws}")).map(|t| t.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default())
            }
            _ => None,
        };
    }

    // --- Data breakpoints ---

    /// "Debug: Break When Value Changes…": variable name; the program stops when the value is written (hardware watchpoint).
    pub(super) fn add_data_breakpoint(&mut self, _: &super::AddDataBreakpoint, window: &mut Window, cx: &mut Context<Self>) {
        let ready = self.dbg.cur().is_some_and(|d| matches!(d.status, DebugStatus::Stopped(_)));
        if !ready {
            self.notice = Some(Notice::Info(crate::i18n::t("Pause the program first (a breakpoint or ⏸): the variable has to be in scope.").into()));
            cx.notify();
            return;
        }
        self.show_input(
            super::InputPurpose::DataBreakpoint,
            crate::i18n::t("Watch"),
            crate::i18n::t("variable name, e.g. total (the program stops when it is written)"),
            "",
            window,
            cx,
        );
    }

    pub(super) fn finish_data_breakpoint(&mut self, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        let sid = self.dbg.cur_id();
        let Some(d) = self.dbg.cur_mut().filter(|d| matches!(d.status, DebugStatus::Stopped(_))) else { return };
        let Some(session) = d.session.clone() else { return };
        if name.is_empty() {
            return;
        }
        if !session.supports_data_breakpoints() {
            self.notice = Some(Notice::Info(crate::i18n::t("This debug adapter does not support data breakpoints.").into()));
            cx.notify();
            return;
        }
        // Scope / object containing the variable: search the loaded child lists, otherwise the first scope.
        let reference = d
            .scopes
            .iter()
            .map(|s| s.reference)
            .chain(d.children.keys().copied())
            .find(|r| d.children.get(r).is_some_and(|vars| vars.iter().any(|v| v.name == name)))
            .or_else(|| d.scopes.first().map(|s| s.reference))
            .unwrap_or(0);
        let existing: Vec<String> = d.data_breakpoints.iter().map(|(id, _)| id.clone()).collect();
        let task = cx.background_executor().spawn(async move {
            let (id, description) = session.data_breakpoint_info(reference, &name)?;
            let mut ids = existing;
            if !ids.contains(&id) {
                ids.push(id.clone());
            }
            session.set_data_breakpoints(&ids)?;
            Ok::<_, String>((id, description))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((id, description)) => {
                        if let Some(d) = this.dbg.by_id_mut(sid) {
                            if !d.data_breakpoints.iter().any(|(i, _)| *i == id) {
                                d.data_breakpoints.push((id, description.clone()));
                            }
                            d.push_console("console", crate::i18n::tf("Data breakpoint set: the program stops when {} is written.", &[&description]));
                        }
                    }
                    Err(e) => this.notice = Some(Notice::Error(crate::i18n::tf("Data breakpoint: {}", &[&e]).into())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn remove_data_breakpoints(&mut self, _: &super::RemoveDataBreakpoints, _: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.dbg.cur_mut() else { return };
        if d.data_breakpoints.is_empty() {
            return;
        }
        d.data_breakpoints.clear();
        if let Some(session) = d.session.clone() {
            session.set_data_breakpoints_async(&[]);
        }
        cx.notify();
    }

    // --- Watch expressions ---

    pub(super) fn add_watch(&mut self, _: &super::AddWatch, window: &mut Window, cx: &mut Context<Self>) {
        self.show_input(
            super::InputPurpose::AddWatch,
            crate::i18n::t("Watch"),
            crate::i18n::t("expression to watch (e.g. total * 2, items.len())"),
            "",
            window,
            cx,
        );
    }

    pub(super) fn finish_add_watch(&mut self, expr: String, cx: &mut Context<Self>) {
        let expr = expr.trim().to_string();
        if expr.is_empty() || self.dbg.watch_exprs.contains(&expr) {
            return;
        }
        self.dbg.watch_exprs.push(expr);
        self.persist_watches();
        self.chrome.bottom = Bottom::Debug;
        self.chrome.panel_open = true;
        self.refresh_watches(cx);
        cx.notify();
    }

    pub(super) fn remove_watch(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.dbg.watch_exprs.len() {
            self.dbg.watch_exprs.remove(i);
            if let Some(d) = self.dbg.cur_mut()
                && i < d.watch_values.len()
            {
                d.watch_values.remove(i);
            }
            self.persist_watches();
            cx.notify();
        }
    }

    pub(super) fn remove_all_watches(&mut self, _: &super::RemoveAllWatches, _: &mut Window, cx: &mut Context<Self>) {
        self.dbg.watch_exprs.clear();
        if let Some(d) = self.dbg.cur_mut() {
            d.watch_values.clear();
        }
        self.persist_watches();
        cx.notify();
    }

    fn persist_watches(&self) {
        if let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) {
            let _ = s.set(&format!("watches:{ws}"), &self.dbg.watch_exprs.join("\n"));
        }
    }

    pub(super) fn load_watches(&mut self) {
        self.dbg.watch_exprs = match (self.persist.workspace_id, &self.persist.state) {
            (Some(ws), Some(s)) => {
                s.get(&format!("watches:{ws}")).map(|t| t.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
            }
            _ => Vec::new(),
        };
    }

    /// Evaluate each watch expression in the stopped frame (in the background); results arrive in the panel in order.
    pub(super) fn refresh_watches(&mut self, cx: &mut Context<Self>) {
        let sid = self.dbg.cur_id();
        let exprs = self.dbg.watch_exprs.clone();
        let Some(d) = self.dbg.cur_mut() else { return };
        if exprs.is_empty() {
            d.watch_values.clear();
            return;
        }
        let (Some(session), true) = (d.session.clone(), matches!(d.status, DebugStatus::Stopped(_))) else {
            d.watch_values = exprs.iter().map(|_| crate::i18n::t("not available").to_string()).collect();
            return;
        };
        let frame = d.frames.get(d.frame).map(|f| f.id);
        let task = cx.background_executor().spawn(async move {
            exprs.iter().map(|e| session.evaluate(frame, e, "watch").map(|(v, _)| v).unwrap_or_else(|err| format!("<{err}>"))).collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let values = task.await;
            this.update(cx, |this, cx| {
                if values.len() == this.dbg.watch_exprs.len()
                    && let Some(d) = this.dbg.by_id_mut(sid)
                {
                    d.watch_values = values;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn remove_all_breakpoints(&mut self, _: &super::RemoveAllBreakpoints, _: &mut Window, cx: &mut Context<Self>) {
        for path in self.dbg.bps.remove_all() {
            self.breakpoints_changed_in_store(&path, true, cx);
        }
        for session in self.dbg.live() {
            session.set_function_breakpoints_async(&[]);
        }
        self.save_breakpoint_settings();
        cx.notify();
    }

    fn apply_debug_line_all(&mut self, cx: &mut Context<Self>) {
        let location = self.dbg.cur().and_then(|d| d.location.clone());
        // Locals of the stopped frame: variables of the first (Locals) scope.
        let locals: Vec<(String, String)> = self
            .dbg
            .cur()
            .and_then(|d| d.scopes.first().and_then(|s| d.children.get(&s.reference)))
            .map(|v| v.iter().map(|v| (v.name.clone(), v.value.clone())).collect())
            .unwrap_or_default();
        // Value on hover if stopped: session and selected frame.
        let eval = self.dbg.cur().and_then(|d| match (&d.status, &d.session) {
            (DebugStatus::Stopped(_), Some(s)) => d.frames.get(d.frame).map(|f| (s.clone(), f.id)),
            _ => None,
        });
        for ed in self.all_editors() {
            let e2 = eval.clone();
            ed.update(cx, |e, _| e.set_debug_eval(e2));
            let here = location.as_ref().filter(|(p, _)| *p == ed.read(cx).path).map(|(_, l)| *l);
            let values = here.map(|stop| inline_values(&ed.read(cx).text_lines(), stop, &locals)).unwrap_or_default();
            ed.update(cx, |e, cx| {
                e.set_debug_line(here, cx);
                e.set_debug_values(values, cx);
            });
        }
    }

    // --- Launch ---

    /// F5: continue if stopped; if not running, find the configuration and launch.
    pub(super) fn start_debugging(&mut self, _: &super::StartDebugging, _: &mut Window, cx: &mut Context<Self>) {
        match self.dbg.cur().map(|d| d.status.clone()) {
            Some(DebugStatus::Stopped(_)) => return self.debug_control("continue", cx),
            Some(DebugStatus::Starting | DebugStatus::Running) => {
                self.notice = Some(Notice::Info(crate::i18n::t("Already debugging: Shift+F5 stops the session.").into()));
                cx.notify();
                return;
            }
            _ => {}
        }
        let (Some(root), host) = (self.active_root(cx), self.remote.host.clone()) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a folder to debug it.").into()));
            cx.notify();
            return;
        };
        let file = self.active_editor().map(|e| e.read(cx).path.clone());
        let read = cx.background_executor().spawn({
            let (root, file) = (root.clone(), file.clone());
            async move {
                let read = |rel: &str| host.read(&ui::join(&root, rel)).ok().map(|(d, _)| String::from_utf8_lossy(&d).into_owned());
                let mut tasks = Vec::new();
                for (path, source) in [(".vscode/tasks.json", ".vscode/tasks.json"), (".codeyard/tasks.jsonc", "tasks.jsonc")] {
                    if let Some(t) = read(path) {
                        tasks.extend(super::tasks::parse_task_file(&t, source));
                    }
                }
                let mut configs = Vec::new();
                for path in [".codeyard/launch.jsonc", ".vscode/launch.json"] {
                    if let Some(t) = read(path) {
                        configs.extend(debug_config::parse_launch_json(&t, &root, file.as_deref(), &tasks));
                    }
                }
                let cargo = read("Cargo.toml");
                let go = host.stat(&ui::join(&root, "go.mod")).is_ok();
                configs.extend(debug_config::detect(cargo.as_deref(), go, file.as_deref(), &root));
                configs
            }
        });
        cx.spawn(async move |this, cx| {
            let configs = read.await;
            with_window(&this, cx, move |this, window, cx| this.choose_debug_config(configs, window, cx));
        })
        .detach();
    }

    fn choose_debug_config(&mut self, configs: Vec<LaunchConfig>, window: &mut Window, cx: &mut Context<Self>) {
        self.require_trust(window, cx, move |this, window, cx| this.choose_debug_config_trusted(configs, window, cx));
    }

    fn choose_debug_config_trusted(&mut self, mut configs: Vec<LaunchConfig>, window: &mut Window, cx: &mut Context<Self>) {
        match configs.len() {
            0 => {
                self.notice = Some(Notice::Info(
                    crate::i18n::t("No debug configuration: add .vscode/launch.json, or open a Cargo / Go project or a Python file.").into(),
                ));
                cx.notify();
            }
            1 => self.start_debug(configs.remove(0), cx),
            _ => {
                // The last used configuration comes first.
                if let Some(last) = &self.dbg.last
                    && let Some(i) = configs.iter().position(|c| &c.name == last)
                {
                    let c = configs.remove(i);
                    configs.insert(0, c);
                }
                if self.overlay.is_some() {
                    self.close_overlay(window, cx);
                }
                let items = configs
                    .iter()
                    .map(|c| PickItem {
                        label: c.name.clone().into(),
                        detail: Some(c.adapter.as_ref().map(|a| a.id.clone()).unwrap_or_else(|e| e.chars().take(60).collect()).into()),
                        keys: None,
                    })
                    .collect();
                let picker =
                    cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Select a debug configuration"), "", None, cx));
                self.show_overlay(picker, OverlayKind::DebugConfigs(configs), window, cx);
            }
        }
    }

    pub(super) fn start_debug(&mut self, config: LaunchConfig, cx: &mut Context<Self>) {
        let Some(root) = self.active_root(cx) else { return };
        // Starting a configuration again replaces its session (restart); other sessions keep running.
        self.retire_sessions(&config.name, cx);
        self.dbg.last = Some(config.name.clone());
        self.dbg.next_id += 1;
        let sid = self.dbg.next_id;
        let (tx, rx) = mpsc::channel();
        let input = cx.new(|cx| TextField::new(crate::i18n::tr("Evaluate an expression (↵)"), cx));
        self._subs.push(cx.subscribe(&input, |this, input, ev: &FieldEvent, cx| {
            if matches!(ev, FieldEvent::Submit) {
                let text = input.read(cx).text().to_string();
                input.update(cx, |f, cx| f.set_text("", cx));
                this.debug_evaluate(text, cx);
            }
        }));
        for ed in self.all_editors() {
            self.sync_breakpoints_from(&ed, cx);
        }
        let breakpoints = self.dbg.bps.wire_all();
        let functions = self.dbg.bps.wire_functions();
        self.dbg.focused = Some(sid);
        self.dbg.sessions.push(DebugState {
            id: sid,
            config: config.clone(),
            session: None,
            status: DebugStatus::Starting,
            thread: 1,
            frames: Vec::new(),
            frame: 0,
            scopes: Vec::new(),
            children: HashMap::new(),
            expanded: HashSet::new(),
            console: vec![("console".into(), crate::lsp::clip_text(&crate::i18n::tf("Starting “{}”…", &[&config.name]), LINE_LIMIT))],
            partial: None,
            stopped_once: false,
            breakpoints_set: breakpoints.values().map(Vec::len).sum::<usize>() + functions.len(),
            location: None,
            input,
            watch_values: Vec::new(),
            threads: Vec::new(),
            data_breakpoints: Vec::new(),
        });
        self.chrome.bottom = Bottom::Debug;
        self.chrome.panel_open = true;
        // Event pump: handle events in order for the life of the session.
        let backlog = Arc::new(crate::terminal::OutputBacklog::with_limit(OUTPUT_BACKLOG));
        cx.spawn(async move |this, cx| {
            // Output the UI has not read yet is capped: past 8 MB it is dropped (and the console says so); other events always pass.
            let gate = backlog.clone();
            let mut events = ui::bridge_with(rx, move |ev| match ev {
                DapEvent::Output { text, .. } => gate.admit(text.len()),
                _ => true,
            });
            while let Some(ev) = events.next().await {
                let consumed = if let DapEvent::Output { text, .. } = &ev { text.len() } else { 0 };
                backlog.consumed(consumed);
                let told = backlog.take_dropped();
                let alive = this.update(cx, |this, cx| {
                    this.on_dap_event(sid, ev, cx);
                    if told && let Some(d) = this.dbg.by_id_mut(sid) {
                        d.flush_partial();
                        d.push_console("stderr", crate::i18n::t("debug output was dropped: the debugger could not keep up"));
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let host = self.remote.host.clone();
        let (cfg, root2, session_root) = (config.clone(), root.clone(), root.clone());
        let start = cx.background_executor().spawn(async move { launch_session(host, &cfg, &root2, tx) });
        cx.spawn(async move |this, cx| {
            let session = match start.await {
                Ok(s) => s,
                Err((message, output)) => {
                    this.update(cx, |this, cx| this.on_debug_failed(sid, message, output, cx)).ok();
                    return;
                }
            };
            let attached = this
                .update(cx, |this, _| {
                    let current = this.dbg.by_id(sid).is_some();
                    if let Some(d) = this.dbg.by_id_mut(sid) {
                        d.session = Some(session.clone());
                    }
                    if current {
                        session.set_root(&session_root);
                        session.set_exception_pref(this.dbg.exception_filters.clone());
                    }
                    current
                })
                .unwrap_or(false);
            if !attached {
                cx.background_executor().spawn(async move { session.stop() }).detach();
                return;
            }
            let adapter_id = config.adapter.as_ref().map(|a| a.id.clone()).unwrap_or_default();
            let (request, args) = (config.request.clone(), config.args.clone());
            let s2 = session.clone();
            let result = cx.background_executor().spawn(async move { s2.begin(&adapter_id, &request, args, &breakpoints, &functions) }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    if let Some(d) = this.dbg.by_id_mut(sid).filter(|d| d.status == DebugStatus::Starting) {
                        d.status = DebugStatus::Running;
                    }
                    cx.notify();
                }
                Err(e) => this.on_debug_failed(sid, crate::i18n::tf("Could not start debugging: {}", &[&e]), String::new(), cx),
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Launch failed: message in a Notice, pre-build output in the console and in Problems.
    fn on_debug_failed(&mut self, sid: u64, message: String, output: String, cx: &mut Context<Self>) {
        if self.dbg.by_id(sid).is_none() {
            return;
        }
        if !output.is_empty() {
            let root = self.active_root(cx).unwrap_or_default();
            let problems = crate::problems::parse_output(&output, &root);
            self.set_task_problems(problems, cx);
            if let Some(d) = self.dbg.by_id_mut(sid) {
                for line in output.lines().rev().take(60).collect::<Vec<_>>().into_iter().rev() {
                    d.push_console("stderr", line);
                }
            }
        }
        self.notice = Some(Notice::Error(message.clone().into()));
        self.end_debug(sid, message, cx);
    }

    // --- Events ---

    fn on_dap_event(&mut self, sid: u64, ev: DapEvent, cx: &mut Context<Self>) {
        let Some(d) = self.dbg.by_id_mut(sid) else { return };
        match ev {
            DapEvent::Initialized => {}
            DapEvent::Output { category, text } => {
                if category != "telemetry" {
                    d.append_output(&category, &text);
                }
            }
            DapEvent::Stopped { thread_id, reason, text } => {
                d.stopped_once = true;
                d.status = DebugStatus::Stopped(reason.clone());
                d.thread = thread_id;
                if let Some(t) = text.filter(|_| reason == "exception") {
                    d.push_console("stderr", crate::i18n::tf("Paused on exception: {}", &[&t]));
                }
                // The session that stopped is the one the controls and the editors follow.
                self.dbg.focused = Some(sid);
                self.refresh_stack(sid, thread_id, cx);
            }
            DapEvent::Continued => {
                if !matches!(d.status, DebugStatus::Ended(_)) {
                    d.status = DebugStatus::Running;
                }
                d.location = None;
                self.apply_debug_line_all(cx);
            }
            DapEvent::Exited(code) => {
                d.flush_partial();
                d.push_console("console", crate::i18n::tf("Process exited with code {}", &[&code]));
            }
            DapEvent::Terminated => {
                // Breakpoints were set but the program exited without stopping: say so instead of staying silent.
                if d.breakpoints_set > 0 && !d.stopped_once {
                    d.flush_partial();
                    d.push_console(
                        "stderr",
                        crate::i18n::t("The program finished without stopping at any breakpoint. The debugger matches breakpoints to the debug info by file path: build with debug info (-g) from this folder, and check that the breakpoint is on a line with code."),
                    );
                }
                self.end_debug(sid, crate::i18n::t("Debug session ended").into(), cx)
            }
            DapEvent::AdapterExited(code) => {
                if !matches!(d.status, DebugStatus::Ended(_)) {
                    let msg = match code {
                        Some(c) => crate::i18n::tf("The debug adapter stopped (exit code {})", &[&c]),
                        None => crate::i18n::t("The debug adapter stopped").to_string(),
                    };
                    self.end_debug(sid, msg, cx);
                }
            }
            DapEvent::BreakpointsSet { path, sent, results } => {
                self.dbg.bps.on_results(&path, &sent, &results);
                self.breakpoints_changed_in_store(&path, false, cx);
            }
            DapEvent::BreakpointChanged(r) => {
                if let Some(path) = self.dbg.bps.on_changed(&r) {
                    self.breakpoints_changed_in_store(&path, false, cx);
                }
            }
            DapEvent::FunctionBreakpointsSet(results) => self.dbg.bps.on_function_results(&results),
            DapEvent::Thread { .. } | DapEvent::Capabilities => {}
        }
        cx.notify();
    }

    /// When stopped: call stack, scopes of the first frame with a source, and variables of the first scope.
    fn refresh_stack(&mut self, sid: u64, thread: i64, cx: &mut Context<Self>) {
        let Some(session) = self.dbg.by_id(sid).and_then(|d| d.session.clone()) else { return };
        let local = !self.remote.host.is_remote();
        let task = cx.background_executor().spawn(async move {
            let mut frames = session.stack_trace(thread).unwrap_or_default();
            // The compiler may report the logical path (`/tmp/x`) while the editor holds the canonical one (`/private/tmp/x`).
            if local {
                for f in &mut frames {
                    if let Some(c) = f.path.as_deref().and_then(|p| std::fs::canonicalize(p).ok()) {
                        f.path = Some(c.to_string_lossy().into_owned());
                    }
                }
            }
            let top = frames.iter().position(|f| f.path.is_some()).unwrap_or(0);
            let (scopes, children) = frames.get(top).map(|f| load_scopes(&session, f.id)).unwrap_or_default();
            (frames, top, scopes, children, session.threads())
        });
        cx.spawn(async move |this, cx| {
            let (frames, top, scopes, children, threads) = task.await;
            with_window(&this, cx, move |this, window, cx| {
                let Some(d) = this.dbg.by_id_mut(sid).filter(|d| matches!(d.status, DebugStatus::Stopped(_))) else { return };
                d.location = frames.get(top).and_then(|f| f.path.clone().map(|p| (p, f.line.saturating_sub(1) as usize)));
                d.frames = frames;
                d.frame = top;
                d.threads = threads;
                d.scopes = scopes;
                d.expanded = d.scopes.first().map(|s| s.reference).into_iter().collect();
                d.children = children;
                this.show_debug_location(window, cx);
                this.refresh_watches(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn show_debug_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line)) = self.dbg.cur().and_then(|d| d.location.clone()) else { return };
        self.open_location(path, line as u32, 0, window, cx);
        self.apply_debug_line_all(cx);
    }

    /// Look at another thread's stack (the program stays stopped).
    pub(super) fn select_thread(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(d) = self.dbg.cur_mut().filter(|d| matches!(d.status, DebugStatus::Stopped(_))) {
            d.thread = id;
            let sid = d.id;
            self.refresh_stack(sid, id, cx);
        }
    }

    pub(super) fn select_frame(&mut self, i: usize, cx: &mut Context<Self>) {
        let sid = self.dbg.cur_id();
        let Some(d) = self.dbg.cur_mut() else { return };
        let (Some(frame), Some(session)) = (d.frames.get(i).cloned(), d.session.clone()) else { return };
        d.frame = i;
        d.location = frame.path.clone().map(|p| (p, frame.line.saturating_sub(1) as usize));
        let task = cx.background_executor().spawn(async move { load_scopes(&session, frame.id) });
        cx.spawn(async move |this, cx| {
            let (scopes, children) = task.await;
            with_window(&this, cx, move |this, window, cx| {
                let Some(d) = this.dbg.by_id_mut(sid) else { return };
                d.expanded = scopes.first().map(|s| s.reference).into_iter().collect();
                d.scopes = scopes;
                d.children = children;
                this.show_debug_location(window, cx);
                this.refresh_watches(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Toggle a variable or scope row; children are requested on first expansion.
    pub(super) fn toggle_variable(&mut self, reference: i64, cx: &mut Context<Self>) {
        let sid = self.dbg.cur_id();
        let Some(d) = self.dbg.cur_mut() else { return };
        if !d.expanded.insert(reference) {
            d.expanded.remove(&reference);
            cx.notify();
            return;
        }
        if d.children.contains_key(&reference) {
            cx.notify();
            return;
        }
        let Some(session) = d.session.clone() else { return };
        let task = cx.background_executor().spawn(async move { session.variables(reference).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let vars = task.await;
            this.update(cx, |this, cx| {
                if let Some(d) = this.dbg.by_id_mut(sid) {
                    d.children.insert(reference, vars);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn debug_evaluate(&mut self, expression: String, cx: &mut Context<Self>) {
        let expression = expression.trim().to_string();
        let sid = self.dbg.cur_id();
        let Some(d) = self.dbg.cur_mut() else { return };
        if expression.is_empty() {
            return;
        }
        d.push_console("input", format!("> {expression}"));
        let Some(session) = d.session.clone().filter(|_| !matches!(d.status, DebugStatus::Ended(_))) else {
            d.push_console("stderr", crate::i18n::t("No active debug session."));
            cx.notify();
            return;
        };
        let frame = matches!(d.status, DebugStatus::Stopped(_)).then(|| d.frames.get(d.frame).map(|f| f.id)).flatten();
        let task = cx.background_executor().spawn(async move { session.evaluate(frame, &expression, "repl") });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Some(d) = this.dbg.by_id_mut(sid) {
                    match result {
                        Ok((text, _)) => text.lines().for_each(|l| d.push_console("console", l)),
                        Err(e) => d.push_console("stderr", e),
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // --- Control ---

    /// `continue`, `next`, `stepIn`, `stepOut` (while stopped) and `pause` (while running).
    pub(super) fn debug_control(&mut self, command: &'static str, cx: &mut Context<Self>) {
        let Some(d) = self.dbg.cur_mut() else { return };
        let Some(session) = d.session.clone() else { return };
        let allowed = match command {
            "pause" => d.status == DebugStatus::Running,
            _ => matches!(d.status, DebugStatus::Stopped(_)),
        };
        if !allowed {
            return;
        }
        let thread = d.thread;
        if command != "pause" {
            d.status = DebugStatus::Running;
            d.location = None;
        }
        self.apply_debug_line_all(cx);
        session.control(command, thread);
        cx.notify();
    }

    pub(super) fn stop_debugging(&mut self, _: &super::StopDebugging, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(sid) = self.dbg.cur().filter(|d| !matches!(d.status, DebugStatus::Ended(_))).map(|d| d.id) {
            self.end_debug(sid, crate::i18n::t("Debug session stopped").into(), cx);
        }
    }

    pub(super) fn restart_debugging(&mut self, _: &super::RestartDebugging, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(config) = self.dbg.cur().map(|d| d.config.clone()) {
            self.start_debug(config, cx);
        }
    }

    /// End a session (adapter and program exit); console and message stay.
    fn end_debug(&mut self, sid: u64, message: String, cx: &mut Context<Self>) {
        if let Some(d) = self.dbg.by_id_mut(sid) {
            if let Some(s) = d.session.take() {
                cx.background_executor().spawn(async move { s.stop() }).detach();
            }
            d.flush_partial();
            if !matches!(d.status, DebugStatus::Ended(_)) {
                d.push_console("console", &message);
            }
            d.status = DebugStatus::Ended(message);
            d.frames.clear();
            d.scopes.clear();
            d.children.clear();
            d.location = None;
        }
        // Another session that is still running becomes the current one.
        if self.dbg.focused == Some(sid)
            && let Some(other) = self.dbg.sessions.iter().rev().find(|d| d.id != sid && !matches!(d.status, DebugStatus::Ended(_)))
        {
            self.dbg.focused = Some(other.id);
        }
        if self.dbg.live().next().is_none() {
            self.forget_breakpoint_states(cx);
        }
        self.apply_debug_line_all(cx);
        cx.notify();
    }

    /// No session: breakpoints are neither verified nor rejected any more.
    fn forget_breakpoint_states(&mut self, cx: &mut Context<Self>) {
        self.dbg.bps.clear_verified();
        for path in self.dbg.bps.paths() {
            self.breakpoints_changed_in_store(&path, false, cx);
        }
    }

    /// Folder change: close every session silently.
    pub(super) fn stop_debug_quietly(&mut self, cx: &mut Context<Self>) {
        for d in std::mem::take(&mut self.dbg.sessions) {
            if let Some(s) = d.session {
                cx.background_executor().spawn(async move { s.stop() }).detach();
            }
        }
        self.dbg.focused = None;
        self.forget_breakpoint_states(cx);
        self.apply_debug_line_all(cx);
    }

    /// Before a configuration starts: its earlier session (running or ended) goes, and so do the oldest ended sessions
    /// beyond the limit.
    fn retire_sessions(&mut self, name: &str, cx: &mut Context<Self>) {
        let (gone, kept): (Vec<DebugState>, Vec<DebugState>) = std::mem::take(&mut self.dbg.sessions).into_iter().partition(|d| d.config.name == name);
        self.dbg.sessions = kept;
        while self.dbg.sessions.len() >= MAX_SESSIONS {
            let i = self.dbg.sessions.iter().position(|d| matches!(d.status, DebugStatus::Ended(_))).unwrap_or(0);
            let d = self.dbg.sessions.remove(i);
            if let Some(s) = d.session {
                cx.background_executor().spawn(async move { s.stop() }).detach();
            }
        }
        for d in gone {
            if let Some(s) = d.session {
                cx.background_executor().spawn(async move { s.stop() }).detach();
            }
        }
    }

    // --- UI ---

    pub(super) fn debug_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(d) = self.dbg.cur() else {
            return div()
                .px(px(theme::SPACE[6]))
                .py(px(theme::SPACE[3]))
                .text_color(c(p.text_muted))
                .child(crate::i18n::t("No debug session. F5 starts one."))
                .into_any_element();
        };
        let stopped = matches!(d.status, DebugStatus::Stopped(_));
        let running = d.status == DebugStatus::Running;
        let live = !matches!(d.status, DebugStatus::Ended(_));
        let dim = |on: bool| if on { 1.0 } else { 0.35 };
        let toolbar = div()
            .h(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[1]))
            .px(px(theme::SPACE[3]))
            .border_b_1()
            .border_color(c(p.border))
            .child(div().opacity(dim(stopped || running)).child(if running {
                ui::icon_button("dbg-pause", "pause", crate::i18n::t("Pause"), p).on_click(cx.listener(|this, _, _, cx| this.debug_control("pause", cx)))
            } else {
                ui::icon_button("dbg-continue", "play", crate::i18n::t("Continue (F5)"), p)
                    .on_click(cx.listener(|this, _, _, cx| this.debug_control("continue", cx)))
            }))
            .child(
                div().opacity(dim(stopped)).child(
                    ui::icon_button("dbg-over", "step-over", crate::i18n::t("Step Over (F10)"), p)
                        .on_click(cx.listener(|this, _, _, cx| this.debug_control("next", cx))),
                ),
            )
            .child(
                div().opacity(dim(stopped)).child(
                    ui::icon_button("dbg-into", "step-into", crate::i18n::t("Step Into (F11)"), p)
                        .on_click(cx.listener(|this, _, _, cx| this.debug_control("stepIn", cx))),
                ),
            )
            .child(
                div().opacity(dim(stopped)).child(
                    ui::icon_button("dbg-out", "step-out", crate::i18n::t("Step Out (⇧F11)"), p)
                        .on_click(cx.listener(|this, _, _, cx| this.debug_control("stepOut", cx))),
                ),
            )
            .child(
                ui::icon_button("dbg-restart", "rotate-ccw", crate::i18n::t("Restart (⇧⌘F5)"), p)
                    .on_click(cx.listener(|this, _, w, cx| this.restart_debugging(&super::RestartDebugging, w, cx))),
            )
            .child(
                div().opacity(dim(live)).child(
                    ui::icon_button("dbg-stop", "square", crate::i18n::t("Stop (⇧F5)"), p)
                        .on_click(cx.listener(|this, _, w, cx| this.stop_debugging(&super::StopDebugging, w, cx))),
                ),
            )
            .child(div().w(px(theme::SPACE[3])))
            .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(match &d.status {
                DebugStatus::Starting => crate::i18n::tf("{}: starting…", &[&d.config.name]),
                DebugStatus::Running => crate::i18n::tf("{}: running", &[&d.config.name]),
                DebugStatus::Stopped(r) => crate::i18n::tf("{}: paused ({})", &[&d.config.name, &stop_reason(r)]),
                DebugStatus::Ended(m) => m.clone(),
            }));

        let header = |label: &'static str| {
            let label = crate::i18n::upper(crate::i18n::t(label));
            div()
                .h(px(layout::ROW_H))
                .flex_none()
                .flex()
                .items_center()
                .px(px(theme::SPACE[4]))
                .text_size(px(text::XXS.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c(p.text_muted))
                .child(label)
        };
        let column = |id: &'static str| div().id(id).h_full().min_w_0().flex_1().flex().flex_col().border_r_1().border_color(c(p.border));

        // Call stack (with a thread list on top if there are several threads).
        let thread_rows: Vec<AnyElement> = if stopped && d.threads.len() > 1 {
            d.threads
                .iter()
                .map(|(id, name)| {
                    let id = *id;
                    let selected = id == d.thread;
                    div()
                        .id(("thread", id as usize))
                        .h(px(layout::ROW_H))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[2]))
                        .px(px(theme::SPACE[4]))
                        .cursor_pointer()
                        .text_size(px(text::XS.0))
                        .when(selected, |d| d.bg(c(p.surface_active)))
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(icon("list", 12., p.text_muted))
                        .child(name.to_string())
                        .child(div().text_color(c(p.text_muted)).child(format!("#{id}")))
                        .on_click(cx.listener(move |this, _, _, cx| this.select_thread(id, cx)))
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let stack: Vec<AnyElement> = if stopped {
            d.frames
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let selected = i == d.frame;
                    let place = f.path.as_deref().map(|p| format!("{}:{}", ui::file_name(p), f.line)).unwrap_or_default();
                    div()
                        .id(("frame", i))
                        .h(px(layout::ROW_H))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[2]))
                        .px(px(theme::SPACE[4]))
                        .cursor_pointer()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_size(px(text::XS.0))
                        .when(selected, |d| d.bg(c(p.surface_active)))
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .when(f.path.is_none(), |d| d.text_color(c(p.text_muted)))
                        .child(f.name.clone())
                        .child(div().text_color(c(p.text_muted)).child(place))
                        .on_click(cx.listener(move |this, _, _, cx| this.select_frame(i, cx)))
                        .into_any_element()
                })
                .collect()
        } else {
            vec![
                div()
                    .px(px(theme::SPACE[4]))
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .child(if running { crate::i18n::t("Running…") } else { crate::i18n::t("Not paused") })
                    .into_any_element(),
            ]
        };

        // Variables (watch expressions on top).
        let mut vars: Vec<AnyElement> = Vec::new();
        for (i, expr) in self.dbg.watch_exprs.iter().enumerate() {
            let value = d.watch_values.get(i).cloned().unwrap_or_else(|| if stopped { "…".into() } else { crate::i18n::t("not paused").into() });
            vars.push(
                div()
                    .id(("watch", i))
                    .h(px(layout::ROW_H))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .px(px(theme::SPACE[4]))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_size(px(text::XS.0))
                    .child(icon("eye", 12., p.accent))
                    .child(div().text_color(c(p.syntax_function)).child(expr.clone()))
                    .child(div().text_color(c(p.text_muted)).child("="))
                    .child(div().flex_1().min_w_0().overflow_hidden().font_family(ui::MONO_FONT).child(value))
                    .child(
                        div()
                            .id(("watch-x", i))
                            .flex_none()
                            .cursor_pointer()
                            .child(icon("x", 12., p.text_muted))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_watch(i, cx))),
                    )
                    .into_any_element(),
            );
        }
        for (i, (_, description)) in d.data_breakpoints.iter().enumerate() {
            vars.push(
                div()
                    .id(("data-bp", i))
                    .h(px(layout::ROW_H))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .px(px(theme::SPACE[4]))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_size(px(text::XS.0))
                    .child(div().size(px(8.)).flex_none().rounded_full().bg(c(p.danger)))
                    .child(div().text_color(c(p.text_muted)).child(crate::i18n::t("breaks when written:")))
                    .child(div().min_w_0().overflow_hidden().child(description.clone()))
                    .into_any_element(),
            );
        }
        if stopped {
            for s in &d.scopes {
                self.variable_row(d, p, cx, &mut vars, s.reference, &s.name, "", None, 0);
            }
        }

        // Console: last lines; expression input at the bottom.
        let skip = d.console.len().saturating_sub(CONSOLE_SHOWN);
        let lines: Vec<AnyElement> = d.console[skip..]
            .iter()
            .chain(d.partial.iter())
            .map(|(cat, line)| {
                let color = match cat.as_str() {
                    "stderr" => p.danger,
                    "console" => p.text_muted,
                    "input" => p.accent,
                    _ => p.text,
                };
                div().px(px(theme::SPACE[4])).font_family(ui::MONO_FONT).text_size(px(text::XS.0)).text_color(c(color)).child(line.clone()).into_any_element()
            })
            .collect();

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        column("dbg-stack")
                            .max_w(px(260.))
                            .child(header(crate::i18n::tr("Call Stack")))
                            .child(div().id("dbg-stack-rows").flex_1().min_h_0().overflow_y_scroll().children(thread_rows).children(stack)),
                    )
                    .child(
                        column("dbg-vars")
                            .child(header(crate::i18n::tr("Variables")))
                            .child(div().id("dbg-var-rows").flex_1().min_h_0().overflow_y_scroll().children(vars)),
                    )
                    .child(
                        column("dbg-console")
                            .border_r_0()
                            .child(header(crate::i18n::tr("Debug Console")))
                            .child(div().id("dbg-console-rows").flex_1().min_h_0().overflow_y_scroll().children(lines))
                            .child(
                                div()
                                    .h(px(26.))
                                    .flex_none()
                                    .px(px(theme::SPACE[4]))
                                    .border_t_1()
                                    .border_color(c(p.border))
                                    .flex()
                                    .items_center()
                                    .child(d.input.clone()),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// A scope or variable row and (if expanded) its children; depth is limited.
    fn variable_row(
        &self,
        d: &DebugState,
        p: &Palette,
        cx: &mut Context<Self>,
        out: &mut Vec<AnyElement>,
        reference: i64,
        name: &str,
        value: &str,
        ty: Option<&str>,
        depth: usize,
    ) {
        let open = d.expanded.contains(&reference);
        let expandable = reference > 0;
        let name = name.to_string();
        out.push(
            div()
                .id(("var", out.len()))
                .h(px(layout::ROW_H))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .pl(px(theme::SPACE[4] + 12. * depth as f32))
                .pr(px(theme::SPACE[4]))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_size(px(text::XS.0))
                .when(expandable, |d| d.cursor_pointer().hover(|s| s.bg(c(p.surface_hover))))
                .child(div().w(px(12.)).flex_none().when(expandable, |d| d.child(icon(if open { "chevron-down" } else { "chevron-right" }, 12., p.text_muted))))
                .child(div().text_color(c(p.syntax_function)).child(name))
                .when(!value.is_empty(), |d| d.child(div().text_color(c(p.text_muted)).child("=")))
                .child(div().min_w_0().overflow_hidden().font_family(ui::MONO_FONT).child(value.to_string()))
                .when_some(ty.map(str::to_string), |d, t| d.child(div().flex_none().text_color(c(p.text_muted)).child(t)))
                .when(expandable, |d| d.on_click(cx.listener(move |this, _, _, cx| this.toggle_variable(reference, cx))))
                .into_any_element(),
        );
        if open && depth < 8 {
            for v in d.children.get(&reference).into_iter().flatten() {
                self.variable_row(d, p, cx, out, v.reference, &v.name, &v.value, v.ty.as_deref(), depth + 1);
            }
        }
    }

    /// Summary of state for tests: (state, frame names, stopped line, console).
    #[cfg(test)]
    pub fn debug_snapshot(&self) -> Option<(String, Vec<String>, Option<(String, usize)>, Vec<String>)> {
        self.dbg.cur().map(|d| {
            let status = match &d.status {
                DebugStatus::Starting => "starting".to_string(),
                DebugStatus::Running => "running".to_string(),
                DebugStatus::Stopped(r) => format!("stopped:{r}"),
                DebugStatus::Ended(_) => "ended".to_string(),
            };
            (status, d.frames.iter().map(|f| f.name.clone()).collect(), d.location.clone(), d.console.iter().map(|(_, l)| l.clone()).collect())
        })
    }

    /// Breakpoints of a file as the store has them: (0-based line, enabled, verified as "yes" / "no" / "?").
    #[cfg(test)]
    pub fn breakpoints_for_test(&self, path: &str) -> Vec<(usize, bool, &'static str)> {
        use crate::debug_model::Verified;
        self.dbg
            .bps
            .file(path)
            .iter()
            .map(|(l, m)| {
                (
                    *l,
                    !m.disabled,
                    match m.verified {
                        Verified::Yes => "yes",
                        Verified::No(_) => "no",
                        Verified::Unknown => "?",
                    },
                )
            })
            .collect()
    }

    #[cfg(test)]
    pub fn set_breakpoint_enabled_for_test(&mut self, path: &str, line: usize, on: bool, cx: &mut Context<Self>) {
        self.set_breakpoint_enabled(path.to_string(), line, on, cx);
    }

    #[cfg(test)]
    pub fn add_function_breakpoint_for_test(&mut self, name: &str, cx: &mut Context<Self>) {
        self.finish_function_breakpoint(name.to_string(), cx);
    }

    #[cfg(test)]
    pub fn toggle_exception_filter_for_test(&mut self, id: &str, cx: &mut Context<Self>) {
        self.toggle_exception_filter(id.to_string(), cx);
    }

    #[cfg(test)]
    pub fn set_exception_filters_for_test(&mut self, filters: Option<Vec<String>>) {
        self.dbg.exception_filters = filters;
    }

    #[cfg(test)]
    pub fn add_data_breakpoint_for_test(&mut self, name: &str, cx: &mut Context<Self>) {
        self.finish_data_breakpoint(name.to_string(), cx);
    }

    #[cfg(test)]
    pub fn data_breakpoints_for_test(&self) -> Vec<String> {
        self.dbg.cur().map(|d| d.data_breakpoints.iter().map(|(_, desc)| desc.clone()).collect()).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn add_watch_for_test(&mut self, expr: &str, cx: &mut Context<Self>) {
        self.finish_add_watch(expr.to_string(), cx);
    }

    #[cfg(test)]
    pub fn debug_threads_for_test(&self) -> Vec<(i64, String)> {
        self.dbg.cur().map(|d| d.threads.clone()).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn select_thread_for_test(&mut self, id: i64, cx: &mut Context<Self>) {
        self.select_thread(id, cx);
    }

    #[cfg(test)]
    pub fn debug_watch_values(&self) -> Vec<(String, String)> {
        let values = self.dbg.cur().map(|d| d.watch_values.clone()).unwrap_or_default();
        self.dbg.watch_exprs.iter().cloned().zip(values).collect()
    }

    /// For tests: visible variables (name, value), from the expanded scopes.
    #[cfg(test)]
    pub fn debug_variables(&self) -> Vec<(String, String)> {
        let Some(d) = self.dbg.cur() else { return Vec::new() };
        d.scopes
            .iter()
            .filter(|s| d.expanded.contains(&s.reference))
            .flat_map(|s| d.children.get(&s.reference).into_iter().flatten().map(|v| (v.name.clone(), v.value.clone())))
            .collect()
    }
}

/// Show the stopped frame's locals at the end of source lines (VS Code "inline values"):
/// locals mentioned on each line from the function start to the stopped line (max 4), values cut to 30
/// chars. Function start: first unindented line above the stopped line.
pub(super) fn inline_values(lines: &[String], stop: usize, locals: &[(String, String)]) -> HashMap<usize, String> {
    let mut out = HashMap::new();
    if locals.is_empty() || stop >= lines.len() {
        return out;
    }
    let start =
        (0..=stop).rev().find(|&i| lines[i].chars().next().is_some_and(|c| !c.is_whitespace() && c != '}' && c != '#')).unwrap_or(stop.saturating_sub(40));
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    for (i, line) in lines.iter().enumerate().take(stop + 1).skip(start) {
        let code = line.split("//").next().unwrap_or(line);
        let words: Vec<&str> = code.split(|c: char| !is_word(c)).filter(|w| !w.is_empty()).collect();
        let mut shown: Vec<String> = Vec::new();
        for (name, value) in locals {
            if shown.len() < 4 && words.contains(&name.as_str()) {
                let v: String = value.chars().take(30).collect();
                shown.push(format!("{name} = {v}{}", if value.chars().count() > 30 { "…" } else { "" }));
            }
        }
        if !shown.is_empty() {
            out.insert(i, shown.join("  "));
        }
    }
    out
}

/// Scopes of a frame and the first scope's variables (in the background).
fn load_scopes(session: &dap::Session, frame_id: i64) -> (Vec<Scope>, HashMap<i64, Vec<Variable>>) {
    let scopes = session.scopes(frame_id).unwrap_or_default();
    let mut children = HashMap::new();
    if let Some(first) = scopes.first() {
        children.insert(first.reference, session.variables(first.reference).unwrap_or_default());
    }
    (scopes, children)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_values_cover_the_function_up_to_the_stop_line() {
        let src: Vec<String> =
            ["int helper(void);", "", "int add(int a, int b) {", "    int sum = a + b;", "    // sum is unused here", "    return sum;", "}"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let locals = vec![("a".to_string(), "3".to_string()), ("b".to_string(), "4".to_string()), ("sum".to_string(), "7".to_string())];
        let v = inline_values(&src, 5, &locals);
        assert_eq!(v.get(&2).map(String::as_str), Some("a = 3  b = 4"), "signature line");
        assert_eq!(v.get(&3).map(String::as_str), Some("a = 3  b = 4  sum = 7"));
        assert_eq!(v.get(&4), None, "a name in a comment does not count");
        assert_eq!(v.get(&5).map(String::as_str), Some("sum = 7"));
        assert_eq!(v.get(&0), None, "other function");
        assert!(inline_values(&src, 5, &[]).is_empty() && inline_values(&src, 99, &locals).is_empty());
        let long = inline_values(&src, 3, &[("sum".into(), "x".repeat(50))]);
        assert!(long[&3].ends_with('…') && long[&3].chars().count() < 45);
    }

    #[test]
    fn console_lines_are_cut_and_capped_whatever_the_adapter_sends() {
        let (mut console, mut partial) = (ConsoleLines::new(), None);
        // 300 chunks of 64 KB without a newline: the unfinished line never grows past PARTIAL_LIMIT.
        let chunk = "x".repeat(64 * 1024);
        for _ in 0..300 {
            console_append(&mut console, &mut partial, "stdout", &chunk);
            assert!(partial.as_ref().is_none_or(|(_, p): &(String, String)| p.len() <= PARTIAL_LIMIT));
        }
        assert!(!console.is_empty() && console.iter().all(|(_, l)| l.chars().count() <= LINE_LIMIT + 1), "every line is cut");
        // Many short lines: the console keeps CONSOLE_LIMIT.
        console_append(&mut console, &mut partial, "stdout", &"a\n".repeat(CONSOLE_LIMIT * 2));
        assert_eq!(console.len(), CONSOLE_LIMIT);
        console_push(&mut console, "stderr", &"y".repeat(LINE_LIMIT * 3));
        assert_eq!(console.len(), CONSOLE_LIMIT);
        assert!(console.last().unwrap().1.chars().count() <= LINE_LIMIT + 1);
    }
}
