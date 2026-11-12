//! Workbench: run and debug what rust-analyzer calls runnables: the gutter marks, *Rust: Run Runnable at Cursor* and the
//! list that offers *Run* and *Debug* for each. Running types the command into the task terminal (the same path as
//! tasks and tests); debugging builds the target with Cargo and starts the executable it reports under the debugger.

use gpui::{Context, Entity, Window};
use serde_json::json;

#[cfg(not(test))]
use super::super::tasks::Task;
use super::super::*;
use crate::i18n::{t, tf};
use crate::picker::{PickItem, Picker, Source as PickSource};
use crate::rust_toolchain::runnables as run;

gpui::actions!(workbench, [RustRunRunnable]);

/// How long the server has to list the runnables of a file.
const LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Where in the file the user asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// The cursor (the palette command).
    Cursor,
    /// A gutter mark.
    Line(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum Action {
    Run,
    Debug,
}

/// One row of the list: a runnable and what to do with it.
#[derive(Clone, Debug)]
pub(in super::super) struct Entry {
    item: run::Item,
    action: Action,
    root: String,
}

impl Entry {
    /// The line under the row: the command Run types, or the build and program Debug starts.
    fn detail(&self, windows: bool) -> String {
        match (self.action, &self.item.debug) {
            (Action::Debug, Some(plan)) => plan.describe(windows),
            _ => self.item.command.clone(),
        }
    }

    fn label(&self) -> String {
        match self.action {
            Action::Run => tf("Run {}", &[&self.item.label]),
            Action::Debug => tf("Debug {}", &[&self.item.label]),
        }
    }
}

/// The rows for these items: *Run* for each, then *Debug* where the runnable can be debugged.
fn entries(items: Vec<run::Item>, root: &str) -> Vec<Entry> {
    items
        .into_iter()
        .flat_map(|item| {
            let debug = item.debug.is_some().then(|| Entry { item: item.clone(), action: Action::Debug, root: root.to_string() });
            [Some(Entry { item, action: Action::Run, root: root.to_string() }), debug]
        })
        .flatten()
        .collect()
}

/// What the server and the text said about the file.
enum Offer {
    /// rust-analyzer's list (the whole file).
    Server(Vec<crate::lsp::Runnable>),
    /// The project's test finder and `fn main`.
    Text(Vec<run::Item>),
}

impl Workbench {
    /// The settings that change what runs.
    pub(in super::super) fn rust_run_options(&self, root: Option<String>, cx: &gpui::App) -> run::Options {
        run::Options { target: cx.global::<Settings>().rust_build.target.clone(), root, nextest: false, windows: self.remote.host.info().os == "windows" }
    }

    /// *Rust: Run Runnable at Cursor*.
    pub(in super::super) fn rust_run_runnable(&mut self, _: &RustRunRunnable, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor().cloned().filter(|ed| ed.read(cx).language() == "Rust") else {
            self.notify(NoticeSource::Language, Notice::Info(t("Open a Rust file first").into()));
            cx.notify();
            return;
        };
        self.offer_runnables(editor, Place::Cursor, window, cx);
    }

    /// The gutter mark of `line` in `editor` was pressed.
    pub(in super::super) fn run_mark_pressed(&mut self, editor: &Entity<EditorView>, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.offer_runnables(editor.clone(), Place::Line(line), window, cx);
    }

    fn offer_runnables(&mut self, editor: Entity<EditorView>, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        let path = editor.read(cx).path.clone();
        let Some(root) = self.root_of(&path, cx) else {
            self.notify(NoticeSource::Language, Notice::Info(t("Open a file inside a folder to run it.").into()));
            cx.notify();
            return;
        };
        let options = self.rust_run_options(Some(root.clone()), cx);
        let runner = cx.global::<Settings>().rust_build.test_runner;
        // Cargo reads the folder's configuration, so nextest is looked for only where the user has trusted the folder.
        let trusted = self.is_trusted_root(&root, cx);
        let nextest_known = self.lsp.rust.nextest.known(&root, std::time::Instant::now());
        let asks = runner != crate::rust_toolchain::nextest::TestRunner::Cargo && trusted && nextest_known.is_none();
        if asks {
            self.notify(NoticeSource::Language, Notice::Info(t("Looking for cargo-nextest…").into()));
            cx.notify();
        }
        let host = editor.read(cx).host_arc();
        let (text, cursor, cached, client) = editor.update(cx, |e, _| {
            let client = e.lsp_context().map(|(client, ..)| client);
            (e.buffer_text(), e.cursor_lsp_position(), e.runnables(), client)
        });
        let server = client.filter(|c| c.config.language == "rust" && c.capabilities["experimental"]["runnables"].is_object());
        let rel = path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).unwrap_or(&path).to_string();
        let (task_root, task_rel, task_text) = (root.clone(), rel, text.clone());
        let mut task_options = options.clone();
        let uri = crate::lsp::path_to_uri(&path);
        let task = cx.background_executor().spawn(async move {
            // Tests run through nextest when the setting and the machine allow it.
            let found = if !asks { nextest_known.unwrap_or(false) && trusted } else { super::nextest::probe(host.clone(), &task_root) };
            task_options.nextest = runner.uses_nextest(found);
            let list = match (cached, server) {
                (Some(list), _) => Some(list.to_vec()),
                (None, Some(client)) => client
                    .request(crate::lsp::RUNNABLES, json!({ "textDocument": { "uri": uri }, "position": serde_json::Value::Null }), LIST_TIMEOUT)
                    .ok()
                    .map(|v| crate::lsp::parse_runnables(&v)),
                (None, None) => None,
            };
            let items = match list {
                Some(list) => Offer::Server(list),
                None => {
                    let context = crate::cargo_project::context_on_host(host.as_ref(), &task_root, &task_rel);
                    Offer::Text(run::fallback_items(&task_rel, &task_text, context.as_ref(), &task_options))
                }
            };
            (items, task_options.nextest, found, asks)
        });
        let generation = self.lsp.generation;
        cx.spawn_in(window, async move |this, cx| {
            let (offer, nextest, found, asked) = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.lsp.generation != generation {
                    return;
                }
                let root_key = root.clone();
                let options = run::Options { nextest, ..options };
                if asked {
                    this.lsp.rust.nextest.remember(&root_key, found, std::time::Instant::now());
                    this.dismiss_notice();
                }
                let items = match (offer, place) {
                    (Offer::Server(list), Place::Cursor) => run::items_at(&list, cursor, &options),
                    (Offer::Server(list), Place::Line(line)) => run::items(&list, &options).into_iter().filter(|i| i.line == line).collect(),
                    (Offer::Text(items), Place::Cursor) => run::fallback_at(items, &text, cursor.0),
                    (Offer::Text(items), Place::Line(line)) => items.into_iter().filter(|i| i.line == line).collect(),
                };
                this.present_runnables(items, root, place, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn present_runnables(&mut self, items: Vec<run::Item>, root: String, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        if items.is_empty() {
            let message = match place {
                Place::Cursor => t("Nothing to run at the cursor: put it inside a test or main."),
                Place::Line(_) => t("Nothing to run on this line."),
            };
            self.notify(NoticeSource::Language, Notice::Info(message.into()));
            cx.notify();
            return;
        }
        let rows = entries(items, &root);
        // The palette command runs the nearest runnable; the list is for choosing between several, or for debugging.
        if place == Place::Cursor && rows.iter().filter(|e| e.action == Action::Run).count() == 1 {
            let first = rows[0].clone();
            return self.start_runnable(first, window, cx);
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let windows = self.remote.host.info().os == "windows";
        let list = rows.iter().map(|e| PickItem { label: e.label().into(), detail: Some(e.detail(windows).into()), keys: None }).collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items: list }], t("Run or debug"), "", None, cx));
        self.show_overlay(picker, OverlayKind::RustChoice(super::Choice::Runnables(rows)), window, cx);
    }

    /// The list's answer: run or debug the row.
    pub(super) fn chose_runnable(&mut self, rows: Vec<Entry>, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = rows.get(index).cloned() {
            self.start_runnable(entry, window, cx);
        }
    }

    pub(in super::super) fn start_runnable(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        match entry.action {
            Action::Run => {
                // Tests record the command instead of starting Cargo.
                if entry.item.command.starts_with("cargo nextest ") {
                    self.notify(NoticeSource::Language, Notice::Info(crate::rust_toolchain::nextest::notice().into()));
                    cx.notify();
                }
                #[cfg(test)]
                self.lsp.rust.ran.push((entry.item.label.clone(), entry.item.command.clone()));
                #[cfg(not(test))]
                {
                    let source = if entry.item.kind == crate::lsp::RunnableKind::Run { "run" } else { "test" };
                    self.run_task(Task::shell(entry.item.label.clone(), entry.item.command.clone(), source), window, cx);
                }
            }
            Action::Debug => {
                let root = entry.root.clone();
                self.require_trust_for(Some(root), window, cx, move |this, window, cx| this.debug_runnable_trusted(entry, window, cx));
            }
        }
    }

    /// Saves, builds with Cargo and starts the executable the build reported under the debugger.
    fn debug_runnable_trusted(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(plan) = entry.item.debug.clone() else { return };
        if self.dbg.save_task.is_some() {
            return;
        }
        let root = entry.root.clone();
        if !self.debug_unsaved_editors(&root, cx).is_empty() {
            self.save_before_debug(root.clone(), cx, Box::new(move |this, window, cx| this.debug_runnable_trusted(entry, window, cx)));
            return;
        }
        self.dbg.prepare_epoch = self.dbg.prepare_epoch.wrapping_add(1);
        let prepare_epoch = self.dbg.prepare_epoch;
        self.notify(NoticeSource::Language, Notice::Info(if plan.test { t("Building the tests…") } else { t("Building…") }.into()));
        cx.notify();
        let host = self.remote.host.clone();
        let (build_root, args) = (root.clone(), plan.build_args.clone());
        let build = cx.background_executor().spawn(async move {
            let mut args = args;
            args.push("--message-format=json".into());
            let output = host
                .exec("cargo", &args.iter().map(String::as_str).collect::<Vec<_>>(), Some(&build_root), None)
                .map_err(|e| crate::cargo_project::explain_failure(&e.to_string()))?;
            let text = format!("{}\n{}", output.stdout_str(), output.stderr_str());
            if output.code != Some(0) || !crate::cargo_project::build_finished(&text) {
                return Err(crate::cargo_project::diagnostics(&text));
            }
            Ok(run::built_executables(&text, plan.test))
        });
        let generation = self.lsp.generation;
        cx.spawn_in(window, async move |this, cx| {
            let built = build.await;
            this.update_in(cx, |this, window, cx| {
                if this.root(cx).as_deref() != Some(&root) || this.lsp.generation != generation || this.dbg.prepare_epoch != prepare_epoch {
                    return;
                }
                let executables = match built {
                    Ok(list) if !list.is_empty() => list,
                    Ok(_) => {
                        this.notify(NoticeSource::Language, Notice::Error(t("Cargo did not report an executable to debug").into()));
                        return cx.notify();
                    }
                    Err(output) => {
                        for line in output.lines() {
                            this.lsp.output.push_back(line.to_string());
                        }
                        while this.lsp.output.len() > 5000 {
                            this.lsp.output.pop_front();
                        }
                        this.set_task_problems(crate::problems::parse_output(&output, &root), cx);
                        this.notify(NoticeSource::Language, Notice::Error(t("The build failed. See Output and Problems for the compiler errors.").into()));
                        return cx.notify();
                    }
                };
                this.dismiss_notice();
                let title = tf("Debug {}", &[&entry.item.label]);
                let plan = entry.item.debug.as_ref().expect("checked above");
                let configs = run::launch_configs(&title, plan, &root, &executables);
                this.choose_debug_config_trusted(configs, window, cx);
            })
            .ok();
        })
        .detach();
    }
}
