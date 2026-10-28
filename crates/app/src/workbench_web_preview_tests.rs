use super::*;

/// Driven by scripts/test-web-preview-browser.mjs against a real headless Chromium browser.
#[gpui::test]
#[ignore = "run with the browser acceptance driver; see docs/web-preview.md"]
fn web_preview_browser_acceptance(cx: &mut gpui::TestAppContext) {
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let ready = target.join("web-preview-browser.json");
    let done = target.join("web-preview-browser-done.json");
    let _ = std::fs::remove_file(&ready);
    let _ = std::fs::remove_file(&done);
    let root = crate::qa_workbench_tests::fresh("preview-browser");
    std::fs::write(root.join("index.html"), "<!doctype html><link rel=stylesheet href=/style.css><h1>Before</h1><script src=/app.js></script>").unwrap();
    std::fs::write(root.join("style.css"), "h1 { color: rgb(255, 0, 0); }").unwrap();
    std::fs::write(root.join("app.js"), "document.documentElement.dataset.script = 'before';").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    cx.update(|_, cx| {
        let settings = cx.global_mut::<Settings>();
        settings.web_preview_enabled = true;
        settings.web_preview.reload_interval = 500;
    });
    crate::qa_workbench_tests::open(&wb, &root, cx);
    crate::qa_workbench_tests::open_in(&wb, &root.join("index.html"), cx);
    wb.update_in(cx, |wb, window, cx| wb.open_web_preview(window, cx));
    crate::qa_integration_tests::wait_secs(cx, 10, "browser fixture ready", |cx| wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.is_some()));
    let url = wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.as_ref().unwrap().url("index.html"));
    std::fs::write(&ready, serde_json::json!({"url": url, "root": crate::ui::local_path(&root)}).to_string()).unwrap();
    crate::qa_integration_tests::wait_secs(cx, 120, "browser acceptance driver", |_| done.exists());
    let result: serde_json::Value = serde_json::from_slice(&std::fs::read(&done).unwrap()).unwrap();
    wb.update(cx, |wb, cx| wb.stop_web_preview(cx));
    std::fs::remove_file(ready).unwrap();
    assert_eq!(result["passed"], true, "{result}");
}

#[gpui::test]
fn web_preview_toggle_stops_listener_and_workspace_switch_cancels_start(cx: &mut gpui::TestAppContext) {
    let config = crate::qa_workbench_tests::fresh("preview-settings");
    let settings = config.join("settings.jsonc");
    std::fs::write(&settings, r#"{"modules.webPreview.enabled":false}"#).unwrap();
    let root = crate::qa_workbench_tests::fresh("preview-workspace");
    std::fs::write(root.join("index.html"), "<h1>Preview</h1>").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &config);
    crate::qa_workbench_tests::open(&wb, &root, cx);
    crate::qa_workbench_tests::open_in(&wb, &root.join("index.html"), cx);
    wb.update_in(cx, |wb, window, cx| wb.open_web_preview(window, cx));
    assert!(wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.is_none() && !wb.chrome.web_preview.starting));
    std::fs::write(&settings, r#"{"modules.webPreview.enabled":true,"security.workspaceTrust.enabled":false}"#).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("html-preview").is_some());
    assert!(cx.debug_bounds("web-preview-open").is_some());
    let preview_button = cx.debug_bounds("html-preview").unwrap().center();
    cx.simulate_click(preview_button, gpui::Modifiers::none());
    crate::qa_integration_tests::wait_secs(cx, 10, "preview running", |cx| wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.is_some()));
    let port = wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.as_ref().unwrap().port());
    assert!(std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).is_ok());
    std::fs::write(&settings, r#"{"modules.webPreview.enabled":false}"#).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    assert!(wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.is_none()));
    assert!(std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).is_err());
    cx.update(|_, cx| cx.global_mut::<Settings>().web_preview_enabled = true);
    wb.update_in(cx, |wb, window, cx| {
        wb.open_web_preview(window, cx);
        wb.set_root(ui::local_path(&crate::qa_workbench_tests::fresh("preview-another")), cx);
    });
    for _ in 0..10 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.is_none() && !wb.chrome.web_preview.starting));
}

