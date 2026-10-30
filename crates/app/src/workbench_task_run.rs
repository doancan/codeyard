//! Workbench: task runs. A task is planned from its file by the task engine (`task_run`), its inputs are asked one after
//! the other, and each command of the graph runs on the folder's host as a typed process with its own output terminal.
//! Stop stops every process the run started; problem matchers fill the Problems panel; a debug session can wait for
//! its `preLaunchTask` to succeed (or become ready) before the adapter starts.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui::{AppContext as _, Context, Entity, Window};
use task_engine::model::Input;
use task_engine::schedule::State;

use super::tasks::Task;
use super::{Notice, OverlayKind, Workbench};
use crate::picker::{PickItem, Picker, Source as PickSource};
use crate::task_run::{self, Effects, Prepared, Run};
use crate::terminal::{TerminalEvent, TerminalView};
use crate::ui;

/// What happens when a run ends.
pub(super) enum After {
    Nothing,
    /// Start this debug configuration if the run succeeded (or its background root became ready).
    Debug(Box<crate::debug_config::LaunchConfig>),
}

/// A run waiting for the answers to its inputs.
pub(super) struct PendingRun {
    task: Task,
    prepared: Prepared,
    root: String,
    host: Arc<dyn host::Host>,
    environment: BTreeMap<String, String>,
    left: Vec<(String, Input)>,
    answers: BTreeMap<String, String>,
    after: After,
}

/// A run in progress (or finished, until the next one replaces it).
pub(super) struct LiveRun {
    run: Run,
    task: Task,
    host: Arc<dyn host::Host>,
    root: String,
    /// What variables expand to; input answers live only here and are never written anywhere.
    vars: Vars,
    terms: BTreeMap<usize, Entity<TerminalView>>,
    after: Option<After>,
    reported: bool,
}

/// The values a task's variables are expanded from, captured when the run starts.
struct Vars {
    platform: task_engine::model::Platform,
    folders: BTreeMap<String, String>,
    file: Option<String>,
    cursor: Option<(u32, u32)>,
    selected: Option<String>,
    home: String,
    environment: BTreeMap<String, String>,
    config: BTreeMap<String, serde_json::Value>,
    results: BTreeMap<String, String>,
}

impl Vars {
    fn context<'a>(&'a self, root: &'a str) -> task_engine::variables::Context<'a> {
        task_engine::variables::Context {
            platform: self.platform,
            root,
            folders: &self.folders,
            file: self.file.as_deref(),
            cursor: self.cursor,
            selected: self.selected.as_deref(),
            cwd: root,
            home: &self.home,
            executable: "codeyard",
            default_build: None,
            environment: &self.environment,
            config: &self.config,
            results: &self.results,
        }
    }
}

/// The task's file, or a one-task file for a detected command (it runs in the shell as typed).
fn task_file(task: &Task) -> Arc<str> {
    match &task.file {
        Some(file) => file.clone(),
        None => serde_json::json!({ "version": "2.0.0", "tasks": [ { "label": task.label, "type": "shell", "command": task.command } ] }).to_string().into(),
    }
}

/// Whether any planned task reads `${env:…}`: only then is the host's environment fetched.
fn reads_environment(prepared: &Prepared) -> bool {
    prepared.plan.order.iter().any(|&n| {
        let t = &prepared.registry.tasks[n];
        let texts = t.command.iter().chain(t.args.iter()).map(|a| a.value.as_str()).chain(t.options.cwd.as_deref());
        texts.chain(t.options.env.values().filter_map(|v| v.as_deref())).any(|s| s.contains("${env:"))
    })
}

