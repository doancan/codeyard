//! QA (workbench, customization, project management): exploratory end-to-end tests
//! that encode VS Code's behaviour. A failing test here is a bug or a gap; see the
//! comment above each test for the expected behaviour.

use std::path::PathBuf;

use gpui::{Entity, Focusable, TestAppContext, VisualTestContext};

use crate::ui::{self, ActiveTheme};
use crate::workbench::Workbench;

/// Every QA fixture lives under one scratch directory (never the user's files).
fn qa_root() -> PathBuf {
    let base = std::env::var_os("CODEYARD_QA_TMP").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = base.join(format!("codeyard-qa-wb-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(d).unwrap()
}

pub(crate) fn fresh(name: &str) -> PathBuf {
    let d = qa_root().join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) fn project(name: &str) -> PathBuf {
    let dir = fresh(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("README.md"), "# proje\n").unwrap();
    std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    let lib: String = (1..=10).map(|i| format!("// line {i}\n")).collect();
    std::fs::write(dir.join("src/lib.rs"), lib).unwrap();
    dir
}

pub(crate) fn globals(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings::default());
    });
}

pub(crate) fn setup(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    globals(cx);
    let (wb, cx) = cx.add_window_view(|window, cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

pub(crate) fn setup_with_config<'a>(cx: &'a mut TestAppContext, config: &std::path::Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    globals(cx);
    let dir = config.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

pub(crate) fn setup_persistent<'a>(cx: &'a mut TestAppContext, home: &std::path::Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    globals(cx);
    let db = home.join("state.db");
    let data = home.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| {
        let mut wb = Workbench::with_state(state::State::open(&db).ok(), None, window, cx);
        wb.set_data_dir(data);
        wb
    });
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

fn restart(wb: Entity<Workbench>, cx: &mut VisualTestContext, home: &std::path::Path) -> Entity<Workbench> {
    restart_with(wb, cx, home, crate::settings::Startup::Continue)
}

/// Quit and reopen the window with the given `window.startup` setting.
pub(crate) fn restart_with(wb: Entity<Workbench>, cx: &mut VisualTestContext, home: &std::path::Path, startup: crate::settings::Startup) -> Entity<Workbench> {
    assert!(wb.update(cx, |wb, cx| wb.prepare_quit(cx)));
    reopen(wb, cx, home, startup)
}

/// `restart_with` after a crash or a kill: nothing is saved on the way out, the next start finds what was last saved.
pub(crate) fn restart_after_kill(wb: Entity<Workbench>, cx: &mut VisualTestContext, home: &std::path::Path) -> Entity<Workbench> {
    reopen(wb, cx, home, crate::settings::Startup::Continue)
}

fn reopen(wb: Entity<Workbench>, cx: &mut VisualTestContext, home: &std::path::Path, startup: crate::settings::Startup) -> Entity<Workbench> {
    drop(wb);
    let db = home.join("state.db");
    let data = home.to_path_buf();
    let new = cx.update(|window, cx| {
        window.replace_root(cx, |window, cx| {
            let mut w = Workbench::with_state(state::State::open(&db).ok(), None, window, cx);
            w.set_data_dir(data);
            w
        })
    });
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().startup = startup);
    new.update_in(cx, |w, window, cx| {
        window.focus(&w.focus_handle(cx));
        w.restore_last(window, cx);
    });
    cx.run_until_parked();
    new
}

pub(crate) fn open(wb: &Entity<Workbench>, path: &std::path::Path, cx: &mut VisualTestContext) {
    let p = path.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&p, window, cx));
    cx.run_until_parked();
}

pub(crate) fn open_in(wb: &Entity<Workbench>, path: &std::path::Path, cx: &mut VisualTestContext) {
    let p = path.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p, window, cx));
    cx.run_until_parked();
}

fn active_name(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<String> {
    wb.read_with(cx, |wb, cx| wb.active_path(cx)).map(|p| crate::ui::file_name(&p).to_string())
}

fn wait_for(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) -> bool {
    let start = std::time::Instant::now();
    loop {
        cx.run_until_parked();
        if done(cx) {
            return true;
        }
        if start.elapsed() > std::time::Duration::from_secs(5) {
            eprintln!("timeout: {what}");
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Generic fixtures test editing and workbench behaviour; they must never launch an installed language server
/// (rust-analyzer would build the scratch project). LSP integration tests turn `lsp_enabled` on explicitly.
#[gpui::test]
fn generic_fixtures_start_no_language_server(cx: &mut TestAppContext) {
    let dir = project("no-lsp-in-fixtures");
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("src/main.rs"), cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.lsp_server_keys_for_test()), Vec::<String>::new(), "no language server was attempted");
    assert!(!cx.update(|_, cx| cx.global::<crate::settings::Settings>().lsp_enabled), "LSP is off by default under test");
}

// ---------------------------------------------------------------------------
// Settings / customization
// ---------------------------------------------------------------------------

/// README documents `"files.exclude": ["**/node_modules"]`, and the VS Code import
/// writes `{"**/.git": true}` as `["**/.git"]`. VS Code hides such entries in the
/// explorer and Go to File. Here patterns containing `/` are ignored by the tree.
#[gpui::test]
fn qa_files_exclude_double_star_glob_hides_folder_in_tree(cx: &mut TestAppContext) {
    let cfg = fresh("cfg-exclude");
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "files.exclude": ["**/node_modules", "**/.git"] }"#).unwrap();
    let dir = project("exclude");
    std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
    std::fs::write(dir.join("node_modules/pkg/index.js"), "x").unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    open(&wb, &dir, cx);
    let names = wb.read_with(cx, |wb, cx| wb.tree_names(cx));
    assert!(!names.contains(&"node_modules".to_string()), "\"**/node_modules\" must hide the folder: {names:?}");
}

/// While the user is typing in settings.jsonc and saves a half-written file, VS Code
/// keeps the last valid settings. Here a parse error resets EVERY setting to the
/// default (theme flips, excludes reset...).
#[gpui::test]
fn qa_broken_settings_file_keeps_last_good_settings(cx: &mut TestAppContext) {
    let cfg = fresh("cfg-broken");
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "theme": "ember", "editor.fontSize": 18 }"#).unwrap();
    let (_wb, cx) = setup_with_config(cx, &cfg);
    let ember = theme::themes::builtin().into_iter().find(|t| t.id == "ember").unwrap().palette;
    assert_eq!(cx.update(|_, cx| cx.global::<ActiveTheme>().0), ember);
    std::thread::sleep(std::time::Duration::from_millis(1100)); // new mtime
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "theme": "ember", "editor.fontSize": 18, "#).unwrap(); // mid-edit
    cx.executor().advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    let size = cx.update(|_, cx| cx.global::<crate::settings::Settings>().editor_font_size);
    assert_eq!(size, 18., "font size must not reset to the default on a syntax error");
    assert_eq!(cx.update(|_, cx| cx.global::<ActiveTheme>().0), ember, "theme must not flip on a syntax error");
}

