//! Staging, unstaging and reverting single lines. A change is picked row by row from its side-by-side diff (the rows the
//! diff view shows): a picked row takes the other side's line, an unpicked one keeps its own. Whole blocks are the case
//! where every row of a block is picked, so the editor's block staging goes through here too.

use std::collections::BTreeSet;
use std::ops::Range;

use host::Host;

use super::{DiffRow, failure_text, git, side_by_side, write_index};
use crate::i18n::t;

/// Rows of a side-by-side diff (indices into `side_by_side`) picked for an operation.
pub type Picked = BTreeSet<usize>;

/// What an operation changed: changed rows taken and the blocks they belong to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Taken {
    pub lines: usize,
    pub blocks: usize,
}

/// The changed blocks of `rows`: maximal runs of `Changed` rows.
pub fn blocks(rows: &[DiffRow]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if matches!(rows[i], DiffRow::Changed(..)) {
            let start = i;
            while i < rows.len() && matches!(rows[i], DiffRow::Changed(..)) {
                i += 1;
            }
            out.push(start..i);
        } else {
            i += 1;
        }
    }
    out
}

/// How many changed rows of `picked` there are, and in how many blocks.
pub fn taken(rows: &[DiffRow], picked: &Picked) -> Taken {
    let changed = |i: &usize| matches!(rows.get(*i), Some(DiffRow::Changed(..)));
    let lines = picked.iter().filter(|i| changed(i)).count();
    let blocks = blocks(rows).into_iter().filter(|b| picked.range(b.clone()).next().is_some()).count();
    Taken { lines, blocks }
}

/// The rows picked by a selection of lines `lo..=hi` of the new (right) side, as the editor's gutter shows them.
/// `whole_blocks` (a cursor, no selection): every row of each block the lines touch, a deletion counting for the line
/// below it. Otherwise only the rows whose new line is selected; the lines a block deletes beyond its new lines go with
/// its last new line, and a block that only deletes goes with the line below it (where the gutter marks it).
pub fn rows_for_lines(rows: &[DiffRow], lo: usize, hi: usize, whole_blocks: bool) -> Picked {
    let mut picked = Picked::new();
    let mut right_before = 0; // new-side lines above the current row
    let mut i = 0;
    while i < rows.len() {
        match rows[i] {
            DiffRow::Same(..) => {
                right_before += 1;
                i += 1;
            }
            DiffRow::Changed(..) => {
                let start = i;
                while i < rows.len() && matches!(rows[i], DiffRow::Changed(..)) {
                    i += 1;
                }
                let block = start..i;
                let new: Vec<usize> = rows[block.clone()].iter().filter_map(|r| if let DiffRow::Changed(_, b) = r { *b } else { None }).collect();
                let sel = |l: usize| (lo..=hi).contains(&l);
                if whole_blocks {
                    let touched = if new.is_empty() { sel(right_before) } else { new.iter().any(|&l| sel(l)) };
                    if touched {
                        picked.extend(block.clone());
                    }
                } else {
                    // Deleted lines go with the last new line of the block, or with the line below a pure deletion.
                    let anchor = new.last().copied().unwrap_or(right_before);
                    for r in block.clone() {
                        let take = match rows[r] {
                            DiffRow::Changed(_, Some(b)) => sel(b),
                            _ => sel(anchor),
                        };
                        if take {
                            picked.insert(r);
                        }
                    }
                }
                right_before += new.len();
            }
        }
    }
    picked
}

/// The text that results from `rows`: the `keep` side's lines, except in picked changed rows, which take the other
/// side's line (or none). Lines keep their own terminators; with `eol` the taken lines are written with it instead. A
/// line without a terminator that ends up followed by another gets the kept side's dominant one.
pub fn compose(rows: &[DiffRow], left: &str, right: &str, keep_left: bool, picked: &Picked, eol: Option<&str>) -> String {
    let left: Vec<&str> = left.split_inclusive('\n').collect();
    let right: Vec<&str> = right.split_inclusive('\n').collect();
    let (kept, other) = if keep_left { (&left, &right) } else { (&right, &left) };
    let dominant = dominant_eol(&kept.concat());
    let mut out = String::new();
    let push = |out: &mut String, line: &str, rewrite: bool| {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(dominant);
        }
        match (rewrite, eol, line.strip_suffix('\n')) {
            (true, Some(eol), Some(body)) => {
                out.push_str(body.strip_suffix('\r').unwrap_or(body));
                out.push_str(eol);
            }
            _ => out.push_str(line),
        }
    };
    for (i, row) in rows.iter().enumerate() {
        let (a, b) = match *row {
            DiffRow::Same(a, b) => (Some(a), Some(b)),
            DiffRow::Changed(a, b) => (a, b),
        };
        let (k, o) = if keep_left { (a, b) } else { (b, a) };
        let take = matches!(row, DiffRow::Changed(..)) && picked.contains(&i);
        if take {
            if let Some(line) = o.and_then(|o| other.get(o)) {
                push(&mut out, line, true);
            }
        } else if let Some(line) = k.and_then(|k| kept.get(k)) {
            push(&mut out, line, false);
        }
    }
    out
}