/// The host's environment: this process's for the local machine, the remote shell's over SSH.
fn host_environment(host: &Arc<dyn host::Host>) -> BTreeMap<String, String> {
    if !host.is_remote() {
        return std::env::vars().collect();
    }
    let program = if host.info().os == "windows" { "cmd" } else { "/usr/bin/env" };
    let args: &[&str] = if host.info().os == "windows" { &["/d", "/c", "set"] } else { &["-0"] };
    let Ok(out) = host.exec(program, args, None, None) else { return BTreeMap::new() };
    let sep = if host.info().os == "windows" { b'\n' } else { 0 };
    out.stdout
        .split(|b| *b == sep)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (k, v) = entry.trim_end_matches('\r').split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

impl Workbench {
    /// Plan `task` and run it after its inputs are answered. Trust has been checked by the caller.
    pub(super) fn start_task_run(&mut self, task: Task, after: After, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.active_root(cx).unwrap_or_default();
        let host = self.remote.host.clone();
        let (file, label) = (task_file(&task), task.label.clone());
        let platform = task_run::platform_of(&host.info().os);
        let h = host.clone();
        // Parsing and an environment fetch over SSH stay off the UI thread.
        let planned = cx.background_executor().spawn(async move {
            let prepared = task_run::prepare(&file, platform, &label)?;
            let environment = if reads_environment(&prepared) { host_environment(&h) } else { BTreeMap::new() };
            Ok::<_, String>((prepared, environment))
        });
        cx.spawn_in(window, async move |this, cx| {
            let planned = planned.await;
            this.update_in(cx, |this, window, cx| match planned {
                Err(message) => this.task_failed_to_start(&task.label, message, cx),
                Ok((prepared, environment)) => {
                    // The folder or connection changed while the file was read: the plan belongs to the old one.
                    if !Arc::ptr_eq(&host, &this.remote.host) || this.active_root(cx).as_deref() != Some(root.as_str()) {
                        return;
                    }
                    let ids = match task_run::inputs_needed(&prepared) {
                        Ok(ids) => ids,
                        Err(message) => return this.task_failed_to_start(&task.label, message, cx),
                    };
                    let left = ids.into_iter().filter_map(|id| prepared.registry.inputs.get(&id).cloned().map(|i| (id, i))).collect();
                    this.jobs.pending = Some(PendingRun { task, prepared, root, host, environment, left, answers: BTreeMap::new(), after });
                    this.ask_next_task_input(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Run the task called `label` (`preLaunchTask`, `postDebugTask`) from the folder's task files; `fallback` is a shell
    /// command to run under that name when no task file defines it.
    pub(super) fn run_task_named(&mut self, label: String, fallback: Option<String>, after: After, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        self.detect_tasks(cx, move |this, tasks, window, cx| {
            let found = tasks.into_iter().find(|t| t.label == label && t.file.is_some());
            let task = found.or_else(|| fallback.map(|command| Task::shell(label.clone(), command, "launch")));
            match task {
                Some(task) => this.require_trust(window, cx, move |this, window, cx| this.start_task_run(task, after, window, cx)),
                None => this.task_failed_to_start(&label, crate::i18n::tf("There is no task called “{}”.", &[&label]), cx),
            }
        });
    }

    fn task_failed_to_start(&mut self, label: &str, message: String, cx: &mut Context<Self>) {
        self.notice = Some(Notice::Error(crate::i18n::tf("Task could not start: {}", &[&format!("{label}: {message}")]).into()));
        cx.notify();
    }

    fn ask_next_task_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.jobs.pending.as_ref() else { return };
        let Some((_, input)) = pending.left.first().cloned() else {
            let Some(pending) = self.jobs.pending.take() else { return };
            return self.begin_run(pending, window, cx);
        };
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        match input {
            Input::Prompt { description, default, .. } => {
                let default = default.unwrap_or_default();
                self.show_input(super::InputPurpose::TaskInput, crate::i18n::t("OK"), &description, &default, window, cx);
            }
            Input::Pick { description, options, default } => {
                let mut options = options;
                if let Some(i) = default.and_then(|d| options.iter().position(|(_, v)| *v == d)) {
                    let d = options.remove(i);
                    options.insert(0, d);
                }
                let items = options.iter().map(|(label, _)| PickItem { label: label.clone().into(), detail: None, keys: None }).collect();
                let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], &description, "", None, cx));
                self.show_overlay(picker, OverlayKind::TaskInput(options.into_iter().map(|(_, v)| v).collect()), window, cx);
            }
            Input::Command { .. } => {}
        }
        self.jobs.asking = self.jobs.asking.wrapping_add(1);
    }

    /// The answer to the current question.
    pub(super) fn answer_task_input(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.jobs.pending.as_mut() else { return };
        if pending.left.is_empty() {
            return;
        }
        let (id, _) = pending.left.remove(0);
        pending.answers.insert(format!("input:{id}"), value);
        self.ask_next_task_input(window, cx);
    }

    /// The question was dismissed: nothing of the run starts.
    pub(super) fn task_input_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = self.jobs.asking;
        // A confirmed answer arrives right after the overlay closes; only a question still open after that was cancelled.
        cx.defer_in(window, move |this, _, cx| {
            if this.jobs.asking == asked && this.jobs.pending.take().is_some() {
                this.notice = Some(Notice::Info(crate::i18n::t("Task input cancelled").into()));
                cx.notify();
            }
        });
    }

    fn begin_run(&mut self, pending: PendingRun, window: &mut Window, cx: &mut Context<Self>) {
        let PendingRun { task, prepared, root, host, environment, answers, after, .. } = pending;
        // One instance of a task at a time: running it again restarts it.
        let same: Vec<u64> = self.jobs.runs.iter().filter(|r| r.task.label == task.label && r.root == root && !r.run.finished()).map(|r| r.run.token).collect();
        for token in same {
            self.stop_run(token, cx);
        }
        self.jobs.next_token += 1;
        let token = self.jobs.next_token;
        let run = match Run::new(token, prepared) {
            Ok(run) => run,
            Err(message) => return self.task_failed_to_start(&task.label, message, cx),
        };
        let (file, cursor, selected) = match self.active_editor() {
            Some(ed) => {
                let e = ed.read(cx);
                let (line, col) = e.cursor_position();
                (Some(e.path.clone()), Some((line as u32, col as u32)), e.selected_text())
            }
            None => (None, None, None),
        };
        let folders =
            self.all_roots(cx).into_iter().map(|r| (r.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(&r).to_string(), r)).collect();
        let vars = Vars {
            platform: task_run::platform_of(&host.info().os),
            folders,
            file,
            cursor,
            selected,
            home: host.info().home.clone(),
            environment,
            config: BTreeMap::new(),
            results: answers,
        };
        self.jobs.root = Some(root.clone());
        self.jobs.last = Some(task.clone());
        self.set_task_problems(Vec::new(), cx);
        // Finished runs are kept only until the next one, so their terminals can be read; they hold no process.
        self.jobs.runs.retain(|r| !r.run.finished());
        self.jobs.runs.push(LiveRun { run, task, host, root, vars, terms: BTreeMap::new(), after: Some(after), reported: false });
        let effects = self.run_mut(token).map(|r| r.run.advance()).unwrap_or_default();
        self.apply_run_effects(token, effects, window, cx);
    }

    fn run_mut(&mut self, token: u64) -> Option<&mut LiveRun> {
        self.jobs.runs.iter_mut().find(|r| r.run.token == token)
    }

    fn apply_run_effects(&mut self, token: u64, effects: Effects, window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = self.run_mut(token) else { return };
        for proc in &effects.kill {
            live.host.proc_kill(*proc);
        }
        if effects.problems {
            self.publish_run_problems(cx);
        }
        for node in effects.start {
            self.spawn_node(token, node, window, cx);
        }
        self.report_if_finished(token, window, cx);
        cx.notify();
    }

    /// The output terminal of a node, created the first time it runs and cleared when it runs again.
    fn node_terminal(&mut self, token: u64, node: usize, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let live = self.run_mut(token)?;
        if let Some(t) = live.terms.get(&node) {
            return Some(t.clone());
        }
        let (host, title) = (live.host.clone(), live.run.task(node).label.clone());
        let term = cx.new(|cx| TerminalView::output_only(host, title, cx));
        self._subs.push(cx.subscribe_in(&term, window, |this, _, event: &TerminalEvent, window, cx| match event {
            TerminalEvent::OpenLink(link) => this.open_link(link.clone(), window, cx),
            TerminalEvent::TitleChanged | TerminalEvent::Exited => cx.notify(),
            _ => {}
        }));
        // Without any problem matcher in the run, the output is scanned for common compiler messages (as for a terminal
        // task); with one, the matchers own the Problems panel and the scanner stays out of it.
        let scan = {
            let run = &self.run_mut(token)?.run;
            run.prepared.plan.order.iter().all(|n| run.task(*n).matchers.is_empty())
        };
        self.run_mut(token)?.terms.insert(node, term.clone());
        self.terminals.push(term.clone());
        self.active_terminal = self.terminals.len() - 1;
        self.chrome.bottom = super::Bottom::Terminal;
        self.chrome.panel_open = true;
        if scan {
            self.watch_task_output(cx);
        }
        Some(term)
    }

    fn spawn_node(&mut self, token: u64, node: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(term) = self.node_terminal(token, node, window, cx) else { return };
        let Some(live) = self.run_mut(token) else { return };
        let task = live.run.task(node);
        let label = task.label.clone();
        let spawn = task_run::spawn_for(task, &live.vars.context(&live.root));
        let host = live.host.clone();
        // The command line is not echoed: it may hold an answer typed into a password input.
        let header = format!("\x1b[1m{}\x1b[0m\n", crate::i18n::tf("Running task: {}", &[&label]));
        term.update(cx, |t, cx| t.append(header.as_bytes(), cx));
        let spawn = match spawn {
            Ok(spawn) => spawn,
            Err(message) => {
                term.update(cx, |t, cx| t.append(format!("{message}\n").as_bytes(), cx));
                let effects = self.run_mut(token).map(|r| r.run.spawn_failed(node)).unwrap_or_default();
                return self.apply_run_effects(token, effects, window, cx);
            }
        };
        let started = cx.background_executor().spawn({
            let host = host.clone();
            async move { host.task_spawn(spawn.command, spawn.cwd, spawn.env) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let proc = match started.await {
                Ok(proc) => proc,
                Err(e) => {
                    this.update_in(cx, |this, window, cx| {
                        let line = format!("{}\n", crate::i18n::tf("Task could not start: {}", &[&e]));
                        term.update(cx, |t, cx| t.append(line.as_bytes(), cx));
                        let effects = this.run_mut(token).map(|r| r.run.spawn_failed(node)).unwrap_or_default();
                        this.apply_run_effects(token, effects, window, cx);
                    })
                    .ok();
                    return;
                }
            };
            let id = proc.id;
            // The run was stopped or replaced while the process started: it must not outlive it.
            let kept = this.update(cx, |this, _| match this.run_mut(token) {
                Some(live) if live.run.state(node) == Some(&State::Running) => {
                    live.run.started(node, id);
                    true
                }
                _ => false,
            });
            if !matches!(kept, Ok(true)) {
                host.proc_kill(id);
                return;
            }
            let mut events = ui::bridge(proc.events);
            while let Some(event) = futures::StreamExt::next(&mut events).await {
                let done = matches!(event, host::ProcEvent::Exit(_));
                let alive = this.update_in(cx, |this, window, cx| {
                    let effects = match event {
                        host::ProcEvent::Output(data) => {
                            term.update(cx, |t, cx| t.append(&data, cx));
                            this.run_mut(token).map(|r| r.run.output(node, &data))
                        }
                        host::ProcEvent::Exit(code) => this.run_mut(token).map(|r| {
                            let effects = r.run.exited(node, code);
                            let status = r.run.state(node).map(task_run::state_label).unwrap_or_default();
                            let line = format!("\n\x1b[2m{}\x1b[0m\n", crate::i18n::tf("Task “{0}”: {1}", &[&label, &status]));
                            term.update(cx, |t, cx| t.append(line.as_bytes(), cx));
                            effects
                        }),
                    };
                    if let Some(effects) = effects {
                        this.apply_run_effects(token, effects, window, cx);
                    }
                });
                if alive.is_err() {
                    host.proc_kill(id);
                    return;
                }
                if done {
                    return;
                }
            }
        })
        .detach();
    }

    /// Stop one run: queued commands never start and every live process is asked to stop. The run reports when the
    /// host confirms the last exit.
    pub(super) fn stop_run(&mut self, token: u64, cx: &mut Context<Self>) {
        let Some(live) = self.run_mut(token) else { return };
        live.after = None;
        for proc in live.run.cancel() {
            live.host.proc_kill(proc);
        }
        cx.notify();
    }

    /// "Tasks: Terminate Task": stop every run in progress.
    pub(super) fn terminate_tasks(&mut self, cx: &mut Context<Self>) {
        let live: Vec<u64> = self.jobs.runs.iter().filter(|r| !r.run.finished()).map(|r| r.run.token).collect();
        if live.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("No task is running.").into()));
            return cx.notify();
        }
        for token in live {
            self.stop_run(token, cx);
        }
    }

    /// "Tasks: Restart Running Task": stop the latest run and start its task again (inputs are asked again).
    pub(super) fn restart_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.jobs.runs.iter().rev().find(|r| !r.run.finished()).map(|r| r.task.clone()) else {
            return self.rerun_last_task(window, cx);
        };
        self.run_task(task, window, cx);
    }

    /// A task terminal was closed: its run stops too, so no process keeps running out of sight.
    pub(super) fn task_terminal_closed(&mut self, term: &Entity<TerminalView>, cx: &mut Context<Self>) {
        let tokens: Vec<u64> = self.jobs.runs.iter().filter(|r| r.terms.values().any(|t| t == term)).map(|r| r.run.token).collect();
        for token in tokens {
            self.stop_run(token, cx);
            if let Some(live) = self.run_mut(token) {
                live.terms.retain(|_, t| t != term);
            }
        }
    }

    /// The app is quitting: every task process is stopped before it returns, since the process ends right after and
    /// nothing would be left to stop them.
    pub(super) fn stop_task_runs_for_quit(&mut self) {
        self.jobs.pending = None;
        for live in &mut self.jobs.runs {
            live.after = None;
            for proc in live.run.cancel() {
                let _ = live.host.call(proto::Request::ProcKill { proc });
            }
        }
    }

    /// The connection changed: the runs' processes belong to the old host.
    pub(super) fn stop_all_task_runs(&mut self, cx: &mut Context<Self>) {
        self.jobs.pending = None;
        let tokens: Vec<u64> = self.jobs.runs.iter().map(|r| r.run.token).collect();
        for token in tokens {
            self.stop_run(token, cx);
        }
        // The old host cannot confirm the stops any more: say so instead of reporting the runs as stopped.
        if self.jobs.runs.iter().any(|r| r.run.unconfirmed() > 0) {
            self.notice = Some(Notice::Error(crate::i18n::t("Task cancellation could not be confirmed. Reconnect and inspect the process.").into()));
        }
        self.jobs.runs.clear();
    }

    fn report_if_finished(&mut self, token: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = self.run_mut(token) else { return };
        let outcome = live.run.outcome().clone();
        // A background root that became ready lets a waiting debug session start; the run itself goes on.
        let ready = outcome == State::Ready;
        if live.reported || !(live.run.finished() || ready) {
            return;
        }
        if live.run.finished() {
            live.reported = true;
        }
        let label = live.task.label.clone();
        let after = live.after.take();
        let status = task_run::state_label(&outcome);
        if live.run.finished() {
            let text = crate::i18n::tf("Task “{0}”: {1}", &[&label, &status]);
            // A run the user stopped is no failure: it is reported as information, like a success.
            let fine = outcome.satisfied() || outcome == State::Cancelled;
            self.notice = Some(if fine { Notice::Info(text.into()) } else { Notice::Error(text.into()) });
        }
        match after {
            Some(After::Debug(config)) if outcome.satisfied() => self.start_debug(*config, cx),
            // Debugging never starts after a failed preLaunchTask: the user fixes it and starts again.
            Some(After::Debug(_)) => {
                let text = crate::i18n::tf(
                    "The preLaunchTask “{0}” did not succeed ({1}), so debugging was not started. Check its terminal, then start debugging again.",
                    &[&label, &status],
                );
                self.notice = Some(Notice::Error(text.into()));
            }
            Some(After::Nothing) | None => {}
        }
        let _ = window;
    }

    /// Matcher problems of every run, resolved to paths on the run's host.
    fn publish_run_problems(&mut self, cx: &mut Context<Self>) {
        let mut found = Vec::new();
        for live in &self.jobs.runs {
            for (node, p) in live.run.problems() {
                let path = resolve_problem_path(&p.file, live.run.location(node), &live.root);
                let line = p.line.saturating_sub(1);
                let col = p.column.unwrap_or(1).saturating_sub(1);
                let end = (p.end_line.map_or(line, |l| l.saturating_sub(1)), p.end_column.map_or(col + 1, |c| c.saturating_sub(1)));
                let severity = if p.severity == 0 { 1 } else { p.severity };
                let source = live.run.task(node).label.clone();
                found.push((
                    path,
                    crate::lsp::Diagnostic {
                        start: (line, col),
                        end,
                        severity,
                        message: p.message.clone(),
                        source: Some(source),
                        code: p.code.clone(),
                        ..Default::default()
                    },
                ));
            }
        }
        self.set_task_problems(found, cx);
    }
}

