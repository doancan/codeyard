//! UI localization tests: table health, fallback, placeholders, `auto` resolution, menus, palette
//! (Turkish display + English search), coverage percentage and cross-checking the table against the source.

use std::collections::BTreeSet;

use gpui::{Menu, MenuItem, TestAppContext};

use crate::i18n::{self, LangGuard, Plural, Status, fill, lookup, parse_table, parse_tables, pick_form, placeholders, resolve, t, tf, tn};
use crate::qa_workbench_tests::{fresh, setup, setup_with_config};
use crate::workbench::Workbench;

const TR: usize = 1;

fn tr_pairs() -> Vec<(&'static str, &'static str)> {
    parse_tables(i18n::LANGUAGES[TR].tables).0
}

/// Every language that has a translation file: `(code, pairs)`. Adding a language makes the coverage tests below apply to it too.
fn tables() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    i18n::LANGUAGES.iter().filter(|l| !l.tables.is_empty()).map(|l| (l.code, parse_tables(l.tables).0)).collect()
}

/// Only fully maintained languages (`Status::Complete`): coverage tests apply only to them, a partial language doesn't fail them.
fn complete_tables() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    i18n::LANGUAGES.iter().filter(|l| l.status == Status::Complete).map(|l| (l.code, parse_tables(l.tables).0)).collect()
}

/// Every menu name and item name (including submenus and system menus), in the current language.
fn menu_labels(menus: &[Menu]) -> Vec<String> {
    fn walk(menu: &Menu, out: &mut Vec<String>) {
        out.push(menu.name.to_string());
        for item in &menu.items {
            match item {
                MenuItem::Separator => {}
                MenuItem::Submenu(m) => walk(m, out),
                MenuItem::SystemMenu(os) => out.push(os.name.to_string()),
                MenuItem::Action { name, .. } => out.push(name.to_string()),
            }
        }
    }
    let mut out = Vec::new();
    menus.iter().for_each(|m| walk(m, &mut out));
    out
}

fn recent(path: &str) -> state::RecentWorkspace {
    state::RecentWorkspace { host: None, path: path.into(), last_opened_at: 1 }
}

