//! Line diff for the gutter and the diff view, and staging single blocks (through `lines`).

use std::ops::Range;

use host::Host;

use super::git_ok;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineMark {
    Added,
    Modified,
    /// There are deleted line(s) right above this line.
    DeletedAbove,
}

/// Mark for each line of the new text (mostly `None`).
pub fn line_marks(base: &str, current: &str) -> Vec<Option<LineMark>> {
    // Unchanged file (the most common case): skip the diff computation, no allocation.
    if base == current {
        return Vec::new();
    }
    let lines = current.split('\n').count();
    let mut marks = vec![None; lines];
    for (tag, _old, new) in diff_ops(base, current) {
        match tag {
            similar::DiffTag::Insert => new.clone().for_each(|i| set(&mut marks, i, LineMark::Added)),
            similar::DiffTag::Replace => new.clone().for_each(|i| set(&mut marks, i, LineMark::Modified)),
            similar::DiffTag::Delete => {
                let at = new.start.min(lines.saturating_sub(1));
                if marks.get(at).is_some_and(|m| m.is_none()) {
                    marks[at] = Some(LineMark::DeletedAbove);
                }
            }
            similar::DiffTag::Equal => {}
        }
    }
    marks
}

fn set(marks: &mut [Option<LineMark>], i: usize, m: LineMark) {
    if let Some(slot) = marks.get_mut(i) {
        *slot = Some(m);
    }
}

/// (kind, old lines, new lines). Lines are split on `\n`.
pub fn diff_ops(base: &str, current: &str) -> Vec<(similar::DiffTag, Range<usize>, Range<usize>)> {
    let diff = line_diff_config().diff_lines(base, current);
    diff.ops().iter().map(|op| op.as_tag_tuple()).collect()
}

/// The line differ every view of a file's changes uses (Myers, giving up on a hard pair after 200 ms).
pub fn line_diff_config() -> similar::TextDiffConfig {
    let mut config = similar::TextDiff::configure();
    config.algorithm(similar::Algorithm::Myers).timeout(std::time::Duration::from_millis(200));
    config
}

/// The lines of `text` as the diff counts them: split on `\n`, and the final newline does not start another line.
pub fn split_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// Inline diffs within a changed line pair: byte ranges in the old and new line. Words are compared whole (a renamed
/// identifier is one mark, not the letters the names differ in), and marks with only punctuation or spaces between them
/// join. Returns empty if the two lines are only slightly similar or very long (piecemeal highlighting would be noise;
/// the whole line is already painted).
pub fn intraline(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    const MAX_LEN: usize = 2000;
    const MIN_RATIO: f32 = 0.4;
    if old.len() > MAX_LEN || new.len() > MAX_LEN {
        return Default::default();
    }
    let (old_tokens, new_tokens) = (tokens(old), tokens(new));
    let diff =
        similar::TextDiff::configure().algorithm(similar::Algorithm::Myers).timeout(std::time::Duration::from_millis(20)).diff_slices(&old_tokens, &new_tokens);
    let ops = diff.ops();
    // Offsets of each token, and one past the last: token range → byte range.
    let starts =
        |toks: &[&str]| toks.iter().scan(0, |at, t| Some(std::mem::replace(at, *at + t.len()))).chain([toks.iter().map(|t| t.len()).sum()]).collect::<Vec<_>>();
    let (old_at, new_at) = (starts(&old_tokens), starts(&new_tokens));
    let mut same = 0;
    let (mut old_marks, mut new_marks) = (Vec::new(), Vec::new());
    for op in ops {
        let (tag, o, n) = op.as_tag_tuple();
        match tag {
            similar::DiffTag::Equal => same += old_at[o.end] - old_at[o.start],
            _ => {
                join(&mut old_marks, old_at[o.start]..old_at[o.end], old);
                join(&mut new_marks, new_at[n.start]..new_at[n.end], new);
            }
        }
    }
    if (2 * same) as f32 / (old.len() + new.len()).max(1) as f32 > MIN_RATIO { (old_marks, new_marks) } else { Default::default() }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Words (letters, digits, `_`), runs of spaces, and every other character alone.
fn tokens(line: &str) -> Vec<&str> {
    // 0: a character that stands alone; equal non-zero classes run together.
    let class = |c: char| {
        if is_word(c) {
            1
        } else if c.is_whitespace() {
            2
        } else {
            0
        }
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = line.char_indices().map(|(i, c)| (i, class(c))).peekable();
    while let Some((_, k)) = chars.next() {
        let next = chars.peek().copied();
        if k == 0 || next.is_none_or(|(_, n)| n != k) {
            let end = next.map_or(line.len(), |(j, _)| j);
            out.push(&line[start..end]);
            start = end;
        }
    }
    out
}

/// Adds a mark, joined to the previous one when only a little punctuation or space separates them.
fn join(marks: &mut Vec<Range<usize>>, r: Range<usize>, line: &str) {
    const GAP: usize = 3;
    if r.is_empty() {
        return;
    }
    match marks.last_mut() {
        Some(last) if r.start - last.end <= GAP && !line[last.end..r.start].chars().any(is_word) => last.end = r.end,
        _ => marks.push(r),
    }
}

/// Make the file's index content `content` (empty if absent); file mode is preserved. Does not touch
/// the working tree: only for "stage these blocks".
pub fn write_index(host: &dyn Host, root: &str, rel: &str, content: &str) -> Result<(), String> {
    let entry = git_ok(host, root, &["--literal-pathspecs", "ls-files", "-s", "--", rel])?;
    let mode = entry.split_whitespace().next().unwrap_or("100644");
    let out = host.exec("git", &["hash-object", "-w", "--stdin"], Some(root), Some(content.as_bytes().to_vec())).map_err(|e| e.to_string())?;
    if !out.ok() {
        return Err(out.stderr_str());
    }
    let sha = out.stdout_str().trim().to_string();
    let info = format!("{mode},{sha},{rel}");
    git_ok(host, root, &["update-index", "--add", "--cacheinfo", &info]).map(|_| ())
}

/// Whether any line ends in a bare `\r` (old Mac); diff and rope line counting would disagree.
pub fn has_lone_cr(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().any(|(i, &c)| c == b'\r' && b.get(i + 1) != Some(&b'\n'))
}

/// Side-by-side diff row: left (old) and right (new) line numbers, 0-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRow {
    Same(usize, usize),
    /// Side by side in a changed block; one side may be missing.
    Changed(Option<usize>, Option<usize>),
}

/// Inline (single column) diff row: in a changed block, deletions first, then additions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineRow {
    Same(usize, usize),
    Removed(usize),
    Added(usize),
}

