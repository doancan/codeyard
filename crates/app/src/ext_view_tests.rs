//! The Extensions view (#42): the installed list and its search, the detail page, disable / uninstall / roll back,
//! installing a folder through the consent sheet, the High-risk line and `--no-extensions`.

use std::path::{Path, PathBuf};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extview-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn manifest(id: &str, version: &str, rest: &str) -> String {
    format!("[extension]\nid = \"{id}\"\nversion = \"{version}\"\nname = \"Test {id}\"\nlicense = \"MIT\"\ndescription = \"Made for {id}.\"\n{rest}")
}

const THEME: &str = r##"{"name":"x","colors":{"editor.background":"#101418","editor.foreground":"#d0d6dc"}}"##;

/// A theme extension's files: the theme is `ext:<id>/t`.
fn theme_files(id: &str, version: &str) -> Vec<(String, String)> {
    vec![("codeyard-extension.toml".into(), manifest(id, version, "[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n")), ("t.json".into(), THEME.into())]
}

fn install(cfg: &Path, files: &[(String, String)]) -> ext_core::Installed {
    let entries = files.iter().map(|(p, t)| ext_pkg::Entry { path: p.clone(), data: t.as_bytes().to_vec(), executable: false }).collect();
    let (bytes, _) = ext_pkg::write(entries).expect("test package is valid");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "/downloads/test.cyx".into() }).expect("installs")
}

fn setup<'a>(cx: &'a mut TestAppContext, cfg: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let dir = cfg.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| {
        wb.reload_config_for_test(window, cx);
        wb.show_panel_named("extensions", window, cx);
    });
    cx.run_until_parked();
    (wb, cx)
}

fn listed(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<(String, bool)> {
    wb.read_with(cx, |wb, cx| wb.extensions_for_test(cx))
}

fn has_theme(wb: &Entity<Workbench>, cx: &mut VisualTestContext, id: &str) -> bool {
    wb.read_with(cx, |wb, _| wb.themes_for_test()).iter().any(|(t, _)| t == id)
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default()
}

fn write_folder(dir: &Path, files: &[(String, String)]) {
    let _ = std::fs::remove_dir_all(dir);
    for (p, t) in files {
        let path = dir.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, t).unwrap();
    }
}

#[gpui::test]
fn installed_and_disabled_extensions_are_listed_and_searched(cx: &mut TestAppContext) {
    let cfg = config_dir("list");
    install(&cfg, &theme_files("you.night", "1.0.0"));
    install(&cfg, &theme_files("acme.dawn", "2.0.0"));
    crate::ext::store(&cfg).set_disabled(&ext_manifest::ExtId::parse("acme.dawn").unwrap(), true).unwrap();
    let (wb, cx) = setup(cx, &cfg);
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some("extensions"));
    assert_eq!(listed(&wb, cx), [("acme.dawn".to_string(), true), ("you.night".to_string(), false)], "a disabled extension is still listed");
    wb.update(cx, |wb, cx| wb.set_extension_filter_for_test("NIGHT", cx));
    assert_eq!(listed(&wb, cx), [("you.night".to_string(), false)], "search ignores case");
    wb.update(cx, |wb, cx| wb.set_extension_filter_for_test("made for acme", cx));
    assert_eq!(listed(&wb, cx), [("acme.dawn".to_string(), true)], "the description is searched too");
    wb.update(cx, |wb, cx| wb.set_extension_filter_for_test("nothing like it", cx));
    assert!(listed(&wb, cx).is_empty());
}

