//! Debugger UI (F5): sessions, stopped line, call stack, variables, watches and the debug console. Breakpoints are in
//! `workbench_debug_breakpoints.rs`, starting configurations in `workbench_debug_launch.rs`, the sidebar view in
//! `workbench_debug_view.rs`; the protocol lives in `dap.rs`, configuration in `debug_config.rs`.
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
    /// Depth of the selected thread's stack when the adapter says (more frames load on demand).
    pub total_frames: Option<usize>,
    /// Values whose elements load a page at a time: reference → (elements loaded, elements in all).
    pub pages: HashMap<i64, (usize, usize)>,
    /// Sessions started together by a compound share a group; with `stop_all`, stopping one stops them all.
    pub group: Option<(u64, bool)>,
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

gpui::actions!(
    workbench,
    [
        ShowRunAndDebug,
        ToggleBreakpointEnabled,
        EnableAllBreakpoints,
        DisableAllBreakpoints,
        ToggleBreakpointsActive,
        AddFunctionBreakpoint,
        ConsoleHistoryPrev,
        ConsoleHistoryNext,
        ConsoleComplete,
        RunWithoutDebugging,
        RunToCursor,
        AddDebugConfiguration,
        OpenLaunchConfiguration,
        StopAllDebugging
    ]
);

#[path = "debug_cargo.rs"]
mod cargo;

#[path = "workbench_debug_save.rs"]
mod save;

#[path = "workbench_debug_breakpoints.rs"]
mod breakpoints;
#[path = "workbench_debug_launch.rs"]
mod launch;
#[cfg(test)]
#[path = "workbench_debug_testing.rs"]
mod testing;
#[path = "workbench_debug_view.rs"]
mod view;
pub use launch::PendingLaunch;
pub use view::DebugMenuCmd;
pub(super) use view::{StackRow, VarRow, stack_rows, status_text, variable_rows};

/// Stack frames requested at a time.
const STACK_PAGE: usize = 20;
/// Array elements loaded at a time.
pub(super) const VAR_PAGE: usize = 100;
/// Console inputs remembered per project.
const HISTORY_LIMIT: usize = 200;

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

/// For a Rust configuration: the `initCommands` to launch with (formatters first), or why the formatters are not loaded.
type RustSetup = Option<Result<Vec<String>, String>>;

/// In the background: pre-build, resolve Cargo artifacts, load Rust formatters and start the adapter.
fn launch_session(
    host: Arc<dyn host::Host>,
    mut config: LaunchConfig,
    root: &str,
    tx: mpsc::Sender<DapEvent>,
) -> Result<(Arc<dap::Session>, LaunchConfig, RustSetup), StartError> {
    let adapter = config.adapter.clone().map_err(|e| (e, String::new()))?;
    if let Some(cmd) = &config.pre_launch {
        run_pre_launch(&host, cmd, root)?;
    }
    if let Some(bin) = &config.cargo_target {
        config.args["program"] = serde_json::json!(cargo::build(host.as_ref(), bin, root)?);
    }
    let rust = crate::debug_config::is_rust_config(&config, host.stat(&format!("{}/Cargo.toml", root.trim_end_matches('/'))).is_ok())
        .then(|| {
            let existing: Vec<String> =
                config.args["initCommands"].as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect()).unwrap_or_default();
            crate::debug_config::rust_formatters(host.as_ref(), root, &existing).transpose()
        })
        .flatten();
    if let Some((host_name, port)) = &adapter.connect {
        return dap::Session::attach_socket(host, host_name, *port, tx).map(|session| (session, config, rust)).map_err(|e| (e, String::new()));
    }
    // lldb resolves a relative source path to an absolute one if the file exists in the adapter's cwd, which
    // breaks matching against debug info built with the logical path. So the adapter starts outside the project
    // (the program's own `cwd` goes in the `launch` arguments).
    let neutral = if cfg!(windows) && !host.is_remote() { std::env::temp_dir().to_string_lossy().into_owned() } else { "/".to_string() };
    let adapter_cwd = if adapter.id == "lldb-dap" { neutral } else { root.to_string() };
    dap::Session::start(host, &adapter.command, adapter.args, Some(adapter_cwd), tx)
        .map(|session| (session, config, rust))
        .map_err(|e| (crate::i18n::tf("Could not start the debug adapter: {}", &[&e]), String::new()))
}