/// Every `.rs` file under `dir`, folders (like `lsp/`) included.
pub(crate) fn rs_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(rs_files(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// English keys of the `t("…")`, `tf("…"`, `tr("…")` calls in the source (excluding tests and translation files).
fn source_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for path in rs_files(&dir) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") || name == "i18n.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for needle in ["t(", "tf(", "tr(", "tn("] {
            for (at, _) in text.match_indices(needle) {
                if text[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                // If the call's first argument (may have whitespace and newlines) isn't a string literal, there is no key.
                let rest = text[at + needle.len()..].trim_start();
                let Some(lit) = rest.strip_prefix('"') else { continue };
                let end = lit.find('"').unwrap();
                assert!(!lit[..end].contains('\\'), "{name}: keys must not contain escapes: {}", &lit[..end]);
                if needle == "tn(" {
                    // Plural: the second literal is part of the key too (`tn("{} file", "{} files", n, …)` → "{} file|{} files").
                    let after = lit[end + 1..].trim_start().strip_prefix(',').map(str::trim_start).and_then(|r| r.strip_prefix('"'));
                    let other = after.and_then(|r| r.find('"').map(|e| &r[..e])).unwrap_or_else(|| panic!("{name}: tn() needs two string literals"));
                    keys.insert(format!("{}|{other}", &lit[..end]));
                } else {
                    keys.insert(lit[..end].to_string());
                }
            }
        }
    }
    keys
}

#[test]
fn translation_tables_are_well_formed() {
    for l in i18n::LANGUAGES.iter().filter(|l| !l.tables.is_empty()) {
        let (_, errors) = parse_tables(l.tables);
        assert!(errors.is_empty(), "{}: {errors:?}", l.code);
    }
    assert!(tr_pairs().len() > 400, "{} entries", tr_pairs().len());
    for (code, pairs) in tables() {
        for (key, value) in &pairs {
            assert_eq!(key.trim(), *key, "{code}/{key:?}: no leading or trailing space in keys");
            assert_eq!(value.trim(), *value, "{code}/{key:?}: no leading or trailing space in translations");
            // In a plural entry each form is filled separately and `{}` is the count in each: the per-form check
            // is done by `plural_entries_are_well_formed_in_every_language` (a sequential count would give a wrong result here).
            if key.contains('|') {
                continue;
            }
            // A translation uses a subset of the key's placeholders.
            let allowed: BTreeSet<usize> = placeholders(key).into_iter().collect();
            for p in placeholders(value) {
                assert!(allowed.contains(&p), "{key:?} -> {value:?}: placeholder {p} does not exist in the key");
            }
            // A key with no placeholders must have a translation without any, and vice versa.
            assert_eq!(placeholders(key).is_empty(), placeholders(value).is_empty(), "{code}/{key:?}: placeholders must be translated too");
        }
    }
}

#[test]
fn malformed_and_duplicate_lines_are_reported() {
    let (pairs, errors) = parse_table("# comment\n\nA\tB\nno tab here\nA\tC\n\tempty key\nkey\t\n");
    assert_eq!(pairs.len(), 2, "{pairs:?}");
    assert!(errors.iter().any(|e| e.contains("duplicate key")), "{errors:?}");
    // A line without a TAB and a line without a key are errors; `key<TAB>` (empty translation) means "not translated yet" and is skipped.
    assert_eq!(errors.iter().filter(|e| e.contains("expected")).count(), 2, "{errors:?}");
    let (pairs, errors) = parse_table("done\tfertig\ntodo\t\nspaces\t   \n");
    assert_eq!((pairs, errors.len()), (vec![("done", "fertig")], 0), "untranslated lines are skipped, not errors");
}

#[test]
fn missing_keys_fall_back_to_english() {
    let _tr = LangGuard::set("tr");
    assert_eq!(t("Save"), "Kaydet");
    assert_eq!(t("A string nobody translated"), "A string nobody translated");
    assert_eq!(tf("Nobody translated {} either", &[&7]), "Nobody translated 7 either");
    assert_eq!(lookup(TR, "Save"), Some("Kaydet"));
    assert_eq!(lookup(TR, "A string nobody translated"), None);
    assert_eq!(lookup(0, "Save"), None, "English has no table: the key is the text");
    // An invalid language index falls back to English.
    drop(_tr);
    assert!(!i18n::set(99), "an unknown language index keeps English");
    assert_eq!(t("Save"), "Save");
}

#[test]
fn placeholders_are_filled_in_order_and_by_position() {
    assert_eq!(fill("{} and {}", &[&"a", &2]), "a and 2");
    assert_eq!(fill("{1} before {0}", &[&"a", &"b"]), "b before a");
    assert_eq!(fill("{0} twice {0}", &[&"x"]), "x twice x");
    assert_eq!(fill("missing {} {}", &[&"only"]), "missing only ", "a missing argument fills nothing");
    assert_eq!(fill("no braces", &[&"x"]), "no braces");
    assert_eq!(fill("open { brace {abc} and {", &[&"x"]), "open { brace {abc} and {", "non-placeholder braces stay");
    assert_eq!(fill("ünïcode {} ✓", &[&"İ"]), "ünïcode İ ✓");
    assert_eq!(tf("Delete branch {}?", &[&"feature"]), "Delete branch feature?");
    {
        let _tr = LangGuard::set("tr");
        assert_eq!(tf("Delete branch {}?", &[&"feature"]), "feature dalı silinsin mi?");
        assert_eq!(tf("Ln {}, Col {}", &[&3, &9]), "Sat 3, Süt 9");
        // The plural suffix exists only in the English key; Turkish uses `{0}`.
        let one = tf("{0} commit{1} only exist on this branch and will be lost (the reflog keeps them for a while).", &[&1, &""]);
        assert!(one.starts_with("1 commit yalnızca"), "{one}");
    }
    let many = tf("{0} commit{1} only exist on this branch and will be lost (the reflog keeps them for a while).", &[&3, &"s"]);
    assert!(many.starts_with("3 commits only exist"), "{many}");
}

#[test]
fn auto_resolves_from_the_injected_locales() {
    let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(resolve("auto", &l(&["tr_TR.UTF-8"])), TR);
    assert_eq!(resolve("auto", &l(&["tr-TR"])), TR);
    assert_eq!(resolve("auto", &l(&["TR_tr@euro"])), TR);
    assert_eq!(resolve("auto", &l(&["tr"])), TR);
    assert_eq!(resolve("auto", &l(&["en_US.UTF-8"])), 0);
    assert_eq!(resolve("auto", &l(&["C"])), 0);
    assert_eq!(resolve("auto", &l(&["POSIX"])), 0);
    assert_eq!(resolve("auto", &l(&[])), 0, "no information: English");
    assert_eq!(resolve("auto", &l(&["de_DE", "fr"])), 0, "no supported language: English");
    assert_eq!(resolve("auto", &l(&["de-DE", "tr-TR", "en-US"])), TR, "the first supported language of the list wins");
    assert_eq!(resolve("auto", &l(&["en-US", "tr-TR"])), 0, "...in the system's order of preference");
    assert_eq!(resolve("auto", &l(&["trx_TR"])), 0, "only the exact language code matches");
    assert_eq!(resolve("tr", &l(&["en_US"])), TR, "an explicit setting beats the system language");
    assert_eq!(resolve("en", &l(&["tr_TR"])), 0);
    assert_eq!(resolve("klingon", &l(&["tr_TR"])), TR, "an unknown value behaves like auto");
    // The system language isn't read in tests: `auto` is English, independent of the machine.
    assert!(i18n::system_locales().is_empty());
    assert!(!i18n::apply_setting("auto"));
    assert_eq!(i18n::current(), 0);
}

#[test]
fn setting_choices_match_the_language_table() {
    let mut expected = vec!["auto"];
    expected.extend(i18n::LANGUAGES.iter().map(|l| l.code));
    assert_eq!(i18n::SETTING_CHOICES, expected.as_slice());
    assert_eq!(i18n::LANGUAGES[0].code, "en", "index 0 is the language of the keys");
    assert!(i18n::LANGUAGES[0].tables.is_empty());
    assert_eq!(i18n::LANGUAGES[0].status, Status::Source);
    assert!(i18n::LANGUAGES[1..].iter().all(|l| l.status != Status::Source && !l.tables.is_empty()), "every other language has a table");
}

#[test]
fn ui_language_setting_parses_and_is_user_only() {
    use crate::settings::{parse_layers, parse_layers_trusted};
    assert_eq!(crate::settings::Settings::default().ui_language, "en", "tests are hermetic: English regardless of the machine");
    for v in ["auto", "en", "tr"] {
        let (s, w) = parse_layers(&format!(r#"{{ "ui.language": "{v}" }}"#), &[]).unwrap();
        assert_eq!((s.ui_language.as_str(), w.len()), (v, 0), "{w:?}");
    }
    let (s, w) = parse_layers(r#"{ "ui.language": "klingon" }"#, &[]).unwrap();
    assert_eq!(s.ui_language, "en", "an invalid value keeps the default");
    assert!(w.iter().any(|m| m.contains("ui.language")), "{w:?}");
    // A folder can't change the language (even if trusted).
    let project = [(".codeyard/settings.jsonc", r#"{ "ui.language": "tr" }"#, true)];
    for trusted in [false, true] {
        let (s, w) = parse_layers_trusted("{}", &project, trusted).unwrap();
        assert_eq!(s.ui_language, "en", "trusted={trusted}");
        assert!(w.iter().any(|m| m.contains("can only be set in your own settings.jsonc")), "{w:?}");
    }
    let (s, _) = parse_layers_trusted(r#"{ "ui.language": "tr" }"#, &project, true).unwrap();
    assert_eq!(s.ui_language, "tr", "the user's own value stands");
    assert!(crate::workbench::settings_ui::entry("ui.language").is_some(), "listed in the settings catalog");
}

#[test]
fn menus_follow_the_language() {
    let recents = [recent("/home/u/proje")];
    for (lang, file, edit, view, source_control, quit, more) in
        [("en", "File", "Edit", "View", "Source Control", "Exit", "More…"), ("tr", "Dosya", "Düzenle", "Görünüm", "Kaynak Denetimi", "Çıkış", "Diğerleri…")]
    {
        let _g = LangGuard::set(lang);
        let desktop = menu_labels(&crate::menus::desktop_menus(&recents));
        let native = menu_labels(&crate::menus::menus(&recents));
        let dock = menu_labels(&[Menu { name: "dock".into(), items: crate::menus::dock_items(&recents) }]);
        for (what, labels) in [("desktop", &desktop), ("native", &native)] {
            for want in [file, edit, view, source_control] {
                assert!(labels.iter().any(|l| l == want), "{lang}/{what}: {want} missing in {labels:?}");
            }
            assert!(labels.iter().any(|l| l == more), "{lang}/{what}: {more}");
        }
        assert!(desktop.iter().any(|l| l == quit), "{lang}: {quit}");
        assert!(dock.iter().any(|l| l == if lang == "tr" { "Yeni Pencere" } else { "New Window" }), "{lang}: {dock:?}");
        // Recent items aren't translated (folder name).
        assert!(native.iter().any(|l| l == "proje"), "{native:?}");
        if lang == "tr" {
            assert!(!desktop.iter().any(|l| l == "File" || l == "Edit"), "{desktop:?}");
        }
    }
    // The placeholder row is translated too when there is no recent folder.
    let _g = LangGuard::set("tr");
    assert!(menu_labels(&crate::menus::menus(&[])).iter().any(|l| l == "Hoş Geldiniz (henüz son klasör yok)"));
}

/// Every English label of the palette and menus is in the Turkish table (coverage 100%).
#[gpui::test]
fn palette_labels_are_fully_translated(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    // Server aliases come from the environment (~/.ssh/config): "Remote: Connect to {}" is a single key.
    let labels: Vec<String> = wb
        .read_with(cx, |wb, _| wb.command_labels_for_test())
        .into_iter()
        .map(|l| if l.starts_with("Remote: Connect to ") && !l.ends_with('…') { "Remote: Connect to {}".into() } else { l })
        .collect();
    for (code, pairs) in complete_tables() {
        let table: BTreeSet<&str> = pairs.into_iter().map(|(k, _)| k).collect();
        let missing: Vec<&String> = labels.iter().filter(|l| !table.contains(l.as_str())).collect();
        let pct = 100.0 * (labels.len() - missing.len()) as f64 / labels.len() as f64;
        eprintln!("{code}: palette labels translated: {}/{} = {pct:.1}%", labels.len() - missing.len(), labels.len());
        assert!(missing.is_empty(), "{code}: palette labels without an entry ({pct:.1}%): {missing:?}");
        assert_eq!(pct, 100.0);
    }
    // A translation can't be identical to the English (it would count as untranslated); only known Git and tool terms intentionally stay the same.
    const KEPT: &[&str] = &["Terminal", "Rebase", "Git: Pull", "Git: Push", "Git: Fetch", "Git: Stash", "Git: Commit (Amend)"];
    let same: Vec<&str> = tr_pairs().into_iter().filter(|(k, v)| k == v && !KEPT.contains(k)).map(|(k, _)| k).collect();
    assert!(same.is_empty(), "entries identical to English (untranslated?): {same:?}");
}

/// Every English text in the UI: palette labels, menus, tooltips and the `t()` keys in the source.
fn known_english_keys(cx: &mut TestAppContext) -> BTreeSet<String> {
    let (wb, cx) = setup(cx);
    let mut known: BTreeSet<String> = wb.read_with(cx, |wb, _| wb.command_labels_for_test()).into_iter().collect();
    // Server aliases come from ~/.ssh/config: a single machine-independent key.
    known.retain(|l| !(l.starts_with("Remote: Connect to ") && !l.ends_with('…')));
    known.insert("Remote: Connect to {}".into());
    known.insert("Remote: Disconnect".into());
    known.extend(Workbench::tips_for_test().iter().map(|s| s.to_string()));
    let recents = [recent("/x/y")];
    known.extend(menu_labels(&crate::menus::menus(&recents)));
    known.extend(menu_labels(&crate::menus::menus(&[])));
    known.extend(menu_labels(&crate::menus::desktop_menus(&recents)));
    known.extend(menu_labels(&[Menu { name: "dock".into(), items: crate::menus::dock_items(&recents) }]));
    known.extend(source_keys());
    // Names from test data (fake dock menu, sample folder, product name) aren't text to translate.
    for not_text in ["dock", "y", "codeyard"] {
        known.remove(not_text);
    }
    known
}

#[gpui::test]
fn every_translation_key_is_a_real_english_label(cx: &mut TestAppContext) {
    let known = known_english_keys(cx);
    for (code, pairs) in tables() {
        let dead: Vec<&str> = pairs.into_iter().map(|(k, _)| k).filter(|k| !known.contains(*k)).collect();
        assert!(dead.is_empty(), "keys of {code}.tsv that exist nowhere as an English label (typo or dead string): {dead:?}");
    }
}

/// Every `t("…")` key in the source (and every tooltip) has a Turkish translation: no half-finished surface.
#[test]
fn every_translated_call_site_has_a_turkish_entry() {
    for (code, pairs) in complete_tables() {
        let table: BTreeSet<&str> = pairs.into_iter().map(|(k, _)| k).collect();
        let mut missing: Vec<String> = source_keys().into_iter().filter(|k| !table.contains(k.as_str())).collect();
        missing.extend(Workbench::tips_for_test().iter().filter(|t| !table.contains(**t)).map(|t| t.to_string()));
        assert!(missing.is_empty(), "strings passed to t()/tf() without an entry in locales/{code}.tsv: {missing:?}");
    }
    assert!(source_keys().len() > 80, "the scanner must find the call sites ({})", source_keys().len());
}

#[gpui::test]
fn palette_shows_turkish_and_still_finds_english(cx: &mut TestAppContext) {
    let _g = LangGuard::set("tr");
    let dir = fresh("i18n-palette");
    let (wb, cx) = setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    cx.simulate_keystrokes("cmd-shift-p");
    cx.run_until_parked();
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert!(labels.iter().any(|l| l == "Dosya: Yeni Dosya…"), "{:?}", &labels[..5]);
    assert!(!labels.iter().any(|l| l == "File: New File…"), "the English label is not displayed");
    // Searching in Turkish...
    wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(">yeni dosya".into(), cx));
    let found = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert!(found.iter().any(|l| l == "Dosya: Yeni Dosya…"), "{found:?}");
    // ...and the English name is found too (the displayed label stays Turkish).
    wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(">file: new file".into(), cx));
    let found = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert_eq!(found.first().map(String::as_str), Some("Dosya: Yeni Dosya…"), "{found:?}");
    wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(">toggle line comment".into(), cx));
    let found = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert!(found.iter().any(|l| l == "Düzenleyici: Satır Yorumunu Aç/Kapat"), "{found:?}");
    // Turkish folding: ASCII spelling finds it too ("gorunum" → Görünüm).
    wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(">gorunum: kenar".into(), cx));
    let found = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert!(found.iter().any(|l| l == "Görünüm: Kenar Çubuğunu Aç/Kapat"), "{found:?}");
    // The command id and the test accessor stay English.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("File: New Window", window, cx));
}

#[gpui::test]
fn palette_stays_english_when_the_language_is_en(cx: &mut TestAppContext) {
    let dir = fresh("i18n-palette-en");
    let (wb, cx) = setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    cx.simulate_keystrokes("cmd-shift-p");
    cx.run_until_parked();
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert!(labels.iter().any(|l| l == "File: New File…"));
    assert!(!labels.iter().any(|l| l == "Dosya: Yeni Dosya…"));
    wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(">dosya: yeni".into(), cx));
    assert!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap().is_empty(), "Turkish labels are not searched in English mode");
}

/// `ui.language` is applied at runtime: when the settings file changes, menus and the palette change without a restart.
#[gpui::test]
fn changing_the_setting_switches_the_language_without_restart(cx: &mut TestAppContext) {
    let _g = LangGuard::set("en");
    let cfg = fresh("i18n-live-cfg");
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "ui.language": "tr" }"#).unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    assert_eq!(i18n::current(), TR, "applied while the settings load");
    assert!(menu_labels(&crate::menus::desktop_menus(&[])).iter().any(|l| l == "Dosya"));
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "ui.language": "en" }"#).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    assert_eq!(i18n::current(), 0, "applied when the file changes");
    assert!(menu_labels(&crate::menus::desktop_menus(&[])).iter().any(|l| l == "File"));
    // Invalid value: a warning, the language stays English (a value that can't be any real language code).
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "ui.language": "not-a-language" }"#).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    assert_eq!(i18n::current(), 0);
}

