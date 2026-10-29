//! The Search Editor's document: one retained search (query, options, context lines, the host and folder it ran in) and a
//! bounded snapshot of its results. It is separate from the sidebar's streaming rows: the sidebar redraws while results
//! arrive, this keeps a model that can be navigated, rerun, saved across restarts and turned into a replacement plan.
//!
//! Everything here is bounded: results stop at `MAX_FILES`/`MAX_MATCHES` (and say so), context is fetched for a limited
//! number of matches, and the saved form keeps the query plus at most `SAVED_MAX_MATCHES` results per tab. Restored results
//! are always marked stale: they describe files as they were, and nothing may be replaced from them before a rerun.

use serde::{Deserialize, Serialize};

/// Results kept in memory per Search Editor. Beyond these the run is marked truncated.
pub const MAX_FILES: usize = 5_000;
pub const MAX_MATCHES: usize = 20_000;
/// Context lines above and below each match (the stepper's range).
pub const MAX_CONTEXT: u8 = 5;
/// Context is read for this many matches; later matches show without it.
pub const CONTEXT_MATCHES: usize = 2_000;
/// Longest stored preview or context line, in characters.
pub const LINE_CHARS: usize = 400;
/// Saved state: tabs, results per tab and bytes per tab.
pub const SAVED_MAX_TABS: usize = 8;
pub const SAVED_MAX_MATCHES: usize = 200;
pub const SAVED_MAX_BYTES: usize = 64 * 1024;
/// Saved text larger than this is ignored on restore (a corrupt or foreign value).
const SAVED_MAX_TOTAL: usize = 1024 * 1024;
const SAVED_VERSION: u8 = 1;
const MAX_PATTERN: usize = 10_000;

/// What to search for and how. The replacement text lives here too, so a retained search keeps its pending replacement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SearchSpec {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub preserve_case: bool,
    /// Comma-separated globs, as typed.
    pub include: String,
    pub exclude: String,
    /// Context lines above and below each match.
    pub context: u8,
    pub replacement: String,
}

impl SearchSpec {
    /// Comma-separated globs as a list (blank entries dropped).
    pub fn globs(text: &str) -> Vec<String> {
        text.split(',').map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect()
    }

    /// The query sent to the agent for one root. `smart_case` and the extra excludes come from the settings.
    pub fn query(&self, root: &str, smart_case: bool, extra_exclude: &[String], max_results: u32) -> Option<proto::SearchQuery> {
        if self.pattern.is_empty() {
            return None;
        }
        let mut exclude = Self::globs(&self.exclude);
        exclude.extend(extra_exclude.iter().cloned());
        Some(proto::SearchQuery {
            root: root.to_string(),
            pattern: self.pattern.clone(),
            regex: self.regex,
            case_sensitive: self.case_sensitive || (smart_case && self.pattern.chars().any(char::is_uppercase)),
            whole_word: self.whole_word,
            include: Self::globs(&self.include),
            exclude,
            max_results: max_results.min(MAX_MATCHES as u32),
        })
    }
}

/// Where a search ran: the host's label, the workspace's first folder and its other folders. Results, plans and undo
/// records are only valid there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Provenance {
    pub host: String,
    pub root: String,
    /// The workspace's other folders, searched too (saved state from before multi-folder support has none).
    #[serde(default)]
    pub extra_roots: Vec<String>,
}

impl Provenance {
    /// Every folder searched: the first one, then the others.
    pub fn roots(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.root.as_str()).chain(self.extra_roots.iter().map(String::as_str))
    }
}

/// `path` inside folder `root`: its path relative to it.
fn inside<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    path.strip_prefix(root.trim_end_matches('/')).and_then(|p| p.strip_prefix('/'))
}

/// How the last run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunState {
    /// Never run (a new or restored tab with no results).
    NotRun,
    Running,
    Complete,
    /// Stopped at the result limit: files beyond it were not searched.
    Truncated,
    Cancelled,
    Failed(String),
}

/// Why results may no longer describe the files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stale {
    /// Restored from the last session.
    Restored,
    /// A replacement changed files since this run.
    Replaced,
    /// The window now shows another host or folder.
    Workspace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocMatch {
    /// 1-based.
    pub line: u32,
    /// Column of the first match (characters, 0-based).
    pub column: u32,
    pub preview: String,
    /// Byte ranges of the matches within `preview`.
    pub ranges: Vec<(u32, u32)>,
    /// Context lines above and below (filled after the run, for the first `CONTEXT_MATCHES` matches).
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocFile {
    /// Absolute path on the search's host.
    pub path: String,
    pub matches: Vec<DocMatch>,
    pub collapsed: bool,
}

/// One visible row of the results list (the list is virtualized: only the rows on screen are built).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    File(usize),
    Match(usize, usize),
    /// A context line: file, the match it belongs to, and its line number.
    Context(usize, usize, u32),
}

