//! Structural search and replace, the part without a window: finding the files worth parsing, matching, and computing
//! edits. The panel (`search_structural.rs`) and the workbench (`workbench_structural.rs`) only drive this.
//!
//! It lives in the app and not in the agent on purpose: the agent has no tree-sitter and stays a 1.5 MB static binary,
//! and the protocol needs no new request. The agent's text search narrows the files (a match must contain the longest
//! token of the pattern), then only those files are read and parsed here, one tree at a time. Nothing from the
//! searched folder is executed; the files are only read until the user applies a replacement.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use host::{Host, SearchStream};
use proto::{FileMatches, LineMatch, Response};
use syntax::structural::{self as st, Family, Pattern};

/// Files above this size are not parsed (generated code, bundles): parsing them costs more than the answer is worth.
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
/// The narrowing search's result limit; the protocol allows at most this many.
const NARROWING_LIMIT: u32 = 100_000;
const PREVIEW_CHARS: usize = 240;

/// What the panel needs to know after a run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    pub files: u32,
    pub matches: u32,
    /// Files that did not parse without errors.
    pub skipped_parse: u32,
    /// Binary, too large, not UTF-8 or unreadable.
    pub skipped_other: u32,
    /// More candidate files than `search.structural.maxFiles`: the rest were not looked at.
    pub candidates_cut: bool,
    /// The text search that narrows the files stopped at its own result limit.
    pub narrowing_cut: bool,
}

pub enum Event {
    Found(FileMatches),
    Done(Totals),
    Failed(String),
}

/// One folder of the workspace being searched, with the text search already running in it.
pub struct Root {
    /// Shown in front of the path for extra workspace folders (`folder › path`).
    pub label: Option<String>,
    pub dir: String,
    pub stream: SearchStream,
}

pub struct Job {
    pub family: &'static Family,
    pub pattern: Arc<Pattern>,
    pub max_files: usize,
    /// Unsaved editor text by absolute path; it wins over what is on disk.
    pub open: HashMap<String, String>,
}

/// The query for the text search that narrows the files: the pattern's longest token, in files of the family.
pub fn narrowing_query(root: &str, pattern: &Pattern, family: &Family, include: Vec<String>, exclude: Vec<String>) -> proto::SearchQuery {
    // Not every pattern has a token (`$A + $B` still has `+`); the hint is only absent for patterns that are all variables,
    // which `compile` refuses.
    let hint = pattern.literal_hint().unwrap_or_default();
    let mut globs: Vec<String> = family.extensions.iter().map(|e| format!("*.{e}")).collect();
    // The user's own include globs narrow further; both must hold, so the family's globs only apply without them.
    if !include.is_empty() {
        globs = include;
    }
    proto::SearchQuery {
        root: root.into(),
        pattern: hint,
        regex: false,
        case_sensitive: true,
        whole_word: false,
        include: globs,
        exclude,
        max_results: NARROWING_LIMIT,
    }
}

struct Candidate {
    shown: String,
    abs: String,
    lang: &'static syntax::Language,
}

/// What the reader hands to the matcher.
enum Item {
    Text(String),
    Skipped,
}

/// Text as the editor sees it: no BOM, `\n` line ends. Offsets found in it are the offsets of the open buffer.
pub fn editor_text(bytes: &[u8]) -> Option<String> {
    let b = buffer::Buffer::from_bytes(bytes).ok()?;
    Some(b.rope().to_string())
}

/// Run a search: wait for the narrowing searches, then read and match each candidate file. Blocking; call it from a
/// background thread. `cancel` is checked between files.
pub fn scan(host: Arc<dyn Host>, job: Job, roots: Vec<Root>, cancel: &AtomicBool, mut emit: impl FnMut(Event)) {
    let mut totals = Totals::default();
    let mut found: Vec<Candidate> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in &roots {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            match root.stream.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Response::SearchChunk(files)) => {
                    for fm in files {
                        let abs = crate::ui::join(&root.dir, &fm.path);
                        let Some(lang) = job.family.language_of(&abs) else { continue };
                        let shown = match &root.label {
                            Some(l) => format!("{l}{}{}", crate::workbench::ROOT_SEP, fm.path),
                            None => fm.path,
                        };
                        if seen.insert(abs.clone()) {
                            found.push(Candidate { shown, abs, lang });
                        }
                    }
                }
                Ok(Response::SearchDone { truncated, .. }) => {
                    totals.narrowing_cut |= truncated;
                    break;
                }
                Ok(Response::Error(e)) => return emit(Event::Failed(e.to_string())),
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }
    // An unsaved buffer may hold a match the saved file does not (and the other way round), so it is always looked at.
    for abs in job.open.keys() {
        let Some(lang) = job.family.language_of(abs) else { continue };
        let Some((root, rel)) = roots.iter().find_map(|r| relative(abs, &r.dir).map(|rel| (r, rel))) else { continue };
        if seen.insert(abs.clone()) {
            let shown = match &root.label {
                Some(l) => format!("{l}{}{rel}", crate::workbench::ROOT_SEP),
                None => rel,
            };
            found.push(Candidate { shown, abs: abs.clone(), lang });
        }
    }
    found.sort_by(|a, b| a.shown.cmp(&b.shown));
    if found.len() > job.max_files {
        found.truncate(job.max_files);
        totals.candidates_cut = true;
    }

    // Reading (a network round trip on a remote folder) overlaps with matching; at most one tree exists at a time.
    let (tx, rx) = mpsc::sync_channel::<Item>(4);
    std::thread::scope(|scope| {
        let (host, open, found) = (&host, &job.open, &found);
        scope.spawn(move || {
            for c in found {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let item = match open.get(&c.abs) {
                    Some(text) => Item::Text(text.clone()),
                    None => match host.read(&c.abs) {
                        Ok((data, _)) if data.len() <= MAX_FILE_BYTES && !data[..data.len().min(8192)].contains(&0) => {
                            editor_text(&data).map_or(Item::Skipped, Item::Text)
                        }
                        _ => Item::Skipped,
                    },
                };
                if tx.send(item).is_err() {
                    return;
                }
            }
        });
        for c in found.iter() {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let Ok(item) = rx.recv() else { break };
            match item {
                Item::Skipped => totals.skipped_other += 1,
                Item::Text(text) if text.len() > MAX_FILE_BYTES => totals.skipped_other += 1,
                Item::Text(text) => match line_matches(&job.pattern, c.lang, &text) {
                    Err(st::ParseFailed) => totals.skipped_parse += 1,
                    Ok(lines) if lines.is_empty() => {}
                    Ok(lines) => {
                        totals.files += 1;
                        totals.matches += lines.len() as u32;
                        emit(Event::Found(FileMatches { path: c.shown.clone(), matches: lines }));
                    }
                },
            }
        }
    });
    if !cancel.load(Ordering::Relaxed) {
        emit(Event::Done(totals));
    }
}