/// A problem's file on the run's host: absolute paths stay, others are joined to the matcher's root (the folder unless
/// the matcher names another).
fn resolve_problem_path(file: &str, location: Option<&task_engine::matchers::FileLocation>, root: &str) -> String {
    use task_engine::matchers::FileLocation;
    let absolute = file.starts_with('/') || file.as_bytes().get(1) == Some(&b':') || file.starts_with("\\\\");
    if absolute {
        return file.to_string();
    }
    let base = match location {
        Some(FileLocation::Relative(base) | FileLocation::Auto(base)) => base.replace("${workspaceFolder}", root),
        _ => root.to_string(),
    };
    ui::join(&base, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problem_paths_resolve_against_the_matcher_root() {
        use task_engine::matchers::FileLocation;
        assert_eq!(resolve_problem_path("/abs/a.rs", None, "/w"), "/abs/a.rs");
        assert_eq!(resolve_problem_path("src/a.rs", None, "/w"), "/w/src/a.rs");
        let sub = FileLocation::Relative("${workspaceFolder}/sub".into());
        assert_eq!(resolve_problem_path("a.c", Some(&sub), "/w"), "/w/sub/a.c");
        assert_eq!(resolve_problem_path("C:\\x\\a.cs", Some(&sub), "/w"), "C:\\x\\a.cs");
    }

    #[test]
    fn detected_tasks_become_one_shell_task() {
        let task = Task::shell("cargo: build".into(), "cargo build --release".into(), "cargo");
        let p = task_run::prepare(&task_file(&task), task_engine::model::Platform::Linux, "cargo: build").unwrap();
        let t = &p.registry.tasks[p.plan.root];
        assert_eq!(t.command.as_ref().unwrap().value, "cargo build --release");
        assert_eq!(t.kind, task_engine::model::Kind::Shell);
    }
}

#[cfg(test)]
#[path = "workbench_task_run_tests.rs"]
mod run_tests;
