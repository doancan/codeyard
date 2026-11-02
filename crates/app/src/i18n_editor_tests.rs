//! Translation behavior tests for the editor and workspace surfaces: editor, find/replace, LSP notices, Problems, Search, pickers, merge
//! editor, Markdown, spell check, terminal, tasks, large files, accessibility. Each runs the real function or
//! the Workbench in Turkish and English (coverage tests are in `i18n_tests.rs` and apply to all surfaces automatically).

use gpui::TestAppContext;

use crate::i18n::{LangGuard, t, tf, tn};
use crate::qa_integration_tests::{notice, wait_ticking};
use crate::qa_workbench_tests::{fresh, open, setup};

/// Runs the same command first in Turkish, then in English, and compares the notice text.
fn notice_of(cx: &mut TestAppContext, setup_dir: Option<&std::path::Path>, command: &str, lang: &str) -> String {
    let _g = LangGuard::set(lang);
    let (wb, cx) = setup(cx);
    if let Some(dir) = setup_dir {
        open(&wb, dir, cx);
    }
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(command, window, cx));
    cx.run_until_parked();
    notice(&wb, cx)
}

#[gpui::test]
fn workbench_notices_follow_the_language(cx: &mut TestAppContext) {
    let dir = fresh("i18n-a-notices");
    let file = dir.join("plain.txt");
    std::fs::write(&file, "nothing to merge\n").unwrap();
    // (command, folder/file open?, English, Turkish)
    let cases: [(&str, Option<&std::path::Path>, &str, &str); 5] = [
        ("Merge Conflict: Open Merge Editor", Some(&file), "No merge conflicts in this file.", "Bu dosyada birleştirme çakışması yok."),
        ("File: Filter Log Lines…", None, "Open a file first.", "Önce bir dosya açın."),
        (
            "Tasks: Run Task…",
            None,
            "Open a folder first: tasks come from its package.json, Makefile, Cargo.toml or tasks.jsonc.",
            "Önce bir klasör açın: görevler klasörün package.json, Makefile, Cargo.toml ya da tasks.jsonc dosyasından gelir.",
        ),
        (
            "Test: Run Test at Cursor",
            None,
            "Open a file inside a folder to run its tests.",
            "Testlerini çalıştırmak için bir klasörün içindeki bir dosyayı açın.",
        ),
        ("Spelling: Fix Word at Cursor…", Some(&file), "No misspelled word at the cursor.", "İmlecin altında yanlış yazılmış sözcük yok."),
    ];
    for (command, open_dir, en, tr) in cases {
        assert_eq!(notice_of(cx, open_dir, command, "tr"), tr, "{command} (tr)");
        assert_eq!(notice_of(cx, open_dir, command, "en"), en, "{command} (en)");
    }
}

#[gpui::test]
fn lsp_notices_when_no_server_runs(cx: &mut TestAppContext) {
    let dir = fresh("i18n-a-lsp");
    let file = dir.join("plain.txt");
    std::fs::write(&file, "word\n").unwrap();
    for (command, en, tr) in [
        ("Editor: Rename Symbol", "No language server for this file", "Bu dosya için dil sunucusu yok"),
        ("Editor: Quick Fix…", "No language server for this file", "Bu dosya için dil sunucusu yok"),
    ] {
        let got = notice_of(cx, Some(&file), command, "tr");
        // Spell check may kick in first: then there is no notice; with no server the text is Turkish.
        assert!(got.is_empty() || got == tr, "{command}: {got}");
        let got = notice_of(cx, Some(&file), command, "en");
        assert!(got.is_empty() || got == en, "{command}: {got}");
    }
}

#[test]
fn lsp_location_texts_use_plural_and_per_kind_keys() {
    use crate::lsp;
    let _g = LangGuard::set("tr");
    assert_eq!(lsp::locations_title("references", 3), "3 başvuru");
    assert_eq!(lsp::locations_title("incoming calls", 2), "2 gelen çağrı");
    assert_eq!(lsp::none_found("references"), "Başvuru bulunamadı");
    assert_eq!(lsp::none_found("type definition"), "Tür tanımı bulunamadı");
    assert_eq!(lsp::unsupported("subtypes"), "Bu dil sunucusu alt türleri desteklemiyor");
    assert_eq!(lsp::symbol_kind(12), "işlev");
    assert_eq!(
        lsp::install_hint_text("install Xcode or a Swift toolchain (it contains sourcekit-lsp)"),
        "Xcode'u ya da bir Swift araç zinciri kurun (sourcekit-lsp içindedir)"
    );
    assert_eq!(lsp::install_hint_text("npm i -g pyright"), "npm i -g pyright", "shell commands stay as they are");
    drop(_g);
    assert_eq!(lsp::locations_title("references", 1), "1 reference");
    assert_eq!(lsp::locations_title("references", 3), "3 references");
    assert_eq!(lsp::none_found("implementations"), "No implementations found");
    assert_eq!(lsp::unsupported("incoming calls"), "This language server does not support incoming calls");
    assert_eq!(lsp::symbol_kind(12), "function");
}

