//! View-title menus and core commands (#104), end to end with the probe extension (crates/ext-host/testdata/probe.wasm)
//! in a real codeyard-exthost: a view's title menu runs the extension's command with the view id, or a core command;
//! `commands.execute("core:…")` from code runs a listed core command only when the manifest lists it and the extension
//! holds what the command implies; and the extensions of the performance scenarios. Skipped when code extensions cannot
//! run here (no codeyard-exthost or sandbox).

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
read = ["a.txt", "docs/**"]
write = ["src/**"]
run = ["echo"]
net = ["api.example.com"]
auth = ["github"]
secrets = true
commands = ["core:workbench.toggleZenMode", "core:workbench.saveAll", "core:editor.open", "core:tasks.run"]
why = "Tests every part of the host."

[[commands]]
id = "echo"
title = "Probe: Echo"

[[commands]]
id = "notice"
title = "Probe: Notice"

[[commands]]
id = "exec"
title = "Probe: Execute"

[[views]]
id = "counter"
title = "Counter"
location = "sidebar"

[[menus]]
command = "echo"
location = "view-title"
view = "counter"

[[menus]]
command = "notice"
location = "view-title"
view = "counter"
when = "editor"

[[menus]]
command = "core:workbench.toggleZenMode"
location = "view-title"
view = "counter"
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

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extcore-{name}-{}", std::process::id()));
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
    open_window(cx, cfg)
}

/// A window on `cfg` with what is installed there.
fn open_window<'a>(cx: &'a mut TestAppContext, cfg: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
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

/// The exthost is a real process: run the window's tasks until `done`, up to 20 s.
fn wait_for(cx: &mut VisualTestContext, wb: &Entity<Workbench>, what: &str, done: impl Fn(&Workbench) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20 * crate::test_sshd::timeout_scale());
    while Instant::now() < deadline {
        cx.run_until_parked();
        if wb.read_with(cx, |wb, _| done(wb)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out: {what}; answer: {:?}", wb.read_with(cx, |wb, _| wb.extension_answer_for_test("you.probe")));
}

/// The probe runs `commands.execute(command, argument)` and answers with what came back.
fn execute(wb: &Entity<Workbench>, cx: &mut VisualTestContext, line: &str) -> String {
    wb.update_in(cx, |wb, _, _| wb.forget_extension_answer_for_test("you.probe"));
    let args = serde_json::Value::String(line.to_string()).to_string();
    wb.update_in(cx, |wb, window, cx| wb.run_extension_command_with_for_test("you.probe", "exec", &args, window, cx));
    wait_for(cx, wb, line, |wb| wb.extension_answer_for_test("you.probe").is_some());
    wb.read_with(cx, |wb, _| wb.extension_answer_for_test("you.probe")).unwrap_or_default()
}

/// The view-title menu shows the entries for that view (`when` hides one), runs the extension's command with the
/// view's id, and runs a core command as the palette does.
#[gpui::test]
fn view_title_menus_run_with_the_view_id(cx: &mut TestAppContext) {
    if skipped() {
        return;
    }
    let cfg = config_dir("view-title");
    let (wb, cx) = setup(cx, &cfg);
    let view = "you.probe.counter";
    let menu = wb.update(cx, |wb, cx| wb.ext_view_title_menu_for_test(view, cx));
    assert_eq!(menu, [("Probe: Echo".to_string(), true), ("View: Toggle Zen Mode".to_string(), true)]);
    assert!(wb.update(cx, |wb, cx| wb.ext_view_title_menu_for_test("you.probe.other", cx)).is_empty());

    wb.update_in(cx, |wb, window, cx| wb.run_ext_view_title_menu_for_test(view, "Probe: Echo", window, cx));
    wait_for(cx, &wb, "the echo", |wb| wb.extension_answer_for_test("you.probe").as_deref() == Some("\"you.probe.counter\""));

    assert!(!wb.read_with(cx, |wb, _| wb.zen_for_test()));
    wb.update_in(cx, |wb, window, cx| wb.run_ext_view_title_menu_for_test(view, "View: Toggle Zen Mode", window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.zen_for_test()), "the core command ran");
}

/// `commands.execute("core:…")`: a listed command the manifest names runs; one the manifest does not name is denied
/// with the line to add; one not listed for extensions is refused with where the list is.
#[gpui::test]
fn core_commands_need_the_commands_permission(cx: &mut TestAppContext) {
    if skipped() {
        return;
    }
    let cfg = config_dir("execute");
    let (wb, cx) = setup(cx, &cfg);
    assert_eq!(execute(&wb, cx, "core:workbench.toggleZenMode"), "done");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.zen_for_test()));
    assert_eq!(execute(&wb, cx, "core:editor.formatDocument"), "denied commands = [\"core:editor.formatDocument\"] Why::NotDeclared");
    let unknown = execute(&wb, cx, "core:workbench.quit");
    assert!(unknown.contains("Invalid") && unknown.contains("core-commands.md"), "{unknown}");
}

