//! What the Problems panel shows: severity switches, a text filter, "active file only", the sort order and the files
//! folded shut. Pure data and text: the panel (`workbench_problems.rs`) draws it and keeps it per workspace.

use std::collections::BTreeSet;

use crate::lsp::Diagnostic;

/// Longest filter text kept, and most folded files remembered, so a damaged record cannot grow the panel's state.
const MAX_TEXT: usize = 200;
const MAX_COLLAPSED: usize = 500;

/// The order of the files and problems.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    /// Files in path order, each file's problems by severity, then position.
    #[default]
    File,
    /// The files with the most severe problems first, each file's problems by severity, then position.
    Severity,
    /// Files in path order, each file's problems strictly by position.
    Line,
}

impl Sort {
    pub fn next(self) -> Self {
        match self {
            Sort::File => Sort::Severity,
            Sort::Severity => Sort::Line,
            Sort::Line => Sort::File,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Sort::File => "file",
            Sort::Severity => "severity",
            Sort::Line => "line",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "severity" => Sort::Severity,
            "line" => Sort::Line,
            _ => Sort::File,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProblemFilter {
    pub errors: bool,
    pub warnings: bool,
    /// Information and hints.
    pub info: bool,
    pub text: String,
    pub active_file_only: bool,
    pub sort: Sort,
    /// Files (absolute paths) shown as one line.
    pub collapsed: BTreeSet<String>,
}

impl Default for ProblemFilter {
    fn default() -> Self {
        Self { errors: true, warnings: true, info: true, text: String::new(), active_file_only: false, sort: Sort::File, collapsed: BTreeSet::new() }
    }
}

/// Problems per severity, as the header counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub errors: usize,
    pub warnings: usize,
    pub info: usize,
}

impl Counts {
    pub fn add(&mut self, severity: u8) {
        match severity {
            1 => self.errors += 1,
            2 => self.warnings += 1,
            _ => self.info += 1,
        }
    }

    pub fn total(self) -> usize {
        self.errors + self.warnings + self.info
    }
}

fn folded(text: &str) -> String {
    text.chars().map(crate::fuzzy::fold).collect()
}

impl ProblemFilter {
    /// Whether the severity filter, the text filter or the active-file switch hides anything (the sort and the folded
    /// files do not hide problems).
    pub fn narrows(&self) -> bool {
        !(self.errors && self.warnings && self.info) || !self.text.trim().is_empty() || self.active_file_only
    }

    pub fn shows_severity(&self, severity: u8) -> bool {
        match severity {
            1 => self.errors,
            2 => self.warnings,
            _ => self.info,
        }
    }

    /// Whether the problem passes the severity switches and the text filter: every word of the text must be found in
    /// the path, the message, the code or the source, ignoring case (and Turkish i/ı and dotted letters).
    pub fn admits(&self, rel_path: &str, d: &Diagnostic) -> bool {
        if !self.shows_severity(d.severity) {
            return false;
        }
        let words: Vec<String> = self.text.split_whitespace().map(folded).collect();
        if words.is_empty() {
            return true;
        }
        let haystack = folded(&format!("{rel_path} {} {} {}", d.message, d.code.as_deref().unwrap_or_default(), d.source.as_deref().unwrap_or_default()));
        words.iter().all(|w| haystack.contains(w.as_str()))
    }

    /// The saved form (one line of JSON).
    pub fn encode(&self) -> String {
        serde_json::json!({
            "errors": self.errors,
            "warnings": self.warnings,
            "info": self.info,
            "text": self.text,
            "activeFileOnly": self.active_file_only,
            "sort": self.sort.key(),
            "collapsed": self.collapsed,
        })
        .to_string()
    }

    /// Reads a saved form; anything unreadable gives the defaults, a field of the wrong type its default.
    pub fn decode(saved: &str) -> Self {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(saved) else { return Self::default() };
        let flag = |name: &str| v[name].as_bool().unwrap_or(true);
        Self {
            errors: flag("errors"),
            warnings: flag("warnings"),
            info: flag("info"),
            text: v["text"].as_str().unwrap_or_default().chars().take(MAX_TEXT).collect(),
            active_file_only: v["activeFileOnly"].as_bool().unwrap_or(false),
            sort: Sort::from_key(v["sort"].as_str().unwrap_or_default()),
            collapsed: v["collapsed"].as_array().into_iter().flatten().filter_map(|p| p.as_str()).take(MAX_COLLAPSED).map(str::to_string).collect(),
        }
    }
}

/// One problem as a line of text for the clipboard: `path:line:col: error [source code] message`.
pub fn copy_line(rel_path: &str, severity: u8, source: &str, code: &str, message: &str, line: u32, col: u32) -> String {
    let word = match severity {
        1 => "error",
        2 => "warning",
        _ => "info",
    };
    let origin = [source, code].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
    let origin = if origin.is_empty() { String::new() } else { format!(" [{origin}]") };
    format!("{rel_path}:{}:{}: {word}{origin} {message}", line.saturating_add(1), col.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(severity: u8, message: &str) -> Diagnostic {
        Diagnostic { severity, message: message.into(), source: Some("rustc".into()), code: Some("E0382".into()), ..Default::default() }
    }

    #[test]
    fn severity_switches_hide_their_problems() {
        let mut f = ProblemFilter::default();
        assert!(f.admits("a.rs", &diag(1, "x")) && f.admits("a.rs", &diag(2, "x")) && f.admits("a.rs", &diag(3, "x")) && f.admits("a.rs", &diag(4, "x")));
        f.warnings = false;
        f.info = false;
        assert!(f.admits("a.rs", &diag(1, "x")));
        assert!(!f.admits("a.rs", &diag(2, "x")));
        assert!(!f.admits("a.rs", &diag(3, "x")) && !f.admits("a.rs", &diag(4, "x")), "hints count as information");
        assert!(f.narrows());
    }

    #[test]
    fn text_filter_needs_every_word_in_path_message_code_or_source() {
        let f = ProblemFilter { text: "moved e0382".into(), ..Default::default() };
        assert!(f.admits("src/a.rs", &diag(1, "borrow of moved value")));
        assert!(!f.admits("src/a.rs", &diag(1, "unused variable")));
        for (text, hit) in [("RUSTC", true), ("src/a", true), ("zzz", false), ("  ", true)] {
            let f = ProblemFilter { text: text.into(), ..Default::default() };
            assert_eq!(f.admits("src/a.rs", &diag(1, "x")), hit, "{text:?}");
        }
        // Turkish: "I" and "ı" and "İ" are the same letter for the filter.
        let tr = ProblemFilter { text: "ISIK".into(), ..Default::default() };
        assert!(tr.admits("a.rs", &diag(1, "ışık yok")));
    }

    #[test]
    fn narrowing_ignores_sort_and_folded_files() {
        let mut f = ProblemFilter::default();
        assert!(!f.narrows());
        f.sort = Sort::Line;
        f.collapsed.insert("/p/a.rs".into());
        assert!(!f.narrows());
        f.active_file_only = true;
        assert!(f.narrows());
    }

    #[test]
    fn the_saved_form_round_trips_and_damage_gives_defaults() {
        let f = ProblemFilter {
            errors: true,
            warnings: false,
            info: true,
            text: "borrow".into(),
            active_file_only: true,
            sort: Sort::Severity,
            collapsed: BTreeSet::from(["/p/a.rs".to_string()]),
        };
        assert_eq!(ProblemFilter::decode(&f.encode()), f);
        for bad in ["", "not json", "[]", "{\"errors\": 5, \"sort\": 9, \"collapsed\": \"x\"}"] {
            assert_eq!(ProblemFilter::decode(bad), ProblemFilter::default(), "{bad:?}");
        }
        let long = serde_json::json!({ "text": "x".repeat(10_000), "collapsed": (0..5000).map(|i| format!("/p/{i}")).collect::<Vec<_>>() }).to_string();
        let g = ProblemFilter::decode(&long);
        assert_eq!((g.text.chars().count(), g.collapsed.len()), (MAX_TEXT, MAX_COLLAPSED));
    }

    #[test]
    fn sort_cycles_through_all_orders() {
        assert_eq!(Sort::File.next().next().next(), Sort::File);
        assert_eq!([Sort::File, Sort::Severity, Sort::Line].map(|s| Sort::from_key(s.key())), [Sort::File, Sort::Severity, Sort::Line]);
    }

    #[test]
    fn copied_lines_name_the_place_severity_origin_and_message() {
        assert_eq!(copy_line("src/a.rs", 1, "rustc", "E0382", "moved", 3, 4), "src/a.rs:4:5: error [rustc E0382] moved");
        assert_eq!(copy_line("a.c", 2, "", "", "unused", 0, 0), "a.c:1:1: warning unused");
        assert_eq!(copy_line("a.c", 4, "task", "", "hint", 9, 1), "a.c:10:2: info [task] hint");
    }
}