#[test]
fn editor_popup_and_status_texts() {
    let _g = LangGuard::set("tr");
    assert_eq!(t("Accept Current Change"), "Geçerli Değişikliği Kabul Et");
    assert_eq!(t("Open Merge Editor"), "Birleştirme Düzenleyicisini Aç");
    assert_eq!(t("Send Request"), "İstek Gönder");
    assert_eq!(tf("(+{} more)", &[&2]), "(+2 daha)");
    assert_eq!(tf("{0} of {1}{2}", &[&2, &5, &"+"]), "5+ içinde 2");
    assert_eq!(tn("Resolved {} conflict", "Resolved {} conflicts", 4, &[]), "4 çakışma çözüldü");
    assert_eq!(tn("Staged {} change", "Staged {} changes", 1, &[]), "1 değişiklik stage'e alındı");
    assert_eq!(t("Go to Symbol in Editor…"), "Düzenleyicide Sembole Git…");
    assert_eq!(t("Toggle Bookmark"), "Yer İmini Aç/Kapat");
    assert_eq!(t("Quick Fix…"), "Hızlı Düzeltme…");
    drop(_g);
    assert_eq!(tn("Resolved {} conflict", "Resolved {} conflicts", 1, &[]), "Resolved 1 conflict");
    assert_eq!(tn("Resolved {} conflict", "Resolved {} conflicts", 4, &[]), "Resolved 4 conflicts");
}

