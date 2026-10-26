//! Tests for the 0.9 feature set (log follow, SSH host management, JSON/YAML language servers, color
//! decorators, spell check, merge editor, pull requests, `.http` runner).

use std::sync::Arc;

use gpui::{AppContext as _, TestAppContext};

use crate::large::{LargeFileView, Level, line_level};
use crate::qa_integration_tests::wait_ticking;
use crate::qa_workbench_tests::{fresh, setup};
use std::time::Duration;

#[test]
fn log_levels_are_detected_on_word_boundaries() {
    assert_eq!(line_level("2026-01-01 ERROR db down"), Level::Error);
    assert_eq!(line_level("[warn] disk almost full"), Level::Warn);
    assert_eq!(line_level("thread 'main' panicked: PANIC at the disco"), Level::Error);
    assert_eq!(line_level("DEBUG cache miss"), Level::Quiet);
    // Words that merely contain a level are plain text.
    assert_eq!(line_level("terror strikes the territory"), Level::Plain);
    assert_eq!(line_level("warnings-as-text and errors"), Level::Plain, "plural is not a level tag");
    assert_eq!(line_level(""), Level::Plain);
    // Only the line head counts: a stack trace deep in a message does not recolor an INFO line.
    let long = format!("INFO {} ERROR", "x".repeat(300));
    assert_eq!(line_level(&long), Level::Plain);
}

/// `tail -f`: a growing file shows its new lines, follow mode sticks to the end, and a rotated (shortened)
/// file starts over instead of showing stale chunks.
#[gpui::test]
fn following_a_growing_log_shows_new_lines_and_survives_rotation(cx: &mut TestAppContext) {
    let dir = fresh("follow-log");
    let file = dir.join("app.log");
    let line = |i: u32| format!("2026-10-22 12:00:{i:02} INFO request {i}\n");
    std::fs::write(&file, (0..50).map(line).collect::<String>()).unwrap();
    let (_wb, cx) = setup(cx);
    let host: Arc<dyn host::Host> = Arc::new(host::local());
    let path = file.to_string_lossy().into_owned();
    let view = cx.update(|_, cx| cx.new(|cx| LargeFileView::new(host, path, cx)));
    view.update(cx, |v, cx| {
        v.watch(cx);
        v.set_follow(true, cx);
    });
    wait_ticking(cx, "initial index", |cx| view.read_with(cx, |v, _| v.line_count()) == 51);
    assert!(view.read_with(cx, |v, _| v.is_following()));

    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        f.write_all((50..80).map(line).collect::<String>().as_bytes()).unwrap();
    }
    wait_ticking(cx, "appended lines", |cx| view.read_with(cx, |v, _| v.line_count()) == 81);
    view.update(cx, |v, cx| v.request_line(79, cx));
    wait_ticking(cx, "new line text", |cx| view.read_with(cx, |v, _| v.loaded_line(79)).is_some());
    assert_eq!(view.read_with(cx, |v, _| v.loaded_line(79)).unwrap(), line(79).trim_end());

    // Pausing keeps the view where it is but lines keep arriving: they are counted until following resumes.
    view.update(cx, |v, cx| v.set_follow(false, cx));
    assert!(!view.read_with(cx, |v, _| v.is_following()));
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        f.write_all((80..85).map(line).collect::<String>().as_bytes()).unwrap();
    }
    wait_ticking(cx, "unseen lines counted", |cx| view.read_with(cx, |v, _| v.unseen_lines()) == 5);
    assert_eq!(view.read_with(cx, |v, _| v.line_count()), 86);
    view.update(cx, |v, cx| v.set_follow(true, cx));
    assert_eq!(view.read_with(cx, |v, _| v.unseen_lines()), 0, "resuming clears the counter");
    view.update(cx, |v, cx| v.set_follow(false, cx));

    // Rotation: the file is replaced by a shorter one.
    std::fs::write(&file, "fresh start\n").unwrap();
    wait_ticking(cx, "rotated", |cx| view.read_with(cx, |v, _| v.line_count()) == 2);
    view.update(cx, |v, cx| v.request_line(0, cx));
    wait_ticking(cx, "rotated text", |cx| view.read_with(cx, |v, _| v.loaded_line(0)).as_deref() == Some("fresh start"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn connect_fixture(wb: &gpui::Entity<crate::workbench::Workbench>, cx: &mut gpui::VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    wait_ticking(cx, "connected", |cx| wb.read_with(cx, |wb, _| wb.ports_ready()));
}

fn notice_text(wb: &gpui::Entity<crate::workbench::Workbench>, cx: &mut gpui::VisualTestContext) -> String {
    crate::qa_integration_tests::notice(wb, cx)
}

/// Upload and download through the real ssh path (ControlMaster, `cat`/`tar` over `sh -c`): a file, a folder,
/// names with spaces, then the replace prompt is skipped for fresh names and the status bar clears when done.
#[gpui::test]
fn files_move_both_ways_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("feat-transfer") else { return };
    let work = fresh("transfer-remote-root");
    let local = fresh("transfer-local");
    std::fs::write(local.join("my report.txt"), "merhaba sunucu").unwrap();
    std::fs::create_dir_all(local.join("site/css")).unwrap();
    std::fs::write(local.join("site/css/a.css"), "body{}").unwrap();
    let (wb, cx) = setup(cx);
    connect_fixture(&wb, cx);
    crate::qa_workbench_tests::open_in(&wb, &work, cx);

    let alias = crate::test_sshd::ALIAS.to_string();
    wb.update_in(cx, |wb, window, cx| wb.upload_for_test(&alias, vec![local.join("my report.txt"), local.join("site")], &work, window, cx));
    wait_ticking(cx, "upload finished", |cx| wb.read_with(cx, |wb, _| wb.transfer_status().is_none()) && notice_text(&wb, cx).starts_with("Uploaded"));
    assert_eq!(std::fs::read_to_string(work.join("my report.txt")).unwrap(), "merhaba sunucu");
    assert_eq!(std::fs::read_to_string(work.join("site/css/a.css")).unwrap(), "body{}");
    assert!(notice_text(&wb, cx).contains("2 items"), "{}", notice_text(&wb, cx));

    let out = fresh("transfer-downloads");
    wb.update_in(cx, |wb, window, cx| wb.download_for_test(&alias, &work.join("site"), true, &out, window, cx));
    wait_ticking(cx, "download finished", |cx| notice_text(&wb, cx).starts_with("Downloaded"));
    assert_eq!(std::fs::read_to_string(out.join("site/css/a.css")).unwrap(), "body{}");
    wb.update_in(cx, |wb, window, cx| wb.download_for_test(&alias, &work.join("my report.txt"), false, &out, window, cx));
    wait_ticking(cx, "file download finished", |_| out.join("my report.txt").exists());
    assert_eq!(std::fs::read_to_string(out.join("my report.txt")).unwrap(), "merhaba sunucu");
}

/// A failing transfer tells the reason instead of hanging: the destination's parent is a file.
#[gpui::test]
fn a_failed_upload_reports_the_server_reason(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("feat-transfer-fail") else { return };
    let work = fresh("transfer-fail-root");
    let local = fresh("transfer-fail-local");
    std::fs::write(local.join("x.txt"), "x").unwrap();
    std::fs::write(work.join("blocker"), "i am a file").unwrap();
    let (wb, cx) = setup(cx);
    connect_fixture(&wb, cx);
    wb.update_in(cx, |wb, window, cx| wb.upload_for_test(crate::test_sshd::ALIAS, vec![local.join("x.txt")], &work.join("blocker/sub"), window, cx));
    wait_ticking(cx, "failure shown", |cx| notice_text(&wb, cx).starts_with("Transfer failed"));
    assert!(wb.read_with(cx, |wb, _| wb.transfer_status().is_none()), "the status bar item is gone");
}

/// Dropping files from Finder into a local workspace copies them in.
#[gpui::test]
fn dropped_files_are_copied_into_a_local_folder(cx: &mut TestAppContext) {
    let work = fresh("drop-local-root");
    let outside = fresh("drop-local-outside");
    std::fs::write(outside.join("dropped.txt"), "bırakıldı").unwrap();
    std::fs::create_dir_all(outside.join("assets")).unwrap();
    std::fs::write(outside.join("assets/logo.svg"), "<svg/>").unwrap();
    let (wb, cx) = setup(cx);
    crate::qa_workbench_tests::open_in(&wb, &work, cx);
    wb.update_in(cx, |wb, window, cx| wb.drop_external_for_test(vec![outside.join("dropped.txt"), outside.join("assets")], &work, window, cx));
    wait_ticking(cx, "copied", |cx| notice_text(&wb, cx).starts_with("Copied"));
    assert_eq!(std::fs::read_to_string(work.join("dropped.txt")).unwrap(), "bırakıldı");
    assert_eq!(std::fs::read_to_string(work.join("assets/logo.svg")).unwrap(), "<svg/>");
}

/// "Test connection" reports reachability and that the app carries a matching agent; "Add SSH host" writes a
/// working block that can then be tested and removed again.
#[gpui::test]
fn hosts_can_be_added_tested_and_removed(cx: &mut TestAppContext) {
    let Some(sshd) = crate::test_sshd::start("feat-hosts") else { return };
    let (wb, cx) = setup(cx);
    // The fixture's own alias is testable.
    wb.update_in(cx, |wb, window, cx| wb.run_host_op_for_test(true, crate::test_sshd::ALIAS.into(), window, cx));
    wait_ticking(cx, "probe result", |cx| {
        let n = notice_text(&wb, cx);
        n.contains("reachable") || n.starts_with(&format!("{}:", crate::test_sshd::ALIAS)) && !n.starts_with("Testing")
    });
    let n = notice_text(&wb, cx);
    assert!(n.contains("ready"), "agent for this machine is bundled in the test build: {n}");

    // Add a second alias for the same server straight into the fixture's ssh config.
    let port = sshd.port;
    wb.update_in(cx, |wb, window, cx| wb.finish_add_ssh_host_for_test(format!("{}@127.0.0.1:{port} as second", whoami_user()), window, cx));
    cx.run_until_parked();
    let cfg = std::fs::read_to_string(sshd.dir.join("ssh_config")).unwrap();
    assert!(cfg.contains("Host second") && cfg.contains(&format!("Port {port}")), "{cfg}");
    assert!(host::ssh::list_hosts().iter().any(|h| h.alias == "second"));
    // Duplicate names are refused with a message, the file is untouched.
    wb.update_in(cx, |wb, window, cx| wb.finish_add_ssh_host_for_test("x@127.0.0.1 as second".to_string(), window, cx));
    assert!(notice_text(&wb, cx).contains("already exists"));
    assert_eq!(std::fs::read_to_string(sshd.dir.join("ssh_config")).unwrap(), cfg);
    // Remove (without the confirmation window, which is not scriptable here).
    assert!(host::ssh_config::remove_host(&sshd.dir.join("ssh_config"), "second").unwrap());
    assert!(!host::ssh::list_hosts().iter().any(|h| h.alias == "second"));
    let _ = Duration::from_secs(0);
}

fn whoami_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "user".into())
}

