//! API 1.1 end to end with the probe extension (crates/ext-host/testdata/probe.wasm) in a real codeyard-exthost:
//! document and file watches get what the extension may read, `core:editor.open` selects a range, and a server named
//! in a `net-from-setting` setting is reached only after the user allowed it on the extension page. Skipped when code
//! extensions cannot run here (no codeyard-exthost or sandbox).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe.wasm");

const MANIFEST: &str = r#"[extension]
id = "you.probe"
version = "1.0.0"
name = "Probe"
license = "MIT"

[code]
entry = "Cargo.toml"

# The probe also imports net, auth and secrets (for the exthost's own tests).
[permissions]
read = ["src/**/*.rs"]
net-from-setting = "server"
auth = ["github"]
secrets = true
commands = ["core:editor.open"]
why = "Tests every part of the host."

[[commands]]
id = "watched"
title = "Probe: Watched"

[settings.server]
type = "string"
default = ""
description = "Your GitHub Enterprise host."
"#;

fn skipped() -> bool {
    match crate::ext_runtime::code_runs_here(&"tests") {
        Ok(()) => false,
        Err(why) => {
            eprintln!("skipped: {why}");
            true
        }
    }
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extwatch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn setup<'a>(cx: &'a mut TestAppContext, cfg: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: MANIFEST.as_bytes().to_vec(), executable: false },
        ext_pkg::Entry { path: "Cargo.toml".into(), data: Vec::new(), executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: PROBE.to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).expect("the probe passes the import check");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "/downloads/probe.cyx".into() }).expect("installs");
    std::fs::write(cfg.join("keybindings.jsonc"), "[]").unwrap();
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let dir = cfg.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&gpui::Focusable::focus_handle(wb, cx)));
    cx.run_until_parked();
    (wb, cx)
}

/// Run the window until `done`, or fail after 20 s. Each round moves the test clock on a little, for the debounce and
/// the delivery interval; the file watcher and the exthost are real.
fn until(cx: &mut VisualTestContext, what: &str, done: impl Fn(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20 * crate::test_sshd::timeout_scale());
    while Instant::now() < deadline {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        cx.executor().advance_clock(Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// Run a probe command with a JSON string argument and wait for its answer.
fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, command: &str, arg: &str) -> String {
    run_raw(wb, cx, command, &serde_json::Value::String(arg.to_string()).to_string())
}

/// Run a probe command with `args` as they are (the probe's `fetch` takes the URL itself).
fn run_raw(wb: &Entity<Workbench>, cx: &mut VisualTestContext, command: &str, args: &str) -> String {
    wb.update_in(cx, |wb, _, _| wb.forget_extension_answer_for_test("you.probe"));
    wb.update_in(cx, |wb, window, cx| wb.run_extension_command_with_for_test("you.probe", command, args, window, cx));
    let wb2 = wb.clone();
    until(cx, command, move |cx| wb2.read_with(cx, |wb, _| wb.extension_answer_for_test("you.probe")).is_some());
    let answer = wb.read_with(cx, |wb, _| wb.extension_answer_for_test("you.probe")).unwrap_or_default();
    serde_json::from_str::<String>(&answer).unwrap_or(answer)
}

/// What the probe's watches recorded so far, one event per line.
fn watched(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    run(wb, cx, "watched", "")
}

fn wait_watched(wb: &Entity<Workbench>, cx: &mut VisualTestContext, what: &str, want: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(20 * crate::test_sshd::timeout_scale());
    loop {
        let seen = watched(wb, cx);
        if want(&seen) {
            return seen;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}; the probe saw:\n{seen}");
        cx.executor().advance_clock(Duration::from_millis(200));
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A document watch gets `opened` for what is already open and then `changed` (once typing pauses) and `saved`, with
/// the version and, when asked, the text; never for a file outside `read`.
#[gpui::test]
fn document_watches_see_open_edit_and_save_of_readable_files(cx: &mut TestAppContext) {
    if skipped() {
        return;
    }
    let cfg = temp("docs-cfg");
    let root = temp("docs-root");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "fn b() {}\n").unwrap();
    std::fs::write(root.join("notes.md"), "private\n").unwrap();
    let (wb, cx) = setup(cx, &cfg);
    let root_s = root.to_string_lossy().replace('\\', "/");
    wb.update(cx, |wb, cx| wb.set_root(root_s.clone(), cx));
    let open = |wb: &Entity<Workbench>, cx: &mut VisualTestContext, rel: &str| {
        let path = format!("{root_s}/{rel}");
        wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(path, window, cx));
        cx.run_until_parked();
    };
    open(&wb, cx, "src/a.rs");
    open(&wb, cx, "notes.md");

    assert_eq!(run(&wb, cx, "watch-docs", "text"), "Ok(1)");
    let uri = |rel: &str| crate::ext_broker::uri(&format!("{root_s}/{rel}"));
    let seen = wait_watched(&wb, cx, "the open document", |s| s.contains("opened"));
    assert_eq!(seen.lines().count(), 1, "{seen}");
    assert!(seen.starts_with(&format!("1 opened {} rust v", uri("src/a.rs"))), "{seen}");
    assert!(seen.ends_with(" Some(\"fn a() {}\\n\")\n"), "{seen}");

    // Typing sends one `changed` once it pauses, with the latest version; saving sends `saved`.
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).expect("editor");
    open(&wb, cx, "src/a.rs");
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap_or(ed);
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "// x\n", cx));
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "// y\n", cx));
    let seen = wait_watched(&wb, cx, "the change", |s| s.contains("changed"));
    let changed: Vec<&str> = seen.lines().filter(|l| l.contains("changed")).collect();
    assert_eq!(changed.len(), 1, "one event for the pause: {seen}");
    assert!(changed[0].contains("// y\\n// x\\nfn a() {}"), "{seen}");
    ed.update(cx, |e, cx| e.save_now(cx));
    let seen = wait_watched(&wb, cx, "the save", |s| s.contains("saved"));
    let version = |l: &str| l.split(' ').find(|w| w.starts_with('v')).map(str::to_string);
    let saved = seen.lines().find(|l| l.contains("saved")).unwrap();
    assert_eq!(version(saved), version(changed[0]), "{seen}");

    // Another readable file opens; `notes.md` (outside `read`) never reached the extension.
    open(&wb, cx, "src/b.rs");
    let seen = wait_watched(&wb, cx, "the second document", |s| s.contains("b.rs"));
    assert!(!seen.contains("notes.md"), "{seen}");
}

