//! Code contributions end to end (#104): the probe extension (crates/ext-host/testdata/probe.wasm) runs in a real
//! codeyard-exthost and contributes menus, a key binding, a status item, a formatter, code actions, diagnostics,
//! decorations and tasks. Skipped when codeyard-exthost is not built (`cargo test --workspace` always builds it).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ext_manifest::MenuLocation;
use gpui::{Entity, TestAppContext, VisualTestContext};

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
# The probe also imports `auth` and `secrets` (#105).
auth = ["github"]
secrets = true
why = "Tests every part of the host."

[[commands]]
id = "notice"
title = "Probe: Notice"

[[commands]]
id = "echo"
title = "Probe: Echo"
enablement = "editor.language == 'toml'"

[[commands]]
id = "status"
title = "Probe: Status"

[[commands]]
id = "status-undeclared"
title = "Probe: Undeclared Status"

[[commands]]
id = "unstatus"
title = "Probe: Remove Status"

[[menus]]
command = "notice"
location = "editor-context"

[[menus]]
command = "echo"
location = "editor-context"

[[menus]]
command = "echo"
location = "file-tree"

[[menus]]
command = "notice"
location = "file-tree"
when = "editor"

[[keybindings]]
key = "ctrl-alt-f9"
command = "notice"

[[status-bar]]
id = "state"

[[formatters]]
languages = ["toml"]

[[code-actions]]
languages = ["toml"]

[[diagnostics]]
id = "todos"
languages = ["toml"]

[[decorations]]
id = "todos"
languages = ["toml"]

[[tasks]]
id = "probe"
"#;

fn exthost_missing() -> bool {
    !crate::ext_runtime::code_runs_for_test()
}

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extprov-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn install(cfg: &Path) {
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: MANIFEST.as_bytes().to_vec(), executable: false },
        ext_pkg::Entry { path: "Cargo.toml".into(), data: Vec::new(), executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: PROBE.to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).expect("the probe passes the import check");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "/downloads/probe.cyx".into() }).expect("installs");
}

/// A window with the probe installed; `settings` is the user's settings.jsonc.
fn setup<'a>(cx: &'a mut TestAppContext, cfg: &Path, settings: &str) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    install(cfg);
    std::fs::write(cfg.join("settings.jsonc"), settings).unwrap();
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

fn open(wb: &Entity<Workbench>, path: &Path, cx: &mut VisualTestContext) -> Entity<EditorView> {
    let p = path.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&p, window, cx));
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| wb.active_editor_entity()).expect("editor opened")
}

/// The exthost is a real process: run the window's tasks until `done`, up to 20 s.
fn wait_for(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
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

/// Let real time pass with the window running (to show that something does not happen).
fn settle(cx: &mut VisualTestContext, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default()
}

fn text(ed: &Entity<EditorView>, cx: &mut VisualTestContext) -> String {
    ed.read_with(cx, |e, _| e.text())
}

/// AC1: an extension command runs from the palette, a menu and a key binding, and the first call starts the extension.
/// `when` hides a menu entry and `enablement` greys it out; a status item runs its command on click.
#[gpui::test]
fn extension_commands_run_from_menus_keys_and_status_items(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("commands");
    let (wb, cx) = setup(cx, &cfg, "{}");
    let running = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.extension_running_for_test("you.probe"));
    assert!(!running(&wb, cx), "nothing starts the extension before it is used");

    // Menus: with no editor, the command whose enablement asks for TOML is greyed out; `when = "editor"` hides the
    // file-tree entry.
    let menu = |wb: &Entity<Workbench>, cx: &mut VisualTestContext, at| wb.update(cx, |wb, cx| wb.extension_menu_for_test(at, cx));
    assert_eq!(menu(&wb, cx, MenuLocation::EditorContext), [("Probe: Notice".to_string(), true), ("Probe: Echo".to_string(), false)]);
    assert_eq!(menu(&wb, cx, MenuLocation::FileTree), [("Probe: Echo".to_string(), false)]);
    let dir = config_dir("commands-files");
    std::fs::write(dir.join("a.toml"), "a = 1\n").unwrap();
    open(&wb, &dir.join("a.toml"), cx);
    assert_eq!(menu(&wb, cx, MenuLocation::EditorContext), [("Probe: Notice".to_string(), true), ("Probe: Echo".to_string(), true)]);

    // The menu entry's first use starts the extension.
    wb.update_in(cx, |wb, _, _| wb.clear_notice_for_test());
    wb.update_in(cx, |wb, window, cx| wb.run_extension_menu_for_test(MenuLocation::EditorContext, "Probe: Notice", None, window, cx));
    wait_for(cx, "the menu's command", |cx| notice(&wb, cx).contains("you.probe: Pick"));
    assert!(running(&wb, cx));
    // A file-tree entry gets the file as its argument; the probe echoes it back without an error.
    let file = dir.join("a.toml").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, _, _| wb.clear_notice_for_test());
    wb.update_in(cx, |wb, window, cx| wb.run_extension_menu_for_test(MenuLocation::FileTree, "Probe: Echo", Some(&file), window, cx));
    settle(cx, 300);
    assert!(!notice(&wb, cx).contains("you.probe:"), "{}", notice(&wb, cx));

    // The key binding: the key is bound, and pressing it runs the command.
    wb.update_in(cx, |wb, _, _| wb.clear_notice_for_test());
    cx.simulate_keystrokes("ctrl-alt-f9");
    wait_for(cx, "the key binding's command", |cx| notice(&wb, cx).contains("you.probe: Pick"));

    // The palette.
    wb.update_in(cx, |wb, _, _| wb.clear_notice_for_test());
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Notice", window, cx));
    wait_for(cx, "the palette's command", |cx| notice(&wb, cx).contains("you.probe: Pick"));

    // A status item: shown once set, runs its command on click, and an undeclared one is refused and logged.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Status", window, cx));
    wait_for(cx, "the status item", |cx| wb.read_with(cx, |wb, _| wb.extension_status_for_test()) == [("you.probe".to_string(), "Probe ready".to_string())]);
    wb.update_in(cx, |wb, _, _| wb.clear_notice_for_test());
    wb.update_in(cx, |wb, window, cx| wb.click_extension_status_for_test("Probe ready", window, cx));
    wait_for(cx, "the status item's command", |cx| notice(&wb, cx).contains("you.probe: Pick"));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Undeclared Status", window, cx));
    wait_for(cx, "the refusal in the log", |cx| {
        wb.read_with(cx, |wb, _| wb.extension_log_for_test("you.probe")).iter().any(|l| l.contains("\"other\" is not declared"))
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.extension_status_for_test()).len(), 1);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Remove Status", window, cx));
    wait_for(cx, "the status item to go", |cx| wb.read_with(cx, |wb, _| wb.extension_status_for_test()).is_empty());
}