/// What a core command implies is checked on what it touches: opening a file reads it, Save All writes every unsaved
/// file, and a task runs its program.
#[gpui::test]
fn core_commands_need_what_they_imply(cx: &mut TestAppContext) {
    if skipped() {
        return;
    }
    let cfg = config_dir("imply");
    let root = cfg.join("work");
    for dir in ["src", "docs", ".codeyard"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(root.join("docs/guide.md"), "# Guide\n").unwrap();
    std::fs::write(root.join("notes.txt"), "private\n").unwrap();
    let tasks = r#"{ "tasks": [
        { "label": "say", "command": "echo hi" },
        { "label": "chain", "command": "echo hi && echo there" },
        { "label": "make", "command": "make" }
    ] }"#;
    std::fs::write(root.join(".codeyard/tasks.jsonc"), tasks).unwrap();
    let (wb, cx) = setup(cx, &cfg);
    let root_s = root.to_string_lossy().replace('\\', "/");
    wb.update_in(cx, |wb, window, cx| {
        wb.set_root(root_s.clone(), cx);
        wb.reload_config_for_test(window, cx);
    });
    cx.run_until_parked();
    let uri = |rel: &str| crate::ext_broker::uri(&format!("{root_s}/{rel}"));

    // Opening a file implies `read` on it.
    assert_eq!(execute(&wb, cx, &format!("core:editor.open {}", uri("docs/guide.md"))), "done");
    cx.run_until_parked();
    let active = wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).path.clone()));
    assert_eq!(active.as_deref(), Some(format!("{root_s}/docs/guide.md").as_str()));
    assert_eq!(execute(&wb, cx, &format!("core:editor.open {}", uri("notes.txt"))), "denied read = [\"notes.txt\"] Why::NotDeclared");

    // Save All implies `write` on every file it would save.
    let edit = |wb: &Entity<Workbench>, cx: &mut VisualTestContext, rel: &str| {
        let path = format!("{root_s}/{rel}");
        wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(path, window, cx));
        cx.run_until_parked();
        let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).expect("editor");
        ed.update(cx, |e, cx| e.insert_at_for_test(0, "// x\n", cx));
    };
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap();
    edit(&wb, cx, "src/a.rs");
    assert_eq!(execute(&wb, cx, "core:workbench.saveAll"), "done");
    wait_for(cx, &wb, "the save", |_| read("src/a.rs") == "// x\nfn a() {}\n");
    // One file it may not write refuses the whole command: nothing is saved.
    edit(&wb, cx, "src/a.rs");
    edit(&wb, cx, "Cargo.toml");
    assert_eq!(execute(&wb, cx, "core:workbench.saveAll"), "denied write = [\"Cargo.toml\"] Why::NotDeclared");
    std::thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!((read("src/a.rs"), read("Cargo.toml")), ("// x\nfn a() {}\n".to_string(), "[package]\n".to_string()));

    // A task runs only as one program the extension may run.
    assert_eq!(execute(&wb, cx, "core:tasks.run make"), "denied run = [\"make\"] Why::NotDeclared");
    let chain = execute(&wb, cx, "core:tasks.run chain");
    assert!(chain.starts_with("denied a task that runs one program"), "{chain}");
    let missing = execute(&wb, cx, "core:tasks.run nope");
    assert!(missing.contains("NotFound") && missing.contains("nope"), "{missing}");
    assert_eq!(execute(&wb, cx, "core:tasks.run say"), "done");
}

