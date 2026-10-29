//! Installing from a registry (#67): the consent sheet names the publisher, the lock pins it, a new publisher or a
//! stale index is called out, the command line refuses what needs a look, and a user with no extension from a registry
//! makes no request at all. The registry is a folder (`ext_core::testing::TestRegistry`); nothing touches the network.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ext_core::testing::{DirFetch, TestRegistry, github};
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extreg-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const THEME: &str = r##"{"name":"x","colors":{"editor.background":"#101418","editor.foreground":"#d0d6dc"}}"##;

fn package(id: &str, version: &str) -> Vec<u8> {
    let manifest =
        format!("[extension]\nid = \"{id}\"\nversion = \"{version}\"\nname = \"Night\"\nlicense = \"MIT\"\n[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n");
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.into_bytes(), executable: false },
        ext_pkg::Entry { path: "t.json".into(), data: THEME.as_bytes().to_vec(), executable: false },
    ];
    ext_pkg::write(entries).unwrap().0
}

/// A registry named `name` with `you.night` 1.0.0 by `github(7, 42)`, in the user's settings, its requests counted.
fn registry(root: &Path, name: &str, now: u64) -> (TestRegistry, Arc<DirFetch>) {
    let mut r = TestRegistry::new(&root.join("yard"), name, now);
    r.publish(&package("you.night", "1.0.0"), &github(7, 42), now);
    let cfg = root.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.jsonc"), serde_json::json!({ "extensions.registries": [r.setting()] }).to_string()).unwrap();
    let fetch = Arc::new(DirFetch::new(&r));
    crate::ext_registry::add_fetch_for_test(fetch.clone());
    (r, fetch)
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

fn settings_of(cfg: &Path) -> crate::settings::Settings {
    crate::settings::parse_layers(&std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap(), &[]).unwrap().0
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default()
}

#[gpui::test]
fn installing_from_a_registry_names_and_pins_the_publisher(cx: &mut TestAppContext) {
    let root = dir("install");
    let (_r, fetch) = registry(&root, "acme", crate::ext_registry::now());
    let cfg = root.join("cfg");
    let (wb, cx) = setup(cx, &cfg);
    assert_eq!(fetch.count(), 0, "nothing is asked before the user asks for an install");
    wb.update_in(cx, |wb, window, cx| wb.review_registry_extension_for_test("you.night", window, cx));
    cx.run_until_parked();
    let (source, warnings) = wb.read_with(cx, |wb, _| wb.consent_source_for_test()).unwrap_or_else(|| panic!("no sheet: {}", notice(&wb, cx)));
    assert_eq!(source, "From the acme registry, published by repository 42 of owner 7, .github/workflows/publish.yml");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).unwrap().3, "install");
    wb.update_in(cx, |wb, window, cx| wb.answer_consent_for_test(true, window, cx));
    cx.run_until_parked();
    let locked = crate::ext::store(&cfg).locked().unwrap();
    let l = &locked[&ext_manifest::ExtId::parse("you.night").unwrap()];
    assert_eq!(l.source.registry(), Some(("acme", &github(7, 42))));
    assert!(std::fs::read_to_string(cfg.join(crate::ext::LOCK_FILE)).unwrap().contains("\"format\": 2"));
    assert!(wb.read_with(cx, |wb, _| wb.themes_for_test()).iter().any(|(t, _)| t == "ext:you.night/t"));
}

