use super::*;
use crate::git::repo;

fn set(rows: &[usize]) -> Picked {
    rows.iter().copied().collect()
}

/// Rows picked by a selection of new lines, then the staged text.
fn stage_sel(base: &str, current: &str, lo: usize, hi: usize, whole: bool) -> String {
    let rows = side_by_side(base, current);
    compose(&rows, base, current, true, &rows_for_lines(&rows, lo, hi, whole), None)
}

#[test]
fn a_selection_stages_only_its_lines_and_a_cursor_the_whole_block() {
    let base = "a\nb\nc\n";
    let cur = "a\nx1\nx2\nx3\nc\n";
    // A cursor on x2: the whole block (b replaced by three lines).
    assert_eq!(stage_sel(base, cur, 2, 2, true), cur);
    // Only x2 selected: b stays, x2 is the line that replaces it (row 2 pairs no old line).
    let rows = side_by_side(base, cur);
    assert_eq!(rows[1], DiffRow::Changed(Some(1), Some(1)));
    assert_eq!(stage_sel(base, cur, 2, 2, false), "a\nb\nx2\nc\n");
    // x1 selected: it takes the place of b.
    assert_eq!(stage_sel(base, cur, 1, 1, false), "a\nx1\nc\n");
    // Nothing changed under the selection: the index stays.
    assert_eq!(stage_sel(base, cur, 0, 0, false), base);
    assert_eq!(stage_sel(base, cur, 4, 4, true), base);
}

#[test]
fn deleted_lines_go_with_the_last_new_line_or_the_line_below() {
    // b, c, d replaced by one line: c and d go with it.
    let base = "a\nb\nc\nd\ne\n";
    let cur = "a\nB\ne\n";
    assert_eq!(stage_sel(base, cur, 1, 1, false), cur);
    assert_eq!(stage_sel(base, cur, 2, 2, false), base, "the line below is not part of the block");
    // A pure deletion (c) goes with the line below it, where the gutter marks it.
    let cur = "a\nb\nd\ne\n";
    assert_eq!(stage_sel(base, cur, 2, 2, false), cur);
    assert_eq!(stage_sel(base, cur, 1, 1, false), base);
    assert_eq!(stage_sel(base, cur, 0, 3, false), cur);
    // Deleted at the end of the file: the empty last line of the editor holds it.
    let cur = "a\nb\nc\nd\n";
    assert_eq!(stage_sel(base, cur, 4, 4, false), cur);
}

#[test]
fn picked_rows_of_the_diff_view_come_from_the_other_side() {
    let (left, right) = ("1\n2\n3\n4\n", "1\nTWO\n3\nFOUR\nFIVE\n");
    let rows = side_by_side(left, right);
    let changed: Vec<usize> = blocks(&rows).into_iter().flatten().collect();
    assert_eq!(changed.len(), 3);
    // Stage the first block only.
    assert_eq!(compose(&rows, left, right, true, &set(&changed[..1]), None), "1\nTWO\n3\n4\n");
    // Unstage (keep the right side) the added FIVE only.
    assert_eq!(compose(&rows, left, right, false, &set(&changed[2..]), None), "1\nTWO\n3\nFOUR\n");
    assert_eq!(taken(&rows, &set(&changed)), Taken { lines: 3, blocks: 2 });
    // Unchanged rows in the pick change nothing.
    assert_eq!(compose(&rows, left, right, true, &set(&[0]), None), left);
}

#[test]
fn a_line_without_a_final_newline_never_merges_with_the_next() {
    // The new side lost its final newline on a changed last line; the old side has a line after it.
    let (base, cur) = ("a\nb\nc\n", "a\nX");
    let rows = side_by_side(base, cur);
    let first = blocks(&rows)[0].start;
    assert_eq!(compose(&rows, base, cur, true, &set(&[first]), None), "a\nX\nc\n");
    // The old side ends without a newline and a line is staged after it.
    let (base, cur) = ("a", "a\nb\n");
    let rows = side_by_side(base, cur);
    assert_eq!(compose(&rows, base, cur, true, &blocks(&rows).into_iter().flatten().collect(), None), "a\nb\n");
}

#[test]
fn taken_lines_use_the_index_line_ending_and_kept_ones_their_own() {
    let raw = "l1\r\nl2\r\nl3\n";
    let rows = side_by_side(&raw.replace("\r\n", "\n"), "l1\nL2\nl3\nl4\n");
    let all: Picked = blocks(&rows).into_iter().flatten().collect();
    assert_eq!(compose(&rows, raw, "l1\nL2\nl3\nl4\n", true, &all, Some(dominant_eol(raw))), "l1\r\nL2\r\nl3\nl4\r\n");
    assert_eq!(dominant_eol("a\nb\r\n"), "\n");
    assert_eq!(dominant_eol(""), "\n");
}

#[test]
fn revert_parts_put_back_only_the_picked_lines() {
    let base = "a\nb\nc\nd\n";
    let cur = "a\nB\nC\nd\nnew\n";
    let rows = side_by_side(base, cur);
    // Pick the row of C only: the block (lines 1..3) becomes B + c.
    let c_row = rows.iter().position(|r| *r == DiffRow::Changed(Some(2), Some(2))).unwrap();
    assert_eq!(revert_parts(&rows, base, cur, &set(&[c_row])), vec![(1..3, "B\nc\n".to_string())]);
    // The added last line: its block goes away.
    let new_row = rows.iter().position(|r| *r == DiffRow::Changed(None, Some(4))).unwrap();
    assert_eq!(revert_parts(&rows, base, cur, &set(&[new_row])), vec![(4..5, String::new())]);
    // A pure deletion comes back in an empty range.
    let rows = side_by_side(base, "a\nd\n");
    let all: Picked = blocks(&rows).into_iter().flatten().collect();
    assert_eq!(revert_parts(&rows, base, "a\nd\n", &all), vec![(1..1, "b\nc\n".to_string())]);
}

