//! The Market (#68): the Extensions view's second tab lists and searches the registries' indexes locally, a page shows
//! a registry extension's README, versions, permissions, publisher and downloads, and Install goes through the
//! consent sheet. The registries are folders (`ext_core::testing::TestRegistry`); nothing touches the network.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ext_core::testing::{DirFetch, TestRegistry, github};
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

pub(crate) fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-market-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const THEME: &str = r##"{"name":"x","colors":{"editor.background":"#101418","editor.foreground":"#d0d6dc"}}"##;

/// A theme package with a description, categories and optionally a README.
pub(crate) fn package(id: &str, version: &str, name: &str, description: &str, readme: Option<&str>) -> Vec<u8> {
    let manifest = format!(
        "[extension]\nid = \"{id}\"\nversion = \"{version}\"\nname = \"{name}\"\nlicense = \"MIT\"\ndescription = \"{description}\"\ncategories = [\"themes\"]\n[[themes]]\nlabel = \"{name}\"\npath = \"t.json\"\n"
    );
    let mut entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.into_bytes(), executable: false },
        ext_pkg::Entry { path: "t.json".into(), data: THEME.as_bytes().to_vec(), executable: false },
    ];
    if let Some(r) = readme {
        entries.push(ext_pkg::Entry { path: "README.md".into(), data: r.as_bytes().to_vec(), executable: false });
    }
    ext_pkg::write(entries).unwrap().0
}

pub(crate) const README: &str =
    "# Night Owl\n\nA theme for late hours.\n\n![screenshot](https://images.example.test/night.png)\n\nSee [the site](https://night.example.test).\n";

/// A registry named `name` with three themes, in the user's settings, its requests counted.
pub(crate) fn registry(root: &Path, name: &str, now: u64) -> (TestRegistry, Arc<DirFetch>) {
    let mut r = TestRegistry::new(&root.join(format!("yard-{name}")), name, now);
    r.publish(&package("you.night", "1.0.0", "Night Owl", "A dark theme.", Some(README)), &github(7, 42), now);
    r.publish(&package("you.night", "1.1.0", "Night Owl", "A dark theme.", Some(README)), &github(7, 42), now);
    r.publish(&package("you.dawn", "2.0.0", "Dawn", "A light theme for the morning.", None), &github(7, 42), now);
    r.publish(&package("acme.forest", "0.3.0", "Forest", "Greens and browns.", None), &github(8, 9), now);
    r.edit_release("you.night", "1.0.0", now, |rel| rel.downloads = 40);
    r.edit_release("you.night", "1.1.0", now, |rel| rel.downloads = 2);
    r.edit_release("you.dawn", "2.0.0", now, |rel| rel.downloads = 7);
    let cfg = root.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.jsonc"), serde_json::json!({ "extensions.registries": [r.setting()] }).to_string()).unwrap();
    let fetch = Arc::new(DirFetch::new(&r));
    crate::ext_registry::add_fetch_for_test(fetch.clone());
    (r, fetch)
}

pub(crate) fn setup<'a>(cx: &'a mut TestAppContext, cfg: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
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

pub(crate) fn open_market(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.extensions_tab_for_test(true, window, cx));
    cx.run_until_parked();
}