#[test]
fn headings_are_uppercased_with_the_language_rules() {
    assert_eq!(i18n::upper("Recent"), "RECENT");
    let _g = LangGuard::set("tr");
    assert_eq!(i18n::upper("Özelleştir"), "ÖZELLEŞTİR");
    assert_eq!(i18n::upper("Son Açılanlar"), "SON AÇILANLAR");
    assert_eq!(i18n::upper("Öğren"), "ÖĞREN");
}

#[gpui::test]
fn welcome_status_and_git_dialogs_are_translated(cx: &mut TestAppContext) {
    let _g = LangGuard::set("tr");
    // Tooltip: the Turkish text from the table comes (for each day).
    for day in 0..14 {
        let tip = Workbench::tip_for_test(day);
        assert!(!Workbench::tips_for_test().contains(&tip), "day {day}: still English: {tip}");
    }
    // Git confirmation dialog: title, detail and buttons are Turkish; "Cancel" changes nothing.
    let dir = std::fs::canonicalize(crate::qa_integration_tests::repo("i18n-git")).unwrap();
    let (wb, cx) = setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    std::fs::write(dir.join("README.md"), "# değişti\n").unwrap();
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    crate::qa_integration_tests::wait_ticking(cx, "changes", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|g| g.changes.len() == 1)));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Discard All Changes", window, cx));
    cx.run_until_parked();
    let (title, detail) = cx.pending_prompt().expect("asks first");
    assert_eq!(title, "“README.md” içindeki değişiklikler atılsın mı?");
    assert_eq!(detail, "Bu işlem geri alınamaz.");
    cx.simulate_prompt_answer("İptal");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "# değişti\n");
    // The same dialog, in English.
    drop(_g);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Discard All Changes", window, cx));
    cx.run_until_parked();
    assert_eq!(cx.pending_prompt().unwrap().0, "Discard changes in “README.md”?");
    cx.simulate_prompt_answer("Cancel");
}

