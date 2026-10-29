//! Replacement plans for the Search Editor. A plan is an immutable preview of every change, made from the authoritative
//! text of each file (an open editor's buffer when there is one, the bytes on disk otherwise) and tied to the version it
//! was read at: the disk hash or the buffer version. Which changes to apply is a separate `Selection`, so the plan itself
//! never changes after the preview. Nothing is written while a plan exists; `replace_apply` does that, per file.
//!
//! Matching follows the search exactly: the same compiled pattern, line by line (a line ends at `\n`; a `\r` before it is
//! not part of the line), and empty (zero-width) matches are never replacements, as they are never search results. A
//! change only touches the bytes of its match, so line endings, mixed endings and a byte-order mark survive untouched;
//! line breaks inside a replacement take the file's first line ending. Files in another encoding are decoded with the
//! editor's rules and must round-trip exactly, otherwise they are rejected rather than rewritten.

use std::ops::Range;
use std::sync::Arc;

use crate::encoding::Encoding;

/// A plan larger than this is refused (narrow the search instead): files, changes and bytes read.
pub struct Limits {
    pub files: usize,
    pub changes: usize,
    pub bytes: usize,
}
pub const LIMITS: Limits = Limits { files: 1_000, changes: 20_000, bytes: 64 * 1024 * 1024 };

/// Bytes of context kept on each side of a change in a diff row.
pub const DIFF_WINDOW: usize = 400;

/// The compiled search and what to replace each match with.
pub struct Pattern {
    re: regex::bytes::Regex,
    /// In regex mode: the template in the regex crate's syntax (`$1` → `${1}`); in plain mode the text as typed.
    template: String,
    regex: bool,
    preserve_case: bool,
}

impl Pattern {
    pub fn new(query: &proto::SearchQuery, replacement: &str, preserve_case: bool) -> Result<Self, String> {
        let re = agent::search::compile(query).map_err(|e| e.to_string())?;
        let template = if query.regex { crate::find::regex_template(replacement) } else { replacement.to_string() };
        Ok(Pattern { re, template, regex: query.regex, preserve_case })
    }

    /// The replacement for one match (`caps` from this pattern's regex on `line`).
    fn expand(&self, caps: &regex::bytes::Captures<'_>, old: &str) -> String {
        let new = if self.regex {
            let mut out = Vec::new();
            caps.expand(self.template.as_bytes(), &mut out);
            String::from_utf8_lossy(&out).into_owned()
        } else {
            self.template.clone()
        };
        if self.preserve_case { crate::find::preserve_case(old, &new) } else { new }
    }
}

/// The authoritative content of a file when the preview was made.
pub enum Source {
    /// Bytes on disk and their hash (`proto::content_hash`).
    Disk { bytes: Vec<u8>, hash: u64 },
    /// An open editor's text (`\n` line ends, no byte-order mark), its identity (buffer epoch and undo version) and the
    /// encoding it saves in.
    Buffer { text: String, epoch: u64, version: u64, encoding: Encoding },
}

/// The version a file's changes were computed from; apply and undo check it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    Disk {
        hash: u64,
    },
    /// The buffer object, its undo step and a hash of its text: undo ids restart when a buffer is reloaded, so the
    /// version alone does not name a document state.
    Buffer {
        epoch: u64,
        version: u64,
        hash: u64,
    },
}

/// One proposed change: a match and its replacement. Offsets are bytes into the file's decoded text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// 1-based.
    pub line: u32,
    pub start: usize,
    pub end: usize,
    pub old: String,
    pub new: String,
}

/// Why a file cannot be changed (shown in the preview; its changes cannot be selected).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    Binary,
    /// The bytes do not survive decoding and encoding again, so rewriting would change text nobody touched.
    Unsupported,
    /// A replacement holds a character the file's encoding cannot store.
    Unencodable {
        ch: char,
        encoding: &'static str,
    },
    /// Could not be read.
    Unreadable(String),
    /// No match in the file's current text.
    NoChanges,
}

impl Problem {
    pub fn message(&self) -> String {
        match self {
            Problem::Binary => crate::i18n::t("Binary file: not changed.").into(),
            Problem::Unsupported => crate::i18n::t("Unsupported encoding: the file would not be written back exactly, so it is not changed.").into(),
            Problem::Unencodable { ch, encoding } => crate::i18n::tf("“{0}” cannot be written in {1}: not changed.", &[ch, encoding]),
            Problem::Unreadable(e) => crate::i18n::tf("Could not read: {}", &[e]),
            Problem::NoChanges => crate::i18n::t("No matches in this file any more.").into(),
        }
    }
}

