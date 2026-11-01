//! Code lenses and inlay hints from code extensions end to end (#143): the probe extension
//! (crates/ext-host/testdata/probe.wasm) runs in a real codeyard-exthost and gives lenses and hints for TOML files.
//! Skipped where codeyard-exthost cannot run (not built, or no OS sandbox: `ext_runtime::code_runs_for_test`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

use crate::editor::EditorView;
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

# The probe imports these (for the exthost's own tests); nothing here uses them.
[permissions]
read = ["a.txt"]
net = ["api.example.com"]
auth = ["github"]
secrets = true
why = "Tests every part of the host."

[[commands]]
id = "echo"
title = "Probe: Echo"

[[code-lens]]
languages = ["toml"]

[[inlay-hints]]
languages = ["toml"]
"#;

/// More than the debounce after an edit or a scroll (workbench_ext_lenses.rs).
const PAUSE: Duration = Duration::from_millis(400);

fn exthost_missing() -> bool {
    !crate::ext_runtime::code_runs_for_test()
}

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extlens-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn setup<'a>(cx: &'a mut TestAppContext, cfg: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: MANIFEST.as_bytes().to_vec(), executable: false },
        ext_pkg::Entry { path: "Cargo.toml".into(), data: Vec::new(), executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: PROBE.to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).expect("the probe passes the import check");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "/downloads/probe.cyx".into() }).expect("installs");
    std::fs::write(cfg.join("settings.jsonc"), "{}").unwrap();
    std::fs::write(cfg.join("keybindings.jsonc"), "[]").unwrap();
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, minimap: false, ..Default::default() });
    });
    let dir = cfg.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    (wb, cx)
}

fn open(wb: &Entity<Workbench>, path: &Path, cx: &mut VisualTestContext) -> Entity<EditorView> {
    let p = path.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&p, window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).expect("editor opened");
    draw(cx);
    ed
}

/// The test platform draws a scheduled frame on input: a mouse move over the editor draws it (and reports its lines).
fn draw(cx: &mut VisualTestContext) {
    cx.simulate_mouse_move(gpui::point(gpui::px(500.), gpui::px(300.)), None, Modifiers::none());
    cx.run_until_parked();
}

/// Let the debounce pass (on the test clock), then run the window's tasks until `done`, up to 20 s: the exthost is a
/// real process.
fn wait_for(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    cx.executor().advance_clock(PAUSE);
    let deadline = Instant::now() + Duration::from_secs(20 * crate::test_sshd::timeout_scale());
    while Instant::now() < deadline {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out: {what}");
}

fn shown(ed: &Entity<EditorView>, cx: &mut VisualTestContext) -> (Vec<(usize, String)>, Vec<(usize, String)>) {
    ed.read_with(cx, |e, _| e.ext_lenses_for_test())
}

fn view(ed: &Entity<EditorView>, cx: &mut VisualTestContext) -> std::ops::Range<usize> {
    ed.read_with(cx, |e, _| e.ext_view())
}

/// 300 lines: a `[section]` every tenth line, `key = number` otherwise; a lens resolved on demand on line 3 and one
/// running a core command on line 5.
fn document(dir: &Path, name: &str, extra: &str) -> PathBuf {
    let mut text = String::new();
    for i in 0..300 {
        let line = match i {
            3 => "#later".to_string(),
            5 => "#core".to_string(),
            i if i % 10 == 0 => format!("[s{i}]"),
            i => format!("k{i} = {i}"),
        };
        text.push_str(&line);
        text.push('\n');
    }
    text.push_str(extra);
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// AC1: lenses and hints appear for the lines in view only, next to their line, and follow edits and scrolling.
#[gpui::test]
fn lenses_and_hints_show_for_the_lines_in_view_and_follow_edits(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("view");
    let (wb, cx) = setup(cx, &cfg);
    let dir = config_dir("view-files");
    let ed = open(&wb, &document(&dir, "a.toml", ""), cx);
    wait_for(cx, "lenses and hints", |cx| {
        let (lenses, hints) = shown(&ed, cx);
        lenses.iter().any(|l| l == &(3, "Resolved".to_string())) && hints.contains(&(1, ": int".to_string()))
    });
    let v = view(&ed, cx);
    assert!(v.start == 0 && v.end > 20 && v.end < 200, "the view: {v:?}");
    let (lenses, hints) = shown(&ed, cx);
    assert!(lenses.contains(&(0, "Run s0".into())) && lenses.contains(&(10, "Run s10".into())), "{lenses:?}");
    assert!(lenses.iter().chain(&hints).all(|(l, _)| v.contains(l)), "only lines in view: {v:?} {lenses:?} {hints:?}");
    assert!(!hints.iter().any(|(l, _)| *l >= 200));
    // Shown at the end of the line with the language server's, and the tooltip names the extension.
    assert_eq!(ed.read_with(cx, |e, _| e.eol_hint(1)), Some(": int".into()));
    assert!(ed.read_with(cx, |e, _| e.ext_eol_tooltip(0)).is_some_and(|t| t.contains("Run s0") && t.contains("you.probe")));

    // An edit that keeps the line count: the hint follows the new text.
    ed.update(cx, |e, cx| e.insert_at_for_test("[s0]\nk1 = ".len(), "\"a\"", cx));
    wait_for(cx, "the hint after the edit", |cx| shown(&ed, cx).1.contains(&(1, ": string".to_string())));
    // A new line: what was shown is stale at once, then comes back on the moved lines.
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "\n", cx));
    assert_eq!(shown(&ed, cx), (vec![], vec![]), "nothing stale is shown");
    wait_for(cx, "lenses on the moved lines", |cx| shown(&ed, cx).0.contains(&(1, "Run s0".to_string())));

    // Scrolling asks for the new lines in view.
    ed.update_in(cx, |e, window, cx| e.go_to_line(251, window, cx));
    draw(cx);
    let v = view(&ed, cx);
    assert!(v.contains(&250), "{v:?}");
    wait_for(cx, "hints after scrolling", |cx| shown(&ed, cx).1.iter().any(|(l, _)| (245..255).contains(l)));
    let (lenses, hints) = shown(&ed, cx);
    assert!(lenses.iter().chain(&hints).all(|(l, _)| v.contains(l)), "only lines in view: {v:?} {lenses:?} {hints:?}");
}

