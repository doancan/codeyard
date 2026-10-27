//! Run and Debug: what tests read and drive (compiled into test builds only).

use super::*;

impl Workbench {
    /// Summary of state for tests: (state, frame names, stopped line, console).
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

    pub fn reference_for_test(&self, name: &str) -> Option<i64> {
        self.dbg.cur()?.children.values().flatten().find(|v| v.name == name).map(|v| v.reference)
    }

    pub fn set_variable_for_test(&mut self, name: &str, value: &str, cx: &mut Context<Self>) {
        let parent = self.dbg.cur().and_then(|d| d.children.iter().find(|(_, vs)| vs.iter().any(|v| v.name == name)).map(|(r, _)| *r));
        if let Some(parent) = parent {
            self.finish_set_variable(parent, name.into(), value.into(), cx);
        }
    }

    pub fn set_console_input_for_test(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(input) = self.dbg.cur().map(|d| d.input.clone()) {
            input.update(cx, |f, cx| f.set_text(text, cx));
        }
    }

    pub fn console_input_for_test(&self, cx: &gpui::App) -> String {
        self.dbg.cur().map(|d| d.input.read(cx).text().to_string()).unwrap_or_default()
    }

    pub fn evaluate_for_test(&mut self, text: &str, cx: &mut Context<Self>) {
        self.debug_evaluate(text.into(), cx);
    }

    pub fn start_named_for_test(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.start_named(name.into(), false, window, cx);
    }

    pub fn answer_launch_input_for_test(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(window, cx);
        self.answer_launch_input(value.into(), window, cx);
    }

    pub fn debug_catalog_for_test(&self) -> Vec<String> {
        self.dbg.catalog.clone()
    }

    pub fn start_selected_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_selected(window, cx);
    }

    /// Open or close the first visible variable with this name.
    pub fn toggle_variable_for_test(&mut self, name: &str, cx: &mut Context<Self>) {
        let reference = self.dbg.cur().and_then(|d| d.children.values().flatten().find(|v| v.name == name).map(|v| v.reference));
        if let Some(r) = reference {
            self.toggle_variable(r, cx);
        }
    }

    pub fn set_breakpoint_enabled_for_test(&mut self, path: &str, line: usize, on: bool, cx: &mut Context<Self>) {
        self.set_breakpoint_enabled(path.to_string(), line, on, cx);
    }

    pub fn add_function_breakpoint_for_test(&mut self, name: &str, cx: &mut Context<Self>) {
        self.finish_function_breakpoint(name.to_string(), cx);
    }

    pub fn toggle_exception_filter_for_test(&mut self, id: &str, cx: &mut Context<Self>) {
        self.toggle_exception_filter(id.to_string(), cx);
    }

    pub fn set_exception_filters_for_test(&mut self, filters: Option<Vec<String>>) {
        self.dbg.exception_filters = filters;
    }

    pub fn add_data_breakpoint_for_test(&mut self, name: &str, cx: &mut Context<Self>) {
        self.finish_data_breakpoint(name.to_string(), cx);
    }

    pub fn data_breakpoints_for_test(&self) -> Vec<String> {
        self.dbg.cur().map(|d| d.data_breakpoints.iter().map(|(_, desc)| desc.clone()).collect()).unwrap_or_default()
    }

    pub fn add_watch_for_test(&mut self, expr: &str, cx: &mut Context<Self>) {
        self.finish_add_watch(expr.to_string(), cx);
    }

    pub fn debug_threads_for_test(&self) -> Vec<(i64, String)> {
        self.dbg.cur().map(|d| d.threads.clone()).unwrap_or_default()
    }

    pub fn select_thread_for_test(&mut self, id: i64, cx: &mut Context<Self>) {
        self.select_thread(id, cx);
    }

    pub fn debug_watch_values(&self) -> Vec<(String, String)> {
        let values = self.dbg.cur().map(|d| d.watch_values.clone()).unwrap_or_default();
        self.dbg.watch_exprs.iter().cloned().zip(values).collect()
    }

    /// For tests: visible variables (name, value), from the expanded scopes.
    pub fn debug_variables(&self) -> Vec<(String, String)> {
        let Some(d) = self.dbg.cur() else { return Vec::new() };
        d.scopes
            .iter()
            .filter(|s| d.expanded.contains(&s.reference))
            .flat_map(|s| d.children.get(&s.reference).into_iter().flatten().map(|v| (v.name.clone(), v.value.clone())))
            .collect()
    }
}
