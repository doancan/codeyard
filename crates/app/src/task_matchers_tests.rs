//! Per-task problem matchers against output captured from real tools (`testdata/matchers`: clang, cargo, Python), fed
//! the way a task's output arrives: in chunks, with LF or CRLF line ends, on Unix and Windows paths.

use task_engine::model::Platform;

use super::*;

const CLANG: &str = include_str!("../testdata/matchers/clang-errors.txt");
const CARGO: &str = include_str!("../testdata/matchers/cargo-errors.txt");
const PYTHON: &str = include_str!("../testdata/matchers/python-traceback.txt");

fn file_for(matcher: &str, background: bool) -> String {
    let flag = if background { r#""isBackground": true,"# } else { "" };
    format!(r#"{{ "version": "2.0.0", "tasks": [ {{ "label": "t", "command": "x", {flag} "problemMatcher": {matcher} }} ] }}"#)
}

/// Run a one-task file and feed `output` in chunks of `chunk` bytes; the problems it found.
fn problems_of(matcher: &str, output: &str, chunk: usize) -> Vec<Problem> {
    let mut run = Run::new(1, prepare(&file_for(matcher, false), Platform::Linux, "t").unwrap()).unwrap();
    let node = run.prepared.plan.root;
    run.advance();
    run.started(node, 1);
    for piece in output.as_bytes().chunks(chunk) {
        run.output(node, piece);
    }
    run.exited(node, Some(1));
    run.problems().into_iter().map(|(_, p)| p.clone()).collect()
}

fn summary(p: &[Problem]) -> Vec<(String, u32, Option<u32>, u8, String)> {
    p.iter().map(|p| (p.file.clone(), p.line, p.column, p.severity, p.message.clone())).collect()
}

#[test]
fn clang_output_with_a_turkish_file_name_fills_the_problems() {
    let found = problems_of(r#""$gcc""#, CLANG, 4096);
    let s = summary(&found);
    assert_eq!(s.len(), 3, "{s:?}");
    assert_eq!((s[0].0.as_str(), s[0].1, s[0].2, s[0].3), ("şablon.c", 4, Some(3), 1));
    assert!(s[0].4.starts_with("call to undeclared function 'undefined_fn'"), "{}", s[0].4);
    assert_eq!((s[2].1, s[2].2, s[2].3), (3, Some(7), 2), "the warning keeps its severity");
}

#[test]
fn the_same_result_for_crlf_output_and_for_any_chunking() {
    let whole = summary(&problems_of(r#""$gcc""#, CLANG, 1 << 20));
    let crlf = CLANG.replace('\n', "\r\n");
    for chunk in [1, 3, 7, 64, 1 << 20] {
        assert_eq!(summary(&problems_of(r#""$gcc""#, &crlf, chunk)), whole, "CRLF in chunks of {chunk}");
        assert_eq!(summary(&problems_of(r#""$gcc""#, CLANG, chunk)), whole, "LF in chunks of {chunk}");
    }
}

#[test]
fn rustc_output_is_read_as_two_line_blocks() {
    let found = problems_of(r#""$rustc""#, CARGO, 4096);
    let s = summary(&found);
    assert_eq!(s.len(), 2, "{s:?}");
    assert_eq!((s[0].0.as_str(), s[0].1, s[0].2, s[0].3), ("src/main.rs", 3, Some(20), 1));
    assert_eq!(s[0].4, "cannot find value `missing` in this scope");
    assert_eq!(found[0].code.as_deref(), Some("E0425"));
    assert_eq!((s[1].1, s[1].2), (4, Some(18)));
}

#[test]
fn an_inline_multi_line_matcher_reads_a_python_traceback() {
    let matcher = r#"{ "owner": "python", "severity": "error", "fileLocation": "absolute", "pattern": [
        { "regexp": "^\\s+File \"(.*)\", line (\\d+), in .*$", "file": 1, "line": 2 },
        { "regexp": "^(\\w*Error: .*)$", "message": 1 } ] }"#;
    let s = summary(&problems_of(matcher, PYTHON, 4096));
    assert_eq!(s, vec![("<string>".to_string(), 3, None, 1, "ValueError: x".to_string())]);
}

#[test]
fn windows_paths_and_columns_in_parentheses_are_read() {
    let out = "C:\\work\\şablon\\a.cs(10,5): error CS1002: ; expected\r\nD:\\x\\b.cs(7): warning CS0168: unused\r\n";
    let s = summary(&problems_of(r#""$msCompile""#, out, 5));
    assert_eq!(s[0], ("C:\\work\\şablon\\a.cs".to_string(), 10, Some(5), 1, "; expected".to_string()));
    assert_eq!((s[1].1, s[1].2, s[1].3), (7, None, 2));
}

#[test]
fn a_task_keeps_at_most_a_thousand_problems() {
    let line = |i: usize| format!("src/f{i}.c:{}:1: error: boom\n", i + 1);
    let output: String = (0..MAX_PROBLEMS + 500).map(line).collect();
    assert_eq!(problems_of(r#""$gcc""#, &output, 1 << 20).len(), MAX_PROBLEMS);
}

#[test]
fn reaching_the_cap_is_reported_once_per_run() {
    let mut run = Run::new(1, prepare(&file_for(r#""$gcc""#, false), Platform::Linux, "t").unwrap()).unwrap();
    let node = run.prepared.plan.root;
    run.advance();
    run.started(node, 1);
    let batch = |from: usize, n: usize| (from..from + n).map(|i| format!("src/f{i}.c:{}:1: error: boom\n", i + 1)).collect::<String>();
    assert!(run.output(node, batch(0, MAX_PROBLEMS).as_bytes()).capped.is_empty(), "exactly the cap is not over it");
    assert_eq!(run.output(node, batch(MAX_PROBLEMS, 1).as_bytes()).capped, vec![node]);
    assert!(run.output(node, batch(MAX_PROBLEMS + 1, 50).as_bytes()).capped.is_empty(), "said once");
    assert_eq!(run.problems().len(), MAX_PROBLEMS);
}

#[test]
fn a_watch_cycle_clears_the_previous_problems_and_a_re_run_starts_empty() {
    let matcher = r#""$rustc-watch""#;
    let mut run = Run::new(1, prepare(&file_for(matcher, true), Platform::Linux, "t").unwrap()).unwrap();
    let node = run.prepared.plan.root;
    run.advance();
    run.started(node, 1);
    assert!(run.output(node, b"[Running 'cargo check']\nerror: first\n --> src/a.rs:1:2\n").problems);
    assert_eq!(run.problems().len(), 1);
    let e = run.output(node, b"[Finished running. Exit status: 101]\n");
    assert!(!e.problems);
    assert_eq!(run.problems().len(), 1, "the problems stay while the check is idle");
    assert!(run.output(node, b"[Running 'cargo check']\n").problems, "the next cycle clears them");
    assert!(run.problems().is_empty());
    // A new run is a new `Run`: nothing of the old one is left in it.
    let again = Run::new(2, prepare(&file_for(matcher, true), Platform::Linux, "t").unwrap()).unwrap();
    assert!(again.problems().is_empty());
}

#[test]
fn a_line_the_matcher_cannot_read_is_reported_once_and_the_rest_still_work() {
    let mut run = Run::new(1, prepare(&file_for(r#""$gcc""#, false), Platform::Linux, "t").unwrap()).unwrap();
    let node = run.prepared.plan.root;
    run.advance();
    run.started(node, 1);
    let huge = format!("{}\n", "x".repeat(70_000));
    let first = run.output(node, huge.as_bytes());
    assert_eq!(first.unreadable, vec![node]);
    assert!(run.output(node, huge.as_bytes()).unreadable.is_empty(), "said once per run");
    assert!(run.output(node, b"a.c:1:1: error: still read\n").problems);
}

#[test]
fn a_pathological_pattern_gives_up_instead_of_hanging() {
    let matcher = r#"{ "pattern": { "regexp": "^(a|aa)+\\1$", "file": 1, "line": 1, "message": 1 } }"#;
    let mut run = Run::new(1, prepare(&file_for(matcher, false), Platform::Linux, "t").unwrap()).unwrap();
    let node = run.prepared.plan.root;
    run.advance();
    run.started(node, 1);
    let started = std::time::Instant::now();
    let e = run.output(node, format!("{}!\n", "a".repeat(200)).as_bytes());
    assert_eq!(e.unreadable, vec![node]);
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "bounded by the engine's step limit");
}

#[test]
fn a_bad_regular_expression_is_an_error_message_not_a_panic() {
    let text = file_for(r#"{ "pattern": { "regexp": "(unclosed", "file": 1, "line": 1, "message": 1 } }"#, false);
    let message = Run::new(1, prepare(&text, Platform::Linux, "t").unwrap()).err().expect("refused before anything runs");
    assert!(message.contains("Task “t” has an invalid problem matcher"), "{message}");
}

#[test]
fn builtin_names_from_vs_code_are_known() {
    for name in ["$gcc", "$go", "$tsc", "$tsc-watch", "$eslint-stylish", "$eslint-compact", "$msCompile", "$rustc", "$rustc-watch", "$jshint", "$lessCompile"] {
        let text = file_for(&format!("\"{name}\""), false);
        assert!(Run::new(1, prepare(&text, Platform::Linux, "t").unwrap()).is_ok(), "{name}");
    }
    let jshint = problems_of(r#""$jshint""#, "src/a.js: line 4, col 9, Missing semicolon.\n", 64);
    assert_eq!((jshint[0].line, jshint[0].column, jshint[0].severity), (4, Some(9), 2));
    let less = problems_of(r#""$lessCompile""#, "ParseError: Unrecognised input in file /w/a.less line no. 12\n", 64);
    assert_eq!((less[0].file.as_str(), less[0].line), ("/w/a.less", 12));
}