pub struct FilePlan {
    pub path: String,
    pub version: Version,
    pub encoding: Encoding,
    bom: bool,
    /// Decoded text, without a byte-order mark.
    text: String,
    /// Disk bytes as read (kept for undo); empty for buffers.
    original: Arc<Vec<u8>>,
    pub changes: Vec<Change>,
    pub problem: Option<Problem>,
}

/// An immutable preview: every change in every file, tied to where and from which run it was made.
pub struct Plan {
    pub host: String,
    pub root: String,
    /// The Search Editor run this plan was made from.
    pub generation: u64,
    pub files: Vec<FilePlan>,
}

/// The preview was refused before it was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    TooLarge,
}

impl FilePlan {
    /// Compute every change in one file from its authoritative content; collecting stops after `limit` changes (the
    /// caller refuses such a plan).
    pub fn build(path: String, source: Source, pattern: &Pattern, fallback: Encoding, limit: usize) -> FilePlan {
        let (text, encoding, bom, version, original, eol) = match source {
            Source::Buffer { text, epoch, version, encoding } => {
                let hash = proto::content_hash(text.as_bytes());
                (text, encoding, false, Version::Buffer { epoch, version, hash }, Arc::new(Vec::new()), "\n")
            }
            Source::Disk { bytes, hash } => match decode(&bytes, fallback) {
                Ok((text, encoding, bom)) => {
                    let eol = first_eol(&text);
                    (text, encoding, bom, Version::Disk { hash }, Arc::new(bytes), eol)
                }
                Err(problem) => {
                    return FilePlan {
                        path,
                        version: Version::Disk { hash },
                        encoding: Encoding::Utf8,
                        bom: false,
                        text: String::new(),
                        original: Arc::new(bytes),
                        changes: Vec::new(),
                        problem: Some(problem),
                    };
                }
            },
        };
        let changes = find_changes(&text, pattern, eol, limit);
        let problem = unencodable(&changes, encoding);
        // Nothing can ever be written for a file without changes: its text and bytes are not kept.
        let (text, original) = if changes.is_empty() { (String::new(), Arc::new(Vec::new())) } else { (text, original) };
        FilePlan { path, version, encoding, bom, text, original, changes, problem }
    }

    /// A file that could not be read: shown with the reason, never changed.
    pub fn unreadable(path: String, why: String) -> FilePlan {
        FilePlan {
            path,
            version: Version::Disk { hash: 0 },
            encoding: Encoding::Utf8,
            bom: false,
            text: String::new(),
            original: Arc::new(Vec::new()),
            changes: Vec::new(),
            problem: Some(Problem::Unreadable(why)),
        }
    }

    pub fn original(&self) -> Arc<Vec<u8>> {
        self.original.clone()
    }

    /// The text after applying the selected changes (`selected[i]` for `changes[i]`).
    pub fn render(&self, selected: &[bool]) -> String {
        let mut out = String::with_capacity(self.text.len());
        let mut done = 0;
        for (c, _) in self.changes.iter().zip(selected).filter(|(_, s)| **s) {
            out.push_str(&self.text[done..c.start]);
            out.push_str(&c.new);
            done = c.end;
        }
        out.push_str(&self.text[done..]);
        out
    }

    /// Bytes to write for the selected changes, in the file's encoding and with its byte-order mark.
    pub fn output_bytes(&self, selected: &[bool]) -> Result<Vec<u8>, Problem> {
        if let Some(p) = &self.problem {
            return Err(p.clone());
        }
        // Its content was not kept (see `build`): there is nothing to write, and never an empty file.
        if self.changes.is_empty() {
            return Err(Problem::NoChanges);
        }
        encode(&self.render(selected), self.encoding, self.bom).map_err(|ch| Problem::Unencodable { ch, encoding: self.encoding.label() })
    }

    /// Edits for an open editor: character ranges in its text and the replacement for each.
    pub fn buffer_edits(&self, selected: &[bool]) -> Vec<(Range<usize>, String)> {
        let (mut chars, mut byte) = (0usize, 0usize);
        let mut to_char = |b: usize| {
            chars += self.text[byte..b].chars().count();
            byte = b;
            chars
        };
        let mut out = Vec::new();
        for (c, _) in self.changes.iter().zip(selected).filter(|(_, s)| **s) {
            let start = to_char(c.start);
            let end = to_char(c.end);
            out.push((start..end, c.new.clone()));
        }
        out
    }