/// An extension binding never takes a built-in key, and an extension command is bound like a core one.
#[gpui::test]
fn extension_command_keys_are_bound(cx: &mut TestAppContext) {
    // Only the bindings are looked at, the extension never runs: it loads where the exthost cannot (Windows).
    let _offline = crate::ext_runtime::load_without_exthost_for_test();
    let cfg = config_dir("keys");
    let (_wb, cx) = setup(cx, &cfg, "{}");
    let bound = cx.update(|_, cx| {
        cx.key_bindings().borrow().bindings().any(|b| {
            b.action().as_any().downcast_ref::<crate::workbench::RunExtensionCommand>().is_some_and(|a| a.extension == "you.probe" && a.command == "notice")
        })
    });
    assert!(bound);
}

/// AC2 (locally): with `editor.formatOnSave` the extension formats the file before it is written.
#[gpui::test]
fn an_extension_formatter_formats_on_save(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("save");
    let (wb, cx) = setup(cx, &cfg, r#"{ "editor.formatOnSave": true }"#);
    let dir = config_dir("save-files");
    let file = dir.join("a.toml");
    std::fs::write(&file, "a=1   \nb =2\n").unwrap();
    let ed = open(&wb, &file, cx);
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "# top\n", cx));
    ed.update(cx, |e, cx| e.save_now(cx));
    wait_for(cx, "the formatted file on disk", |_| std::fs::read_to_string(&file).unwrap_or_default() == "# top\na = 1\nb = 2\n");
    assert!(!ed.read_with(cx, |e, _| e.is_dirty()));

    // `editor.formatters` can hand the language back to the language server: then the extension is not asked.
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "editor.formatOnSave": true, "editor.formatters": { "toml": "lsp" } }"#).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "c=3\n", cx));
    ed.update(cx, |e, cx| e.save_now(cx));
    wait_for(cx, "the file saved as typed", |_| std::fs::read_to_string(&file).unwrap_or_default().starts_with("c=3\n"));
}

/// AC2 over SSH: the document travels to the extension and back the same way; the file is written on the host.
#[gpui::test]
fn an_extension_formatter_formats_on_save_over_ssh(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let Some(_sshd) = crate::test_sshd::start("ext-format-ssh") else { return };
    let cfg = config_dir("save-ssh");
    let root = cfg.join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.toml"), "a=1\n").unwrap();
    let (wb, cx) = setup(cx, &cfg, r#"{ "editor.formatOnSave": true }"#);
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 30, "connected", |cx| {
        cx.executor().advance_clock(Duration::from_secs(1));
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    let r = root.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&r, window, cx));
    cx.run_until_parked();
    let ed = crate::qa_integration_tests::ssh_open_file(&wb, cx, &root.join("a.toml"));
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "x=0\n", cx));
    ed.update(cx, |e, cx| e.save_now(cx));
    let file = root.join("a.toml");
    wait_for(cx, "the formatted file on the host", |_| std::fs::read_to_string(&file).unwrap_or_default() == "x = 0\na = 1\n");
}