/// `abs` relative to `dir`, if it is inside it.
fn relative(abs: &str, dir: &str) -> Option<String> {
    let rest = abs.strip_prefix(dir.trim_end_matches('/'))?.strip_prefix('/')?;
    Some(rest.to_string())
}

/// The matches of one file, one row each, as the search panel lists them.
pub fn line_matches(pattern: &Pattern, lang: &syntax::Language, text: &str) -> Result<Vec<LineMatch>, st::ParseFailed> {
    let matches = st::find_all_in(pattern, lang, text)?;
    // Matches come in text order, so the line number is counted forward from the previous match.
    let (mut line, mut line_start, mut at) = (1u32, 0usize, 0usize);
    let mut out = Vec::with_capacity(matches.len());
    for m in matches {
        let start = m.range.start;
        for nl in memchr::memchr_iter(b'\n', &text.as_bytes()[at..start]) {
            line += 1;
            line_start = at + nl + 1;
        }
        at = start;
        let line_end = text[line_start..].find('\n').map_or(text.len(), |i| line_start + i);
        let full = &text[line_start..line_end];
        let (a, b) = (start - line_start, (m.range.end.min(line_end)) - line_start);
        // Leave out the indentation, and keep at most 40 characters before the match when the line is long.
        let indent = full.len() - full.trim_start().len();
        let mut from = indent.min(a);
        let before: usize = full[from..a].chars().rev().take(40).map(char::len_utf8).sum();
        if full[from..a].chars().count() > 40 {
            from = a - before;
        }
        let shown_len: usize = full[from..].chars().take(PREVIEW_CHARS).map(char::len_utf8).sum();
        let to = from + shown_len;
        let ranges = if a < to { vec![((a - from) as u32, (b.min(to) - from) as u32)] } else { Vec::new() };
        out.push(LineMatch { line, preview: full[from..to].to_string(), ranges, column: text[line_start..start].chars().count() as u32 });
    }
    Ok(out)
}

/// Replacements for every outermost match of the pattern in `text`: byte ranges and the text that replaces each.
pub fn replacements(pattern: &Pattern, lang: &syntax::Language, text: &str, template: &str) -> Vec<(Range<usize>, String)> {
    let Ok(matches) = st::find_all_in(pattern, lang, text) else { return Vec::new() };
    st::outermost(&matches).into_iter().map(|m| (m.range.clone(), st::expand(template, &m.captures))).collect()
}

/// The text after applying byte-range replacements (sorted, not overlapping).
pub fn apply(text: &str, edits: &[(Range<usize>, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut done = 0;
    for (r, new) in edits {
        out.push_str(&text[done..r.start]);
        out.push_str(new);
        done = r.end;
    }
    out.push_str(&text[done..]);
    out
}

/// Replacements as the buffer wants them: char ranges.
pub fn buffer_edits(pattern: &Pattern, lang: &syntax::Language, b: &buffer::Buffer, template: &str) -> Vec<(Range<usize>, String)> {
    let rope = b.rope();
    let text = rope.to_string();
    replacements(pattern, lang, &text, template).into_iter().map(|(r, new)| (rope.byte_to_char(r.start)..rope.byte_to_char(r.end), new)).collect()
}

/// Would the file still parse after the replacement? (A file that parsed before; `None` if it has no matches.)
pub fn result_parses(pattern: &Pattern, lang: &syntax::Language, text: &str, template: &str) -> Option<bool> {
    let edits = replacements(pattern, lang, text, template);
    (!edits.is_empty()).then(|| st::parses_cleanly(lang, &apply(text, &edits)))
}