#[test]
fn revert_parts_keep_a_last_line_without_newline_apart_from_the_next() {
    // Old "x" (no newline) was the last line; now a line follows it.
    let (base, cur) = ("a\nx", "a\ny\nz\n");
    let rows = side_by_side(base, cur);
    let all: Picked = blocks(&rows).into_iter().flatten().collect();
    let parts = revert_parts(&rows, base, cur, &all);
    assert_eq!(parts, vec![(1..3, "x".to_string())]);
}

fn index(dir: &std::path::Path, name: &str) -> Option<String> {
    let o = crate::test_git::git().args(["show", &format!(":{name}")]).current_dir(dir).output().unwrap();
    o.status.success().then(|| String::from_utf8(o.stdout).unwrap())
}

#[test]
fn stage_lines_writes_the_picked_lines_and_refuses_a_stale_diff() {
    let Some((dir, run)) = repo("stagelines") else { return };
    let host = host::local();
    let root = dir.to_string_lossy().into_owned();
    std::fs::write(dir.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    run(&["add", "f.txt"]).unwrap();
    run(&["commit", "-q", "-m", "f"]).unwrap();
    let cur = "one\nTWO\nthree\nfour\n";
    // The diff view picks the added line only.
    let staged = stage_lines(&host, &root, "f.txt", cur, Some("one\ntwo\nthree\n"), |rows| {
        rows.iter().enumerate().filter(|(_, r)| matches!(r, DiffRow::Changed(None, Some(_)))).map(|(i, _)| i).collect()
    });
    assert_eq!(staged, Ok(Taken { lines: 1, blocks: 1 }));
    assert_eq!(index(&dir, "f.txt").as_deref(), Some("one\ntwo\nthree\nfour\n"));
    // The view still shows the old index: nothing is written.
    let err = stage_lines(&host, &root, "f.txt", cur, Some("one\ntwo\nthree\n"), |rows| (0..rows.len()).collect()).unwrap_err();
    assert_eq!(err, stale_diff_message());
    assert_eq!(index(&dir, "f.txt").as_deref(), Some("one\ntwo\nthree\nfour\n"));
    // Nothing picked: nothing written.
    assert_eq!(stage_lines(&host, &root, "f.txt", cur, None, |_| Picked::new()), Ok(Taken::default()));
    // An untracked file: the picked lines make it a new file in the index.
    assert_eq!(stage_lines(&host, &root, "n.txt", "keep\nlater\n", Some(""), |_| set(&[0])), Ok(Taken { lines: 1, blocks: 1 }));
    assert_eq!(index(&dir, "n.txt").as_deref(), Some("keep\n"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unstage_lines_puts_head_lines_back_and_drops_a_new_file_entirely() {
    let Some((dir, run)) = repo("unstagelines") else { return };
    let host = host::local();
    let root = dir.to_string_lossy().into_owned();
    std::fs::write(dir.join("f.txt"), "a\r\nb\r\nc\r\n").unwrap();
    run(&["add", "f.txt"]).unwrap();
    run(&["commit", "-q", "-m", "f"]).unwrap();
    std::fs::write(dir.join("f.txt"), "a\r\nB\r\nc\r\nd\r\n").unwrap();
    run(&["add", "f.txt"]).unwrap();
    let (head, idx) = ("a\nb\nc\n", "a\nB\nc\nd\n");
    let rows = side_by_side(head, idx);
    let b_row = rows.iter().position(|r| *r == DiffRow::Changed(Some(1), Some(1))).unwrap();
    assert_eq!(unstage_lines(&host, &root, "f.txt", head, idx, &set(&[b_row])), Ok(Taken { lines: 1, blocks: 1 }));
    // B is back to b (CRLF kept), d stays staged.
    assert_eq!(index(&dir, "f.txt").as_deref(), Some("a\r\nb\r\nc\r\nd\r\n"));
    // A second call from the same, now stale, view changes nothing.
    assert_eq!(unstage_lines(&host, &root, "f.txt", head, idx, &set(&[b_row])), Err(stale_diff_message()));
    // A new file whose every line is unstaged leaves the index.
    std::fs::write(dir.join("n.txt"), "x\ny\n").unwrap();
    run(&["add", "n.txt"]).unwrap();
    assert_eq!(unstage_lines(&host, &root, "n.txt", "", "x\ny\n", &set(&[0, 1])), Ok(Taken { lines: 2, blocks: 1 }));
    assert_eq!(index(&dir, "n.txt"), None);
    assert!(dir.join("n.txt").exists(), "the working file stays");
    // Part of a new file: the rest stays staged.
    run(&["add", "n.txt"]).unwrap();
    assert_eq!(unstage_lines(&host, &root, "n.txt", "", "x\ny\n", &set(&[1])), Ok(Taken { lines: 1, blocks: 1 }));
    assert_eq!(index(&dir, "n.txt").as_deref(), Some("x\n"));
    // A staged deletion is unstaged as a whole file.
    run(&["rm", "-q", "--cached", "-f", "f.txt"]).unwrap();
    let err = unstage_lines(&host, &root, "f.txt", head, "", &set(&[0])).unwrap_err();
    assert!(err.contains("whole file"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
