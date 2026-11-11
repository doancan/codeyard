//! `Cargo.toml` dependency intelligence in the workbench: hints, the Problems entry, Quick Fix, the palette commands, the Add
//! Dependency picker, completion and hover. The registry is a fake; the index lines are real captures (testdata/cargo_deps).

use std::sync::Arc;

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::cargo_project::deps::tests::{CC_INDEX, CRATE_JSON, Fake, SEARCH, SERDE_INDEX, TOKIO_INDEX, VERSION_JSON};
use crate::editor::EditorView;
use crate::qa_integration_tests::{notice, wait_ticking};
use crate::qa_workbench_tests::setup_with_config;
use crate::workbench::Workbench;

impl Workbench {
    fn all_problems_for_test(&self) -> std::collections::BTreeMap<String, Vec<crate::lsp::Diagnostic>> {
        self.all_problems()
    }

    fn overlay_picker_for_test(&self) -> Option<Entity<crate::picker::Picker>> {
        self.overlay.as_ref().map(|o| o.picker.clone())
    }
}

/// The picker's first rows as "label detail".
fn rows(picker: &Entity<crate::picker::Picker>, cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, cx| {
        picker.read(cx).a11y_view(20, window).rows.into_iter().map(|(label, detail)| format!("{label} {}", detail.unwrap_or_default())).collect()
    })
}

const MANIFEST: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0.100\"\ntokio = \"=1.54.0\"\ncc = \"0.0\"\nno-such-crate-zzz = \"1\"\nlocal = { path = \"../local\" }\n";

struct Fixture<'a> {
    wb: Entity<Workbench>,
    cx: &'a mut VisualTestContext,
    fake: Arc<Fake>,
    path: String,
    ed: Entity<EditorView>,
}

fn dir(name: &str) -> std::path::PathBuf {
    crate::qa_lsp_settings_tests::dir(&format!("cargo-deps-{name}"))
}

fn fake_registry() -> Arc<Fake> {
    let fake = Arc::new(Fake::default());
    fake.ok("https://index.crates.io/se/rd/serde", SERDE_INDEX, "\"s\"");
    fake.ok("https://index.crates.io/to/ki/tokio", TOKIO_INDEX, "\"t\"");
    fake.ok("https://index.crates.io/2/cc", CC_INDEX, "\"c\"");
    fake.ok("https://crates.io/api/v1/crates?q=serde&per_page=20", SEARCH, "\"q\"");
    fake.ok("https://crates.io/api/v1/crates/serde?include=", CRATE_JSON, "\"m\"");
    fake.ok("https://crates.io/api/v1/crates/serde/1.0.229", VERSION_JSON, "\"v\"");
    fake
}

/// Open `text` as `Cargo.toml` of a throw-away project with `settings` and the fake registry, and wait for the hints.
fn fixture<'a>(cx: &'a mut TestAppContext, name: &str, text: &str, settings: &str, fake: Arc<Fake>, want_hints: bool) -> Fixture<'a> {
    fixture_with(cx, name, text, settings, fake, want_hints, &[])
}

/// `fixture` with more files in the project folder.
#[allow(clippy::too_many_arguments)]
fn fixture_with<'a>(
    cx: &'a mut TestAppContext,
    name: &str,
    text: &str,
    settings: &str,
    fake: Arc<Fake>,
    want_hints: bool,
    files: &[(&str, &str)],
) -> Fixture<'a> {
    let d = dir(name);
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.jsonc"), settings).unwrap();
    let project = d.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let file = project.join("Cargo.toml");
    std::fs::write(&file, text).unwrap();
    for (name, body) in files {
        std::fs::write(project.join(name), body).unwrap();
    }
    let (wb, cx) = setup_with_config(cx, &cfg);
    wb.update(cx, |wb, _| wb.set_cargo_fetch_for_test(fake.clone()));
    let ed = crate::qa_editor_tests::open(&wb, &file, cx);
    let path = ed.read_with(cx, |e, _| e.path.clone());
    if want_hints {
        wait_ticking(cx, "the hints", |cx| !wb.read_with(cx, |wb, cx| wb.cargo_hints_for_test(&path, cx)).is_empty());
    }
    Fixture { wb, cx, fake, path, ed }
}