pub fn inline_rows(rows: &[DiffRow]) -> Vec<InlineRow> {
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        match rows[i] {
            DiffRow::Same(a, b) => {
                out.push(InlineRow::Same(a, b));
                i += 1;
            }
            DiffRow::Changed(..) => {
                let start = i;
                while i < rows.len() && matches!(rows[i], DiffRow::Changed(..)) {
                    i += 1;
                }
                out.extend(rows[start..i].iter().filter_map(|r| if let DiffRow::Changed(Some(a), _) = r { Some(InlineRow::Removed(*a)) } else { None }));
                out.extend(rows[start..i].iter().filter_map(|r| if let DiffRow::Changed(_, Some(b)) = r { Some(InlineRow::Added(*b)) } else { None }));
            }
        }
    }
    out
}

pub fn side_by_side(base: &str, current: &str) -> Vec<DiffRow> {
    rows_from_ops(diff_ops(base, current))
}

/// The side-by-side rows of a line diff's operations (`diff_ops`).
pub fn rows_from_ops(ops: impl IntoIterator<Item = (similar::DiffTag, Range<usize>, Range<usize>)>) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for (tag, old, new) in ops {
        match tag {
            similar::DiffTag::Equal => rows.extend(old.zip(new).map(|(a, b)| DiffRow::Same(a, b))),
            _ => {
                let n = old.len().max(new.len());
                for k in 0..n {
                    let a = (k < old.len()).then(|| old.start + k);
                    let b = (k < new.len()).then(|| new.start + k);
                    rows.push(DiffRow::Changed(a, b));
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{has_unmerged, merge, merge_abort, repo};

    #[test]
    fn line_marks_from_diff() {
        let base = "a\nb\nc\nd\n";
        assert!(line_marks(base, base).is_empty(), "same content: no marks, no diff");
        let m = line_marks(base, "a\nB\nc\nyeni\nd\n");
        assert_eq!(m[1], Some(LineMark::Modified));
        assert_eq!(m[3], Some(LineMark::Added));
        assert_eq!((m[0], m[2], m[4]), (None, None, None));
        let m = line_marks(base, "a\nd\n");
        assert_eq!(m[1], Some(LineMark::DeletedAbove), "b and c deleted, above d");
    }

    #[test]
    fn side_by_side_aligns_rows() {
        let rows = side_by_side("a\nb\nc\n", "a\nB\nx\nc\n");
        assert_eq!(rows[0], DiffRow::Same(0, 0));
        assert_eq!(rows[1], DiffRow::Changed(Some(1), Some(1)));
        assert_eq!(rows[2], DiffRow::Changed(None, Some(2)));
        assert_eq!(rows[3], DiffRow::Same(2, 3));
    }

    fn nums(n: usize, change: &[(usize, &str)]) -> String {
        (1..=n).map(|i| format!("{}\n", change.iter().find(|(k, _)| *k == i).map_or(format!("l{i}"), |(_, t)| t.to_string()))).collect()
    }

    /// The editor's block staging: the blocks touching lines `lo..=hi`, as the number of blocks staged.
    fn stage_hunks(host: &dyn Host, root: &str, rel: &str, current: &str, lo: usize, hi: usize) -> Result<usize, String> {
        crate::git::stage_lines(host, root, rel, current, None, |rows| crate::git::rows_for_lines(rows, lo, hi, true)).map(|t| t.blocks)
    }

    #[test]
    fn blocks_pick_stage_and_revert_by_line() {
        use crate::git::{blocks, compose, revert_parts, rows_for_lines};
        let base = nums(12, &[]);
        // Line 3 changed, a line was added after line 8, line 11 was deleted.
        let mut cur: Vec<String> = base.split_inclusive('\n').map(str::to_string).collect();
        cur[2] = "L3\n".into();
        cur.insert(8, "yeni\n".into());
        cur.remove(11);
        let cur = cur.concat();
        let rows = side_by_side(&base, &cur);
        assert_eq!(blocks(&rows).len(), 3);
        // Only the block at line 3 (0-based 2).
        let first = rows_for_lines(&rows, 2, 2, true);
        assert_eq!(compose(&rows, &base, &cur, true, &first, None), nums(12, &[(3, "L3")]));
        // All three blocks: index = working text.
        let all = blocks(&rows).into_iter().flatten().collect();
        assert_eq!(compose(&rows, &base, &cur, true, &all, None), cur);
        // Revert: only the added line goes away.
        let added = rows_for_lines(&rows, 8, 8, true);
        assert_eq!(revert_parts(&rows, &base, &cur, &added), vec![(8..9, String::new())]);
        // Untouched lines: nothing picked.
        assert!(rows_for_lines(&rows, 5, 6, true).is_empty());
    }

    #[test]
    fn a_missing_final_newline_and_an_empty_base_stage_whole() {
        use crate::git::{blocks, compose};
        for (base, cur) in [("", "a\nb"), ("a\nb", "a\nb\nc")] {
            let rows = side_by_side(base, cur);
            assert_eq!(compose(&rows, base, cur, true, &blocks(&rows).into_iter().flatten().collect(), None), cur);
        }
    }

    #[test]
    fn stage_hunks_writes_only_the_chosen_block_to_the_index() {
        let dir = std::env::temp_dir().join(format!("codeyard-hunk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| crate::test_git::git().args(args).current_dir(&dir).output().ok().filter(|o| o.status.success());
        if run(&["init", "-q"]).is_none() {
            return;
        }
        let base = nums(10, &[]);
        std::fs::write(dir.join("f.txt"), &base).unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "x"]).unwrap();
        let cur = nums(10, &[(2, "B"), (9, "I")]);
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        assert_eq!(stage_hunks(&host, &root, "f.txt", &cur, 1, 1), Ok(1));
        let staged = run(&["show", ":f.txt"]).unwrap().stdout;
        assert_eq!(String::from_utf8(staged).unwrap(), nums(10, &[(2, "B")]));
        // Second block afterwards: the index is now the full working text.
        assert_eq!(stage_hunks(&host, &root, "f.txt", &cur, 8, 8), Ok(1));
        assert_eq!(String::from_utf8(run(&["show", ":f.txt"]).unwrap().stdout).unwrap(), cur);
        // No changes left.
        assert_eq!(stage_hunks(&host, &root, "f.txt", &cur, 0, 9), Ok(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inline_rows_put_removed_before_added_per_block() {
        let rows = side_by_side("a\nb\nc\nd\n", "a\nB\nB2\nd\n");
        assert_eq!(
            inline_rows(&rows),
            vec![InlineRow::Same(0, 0), InlineRow::Removed(1), InlineRow::Removed(2), InlineRow::Added(1), InlineRow::Added(2), InlineRow::Same(3, 3)]
        );
        assert!(inline_rows(&side_by_side("x\n", "x\n")).iter().all(|r| matches!(r, InlineRow::Same(..))));
        assert_eq!(inline_rows(&side_by_side("", "n\n")), vec![InlineRow::Added(0)]);
    }

    #[test]
    fn intraline_marks_only_the_changed_characters() {
        let (o, n) = intraline("let total = a + b;", "let total = a - b;");
        assert_eq!((o, n), (vec![14..15], vec![14..15]));
        // Addition: mark only on the new line.
        let (o, n) = intraline("foo(a)", "foo(a, b)");
        assert!(o.is_empty(), "{o:?}");
        assert_eq!(n, vec![5..8]);
        // Very different lines: no mark (the whole line is already painted).
        assert_eq!(intraline("alpha beta gamma", "0123456789 xyz!!"), Default::default());
        // A very long line is skipped; with multibyte characters the range stays on a character boundary.
        assert_eq!(intraline(&"a".repeat(3000), &"b".repeat(3000)), Default::default());
        let (_, n) = intraline("ğüş x", "ğüş y");
        assert_eq!(n, vec![7..8]);
    }

    #[test]
    fn intraline_marks_whole_words_and_joins_close_ones() {
        // A renamed word is marked whole, not as the scattered letters the two names happen to differ in.
        assert_eq!(intraline("let count = items.len();", "let amount = items.len();"), (vec![4..9], vec![4..10]));
        assert_eq!(intraline("const değer = 1;", "const sayı = 1;"), (vec![6..12], vec![6..11]));
        // Two changed words with only a comma and a space between them read as one change.
        assert_eq!(intraline("foo(bar, baz)", "foo(qux, quux)"), (vec![4..12], vec![4..13]));
        // A real word in between keeps them apart.
        assert_eq!(intraline("a(bar, keep, baz)", "a(qux, keep, quux)"), (vec![2..5, 13..16], vec![2..5, 13..17]));
        // Lines that share only their spaces are not marked piecemeal.
        assert_eq!(intraline("alpha beta", "gamma delta"), Default::default());
    }

    #[test]
    fn stage_hunks_keeps_bytes_and_refuses_unsafe_files() {
        let Some((dir, run)) = repo("stagesafe") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        let commit = |name: &str, bytes: &[u8]| {
            std::fs::write(dir.join(name), bytes).unwrap();
            run(&["add", name]).unwrap();
            run(&["commit", "-q", "-m", name]).unwrap();
        };
        let index = |name: &str| {
            let o = crate::test_git::git().args(["show", &format!(":{name}")]).current_dir(&dir).output().unwrap();
            o.stdout
        };
        // Mixed line endings: unchanged line stays as is, the new line uses the dominant (CRLF) style.
        commit("mixed.txt", b"l1\r\nl2\r\nl3\n");
        assert_eq!(stage_hunks(&host, &root, "mixed.txt", "l1\nL2\nl3\n", 1, 1), Ok(1));
        assert_eq!(index("mixed.txt"), b"l1\r\nL2\r\nl3\n");
        // The BOM is preserved; the working text has no BOM (the editor strips the BOM).
        commit("bom.txt", b"\xef\xbb\xbfa\nb\nc\n");
        assert_eq!(stage_hunks(&host, &root, "bom.txt", "a\nb\nC\n", 2, 2), Ok(1));
        assert_eq!(index("bom.txt"), b"\xef\xbb\xbfa\nb\nC\n");
        // Non-UTF-8 file: rejected, index unchanged.
        commit("latin.txt", b"caf\xe9\nx\n");
        let err = stage_hunks(&host, &root, "latin.txt", "café\ny\n", 1, 1).unwrap_err();
        assert!(err.contains("UTF-8"), "{err}");
        assert_eq!(index("latin.txt"), b"caf\xe9\nx\n");
        // CR-only line endings: rejected.
        commit("cr.txt", b"a\rb\nl2\nl3\n");
        assert!(stage_hunks(&host, &root, "cr.txt", "a\rb\nl2\nX\nl3\n", 2, 2).unwrap_err().contains("CR"));
        // Symlink: rejected.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("mixed.txt", dir.join("link")).unwrap();
            run(&["add", "link"]).unwrap();
            run(&["commit", "-q", "-m", "link"]).unwrap();
            assert!(stage_hunks(&host, &root, "link", "x\n", 0, 0).unwrap_err().contains("symlink"));
        }
        // Conflicted file: not written to stage 0 as "resolved".
        run(&["checkout", "-q", "-b", "c1"]).unwrap();
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        run(&["commit", "-qam", "one"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "two"]).unwrap();
        assert_eq!(merge(&host, &root, "c1"), Ok(true));
        let err = stage_hunks(&host, &root, "f.txt", "<<<<<<< HEAD\ntwo\n=======\none\n>>>>>>> c1\n", 0, 5).unwrap_err();
        assert!(err.contains("conflict"), "{err}");
        assert!(has_unmerged(&host, &root), "conflict is still unresolved");
        merge_abort(&host, &root).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