/// A file watch reports files matching its glob that are created, changed and deleted outside Codeyard, coalesced,
/// and only those the extension may read; a stopped watch reports nothing.
#[gpui::test]
fn file_watches_see_changes_on_disk(cx: &mut TestAppContext) {
    if skipped() {
        return;
    }
    let cfg = temp("files-cfg");
    let root = temp("files-root");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src/a.txt"), "not rust\n").unwrap();
    let (wb, cx) = setup(cx, &cfg);
    let root_s = root.to_string_lossy().replace('\\', "/");
    wb.update(cx, |wb, cx| wb.set_root(root_s.clone(), cx));
    cx.run_until_parked();
    let uri = |rel: &str| crate::ext_broker::uri(&format!("{root_s}/{rel}"));

    assert_eq!(run(&wb, cx, "watch-files", "**/*.rs"), "Ok(1)");
    // The listing that later ones compare with, and the folders it follows.
    let wb2 = wb.clone();
    until(cx, "the watched folders", move |cx| wb2.read_with(cx, |wb, _| wb.watched_for_test().iter().any(|p| p.ends_with("/src/a.rs"))));

    std::fs::write(root.join("src/a.rs"), "fn a() { changed() }\n").unwrap();
    std::fs::write(root.join("src/new.rs"), "fn new() {}\n").unwrap();
    std::fs::write(root.join("src/new.txt"), "outside the glob\n").unwrap();
    let seen = wait_watched(&wb, cx, "the changes", |s| s.contains("changed") && s.contains("created"));
    assert!(seen.contains(&format!("1 changed {}", uri("src/a.rs"))), "{seen}");
    assert!(seen.contains(&format!("1 created {}", uri("src/new.rs"))), "{seen}");
    assert!(!seen.contains("new.txt") && !seen.contains("a.txt"), "{seen}");

    std::fs::remove_file(root.join("src/new.rs")).unwrap();
    let seen = wait_watched(&wb, cx, "the deletion", |s| s.contains("deleted"));
    assert!(seen.contains(&format!("1 deleted {}", uri("src/new.rs"))), "{seen}");
    assert_eq!(seen.lines().filter(|l| l.contains("src/a.rs")).count(), 1, "coalesced: {seen}");

    // Stopped: nothing more, and the exthost may be stopped as idle again.
    // `unwatch` answers with nothing, so wait for its effect.
    wb.update_in(cx, |wb, window, cx| wb.run_extension_command_with_for_test("you.probe", "unwatch", "\"1\"", window, cx));
    let wb2 = wb.clone();
    until(cx, "the watch to stop", move |cx| wb2.read_with(cx, |wb, _| !wb.extension_watching_for_test("you.probe")));
    let before = watched(&wb, cx);
    std::fs::write(root.join("src/later.rs"), "fn later() {}\n").unwrap();
    for _ in 0..30 {
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(watched(&wb, cx), before);
}

/// `core:editor.open` with a selection opens the file and selects the range.
#[gpui::test]
fn editor_open_selects_the_range(cx: &mut TestAppContext) {
    if skipped() {
        return;
    }
    let cfg = temp("open-cfg");
    let root = temp("open-root");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    let (wb, cx) = setup(cx, &cfg);
    let root_s = root.to_string_lossy().replace('\\', "/");
    wb.update(cx, |wb, cx| wb.set_root(root_s.clone(), cx));
    let uri = crate::ext_broker::uri(&format!("{root_s}/src/a.rs"));
    let arg = serde_json::json!({"uri": uri, "selection": {"start": {"line": 1, "character": 3}, "end": {"line": 1, "character": 4}}});
    assert_eq!(run(&wb, cx, "exec-json", &format!("core:editor.open {arg}")), "done");
    let wb2 = wb.clone();
    until(cx, "the selection", move |cx| wb2.read_with(cx, |wb, cx| wb.active_editor_entity().is_some_and(|e| e.read(cx).lsp_selection() == ((1, 3), (1, 4)))));
    // A plain URI still opens the file, and a malformed argument says what it takes.
    assert_eq!(run(&wb, cx, "exec", &format!("core:editor.open {uri}")), "done");
    let bad = run(&wb, cx, "exec-json", r#"core:editor.open {"uri": 1}"#);
    assert!(bad.contains("takes a `file://` URI, or"), "{bad}");
}

/// `net-from-setting`: a server is refused, with the fix that only the user can apply, until the user allows it on the
/// extension page through a system dialog that names the extension, its publisher and the host; then requests reach it.
#[gpui::test]
fn a_server_from_a_setting_is_reached_only_after_the_user_allows_it(cx: &mut TestAppContext) {
    if skipped() {
        return;
    }
    let cfg = temp("hosts-cfg");
    // No folder: nothing to trust, so `net` applies.
    let (wb, cx) = setup(cx, &cfg);
    let fake = std::sync::Arc::new(crate::ext_fetch::tests::Fake::default().route("https://ghe.example.com/", 200, "{}"));
    wb.update(cx, |wb, _| wb.set_ext_transport_for_test(fake.clone()));
    let url = "https://ghe.example.com/api/v3/user";
    assert_eq!(run_raw(&wb, cx, "fetch", url), r#"denied net-from-setting host = "ghe.example.com" Why::NotDeclared"#);
    assert!(fake.sent().is_empty());

    let allow = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| {
        wb.update_in(cx, |wb, window, cx| wb.allow_host_for_test("you.probe", "https://GHE.example.com/", window, cx));
        cx.run_until_parked();
        let notice = wb.read_with(cx, |wb, _| wb.notice_for_test());
        assert!(cx.has_pending_prompt(), "no dialog; notice: {notice:?}");
    };
    allow(&wb, cx);
    crate::question::cancel_keeps_focus(&wb, cx, "Cancel");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.extension_user_hosts_for_test("you.probe")).is_empty(), "cancelled: nothing stored");

    allow(&wb, cx);
    let (title, detail) = cx.pending_prompt().unwrap();
    assert!(title.contains("Probe") && title.contains("ghe.example.com"), "{title}");
    assert!(detail.contains("installed from /downloads/probe.cyx"), "{detail}");
    cx.simulate_prompt_answer("Allow Host");
    let wb2 = wb.clone();
    until(cx, "the host", move |cx| !wb2.read_with(cx, |wb, _| wb.extension_user_hosts_for_test("you.probe")).is_empty());
    assert_eq!(wb.read_with(cx, |wb, _| wb.extension_user_hosts_for_test("you.probe")), ["ghe.example.com"]);
    assert_eq!(run_raw(&wb, cx, "fetch", url), "done body {}");
    assert_eq!(fake.sent().len(), 1);

    // Turning the permission off on the extension page refuses it again, without a restart.
    wb.update(cx, |wb, cx| wb.set_permission_for_test("you.probe", "net-from-setting", "server", false, cx));
    let refused = run_raw(&wb, cx, "fetch", url);
    assert!(refused.starts_with("denied"), "{refused}");
    assert_eq!(fake.sent().len(), 1);
}