impl Fixture<'_> {
    fn hints(&mut self) -> Vec<(usize, String)> {
        let path = self.path.clone();
        self.wb.read_with(self.cx, |wb, cx| wb.cargo_hints_for_test(&path, cx))
    }

    fn text(&mut self) -> String {
        self.ed.read_with(self.cx, |e, _| e.text())
    }

    fn run(&mut self, label: &str) {
        self.wb.update_in(self.cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
        self.cx.run_until_parked();
    }

    fn cursor_on(&mut self, needle: &str) {
        let text = self.text();
        let at = text.find(needle).unwrap();
        let line = text[..at].matches('\n').count();
        let col = text[..at].rsplit('\n').next().unwrap().chars().count() + 1;
        self.ed.update(self.cx, |e, cx| e.set_cursor_line_col_for_test(line, col, cx));
    }
}

#[gpui::test]
fn the_manifest_gets_a_hint_per_registry_dependency_and_a_warning_for_a_yanked_version(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let mut f = fixture(cx, "hints", MANIFEST, "{}", fake_registry(), true);
    wait_ticking(f.cx, "all five lookups", |cx| f.wb.read_with(cx, |wb, cx| wb.cargo_hints_for_test(&f.path, cx)).len() == 4);
    assert_eq!(
        f.hints(),
        [
            (5, "up to date (latest 1.0.229)".to_string()),
            (6, "yanked: 1.54.0".to_string()),
            (7, "update available: 1.11.1".to_string()),
            (8, "not found on crates.io".to_string()),
        ],
        "the path dependency gets no hint"
    );
    // The yanked version is a warning in the Problems panel and on the editor, never an error.
    let problems = f.wb.read_with(f.cx, |wb, _| wb.all_problems_for_test());
    let ours: Vec<_> =
        problems.iter().filter(|(p, _)| p.ends_with("Cargo.toml")).flat_map(|(_, d)| d.iter()).filter(|d| d.source.as_deref() == Some("cargo")).collect();
    assert_eq!(ours.len(), 1, "{problems:?}");
    assert_eq!((ours[0].severity, ours[0].start.0), (2, 6));
    assert!(ours[0].message.contains("1.54.0") && ours[0].message.contains("yanked"));
}

#[gpui::test]
fn hints_are_hidden_once_the_line_count_changes_and_come_back_with_the_next_refresh(cx: &mut TestAppContext) {
    let mut f = fixture(cx, "stale", MANIFEST, "{}", fake_registry(), true);
    f.ed.update(f.cx, |e, cx| e.replace_all_text_for_test(&format!("\n{MANIFEST}"), cx));
    assert!(f.hints().len() <= 4);
    assert!(f.ed.read_with(f.cx, |e, _| e.cargo_hints().is_empty()), "stale hints are not shown on shifted lines");
    wait_ticking(f.cx, "the refresh", |cx| f.ed.read_with(cx, |e, _| !e.cargo_hints().is_empty()));
    assert_eq!(f.hints()[0].0, 6);
}