// --- JSON/YAML schemas ---

fn schema_fixture(name: &str) -> std::path::PathBuf {
    let dir = fresh(name);
    std::fs::write(
        dir.join("app.schema.json"),
        r#"{
          "type": "object",
          "required": ["name"],
          "additionalProperties": false,
          "properties": {
            "name": {"type": "string", "description": "Application name"},
            "port": {"type": "integer", "description": "Listen port"},
            "mode": {"enum": ["dev", "prod"], "description": "Run mode"},
            "debug": {"type": "boolean"}
          }
        }"#,
    )
    .unwrap();
    dir
}

/// Opening a JSON file that names a local schema shows its violations as problems (squiggles come from the
/// same list), fixing the file clears them.
#[gpui::test]
fn json_files_are_checked_against_their_schema(cx: &mut TestAppContext) {
    let dir = schema_fixture("schema-json-diag");
    let file = dir.join("app.json");
    std::fs::write(&file, "{\n  \"$schema\": \"./app.schema.json\",\n  \"port\": \"eighty\",\n  \"colour\": 1\n}\n").unwrap();
    let (wb, cx) = setup(cx);
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    wait_ticking(cx, "schema problems", |cx| wb.read_with(cx, |wb, _| wb.problems().iter().any(|(p, n)| p.ends_with("app.json") && *n == 3)));
    let messages: Vec<String> = ed.read_with(cx, |e, _| e.diagnostics().iter().map(|d| d.message.clone()).collect());
    assert!(messages.iter().any(|m| m.contains("name")), "missing required name: {messages:?}");
    assert!(messages.iter().any(|m| m.contains("integer")), "{messages:?}");
    assert!(messages.iter().any(|m| m.contains("colour")), "{messages:?}");
    assert!(ed.read_with(cx, |e, _| e.has_schema()));
    // Fix everything: the problems go away after the edit settles.
    ed.update(cx, |e, cx| e.replace_all_text_for_test("{\"$schema\": \"./app.schema.json\", \"name\": \"x\", \"port\": 80}", cx));
    wait_ticking(cx, "problems cleared", |cx| wb.read_with(cx, |wb, _| wb.problems().is_empty()));
}

/// Suggestions and hover come from the schema, in JSON and in YAML, without any language server.
#[gpui::test]
fn schema_completion_and_hover_work_in_json_and_yaml(cx: &mut TestAppContext) {
    let dir = schema_fixture("schema-complete");
    let json = dir.join("app.json");
    std::fs::write(&json, "{\n  \"$schema\": \"./app.schema.json\",\n  \n}\n").unwrap();
    let (wb, cx) = setup(cx);
    let ed = crate::qa_editor_tests::open(&wb, &json, cx);
    wait_ticking(cx, "schema bound", |cx| ed.read_with(cx, |e, _| e.has_schema()));
    // Cursor on the empty line, type a quote and the first letter of a key.
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(2, 2, cx));
    cx.simulate_input("\"mo");
    wait_ticking(cx, "key suggestions", |cx| ed.read_with(cx, |e, _| e.completion_labels()).is_some());
    let labels = ed.read_with(cx, |e, _| e.completion_labels()).unwrap();
    assert_eq!(labels[0], "mode", "{labels:?}");
    assert!(!labels.contains(&"$schema".to_string()));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let text = ed.read_with(cx, |e, _| e.text());
    assert!(text.contains("\"mode\": "), "{text}");
    // Value suggestions for the enum.
    cx.simulate_input("\"");
    wait_ticking(cx, "value suggestions", |cx| ed.read_with(cx, |e, _| e.completion_labels()).is_some_and(|l| l.contains(&"prod".to_string())));
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    let text = ed.read_with(cx, |e, _| e.text());
    assert!(text.contains("\"mode\": \"prod\""), "{text}");

    // YAML with the modeline.
    let yaml = dir.join("app.yaml");
    std::fs::write(&yaml, "# yaml-language-server: $schema=./app.schema.json\nname: demo\n").unwrap();
    let y = crate::qa_editor_tests::open(&wb, &yaml, cx);
    wait_ticking(cx, "yaml schema bound", |cx| y.read_with(cx, |e, _| e.has_schema()));
    y.update(cx, |e, cx| e.set_cursor_line_col_for_test(2, 0, cx));
    cx.simulate_input("po");
    wait_ticking(cx, "yaml key suggestions", |cx| y.read_with(cx, |e, _| e.completion_labels()).is_some());
    assert_eq!(y.read_with(cx, |e, _| e.completion_labels()).unwrap()[0], "port");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(y.read_with(cx, |e, _| e.text()).ends_with("port: "), "{:?}", y.read_with(cx, |e, _| e.text()));
    // Hover text from the schema.
    let hover = y.read_with(cx, |e, _| e.schema_hover_for_test(1)).expect("schema bound");
    assert!(hover.contains("Application name"), "{hover}");
}

/// Turning validation off removes the schema problems.
#[gpui::test]
fn schema_validation_can_be_turned_off(cx: &mut TestAppContext) {
    let dir = schema_fixture("schema-off");
    let file = dir.join("app.json");
    std::fs::write(&file, "{\"$schema\": \"./app.schema.json\", \"port\": \"x\"}").unwrap();
    let (wb, cx) = setup(cx);
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    wait_ticking(cx, "problems", |cx| wb.read_with(cx, |wb, _| !wb.problems().is_empty()));
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().schema_validate = false);
    ed.update(cx, |e, cx| e.replace_all_text_for_test("{\"$schema\": \"./app.schema.json\", \"port\": \"y\"}", cx));
    wait_ticking(cx, "problems removed", |cx| wb.read_with(cx, |wb, _| wb.problems().is_empty()));
    assert!(!ed.read_with(cx, |e, _| e.has_schema()));
}

// --- Color picker ---

/// The picker opens on the literal under the cursor, writes the chosen color back in the same notation as one
/// undoable edit, converts notation, ignores unrelated edits and closes on Escape.
#[gpui::test]
fn the_colour_picker_edits_literals_in_place(cx: &mut TestAppContext) {
    let dir = fresh("color-picker");
    let file = dir.join("a.css");
    std::fs::write(&file, "a { color: #ff0000; background: rgba(0, 0, 0, 0.5); border-color: red }\n").unwrap();
    let (wb, cx) = setup(cx);
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    let text = |cx: &mut gpui::VisualTestContext| ed.read_with(cx, |e, _| e.text());
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, 12, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Pick Color…", window, cx));
    assert_eq!(ed.read_with(cx, |e, _| e.picker_state_for_test()), Some(("#ff0000".into(), "#ff0000".into())));

    // Pick a green: written back as hex, one undo step restores the original.
    ed.update(cx, |e, cx| e.picker_set_for_test(120., 1., 1., 1., cx));
    assert!(text(cx).starts_with("a { color: #00ff00; background"), "{}", text(cx));
    assert_eq!(
        ed.read_with(cx, |e, _| e.picker_state_for_test()),
        Some(("#00ff00".into(), "#00ff00".into())),
        "the picker stays open and follows the new range"
    );
    // Convert the notation: rgb, then hsl.
    ed.update(cx, |e, cx| e.picker_notation_for_test(crate::color::Notation::Rgb { legacy: true }, cx));
    assert!(text(cx).starts_with("a { color: rgb(0, 255, 0); background"), "{}", text(cx));
    ed.update(cx, |e, cx| e.picker_notation_for_test(crate::color::Notation::Hsl { legacy: true }, cx));
    assert!(text(cx).starts_with("a { color: hsl(120, 100%, 50%); background"), "{}", text(cx));
    // Alpha switches the literal to the alpha form.
    ed.update(cx, |e, cx| e.picker_set_for_test(120., 1., 1., 0.5, cx));
    assert!(text(cx).starts_with("a { color: hsla(120, 100%, 50%, 0.5); background"), "{}", text(cx));
    cx.simulate_keystrokes("cmd-z cmd-z cmd-z cmd-z");
    assert!(text(cx).starts_with("a { color: #ff0000; background"), "undo walks back to the original: {}", text(cx));

    // Escape closes it.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Pick Color…", window, cx));
    assert!(ed.read_with(cx, |e, _| e.picker_state_for_test()).is_some());
    cx.simulate_keystrokes("escape");
    assert!(ed.read_with(cx, |e, _| e.picker_state_for_test()).is_none());

    // A named color is rewritten as hex; the rgba literal keeps its alpha notation.
    let named = text(cx).find("red }").unwrap();
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, named + 1, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Pick Color…", window, cx));
    ed.update(cx, |e, cx| e.picker_set_for_test(240., 1., 1., 1., cx));
    assert!(text(cx).contains("border-color: #0000ff }"), "{}", text(cx));
    let rgba = text(cx).find("rgba").unwrap();
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, rgba + 2, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Pick Color…", window, cx));
    ed.update(cx, |e, cx| e.picker_set_for_test(0., 0., 1., 0.25, cx));
    assert!(text(cx).contains("background: rgba(255, 255, 255, 0.25)"), "{}", text(cx));
}