    /// The line a change is on, before and after the selected changes on that line: (old line, new line, the change's
    /// byte range in the old line, its range in the new line). Exactly the bytes that will be written, minus line breaks.
    /// A long line is cut to about `DIFF_WINDOW` bytes on each side of the change (marked with "…"), so the work per
    /// shown row stays bounded however long the line is; changes cut by the window edge show unchanged.
    pub fn line_diff(&self, change: usize, selected: &[bool]) -> (String, String, Range<usize>, Range<usize>) {
        let Some(c) = self.changes.get(change) else { return Default::default() };
        let text = &self.text;
        let back = text.floor_char_boundary(c.start.saturating_sub(DIFF_WINDOW));
        let (start, cut_front) = match text[back..c.start].rfind('\n') {
            Some(i) => (back + i + 1, false),
            None => (back, back > 0),
        };
        let fwd = text.ceil_char_boundary((c.end + DIFF_WINDOW).min(text.len()));
        let (end, cut_back) = match text[c.end..fwd].find('\n') {
            Some(i) if i > 0 && text[..c.end + i].ends_with('\r') => (c.end + i - 1, false),
            Some(i) => (c.end + i, false),
            None => (fwd, fwd < text.len()),
        };
        let (pre, suf) = (if cut_front { "…" } else { "" }, if cut_back { "…" } else { "" });
        let old = format!("{pre}{}{suf}", &text[start..end]);
        let mut new = String::from(pre);
        let mut done = start;
        let mut range_new = 0..0;
        // Changes are in text order: only those inside the window are looked at.
        let (lo, hi) = (self.changes.partition_point(|o| o.start < start), self.changes.partition_point(|o| o.start < end));
        for i in lo..hi.max(lo) {
            let (o, s) = (&self.changes[i], selected.get(i).copied().unwrap_or(false));
            if o.end > end {
                continue;
            }
            if i == change {
                new.push_str(&text[done..o.start]);
                let at = new.len();
                new.push_str(if s { &o.new } else { &o.old });
                range_new = at..new.len();
                done = o.end;
            } else if s {
                new.push_str(&text[done..o.start]);
                new.push_str(&o.new);
                done = o.end;
            }
        }
        new.push_str(&text[done..end]);
        new.push_str(suf);
        (old, new, c.start - start + pre.len()..c.end - start + pre.len(), range_new)
    }
}

impl Plan {
    /// Build a plan for every file; refuses when it would exceed `limits`.
    pub fn build(
        host: String,
        root: String,
        generation: u64,
        sources: Vec<(String, Result<Source, String>)>,
        pattern: &Pattern,
        fallback: Encoding,
        limits: &Limits,
    ) -> Result<Plan, PlanError> {
        if sources.len() > limits.files {
            return Err(PlanError::TooLarge);
        }
        let bytes: usize = sources
            .iter()
            .map(|(_, s)| match s {
                Ok(Source::Disk { bytes, .. }) => bytes.len(),
                Ok(Source::Buffer { text, .. }) => text.len(),
                Err(_) => 0,
            })
            .sum();
        if bytes > limits.bytes {
            return Err(PlanError::TooLarge);
        }
        let mut files = Vec::new();
        let mut changes = 0;
        for (path, source) in sources {
            let file = match source {
                Ok(s) => FilePlan::build(path, s, pattern, fallback, limits.changes - changes + 1),
                Err(e) => FilePlan::unreadable(path, e),
            };
            changes += file.changes.len();
            if changes > limits.changes {
                return Err(PlanError::TooLarge);
            }
            files.push(file);
        }
        Ok(Plan { host, root, generation, files })
    }
}

/// Which changes of a plan are selected. Kept apart from the plan, which never changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection(pub Vec<Vec<bool>>);

/// A file's selection: all, some or none of its changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tri {
    All,
    Some,
    None,
}

impl Selection {
    /// Every change of every file that can be changed.
    pub fn all(plan: &Plan) -> Self {
        Selection(plan.files.iter().map(|f| vec![f.problem.is_none(); f.changes.len()]).collect())
    }

    /// Only the changes of one file.
    pub fn only(plan: &Plan, path: &str) -> Self {
        Selection(plan.files.iter().map(|f| vec![f.problem.is_none() && f.path == path; f.changes.len()]).collect())
    }