#[gpui::test]
fn lookups_off_asks_nothing_and_shows_nothing(cx: &mut TestAppContext) {
    let mut f = fixture(cx, "off", MANIFEST, r#"{ "rust.cargo.dependencyLookups": false }"#, fake_registry(), false);
    for _ in 0..5 {
        f.cx.executor().advance_clock(std::time::Duration::from_millis(700));
        f.cx.run_until_parked();
    }
    assert!(f.hints().is_empty());
    assert_eq!(f.fake.count(), 0, "not one request");
    assert!(!f.ed.read_with(f.cx, |e, _| e.has_cargo()));
    // Commands say why they do nothing.
    f.run("Cargo: Update All Dependencies in This File (Compatible)");
    assert!(notice(&f.wb, f.cx).contains("rust.cargo.dependencyLookups"), "{}", notice(&f.wb, f.cx));
}

#[gpui::test]
fn offline_the_hints_say_offline_and_the_editor_stays_usable(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let fake = Arc::new(Fake::default());
    fake.route("https://index.crates.io/se/rd/serde", Err(crate::cargo_project::deps::fetch::FetchError::Offline));
    let mut f = fixture(cx, "offline", "[dependencies]\nserde = \"1\"\n", "{}", fake, true);
    assert_eq!(f.hints(), [(1, "offline".to_string())]);
    f.ed.update(f.cx, |e, cx| e.replace_all_text_for_test("[dependencies]\nserde = \"1\"\n# edited\n", cx));
    assert!(f.text().contains("# edited"));
}

#[gpui::test]
fn only_registry_dependencies_cause_requests(cx: &mut TestAppContext) {
    let text = "[dependencies]\nlocal = { path = \"../x\", version = \"1\" }\ng = { git = \"https://example.com/g\" }\nw = { workspace = true }\nother = { version = \"1\", registry = \"corp\" }\n";
    let mut f = fixture(cx, "skipped", text, "{}", fake_registry(), false);
    for _ in 0..4 {
        f.cx.executor().advance_clock(std::time::Duration::from_millis(700));
        f.cx.run_until_parked();
    }
    assert!(f.hints().is_empty());
    assert_eq!(f.fake.count(), 0);
}

#[gpui::test]
fn quick_fix_offers_the_updates_and_applies_them_in_one_undo_step(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let mut f = fixture(cx, "quickfix", MANIFEST, "{}", fake_registry(), true);
    wait_ticking(f.cx, "all lookups", |cx| f.wb.read_with(cx, |wb, cx| wb.cargo_hints_for_test(&f.path, cx)).len() == 4);
    f.cursor_on("serde");
    f.run("Editor: Quick Fix…");
    let labels = f.wb.read_with(f.cx, |wb, cx| wb.quick_fix_labels_for_test(cx)).expect("a quick-fix list");
    assert!(labels.contains(&"Update “serde” to the latest version, 1.0.229".to_string()), "{labels:?}");
    assert!(labels.contains(&"Remove dependency “serde”".to_string()), "{labels:?}");
    assert!(labels.iter().any(|l| l.starts_with("Update 3 dependencies in this file to the latest versions")), "{labels:?}");
    f.wb.update_in(f.cx, |wb, window, cx| wb.choose_quick_fix_for_test("Update “serde” to the latest version, 1.0.229", window, cx));
    f.cx.run_until_parked();
    assert_eq!(f.text(), MANIFEST.replace("\"1.0.100\"", "\"1.0.229\""));
    f.ed.update_in(f.cx, |e, window, cx| e.undo_edit(window, cx));
    assert_eq!(f.text(), MANIFEST, "one undo restores the file");
}

#[gpui::test]
fn the_palette_updates_every_dependency_compatibly_or_to_the_latest(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let mut f = fixture(cx, "update-all", MANIFEST, "{}", fake_registry(), true);
    wait_ticking(f.cx, "all lookups", |cx| f.wb.read_with(cx, |wb, cx| wb.cargo_hints_for_test(&f.path, cx)).len() == 4);
    f.run("Cargo: Update All Dependencies in This File (Compatible)");
    assert_eq!(f.text(), MANIFEST.replace("\"1.0.100\"", "\"1.0.229\""), "{}", notice(&f.wb, f.cx));
    assert!(notice(&f.wb, f.cx).contains("Updated 1 dependency"), "{}", notice(&f.wb, f.cx));
    f.run("Cargo: Update All Dependencies in This File to Latest");
    let text = f.text();
    assert!(text.contains("cc = \"1.11\"") && text.contains("tokio = \"=1.54.1\""), "{text}");
    assert!(text.contains("no-such-crate-zzz = \"1\"") && text.contains("local = { path = \"../local\" }"), "unknown and local entries stay");
    f.run("Cargo: Update All Dependencies in This File to Latest");
    assert!(notice(&f.wb, f.cx).contains("Nothing to update"), "{}", notice(&f.wb, f.cx));
}

#[gpui::test]
fn remove_dependency_edits_the_text_and_keeps_comments(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let text = "[dependencies]\n# keep\nserde = \"1\" # note\ncc = \"1\"\n";
    let mut f = fixture(cx, "remove", text, "{}", fake_registry(), true);
    f.cursor_on("cc =");
    f.run("Cargo: Remove Dependency at Cursor");
    wait_ticking(f.cx, "the edit", |cx| !f.ed.read_with(cx, |e, _| e.text()).contains("cc ="));
    assert_eq!(f.text(), "[dependencies]\n# keep\nserde = \"1\" # note\n");
    assert!(notice(&f.wb, f.cx).contains("Removed “cc”"), "{}", notice(&f.wb, f.cx));
    f.cursor_on("[dependencies]");
    f.run("Cargo: Remove Dependency at Cursor");
    assert!(notice(&f.wb, f.cx).contains("Put the cursor on a dependency"), "{}", notice(&f.wb, f.cx));
}

#[gpui::test]
fn add_dependency_searches_and_writes_the_chosen_crate(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let text = "[package]\nname = \"demo\"\n\n[dependencies]\ntokio = \"1\"\n\n[dev-dependencies]\ntempfile = \"3\"\n";
    let mut f = fixture(cx, "add", text, "{}", fake_registry(), false);
    f.cursor_on("tokio");
    f.run("Cargo: Add Dependency…");
    let picker = f.wb.read_with(f.cx, |wb, _| wb.overlay_picker_for_test()).expect("the picker");
    picker.update(f.cx, |p, cx| p.set_query("serde".into(), cx));
    wait_ticking(f.cx, "the search", |cx| rows(&picker, cx).iter().any(|r| r.contains("serde-saphyr")));
    let listed = rows(&picker, f.cx);
    assert!(listed[0].starts_with("serde") && listed[0].contains("A generic serialization/deserialization framework"), "{listed:?}");
    f.cx.simulate_keystrokes("enter");
    f.cx.run_until_parked();
    wait_ticking(f.cx, "the edit", |cx| f.ed.read_with(cx, |e, _| e.text()).contains("serde"));
    let out = f.text();
    assert!(out.contains("[dependencies]\nserde = \"1.0.229\"\ntokio = \"1\"\n"), "added in order:\n{out}");
    assert!(notice(&f.wb, f.cx).contains("Added “serde”"), "{}", notice(&f.wb, f.cx));
    // With the cursor in [dev-dependencies] the crate goes there.
    f.cursor_on("tempfile");
    f.run("Cargo: Add Dependency…");
    let picker = f.wb.read_with(f.cx, |wb, _| wb.overlay_picker_for_test()).unwrap();
    picker.update(f.cx, |p, cx| p.set_query("serde".into(), cx));
    wait_ticking(f.cx, "the search again", |cx| rows(&picker, cx).iter().any(|r| r.contains("serde-saphyr")));
    f.cx.simulate_keystrokes("enter");
    wait_ticking(f.cx, "the second edit", |cx| f.ed.read_with(cx, |e, _| e.text()).contains("[dev-dependencies]\nserde"));
}

#[gpui::test]
fn add_dependency_offline_still_offers_the_typed_name_and_says_why_it_cannot_add_it(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let fake = Arc::new(Fake::default());
    fake.route("https://crates.io/api/v1/crates?q=tokio&per_page=20", Err(crate::cargo_project::deps::fetch::FetchError::Offline));
    let mut f = fixture(cx, "add-offline", "[dependencies]\n", "{}", fake, false);
    f.run("Cargo: Add Dependency…");
    let picker = f.wb.read_with(f.cx, |wb, _| wb.overlay_picker_for_test()).unwrap();
    picker.update(f.cx, |p, cx| p.set_query("tokio".into(), cx));
    wait_ticking(f.cx, "the failed search", |cx| rows(&picker, cx).iter().any(|r| r.starts_with("tokio")));
    f.cx.simulate_keystrokes("enter");
    f.cx.run_until_parked();
    wait_ticking(f.cx, "the notice", |cx| notice(&f.wb, cx).contains("Could not add “tokio”"));
    assert_eq!(f.text(), "[dependencies]\n", "nothing was written");
}

#[gpui::test]
fn versions_features_and_crate_names_complete_from_the_registry(cx: &mut TestAppContext) {
    let text = "[dependencies]\nserde = \"\"\ntokio = { version = \"1\", features = [] }\n\n";
    let mut f = fixture(cx, "complete", text, "{}", fake_registry(), true);
    // Versions: newest first, the stable release before the pre-release.
    f.ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(1, 9, cx));
    f.cx.simulate_input("1");
    wait_ticking(f.cx, "version suggestions", |cx| f.ed.read_with(cx, |e, _| e.completion_labels()).is_some_and(|l| !l.is_empty()));
    let labels = f.ed.read_with(f.cx, |e, _| e.completion_labels()).unwrap();
    assert_eq!(labels[0], "1.0.229", "{labels:?}");
    let at = |v: &str| labels.iter().position(|l| l == v);
    assert!(at("1.0.228") < at("0.9.0-rc1"), "releases come before pre-releases: {labels:?}");
    f.cx.simulate_keystrokes("enter");
    f.cx.run_until_parked();
    assert_eq!(f.text().lines().nth(1).unwrap(), "serde = \"1.0.229\"");
    // Features: from the published list.
    f.ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(2, 37, cx));
    f.cx.simulate_input("\"");
    wait_ticking(f.cx, "feature suggestions", |cx| {
        f.ed.read_with(cx, |e, _| e.completion_labels()).is_some_and(|l| l.contains(&"rt-multi-thread".to_string()))
    });
    f.cx.simulate_input("ful");
    wait_ticking(f.cx, "filtered", |cx| f.ed.read_with(cx, |e, _| e.completion_labels()).is_some_and(|l| l.first().is_some_and(|f| f == "full")));
    f.cx.simulate_keystrokes("enter");
    f.cx.run_until_parked();
    assert!(f.text().contains("features = [\"full\"]"), "{}", f.text());
    // Crate names: a search after a pause.
    f.ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(4, 0, cx));
    f.cx.simulate_input("serde");
    wait_ticking(f.cx, "crate suggestions", |cx| f.ed.read_with(cx, |e, _| e.completion_labels()).is_some_and(|l| l.first().is_some_and(|f| f == "serde")));
    f.cx.simulate_keystrokes("enter");
    f.cx.run_until_parked();
    assert!(f.text().lines().last().unwrap_or_default().starts_with("serde = \"1.0.229\"") || f.text().contains("\nserde = \"1.0.229\"\n"), "{}", f.text());
}

