//! Workbench: the Testing sidebar view. Tests are found by reading the project's test files (Rust, Go, Python and
//! JavaScript) in the background. Each run uses the project's own runner: its output goes to a "Test Results" terminal
//! and is read for every test's outcome, time and failure message. A test can also be started under the debugger.

use std::collections::HashSet;

use gpui::{AnyElement, Context, Entity, SharedString, Window, div, px};

use super::*;
use crate::input::{FieldEvent, TextField};
use crate::testexp::{self, Job, Kind, Outcome, Row, TestFile, TestItem};

gpui::actions!(workbench, [ShowTesting, RunAllTests, RunFailedTests, RefreshTests, StopTests, DebugTestAtCursor]);

/// Most files read when looking for tests, and the largest one read.
const MAX_FILES: usize = 4000;
const MAX_FILE_BYTES: usize = 1 << 20;
/// Most files listed under the folder.
const LIST_LIMIT: u32 = 100_000;
/// Most output kept from one run for reading the results (the terminal shows all of it).
const MAX_OUTPUT: usize = 8 << 20;

/// State of the Testing view (kept while the window lives).
#[derive(Default)]
pub(super) struct TestingState {
    pub(super) files: Vec<TestFile>,
    pub(super) results: testexp::Results,
    /// Folder the files were found in.
    pub(super) root: Option<String>,
    js_runner: Option<&'static str>,
    /// Files shown open (they start closed; a run opens the ones with failures).
    pub(super) expanded: HashSet<String>,
    pub(super) only_failed: bool,
    pub(super) filter: Option<Entity<TextField>>,
    pub(super) scanning: bool,
    /// The running process, to stop it.
    running: Option<(Arc<dyn Host>, u32)>,
    /// Results from before the run, put back for tests the run did not report (or did not reach).
    before: testexp::Results,
    pub(super) run: Option<gpui::Task<()>>,
    output: Option<Entity<TerminalView>>,
    scroll: gpui::UniformListScrollHandle,
}

impl TestingState {
    /// Tests of `scope` still marked running get their earlier result back (or none).
    fn restore_unreported(&mut self, scope: &[(String, String)]) {
        for key in scope {
            if self.results.get(key).is_some_and(|r| r.outcome == Outcome::Running) {
                match self.before.remove(key) {
                    Some(old) => self.results.insert(key.clone(), old),
                    None => self.results.remove(key),
                };
            }
        }
    }
}

/// `sh -c` arguments running `command` with its errors in the same stream (the process's own error stream is dropped).
fn shell(command: &str) -> Vec<String> {
    vec!["-c".into(), format!("exec {command} 2>&1")]
}

/// Reads the candidate files and keeps those with tests.
fn discover(host: &Arc<dyn Host>, root: &str) -> Result<(Vec<TestFile>, Option<&'static str>), String> {
    let list = host.list_files(root, LIST_LIMIT).map_err(|e| e.to_string())?;
    let read = |rel: &str| host.read(&ui::join(root, rel)).ok().map(|(d, _)| d);
    let package_json = read("package.json").map(|d| String::from_utf8_lossy(&d).into_owned());
    let pj = package_json.as_deref();
    let manifests = list.paths.iter().filter(|p| p.as_str() == "Cargo.toml" || p.ends_with("/Cargo.toml")).cloned().collect();
    let mut files = Vec::new();
    for rel in list.paths.iter().filter(|r| testexp::is_candidate(r)).take(MAX_FILES) {
        let (Some(kind), Some(data)) = (testexp::kind_of(rel), read(rel)) else { continue };
        if data.len() > MAX_FILE_BYTES {
            continue;
        }
        let text = String::from_utf8_lossy(&data);
        let context = crate::cargo_project::context_from_files(rel, &manifests);
        let tests: Vec<TestItem> = crate::testrun::cases_in_crate(rel, &text, pj, context.as_ref())
            .into_iter()
            .map(|c| TestItem { name: c.name, id: c.run.id, line: c.line, command: c.run.command })
            .collect();
        if tests.is_empty() {
            continue;
        }
        let command = crate::testrun::plan_in_crate(rel, &text, 0, true, pj, context.as_ref()).map(|r| r.command);
        files.push(TestFile { rel: rel.clone(), kind, tests, command });
    }
    Ok((files, crate::testrun::js_runner(pj)))
}