    pub fn file(&self, f: usize) -> &[bool] {
        self.0.get(f).map_or(&[], Vec::as_slice)
    }

    pub fn state(&self, f: usize) -> Tri {
        let s = self.file(f);
        match s.iter().filter(|x| **x).count() {
            0 => Tri::None,
            n if n == s.len() => Tri::All,
            _ => Tri::Some,
        }
    }

    pub fn toggle_change(&mut self, plan: &Plan, f: usize, c: usize) {
        if plan.files.get(f).is_some_and(|p| p.problem.is_none())
            && let Some(x) = self.0.get_mut(f).and_then(|v| v.get_mut(c))
        {
            *x = !*x;
        }
    }

    /// All selected → none; otherwise all.
    pub fn toggle_file(&mut self, plan: &Plan, f: usize) {
        if plan.files.get(f).is_none_or(|p| p.problem.is_some()) {
            return;
        }
        let on = self.state(f) != Tri::All;
        if let Some(v) = self.0.get_mut(f) {
            v.iter_mut().for_each(|x| *x = on);
        }
    }

    /// (selected changes, files with at least one).
    pub fn counts(&self) -> (usize, usize) {
        let changes = self.0.iter().map(|v| v.iter().filter(|x| **x).count()).sum();
        let files = self.0.iter().filter(|v| v.iter().any(|x| *x)).count();
        (changes, files)
    }
}

/// Every non-empty match of `pattern` in `text`, line by line, with its replacement.
fn find_changes(text: &str, pattern: &Pattern, eol: &str, limit: usize) -> Vec<Change> {
    let mut out = Vec::new();
    let mut offset = 0;
    for (ix, raw) in text.split('\n').enumerate() {
        if out.len() >= limit {
            break;
        }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        for caps in pattern.re.captures_iter(line.as_bytes()) {
            if out.len() >= limit {
                break;
            }
            let Some(m) = caps.get(0) else { continue };
            // Zero-width matches are not search results; a byte pattern (`(?-u)`) may also end inside a character.
            if m.is_empty() || !line.is_char_boundary(m.start()) || !line.is_char_boundary(m.end()) {
                continue;
            }
            let old = &line[m.start()..m.end()];
            let new = unify_breaks(&pattern.expand(&caps, old), eol);
            out.push(Change { line: ix as u32 + 1, start: offset + m.start(), end: offset + m.end(), old: old.to_string(), new });
        }
        offset += raw.len() + 1;
    }
    out
}

/// Line breaks inside a replacement become the file's line ending.
fn unify_breaks(text: &str, eol: &str) -> String {
    if !text.contains(['\n', '\r']) {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n").replace('\n', eol)
}

/// The file's first line ending (as the editor picks one); `\n` if it has none.
fn first_eol(text: &str) -> &'static str {
    match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
        Some(_) => "\n",
        None if text.contains('\r') => "\r",
        None => "\n",
    }
}

/// A replacement the encoding cannot store makes the whole file unchangeable (it could not be saved).
fn unencodable(changes: &[Change], encoding: Encoding) -> Option<Problem> {
    if matches!(encoding, Encoding::Utf8 | Encoding::Utf16Le | Encoding::Utf16Be) {
        return None;
    }
    changes.iter().find_map(|c| crate::encoding::encode(&c.new, encoding).err()).map(|ch| Problem::Unencodable { ch, encoding: encoding.label() })
}

/// Decode with the editor's rules; refuse what would not be written back byte for byte.
pub fn decode(bytes: &[u8], fallback: Encoding) -> Result<(String, Encoding, bool), Problem> {
    let (text, encoding) = crate::encoding::decode(bytes, fallback).ok_or(Problem::Binary)?;
    let (bom, text) = match text.strip_prefix('\u{feff}') {
        Some(rest) if encoding == Encoding::Utf8 => (true, rest.to_string()),
        _ => (false, text),
    };
    match encode(&text, encoding, bom) {
        Ok(back) if back == bytes => Ok((text, encoding, bom)),
        _ => Err(Problem::Unsupported),
    }
}

fn encode(text: &str, encoding: Encoding, bom: bool) -> Result<Vec<u8>, char> {
    let mut out = crate::encoding::encode(text, encoding)?;
    if bom {
        out.splice(0..0, [0xEF, 0xBB, 0xBF]);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "replace_plan_tests.rs"]
mod tests;