/// Sample keys per surface and their expected Turkish equivalents (the table really covers these surfaces).
#[test]
fn the_new_surfaces_have_turkish_entries() {
    let samples: &[(&str, &str, &str)] = &[
        ("source control", "Stage all", "Tümünü Stage'e Al"),
        ("source control", "Staged Changes", "Stage'deki Değişiklikler"),
        ("source control", "Commit All ({})", "Tümünü Commit Et ({})"),
        ("source control", "All Branches", "Tüm Dallar"),
        ("source control", "Hide Merge Commits", "Merge Commit'lerini Gizle"),
        ("terminal", "Previous Match", "Önceki Eşleşme"),
        ("source control", "No commits yet.", "Henüz commit yok."),
        ("source control", "Pop Latest Stash", "Son Stash'i Pop Et"),
        ("source control", "Reset Hard to Here…", "Buraya Hard Reset…"),
        ("source control", "Message (↵ to commit)", "İleti (commit için ↵)"),
        ("explorer", "Open to the Side", "Yana Aç"),
        ("explorer", "No folder is open.", "Açık klasör yok."),
        ("explorer", "Close to the Right", "Sağdakileri Kapat"),
        ("explorer", "Timeline", "Zaman Çizelgesi"),
        ("settings and shortcuts", "Search settings", "Ayarlarda ara"),
        ("settings and shortcuts", "Commonly Used", "Yaygın Kullanılanlar"),
        ("settings and shortcuts", "{} command|{} commands", "{} komut"),
        ("settings and shortcuts", "Search commands or keys (e.g. “comment”, “⌘D”)", "Komut ya da tuş ara (ör. “comment”, “⌘D”)"),
        (
            "file operations",
            "The file changed on disk while you were editing. Nothing was saved.",
            "Siz düzenlerken dosya diskte değişti. Hiçbir şey kaydedilmedi.",
        ),
        ("file operations", "Overwrite", "Üzerine Yaz"),
        ("file operations", "Move “{}” to the Trash?", "“{}” Çöp Kutusu'na taşınsın mı?"),
        ("file operations", "Type a name", "Bir ad yazın"),
        ("file operations", "Do you want to save the changes you made to {}?", "{} dosyasında yaptığınız değişiklikler kaydedilsin mi?"),
        ("git prompts", "Create branch", "Dal oluştur"),
        ("git prompts", "Name of the new branch (from the current one)", "Yeni dalın adı (geçerli daldan)"),
        ("git prompts", "Select a branch to check out", "Checkout edilecek dalı seçin"),
        ("git prompts", "Tag name, then an optional message for an annotated tag", "Etiket adı, ardından açıklamalı etiket için isteğe bağlı bir mesaj"),
    ];
    for (surface, key, want) in samples {
        assert_eq!(lookup(TR, key), Some(*want), "{surface}: {key}");
    }
    // Every month and day name is translated (for day headings), none is the same as English.
    for key in [
        "Jan",
        "Feb",
        "Mar",
        "Apr",
        "May",
        "Jun",
        "Jul",
        "Aug",
        "Sep",
        "Oct",
        "Nov",
        "Dec",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ] {
        assert!(lookup(TR, key).is_some_and(|v| v != key), "{key}");
    }
}

