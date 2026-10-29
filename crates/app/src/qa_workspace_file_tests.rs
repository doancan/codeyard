//! Workspace files end to end: opening, names, saving back without loss, reload, missing folders, conflicts, write
//! failures, settings precedence and provenance, shared tasks behind trust, restart restore and SSH. Every test owns its
//! temporary folders.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Entity, Focusable, TestAppContext, VisualTestContext};

use crate::folder_settings::FolderSettings;
use crate::qa_integration_tests::{notice, setup_with, wait_secs};
use crate::settings::{Scope, ScopedLayer, Settings, parse_scoped};
use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-ws-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(d).unwrap()
}

fn setup(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    setup_with(cx, Settings { lsp_enabled: false, ..Default::default() })
}

fn s(p: &Path) -> String {
    crate::ui::local_path(p)
}

fn open(wb: &Entity<Workbench>, path: &Path, cx: &mut VisualTestContext) {
    let p = s(path);
    wb.update_in(cx, |wb, window, cx| wb.open_path(&p, window, cx));
}

fn wait(cx: &mut VisualTestContext, what: &str, done: impl FnMut(&mut VisualTestContext) -> bool) {
    wait_secs(cx, 15, what, done);
}

type Snapshot = (String, Vec<String>, Vec<String>, Option<String>);