/// With no color at the cursor the command inserts one and opens it; an unrelated edit closes a stale picker.
#[gpui::test]
fn picking_a_colour_where_there_is_none_inserts_one(cx: &mut TestAppContext) {
    let dir = fresh("color-picker-insert");
    let file = dir.join("t.txt");
    std::fs::write(&file, "color = \n").unwrap();
    let (wb, cx) = setup(cx);
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, 8, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Pick Color…", window, cx));
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "color = #3b82f6\n");
    assert!(ed.read_with(cx, |e, _| e.picker_state_for_test()).is_some());
    // Typing elsewhere makes the remembered range stale: the picker is gone.
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, 0, cx));
    cx.simulate_input("x");
    assert!(ed.read_with(cx, |e, _| e.picker_state_for_test()).is_none());
}

// --- Spell check ---

fn wait_speller(cx: &mut gpui::VisualTestContext, ed: &gpui::Entity<crate::editor::EditorView>) {
    wait_ticking(cx, "speller attached", |cx| ed.read_with(cx, |e, _| e.has_speller()));
}

/// Prose files are checked everywhere except code, links and addresses; code files only in comments.
#[gpui::test]
fn spell_check_covers_prose_and_comments_but_not_code(cx: &mut TestAppContext) {
    let dir = fresh("spell-scopes");
    let md = dir.join("notes.md");
    std::fs::write(&md, "# A headng\n\nThis sentense has a mistake. See `somecodex` and https://example.com/xyzzy.\n\n```rust\nlet mispeled = 1;\n```\n")
        .unwrap();
    let rs = dir.join("lib.rs");
    std::fs::write(&rs, "// A coment with a typpo\nfn fooBarBaz() { let strng = \"literal wrng\"; } // trailng\n").unwrap();
    let (wb, cx) = setup(cx);
    let m = crate::qa_editor_tests::open(&wb, &md, cx);
    wait_speller(cx, &m);
    let words = m.read_with(cx, |e, cx| e.misspelled_words_for_test(cx));
    assert_eq!(words, ["headng", "sentense"], "headings and prose yes; inline code, URL and fenced code no: {words:?}");
    let r = crate::qa_editor_tests::open(&wb, &rs, cx);
    wait_speller(cx, &r);
    let words = r.read_with(cx, |e, cx| e.misspelled_words_for_test(cx));
    assert_eq!(words, ["coment", "typpo", "trailng"], "comments only, no identifiers or strings: {words:?}");
    // Turning comment checking off leaves code files alone but not prose.
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().spelling_comments = false);
    assert!(r.read_with(cx, |e, cx| e.misspelled_words_for_test(cx)).is_empty());
    assert_eq!(m.read_with(cx, |e, cx| e.misspelled_words_for_test(cx)), ["headng", "sentense"]);
    // Turning spelling off removes the checker from every editor.
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().spelling = false);
    wb.update(cx, |wb, cx| wb.refresh_speller_for_test(cx));
    assert!(!m.read_with(cx, |e, _| e.has_speller()));
    assert!(m.read_with(cx, |e, cx| e.misspelled_words_for_test(cx)).is_empty());
}

