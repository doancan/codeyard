//! Translation behavior tests for remote server, debugger and network surfaces: SSH, ports, debugger, .http, GitHub, git results, settings warnings,
//! About window. Each runs the real function in Turkish and English (coverage tests are in `i18n_tests.rs`).

use gpui::TestAppContext;

use crate::i18n::{LangGuard, t, tf, tn};
use crate::qa_workbench_tests::setup;

#[gpui::test]
fn about_dialog_and_buttons_are_turkish(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    {
        let _g = LangGuard::set("tr");
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Help: About Codeyard", window, cx));
        cx.run_until_parked();
        let (title, detail) = cx.pending_prompt().expect("about dialog");
        assert_eq!(title, "Codeyard");
        assert!(detail.contains(&format!("Sürüm {}", env!("CARGO_PKG_VERSION"))), "{detail}");
        cx.simulate_prompt_answer("Kapat");
        cx.run_until_parked();
    }
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Help: About Codeyard", window, cx));
    cx.run_until_parked();
    let (_, detail) = cx.pending_prompt().expect("about dialog");
    assert!(detail.contains(&format!("Version {}", env!("CARGO_PKG_VERSION"))), "{detail}");
    cx.simulate_prompt_answer("Close");
}

#[test]
fn ssh_host_and_transfer_texts() {
    let _g = LangGuard::set("tr");
    assert_eq!(tf("Added “{}”", &[&"node-dev"]), "“node-dev” eklendi");
    assert_eq!(tf("Remove “{}” from ~/.ssh/config?", &[&"node-dev"]), "“node-dev” ~/.ssh/config dosyasından kaldırılsın mı?");
    assert_eq!(tf("{}: reachable in {} ms · {} · ready", &[&"node-dev", &12, &"Linux x86_64"]), "node-dev: 12 ms içinde erişildi · Linux x86_64 · hazır");
    assert_eq!(
        tf("Key login is set up for “{}” (created {}). The next connection needs no password.", &[&"a", &"/k"]),
        "“a” için anahtarla giriş kuruldu (oluşturulan: /k). Sonraki bağlantı parola istemez."
    );
    assert_eq!(t("Do you trust the authors of this folder?"), "Bu klasörün yazarlarına güveniyor musunuz?");
    assert_eq!(t("Trust Folder and Continue"), "Klasöre Güven ve Devam Et");
    assert_eq!(t("Trust this host and connect"), "Bu sunucuya güven ve bağlan");
    assert_eq!(tn("{} item already exists. Replace it?", "{} items already exist. Replace them?", 3, &[]), "3 öğe zaten var. Değiştirilsin mi?");
    assert_eq!(tn("Uploaded {} item ({}) to {}", "Uploaded {} items ({}) to {}", 2, &[&"1.0 KB", &"/srv"]), "2 öğe (1.0 KB) /srv konumuna yüklendi");
    drop(_g);
    assert_eq!(tn("Uploaded {} item ({}) to {}", "Uploaded {} items ({}) to {}", 2, &[&"1.0 KB", &"/srv"]), "Uploaded 2 items (1.0 KB) to /srv");
    assert_eq!(tn("Uploaded {} item ({}) to {}", "Uploaded {} items ({}) to {}", 1, &[&"1.0 KB", &"/srv"]), "Uploaded 1 item (1.0 KB) to /srv");
}