#[gpui::test]
fn web_preview_settings_restart_a_running_server_and_apply_the_selected_root(cx: &mut gpui::TestAppContext) {
    use crate::web_preview::tests::request;
    let config = crate::qa_workbench_tests::fresh("preview-options-config");
    std::fs::write(config.join("settings.jsonc"), r#"{"modules.webPreview.enabled":true,"security.workspaceTrust.enabled":false}"#).unwrap();
    let root = crate::qa_workbench_tests::fresh("preview-options-root");
    std::fs::create_dir(root.join("public")).unwrap();
    std::fs::write(root.join("index.html"), "workspace index").unwrap();
    std::fs::write(root.join("public/index.html"), "public index").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &config);
    crate::qa_workbench_tests::open(&wb, &root, cx);
    crate::qa_workbench_tests::open_in(&wb, &root.join("index.html"), cx);
    wb.update_in(cx, |wb, window, cx| wb.open_web_preview(window, cx));
    crate::qa_integration_tests::wait_secs(cx, 10, "preview running", |cx| wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.is_some()));
    let port = wb.read_with(cx, |wb, _| {
        let server = wb.chrome.web_preview.server.as_ref().unwrap();
        assert!(request(server, "GET", "/", true, None).contains("workspace index"));
        server.port()
    });
    wb.update(cx, |wb, cx| wb.copy_web_preview_url(cx));
    let copied = cx.read_from_clipboard().and_then(|c| c.text()).unwrap();
    assert!(copied.starts_with(&format!("http://127.0.0.1:{port}/index.html?__codeyard_token=")));
    // Another window may have applied global settings before this window reloads its own server.
    cx.update(|_, cx| cx.global_mut::<Settings>().web_preview.port = port);
    wb.update_in(cx, |wb, window, cx| wb.write_setting("webPreview.port", serde_json::json!(port), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 10, "preview on fixed port", |cx| {
        wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.as_ref().is_some_and(|s| s.config().port == port && s.port() == port))
    });
    wb.update_in(cx, |wb, window, cx| wb.write_setting("webPreview.reloadInterval", serde_json::json!(500), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 10, "preview interval applied", |cx| {
        wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.as_ref().is_some_and(|s| s.config().reload_interval == 500))
    });
    wb.read_with(cx, |wb, _| {
        let server = wb.chrome.web_preview.server.as_ref().unwrap();
        assert_eq!(server.port(), port);
        assert!(request(server, "GET", "/", true, None).contains("data-interval=\"500\""));
    });
    wb.update_in(cx, |wb, window, cx| wb.write_setting("webPreview.liveReload", serde_json::json!(false), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 10, "live reload disabled", |cx| {
        wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.as_ref().is_some_and(|s| !s.config().live_reload))
    });
    wb.update_in(cx, |wb, window, cx| wb.write_setting("webPreview.root", serde_json::json!("public"), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 10, "preview root applied", |cx| {
        wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.as_ref().is_some_and(|s| s.root() == crate::ui::local_path(&root.join("public"))))
    });
    wb.read_with(cx, |wb, _| {
        let server = wb.chrome.web_preview.server.as_ref().unwrap();
        let html = request(server, "GET", "/", true, None);
        assert!(html.contains("public index"), "{html}");
        assert!(!html.contains("/__codeyard_live.js"));
        assert_eq!(server.port(), port);
    });
    wb.update_in(cx, |wb, window, cx| wb.write_setting("webPreview.root", serde_json::Value::Null, window, cx));
    crate::qa_integration_tests::wait_secs(cx, 10, "automatic root restored", |cx| {
        wb.read_with(cx, |wb, _| wb.chrome.web_preview.server.as_ref().is_some_and(|s| s.root() == crate::ui::local_path(&root)))
    });
    wb.update(cx, |wb, cx| wb.stop_web_preview(cx));
    let saved = std::fs::read_to_string(config.join("settings.jsonc")).unwrap();
    let (settings, warnings) = crate::settings::parse_layers_trusted(&saved, &[], true).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.web_preview.port, port);
    assert_eq!(settings.web_preview.reload_interval, 500);
    assert!(!settings.web_preview.live_reload);
}
