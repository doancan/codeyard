//! Language server settings end to end: `lsp.servers.<key>.settings` and `initializationOptions` from `settings.jsonc`,
//! read by the scriptable fake server (what it received, how often it started), locally and over SSH, and by a real
//! rust-analyzer when one is installed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::qa_integration_tests::{notice, ssh_open_file, wait_secs, wait_ticking};
use crate::qa_workbench_tests::setup_with_config;
use crate::test_lsp::FakeServer;
use crate::workbench::Workbench;

pub(crate) fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-lsp-settings-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(d).unwrap()
}

/// `settings.jsonc` with the fake server as the `rust` server; `entry` adds keys to that entry, `top` to the file.
pub(crate) fn write_settings(cfg: &Path, fake: &FakeServer, entry: Value, top: Value) {
    let (command, args) = fake.command();
    let mut server = json!({ "command": command, "args": args });
    server.as_object_mut().unwrap().extend(entry.as_object().cloned().unwrap_or_default());
    let mut file = json!({ "lsp.servers": { "rust": server } });
    file.as_object_mut().unwrap().extend(top.as_object().cloned().unwrap_or_default());
    std::fs::write(cfg.join("settings.jsonc"), file.to_string()).unwrap();
}

/// A `workspace/configuration` request from the server, one item per section (`None`: no section).
fn ask(id: u64, sections: &[Option<&str>]) -> Value {
    let items: Vec<Value> = sections.iter().map(|s| s.map_or(json!({}), |s| json!({ "section": s }))).collect();
    json!({ "id": id, "method": "workspace/configuration", "params": { "items": items } })
}

/// Codeyard's replies to the server's request `id`, oldest first.
fn replies(fake: &FakeServer, id: u64) -> Vec<Value> {
    fake.replies().into_iter().filter(|r| r["id"] == id).map(|r| r["result"].clone()).collect()
}

/// A project with `src/lib.rs`, opened in the workbench, with the file open.
pub(crate) fn open_project(wb: &Entity<Workbench>, cx: &mut VisualTestContext, root: &Path) -> String {
    std::fs::create_dir_all(root.join("src")).unwrap();
    let file = root.join("src/lib.rs");
    if !file.exists() {
        std::fs::write(&file, "pub fn f() {}\n").unwrap();
    }
    let (root, file) = (root.to_string_lossy().into_owned(), file.to_string_lossy().into_owned());
    wb.update_in(cx, |wb, window, cx| wb.open_path(&root, window, cx));
    cx.run_until_parked();
    // `open_path` on a file makes its folder the root; this keeps the project's.
    let f = file.clone();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(f, window, cx));
    file
}

pub(crate) fn reload(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
}

/// Ticks for a while so that anything late (a second start, a second message) would have happened.
pub(crate) fn settle(cx: &mut VisualTestContext) {
    for _ in 0..10 {
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(30));
    }
}

const CLIPPY: &str = r#"{ "rust-analyzer": { "check": { "command": "clippy" } } }"#;

fn clippy() -> Value {
    serde_json::from_str(CLIPPY).unwrap()
}

/// AC1, AC2: the server reads the configured tree by section, and `initialize` carries the options unchanged.
#[gpui::test]
fn qa_lsp_settings_reach_the_server_by_section_and_initialize_carries_the_options(cx: &mut TestAppContext) {
    let d = dir("ac1");
    let scenario = json!({ "on_open": [ask(900, &[Some("rust-analyzer"), None, Some("rust-analyzer.check.command"), Some("other")])] });
    let Some(fake) = FakeServer::new(&d.join("server"), &scenario) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({ "settings": clippy(), "initializationOptions": { "cargo": { "features": ["foo"] } } }), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &d.join("project"));
    wait_ticking(cx, "the reply to workspace/configuration", |_| !replies(&fake, 900).is_empty());
    assert_eq!(replies(&fake, 900)[0], json!([{ "check": { "command": "clippy" } }, clippy(), "clippy", null]));
    let init = fake.received("initialize");
    assert_eq!(init[0]["params"]["initializationOptions"], json!({ "cargo": { "features": ["foo"] } }));
    let changed = fake.received("workspace/didChangeConfiguration");
    assert_eq!(changed.len(), 1, "once, after initialized");
    assert_eq!(changed[0]["params"]["settings"], clippy());
}

