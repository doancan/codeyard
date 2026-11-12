//! cargo-nextest as the test runner: the `rust.testRunner` setting, turning a `cargo test` command into the nextest one,
//! and reading nextest's report into the same results the Testing view gets from the standard harness.

use std::collections::BTreeMap;

use crate::testexp::{Outcome, TestResult};

/// `rust.testRunner`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TestRunner {
    /// nextest when `cargo nextest` is installed, otherwise `cargo test`.
    Auto,
    /// `cargo test`, the default: nextest is something the user asks for.
    #[default]
    Cargo,
    Nextest,
}

impl TestRunner {
    pub const CHOICES: [&'static str; 3] = ["auto", "cargo", "nextest"];

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "cargo" => Some(Self::Cargo),
            "nextest" => Some(Self::Nextest),
            _ => None,
        }
    }

    /// Whether to run through nextest, given whether it is installed.
    pub fn uses_nextest(self, installed: bool) -> bool {
        match self {
            Self::Cargo => false,
            Self::Auto | Self::Nextest => installed,
        }
    }

    /// Whether the setting asks for nextest by name, so its absence is worth saying.
    pub fn insists(self) -> bool {
        self == Self::Nextest
    }
}

/// What was found out about `cargo nextest` in each folder. A yes stays; a no is remembered for a while so that repeated runs
/// do not start Cargo each time to ask again.
#[derive(Debug, Default)]
pub struct ProbeMemory(std::collections::HashMap<String, (bool, std::time::Instant)>);

impl ProbeMemory {
    /// How long a missing nextest is believed.
    pub const MISSING_FOR: std::time::Duration = std::time::Duration::from_secs(300);

    pub fn known(&self, root: &str, now: std::time::Instant) -> Option<bool> {
        match self.0.get(root) {
            Some((true, _)) => Some(true),
            Some((false, at)) if now.saturating_duration_since(*at) < Self::MISSING_FOR => Some(false),
            _ => None,
        }
    }

