//! Tasks: find the project's scripts and run them in a terminal (VS Code "Run Task").
//!
//! Sources: `.codeyard/tasks.jsonc` and `.vscode/tasks.json` (label + command),
//! `package.json` scripts (npm/pnpm/yarn/bun, by lock file), Makefile
//! targets, Cargo and Go. Files are read through the host: on the server over SSH.

use gpui::{AppContext as _, Context, Window};
use serde_json::Value;

use super::{Notice, OverlayKind, Workbench};
use crate::picker::{PickItem, Picker, Source as PickSource};
use crate::ui;

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub label: String,
    pub command: String,
    /// Where it came from ("npm", "make", "cargo", "tasks.jsonc"…).
    pub source: &'static str,
}

const TASKS_FILE: &str = ".codeyard/tasks.jsonc";
/// Source shown for the tasks of a workspace file.
pub(crate) const WORKSPACE_SOURCE: &str = "workspace";
/// Task file left under the old name (codeapp): read if the new one is missing.
const LEGACY_TASKS_FILE: &str = ".codeapp/tasks.jsonc";
const TASKS_TEMPLATE: &str = "// Tasks for this project (Tasks: Run Task, ⇧⌘B runs the first one with \"build\" in its label).\n[\n  // { \"label\": \"build\", \"command\": \"make build\" },\n]\n";

/// `[ { label, command } ]` (ours) or `{ "tasks": [ { label, command, args } ] }` (VS Code).
///
/// `dependsOn` (a label or list of labels) refers to tasks in the same file and runs their commands
/// first: `(dep) && (dep) && command`. VS Code defaults to parallel,
/// but one terminal runs them in order so a failure stops the chain; each dependency gets its own subshell
/// so a `cd` can't leak into the next. Cycles and unknown labels are skipped.
pub fn parse_task_file(text: &str, source: &'static str) -> Vec<Task> {
    let Ok(v) = serde_json::from_str::<Value>(&theme::themes::strip_jsonc(text)) else { return Vec::new() };
    let items = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.get("tasks").and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    // (label, command, dependency labels)
    let raw: Vec<(String, String, Vec<String>)> = items
        .iter()
        .filter_map(|t| {
            let depends: Vec<String> = match &t["dependsOn"] {
                Value::String(s) => vec![s.clone()],
                Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                _ => Vec::new(),
            };
            let args: Vec<String> = t["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(shell_quote)).collect()).unwrap_or_default();
            let command = match (t["command"].as_str(), args.is_empty()) {
                (Some(c), true) => c.to_string(),
                (Some(c), false) => format!("{c} {}", args.join(" ")),
                // A combined task made only of dependencies.
                (None, _) if !depends.is_empty() => String::new(),
                (None, _) => return None,
            };
            // `options.cwd`: the command runs in that folder (variables are expanded at run time).
            let command = match (t["options"]["cwd"].as_str(), command.is_empty()) {
                (Some(cwd), false) => format!("cd {} && {command}", shell_quote(cwd)),
                _ => command,
            };
            let label = t["label"].as_str().map(str::to_string).unwrap_or_else(|| command.clone());
            Some((label, command, depends))
        })
        .collect();

    fn chain(label: &str, raw: &[(String, String, Vec<String>)], stack: &mut Vec<String>) -> Vec<String> {
        let Some((_, command, depends)) = raw.iter().find(|(l, ..)| l == label) else { return Vec::new() };
        if stack.iter().any(|s| s == label) {
            return Vec::new();
        }
        stack.push(label.to_string());
        let mut parts: Vec<String> = Vec::new();
        for d in depends {
            for part in chain(d, raw, stack) {
                if !parts.contains(&part) {
                    parts.push(part);
                }
            }
        }
        stack.pop();
        if !command.is_empty() {
            parts.push(command.clone());
        }
        parts
    }

    raw.iter()
        .filter_map(|(label, own, depends)| {
            let mut parts: Vec<String> = Vec::new();
            for d in depends {
                for part in chain(d, &raw, &mut vec![label.clone()]) {
                    if !parts.contains(&part) {
                        parts.push(format!("({part})"));
                    }
                }
            }
            if !own.is_empty() {
                parts.push(own.clone());
            }
            (!parts.is_empty()).then(|| Task { label: label.clone(), command: parts.join(" && "), source })
        })
        .collect()
}

/// Expand VS Code task variables: `${workspaceFolder}`, `${workspaceFolderBasename}`, `${file}`,
/// `${fileBasename}`, `${fileBasenameNoExtension}`, `${fileExtname}`, `${fileDirname}`,
/// `${relativeFile}`, `${env:AD}`. With no active file, `${file…}` and unknown variables stay
/// as written (the shell won't expand them either, and the user sees it).
pub fn expand_variables(command: &str, root: &str, file: Option<&str>) -> String {
    expand(command, root, file, false)
}

/// The same for a command line that a shell will read: a value is quoted for the place it lands in (bare, inside `'…'` or
/// inside `"…"`), so a file called `a;rm -rf ~;.txt` is still one argument. Values that need no quoting are left alone.
pub fn expand_variables_for_shell(command: &str, root: &str, file: Option<&str>) -> String {
    expand(command, root, file, true)
}

#[derive(Clone, Copy, PartialEq)]
enum Quote {
    Bare,
    Single,
    Double,
}

/// Where the next character of `text` would be, as a POSIX shell reads it.
fn quote_at_end(text: &str) -> Quote {
    let (mut state, mut escaped) = (Quote::Bare, false);
    for c in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match (state, c) {
            (Quote::Single, '\'') => state = Quote::Bare,
            (Quote::Single, _) => {}
            (_, '\\') => escaped = true,
            (Quote::Bare, '\'') => state = Quote::Single,
            (Quote::Bare, '"') => state = Quote::Double,
            (Quote::Double, '"') => state = Quote::Bare,
            _ => {}
        }
    }
    state
}