/// AC2: without `initializationOptions` the key is absent, and a server with no settings is told `{}` and reads `null`.
#[gpui::test]
fn qa_lsp_without_options_initialize_has_no_initialization_options(cx: &mut TestAppContext) {
    let d = dir("ac2");
    let Some(fake) = FakeServer::new(&d.join("server"), &json!({ "on_open": [ask(900, &[Some("rust-analyzer"), None])] })) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({}), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &d.join("project"));
    wait_ticking(cx, "the reply to workspace/configuration", |_| !replies(&fake, 900).is_empty());
    assert_eq!(replies(&fake, 900)[0], json!([null, null]));
    assert!(fake.received("initialize")[0]["params"].get("initializationOptions").is_none());
    assert_eq!(fake.received("workspace/didChangeConfiguration")[0]["params"]["settings"], json!({}));
}

/// AC3 and the "asks again" edge: a change of `settings` alone reaches the running server (`didChangeConfiguration`, and the
/// server's next question gets the new answer) without a restart.
#[gpui::test]
fn qa_lsp_a_settings_change_reaches_the_running_server_without_a_restart(cx: &mut TestAppContext) {
    let d = dir("ac3");
    let scenario = json!({ "after": { "workspace/didChangeConfiguration": [ask(901, &[Some("rust-analyzer.check")])] } });
    let Some(fake) = FakeServer::new(&d.join("server"), &scenario) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({ "settings": clippy() }), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &d.join("project"));
    wait_ticking(cx, "the first answer", |_| replies(&fake, 901).len() == 1);
    assert_eq!(replies(&fake, 901)[0], json!([{ "command": "clippy" }]));

    write_settings(&cfg, &fake, json!({ "settings": { "rust-analyzer.check.command": "check", "rust-analyzer.check.extraArgs": ["--all"] } }), json!({}));
    reload(&wb, cx);
    wait_ticking(cx, "the second answer", |_| replies(&fake, 901).len() == 2);
    let new = json!({ "check": { "command": "check", "extraArgs": ["--all"] } });
    assert_eq!(replies(&fake, 901)[1], json!([new["check"].clone()]), "the server's next question gets the new settings");
    let changed = fake.received("workspace/didChangeConfiguration");
    assert_eq!(changed.last().unwrap()["params"]["settings"], json!({ "rust-analyzer": new }));
    settle(cx);
    assert_eq!(fake.starts(), 1, "not restarted");
    assert!(fake.received("shutdown").is_empty());
    // An unrelated change of the settings file says nothing to the server.
    write_settings(
        &cfg,
        &fake,
        json!({ "settings": { "rust-analyzer.check.command": "check", "rust-analyzer.check.extraArgs": ["--all"] } }),
        json!({ "editor.tabSize": 3 }),
    );
    reload(&wb, cx);
    settle(cx);
    assert_eq!(fake.received("workspace/didChangeConfiguration").len(), 2);
}

/// AC4: `args` or `initializationOptions` changing restarts the server, exactly once each time.
#[gpui::test]
fn qa_lsp_changed_args_or_options_restart_the_server_once(cx: &mut TestAppContext) {
    let d = dir("ac4");
    let Some(fake) = FakeServer::new(&d.join("server"), &json!({})) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({}), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &d.join("project"));
    wait_ticking(cx, "the first start", |_| !fake.received("textDocument/didOpen").is_empty());

    // The script ignores arguments after its own three, so an extra one only changes `args`.
    let (_, mut args) = fake.command();
    args.push("--extra".into());
    write_settings(&cfg, &fake, json!({ "args": args }), json!({}));
    reload(&wb, cx);
    wait_ticking(cx, "the restart after args changed", |_| fake.received("textDocument/didOpen").len() == 2);
    settle(cx);
    assert_eq!(fake.starts(), 2, "args: restarted exactly once");

    write_settings(&cfg, &fake, json!({ "args": args, "initializationOptions": { "x": 1 } }), json!({}));
    reload(&wb, cx);
    wait_ticking(cx, "the restart after options changed", |_| fake.received("textDocument/didOpen").len() == 3);
    settle(cx);
    assert_eq!(fake.starts(), 3, "initializationOptions: restarted exactly once");
    assert_eq!(fake.received("initialize").last().unwrap()["params"]["initializationOptions"], json!({ "x": 1 }));
}

