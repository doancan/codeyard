//! *Rust: Expand Macro Recursively*, *Rust: Locate Parent Module* and *Rust: Open Cargo.toml* end to end: what the fake
//! server receives, what opens for each kind of answer, and the notices; then against a real rust-analyzer.

use std::path::Path;

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::qa_integration_tests::{notice, wait_ticking};
use crate::qa_lsp_settings_tests::{cargo_project, dir, open_project, rust_analyzer_installed, settle, wait_checked, write_settings};
use crate::qa_workbench_tests::setup_with_config;
use crate::test_lsp::FakeServer;
use crate::workbench::Workbench;

/// `ğ😀` before the macro: the cursor on `e` of `vec!` is character 14, UTF-16 column 15.
const LIB: &str = "pub fn f() {\n    let ğ😀 = vec![1, 2];\n}\n";

const EXPAND: &str = "Rust: Expand Macro Recursively";
const PARENT: &str = "Rust: Locate Parent Module";
const CARGO: &str = "Rust: Open Cargo.toml";

struct Fixture<'a> {
    wb: Entity<Workbench>,
    cx: &'a mut VisualTestContext,
    fake: FakeServer,
    root: std::path::PathBuf,
    file: String,
}

/// A project whose `src/lib.rs` is open with the cursor on `vec!`, served by the fake with `responses`; the settings live
/// in a folder of the test's own (scratches go there too).
fn fixture<'a>(cx: &'a mut TestAppContext, name: &str, responses: Value) -> Option<Fixture<'a>> {
    fixture_in(cx, name, responses, "en")
}

/// `fixture` with the interface in `language` (the settings file sets it when it loads).
fn fixture_in<'a>(cx: &'a mut TestAppContext, name: &str, responses: Value, language: &str) -> Option<Fixture<'a>> {
    let d = dir(name);
    let fake = FakeServer::new(
        &d.join("server"),
        &json!({ "responses": responses, "on_open": [{ "method": "textDocument/publishDiagnostics", "params": { "uri": "$uri", "diagnostics": [
        { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "severity": 2, "message": "from the project's server" }
    ] } }] }),
    )?;
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({}), json!({ "ui.language": language }));
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), LIB).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"p\"\nversion = \"0.1.0\"\n").unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    let file = open_project(&wb, cx, &root);
    wait_ticking(cx, "the server to see the file", |_| !fake.received("textDocument/didOpen").is_empty());
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(1, 14, cx));
    Some(Fixture { wb, cx, fake, root, file })
}

impl Fixture<'_> {
    fn run(&mut self, label: &str) {
        self.wb.update_in(self.cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    }

    fn active_path(&mut self) -> Option<String> {
        self.wb.read_with(self.cx, |wb, cx| wb.active_path(cx))
    }

    fn wait_notice(&mut self, text: &str) {
        let wb = self.wb.clone();
        wait_ticking(self.cx, text, |cx| notice(&wb, cx) == text);
    }

    fn wait_active(&mut self, what: &str, done: impl Fn(&crate::editor::EditorView) -> bool) {
        let wb = self.wb.clone();
        wait_ticking(self.cx, what, |cx| wb.read_with(cx, |wb, _| wb.active_editor_entity()).is_some_and(|ed| ed.read_with(cx, |e, _| done(e))));
    }
}

fn location(path: &Path, line: u32) -> Value {
    json!({ "uri": crate::lsp::path_to_uri(&path.to_string_lossy()), "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": 7 } } })
}

/// AC1: each command sends its method with the file's URI and, where it takes one, the UTF-16 position.
#[gpui::test]
fn qa_rust_commands_send_the_method_uri_and_utf16_position(cx: &mut TestAppContext) {
    let Some(mut f) = fixture(cx, "ac1", json!({})) else { return };
    let uri = crate::lsp::path_to_uri(&f.file);
    for (label, method) in [(EXPAND, "rust-analyzer/expandMacro"), (PARENT, "experimental/parentModule"), (CARGO, "experimental/openCargoToml")] {
        f.run(label);
        let fake = &f.fake;
        wait_ticking(f.cx, method, |_| !fake.received(method).is_empty());
        let params = &f.fake.received(method)[0]["params"];
        assert_eq!(params["textDocument"]["uri"], uri, "{method}");
        if method == "experimental/openCargoToml" {
            assert!(params.get("position").is_none());
        } else {
            assert_eq!(params["position"], json!({ "line": 1, "character": 15 }), "{method}");
        }
    }
}

