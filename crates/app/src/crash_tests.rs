//! The parts of a crash report and its follow-up that need no panic: what the report holds, how big it can get, and when the
//! next start announces that the last run did not finish.

use super::session::{self, PreviousCrash, begin_with, crash_key, summarize};
use super::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-crash-follow-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&d).ok();
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn report(location: &str, message: &str) -> String {
    format!("Codeyard 0.9.1 (macos aarch64)\nthread: main\nat: {location}\n{FATAL_UNKNOWN}\n\n{message}\n\nlast notifications:\n\n   0: std::backtrace\n")
}

fn dead(_: u32) -> bool {
    false
}

#[test]
fn a_report_carries_the_log_and_notifications_without_the_home_folder() {
    let text = compose(
        "Codeyard 0.9.1 (macos aarch64)\nthread: main\nat: /Users/dogan/work/codeyard/src/a.rs:1:2\n",
        "index out of bounds: /Users/dogan/notes.txt",
        &["2026-11-12T10:00:00.000Z INFO  app: opened /Users/dogan/work".into()],
        &["2026-11-12T10:00:01.000Z ERROR Git: push failed for bob@host.example".into()],
        "   0: core::panicking at /Users/dogan/.cargo/registry/x.rs:5",
        Some("/Users/dogan"),
    );
    assert!(!text.contains("dogan") && !text.contains("bob@"), "{text}");
    for expected in
        ["at: ~/work/codeyard/src/a.rs:1:2", "last notifications:", "ERROR Git: push failed", "last log lines:", "INFO  app: opened ~/work", "core::panicking"]
    {
        assert!(text.contains(expected), "{expected:?} missing from:\n{text}");
    }
}

#[test]
fn a_report_is_bounded_and_cut_on_a_character_boundary() {
    let huge = "frame ünïcode ".repeat(100_000);
    let text = compose("h\n", "m", &[], &[], &huge, None);
    assert!(text.len() <= MAX_REPORT_BYTES + 16, "{}", text.len());
    assert!(text.ends_with("(cut)\n"));
}