#[test]
fn ports_and_debugger_texts() {
    let _g = LangGuard::set("tr");
    assert_eq!(
        tf("Port {} is forwarded to localhost:{} ({} was busy here)", &[&3000, &3001, &3000]),
        "Bağlantı noktası 3000, localhost:3001 adresine yönlendirildi (3000 burada meşguldü)"
    );
    assert_eq!(t("No debug session. F5 starts one."), "Hata ayıklama oturumu yok. F5 bir oturum başlatır.");
    assert_eq!(tf("paused on {}", &[&"kesme noktası"]), "kesme noktası nedeniyle duraklatıldı");
    let detected = crate::debug_config::detect(Some("[package]\nname = \"demo\"\n"), true, Some("/p/a.py"), "/p");
    let names: Vec<&str> = detected.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Rust Hata Ayıklama: demo", "Go Paketi Hata Ayıklama", "Python Hata Ayıklama: a.py"]);
    let e = crate::debug_config::adapter_for("cobol").unwrap_err();
    assert!(e.starts_with("“cobol” türü için hata ayıklama bağdaştırıcısı yok."), "{e}");
    drop(_g);
    let detected = crate::debug_config::detect(Some("[package]\nname = \"demo\"\n"), false, None, "/p");
    assert_eq!(detected[0].name, "Debug Rust: demo");
    assert!(crate::debug_config::adapter_for("cobol").unwrap_err().starts_with("No debug adapter for type “cobol”."));
}

#[test]
fn http_explanations_and_confirmations() {
    use crate::http_client as http;
    let _g = LangGuard::set("tr");
    let e = http::explain_curl(Some(7), "", "bu bilgisayar", "http://localhost:3000/x");
    assert!(e.starts_with("localhost:3000 sunucusuna bağlanılamadı: orada dinleyen yok"), "{e}");
    assert!(e.contains("http.runOn ayarını \"local\" yapın."), "{e}");
    assert_eq!(http::explain_curl(Some(28), "", "x", "http://a/"), "Sunucu 60 saniye içinde yanıt vermedi.");
    assert_eq!(http::explain_curl(Some(99), "boom", "x", "http://a/"), "boom (curl çıkış kodu 99)");
    assert_eq!(http::explain_curl(None, "", "x", "http://a/"), "İstek başarısız oldu.");
    let req = http::parse("POST http://a/x\n\n< /etc/passwd\n").requests[0].clone();
    let parts = http::sensitive_parts(&req, &[]);
    assert!(parts.iter().any(|p| p.contains("/etc/passwd dosyasını yüklüyor")), "{parts:?}");
    let ctx_vars: Vec<(String, String)> = Vec::new();
    let responses = std::collections::HashMap::new();
    let env = |_: &str| None;
    let uuid = || String::new();
    let random = |a: i64, _: i64| a;
    let ctx = http::Context { vars: &ctx_vars, responses: &responses, now: 0, env: &env, uuid: &uuid, random: &random };
    assert_eq!(http::substitute("{{nope}}", &ctx, 0).unwrap_err(), "tanımsız değişken nope");
    assert_eq!(http::substitute("{{$foo}}", &ctx, 0).unwrap_err(), "bilinmeyen değişken {{$foo}}");
    drop(_g);
    assert_eq!(http::substitute("{{nope}}", &ctx, 0).unwrap_err(), "undefined variable nope");
    assert_eq!(http::explain_curl(Some(28), "", "x", "http://a/"), "The server did not answer within 60 seconds.");
    assert!(http::explain_curl(Some(7), "", "this computer", "http://localhost:1/").contains("set http.runOn to \"local\"."));
}