fn snap(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<Snapshot> {
    wb.read_with(cx, |wb, cx| wb.workspace_snapshot_for_test(cx))
}

fn labels(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    snap(wb, cx).map(|s| s.1).unwrap_or_default()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

/// Wait until no write of the workspace file is running.
fn settle(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wait(cx, "workspace write finished", |cx| !wb.read_with(cx, |wb, _| wb.workspace_saving_for_test()));
    cx.run_until_parked();
}

const MONO: &str = r#"// Team workspace: keep the comments.
{
    "folders": [
        { "path": "api", "name": "Backend", "x-owner": "ada" }, // the service
        /* the site */ { "path": "web" },
    ],
    "settings": { "editor.tabSize": 3 },
    "extensions": { "recommendations": ["one.two"] },
    "unknownSection": [1, 2, 3],
}
"#;

/// Two folders, the file one level up.
fn mono(name: &str) -> (PathBuf, PathBuf) {
    let base = dir(name);
    for d in ["api", "web"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
        std::fs::write(base.join(d).join("f.txt"), "x\n").unwrap();
    }
    let file = base.join("mono.code-workspace");
    std::fs::write(&file, MONO).unwrap();
    (base, file)
}

// --- Settings: precedence and provenance (no window) ---------------------------------------------------------

/// AC2: user < workspace file < folder, each key reports the layer it came from; restricted keys need trust per layer.
#[test]
fn settings_follow_user_workspace_folder_precedence_with_sources() {
    let user = r#"{ "editor.tabSize": 2, "editor.fontSize": 15, "files.insertFinalNewline": true }"#;
    let workspace = r#"{ "editor.tabSize": 4, "editor.formatOnSave": true, "terminal.shell": "/bin/ws-shell" }"#;
    let folder = r#"{ "editor.tabSize": 8 }"#;
    let layers = [
        ScopedLayer { name: "mono.code-workspace", text: workspace, warn: false, trusted: false, scope: Scope::Workspace },
        ScopedLayer { name: ".codeyard/settings.jsonc", text: folder, warn: true, trusted: true, scope: Scope::Folder },
    ];
    let (s, warnings, sources) = parse_scoped(user, &layers).unwrap();
    assert_eq!(s.tab_size, Some(8), "the folder wins");
    assert!(s.format_on_save, "the workspace file sets what the folder leaves alone");
    assert_eq!(s.editor_font_size, 15., "the user file sets the rest");
    assert_eq!(sources.of("editor.tabSize"), Scope::Folder);
    assert_eq!(sources.of("editor.formatOnSave"), Scope::Workspace);
    assert_eq!(sources.of("editor.fontSize"), Scope::User);
    assert_eq!(sources.of("files.insertFinalNewline"), Scope::User);
    assert_eq!(s.terminal_shell, None, "an untrusted workspace file cannot choose a program");
    assert!(warnings.iter().any(|w| w.contains("mono.code-workspace") && w.contains("terminal.shell")), "{warnings:?}");
    assert_eq!(sources.of("terminal.shell"), Scope::User, "an ignored key has no workspace source");

    let trusted = [ScopedLayer { trusted: true, ..layers[0] }];
    let (s, _, _) = parse_scoped(user, &trusted).unwrap();
    assert_eq!(s.terminal_shell.as_deref(), Some("/bin/ws-shell"));
    // User-only keys never come from a workspace file, trusted or not.
    let gate = [ScopedLayer { text: r#"{ "security.workspaceTrust.enabled": false }"#, ..trusted[0] }];
    let (s, w, sources) = parse_scoped(r#"{ "security.workspaceTrust.enabled": true }"#, &gate).unwrap();
    assert!(s.workspace_trust && w.iter().any(|w| w.contains("can only be set in your own settings")));
    assert_eq!(sources.of("security.workspaceTrust.enabled"), Scope::User);
}

/// AC2 for additional roots: user < workspace < that folder, and the first folder's own values never leak in.
#[test]
fn additional_roots_resolve_their_own_precedence() {
    let user = r#"{ "editor.formatOnSave": true, "editor.rulers": [100] }"#;
    let workspace = r#"{ "editor.tabSize": 4, "files.trimTrailingWhitespace": true }"#;
    let folder = r#"{ "editor.tabSize": 2 }"#;
    let f = FolderSettings::resolve(user, Some(workspace), &[folder]);
    assert_eq!(f.tab_size, Some(2), "folder over workspace");
    assert_eq!(f.trim_trailing_whitespace, Some(true), "workspace over user/default");
    assert_eq!(f.format_on_save, Some(true), "user over default");
    assert_eq!(f.rulers, Some(vec![100]));
    assert_eq!(f.insert_final_newline, Some(false), "default, not whatever the first folder says");
    assert!(f.isolated);
    let bare = FolderSettings::resolve("{}", None, &[]);
    assert_eq!((bare.tab_size, bare.insert_spaces), (None, None), "unset indentation is guessed from the file");
    assert_eq!(bare.format_on_save, Some(false));
    // A broken layer is skipped, the rest still applies.
    let broken = FolderSettings::resolve("{ broken", Some(workspace), &["[1]"]);
    assert_eq!(broken.tab_size, Some(4));
}

// --- GPUI ---------------------------------------------------------------------------------------------------

/// AC1: names show in the Explorer, the shared settings apply, and saving without a change leaves the file byte for byte.
/// Adding and removing a folder edits only the folder list.
#[gpui::test]
fn workspace_file_opens_named_folders_and_saves_without_loss(cx: &mut TestAppContext) {
    let (base, file) = mono("named");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    wait(cx, "both folders open", |cx| labels(&wb, cx) == ["Backend", "web"]);
    assert_eq!(wb.read_with(cx, |wb, cx| wb.all_roots(cx)), [s(&base.join("api")), s(&base.join("web"))]);
    assert_eq!(cx.update(|_, cx| cx.global::<Settings>().tab_size), Some(3), "the shared settings apply");
    assert_eq!(cx.update(|_, cx| cx.global::<crate::settings::Sources>().of("editor.tabSize")), Scope::Workspace);
    assert_eq!(
        cx.update(|_, cx| crate::settings_page::source_note("editor.tabSize", cx)),
        Some("Set by the workspace file"),
        "the Settings page says where it comes from"
    );
    assert_eq!(cx.update(|_, cx| crate::settings_page::source_note("editor.fontSize", cx)), None);
    let modified = std::fs::metadata(&file).unwrap().modified().unwrap();

    wb.update(cx, |wb, cx| wb.save_workspace_for_test(cx));
    settle(&wb, cx);
    assert_eq!(read(&file), MONO, "nothing changed: nothing written");
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), modified);

    std::fs::create_dir_all(base.join("docs")).unwrap();
    let docs = s(&base.join("docs"));
    assert!(wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(docs.clone(), window, cx)));
    settle(&wb, cx);
    let text = read(&file);
    assert!(text.contains(r#"{ "path": "api", "name": "Backend", "x-owner": "ada" }, // the service"#) && text.contains(r#"{ "path": "docs" }"#), "{text}");
    assert!(text.contains(r#"/* the site */ { "path": "web" }"#), "{text}");
    assert!(text.starts_with("// Team workspace: keep the comments.\n{\n    \"folders\": ["), "{text}");
    let tail = &MONO[MONO.find("    \"settings\"").unwrap()..];
    assert!(text.ends_with(tail), "everything after the folder list is untouched:\n{text}");

    wb.update(cx, |wb, cx| wb.remove_workspace_folder(&docs, cx));
    settle(&wb, cx);
    let text = read(&file);
    assert!(!text.contains("docs") && text.contains("\"web\"") && text.ends_with(tail), "{text}");
    assert_eq!(labels(&wb, cx), ["Backend", "web"]);

    // Added and removed again while the first write still runs: the file ends up without it.
    wb.update_in(cx, |wb, window, cx| {
        wb.add_workspace_folder_now(docs.clone(), window, cx);
        assert!(wb.workspace_saving_for_test());
        wb.remove_workspace_folder(&docs, cx);
    });
    settle(&wb, cx);
    wait(cx, "second write", |_| !read(&file).contains("docs"));
    settle(&wb, cx);
    assert!(!read(&file).contains("docs"), "{}", read(&file));
}

/// AC5: a missing folder keeps the workspace open, is listed as unavailable with the localized message, comes back on
/// reload once it exists, and can be removed from the file; a stale removal does nothing.
#[gpui::test]
fn missing_folder_stays_visible_and_repairable(cx: &mut TestAppContext) {
    let base = dir("missing");
    std::fs::create_dir_all(base.join("api")).unwrap();
    let file = base.join("w.code-workspace");
    let text = "{ \"folders\": [ { \"path\": \"api\" }, { \"path\": \"gone\", \"name\": \"Gone ğ\" }, { \"path\": \"also-gone\" } ] }\n";
    std::fs::write(&file, text).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    wait(cx, "opened", |cx| snap(&wb, cx).is_some());
    let (_, names, missing, _) = snap(&wb, cx).unwrap();
    assert_eq!((names, missing.clone()), (vec!["api".to_string()], vec!["Gone ğ".to_string(), "also-gone".to_string()]));
    assert!(notice(&wb, cx).contains("Workspace folder is unavailable: Gone ğ, also-gone"), "{}", notice(&wb, cx));
    assert_eq!(read(&file), text, "opening writes nothing");

    std::fs::create_dir_all(base.join("gone")).unwrap();
    wb.update(cx, |wb, cx| wb.reload_workspace_for_test(cx));
    wait(cx, "reappeared", |cx| labels(&wb, cx) == ["api", "Gone ğ"]);
    assert_eq!(wb.read_with(cx, |wb, _| wb.unavailable_items_for_test()), [2]);

    // A reply for an entry that no longer exists does nothing.
    wb.update(cx, |wb, cx| wb.remove_unavailable_for_test(1, cx));
    settle(&wb, cx);
    assert_eq!(read(&file), text);
    wb.update(cx, |wb, cx| wb.remove_unavailable_for_test(2, cx));
    settle(&wb, cx);
    let saved = read(&file);
    assert!(!saved.contains("also-gone") && saved.contains("\"Gone ğ\""), "{saved}");
    assert!(wb.read_with(cx, |wb, _| wb.unavailable_items_for_test()).is_empty());
}

/// AC6: an external edit before the save is detected and kept; a malformed file is reported, never written, and can be
/// reloaded once repaired.
#[gpui::test]
fn external_edits_and_malformed_files_are_never_overwritten(cx: &mut TestAppContext) {
    let (base, file) = mono("conflict");
    std::fs::create_dir_all(base.join("docs")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    wait(cx, "open", |cx| labels(&wb, cx).len() == 2);

    // Someone else edits the file, then this window adds a folder before reading it again.
    let theirs = MONO.replace("\"web\" }", "\"web\", \"name\": \"Site\" }");
    let docs = s(&base.join("docs"));
    // Same turn: the watcher cannot reload the file before the save starts.
    let (f, t) = (file.clone(), theirs.clone());
    wb.update_in(cx, |wb, window, cx| {
        std::fs::write(&f, &t).unwrap();
        wb.add_workspace_folder_now(docs.clone(), window, cx)
    });
    settle(&wb, cx);
    wait(cx, "conflict reported", |cx| notice(&wb, cx).contains("Workspace changed on disk. Reload before saving."));
    assert_eq!(read(&file), theirs, "their edit is kept");

    // Reload picks it up (the external name shows, the extra folder this window added is dropped: the file decides).
    wb.update(cx, |wb, cx| wb.reload_workspace_for_test(cx));
    wait(cx, "reloaded", |cx| labels(&wb, cx) == ["Backend", "Site"]);

    // A broken file: an error, the window stays as it was, and nothing is written.
    let broken = "{ \"folders\": [ { \"path\": \"api\" }, /* cut off";
    std::fs::write(&file, broken).unwrap();
    wb.update(cx, |wb, cx| wb.reload_workspace_for_test(cx));
    wait(cx, "error", |cx| notice(&wb, cx).starts_with("Workspace file could not be read: "));
    assert_eq!(labels(&wb, cx), ["Backend", "Site"]);
    let web = s(&base.join("web"));
    wb.update(cx, |wb, cx| wb.remove_workspace_folder(&web, cx));
    settle(&wb, cx);
    assert_eq!(read(&file), broken, "a file that cannot be read is never replaced");
    assert!(notice(&wb, cx).starts_with("Workspace file could not be read: "), "{}", notice(&wb, cx));
    wb.update(cx, |wb, cx| wb.save_workspace_as_for_test(s(&base.join("copy.code-workspace")), cx));
    settle(&wb, cx);
    assert!(!base.join("copy.code-workspace").exists(), "nor saved elsewhere from a broken model");

    // Repaired: it reads again.
    std::fs::write(&file, MONO).unwrap();
    wb.update(cx, |wb, cx| wb.reload_workspace_for_test(cx));
    wait(cx, "repaired", |cx| snap(&wb, cx).is_some_and(|s| s.3.is_none()) && labels(&wb, cx) == ["Backend", "web"]);

    // An unreadable file never opens as an empty workspace either.
    for bad in ["", "{", "[]", "\u{0}", "{ \"folders\": 7 }"] {
        let f = base.join("bad.code-workspace");
        std::fs::write(&f, bad).unwrap();
        open(&wb, &f, cx);
        wait(cx, "refused", |cx| notice(&wb, cx).starts_with("Workspace file could not be read: "));
        assert_eq!(read(&f), bad);
        wb.update(cx, |wb, _| wb.clear_notice_for_test());
    }
}

/// An edit racing the save: whichever lands first, the other side's edit is never lost silently.
#[gpui::test]
fn an_external_edit_racing_a_save_is_never_lost(cx: &mut TestAppContext) {
    for round in 0..4 {
        let (base, file) = mono(&format!("race-{round}"));
        std::fs::create_dir_all(base.join("docs")).unwrap();
        let (wb, cx) = setup(cx);
        open(&wb, &file, cx);
        wait(cx, "open", |cx| labels(&wb, cx).len() == 2);
        let theirs = format!("{MONO}// edited elsewhere {round}\n");
        let docs = s(&base.join("docs"));
        let f = file.clone();
        let t = theirs.clone();
        wb.update_in(cx, |wb, window, cx| {
            wb.add_workspace_folder_now(docs.clone(), window, cx);
            std::thread::sleep(Duration::from_millis(round * 3));
            std::fs::write(&f, &t).unwrap();
        });
        settle(&wb, cx);
        let on_disk = read(&file);
        // Either their write came last (and ours was refused or overwritten by theirs), or ours was refused: never a mix.
        assert_eq!(on_disk, theirs, "round {round}");
        cx.update(|window, _| window.remove_window());
    }
}

/// A write that fails part way (read-only folder) leaves the original intact and says so.
#[cfg(unix)]
#[gpui::test]
fn a_failed_write_leaves_the_original_file_intact(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let (base, _) = mono("readonly");
    let ro = base.join("locked");
    std::fs::create_dir_all(&ro).unwrap();
    let file = ro.join("mono.code-workspace");
    std::fs::write(&file, MONO.replace("\"api\"", "\"../api\"").replace("\"web\"", "\"../web\"")).unwrap();
    let original = read(&file);
    std::fs::create_dir_all(base.join("docs")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    wait(cx, "open", |cx| labels(&wb, cx).len() == 2);
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let docs = s(&base.join("docs"));
    wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(docs, window, cx));
    settle(&wb, cx);
    let message = notice(&wb, cx);
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(message.starts_with("Could not save the workspace file: "), "{message}");
    assert_eq!(read(&file), original, "the original is intact");
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 1, "no temporary file is left behind");
}

/// AC4: Save As into another folder keeps every folder reachable and keeps comments; the window follows the new file.
#[gpui::test]
fn save_as_rebases_relative_paths(cx: &mut TestAppContext) {
    let (base, file) = mono("save-as");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    wait(cx, "open", |cx| labels(&wb, cx).len() == 2);
    let target = base.join("nested/deeper/copy.code-workspace");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    wb.update(cx, |wb, cx| wb.save_workspace_as_for_test(s(&target), cx));
    wait(cx, "saved as", |cx| snap(&wb, cx).is_some_and(|s| s.0 == crate::ui::local_path(&target)));
    let text = read(&target);
    assert!(text.contains(r#""path": "../../api""#) && text.contains(r#""path": "../../web""#), "{text}");
    assert!(text.contains("// the service") && text.contains("unknownSection"), "{text}");
    assert_eq!(read(&file), MONO, "the original is left alone");
    // The copy opens the same folders.
    let doc = crate::workspace_file::Document::parse(&text, &s(target.parent().unwrap())).unwrap();
    assert_eq!(doc.folders().iter().map(|f| f.path().unwrap().to_string()).collect::<Vec<_>>(), [s(&base.join("api")), s(&base.join("web"))]);
    // Later changes go to the new file.
    std::fs::create_dir_all(base.join("docs")).unwrap();
    let docs = s(&base.join("docs"));
    wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(docs, window, cx));
    settle(&wb, cx);
    assert!(read(&target).contains(r#""path": "../../docs""#), "{}", read(&target));
    assert_eq!(read(&file), MONO);
}

/// A window with several folders but no workspace file saves a new one, relative to where it is saved.
#[gpui::test]
fn save_as_without_a_workspace_file_creates_one(cx: &mut TestAppContext) {
    let (base, _) = mono("save-new");
    let (wb, cx) = setup(cx);
    open(&wb, &base.join("api"), cx);
    cx.run_until_parked();
    let web = s(&base.join("web"));
    assert!(wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(web, window, cx)));
    let target = base.join("new.code-workspace");
    wb.update(cx, |wb, cx| wb.save_workspace_as_for_test(s(&target), cx));
    wait(cx, "saved", |cx| snap(&wb, cx).is_some());
    let text = read(&target);
    let doc = crate::workspace_file::Document::parse(&text, &s(&base)).unwrap();
    assert_eq!(
        doc.folders().iter().map(|f| f.source.clone()).collect::<Vec<_>>(),
        [crate::workspace_file::Source::Path("api".into()), crate::workspace_file::Source::Path("web".into())]
    );
    assert_eq!(labels(&wb, cx), ["api", "web"]);
}

/// Multi-root reload: order, names, folders and shared settings follow the file; extra folders resolve
/// user < workspace < folder.
#[gpui::test]
fn reload_follows_the_files_folders_order_and_settings(cx: &mut TestAppContext) {
    let (base, file) = mono("reload");
    std::fs::create_dir_all(base.join("web/.codeyard")).unwrap();
    std::fs::write(base.join("web/.codeyard/settings.jsonc"), r#"{ "editor.tabSize": 5 }"#).unwrap();
    std::fs::create_dir_all(base.join("lib")).unwrap();
    std::fs::write(base.join("lib/f.txt"), "x\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    wait(cx, "open", |cx| labels(&wb, cx).len() == 2);
    let web_file = s(&base.join("web/f.txt"));
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(web_file.clone(), window, cx));
    wait(cx, "folder over workspace", |cx| {
        wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&web_file, cx).is_some_and(|e| e.read(cx).indent_unit_for_test() == 5))
    });

    let next = MONO
        .replace(
            "{ \"path\": \"api\", \"name\": \"Backend\", \"x-owner\": \"ada\" }, // the service\n        /* the site */ { \"path\": \"web\" },",
            "{ \"path\": \"api\" }, { \"path\": \"lib\", \"name\": \"Library\" }, { \"path\": \"web\" },",
        )
        .replace("\"editor.tabSize\": 3", "\"editor.tabSize\": 6");
    assert_ne!(next, MONO);
    std::fs::write(&file, &next).unwrap();
    wb.update(cx, |wb, cx| wb.reload_workspace_for_test(cx));
    wait(cx, "reloaded", |cx| labels(&wb, cx) == ["api", "Library", "web"]);
    assert_eq!(cx.update(|_, cx| cx.global::<Settings>().tab_size), Some(6));
    let lib_file = s(&base.join("lib/f.txt"));
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(lib_file.clone(), window, cx));
    wait(cx, "an extra folder without its own settings takes the workspace's", |cx| {
        wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&lib_file, cx).is_some_and(|e| e.read(cx).indent_unit_for_test() == 6))
    });
    let web_ed = wb.read_with(cx, |wb, cx| wb.editor_for_path_for_test(&web_file, cx)).unwrap();
    assert_eq!(web_ed.read_with(cx, |e, _| e.indent_unit_for_test()), 5, "the folder still wins");
    assert_eq!(read(&file), next, "reloading writes nothing");
}

/// AC3: the workspace file's tasks are found for the active folder and run there, after the folder and the workspace
/// file's own folder are trusted.
#[gpui::test]
fn shared_tasks_run_in_the_active_folder_behind_trust(cx: &mut TestAppContext) {
    let base = dir("tasks");
    std::fs::create_dir_all(base.join("app")).unwrap();
    let file = base.join("w.code-workspace");
    std::fs::write(
        &file,
        r#"{ "folders": [ { "path": "app" } ], "tasks": { "version": "2.0.0", "tasks": [ { "label": "ws build", "command": "touch ran-here.txt" } ] } }"#,
    )
    .unwrap();
    let config = dir("tasks-config");
    std::fs::write(config.join("settings.jsonc"), r#"{ "security.workspaceTrust.enabled": true }"#).unwrap();
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(Settings { workspace_trust: true, lsp_enabled: false, ..Settings::default() });
    });
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(config), window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    open(&wb, &file, cx);
    wait(cx, "open", |cx| labels(&wb, cx) == ["app"]);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));

    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Tasks: Run Build Task", window, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "the folder is asked about first");
    cx.simulate_prompt_answer("Trust Folder and Continue");
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "then the workspace file's folder, which defines the task");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.terminal_count()), 0, "Cancel runs nothing");

    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Tasks: Run Build Task", window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Trust Folder and Continue");
    wait_secs(cx, 15, "the shared task ran in the folder", |cx| {
        cx.executor().advance_clock(Duration::from_millis(300));
        base.join("app/ran-here.txt").exists()
    });
    assert!(!base.join("ran-here.txt").exists(), "not next to the workspace file");
}