#[test]
fn a_first_run_says_nothing_even_with_old_reports_in_the_folder() {
    let dir = tmp("first");
    assert_eq!(begin_with(&dir.join("does-not-exist"), 10, 1_000, dead), None);
    assert_eq!(begin_with(&dir, 10, 1_000, dead), None, "an empty folder");
    std::fs::write(dir.join("crash-100.txt"), report("a.rs:1:1", "from an older version")).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a report").unwrap();
    std::fs::remove_file(dir.join("running-10")).ok();
    assert_eq!(begin_with(&dir, 11, 2_000, dead), None, "old reports without a running marker announce nothing");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_clean_quit_after_a_survived_panic_announces_nothing_and_the_report_says_so() {
    let dir = tmp("survived");
    assert_eq!(begin_with(&dir, 10, 1_000, dead), None);
    assert!(dir.join("running-10").exists(), "the run is marked");
    // The panic of a reader thread: a report is written, the app goes on.
    let path = write_report(&dir, 1_050, &report("src/lsp.rs:5:5", "reader exploded")).unwrap();
    session::end_with(&dir, 10, std::slice::from_ref(&path));
    assert!(!dir.join("running-10").exists(), "a clean exit removes the marker");
    assert!(path.exists(), "the report stays for a bug report");
    let marked = std::fs::read_to_string(&path).unwrap();
    assert!(marked.contains("fatal: false") && !marked.contains(FATAL_UNKNOWN), "{marked}");
    assert_eq!(begin_with(&dir, 11, 2_000, dead), None, "the next start has nothing to announce");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_killed_process_is_announced_exactly_once_with_the_report_it_left() {
    let dir = tmp("killed");
    assert_eq!(begin_with(&dir, 10, 1_000, dead), None);
    std::fs::write(dir.join("crash-1500.txt"), report("src/x.rs:4:2", "index out of bounds")).unwrap();
    // No clean exit: the marker of process 10 is still there. The next run, process 11, finds it.
    let got = begin_with(&dir, 11, 2_000, dead).expect("announced");
    assert_eq!(got, PreviousCrash { location: Some("src/x.rs:4:2".into()), message: Some("index out of bounds".into()) });
    assert!(!dir.join("running-10").exists() && dir.join("running-11").exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_kill_without_any_report_is_still_announced_once_without_details() {
    let dir = tmp("kill9");
    assert_eq!(begin_with(&dir, 10, 1_000, dead), None);
    let got = begin_with(&dir, 11, 2_000, dead).expect("announced");
    assert_eq!(got, PreviousCrash::default());
    std::fs::remove_file(dir.join("running-11")).unwrap(); // 11 quit cleanly
    assert_eq!(begin_with(&dir, 12, 3_000, dead), None, "once and only once");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_report_older_than_the_unfinished_run_is_not_its_cause() {
    let dir = tmp("older");
    std::fs::write(dir.join("crash-100.txt"), report("old.rs:1:1", "an earlier run")).unwrap();
    std::fs::write(dir.join("running-10"), "10\n1000\n0.9.1\n").unwrap();
    assert_eq!(begin_with(&dir, 11, 2_000, dead), Some(PreviousCrash::default()));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_marker_with_a_future_start_time_does_not_hide_the_crash() {
    let dir = tmp("future");
    std::fs::write(dir.join("running-10"), "10\n99999999999\n0.9.1\n").unwrap();
    assert!(begin_with(&dir, 11, 2_000, dead).is_some(), "no clock comparison decides it");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_run_that_is_still_going_is_not_a_crash_but_a_reused_process_id_is() {
    let dir = tmp("alive");
    std::fs::write(dir.join("running-10"), "10\n1000\n0.9.1\n").unwrap();
    assert_eq!(begin_with(&dir, 11, 2_000, |pid| pid == 10), None, "another Codeyard that is running");
    assert!(dir.join("running-10").exists(), "its marker is left alone");
    let eight_days = 1_000 + 8 * 24 * 3600;
    assert!(begin_with(&dir, 12, eight_days, |pid| pid == 10).is_some(), "alive but a week old: a reused process id");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_corrupt_marker_counts_as_an_unfinished_run() {
    let dir = tmp("corrupt");
    for junk in ["", "garbage", "\u{0}\u{1}", "x\ny\nz"] {
        std::fs::write(dir.join("running-10"), junk).unwrap();
        assert!(begin_with(&dir, 11, 2_000, dead).is_some(), "marker {junk:?}");
        std::fs::remove_file(dir.join("running-11")).ok();
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn a_read_only_folder_never_panics() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp("readonly");
    std::fs::write(dir.join("running-10"), "10\n1000\n0.9.1\n").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let _ = begin_with(&dir, 11, 2_000, dead);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

/// The running marker belongs to a window run only: `begin` is called after every command-line exit of `main`, so `--version`,
/// `--help`, an extension install and the other CLI paths never write a marker, and never delete another run's.
#[test]
fn only_a_window_run_marks_itself_as_running() {
    let main = include_str!("main.rs");
    let begin = main.find("crash::session::begin(").expect("main starts a session");
    assert_eq!(main.matches("crash::session::begin(").count(), 1);
    let start = main.find("fn main()").expect("main");
    let window = main[start..].find("Application::new()").map(|i| i + start).expect("the window run");
    assert!(begin < window && main[begin..window].lines().count() < 8, "begin sits right before the application is created");
    for exit in ["forward_callback_from_command_line", "accept_permissions_misuse(&args)", "install_extensions(", "update_extensions()", "askpass::client_main"]
    {
        let at = main[start..].find(exit).map(|i| i + start).unwrap_or_else(|| panic!("{exit} in main"));
        assert!(at < begin, "{exit} comes before begin");
    }
    let parse = main.find("std::process::exit(0);").expect("-V/--help exits");
    assert!(parse < start, "the argument parser (with -V and --help) runs outside the session");
    assert!(!main[..start].contains("crash::session::begin"), "no helper called before main starts a session");
}

#[test]
fn a_stale_marker_survives_command_line_runs_and_the_next_window_run_announces_it() {
    let dir = tmp("cli");
    assert_eq!(begin_with(&dir, 10, 1_000, dead), None);
    // Process 10 is killed. Command-line runs (--version, --install-extension) never call `begin_with`, so nothing touches the marker.
    assert!(dir.join("running-10").exists());
    let got = begin_with(&dir, 11, 2_000, dead);
    assert_eq!(got, Some(PreviousCrash::default()), "the next window run announces the crash");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn a_terminating_signal_removes_the_marker_but_a_kill_cannot() {
    let dir = tmp("signal");
    let marker = dir.join("running-77");
    std::fs::write(&marker, "77\n1000\n0.9.1\n").unwrap();
    assert!(session::unlink_marker_for_test(&marker), "the handler's path is set once per process");
    assert!(!marker.exists(), "what the signal handler does");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn a_process_that_may_not_be_signalled_still_counts_as_alive() {
    // pid 1 belongs to root: kill(1, 0) answers EPERM for an ordinary user, and the process exists either way.
    assert!(session::process_alive_for_test(1));
    assert!(session::process_alive_for_test(std::process::id()));
    assert!(!session::process_alive_for_test(u32::MAX >> 2), "no such process");
}

#[test]
fn a_report_in_another_shape_is_summarized_as_far_as_possible() {
    assert_eq!(summarize(""), PreviousCrash::default());
    assert_eq!(summarize(&report("l:1:1", &"é".repeat(1_000))).message.map(|m| m.chars().count()), Some(300));
}

#[test]
fn crash_file_names_sort_by_time_then_sequence() {
    assert_eq!(crash_key("crash-12.txt"), Some((12, 0)));
    assert_eq!(crash_key("crash-12-3.txt"), Some((12, 3)));
    assert_eq!(crash_key("crash-x.txt"), None);
    assert_eq!(crash_key("running-12"), None);
    assert!(crash_key("crash-9-1.txt") < crash_key("crash-10.txt"));
}