/// AC2, AC6: the expansion opens as a Rust scratch tab named after the macro (a second one gets its own name), and the
/// project's server never sees it, so it has no diagnostics.
#[gpui::test]
fn qa_rust_expand_macro_opens_a_scratch_without_diagnostics(cx: &mut TestAppContext) {
    let expansion = "<[_]>::into_vec(\n    #[rustc_box]\n    ::alloc::boxed::Box::new([1, 2]),\n)";
    let Some(mut f) = fixture(cx, "ac2", json!({ "rust-analyzer/expandMacro": { "result": { "name": "vec", "expansion": expansion } } })) else { return };
    f.run(EXPAND);
    f.wait_active("the expansion tab", |e| e.path.ends_with("/scratches/vec!.expansion.rs"));
    let scratch = f.active_path().unwrap();
    let ed = f.wb.read_with(f.cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert_eq!(ed.read_with(f.cx, |e, _| (e.text(), e.language())), (expansion.to_string(), "Rust"));
    assert!(!scratch.starts_with(&*f.root.to_string_lossy()), "outside the project");

    // The same macro again: a second scratch.
    let file = f.file.clone();
    f.wb.update_in(f.cx, |wb, window, cx| wb.open_file_in_root(file, window, cx));
    let ed = f.wb.read_with(f.cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(1, 14, cx));
    f.run(EXPAND);
    f.wait_active("the second expansion tab", |e| e.path.ends_with("/scratches/vec!.expansion 2.rs"));

    settle(f.cx);
    let opened: Vec<String> =
        f.fake.received("textDocument/didOpen").iter().map(|m| m["params"]["textDocument"]["uri"].as_str().unwrap_or_default().to_string()).collect();
    assert!(opened.iter().all(|u| !u.contains("expansion")), "the server never saw the scratch: {opened:?}");
    let problems = f.wb.read_with(f.cx, |wb, _| wb.problems());
    assert!(problems.iter().all(|(p, _)| !p.contains("expansion")), "{problems:?}");
    assert!(problems.iter().any(|(p, _)| p == &f.file), "the project's file still has its diagnostic");
}

/// AC3: one parent module opens it.
#[gpui::test]
fn qa_rust_parent_module_opens_the_only_result(cx: &mut TestAppContext) {
    let d = dir("ac3-one-target");
    std::fs::create_dir_all(&d).unwrap();
    let parent = d.join("main.rs");
    std::fs::write(&parent, "fn main() {}\n\nmod a;\nmod lib;\n").unwrap();
    let Some(mut f) = fixture(cx, "ac3-one", json!({ "experimental/parentModule": { "result": [location(&parent, 3)] } })) else { return };
    f.run(PARENT);
    let p = crate::ui::local_path(&parent);
    f.wait_active("the parent module at its mod line", |e| e.path == p && e.cursor_position().0 == 4);
}

/// AC3: several parent modules open the location list.
#[gpui::test]
fn qa_rust_parent_module_lists_several_results(cx: &mut TestAppContext) {
    let d = dir("ac3-many-targets");
    std::fs::create_dir_all(&d).unwrap();
    let (a, b) = (d.join("a.rs"), d.join("b.rs"));
    std::fs::write(&a, "mod lib;\n").unwrap();
    std::fs::write(&b, "\nmod lib;\n").unwrap();
    let Some(mut f) = fixture(cx, "ac3-many", json!({ "experimental/parentModule": { "result": [location(&a, 0), location(&b, 1)] } })) else { return };
    f.run(PARENT);
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the location list", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some_and(|l| l.len() == 2));
    assert_eq!(f.wb.read_with(f.cx, |wb, cx| wb.overlay_labels(cx)).unwrap(), ["mod lib;", "mod lib;"]);
}

/// AC3: no parent module says so.
#[gpui::test]
fn qa_rust_parent_module_says_when_there_is_none(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = fixture(cx, "ac3-none", json!({ "experimental/parentModule": { "result": [] } })) else { return };
    f.run(PARENT);
    f.wait_notice("No parent module found");
}

/// AC4: the reported `Cargo.toml` opens at the reported line.
#[gpui::test]
fn qa_rust_open_cargo_toml_opens_the_manifest(cx: &mut TestAppContext) {
    let Some(mut f) = fixture(cx, "ac4", json!({})) else { return };
    let manifest = f.root.join("Cargo.toml");
    let scenario = json!({ "responses": { "experimental/openCargoToml": { "result": location(&manifest, 2) } } });
    // The server reads its scenario when it starts: restart it with the answer.
    std::fs::write(f.root.parent().unwrap().join("server/scenario.json"), scenario.to_string()).unwrap();
    f.run("Developer: Restart Language Servers");
    let fake = &f.fake;
    wait_ticking(f.cx, "the restart", |_| fake.starts() == 2 && fake.received("textDocument/didOpen").len() == 2);
    f.run(CARGO);
    let m = crate::ui::local_path(&manifest);
    f.wait_active("Cargo.toml at line 3", |e| e.path == m && e.cursor_position().0 == 3);
}

/// AC5: `null`, "method not found" and no answer each have their notice, in English and Turkish.
#[gpui::test]
fn qa_rust_command_failures_have_notices_in_english(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let responses =
        json!({ "rust-analyzer/expandMacro": { "result": null }, "experimental/openCargoToml": { "error": { "code": -32601, "message": "unknown request" } } });
    let Some(mut f) = fixture(cx, "ac5-en", responses) else { return };
    f.run(EXPAND);
    f.wait_notice("No macro at the cursor");
    f.run(CARGO);
    f.wait_notice("The Rust language server does not support this command");
}

#[gpui::test]
fn qa_rust_command_failures_have_notices_in_turkish(cx: &mut TestAppContext) {
    let _tr = crate::i18n::LangGuard::set("tr");
    let responses = json!({
        "rust-analyzer/expandMacro": { "result": null },
        "experimental/openCargoToml": { "error": { "code": -32601, "message": "unknown request" } },
        "experimental/parentModule": "silent",
    });
    let Some(mut f) = fixture_in(cx, "ac5-tr", responses, "tr") else { return };
    f.run(EXPAND);
    f.wait_notice("İmleçte makro yok");
    f.run(CARGO);
    f.wait_notice("Rust dil sunucusu bu komutu desteklemiyor");
    // No answer: the request gives up after its timeout with the existing message.
    f.run(PARENT);
    let (cmd, _) = f.fake.command();
    let expected = format!("experimental/parentModule: {cmd} yanıt vermedi");
    let wb = f.wb.clone();
    crate::qa_integration_tests::wait_secs(f.cx, 40, "the timeout notice", |cx| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        notice(&wb, cx) == expected
    });
}