/// Source control: times, day headings, button and status texts follow the language; English is unchanged.
#[test]
fn source_control_texts_follow_the_language() {
    assert_eq!(crate::scm::commit_button(2, 5), ("Commit (2)".into(), true));
    assert_eq!(crate::graph::day_label("2026-10-24 21:46").as_deref(), Some("Saturday, 24 Oct 2026"));
    assert_eq!(crate::git::short_ago("4 days ago"), "4d");
    assert_eq!(crate::git::short_ago("just now"), "now");
    assert_eq!(crate::git::Mark::Modified.describe(), "Modified");
    assert_eq!(crate::graph::GraphView::Dates.label(), "Days");
    let _tr = LangGuard::set("tr");
    assert_eq!(crate::scm::commit_button(2, 5), ("Commit Et (2)".into(), true));
    assert_eq!(crate::scm::commit_button(0, 3), ("Tümünü Commit Et (3)".into(), true));
    assert_eq!(crate::scm::commit_button(0, 0), ("Commit Et".into(), false));
    assert_eq!(crate::graph::day_label("2026-10-24 21:46").as_deref(), Some("24 Ekim 2026 Cumartesi"));
    assert_eq!(crate::graph::day_label("1970-01-01 00:00").as_deref(), Some("1 Ocak 1970 Perşembe"));
    assert_eq!(crate::git::short_ago("4 days ago"), "4g");
    assert_eq!(crate::git::short_ago("25 minutes ago"), "25dk");
    assert_eq!(crate::git::short_ago("1 year, 2 months ago"), "1yıl");
    assert_eq!(crate::git::short_ago("just now"), "şimdi");
    assert_eq!(crate::git::short_ago("whenever"), "whenever", "unknown text stays as it is");
    assert_eq!(crate::git::Mark::Untracked.describe(), "Yeni (izlenmiyor)");
    assert_eq!(crate::graph::GraphView::Graph.label(), "Grafik");
    assert_eq!(tf("Commit {}", &[&"abc1234"]), "Commit: abc1234");
}

/// Settings page: category names are translated on display; ids (`CATEGORIES`) stay English. "Git" is a term.
#[test]
fn settings_categories_are_translated_for_display_only() {
    use crate::settings_page::{CATEGORIES, category_label};
    assert_eq!(CATEGORIES[0], "Commonly Used", "ids stay English");
    for name in CATEGORIES {
        assert_eq!(category_label(name), *name, "English mode shows the id");
    }
    let _tr = LangGuard::set("tr");
    let shown: Vec<&str> = CATEGORIES.iter().map(|c| category_label(c)).collect();
    assert_eq!(
        shown,
        [
            "Yaygın Kullanılanlar",
            "Değiştirildi",
            "Modüller",
            "Düzenleyici",
            "Çalışma Ekranı",
            "Dosyalar",
            "Gezgin",
            "Arama",
            "Git",
            "Terminal",
            "Pencere",
            "Uzak ve Diller"
        ]
    );
}