#[test]
fn github_and_git_messages() {
    use crate::github;
    let _g = LangGuard::set("tr");
    assert!(
        github::explain_checkout("fatal: couldn't find remote ref pull/7/head", 7, "origin")
            .starts_with("#7 numaralı pull request'in dalı origin üzerinde bulunamadı")
    );
    assert!(github::explain_checkout("fatal: unable to access 'x': Could not resolve host", 7, "origin").starts_with("origin sunucusuna erişilemedi"));
    let now = github::parse_iso("2026-10-24T12:00:00Z").unwrap();
    assert_eq!(github::ago("2026-10-24T09:00:00Z", now), "3sa");
    assert_eq!(github::ago("2026-10-24T11:59:59Z", now), "şimdi");
    assert_eq!(crate::git::ago_phrase("5 minutes ago"), "5 dakika önce");
    assert_eq!(crate::git::ago_phrase("1 year ago"), "1 yıl önce");
    assert_eq!(crate::git::ago_phrase("just now"), "az önce");
    assert_eq!(crate::git::ago_phrase("whenever"), "whenever");
    let m = crate::git::explain_commit_error("Author identity unknown\n*** Please tell me who you are.");
    assert!(m.starts_with("Git bu sunucuda kim olduğunuzu bilmiyor. Terminalde şunları çalıştırın: git config --global user.name \"Your Name\" ve "), "{m}");
    assert_eq!(t("Pull failed: {}").replace("{}", "x"), "Pull başarısız: x");
    assert_eq!(t("Pulling…"), "Pull ediliyor…");
    assert_eq!(t("Synced"), "Eşitlendi");
    drop(_g);
    assert_eq!(crate::git::ago_phrase("5 minutes ago"), "5 minutes ago");
    assert_eq!(crate::git::ago_phrase("1 year ago"), "1 year ago");
    assert!(crate::git::explain_commit_error("Author identity unknown").starts_with("Git doesn't know who you are on this host."));
    assert_eq!(github::ago("2026-10-24T09:00:00Z", now), "3h");
}

#[test]
fn settings_and_keybinding_warnings() {
    let _g = LangGuard::set("tr");
    let (_, w) = crate::settings::parse_layers("{ \"files.autoSave\": \"sometimes\", \"editor.fontSize\": \"big\", \"nope\": 1 }", &[]).unwrap();
    assert!(w.contains(&"settings.jsonc: files.autoSave: bilinmeyen değer \"sometimes\" (off, afterDelay, onFocusChange)".to_string()), "{w:?}");
    assert!(w.iter().any(|x| x == "settings.jsonc: editor.fontSize: sayı bekleniyordu"), "{w:?}");
    assert!(w.iter().any(|x| x == "settings.jsonc: bilinmeyen ayar \"nope\""), "{w:?}");
    assert_eq!(crate::settings::parse_layers("[1]", &[]).unwrap_err(), "settings.jsonc: { … } biçiminde bir nesne bekleniyordu (önceki ayarlar korundu)");
    let (_, w) = crate::settings::parse_keybindings("[{ \"key\": \"cmd-k\" }]");
    assert_eq!(w, ["keybindings.jsonc: 1. girdide \"key\" ve \"command\" bulunmalıdır"]);
    drop(_g);
    let (_, w) = crate::settings::parse_layers("{ \"files.autoSave\": \"sometimes\" }", &[]).unwrap();
    assert_eq!(w, ["settings.jsonc: files.autoSave: unknown value \"sometimes\" (off, afterDelay, onFocusChange)"]);
    let (_, w) = crate::settings::parse_keybindings("[{ \"key\": \"cmd-k\" }]");
    assert_eq!(w, ["keybindings.jsonc: entry 1 needs \"key\" and \"command\""]);
}

#[test]
fn schema_trash_and_vscode_texts() {
    let _g = LangGuard::set("tr");
    assert_eq!(tf("Download a schema from {}?", &[&"json.schemastore.org"]), "json.schemastore.org sunucusundan bir şema indirilsin mi?");
    let e = crate::trash::move_to_trash(std::path::Path::new("/")).unwrap_err();
    assert_eq!(e.to_string(), "dosya adı yok");
    assert_eq!(
        tf("Imported {0} settings and {1} shortcuts from VS Code ({2} settings and {3} shortcuts have no equivalent here)", &[&4, &2, &1, &0]),
        "VS Code'dan 4 ayar ve 2 kısayol içe aktarıldı (1 ayarın ve 0 kısayolun burada karşılığı yok)"
    );
    let c = crate::vscode::convert_settings("{ \"workbench.colorTheme\": \"Monokai\" }", &[]);
    assert!(
        c.notes.iter().any(|n| n
            == "Monokai teması burada kurulu değil: kullanmak için .json dosyasını ~/.config/codeyard/themes içine kopyalayın"
                .replace("Monokai", "\"Monokai\"")
                .as_str()),
        "{:?}",
        c.notes
    );
}