fn market(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> (Vec<String>, String) {
    wb.read_with(cx, |wb, _| wb.market_for_test())
}

fn search(wb: &Entity<Workbench>, cx: &mut VisualTestContext, q: &str) -> Vec<String> {
    wb.update_in(cx, |wb, _, cx| wb.market_search_for_test(q, cx));
    cx.run_until_parked();
    market(wb, cx).0
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default()
}

/// AC2: opening the Market reads each registry's index at most once a day; searching asks nothing.
#[gpui::test]
fn the_market_lists_and_searches_without_asking_the_registry_again(cx: &mut TestAppContext) {
    let root = dir("list");
    let (_r, fetch) = registry(&root, "list", crate::ext_registry::now());
    let (wb, cx) = setup(cx, &root.join("cfg"));
    assert_eq!(fetch.count(), 0, "the Installed tab asks nothing");
    open_market(&wb, cx);
    let (ids, status) = market(&wb, cx);
    assert_eq!(ids, ["you.night", "you.dawn", "acme.forest"], "most downloaded first");
    assert_eq!(status, "3 extensions");
    let after_open = fetch.count();
    assert_eq!(after_open, 4, "the index, its signature and the root with its signatures: {:?}", fetch.requests.lock().unwrap());
    assert_eq!(search(&wb, cx, "light"), ["you.dawn"], "the description");
    assert_eq!(search(&wb, cx, "THEMES forest"), ["acme.forest"], "a category and the name");
    assert_eq!(search(&wb, cx, "zzz"), Vec::<String>::new());
    assert_eq!(market(&wb, cx).1, "No extension matches the search.");
    // Leaving and opening the Market again the same day reads the cached copy.
    wb.update_in(cx, |wb, window, cx| wb.extensions_tab_for_test(false, window, cx));
    open_market(&wb, cx);
    assert_eq!(fetch.count(), after_open, "searching and reopening asked nothing: {:?}", fetch.requests.lock().unwrap());
}

/// A new window the next day reads the index again, once, with its ETag.
#[gpui::test]
fn a_day_later_the_market_asks_once_more(cx: &mut TestAppContext) {
    let root = dir("daily");
    let now = crate::ext_registry::now();
    let (_r, fetch) = registry(&root, "daily", now);
    let cfg = root.join("cfg");
    let regs = crate::ext_registry::registries(&crate::settings::parse_layers(&std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap(), &[]).unwrap().0);
    // Fetched "yesterday".
    crate::ext_registry::market(&cfg, &regs, now - ext_core::registry::REFRESH_SECS);
    let before = fetch.count();
    let (wb, cx) = setup(cx, &cfg);
    open_market(&wb, cx);
    assert_eq!(market(&wb, cx).0.len(), 3);
    assert_eq!(fetch.count(), before + 1, "one conditional request for the index, unchanged: {:?}", fetch.requests.lock().unwrap());
}

/// AC3 and the page: README without its image (and no request for it), versions with status, permissions, publisher,
/// downloads "including updates"; the details file is fetched once.
#[gpui::test]
fn a_market_page_shows_the_readme_without_images_and_everything_the_index_says(cx: &mut TestAppContext) {
    let root = dir("page");
    let now = crate::ext_registry::now();
    let (mut r, fetch) = registry(&root, "page", now);
    r.set_status("you.night", "1.0.0", ext_core::yard::Status::Yanked, now);
    let (wb, cx) = setup(cx, &root.join("cfg"));
    open_market(&wb, cx);
    let before = fetch.count();
    search(&wb, cx, "night");
    wb.update_in(cx, |wb, window, cx| wb.open_market_row_for_test(0, window, cx));
    cx.run_until_parked();
    // Drawn, so a renderer that loaded images would have asked by now.
    cx.update(|window, cx| window.draw(cx).clear());
    cx.run_until_parked();
    let (title, lines, install, readme) = wb.read_with(cx, |wb, _| wb.market_page_for_test()).expect("the page is in front");
    assert_eq!(title, "Night Owl");
    assert_eq!(install.as_deref(), Some("Install"));
    let readme = readme.unwrap().expect("a README");
    assert!(readme.contains("A theme for late hours.") && readme.contains("the site"), "{readme}");
    let all = lines.join("\n");
    for want in [
        "you.night · version 1.1.0",
        "From the page registry, published by repository 42 of owner 7, .github/workflows/publish.yml",
        "42 downloads, including updates",
        "Permissions: none.",
        "1.1.0 · ",
        "1.0.0 · ",
        "withdrawn by its publisher",
    ] {
        assert!(all.contains(want), "missing {want:?} in\n{all}");
    }
    let asked: Vec<String> = fetch.requests.lock().unwrap()[before..].to_vec();
    assert_eq!(asked.len(), 1, "only the details file: {asked:?}");
    assert!(asked[0].ends_with(".details.json"), "{asked:?}");
    let unrouted = crate::ext_registry::UNROUTED_FOR_TEST.lock().unwrap().clone();
    assert!(!unrouted.iter().any(|u| u.contains("example.test")), "nothing is asked for the README's image or link: {unrouted:?}");
    // Opened again: from the cache.
    wb.update_in(cx, |wb, window, cx| wb.open_market_row_for_test(0, window, cx));
    cx.run_until_parked();
    assert_eq!(fetch.count(), before + 1);
}

/// Install on the page: the consent sheet from this registry, then the lock pins the publisher.
#[gpui::test]
fn installing_from_a_market_page_goes_through_the_consent_sheet(cx: &mut TestAppContext) {
    let root = dir("install");
    let (_r, _fetch) = registry(&root, "install", crate::ext_registry::now());
    let cfg = root.join("cfg");
    let (wb, cx) = setup(cx, &cfg);
    open_market(&wb, cx);
    search(&wb, cx, "dawn");
    wb.update_in(cx, |wb, window, cx| wb.open_market_row_for_test(0, window, cx));
    cx.run_until_parked();
    wb.update_in(cx, |wb, window, cx| wb.install_from_market_for_test(window, cx));
    cx.run_until_parked();
    let (source, warnings) = wb.read_with(cx, |wb, _| wb.consent_source_for_test()).unwrap_or_else(|| panic!("no sheet: {}", notice(&wb, cx)));
    assert!(source.starts_with("From the install registry, published by"), "{source}");
    assert!(warnings.is_empty(), "{warnings:?}");
    wb.update_in(cx, |wb, window, cx| wb.answer_consent_for_test(true, window, cx));
    cx.run_until_parked();
    let locked = crate::ext::store(&cfg).locked().unwrap();
    assert_eq!(locked[&ext_manifest::ExtId::parse("you.dawn").unwrap()].source.registry(), Some(("install", &github(7, 42))));
    // Back in the Market, the row says so and the page offers a reinstall.
    open_market(&wb, cx);
    search(&wb, cx, "dawn");
    wb.update_in(cx, |wb, window, cx| wb.open_market_row_for_test(0, window, cx));
    cx.run_until_parked();
    let (_, lines, install, _) = wb.read_with(cx, |wb, _| wb.market_page_for_test()).unwrap();
    assert_eq!(install.as_deref(), Some("Reinstall"), "{lines:?}");
}

/// AC4, keyboard: the search field takes the keys, ↑ ↓ move the highlighted row, ↵ opens its page.
#[gpui::test]
fn the_market_works_from_the_keyboard(cx: &mut TestAppContext) {
    let root = dir("keys");
    let (_r, _fetch) = registry(&root, "keys", crate::ext_registry::now());
    let (wb, cx) = setup(cx, &root.join("cfg"));
    open_market(&wb, cx);
    cx.simulate_input("theme");
    cx.run_until_parked();
    assert_eq!(market(&wb, cx).0, ["you.night", "you.dawn", "acme.forest"]);
    cx.simulate_keystrokes("down down down up");
    assert_eq!(wb.read_with(cx, |wb, _| wb.market_cursor_for_test()), 1);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (title, ..) = wb.read_with(cx, |wb, _| wb.market_page_for_test()).expect("↵ opened the page");
    assert_eq!(title, "Dawn");
}

/// Without a registry the Market says how to add one; a registry added in the settings is listed when the Market is
/// opened again, and Show Extensions keeps the Market's search field in front.
#[gpui::test]
fn without_a_registry_the_market_explains_and_follows_the_settings(cx: &mut TestAppContext) {
    let root = dir("none");
    let cfg = root.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let (wb, cx) = setup(cx, &cfg);
    open_market(&wb, cx);
    let (ids, status) = market(&wb, cx);
    assert!(ids.is_empty());
    assert!(status.contains("No extension registry is set up yet"), "{status}");
    // `registry` writes the settings file; the reload is what a save in the editor triggers.
    let (_r, _fetch) = registry(&root, "later", crate::ext_registry::now());
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.dispatch_action(crate::workbench::ShowExtensions);
    cx.run_until_parked();
    assert_eq!(market(&wb, cx).0.len(), 3);
    cx.simulate_input("dawn");
    cx.run_until_parked();
    assert_eq!(market(&wb, cx).0, ["you.dawn"], "the keys went to the Market's search");
}