/// AC5: dotted keys answer exactly like the nested form.
#[gpui::test]
fn qa_lsp_dotted_settings_answer_like_nested_ones(cx: &mut TestAppContext) {
    let d = dir("ac5");
    let Some(fake) = FakeServer::new(&d.join("server"), &json!({ "on_open": [ask(900, &[Some("rust-analyzer.check"), Some("rust-analyzer")])] })) else {
        return;
    };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({ "settings": { "rust-analyzer.check.command": "clippy", "rust-analyzer.cargo.features": ["x"] } }), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &d.join("project"));
    wait_ticking(cx, "the reply", |_| !replies(&fake, 900).is_empty());
    assert_eq!(replies(&fake, 900)[0], json!([{ "command": "clippy" }, { "check": { "command": "clippy" }, "cargo": { "features": ["x"] } }]));
}

/// AC6: malformed `workspace/configuration` requests get well-formed replies, and the editor keeps working (the server's
/// next message still arrives).
#[gpui::test]
fn qa_lsp_malformed_configuration_requests_get_well_formed_replies(cx: &mut TestAppContext) {
    let d = dir("ac6");
    let diagnostic = json!({ "method": "textDocument/publishDiagnostics", "params": { "uri": "$uri", "diagnostics": [
        { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "severity": 1, "message": "still here" }
    ] } });
    let scenario = json!({ "on_open": [
        { "id": 910, "method": "workspace/configuration" },
        { "id": 911, "method": "workspace/configuration", "params": { "items": 5 } },
        { "id": 912, "method": "workspace/configuration", "params": { "items": [7, { "section": 3 }, { "section": "rust-analyzer.check.command.deeper" }] } },
        diagnostic,
    ] });
    let Some(fake) = FakeServer::new(&d.join("server"), &scenario) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({ "settings": clippy() }), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    let file = open_project(&wb, cx, &d.join("project"));
    wait_ticking(cx, "the diagnostic sent after the malformed requests", |cx| {
        wb.read_with(cx, |wb, _| wb.problems().iter().any(|(p, n)| p == &file && *n == 1))
    });
    wait_ticking(cx, "three replies", |_| [910, 911, 912].iter().all(|id| !replies(&fake, *id).is_empty()));
    assert_eq!(replies(&fake, 910)[0], json!([]));
    assert_eq!(replies(&fake, 911)[0], json!([]));
    assert_eq!(replies(&fake, 912)[0], json!([null, null, null]));
    assert!(fake.replies().iter().all(|r| r.get("error").is_none()), "{:?}", fake.replies());
}

/// AC7: an invalid `settings` value is named in the status bar in the interface language; the rest of the entry applies
/// and the server reads `null`.
#[gpui::test]
fn qa_lsp_invalid_settings_are_reported_in_the_interface_language(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let d = dir("ac7");
    let Some(fake) = FakeServer::new(&d.join("server"), &json!({ "on_open": [ask(900, &[None])] })) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({ "settings": 5 }), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    assert_eq!(notice(&wb, cx), "settings.jsonc: lsp.servers.rust.settings: expected an object");
    open_project(&wb, cx, &d.join("project"));
    wait_ticking(cx, "the reply", |_| !replies(&fake, 900).is_empty());
    assert_eq!(replies(&fake, 900)[0], json!([null]), "the entry still applies, without settings");

    // The language applies once the file that sets it has loaded; the warnings of the next load are in it.
    write_settings(&cfg, &fake, json!({ "settings": clippy() }), json!({ "ui.language": "tr" }));
    reload(&wb, cx);
    write_settings(&cfg, &fake, json!({ "settings": [1] }), json!({ "ui.language": "tr" }));
    reload(&wb, cx);
    assert_eq!(notice(&wb, cx), "settings.jsonc: lsp.servers.rust.settings: bir nesne bekleniyordu");
}

