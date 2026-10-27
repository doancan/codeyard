//! Starting debug sessions beyond F5: Run Without Debugging, Run to Cursor, compounds (several configurations started
//! together), `${input:…}` questions asked before a start, the post-debug task, Add Configuration, and the adapter's
//! request to run the program in a terminal.

use gpui::{Context, Window};

use super::super::*;
use super::{DebugStatus, with_window};
use crate::debug_config::{self, Compound, InputDef, InputKind, LaunchConfig};

/// A configuration waiting for the answers to its `${input:…}` questions.
pub struct PendingLaunch {
    config: LaunchConfig,
    answers: Vec<(String, String)>,
    left: Vec<InputDef>,
}

/// Snippets for "Debug: Add Configuration…": (label, configuration).
fn templates() -> Vec<(&'static str, serde_json::Value)> {
    use serde_json::json;
    vec![
        (
            crate::i18n::t("Native program (C, C++, Rust, Swift)"),
            json!({ "name": "Launch program", "type": "lldb", "request": "launch", "program": "${workspaceFolder}/target/debug/app", "args": [], "cwd": "${workspaceFolder}" }),
        ),
        (crate::i18n::t("Attach to a native process"), json!({ "name": "Attach to process", "type": "lldb", "request": "attach", "pid": 0 })),
        (
            crate::i18n::t("Go package"),
            json!({ "name": "Launch Go package", "type": "go", "request": "launch", "mode": "debug", "program": "${workspaceFolder}" }),
        ),
        (
            crate::i18n::t("Python: current file"),
            json!({ "name": "Python: current file", "type": "python", "request": "launch", "program": "${file}", "console": "integratedTerminal" }),
        ),
        (
            crate::i18n::t("Python: attach to a debug server"),
            json!({ "name": "Python: attach", "type": "python", "request": "attach", "connect": { "host": "localhost", "port": 5678 } }),
        ),
        (
            crate::i18n::t("Node.js program"),
            json!({ "name": "Launch Node program", "type": "node", "request": "launch", "program": "${workspaceFolder}/index.js" }),
        ),
        (
            crate::i18n::t("Another debug adapter"),
            json!({ "name": "Custom adapter", "type": "custom", "request": "launch", "program": "${workspaceFolder}/app", "debugAdapter": { "command": "my-debug-adapter", "args": [] } }),
        ),
    ]
}

impl Workbench {
    /// "Debug: Run Without Debugging" (⌃F5): the selected configuration (or the one F5 would find) runs without stopping.
    pub(in crate::workbench) fn run_without_debugging(&mut self, _: &super::RunWithoutDebugging, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(name) = self.dbg.last.clone().filter(|n| self.dbg.catalog.contains(n)) {
            return self.start_named(name, true, window, cx);
        }
        let Some(read) = self.read_launch_configs(true, cx) else { return };
        let asked = self.active_root(cx);
        cx.spawn(async move |this, cx| {
            let configs: Vec<LaunchConfig> = read.await.0.into_iter().map(|c| LaunchConfig { no_debug: true, ..c }).collect();
            with_window(&this, cx, move |this, window, cx| {
                if this.active_root(cx) == asked {
                    this.choose_debug_config(configs, window, cx);
                }
            });
        })
        .detach();
    }

    /// Start a configuration or compound of the launch files by name.
    pub(in crate::workbench) fn start_named(&mut self, name: String, no_debug: bool, _: &mut Window, cx: &mut Context<Self>) {
        let Some(read) = self.read_launch_configs(false, cx) else { return };
        let asked = self.active_root(cx);
        cx.spawn(async move |this, cx| {
            let (configs, compounds) = read.await;
            with_window(&this, cx, move |this, window, cx| {
                // Read from the active file's folder, which is also the one trust is asked for: drop them if it changed.
                if this.active_root(cx) != asked {
                    return;
                }
                if let Some(compound) = compounds.into_iter().find(|c| c.name == name) {
                    return this.start_compound(compound, configs, no_debug, window, cx);
                }
                let picked: Vec<LaunchConfig> = configs.into_iter().filter(|c| c.name == name).take(1).map(|c| LaunchConfig { no_debug, ..c }).collect();
                this.choose_debug_config(picked, window, cx);
            });
        })
        .detach();
    }

