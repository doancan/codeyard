//! The Testing view's model: tests found in the project's files and what their runs reported. Test commands come from
//! `testrun` (the project's own runner); this file reads their output: cargo's test harness, `go test -v` and
//! pytest's `-rA` summary. JavaScript runners are judged by their exit code.

use std::collections::{BTreeMap, HashMap, HashSet};

/// Which runner a file's tests use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Rust,
    Go,
    Python,
    Js,
}

pub fn kind_of(rel: &str) -> Option<Kind> {
    match rel.rsplit_once('.')?.1 {
        "rs" => Some(Kind::Rust),
        "go" if rel.ends_with("_test.go") => Some(Kind::Go),
        "py" => Some(Kind::Python),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Some(Kind::Js),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outcome {
    #[default]
    NotRun,
    Running,
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestResult {
    pub outcome: Outcome,
    pub millis: Option<u64>,
    /// First lines of the failure.
    pub message: Option<String>,
    /// Where it failed: (path as the runner printed it, 1-based line).
    pub location: Option<(String, u32)>,
}

/// A test in the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestItem {
    pub name: String,
    pub id: String,
    /// 0-based line of its definition.
    pub line: usize,
    pub command: String,
}

/// A file with tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFile {
    pub rel: String,
    pub kind: Kind,
    pub tests: Vec<TestItem>,
    /// Command running every test of the file.
    pub command: Option<String>,
}

/// Most characters kept from a failure message.
const MAX_MESSAGE: usize = 600;

/// The command with what its output needs to be read: `-v` for go (one line per test).
pub fn parsable(kind: Kind, command: &str) -> String {
    match kind {
        Kind::Go => command.replacen("go test ", "go test -v ", 1),
        Kind::Python => format!("{command} -rA"),
        _ => command.to_string(),
    }
}

fn clip(lines: &[&str]) -> Option<String> {
    let text: String = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
    let text: String = text.chars().filter(|c| !c.is_control() || *c == '\n').take(MAX_MESSAGE).collect();
    (!text.is_empty()).then_some(text)
}

/// Results by test id.
pub fn parse_results(kind: Kind, output: &str) -> BTreeMap<String, TestResult> {
    match kind {
        Kind::Rust => parse_libtest(output),
        Kind::Go => parse_go(output),
        Kind::Python => parse_pytest(output),
        Kind::Js => BTreeMap::new(),
    }
}

/// cargo's harness: `test a::b ... ok|FAILED|ignored`, then `---- a::b stdout ----` blocks with the panic.
fn parse_libtest(output: &str) -> BTreeMap<String, TestResult> {
    let mut out = BTreeMap::new();
    let mut repeated = HashSet::new();
    let lines: Vec<&str> = output.lines().collect();
    for l in &lines {
        let Some(rest) = l.strip_prefix("test ") else { continue };
        let Some((id, verdict)) = rest.split_once(" ... ") else { continue };
        let outcome = match verdict.trim() {
            "ok" => Outcome::Passed,
            "FAILED" => Outcome::Failed,
            v if v.starts_with("ignored") => Outcome::Skipped,
            _ => continue,
        };
        let id = id.trim().to_string();
        if out.contains_key(&id) {
            repeated.insert(id.clone());
        }
        let result = out.entry(id).or_insert(TestResult { outcome, ..Default::default() });
        if outcome == Outcome::Failed || result.outcome == Outcome::Failed {
            result.outcome = Outcome::Failed;
        } else if outcome == Outcome::Skipped {
            result.outcome = Outcome::Skipped;
        }
    }
    let mut i = 0;
    while i < lines.len() {
        let Some(id) = lines[i].strip_prefix("---- ").and_then(|r| r.strip_suffix(" stdout ----")) else {
            i += 1;
            continue;
        };
        let start = i + 1;
        let mut end = start;
        while end < lines.len() && !lines[end].starts_with("---- ") && lines[end] != "failures:" {
            end += 1;
        }
        let block = &lines[start..end];
        if let Some(r) = out.get_mut(id) {
            if let Some(p) = block.iter().position(|l| l.contains("panicked at ")) {
                let at = block[p].split("panicked at ").nth(1).unwrap_or_default().trim_end_matches(':');
                let mut parts = at.rsplitn(3, ':');
                let (_col, line, path) = (parts.next(), parts.next().and_then(|l| l.parse().ok()), parts.next());
                if let (Some(path), Some(line)) = (path, line) {
                    r.location = Some((path.to_string(), line));
                }
                let msg: Vec<&str> = block[p + 1..].iter().take_while(|l| !l.starts_with("note:") && !l.starts_with("stack backtrace")).copied().collect();
                r.message = clip(&msg);
            } else {
                r.message = clip(block);
            }
        }
        i = end;
    }
    for id in repeated {
        if let Some(result) = out.get_mut(&id)
            && result.outcome == Outcome::Failed
        {
            let note = crate::i18n::t("Several Rust targets reported this test name. Run it individually to identify the failing target.");
            let message = format!("{note}\n{}", result.message.take().unwrap_or_default());
            result.message = Some(message.trim().chars().take(MAX_MESSAGE).collect());
        }
    }
    out
}

/// `go test -v`: `--- PASS: TestA (0.01s)`, with `file_test.go:12: message` lines before a failure.
fn parse_go(output: &str) -> BTreeMap<String, TestResult> {
    let mut out = BTreeMap::new();
    let mut notes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for l in output.lines() {
        if let Some(name) = l.strip_prefix("=== RUN").map(str::trim) {
            current = Some(name.to_string());
            continue;
        }
        let t = l.trim_start();
        let verdict = [("--- PASS: ", Outcome::Passed), ("--- FAIL: ", Outcome::Failed), ("--- SKIP: ", Outcome::Skipped)]
            .into_iter()
            .find_map(|(p, o)| t.strip_prefix(p).map(|r| (r, o)));
        if let Some((rest, outcome)) = verdict {
            let (name, time) = rest.split_once(' ').unwrap_or((rest, ""));
            // Subtests count with their parent.
            if name.contains('/') {
                continue;
            }
            let millis = time.trim().trim_start_matches('(').trim_end_matches(')').trim_end_matches('s').parse::<f64>().ok().map(|s| (s * 1000.0) as u64);
            let lines = notes.remove(name).unwrap_or_default();
            let location = lines.iter().find_map(|l| {
                let (file, rest) = l.split_once(':')?;
                let line = rest.split(':').next()?.trim().parse().ok()?;
                file.ends_with(".go").then(|| (file.to_string(), line))
            });
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            let message = (outcome == Outcome::Failed).then(|| clip(&refs)).flatten();
            out.insert(name.to_string(), TestResult { outcome, millis, message, location });
            continue;
        }
        if let Some(name) = current.as_ref().filter(|_| l.starts_with("    ")) {
            notes.entry(name.split('/').next().unwrap_or(name).to_string()).or_default().push(l.trim().to_string());
        }
    }
    out
}

/// pytest `-rA` summary: `PASSED path::test`, `FAILED path::test - message`, `ERROR path::test - message`.
fn parse_pytest(output: &str) -> BTreeMap<String, TestResult> {
    let mut out = BTreeMap::new();
    for l in output.lines() {
        let found = [("PASSED ", Outcome::Passed), ("FAILED ", Outcome::Failed), ("ERROR ", Outcome::Failed), ("XFAIL ", Outcome::Skipped)]
            .into_iter()
            .find_map(|(p, o)| l.strip_prefix(p).map(|r| (r, o)));
        let Some((rest, outcome)) = found else { continue };
        let (id, message) = match rest.split_once(" - ") {
            Some((id, m)) => (id, clip(&[m])),
            None => (rest, None),
        };
        // Parametrised cases (`test_x[1]`) count with their test.
        let id = id.split('[').next().unwrap_or(id).trim().to_string();
        let entry = out.entry(id).or_insert(TestResult { outcome, ..Default::default() });
        if outcome == Outcome::Failed {
            entry.outcome = Outcome::Failed;
            entry.message = entry.message.take().or(message);
        }
    }
    out
}

/// Files worth reading for tests: Rust sources, `*_test.go`, `test_*.py` / `*_test.py`, `*.test.*` / `*.spec.*` and
/// files under `__tests__`. Build output and dependencies are skipped.
pub fn is_candidate(rel: &str) -> bool {
    if rel.split('/').any(|part| matches!(part, "target" | "node_modules" | ".git" | "vendor" | "dist" | "build" | ".venv" | "venv" | "__pycache__")) {
        return false;
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    match kind_of(rel) {
        Some(Kind::Rust | Kind::Go) => true,
        Some(Kind::Python) => name.starts_with("test_") || name.ends_with("_test.py"),
        Some(Kind::Js) => name.contains(".test.") || name.contains(".spec.") || rel.contains("__tests__/"),
        None => false,
    }
}

/// Results by (file, test id).
pub type Results = HashMap<(String, String), TestResult>;

/// A file's outcome from its tests: failed beats running beats not run; passed when every test ran (skipped counts).
pub fn file_outcome(file: &TestFile, results: &Results) -> Outcome {
    let of = |t: &TestItem| results.get(&(file.rel.clone(), t.id.clone())).map_or(Outcome::NotRun, |r| r.outcome);
    let all: Vec<Outcome> = file.tests.iter().map(of).collect();
    if all.contains(&Outcome::Failed) {
        Outcome::Failed
    } else if all.contains(&Outcome::Running) {
        Outcome::Running
    } else if !all.is_empty() && all.iter().all(|o| matches!(o, Outcome::Passed | Outcome::Skipped)) {
        Outcome::Passed
    } else {
        Outcome::NotRun
    }
}

/// Counts for the summary: (passed, failed, skipped, total).
pub fn summary(files: &[TestFile], results: &Results) -> (usize, usize, usize, usize) {
    let mut out = (0, 0, 0, 0);
    for f in files {
        for t in &f.tests {
            out.3 += 1;
            match results.get(&(f.rel.clone(), t.id.clone())).map(|r| r.outcome) {
                Some(Outcome::Passed) => out.0 += 1,
                Some(Outcome::Failed) => out.1 += 1,
                Some(Outcome::Skipped) => out.2 += 1,
                _ => {}
            }
        }
    }
    out
}

/// A row of the Testing view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    File {
        file: usize,
        outcome: Outcome,
        open: bool,
    },
    Test {
        file: usize,
        test: usize,
        outcome: Outcome,
        millis: Option<u64>,
    },
    /// A failed test's first message line, under it.
    Message {
        file: usize,
        test: usize,
        text: String,
    },
}