fn for_shell(value: &str, at: Quote) -> String {
    match at {
        Quote::Bare => shell_quote(value),
        Quote::Single => crate::shell_quote::in_single(value),
        Quote::Double => value.chars().fold(String::with_capacity(value.len()), |mut s, c| {
            if matches!(c, '\\' | '"' | '$' | '`') {
                s.push('\\');
            }
            s.push(c);
            s
        }),
    }
}

fn expand(command: &str, root: &str, file: Option<&str>, shell: bool) -> String {
    let root = root.trim_end_matches('/');
    let name = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            // Unterminated variable: leave the rest as it is (the prefix was already written).
            out.push_str(&rest[start..]);
            return out;
        };
        let var = &rest[start + 2..start + end];
        let value = match var {
            "workspaceFolder" => Some(root.to_string()),
            "workspaceFolderBasename" => Some(name(root)),
            "file" => file.map(str::to_string),
            "fileBasename" => file.map(name),
            "fileBasenameNoExtension" => file.map(|f| name(f).rsplit_once('.').map_or_else(|| name(f), |(s, _)| s.to_string())),
            "fileExtname" => file.map(|f| name(f).rsplit_once('.').map_or(String::new(), |(_, e)| format!(".{e}"))),
            "fileDirname" => file.map(|f| f.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string())),
            "relativeFile" => file.map(|f| f.strip_prefix(&format!("{root}/")).unwrap_or(f).to_string()),
            v => v.strip_prefix("env:").map(|n| std::env::var(n).unwrap_or_default()),
        };
        match value {
            Some(v) if shell => {
                let at = quote_at_end(&out);
                // An empty value bare would vanish as an argument: `''` keeps it one.
                out.push_str(&if v.is_empty() && at == Quote::Bare { "''".to_string() } else { for_shell(&v, at) });
            }
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..=start + end]),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

pub(crate) fn shell_quote(s: &str) -> String {
    // A task's own arguments (`--fix`) stay bare; anything a shell could read differently is quoted.
    if !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "-_./=:@%+,".contains(c)) { s.to_string() } else { crate::shell_quote::quote(s) }
}

pub fn npm_tasks(package_json: &str, runner: &str) -> Vec<Task> {
    let Ok(v) = serde_json::from_str::<Value>(package_json) else { return Vec::new() };
    v["scripts"]
        .as_object()
        .map(|o| {
            o.keys().map(|k| Task { label: format!("{runner}: {k}"), command: format!("{runner} run {}", shell_quote(k)), source: "package.json" }).collect()
        })
        .unwrap_or_default()
}

/// Makefile targets: lines starting with `name:` (excluding variable assignments, pattern and special targets).
pub fn make_tasks(makefile: &str) -> Vec<Task> {
    let mut out: Vec<Task> = Vec::new();
    for line in makefile.lines() {
        let Some((name, rest)) = line.split_once(':') else { continue };
        if rest.starts_with('=') || name.is_empty() || name.starts_with(['.', '\t', ' ', '#']) {
            continue;
        }
        if !name.chars().all(|c| c.is_alphanumeric() || "_-./".contains(c)) {
            continue;
        }
        if !out.iter().any(|t| t.label == format!("make: {name}")) {
            out.push(Task { label: format!("make: {name}"), command: format!("make {name}"), source: "Makefile" });
        }
    }
    out
}