#[gpui::test]
fn the_detail_page_lists_contributions_and_permissions(cx: &mut TestAppContext) {
    let cfg = config_dir("detail");
    install(
        &cfg,
        &[
            (
                "codeyard-extension.toml".into(),
                manifest(
                    "you.tools",
                    "1.2.0",
                    "[permissions]\nrun = [\"taplo\"]\nwhy = \"Starts taplo for TOML files.\"\n[[language-servers]]\ncommand = \"taplo\"\nlanguages = [\"toml\"]\n[[snippets]]\nlanguage = \"rust\"\npath = \"s.json\"\n[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n",
                ),
            ),
            ("s.json".into(), "{}".into()),
            ("t.json".into(), THEME.into()),
        ],
    );
    let (wb, cx) = setup(cx, &cfg);
    wb.update(cx, |wb, cx| wb.select_extension_for_test("you.tools", cx));
    cx.run_until_parked();
    let (id, version, contributions, permissions, previous) = wb.read_with(cx, |wb, _| wb.extension_detail_for_test()).unwrap();
    assert_eq!((id.as_str(), version.as_str(), previous), ("you.tools", "1.2.0", None));
    assert_eq!(contributions, ["Color theme: Night", "Snippets for rust", "Language server: taplo for toml"]);
    assert_eq!(permissions, ["Run taplo with any arguments"]);
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("extension", true)));
    assert!(cx.debug_bounds("ext-high-risk").is_some(), "running a program is High risk and the page says so");
}

#[gpui::test]
fn disabling_and_enabling_updates_the_contributions(cx: &mut TestAppContext) {
    let cfg = config_dir("toggle");
    install(&cfg, &theme_files("you.night", "1.0.0"));
    let (wb, cx) = setup(cx, &cfg);
    assert!(has_theme(&wb, cx, "ext:you.night/t"));
    wb.update_in(cx, |wb, window, cx| wb.set_extension_enabled_for_test("you.night", false, window, cx));
    assert!(!has_theme(&wb, cx, "ext:you.night/t"), "a disabled extension's theme is gone at once");
    assert_eq!(listed(&wb, cx), [("you.night".to_string(), true)]);
    wb.update_in(cx, |wb, window, cx| wb.set_extension_enabled_for_test("you.night", true, window, cx));
    assert!(has_theme(&wb, cx, "ext:you.night/t"));
    assert_eq!(listed(&wb, cx), [("you.night".to_string(), false)]);
}

#[gpui::test]
fn uninstall_asks_first_and_removes_it(cx: &mut TestAppContext) {
    let cfg = config_dir("uninstall");
    install(&cfg, &theme_files("you.night", "1.0.0"));
    let (wb, cx) = setup(cx, &cfg);
    wb.update(cx, |wb, cx| wb.select_extension_for_test("you.night", cx));
    wb.update_in(cx, |wb, window, cx| wb.uninstall_extension_for_test("you.night", window, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "uninstall is confirmed first");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(listed(&wb, cx).len(), 1, "Cancel keeps it");
    wb.update_in(cx, |wb, window, cx| wb.uninstall_extension_for_test("you.night", window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Uninstall");
    cx.run_until_parked();
    assert!(listed(&wb, cx).is_empty());
    assert!(!has_theme(&wb, cx, "ext:you.night/t"));
    assert!(crate::ext::store(&cfg).locked().unwrap().is_empty());
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), None, "its page closes");
}

#[gpui::test]
fn roll_back_restores_the_previous_version(cx: &mut TestAppContext) {
    let cfg = config_dir("rollback");
    install(&cfg, &theme_files("you.night", "1.0.0"));
    install(&cfg, &theme_files("you.night", "1.1.0"));
    let (wb, cx) = setup(cx, &cfg);
    wb.update(cx, |wb, cx| wb.select_extension_for_test("you.night", cx));
    let (_, version, _, _, previous) = wb.read_with(cx, |wb, _| wb.extension_detail_for_test()).unwrap();
    assert_eq!((version.as_str(), previous.as_deref()), ("1.1.0", Some("1.0.0")));
    wb.update_in(cx, |wb, window, cx| wb.roll_back_extension_for_test("you.night", window, cx));
    let (_, version, _, _, _) = wb.read_with(cx, |wb, _| wb.extension_detail_for_test()).unwrap();
    assert_eq!(version, "1.0.0");
    assert!(notice(&wb, cx).contains("back at version 1.0.0"), "{}", notice(&wb, cx));
    assert!(has_theme(&wb, cx, "ext:you.night/t"));
    wb.update(cx, |wb, cx| wb.verify_extension_for_test("you.night", cx));
    assert!(notice(&wb, cx).contains("matches its package"), "{}", notice(&wb, cx));
}

#[gpui::test]
fn installing_a_folder_goes_through_the_consent_sheet(cx: &mut TestAppContext) {
    let cfg = config_dir("folder");
    let src = cfg.join("src-ext");
    write_folder(&src, &theme_files("you.folder", "0.1.0"));
    let (wb, cx) = setup(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.review_extension_for_test(&src.to_string_lossy(), false, window, cx));
    cx.run_until_parked();
    let (id, permissions, risk, focus) = wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).expect("the sheet is shown");
    assert_eq!((id.as_str(), permissions.is_empty(), risk, focus), ("you.folder", true, None, "install"));
    assert!(listed(&wb, cx).is_empty(), "nothing is installed before the answer");
    wb.update_in(cx, |wb, window, cx| wb.answer_consent_for_test(true, window, cx));
    assert_eq!(listed(&wb, cx), [("you.folder".to_string(), false)]);
    assert!(has_theme(&wb, cx, "ext:you.folder/t"), "its theme applies without a restart");
    let locked = crate::ext::store(&cfg).locked().unwrap();
    assert!(matches!(&locked.values().next().unwrap().source, ext_core::Source::Folder { path } if path.ends_with("src-ext")));
    assert_eq!(wb.read_with(cx, |wb, _| wb.extension_detail_for_test()).map(|d| d.0), Some("you.folder".into()), "its page is shown");
}

