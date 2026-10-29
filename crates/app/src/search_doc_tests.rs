//! Search Editor document: result bounds, run states, rows with context, and the bounded saved form.

use super::*;

fn lm(line: u32, preview: &str, ranges: &[(u32, u32)]) -> proto::LineMatch {
    proto::LineMatch { line, preview: preview.into(), ranges: ranges.to_vec(), column: ranges.first().map_or(0, |r| r.0) }
}

fn doc() -> SearchDoc {
    SearchDoc::new(SearchSpec { pattern: "needle".into(), context: 1, ..Default::default() }, Provenance { host: "Local".into(), root: "/w".into() })
}

/// A small deterministic generator: the property tests below need variety, not a dependency.
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
fn results_are_sorted_merged_and_counted() {
    let mut d = doc();
    let g = d.begin();
    d.add("/w/b.rs".into(), vec![lm(3, "needle", &[(0, 6)])]);
    d.add("/w/a.rs".into(), vec![lm(1, "needle needle", &[(0, 6), (7, 13)])]);
    d.add("/w/b.rs".into(), vec![lm(9, "x needle", &[(2, 8)])]);
    d.finish(false, false);
    assert_eq!(d.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["/w/a.rs", "/w/b.rs"]);
    assert_eq!(d.files[1].matches.iter().map(|m| m.line).collect::<Vec<_>>(), [3, 9]);
    assert_eq!(d.match_count(), 4, "matches count ranges, as the agent does");
    assert_eq!(d.state, RunState::Complete);
    assert!(d.is_current_and_complete());
    assert_eq!(d.generation, g);
    assert_eq!(d.begin(), g + 1, "every run has its own generation");
    assert!(d.files.is_empty() && d.match_count() == 0);
}

#[test]
fn incomplete_runs_are_never_current_and_complete() {
    for (truncated, cancelled, want) in [(true, false, RunState::Truncated), (false, true, RunState::Cancelled)] {
        let mut d = doc();
        d.begin();
        d.add("/w/a".into(), vec![lm(1, "needle", &[(0, 6)])]);
        d.finish(truncated, cancelled);
        assert_eq!(d.state, want);
        assert!(!d.is_current_and_complete());
    }
    let mut d = doc();
    d.begin();
    d.fail("boom".into());
    assert!(!d.is_current_and_complete());
    let mut d = doc();
    d.begin();
    d.cancel();
    assert_eq!(d.state, RunState::Cancelled, "a stopped search keeps what arrived, marked cancelled");
    // Stale marks block it too, whatever the run state.
    let mut d = doc();
    d.begin();
    d.add("/w/a".into(), vec![lm(1, "needle", &[(0, 6)])]);
    d.finish(false, false);
    for why in [Stale::Restored, Stale::Replaced, Stale::Workspace] {
        let mut s = d.clone();
        s.mark_stale(why);
        assert_eq!(s.stale, Some(why));
        assert!(!s.is_current_and_complete());
    }
}

#[test]
fn huge_result_sets_stay_bounded_and_say_so() {
    let mut d = doc();
    d.begin();
    for i in 0..(MAX_FILES + 50) {
        let matches: Vec<_> = (0..5).map(|l| lm(l + 1, "needle", &[(0, 6)])).collect();
        d.add(format!("/w/f{i:06}"), matches);
    }
    d.finish(false, false);
    assert!(d.match_count() <= MAX_MATCHES, "{}", d.match_count());
    assert!(d.files.len() <= MAX_FILES);
    assert!(d.capped);
    assert_eq!(d.state, RunState::Truncated, "a cap hit here is a truncation even if the agent did not stop");
    assert!(!d.is_current_and_complete());
    // Rows stay proportional to what is kept, and context fetching is bounded on its own.
    assert!(d.rows().len() <= d.files.len() + d.match_count());
    assert!(d.context_targets().len() <= CONTEXT_MATCHES);
}

#[test]
fn rows_show_context_once_and_never_over_a_match() {
    let mut d = doc();
    d.begin();
    d.add("/w/a".into(), vec![lm(3, "needle 3", &[(0, 6)]), lm(5, "needle 5", &[(0, 6)])]);
    d.finish(false, false);
    d.set_context(0, 0, vec!["two".into()], vec!["four".into()]);
    d.set_context(0, 1, vec!["four".into()], vec!["six".into()]);
    let rows = d.rows();
    assert_eq!(
        rows,
        vec![Row::File(0), Row::Context(0, 0, 2), Row::Match(0, 0), Row::Context(0, 0, 4), Row::Match(0, 1), Row::Context(0, 1, 6)],
        "line 4 is shown once"
    );
    assert_eq!(d.context_text(0, 0, 2), "two");
    assert_eq!(d.context_text(0, 1, 6), "six");
    assert_eq!(d.context_text(9, 9, 9), "", "out of range reads as empty");
    d.files[0].collapsed = true;
    assert_eq!(d.rows(), vec![Row::File(0)]);
}

#[test]
fn long_lines_are_clipped_on_character_boundaries() {
    let mut d = doc();
    d.begin();
    let long = "ğ".repeat(LINE_CHARS * 2);
    d.add("/w/a".into(), vec![lm(1, &long, &[(0, 2), (2000, 2004)])]);
    let m = &d.files[0].matches[0];
    assert_eq!(m.preview.chars().count(), LINE_CHARS);
    assert_eq!(m.ranges, vec![(0, 2)], "ranges past the clipped preview are dropped");
}