/// AC8: an untrusted folder's own settings cannot start a server or send it settings; the status bar names the key.
#[gpui::test]
fn qa_lsp_an_untrusted_project_cannot_configure_a_server(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let d = dir("ac8");
    let Some(fake) = FakeServer::new(&d.join("server"), &json!({})) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "security.workspaceTrust.enabled": true }"#).unwrap();
    let project = d.join("project");
    std::fs::create_dir_all(project.join(".codeyard")).unwrap();
    let (command, args) = fake.command();
    let entry = json!({ "lsp.servers": { "rust": { "command": command, "args": args, "settings": clippy(), "initializationOptions": {} } } });
    std::fs::write(project.join(".codeyard/settings.jsonc"), entry.to_string()).unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    let root = project.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&root, window, cx));
    cx.run_until_parked();
    let n = notice(&wb, cx);
    assert!(n.contains("“lsp.servers” is ignored until you trust this folder"), "{n}");
    assert!(cx.update(|_, cx| cx.global::<crate::settings::Settings>().lsp_servers.is_empty()));
    open_project(&wb, cx, &project);
    settle(cx);
    assert_eq!(fake.starts(), 0, "no server started");
    assert!(fake.messages().is_empty(), "nothing was sent");
}

/// AC9: over SSH the server runs on the host and gets the same options, answers and changes.
#[gpui::test]
fn qa_lsp_settings_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("lsp-settings") else { return };
    let d = dir("ac9");
    let scenario = json!({ "after": { "workspace/didChangeConfiguration": [ask(901, &[Some("rust-analyzer")])] } });
    let Some(fake) = FakeServer::new(&d.join("server"), &scenario) else { return };
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({ "settings": clippy(), "initializationOptions": { "cargo": { "features": ["foo"] } } }), json!({}));
    let (wb, cx) = setup_with_config(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    wait_secs(cx, 30, "connected", |cx| {
        cx.executor().advance_clock(Duration::from_secs(1));
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    let project = d.join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    let root = project.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&root, window, cx));
    cx.run_until_parked();
    ssh_open_file(&wb, cx, &project.join("src/lib.rs"));
    wait_secs(cx, 30, "the first answer", |cx| {
        cx.executor().advance_clock(Duration::from_millis(500));
        replies(&fake, 901).len() == 1
    });
    assert_eq!(replies(&fake, 901)[0], json!([{ "check": { "command": "clippy" } }]));
    assert_eq!(fake.received("initialize")[0]["params"]["initializationOptions"], json!({ "cargo": { "features": ["foo"] } }));

    write_settings(
        &cfg,
        &fake,
        json!({ "settings": { "rust-analyzer.check.command": "check" }, "initializationOptions": { "cargo": { "features": ["foo"] } } }),
        json!({}),
    );
    reload(&wb, cx);
    wait_secs(cx, 30, "the second answer", |cx| {
        cx.executor().advance_clock(Duration::from_millis(500));
        replies(&fake, 901).len() == 2
    });
    assert_eq!(replies(&fake, 901)[1], json!([{ "check": { "command": "check" } }]));
    assert_eq!(fake.starts(), 1, "not restarted");
}

// --- Real rust-analyzer (skipped when it is not installed) ---

pub(crate) fn rust_analyzer_installed() -> bool {
    let ok = |cmd: &str| std::process::Command::new(cmd).arg("--version").output().is_ok_and(|o| o.status.success());
    if ok("rust-analyzer") && ok("cargo") {
        return true;
    }
    crate::test_sshd::skip_missing("rust-analyzer,cargo");
    false
}

/// A one-crate Cargo project with `src/lib.rs`, its own target folder, and the user settings in `cfg`. The `rust` entry
/// has no `command`: the default server (`rust-analyzer`) starts with the settings.
pub(crate) fn cargo_project(name: &str, lib: &str, features: &str) -> (PathBuf, PathBuf) {
    let d = dir(name);
    let project = d.join(name);
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\n{features}\n"))
        .unwrap();
    std::fs::write(project.join("src/lib.rs"), lib).unwrap();
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    (project, cfg)
}