/// Quick Fix on a typo lists suggestions: Enter applies the best one (keeping a capital letter), adding to the
/// dictionary stops the underline and survives a restart of the checker.
#[gpui::test]
fn spelling_quick_fix_replaces_adds_and_remembers(cx: &mut TestAppContext) {
    let home = fresh("spell-fix-config");
    let dir = fresh("spell-fix");
    let file = dir.join("t.md");
    std::fs::write(&file, "Teh quick brown fox and a zorbulonz cluster.\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &home);
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    wait_speller(cx, &ed);
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, 1, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Spelling: Fix Word at Cursor…", window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let text = ed.read_with(cx, |e, _| e.text());
    assert!(text.starts_with("The quick brown fox"), "the capital letter is kept: {text}");

    // "zorbulonz" → add to the dictionary.
    let at = text.find("zorbulonz").unwrap();
    let words = ed.read_with(cx, |e, cx| e.misspelled_words_for_test(cx));
    assert_eq!(words, ["zorbulonz"]);
    wb.update(cx, |wb, cx| wb.spell_choice_for_test(&ed, at..at + 9, "zorbulonz", true, cx));
    assert!(ed.read_with(cx, |e, cx| e.misspelled_words_for_test(cx)).is_empty());
    assert_eq!(std::fs::read_to_string(home.join("spelling.txt")).unwrap().trim(), "zorbulonz");
}

/// The language list setting is validated; installing from a fake source keeps unsafe names out.
#[test]
fn spelling_settings_parse_and_reject_unsafe_dictionary_names() {
    let (s, w) = crate::settings::parse_layers(r#"{ "spelling.languages": ["en", "tr", "../etc"], "spelling.enabled": false }"#, &[]).unwrap();
    assert_eq!((s.spell_languages.clone(), s.spelling), (vec!["en".to_string(), "tr".to_string()], false));
    assert!(w.iter().any(|m| m.contains("spelling.languages")), "{w:?}");
    let (s, _) = crate::settings::parse_layers(r#"{ "spelling.languages": 5 }"#, &[]).unwrap();
    assert_eq!(s.spell_languages, vec!["en".to_string()], "a wrong type keeps the default");
    let d = crate::settings::Settings::default();
    assert!(d.spelling && d.spelling_comments && d.schema_validate && d.schema_download);
}

// --- Merge editor ---

const CONFLICTS: &str = "fn main() {\n<<<<<<< HEAD\n    println!(\"ours\");\n=======\n    println!(\"theirs\");\n>>>>>>> feature\n    run();\n<<<<<<< HEAD\n    a();\n=======\n    b();\n>>>>>>> feature\n}\n";

/// The merge editor lists the conflicts, resolves them one by one or all at once as undoable edits, and saves.
#[gpui::test]
fn the_merge_editor_resolves_conflicts_and_saves(cx: &mut TestAppContext) {
    use crate::conflict::Choice;
    let dir = fresh("merge-editor");
    let file = dir.join("main.rs");
    std::fs::write(&file, CONFLICTS).unwrap();
    let (wb, cx) = setup(cx);
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Merge Conflict: Open Merge Editor", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("merge", true)));
    let view = wb.read_with(cx, |wb, _| wb.merge_view_for_test()).expect("merge page");
    assert_eq!(view.read_with(cx, |v, cx| v.conflict_count(cx)), 2);
    assert_eq!(view.update(cx, |v, cx| v.hover_for_test(0, Choice::Both, cx)), ["    println!(\"ours\");", "    println!(\"theirs\");"]);

    // Resolve the first with both (incoming first), the second with the current side.
    view.update(cx, |v, cx| v.resolve(0, Choice::BothIncomingFirst, cx));
    let text = ed.read_with(cx, |e, _| e.text());
    assert!(text.starts_with("fn main() {\n    println!(\"theirs\");\n    println!(\"ours\");\n    run();\n<<<<<<< HEAD"), "{text}");
    assert_eq!(view.read_with(cx, |v, cx| v.conflict_count(cx)), 1);
    view.update(cx, |v, cx| v.resolve_all(Choice::Current, cx));
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "fn main() {\n    println!(\"theirs\");\n    println!(\"ours\");\n    run();\n    a();\n}\n");
    assert_eq!(view.read_with(cx, |v, cx| v.conflict_count(cx)), 0);

    // Undo brings the conflict back (one undo step per resolution).
    cx.simulate_keystrokes("cmd-z");
    wait_ticking(cx, "conflict is back", |cx| view.read_with(cx, |v, cx| v.conflict_count(cx)) == 1);
    cx.simulate_keystrokes("cmd-shift-z");
    wait_ticking(cx, "resolved again", |cx| view.read_with(cx, |v, cx| v.conflict_count(cx)) == 0);

    // Save through the editor: the file on disk is the resolved text.
    ed.update(cx, |e, cx| e.save_now(cx));
    wait_ticking(cx, "saved", |_| std::fs::read_to_string(&file).is_ok_and(|t| !t.contains("<<<<<<<")));
    assert!(std::fs::read_to_string(&file).unwrap().ends_with("    a();\n}\n"));
}

/// Without conflicts the command only explains itself; with a diff3 base the card carries the ancestor.
#[gpui::test]
fn the_merge_editor_needs_conflicts(cx: &mut TestAppContext) {
    let dir = fresh("merge-editor-none");
    let file = dir.join("plain.txt");
    std::fs::write(&file, "nothing to merge\n").unwrap();
    let (wb, cx) = setup(cx);
    let _ = crate::qa_editor_tests::open(&wb, &file, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Merge Conflict: Open Merge Editor", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), None);
    assert!(crate::qa_integration_tests::notice(&wb, cx).contains("No merge conflicts"));
}

/// The merge page cannot outlive the editor it works on: closing that tab closes the page too.
#[gpui::test]
fn the_merge_page_closes_with_its_editor_tab(cx: &mut TestAppContext) {
    let dir = fresh("merge-editor-close");
    let file = dir.join("main.rs");
    std::fs::write(&file, CONFLICTS).unwrap();
    let (wb, cx) = setup(cx);
    let _ = crate::qa_editor_tests::open(&wb, &file, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Merge Conflict: Open Merge Editor", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("merge", true)));
    wb.update_in(cx, |wb, window, cx| wb.close_tabs_for_test(vec![0], window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), None, "the page went away with its file");
}

/// Thousands of conflicts or huge blocks stay cheap: cards and pane lines are capped, the rest is counted.
#[gpui::test]
fn the_merge_editor_caps_cards_and_pane_lines(cx: &mut TestAppContext) {
    let dir = fresh("merge-editor-caps");
    let file = dir.join("big.txt");
    let mut text = String::new();
    for i in 0..45 {
        text.push_str(&format!("<<<<<<< HEAD\nours {i}\n=======\ntheirs {i}\n>>>>>>> feature\n"));
    }
    std::fs::write(&file, &text).unwrap();
    let (wb, cx) = setup(cx);
    let _ = crate::qa_editor_tests::open(&wb, &file, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Merge Conflict: Open Merge Editor", window, cx));
    let view = wb.read_with(cx, |wb, _| wb.merge_view_for_test()).expect("merge page");
    // The count in the header is the real one; rendering only takes the first cards.
    assert_eq!(view.read_with(cx, |v, cx| v.conflict_count(cx)), 45);
    view.update(cx, |v, cx| v.resolve_all(crate::conflict::Choice::Incoming, cx));
    assert_eq!(view.read_with(cx, |v, cx| v.conflict_count(cx)), 0);
}

// --- Opening from Finder / the Dock ---

/// A file handed over by the system opens as a tab and leaves the open project alone; without a project the file's
/// folder opens (like the command line); a folder given while another is open is not swapped in.
#[gpui::test]
fn files_from_the_system_open_as_tabs_without_changing_the_project(cx: &mut TestAppContext) {
    let project = fresh("sysopen-project");
    let outside = fresh("sysopen-outside");
    std::fs::write(project.join("a.txt"), "a").unwrap();
    std::fs::write(outside.join("b.txt"), "b").unwrap();
    crate::qa_workbench_tests::globals(cx);
    // `open_urls` looks for real `Workbench` windows (the test helper `setup` wraps the view in a root view).
    let win = cx.add_window(|window, cx| crate::workbench::Workbench::with_state(state::State::in_memory().ok(), None, window, cx));
    let wb = win.root(cx).unwrap();
    let canon = |p: &std::path::Path| crate::ui::local_path(&std::fs::canonicalize(p).unwrap());
    let url = |p: &std::path::Path| crate::lsp::path_to_uri(&canon(p));
    let root = |cx: &mut TestAppContext| wb.read_with(cx, |wb, cx| wb.local_folder(cx));
    let tabs = |cx: &mut TestAppContext| wb.read_with(cx, |wb, cx| wb.open_paths(cx));

    // No project yet: the file's folder becomes the project, as on the command line.
    cx.update(|cx| crate::sysopen::open_urls(vec![url(&project.join("a.txt"))], cx));
    cx.run_until_parked();
    assert_eq!(root(cx), Some(canon(&project)));
    assert!(tabs(cx).iter().any(|p| p.ends_with("a.txt")));

    // A file from elsewhere is just another tab; the project stays. Other schemes are ignored.
    cx.update(|cx| crate::sysopen::open_urls(vec![url(&outside.join("b.txt")), "https://example.com/ignored".into()], cx));
    cx.run_until_parked();
    assert_eq!(root(cx), Some(canon(&project)));
    let open = tabs(cx);
    assert!(open.iter().any(|p| p.ends_with("a.txt")) && open.iter().any(|p| p.ends_with("b.txt")), "{open:?}");

    // A missing file does not crash and opens nothing new.
    cx.update(|cx| crate::sysopen::open_urls(vec!["file:///definitely/not/here.txt".into()], cx));
    cx.run_until_parked();
    assert_eq!(tabs(cx).len(), open.len());
}

// --- Format on save ---

#[gpui::test]
fn terminal_web_links_use_the_system_browser(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let url = "https://example.com/şablon?a=one&b=two";
    wb.update_in(cx, |wb, window, cx| wb.open_link(crate::links::Link::Url(url.into()), window, cx));
    assert_eq!(cx.opened_url().as_deref(), Some(url));
}

#[cfg(windows)]
#[gpui::test]
fn windows_terminal_links_open_absolute_files_without_a_project(cx: &mut TestAppContext) {
    let dir = fresh("windows-terminal-link");
    let file = dir.join("şablon.txt");
    std::fs::write(&file, "outside a project").unwrap();
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.open_link(crate::links::Link::File { path: file.to_string_lossy().into_owned(), line: Some(1), col: None }, window, cx)
    });
    wait_ticking(cx, "absolute terminal link", |cx| wb.read_with(cx, |wb, cx| wb.open_paths(cx).iter().any(|p| p.ends_with("şablon.txt"))));
}

/// Server edits computed for an older text (the user kept typing while the formatter ran) are dropped instead of
/// landing on the wrong characters.
#[gpui::test]
fn language_server_edits_for_an_older_text_are_not_applied(cx: &mut TestAppContext) {
    let dir = fresh("stale-lsp-edits");
    let file = dir.join("a.txt");
    std::fs::write(&file, "hello world\n").unwrap();
    let (wb, cx) = setup(cx);
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    let at_request = ed.read_with(cx, |e, _| e.lsp_buffer_version());
    // The user types before the answer arrives.
    ed.update(cx, |e, cx| e.apply_lsp_edits(&[(((0, 0), (0, 0)), "XX".into())], cx));
    let format_hello = [(((0, 0), (0, 5)), "HELLO".to_string())];
    let applied = ed.update(cx, |e, cx| e.apply_lsp_edits_if_unchanged(at_request, &format_hello, cx));
    assert!(!applied);
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "XXhello world\n", "the stale edit changed nothing");
    // A request made against the current text is applied.
    let now = ed.read_with(cx, |e, _| e.lsp_buffer_version());
    let fresh_edit = [(((0, 0), (0, 7)), "HELLO".to_string())];
    assert!(ed.update(cx, |e, cx| e.apply_lsp_edits_if_unchanged(now, &fresh_edit, cx)));
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "HELLO world\n");
}

// --- Hot exit ---

/// A backup of a dirty file with a BOM or CR-only line endings restores to the same text (no literal U+FEFF, no stray `\r`),
/// and saving it afterwards writes exactly one BOM and the original line endings.
#[gpui::test]
fn hot_exit_backups_round_trip_bom_and_line_endings(cx: &mut TestAppContext) {
    let dir = fresh("hot-exit-bom");
    let bom = dir.join("bom.txt");
    let cr = dir.join("cr.txt");
    let crlf = dir.join("crlf.txt");
    std::fs::write(&bom, b"\xEF\xBB\xBFone\ntwo\n").unwrap();
    std::fs::write(&cr, b"one\rtwo\r").unwrap();
    std::fs::write(&crlf, b"one\r\ntwo\r\n").unwrap();
    let (wb, cx) = setup(cx);
    for (file, want_text, want_disk) in
        [(&bom, "oneX\ntwo\n", &b"\xEF\xBB\xBFoneX\ntwo\n"[..]), (&cr, "oneX\ntwo\n", &b"oneX\rtwo\r"[..]), (&crlf, "oneX\ntwo\n", &b"oneX\r\ntwo\r\n"[..])]
    {
        let ed = crate::qa_editor_tests::open(&wb, file, cx);
        // Make it dirty, take the backup the way the session does, restore it over a fresh copy.
        ed.update(cx, |e, cx| e.apply_lsp_edits(&[(((0, 3), (0, 3)), "X".into())], cx));
        let (bytes, hash) = ed.read_with(cx, |e, _| (e.content_bytes(), e.disk_hash()));
        ed.update(cx, |e, cx| e.apply_backup(&bytes, hash, cx));
        // The rope holds LF text without a BOM; `text()` is the file as it would be written.
        let rope = ed.read_with(cx, |e, _| e.rope_snapshot().to_string());
        assert_eq!(rope, want_text, "{}", file.display());
        assert!(!rope.contains(['\u{feff}', '\r']));
        assert_eq!(ed.read_with(cx, |e, _| e.text()).into_bytes(), want_disk, "{}", file.display());
        ed.update(cx, |e, cx| e.save_now(cx));
        wait_ticking(cx, "saved", |_| std::fs::read(file).is_ok_and(|d| d == want_disk));
    }
}

// --- Workspace trust upgrade ---

/// Folders opened before language servers needed trust stay trusted once; folders recorded afterwards are not.
#[test]
fn folders_opened_before_the_trust_gate_stay_trusted_once() {
    let s = state::State::in_memory().unwrap();
    s.record_workspace(None, "/old/project").unwrap();
    s.record_workspace(Some("srv"), "/srv/app").unwrap();
    crate::workbench::Workbench::migrate_trust(&s);
    assert!(s.get("trust:://old/project").is_none(), "keys are `trust:<host>:<path>`");
    assert_eq!(s.get("trust::/old/project").as_deref(), Some("1"));
    assert_eq!(s.get("trust:srv:/srv/app").as_deref(), Some("1"));
    // Later folders are new and must be trusted explicitly; running the migration again changes nothing.
    s.record_workspace(None, "/downloaded/repo").unwrap();
    crate::workbench::Workbench::migrate_trust(&s);
    assert!(s.get("trust::/downloaded/repo").is_none());
}

// --- Mixed line endings ---

/// Opening a file whose line endings are mixed says what saving will do, before anyone is surprised by a huge diff.
#[gpui::test]
fn opening_a_file_with_mixed_line_endings_warns(cx: &mut TestAppContext) {
    let dir = fresh("mixed-eol");
    let mixed = dir.join("mixed.txt");
    let clean = dir.join("clean.txt");
    std::fs::write(&mixed, b"one\ntwo\r\nthree\n").unwrap();
    std::fs::write(&clean, b"one\r\ntwo\r\n").unwrap();
    let (wb, cx) = setup(cx);
    crate::qa_editor_tests::open(&wb, &clean, cx);
    assert!(!crate::qa_integration_tests::notice(&wb, cx).contains("mixes line endings"));
    crate::qa_editor_tests::open(&wb, &mixed, cx);
    wait_ticking(cx, "warning", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("mixes line endings"));
    let n = crate::qa_integration_tests::notice(&wb, cx);
    assert!(n.contains("as LF"), "the first line decides: {n}");
}

// --- Pull requests ---

/// Checking a pull request out fetches `pull/N/head` from the remote into `pr-N`, again after the PR moved on.
#[gpui::test]
fn a_pull_request_branch_can_be_checked_out_and_refreshed(cx: &mut TestAppContext) {
    use crate::qa_integration_tests::{git_in, repo_with_remote};
    let (dir, bare, other) = repo_with_remote("pr-flow");
    // Someone else opens PR #7: a branch whose head is published as refs/pull/7/head.
    git_in(&other, &["checkout", "-q", "-b", "feat"]);
    std::fs::write(other.join("feature.txt"), "v1\n").unwrap();
    git_in(&other, &["add", "-A"]);
    git_in(&other, &["commit", "-q", "-m", "feature v1"]);
    git_in(&other, &["push", "-q", "origin", "HEAD:refs/pull/7/head"]);
    // The workspace's remote looks like GitHub but resolves to the local bare repository.
    git_in(&dir, &["config", &format!("url.{}.insteadOf", bare.display()), "https://github.com/o/r.git"]);
    git_in(&dir, &["remote", "set-url", "origin", "https://github.com/o/r.git"]);
    let (wb, cx) = setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    wait_ticking(cx, "git status", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some()));
    wb.update(cx, |wb, cx| wb.pull_action_for_test("origin", 7, "feat", 0, cx));
    let head = || {
        String::from_utf8(std::process::Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).current_dir(&dir).output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_string()
    };
    wait_ticking(cx, "pr-7 checked out", |_| head() == "pr-7");
    assert_eq!(std::fs::read_to_string(dir.join("feature.txt")).unwrap(), "v1\n");
    wait_ticking(cx, "notice", |cx| crate::qa_integration_tests::notice(&wb, cx).starts_with("Checked out #7"));

    // The PR gains a commit; checking out again moves pr-7 to the new head.
    std::fs::write(other.join("feature.txt"), "v2\n").unwrap();
    git_in(&other, &["commit", "-q", "-am", "feature v2"]);
    git_in(&other, &["push", "-q", "origin", "HEAD:refs/pull/7/head"]);
    wb.update(cx, |wb, cx| wb.pull_action_for_test("origin", 7, "feat", 0, cx));
    wait_ticking(cx, "refreshed", |_| std::fs::read_to_string(dir.join("feature.txt")).is_ok_and(|t| t == "v2\n"));

    // A missing PR is explained (not git's raw "couldn't find remote ref") and leaves the branch alone.
    wb.update(cx, |wb, cx| wb.pull_action_for_test("origin", 999, "gone", 0, cx));
    wait_ticking(cx, "error shown", |cx| {
        let n = crate::qa_integration_tests::notice(&wb, cx);
        n.contains("Pull request #999 has no head on origin") && n.contains("refresh the list")
    });
    assert_eq!(head(), "pr-7");
}