/// Every core command extensions may run is one the app has: those that need no argument are palette commands, under
/// the title the table gives (menus show it).
#[gpui::test]
fn the_core_command_table_matches_the_palette(cx: &mut TestAppContext) {
    let cfg = config_dir("table");
    let (wb, cx) = setup(cx, &cfg);
    let labels = wb.read_with(cx, |wb, _| wb.command_labels_for_test());
    for c in &ext_manifest::CORE_COMMANDS {
        if c.argument.optional() {
            assert!(crate::commands::action_by_id(c.id).is_some(), "{} is not a palette command id", c.id);
            assert!(labels.iter().any(|l| l == c.title), "{}: no palette command is titled {:?}", c.id, c.title);
        } else {
            assert!(c.title.is_empty(), "{}: menus cannot run it, so it has no title", c.id);
        }
    }
}

/// The performance scenarios' decoration provider (crates/ext-host/testdata/decorator.wasm, scripts/perf/run.sh):
/// installed twice from folders, both decorate the open Rust file on `decorate` and answer for their tasks at once.
#[gpui::test]
fn the_perf_decoration_providers_decorate_and_answer(cx: &mut TestAppContext) {
    if skipped() {
        return;
    }
    let cfg = config_dir("perf");
    let store = crate::ext::store(&cfg);
    for n in ["a", "b"] {
        let dir = cfg.join(format!("decorations-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = format!(
            "[extension]\nid = \"perf.decorations-{n}\"\nversion = \"1.0.0\"\nname = \"Decorations\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n\
             [[commands]]\nid = \"decorate\"\ntitle = \"Perf: Decorate\"\n[[decorations]]\nid = \"lines\"\nlanguages = [\"rust\"]\n[[tasks]]\nid = \"noop\"\n"
        );
        std::fs::write(dir.join(ext_manifest::FILE_NAME), manifest).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.join(ext_pkg::COMPONENT), include_bytes!("../../ext-host/testdata/decorator.wasm")).unwrap();
        crate::ext::install_file(&store, &dir).expect("installs from the folder");
    }
    let (wb, cx) = setup(cx, &cfg);
    let file = cfg.join("big.rs");
    std::fs::write(&file, "fn main() {}\n".repeat(50)).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&file.to_string_lossy(), window, cx));
    cx.run_until_parked();
    assert_eq!(wb.update_in(cx, |wb, window, cx| wb.start_decorations_for_perf(window, cx)), 2);
    wait_for(cx, &wb, "both decoration sets", |wb| wb.decoration_sets_any_file_for_test() == 2);
    let done = wb.update_in(cx, |wb, window, cx| wb.ask_tasks_for_perf(window, cx));
    wait_for(cx, &wb, "the task providers", |_| done.get());
}

/// The performance scenarios' view (crates/ext-host/testdata/viewlist.wasm, scripts/perf/run.sh): installed from a
/// folder, its sidebar view opens with 10 000 rows and holds the keyboard, so PageDown reaches the list.
#[gpui::test]
fn the_perf_view_lists_ten_thousand_rows(cx: &mut TestAppContext) {
    if skipped() {
        return;
    }
    let cfg = config_dir("perf-view");
    let dir = cfg.join("viewlist");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = "[extension]\nid = \"perf.viewlist\"\nversion = \"1.0.0\"\nname = \"View List\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n\
                    [[views]]\nid = \"rows\"\ntitle = \"Perf: Rows\"\nlocation = \"sidebar\"\n";
    std::fs::write(dir.join(ext_manifest::FILE_NAME), manifest).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "").unwrap();
    std::fs::write(dir.join(ext_pkg::COMPONENT), include_bytes!("../../ext-host/testdata/viewlist.wasm")).unwrap();
    crate::ext::install_file(&crate::ext::store(&cfg), &dir).expect("installs from the folder");
    let (wb, cx) = open_window(cx, &cfg);
    let id = wb.update_in(cx, |wb, window, cx| wb.open_ext_view_for_perf(window, cx)).expect("a view");
    assert_eq!(id, "perf.viewlist.rows");
    let deadline = Instant::now() + Duration::from_secs(20 * crate::test_sshd::timeout_scale());
    while !wb.update_in(cx, |wb, window, cx| wb.focus_ext_view_for_perf(&id, window, cx)) {
        assert!(Instant::now() < deadline, "the view was not drawn: {:?}", wb.read_with(cx, |wb, _| wb.ext_view_error_for_test(&id)));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_texts_for_test(&id)).len(), 10_000);
    cx.simulate_keystrokes("pagedown pagedown");
    assert_eq!(wb.update_in(cx, |wb, window, _| wb.ext_view_focused_for_test(&id, window)).as_deref(), Some("rows"));
}