fn write_rust_settings(cfg: &Path, settings: Value) {
    let file = json!({ "lsp.servers": { "rust": { "settings": { "rust-analyzer": settings } } } });
    std::fs::write(cfg.join("settings.jsonc"), file.to_string()).unwrap();
}

/// An edit and a save: an unchanged file is not written, so the server would not be told and would not check again.
pub(crate) fn save(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| {
        let end = e.text().len();
        e.insert_at_for_test(end, "\n", cx);
        e.save_now(cx);
    });
}

pub(crate) fn has_diagnostic(wb: &Entity<Workbench>, cx: &mut VisualTestContext, file: &str, what: impl Fn(&crate::lsp::Diagnostic) -> bool) -> bool {
    wb.read_with(cx, |wb, _| wb.lsp_diagnostics_for_test(file).iter().any(what))
}

/// Waits (saving now and then, which runs the check) until `done`; the first check of a crate builds it, so it can take
/// a while.
pub(crate) fn wait_checked(wb: &Entity<Workbench>, cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    let mut n = 0u32;
    wait_secs(cx, 180, what, |cx| {
        n += 1;
        if n % 40 == 1 {
            save(wb, cx);
        }
        std::thread::sleep(Duration::from_millis(100));
        cx.executor().advance_clock(Duration::from_millis(300));
        done(cx)
    });
}

/// AC10: code behind `#[cfg(feature = "x")]` is analysed only when `cargo.features` names it; changing the setting while
/// the server runs takes effect after the next save.
#[gpui::test]
fn qa_lsp_real_rust_analyzer_analyses_code_behind_the_configured_feature(cx: &mut TestAppContext) {
    if !rust_analyzer_installed() {
        return;
    }
    let lib = "#[cfg(feature = \"x\")]\npub fn f() {\n    let _: u32 = \"s\";\n}\n";
    let (project, cfg) = cargo_project("cy_features", lib, "x = []");
    write_rust_settings(&cfg, json!({ "cargo": { "features": ["x"] } }));
    let (wb, cx) = setup_with_config(cx, &cfg);
    let file = open_project(&wb, cx, &project);
    let mismatch = |d: &crate::lsp::Diagnostic| d.severity == 1 && d.message.contains("mismatched types");
    wait_checked(&wb, cx, "the type error behind the feature", |cx| has_diagnostic(&wb, cx, &file, mismatch));

    write_rust_settings(&cfg, json!({ "cargo": { "features": [] } }));
    reload(&wb, cx);
    wait_checked(&wb, cx, "the error to go once the feature is off", |cx| !has_diagnostic(&wb, cx, &file, mismatch));
}

/// AC11: with the default check a save shows the compiler's warnings and no Clippy lint; with `check.command = "clippy"`
/// the `needless_return` lint appears after saving. The compiler's own warning shows when a check has finished.
#[gpui::test]
fn qa_lsp_real_rust_analyzer_runs_clippy_when_configured(cx: &mut TestAppContext) {
    if !rust_analyzer_installed() {
        return;
    }
    let (project, cfg) = cargo_project("cy_clippy", "pub fn f() -> i32 {\n    let unused = 1;\n    return 1;\n}\n", "");
    std::fs::write(cfg.join("settings.jsonc"), "{}").unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    let file = open_project(&wb, cx, &project);
    let checked = |d: &crate::lsp::Diagnostic| d.source.as_deref() == Some("rustc") && d.message.contains("unused");
    let lint = |d: &crate::lsp::Diagnostic| d.source.as_deref() == Some("clippy") && d.message.contains("return");
    wait_checked(&wb, cx, "the default check to finish", |cx| has_diagnostic(&wb, cx, &file, checked));
    assert!(!has_diagnostic(&wb, cx, &file, lint), "no Clippy lint with the default settings");

    write_rust_settings(&cfg, json!({ "check": { "command": "clippy" } }));
    reload(&wb, cx);
    wait_checked(&wb, cx, "the needless_return lint", |cx| has_diagnostic(&wb, cx, &file, lint));
}