/// An unknown theme id in settings.jsonc should be reported (VS Code: "theme not found").
#[gpui::test]
fn qa_unknown_theme_in_settings_is_reported(cx: &mut TestAppContext) {
    let cfg = fresh("cfg-theme");
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "theme": "no-such-theme" }"#).unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    let notice = wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default();
    assert!(notice.contains("no-such-theme"), "expected a warning about the unknown theme, got {notice:?}");
}

/// README: `{ "key": "cmd-d", "command": null }   // unbind`. The default ⌘D is bound
/// in the Editor context; the unbind without "when" must still turn it off.
#[gpui::test]
fn qa_readme_unbind_example_turns_off_cmd_d(cx: &mut TestAppContext) {
    let cfg = fresh("cfg-unbind");
    let key = if cfg!(target_os = "macos") { "cmd-d" } else { "ctrl-d" };
    std::fs::write(cfg.join("keybindings.jsonc"), format!(r#"[ {{ "key": "{key}", "command": null }} ]"#)).unwrap();
    let dir = fresh("unbind");
    std::fs::write(dir.join("a.txt"), "foo foo\n").unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    open(&wb, &dir.join("a.txt"), cx);
    cx.simulate_keystrokes("cmd-d");
    let sel = wb.read_with(cx, |wb, cx| wb.active_selection(cx));
    assert_eq!(sel, None, "⌘D was unbound, nothing should be selected");
}

/// VS Code key syntax is `cmd+1`. Users pasting from VS Code docs get either a
/// working binding or a warning; here we expect one of the two.
#[gpui::test]
fn qa_vscode_plus_key_syntax_works_or_warns(cx: &mut TestAppContext) {
    let cfg = fresh("cfg-plus");
    let key = if cfg!(target_os = "macos") { "cmd+1" } else { "ctrl+1" };
    std::fs::write(cfg.join("keybindings.jsonc"), format!(r#"[ {{ "key": "{key}", "command": "workbench.quickOpen" }} ]"#)).unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    let notice = wb.read_with(cx, |wb, _| wb.notice_text());
    cx.simulate_keystrokes("cmd-1");
    let works = wb.read_with(cx, |wb, _| wb.palette_open());
    assert!(works || notice.is_some(), "\"cmd+1\" neither works nor produces a warning");
}

/// VS Code applies `.vscode/settings.json` (workspace settings) on top of user settings.
#[gpui::test]
fn qa_workspace_settings_vscode_folder_are_applied(cx: &mut TestAppContext) {
    let dir = project("ws-settings");
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();
    std::fs::write(dir.join(".vscode/settings.json"), r#"{ "editor.tabSize": 2, "editor.insertSpaces": true }"#).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    cx.simulate_keystrokes("cmd-down tab");
    cx.simulate_input("x");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap(), "fn main() {}\n  x");
}

// ---------------------------------------------------------------------------
// Explorer
// ---------------------------------------------------------------------------

/// Case-only rename (a.txt → A.txt) works in VS Code on macOS. Here the agent's
/// "target exists" check sees the same file on the case-insensitive disk and refuses.
#[gpui::test]
fn qa_case_only_rename_in_tree(cx: &mut TestAppContext) {
    let dir = fresh("case-rename");
    std::fs::write(dir.join("readme.txt"), "x\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("down f2");
    for _ in 0.."readme.txt".len() {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_input("README.txt");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.contains(&"README.txt".to_string()), "on disk: {names:?}; notice: {:?}", wb.read_with(cx, |wb, _| wb.notice_text()));
}

/// Deleting a file whose editor has unsaved changes: VS Code keeps the dirty editor
/// open (marked deleted) so the text can still be saved. Here the tab is closed
/// and its hot-exit backup removed: the unsaved text is gone for good.
#[gpui::test]
fn qa_delete_file_with_unsaved_changes_keeps_the_text(cx: &mut TestAppContext) {
    let dir = fresh("delete-dirty");
    std::fs::write(dir.join("a.txt"), "disk\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("a.txt"), cx);
    cx.simulate_input("unsaved work ");
    assert!(wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("down cmd-backspace");
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    assert!(!dir.join("a.txt").exists());
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity());
    let text = ed.map(|e| e.read_with(cx, |e, _| e.text()));
    assert_eq!(text.as_deref(), Some("unsaved work disk\n"), "the unsaved text must survive the delete");
}

/// Renaming a file with unsaved changes (and a hot-exit backup) and restarting: only
/// the renamed file should come back. A stale backup under the old name would
/// resurrect a phantom "deleted" tab.
#[gpui::test]
fn qa_rename_dirty_file_then_restart_has_no_ghost_tab(cx: &mut TestAppContext) {
    let home = fresh("home-rename");
    let dir = fresh("rename-dirty");
    std::fs::write(dir.join("a.txt"), "disk\n").unwrap();
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir.join("a.txt"), cx);
    cx.simulate_input("unsaved ");
    cx.executor().advance_clock(std::time::Duration::from_secs(2)); // backup written
    cx.run_until_parked();
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("down f2");
    for _ in 0.."a.txt".len() {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_input("b.txt");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(dir.join("b.txt").exists());
    cx.executor().advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    let wb2 = restart(wb, cx, &home);
    let paths: Vec<String> = wb2.read_with(cx, |w, cx| w.open_paths(cx)).into_iter().map(|p| crate::ui::file_name(&p).to_string()).collect();
    assert_eq!(paths, vec!["b.txt".to_string()], "only the renamed file should be restored");
}

/// VS Code's "New File…" (and the explorer) accepts `a/b/c.txt` and creates the
/// missing folders. Here ⌘N offers "Create file “deep/er/x.rs”" but then fails.
#[gpui::test]
fn qa_new_file_with_missing_parent_folders(cx: &mut TestAppContext) {
    let dir = project("newfile-deep");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("cmd-n");
    cx.simulate_input("deep/er/x.rs");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(dir.join("deep/er/x.rs").is_file(), "notice: {:?}", wb.read_with(cx, |wb, _| wb.notice_text()));
}

/// VS Code explorer keyboard: Home / End jump to the first / last row.
#[gpui::test]
fn qa_tree_home_end_keys(cx: &mut TestAppContext) {
    let dir = project("tree-keys");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("end");
    let sel = wb.read_with(cx, |wb, cx| wb.tree_selected(cx)).unwrap_or_default();
    assert!(sel.ends_with("README.md"), "End should select the last row, got {sel:?}");
}

// ---------------------------------------------------------------------------
// Tabs, groups, navigation
// ---------------------------------------------------------------------------

/// VS Code: ⌃Tab cycles editors, ⌥⌘→ / ⌥⌘← (and ⌃PgDn) go to next/previous tab.
#[gpui::test]
fn qa_keyboard_next_previous_tab(cx: &mut TestAppContext) {
    let dir = project("tab-keys");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("README.md"), cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    assert_eq!(active_name(&wb, cx).as_deref(), Some("main.rs"));
    cx.simulate_keystrokes("alt-cmd-left");
    cx.run_until_parked();
    let after_alt_cmd = active_name(&wb, cx);
    cx.simulate_keystrokes("ctrl-pageup");
    cx.run_until_parked();
    let after_ctrl_pgup = active_name(&wb, cx);
    assert!(
        after_alt_cmd.as_deref() == Some("README.md") || after_ctrl_pgup.as_deref() == Some("README.md"),
        "no keyboard shortcut switches to the previous tab ({after_alt_cmd:?}, {after_ctrl_pgup:?})"
    );
}

/// The palette should contain the core project/workbench commands VS Code users
/// reach for. Missing ones are listed in the failure message.
#[gpui::test]
fn qa_palette_has_core_workbench_commands(cx: &mut TestAppContext) {
    let dir = project("palette-cmds");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("cmd-shift-p");
    cx.run_until_parked();
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    let lower: Vec<String> = labels.iter().map(|l| l.to_lowercase()).collect();
    // (VS Code command, words that would satisfy it here)
    let wanted: &[(&str, &[&str])] = &[
        ("File: Open Recent…", &["recent"]),
        ("File: Close Folder", &["close folder"]),
        ("View: Close All Editors", &["close all"]),
        ("View: Close Other Editors", &["close other"]),
        ("View: Open Next Editor", &["next editor"]),
        ("View: Keep Editor (pin preview)", &["keep editor", "pin"]),
        ("Terminal: Clear", &["clear"]),
        ("File: Copy Path of Active File", &["copy path"]),
        ("File: Compare Active File With…", &["compare"]),
        ("File: New Folder…", &["new folder"]),
        ("File: Refresh Explorer", &["refresh"]),
        ("Workspaces: Add Folder to Workspace…", &["add folder"]),
        ("View: Toggle Maximized Panel / Move Panel", &["move panel", "panel position"]),
        ("Preferences: Open Workspace Settings", &["workspace settings"]),
        ("Tasks: Run Task", &["run task"]),
    ];
    let missing: Vec<&str> = wanted.iter().filter(|(_, alts)| !lower.iter().any(|l| alts.iter().any(|a| l.contains(a)))).map(|(name, _)| *name).collect();
    assert!(missing.is_empty(), "palette lacks: {missing:?}");
}

/// VS Code restores the editor layout (both groups) on restart.
#[gpui::test]
fn qa_session_restores_split_groups(cx: &mut TestAppContext) {
    let home = fresh("home-split");
    let dir = project("split-restore");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    open_in(&wb, &dir.join("src/lib.rs"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Move Editor into Other Group", window, cx));
    cx.run_until_parked(); // lib.rs → right group
    let before = wb.read_with(cx, |w, cx| w.groups(cx));
    assert!(before.1.is_some());
    let wb2 = restart(wb, cx, &home);
    let after = wb2.read_with(cx, |w, cx| w.groups(cx));
    assert_eq!(after, before, "split layout should come back after restart");
}

/// VS Code: when a tab is restored after restart its preview state is dropped and
/// pinned tabs are restored before others; here we check that a tab ORDER the
/// user arranged via the right group survives (left group tabs first).
#[gpui::test]
fn qa_session_tab_order_with_focus_in_right_group(cx: &mut TestAppContext) {
    let home = fresh("home-order");
    let dir = project("order-restore");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("README.md"), cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    open_in(&wb, &dir.join("src/lib.rs"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Move Editor into Other Group", window, cx));
    cx.run_until_parked(); // lib.rs → right group, focus right
    let wb2 = restart(wb, cx, &home);
    // The layout comes back: left group order, right group and focus (on the right) are kept.
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(wb2.read_with(cx, |w, cx| w.groups(cx)), (s(&["README.md", "main.rs"]), Some(s(&["lib.rs"])), false));
}

// ---------------------------------------------------------------------------
// Quick open
// ---------------------------------------------------------------------------

/// VS Code ⌘P accepts `file:line` (e.g. "lib.rs:5") and jumps there.
#[gpui::test]
fn qa_quick_open_file_colon_line(cx: &mut TestAppContext) {
    let dir = project("qo-line");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    cx.simulate_input("lib.rs:5");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(active_name(&wb, cx).as_deref(), Some("lib.rs"));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)).map(|c| c.0), Some(5));
}

/// VS Code ⌃G accepts `line:column`.
#[gpui::test]
fn qa_go_to_line_and_column(cx: &mut TestAppContext) {
    let dir = project("goto-col");
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("src/lib.rs"), cx);
    cx.simulate_keystrokes("ctrl-g");
    cx.simulate_input("3:4");
    cx.simulate_keystrokes("enter");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)), Some((3, 4)));
}

/// VS Code ranks recently opened files first even while typing (for equal matches).
#[gpui::test]
fn qa_quick_open_ranks_recent_file_first_when_typing(cx: &mut TestAppContext) {
    let dir = project("qo-recent");
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("zzz/deeper")).unwrap();
    std::fs::write(dir.join("a/mod.rs"), "").unwrap();
    std::fs::write(dir.join("zzz/deeper/mod.rs"), "").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("zzz/deeper/mod.rs"), cx);
    open_in(&wb, &dir.join("README.md"), cx);
    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    cx.simulate_input("mod");
    cx.run_until_parked();
    let details = wb.read_with(cx, |wb, cx| wb.overlay_details(cx)).unwrap();
    assert_eq!(details.first().map(String::as_str), Some("zzz/deeper"), "recently opened match should be first: {details:?}");
}

/// Quick open after a file was created outside the app (in a folder never expanded
/// in the tree) should find it.
#[gpui::test]
fn qa_quick_open_sees_externally_created_file(cx: &mut TestAppContext) {
    let dir = project("qo-external");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    std::fs::create_dir_all(dir.join("later")).unwrap();
    std::fs::write(dir.join("later/brandnew.rs"), "").unwrap();
    cx.simulate_keystrokes("cmd-p");
    cx.simulate_input("brandnew");
    let found = wait_for(cx, "file listed", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap_or_default().iter().any(|l| l == "brandnew.rs"));
    assert!(found);
}

/// Closing a dirty tab: VS Code asks "Save / Don't Save / Cancel". Here the dialog
/// only offers "Close without saving" and "Cancel" (no way to save from it).
#[gpui::test]
fn qa_close_dirty_tab_offers_save(cx: &mut TestAppContext) {
    let dir = fresh("close-save");
    std::fs::write(dir.join("a.txt"), "disk\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("a.txt"), cx);
    cx.simulate_input("new ");
    cx.simulate_keystrokes("cmd-w");
    assert!(cx.has_pending_prompt());
    let answers = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cx.simulate_prompt_answer("Save")));
    assert!(answers.is_ok(), "the unsaved-changes dialog has no \"Save\" button");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "new disk\n");
}

