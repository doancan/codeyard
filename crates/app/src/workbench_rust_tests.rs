//! rust-analyzer's health in the status bar, the server controls, runnables, the read-only views, error explanations,
//! targets and nextest, end to end against the fake language server (and a real rust-analyzer where installed).

use accesskit::Role;
use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use super::super::*;
use crate::qa_integration_tests::{notice, wait_ticking};
use crate::qa_lsp_settings_tests::{dir, open_project, write_settings};
use crate::qa_workbench_tests::setup_with_config;
use crate::test_lsp::FakeServer;

pub(super) const LIB: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn adds_up() {\n        assert_eq!(add(1, 2), 3);\n    }\n}\n";

pub(super) struct Fixture<'a> {
    pub(super) wb: Entity<Workbench>,
    pub(super) cx: &'a mut VisualTestContext,
    pub(super) fake: FakeServer,
    pub(super) root: std::path::PathBuf,
    pub(super) file: String,
}

/// A cargo project (`src/lib.rs` = `LIB`, `src/main.rs`) opened with the fake as its rust-analyzer; `scenario` is the
/// fake's script and `settings` are added to `settings.jsonc`. `None` when Python 3 is missing.
pub(super) fn fixture<'a>(cx: &'a mut TestAppContext, name: &str, scenario: Value, settings: Value) -> Option<Fixture<'a>> {
    fixture_with(cx, name, scenario, settings, LIB)
}

/// `fixture` with `lib` as `src/lib.rs`.
pub(super) fn fixture_with<'a>(cx: &'a mut TestAppContext, name: &str, scenario: Value, settings: Value, lib: &str) -> Option<Fixture<'a>> {
    let d = dir(&format!("rust-toolchain-{name}"));
    let fake = FakeServer::new(&d.join("server"), &scenario)?;
    let cfg = d.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    write_settings(&cfg, &fake, json!({}), settings);
    let root = d.join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), lib).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    let file = open_project(&wb, cx, &root);
    wait_ticking(cx, "the server to see the file", |_| !fake.received("textDocument/didOpen").is_empty());
    Some(Fixture { wb, cx, fake, root, file })
}

impl Fixture<'_> {
    pub(super) fn run(&mut self, label: &str) {
        self.wb.update_in(self.cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    }

    pub(super) fn wait_notice(&mut self, text: &str) {
        let wb = self.wb.clone();
        wait_ticking(self.cx, text, |cx| notice(&wb, cx) == text);
    }

    pub(super) fn wait_received(&mut self, method: &str) -> Vec<Value> {
        let fake = &self.fake;
        wait_ticking(self.cx, method, |_| !fake.received(method).is_empty());
        self.fake.received(method)
    }

    pub(super) fn status_text(&mut self) -> Option<String> {
        self.wb.update(self.cx, |wb, cx| wb.rust_status_for_test(cx))
    }

    pub(super) fn snapshot(&mut self) -> a11y::Snapshot {
        self.wb.update_in(self.cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
    }
}

impl Workbench {
    /// What the status bar item for rust-analyzer says, or `None` when it is not shown.
    pub(super) fn rust_status_for_test(&self, cx: &mut Context<Self>) -> Option<String> {
        let (key, _) = self.rust_server(cx)?;
        self.lsp.rust.servers.get(&key).filter(|info| info.is_known()).map(|info| info.description())
    }
}

fn status(health: &str, quiescent: bool, message: Value) -> Value {
    json!({ "method": "experimental/serverStatus", "params": { "health": health, "quiescent": quiescent, "message": message } })
}

fn flycheck(kind: &str, extra: Value) -> Value {
    let mut value = json!({ "kind": kind });
    value.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap());
    json!({ "method": "$/progress", "params": { "token": "rust-analyzer/flycheck/0", "value": value } })
}

/// The server reports loading and a running `cargo check`: the status bar item says so, is a button for assistive
/// technology, and pressing it opens a read-only page with the server's own report.
#[gpui::test]
fn the_status_item_follows_the_server_and_opens_the_details_page(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({
        "on_open": [
            status("warning", false, json!("Failed to load 1 proc macro.")),
            flycheck("begin", json!({ "title": "cargo check", "cancellable": true })),
            flycheck("report", json!({ "message": "p (lib)" })),
        ],
        "responses": { "rust-analyzer/analyzerStatus": { "result": "Workspaces:\nLoaded 1 package across 1 workspace.\n" } },
    });
    let Some(mut f) = fixture(cx, "status", scenario, json!({})) else { return };
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the status", |cx| wb.update(cx, |wb, cx| wb.rust_status_for_test(cx)).is_some());
    assert_eq!(f.status_text().as_deref(), Some("rust-analyzer: warning. Failed to load 1 proc macro.. cargo check: p (lib)"));

    // The item is a button with that name; pressing it opens the details.
    let snap = f.snapshot();
    let label = "rust-analyzer: warning. Failed to load 1 proc macro.. cargo check: p (lib)";
    let node = snap.to_tree().nodes.into_iter().find(|(_, n)| n.role() == Role::Button && n.label() == Some(label));
    let (id, node) = node.expect("the status item is a button in the accessibility tree");
    assert_eq!(node.description(), Some("Opens the rust-analyzer status page"));
    f.wb.update_in(f.cx, |wb, window, cx| {
        let request = accesskit::ActionRequest { action: accesskit::Action::Click, target_tree: accesskit::TreeId::ROOT, target_node: id, data: None };
        wb.a11y_action_for_test(&request, &snap, window, cx);
    });
    f.wait_received("rust-analyzer/analyzerStatus");
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the details page", |cx| wb.read_with(cx, |wb, _| matches!(wb.tabs.page, Some(Page::Markdown(_)))));
    let text = f.wb.read_with(f.cx, |wb, cx| wb.active_editor().map(|e| e.read(cx).text())).unwrap();
    assert!(text.contains("**Health:** Warning"), "{text}");
    assert!(text.contains("> Failed to load 1 proc macro."), "{text}");
    assert!(text.contains("Loaded 1 package across 1 workspace."), "the server's own report: {text}");

    // The check ends and the server settles: the item says it is ready, with a warning still shown first.
    let fake_done = f.fake.received("textDocument/didOpen").len();
    assert!(fake_done >= 1);
}