/// A pre-launch command (build) in the folder; an error carries its output.
fn run_pre_launch(host: &Arc<dyn host::Host>, cmd: &str, root: &str) -> Result<(), StartError> {
    // On local Windows the shell is `cmd`; everywhere else (including SSH servers) it is `sh`.
    let (shell, flag) = if cfg!(windows) && !host.is_remote() { ("cmd", "/C") } else { ("sh", "-c") };
    let out = host.exec(shell, &[flag, cmd], Some(root), None).map_err(|e| (crate::i18n::tf("Could not run “{}”: {}", &[&cmd, &e]), String::new()))?;
    if !out.ok() {
        return Err((crate::i18n::tf("Build failed: {}", &[&cmd]), format!("{}{}", out.stdout_str(), out.stderr_str())));
    }
    Ok(())
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
    pub(super) fn start_debugging(&mut self, _: &super::StartDebugging, window: &mut Window, cx: &mut Context<Self>) {
        match self.dbg.cur().map(|d| d.status.clone()) {
            Some(DebugStatus::Stopped(_)) => return self.debug_control("continue", cx),
            Some(DebugStatus::Starting | DebugStatus::Running) => {
                self.notice = Some(Notice::Info(crate::i18n::t("Already debugging: Shift+F5 stops the session.").into()));
                cx.notify();
                return;
            }
            _ => {}
        }
        if self.active_root(cx).is_none() {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a folder to debug it.").into()));
            cx.notify();
            return;
        }
        self.require_trust(window, cx, |this, window, cx| this.choose_detected_debug_configs(false, window, cx));
    }

    /// Cargo discovery can invoke the toolchain: callers pass the workspace trust gate first.
    pub(super) fn choose_detected_debug_configs(&mut self, no_debug: bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.dbg.save_task.is_some() {
            return;
        }
        if let Some(root) = self.active_root(cx)
            && !self.debug_unsaved_editors(&root, cx).is_empty()
        {
            self.save_before_debug(root, cx, Box::new(move |this, window, cx| this.choose_detected_debug_configs(no_debug, window, cx)));
            return;
        }
        let Some(read) = self.read_launch_configs(true, cx) else { return };
        self.dbg.prepare_epoch = self.dbg.prepare_epoch.wrapping_add(1);
        let prepare_epoch = self.dbg.prepare_epoch;
        let asked = self.active_root(cx);
        let generation = self.lsp.generation;
        cx.spawn(async move |this, cx| {
            let (configs, _) = read.await;
            with_window(&this, cx, move |this, window, cx| {
                // The configurations were read from (and trust is asked for) the active file's folder: drop them if it changed.
                if this.active_root(cx) == asked && this.lsp.generation == generation && this.dbg.prepare_epoch == prepare_epoch {
                    this.choose_debug_config(configs.into_iter().map(|c| LaunchConfig { no_debug, ..c }).collect(), window, cx);
                }
            });
        })
        .detach();
    }

    /// The configurations of the launch files, for the Run and Debug view's selector.
    pub(super) fn load_launch_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(read) = self.read_launch_configs(false, cx) else {
            self.dbg.catalog.clear();
            return;
        };
        cx.spawn(async move |this, cx| {
            let (configs, compounds) = read.await;
            let names: Vec<String> = configs.into_iter().map(|c| c.name).chain(compounds.into_iter().map(|c| c.name)).collect();
            this.update(cx, |this, cx| {
                if this.dbg.last.as_ref().is_none_or(|l| !names.contains(l)) {
                    this.dbg.last = names.first().cloned().or(this.dbg.last.take());
                }
                this.dbg.catalog = names;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The view's ▶: start the selected configuration, or (with none) look for one like F5 does.
    pub(super) fn start_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.dbg.last.clone().filter(|n| self.dbg.catalog.contains(n)) else {
            return self.start_debugging(&super::StartDebugging, window, cx);
        };
        self.start_named(name, false, window, cx);
    }

    pub(super) fn choose_debug_config(&mut self, configs: Vec<LaunchConfig>, window: &mut Window, cx: &mut Context<Self>) {
        self.require_trust(window, cx, move |this, window, cx| this.choose_debug_config_trusted(configs, window, cx));
    }

    pub(super) fn choose_debug_config_trusted(&mut self, mut configs: Vec<LaunchConfig>, window: &mut Window, cx: &mut Context<Self>) {
        match configs.len() {
            0 => {
                self.notice = Some(Notice::Info(
                    crate::i18n::t("No debug configuration: add .vscode/launch.json, or open a Cargo / Go project or a Python file.").into(),
                ));
                cx.notify();
            }
            1 => self.launch_with_inputs(configs.remove(0), window, cx),
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

    pub(super) fn start_debug(&mut self, mut config: LaunchConfig, cx: &mut Context<Self>) {
        let Some(root) = self.active_root(cx) else { return };
        // A preLaunchTask from a task file runs as a planned task; the adapter starts when it succeeds or becomes ready.
        if let Some(label) = config.pre_launch_task.take() {
            let after = super::task_run_glue::After::Debug(Box::new(config));
            cx.spawn(async move |this, cx| with_window(&this, cx, move |this, window, cx| this.run_task_named(label, None, after, window, cx))).detach();
            return;
        }
        if self.dbg.save_task.is_some() {
            return;
        }
        if !self.debug_unsaved_editors(&root, cx).is_empty() {
            self.save_before_debug(root, cx, Box::new(move |this, _, cx| this.start_debug(config, cx)));
            return;
        }
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
        // Run Without Debugging: the program runs as usual, nothing stops it.
        let (breakpoints, functions) = if config.no_debug { Default::default() } else { (self.dbg.bps.wire_all(), self.dbg.bps.wire_functions()) };
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
            total_frames: None,
            pages: HashMap::new(),
            group: self.dbg.starting_group,
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
        let (cfg, root2, session_root) = (config, root.clone(), root.clone());
        let start = cx.background_executor().spawn(async move { launch_session(host, cfg, &root2, tx) });
        cx.spawn(async move |this, cx| {
            let (session, config, rust) = match start.await {
                Ok(started) => started,
                Err((message, output)) => {
                    this.update(cx, |this, cx| this.on_debug_failed(sid, message, output, cx)).ok();
                    return;
                }
            };
            let attached = this
                .update(cx, |this, _| {
                    let current = this.dbg.by_id(sid).is_some_and(|d| d.status == DebugStatus::Starting);
                    if let Some(d) = this.dbg.by_id_mut(sid).filter(|d| d.status == DebugStatus::Starting) {
                        d.config = config.clone();
                        d.session = Some(session.clone());
                    }
                    if current {
                        session.set_root(&session_root);
                        session.set_exception_pref(this.dbg.exception_filters.clone());
                    }
                    if let (Some(Err(reason)), Some(d)) = (&rust, this.dbg.by_id_mut(sid).filter(|d| d.status == DebugStatus::Starting)) {
                        d.push_console("console", crate::i18n::tf("Rust formatters not loaded: {}", &[reason]));
                    }
                    current
                })
                .unwrap_or(false);
            if !attached {
                cx.background_executor().spawn(async move { session.stop() }).detach();
                return;
            }
            let adapter_id = config.adapter.as_ref().map(|a| a.id.clone()).unwrap_or_default();
            let (request, mut args) = (config.request.clone(), config.args.clone());
            if let (Some(Ok(commands)), Some(o)) = (rust, args.as_object_mut()) {
                o.insert("initCommands".into(), serde_json::json!(commands));
            }
            if config.no_debug
                && let Some(o) = args.as_object_mut()
            {
                o.insert("noDebug".into(), serde_json::json!(true));
            }
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
        if self.dbg.by_id(sid).is_none_or(|d| matches!(d.status, DebugStatus::Ended(_))) {
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
                if matches!(d.status, DebugStatus::Ended(_)) {
                    return;
                }
                d.stopped_once = true;
                d.status = DebugStatus::Stopped(reason.clone());
                d.thread = thread_id;
                if let Some(t) = text.filter(|_| reason == "exception") {
                    d.push_console("stderr", crate::i18n::tf("Paused on exception: {}", &[&t]));
                }
                // The session that stopped is the one the controls and the editors follow.
                self.dbg.focused = Some(sid);
                if let Some(path) = self.dbg.bps.set_temp(None) {
                    self.after_run_to_cursor(path);
                }
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
            // Answers that arrive after the session ended must not mark breakpoints again.
            DapEvent::BreakpointsSet { .. } | DapEvent::BreakpointChanged(_) | DapEvent::FunctionBreakpointsSet(_)
                if matches!(d.status, DebugStatus::Ended(_)) => {}
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
            DapEvent::RunInTerminal(request) => self.run_in_terminal(sid, request, cx),
        }
        cx.notify();
    }

    /// When stopped: call stack, scopes of the first frame with a source, and variables of the first scope.
    fn refresh_stack(&mut self, sid: u64, thread: i64, cx: &mut Context<Self>) {
        let Some(session) = self.dbg.by_id(sid).and_then(|d| d.session.clone()) else { return };
        let local = !self.remote.host.is_remote();
        let task = cx.background_executor().spawn(async move {
            let (mut frames, total) = session.stack_page(thread, 0, STACK_PAGE).unwrap_or_default();
            normalize_frames(&mut frames, local);
            let top = frames.iter().position(|f| f.path.is_some()).unwrap_or(0);
            let (scopes, children) = frames.get(top).map(|f| load_scopes(&session, f.id)).unwrap_or_default();
            (frames, total, top, scopes, children, session.threads())
        });
        cx.spawn(async move |this, cx| {
            let (frames, total, top, scopes, children, threads) = task.await;
            with_window(&this, cx, move |this, window, cx| {
                let Some(d) = this.dbg.by_id_mut(sid).filter(|d| matches!(d.status, DebugStatus::Stopped(_))) else { return };
                d.location = frames.get(top).and_then(|f| f.path.clone().map(|p| (p, f.line.saturating_sub(1) as usize)));
                d.total_frames = total;
                d.pages.clear();
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

    /// More frames of the selected thread can be loaded.
    pub(super) fn more_frames(d: &DebugState) -> bool {
        match d.total_frames {
            Some(total) => d.frames.len() < total,
            None => d.frames.len() >= STACK_PAGE && d.frames.len().is_multiple_of(STACK_PAGE),
        }
    }

    /// "Load More Stack Frames": the next page of the selected thread's stack.
    pub(crate) fn load_more_frames(&mut self, cx: &mut Context<Self>) {
        let local = !self.remote.host.is_remote();
        let Some(d) = self.dbg.cur() else { return };
        let (sid, thread, start) = (d.id, d.thread, d.frames.len());
        let Some(session) = d.session.clone() else { return };
        let task = cx.background_executor().spawn(async move {
            let (mut frames, total) = session.stack_page(thread, start, STACK_PAGE).unwrap_or_default();
            normalize_frames(&mut frames, local);
            (frames, total)
        });
        cx.spawn(async move |this, cx| {
            let (frames, total) = task.await;
            this.update(cx, |this, cx| {
                if let Some(d) = this.dbg.by_id_mut(sid).filter(|d| d.thread == thread && d.frames.len() == start) {
                    // An empty page means the end, whatever the adapter said about the depth.
                    d.total_frames = if frames.is_empty() { Some(start) } else { total.or(d.total_frames) };
                    d.frames.extend(frames);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Restart Frame": run the frame again from its start (adapters that support it).
    pub(crate) fn restart_frame(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(d) = self.dbg.cur_mut() else { return };
        let (Some(frame), Some(session)) = (d.frames.get(index).cloned(), d.session.clone()) else { return };
        if !(frame.can_restart && session.supports("supportsRestartFrame")) {
            self.notice = Some(Notice::Info(crate::i18n::t("This frame cannot be restarted.").into()));
            cx.notify();
            return;
        }
        d.status = DebugStatus::Running;
        d.location = None;
        session.restart_frame(frame.id);
        self.apply_debug_line_all(cx);
        cx.notify();
    }

    /// The selected thread's stack as text (`name (file:line)` per line).
    pub(super) fn copy_call_stack(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.dbg.cur() else { return };
        let text: Vec<String> = d
            .frames
            .iter()
            .map(|f| match &f.path {
                Some(p) => format!("{} ({}:{})", f.name, p, f.line),
                None => f.name.clone(),
            })
            .collect();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.join("\n")));
    }

    fn show_debug_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line)) = self.dbg.cur().and_then(|d| d.location.clone()) else { return };
        self.open_location(path, line as u32, 0, window, cx);
        self.apply_debug_line_all(cx);
    }

    /// The controls, the editors and the toolbar follow this session.
    pub(super) fn focus_debug_session(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.dbg.by_id(id).is_some() {
            self.dbg.focused = Some(id);
            self.apply_debug_line_all(cx);
            self.refresh_watches(cx);
            cx.notify();
        }
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

    /// Toggle a variable or scope row; children are requested on first expansion. A value with many elements gets its
    /// named children and the first page of elements.
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
        let indexed = d.children.values().flatten().find(|v| v.reference == reference).map_or(0, |v| v.indexed);
        let task = cx.background_executor().spawn(async move {
            if indexed > VAR_PAGE {
                let mut vars = session.named_variables(reference).unwrap_or_default();
                vars.extend(session.indexed_variables(reference, 0, VAR_PAGE).unwrap_or_default());
                vars
            } else {
                session.variables(reference).unwrap_or_default()
            }
        });
        cx.spawn(async move |this, cx| {
            let vars = task.await;
            this.update(cx, |this, cx| {
                if let Some(d) = this.dbg.by_id_mut(sid) {
                    if indexed > VAR_PAGE {
                        d.pages.insert(reference, (VAR_PAGE, indexed));
                    }
                    d.children.insert(reference, vars);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The next page of a paged value's elements.
    pub(crate) fn load_more_variables(&mut self, reference: i64, cx: &mut Context<Self>) {
        let sid = self.dbg.cur_id();
        let Some(d) = self.dbg.cur() else { return };
        let (Some(&(loaded, total)), Some(session)) = (d.pages.get(&reference), d.session.clone()) else { return };
        let count = VAR_PAGE.min(total.saturating_sub(loaded));
        let task = cx.background_executor().spawn(async move { session.indexed_variables(reference, loaded, count).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let vars = task.await;
            this.update(cx, |this, cx| {
                if let Some(d) = this.dbg.by_id_mut(sid)
                    && d.pages.get(&reference).is_some_and(|p| p.0 == loaded)
                {
                    // Nothing came back: stop offering more.
                    let got = if vars.is_empty() { total - loaded } else { vars.len() };
                    d.pages.insert(reference, (loaded + got, total));
                    d.children.entry(reference).or_default().extend(vars);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Set Value": change a variable (by `setVariable`, else by assigning to its expression).
    pub(super) fn start_set_variable(&mut self, parent: i64, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(v) = self.dbg.cur().and_then(|d| d.children.get(&parent)?.iter().find(|v| v.name == name).cloned()) else { return };
        self.show_input(super::InputPurpose::SetVariable { parent, name }, crate::i18n::t("Set"), crate::i18n::t("new value"), &v.value, window, cx);
    }

    pub(super) fn finish_set_variable(&mut self, parent: i64, name: String, value: String, cx: &mut Context<Self>) {
        let sid = self.dbg.cur_id();
        let Some(d) = self.dbg.cur().filter(|d| matches!(d.status, DebugStatus::Stopped(_))) else { return };
        let Some(session) = d.session.clone() else { return };
        let frame = d.frames.get(d.frame).map(|f| f.id);
        let expr = d.children.get(&parent).and_then(|c| c.iter().find(|v| v.name == name)).and_then(|v| v.evaluate_name.clone());
        let n = name.clone();
        let task = cx.background_executor().spawn(async move {
            if session.supports("supportsSetVariable") {
                session.set_variable(parent, &n, &value)
            } else if let (true, Some(e)) = (session.supports("supportsSetExpression"), expr) {
                session.set_expression(frame, &e, &value)
            } else {
                Err(crate::i18n::t("This debug adapter cannot change values.").to_string())
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(shown) => {
                        if let Some(v) = this.dbg.by_id_mut(sid).and_then(|d| d.children.get_mut(&parent)?.iter_mut().find(|v| v.name == name)) {
                            v.value = shown;
                        }
                        this.refresh_watches(cx);
                    }
                    Err(e) => this.notice = Some(Notice::Error(crate::i18n::tf("Could not set {}: {}", &[&name, &e]).into())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Expression of a variable (its evaluate name, else its name).
    pub(super) fn variable_expression(&self, parent: i64, name: &str) -> Option<(String, String)> {
        let v = self.dbg.cur()?.children.get(&parent)?.iter().find(|v| v.name == name)?;
        Some((v.evaluate_name.clone().unwrap_or_else(|| v.name.clone()), v.value.clone()))
    }

    // --- Console history and completion ---

    fn remember_input(&mut self, text: &str) {
        self.dbg.history.retain(|h| h != text);
        self.dbg.history.push(text.to_string());
        let extra = self.dbg.history.len().saturating_sub(HISTORY_LIMIT);
        self.dbg.history.drain(..extra);
        self.dbg.history_pos = None;
        if let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) {
            let _ = s.set(&format!("dbg.history:{ws}"), &self.dbg.history.join("\n"));
        }
    }

    pub(super) fn load_console_history(&mut self) {
        self.dbg.history = match (self.persist.workspace_id, &self.persist.state) {
            (Some(ws), Some(s)) => {
                s.get(&format!("dbg.history:{ws}")).map(|t| t.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        self.dbg.history_pos = None;
    }

    /// ↑ / ↓ in the console input: older / newer inputs.
    pub(crate) fn console_history(&mut self, older: bool, cx: &mut Context<Self>) {
        let n = self.dbg.history.len();
        if n == 0 {
            return;
        }
        let pos = match (self.dbg.history_pos, older) {
            (None, true) => Some(n - 1),
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) => (i + 1 < n).then_some(i + 1),
        };
        self.dbg.history_pos = pos;
        let text = pos.map(|i| self.dbg.history[i].clone()).unwrap_or_default();
        if let Some(input) = self.dbg.cur().map(|d| d.input.clone()) {
            input.update(cx, |f, cx| f.set_text(&text, cx));
        }
    }

    /// Tab in the console input: the adapter's completions; one is filled in, several are listed in the console.
    pub(crate) fn console_complete(&mut self, cx: &mut Context<Self>) {
        let sid = self.dbg.cur_id();
        let Some(d) = self.dbg.cur() else { return };
        let Some(session) = d.session.clone().filter(|s| s.supports("supportsCompletionsRequest")) else { return };
        let frame = matches!(d.status, DebugStatus::Stopped(_)).then(|| d.frames.get(d.frame).map(|f| f.id)).flatten();
        let input = d.input.clone();
        let text = input.read(cx).text().to_string();
        let column = text.chars().count() + 1;
        let t2 = text.clone();
        let task = cx.background_executor().spawn(async move { session.completions(frame, &t2, column).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            this.update(cx, |this, cx| {
                // The word being typed is replaced by the completion.
                let start = text.rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
                match found.as_slice() {
                    [] => {}
                    [one] => input.update(cx, |f, cx| f.set_text(&format!("{}{one}", &text[..start]), cx)),
                    many => {
                        if let Some(d) = this.dbg.by_id_mut(sid) {
                            d.push_console("console", many.iter().take(40).cloned().collect::<Vec<_>>().join("  "));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn debug_evaluate(&mut self, expression: String, cx: &mut Context<Self>) {
        let expression = expression.trim().to_string();
        let sid = self.dbg.cur_id();
        let Some(d) = self.dbg.cur_mut() else { return };
        if expression.is_empty() {
            return;
        }
        d.push_console("input", format!("> {expression}"));
        self.remember_input(&expression);
        let Some(d) = self.dbg.cur_mut() else { return };
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
        self.cancel_pending_debug_start(cx);
        let Some(d) = self.dbg.cur().filter(|d| !matches!(d.status, DebugStatus::Ended(_))) else { return };
        // A compound with `stopAll`: its other sessions stop too.
        let ids: Vec<u64> = match d.group {
            Some((g, true)) => self.dbg.sessions.iter().filter(|s| s.group.is_some_and(|(x, _)| x == g)).map(|s| s.id).collect(),
            _ => vec![d.id],
        };
        for sid in ids {
            if self.dbg.by_id(sid).is_some_and(|d| !matches!(d.status, DebugStatus::Ended(_))) {
                self.end_debug(sid, crate::i18n::t("Debug session stopped").into(), cx);
            }
        }
    }

    pub(super) fn restart_debugging(&mut self, _: &super::RestartDebugging, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(config) = self.dbg.cur().map(|d| d.config.clone()) {
            self.start_debug(config, cx);
        }
    }

    /// End a session (adapter and program exit); console and message stay.
    fn end_debug(&mut self, sid: u64, message: String, cx: &mut Context<Self>) {
        let post = self.dbg.by_id(sid).filter(|d| !matches!(d.status, DebugStatus::Ended(_))).and_then(|d| d.config.post_debug.clone());
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
        if let Some(path) = self.dbg.bps.set_temp(None) {
            self.after_run_to_cursor(path);
        }
        if let Some((label, command)) = post {
            self.run_post_debug_task(label, command, cx);
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
        self.cancel_pending_debug_start(cx);
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

    /// The floating toolbar over the editors while a session is live: continue or pause, step over, into and out,
    /// restart and stop. It follows the focused session.
    pub(super) fn debug_toolbar(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.dbg.cur().filter(|d| !matches!(d.status, DebugStatus::Ended(_)))?;
        let stopped = matches!(d.status, DebugStatus::Stopped(_));
        let running = d.status == DebugStatus::Running;
        let dim = |on: bool| if on { 1.0 } else { 0.35 };
        let bar = div()
            .id("debug-toolbar")
            .flex()
            .items_center()
            .gap(px(theme::SPACE[1]))
            .p(px(theme::SPACE[1]))
            .rounded(px(theme::radius::MD))
            .border_1()
            .border_color(c(p.border))
            .bg(c(p.surface_raised))
            .shadow_md()
            .child(icon("bug-play", layout::ICON - 2., p.text_muted))
            .child(div().opacity(dim(stopped || running)).child(if running {
                ui::icon_button("dbg-pause", "pause", crate::i18n::t("Pause (F6)"), p).on_click(cx.listener(|this, _, _, cx| this.debug_control("pause", cx)))
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
                ui::icon_button("dbg-stop", "square", crate::i18n::t("Stop (⇧F5)"), p)
                    .on_click(cx.listener(|this, _, w, cx| this.stop_debugging(&super::StopDebugging, w, cx))),
            )
            // Which session the buttons act on, when there are several.
            .when(self.dbg.sessions.len() > 1, |b| {
                b.child(div().px(px(theme::SPACE[2])).text_size(px(text::XS.0)).text_color(c(p.text_muted)).whitespace_nowrap().child(d.config.name.clone()))
            });
        Some(
            div()
                .absolute()
                .top(px(layout::TABBAR_H + theme::SPACE[2]))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(bar.on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()))
                .into_any_element(),
        )
    }

    /// The bottom panel's Debug Console: the focused session's output and an expression field.
    pub(super) fn debug_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(d) = self.dbg.cur() else {
            return div()
                .px(px(theme::SPACE[6]))
                .py(px(theme::SPACE[3]))
                .text_color(c(p.text_muted))
                .child(crate::i18n::t("No debug session. F5 starts one."))
                .into_any_element();
        };
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
                    .gap(px(theme::SPACE[2]))
                    .key_context("DebugRepl")
                    .on_action(cx.listener(|this, _: &ConsoleHistoryPrev, _, cx| this.console_history(true, cx)))
                    .on_action(cx.listener(|this, _: &ConsoleHistoryNext, _, cx| this.console_history(false, cx)))
                    .on_action(cx.listener(|this, _: &ConsoleComplete, _, cx| this.console_complete(cx)))
                    .child(icon("chevron-right", 12., p.accent))
                    .child(d.input.clone()),
            )
            .into_any_element()
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
/// The compiler may report the logical path (`/tmp/x`) while the editor holds the canonical one (`/private/tmp/x`).
fn normalize_frames(frames: &mut [Frame], local: bool) {
    if local {
        for f in frames {
            if let Some(c) = f.path.as_deref().and_then(|p| std::fs::canonicalize(p).ok()) {
                f.path = Some(c.to_string_lossy().into_owned());
            }
        }
    }
}

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
