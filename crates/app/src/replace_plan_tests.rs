//! Replacement plans: what a preview proposes, byte for byte, and what it refuses.

use super::*;
use crate::encoding::Encoding;

fn query(pattern: &str, regex: bool, case_sensitive: bool, whole_word: bool) -> proto::SearchQuery {
    proto::SearchQuery { root: "/w".into(), pattern: pattern.into(), regex, case_sensitive, whole_word, include: vec![], exclude: vec![], max_results: 1000 }
}

fn pat(pattern: &str, regex: bool, replacement: &str) -> Pattern {
    Pattern::new(&query(pattern, regex, true, false), replacement, false).unwrap()
}

fn disk(bytes: &[u8], p: &Pattern) -> FilePlan {
    FilePlan::build("/w/f".into(), Source::Disk { bytes: bytes.to_vec(), hash: proto::content_hash(bytes) }, p, Encoding::Utf8, usize::MAX)
}

fn all(f: &FilePlan) -> Vec<bool> {
    vec![true; f.changes.len()]
}

fn out(f: &FilePlan) -> Vec<u8> {
    f.output_bytes(&all(f)).unwrap()
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn regex_captures_expand_like_the_find_bar() {
    let p = pat(r"(\w+)@(\w+)", true, "$2 at $1");
    let f = disk(b"mail ali@host and veli@ev\n", &p);
    assert_eq!(f.changes.iter().map(|c| (c.old.as_str(), c.new.as_str())).collect::<Vec<_>>(), [("ali@host", "host at ali"), ("veli@ev", "ev at veli")]);
    assert_eq!(out(&f), b"mail host at ali and ev at veli\n");
    // `$1_x` is group 1 then "_x"; `$&` is the whole match; `\n` is a line break.
    let p = pat(r"(\d+)", true, "$1_x[$&]");
    assert_eq!(out(&disk(b"a 12 b\n", &p)), b"a 12_x[12] b\n");
    // Named groups.
    let p = pat(r"(?P<k>\w+)=(?P<v>\w+)", true, "${v}=${k}");
    assert_eq!(out(&disk(b"a=b\n", &p)), b"b=a\n");
    // Plain mode takes `$1` literally.
    let p = pat("a", false, "$1");
    assert_eq!(out(&disk(b"a\n", &p)), b"$1\n");
}

#[test]
fn zero_width_matches_are_never_replacements() {
    for re in ["^", "$", r"\b", "(?:)", "x*"] {
        let p = pat(re, true, "!");
        let f = disk(b"axxb\ncd\n", &p);
        assert!(f.changes.iter().all(|c| !c.old.is_empty()), "{re}: {:?}", f.changes);
    }
    // `x*` replaces the non-empty run only.
    let p = pat("x*", true, "-");
    assert_eq!(out(&disk(b"axxb\n", &p)), b"a-b\n");
    // Anchors combined with text still work per line.
    let p = pat("^a", true, "A");
    assert_eq!(out(&disk(b"ab\nab\r\nba\n", &p)), b"Ab\nAb\r\nba\n");
    let p = pat("b$", true, "B");
    assert_eq!(out(&disk(b"ab\r\nab\n", &p)), b"aB\r\naB\n", "`$` matches before the \\r of a CRLF line");
}

#[test]
fn case_policy_and_turkish_i() {
    let q = query("istanbul", false, false, false);
    let p = Pattern::new(&q, "ankara", true).unwrap();
    let f = disk("İSTANBUL Istanbul istanbul\n".as_bytes(), &p);
    assert_eq!(f.changes.len(), 3, "case-insensitive i/İ/ı/I match each other");
    assert_eq!(String::from_utf8(out(&f)).unwrap(), "ANKARA Ankara ankara\n", "preserve case follows each match");
    // A replacement with i/ı under a Turkish match takes Turkish casing.
    let p = Pattern::new(&query("istanbul", false, false, false), "izmir", true).unwrap();
    assert_eq!(String::from_utf8(out(&disk("İSTANBUL Istanbul istanbul\n".as_bytes(), &p))).unwrap(), "İZMİR Izmir izmir\n");
    let q = query("old_name", false, false, false);
    let p = Pattern::new(&q, "new_name", true).unwrap();
    assert_eq!(out(&disk(b"Old_Name OLD_NAME\n", &p)), b"New_Name NEW_NAME\n");
    // Whole word.
    let p = Pattern::new(&query("cat", false, true, true), "dog", false).unwrap();
    assert_eq!(out(&disk(b"cat category cat.\n", &p)), b"dog category dog.\n");
}

#[test]
fn crlf_mixed_endings_and_inserted_breaks_are_preserved() {
    let p = pat("x", false, "y");
    assert_eq!(out(&disk(b"x\r\nx\nx\rx\r\n", &p)), b"y\r\ny\ny\ry\r\n");
    // Inserted line breaks take the file's first line ending.
    let p = pat("SPLIT", true, r"a\nb");
    assert_eq!(out(&disk(b"1 SPLIT 2\r\n3\n", &p)), b"1 a\r\nb 2\r\n3\n");
    assert_eq!(out(&disk(b"1 SPLIT 2\n", &p)), b"1 a\nb 2\n");
    // A CR-only file is one line for search; its endings stay.
    let p = pat("q", false, "Q");
    assert_eq!(out(&disk(b"q\rq\r", &p)), b"Q\rQ\r");
    // No trailing newline.
    assert_eq!(out(&disk(b"q", &p)), b"Q");
}

#[test]
fn byte_order_marks_and_utf16_round_trip() {
    let p = pat("^a", true, "b");
    let f = disk(b"\xEF\xBB\xBFa1\r\na2\r\n", &p);
    assert_eq!(f.changes.len(), 2, "the BOM is not part of the first line");
    assert_eq!(out(&f), b"\xEF\xBB\xBFb1\r\nb2\r\n");
    let utf16 = |be: bool, s: &str| {
        let mut v = if be { vec![0xFE, 0xFF] } else { vec![0xFF, 0xFE] };
        for u in s.encode_utf16() {
            v.extend(if be { u.to_be_bytes() } else { u.to_le_bytes() });
        }
        v
    };
    for be in [false, true] {
        let p = pat("ğ", false, "😀");
        let f = disk(&utf16(be, "aği\r\nğ\r\n"), &p);
        assert_eq!(f.encoding, if be { Encoding::Utf16Be } else { Encoding::Utf16Le });
        assert_eq!(out(&f), utf16(be, "a😀i\r\n😀\r\n"));
    }
    // Odd-length UTF-16 is not text.
    let p = pat("a", false, "b");
    assert_eq!(disk(&[0xFF, 0xFE, 0x61, 0x00, 0x62], &p).problem, Some(Problem::Binary));
}

#[test]
fn single_byte_encodings_round_trip_or_are_refused() {
    // "Işık\r\n" in Windows-1254 (ş=0xFE, ı=0xFD).
    let bytes = b"I\xfe\xfdk\r\n";
    let p = pat("k", false, "ğ");
    let f = FilePlan::build("/w/t".into(), Source::Disk { bytes: bytes.to_vec(), hash: 1 }, &p, Encoding::Windows1254, usize::MAX);
    assert_eq!(f.encoding, Encoding::Windows1254);
    assert_eq!(out(&f), b"I\xfe\xfd\xf0\r\n");
    // A replacement the encoding cannot hold makes the file unchangeable, explicitly.
    let p = pat("k", false, "😀");
    let f = FilePlan::build("/w/t".into(), Source::Disk { bytes: bytes.to_vec(), hash: 1 }, &p, Encoding::Windows1254, usize::MAX);
    assert!(matches!(f.problem, Some(Problem::Unencodable { ch: '😀', .. })), "{:?}", f.problem);
    assert!(f.output_bytes(&all(&f)).is_err());
    // Bytes undefined in Windows-1254 would not be written back the same: refused.
    let f = FilePlan::build("/w/t".into(), Source::Disk { bytes: b"k\x81\n".to_vec(), hash: 1 }, &pat("k", false, "K"), Encoding::Windows1254, usize::MAX);
    assert_eq!(f.problem, Some(Problem::Unsupported));
    // Latin-1 has no gaps.
    let f = FilePlan::build("/w/t".into(), Source::Disk { bytes: b"caf\xe9 k\n".to_vec(), hash: 1 }, &pat("k", false, "K"), Encoding::Iso8859_1, usize::MAX);
    assert_eq!(out(&f), b"caf\xe9 K\n");
}

#[test]
fn binary_and_unreadable_files_are_shown_but_never_changed() {
    let p = pat("a", false, "b");
    let f = disk(b"a\0a", &p);
    assert_eq!(f.problem, Some(Problem::Binary));
    assert!(f.changes.is_empty());
    let f = FilePlan::unreadable("/w/x".into(), "denied".into());
    assert!(matches!(f.problem, Some(Problem::Unreadable(_))));
    let plan = Plan { host: "h".into(), root: "/w".into(), generation: 1, files: vec![disk(b"a\0", &p), disk(b"a a\n", &p)] };
    let sel = Selection::all(&plan);
    assert_eq!(sel.counts(), (2, 1), "problem files are never selected");
    let mut s2 = sel.clone();
    s2.toggle_file(&plan, 0);
    assert_eq!(s2, sel, "and cannot be selected");
}

#[test]
fn a_byte_pattern_that_splits_a_character_is_skipped_not_a_panic() {
    let p = Pattern::new(&query(r"(?-u:\xC3)", true, true, false), "x", false);
    if let Ok(p) = p {
        let f = disk("é ü\n".as_bytes(), &p);
        assert!(f.changes.is_empty());
    }
}

#[test]
fn files_without_changes_keep_no_content_and_never_write() {
    let f = disk(&b"nothing here\n".repeat(10_000), &pat("absent", false, "x"));
    assert!(f.changes.is_empty());
    assert!(f.original().is_empty() && f.render(&[]).is_empty(), "neither the bytes nor the text are kept");
    assert_eq!(f.output_bytes(&[]), Err(Problem::NoChanges), "never an empty write");
}

#[test]
fn versions_record_where_the_text_came_from() {
    let p = pat("a", false, "b");
    let f = disk(b"a\n", &p);
    assert_eq!(f.version, Version::Disk { hash: proto::content_hash(b"a\n") });
    assert_eq!(*f.original(), b"a\n");
    let f = FilePlan::build(
        "/w/o".into(),
        Source::Buffer { text: "çöş a\n".into(), epoch: 7, version: 42, encoding: Encoding::Utf8 },
        &p,
        Encoding::Utf8,
        usize::MAX,
    );
    assert_eq!(f.version, Version::Buffer { epoch: 7, version: 42, hash: proto::content_hash("çöş a\n".as_bytes()) });
    // Buffer edits are character ranges (multi-byte letters before the match).
    assert_eq!(f.buffer_edits(&all(&f)), vec![(4..5, "b".to_string())]);
}

#[test]
fn selection_controls_exactly_which_changes_are_written() {
    let p = pat("x", false, "Y");
    let a = disk(b"x1 x2\nx3\n", &p);
    let b = disk(b"x4\n", &p);
    let plan = Plan { host: "h".into(), root: "/w".into(), generation: 1, files: vec![a, b] };
    let mut sel = Selection::all(&plan);
    assert_eq!(sel.counts(), (4, 2));
    sel.toggle_change(&plan, 0, 1);
    assert_eq!(sel.state(0), Tri::Some);
    assert_eq!(plan.files[0].output_bytes(sel.file(0)).unwrap(), b"Y1 x2\nY3\n");
    sel.toggle_file(&plan, 0);
    assert_eq!(sel.state(0), Tri::All, "a partly selected file toggles to all");
    sel.toggle_file(&plan, 0);
    assert_eq!(sel.state(0), Tri::None);
    assert_eq!(plan.files[0].output_bytes(sel.file(0)).unwrap(), b"x1 x2\nx3\n", "nothing selected: the bytes as read");
    let only = Selection::only(&plan, "/w/f");
    assert_eq!(only.counts(), (4, 2), "both test files share a path here");
    // Out-of-range toggles are ignored.
    sel.toggle_change(&plan, 9, 9);
    sel.toggle_file(&plan, 9);
}

#[test]
fn line_diff_shows_the_exact_line_before_and_after() {
    let p = pat("x", false, "YY");
    let f = disk(b"a x b x c\r\nx\n", &p);
    let sel = vec![true, false, true];
    let (old, new, r_old, r_new) = f.line_diff(0, &sel);
    assert_eq!((old.as_str(), new.as_str()), ("a x b x c", "a YY b x c"), "the \\r is not part of the shown line");
    assert_eq!((&old[r_old], &new[r_new]), ("x", "YY"));
    let (_, new, _, r_new) = f.line_diff(1, &sel);
    assert_eq!((new.as_str(), &new[r_new]), ("a YY b x c", "x"), "an unselected change shows unchanged");
    let (old, new, _, _) = f.line_diff(2, &sel);
    assert_eq!((old.as_str(), new.as_str()), ("x", "YY"));
    assert_eq!(f.line_diff(99, &sel), Default::default());
}

#[test]
fn plans_beyond_the_limits_are_refused() {
    let p = pat("a", false, "b");
    let src = |n: usize| (0..n).map(|i| (format!("/w/{i}"), Ok(Source::Disk { bytes: b"a a a\n".to_vec(), hash: 0 }))).collect::<Vec<_>>();
    let small = Limits { files: 3, changes: 7, bytes: 1000 };
    assert!(Plan::build("h".into(), "/w".into(), 1, src(2), &p, Encoding::Utf8, &small).is_ok());
    assert_eq!(Plan::build("h".into(), "/w".into(), 1, src(4), &p, Encoding::Utf8, &small).err(), Some(PlanError::TooLarge));
    assert_eq!(Plan::build("h".into(), "/w".into(), 1, src(3), &p, Encoding::Utf8, &small).err(), Some(PlanError::TooLarge), "9 changes > 7");
    let tiny = Limits { files: 10, changes: 100, bytes: 10 };
    assert_eq!(Plan::build("h".into(), "/w".into(), 1, src(2), &p, Encoding::Utf8, &tiny).err(), Some(PlanError::TooLarge), "12 bytes > 10");
    let with_error = vec![("/w/x".to_string(), Err("gone".to_string()))];
    let plan = Plan::build("h".into(), "/w".into(), 1, with_error, &p, Encoding::Utf8, &small).unwrap();
    assert!(matches!(plan.files[0].problem, Some(Problem::Unreadable(_))));
}

#[test]
fn pathological_patterns_and_very_long_lines_finish_quickly() {
    let line = format!("{}b\n", "a".repeat(200_000));
    let start = std::time::Instant::now();
    for re in [r"(a+)+$", r"(a|aa)*c", r"(a*)*b", r"(?:a?){50}a{50}"] {
        let p = pat(re, true, "z");
        let f = disk(line.as_bytes(), &p);
        if !f.changes.is_empty() {
            let _ = f.output_bytes(&all(&f)).unwrap();
        }
    }
    // One 4 MiB line: a few matches far apart.
    let big = format!("{}needle{}needle\n", "a".repeat(2 << 20), "b".repeat(2 << 20));
    let f = disk(big.as_bytes(), &pat("needle", false, "pin"));
    assert_eq!(f.changes.len(), 2);
    assert_eq!(out(&f), format!("{}pin{}pin\n", "a".repeat(2 << 20), "b".repeat(2 << 20)).into_bytes());
    // A file with more matches than a plan may hold stops counting early instead of building them all.
    let many = "ab".repeat(1 << 20);
    let sources = vec![("/w/m".to_string(), Ok(Source::Disk { bytes: many.into_bytes(), hash: 0 }))];
    assert_eq!(Plan::build("h".into(), "/w".into(), 1, sources, &pat("b", false, "c"), Encoding::Utf8, &LIMITS).err(), Some(PlanError::TooLarge));
    assert!(start.elapsed() < std::time::Duration::from_secs(30), "{:?}", start.elapsed());
}

/// A 5 MB minified single line with thousands of matches: each diff row costs a bounded window, not the whole line.
#[test]
fn diff_rows_of_a_huge_single_line_are_windowed() {
    let unit = format!("{}needle", "x".repeat(250));
    let line = unit.repeat(20_000);
    assert!(line.len() > 5_000_000);
    let f = disk(line.as_bytes(), &pat("needle", false, "pin"));
    assert_eq!(f.changes.len(), 20_000);
    let sel = all(&f);
    let start = std::time::Instant::now();
    for ch in (0..f.changes.len()).step_by(7) {
        let (old, new, r_old, r_new) = f.line_diff(ch, &sel);
        assert!(old.len() <= 2 * DIFF_WINDOW + 64 && new.len() <= 2 * DIFF_WINDOW + 64, "{} {}", old.len(), new.len());
        assert_eq!((&old[r_old], &new[r_new]), ("needle", "pin"));
    }
    // 2857 rows; the old copy-the-line approach would move ~28 GB here.
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
    // The middle row shows cut markers on both sides; the first one only at the end.
    let (old, _, _, _) = f.line_diff(10_000, &sel);
    assert!(old.starts_with('…') && old.ends_with('…'));
    let (old, _, _, _) = f.line_diff(0, &sel);
    assert!(!old.starts_with('…') && old.ends_with('…'));
}

/// Property: for random text with mixed endings, a BOM or not, and a random selection, the output equals the input with
/// exactly the selected matches replaced, and every byte outside them (line endings included) is unchanged.
#[test]
fn random_texts_keep_every_untouched_byte() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let words = ["foo", "bar", "ğüş", "x", " ", "\t", "fo", "o"];
    let ends = ["\n", "\r\n", "\r"];
    for _ in 0..400 {
        let mut text = String::new();
        let bom = rng.below(4) == 0;
        for _ in 0..rng.below(40) {
            text.push_str(words[rng.below(words.len())]);
            if rng.below(3) == 0 {
                text.push_str(ends[rng.below(ends.len())]);
            }
        }
        let mut bytes = Vec::new();
        if bom {
            bytes.extend_from_slice(b"\xEF\xBB\xBF");
        }
        bytes.extend_from_slice(text.as_bytes());
        let f = disk(&bytes, &pat("foo", false, "Q"));
        assert!(f.problem.is_none());
        if f.changes.is_empty() {
            assert_eq!(f.output_bytes(&[]), Err(Problem::NoChanges), "a file without matches has nothing to write");
            continue;
        }
        let sel: Vec<bool> = f.changes.iter().map(|_| rng.below(2) == 0).collect();
        let got = f.output_bytes(&sel).unwrap();
        // Expected: splice by hand from the original bytes.
        let mut want = Vec::new();
        let base = if bom { 3 } else { 0 };
        want.extend_from_slice(&bytes[..base]);
        let mut done = 0;
        for (c, s) in f.changes.iter().zip(&sel) {
            assert_eq!(&text[c.start..c.end], "foo");
            if *s {
                want.extend_from_slice(&text.as_bytes()[done..c.start]);
                want.extend_from_slice(b"Q");
                done = c.end;
            }
        }
        want.extend_from_slice(&text.as_bytes()[done..]);
        assert_eq!(got, want, "text {text:?}");
        // Nothing selected: exactly the original bytes.
        assert_eq!(f.output_bytes(&vec![false; f.changes.len()]).unwrap(), bytes);
        // Line ending sequence is the same.
        let endings = |b: &[u8]| b.iter().filter(|c| **c == b'\n' || **c == b'\r').copied().collect::<Vec<_>>();
        assert_eq!(endings(&got), endings(&bytes));
    }
}