#[derive(Clone, Debug)]
pub struct SearchDoc {
    pub spec: SearchSpec,
    pub provenance: Provenance,
    pub files: Vec<DocFile>,
    pub state: RunState,
    pub stale: Option<Stale>,
    /// Matches dropped by the `MAX_FILES`/`MAX_MATCHES` caps.
    pub capped: bool,
    /// Restored from a saved snapshot that kept only part of the results.
    pub partial: bool,
    /// Bumped on every run: a plan or reply made for an older run is refused.
    pub generation: u64,
    matches: usize,
}

impl SearchDoc {
    pub fn new(spec: SearchSpec, provenance: Provenance) -> Self {
        SearchDoc { spec, provenance, files: Vec::new(), state: RunState::NotRun, stale: None, capped: false, partial: false, generation: 0, matches: 0 }
    }

    pub fn match_count(&self) -> usize {
        self.matches
    }

    /// Results describe the current files completely: the run finished without truncation, cancellation or failure, and
    /// nothing has made it stale since. Bulk replacement needs this.
    pub fn is_current_and_complete(&self) -> bool {
        self.state == RunState::Complete && self.stale.is_none() && !self.capped && !self.partial
    }

    /// Start a new run: results are cleared and the generation moves on.
    pub fn begin(&mut self) -> u64 {
        self.files.clear();
        self.matches = 0;
        self.capped = false;
        self.partial = false;
        self.stale = None;
        self.state = RunState::Running;
        self.generation += 1;
        self.generation
    }

    /// Add one file's matches (sorted by path; a second chunk for the same file is appended). Over the caps the rest is
    /// dropped and the run will end as truncated.
    pub fn add(&mut self, path: String, matches: Vec<proto::LineMatch>) {
        let room = MAX_MATCHES.saturating_sub(self.matches);
        let mut kept: Vec<DocMatch> = matches.into_iter().map(DocMatch::from_line).collect();
        let total: usize = kept.iter().map(|m| m.ranges.len().max(1)).sum();
        if total > room {
            let mut n = 0;
            kept.retain(|m| {
                n += m.ranges.len().max(1);
                n <= room
            });
            self.capped = true;
        }
        if kept.is_empty() {
            return;
        }
        let at = self.files.partition_point(|f| f.path < path);
        if self.files.get(at).is_some_and(|f| f.path == path) {
            self.files[at].matches.extend(kept);
        } else if self.files.len() >= MAX_FILES {
            self.capped = true;
            return;
        } else {
            self.files.insert(at, DocFile { path, matches: kept, collapsed: false });
        }
        self.matches = self.files.iter().flat_map(|f| &f.matches).map(|m| m.ranges.len().max(1)).sum();
    }

    /// The run ended. `truncated`/`cancelled` come from the agent; a cap hit here also counts as truncated.
    pub fn finish(&mut self, truncated: bool, cancelled: bool) {
        self.state = if cancelled {
            RunState::Cancelled
        } else if truncated || self.capped {
            RunState::Truncated
        } else {
            RunState::Complete
        };
    }

    pub fn fail(&mut self, why: String) {
        self.state = RunState::Failed(why);
    }

    /// Stop a running search: what arrived stays, marked cancelled.
    pub fn cancel(&mut self) {
        if self.state == RunState::Running {
            self.state = RunState::Cancelled;
        }
    }

    pub fn mark_stale(&mut self, why: Stale) {
        if !self.files.is_empty() || self.state != RunState::NotRun {
            self.stale = Some(why);
        }
    }

    /// Context for one match (`before` and `after`, nearest last/first), each line trimmed to `LINE_CHARS`.
    pub fn set_context(&mut self, file: usize, m: usize, before: Vec<String>, after: Vec<String>) {
        if let Some(dm) = self.files.get_mut(file).and_then(|f| f.matches.get_mut(m)) {
            dm.before = before.into_iter().map(|l| clip(&l)).collect();
            dm.after = after.into_iter().map(|l| clip(&l)).collect();
        }
    }

    /// The matches context is fetched for: (file index, match index, line), in order, at most `CONTEXT_MATCHES`.
    pub fn context_targets(&self) -> Vec<(usize, usize, u32)> {
        self.files.iter().enumerate().flat_map(|(fi, f)| f.matches.iter().enumerate().map(move |(mi, m)| (fi, mi, m.line))).take(CONTEXT_MATCHES).collect()
    }