#[gpui::test]
fn a_high_risk_package_shows_the_red_line_and_install_is_not_the_default(cx: &mut TestAppContext) {
    let cfg = config_dir("risk");
    let src = cfg.join("risky");
    write_folder(
        &src,
        &[(
            "codeyard-extension.toml".into(),
            manifest(
                "you.lsp",
                "1.0.0",
                "[permissions]\nrun = [\"taplo\"]\nwhy = \"Starts taplo.\"\n[[language-servers]]\ncommand = \"taplo\"\nlanguages = [\"toml\"]\n",
            ),
        )],
    );
    let (wb, cx) = setup(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.review_extension_for_test(&src.to_string_lossy(), false, window, cx));
    cx.run_until_parked();
    let (_, permissions, risk, focus) = wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).unwrap();
    assert_eq!(permissions, ["Run taplo with any arguments"]);
    assert_eq!(risk.as_deref(), Some("High risk: this extension can run programs as you."));
    assert_eq!(focus, "cancel", "Enter on the sheet cancels");
    assert!(cx.debug_bounds("ext-high-risk").is_some());
    wb.update_in(cx, |wb, window, cx| wb.answer_consent_for_test(false, window, cx));
    assert!(listed(&wb, cx).is_empty() && wb.read_with(cx, |wb, _| wb.page_for_test()).is_none());
}

#[gpui::test]
fn package_errors_are_shown_as_written(cx: &mut TestAppContext) {
    let cfg = config_dir("bad");
    let src = cfg.join("bad");
    write_folder(&src, &[("codeyard-extension.toml".into(), manifest("you.bad", "1.0.0", "[[themes]]\nlabel = \"Night\"\npath = \"missing.json\"\n"))]);
    let (wb, cx) = setup(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.review_extension_for_test(&src.to_string_lossy(), false, window, cx));
    cx.run_until_parked();
    let (error, text) = wb.read_with(cx, |wb, _| wb.notice_for_test()).unwrap();
    assert!(error && text.contains("'missing.json', which is not in the package"), "{text}");
    assert!(wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).is_none());
}

