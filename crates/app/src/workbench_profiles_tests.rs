//! Profiles in a real workbench: creating and switching apply settings, keybindings, snippets and theme at once; the
//! files watched are the active profile's; a missing profile falls back; the default profile is as it always was.

use std::path::{Path, PathBuf};

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::super::super::Workbench;
use super::store;

fn config(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("codeyard-wbprofiles-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn setup<'a>(cx: &'a mut TestAppContext, dir: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(crate::ui::ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings::default());
    });
    let dir = dir.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&gpui::Focusable::focus_handle(wb, cx)));
    (wb, cx)
}

fn run(wb: &Entity<Workbench>, label: &str, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    cx.run_until_parked();
}

fn profile(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<String> {
    wb.read_with(cx, |wb, _| wb.active_profile().map(str::to_string))
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default()
}

fn theme_of(cx: &mut VisualTestContext, id: &str) -> bool {
    let want = theme::themes::builtin().into_iter().find(|t| t.id == id).unwrap().palette;
    cx.update(|_, cx| cx.global::<crate::ui::ActiveTheme>().0) == want
}

/// Lets the profile watcher notice a change made on disk. The OS delivers file events on its own
/// schedule (slowest on Windows), so wait until `done` holds instead of assuming one fixed delay.
fn settle(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        cx.executor().advance_clock(std::time::Duration::from_secs(2));
        cx.run_until_parked();
        if done(cx) || std::time::Instant::now() > deadline {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn create(wb: &Entity<Workbench>, name: &str, cx: &mut VisualTestContext) {
    run(wb, "Profiles: Create…", cx);
    cx.simulate_input(name);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
}

fn switch(wb: &Entity<Workbench>, to: &str, cx: &mut VisualTestContext) {
    run(wb, "Profiles: Switch…", cx);
    cx.simulate_input(to);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
}

#[gpui::test]
fn the_default_profile_is_the_config_folder_and_nothing_extra_is_created(cx: &mut TestAppContext) {
    let cfg = config("default");
    write(&cfg.join("settings.jsonc"), "{ \"theme\": \"paper\" }");
    let (wb, cx) = setup(cx, &cfg);
    assert_eq!(profile(&wb, cx), None);
    assert!(theme_of(cx, "paper"));
    cx.executor().advance_clock(std::time::Duration::from_secs(3));
    cx.run_until_parked();
    assert!(!cfg.join("profiles").exists() && !cfg.join("active-profile").exists(), "nothing for profiles exists until one is made");
    assert_eq!(wb.read_with(cx, |wb, _| wb.config_paths_for_test()), Some((cfg.join("settings.jsonc"), cfg.join("keybindings.jsonc"))));
}

#[gpui::test]
fn creating_a_profile_switches_to_its_own_settings_keybindings_and_snippets_at_once(cx: &mut TestAppContext) {
    let cfg = config("switch");
    write(&cfg.join("settings.jsonc"), "{ \"theme\": \"paper\" }");
    write(&cfg.join("keybindings.jsonc"), "[ { \"key\": \"ctrl-alt-shift-1\", \"command\": \"workbench.commandPalette\" } ]");
    write(&cfg.join("snippets/rust.json"), "{ \"d\": { \"prefix\": \"zzdefault\", \"body\": \"x\" } }");
    let (wb, cx) = setup(cx, &cfg);
    assert!(theme_of(cx, "paper"));
    cx.simulate_keystrokes("ctrl-alt-shift-1");
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()), "the default profile's key works");
    cx.simulate_keystrokes("escape");
    assert!(crate::snippets::for_path("a.rs").iter().any(|s| s.prefix == "zzdefault"));

    create(&wb, "Work", cx);
    assert_eq!(profile(&wb, cx).as_deref(), Some("Work"));
    assert!(cfg.join("profiles/Work/settings.jsonc").is_file() && cfg.join("active-profile").is_file());
    assert_eq!(std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap(), "{ \"theme\": \"paper\" }", "the default files are untouched");
    assert!(!theme_of(cx, "paper"), "the new profile has the default theme");
    cx.simulate_keystrokes("ctrl-alt-shift-1");
    assert!(!wb.read_with(cx, |wb, _| wb.palette_open()), "the default profile's key is gone in the new profile");
    assert!(!crate::snippets::for_path("a.rs").iter().any(|s| s.prefix == "zzdefault"));
    assert!(notice(&wb, cx).contains("Work"), "{}", notice(&wb, cx));

    // Edit the profile's own files: they apply, and the default ones do not change.
    write(&cfg.join("profiles/Work/settings.jsonc"), "{ \"theme\": \"ember\" }");
    write(&cfg.join("profiles/Work/keybindings.jsonc"), "[ { \"key\": \"ctrl-alt-shift-2\", \"command\": \"workbench.commandPalette\" } ]");
    write(&cfg.join("profiles/Work/snippets/rust.json"), "{ \"w\": { \"prefix\": \"zzwork\", \"body\": \"y\" } }");
    settle(cx, |cx| theme_of(cx, "ember"));
    assert!(theme_of(cx, "ember"));
    cx.simulate_keystrokes("ctrl-alt-shift-2");
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()));
    cx.simulate_keystrokes("escape");

    // Back to the default profile: its theme, key and snippets are back.
    switch(&wb, "Default", cx);
    assert_eq!(profile(&wb, cx), None);
    assert!(!cfg.join("active-profile").exists());
    assert!(theme_of(cx, "paper"));
    cx.simulate_keystrokes("ctrl-alt-shift-1");
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()));
    cx.simulate_keystrokes("escape");
    cx.simulate_keystrokes("ctrl-alt-shift-2");
    assert!(!wb.read_with(cx, |wb, _| wb.palette_open()));
    assert!(
        crate::snippets::for_path("a.rs").iter().any(|s| s.prefix == "zzdefault") && !crate::snippets::for_path("a.rs").iter().any(|s| s.prefix == "zzwork")
    );

    switch(&wb, "Work", cx);
    assert_eq!(profile(&wb, cx).as_deref(), Some("Work"));
    assert!(theme_of(cx, "ember"));
}