/// AC7: in a file of another language nothing is sent and the notice says what to do.
#[gpui::test]
fn qa_rust_commands_in_a_python_file_send_nothing(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = fixture(cx, "ac7", json!({})) else { return };
    let py = crate::ui::local_path(&f.root.join("tool.py"));
    std::fs::write(&py, "print('merhaba')\n").unwrap();
    f.wb.update_in(f.cx, |wb, window, cx| wb.open_file_in_root(py.clone(), window, cx));
    f.wait_active("the Python file", |e| e.path == py);
    for label in [EXPAND, PARENT, CARGO] {
        f.run(label);
        f.wait_notice("Open a Rust file first");
    }
    settle(f.cx);
    for method in ["rust-analyzer/expandMacro", "experimental/parentModule", "experimental/openCargoToml"] {
        assert!(f.fake.received(method).is_empty(), "{method} was sent");
    }
}

/// AC8: against a real rust-analyzer: `vec![1, 2]` expands, the parent module of `src/foo.rs` is the `mod foo;` line of
/// `src/lib.rs`, and the `Cargo.toml` of a member crate is that member's own.
#[gpui::test]
fn qa_rust_commands_against_a_real_rust_analyzer(cx: &mut TestAppContext) {
    if !rust_analyzer_installed() {
        return;
    }
    let lib = "//! Two crates.\n\nmod foo;\n\npub fn f() -> Vec<i32> {\n    vec![1, 2]\n}\n";
    let (a, cfg) = cargo_project("cy_cmds", lib, "");
    std::fs::write(a.join("src/foo.rs"), "pub fn g() {}\n").unwrap();
    // A workspace around it with a second member, `b`.
    let ws = a.parent().unwrap().to_path_buf();
    std::fs::write(ws.join("Cargo.toml"), "[workspace]\nmembers = [\"cy_cmds\", \"b\"]\nresolver = \"2\"\n").unwrap();
    std::fs::create_dir_all(ws.join("b/src")).unwrap();
    std::fs::write(ws.join("b/Cargo.toml"), "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(ws.join("b/src/lib.rs"), "pub fn h() {}\n").unwrap();
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "lsp.enabled": true }"#).unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    open_project(&wb, cx, &ws);
    let open = |wb: &Entity<Workbench>, cx: &mut VisualTestContext, path: &Path, line: usize, col: usize| {
        let p = crate::ui::local_path(path);
        wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p.clone(), window, cx));
        wait_ticking(cx, "the file", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).as_deref() == Some(&p));
        let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
        ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(line, col, cx));
    };
    let run = |wb: &Entity<Workbench>, cx: &mut VisualTestContext, label: &str| wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    let active = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| {
        wb.read_with(cx, |wb, _| wb.active_editor_entity()).map(|ed| ed.read_with(cx, |e, _| (e.path.clone(), e.cursor_position(), e.text())))
    };

    // Expand `vec!` (retried while the server is still loading the workspace).
    let lib_rs = a.join("src/lib.rs");
    open(&wb, cx, &lib_rs, 5, 5);
    let mut n = 0;
    wait_checked(&wb, cx, "the expansion", |cx| {
        n += 1;
        if n % 20 == 1 {
            let p = crate::ui::local_path(&lib_rs);
            if wb.read_with(cx, |wb, cx| wb.active_path(cx)).as_deref() == Some(&p) {
                run(&wb, cx, EXPAND);
            }
        }
        active(&wb, cx).is_some_and(|(p, _, text)| p.ends_with("/scratches/vec!.expansion.rs") && !text.trim().is_empty())
    });

    // The parent module of src/foo.rs: the `mod foo;` line (3) of src/lib.rs.
    open(&wb, cx, &a.join("src/foo.rs"), 0, 4);
    run(&wb, cx, PARENT);
    let lib_path = crate::ui::local_path(&lib_rs);
    wait_ticking(cx, "src/lib.rs at mod foo;", |cx| active(&wb, cx).is_some_and(|(p, (line, _), _)| p == lib_path && line == 3));

    // The Cargo.toml of member b, not the workspace's.
    open(&wb, cx, &ws.join("b/src/lib.rs"), 0, 4);
    run(&wb, cx, CARGO);
    let manifest = crate::ui::local_path(&ws.join("b/Cargo.toml"));
    wait_ticking(cx, "b/Cargo.toml", |cx| active(&wb, cx).is_some_and(|(p, _, _)| p == manifest));
}