#[test]
fn breadcrumbs_cut_root_only_at_folder_boundary() {
    use crate::workbench::crumb_paths;
    let names = |v: Vec<(String, String)>| v.into_iter().map(|(n, _)| n).collect::<Vec<_>>();
    assert_eq!(names(crumb_paths(Some("/a/b"), "/a/b/src/x.rs")), ["src", "x.rs"]);
    assert_eq!(crumb_paths(Some("/a/b/"), "/a/b/src/x.rs")[0].1, "/a/b/src");
    // `/a/bc` is not under the root: full path
    assert_eq!(names(crumb_paths(Some("/a/b"), "/a/bc/x.rs")), ["a", "bc", "x.rs"]);
    assert_eq!(crumb_paths(Some("/a/b"), "/a/bc/x.rs")[1].1, "/a/bc");
    assert_eq!(names(crumb_paths(None, "/tmp/y.txt")), ["tmp", "y.txt"]);
}

#[test]
fn window_bounds_round_trip() {
    use crate::workbench::{format_bounds, parse_bounds};
    let r = gpui::Bounds::new(gpui::point(gpui::px(40.), gpui::px(-20.)), gpui::size(gpui::px(1500.), gpui::px(900.)));
    for b in [gpui::WindowBounds::Windowed(r), gpui::WindowBounds::Maximized(r), gpui::WindowBounds::Fullscreen(r)] {
        assert_eq!(parse_bounds(&format_bounds(b)), Some(b));
    }
    assert_eq!(parse_bounds("windowed 0 0 10 10"), None, "window too small to be real");
    assert_eq!(parse_bounds("tiled 0 0 800 600"), None);
    assert_eq!(parse_bounds("windowed 0 0 800"), None);
}

