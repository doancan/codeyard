//! The debugger's breakpoint model, without UI: line breakpoints per file (with condition, hit count, log message,
//! enabled and verified state), function breakpoints, the "breakpoints active" switch and the temporary breakpoint
//! "Run to Cursor" uses. Editors show the lines of open files and report edits; this store also keeps the files that
//! are closed, persists them, and decides what is sent to the debug adapter.

use std::collections::{BTreeMap, HashMap};

use crate::dap;

/// Whether the debug adapter accepted a breakpoint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Verified {
    /// Not sent yet, or no session.
    #[default]
    Unknown,
    Yes,
    /// Rejected or not bound to code yet, with the adapter's reason.
    No(Option<String>),
}

/// Extra settings of a breakpoint: conditional, logpoint, hit count; and its state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BpMeta {
    pub condition: Option<String>,
    /// If set, the breakpoint doesn't stop but writes this message to the console (`{expression}` is expanded).
    pub log: Option<String>,
    pub hit: Option<String>,
    /// Kept but not sent to the adapter.
    pub disabled: bool,
    pub verified: Verified,
}

impl BpMeta {
    /// Same settings (state aside).
    fn same_settings(&self, other: &BpMeta) -> bool {
        (&self.condition, &self.log, &self.hit, self.disabled) == (&other.condition, &other.log, &other.hit, other.disabled)
    }
}

/// A breakpoint on a function name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionBp {
    pub name: String,
    pub condition: Option<String>,
    pub disabled: bool,
    pub verified: Verified,
}

/// Result for one sent breakpoint, as the adapter reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BpResult {
    pub id: Option<i64>,
    pub verified: bool,
    pub message: Option<String>,
    /// Line the adapter moved it to (1-based).
    pub line: Option<u32>,
}

impl BpResult {
    fn verified(&self) -> Verified {
        if self.verified { Verified::Yes } else { Verified::No(self.message.clone()) }
    }
}

/// A data breakpoint: the program stops when a value is written. It belongs to its debug session (the adapter's id for
/// the value means nothing to another run), so it is never saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataBp {
    /// The adapter's `dataId`.
    pub id: String,
    /// What the adapter calls the value.
    pub desc: String,
    pub enabled: bool,
}

/// One row of the Breakpoints list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BpRow {
    /// A data breakpoint of the current session, by its place in the session's list.
    Data {
        index: usize,
        bp: DataBp,
    },
    Line {
        path: String,
        line: usize,
        meta: BpMeta,
    },
    Function {
        index: usize,
        bp: FunctionBp,
    },
}

#[derive(Debug)]
pub struct BreakpointStore {
    /// Absolute path → (0-based line, meta), sorted by line, one per line.
    files: BTreeMap<String, Vec<(usize, BpMeta)>>,
    pub functions: Vec<FunctionBp>,
    /// `false`: "Deactivate Breakpoints": nothing is sent, the list stays.
    pub active: bool,
    /// "Run to Cursor": sent like a breakpoint until the next stop; never listed or saved.
    temp: Option<(String, usize)>,
    /// Adapter breakpoint id → (path, line), for `breakpoint` change events.
    ids: HashMap<i64, (String, usize)>,
}

impl Default for BreakpointStore {
    fn default() -> Self {
        BreakpointStore { files: BTreeMap::new(), functions: Vec::new(), active: true, temp: None, ids: HashMap::new() }
    }
}

impl BreakpointStore {
    pub fn file(&self, path: &str) -> &[(usize, BpMeta)] {
        self.files.get(path).map_or(&[], Vec::as_slice)
    }