    /// A compound: its pre-launch task once, then each configuration in its own session, in one group.
    fn start_compound(&mut self, compound: Compound, configs: Vec<LaunchConfig>, no_debug: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut chosen = Vec::new();
        for name in &compound.configs {
            match configs.iter().find(|c| &c.name == name) {
                Some(c) if !debug_config::input_ids(c).is_empty() => {
                    return self.say(crate::i18n::tf("“{}” asks for inputs, which a compound cannot ask for: start it on its own.", &[name]), cx);
                }
                Some(c) => chosen.push(LaunchConfig { no_debug, ..c.clone() }),
                None => return self.say(crate::i18n::tf("Unknown configuration “{}” in compound “{}”", &[name, &compound.name]), cx),
            }
        }
        if chosen.is_empty() {
            return self.say(crate::i18n::tf("Compound “{}” has no configurations", &[&compound.name]), cx);
        }
        self.require_trust(window, cx, move |this, _, cx| {
            let (Some(root), host) = (this.active_root(cx), this.remote.host.clone()) else { return };
            let pre = compound.pre_launch.clone();
            let task = cx.background_executor().spawn(async move { pre.map_or(Ok(()), |cmd| super::run_pre_launch(&host, &cmd, &root)) });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    if let Err((message, _)) = result {
                        return this.say(message, cx);
                    }
                    this.dbg.next_id += 1;
                    let group = Some((this.dbg.next_id, compound.stop_all));
                    for config in chosen {
                        this.dbg.starting_group = group;
                        this.start_debug(config, cx);
                    }
                    this.dbg.starting_group = None;
                })
                .ok();
            })
            .detach();
        });
    }

    fn say(&mut self, message: String, cx: &mut Context<Self>) {
        self.notice = Some(Notice::Error(message.into()));
        cx.notify();
    }

    /// Ask the configuration's `${input:…}` questions (one after the other), then start it.
    pub(in crate::workbench) fn launch_with_inputs(&mut self, config: LaunchConfig, window: &mut Window, cx: &mut Context<Self>) {
        let ids = debug_config::input_ids(&config);
        if ids.is_empty() {
            return self.start_debug(config, cx);
        }
        let mut left = Vec::new();
        for id in ids {
            match config.inputs.iter().find(|i| i.id == id) {
                Some(InputDef { kind: InputKind::Refused(kind), .. }) => {
                    return self.say(
                        crate::i18n::tf("The input “{0}” is of type “{1}”, which would run a command; use promptString or pickString.", &[&id, kind]),
                        cx,
                    );
                }
                Some(def) => left.push(def.clone()),
                None => return self.say(crate::i18n::tf("Unknown input “{0}” in “{1}”", &[&id, &config.name]), cx),
            }
        }
        self.dbg.pending_launch = Some(PendingLaunch { config, answers: Vec::new(), left });
        self.ask_next_input(window, cx);
    }

    fn ask_next_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(next) = self.dbg.pending_launch.as_ref().map(|p| p.left.first().cloned()) else { return };
        let Some(def) = next else {
            let Some(p) = self.dbg.pending_launch.take() else { return };
            let mut config = p.config;
            debug_config::substitute_inputs(&mut config, &p.answers);
            return self.start_debug(config, cx);
        };
        match def.kind {
            InputKind::Prompt { description, default } => self.show_input(InputPurpose::LaunchInput, crate::i18n::t("OK"), &description, &default, window, cx),
            InputKind::Pick { description, options, default } => {
                if self.overlay.is_some() {
                    self.close_overlay(window, cx);
                }
                let mut options = options;
                if let Some(i) = default.and_then(|d| options.iter().position(|o| *o == d)) {
                    let d = options.remove(i);
                    options.insert(0, d);
                }
                let items = options.iter().map(|o| PickItem { label: o.clone().into(), detail: None, keys: None }).collect();
                let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], &description, "", None, cx));
                self.show_overlay(picker, OverlayKind::LaunchInput(options), window, cx);
            }
            InputKind::Refused(_) => {}
        }
    }

    /// The answer to the current question.
    pub(in crate::workbench) fn answer_launch_input(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.dbg.pending_launch.as_mut() else { return };
        if p.left.is_empty() {
            return;
        }
        let def = p.left.remove(0);
        p.answers.push((def.id, value));
        self.ask_next_input(window, cx);
    }

    /// "Debug: Run to Cursor": continue to the cursor's line (or start and stop there).
    pub(in crate::workbench) fn run_to_cursor(&mut self, _: &super::RunToCursor, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let (path, line) = {
            let e = ed.read(cx);
            (e.path.clone(), e.cursor_position().0 - 1)
        };
        let stopped = self.dbg.cur().is_some_and(|d| matches!(d.status, DebugStatus::Stopped(_)));
        let running = self.dbg.cur().is_some_and(|d| !matches!(d.status, DebugStatus::Ended(_)));
        if running && !stopped {
            return self.say(crate::i18n::t("The program is running: pause it first, or set a breakpoint.").into(), cx);
        }
        self.dbg.bps.set_temp(Some((path.clone(), line)));
        if stopped {
            self.push_breakpoints(&path);
            self.debug_control("continue", cx);
        } else {
            self.start_debugging(&super::super::StartDebugging, window, cx);
        }
    }

    /// The program stopped or ended after Run to Cursor: the temporary breakpoint goes from the adapter.
    pub(super) fn after_run_to_cursor(&mut self, path: String) {
        self.push_breakpoints(&path);
    }

    /// The session's `postDebugTask`, in a terminal like any task.
    pub(super) fn run_post_debug_task(&mut self, label: String, command: String, cx: &mut Context<Self>) {
        let task = super::super::tasks::Task { label, command, source: "launch" };
        cx.spawn(async move |this, cx| with_window(&this, cx, move |this, window, cx| this.run_task(task, window, cx))).detach();
    }

    /// "Debug: Stop All".
    pub(in crate::workbench) fn stop_all_debugging(&mut self, _: &super::StopAllDebugging, _: &mut Window, cx: &mut Context<Self>) {
        let live: Vec<u64> = self.dbg.sessions.iter().filter(|d| !matches!(d.status, DebugStatus::Ended(_))).map(|d| d.id).collect();
        for sid in live {
            self.end_debug(sid, crate::i18n::t("Debug session stopped").into(), cx);
        }
    }

    /// The adapter's `runInTerminal`: the program runs in a new terminal, in plain sight.
    pub(super) fn run_in_terminal(&mut self, sid: u64, req: crate::dap::TerminalRequest, cx: &mut Context<Self>) {
        if cfg!(windows) && !self.remote.host.is_remote() {
            if let Some(d) = self.dbg.by_id_mut(sid) {
                d.push_console(
                    "stderr",
                    crate::i18n::tf(
                        "Running the program in a terminal is not supported on Windows yet: use {} in the configuration.",
                        &[&"\"console\": \"internalConsole\""],
                    ),
                );
            }
            return;
        }
        let quote = super::super::tasks::shell_quote;
        let mut line = String::from("env");
        for (k, v) in &req.env {
            line.push(' ');
            match v {
                Some(v) => line.push_str(&quote(&format!("{k}={v}"))),
                None => line.push_str(&format!("-u {}", quote(k))),
            }
        }
        for a in &req.args {
            line.push(' ');
            line.push_str(&quote(a));
        }
        let title = req.title.clone().unwrap_or_else(|| self.dbg.by_id(sid).map(|d| d.config.name.clone()).unwrap_or_default());
        let cwd = (!req.cwd.is_empty()).then_some(req.cwd);
        cx.spawn(async move |this, cx| {
            with_window(&this, cx, move |this, window, cx| {
                this.new_terminal_in(cwd, window, cx);
                if let Some(t) = this.terminals.get(this.active_terminal).cloned() {
                    t.update(cx, |t, cx| {
                        t.set_fixed_title(title);
                        t.send(&format!("{line}\n"));
                        cx.notify();
                    });
                }
            })
        })
        .detach();
    }

    /// "Debug: Add Configuration…": pick a kind; it is added to the launch file (created if missing) and opened.
    pub(in crate::workbench) fn add_debug_configuration(&mut self, _: &super::AddDebugConfiguration, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let list = templates();
        let items =
            list.iter().map(|(label, v)| PickItem { label: (*label).into(), detail: v["type"].as_str().map(|t| t.to_string().into()), keys: None }).collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Add a debug configuration"), "", None, cx));
        self.show_overlay(picker, OverlayKind::LaunchTemplates(list.into_iter().map(|(_, v)| v).collect()), window, cx);
    }

    pub(in crate::workbench) fn insert_debug_configuration(&mut self, snippet: serde_json::Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else { return };
        let host = self.remote.host.clone();
        let task = cx.background_executor().spawn(async move {
            let path = [super::view::LAUNCH_FILE, ".vscode/launch.json"].into_iter().map(|r| ui::join(&root, r)).find(|p| host.stat(p).is_ok());
            let path = match path {
                Some(p) => p,
                None => {
                    let p = ui::join(&root, super::view::LAUNCH_FILE);
                    host.create_file(&p).map_err(|e| e.to_string())?;
                    p
                }
            };
            let text = host.read(&path).map(|(d, _)| String::from_utf8_lossy(&d).into_owned()).unwrap_or_default();
            let new = debug_config::insert_configuration(&text, &snippet)?;
            host.write(&path, new.into_bytes(), None).map_err(|e| e.to_string())?;
            Ok::<_, String>(path)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(path) => {
                    this.open_file(path, window, cx);
                    this.load_launch_catalog(cx);
                }
                Err(e) => this.say(crate::i18n::tf("Could not add the configuration: {}", &[&e]), cx),
            })
            .ok();
        })
        .detach();
    }
}