#[gpui::test]
fn qa_info_messages_fade_errors_stay_until_closed(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let notice = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.notice_text());
    let tick = |cx: &mut VisualTestContext, s: u64| {
        cx.executor().advance_clock(std::time::Duration::from_secs(s));
        cx.run_until_parked();
    };
    wb.update(cx, |wb, cx| wb.set_notice_for_test("Kaydedildi", false, cx));
    cx.run_until_parked();
    tick(cx, 5);
    assert_eq!(notice(cx).as_deref(), Some("Kaydedildi"), "visible for a short time");
    // A new message isn't affected by the old timer.
    wb.update(cx, |wb, cx| wb.set_notice_for_test("İkinci", false, cx));
    cx.run_until_parked();
    tick(cx, 4);
    assert_eq!(notice(cx).as_deref(), Some("İkinci"));
    tick(cx, 5);
    assert_eq!(notice(cx), None, "info message goes away");
    // If the same text arrives again the duration restarts.
    wb.update(cx, |wb, cx| wb.set_notice_for_test("Stashed changes", false, cx));
    cx.run_until_parked();
    tick(cx, 6);
    wb.update(cx, |wb, cx| wb.set_notice_for_test("Stashed changes", false, cx));
    cx.run_until_parked();
    tick(cx, 6);
    assert_eq!(notice(cx).as_deref(), Some("Stashed changes"), "second message has not expired");
    // The error stays.
    wb.update(cx, |wb, cx| wb.set_notice_for_test("Bağlantı koptu", true, cx));
    cx.run_until_parked();
    tick(cx, 60);
    assert_eq!(notice(cx).as_deref(), Some("Bağlantı koptu"));
}

