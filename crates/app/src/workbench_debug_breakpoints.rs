//! Run and Debug: breakpoints. Line breakpoints follow the editors and are saved per project; function, exception and
//! data breakpoints, the logpoint and condition editor, and sending all of them to every running session.

use super::*;

impl Workbench {
    /// Take an editor's breakpoints into the store (their lines move with edits); saved when they changed.
    pub(in crate::workbench) fn sync_breakpoints_from(&mut self, ed: &Entity<crate::editor::EditorView>, cx: &mut Context<Self>) -> bool {
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
    pub(in crate::workbench) fn on_breakpoints_changed(&mut self, ed: &Entity<crate::editor::EditorView>, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        self.sync_breakpoints_from(ed, cx);
        self.push_breakpoints(&path);
        cx.notify();
    }

    /// Report a file's breakpoints to every running session.
    pub(super) fn push_breakpoints(&self, path: &str) {
        // Main thread, no waiting for the reply: a later `continue` can't overtake this request.
        let wire = self.dbg.bps.wire(path);
        for session in self.dbg.live() {
            session.set_breakpoints_async(path, &wire);
        }
    }

    /// The store changed a file's breakpoints (list actions, adapter results): show them in its open editors, send them
    /// and save them.
    pub(in crate::workbench) fn breakpoints_changed_in_store(&mut self, path: &str, send: bool, cx: &mut Context<Self>) {
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
    pub(in crate::workbench) fn apply_breakpoints_to(&mut self, ed: &Entity<crate::editor::EditorView>, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        let list = self.dbg.bps.file(&path).to_vec();
        if !list.is_empty() {
            ed.update(cx, |e, cx| e.set_breakpoints(&list, cx));
        }
        self.apply_debug_line_all(cx);
    }

    /// Writes one file's breakpoints to the project's saved state (files outside the folders are not kept).
    pub(super) fn save_breakpoints(&self, path: &str, cx: &gpui::App) {
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
    pub(super) fn save_breakpoint_settings(&self) {
        let (Some(s), Some(ws)) = (&self.persist.state, self.persist.workspace_id) else { return };
        let names: Vec<&str> = self.dbg.bps.functions.iter().map(|f| f.name.as_str()).collect();
        let _ = s.set(&format!("dbg.functions:{ws}"), &names.join("\n"));
        let _ = s.set(&format!("dbg.inactive:{ws}"), if self.dbg.bps.active { "" } else { "1" });
    }

    /// A folder was opened: its watches, exception filters and breakpoints come back.
    pub(in crate::workbench) fn load_debug_state(&mut self, cx: &mut Context<Self>) {
        self.load_watches();
        self.load_console_history();
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
    pub(in crate::workbench) fn toggle_breakpoint_enabled(&mut self, _: &ToggleBreakpointEnabled, _: &mut Window, cx: &mut Context<Self>) {
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

    pub(in crate::workbench) fn set_breakpoint_enabled(&mut self, path: String, line: usize, on: bool, cx: &mut Context<Self>) {
        self.dbg.bps.set_enabled(&path, line, on);
        self.breakpoints_changed_in_store(&path, true, cx);
    }

    pub(super) fn set_all_breakpoints_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        for ed in self.all_editors() {
            self.sync_breakpoints_from(&ed, cx);
        }
        self.dbg.bps.set_all_enabled(on);
        for path in self.dbg.bps.paths() {
            self.breakpoints_changed_in_store(&path, true, cx);
        }
        self.push_function_breakpoints();
    }

    pub(in crate::workbench) fn enable_all_breakpoints(&mut self, _: &EnableAllBreakpoints, _: &mut Window, cx: &mut Context<Self>) {
        self.set_all_breakpoints_enabled(true, cx);
    }

    pub(in crate::workbench) fn disable_all_breakpoints(&mut self, _: &DisableAllBreakpoints, _: &mut Window, cx: &mut Context<Self>) {
        self.set_all_breakpoints_enabled(false, cx);
    }

    /// "Debug: Deactivate/Activate Breakpoints": the list stays, but nothing is sent while deactivated.
    pub(in crate::workbench) fn toggle_breakpoints_active(&mut self, _: &ToggleBreakpointsActive, _: &mut Window, cx: &mut Context<Self>) {
        self.dbg.bps.active = !self.dbg.bps.active;
        for path in self.dbg.bps.paths() {
            self.push_breakpoints(&path);
        }
        self.push_function_breakpoints();
        self.push_data_breakpoints();
        self.save_breakpoint_settings();
        self.notice = Some(Notice::Info(if self.dbg.bps.active {
            crate::i18n::t("Breakpoints are active.").into()
        } else {
            crate::i18n::t("Breakpoints are deactivated: the program runs past them.").into()
        }));
        cx.notify();
    }

    pub(in crate::workbench) fn add_function_breakpoint(&mut self, _: &AddFunctionBreakpoint, window: &mut Window, cx: &mut Context<Self>) {
        self.show_input(
            super::super::InputPurpose::FunctionBreakpoint,
            crate::i18n::t("Add"),
            crate::i18n::t("function name, e.g. main or app::run"),
            "",
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn finish_function_breakpoint(&mut self, name: String, cx: &mut Context<Self>) {
        if self.dbg.bps.add_function(&name) {
            self.push_function_breakpoints();
            self.save_breakpoint_settings();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn remove_function_breakpoint(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.dbg.bps.functions.len() {
            self.dbg.bps.functions.remove(index);
            self.push_function_breakpoints();
            self.save_breakpoint_settings();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn set_function_breakpoint_enabled(&mut self, index: usize, on: bool, cx: &mut Context<Self>) {
        if let Some(f) = self.dbg.bps.functions.get_mut(index) {
            f.disabled = !on;
            self.push_function_breakpoints();
            cx.notify();
        }
    }

    pub(super) fn push_function_breakpoints(&self) {
        let wire = self.dbg.bps.wire_functions();
        for session in self.dbg.live() {
            session.set_function_breakpoints_async(&wire);
        }
    }

    /// "Debug: Edit Breakpoint…": condition, `log: message` or `hit: count` for the cursor's line.
    pub(in crate::workbench) fn edit_breakpoint(&mut self, _: &super::super::EditBreakpoint, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let (path, line) = {
            let e = ed.read(cx);
            (e.path.clone(), e.cursor_position().0 - 1)
        };
        self.start_edit_breakpoint(path, line, window, cx);
    }

    pub(in crate::workbench) fn start_edit_breakpoint(&mut self, path: String, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.dbg.bps.file(&path).iter().find(|(l, _)| *l == line).map(|(_, m)| m.clone()).unwrap_or_default();
        let initial = crate::debug_model::breakpoint_meta_text(&current);
        self.show_input(
            super::super::InputPurpose::EditBreakpoint { path, line },
            crate::i18n::t("Set"),
            crate::i18n::t("condition  ·  log: message with {expr}  ·  hit: 5  ·  empty: plain breakpoint"),
            &initial,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn finish_edit_breakpoint(&mut self, path: String, line: usize, text: String, cx: &mut Context<Self>) {
        let meta = crate::debug_model::parse_breakpoint_meta(&text);
        self.dbg.bps.set_meta(&path, line, meta);
        self.breakpoints_changed_in_store(&path, true, cx);
    }

    /// "Debug: Install JavaScript Debugger": downloads Microsoft's js-debug adapter (MIT) into the settings folder.
    pub(in crate::workbench) fn install_js_debug(&mut self, _: &super::super::InstallJsDebug, _: &mut Window, cx: &mut Context<Self>) {
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

    /// "Debug: Break on Exceptions…": filters offered by the adapter (while a session is open), toggled on selection.
    pub(in crate::workbench) fn break_on_exceptions(&mut self, _: &super::super::BreakOnExceptions, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn selected_exception_filters(&self, available: &[(String, String, bool)]) -> Vec<String> {
        match &self.dbg.exception_filters {
            Some(p) => p.clone(),
            None => available.iter().filter(|(.., d)| *d).map(|(id, ..)| id.clone()).collect(),
        }
    }

    pub(in crate::workbench) fn toggle_exception_filter(&mut self, id: String, cx: &mut Context<Self>) {
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

    pub(in crate::workbench) fn load_exception_filters(&mut self) {
        self.dbg.exception_filters = match (self.persist.workspace_id, &self.persist.state) {
            (Some(ws), Some(s)) if s.get(&format!("exfilters-set:{ws}")).is_some() => {
                Some(s.get(&format!("exfilters:{ws}")).map(|t| t.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default())
            }
            _ => None,
        };
    }

    /// "Debug: Break When Value Changes…": variable name; the program stops when the value is written (hardware watchpoint).
    pub(in crate::workbench) fn add_data_breakpoint(&mut self, _: &super::super::AddDataBreakpoint, window: &mut Window, cx: &mut Context<Self>) {
        let ready = self.dbg.cur().is_some_and(|d| matches!(d.status, DebugStatus::Stopped(_)));
        if !ready {
            self.notice = Some(Notice::Info(crate::i18n::t("Pause the program first (a breakpoint or ⏸): the variable has to be in scope.").into()));
            cx.notify();
            return;
        }
        self.show_input(
            super::super::InputPurpose::DataBreakpoint,
            crate::i18n::t("Watch"),
            crate::i18n::t("variable name, e.g. total (the program stops when it is written)"),
            "",
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn finish_data_breakpoint(&mut self, name: String, cx: &mut Context<Self>) {
        self.add_data_breakpoint_in(None, name, cx);
    }

    /// A data breakpoint on the variable `name`: in the scope or object `parent` (a variable's own menu), or else in
    /// the loaded scope that has it (the first scope otherwise). Only while the program is paused, and only when the
    /// adapter says it supports them.
    pub(in crate::workbench) fn add_data_breakpoint_in(&mut self, parent: Option<i64>, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        let sid = self.dbg.cur_id();
        let active = self.dbg.bps.active;
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
        let reference = parent
            .or_else(|| {
                d.scopes
                    .iter()
                    .map(|s| s.reference)
                    .chain(d.children.keys().copied())
                    .find(|r| d.children.get(r).is_some_and(|vars| vars.iter().any(|v| v.name == name)))
            })
            .or_else(|| d.scopes.first().map(|s| s.reference))
            .unwrap_or(0);
        let existing: Vec<String> = if active { d.data_breakpoints.iter().filter(|b| b.enabled).map(|b| b.id.clone()).collect() } else { Vec::new() };
        let task = cx.background_executor().spawn(async move {
            let (id, description) = session.data_breakpoint_info(reference, &name)?;
            let mut ids = existing;
            if !ids.contains(&id) {
                ids.push(id.clone());
            }
            if active {
                session.set_data_breakpoints(&ids)?;
            }
            Ok::<_, String>((id, description))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((id, desc)) => {
                        if let Some(d) = this.dbg.by_id_mut(sid) {
                            match d.data_breakpoints.iter_mut().find(|b| b.id == id) {
                                Some(b) => b.enabled = true,
                                None => d.data_breakpoints.push(crate::debug_model::DataBp { id, desc: desc.clone(), enabled: true }),
                            }
                            d.push_console("console", crate::i18n::tf("Data breakpoint set: the program stops when {} is written.", &[&desc]));
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

    /// Sends the current session's enabled data breakpoints (none while breakpoints are deactivated).
    pub(in crate::workbench) fn push_data_breakpoints(&mut self) {
        let active = self.dbg.bps.active;
        let Some(d) = self.dbg.cur() else { return };
        let Some(session) = d.session.clone() else { return };
        let ids: Vec<String> = if active { d.data_breakpoints.iter().filter(|b| b.enabled).map(|b| b.id.clone()).collect() } else { Vec::new() };
        session.set_data_breakpoints_async(&ids);
    }

    /// Turns the data breakpoint at `index` of the current session on or off.
    pub(in crate::workbench) fn set_data_breakpoint_enabled(&mut self, index: usize, enabled: bool, cx: &mut Context<Self>) {
        let Some(bp) = self.dbg.cur_mut().and_then(|d| d.data_breakpoints.get_mut(index)) else { return };
        bp.enabled = enabled;
        self.push_data_breakpoints();
        cx.notify();
    }

    /// Removes the data breakpoint at `index` of the current session.
    pub(in crate::workbench) fn remove_data_breakpoint(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(d) = self.dbg.cur_mut().filter(|d| index < d.data_breakpoints.len()) else { return };
        d.data_breakpoints.remove(index);
        self.push_data_breakpoints();
        cx.notify();
    }

    /// The Breakpoints list: the current session's data breakpoints first, then the function and line breakpoints.
    pub(in crate::workbench) fn bp_rows(&self) -> Vec<crate::debug_model::BpRow> {
        let mut rows: Vec<crate::debug_model::BpRow> = self
            .dbg
            .cur()
            .map(|d| d.data_breakpoints.iter().cloned().enumerate().map(|(index, bp)| crate::debug_model::BpRow::Data { index, bp }).collect())
            .unwrap_or_default();
        rows.extend(self.dbg.bps.rows());
        rows
    }

    pub(in crate::workbench) fn remove_data_breakpoints(&mut self, _: &super::super::RemoveDataBreakpoints, _: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.dbg.cur_mut() else { return };
        if d.data_breakpoints.is_empty() {
            return;
        }
        d.data_breakpoints.clear();
        self.push_data_breakpoints();
        cx.notify();
    }
}