#[gpui::test]
fn outside_dependencies_the_document_words_are_still_suggested(cx: &mut TestAppContext) {
    let text = "[package]\nname = \"demonstration\"\nversion = \"0.1.0\"\n\n[features]\nfast = []\n\n[dependencies]\n";
    let f = fixture(cx, "words", text, "{}", fake_registry(), false);
    f.ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(6, 0, cx));
    f.cx.simulate_input("demo");
    wait_ticking(f.cx, "word suggestions", |cx| f.ed.read_with(cx, |e, _| e.completion_labels()).is_some_and(|l| l.contains(&"demonstration".to_string())));
    assert_eq!(f.fake.count(), 0, "no request for a word");
}

#[gpui::test]
fn hover_shows_the_crate_its_licence_the_lock_version_and_links(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let text = "[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\n";
    let fake = fake_registry();
    fake.ok("https://crates.io/api/v1/crates/serde/1.0.229", &VERSION_JSON.replace("1.0.219", "1.0.229"), "\"v\"");
    let lock = "version = 4\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    let mut f = fixture_with(cx, "hover", text, "{}", fake, true, &[("Cargo.lock", lock)]);
    let at = f.text().find("serde").unwrap();
    let hover = f.ed.read_with(f.cx, |e, _| e.cargo_hover_for_test(at + 1)).expect("a hover");
    for expected in [
        "serde 1.0.229",
        "A generic serialization/deserialization framework",
        "License: MIT OR Apache-2.0",
        "Cargo.lock has 1.0.200",
        "https://docs.rs/serde",
        "https://github.com/serde-rs/serde",
    ] {
        assert!(hover.contains(expected), "{expected}\n{hover}");
    }
    let derive = f.text().find("derive").unwrap();
    let feature = f.ed.read_with(f.cx, |e, _| e.cargo_hover_for_test(derive + 1)).unwrap();
    assert_eq!(feature, "feature derive of serde: enables serde_derive");
    assert_eq!(f.ed.read_with(f.cx, |e, _| e.cargo_hover_for_test(0)), None, "nothing on the table header");
}