#[test]
fn same_name_tabs_get_the_shortest_distinguishing_folder() {
    use crate::workbench::twin_hints;
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let s = |o: &[Option<&str>]| o.iter().map(|x| x.map(str::to_string)).collect::<Vec<_>>();
    assert_eq!(twin_hints(&v(&["server/src/main.rs", "client/src/main.rs", "README.md"])), s(&[Some("server/src"), Some("client/src"), None]));
    assert_eq!(twin_hints(&v(&["a/lib.rs", "b/lib.rs"])), s(&[Some("a"), Some("b")]));
    assert_eq!(twin_hints(&v(&["x/a/b/c/mod.rs", "y/a/b/c/mod.rs"])), s(&[Some("x/a/b/c"), Some("y/a/b/c")]));
    assert_eq!(twin_hints(&v(&["p/q/r/mod.rs", "p/s/r/mod.rs"])), s(&[Some("…/q/r"), Some("…/s/r")]));
    // File at the root vs one in a subfolder: root as "/".
    assert_eq!(twin_hints(&v(&["main.rs", "src/main.rs"])), s(&[Some("/"), Some("src")]));
}

#[test]
fn saved_window_must_be_visible_enough_on_a_display() {
    use crate::workbench::visible_enough;
    use gpui::{Bounds, point, px, size};
    let display = Bounds::new(point(px(0.), px(0.)), size(px(1440.), px(900.)));
    let win = |x: f32, y: f32| Bounds::new(point(px(x), px(y)), size(px(1280.), px(800.)));
    assert!(visible_enough(display, win(80., 50.)));
    assert!(visible_enough(display, win(1000., 50.)), "half outside but still grabbable");
    assert!(!visible_enough(display, win(1430., 50.)), "only 10 px visible");
    assert!(!visible_enough(display, win(100., 880.)), "title bar below the screen");
    assert!(!visible_enough(display, win(100., -700.)), "title bar above the screen");
    assert!(!visible_enough(display, win(3000., 50.)), "second display was removed");
}