#[test]
fn saved_state_round_trips_query_and_marks_results_stale() {
    let mut d = doc();
    d.spec.replacement = "pin".into();
    d.spec.include = "*.rs".into();
    d.begin();
    d.add("/w/a.rs".into(), vec![lm(2, "a needle", &[(2, 8)])]);
    d.finish(false, false);
    let text = save(&[&d]);
    let back = restore(&text);
    assert_eq!(back.len(), 1);
    let r = &back[0];
    assert_eq!(r.spec, d.spec);
    assert_eq!(r.provenance, d.provenance);
    assert_eq!(r.files[0].matches[0].preview, "a needle");
    assert_eq!(r.stale, Some(Stale::Restored));
    assert!(!r.is_current_and_complete(), "restored results never allow replacing");
    assert!(!r.partial);
}

#[test]
fn saved_state_is_bounded_and_partial_snapshots_say_so() {
    let mut d = doc();
    d.begin();
    for i in 0..50 {
        d.add(format!("/w/f{i:03}"), (1..=20).map(|l| lm(l, &"needle ".repeat(40), &[(0, 6)])).collect());
    }
    d.finish(false, false);
    let docs: Vec<SearchDoc> = (0..SAVED_MAX_TABS + 4).map(|_| d.clone()).collect();
    let refs: Vec<&SearchDoc> = docs.iter().collect();
    let text = save(&refs);
    assert!(text.len() <= SAVED_MAX_TABS * (SAVED_MAX_BYTES + 4096), "{}", text.len());
    let back = restore(&text);
    assert_eq!(back.len(), SAVED_MAX_TABS);
    for r in &back {
        let kept: usize = r.files.iter().map(|f| f.matches.len()).sum();
        assert!(kept <= SAVED_MAX_MATCHES);
        assert!(r.partial, "the snapshot kept only part of the results");
    }
}

#[test]
fn foreign_or_damaged_saved_text_restores_nothing_or_safe_values() {
    assert!(restore("").is_empty());
    assert!(restore("not json").is_empty());
    assert!(restore(r#"{"version":9,"docs":[]}"#).is_empty());
    assert!(restore(&" ".repeat(2 * 1024 * 1024)).is_empty(), "oversized text is ignored");
    // Bad ranges, a NUL in a path and an out-of-range context survive as safe values.
    let text = r#"{"version":1,"docs":[{"spec":{"pattern":"x","regex":false,"case_sensitive":false,"whole_word":false,"preserve_case":false,"include":"","exclude":"","context":99,"replacement":""},
        "provenance":{"host":"Local","root":"/w"},"state":"complete","capped":false,"omitted":0,
        "files":[{"path":"/w/a","matches":[{"line":0,"column":0,"preview":"ğx","ranges":[[1,2],[0,99],[3,1]]}]},{"path":"/w/\u0000b","matches":[{"line":1,"column":0,"preview":"x","ranges":[]}]}]}]}"#;
    let back = restore(text);
    assert_eq!(back.len(), 1);
    let r = &back[0];
    assert_eq!(r.spec.context, MAX_CONTEXT);
    assert_eq!(r.files.len(), 1, "the path with a NUL is dropped");
    assert_eq!(r.files[0].matches[0].line, 1);
    assert!(r.files[0].matches[0].ranges.is_empty(), "ranges inside a character or past the end are dropped");
}

#[test]
fn restoring_mutated_saved_text_never_panics() {
    let mut d = doc();
    d.begin();
    d.add("/w/a".into(), vec![lm(1, "ünïcode needle", &[(10, 16)])]);
    d.finish(true, false);
    let good = save(&[&d]).into_bytes();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..2000 {
        let mut b = good.clone();
        for _ in 0..1 + rng.below(4) {
            let i = rng.below(b.len());
            b[i] = (rng.next() & 0x7f) as u8;
        }
        if let Ok(s) = std::str::from_utf8(&b) {
            for r in restore(s) {
                assert_eq!(r.stale, Some(Stale::Restored));
                let _ = r.rows();
                let _ = r.as_text();
            }
        }
    }
}

#[test]
fn query_carries_options_and_extra_excludes() {
    let spec = SearchSpec { pattern: "Foo".into(), include: "*.rs, src/** ,".into(), exclude: "target".into(), ..Default::default() };
    let q = spec.query("/w", true, &["node_modules".into()], 1_000_000).unwrap();
    assert!(q.case_sensitive, "smart case: an uppercase letter makes it case-sensitive");
    assert_eq!(q.include, ["*.rs", "src/**"]);
    assert_eq!(q.exclude, ["target", "node_modules"]);
    assert_eq!(q.max_results as usize, MAX_MATCHES, "the agent never sends more than the editor keeps");
    assert!(SearchSpec::default().query("/w", true, &[], 10).is_none());
}

#[test]
fn text_export_lists_query_files_and_lines() {
    let mut d = doc();
    d.begin();
    d.add("/w/src/lib.rs".into(), vec![lm(7, "// needle 7  ", &[(3, 9)])]);
    d.finish(false, false);
    let t = d.as_text();
    assert!(t.starts_with("# Query: needle\n# Ignore Case\n"), "{t}");
    assert!(t.contains("src/lib.rs:\n  7: // needle 7\n"), "{t}");
}