/// AC8: after a restart the named workspace, its folder order and names come back from the file.
#[gpui::test]
fn restart_restores_the_named_workspace(cx: &mut TestAppContext) {
    let (base, file) = mono("restart");
    std::fs::create_dir_all(base.join("lib")).unwrap();
    std::fs::write(
        &file,
        MONO.replace(
            "/* the site */ { \"path\": \"web\" },",
            "/* the site */ { \"path\": \"web\" }, { \"path\": \"lib\", \"name\": \"Lib\" }, { \"path\": \"gone\" },",
        ),
    )
    .unwrap();
    let db = base.join("state.db");
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(Settings { lsp_enabled: false, startup: crate::settings::Startup::Continue, ..Default::default() });
    });
    let first_db = db.clone();
    let (wb, vcx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::open(&first_db).ok(), None, window, cx));
    open(&wb, &file, vcx);
    wait(vcx, "open", |cx| labels(&wb, cx) == ["Backend", "web", "Lib"]);
    vcx.update(|window, _| window.remove_window());
    cx.run_until_parked();

    let (wb, vcx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::open(&db).ok(), None, window, cx));
    // Without a settings folder the first window's workspace settings replaced the test's; "continue" again.
    vcx.update(|_, cx| cx.set_global(Settings { lsp_enabled: false, startup: crate::settings::Startup::Continue, ..Default::default() }));
    wb.update_in(vcx, |wb, window, cx| wb.restore_last(window, cx));
    wait(vcx, "restored", |cx| labels(&wb, cx) == ["Backend", "web", "Lib"]);
    let (path, _, missing, error) = snap(&wb, vcx).unwrap();
    assert_eq!((path, missing, error), (s(&file), vec!["gone".to_string()], None));
    assert_eq!(vcx.update(|_, cx| cx.global::<Settings>().tab_size), Some(3), "its settings too");
    assert!(read(&file).contains("{ \"path\": \"gone\" }"), "restoring writes nothing");
}