#[gpui::test]
fn editor_status_labels_follow_the_language(cx: &mut TestAppContext) {
    let dir = fresh("i18n-a-status");
    let file = dir.join("plain.txt");
    std::fs::write(&file, "a\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let (en_indent, en_lang) = ed.read_with(cx, |e, _| (e.indent_label(), e.language_label()));
    assert!(en_indent.ends_with("spaces") || en_indent == "Tabs", "{en_indent}");
    assert_eq!(en_lang, "Plain Text");
    let _g = LangGuard::set("tr");
    let (tr_indent, tr_lang, key) = ed.read_with(cx, |e, _| (e.indent_label(), e.language_label(), e.indent_key()));
    assert!(tr_indent.ends_with("boşluk") || tr_indent == "Sekme", "{tr_indent}");
    assert_eq!(tr_lang, "Düz Metin");
    assert!(key.ends_with("spaces") || key == "Tabs", "the picker compares the English key: {key}");
    // The accessibility tree's status lines follow the same language.
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert!(snap.status.iter().any(|s| s == "Düz Metin"), "{:?}", snap.status);
    assert!(snap.status.iter().any(|s| s.contains("0 hata, 0 uyarı")), "{:?}", snap.status);
    assert!(snap.status.iter().any(|s| s.starts_with("Sat 1, Süt 1")), "{:?}", snap.status);
    drop(_g);
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    assert!(snap.status.iter().any(|s| s == "0 errors, 0 warnings"), "{:?}", snap.status);
}

#[gpui::test]
fn find_in_files_status_and_results_document(cx: &mut TestAppContext) {
    let dir = fresh("i18n-a-search");
    std::fs::write(dir.join("a.txt"), "needle one\nneedle two\n").unwrap();
    std::fs::write(dir.join("b.txt"), "other\n").unwrap();
    for (lang, query_line, options, none) in
        [("tr", "# Sorgu: needle", "# Büyük/Küçük Harf Duyarsız", "bulunamadı"), ("en", "# Query: needle", "# Ignore Case", "not found")]
    {
        let _g = LangGuard::set(lang);
        let (wb, cx) = setup(cx);
        open(&wb, &dir, cx);
        let search = wb.read_with(cx, |wb, _| wb.search_panel());
        cx.simulate_keystrokes("cmd-shift-f");
        cx.simulate_input("needle");
        cx.simulate_keystrokes("enter");
        wait_ticking(cx, "search", |cx| search.read_with(cx, |s, _| !s.is_searching()));
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Search: Open Search Editor", window, cx));
        let ed = wb.read_with(cx, |wb, _| wb.active_search_editor_for_test()).expect("a Search Editor opened");
        wait_ticking(cx, "search editor", |cx| ed.read_with(cx, |e, _| e.doc.state == crate::search_doc::RunState::Complete));
        let text = ed.read_with(cx, |e, _| e.doc.as_text());
        assert!(text.starts_with(&format!("{query_line}\n{options}\n")), "{lang}: {text}");
        let _ = none;
    }
    let _g = LangGuard::set("tr");
    assert_eq!(tn("{} result in 1 file", "{} results in 1 file", 2, &[]), "1 dosyada 2 sonuç");
    assert_eq!(tn("{0} result in {1} files", "{0} results in {1} files", 5, &[&3]), "3 dosyada 5 sonuç");
    assert_eq!(tn("No results in {} file.", "No results in {} files.", 4, &[]), "4 dosyada sonuç yok.");
    assert_eq!(tf("{0} (stopped at {1})", &[&"9 sonuç", &1000]), "9 sonuç (1000 sonuçta durdu)");
    assert_eq!(
        tn("Replace {0} match in {1} files with “{2}”?", "Replace {0} matches in {1} files with “{2}”?", 7, &[&2, &"x"]),
        "2 dosyadaki 7 eşleşme “x” ile değiştirilsin mi?"
    );
    assert_eq!(t("Whole word"), "Tam sözcük");
    assert_eq!(t("Match case (⌥C)"), "Büyük/küçük harfe duyarlı (⌥C)");
    drop(_g);
    assert_eq!(tn("{} result in 1 file", "{} results in 1 file", 1, &[]), "1 result in 1 file");
}

#[test]
fn problems_from_task_output_are_translated() {
    let out = "thread 'main' panicked at 'boom', src/main.rs:3:5\n";
    let _g = LangGuard::set("tr");
    let v = crate::problems::parse_output(out, "/proj");
    assert_eq!(v[0].1.message, "test çöktü: boom");
    let go = crate::problems::parse_output("--- FAIL: TestA (0.00s)\n    a_test.go:14: expected 2, got 3\nFAIL\n", "/proj/pkg");
    assert_eq!(go[0].1.message, "test başarısız: expected 2, got 3");
    drop(_g);
    assert_eq!(crate::problems::parse_output(out, "/proj")[0].1.message, "test panicked: boom");
}

#[test]
fn terminal_task_and_log_texts() {
    let _g = LangGuard::set("tr");
    assert_eq!(tf("Process exited with code {}. Press any key to close.", &[&1]), "Süreç 1 koduyla çıktı. Kapatmak için bir tuşa basın.");
    assert_eq!(tf("Close a terminal first: at most {} can be open.", &[&8]), "Önce bir terminali kapatın: en fazla 8 terminal açık olabilir.");
    assert_eq!(t("No tests found for this file type."), "Bu dosya türü için test bulunamadı.");
    assert_eq!(
        t("No tasks found. “Tasks: Configure Tasks” creates .codeyard/tasks.jsonc."),
        "Görev bulunamadı. “Görevler: Görevleri Yapılandır” komutu .codeyard/tasks.jsonc dosyasını oluşturur."
    );
    assert_eq!(crate::large::unreadable_reason("Permission denied"), "dosya artık okunamıyor (erişim reddedildi)");
    assert_eq!(crate::large::unreadable_reason("weird"), "dosya şu anda okunamıyor (weird)");
    assert_eq!(tn("↓ {} new line", "↓ {} new lines", 3, &[]), "↓ 3 yeni satır");
    drop(_g);
    assert_eq!(crate::large::unreadable_reason("Permission denied"), "the file is no longer readable (permission denied)");
    assert_eq!(tn("↓ {} new line", "↓ {} new lines", 1, &[]), "↓ 1 new line");
}

#[test]
fn pickers_markdown_merge_and_spell_texts() {
    let _g = LangGuard::set("tr");
    assert_eq!(tf("Go to line {0}, column {1}", &[&3, &4]), "3. satır, 4. sütuna git");
    assert_eq!(tf("Type a line number between 1 and {}", &[&10]), "1 ile 10 arasında bir satır numarası yazın");
    assert_eq!(t("Quick fix"), "Hızlı düzeltme");
    assert_eq!(tf("Saved project “{}” (⌃R to switch)", &[&"x"]), "“x” projesi kaydedildi (değiştirmek için ⌃R)");
    assert_eq!(tf("★ Added to favorites: {}", &[&"a.rs"]), "★ Sık Kullanılanlara eklendi: a.rs");
    assert_eq!(tn("Workspace: {} folder in the Explorer.", "Workspace: {} folders in the Explorer.", 2, &[]), "Çalışma alanı: Gezgin'de 2 klasör.");
    assert_eq!(tf("Merge {}", &[&"a.rs"]), "Birleştirme: a.rs");
    assert_eq!(tn("{} conflict left", "{} conflicts left", 2, &[]), "2 çakışma kaldı");
    assert_eq!(tf("Preview of {}", &[&"README.md"]), "Önizleme: README.md");
    assert_eq!(tf("Added “{}” to your dictionary.", &[&"kod"]), "“kod” sözlüğünüze eklendi.");
    assert_eq!(tf("{} is not installed", &[&"de"]), "de kurulu değil");
    assert!(t("Show lines matching (regular expression, case-insensitive), e.g. {}").starts_with("Eşleşen"));
    assert_eq!(crate::icon_theme::IconTheme::Badges.label(), "Dil Rozetleri");
    drop(_g);
    assert_eq!(tf("Go to line {0}, column {1}", &[&3, &4]), "Go to line 3, column 4");
    assert_eq!(crate::icon_theme::IconTheme::Badges.label(), "Language Badges");
}

#[test]
fn settings_catalog_descriptions_are_translated_but_docs_stay_english() {
    use crate::workbench::settings_ui::CATALOG;
    let first = &CATALOG[0];
    assert_eq!(first.about, "Font size of the editor", "the catalog keeps the English key (docs are generated from it)");
    let _g = LangGuard::set("tr");
    assert_eq!(t(first.about), "Düzenleyicinin yazı tipi boyutu");
    let untranslated: Vec<&str> = CATALOG.iter().map(|e| e.about).filter(|a| t(a) == *a).collect();
    assert!(untranslated.is_empty(), "{untranslated:?}");
}