/// The line ending most lines of `text` use (`\n` when there are none).
pub fn dominant_eol(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    if crlf > text.matches('\n').count() - crlf { "\r\n" } else { "\n" }
}

/// The editor's revert of picked rows (`rows` from `side_by_side(base, current)`): for each block with a picked row, the
/// line range of `current` it covers and its new text (picked rows back to `base`, the others as they are).
pub fn revert_parts(rows: &[DiffRow], base: &str, current: &str, picked: &Picked) -> Vec<(Range<usize>, String)> {
    let base: Vec<&str> = base.split_inclusive('\n').collect();
    let cur: Vec<&str> = current.split_inclusive('\n').collect();
    let mut parts = Vec::new();
    let mut right_before = 0;
    let mut i = 0;
    while i < rows.len() {
        if let DiffRow::Same(..) = rows[i] {
            right_before += 1;
            i += 1;
            continue;
        }
        let start = i;
        while i < rows.len() && matches!(rows[i], DiffRow::Changed(..)) {
            i += 1;
        }
        let new_lines = rows[start..i].iter().filter(|r| matches!(r, DiffRow::Changed(_, Some(_)))).count();
        if picked.range(start..i).next().is_some() {
            let mut text = String::new();
            for (r, row) in rows.iter().enumerate().take(i).skip(start) {
                let DiffRow::Changed(a, b) = *row else { continue };
                let line = if picked.contains(&r) { a.and_then(|a| base.get(a)) } else { b.and_then(|b| cur.get(b)) };
                if let Some(line) = line {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(line);
                }
            }
            // The block's last line had no terminator but more text follows it now: keep the following line apart.
            let followed = right_before + new_lines < cur.len();
            if followed && !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            parts.push((right_before..right_before + new_lines, text));
        }
        right_before += new_lines;
    }
    parts
}

/// A file's index content as Git stores it, checked for line staging. `None`: not in the index. Its BOM is kept apart.
struct IndexText {
    raw: String,
    bom: bool,
}

impl IndexText {
    /// The text compared with the editor and the diff view (no BOM, LF line endings).
    fn lf(&self) -> String {
        self.raw.replace("\r\n", "\n")
    }

    fn with_bom(&self, mut text: String) -> String {
        if self.bom {
            text.insert(0, '\u{feff}');
        }
        text
    }
}

/// The index entry of `rel`, refused when it is conflicted, a symlink or a submodule, not UTF-8 or uses CR-only lines.
fn read_index(host: &dyn Host, root: &str, rel: &str) -> Result<Option<IndexText>, String> {
    let entries = super::git_ok(host, root, &["ls-files", "-s", "--", rel])?;
    let mut exists = false;
    for line in entries.lines() {
        let mut f = line.split_whitespace();
        let (mode, _sha, stage) = (f.next().unwrap_or(""), f.next(), f.next().unwrap_or("0"));
        if stage != "0" {
            return Err(t("This file has merge conflicts: resolve them first (Source Control marks it resolved when staged)").into());
        }
        if mode == "120000" || mode == "160000" {
            return Err(t("Staging changes inside a symlink or submodule is not supported").into());
        }
        exists = true;
    }
    if !exists {
        return Ok(None);
    }
    let out = git(host, root, &["show", "--no-textconv", &format!(":{rel}")])?;
    if !out.ok() {
        return Err(failure_text(&out));
    }
    text_of(out.stdout).map(Some)
}