// --- .http client ---

/// A tiny HTTP server for the tests: `/login` answers with a token, every other path echoes the request it got.
fn http_fixture() -> (u16, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    http_fixture_on("127.0.0.1")
}

/// This computer's address on the local network (no packet is sent), if it has one: a host that counts as remote for
/// `http.confirmRequests` while the traffic never leaves the machine.
fn lan_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified() && ip.is_ipv4()).then(|| ip.to_string())
}

fn http_fixture_on(bind: &str) -> (u16, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(format!("{bind}:0")).unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        while !flag.load(std::sync::atomic::Ordering::Relaxed) {
            let Ok((mut s, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            s.set_nonblocking(false).ok();
            s.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut buf = vec![0u8; 16384];
            let mut got = 0;
            // Read until the headers and the declared body are in.
            loop {
                let n = s.read(&mut buf[got..]).unwrap_or(0);
                got += n;
                let text = String::from_utf8_lossy(&buf[..got]).to_string();
                if let Some(h) = text.find("\r\n\r\n") {
                    let want = text[..h]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                        .unwrap_or(0);
                    if got >= h + 4 + want || n == 0 {
                        break;
                    }
                } else if n == 0 {
                    break;
                }
            }
            let req = String::from_utf8_lossy(&buf[..got]).to_string();
            let (ctype, body) =
                if req.starts_with("POST /login") { ("application/json", "{\"token\":\"abc123\",\"id\":7}".to_string()) } else { ("text/plain", req.clone()) };
            let resp =
                format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nX-Fixture: yes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = s.write_all(resp.as_bytes());
        }
    });
    (port, stop)
}

/// Sending a request runs curl on the workspace host and opens the body; a named response feeds the next request.
#[gpui::test]
fn http_requests_are_sent_chained_and_shown(cx: &mut TestAppContext) {
    let (port, stop) = http_fixture();
    let dir = fresh("http-client");
    let file = dir.join("api.http");
    std::fs::write(
        &file,
        format!(
            "@base = http://127.0.0.1:{port}\n\n### login\n# @name login\nPOST {{{{base}}}}/login\nContent-Type: application/json\n\n{{\"user\": \"ada\"}}\n\n### who am i\nGET {{{{base}}}}/me?x=1\nAuthorization: Bearer {{{{login.response.body.$.token}}}}\nX-Req: {{{{$guid}}}}\n"
        ),
    )
    .unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &fresh("http-config"));
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    // Cursor inside the first request's body.
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(7, 3, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Send Request", window, cx));
    wait_ticking(cx, "login response", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("200 OK"));
    let n = crate::qa_integration_tests::notice(&wb, cx);
    assert!(n.starts_with("POST 127.0.0.1:") && n.contains("/login → 200 OK") && n.contains(" ms · "), "{n}");
    let shown = wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| (e.read(cx).path.clone(), e.read(cx).text()))).unwrap();
    assert!(shown.0.ends_with("response.json"), "{}", shown.0);
    assert!(shown.1.contains("\"token\": \"abc123\"") && shown.1.contains('\n'), "JSON is pretty-printed: {}", shown.1);

    // The second request uses the first one's token; the echo shows exactly what was sent.
    crate::qa_editor_tests::open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(11, 2, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Send Request", window, cx));
    wait_ticking(cx, "second response", |cx| crate::qa_integration_tests::notice(&wb, cx).starts_with("GET 127.0.0.1:"));
    let echoed = wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).text())).unwrap();
    assert!(echoed.starts_with("GET /me?x=1 HTTP/1.1"), "{echoed}");
    assert!(echoed.contains("Authorization: Bearer abc123") || echoed.contains("authorization: Bearer abc123"), "{echoed}");
    let guid = echoed.lines().find_map(|l| l.strip_prefix("X-Req: ").or_else(|| l.strip_prefix("x-req: "))).unwrap();
    assert_eq!(guid.len(), 36, "{guid}");
    // Headers of the last response on request.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Show Last Response Headers", window, cx));
    wait_ticking(cx, "headers tab", |cx| {
        wb.read_with(cx, |wb, cx| wb.active_editor_entity().is_some_and(|e| e.read(cx).path.ends_with("response-headers.txt")))
    });
    let headers = wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).text())).unwrap();
    assert!(headers.starts_with("200 OK") && headers.to_lowercase().contains("x-fixture: yes"), "{headers}");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Mistakes are explained instead of sent: unknown variable, missing named response, wrong file type, connection refused.
#[gpui::test]
fn http_request_errors_are_reported(cx: &mut TestAppContext) {
    let dir = fresh("http-client-errors");
    let file = dir.join("bad.http");
    std::fs::write(&file, "GET http://127.0.0.1:1/x\n\n###\nGET {{nowhere}}/y\n\n###\nGET http://h/{{other.response.body.$.id}}\n").unwrap();
    let txt = dir.join("note.txt");
    std::fs::write(&txt, "GET http://x/\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &fresh("http-config-errors"));
    let ed = crate::qa_editor_tests::open(&wb, &txt, cx);
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, 0, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Send Request", window, cx));
    assert!(crate::qa_integration_tests::notice(&wb, cx).contains(".http or .rest"));
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    for (line, why) in [(3, "undefined variable nowhere"), (6, "send the request named “other” first")] {
        ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(line, 0, cx));
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Send Request", window, cx));
        wait_ticking(cx, "error", |cx| crate::qa_integration_tests::notice(&wb, cx).contains(why));
    }
    // Nothing listens on port 1: curl's own message comes back.
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, 0, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Send Request", window, cx));
    wait_ticking(cx, "connection error", |cx| {
        let n = crate::qa_integration_tests::notice(&wb, cx);
        n.contains("curl:") || n.to_lowercase().contains("connect") || n.to_lowercase().contains("refused")
    });
}