#[gpui::test]
fn a_new_publisher_and_a_stale_index_are_called_out(cx: &mut TestAppContext) {
    let root = dir("changed");
    let now = crate::ext_registry::now();
    let cfg = root.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    // Installed earlier from the "test" registry, pinned to another publisher.
    let entries = vec![
        ext_pkg::Entry {
            path: ext_manifest::FILE_NAME.into(),
            data:
                b"[extension]\nid = \"you.night\"\nversion = \"0.9.0\"\nname = \"Night\"\nlicense = \"MIT\"\n[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n"
                    .to_vec(),
            executable: false,
        },
        ext_pkg::Entry { path: "t.json".into(), data: THEME.as_bytes().to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).unwrap();
    crate::ext::store(&cfg).install(&bytes, ext_core::Source::Registry { registry: "changed".into(), publisher: github(1, 1) }).unwrap();
    // The registry's index was last signed four days ago.
    let (mut r, _fetch) = registry(&root, "changed", now);
    r.sign(now - 4 * 86400);
    let (wb, cx) = setup(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.review_registry_extension_for_test("you.night", window, cx));
    cx.run_until_parked();
    let (_, warnings) = wb.read_with(cx, |wb, _| wb.consent_source_for_test()).unwrap_or_else(|| panic!("no sheet: {}", notice(&wb, cx)));
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].starts_with("Another publisher now: repository 42 of owner 7"), "{warnings:?}");
    assert!(warnings[1].contains("more than 3 days old"), "{warnings:?}");
    assert_eq!(wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).unwrap().3, "cancel", "Enter does not install");
    // The command line refuses what needs that look.
    let regs = crate::ext_registry::registries(&settings_of(&cfg));
    let e = crate::ext_registry::install_from_command_line(&cfg, &regs, "you.night").unwrap_err();
    assert!(e.contains("is now published by"), "{e}");
}

#[test]
fn the_command_line_installs_from_a_registry_and_explains_refusals() {
    let root = dir("cli");
    let now = crate::ext_registry::now();
    let (mut r, _fetch) = registry(&root, "cli", now);
    let cfg = root.join("cfg");
    let regs = crate::ext_registry::registries(&settings_of(&cfg));
    let m = crate::ext_registry::install_from_command_line(&cfg, &regs, "you.night@1.0.0").unwrap();
    assert_eq!((m.id.as_str(), m.version.to_string().as_str()), ("you.night", "1.0.0"));
    let e = crate::ext_registry::install_from_command_line(&cfg, &regs, "you.none").unwrap_err();
    assert!(e.contains("not in any of your registries"), "{e}");
    let e = crate::ext_registry::install_from_command_line(&cfg, &[], "you.night").unwrap_err();
    assert!(e.contains("No extension registry is set up yet"), "{e}");
    // Stale: refused here, the Extensions view asks instead.
    r.publish(&package("you.day", "1.0.0"), &github(7, 42), now);
    r.sign(now - 4 * 86400);
    let e = crate::ext_registry::install_from_command_line(&cfg, &regs, "you.day").unwrap_err();
    assert!(e.contains("more than 3 days old"), "{e}");
}

/// Exit criterion 2 and #67 AC1: with registries configured but nothing installed from them, starting and reloading
/// make no request; with one installed, its registry's index is fetched once and not again that day.
#[gpui::test]
fn no_request_without_an_extension_from_a_registry(cx: &mut TestAppContext) {
    let root = dir("quiet");
    let (_r, fetch) = registry(&root, "quiet", crate::ext_registry::now());
    let cfg = root.join("cfg");
    let (wb, cx) = setup(cx, &cfg);
    for _ in 0..3 {
        wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
        cx.run_until_parked();
    }
    assert_eq!(fetch.count(), 0, "{:?}", fetch.requests.lock().unwrap());
    let regs = crate::ext_registry::registries(&settings_of(&cfg));
    crate::ext_registry::install_from_command_line(&cfg, &regs, "you.night").unwrap();
    let after_install = fetch.count();
    // The cached index is less than a day old: reloads ask nothing more.
    for _ in 0..3 {
        wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
        cx.run_until_parked();
    }
    assert_eq!(fetch.count(), after_install);
    // A day later, one conditional request for the index, and nothing else since it did not change.
    crate::ext_registry::refresh_installed(&cfg, &regs, crate::ext_registry::now() + ext_core::registry::REFRESH_SECS);
    assert_eq!(fetch.count(), after_install + 1);
}

/// The public registry the app ships: once set, it is a valid `extensions.registries` entry over https with the
/// ceremony's three root keys, two of which must sign, so a typo fails here and not on users' machines.
#[test]
fn the_public_registry_entry_is_valid() {
    let Some((name, url, keys, threshold)) = crate::ext_registry::PUBLIC else { return };
    let setting = serde_json::json!({ "name": name, "url": url, "root-keys": keys, "threshold": threshold });
    let r = ext_core::registry::Registry::from_setting(&setting).unwrap();
    assert!(r.url.starts_with("https://"), "{}", r.url);
    assert_eq!((r.keys.keys.len(), r.keys.threshold), (3, 2));
}