/// AC3: a provider that does not answer never holds up typing. Typing cancels the request, and an answer that comes
/// after the deadline is dropped and noted in the extension's log.
#[gpui::test]
fn a_slow_provider_does_not_delay_typing(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("slow");
    let (wb, cx) = setup(cx, &cfg, "{}");
    let dir = config_dir("slow-files");
    let file = dir.join("slow.toml");
    std::fs::write(&file, "slow = 1\n").unwrap();
    let ed = open(&wb, &file, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Format Document", window, cx));
    wait_for(cx, "the extension to start", |cx| wb.read_with(cx, |wb, _| wb.extension_running_for_test("you.probe")));
    // The probe sits on the request. Typing goes on regardless.
    let t0 = Instant::now();
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "x", cx));
    cx.run_until_parked();
    assert_eq!(text(&ed, cx), "xslow = 1\n");
    assert!(t0.elapsed() < Duration::from_secs(1), "typing waited for the provider: {:?}", t0.elapsed());
    // The keystroke cancelled the request; the probe answers anyway when cancelled, and that answer is dropped.
    settle(cx, 500);
    assert_eq!(text(&ed, cx), "xslow = 1\n", "a cancelled answer is never applied");

    // No typing: the deadline passes, the user hears it timed out, and the late answer lands only in the log.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Format Document", window, cx));
    settle(cx, 200);
    cx.executor().advance_clock(Duration::from_millis(ext_manifest::limits::PROVIDER_DEADLINE_MILLIS + 50));
    wait_for(cx, "the timeout", |cx| notice(&wb, cx).contains("you.probe: timed out"));
    wait_for(cx, "the late answer in the log", |cx| {
        wb.read_with(cx, |wb, _| wb.extension_log_for_test("you.probe")).iter().any(|l| l.contains("after its") && l.contains("dropped"))
    });
    assert_eq!(text(&ed, cx), "xslow = 1\n");
}

/// AC4: an extension's diagnostics and decorations show in the Problems view, the gutter and after the text, tagged
/// with the extension, and go when it is disabled or uninstalled.
#[gpui::test]
fn extension_diagnostics_go_when_it_is_disabled_or_uninstalled(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("diagnostics");
    let (wb, cx) = setup(cx, &cfg, "{}");
    let dir = config_dir("diagnostics-files");
    let file = dir.join("todo.toml");
    std::fs::write(&file, "a = 1 # TODO\n").unwrap();
    let ed = open(&wb, &file, cx);
    let lint = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| {
        // Formatting lints too (the probe's way of seeing a document).
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Format Document", window, cx));
        wait_for(cx, "diagnostics", |cx| ed.read_with(cx, |e, _| !e.diagnostics().is_empty()));
    };
    lint(&wb, cx);
    let d = ed.read_with(cx, |e, _| e.diagnostics().to_vec());
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].message.as_str(), d[0].source.as_deref(), d[0].severity, d[0].start), ("TODO left here", Some("you.probe"), 2, (0, 8)));
    assert_eq!(wb.read_with(cx, |wb, _| wb.problems()).iter().map(|(_, n)| n).sum::<usize>(), 1, "listed in Problems");
    let decorations = ed.read_with(cx, |e, _| e.ext_decorations().to_vec());
    assert_eq!(decorations.len(), 2, "a gutter dot and text after the line: {decorations:?}");
    assert!(decorations.iter().any(|d| d.place == ext_api::value::DecorationPlace::Gutter && d.tone == ext_api::value::Tone::Warning));

    wb.update_in(cx, |wb, window, cx| wb.set_extension_enabled_for_test("you.probe", false, window, cx));
    wait_for(cx, "diagnostics to go on disable", |cx| ed.read_with(cx, |e, _| e.diagnostics().is_empty() && e.ext_decorations().is_empty()));
    assert!(wb.read_with(cx, |wb, _| wb.problems()).is_empty());

    wb.update_in(cx, |wb, window, cx| wb.set_extension_enabled_for_test("you.probe", true, window, cx));
    cx.run_until_parked();
    lint(&wb, cx);
    wb.update_in(cx, |wb, window, cx| wb.uninstall_extension_for_test("you.probe", window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Uninstall");
    wait_for(cx, "diagnostics to go on uninstall", |cx| ed.read_with(cx, |e, _| e.diagnostics().is_empty() && e.ext_decorations().is_empty()));
}

/// Code actions join the language server's in the quick-fix list; edits apply, and an action without edits asks the
/// extension to run it.
#[gpui::test]
fn extension_code_actions_join_quick_fix(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("actions");
    let (wb, cx) = setup(cx, &cfg, "{}");
    let dir = config_dir("actions-files");
    let file = dir.join("todo.toml");
    std::fs::write(&file, "a = 1 # TODO\n").unwrap();
    let ed = open(&wb, &file, cx);
    let quick_fix = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Quick Fix…", window, cx));
        wait_for(cx, "the quick-fix list", |cx| wb.update(cx, |wb, cx| wb.quick_fix_labels_for_test(cx)).is_some());
        wb.update(cx, |wb, cx| wb.quick_fix_labels_for_test(cx)).unwrap()
    };
    assert_eq!(quick_fix(&wb, cx), ["Probe: Mark as done", "Probe: Say hello"], "the preferred action first");
    wb.update_in(cx, |wb, window, cx| wb.choose_quick_fix_for_test("Probe: Mark as done", window, cx));
    wait_for(cx, "the edit", |cx| text(&ed, cx) == "a = 1 # DONE\n");
    assert_eq!(quick_fix(&wb, cx), ["Probe: Say hello"]);
    wb.update_in(cx, |wb, window, cx| wb.choose_quick_fix_for_test("Probe: Say hello", window, cx));
    wait_for(cx, "the extension to run the action", |cx| notice(&wb, cx).contains("you.probe: Code action hello"));
}