    pub fn remember(&mut self, root: &str, found: bool, now: std::time::Instant) {
        self.0.insert(root.to_string(), (found, now));
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// The line the output starts with when tests run through nextest.
pub fn notice() -> String {
    crate::i18n::t("Running with cargo-nextest. Documentation tests are not run.").to_string()
}

/// What a command runs with.
pub fn is_nextest_command(command: &str) -> bool {
    command.starts_with("cargo nextest ")
}

/// Arguments of the test binary that nextest accepts after `--`; the others (`--test-threads`, `--format`, …) are dropped.
const KEPT_AFTER_DASHES: [&str; 3] = ["--exact", "--ignored", "--include-ignored"];

/// `args` of `cargo test` (the words after `test`, before `--`) and the test binary's `libtest` arguments as the words
/// after `cargo` for nextest. `None` for a command nextest cannot run: documentation tests are rustdoc's.
pub fn nextest_words(args: &[String], libtest: &[String], report_skips: bool) -> Option<Vec<String>> {
    if args.iter().any(|a| a == "--doc" || a == "--no-run") {
        return None;
    }
    let mut words: Vec<String> = vec!["nextest".into(), "run".into()];
    if !args.iter().any(|a| a == "--no-fail-fast") {
        words.push("--no-fail-fast".into());
    }
    // The Testing view needs skipped (ignored) tests reported as such, as the standard harness does; by default nextest says
    // nothing about them, and the view would take them for tests the runner lost. A run in the terminal does not need the lines.
    if report_skips && !args.iter().any(|a| a == "--status-level" || a.starts_with("--status-level=")) {
        words.extend(["--status-level".to_string(), "skip".to_string()]);
    }
    words.extend(args.iter().cloned());
    let mut after: Vec<String> = Vec::new();
    let mut tail = libtest.iter();
    while let Some(arg) = tail.next() {
        match arg.as_str() {
            "--nocapture" | "--show-output" => words.push("--no-capture".into()),
            "--skip" => {
                after.push(arg.clone());
                after.extend(tail.next().cloned());
            }
            kept if KEPT_AFTER_DASHES.contains(&kept) => after.push(arg.clone()),
            dropped if dropped.starts_with('-') => {}
            name => after.push(name.to_string()),
        }
    }
    if !after.is_empty() {
        words.push("--".into());
        words.extend(after);
    }
    Some(words)
}

/// The nextest command for a `cargo test …` command line; `None` when it is not one, or nextest cannot run it.
pub fn convert(command: &str, windows: bool, report_skips: bool) -> Option<String> {
    let words = crate::cargo_args::words(command)?;
    let [cargo, test, rest @ ..] = words.as_slice() else { return None };
    if cargo != "cargo" || test != "test" {
        return None;
    }
    let (before, after) = match rest.iter().position(|w| w == "--") {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => (rest, &rest[..0]),
    };
    let words = nextest_words(before, after, report_skips)?;
    let quoted: Option<Vec<String>> = words.iter().map(|w| super::runnables::word_for(w, windows)).collect();
    Some(format!("cargo {}", quoted?.join(" ")))
}

/// The results in the report of `cargo nextest run`: a line per test (`PASS [ 0.018s] (2/6) demo tests::adds_up`), and
/// under a failure the test's output, from which the panic gives the message and the place.
pub fn parse(output: &str) -> BTreeMap<String, TestResult> {
    let output = crate::lsp::strip_ansi(output);
    // The recap of failures under the summary repeats lines already read.
    let lines: Vec<&str> = output.lines().take_while(|l| !l.trim_start().starts_with("Summary [")).collect();
    let mut found: Vec<(usize, String, TestResult)> = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        let Some((status, millis, name)) = status_line(line) else { continue };
        let outcome = match status {
            "PASS" | "LEAK" | "FLAKY" => Outcome::Passed,
            "SKIP" => Outcome::Skipped,
            "FAIL" | "TIMEOUT" | "EXECFAIL" | "LEAK-FAIL" | "ABORT" => Outcome::Failed,
            s if s.starts_with("SIG") => Outcome::Failed,
            // SLOW, TRY, RETRY and the like say a test is still going.
            _ => continue,
        };
        let (message, location) = if outcome == Outcome::Failed { failure_block(&lines[at + 1..]) } else { (None, None) };
        found.push((at, name, TestResult { outcome, millis, message, location }));
    }
    // The same name in several binaries counts once; a failure anywhere is a failure, as with the standard harness.
    let mut out: BTreeMap<String, TestResult> = BTreeMap::new();
    let mut repeated = std::collections::BTreeSet::new();
    for (_, name, result) in found {
        match out.get_mut(&name) {
            None => {
                out.insert(name, result);
            }
            Some(old) => {
                repeated.insert(name);
                if result.outcome == Outcome::Failed && old.outcome != Outcome::Failed {
                    *old = result;
                } else if result.outcome == Outcome::Skipped && old.outcome == Outcome::Passed {
                    old.outcome = Outcome::Skipped;
                }
            }
        }
    }
    for name in repeated {
        if let Some(result) = out.get_mut(&name).filter(|r| r.outcome == Outcome::Failed) {
            let note = crate::i18n::t("Several Rust targets reported this test name. Run it individually to identify the failing target.");
            result.message = Some(format!("{note}\n{}", result.message.take().unwrap_or_default()).trim().chars().take(600).collect());
        }
    }
    out
}

/// `("PASS", Some(18), "tests::adds_up")` from a status line; `None` for any other line.
fn status_line(line: &str) -> Option<(&str, Option<u64>, String)> {
    let line = line.trim_start();
    let (status, rest) = line.split_once(' ')?;
    if status.is_empty() || !status.chars().all(|c| c.is_ascii_uppercase() || c == '-') {
        return None;
    }
    let mut rest = rest.trim_start();
    // `FLAKY 2/3 [ 1.0s]`: the try that passed, then the time.
    if let Some((tries, after)) = rest.split_once(' ')
        && tries
            .split_once('/')
            .is_some_and(|(a, b)| a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit()) && !a.is_empty() && !b.is_empty())
    {
        rest = after.trim_start();
    }
    let rest = rest.strip_prefix('[')?;
    let (time, rest) = rest.split_once(']')?;
    let millis = time.trim().strip_suffix('s').and_then(|s| s.trim_start_matches('>').trim().parse::<f64>().ok()).map(|s| (s * 1000.0).round() as u64);
    let mut rest = rest.trim_start();
    // `(2/6)`, or `(───)` for a test that was not run.
    if rest.starts_with('(') {
        rest = rest.split_once(')')?.1.trim_start();
    }
    // The binary (`demo`, `demo::integration`, `demo::bin/demo`), then the test's name.
    let (_binary, name) = rest.split_once(' ')?;
    let name = name.trim();
    (!name.is_empty()).then(|| (status, millis, name.to_string()))
}

/// The message and place of a failure from the lines under its `FAIL` line, up to the next status line or the summary.
fn failure_block(lines: &[&str]) -> (Option<String>, Option<(String, u32)>) {
    let end = lines.iter().position(|l| status_line(l).is_some() || l.trim_start().starts_with("────")).unwrap_or(lines.len());
    // The `stdout ───` and `stderr ───` headings are nextest's, not the test's.
    let block: Vec<&str> =
        lines[..end].iter().copied().filter(|l| !l.trim_start().starts_with("stdout ───") && !l.trim_start().starts_with("stderr ───")).collect();
    crate::testexp::failure_details(&block)
}

#[cfg(test)]
#[path = "rust_nextest_tests.rs"]
mod tests;
