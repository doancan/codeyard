use super::*;
use std::path::Path;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-log-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&d).ok();
    d
}

#[test]
fn a_line_has_time_level_area_and_one_line_of_message() {
    // 2026-11-12T10:00:00.123Z
    let line = format_line(1_794_477_600_123, Level::Warn, "update", "first\nsecond\t\u{7}third", None);
    assert_eq!(line, "2026-11-12T10:00:00.123Z WARN  update: first second  third");
}

#[test]
fn a_line_is_redacted_before_it_is_kept() {
    let line = format_line(0, Level::Error, "git", "failed in /Users/dogan/work with token=abc123 via bob@host.example", Some("/Users/dogan"));
    assert!(line.ends_with("failed in ~/work with token=[redacted] via ***@host.example"), "{line}");
}

#[test]
fn a_runaway_message_is_cut() {
    let line = format_line(0, Level::Info, "x", &"a ".repeat(50_000), None);
    assert!(line.chars().count() < MAX_LINE_CHARS + 100, "{}", line.len());
}

#[test]
fn levels_parse_and_order_by_severity() {
    assert_eq!(Level::parse("warn"), Some(Level::Warn));
    assert_eq!(Level::parse("verbose"), None);
    assert!(Level::Error < Level::Debug);
    for name in Level::NAMES {
        assert!(Level::parse(name).is_some(), "{name}");
    }
}

#[test]
fn the_file_rotates_and_keeps_three_files() {
    let dir = tmp("rotate");
    let mut files = Files::new(dir.clone(), FILE_NAME, 100);
    for i in 0..40 {
        files.append(&format!("line {i:03} {}", "x".repeat(20)));
    }
    files.flush();
    let names: Vec<String> = {
        let mut n: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        n.sort();
        n
    };
    assert_eq!(names, ["codeyard.log", "codeyard.log.1", "codeyard.log.2"]);
    for n in 0..FILES {
        let size = std::fs::metadata(files.path(n)).unwrap().len();
        assert!(size <= 100, "file {n} has {size} bytes");
    }
    // The newest line is in the current file, and the oldest kept lines are in older files, in order.
    let all = read_all(&dir, FILE_NAME);
    assert!(all.trim_end().ends_with("line 039 xxxxxxxxxxxxxxxxxxxx"), "{all}");
    assert!(!all.contains("line 000"), "the oldest lines are gone");
    let numbers: Vec<u32> = all.lines().map(|l| l[5..8].parse().unwrap()).collect();
    assert!(numbers.windows(2).all(|w| w[0] < w[1]), "{numbers:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_existing_log_is_appended_to_and_an_oversized_one_rotated() {
    let dir = tmp("append");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(FILE_NAME), "old line\n").unwrap();
    let mut files = Files::new(dir.clone(), FILE_NAME, 1_000);
    files.append("new line");
    files.flush();
    assert_eq!(std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(), "old line\nnew line\n");
    std::fs::write(dir.join(FILE_NAME), "z".repeat(5_000)).unwrap();
    let mut files = Files::new(dir.clone(), FILE_NAME, 1_000);
    files.append("fresh");
    files.flush();
    assert_eq!(std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(), "fresh\n");
    assert_eq!(std::fs::read_to_string(dir.join("codeyard.log.1")).unwrap().len(), 5_000);
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn the_log_folder_and_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp("private").join("logs");
    let mut files = Files::new(dir.clone(), FILE_NAME, 1_000);
    files.append("x");
    files.flush();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!((mode(&dir), mode(&dir.join(FILE_NAME))), (0o700, 0o600));
    std::fs::remove_dir_all(dir.parent().unwrap()).ok();
}

#[test]
fn a_folder_that_cannot_be_used_is_not_an_error() {
    let blocker = tmp("blocked");
    std::fs::write(&blocker, "a file where the folder should be").unwrap();
    let mut files = Files::new(blocker.join("logs"), FILE_NAME, 1_000);
    files.append("lost");
    files.flush();
    assert!(files.out.is_none());
    std::fs::remove_file(&blocker).ok();
}