/// Rows for the tree: files (open ones in `expanded`), their tests and failure messages. `filter` keeps tests whose
/// name or file contains it (any case); `only_failed` keeps failed tests. While either narrows the tree, files are open.
pub fn rows(files: &[TestFile], results: &Results, expanded: &HashSet<String>, filter: &str, only_failed: bool) -> Vec<Row> {
    let needle = filter.trim().to_lowercase();
    let mut out = Vec::new();
    for (fi, f) in files.iter().enumerate() {
        let shown: Vec<(usize, Option<&TestResult>)> = f
            .tests
            .iter()
            .enumerate()
            .map(|(ti, t)| (ti, results.get(&(f.rel.clone(), t.id.clone()))))
            .filter(|(ti, _)| needle.is_empty() || f.rel.to_lowercase().contains(&needle) || f.tests[*ti].name.to_lowercase().contains(&needle))
            .filter(|(_, r)| !only_failed || r.is_some_and(|r| r.outcome == Outcome::Failed))
            .collect();
        if shown.is_empty() {
            continue;
        }
        let open = expanded.contains(&f.rel) || !needle.is_empty() || only_failed;
        out.push(Row::File { file: fi, outcome: file_outcome(f, results), open });
        if !open {
            continue;
        }
        for (ti, r) in shown {
            out.push(Row::Test { file: fi, test: ti, outcome: r.map_or(Outcome::NotRun, |r| r.outcome), millis: r.and_then(|r| r.millis) });
            if let Some(text) = r.filter(|r| r.outcome == Outcome::Failed).and_then(|r| r.message.as_ref()).and_then(|m| m.lines().next()) {
                out.push(Row::Message { file: fi, test: ti, text: text.to_string() });
            }
        }
    }
    out
}