/// The file at `HEAD` (`None`: not there), with the same checks as the index.
fn read_head(host: &dyn Host, root: &str, rel: &str) -> Result<Option<IndexText>, String> {
    let spec = format!("HEAD:{rel}");
    let exists = git(host, root, &["cat-file", "-e", &spec]).is_ok_and(|o| o.ok());
    if !exists {
        return Ok(None);
    }
    let out = git(host, root, &["show", "--no-textconv", &spec])?;
    if !out.ok() {
        return Err(failure_text(&out));
    }
    text_of(out.stdout).map(Some)
}

fn text_of(bytes: Vec<u8>) -> Result<IndexText, String> {
    let raw = String::from_utf8(bytes).map_err(|_| t("Staging changes needs a UTF-8 file (this one is stored in another encoding)").to_string())?;
    if super::has_lone_cr(&raw) {
        return Err(t("Staging changes is not supported in files with old-Mac (CR only) line endings").into());
    }
    Ok(match raw.strip_prefix('\u{feff}') {
        Some(r) => IndexText { raw: r.to_string(), bom: true },
        None => IndexText { raw, bom: false },
    })
}

/// The diff view's texts no longer match the repository: nothing was written.
pub fn stale_diff_message() -> String {
    t("The file changed since this diff was shown. It has been reloaded: pick the lines again.").into()
}

/// Stage the rows `pick` chooses from the diff of the index (or nothing, for a new file) against `current`, the working
/// text with LF line endings (the editor's, or the file as the diff view shows it). With `expect_index` the index must
/// still be that text (the diff view's left side), else nothing is written. Unchanged lines keep their bytes; taken
/// lines use the index's dominant line ending, and a BOM stays.
pub fn stage_lines(
    host: &dyn Host,
    root: &str,
    rel: &str,
    current: &str,
    expect_index: Option<&str>,
    pick: impl FnOnce(&[DiffRow]) -> Picked,
) -> Result<Taken, String> {
    if super::has_lone_cr(current) {
        return Err(t("Staging changes is not supported in files with old-Mac (CR only) line endings").into());
    }
    let index = read_index(host, root, rel)?;
    let (raw, lf) = index.as_ref().map(|i| (i.raw.clone(), i.lf())).unwrap_or_default();
    if expect_index.is_some_and(|e| e != lf) {
        return Err(stale_diff_message());
    }
    let rows = side_by_side(&lf, current);
    let picked = pick(&rows);
    let taken = taken(&rows, &picked);
    if taken.lines == 0 {
        return Ok(taken);
    }
    let text = compose(&rows, &raw, current, true, &picked, Some(dominant_eol(&raw)));
    let text = match &index {
        Some(i) => i.with_bom(text),
        None => text,
    };
    write_index(host, root, rel, &text)?;
    Ok(taken)
}

/// Unstage the picked rows of the diff of `HEAD` against the index (the diff view's staged side, `expect_head` and
/// `expect_index` its two texts). Picked rows go back to `HEAD`'s lines. When that unstages every change of a file
/// `HEAD` does not have, the file leaves the index instead of staying there empty.
pub fn unstage_lines(host: &dyn Host, root: &str, rel: &str, expect_head: &str, expect_index: &str, picked: &Picked) -> Result<Taken, String> {
    let Some(index) = read_index(host, root, rel)? else {
        return Err(t("This change removes the file: unstage the whole file instead").into());
    };
    let head = read_head(host, root, rel)?;
    let head_lf = head.as_ref().map(IndexText::lf).unwrap_or_default();
    if head_lf != expect_head || index.lf() != expect_index {
        return Err(stale_diff_message());
    }
    let rows = side_by_side(&head_lf, &index.lf());
    let taken = taken(&rows, picked);
    if taken.lines == 0 {
        return Ok(taken);
    }
    let every = rows.iter().enumerate().all(|(i, r)| matches!(r, DiffRow::Same(..)) || picked.contains(&i));
    if head.is_none() && every {
        super::git_ok(host, root, &["update-index", "--force-remove", "--", rel])?;
        return Ok(taken);
    }
    let head_raw = head.as_ref().map(|h| h.raw.clone()).unwrap_or_default();
    let text = compose(&rows, &head_raw, &index.raw, false, picked, None);
    write_index(host, root, rel, &index.with_bom(text))?;
    Ok(taken)
}

#[cfg(test)]
#[path = "lines_tests.rs"]
mod tests;