    pub fn paths(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    pub fn count(&self) -> usize {
        self.files.values().map(Vec::len).sum::<usize>() + self.functions.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Replace a file's breakpoints with an editor's (lines move with edits). Returns whether the settings changed
    /// (moved lines, added, removed or edited), which is when the adapter and the saved list need the new ones.
    pub fn set_file(&mut self, path: &str, list: Vec<(usize, BpMeta)>) -> bool {
        let mut list = list;
        list.sort_by_key(|(l, _)| *l);
        list.dedup_by_key(|(l, _)| *l);
        let old = self.file(path);
        let changed = old.len() != list.len() || old.iter().zip(&list).any(|((la, a), (lb, b))| la != lb || !a.same_settings(b));
        if list.is_empty() {
            self.files.remove(path);
        } else {
            self.files.insert(path.to_string(), list);
        }
        changed
    }

    fn entry(&mut self, path: &str, line: usize) -> Option<&mut BpMeta> {
        self.files.get_mut(path)?.iter_mut().find(|(l, _)| *l == line).map(|(_, m)| m)
    }

    #[cfg(test)]
    pub fn toggle_line(&mut self, path: &str, line: usize) {
        let mut list = self.file(path).to_vec();
        match list.iter().position(|(l, _)| *l == line) {
            Some(i) => {
                list.remove(i);
            }
            None => list.push((line, BpMeta::default())),
        }
        self.set_file(path, list);
    }

    /// Settings of the breakpoint on a line (created if missing); state is kept.
    pub fn set_meta(&mut self, path: &str, line: usize, meta: BpMeta) {
        match self.entry(path, line) {
            Some(m) => {
                let verified = std::mem::take(&mut m.verified);
                *m = BpMeta { verified, ..meta };
            }
            None => {
                let mut list = self.file(path).to_vec();
                list.push((line, meta));
                self.set_file(path, list);
            }
        }
    }

    pub fn set_enabled(&mut self, path: &str, line: usize, on: bool) {
        if let Some(m) = self.entry(path, line) {
            m.disabled = !on;
        }
    }

    /// Enable or disable every breakpoint (line and function).
    pub fn set_all_enabled(&mut self, on: bool) {
        self.files.values_mut().flatten().for_each(|(_, m)| m.disabled = !on);
        self.functions.iter_mut().for_each(|f| f.disabled = !on);
    }

    pub fn remove(&mut self, path: &str, line: usize) {
        let list: Vec<_> = self.file(path).iter().filter(|(l, _)| *l != line).cloned().collect();
        self.set_file(path, list);
    }

    /// Removes every breakpoint; returns the files that had some (their editors and the adapter need the empty list).
    pub fn remove_all(&mut self) -> Vec<String> {
        self.functions.clear();
        self.ids.clear();
        std::mem::take(&mut self.files).into_keys().collect()
    }

    pub fn add_function(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || self.functions.iter().any(|f| f.name == name) {
            return false;
        }
        self.functions.push(FunctionBp { name: name.to_string(), ..Default::default() });
        true
    }

    /// The temporary "Run to Cursor" breakpoint (`None` clears it). Returns the file whose sent list changes.
    pub fn set_temp(&mut self, at: Option<(String, usize)>) -> Option<String> {
        let old = std::mem::replace(&mut self.temp, at);
        self.temp.as_ref().or(old.as_ref()).map(|(p, _)| p.clone())
    }

    /// Breakpoints to send for a file: the enabled ones (none while deactivated) plus the temporary one.
    pub fn wire(&self, path: &str) -> Vec<dap::Breakpoint> {
        let mut out: Vec<dap::Breakpoint> = if self.active {
            self.file(path)
                .iter()
                .filter(|(_, m)| !m.disabled)
                .map(|(l, m)| dap::Breakpoint { line: *l as u32 + 1, condition: m.condition.clone(), log_message: m.log.clone(), hit_condition: m.hit.clone() })
                .collect()
        } else {
            Vec::new()
        };
        if let Some((p, l)) = &self.temp
            && p == path
            && !out.iter().any(|b| b.line == *l as u32 + 1)
        {
            out.push(dap::Breakpoint { line: *l as u32 + 1, ..Default::default() });
        }
        out
    }

    /// Everything to send at session start (files with nothing to send are left out).
    pub fn wire_all(&self) -> BTreeMap<String, Vec<dap::Breakpoint>> {
        let mut paths: Vec<&String> = self.files.keys().collect();
        if let Some((p, _)) = &self.temp {
            paths.push(p);
        }
        paths.into_iter().map(|p| (p.clone(), self.wire(p))).filter(|(_, w)| !w.is_empty()).collect()
    }

    /// Function breakpoints to send.
    pub fn wire_functions(&self) -> Vec<dap::FunctionBreakpoint> {
        if !self.active {
            return Vec::new();
        }
        self.functions.iter().filter(|f| !f.disabled).map(|f| dap::FunctionBreakpoint { name: f.name.clone(), condition: f.condition.clone() }).collect()
    }

    /// The adapter answered `setBreakpoints` for `path`: `sent` are the 1-based lines in the order sent, `results`
    /// the answers in the same order.
    pub fn on_results(&mut self, path: &str, sent: &[u32], results: &[BpResult]) {
        for (line, r) in sent.iter().zip(results) {
            let line = (*line as usize).saturating_sub(1);
            if let Some(m) = self.entry(path, line) {
                m.verified = r.verified();
            }
            if let Some(id) = r.id {
                self.ids.insert(id, (path.to_string(), line));
            }
        }
    }

    /// A `breakpoint` event changed a breakpoint the adapter knows by id.
    pub fn on_changed(&mut self, r: &BpResult) -> Option<String> {
        let (path, line) = self.ids.get(&r.id?)?.clone();
        self.entry(&path, line)?.verified = r.verified();
        Some(path)
    }

    pub fn on_function_results(&mut self, results: &[BpResult]) {
        for (f, r) in self.functions.iter_mut().filter(|f| !f.disabled).zip(results) {
            f.verified = r.verified();
        }
    }

    /// No session any more: every state goes back to unknown.
    pub fn clear_verified(&mut self) {
        self.files.values_mut().flatten().for_each(|(_, m)| m.verified = Verified::Unknown);
        self.functions.iter_mut().for_each(|f| f.verified = Verified::Unknown);
        self.ids.clear();
    }

    /// Rows of the Breakpoints list: functions first, then files by path and line.
    pub fn rows(&self) -> Vec<BpRow> {
        let mut out: Vec<BpRow> = self.functions.iter().cloned().enumerate().map(|(index, bp)| BpRow::Function { index, bp }).collect();
        for (path, list) in &self.files {
            out.extend(list.iter().map(|(line, meta)| BpRow::Line { path: path.clone(), line: *line, meta: meta.clone() }));
        }
        out
    }
}

/// "condition" · "log: message" · "hit: count" → meta (empty: plain breakpoint).
pub fn parse_breakpoint_meta(text: &str) -> BpMeta {
    let t = text.trim();
    let mut m = BpMeta::default();
    if let Some(rest) = t.strip_prefix("log:") {
        m.log = Some(rest.trim().to_string()).filter(|s| !s.is_empty());
    } else if let Some(rest) = t.strip_prefix("hit:") {
        m.hit = Some(rest.trim().to_string()).filter(|s| !s.is_empty());
    } else if !t.is_empty() {
        m.condition = Some(t.to_string());
    }
    m
}

/// The text `parse_breakpoint_meta` reads back.
pub fn breakpoint_meta_text(m: &BpMeta) -> String {
    match (&m.log, &m.hit, &m.condition) {
        (Some(l), ..) => format!("log: {l}"),
        (None, Some(h), _) => format!("hit: {h}"),
        (None, None, Some(c)) => c.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(w: &[dap::Breakpoint]) -> Vec<u32> {
        w.iter().map(|b| b.line).collect()
    }

    #[test]
    fn only_enabled_breakpoints_are_sent_and_deactivating_sends_none() {
        let mut s = BreakpointStore::default();
        s.toggle_line("/a.rs", 4);
        s.toggle_line("/a.rs", 9);
        s.set_meta("/a.rs", 9, BpMeta { condition: Some("i > 2".into()), ..Default::default() });
        assert_eq!(lines(&s.wire("/a.rs")), vec![5, 10]);
        assert_eq!(s.wire("/a.rs")[1].condition.as_deref(), Some("i > 2"));
        s.set_enabled("/a.rs", 4, false);
        assert_eq!(lines(&s.wire("/a.rs")), vec![10]);
        s.active = false;
        assert!(s.wire("/a.rs").is_empty() && s.wire_all().is_empty());
        s.active = true;
        s.set_all_enabled(true);
        assert_eq!(lines(&s.wire("/a.rs")), vec![5, 10]);
        assert_eq!(s.count(), 2);
    }

    #[test]
    fn an_editor_snapshot_reports_real_changes_only() {
        let mut s = BreakpointStore::default();
        assert!(s.set_file("/a.rs", vec![(3, BpMeta::default())]));
        let verified = BpMeta { verified: Verified::Yes, ..Default::default() };
        assert!(!s.set_file("/a.rs", vec![(3, verified.clone())]), "state alone is not a change");
        assert!(s.set_file("/a.rs", vec![(4, verified)]), "a moved line is");
        assert!(s.set_file("/a.rs", Vec::new()));
        assert!(s.paths().is_empty(), "an empty file is dropped");
    }

    #[test]
    fn results_mark_the_lines_that_were_sent_even_if_the_list_changed_since() {
        let mut s = BreakpointStore::default();
        s.toggle_line("/a.rs", 1);
        s.toggle_line("/a.rs", 5);
        let sent = lines(&s.wire("/a.rs"));
        s.toggle_line("/a.rs", 1); // removed while the request was on its way
        let r = |id, ok| BpResult { id: Some(id), verified: ok, message: (!ok).then(|| "no code".into()), line: None };
        s.on_results("/a.rs", &sent, &[r(1, true), r(2, false)]);
        assert_eq!(s.file("/a.rs"), &[(5, BpMeta { verified: Verified::No(Some("no code".into())), ..Default::default() })]);
        assert_eq!(s.on_changed(&r(2, true)), Some("/a.rs".into()));
        assert_eq!(s.file("/a.rs")[0].1.verified, Verified::Yes);
        assert_eq!(s.on_changed(&r(99, true)), None, "unknown ids are ignored");
        s.clear_verified();
        assert_eq!(s.file("/a.rs")[0].1.verified, Verified::Unknown);
    }

    #[test]
    fn the_run_to_cursor_breakpoint_is_sent_but_never_listed() {
        let mut s = BreakpointStore::default();
        s.toggle_line("/a.rs", 2);
        assert_eq!(s.set_temp(Some(("/b.rs".into(), 7))), Some("/b.rs".into()));
        assert_eq!(lines(&s.wire("/b.rs")), vec![8]);
        assert!(s.wire_all().contains_key("/b.rs"));
        assert_eq!(s.rows().len(), 1, "not in the list");
        s.active = false;
        assert_eq!(lines(&s.wire("/b.rs")), vec![8], "still sent while breakpoints are deactivated");
        assert_eq!(s.set_temp(None), Some("/b.rs".into()));
        assert!(s.wire("/b.rs").is_empty());
    }

    #[test]
    fn function_breakpoints_are_unique_and_follow_the_switches() {
        let mut s = BreakpointStore::default();
        assert!(s.add_function(" main "));
        assert!(!s.add_function("main") && !s.add_function("  "));
        s.add_function("helper");
        s.functions[0].disabled = true;
        assert_eq!(s.wire_functions().iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["helper"]);
        s.on_function_results(&[BpResult { verified: true, ..Default::default() }]);
        assert_eq!(s.functions[1].verified, Verified::Yes, "results map to the enabled ones in order");
        assert!(matches!(s.rows()[0], BpRow::Function { index: 0, .. }));
        assert_eq!(s.remove_all(), Vec::<String>::new());
        assert!(s.is_empty());
    }

    #[test]
    fn breakpoint_meta_syntax_round_trips() {
        assert_eq!(parse_breakpoint_meta("  "), BpMeta::default());
        assert_eq!(parse_breakpoint_meta("i == 3"), BpMeta { condition: Some("i == 3".into()), ..Default::default() });
        assert_eq!(parse_breakpoint_meta("log: i is {i}"), BpMeta { log: Some("i is {i}".into()), ..Default::default() });
        assert_eq!(parse_breakpoint_meta("hit: >= 5"), BpMeta { hit: Some(">= 5".into()), ..Default::default() });
        assert_eq!(parse_breakpoint_meta("log:"), BpMeta::default(), "empty message is a plain breakpoint");
        for t in ["i == 3", "log: x={x}", "hit: 5", ""] {
            assert_eq!(breakpoint_meta_text(&parse_breakpoint_meta(t)), t);
        }
    }
}