fn outcome_icon(outcome: Outcome, p: &Palette) -> (&'static str, u32) {
    match outcome {
        Outcome::Passed => ("circle-check", p.success),
        Outcome::Failed => ("circle-x", p.danger),
        Outcome::Skipped => ("circle-slash-2", p.text_muted),
        Outcome::Running | Outcome::NotRun => ("circle-dashed", p.text_muted),
    }
}

impl Workbench {
    pub(super) fn show_testing(&mut self, _: &ShowTesting, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Testing);
        self.testing_opened(window, cx);
        cx.notify();
    }

    /// The view was opened: the filter field exists, and the tests are looked for once per folder.
    pub(super) fn testing_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.jobs.tests.filter.is_none() {
            let field = cx.new(|cx| TextField::new(crate::i18n::tr("Filter (e.g. text, path)"), cx));
            self._subs.push(cx.subscribe_in(&field, window, |_, _, ev: &FieldEvent, _, cx| {
                if matches!(ev, FieldEvent::Changed) {
                    cx.notify();
                }
            }));
            self.jobs.tests.filter = Some(field);
        }
        if self.jobs.tests.root != self.root(cx) && !self.jobs.tests.scanning {
            self.refresh_tests(cx);
        }
    }

    /// Looks for tests again in the background.
    pub(super) fn refresh_tests(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else {
            self.jobs.tests.files.clear();
            self.jobs.tests.root = None;
            cx.notify();
            return;
        };
        let host = self.remote.host.clone();
        let generation = self.lsp.generation;
        self.jobs.tests.scanning = true;
        let r = root.clone();
        let task = cx.background_executor().spawn(async move { discover(&host, &r) });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            this.update(cx, |this, cx| {
                this.jobs.tests.scanning = false;
                if this.root(cx).as_deref() != Some(&root) || this.lsp.generation != generation {
                    if this.chrome.panel == Some(Panel::Testing) {
                        this.refresh_tests(cx);
                    }
                    return;
                }
                let t = &mut this.jobs.tests;
                match found {
                    Ok((files, js)) => {
                        // Results of tests that are gone are dropped.
                        let ids: HashSet<(String, String)> = files.iter().flat_map(|f| f.tests.iter().map(|x| (f.rel.clone(), x.id.clone()))).collect();
                        t.results.retain(|k, _| ids.contains(k));
                        t.files = files;
                        t.js_runner = js;
                        t.root = Some(root);
                    }
                    Err(e) => this.notice = Some(Notice::Error(crate::i18n::tf("Could not look for tests: {}", &[&e]).into())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn failed_test_count(&self) -> usize {
        self.jobs.tests.results.values().filter(|r| r.outcome == Outcome::Failed).count()
    }

    pub(super) fn run_all_tests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let jobs = testexp::run_all(&self.jobs.tests.files, self.jobs.tests.js_runner);
        self.run_test_jobs(jobs, window, cx);
    }

    /// Runs each failed test again (one command per test).
    pub(super) fn run_failed_tests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = &self.jobs.tests;
        let jobs = t
            .files
            .iter()
            .flat_map(|f| {
                f.tests
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| t.results.get(&(f.rel.clone(), x.id.clone())).is_some_and(|r| r.outcome == Outcome::Failed))
                    .filter_map(|(i, _)| testexp::job_for(f, Some(i)))
            })
            .collect();
        self.run_test_jobs(jobs, window, cx);
    }

    fn run_test_jobs(&mut self, jobs: Vec<Job>, window: &mut Window, cx: &mut Context<Self>) {
        if jobs.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("No tests to run.").into()));
            cx.notify();
            return;
        }
        // Trust is asked for the folder the tests run in, whichever file is active.
        let Some(root) = self.jobs.tests.root.clone().or_else(|| self.root(cx)) else { return };
        self.require_trust_for(Some(root.clone()), window, cx, move |this, window, cx| this.run_test_jobs_trusted(jobs, root, window, cx));
    }

    fn run_test_jobs_trusted(&mut self, jobs: Vec<Job>, root: String, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_tests(cx);
        let out = self.test_output(window, cx);
        let host = self.remote.host.clone();
        for key in jobs.iter().flat_map(|j| j.scope.iter()) {
            if let Some(old) = self.jobs.tests.results.insert(key.clone(), testexp::TestResult { outcome: Outcome::Running, ..Default::default() }) {
                self.jobs.tests.before.insert(key.clone(), old);
            }
        }
        // Dropping the task (Stop, or a new run) cancels it; `stop_tests` cleans up after it.
        self.jobs.tests.run = Some(cx.spawn(async move |this, cx| {
            for job in jobs {
                let header = format!("\x1b[1m$ {}\x1b[0m\n", job.command);
                out.update(cx, |t, cx| t.append(header.as_bytes(), cx)).ok();
                let (h, r, command) = (host.clone(), root.clone(), job.command.clone());
                let spawned = cx.background_executor().spawn(async move { h.proc_spawn("sh", shell(&command), Some(r)) }).await;
                let mut output: Vec<u8> = Vec::new();
                let mut exit = None;
                match spawned {
                    Ok(proc) => {
                        let id = proc.id;
                        if this.update(cx, |this, _| this.jobs.tests.running = Some((this.remote.host.clone(), id))).is_err() {
                            host.proc_kill(id);
                            return;
                        }
                        let mut events = ui::bridge(proc.events);
                        while let Some(ev) = futures::StreamExt::next(&mut events).await {
                            match ev {
                                host::ProcEvent::Output(d) => {
                                    if output.len() < MAX_OUTPUT {
                                        let keep = d.len().min(MAX_OUTPUT - output.len());
                                        output.extend_from_slice(&d[..keep]);
                                    }
                                    out.update(cx, |t, cx| t.append(&d, cx)).ok();
                                }
                                host::ProcEvent::Exit(code) => {
                                    exit = code;
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let line = format!("{}\n", crate::i18n::tf("Could not start the test runner: {}", &[&e]));
                        out.update(cx, |t, cx| t.append(line.as_bytes(), cx)).ok();
                    }
                }
                let text = String::from_utf8_lossy(&output).into_owned();
                let applied = this.update(cx, |this, cx| {
                    let t = &mut this.jobs.tests;
                    t.running = None;
                    for (key, r) in testexp::outcomes(&job, &text, exit) {
                        if r.outcome == Outcome::Failed {
                            t.expanded.insert(key.0.clone());
                        }
                        t.before.remove(&key);
                        t.results.insert(key, r);
                    }
                    t.restore_unreported(&job.scope);
                    cx.notify();
                });
                if applied.is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.jobs.tests.run = None;
                this.jobs.tests.before.clear();
                let (passed, failed, _, _) = testexp::summary(&this.jobs.tests.files, &this.jobs.tests.results);
                let text = crate::i18n::tf("Tests: {0} passed, {1} failed", &[&passed, &failed]);
                this.notice = Some(if failed > 0 { Notice::Error(text.into()) } else { Notice::Info(text.into()) });
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Stops the running tests (the ones not reached keep their earlier results).
    pub(super) fn stop_tests(&mut self, cx: &mut Context<Self>) {
        let t = &mut self.jobs.tests;
        if t.run.take().is_none() {
            return;
        }
        if let Some((host, id)) = t.running.take() {
            host.proc_kill(id);
        }
        let running: Vec<(String, String)> = t.results.iter().filter(|(_, r)| r.outcome == Outcome::Running).map(|(k, _)| k.clone()).collect();
        t.restore_unreported(&running);
        t.before.clear();
        cx.notify();
    }

    /// The "Test Results" terminal, cleared and shown (without taking focus).
    fn test_output(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalView> {
        let term = match self.jobs.tests.output.clone().filter(|t| self.terminals.contains(t)) {
            Some(t) => {
                t.update(cx, |t, cx| t.reset_output(cx));
                t
            }
            None => {
                let host = self.remote.host.clone();
                let t = cx.new(|cx| TerminalView::output_only(host, crate::i18n::t("Test Results").into(), cx));
                self._subs.push(cx.subscribe_in(&t, window, |this, _, event: &TerminalEvent, window, cx| match event {
                    TerminalEvent::OpenLink(link) => this.open_link(link.clone(), window, cx),
                    TerminalEvent::TitleChanged | TerminalEvent::Exited => cx.notify(),
                    _ => {}
                }));
                self.terminals.push(t.clone());
                self.jobs.tests.output = Some(t.clone());
                t
            }
        };
        if let Some(ix) = self.terminals.iter().position(|x| *x == term) {
            self.active_terminal = ix;
        }
        self.chrome.bottom = Bottom::Terminal;
        self.chrome.panel_open = true;
        term
    }

    /// Runs one file's tests, or one test.
    pub(super) fn run_test_item(&mut self, file: usize, test: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let job = self.jobs.tests.files.get(file).and_then(|f| testexp::job_for(f, test));
        self.run_test_jobs(job.into_iter().collect(), window, cx);
    }

    /// "Test: Debug Test at Cursor": the test around the cursor in the active file.
    pub(super) fn debug_test_at_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(ed), Some(root)) = (self.active_editor().cloned(), self.active_root(cx)) else { return };
        let (path, text, line) = {
            let e = ed.read(cx);
            (e.path.clone(), e.text_lines().join("\n"), e.cursor_position().0 - 1)
        };
        let rel = path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).unwrap_or(&path).to_string();
        let tests: Vec<TestItem> = crate::testrun::cases(&rel, &text, None)
            .into_iter()
            .map(|c| TestItem { name: c.name, id: c.run.id, line: c.line, command: c.run.command })
            .collect();
        let at_cursor = crate::testrun::plan(&rel, &text, line, false, None);
        let test = at_cursor.and_then(|run| tests.iter().find(|t| t.id == run.id).cloned());
        let (Some(kind), Some(test)) = (testexp::kind_of(&rel), test) else {
            self.notice = Some(Notice::Info(crate::i18n::t("No test at the cursor: put it inside a test function.").into()));
            cx.notify();
            return;
        };
        let file = TestFile { rel, kind, tests, command: None };
        self.debug_test(file, test, root, window, cx);
    }

    /// Starts a test under the debugger. Building a Rust test runs the project's build scripts, so trust is asked for
    /// the test's folder before anything runs.
    pub(super) fn debug_test(&mut self, file: TestFile, test: TestItem, root: String, window: &mut Window, cx: &mut Context<Self>) {
        self.require_trust_for(Some(root.clone()), window, cx, move |this, window, cx| this.debug_test_trusted(file, test, root, window, cx));
    }

    fn debug_test_trusted(&mut self, file: TestFile, test: TestItem, root: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.dbg.save_task.is_some() {
            return;
        }
        if !self.debug_unsaved_editors(&root, cx).is_empty() {
            self.save_before_debug(root.clone(), cx, Box::new(move |this, window, cx| this.debug_test_trusted(file, test, root, window, cx)));
            return;
        }
        self.dbg.prepare_epoch = self.dbg.prepare_epoch.wrapping_add(1);
        let prepare_epoch = self.dbg.prepare_epoch;
        match file.kind {
            Kind::Js => {
                self.notice =
                    Some(Notice::Info(crate::i18n::t("JavaScript tests can be run here, not debugged: add a launch configuration for the runner.").into()));
                cx.notify();
            }
            Kind::Rust => {
                // The test binary is built first; its path comes from cargo's messages.
                self.notice = Some(Notice::Info(crate::i18n::t("Building the tests…").into()));
                cx.notify();
                let host = self.remote.host.clone();
                let r = root.clone();
                let rel = file.rel.clone();
                let source = self.all_editors().into_iter().find_map(|ed| {
                    let ed = ed.read(cx);
                    (ed.path == ui::join(&root, &file.rel) && ed.uses_host(&host)).then(|| ed.buffer_text())
                });
                let generation = self.lsp.generation;
                let build = cx.background_executor().spawn(async move {
                    let mut test = test;
                    if let Some(context) = crate::cargo_project::context_on_host(host.as_ref(), &r, &rel) {
                        let source = source.or_else(|| host.read(&ui::join(&r, &rel)).ok().map(|(d, _)| String::from_utf8_lossy(&d).into_owned()));
                        if let Some(source) = source
                            && let Some(run) = crate::testrun::plan_in_crate(&rel, &source, test.line, false, None, Some(&context))
                        {
                            test.command = run.command;
                            test.id = run.id;
                        }
                    }
                    let mut args = testexp::cargo_select_args(&test.command)?;
                    args.insert(0, "test".into());
                    args.extend(["--no-run".into(), "--message-format=json".into()]);
                    let output = host.exec("cargo", &args.iter().map(String::as_str).collect::<Vec<_>>(), Some(&r), None).map_err(|e| e.to_string())?;
                    let text = format!("{}\n{}", output.stdout_str(), output.stderr_str());
                    testexp::built_test_executables(&text, output.code).map(|bins| (bins, test))
                });
                cx.spawn_in(window, async move |this, cx| {
                    let built = build.await;
                    this.update_in(cx, |this, window, cx| {
                        if this.root(cx).as_deref() != Some(&root) || this.lsp.generation != generation || this.dbg.prepare_epoch != prepare_epoch {
                            return;
                        }
                        let (bins, test) = match built {
                            Ok(bins) => bins,
                            Err(output) => {
                                for line in output.lines() {
                                    this.lsp.output.push_back(line.to_string());
                                }
                                while this.lsp.output.len() > 5000 {
                                    this.lsp.output.pop_front();
                                }
                                this.set_task_problems(crate::problems::parse_output(&output, &root), cx);
                                this.notice =
                                    Some(Notice::Error(crate::i18n::t("Rust tests did not build. See Output and Problems for the compiler errors.").into()));
                                return cx.notify();
                            }
                        };
                        this.notice = None;
                        let configs = testexp::debug_configs(&file, &test, &root, &bins);
                        this.choose_debug_config_trusted(configs, window, cx);
                    })
                    .ok();
                })
                .detach();
            }
            _ => {
                let configs = testexp::debug_configs(&file, &test, &root, &[]);
                self.choose_debug_config_trusted(configs, window, cx);
            }
        }
    }

    /// Opens where a test is defined, or where it failed when the runner said.
    pub(super) fn open_test(&mut self, file: usize, test: usize, at_failure: bool, window: &mut Window, cx: &mut Context<Self>) {
        let t = &self.jobs.tests;
        let (Some(root), Some(f)) = (t.root.clone(), t.files.get(file)) else { return };
        let Some(item) = f.tests.get(test) else { return };
        let failure = at_failure.then(|| t.results.get(&(f.rel.clone(), item.id.clone())).and_then(|r| r.location.clone())).flatten();
        let (path, line) = match failure {
            // Go prints the file name only, relative to the package.
            Some((p, l)) if f.kind == Kind::Go => (f.rel.rsplit_once('/').map_or(p.clone(), |(d, _)| format!("{d}/{p}")), l.saturating_sub(1)),
            Some((p, l)) => (p, l.saturating_sub(1)),
            None => (f.rel.clone(), item.line as u32),
        };
        let path = if path.starts_with('/') { path } else { ui::join(&root, &path) };
        self.open_location(path, line, 0, window, cx);
    }

    pub(super) fn testing_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.jobs.tests;
        let filter = t.filter.as_ref().map(|f| f.read(cx).text().to_string()).unwrap_or_default();
        let rows = testexp::rows(&t.files, &t.results, &t.expanded, &filter, t.only_failed);
        let running = t.run.is_some();
        let (passed, failed, skipped, total) = testexp::summary(&t.files, &t.results);
        let header = div()
            .h(px(layout::VIEW_HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .pl(px(theme::SPACE[6]))
            .pr(px(theme::SPACE[3]))
            .text_size(px(text::XXS.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(c(p.text_muted))
            .child(crate::i18n::upper(crate::i18n::t("Testing")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[1]))
                    .child(if running {
                        ui::icon_button("tests-stop", "square", crate::i18n::t("Stop"), p).on_click(cx.listener(|this, _, _, cx| this.stop_tests(cx)))
                    } else {
                        ui::icon_button("tests-run-all", "play", crate::i18n::t("Run All Tests"), p)
                            .on_click(cx.listener(|this, _, w, cx| this.run_all_tests(w, cx)))
                    })
                    .child(
                        ui::icon_button("tests-run-failed", "rotate-ccw", crate::i18n::t("Run Failed Tests"), p)
                            .on_click(cx.listener(|this, _, w, cx| this.run_failed_tests(w, cx))),
                    )
                    .child(
                        ui::icon_button("tests-only-failed", "list-filter", crate::i18n::t("Show Only Failed Tests"), p)
                            .when(t.only_failed, |b| b.bg(c(p.surface_active)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.jobs.tests.only_failed = !this.jobs.tests.only_failed;
                                cx.notify();
                            })),
                    )
                    .child(
                        ui::icon_button("tests-refresh", "refresh-cw", crate::i18n::t("Refresh Tests"), p)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_tests(cx))),
                    )
                    .child(ui::icon_button("tests-collapse", "chevrons-down-up", crate::i18n::t("Collapse All"), p).on_click(cx.listener(|this, _, _, cx| {
                        let t = &mut this.jobs.tests;
                        t.expanded.clear();
                        cx.notify();
                    }))),
            );
        let muted = |s: SharedString| div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).child(s);
        let body: AnyElement = if t.scanning && t.files.is_empty() {
            div()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .px(px(theme::SPACE[6]))
                .py(px(theme::SPACE[2]))
                .text_color(c(p.text_muted))
                .child(ui::spinner("tests-scanning", layout::ICON - 2., p.text_muted))
                .child(crate::i18n::t("Looking for tests…"))
                .into_any_element()
        } else if self.root(cx).is_none() {
            muted(crate::i18n::t("Open a folder to find its tests.").into()).into_any_element()
        } else if t.files.is_empty() {
            muted(crate::i18n::t("No tests found. Tests are read from Rust, Go, Python and JavaScript test files.").into()).into_any_element()
        } else {
            let p = *p;
            div()
                .flex_1()
                .min_h_0()
                .child(
                    gpui::uniform_list(
                        "test-rows",
                        rows.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                            range.filter_map(|ix| Some(this.test_row(ix, rows.get(ix)?, &p, cx))).collect()
                        }),
                    )
                    .track_scroll(t.scroll.clone())
                    .size_full(),
                )
                .into_any_element()
        };
        let summary = (total > 0).then(|| {
            let mut text = crate::i18n::tf("{0}/{1} passed", &[&passed, &total]);
            if failed > 0 {
                text.push_str(&format!(" · {}", crate::i18n::tf("{} failed", &[&failed])));
            }
            if skipped > 0 {
                text.push_str(&format!(" · {}", crate::i18n::tf("{} skipped", &[&skipped])));
            }
            div()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .px(px(theme::SPACE[6]))
                .pb(px(theme::SPACE[1]))
                .text_size(px(text::XS.0))
                .text_color(c(if failed > 0 { p.danger } else { p.text_muted }))
                .when(running, |d| d.child(ui::spinner("tests-running", layout::ICON - 4., p.text_muted)))
                .child(text)
        });
        div()
            .id("testing-view")
            .flex()
            .flex_col()
            .size_full()
            .text_size(px(text::SM.0))
            .child(header)
            .children(t.filter.clone().map(|f| {
                div()
                    .mx(px(theme::SPACE[3]))
                    .mb(px(theme::SPACE[2]))
                    .px(px(theme::SPACE[2]))
                    .py(px(theme::SPACE[1]))
                    .rounded(px(theme::radius::SM))
                    .border_1()
                    .border_color(c(p.border_input))
                    .bg(c(p.bg))
                    .child(f)
            }))
            .children(summary)
            .child(body)
            .into_any_element()
    }

    fn test_row(&self, ix: usize, row: &Row, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let group = SharedString::from(format!("test-row-{ix}"));
        let base = |depth: f32| {
            div()
                .id(("test-row", ix))
                .w_full()
                .h(px(layout::ROW_H))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .pl(px(theme::SPACE[3] + depth * layout::TREE_INDENT))
                .pr(px(theme::SPACE[3]))
                .relative()
                .cursor_pointer()
                .group(group.clone())
                .hover(|d| d.bg(c(p.surface_hover)))
        };
        // Buttons over the right end of the row while it is hovered (they take no room otherwise).
        let hover_actions = || {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .pl(px(theme::SPACE[2]))
                .pr(px(theme::SPACE[3]))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[1]))
                .bg(c(p.surface_hover))
                .invisible()
                .group_hover(group.clone(), |d| d.visible())
        };
        let status = |outcome: Outcome| -> AnyElement {
            if outcome == Outcome::Running {
                return ui::spinner(("test-running", ix), layout::ICON - 2., p.text_muted).into_any_element();
            }
            let (name, color) = outcome_icon(outcome, p);
            icon(name, layout::ICON - 2., color).into_any_element()
        };
        let label = |s: String| div().flex_shrink().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(s);
        let muted = |s: String| div().flex_none().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(s);
        let files = &self.jobs.tests.files;
        match row {
            Row::File { file, outcome, open } => {
                let Some(f) = files.get(*file) else { return div().into_any_element() };
                let (fi, rel) = (*file, f.rel.clone());
                let (name, dir) = match f.rel.rsplit_once('/') {
                    Some((d, n)) => (n.to_string(), d.to_string()),
                    None => (f.rel.clone(), String::new()),
                };
                base(0.)
                    .child(icon(if *open { "chevron-down" } else { "chevron-right" }, layout::ICON - 2., p.text_muted))
                    .child(status(*outcome))
                    .child(div().flex_none().child(name))
                    .child(label(dir).text_color(c(p.text_muted)).text_size(px(text::XS.0)))
                    .child(div().flex_1())
                    .child(muted(f.tests.len().to_string()))
                    .child(hover_actions().child(ui::icon_button(("test-file-run", ix), "play", crate::i18n::t("Run Tests in File"), p).on_click(cx.listener(
                        move |this, _, w, cx| {
                            cx.stop_propagation();
                            this.run_test_item(fi, None, w, cx);
                        },
                    ))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let c = &mut this.jobs.tests.expanded;
                        if !c.remove(&rel) {
                            c.insert(rel.clone());
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }
            Row::Test { file, test, outcome, millis } => {
                let Some((f, item)) = files.get(*file).and_then(|f| Some((f, f.tests.get(*test)?))) else { return div().into_any_element() };
                let (fi, ti) = (*file, *test);
                let debuggable = f.kind != Kind::Js;
                base(1.)
                    .child(status(*outcome))
                    .child(label(item.name.clone()))
                    .child(div().flex_1())
                    .children(millis.map(|ms| muted(if ms >= 1000 { format!("{:.1} s", ms as f64 / 1000.) } else { format!("{ms} ms") })))
                    .child(
                        hover_actions()
                            .child(ui::icon_button(("test-run", ix), "play", crate::i18n::t("Run Test"), p).on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.run_test_item(fi, Some(ti), w, cx);
                            })))
                            .when(debuggable, |d| {
                                d.child(ui::icon_button(("test-debug", ix), "bug-play", crate::i18n::t("Debug Test"), p).on_click(cx.listener(
                                    move |this, _, w, cx| {
                                        cx.stop_propagation();
                                        let t = &this.jobs.tests;
                                        let (Some(root), Some(f)) = (t.root.clone(), t.files.get(fi).cloned()) else { return };
                                        let Some(item) = f.tests.get(ti).cloned() else { return };
                                        this.debug_test(f, item, root, w, cx);
                                    },
                                )))
                            }),
                    )
                    .on_click(cx.listener(move |this, _, w, cx| this.open_test(fi, ti, false, w, cx)))
                    .into_any_element()
            }
            Row::Message { file, test, text } => {
                let (fi, ti) = (*file, *test);
                base(2.)
                    .text_color(c(p.danger))
                    .text_size(px(text::XS.0))
                    .child(label(text.clone()))
                    .on_click(cx.listener(move |this, _, w, cx| this.open_test(fi, ti, true, w, cx)))
                    .into_any_element()
            }
        }
    }

    #[cfg(test)]
    pub fn testing_rows_for_test(&self) -> Vec<String> {
        let t = &self.jobs.tests;
        testexp::rows(&t.files, &t.results, &t.expanded, "", false)
            .into_iter()
            .map(|r| match r {
                Row::File { file, outcome, .. } => format!("file:{}:{outcome:?}", t.files[file].rel),
                Row::Test { file, test, outcome, .. } => format!("test:{}:{outcome:?}", t.files[file].tests[test].name),
                Row::Message { text, .. } => format!("message:{text}"),
            })
            .collect()
    }

    #[cfg(test)]
    pub fn testing_process_for_test(&self) -> bool {
        self.jobs.tests.running.is_some()
    }

    #[cfg(test)]
    pub fn testing_busy_for_test(&self) -> bool {
        self.jobs.tests.scanning || self.jobs.tests.run.is_some()
    }
}