/// Source control panel: the "…" menu and the commit context menu are Turkish; the items do the same things (by index, not by label).
#[gpui::test]
fn source_control_menus_are_turkish(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(crate::qa_integration_tests::repo("i18n-scm-menus")).unwrap();
    let (wb, cx) = setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    crate::qa_integration_tests::wait_ticking(cx, "graph", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() == 1));
    let commit = scm.read_with(cx, |s, _| s.graph_commit(0)).unwrap();
    let point = gpui::point(gpui::px(80.), gpui::px(80.));
    let labels_after = |cx: &mut gpui::VisualTestContext, event: crate::scm::ScmEvent| {
        scm.update(cx, |_, cx| cx.emit(event));
        cx.run_until_parked();
        let labels = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).expect("menu opened");
        wb.update_in(cx, |wb, window, cx| {
            wb.close_menu_for_test(window, cx);
        });
        labels
    };
    // English
    let more = labels_after(cx, crate::scm::ScmEvent::MoreActions(point));
    assert!(more.iter().any(|l| l == "Create Branch…") && more.iter().any(|l| l == "Discard All Changes"), "{more:?}");
    let commit_menu = labels_after(cx, crate::scm::ScmEvent::CommitMenu(commit.clone(), point));
    assert!(commit_menu.iter().any(|l| l == "Create Tag Here…") && commit_menu.iter().any(|l| l == "Revert"), "{commit_menu:?}");
    // Turkish
    let _tr = LangGuard::set("tr");
    let more = labels_after(cx, crate::scm::ScmEvent::MoreActions(point));
    for want in ["Pull", "Push", "Fetch", "Checkout…", "Dal Oluştur…", "Commit'i Düzelt (Amend)", "Stash", "Stash Uygula…", "Tüm Değişiklikleri At"]
    {
        assert!(more.iter().any(|l| l == want), "{want} missing in {more:?}");
    }
    assert!(!more.iter().any(|l| l == "Create Branch…" || l == "Discard All Changes"), "{more:?}");
    let commit_menu = labels_after(cx, crate::scm::ScmEvent::CommitMenu(commit, point));
    for want in ["Commit Özetini Kopyala", "Burada Etiket Oluştur…", "Cherry-pick Yap", "Revert Et", "Buraya Hard Reset…"] {
        assert!(commit_menu.iter().any(|l| l == want), "{want} missing in {commit_menu:?}");
    }
    // Menu and command stay the same: picking from the Turkish menu (by index) still opens the "Git: Create Tag…" flow.
    scm.update(cx, |_, cx| cx.emit(crate::scm::ScmEvent::MoreActions(point)));
    cx.run_until_parked();
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Dal Oluştur…", cx));
    cx.run_until_parked();
    let rows = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).expect("branch name prompt");
    assert_eq!(rows, ["Bir ad yazın"], "the input prompt of the branch flow is Turkish");
    wb.update(cx, |wb, cx| wb.set_overlay_query_for_test("özellik".into(), cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap(), ["Dal oluştur “özellik”"]);
}