/// Rename (codeapp → codeyard): the old settings/state folder is moved once; if the new one exists
/// it is left alone. The old `.codeapp/settings.jsonc` inside projects is still read.
#[test]
fn legacy_app_folder_moves_once() {
    use crate::workbench::move_legacy_dir;
    let base = std::env::temp_dir().join(format!("codeyard-migrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("codeapp/themes")).unwrap();
    std::fs::write(base.join("codeapp/settings.jsonc"), "{ \"editor.fontSize\": 15 }").unwrap();
    let new = base.join("codeyard");
    assert!(move_legacy_dir(&new).unwrap());
    assert_eq!(std::fs::read_to_string(new.join("settings.jsonc")).unwrap(), "{ \"editor.fontSize\": 15 }");
    assert!(new.join("themes").is_dir() && !base.join("codeapp").exists());
    // If an old folder reappears while the new one exists: left alone.
    std::fs::create_dir_all(base.join("codeapp")).unwrap();
    std::fs::write(base.join("codeapp/settings.jsonc"), "eski").unwrap();
    assert!(!move_legacy_dir(&new).unwrap());
    assert_eq!(std::fs::read_to_string(new.join("settings.jsonc")).unwrap(), "{ \"editor.fontSize\": 15 }");
    assert!(!move_legacy_dir(&base.join("yok/codeyard")).unwrap());
}

#[gpui::test]
fn legacy_project_settings_folder_still_applies(cx: &mut TestAppContext) {
    let dir = project("legacy-settings");
    std::fs::create_dir_all(dir.join(".codeapp")).unwrap();
    std::fs::write(dir.join(".codeapp/settings.jsonc"), r#"{ "editor.tabSize": 3, "editor.insertSpaces": true }"#).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    cx.simulate_keystrokes("cmd-down tab");
    cx.simulate_input("x");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap(), "fn main() {}\n   x");
}

/// "Save a Copy to This Computer": `~` is the home folder, a relative path is relative to it, an existing
/// absolute path stays, and an empty or folder-like name is refused.
#[test]
fn save_local_copy_resolves_the_typed_path() {
    use crate::workbench::local_copy_target;
    let home = std::path::Path::new("/home/ada");
    assert_eq!(local_copy_target("~/Downloads/a.rs", Some(home)).unwrap(), "/home/ada/Downloads/a.rs");
    assert_eq!(local_copy_target("  notes/a.txt ", Some(home)).unwrap(), "/home/ada/notes/a.txt");
    assert_eq!(local_copy_target("/tmp/a.txt", Some(home)).unwrap(), "/tmp/a.txt");
    assert_eq!(local_copy_target("~", Some(home)).unwrap(), "/home/ada");
    assert!(local_copy_target("", Some(home)).is_err());
    assert!(local_copy_target("~/Downloads/", Some(home)).is_err());
    // Without a known home dir a relative path is rejected, an absolute one works.
    assert!(local_copy_target("a.txt", None).is_err());
    assert_eq!(local_copy_target("/tmp/a.txt", None).unwrap(), "/tmp/a.txt");
}

/// Dragging the sidebar's resize line must resize it and nothing else: the explorer row under the
/// line used to start its own drag-and-drop (moving a file) when the line was grabbed next to it.
#[gpui::test]
fn qa_sidebar_sash_drag_does_not_drag_explorer_rows(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px};
    let dir = project("sash-drag");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.run_until_parked();
    let w0 = wb.read_with(cx, |wb, _| wb.sidebar_width_for_test());
    let x = crate::ui::layout::ACTIVITYBAR_W + w0;
    // The line straddles the sidebar's right edge: press on the half that overlaps the rows, at
    // every height the tree can be at.
    for y in (60..400).step_by(7) {
        let at = point(px(x - 1.), px(y as f32));
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(point(px(x + 40.), px(y as f32)), Some(MouseButton::Left), Modifiers::none());
        assert!(!cx.update(|_, cx| cx.has_active_drag()), "a file/folder drag started from the sash at y={y}");
        cx.simulate_mouse_up(point(px(x + 40.), px(y as f32)), MouseButton::Left, Modifiers::none());
        // Back to the original width for the next height.
        wb.update(cx, |wb, cx| {
            wb.set_sidebar_width_for_test(w0);
            cx.notify();
        });
        cx.run_until_parked();
    }
    // And the line really resizes.
    let at = point(px(x - 1.), px(200.));
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(point(px(x + 60.), px(200.)), Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(point(px(x + 60.), px(200.)), MouseButton::Left, Modifiers::none());
    assert!(wb.read_with(cx, |wb, _| wb.sidebar_width_for_test()) > w0 + 30., "the sidebar did not grow");
    let released_width = wb.read_with(cx, |wb, _| wb.sidebar_width_for_test());
    cx.simulate_mouse_move(point(px(x + 100.), px(200.)), None, Modifiers::none());
    assert_eq!(wb.read_with(cx, |wb, _| wb.sidebar_width_for_test()), released_width, "the sidebar kept resizing after release");
}

/// The default start is the welcome page: the last folder (or SSH host) is listed in Recent but
/// does not open by itself. `window.startup = "continue"` brings it back like a browser would.
#[gpui::test]
fn qa_default_startup_is_the_welcome_page_and_continue_resumes(cx: &mut TestAppContext) {
    use crate::settings::Startup;
    assert_eq!(crate::settings::Settings::default().startup, Startup::Welcome, "welcome is the default");
    let home = fresh("home-startup");
    let dir = project("startup");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    assert_eq!(wb.read_with(cx, |w, cx| w.root_for_test(cx)).as_deref(), Some(dir.to_string_lossy().as_ref()));

    let welcome = restart_with(wb, cx, &home, Startup::Welcome);
    assert_eq!(welcome.read_with(cx, |w, cx| w.root_for_test(cx)), None, "welcome: no folder opens by itself");
    assert!(welcome.read_with(cx, |w, cx| w.open_paths(cx)).is_empty(), "welcome: no tabs come back");

    let resumed = restart_with(welcome, cx, &home, Startup::Continue);
    assert_eq!(resumed.read_with(cx, |w, cx| w.root_for_test(cx)).as_deref(), Some(dir.to_string_lossy().as_ref()), "continue: the last folder comes back");
    assert!(resumed.read_with(cx, |w, cx| w.open_paths(cx)).iter().any(|p| p.ends_with("src/main.rs")), "continue: the tabs come back");
}

/// A folder that is gone is not reopened by "continue"; the window stays on the start page and says why.
#[gpui::test]
fn qa_continue_with_a_deleted_folder_falls_back_to_the_welcome_page(cx: &mut TestAppContext) {
    use crate::settings::Startup;
    let home = fresh("home-startup-gone");
    let dir = project("startup-gone");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    std::fs::remove_dir_all(&dir).unwrap();
    let again = restart_with(wb, cx, &home, Startup::Continue);
    assert_eq!(again.read_with(cx, |w, cx| w.root_for_test(cx)), None, "nothing to reopen");
    let note = again.read_with(cx, |w, _| w.notice_text()).unwrap_or_default();
    assert!(note.contains("no longer exists"), "{note:?}");
}

/// "blank" shows no start page in an empty editor; "welcome" and "continue" do.
#[test]
fn qa_blank_startup_hides_the_welcome_content() {
    use crate::settings::Startup;
    for (text, welcome) in [
        ("{}", true),
        ("{ \"window.startup\": \"welcome\" }", true),
        ("{ \"window.startup\": \"continue\" }", true),
        ("{ \"window.startup\": \"blank\" }", false),
    ] {
        let (s, w) = crate::settings::parse(text);
        assert!(w.is_empty(), "{text}: {w:?}");
        assert_eq!(s.shows_welcome(), welcome, "{text}");
    }
    // The old key still works when the new one is not written, and the new one wins.
    assert_eq!(crate::settings::parse("{ \"workbench.startupEditor\": \"none\" }").0.startup, Startup::Blank);
    assert_eq!(crate::settings::parse("{ \"workbench.startupEditor\": \"none\", \"window.startup\": \"continue\" }").0.startup, Startup::Continue);
    for s in [Startup::Welcome, Startup::Continue, Startup::Blank] {
        assert_eq!(Startup::from_id(s.id()), Some(s));
    }
}

/// The version lives in About (gear menu, app menu, palette), not on the start page.
#[gpui::test]
fn qa_about_is_reachable_and_shows_the_version(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    assert!(wb.read_with(cx, |wb, _| wb.command_labels_for_test()).iter().any(|l| l == "Help: About Codeyard"));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Help: About Codeyard", window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("about", true)));
    assert!(wb.read_with(cx, |wb, _| wb.command_labels_for_test()).len() > 50);
    cx.simulate_keystrokes("enter");
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(format!("Codeyard {}", env!("CARGO_PKG_VERSION"))));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.page_for_test()).is_none());
}