    /// Visible rows: file headers, then (unless collapsed) each match with its context lines. A context line already shown
    /// (or that is itself a match) is not repeated.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (fi, f) in self.files.iter().enumerate() {
            rows.push(Row::File(fi));
            if f.collapsed {
                continue;
            }
            let mut shown = 0u32;
            for (mi, m) in f.matches.iter().enumerate() {
                let first = m.line.saturating_sub(m.before.len() as u32);
                for (k, _) in m.before.iter().enumerate() {
                    let line = first + k as u32;
                    if line > shown && line < m.line {
                        rows.push(Row::Context(fi, mi, line));
                        shown = line;
                    }
                }
                rows.push(Row::Match(fi, mi));
                shown = shown.max(m.line);
                let next = f.matches.get(mi + 1).map_or(u32::MAX, |n| n.line);
                for k in 0..m.after.len() {
                    let line = m.line + 1 + k as u32;
                    if line > shown && line < next {
                        rows.push(Row::Context(fi, mi, line));
                        shown = line;
                    }
                }
            }
        }
        rows
    }

    /// Text of a context row.
    pub fn context_text(&self, file: usize, m: usize, line: u32) -> &str {
        let Some(dm) = self.files.get(file).and_then(|f| f.matches.get(m)) else { return "" };
        if line < dm.line {
            let first = dm.line.saturating_sub(dm.before.len() as u32);
            dm.before.get((line.saturating_sub(first)) as usize).map_or("", String::as_str)
        } else {
            dm.after.get((line - dm.line).saturating_sub(1) as usize).map_or("", String::as_str)
        }
    }

    /// Matches in order, as (file, match).
    pub fn flat_matches(&self) -> Vec<(usize, usize)> {
        self.files.iter().enumerate().flat_map(|(fi, f)| (0..f.matches.len()).map(move |mi| (fi, mi))).collect()
    }

    /// Path shown for a file: relative to the first folder, `folder › path` in another folder, else as is.
    pub fn display_path<'a>(&self, path: &'a str) -> std::borrow::Cow<'a, str> {
        if let Some(rel) = inside(path, &self.provenance.root) {
            return rel.into();
        }
        for extra in &self.provenance.extra_roots {
            if let Some(rel) = inside(path, extra) {
                return format!("{}{}{rel}", crate::ui::file_name(extra), crate::workbench::ROOT_SEP).into();
            }
        }
        path.into()
    }

    /// The results as plain text (for the clipboard): the query and options, then each file and its matching lines.
    pub fn as_text(&self) -> String {
        let options = match (self.spec.regex, self.spec.case_sensitive) {
            (true, true) => crate::i18n::t("Regex, Match Case"),
            (true, false) => crate::i18n::t("Regex, Ignore Case"),
            (false, true) => crate::i18n::t("Match Case"),
            (false, false) => crate::i18n::t("Ignore Case"),
        };
        let mut out = format!("# {}\n# {options}\n", crate::i18n::tf("Query: {}", &[&self.spec.pattern]));
        for f in &self.files {
            out.push_str(&format!("\n{}:\n", self.display_path(&f.path)));
            for m in &f.matches {
                out.push_str(&format!("  {}: {}\n", m.line, m.preview.trim_end()));
            }
        }
        out
    }

    // --- Saved state ---

    fn to_saved(&self) -> SavedDoc {
        let mut budget = SAVED_MAX_BYTES.saturating_sub(self.spec.pattern.len() + self.spec.replacement.len() + self.provenance.root.len());
        let (mut kept, mut omitted) = (0usize, 0usize);
        let mut files = Vec::new();
        for f in &self.files {
            let mut matches = Vec::new();
            for m in &f.matches {
                let cost = m.preview.len() + f.path.len() / 4 + 24;
                if kept >= SAVED_MAX_MATCHES || cost > budget {
                    omitted += 1;
                    continue;
                }
                budget -= cost;
                kept += 1;
                matches.push(SavedMatch { line: m.line, column: m.column, preview: m.preview.clone(), ranges: m.ranges.clone() });
            }
            if !matches.is_empty() {
                budget = budget.saturating_sub(f.path.len());
                files.push(SavedFile { path: f.path.clone(), matches });
            }
        }
        let state = match &self.state {
            RunState::Complete => "complete",
            RunState::Truncated => "truncated",
            RunState::Cancelled | RunState::Running => "cancelled",
            RunState::Failed(_) => "failed",
            RunState::NotRun => "not-run",
        };
        SavedDoc {
            spec: self.spec.clone(),
            provenance: self.provenance.clone(),
            state: state.into(),
            capped: self.capped,
            omitted: omitted + usize::from(self.partial),
            files,
        }
    }

    fn from_saved(s: SavedDoc) -> Option<Self> {
        if s.spec.pattern.len() > MAX_PATTERN
            || s.spec.replacement.len() > MAX_PATTERN
            || s.provenance.roots().any(|r| r.contains('\0'))
            || s.provenance.extra_roots.len() > 64
        {
            return None;
        }
        let mut doc = SearchDoc::new(SearchSpec { context: s.spec.context.min(MAX_CONTEXT), ..s.spec }, s.provenance);
        let mut count = 0;
        for f in s.files {
            if f.path.contains('\0') || f.path.len() > 32 * 1024 || f.matches.is_empty() {
                continue;
            }
            let matches: Vec<DocMatch> = f
                .matches
                .into_iter()
                .take(SAVED_MAX_MATCHES.saturating_sub(count))
                .map(|m| {
                    let preview = clip(&m.preview);
                    // Ranges that do not fit the (possibly clipped) preview are dropped rather than trusted.
                    let ranges = m
                        .ranges
                        .into_iter()
                        .filter(|&(a, b)| {
                            a < b && (b as usize) <= preview.len() && preview.is_char_boundary(a as usize) && preview.is_char_boundary(b as usize)
                        })
                        .collect();
                    DocMatch { line: m.line.max(1), column: m.column, preview, ranges, before: Vec::new(), after: Vec::new() }
                })
                .collect();
            count += matches.len();
            let at = doc.files.partition_point(|x| x.path < f.path);
            if doc.files.get(at).is_some_and(|x| x.path == f.path) || matches.is_empty() {
                continue;
            }
            doc.files.insert(at, DocFile { path: f.path, matches, collapsed: false });
        }
        doc.matches = doc.files.iter().flat_map(|f| &f.matches).map(|m| m.ranges.len().max(1)).sum();
        doc.state = match s.state.as_str() {
            "complete" => RunState::Complete,
            "truncated" => RunState::Truncated,
            "failed" => RunState::Failed(String::new()),
            "not-run" => RunState::NotRun,
            _ => RunState::Cancelled,
        };
        doc.capped = s.capped;
        doc.partial = s.omitted > 0;
        doc.stale = Some(Stale::Restored);
        Some(doc)
    }
}