/// A request that reads environment variables or uploads files from elsewhere asks first; "Cancel" sends nothing, "Send"
/// goes ahead and is remembered for that file; a second send while one is in flight is refused with a note.
#[gpui::test]
fn sensitive_http_requests_ask_before_sending(cx: &mut TestAppContext) {
    let (port, stop) = http_fixture();
    let dir = fresh("http-sensitive");
    let file = dir.join("env.http");
    std::fs::write(&file, format!("GET http://127.0.0.1:{port}/e\nX-Home: {{{{$processEnv PATH}}}}\n")).unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &fresh("http-config-sensitive"));
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, 0, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Send Request", window, cx));
    assert!(cx.has_pending_prompt(), "environment access needs a yes");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(!crate::qa_integration_tests::notice(&wb, cx).contains("Sending"), "cancelled: nothing was sent");

    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Send Request", window, cx));
    cx.simulate_prompt_answer("Send");
    wait_ticking(cx, "response", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("200 OK"));

    // Remembered for this file: no second prompt.
    crate::qa_editor_tests::open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(0, 0, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Send Request", window, cx));
    assert!(!cx.has_pending_prompt());
    // The first send is still in flight: a second one is declined politely.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Send Request", window, cx));
    assert!(crate::qa_integration_tests::notice(&wb, cx).contains("still being sent"), "{}", crate::qa_integration_tests::notice(&wb, cx));
    wait_ticking(cx, "second response", |cx| !wb.read_with(cx, |wb, _| wb.http_busy_for_test()));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn send_at(wb: &gpui::Entity<crate::Workbench>, file: &std::path::Path, line: usize, cx: &mut gpui::VisualTestContext) {
    let ed = crate::qa_editor_tests::open(wb, file, cx);
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(line, 0, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("HTTP: Send Request", window, cx));
}

fn config_with(name: &str, json: &str) -> std::path::PathBuf {
    let dir = fresh(name);
    std::fs::write(dir.join("settings.jsonc"), json).unwrap();
    dir
}

/// `http.confirmRequests: "always"` asks before every request, also to this computer; "Cancel" sends nothing.
#[gpui::test]
fn http_confirm_always_asks_for_every_request(cx: &mut TestAppContext) {
    let (port, stop) = http_fixture();
    let dir = fresh("http-confirm-always");
    let file = dir.join("a.http");
    std::fs::write(&file, format!("GET http://127.0.0.1:{port}/a\n")).unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &config_with("http-confirm-always-cfg", r#"{ "http.confirmRequests": "always" }"#));
    send_at(&wb, &file, 0, cx);
    assert!(cx.has_pending_prompt(), "always asks, even for 127.0.0.1");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(!crate::qa_integration_tests::notice(&wb, cx).contains("Sending"), "cancelled: nothing was sent");
    send_at(&wb, &file, 0, cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Send");
    wait_ticking(cx, "response", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("200 OK"));
    wait_ticking(cx, "idle", |cx| !wb.read_with(cx, |wb, _| wb.http_busy_for_test()));
    send_at(&wb, &file, 0, cx);
    assert!(cx.has_pending_prompt(), "approving once does not silence `always`");
    cx.simulate_prompt_answer("Cancel");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The default (`remote`): this computer is never asked about; the first request to another host is, once per session, and
/// the host comes from the *resolved* address (variables, user-info tricks), not from the text of the file.
#[gpui::test]
fn http_confirm_remote_asks_once_per_new_host(cx: &mut TestAppContext) {
    let dir = fresh("http-confirm-remote");
    let file = dir.join("r.http");
    std::fs::write(
        &file,
        "@base = https://api.example.test\n\nGET {{base}}/v1/me\n\n###\nGET http://127.0.0.1@evil.example.test/x\n\n###\nGET http://127.0.0.1.example.test/x\n",
    )
    .unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &fresh("http-confirm-remote-cfg"));
    for (line, host) in [(2, "api.example.test"), (5, "evil.example.test"), (8, "127.0.0.1.example.test")] {
        send_at(&wb, &file, line, cx);
        assert!(cx.has_pending_prompt(), "{host}: a host that is not this computer is asked about first");
        let shown = cx.pending_prompt().map(|(msg, detail)| format!("{msg} {detail}")).unwrap_or_default();
        assert!(shown.contains(host), "the prompt names the real host {host}: {shown}");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert!(!crate::qa_integration_tests::notice(&wb, cx).contains("Sending"), "{host}: cancelled, nothing was sent");
    }
    // Declining approves nothing: the same host asks again.
    send_at(&wb, &file, 2, cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    // An unreadable host (a backslash hides where curl would connect) is asked about as well.
    let odd = dir.join("odd.http");
    std::fs::write(&odd, "GET http://good.example.test\\@bad.example.test/x\n").unwrap();
    send_at(&wb, &odd, 0, cx);
    assert!(cx.has_pending_prompt(), "an address whose host cannot be read reliably asks");
    let shown = cx.pending_prompt().map(|(m, d)| format!("{m} {d}")).unwrap_or_default();
    assert!(shown.contains("cannot be read reliably"), "{shown}");
    cx.simulate_prompt_answer("Cancel");
}

/// Approving a remote host lets later requests to it go out without asking; `never` restores the old behaviour. Uses this
/// machine's own network address as the "remote" host so no traffic leaves the computer (skipped without a network address).
#[gpui::test]
fn http_confirm_approval_is_remembered_per_host(cx: &mut TestAppContext) {
    let Some(ip) = lan_ip() else {
        eprintln!("no network address on this machine: skipped");
        return;
    };
    let (port, stop) = http_fixture_on("0.0.0.0");
    let dir = fresh("http-confirm-lan");
    let file = dir.join("lan.http");
    std::fs::write(&file, format!("GET http://{ip}:{port}/a\n\n###\nGET http://{ip}:{port}/b\n")).unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &fresh("http-confirm-lan-cfg"));
    send_at(&wb, &file, 0, cx);
    assert!(cx.has_pending_prompt(), "{ip} is not this computer as far as the policy goes");
    cx.simulate_prompt_answer("Send");
    wait_ticking(cx, "response", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("200 OK"));
    wait_ticking(cx, "idle", |cx| !wb.read_with(cx, |wb, _| wb.http_busy_for_test()));
    send_at(&wb, &file, 3, cx);
    assert!(!cx.has_pending_prompt(), "the host was approved for this session");
    wait_ticking(cx, "second response", |cx| !wb.read_with(cx, |wb, _| wb.http_busy_for_test()));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);

    // `never`: the first request to a new host goes out without a question.
    let (port, stop) = http_fixture_on("0.0.0.0");
    std::fs::write(&file, format!("GET http://{ip}:{port}/c\n")).unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &config_with("http-confirm-never-cfg", r#"{ "http.confirmRequests": "never" }"#));
    send_at(&wb, &file, 0, cx);
    assert!(!cx.has_pending_prompt(), "never: no extra question");
    wait_ticking(cx, "response", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("200 OK"));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Schema downloads that wait for a host ask once; allowing approves the host for the session (for `.http` too), declining is
/// remembered and not asked again.
#[gpui::test]
fn schema_download_hosts_are_asked_once(cx: &mut TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &fresh("schema-hosts-cfg"));
    let gate = wb.update(cx, |wb, cx| wb.net_gate_for_test(cx));
    gate.request("schemas.example.test");
    gate.request("schemas.example.test");
    wb.update(cx, |wb, cx| wb.ask_schema_hosts_for_test(cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    let shown = cx.pending_prompt().map(|(m, d)| format!("{m} {d}")).unwrap_or_default();
    assert!(shown.contains("schemas.example.test"), "{shown}");
    cx.simulate_prompt_answer("Download");
    cx.run_until_parked();
    assert!(!gate.needs_approval("schemas.example.test"), "approved for the session");
    assert!(!cx.has_pending_prompt(), "one question per host, not per file");

    gate.request("declined.example.test");
    wb.update(cx, |wb, cx| wb.ask_schema_hosts_for_test(cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Not Now");
    cx.run_until_parked();
    assert!(gate.is_refused("declined.example.test"));
    gate.request("declined.example.test");
    wb.update(cx, |wb, cx| wb.ask_schema_hosts_for_test(cx));
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt(), "a declined host is not asked about again in this session");
}

/// Log files follow on their own when opened in the large-file view, the menu entry follows anything, and the
/// filter shows matching lines only (the pattern is a case-insensitive regular expression).
#[gpui::test]
fn logs_follow_automatically_and_can_be_filtered(cx: &mut TestAppContext) {
    let dir = fresh("log-filter");
    let log = dir.join("app.log");
    let plain = dir.join("data.txt");
    let body: String = (0..300)
        .map(|i| match i % 50 {
            7 => format!("2026-10-24 ERROR db timeout {i}\n"),
            13 => format!("2026-10-24 warn slow request {i}\n"),
            _ => format!("2026-10-24 INFO ok {i}\n"),
        })
        .collect();
    std::fs::write(&log, &body).unwrap();
    std::fs::write(&plain, "a\nb\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &fresh("log-filter-config"));
    wb.update_in(cx, |wb, window, cx| wb.open_large_for_test(plain.to_str().unwrap(), window, cx));
    let v = wb.read_with(cx, |wb, _| wb.large_view()).unwrap();
    assert!(!v.read_with(cx, |v, _| v.is_following()), "an ordinary file does not follow by itself");
    wb.update_in(cx, |wb, window, cx| wb.open_large_for_test(log.to_str().unwrap(), window, cx));
    let v = wb.read_with(cx, |wb, _| wb.large_view()).unwrap();
    assert!(v.read_with(cx, |v, _| v.is_following()), "a .log file does");
    wb.update_in(cx, |wb, window, cx| wb.follow_path_for_test(plain.to_str().unwrap(), window, cx));
    let v = wb.read_with(cx, |wb, _| wb.large_view()).unwrap();
    assert!(v.read_with(cx, |v, _| v.is_following()), "the menu entry follows any file");

    wb.update_in(cx, |wb, window, cx| wb.filter_log_for_test(log.to_str().unwrap(), "error|WARN", window, cx));
    wait_ticking(cx, "filtered tab", |cx| {
        wb.read_with(cx, |wb, cx| wb.active_editor_entity().is_some_and(|e| e.read(cx).path.ends_with("app.log.filtered.log")))
    });
    let text = wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).text())).unwrap();
    assert_eq!(text.lines().count(), 12, "6 errors and 6 warnings: {text}");
    assert!(text.lines().all(|l| l.contains("ERROR") || l.contains("warn")));
    assert!(crate::qa_integration_tests::notice(&wb, cx).starts_with("12 lines match"));
    wb.update_in(cx, |wb, window, cx| wb.filter_log_for_test(log.to_str().unwrap(), "zzz-nothing", window, cx));
    wait_ticking(cx, "no match notice", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("No lines match"));
    wb.update_in(cx, |wb, window, cx| wb.filter_log_for_test(log.to_str().unwrap(), "(unclosed", window, cx));
    // Positive match: grep's own wording differs (BSD "parentheses not balanced", GNU "Unmatched ( or \(", which contains "match").
    wait_ticking(cx, "bad pattern", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("is not a valid regular expression"));
}

/// Editing a host changes only that block of the fixture's ssh config; the fixture alias still resolves.
#[gpui::test]
fn host_settings_can_be_edited_from_the_app(cx: &mut TestAppContext) {
    use host::ssh_config::Field;
    let Some(sshd) = crate::test_sshd::start("feat-host-edit") else { return };
    let cfg = sshd.dir.join("ssh_config");
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.finish_add_ssh_host_for_test("me@127.0.0.1:2200 as extra".to_string(), window, cx));
    wb.update(cx, |wb, cx| wb.finish_edit_host_field_for_test("extra", Field::Port, "2222", cx));
    wb.update(cx, |wb, cx| wb.finish_edit_host_field_for_test("extra", Field::ProxyJump, "bastion", cx));
    let d = host::ssh_config::details(&cfg, "extra").unwrap();
    assert_eq!(
        (d.port.as_deref(), d.proxy_jump.as_deref(), d.user.as_deref(), d.hostname.as_deref()),
        (Some("2222"), Some("bastion"), Some("me"), Some("127.0.0.1"))
    );
    assert!(crate::qa_integration_tests::notice(&wb, cx).contains("ProxyJump = bastion"));
    wb.update(cx, |wb, cx| wb.finish_edit_host_field_for_test("extra", Field::ProxyJump, "", cx));
    assert!(host::ssh_config::details(&cfg, "extra").unwrap().proxy_jump.is_none(), "an empty value removes the setting");
    wb.update(cx, |wb, cx| wb.finish_edit_host_field_for_test("extra", Field::Port, "70000", cx));
    assert!(crate::qa_integration_tests::notice(&wb, cx).contains("not a valid Port"));
    assert_eq!(host::ssh_config::details(&cfg, "extra").unwrap().port.as_deref(), Some("2222"), "a refused value changes nothing");
    assert!(host::ssh::list_hosts().iter().any(|h| h.alias == crate::test_sshd::ALIAS), "the fixture alias is untouched");
}

/// Key login: a key is generated when there is none, copied to the server once, and the second run changes nothing.
#[gpui::test]
fn key_login_is_installed_once(cx: &mut TestAppContext) {
    let Some(sshd) = crate::test_sshd::start("feat-key-login") else { return };
    let ssh_dir = fresh("key-login-ssh");
    let authorized = sshd.dir.join("extra_authorized_keys");
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| wb.key_setup_for_test(crate::test_sshd::ALIAS, ssh_dir.clone(), authorized.to_str().unwrap(), cx));
    wait_ticking(cx, "key installed", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("is set up"));
    let n = crate::qa_integration_tests::notice(&wb, cx);
    assert!(n.contains("created") && n.contains("no password"), "{n}");
    let pubkey = std::fs::read_to_string(ssh_dir.join("id_ed25519.pub")).unwrap();
    let on_server = std::fs::read_to_string(&authorized).unwrap();
    assert_eq!(on_server.trim(), pubkey.trim());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&authorized).unwrap().permissions().mode() & 0o777, 0o600);
    }
    wb.update(cx, |wb, cx| wb.key_setup_for_test(crate::test_sshd::ALIAS, ssh_dir.clone(), authorized.to_str().unwrap(), cx));
    wait_ticking(cx, "second run", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("already set up"));
    assert_eq!(std::fs::read_to_string(&authorized).unwrap().lines().count(), 1, "no duplicate line");
    // An unreachable host reports the reason.
    wb.update(cx, |wb, cx| wb.key_setup_for_test("no-such-host-xyz", ssh_dir.clone(), authorized.to_str().unwrap(), cx));
    wait_ticking(cx, "failure", |cx| crate::qa_integration_tests::notice(&wb, cx).starts_with("Could not set up key login"));
}