#[gpui::test]
fn saving_a_setting_or_theme_writes_the_active_profile_not_the_default(cx: &mut TestAppContext) {
    let cfg = config("write");
    write(&cfg.join("settings.jsonc"), "{ \"editor.fontSize\": 15 }\n");
    let (wb, cx) = setup(cx, &cfg);
    create(&wb, "Work", cx);
    wb.update_in(cx, |wb, window, cx| wb.write_setting_for_test("editor.minimap.enabled", serde_json::Value::Bool(false), window, cx));
    cx.run_until_parked();
    assert!(std::fs::read_to_string(cfg.join("profiles/Work/settings.jsonc")).unwrap().contains("\"editor.minimap.enabled\": false"));
    assert_eq!(std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap(), "{ \"editor.fontSize\": 15 }\n");
}

#[gpui::test]
fn the_watcher_follows_a_switch_made_by_another_window_or_by_hand(cx: &mut TestAppContext) {
    let cfg = config("watch");
    let (wb, cx) = setup(cx, &cfg);
    store::create(&cfg, "Other").unwrap();
    write(&cfg.join("profiles/Other/settings.jsonc"), "{ \"theme\": \"ember\" }");
    store::set_active(&cfg, Some("Other")).unwrap();
    settle(cx, |cx| theme_of(cx, "ember"));
    assert_eq!(profile(&wb, cx).as_deref(), Some("Other"));
    assert!(theme_of(cx, "ember"));
    store::set_active(&cfg, None).unwrap();
    settle(cx, |cx| profile(&wb, cx).is_none());
    assert_eq!(profile(&wb, cx), None);
}

#[gpui::test]
fn a_profile_that_is_gone_or_not_a_profile_name_falls_back_with_a_notice(cx: &mut TestAppContext) {
    for (name, text) in [("gone", "Gone"), ("traversal", "../Work"), ("device", "nul")] {
        let cfg = config(&format!("fallback-{name}"));
        write(&cfg.join("active-profile"), text);
        let (wb, cx) = setup(cx, &cfg);
        assert_eq!(profile(&wb, cx), None, "{text}");
        assert!(notice(&wb, cx).contains("default profile"), "{text}: {}", notice(&wb, cx));
        assert!(!cfg.join("active-profile").exists(), "the wrong pointer is forgotten");
    }
}

#[gpui::test]
fn a_profile_whose_folder_goes_missing_while_active_falls_back(cx: &mut TestAppContext) {
    let cfg = config("vanish");
    let (wb, cx) = setup(cx, &cfg);
    create(&wb, "Work", cx);
    std::fs::remove_dir_all(cfg.join("profiles/Work")).unwrap();
    settle(cx, |cx| profile(&wb, cx).is_none());
    assert_eq!(profile(&wb, cx), None);
    assert!(notice(&wb, cx).contains("Work"), "{}", notice(&wb, cx));
}

#[gpui::test]
fn bad_names_are_refused_with_a_message_and_create_nothing(cx: &mut TestAppContext) {
    let cfg = config("bad");
    let (wb, cx) = setup(cx, &cfg);
    // The prompt itself does not accept a name with a slash: it stays open and nothing is created.
    for bad in ["../x", "a/b"] {
        create(&wb, bad, cx);
        assert!(!wb.read_with(cx, |wb, _| wb.palette_open()) && !cfg.join("profiles").exists(), "{bad}");
        cx.simulate_keystrokes("escape");
    }
    for (bad, said) in [
        ("nul", "reserved"),
        ("CON", "reserved"),
        ("trailing.", "end with a dot"),
        ("Default", "Default"),
        ("x:y", "cannot contain"),
        (".hid", "start with a dot"),
    ] {
        create(&wb, bad, cx);
        assert_eq!(profile(&wb, cx), None, "{bad}");
        assert!(notice(&wb, cx).contains(said), "{bad}: {}", notice(&wb, cx));
        assert!(!cfg.join("profiles").exists() || store::list(&cfg).is_empty(), "{bad}");
    }
    create(&wb, "Çalışma", cx);
    assert_eq!(profile(&wb, cx).as_deref(), Some("Çalışma"));
    create(&wb, "çalışma", cx);
    assert_eq!(profile(&wb, cx).as_deref(), Some("Çalışma"), "a name that differs only in case is refused");
    assert_eq!(store::list(&cfg), ["Çalışma"]);
}