impl DocMatch {
    fn from_line(m: proto::LineMatch) -> Self {
        let preview = clip(&m.preview);
        let ranges = m
            .ranges
            .into_iter()
            .filter(|&(a, b)| a < b && (b as usize) <= preview.len() && preview.is_char_boundary(a as usize) && preview.is_char_boundary(b as usize))
            .collect();
        DocMatch { line: m.line, column: m.column, preview, ranges, before: Vec::new(), after: Vec::new() }
    }
}

/// At most `LINE_CHARS` characters (on a character boundary).
fn clip(s: &str) -> String {
    match s.char_indices().nth(LINE_CHARS) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedMatch {
    line: u32,
    column: u32,
    preview: String,
    ranges: Vec<(u32, u32)>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedFile {
    path: String,
    matches: Vec<SavedMatch>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedDoc {
    spec: SearchSpec,
    provenance: Provenance,
    state: String,
    capped: bool,
    omitted: usize,
    files: Vec<SavedFile>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    docs: Vec<SavedDoc>,
}

/// The saved form of a workspace's Search Editors: at most `SAVED_MAX_TABS`, each bounded (see `SAVED_MAX_MATCHES`).
pub fn save(docs: &[&SearchDoc]) -> String {
    let saved = Saved { version: SAVED_VERSION, docs: docs.iter().take(SAVED_MAX_TABS).map(|d| d.to_saved()).collect() };
    serde_json::to_string(&saved).unwrap_or_default()
}

/// Search Editors from saved text, all marked stale (`Stale::Restored`). Unreadable or oversized text restores nothing.
pub fn restore(text: &str) -> Vec<SearchDoc> {
    if text.len() > SAVED_MAX_TOTAL {
        return Vec::new();
    }
    match serde_json::from_str::<Saved>(text) {
        Ok(s) if s.version == SAVED_VERSION => s.docs.into_iter().take(SAVED_MAX_TABS).filter_map(SearchDoc::from_saved).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "search_doc_tests.rs"]
mod tests;