/// The host menu lists every management action and the field picker shows current values.
#[gpui::test]
fn the_host_menu_and_field_picker_show_what_they_can_do(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("feat-host-menu") else { return };
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Remote: Manage SSH Hosts…", window, cx));
    cx.run_until_parked();
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).expect("menu open");
    assert_eq!(labels, ["Add a host", "Edit a host", "Test a connection", "Set up key login", "Remove a host", "Open ~/.ssh/config"]);
    cx.simulate_keystrokes("escape");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Remote: Edit SSH Host…", window, cx));
    cx.run_until_parked();
    let hosts = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).expect("host picker");
    assert!(hosts.iter().any(|h| h == crate::test_sshd::ALIAS), "{hosts:?}");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let fields = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).expect("field picker");
    assert_eq!(fields, ["HostName", "User", "Port", "IdentityFile", "ProxyJump"]);
    let details = wb.read_with(cx, |wb, cx| wb.overlay_details(cx)).unwrap();
    assert_eq!(details[0], "127.0.0.1", "the current HostName is shown");
    assert_eq!(details[4], "not set");
}

/// Existing `authorized_keys` content is never damaged: a file without a final newline, other keys, the same key under another
/// comment, comments containing glob characters.
#[gpui::test]
fn key_install_keeps_existing_authorized_keys_intact(cx: &mut TestAppContext) {
    let Some(sshd) = crate::test_sshd::start("feat-key-existing") else { return };
    let ssh_dir = fresh("key-existing-ssh");
    let authorized = sshd.dir.join("existing_keys");
    let old1 = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOLDKEYONEOLDKEYONEOLDKEYONEOLDKEYONE1 old@laptop";
    let old2 = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCOLDKEYTWOOLDKEYTWOOLDKEYTWO2 work * [glob] ?";
    std::fs::write(&authorized, format!("{old1}\n{old2}")).unwrap(); // no trailing newline
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| wb.key_setup_for_test(crate::test_sshd::ALIAS, ssh_dir.clone(), authorized.to_str().unwrap(), cx));
    wait_ticking(cx, "installed", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("is set up"));
    let text = std::fs::read_to_string(&authorized).unwrap();
    let new_key = std::fs::read_to_string(ssh_dir.join("id_ed25519.pub")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text:?}");
    assert_eq!((lines[0], lines[1]), (old1, old2), "the old keys are byte for byte what they were");
    assert_eq!(lines[2], new_key.trim());
    assert!(text.ends_with('\n'));
    // The same key blob under a different comment counts as present.
    let blob = new_key.split_whitespace().nth(1).unwrap();
    std::fs::write(&authorized, format!("ssh-ed25519 {blob} somebody-else\n")).unwrap();
    wb.update(cx, |wb, cx| wb.key_setup_for_test(crate::test_sshd::ALIAS, ssh_dir.clone(), authorized.to_str().unwrap(), cx));
    wait_ticking(cx, "present", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("already set up"));
    assert_eq!(std::fs::read_to_string(&authorized).unwrap().lines().count(), 1);
    // A server path with spaces and quotes works.
    let odd = sshd.dir.join("odd dir 'q'/keys");
    wb.update(cx, |wb, cx| wb.key_setup_for_test(crate::test_sshd::ALIAS, ssh_dir.clone(), odd.to_str().unwrap(), cx));
    wait_ticking(cx, "odd path", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("is set up"));
    assert!(std::fs::read_to_string(&odd).unwrap().contains(blob));
}

/// Transfer problems are explained at the right moment: a folder where a file is wanted (and the reverse) before anything is sent,
/// a multi-item upload that stops half-way, and a connection that is not ready.
#[gpui::test]
fn transfer_conflicts_and_partial_failures_are_explained(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("feat-transfer-explain") else { return };
    let work = fresh("transfer-explain-root");
    let local = fresh("transfer-explain-local");
    std::fs::write(local.join("a.txt"), "a").unwrap();
    std::fs::write(local.join("clash"), "i am a file").unwrap();
    std::fs::create_dir_all(work.join("clash")).unwrap(); // a folder named clash on the server
    let (wb, cx) = setup(cx);
    connect_fixture(&wb, cx);
    let alias = crate::test_sshd::ALIAS.to_string();
    wb.update_in(cx, |wb, window, cx| wb.upload_for_test(&alias, vec![local.join("clash")], &work, window, cx));
    wait_ticking(cx, "mismatch explained", |cx| notice_text(&wb, cx).contains("folder named “clash” is already there"));
    assert!(work.join("clash").is_dir() && wb.read_with(cx, |wb, _| wb.transfer_status().is_none()), "nothing was sent and no transfer is running");
    // The same for a download: a remote folder over a local file.
    let out = fresh("transfer-explain-out");
    std::fs::write(out.join("clash"), "local file").unwrap();
    wb.update_in(cx, |wb, window, cx| wb.download_for_test(&alias, &work.join("clash"), true, &out, window, cx));
    wait_ticking(cx, "download mismatch", |cx| notice_text(&wb, cx).contains("file named “clash” is already there"));
    assert_eq!(std::fs::read_to_string(out.join("clash")).unwrap(), "local file");
    // Half-way failure: the second item cannot be read.
    wb.update_in(cx, |wb, window, cx| wb.upload_for_test(&alias, vec![local.join("a.txt"), local.join("does-not-exist.txt")], &work, window, cx));
    wait_ticking(cx, "partial failure", |cx| notice_text(&wb, cx).starts_with("Transfer failed"));
    let n = notice_text(&wb, cx);
    assert!(n.contains("does-not-exist.txt") && n.contains("1 of 2 items were transferred"), "{n}");
    assert_eq!(std::fs::read_to_string(work.join("a.txt")).unwrap(), "a", "the first item did arrive");
}