#[gpui::test]
fn duplicate_copies_what_is_in_use_and_rename_keeps_the_profile_active(cx: &mut TestAppContext) {
    let cfg = config("dup");
    write(&cfg.join("settings.jsonc"), "{ \"theme\": \"paper\" } // mine\n");
    write(&cfg.join("snippets/rust.json"), "{}");
    let (wb, cx) = setup(cx, &cfg);
    run(&wb, "Profiles: Duplicate Current…", cx);
    cx.simulate_input(" Copy");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let name = profile(&wb, cx).expect("the copy is active");
    assert!(name.ends_with("Copy"), "{name}");
    assert_eq!(std::fs::read_to_string(cfg.join("profiles").join(&name).join("settings.jsonc")).unwrap(), "{ \"theme\": \"paper\" } // mine\n");
    assert!(cfg.join("profiles").join(&name).join("snippets/rust.json").is_file());
    assert!(theme_of(cx, "paper"));

    run(&wb, "Profiles: Rename…", cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_input(&format!("{name}-renamed"));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(profile(&wb, cx), Some(format!("{name}-renamed")));
    assert_eq!(store::list(&cfg), [format!("{name}-renamed")]);
    assert_eq!(store::read_active(&cfg).name, profile(&wb, cx));
}

#[gpui::test]
fn a_window_that_lost_its_profile_folder_does_not_bring_it_back_by_writing(cx: &mut TestAppContext) {
    let cfg = config("stale");
    let (wb, cx) = setup(cx, &cfg);
    create(&wb, "Work", cx);
    std::fs::remove_dir_all(cfg.join("profiles/Work")).unwrap();
    // Before the watcher has noticed: a write from this window must not recreate the folder.
    wb.update_in(cx, |wb, window, cx| wb.write_setting_for_test("editor.minimap.enabled", serde_json::Value::Bool(false), window, cx));
    cx.run_until_parked();
    assert!(!cfg.join("profiles/Work").exists());
    cx.executor().advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(profile(&wb, cx), None);
    assert!(!cfg.join("profiles/Work").exists());
}

#[gpui::test]
fn the_shortcuts_page_follows_a_profile_switch(cx: &mut TestAppContext) {
    let cfg = config("page-switch");
    write(&cfg.join("keybindings.jsonc"), "[ { \"key\": \"ctrl-alt-shift-3\", \"command\": \"workbench.commandPalette\" } ]");
    let (wb, cx) = setup(cx, &cfg);
    run(&wb, "Preferences: Keyboard Shortcuts", cx);
    let page = wb.read_with(cx, |wb, _| wb.shortcuts_page_for_test()).unwrap();
    let mine = |cx: &mut VisualTestContext| {
        page.read_with(cx, |p, _| {
            p.rows_for_test().iter().any(|r| {
                r.id.as_deref() == Some("workbench.commandPalette") && r.binding.as_ref().is_some_and(|b| b.source == crate::shortcuts_page::rows::Source::User)
            })
        })
    };
    assert!(mine(cx));
    store::create(&cfg, "Work").unwrap();
    store::set_active(&cfg, Some("Work")).unwrap();
    settle(cx, |cx| profile(&wb, cx).as_deref() == Some("Work"));
    assert_eq!(profile(&wb, cx).as_deref(), Some("Work"));
    assert!(!mine(cx), "the other profile's keybindings show");
}

#[cfg(unix)]
#[gpui::test]
fn a_profile_folder_that_cannot_be_read_keeps_the_pointer_and_comes_back(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let cfg = config("transient");
    store::create(&cfg, "Work").unwrap();
    store::set_active(&cfg, Some("Work")).unwrap();
    let dir = cfg.join("profiles/Work");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&dir).is_ok() {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        return; // running as root: permissions do not apply
    }
    let (wb, cx) = setup(cx, &cfg);
    assert_eq!(profile(&wb, cx), None);
    assert!(cfg.join("active-profile").exists(), "a folder that cannot be read now is not forgotten");
    assert!(notice(&wb, cx).contains("cannot be read"), "{}", notice(&wb, cx));
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    settle(cx, |cx| profile(&wb, cx).as_deref() == Some("Work"));
    assert_eq!(profile(&wb, cx).as_deref(), Some("Work"), "it is back when the folder can be read");
}