/// A command to run and the tests whose results it reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub kind: Kind,
    pub command: String,
    /// (file, test id) of every test the command runs.
    pub scope: Vec<(String, String)>,
}

fn scope_of<'a>(files: impl IntoIterator<Item = &'a TestFile>) -> Vec<(String, String)> {
    files.into_iter().flat_map(|f| f.tests.iter().map(|t| (f.rel.clone(), t.id.clone()))).collect()
}

/// One job per runner that has tests: the whole workspace, every Go package, pytest from the root, the JavaScript runner.
pub fn run_all(files: &[TestFile], js_runner: Option<&str>) -> Vec<Job> {
    let mut out = Vec::new();
    for kind in [Kind::Rust, Kind::Go, Kind::Python, Kind::Js] {
        let of_kind: Vec<&TestFile> = files.iter().filter(|f| f.kind == kind && !f.tests.is_empty()).collect();
        if of_kind.is_empty() {
            continue;
        }
        let command = match kind {
            Kind::Rust => "cargo test --workspace --no-fail-fast".to_string(),
            Kind::Go => "go test ./...".to_string(),
            Kind::Python => "python3 -m pytest".to_string(),
            Kind::Js => match js_runner {
                Some(r) => r.trim_end_matches(" --").to_string(),
                None => continue,
            },
        };
        out.push(Job { kind, command: parsable(kind, &command), scope: scope_of(of_kind) });
    }
    out
}