fn fixed(source: &'static str, cmds: &[&str]) -> Vec<Task> {
    cmds.iter().map(|c| Task { label: format!("{source}: {}", c.split_whitespace().nth(1).unwrap_or(c)), command: c.to_string(), source }).collect()
}

impl Workbench {
    /// Find the project's tasks (in the background), then continue with `then`.
    fn detect_tasks(&mut self, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, Vec<Task>, &mut Window, &mut Context<Self>) + 'static) {
        let Some(root) = self.active_root(cx) else {
            self.notice =
                Some(Notice::Info(crate::i18n::t("Open a folder first: tasks come from its package.json, Makefile, Cargo.toml or tasks.jsonc.").into()));
            cx.notify();
            return;
        };
        let host = self.remote.host.clone();
        let asked = root.clone();
        // Shared tasks of the workspace file: they run in the active file's folder like the folder's own.
        let shared = self.workspace_tasks_and_launch().0;
        let task = cx.background_executor().spawn(async move {
            let read = |rel: &str| host.read(&ui::join(&root, rel)).ok().map(|(d, _)| String::from_utf8_lossy(&d).into_owned());
            let exists = |rel: &str| host.stat(&ui::join(&root, rel)).is_ok();
            let mut tasks = Vec::new();
            if let Some(t) = &shared {
                tasks.extend(parse_task_file(t, WORKSPACE_SOURCE));
            }
            if let Some(t) = read(TASKS_FILE).or_else(|| read(LEGACY_TASKS_FILE)) {
                tasks.extend(parse_task_file(&t, "tasks.jsonc"));
            }
            if let Some(t) = read(".vscode/tasks.json") {
                tasks.extend(parse_task_file(&t, ".vscode/tasks.json"));
            }
            if let Some(p) = read("package.json") {
                let runner = if exists("pnpm-lock.yaml") {
                    "pnpm"
                } else if exists("yarn.lock") {
                    "yarn"
                } else if exists("bun.lockb") || exists("bun.lock") {
                    "bun"
                } else {
                    "npm"
                };
                tasks.extend(npm_tasks(&p, runner));
            }
            if let Some(m) = read("Makefile").or_else(|| read("makefile")) {
                tasks.extend(make_tasks(&m));
            }
            if exists("Cargo.toml") {
                tasks.extend(fixed("cargo", &["cargo build", "cargo check", "cargo test", "cargo run", "cargo clippy"]));
            }
            if exists("go.mod") {
                tasks.extend(fixed("go", &["go build ./...", "go test ./...", "go vet ./..."]));
            }
            tasks
        });
        cx.spawn(async move |this, cx| {
            let tasks = task.await;
            let handle = this.update(cx, |this, _| this.window_handle).ok();
            if let Some(handle) = handle {
                cx.update_window(handle, |_, window, cx| {
                    this.update(cx, |this, cx| {
                        // Tasks run in (and trust is asked for) the active file's folder: if that changed while they were read,
                        // they belong to another folder and are dropped.
                        if this.active_root(cx).as_deref() == Some(asked.as_str()) {
                            then(this, tasks, window, cx);
                        }
                    })
                    .ok();
                })
                .ok();
            }
        })
        .detach();
    }

    /// "Tasks: Run Task…"
    pub(super) fn show_tasks(&mut self, cx: &mut Context<Self>) {
        self.detect_tasks(cx, |this, tasks, window, cx| {
            if tasks.is_empty() {
                this.notice = Some(Notice::Info(crate::i18n::t("No tasks found. “Tasks: Configure Tasks” creates .codeyard/tasks.jsonc.").into()));
                cx.notify();
                return;
            }
            if this.overlay.is_some() {
                this.close_overlay(window, cx);
            }
            let items = tasks
                .iter()
                .map(|t| PickItem { label: t.label.clone().into(), detail: Some(format!("{} · {}", t.command, t.source).into()), keys: None })
                .collect();
            let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Run task"), "", None, cx));
            this.show_overlay(picker, OverlayKind::Tasks(tasks), window, cx);
        });
    }

    /// ⇧⌘B: the first task with "build" in its name (open the list if none).
    pub(super) fn run_build_task(&mut self, cx: &mut Context<Self>) {
        self.detect_tasks(cx, |this, tasks, window, cx| match tasks.iter().find(|t| t.label.to_lowercase().contains("build")) {
            Some(t) => this.run_task(t.clone(), window, cx),
            None => this.show_tasks(cx),
        });
    }

    /// "Test: Run Test at Cursor" / "Run Tests in File": the project's own test command runs in the task
    /// terminal ("Rerun Last Task" repeats the same test, errors land in Problems).
    pub(super) fn run_test(&mut self, whole_file: bool, cx: &mut Context<Self>) {
        let (Some(ed), Some(root)) = (self.active_editor().cloned(), self.active_root(cx)) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a file inside a folder to run its tests.").into()));
            cx.notify();
            return;
        };
        let (path, text, line) = {
            let e = ed.read(cx);
            (e.path.clone(), e.text_lines().join("\n"), e.cursor_position().0 - 1)
        };
        let Some(rel) = path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).map(str::to_string) else {
            self.notice = Some(Notice::Info(crate::i18n::t("This file is outside the open folder.").into()));
            cx.notify();
            return;
        };
        let host = self.remote.host.clone();
        let pj_path = ui::join(&root, "package.json");
        let (lookup_root, lookup_rel) = (root.clone(), rel.clone());
        let generation = self.lsp.generation;
        let read = cx.background_executor().spawn(async move {
            let package = host.read(&pj_path).ok().map(|(d, _)| String::from_utf8_lossy(&d).into_owned());
            let context = lookup_rel.ends_with(".rs").then(|| crate::cargo_project::context_on_host(host.as_ref(), &lookup_root, &lookup_rel)).flatten();
            (package, context)
        });
        cx.spawn(async move |this, cx| {
            let (package_json, context) = read.await;
            let handle = this.update(cx, |this, _| this.window_handle).ok();
            let Some(handle) = handle else { return };
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if this.active_root(cx).as_deref() != Some(root.as_str()) || this.lsp.generation != generation {
                        return;
                    }
                    match crate::testrun::plan_in_crate(&rel, &text, line, whole_file, package_json.as_deref(), context.as_ref()) {
                        Some(run) => this.run_task(Task { label: run.label, command: run.command, source: "test" }, window, cx),
                        None => {
                            this.notice = Some(Notice::Info(
                                if whole_file {
                                    crate::i18n::t("No tests found for this file type.")
                                } else {
                                    crate::i18n::t("No test at the cursor: put it inside a test function.")
                                }
                                .into(),
                            ));
                            cx.notify();
                        }
                    }
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn rerun_last_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.jobs.last.clone() {
            Some(t) => self.run_task(t, window, cx),
            None => self.show_tasks(cx),
        }
    }

    /// Run the task in a new terminal (tab name is the task's name).
    pub(super) fn run_task(&mut self, task: Task, window: &mut Window, cx: &mut Context<Self>) {
        self.require_trust(window, cx, move |this, window, cx| this.run_task_trusted(task, window, cx));
    }

    fn run_task_trusted(&mut self, task: Task, window: &mut Window, cx: &mut Context<Self>) {
        // The task runs at the active file's root (in a multi-folder workspace, the file's folder).
        let root = self.active_root(cx).unwrap_or_default();
        self.jobs.root = Some(root.clone());
        self.new_terminal_in(Some(root.clone()), window, cx);
        if let Some(t) = self.terminals.get(self.active_terminal).cloned() {
            let file = self.active_editor().map(|e| e.read(cx).path.clone());
            let (label, command) = (task.label.clone(), expand_variables_for_shell(&task.command, &root, file.as_deref()));
            t.update(cx, |t, cx| {
                t.set_fixed_title(label);
                t.send(&format!("{command}\n"));
                cx.notify();
            });
        }
        self.jobs.last = Some(task);
        self.watch_task_output(cx);
        cx.notify();
    }

    /// Watch the active terminal (the task terminal just opened): problems are rescanned as output arrives.
    /// A new task clears the old task's problems (like VS Code).
    fn watch_task_output(&mut self, cx: &mut Context<Self>) {
        let Some(term) = self.terminals.get(self.active_terminal).cloned() else { return };
        let id = term.entity_id();
        self.jobs.terminal = Some(id);
        self.set_task_problems(Vec::new(), cx);
        cx.observe(&term, move |this, term, cx| this.schedule_problem_scan(id, term, cx)).detach();
    }

    /// While output continues, scan at most every half second (not on every output chunk).
    fn schedule_problem_scan(&mut self, id: gpui::EntityId, term: gpui::Entity<crate::terminal::TerminalView>, cx: &mut Context<Self>) {
        if self.jobs.terminal != Some(id) || self.jobs.scan_pending {
            return;
        }
        self.jobs.scan_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
            this.update(cx, |this, cx| {
                this.jobs.scan_pending = false;
                if this.jobs.terminal != Some(id) {
                    return;
                }
                let (text, root) = (term.read(cx).scrollback_text(), this.jobs.root.clone().or_else(|| this.root(cx)).unwrap_or_default());
                let parse = cx.background_executor().spawn(async move { crate::problems::parse_output(&text, &root) });
                cx.spawn(async move |this, cx| {
                    let found = parse.await;
                    this.update(cx, |this, cx| {
                        if this.jobs.terminal == Some(id) {
                            this.set_task_problems(found, cx);
                        }
                    })
                    .ok();
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }

    /// Replace task problems; squiggles in the affected open editors are updated.
    pub(super) fn set_task_problems(&mut self, mut new: Vec<(String, crate::lsp::Diagnostic)>, cx: &mut Context<Self>) {
        // The same line can be scanned twice (terminal line wrap, command echo): counted once.
        let mut seen = std::collections::HashSet::new();
        new.retain(|(p, d)| seen.insert((p.clone(), d.start, d.end, d.severity, d.message.clone())));
        if new == self.jobs.problems {
            return;
        }
        let touched: std::collections::BTreeSet<String> = self.jobs.problems.iter().chain(new.iter()).map(|(p, _)| p.clone()).collect();
        self.jobs.problems = new;
        for ed in self.all_editors() {
            let path = ed.read(cx).path.clone();
            if touched.contains(&path) {
                let d = self.diagnostics_for(&path);
                ed.update(cx, |e, cx| e.set_diagnostics(d, cx));
            }
        }
        cx.notify();
    }

    /// Language server diagnostics and task problems together, per file.
    pub(super) fn all_problems(&self) -> std::collections::BTreeMap<String, Vec<crate::lsp::Diagnostic>> {
        let mut all = self.lsp.diagnostics.clone();
        for (p, d) in &self.jobs.problems {
            all.entry(p.clone()).or_default().push(d.clone());
        }
        all
    }

    pub(super) fn diagnostics_for(&self, path: &str) -> Vec<crate::lsp::Diagnostic> {
        let mut d = self.lsp.diagnostics.get(path).cloned().unwrap_or_default();
        d.extend(self.jobs.problems.iter().filter(|(p, _)| p == path).map(|(_, x)| x.clone()));
        d
    }

    /// Give a newly opened editor this file's task problems (server diagnostics come through their own path).
    pub(super) fn apply_task_problems(&mut self, ed: &gpui::Entity<crate::editor::EditorView>, cx: &mut Context<Self>) {
        if self.jobs.problems.is_empty() {
            return;
        }
        let d = self.diagnostics_for(&ed.read(cx).path);
        if !d.is_empty() {
            ed.update(cx, |e, cx| e.set_diagnostics(d, cx));
        }
    }

    /// "Tasks: Configure Tasks": `.codeyard/tasks.jsonc` (create with an example if missing).
    pub(super) fn configure_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else { return };
        let path = ui::join(&root, TASKS_FILE);
        let host = self.remote.host.clone();
        let p = path.clone();
        let task = cx.background_executor().spawn(async move {
            if host.stat(&p).is_err() {
                host.create_file(&p)?;
                host.write(&p, TASKS_TEMPLATE.as_bytes().to_vec(), None)?;
            }
            Ok::<_, host::HostError>(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => this.open_file(path, window, cx),
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not create {0}: {1}", &[&TASKS_FILE, &e]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tasks_in_project_files() {
        let ours = parse_task_file(
            r#"[ { "label": "build", "command": "make build" }, // yorum
        ]"#,
            "tasks.jsonc",
        );
        assert_eq!(ours, vec![Task { label: "build".into(), command: "make build".into(), source: "tasks.jsonc" }]);
        let vs = parse_task_file(
            r#"{ "version": "2.0.0", "tasks": [ { "label": "lint", "command": "eslint", "args": ["src", "--fix", "a b"] } ] }"#,
            ".vscode/tasks.json",
        );
        assert_eq!(vs[0].command, "eslint src --fix 'a b'");
        let npm = npm_tasks(r#"{ "scripts": { "dev": "vite", "test": "vitest" } }"#, "pnpm");
        assert_eq!(npm.iter().map(|t| t.command.as_str()).collect::<Vec<_>>(), vec!["pnpm run dev", "pnpm run test"]);
        let make = make_tasks("CC := gcc\n.PHONY: all\nall: build\nbuild: main.o\n\tcc -o x\n%.o: %.c\nclean:\n");
        assert_eq!(make.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), vec!["make: all", "make: build", "make: clean"]);
    }

    /// A file name is data: in a shell command it stays one argument in every quoting context.
    #[test]
    fn variables_are_quoted_for_the_place_they_land_in() {
        let f = Some("/p/src/a;rm -rf ~;.txt");
        assert_eq!(expand_variables_for_shell("cat ${file}", "/p", f), "cat '/p/src/a;rm -rf ~;.txt'");
        assert_eq!(expand_variables_for_shell("cat '${file}'", "/p", f), "cat '/p/src/a;rm -rf ~;.txt'");
        assert_eq!(expand_variables_for_shell(r#"cat "${fileBasename}""#, "/p", Some("/p/$(id)`x`\"q\".txt")), r#"cat "\$(id)\`x\`\"q\".txt""#);
        assert_eq!(expand_variables_for_shell("cat ${file}", "/p", Some("/p/it's.txt")), r#"cat '/p/it'"'"'s.txt'"#);
        assert_eq!(expand_variables_for_shell("cat '${file}'", "/p", Some("/p/it's.txt")), r#"cat '/p/it'"'"'s.txt'"#);
        // Plain names are left alone, so existing tasks read the same.
        assert_eq!(expand_variables_for_shell("make -C ${workspaceFolder}/sub", "/p/", None), "make -C /p/sub");
        assert_eq!(expand_variables_for_shell("echo ${fileExtname}x", "/p", Some("/p/a")), "echo ''x", "an empty value is still one argument");
        // The generic expansion (debug configurations, JSON) stays raw.
        assert_eq!(expand_variables("${file}", "/p", f), "/p/src/a;rm -rf ~;.txt");
    }

    #[test]
    fn expands_vscode_variables_and_cwd() {
        let f = Some("/p/src/app.test.ts");
        assert_eq!(expand_variables("make -C ${workspaceFolder}/sub ${workspaceFolderBasename}", "/p/", None), "make -C /p/sub p");
        assert_eq!(
            expand_variables("run ${file} ${relativeFile} ${fileBasename} ${fileBasenameNoExtension} ${fileExtname} ${fileDirname}", "/p", f),
            "run /p/src/app.test.ts src/app.test.ts app.test.ts app.test .ts /p/src"
        );
        assert_eq!(
            expand_variables("echo ${file} ${nope} ${env:CODEYARD_NO_SUCH_VAR}x", "/p", None),
            "echo ${file} ${nope} x",
            "unknowns stay, a missing env var is empty"
        );
        assert_eq!(expand_variables("broken ${workspaceFolder", "/p", None), "broken ${workspaceFolder", "unterminated variable stays as is");
        let tasks = parse_task_file(
            r#"{ "tasks": [ { "label": "t", "command": "ls", "args": ["-l"], "options": { "cwd": "${workspaceFolder}/my dir" } } ] }"#,
            "tasks.json",
        );
        assert_eq!(tasks[0].command, "cd '${workspaceFolder}/my dir' && ls -l");
    }

    #[test]
    fn depends_on_chains_commands_without_cycles() {
        let tasks = parse_task_file(
            r#"{ "tasks": [
                { "label": "lint", "command": "eslint ." },
                { "label": "gen", "command": "make gen", "options": { "cwd": "api" } },
                { "label": "build", "command": "tsc", "dependsOn": ["lint", "gen"] },
                { "label": "all", "dependsOn": "build" },
                { "label": "loop-a", "command": "a", "dependsOn": "loop-b" },
                { "label": "loop-b", "command": "b", "dependsOn": "loop-a" },
                { "label": "ghost", "command": "g", "dependsOn": "missing" }
            ] }"#,
            "tasks.json",
        );
        let cmd = |l: &str| tasks.iter().find(|t| t.label == l).map(|t| t.command.clone()).unwrap_or_else(|| panic!("{l}"));
        assert_eq!(cmd("build"), "(eslint .) && (cd api && make gen) && tsc");
        assert_eq!(cmd("all"), "(eslint .) && (cd api && make gen) && (tsc)", "a combined task runs only its dependencies");
        assert_eq!(cmd("loop-a"), "(b) && a", "cycles are cut");
        assert_eq!(cmd("ghost"), "g", "unknown dependency is skipped");
    }
}