#[gpui::test]
fn no_extensions_says_so_instead_of_listing(cx: &mut TestAppContext) {
    let cfg = config_dir("off");
    install(&cfg, &theme_files("you.night", "1.0.0"));
    crate::ext::turn_off();
    let (wb, cx) = setup(cx, &cfg);
    assert!(listed(&wb, cx).is_empty());
    assert!(!has_theme(&wb, cx, "ext:you.night/t"));
    assert!(cx.debug_bounds("sb-extensions-off").is_some(), "the status bar says extensions are off");
    let src = cfg.join("src-ext");
    write_folder(&src, &theme_files("you.folder", "0.1.0"));
    wb.update_in(cx, |wb, window, cx| wb.install_extension_from_folder_for_test(window, cx));
    assert!(notice(&wb, cx).contains("--no-extensions"), "{}", notice(&wb, cx));
}

#[test]
fn permission_lines_are_the_consent_sheet_words() {
    let sets = [
        "read = [\"vercel.json\"]\nnet = [\"api.vercel.com\"]",
        "read = [\"**/*.rs\", \".env\"]\nwrite = [\"src/*.rs\"]\nnet = [\"*.github.io\"]\nwhy = \"x\"",
        "write = [\"**\"]\nauth = [\"github\", \"acme\"]\nsecrets = true\nwhy = \"x\"",
        "run = [\"vercel\"]\nclipboard = \"read-write\"\nterminal = true\ncommands = [\"core:workbench.toggleZenMode\"]\nwhy = \"x\"",
        "clipboard = \"read\"",
        "clipboard = \"write\"",
    ];
    for perms in sets {
        let m = ext_manifest::parse(&manifest("you.p", "1.0.0", &format!("[permissions]\n{perms}\n"))).unwrap_or_else(|e| panic!("{perms}: {e:?}"));
        assert_eq!(Workbench::permission_lines_for_test(&m.permissions), m.permissions.describe(), "{perms}");
    }
}

#[gpui::test]
fn escape_cancels_the_consent_sheet(cx: &mut TestAppContext) {
    let cfg = config_dir("escape");
    let src = cfg.join("src-ext");
    write_folder(&src, &theme_files("you.esc", "0.1.0"));
    let (wb, cx) = setup(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.review_extension_for_test(&src.to_string_lossy(), false, window, cx));
    cx.run_until_parked();
    assert!(wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).is_some());
    cx.simulate_keystrokes("escape");
    assert!(wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).is_none());
    assert!(listed(&wb, cx).is_empty());
}

#[gpui::test]
fn a_package_with_code_is_refused_with_the_reason(cx: &mut TestAppContext) {
    let cfg = config_dir("code");
    let src = cfg.join("code");
    write_folder(
        &src,
        &[
            ("codeyard-extension.toml".into(), manifest("you.code", "1.0.0", "[code]\nentry = \"src/main.ts\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n")),
            ("src/main.ts".into(), "export {}".into()),
        ],
    );
    let (wb, cx) = setup(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.review_extension_for_test(&src.to_string_lossy(), false, window, cx));
    cx.run_until_parked();
    let (error, text) = wb.read_with(cx, |wb, _| wb.notice_for_test()).unwrap();
    assert!(error && text.contains("you.code contains code"), "{text}");
    assert!(wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).is_none());
}

#[gpui::test]
fn the_setting_turns_every_extension_off(cx: &mut TestAppContext) {
    let cfg = config_dir("setting-off");
    install(&cfg, &theme_files("you.night", "1.0.0"));
    let (wb, cx) = setup(cx, &cfg);
    assert!(has_theme(&wb, cx, "ext:you.night/t"));
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "extensions.enabled": false }"#).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    assert!(!has_theme(&wb, cx, "ext:you.night/t"));
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "extensions.enabled": true }"#).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    assert!(has_theme(&wb, cx, "ext:you.night/t"));
}

#[test]
fn a_project_cannot_turn_extensions_off_or_pick_a_preset() {
    let layers = [(".codeyard/settings.jsonc", r#"{ "extensions.enabled": false, "keymap.preset": "evil.keys/Trap" }"#, true)];
    let (s, _) = crate::settings::parse_layers_trusted("{}", &layers, true).unwrap();
    assert!(s.extensions_enabled);
    assert!(s.keymap_preset.is_empty());
}