/// A job for one file, or for one of its tests.
pub fn job_for(file: &TestFile, test: Option<usize>) -> Option<Job> {
    match test {
        Some(i) => {
            let t = file.tests.get(i)?;
            Some(Job { kind: file.kind, command: parsable(file.kind, &t.command), scope: vec![(file.rel.clone(), t.id.clone())] })
        }
        None => Some(Job { kind: file.kind, command: parsable(file.kind, file.command.as_ref()?), scope: scope_of([file]) }),
    }
}

/// What a finished job reports for each test in its scope. Rust tests absent from a successful run become NotRun;
/// a failed build/runner must not leave earlier green results visible. JavaScript is judged by exit code. A Rust test whose module
/// path is not the file's (a `#[path]` module) is found by its last two segments when that is unambiguous.
pub fn outcomes(job: &Job, output: &str, exit: Option<i32>) -> Vec<((String, String), TestResult)> {
    let parsed = parse_results(job.kind, output);
    let mut out = Vec::new();
    for (rel, id) in &job.scope {
        let found = match job.kind {
            Kind::Js => Some(TestResult { outcome: if exit == Some(0) { Outcome::Passed } else { Outcome::Failed }, ..Default::default() }),
            Kind::Rust => parsed
                .get(id)
                .cloned()
                .or_else(|| {
                    let segs: Vec<&str> = id.rsplit("::").take(2).collect();
                    (segs.len() == 2).then_some(())?;
                    let tail = format!("::{}::{}", segs[1], segs[0]);
                    let mut hits = parsed.iter().filter(|(k, _)| k.ends_with(&tail));
                    let first = hits.next()?;
                    hits.next().is_none().then(|| first.1.clone())
                })
                .or_else(|| {
                    Some(TestResult {
                        outcome: if exit == Some(0) { Outcome::NotRun } else { Outcome::Failed },
                        message: (exit != Some(0)).then(|| {
                            crate::i18n::t("The test runner stopped without reporting this test. See Test Results for build or runner errors.").to_string()
                        }),
                        ..Default::default()
                    })
                }),
            _ => parsed.get(id).cloned(),
        };
        if let Some(r) = found {
            out.push(((rel.clone(), id.clone()), r));
        }
    }
    out
}

/// `cargo test` arguments that pick the test binary (`-p name`, `--test name`), for building it without running.
pub fn cargo_select_args(command: &str) -> Result<Vec<String>, String> {
    crate::cargo_args::selection(command)
}

/// Test binaries from `cargo test --no-run --message-format=json`: (target name, kind, path).
pub fn test_executables(json_lines: &str) -> Vec<(String, String, String)> {
    let mut seen = HashSet::new();
    json_lines
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["reason"] == "compiler-artifact" && v["profile"]["test"] == true)
        .filter_map(|v| {
            let exe = v["executable"].as_str().filter(|p| !p.is_empty())?.to_string();
            let kind = v["target"]["kind"].get(0).and_then(|k| k.as_str()).unwrap_or("lib").to_string();
            Some((v["target"]["name"].as_str().unwrap_or_default().to_string(), kind, exe))
        })
        .filter(|artifact| seen.insert(artifact.clone()))
        .collect()
}