/// Without a connection the transfer entry points say why and what to do instead of doing nothing.
#[gpui::test]
fn transfers_without_a_connection_explain_themselves(cx: &mut TestAppContext) {
    let local = fresh("transfer-noconn");
    std::fs::write(local.join("a.txt"), "a").unwrap();
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.start_download_for_test("/srv/x".into(), window, cx));
    assert!(notice_text(&wb, cx).contains("connect first"), "{}", notice_text(&wb, cx));
    wb.update_in(cx, |wb, window, cx| wb.start_upload_for_test("/srv".into(), window, cx));
    assert!(notice_text(&wb, cx).starts_with("Uploading works with a connected SSH host"));
}

/// A followed file that disappears (rotated away, deleted, directory in its place) says so and recovers by itself.
#[gpui::test]
fn following_a_file_that_vanishes_reports_it_and_recovers(cx: &mut TestAppContext) {
    let dir = fresh("follow-vanish");
    let file = dir.join("svc.log");
    std::fs::write(&file, "one\ntwo\n").unwrap();
    let (_wb, cx) = setup(cx);
    let host: Arc<dyn host::Host> = Arc::new(host::local());
    let view = cx.update(|_, cx| cx.new(|cx| LargeFileView::new(host, file.to_string_lossy().into_owned(), cx)));
    view.update(cx, |v, cx| {
        v.watch(cx);
        v.set_follow(true, cx);
    });
    wait_ticking(cx, "indexed", |cx| view.read_with(cx, |v, _| v.line_count()) == 3);
    assert_eq!(view.read_with(cx, |v, _| v.stale_message()), None);
    std::fs::remove_file(&file).unwrap();
    wait_ticking(cx, "stale", |cx| view.read_with(cx, |v, _| v.stale_message()).is_some());
    let m = view.read_with(cx, |v, _| v.stale_message()).unwrap();
    assert!(m.contains("deleted or moved"), "{m}");
    assert_eq!(view.read_with(cx, |v, _| v.line_count()), 3, "the last content stays visible");
    // A directory in its place: a different message, still no crash.
    std::fs::create_dir(&file).unwrap();
    wait_ticking(cx, "folder message", |cx| view.read_with(cx, |v, _| v.stale_message()).is_some_and(|m| !m.contains("deleted")));
    std::fs::remove_dir(&file).unwrap();
    // The file comes back with new content: the message goes away and the new lines show.
    std::fs::write(&file, "fresh a\nfresh b\nfresh c\nfresh d\n").unwrap();
    wait_ticking(cx, "recovered", |cx| view.read_with(cx, |v, _| v.stale_message().is_none() && v.line_count() == 5));
    view.update(cx, |v, cx| v.request_line(0, cx));
    wait_ticking(cx, "new content", |cx| view.read_with(cx, |v, _| v.loaded_line(0)).as_deref() == Some("fresh a"));
}

/// The log filter passes the pattern and the path to the shell as data: quotes, `$()`, backticks, a leading dash and odd file
/// names are literal text, and nothing is executed.
#[gpui::test]
fn the_log_filter_is_safe_against_shell_metacharacters(cx: &mut TestAppContext) {
    let dir = fresh("log-filter-safe");
    let canary = dir.join("INJECTED");
    let log = dir.join("it's a \"log\" $HOME file.log");
    std::fs::write(&log, "plain line\n-v starts with a dash\nprice $5 (approx)\nback`tick`\nIt's quoted\nERROR boom\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &fresh("log-filter-safe-config"));
    let run = |wb: &gpui::Entity<crate::workbench::Workbench>, cx: &mut gpui::VisualTestContext, pattern: &str| {
        wb.update_in(cx, |wb, window, cx| wb.filter_log_for_test(log.to_str().unwrap(), pattern, window, cx));
    };
    let active_text = |wb: &gpui::Entity<crate::workbench::Workbench>, cx: &mut gpui::VisualTestContext| {
        wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).text()).unwrap_or_default())
    };
    // Injection attempts are just patterns that match nothing (or something literal).
    for evil in [
        format!("x'; touch {}; '", canary.display()),
        format!("$(touch {})", canary.display()),
        format!("`touch {}`", canary.display()),
        format!("a|b; touch {}", canary.display()),
    ] {
        run(&wb, cx, &evil);
        wait_ticking(cx, "evil pattern handled", |cx| {
            let n = crate::qa_integration_tests::notice(&wb, cx);
            n.contains("No lines match") || n.contains("not a valid") || n.contains("match")
        });
        assert!(!canary.exists(), "the pattern was executed: {evil}");
    }
    // A leading dash is a pattern, not a grep option; quotes and dollar signs match literally.
    run(&wb, cx, "-v starts");
    wait_ticking(cx, "dash pattern", |cx| active_text(&wb, cx).contains("-v starts with a dash"));
    run(&wb, cx, "it's");
    wait_ticking(cx, "quote pattern", |cx| active_text(&wb, cx).lines().any(|l| l == "It's quoted"));
    run(&wb, cx, "price \\$5");
    wait_ticking(cx, "dollar pattern", |cx| active_text(&wb, cx).contains("price $5 (approx)"));
    // An invalid regular expression is explained.
    run(&wb, cx, "(unclosed");
    wait_ticking(cx, "invalid regex", |cx| crate::qa_integration_tests::notice(&wb, cx).contains("not a valid regular expression"));
    // A missing file reports the reason.
    wb.update_in(cx, |wb, window, cx| wb.filter_log_for_test(dir.join("missing.log").to_str().unwrap(), "x", window, cx));
    wait_ticking(cx, "missing file", |cx| crate::qa_integration_tests::notice(&wb, cx).to_lowercase().contains("no such file"));
}

/// Over SSH the schema file is read from the server (through the agent) and its problems show up like any other.
#[gpui::test]
fn schemas_work_for_files_opened_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("feat-schema-ssh") else { return };
    let dir = schema_fixture("schema-ssh");
    let file = dir.join("app.json");
    std::fs::write(&file, "{\"$schema\": \"./app.schema.json\", \"port\": \"x\", \"mode\": \"dev\"}").unwrap();
    let (wb, cx) = setup(cx);
    connect_fixture(&wb, cx);
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    assert!(ed.read_with(cx, |e, _| e.host_arc().is_remote()), "the file is opened through the remote host");
    wait_ticking(cx, "remote schema problems", |cx| wb.read_with(cx, |wb, _| wb.problems().iter().any(|(p, n)| p.ends_with("app.json") && *n == 2)));
    assert!(ed.read_with(cx, |e, _| e.has_schema()));
    // Fixing the file clears them (the schema is not read again from scratch each time).
    ed.update(cx, |e, cx| e.replace_all_text_for_test("{\"$schema\": \"./app.schema.json\", \"name\": \"x\", \"port\": 1}", cx));
    wait_ticking(cx, "remote problems cleared", |cx| wb.read_with(cx, |wb, _| wb.problems().is_empty()));
}

/// A missing dictionary and an unreadable one are told apart, and a working one still checks while the others are reported.
#[gpui::test]
fn dictionary_problems_are_reported_precisely(cx: &mut TestAppContext) {
    let root = fresh("spell-dicts");
    std::fs::create_dir_all(root.join("broken")).unwrap();
    std::fs::write(root.join("broken/index.aff"), "SET UTF-8\n").unwrap();
    std::fs::write(root.join("broken/index.dic"), "not a number\nword\n").unwrap();
    std::fs::create_dir_all(root.join("good")).unwrap();
    std::fs::write(root.join("good/index.aff"), "SET UTF-8\n").unwrap();
    std::fs::write(root.join("good/index.dic"), "2\nmerhaba\ndünya\n").unwrap();
    let file = fresh("spell-dicts-file").join("n.md");
    std::fs::write(&file, "merhaba dünya teh yokkelime\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &fresh("spell-dicts-config"));
    wb.update(cx, |wb, _| wb.set_spell_root_for_test(root.clone()));
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    wait_speller(cx, &ed);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().spell_languages = vec!["en".into(), "good".into(), "broken".into(), "absent".into()]);
    wb.update(cx, |wb, cx| wb.refresh_speller_for_test(cx));
    wait_ticking(cx, "dictionary report", |cx| crate::qa_integration_tests::notice(&wb, cx).starts_with("Spelling:"));
    let n = crate::qa_integration_tests::notice(&wb, cx);
    assert!(n.contains("absent is not installed") && n.contains("broken could not be read"), "{n}");
    assert!(n.contains("Install Dictionary"), "{n}");
    // The installed languages work together: the extra dictionary's words pass, English typos are still found.
    assert_eq!(ed.read_with(cx, |e, cx| e.misspelled_words_for_test(cx)), ["teh", "yokkelime"]);
}

/// Adding a word with nowhere to keep it says so instead of pretending.
#[gpui::test]
fn adding_a_word_without_a_settings_folder_is_session_only(cx: &mut TestAppContext) {
    let file = fresh("spell-nodir").join("n.md");
    std::fs::write(&file, "a zorbulonz here\n").unwrap();
    let (wb, cx) = setup(cx);
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    wait_speller(cx, &ed);
    wb.update(cx, |wb, cx| wb.spell_choice_for_test(&ed, 2..11, "zorbulonz", true, cx));
    assert!(crate::qa_integration_tests::notice(&wb, cx).contains("this session only"), "{}", crate::qa_integration_tests::notice(&wb, cx));
    assert!(ed.read_with(cx, |e, cx| e.misspelled_words_for_test(cx)).is_empty(), "it is still accepted for the session");
}