/// Symlinked folders open; a link to a file or a dangling link is unavailable, never an error.
#[cfg(unix)]
#[gpui::test]
fn symlinked_folders_open_and_broken_links_are_unavailable(cx: &mut TestAppContext) {
    let base = dir("links");
    std::fs::create_dir_all(base.join("real")).unwrap();
    std::fs::write(base.join("file.txt"), "x").unwrap();
    std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();
    std::os::unix::fs::symlink(base.join("file.txt"), base.join("to-file")).unwrap();
    std::os::unix::fs::symlink(base.join("nowhere"), base.join("dangling")).unwrap();
    let file = base.join("w.code-workspace");
    std::fs::write(&file, r#"{ "folders": [ { "path": "link" }, { "path": "to-file" }, { "path": "dangling" }, { "path": "../../../../../../.." } ] }"#)
        .unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    wait(cx, "open", |cx| snap(&wb, cx).is_some());
    assert_eq!(wb.read_with(cx, |wb, cx| wb.all_roots(cx))[0], s(&base.join("real")), "the link opens its target");
    let (_, _, missing, _) = snap(&wb, cx).unwrap();
    assert_eq!(missing, ["to-file", "dangling"]);
}

/// Local and SSH: over SSH the workspace file is read and written on the server through the agent, with the same
/// conflict check.
#[gpui::test]
fn workspace_file_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("ws-file-ssh") else { return };
    let (base, file) = mono("ssh");
    std::fs::create_dir_all(base.join("docs")).unwrap();
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    let tick = |cx: &mut VisualTestContext| cx.executor().advance_clock(Duration::from_secs(1));
    wait_secs(cx, 30, "connected", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    // The user's way: the file open in the editor (on the server), then the palette command.
    open(&wb, &base, cx);
    let ws_path = s(&file);
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(ws_path.clone(), window, cx));
    wait_secs(cx, 30, "file open over ssh", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, cx| wb.active_editor_entity().is_some_and(|e| e.read(cx).path == ws_path))
    });
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("File: Open Active File as Workspace", window, cx));
    wait_secs(cx, 30, "opened over ssh", |cx| {
        tick(cx);
        labels(&wb, cx) == ["Backend", "web"]
    });
    assert!(wb.read_with(cx, |wb, _| wb.workspace_on_remote_for_test()), "read through the SSH host");
    let docs = s(&base.join("docs"));
    wb.update_in(cx, |wb, window, cx| wb.add_workspace_folder_now(docs.clone(), window, cx));
    wait_secs(cx, 30, "written over ssh", |cx| {
        tick(cx);
        read(&file).contains(r#"{ "path": "docs" }"#)
    });
    assert!(read(&file).contains("// the service"));
    // The edit lands before this window has seen it (same turn, so the watcher cannot reload first).
    let f = file.clone();
    wb.update(cx, |wb, cx| {
        std::fs::write(&f, MONO).unwrap();
        wb.remove_workspace_folder(&docs, cx);
    });
    wait_secs(cx, 30, "conflict over ssh", |cx| {
        tick(cx);
        notice(&wb, cx).contains("Workspace changed on disk. Reload before saving.")
    });
    assert_eq!(read(&file), MONO);
}