/// The setting is a normal choice in the settings page and in settings.jsonc.
#[test]
fn qa_startup_setting_parses_and_is_in_the_settings_page() {
    use crate::settings::Startup;
    let (s, warnings) = crate::settings::parse("{ \"window.startup\": \"continue\" }");
    assert_eq!(s.startup, Startup::Continue);
    assert!(warnings.is_empty(), "{warnings:?}");
    let (s, _) = crate::settings::parse("{ \"window.startup\": \"nonsense\" }");
    assert_eq!(s.startup, Startup::Welcome, "unknown value keeps the default");
    assert!(crate::workbench::settings_ui::entry("window.startup").is_some());
}

fn drag(wb: &Entity<Workbench>, cx: &mut VisualTestContext, from: &std::path::Path, to_dir: &std::path::Path) {
    let ev = crate::tree::TreeEvent::Move { from: from.to_string_lossy().into_owned(), to_dir: to_dir.to_string_lossy().into_owned() };
    wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(ev, window, cx));
    cx.run_until_parked();
}

/// Dragging in the explorer asks first; cancelling leaves the file where it was.
#[gpui::test]
fn qa_drag_and_drop_asks_and_cancel_keeps_the_file(cx: &mut TestAppContext) {
    let dir = project("drag-cancel");
    std::fs::create_dir_all(dir.join("dest")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    drag(&wb, cx, &dir.join("README.md"), &dir.join("dest"));
    assert!(cx.has_pending_prompt(), "must ask before moving by drag");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(dir.join("README.md").exists() && !dir.join("dest/README.md").exists());
}

/// With `explorer.confirmDragAndDrop` off the drag moves right away, and a drop onto the file's own folder is ignored.
#[gpui::test]
fn qa_drag_and_drop_moves_silently_when_the_setting_is_off(cx: &mut TestAppContext) {
    let dir = project("drag-silent");
    std::fs::create_dir_all(dir.join("dest")).unwrap();
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().confirm_drag_and_drop = false);
    open(&wb, &dir, cx);
    drag(&wb, cx, &dir.join("README.md"), &dir.join("dest"));
    assert!(!cx.has_pending_prompt());
    assert!(dir.join("dest/README.md").exists() && !dir.join("README.md").exists());
    // Dropping onto the folder it already lives in changes nothing and asks nothing.
    drag(&wb, cx, &dir.join("dest/README.md"), &dir.join("dest"));
    assert!(!cx.has_pending_prompt());
    assert!(dir.join("dest/README.md").exists());
}

/// A drop that could never move anything (onto itself, into its own subfolder) does not even ask.
#[gpui::test]
fn qa_drag_onto_itself_or_its_own_subfolder_does_not_ask(cx: &mut TestAppContext) {
    let dir = project("drag-self");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    drag(&wb, cx, &dir.join("src"), &dir.join("src"));
    assert!(!cx.has_pending_prompt());
    std::fs::create_dir_all(dir.join("src/inner")).unwrap();
    drag(&wb, cx, &dir.join("src"), &dir.join("src/inner"));
    assert!(!cx.has_pending_prompt());
    assert!(dir.join("src/lib.rs").exists());
}