/// A code action's edits outside its document need `write`: one refused file refuses the whole action, nothing is
/// changed, the user is told and the extension's log says why.
#[gpui::test]
fn an_extension_code_action_cannot_edit_files_it_may_not_write(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("actions-scope");
    let (wb, cx) = setup(cx, &cfg, "{}");
    let dir = config_dir("actions-scope-files");
    let file = dir.join("todo.toml");
    let other = dir.join("other.toml");
    std::fs::write(&file, "a = 1 # TODO\n").unwrap();
    std::fs::write(&other, "b = 2\n").unwrap();
    let ed = open(&wb, &file, cx);
    let document = ed.read_with(cx, |e, _| e.path.clone());
    let edit = |line: u32, new_text: &str| {
        let pos = |character| ext_api::value::Position { line, character };
        ext_api::value::TextEdit { range: ext_api::value::Range { start: pos(0), end: pos(0) }, new_text: new_text.into() }
    };
    let uri = |p: &std::path::Path| crate::ext_broker::uri(&p.to_string_lossy());
    // As the app sent it: the editor's path, which may be spelled differently from `file` (`/private/var` on macOS).
    let own_uri = crate::ext_broker::uri(&document);
    let action = ext_api::value::CodeAction {
        id: "sneak".into(),
        title: "Probe: Sneak".into(),
        edits: vec![(own_uri.clone(), vec![edit(0, "# ok\n")]), (uri(&other), vec![edit(0, "# sneaked\n")])],
        command: None,
        preferred: false,
        fixes: vec![],
    };
    wb.update_in(cx, |wb, window, cx| wb.run_extension_code_action_for_test("you.probe", &document, action.clone(), window, cx));
    cx.run_until_parked();
    assert_eq!(text(&ed, cx), "a = 1 # TODO\n", "the document is untouched too");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "b = 2\n");
    assert!(notice(&wb, cx).contains("you.probe"), "{}", notice(&wb, cx));
    let log = wb.read_with(cx, |wb, _| wb.extension_log_for_test("you.probe"));
    assert!(log.iter().any(|l| l.contains("Probe: Sneak") && l.contains("refused")), "{log:?}");
    // Its own document alone is the user's to change.
    let own = ext_api::value::CodeAction { edits: vec![(own_uri, vec![edit(0, "# ok\n")])], ..action };
    wb.update_in(cx, |wb, window, cx| wb.run_extension_code_action_for_test("you.probe", &document, own, window, cx));
    wait_for(cx, "the edit", |cx| text(&ed, cx) == "# ok\na = 1 # TODO\n");
}

/// Tasks an extension provides are listed with the folder's, and running one asks the extension.
#[gpui::test]
fn extension_tasks_are_listed_and_run(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("tasks");
    let (wb, cx) = setup(cx, &cfg, "{}");
    let labels = wb.update_in(cx, |wb, window, cx| wb.task_labels_for_test(window, cx));
    wait_for(cx, "the extension's tasks", |_| labels.borrow().is_some());
    assert_eq!(labels.borrow().clone().unwrap(), ["Probe: Hello task"]);
    wb.update_in(cx, |wb, window, cx| wb.run_task_for_test("Probe: Hello task", window, cx));
    wait_for(cx, "the task to run", |cx| notice(&wb, cx).contains("you.probe: Task hello ran"));
}