#[test]
fn the_writer_thread_writes_and_flush_waits_for_it() {
    let dir = tmp("writer");
    let writer = Writer::spawn(Files::new(dir.clone(), FILE_NAME, 10_000)).expect("thread starts");
    writer.send("one".into());
    writer.send("two".into());
    writer.flush_within(FLUSH_WAIT);
    assert_eq!(read_all(&dir, FILE_NAME), "one\ntwo\n");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_flood_never_blocks_the_caller_and_only_the_overflow_is_dropped() {
    let dir = tmp("flood");
    let writer = Writer::spawn(Files::new(dir.clone(), FILE_NAME, 10_000_000)).expect("thread starts");
    let started = std::time::Instant::now();
    for i in 0..50_000 {
        writer.send(format!("line {i}"));
    }
    assert!(started.elapsed().as_secs() < 5, "sending must not wait for the disk");
    writer.flush_within(FLUSH_WAIT);
    let written = read_all(&dir, FILE_NAME).lines().count();
    assert_eq!(written + writer.dropped(), 50_000);
    assert!(written > 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_memory_copy_is_bounded_and_keeps_the_newest() {
    for i in 0..(RING_LINES + 50) {
        remember(&format!("ring-test {i} {}", "y".repeat(2_000)));
    }
    let lines = tail(10_000);
    assert!(lines.len() <= RING_LINES);
    assert!(lines.iter().all(|l| l.chars().count() <= RING_LINE_CHARS + 1));
    assert!(lines.iter().any(|l| l.starts_with("ring-test")));
}

#[test]
fn the_daemon_has_its_own_file_and_never_touches_the_apps_rotated_files() {
    let dir = tmp("two-writers");
    let mut app = Files::new(dir.clone(), FILE_NAME, 100);
    let mut daemon = Files::new(dir.clone(), DAEMON_FILE_NAME, 100);
    for i in 0..30 {
        app.append(&format!("app line {i:03} {}", "x".repeat(20)));
        daemon.append(&format!("daemon line {i:03} {}", "y".repeat(20)));
    }
    app.flush();
    daemon.flush();
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["codeyard-daemon.log", "codeyard-daemon.log.1", "codeyard-daemon.log.2", "codeyard.log", "codeyard.log.1", "codeyard.log.2"]);
    let app_text = read_all(&dir, FILE_NAME);
    let daemon_text = read_all(&dir, DAEMON_FILE_NAME);
    assert!(app_text.contains("app line 029") && !app_text.contains("daemon"), "{app_text}");
    assert!(daemon_text.contains("daemon line 029") && !daemon_text.contains("app line"), "{daemon_text}");
    // Rotation names: the base name, then `.1` and `.2`.
    assert_eq!(app.path(0).file_name().unwrap(), "codeyard.log");
    assert_eq!(daemon.path(2).file_name().unwrap(), "codeyard-daemon.log.2");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_area_cannot_start_a_second_line_or_field() {
    let line = format_line(0, Level::Warn, "evil\n2026-01-01T00:00:00.000Z ERROR fake: boom\r: x", "message", None);
    assert!(!line.contains('\n') && !line.contains('\r'), "{line:?}");
    assert!(line.contains("WARN  evil_2026-01-01T00_00_00.000Z_ER: message"), "{line}");
    let long = format_line(0, Level::Info, &"a".repeat(500), "m", None);
    assert!(long.len() < 100, "{long}");
}

#[test]
fn a_secret_at_the_cut_does_not_leave_its_first_half_in_the_line() {
    let token = "ghp_0123456789abcdefghijABCDEFGHIJ012345";
    let message = format!("{}{token} and more", "x ".repeat(995));
    let line = format_line(0, Level::Info, "x", &message, None);
    assert!(!line.contains("ghp_"), "{line}");
    let doubled = format_line(0, Level::Info, "x", r"opened C:\\Users\\bob\\proj", None);
    assert!(!doubled.contains("bob"), "{doubled}");
}

#[test]
fn the_hook_path_gives_up_instead_of_waiting_for_a_locked_ring() {
    let held = RING.lock().unwrap_or_else(|e| e.into_inner());
    let (done, finished) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        error_from_hook("crash", format_args!("a panic while another thread holds the ring"));
        assert!(try_tail(10).is_empty(), "a contended ring gives nothing instead of waiting");
        done.send(()).ok();
    });
    assert!(finished.recv_timeout(Duration::from_secs(5)).is_ok(), "the hook path must not block on the ring lock");
    worker.join().unwrap();
    drop(held);
}
