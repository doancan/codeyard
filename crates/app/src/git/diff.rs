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
    let diff = similar::TextDiff::configure().algorithm(similar::Algorithm::Myers).timeout(std::time::Duration::from_millis(200)).diff_lines(base, current);
    diff.ops().iter().map(|op| op.as_tag_tuple()).collect()
}

/// Inline (character-level) diffs within a changed line pair: byte ranges in the old and new line.
/// Returns empty if the two lines are only slightly similar or very long (piecemeal highlighting would be noise; the whole line
/// is already painted).
pub fn intraline(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    const MAX_LEN: usize = 2000;
    const MIN_RATIO: f32 = 0.4;
    if old.len() > MAX_LEN || new.len() > MAX_LEN {
        return Default::default();
    }
    let diff = similar::TextDiff::configure().algorithm(similar::Algorithm::Myers).timeout(std::time::Duration::from_millis(20)).diff_chars(old, new);
    if diff.ratio() < MIN_RATIO {
        return Default::default();
    }
    fn push(v: &mut Vec<Range<usize>>, r: Range<usize>) {
        match v.last_mut() {
            Some(last) if last.end == r.start => last.end = r.end,
            _ => v.push(r),
        }
    }
    let (mut o, mut n) = (0, 0);
    let (mut old_marks, mut new_marks) = (Vec::new(), Vec::new());
    for change in diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            similar::ChangeTag::Equal => {
                o += len;
                n += len;
            }
            similar::ChangeTag::Delete => {
                push(&mut old_marks, o..o + len);
                o += len;
            }
            similar::ChangeTag::Insert => {
                push(&mut new_marks, n..n + len);
                n += len;
            }
        }
    }
    (old_marks, new_marks)
}

/// Make the file's index content `content` (empty if absent); file mode is preserved. Does not touch
/// the working tree: only for "stage these blocks".
pub fn write_index(host: &dyn Host, root: &str, rel: &str, content: &str) -> Result<(), String> {
    let mode = git_ok(host, root, &["ls-files", "-s", "--", rel])
        .ok()
        .and_then(|o| o.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "100644".into());
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