/// Over SSH the request goes to the server on the host and the location it answers opens there.
#[gpui::test]
fn qa_rust_parent_module_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("rust-commands") else { return };
    let d = dir("ssh");
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "mod foo;\n").unwrap();
    std::fs::write(root.join("src/foo.rs"), "pub fn g() {}\n").unwrap();
    let Some(fake) =
        FakeServer::new(&d.join("server"), &json!({ "responses": { "experimental/parentModule": { "result": location(&root.join("src/lib.rs"), 0) } } }))
    else {
        return;
    };
    let settings = crate::settings::Settings { lsp_enabled: true, lsp_servers: vec![("rust".into(), Some(fake.command().into()))], ..Default::default() };
    let (wb, cx) = crate::qa_integration_tests::setup_with(cx, settings);
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 30, "connected", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    let r = crate::ui::local_path(&root);
    wb.update_in(cx, |wb, window, cx| wb.open_path(&r, window, cx));
    cx.run_until_parked();
    let ed = crate::qa_integration_tests::ssh_open_file(&wb, cx, &root.join("src/foo.rs"));
    crate::qa_integration_tests::wait_secs(cx, 30, "the server", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        ed.read_with(cx, |e, _| e.lsp_client().is_some())
    });
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(PARENT, window, cx));
    let lib = crate::ui::local_path(&root.join("src/lib.rs"));
    crate::qa_integration_tests::wait_secs(cx, 30, "src/lib.rs opened on the host", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        wb.read_with(cx, |wb, cx| wb.active_path(cx)).as_deref() == Some(&lib)
    });
    assert_eq!(fake.received("experimental/parentModule").len(), 1);
}