/// AC2: clicking a lens runs its command with the arguments the extension gave, through the broker: its own command
/// runs, a core command its manifest does not list is refused.
#[gpui::test]
fn a_lens_runs_its_command_with_its_arguments(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("run");
    let (wb, cx) = setup(cx, &cfg);
    let dir = config_dir("run-files");
    let ed = open(&wb, &document(&dir, "a.toml", ""), cx);
    wait_for(cx, "lenses", |cx| shown(&ed, cx).0.iter().any(|l| l == &(3, "Resolved".to_string())));
    let answer = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.extension_answer_for_test("you.probe"));

    assert!(ed.update(cx, |e, cx| e.click_ext_lens(10, cx)));
    wait_for(cx, "the lens command", |cx| answer(cx).as_deref() == Some(r#"{"section":"s10","line":10}"#));
    // A lens resolved on demand runs what it was resolved with.
    assert!(ed.update(cx, |e, cx| e.click_ext_lens(3, cx)));
    wait_for(cx, "the resolved lens command", |cx| answer(cx).as_deref() == Some(r#""later-3""#));
    // "Run Code Lens on This Line" runs it too (the keyboard's way).
    ed.update_in(cx, |e, window, cx| e.go_to_line(1, window, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Run Code Lens on This Line", window, cx));
    wait_for(cx, "the lens on the cursor's line", |cx| answer(cx).as_deref() == Some(r#"{"section":"s0","line":0}"#));

    // `core:workbench.saveAll` is not in the probe's permissions.commands: refused, shown and logged.
    assert!(ed.update(cx, |e, cx| e.click_ext_lens(5, cx)));
    cx.run_until_parked();
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default();
    assert!(notice.contains("you.probe:") && notice.contains("core:workbench.saveAll"), "{notice}");
    assert!(wb.read_with(cx, |wb, _| wb.extension_log_for_test("you.probe")).iter().any(|l| l.contains("code lens \"Save all\"") && l.contains("refused")));
    assert!(!ed.update(cx, |e, cx| e.click_ext_lens(1, cx)), "a line with only a hint has no lens to run");
}

/// AC3: typing never waits for a lens or hint provider. A provider that sits on its request does not hold up a key;
/// each edit and each scroll cancels what was asked before.
#[gpui::test]
fn typing_never_waits_for_a_lens_provider(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("slow");
    let (wb, cx) = setup(cx, &cfg);
    let dir = config_dir("slow-files");
    // The probe never answers code lenses for a document containing `slow`; it answers hints.
    let ed = open(&wb, &document(&dir, "slow.toml", "slow = 1\n"), cx);
    wait_for(cx, "hints", |cx| shown(&ed, cx).1.contains(&(1, ": int".to_string())));
    let in_flight = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.lens_requests_in_flight_for_test());
    assert_eq!(in_flight(cx), 1, "the lens request the probe sits on");

    let before = ed.read_with(cx, |e, _| e.text());
    for i in 0..20 {
        ed.update(cx, |e, cx| e.insert_at_for_test("[s0]\nk1 = ".len(), "x", cx));
        // Each key is in the text at once, and drawn, with the provider still silent: nothing waits for it.
        assert_eq!(ed.read_with(cx, |e, _| e.text()).len(), before.len() + i + 1);
        draw(cx);
    }
    assert_eq!(in_flight(cx), 0, "the edits cancelled the request about the old text");
    // Typing paused: asked again for the new text, hints come, the lens request waits again.
    wait_for(cx, "hints for the new text", |cx| shown(&ed, cx).1.contains(&(1, ": value".to_string())));
    assert_eq!(in_flight(cx), 1);
    // Moving the cursor is not an edit: the question stands.
    ed.update(cx, |e, cx| {
        e.set_cursor_line_col_for_test(2, 0, cx);
        cx.emit(crate::editor::EditorEvent::Changed);
    });
    cx.run_until_parked();
    assert_eq!(in_flight(cx), 1, "a cursor move cancels nothing");
    // A scroll cancels it and asks for the new lines.
    ed.update_in(cx, |e, window, cx| e.go_to_line(201, window, cx));
    draw(cx);
    wait_for(cx, "hints after scrolling", |cx| shown(&ed, cx).1.contains(&(201, ": int".to_string())));
    assert_eq!(in_flight(cx), 1, "the request for the old lines was cancelled");
    assert!(!wb.read_with(cx, |wb, _| wb.extension_log_for_test("you.probe")).iter().any(|l| l.contains("dropped")), "no answer came late");
}
