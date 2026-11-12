//! Output captured from cargo-nextest 0.9.151 and the standard harness (Rust 1.99.1) running the same small package
//! (`testdata/nextest`): two readers must give the same verdicts, messages and places.

use super::*;
use crate::testexp::{Kind, parse_results};

fn sample(name: &str) -> String {
    std::fs::read_to_string(format!("{}/testdata/nextest/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn verdicts(results: &BTreeMap<String, TestResult>) -> Vec<(&str, Outcome)> {
    results.iter().map(|(name, r)| (name.as_str(), r.outcome)).collect()
}

#[test]
fn a_full_run_gives_every_verdict_time_message_and_place() {
    let results = parse(&sample("all.txt"));
    assert_eq!(
        verdicts(&results),
        [
            ("adds_from_outside", Outcome::Passed),
            ("panics_with_unicode", Outcome::Failed),
            ("tests::adds_up", Outcome::Passed),
            ("tests::fails_on_purpose", Outcome::Failed),
            ("tests::ignored_one", Outcome::Passed),
            ("tests::main_is_fine", Outcome::Passed),
        ]
    );
    let failed = &results["tests::fails_on_purpose"];
    assert_eq!(failed.millis, Some(27));
    assert_eq!(failed.location, Some(("src/lib.rs".into(), 21)));
    assert_eq!(failed.message.as_deref(), Some("assertion `left == right` failed\nleft: 3\nright: 4"));
    // Text that is not ASCII reaches the message whole.
    let unicode = &results["panics_with_unicode"];
    assert_eq!(unicode.location, Some(("tests/integration.rs".into(), 8)));
    assert_eq!(unicode.message.as_deref(), Some("beklenmeyen durum: ğüşıöç 😀"));
}

/// The standard harness and nextest, on the same package, say the same about every test.
#[test]
fn nextest_and_the_standard_harness_agree_on_a_run_of_everything() {
    let nextest = parse(&sample("all.txt"));
    let libtest = parse_results(Kind::Rust, &sample("libtest-all.txt"));
    assert_eq!(verdicts(&nextest), verdicts(&libtest));
    for name in ["tests::fails_on_purpose", "panics_with_unicode"] {
        assert_eq!(nextest[name].message, libtest[name].message, "{name}");
        assert_eq!(nextest[name].location, libtest[name].location, "{name}");
    }
}

/// With the ignored test left out, nextest says SKIP where the standard harness says `ignored`.
#[test]
fn skipped_tests_agree_too() {
    let nextest = parse(&sample("skip.txt"));
    let libtest = parse_results(Kind::Rust, &sample("libtest-default.txt"));
    assert_eq!(verdicts(&nextest), verdicts(&libtest));
    assert_eq!(nextest["tests::ignored_one"].outcome, Outcome::Skipped);
}

/// A run stopped at the first failure only reports the tests it reached; the rest are not invented.
#[test]
fn a_cancelled_run_reports_only_what_ran() {
    let results = parse(&sample("fail.txt"));
    assert_eq!(verdicts(&results), [("tests::adds_up", Outcome::Passed), ("tests::fails_on_purpose", Outcome::Failed)]);
}

#[test]
fn one_filtered_test_is_one_result() {
    let results = parse(&sample("pass.txt"));
    assert_eq!(verdicts(&results), [("tests::adds_up", Outcome::Passed)]);
    assert_eq!(results["tests::adds_up"].millis, Some(8));
}

/// The recap of failures under the summary repeats status lines; reading it must not count a test twice.
#[test]
fn the_recap_under_the_summary_is_not_read_again() {
    let results = parse(&sample("all.txt"));
    assert!(results["tests::fails_on_purpose"].message.as_deref().is_some_and(|m| !m.contains("Several Rust targets")));
}

#[test]
fn colour_codes_and_other_statuses_do_not_confuse_the_reader() {
    let output = "\u{1b}[32m        PASS\u{1b}[0m [   0.004s] (1/4) demo a\n\
                  \u{1b}[31m        FAIL\u{1b}[0m [   0.005s] (2/4) demo b\n\
                  \x20 stderr ───\n\n\x20   thread 'b' panicked at src/x.rs:3:9:\n\x20   boom\n\
                  \x20       SLOW [> 60.000s] (3/4) demo c\n\
                  \x20     FLAKY 2/3 [   0.2s] (4/4) demo d\n\
                  \x20    TIMEOUT [  10.000s] (5/5) demo e\n\
                  \x20     SIGSEGV [   0.1s] (6/6) demo f\n\
                  ────────────\n     Summary [   0.1s] 6 tests run\n";
    let results = parse(output);
    assert_eq!(
        verdicts(&results),
        [("a", Outcome::Passed), ("b", Outcome::Failed), ("d", Outcome::Passed), ("e", Outcome::Failed), ("f", Outcome::Failed)],
        "SLOW only says a test is still going; FLAKY passed on a retry"
    );
    assert_eq!(results["b"].location, Some(("src/x.rs".into(), 3)));
    assert_eq!(results["b"].message.as_deref(), Some("boom"));
}

#[test]
fn a_name_reported_by_two_binaries_fails_if_one_fails() {
    let output = "        PASS [   0.001s] (1/2) demo::integration same\n        FAIL [   0.001s] (2/2) demo same\n";
    let results = parse(output);
    assert_eq!(results["same"].outcome, Outcome::Failed);
    assert!(results["same"].message.as_deref().unwrap().starts_with("Several Rust targets reported this test name."));
}

#[test]
fn garbage_gives_nothing() {
    assert!(parse("").is_empty());
    assert!(parse("error: no such command: `nextest`\n\nhelp: view all installed commands with `cargo --list`\n").is_empty());
    assert!(parse("PASS\nPASS [\nPASS ] x\nFAIL [ 1s]\n").is_empty());
    assert!(parse(&"PASS [   0.001s] (1/1) a b\n".repeat(10_000)).len() == 1);
}

fn words(command: &str) -> Option<String> {
    convert(command, false, false)
}

#[test]
fn a_cargo_test_command_becomes_the_nextest_one() {
    assert_eq!(
        words("cargo test -p app problems::tests::adds_up -- --exact").as_deref(),
        Some("cargo nextest run --no-fail-fast -p app problems::tests::adds_up -- --exact")
    );
    assert_eq!(
        words("cargo test --manifest-path crates/tool/Cargo.toml tests::works -- --exact").as_deref(),
        Some("cargo nextest run --no-fail-fast --manifest-path crates/tool/Cargo.toml tests::works -- --exact")
    );
    assert_eq!(words("cargo test").as_deref(), Some("cargo nextest run --no-fail-fast"));
    assert_eq!(words("cargo test --workspace --no-fail-fast").as_deref(), Some("cargo nextest run --workspace --no-fail-fast"));
    assert_eq!(words("cargo test --test api t -- --exact").as_deref(), Some("cargo nextest run --no-fail-fast --test api t -- --exact"));
    assert_eq!(words("cargo test -p app 'a b' -- --exact").as_deref(), Some("cargo nextest run --no-fail-fast -p app 'a b' -- --exact"));
}

#[test]
fn what_nextest_cannot_run_stays_with_cargo() {
    assert_eq!(words("cargo test --doc"), None, "documentation tests are rustdoc's");
    assert_eq!(words("cargo build"), None);
    assert_eq!(words("go test ./..."), None);
    assert_eq!(words("cargo test 'unterminated"), None);
}

#[test]
fn libtest_arguments_are_mapped_or_dropped() {
    let args = |s: &[&str]| s.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    let got = nextest_words(
        &args(&["--package", "demo", "--lib"]),
        &args(&["tests::adds_up", "--exact", "--nocapture", "--include-ignored", "--test-threads=1"]),
        false,
    )
    .unwrap();
    assert_eq!(got.join(" "), "nextest run --no-fail-fast --package demo --lib --no-capture -- tests::adds_up --exact --include-ignored");
    let skipped = nextest_words(&[], &args(&["--skip", "slow", "--format", "json", "-Zunstable-options", "name"]), false).unwrap();
    assert_eq!(skipped.join(" "), "nextest run --no-fail-fast -- --skip slow json name", "an unknown option is dropped alone, its value is read as a name");
    assert_eq!(nextest_words(&args(&["--no-run"]), &[], false), None);
}

#[test]
fn the_setting_picks_the_runner() {
    for (text, runner) in [("auto", TestRunner::Auto), ("cargo", TestRunner::Cargo), ("nextest", TestRunner::Nextest)] {
        assert_eq!(TestRunner::parse(text), Some(runner));
    }
    assert_eq!(TestRunner::parse("Nextest"), None);
    assert_eq!(TestRunner::default(), TestRunner::Cargo, "nextest is asked for, not assumed");
    assert!(TestRunner::Auto.uses_nextest(true) && !TestRunner::Auto.uses_nextest(false));
    assert!(TestRunner::Nextest.uses_nextest(true) && !TestRunner::Nextest.uses_nextest(false));
    assert!(!TestRunner::Cargo.uses_nextest(true));
    assert!(TestRunner::Nextest.insists() && !TestRunner::Auto.insists());
    assert!(is_nextest_command("cargo nextest run -p x") && !is_nextest_command("cargo test"));
}

#[test]
fn a_missing_nextest_is_believed_for_a_while_and_a_found_one_stays() {
    let t0 = std::time::Instant::now();
    let mut memory = ProbeMemory::default();
    assert_eq!(memory.known("/p", t0), None);
    memory.remember("/p", false, t0);
    assert_eq!(memory.known("/p", t0 + std::time::Duration::from_secs(10)), Some(false), "no second Cargo start for a repeated click");
    assert_eq!(memory.known("/p", t0 + ProbeMemory::MISSING_FOR), None, "asked again after a while: it may have been installed");
    assert_eq!(memory.known("/q", t0), None);
    memory.remember("/p", true, t0);
    assert_eq!(memory.known("/p", t0 + std::time::Duration::from_secs(100_000)), Some(true));
    memory.clear();
    assert_eq!(memory.known("/p", t0), None);
}

#[test]
fn windows_hosts_get_double_quotes_and_refuse_what_cmd_and_powershell_read_differently() {
    assert_eq!(convert("cargo test -p app 'a b' -- --exact", true, false).as_deref(), Some("cargo nextest run --no-fail-fast -p app \"a b\" -- --exact"));
    assert_eq!(convert("cargo test -p app 'it\"s' -- --exact", true, false), None, "a quote in a name cannot be written for both shells");
    assert_eq!(convert("cargo test -p app 'a b' -- --exact", false, false).as_deref(), Some("cargo nextest run --no-fail-fast -p app 'a b' -- --exact"));
}

/// The Testing view asks nextest to report skipped tests (it would take an ignored test for one the runner lost), once; a run
/// in the terminal does not print them.
#[test]
fn the_testing_views_commands_report_skips_and_the_terminals_do_not() {
    assert_eq!(convert("cargo test -p app", false, true).as_deref(), Some("cargo nextest run --no-fail-fast --status-level skip -p app"));
    assert_eq!(convert("cargo test -p app", false, false).as_deref(), Some("cargo nextest run --no-fail-fast -p app"));
    assert_eq!(
        convert("cargo test --workspace --no-fail-fast", false, true).as_deref(),
        Some("cargo nextest run --status-level skip --workspace --no-fail-fast")
    );
    assert_eq!(
        convert("cargo test --status-level=all", false, true).as_deref(),
        Some("cargo nextest run --no-fail-fast --status-level=all"),
        "the user's own level stays"
    );
}