/// A partial Cargo build may already have emitted binaries before a later target fails.
pub fn built_test_executables(output: &str, exit: Option<i32>) -> Result<Vec<(String, String, String)>, String> {
    if exit != Some(0) || !crate::cargo_project::build_finished(output) {
        return Err(crate::cargo_project::diagnostics(output));
    }
    let binaries = test_executables(output);
    if binaries.is_empty() {
        return Err(crate::i18n::t("Cargo did not report a Rust test executable").to_string());
    }
    Ok(binaries)
}

/// Debug configurations for a test: the Rust test binary (one per candidate binary) under lldb, `go test` under Delve,
/// pytest under debugpy. JavaScript has none.
pub fn debug_configs(file: &TestFile, test: &TestItem, root: &str, rust_binaries: &[(String, String, String)]) -> Vec<crate::debug_config::LaunchConfig> {
    use crate::debug_config::{LaunchConfig, adapter_for};
    let root = root.trim_end_matches('/');
    let config = |name: String, ty: &str, args: serde_json::Value| LaunchConfig {
        name,
        adapter: adapter_for(ty),
        request: "launch".into(),
        args,
        pre_launch: None,
        cargo_target: None,
        post_debug: None,
        no_debug: false,
        inputs: Vec::new(),
    };
    let title = crate::i18n::tf("Debug Test: {}", &[&test.name]);
    match file.kind {
        Kind::Rust => {
            let many = rust_binaries.len() > 1;
            rust_binaries
                .iter()
                .map(|(name, kind, exe)| {
                    let label = if many { format!("{title} ({name}, {kind})") } else { title.clone() };
                    let args = serde_json::json!({ "program": exe, "args": [test.id, "--exact", "--nocapture", "--test-threads=1"], "cwd": root });
                    config(label, "lldb", args)
                })
                .collect()
        }
        Kind::Go => {
            let dir = file.rel.rsplit_once('/').map_or(root.to_string(), |(d, _)| format!("{root}/{d}"));
            let args =
                serde_json::json!({ "mode": "test", "program": dir, "cwd": dir, "args": ["-test.run", format!("^{}$", test.id)], "outputMode": "remote" });
            vec![config(title, "go", args)]
        }
        Kind::Python => {
            let args = serde_json::json!({ "module": "pytest", "args": [test.id, "-q"], "cwd": root, "console": "internalConsole" });
            vec![config(title, "python", args)]
        }
        Kind::Js => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_output_gives_outcomes_messages_and_where_it_failed() {
        let out = "running 3 tests
test problems::tests::adds_up ... ok
test problems::tests::fails ... FAILED
test problems::tests::later ... ignored, needs a network

failures:

---- problems::tests::fails stdout ----

thread 'problems::tests::fails' panicked at crates/app/src/problems.rs:42:9:
assertion `left == right` failed
  left: 1
 right: 2
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    problems::tests::fails

test result: FAILED. 1 passed; 1 failed; 1 ignored
";
        let r = parse_results(Kind::Rust, out);
        assert_eq!(r["problems::tests::adds_up"].outcome, Outcome::Passed);
        assert_eq!(r["problems::tests::later"].outcome, Outcome::Skipped);
        let f = &r["problems::tests::fails"];
        assert_eq!(f.outcome, Outcome::Failed);
        assert_eq!(f.location, Some(("crates/app/src/problems.rs".into(), 42)));
        assert_eq!(f.message.as_deref(), Some("assertion `left == right` failed\nleft: 1\nright: 2"));
    }

    #[test]
    fn go_verbose_output_gives_times_and_failure_lines() {
        let out = "=== RUN   TestA
--- PASS: TestA (0.00s)
=== RUN   TestB
    b_test.go:12: want 2, got 3
=== RUN   TestB/sub
    --- FAIL: TestB/sub (0.00s)
--- FAIL: TestB (0.01s)
=== RUN   TestC
--- SKIP: TestC (0.00s)
FAIL
";
        let r = parse_results(Kind::Go, out);
        assert_eq!((r["TestA"].outcome, r["TestA"].millis), (Outcome::Passed, Some(0)));
        let b = &r["TestB"];
        assert_eq!((b.outcome, b.millis, b.location.clone()), (Outcome::Failed, Some(10), Some(("b_test.go".into(), 12))));
        assert_eq!(b.message.as_deref(), Some("b_test.go:12: want 2, got 3"));
        assert_eq!(r["TestC"].outcome, Outcome::Skipped);
        assert!(!r.contains_key("TestB/sub"));
        assert_eq!(parsable(Kind::Go, "go test ./x -run '^A$'"), "go test -v ./x -run '^A$'");
    }

    #[test]
    fn pytest_summary_gives_outcomes_and_messages() {
        let out = "=========== short test summary info ============
PASSED t/test_u.py::test_top
PASSED t/test_u.py::test_param[1]
FAILED t/test_u.py::test_param[2] - assert 2 == 3
FAILED t/test_u.py::TestUser::test_name - AssertionError: names differ
ERROR t/test_u.py::test_db - fixture 'db' not found
";
        let r = parse_results(Kind::Python, out);
        assert_eq!(r["t/test_u.py::test_top"].outcome, Outcome::Passed);
        assert_eq!(r["t/test_u.py::test_param"].outcome, Outcome::Failed, "a failed case fails its test");
        assert_eq!(r["t/test_u.py::TestUser::test_name"].message.as_deref(), Some("AssertionError: names differ"));
        assert_eq!(r["t/test_u.py::test_db"].outcome, Outcome::Failed);
    }

    #[test]
    fn hostile_output_is_survived() {
        let junk = format!("test \x1b[2J ... ok\n---- x stdout ----\n{}\npanicked at :::\n=== RUN\n--- FAIL: \nFAILED  - \n", "y".repeat(100_000));
        for k in [Kind::Rust, Kind::Go, Kind::Python] {
            for r in parse_results(k, &junk).values() {
                assert!(r.message.as_ref().is_none_or(|m| m.chars().count() <= MAX_MESSAGE && !m.contains('\x1b')));
            }
        }
        assert_eq!(kind_of("a/b_test.go"), Some(Kind::Go));
        assert_eq!(kind_of("a/b.go"), None);
    }

    fn file(rel: &str, kind: Kind, ids: &[&str]) -> TestFile {
        TestFile {
            rel: rel.into(),
            kind,
            tests: ids
                .iter()
                .enumerate()
                .map(|(i, id)| TestItem { name: id.rsplit("::").next().unwrap().into(), id: (*id).into(), line: i, command: format!("run {id}") })
                .collect(),
            command: Some(format!("run {rel}")),
        }
    }

    #[test]
    fn candidates_skip_dependencies_and_non_test_files() {
        assert!(is_candidate("crates/app/src/x.rs"));
        assert!(!is_candidate("target/debug/build/x.rs"));
        assert!(is_candidate("pkg/a_test.go") && !is_candidate("pkg/a.go"));
        assert!(is_candidate("t/test_u.py") && is_candidate("t/u_test.py") && !is_candidate("t/u.py"));
        assert!(is_candidate("web/a.test.ts") && is_candidate("web/__tests__/a.js") && !is_candidate("web/a.ts"));
        assert!(!is_candidate("node_modules/x/a.test.js"));
    }

    #[test]
    fn rows_follow_results_folding_and_filters() {
        let files = vec![file("a.rs", Kind::Rust, &["m::tests::one", "m::tests::two"]), file("b.rs", Kind::Rust, &["n::tests::three"])];
        let mut results = Results::new();
        results.insert(("a.rs".into(), "m::tests::one".into()), TestResult { outcome: Outcome::Passed, ..Default::default() });
        results
            .insert(("a.rs".into(), "m::tests::two".into()), TestResult { outcome: Outcome::Failed, message: Some("boom\nmore".into()), ..Default::default() });
        let open: HashSet<String> = ["a.rs".to_string(), "b.rs".to_string()].into();
        let r = rows(&files, &results, &open, "", false);
        assert_eq!(r.len(), 6, "2 files, 3 tests, 1 message: {r:?}");
        assert_eq!(r[0], Row::File { file: 0, outcome: Outcome::Failed, open: true });
        assert_eq!(r[3], Row::Message { file: 0, test: 1, text: "boom".into() });
        assert_eq!(r[4], Row::File { file: 1, outcome: Outcome::NotRun, open: true });
        assert_eq!(rows(&files, &results, &HashSet::new(), "", false).len(), 2, "files start closed");
        assert_eq!(rows(&files, &results, &HashSet::new(), "THREE", false).len(), 2);
        assert_eq!(rows(&files, &results, &HashSet::new(), "", true).len(), 3, "only the failed test and its file");
        assert_eq!(summary(&files, &results), (1, 1, 0, 3));
    }

    #[test]
    fn jobs_cover_each_runner_and_results_map_back() {
        let files = vec![file("a.rs", Kind::Rust, &["app::tests::one"]), file("x_test.go", Kind::Go, &["TestX"]), file("w/a.test.js", Kind::Js, &["adds"])];
        let jobs = run_all(&files, Some("npm test --"));
        assert_eq!(jobs.iter().map(|j| j.command.as_str()).collect::<Vec<_>>(), ["cargo test --workspace --no-fail-fast", "go test -v ./...", "npm test"]);
        assert!(run_all(&files, None).iter().all(|j| j.kind != Kind::Js), "no runner, no JavaScript job");
        // A module declared with #[path] reports under another path: found by its last two segments.
        let r = outcomes(&jobs[0], "test workbench::remote_glue::tests::one ... ok\n", Some(0));
        assert_eq!(r, vec![(("a.rs".into(), "app::tests::one".into()), TestResult { outcome: Outcome::Passed, ..Default::default() })]);
        let ambiguous = "test a::tests::one ... ok\ntest b::tests::one ... FAILED\n";
        let ambiguous = outcomes(&jobs[0], ambiguous, Some(1));
        assert_eq!(ambiguous[0].1.outcome, Outcome::Failed, "an ambiguous failed run cannot retain an earlier green result");
        assert!(ambiguous[0].1.message.as_deref().unwrap().contains("Test Results"));
        assert_eq!(outcomes(&jobs[2], "", Some(1))[0].1.outcome, Outcome::Failed, "JavaScript is judged by the exit code");
        assert_eq!(job_for(&files[1], Some(0)).unwrap().command, "run TestX");
        assert_eq!(job_for(&files[0], None).unwrap().scope.len(), 1);
    }

    #[test]
    fn debugging_a_test_builds_the_runner_configuration() {
        assert_eq!(cargo_select_args("cargo test -p app --test api x::y -- --exact").unwrap(), ["-p", "app", "--test", "api"]);
        assert!(cargo_select_args("cargo test x").unwrap().is_empty());
        let json = r#"{"reason":"compiler-artifact","target":{"name":"app","kind":["bin"]},"profile":{"test":true},"executable":"/t/app-1"}
{"reason":"compiler-artifact","target":{"name":"dep","kind":["lib"]},"profile":{"test":false},"executable":null}
not json
{"reason":"build-finished","success":true}"#;
        let bins = test_executables(json);
        assert_eq!(bins, vec![("app".to_string(), "bin".to_string(), "/t/app-1".to_string())]);
        let f = file("crates/app/src/a.rs", Kind::Rust, &["a::tests::one"]);
        let c = debug_configs(&f, &f.tests[0], "/p/", &bins);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].args["program"], "/t/app-1");
        assert_eq!(c[0].args["args"][0], "a::tests::one");
        let g = file("pkg/x_test.go", Kind::Go, &["TestX"]);
        let c = debug_configs(&g, &g.tests[0], "/p", &[]);
        assert_eq!((c[0].args["mode"].as_str(), c[0].args["program"].as_str()), (Some("test"), Some("/p/pkg")));
        let py = file("t/test_u.py", Kind::Python, &["t/test_u.py::test_a"]);
        assert_eq!(debug_configs(&py, &py.tests[0], "/p", &[])[0].args["module"], "pytest");
    }
}