#[gpui::test]
fn each_hint_is_a_label_with_its_full_sentence_in_the_accessibility_tree(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let f = fixture(cx, "a11y", "[dependencies]\nserde = \"1.0.100\"\ncc = \"0.0\"\n", "{}", fake_registry(), true);
    let wb = f.wb.clone();
    let labels = |cx: &mut VisualTestContext| -> Vec<String> {
        let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
        snap.to_tree().nodes.into_iter().filter(|(_, n)| n.role() == accesskit::Role::Label).filter_map(|(_, n)| n.label().map(str::to_string)).collect()
    };
    wait_ticking(f.cx, "the labels", |cx| labels(cx).iter().any(|l| l.starts_with("serde is up to date")));
    let all = labels(f.cx);
    assert!(all.contains(&"serde is up to date: “1.0.100” allows the latest release, 1.0.229".to_string()), "{all:?}");
    assert!(all.contains(&"cc can be updated: “0.0” does not allow the latest release, 1.11.1".to_string()), "{all:?}");
}

#[gpui::test]
fn the_interface_language_reaches_hints_and_commands(cx: &mut TestAppContext) {
    let _tr = crate::i18n::LangGuard::set("tr");
    let mut f = fixture(cx, "turkish", "[dependencies]\nserde = \"1\"\ncc = \"0.0\"\n", r#"{ "ui.language": "tr" }"#, fake_registry(), true);
    wait_ticking(f.cx, "both lookups", |cx| f.wb.read_with(cx, |wb, cx| wb.cargo_hints_for_test(&f.path, cx)).len() == 2);
    assert_eq!(f.hints(), [(1, "güncel (son sürüm 1.0.229)".to_string()), (2, "güncelleme var: 1.11.1".to_string())]);
}