/// Explorer: file and tab context menus, delete confirmation and name prompts are Turkish.
#[gpui::test]
fn explorer_menus_prompts_and_tab_menu_are_turkish(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(crate::qa_workbench_tests::fresh("i18n-explorer")).unwrap();
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let (wb, cx) = setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_integration_tests::wait_ticking(cx, "tree", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).contains(&"a.txt".to_string()));
    let tree_menu = |cx: &mut gpui::VisualTestContext, path: std::path::PathBuf, is_dir: bool| {
        let ev = crate::tree::TreeEvent::ContextMenu { path: path.to_string_lossy().into_owned(), is_dir, position: gpui::point(gpui::px(90.), gpui::px(90.)) };
        wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(ev, window, cx));
        let labels = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).expect("menu opened");
        wb.update_in(cx, |wb, window, cx| wb.close_menu_for_test(window, cx));
        labels
    };
    let _tr = LangGuard::set("tr");
    let file = tree_menu(cx, dir.join("a.txt"), false);
    for want in [
        "Yeni Dosya…",
        "Yeni Klasör…",
        "Kes",
        "Kopyala",
        "Yapıştır",
        "Yeniden Adlandır…",
        "Sil",
        "Çoğalt",
        "Yana Aç",
        "Karşılaştırmak İçin Seç",
        "Yolu Kopyala",
        "Sık Kullanılanlara Ekle",
    ] {
        assert!(file.iter().any(|l| l == want), "{want} missing in {file:?}");
    }
    assert!(!file.iter().any(|l| l == "Rename…" || l == "Duplicate"), "{file:?}");
    let folder = tree_menu(cx, dir.join("src"), true);
    for want in ["İçindekilerin Tümünü Genişlet", "İçindekilerin Tümünü Daralt", "Klasörde Bul…"] {
        assert!(folder.iter().any(|l| l == want), "{want} missing in {folder:?}");
    }
    // Tab context menu.
    crate::qa_workbench_tests::open_in(&wb, &dir.join("a.txt"), cx);
    wb.update_in(cx, |wb, window, cx| wb.open_tab_menu_for_test(0, window, cx));
    let tab = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).expect("tab menu");
    for want in ["Kapat", "Diğerlerini Kapat", "Sağdakileri Kapat", "Kaydedilenleri Kapat", "Sabitle", "Göreli Yolu Kopyala"] {
        assert!(tab.iter().any(|l| l == want), "{want} missing in {tab:?}");
    }
    wb.update_in(cx, |wb, window, cx| wb.close_menu_for_test(window, cx));
    // Delete confirmation.
    let ev = crate::tree::TreeEvent::Delete(dir.join("a.txt").to_string_lossy().into_owned(), false);
    wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(ev, window, cx));
    cx.run_until_parked();
    let (title, _detail) = cx.pending_prompt().expect("asks first");
    assert!(title.contains("a.txt") && (title.ends_with("taşınsın mı?") || title.ends_with("silinsin mi?")), "{title}");
    cx.simulate_prompt_answer("İptal");
    cx.run_until_parked();
    assert!(dir.join("a.txt").exists());
    // Name prompts: new file and rename.
    wb.update_in(cx, |wb, window, cx| wb.start_create(dir.to_string_lossy().into_owned(), false, window, cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap(), ["Bir ad yazın"]);
    wb.update(cx, |wb, cx| wb.set_overlay_query_for_test("b/../c".into(), cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap(), ["Yollar / ile bitemez ya da . veya .. parçaları içeremez"]);
    wb.update(cx, |wb, cx| wb.set_overlay_query_for_test("yeni.txt".into(), cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap(), ["Dosya oluştur “yeni.txt”"]);
}

/// File saving and editor band: formatted messages are Turkish, in English mode as before.
#[test]
fn save_and_file_operation_messages_are_translated() {
    let english = tf("Could not save: {}", &[&"disk full"]);
    assert_eq!(english, "Could not save: disk full");
    assert_eq!(
        tf("Not saved: {0} (U+{1}) cannot be written in {2}. Remove it, or save a copy as UTF-8.", &[&"\"ş\"", &"015F", &"ISO-8859-1"]),
        "Not saved: \"ş\" (U+015F) cannot be written in ISO-8859-1. Remove it, or save a copy as UTF-8."
    );
    let _tr = LangGuard::set("tr");
    assert_eq!(tf("Could not save: {}", &[&"disk full"]), "Kaydedilemedi: disk full");
    assert_eq!(tf("{} already exists. Choose another name.", &[&"/x/y.txt"]), "/x/y.txt zaten var. Başka bir ad seçin.");
    assert_eq!(tf("Delete “{}” permanently?", &[&"a.txt"]), "“a.txt” kalıcı olarak silinsin mi?");
    assert_eq!(t("Overwrite"), "Üzerine Yaz");
    assert_eq!(t("Discard mine, load from disk"), "Benimkini at, diskten yükle");
    assert_eq!(tf("Renamed {0} to {1}", &[&"eski", &"yeni"]), "eski, yeni olarak yeniden adlandırıldı");
}

// ---- Multi-language infrastructure: plural, regional matching, completeness tiers, template and status report ----

/// CLDR examples: number → form index for each rule.
#[test]
fn plural_rules_follow_cldr() {
    let cases: &[(Plural, &[(usize, usize)])] = &[
        (Plural::None, &[(0, 0), (1, 0), (2, 0), (100, 0)]),
        (Plural::OneOther, &[(0, 1), (1, 0), (2, 1), (21, 1)]),
        (Plural::French, &[(0, 0), (1, 0), (2, 1), (1000, 1)]),
        (Plural::EastSlavic, &[(0, 2), (1, 0), (2, 1), (4, 1), (5, 2), (11, 2), (12, 2), (14, 2), (21, 0), (22, 1), (25, 2), (101, 0), (111, 2), (112, 2)]),
        (Plural::Polish, &[(0, 2), (1, 0), (2, 1), (4, 1), (5, 2), (12, 2), (14, 2), (21, 2), (22, 1), (25, 2), (112, 2), (122, 1)]),
    ];
    for (rule, expect) in cases {
        for (n, index) in *expect {
            assert_eq!(rule.index(*n), *index, "{rule:?} n={n}");
            assert!(rule.index(*n) < rule.forms(), "{rule:?} n={n}: index within forms()");
        }
    }
    assert_eq!([Plural::None, Plural::OneOther, Plural::French, Plural::EastSlavic, Plural::Polish].map(Plural::forms), [1, 2, 2, 3, 3]);
}

#[test]
fn translated_forms_are_picked_by_the_languages_rule() {
    // A three-form translation like Russian, without a real Russian table.
    let ru = "{} файл|{} файла|{} файлов";
    let pick = |n| fill(pick_form(Plural::EastSlavic, ru, n).unwrap(), &[&n]);
    assert_eq!([pick(1), pick(2), pick(5), pick(21), pick(11)], ["1 файл", "2 файла", "5 файлов", "21 файл", "11 файлов"]);
    assert_eq!(pick_form(Plural::EastSlavic, "{} файл|{} файла", 5), None, "a missing form is reported, never guessed");
    assert_eq!(pick_form(Plural::None, "{} dosya", 7), Some("{} dosya"));
}

#[test]
fn tn_uses_the_english_rule_without_a_translation_and_the_table_with_one() {
    assert_eq!(tn("{} file", "{} files", 1, &[]), "1 file");
    assert_eq!(tn("{} file", "{} files", 0, &[]), "0 files");
    assert_eq!(tn("{} file", "{} files", 2, &[]), "2 files");
    // The first placeholder is n; extra arguments come after it, `{1}` can reorder.
    assert_eq!(tn("{} line in {}", "{} lines in {}", 3, &[&"a.rs"]), "3 lines in a.rs");
    assert_eq!(tn("{0} line in {1}", "{0} lines in {1}", 1, &[&"a.rs"]), "1 line in a.rs");
    // A plural key not in the table stays English in Turkish too, with the English rule.
    let _tr = LangGuard::set("tr");
    assert_eq!(tn("{} nonexistent thing", "{} nonexistent things", 2, &[]), "2 nonexistent things");
    // A plural key in the table: a single form in Turkish, independent of the number.
    assert_eq!([tn("{} conflict", "{} conflicts", 1, &[]), tn("{} conflict", "{} conflicts", 5, &[])], ["1 çakışma", "5 çakışma"]);
    assert_eq!(tn("{} cursor", "{} cursors", 3, &[]), "3 imleç");
}

#[test]
fn plural_entries_are_well_formed_in_every_language() {
    for l in i18n::LANGUAGES.iter().filter(|l| !l.tables.is_empty()) {
        for (key, value) in parse_tables(l.tables).0 {
            if !key.contains('|') {
                assert!(!value.contains('|'), "{}/{key:?}: only plural entries (key `singular|plural`) may use `|` in the translation", l.code);
                continue;
            }
            let english: Vec<&str> = key.split('|').collect();
            assert_eq!(english.len(), 2, "{}/{key:?}: a plural key is exactly `English singular|English plural`", l.code);
            assert!(english.iter().all(|e| placeholders(e).contains(&0)), "{}/{key:?}: both English forms must contain the count ({{}} or {{0}})", l.code);
            let forms: Vec<&str> = value.split('|').collect();
            assert_eq!(
                forms.len(),
                l.plural.forms(),
                "{}/{key:?}: {:?} needs {} form(s) separated by `|`, found {}",
                l.code,
                l.plural,
                l.plural.forms(),
                forms.len()
            );
            let allowed: BTreeSet<usize> = placeholders(english[0]).into_iter().chain(placeholders(english[1])).collect();
            for form in forms {
                assert!(!form.trim().is_empty() && form.trim() == form, "{}/{key:?}: empty or padded form {form:?}", l.code);
                assert!(placeholders(form).iter().all(|p| allowed.contains(p)), "{}/{key:?}: {form:?} uses a placeholder the key does not have", l.code);
            }
        }
    }
}

#[test]
fn regional_variants_resolve_to_the_most_specific_language() {
    let codes = ["en", "pt", "pt-BR", "zh-Hans", "zh-Hant", "sr_Latn"];
    let at = |l: &str| i18n::language_of_in(&codes, l);
    assert_eq!(at("pt_BR.UTF-8"), Some(2), "region beats the bare language");
    assert_eq!(at("PT-br"), Some(2), "case and `_` vs `-` do not matter");
    assert_eq!(at("pt-PT"), Some(1), "an unlisted region falls back to the language");
    assert_eq!(at("pt"), Some(1));
    assert_eq!(at("zh-Hant-TW"), Some(4), "script subtag, region dropped");
    assert_eq!(at("zh-Hans-CN"), Some(3));
    assert_eq!(at("zh-CN"), None, "no bare `zh` entry: Chinese is ambiguous, so English");
    assert_eq!(at("zh"), None);
    assert_eq!(at("sr-Latn-RS"), Some(5), "`_` in a registered code matches `-` in the system tag");
    assert_eq!(at("ptx-BR"), None, "only whole subtags match");
    assert_eq!(at("C"), Some(0));
    assert_eq!(at("en_US@euro"), Some(0));
    assert_eq!(at(""), None);
    let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(i18n::resolve_in(&codes, "auto", &l(&["de-DE", "pt-BR"])), 2, "first supported system language");
    assert_eq!(i18n::resolve_in(&codes, "pt-BR", &l(&["en"])), 2, "an explicit setting is an exact code");
}

#[test]
fn every_language_has_its_own_case_rule_and_name() {
    use crate::textcase::CaseLocale;
    for l in i18n::LANGUAGES {
        assert!(!l.name.trim().is_empty(), "{}: native name", l.code);
        assert_eq!(l.code, l.code.trim(), "{}", l.code);
    }
    let _tr = LangGuard::set("tr");
    assert_eq!(i18n::LANGUAGES[i18n::current()].case, CaseLocale::Turkish);
    assert_eq!(i18n::upper("izle ılık"), "İZLE ILIK");
    drop(_tr);
    assert_eq!(i18n::upper("izle ılık"), "IZLE ILIK", "English uses the invariant mapping");
}

/// Menu and button labels (short texts) that open a dialog end with "…": the translation keeps this rule. Sentences
/// can reorder words, so only labels of at most six words are checked.
#[test]
fn translations_keep_the_ellipsis_convention() {
    for (code, pairs) in tables() {
        let broken: Vec<String> = pairs
            .into_iter()
            .filter(|(k, v)| k.split_whitespace().count() <= 6 && k.ends_with('…') != v.ends_with('…'))
            .map(|(k, v)| format!("{k:?} → {v:?}"))
            .collect();
        assert!(broken.is_empty(), "{code}: a trailing … on a label must appear in both the key and the translation: {broken:?}");
    }
}

fn template_text(keys: &BTreeSet<String>) -> String {
    let mut out = String::from(
        "# Translation template: every English UI string, generated by `UPDATE_I18N=1 cargo test -p app --bin codeyard i18n_template`.\n\
         # Copy this file to `<code>.tsv`, put the translation after the TAB (see docs/translating.md). A key containing `|` is a plural\n\
         # entry: write one form per plural form of the language, separated by `|`. Do not edit the left column.\n",
    );
    for k in keys {
        out.push_str(k);
        out.push_str("\t\n");
    }
    out
}

/// `locales/template.tsv` lists every key: a new language starts as a copy of it; this test keeps the file current.
#[gpui::test]
fn i18n_template_is_up_to_date(cx: &mut TestAppContext) {
    let keys = known_english_keys(cx);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales/template.tsv");
    let want = template_text(&keys);
    if std::env::var_os("UPDATE_I18N").is_some() {
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(have == want, "locales/template.tsv is out of date: run `UPDATE_I18N=1 cargo test -p app --bin codeyard i18n_template`");
}

/// `scripts/i18n-status.sh` runs this test: coverage per language, and with `I18N_LIST=1` the missing strings too.
#[gpui::test]
fn i18n_status_report(cx: &mut TestAppContext) {
    let keys = known_english_keys(cx);
    for l in i18n::LANGUAGES.iter().filter(|l| l.status != Status::Source) {
        let have: BTreeSet<&str> = parse_tables(l.tables).0.into_iter().map(|(k, _)| k).collect();
        let missing: Vec<&String> = keys.iter().filter(|k| !have.contains(k.as_str())).collect();
        let done = keys.len() - missing.len();
        eprintln!("i18n {} ({}, {:?}): {done}/{} strings = {:.1}%", l.code, l.name, l.status, keys.len(), 100.0 * done as f64 / keys.len() as f64);
        if std::env::var_os("I18N_LIST").is_some() {
            for k in &missing {
                eprintln!("  missing: {k}");
            }
        }
        if l.status == Status::Complete {
            assert!(missing.is_empty(), "{}: a Complete language has no missing strings: {missing:?}", l.code);
        }
    }
}