/// Nothing opens the terminal by itself: a fresh window, an opened folder and a resumed session all
/// start with the bottom panel closed and no shell running.
#[gpui::test]
fn qa_no_terminal_opens_by_itself(cx: &mut TestAppContext) {
    use crate::settings::Startup;
    let home = fresh("home-no-terminal");
    let dir = project("no-terminal");
    let (wb, cx) = setup_persistent(cx, &home);
    assert!(!wb.read_with(cx, |w, _| w.bottom_panel_open_for_test()) && wb.read_with(cx, |w, _| w.terminal_count()) == 0, "fresh window");
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    assert!(!wb.read_with(cx, |w, _| w.bottom_panel_open_for_test()) && wb.read_with(cx, |w, _| w.terminal_count()) == 0, "opened folder");
    for startup in [Startup::Continue, Startup::Welcome, Startup::Blank] {
        let wb = restart_with(wb.clone(), cx, &home, startup);
        assert!(!wb.read_with(cx, |w, _| w.bottom_panel_open_for_test()), "{startup:?}: the bottom panel stays closed");
        assert_eq!(wb.read_with(cx, |w, _| w.terminal_count()), 0, "{startup:?}: no shell is started");
    }
}

fn recent_paths(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<(String, bool)> {
    wb.read_with(cx, |w, _| w.recent_for_test()).into_iter().map(|(_, p, pinned)| (p, pinned)).collect()
}

/// Pinning a recent folder floats it to the top, survives a restart, and forgetting it also unpins it.
#[gpui::test]
fn qa_recent_folders_can_be_pinned_and_removed(cx: &mut TestAppContext) {
    use crate::settings::Startup;
    let home = fresh("home-pin");
    let (a, b, c) = (project("pin-a"), project("pin-b"), project("pin-c"));
    let (wb, cx) = setup_persistent(cx, &home);
    for d in [&a, &b, &c] {
        open(&wb, d, cx);
    }
    let paths: Vec<String> = [&c, &b, &a].iter().map(|d| ui::local_path(d)).collect();
    assert_eq!(recent_paths(&wb, cx).iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(), paths, "most recent first");

    // Pin the old one: it moves to the top, marked pinned.
    wb.update(cx, |w, cx| w.toggle_pin_recent_for_test(None, &paths[2], cx));
    assert_eq!(recent_paths(&wb, cx)[0], (paths[2].clone(), true));
    assert_eq!(recent_paths(&wb, cx).iter().filter(|(_, p)| *p).count(), 1);
    // Pinning and order survive a restart.
    let wb = restart_with(wb, cx, &home, Startup::Welcome);
    assert_eq!(recent_paths(&wb, cx)[0], (paths[2].clone(), true), "the pin is remembered");
    // Unpin: back to normal order. Then delete the pinned one: the pin goes too.
    wb.update(cx, |w, cx| w.toggle_pin_recent_for_test(None, &paths[2], cx));
    assert!(recent_paths(&wb, cx).iter().all(|(_, p)| !p));
    wb.update(cx, |w, cx| w.toggle_pin_recent_for_test(None, &paths[1], cx));
    wb.update(cx, |w, cx| w.forget_recent_for_test(None, &paths[1], cx));
    let left = recent_paths(&wb, cx);
    assert!(left.iter().all(|(p, pinned)| *p != paths[1] && !pinned), "{left:?}");
    assert_eq!(left.len(), 2);
}

/// A recent whose folder is gone is not opened: the window asks whether to drop it from Recent, and
/// "Keep" really keeps it.
#[gpui::test]
fn qa_opening_a_missing_recent_asks_to_remove_it(cx: &mut TestAppContext) {
    use crate::settings::Startup;
    let home = fresh("home-missing-recent");
    let (keep_dir, gone_dir) = (project("missing-keep"), project("missing-gone"));
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &gone_dir, cx);
    open(&wb, &keep_dir, cx);
    std::fs::remove_dir_all(&gone_dir).unwrap();
    let wb = restart_with(wb, cx, &home, Startup::Welcome);
    let gone = ui::local_path(&gone_dir);
    let open_gone = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| {
        let r = state::RecentWorkspace { host: None, path: gone.clone(), last_opened_at: 0 };
        wb.update_in(cx, |w, window, cx| w.open_recent(r, window, cx));
        cx.run_until_parked();
    };
    open_gone(&wb, cx);
    assert!(cx.has_pending_prompt(), "must ask before touching the list");
    assert_eq!(wb.read_with(cx, |w, cx| w.root_for_test(cx)), None, "nothing was opened");
    cx.simulate_prompt_answer("Keep");
    cx.run_until_parked();
    assert!(recent_paths(&wb, cx).iter().any(|(p, _)| *p == gone), "Keep keeps it");
    open_gone(&wb, cx);
    cx.simulate_prompt_answer("Remove from Recent");
    cx.run_until_parked();
    assert!(recent_paths(&wb, cx).iter().all(|(p, _)| *p != gone), "Remove drops it");
    assert!(recent_paths(&wb, cx).iter().any(|(p, _)| *p == ui::local_path(&keep_dir)), "the other recent stays");
}
