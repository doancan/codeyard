//! Consecutive-line matching and watch readiness. Captures never execute or expand text.

use std::collections::{BTreeMap, BTreeSet};

use super::*;

type Fields = BTreeMap<&'static str, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub file: String,
    /// Compiler positions remain one-based until the editor converts them to document coordinates.
    pub line: u32,
    pub column: Option<u32>,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
    pub severity: u8,
    pub message: String,
    pub code: Option<String>,
}
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Update {
    pub problem: Option<Problem>,
    pub began: bool,
    pub ready: bool,
}

pub(super) struct State {
    next: usize,
    fields: Fields,
    active: BTreeSet<Option<String>>,
    ready: bool,
}
impl State {
    pub(super) fn new(watch: Option<&Watch>) -> Self {
        let mut active = BTreeSet::new();
        if watch.is_some_and(|watch| watch.active || watch.begin.is_none()) {
            active.insert(None);
        }
        Self { next: 0, fields: Fields::new(), active, ready: false }
    }
    fn reset(&mut self) {
        self.next = 0;
        self.fields.clear();
    }
}

fn capture(value: &str) -> Result<String, Error> {
    if value.len() > 4096 {
        return Err(Error::Limit);
    }
    Ok(value.into())
}
impl WatchPattern {
    fn key(&self, line: &str) -> Result<Option<Option<String>>, Error> {
        let Some(captures) = self.regex.captures(line).map_err(|_| Error::Runtime)? else { return Ok(None) };
        Ok(Some(self.file.and_then(|index| captures.get(index)).map(|value| capture(value.as_str())).transpose()?))
    }
}

impl Matcher {
    pub fn is_ready(&self) -> bool {
        self.watch.is_some() && self.state.ready
    }

    pub fn line(&mut self, line: &str) -> Result<Update, Error> {
        if line.len() > 64 * 1024 {
            self.state.reset();
            return Err(Error::Limit);
        }
        let mut update = Update::default();
        if let Some(watch) = &self.watch {
            if let Some(key) = watch.begin.as_ref().map(|begin| begin.key(line)).transpose()?.flatten() {
                if key.is_some() {
                    self.state.active.remove(&None);
                } else {
                    self.state.active.clear();
                }
                self.state.active.insert(key);
                self.state.ready = false;
                update.began = true;
            }
            if let Some(key) = watch.end.key(line)? {
                let matched = if key.is_none() {
                    let matched = !self.state.active.is_empty();
                    self.state.active.clear();
                    matched
                } else {
                    self.state.active.remove(&key) || self.state.active.remove(&None)
                };
                if matched && self.state.active.is_empty() {
                    update.ready = !self.state.ready;
                    self.state.ready = true;
                }
            }
        }
        if self.patterns.is_empty() {
            return Ok(update);
        }
        for attempt in 0..2 {
            let pattern = &self.patterns[self.state.next];
            if let Some(captures) = pattern.regex.captures(line).map_err(|_| Error::Runtime)? {
                let previous = self.state.fields.clone();
                for (&field, &index) in &pattern.fields {
                    if let Some(value) = captures.get(index) {
                        self.state.fields.insert(field, capture(value.as_str())?);
                    }
                }
                if self.state.next + 1 == self.patterns.len() {
                    update.problem = problem(&self.state.fields, self.file_kind, self.severity);
                    if pattern.repeat {
                        self.state.fields = previous;
                    } else {
                        self.state.reset();
                    }
                } else {
                    self.state.next += 1;
                }
                break;
            }
            let previous = self.state.next;
            self.state.reset();
            if previous == 0 || attempt == 1 {
                break;
            }
            // The mismatch can itself start a new consecutive-line block.
        }
        Ok(update)
    }
}

fn problem(fields: &Fields, file_kind: bool, default_severity: u8) -> Option<Problem> {
    let file = fields.get("file")?.trim();
    let message = fields.get("message")?.trim();
    if file.is_empty() || file.contains('\0') || message.is_empty() {
        return None;
    }
    let number = |key| fields.get(key).and_then(|value| value.trim().parse::<u32>().ok());
    let (line, column, end_line, end_column) = if let Some(location) = fields.get("location") {
        let numbers = location.trim_matches(['(', ')']).split(',').map(|n| n.trim().parse::<u32>().ok()).collect::<Option<Vec<_>>>()?;
        match numbers.as_slice() {
            [line] => (*line, None, None, None),
            [line, column] => (*line, Some(*column), None, None),
            [line, column, end_line, end_column] => (*line, Some(*column), Some(*end_line), Some(*end_column)),
            _ => return None,
        }
    } else {
        (if file_kind { number("line").unwrap_or(1) } else { number("line")? }, number("column"), number("endLine"), number("endColumn"))
    };
    let severity = match fields.get("severity").map(|value| value.to_ascii_lowercase()).as_deref() {
        Some("error" | "fatal error") => 1,
        Some("warning" | "warn") => 2,
        Some("info" | "note") => 3,
        _ => default_severity,
    };
    Some(Problem { file: file.into(), line: line.max(1), column, end_line, end_column, severity, message: message.into(), code: fields.get("code").cloned() })
}