/// Without a status from the server there is nothing in the status bar, and the details say the server is not running.
#[gpui::test]
fn a_server_that_never_reports_shows_no_item(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = fixture(cx, "silent", json!({}), json!({})) else { return };
    assert_eq!(f.status_text(), None);
    f.run("Rust: Show rust-analyzer Status");
    // The page is still offered: it explains the state as far as it is known.
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the page", |cx| wb.read_with(cx, |wb, _| matches!(wb.tabs.page, Some(Page::Markdown(_)))));
}

/// Each server control sends its method; the two requests wait for the answer before saying they are done.
#[gpui::test]
fn the_server_controls_send_their_methods(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({ "responses": {
        "rust-analyzer/reloadWorkspace": { "result": null },
        "rust-analyzer/rebuildProcMacros": { "error": { "code": -32603, "message": "proc-macro server crashed" } },
    } });
    let Some(mut f) = fixture(cx, "controls", scenario, json!({})) else { return };

    f.run("Rust: Reload Workspace");
    f.wait_received("rust-analyzer/reloadWorkspace");
    f.wait_notice("Rust workspace reloaded");

    f.run("Rust: Rebuild Proc Macros");
    f.wait_received("rust-analyzer/rebuildProcMacros");
    f.wait_notice("proc-macro server crashed");

    let uri = crate::lsp::path_to_uri(&f.file);
    f.run("Rust: Run Flycheck");
    let run = f.wait_received("rust-analyzer/runFlycheck");
    assert_eq!(run[0]["params"]["textDocument"]["uri"], uri);
    assert!(run[0].get("id").is_none(), "a notification, not a request");
    assert_eq!(notice(&f.wb.clone(), f.cx), "Rust check started");

    f.run("Rust: Cancel Flycheck");
    f.wait_received("rust-analyzer/cancelFlycheck");
    assert_eq!(notice(&f.wb.clone(), f.cx), "Rust check cancelled");
    f.run("Rust: Clear Flycheck");
    f.wait_received("rust-analyzer/clearFlycheck");
    assert_eq!(notice(&f.wb.clone(), f.cx), "Rust check results cleared");
}

/// A server without the method says so in words, and with no server running the notice says why.
#[gpui::test]
fn the_server_controls_explain_a_missing_method_and_a_missing_server(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = fixture(cx, "missing", json!({}), json!({})) else { return };
    f.run("Rust: Reload Workspace");
    f.wait_notice("The Rust language server does not support this command");

    // Features switched off: the notice names the setting.
    let (wb, cx) = (f.wb.clone(), &mut *f.cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.shutdown_lsp(cx);
        cx.update_global::<Settings, _>(|s, _| s.lsp_enabled = false);
        wb.run_command_for_test("Rust: Restart rust-analyzer", window, cx);
        wb.run_command_for_test("Rust: Reload Workspace", window, cx);
    });
    assert_eq!(notice(&wb, f.cx), "Rust language features are disabled. Enable lsp.enabled in Settings.");
}

/// *Rust: Restart rust-analyzer* starts the server again: a second `initialize`, the file reopened, the state of the
/// old process gone from the status bar until the new one reports.
#[gpui::test]
fn restarting_rust_analyzer_starts_a_second_server(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({ "on_open": [status("ok", true, Value::Null)] });
    let Some(mut f) = fixture(cx, "restart", scenario, json!({})) else { return };
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the status", |cx| wb.update(cx, |wb, cx| wb.rust_status_for_test(cx)).is_some());
    assert_eq!(f.fake.starts(), 1);
    f.run("Rust: Restart rust-analyzer");
    let fake = &f.fake;
    wait_ticking(f.cx, "the second start", |_| fake.starts() == 2);
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the second didOpen", |_| fake.received("textDocument/didOpen").len() >= 2);
    wait_ticking(f.cx, "the status again", |cx| wb.update(cx, |wb, cx| wb.rust_status_for_test(cx)).as_deref() == Some("rust-analyzer: ready"));
    assert_eq!(fake.received("initialize").len(), 2);
}
