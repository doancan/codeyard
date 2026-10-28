use super::*;

fn matcher(value: Value) -> Matcher {
    Matcher::compile(&value, &mut Limits::default()).unwrap()
}

#[test]
fn task_matcher_multiline_loops_keep_prefix_data_without_carrying_prior_problem_fields() {
    let mut m = matcher(
        json!({"owner":"lint","pattern":[{"regexp":"^FILE (.+)$","file":1},{"regexp":r"^at (\d+):(\d+) (error|warning): (.*)$","line":1,"column":2,"severity":3,"message":4,"loop":true}]}),
    );
    assert!(m.line("FILE şablon a.ts").unwrap().problem.is_none());
    let first = m.line("at 12:3 warning: first").unwrap().problem.unwrap();
    let second = m.line("at 13:1 error: second").unwrap().problem.unwrap();
    assert_eq!((&first.file, first.line, first.severity), (&"şablon a.ts".into(), 12, 2));
    assert_eq!((&second.file, second.line, second.severity), (&first.file, 13, 1));
    m.line("FILE other.ts").unwrap();
    assert_eq!(m.line("at 1:0 error: next").unwrap().problem.unwrap().file, "other.ts");
}

#[test]
fn task_matcher_requires_consecutive_lines_and_restarts_on_a_new_first_line() {
    let mut m = matcher(json!({"pattern":[{"regexp":"^Error: (.*)$","message":1},{"regexp":"^context$"},{"regexp":r"^at (.*):(\d+)$","file":1,"line":2}]}));
    m.line("Error: discarded").unwrap();
    m.line("unrelated").unwrap();
    m.line("context").unwrap();
    assert!(m.line("at x.rs:2").unwrap().problem.is_none());
    m.line("Error: old").unwrap();
    m.line("Error: kept").unwrap();
    m.line("context").unwrap();
    assert_eq!(m.line("at x.rs:2").unwrap().problem.unwrap().message, "kept");
}

#[test]
fn task_matcher_watch_readiness_tracks_file_scopes_and_repeated_build_cycles() {
    let mut m =
        matcher(json!({"background":{"activeOnStart":true,"beginsPattern":{"regexp":"^BEGIN (.*)$","file":1},"endsPattern":{"regexp":"^END (.*)$","file":1}}}));
    assert!(!m.is_ready());
    m.line("BEGIN a").unwrap();
    m.line("BEGIN b").unwrap();
    assert!(!m.line("END a").unwrap().ready);
    assert!(!m.line("END unknown").unwrap().ready);
    assert!(m.line("END b").unwrap().ready);
    assert!(m.is_ready());
    assert!(m.line("BEGIN a").unwrap().began);
    assert!(!m.is_ready());
    assert!(m.line("END a").unwrap().ready);
    let mut only_end = matcher(json!({"background":{"endsPattern":"^READY$"}}));
    assert!(only_end.line("READY").unwrap().ready);
}

#[test]
fn task_matcher_builtins_and_overrides_preserve_source_ranges_severity_and_readiness() {
    let mut m = matcher(json!({"base":"$tsc-watch","source":"compiler","applyTo":"closedDocuments"}));
    let p = m.line("src/şablon.ts(5,8): error TS2322: wrong type").unwrap().problem.unwrap();
    assert_eq!((p.line, p.column, p.code.as_deref()), (5, Some(8), Some("TS2322")));
    assert_eq!(m.source, "compiler");
    assert_eq!(m.apply_to, ApplyTo::Closed);
    assert!(m.line("12:00:00 - Found 1 error. Watching for file changes.").unwrap().ready);
    let mut location = matcher(json!({"severity":"warning","pattern":{"regexp":r"^(.*)\((\d+,\d+,\d+,\d+)\): (.*)$","file":1,"location":2,"message":3}}));
    let p = location.line("x.ts(1,2,3,4): range").unwrap().problem.unwrap();
    assert_eq!((p.line, p.column, p.end_line, p.end_column, p.severity), (1, Some(2), Some(3), Some(4), 2));
}

#[test]
fn task_matcher_accepts_file_only_problems_and_ecmascript_lookarounds_and_backreferences() {
    let mut m = matcher(json!({"pattern":{"kind":"file","regexp":r"^(.*?): (?=duplicate)(duplicate)\s+\2$","file":1,"message":2}}));
    let p = m.line("C:/work/şablon.ts: duplicate duplicate").unwrap().problem.unwrap();
    assert_eq!(p.file, "C:/work/şablon.ts");
    assert_eq!(p.line, 1);
}

#[test]
fn task_matcher_rejects_invalid_captures_patterns_loops_and_excessive_configuration_before_running() {
    for value in [
        json!({"pattern":{"regexp":"(.*)","file":2,"message":1,"kind":"file"}}),
        json!({"pattern":{"regexp":"(","file":1,"message":1,"kind":"file"}}),
        json!({"pattern":{"regexp":"(.*)","file":1,"message":1}}),
        json!({"pattern":{"regexp":"(.*)","file":1,"message":1,"kind":"file","loop":true}}),
        json!({"background":{"endsPattern":17}}),
        json!("$unknown"),
    ] {
        assert!(Matcher::compile(&value, &mut Limits::default()).is_err());
    }
    let mut limits = Limits { patterns: 1, text: 4096 };
    assert!(Matcher::compile(&json!("$tsc"), &mut limits).is_ok());
    assert!(matches!(Matcher::compile(&json!("$tsc"), &mut limits), Err(Error::Limit)));
}

#[test]
fn task_matcher_bounds_long_output_and_expensive_backtracking() {
    let mut m = matcher(json!({"pattern":{"kind":"file","regexp":r"^(a|aa)+\1$","file":0,"message":0}}));
    let start = std::time::Instant::now();
    let result = m.line(&("a".repeat(128) + "!"));
    assert!(result.is_err() || result.unwrap().problem.is_none());
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(m.line(&"x".repeat(64 * 1024 + 1)), Err(Error::Limit));
}
