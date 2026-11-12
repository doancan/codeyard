//! Runnables end to end: the gutter marks, the palette command, the list, the task that runs, the fallback without a
//! server and `rust.target`.

use accesskit::Role;
use gpui::{Entity, TestAppContext};
use serde_json::{Value, json};

use super::super::*;
use super::tests::{Fixture, fixture, fixture_with};
use crate::qa_integration_tests::wait_ticking;
use crate::qa_lsp_settings_tests::dir;

impl Workbench {
    /// The label and command line of the command that would have run last (tests never start Cargo).
    pub(super) fn last_task_for_test(&self) -> Option<(String, String)> {
        self.lsp.rust.ran.last().cloned()
    }
}

/// The real project rust-analyzer 0.3.3113 listed runnables for (`testdata/rust-analyzer/runnables-lib.json`).
const DEMO_LIB: &str = "/// Adds two numbers.\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn unsafe_read(p: *const i32) -> i32 {\n    unsafe { *p }\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn adds_up() {\n        assert_eq!(add(1, 2), 3);\n    }\n\n    #[test]\n    fn fails_on_purpose() {\n        assert_eq!(add(1, 2), 4);\n    }\n\n    #[test]\n    #[ignore = \"slow\"]\n    fn ignored_one() {}\n}\n";

fn real_runnables(root: &std::path::Path) -> Value {
    let text = std::fs::read_to_string(format!("{}/testdata/rust-analyzer/runnables-lib.json", env!("CARGO_MANIFEST_DIR"))).unwrap();
    serde_json::from_str(&text.replace("/work/demo", &root.to_string_lossy().replace('\\', "/"))).unwrap()
}

/// `fixture` with the real runnables of the demo library as the server's answer.
fn runnables_fixture<'a>(cx: &'a mut TestAppContext, name: &str, settings: Value) -> Option<Fixture<'a>> {
    // The server names locations by the project's path, which is known before the project is made.
    let root = dir(&format!("rust-toolchain-{name}")).join("project");
    let scenario = json!({
        "capabilities": { "experimental": { "runnables": { "kinds": ["cargo"] } } },
        "responses": { "experimental/runnables": { "result": real_runnables(&root) } },
    });
    fixture_with(cx, name, scenario, settings, DEMO_LIB)
}

fn editor(f: &mut Fixture) -> Entity<crate::editor::EditorView> {
    f.wb.read_with(f.cx, |wb, _| wb.active_editor_entity()).unwrap()
}

/// With rust-analyzer: the gutter has a mark at each runnable's name, the palette command offers Run and Debug for the
/// runnables around the cursor (innermost first), the first row types its command into the terminal, a mark opens the list
/// for its own line, and nothing is offered where nothing runs.
#[cfg(unix)]
#[gpui::test]
fn runnables_from_rust_analyzer_run_from_the_palette_and_the_gutter(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = runnables_fixture(cx, "runnables", json!({})) else { return };
    let ed = editor(&mut f);
    ed.update(f.cx, |e, cx| {
        e.set_cursor_line_col_for_test(14, 8, cx);
        e.schedule_lsp_extras(0, cx);
    });
    wait_ticking(f.cx, "the marks", |cx| ed.read_with(cx, |e, _| e.run_mark_lines()) == [10, 14, 19, 25]);

    f.run("Rust: Run Runnable at Cursor");
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the list", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some());
    let labels = f.wb.read_with(f.cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert_eq!(labels, ["Run test tests::adds_up", "Debug test tests::adds_up", "Run test-mod tests", "Debug test-mod tests"]);
    let details = f.wb.read_with(f.cx, |wb, cx| wb.overlay_details(cx)).unwrap();
    assert!(
        details[0].starts_with("cargo test --package demo") && details[1].starts_with("cargo test --package demo --lib --no-run \u{2192} tests::adds_up"),
        "{details:?}"
    );
    f.cx.simulate_keystrokes("enter");
    f.cx.run_until_parked();
    let (label, command) = f.wb.read_with(f.cx, |wb, _| wb.last_task_for_test()).expect("a task ran");
    assert_eq!(label, "test tests::adds_up");
    assert_eq!(command, "cargo test --package demo --lib -- tests::adds_up --exact --nocapture --include-ignored");

    // A mark opens the list for its line: the module's runnable only.
    ed.update(f.cx, |e, cx| e.press_run_mark(10, cx));
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the mark's list", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some());
    assert_eq!(f.wb.read_with(f.cx, |wb, cx| wb.overlay_labels(cx)).unwrap(), ["Run test-mod tests", "Debug test-mod tests"]);
    f.cx.simulate_keystrokes("escape");

    // Outside every item there is nothing to run.
    ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(3, 0, cx));
    f.run("Rust: Run Runnable at Cursor");
    f.wait_notice("Nothing to run at the cursor: put it inside a test or main.");

    // The mark is a button for assistive technology, named by its line.
    let snap = f.snapshot();
    let found = snap.to_tree().nodes.into_iter().any(|(_, n)| n.role() == Role::Button && n.label() == Some("Run or debug on line 15"));
    assert!(found, "the mark of line 15 is in the accessibility tree");
}

/// Without the method (or without a server) the marks come from the text and the project's own test finder and entry
/// point stand in: a binary's `main` runs with `cargo run`, with `--target` when `rust.target` is set.
#[cfg(unix)]
#[gpui::test]
fn without_rust_analyzer_the_text_gives_the_marks_and_the_target_applies(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = fixture(cx, "fallback", json!({}), json!({ "rust.target": "aarch64-unknown-linux-musl" })) else { return };
    let main = crate::ui::local_path(&f.root.join("src/main.rs"));
    f.wb.update_in(f.cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    let wb = f.wb.clone();
    wait_ticking(f.cx, "main.rs", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).as_deref() == Some(main.as_str()));
    let ed = editor(&mut f);
    assert_eq!(ed.read_with(f.cx, |e, _| e.run_mark_lines()), [0], "fn main");
    ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(1, 4, cx));
    f.run("Rust: Run Runnable at Cursor");
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the task", |cx| wb.read_with(cx, |wb, _| wb.last_task_for_test()).is_some());
    let (label, command) = f.wb.read_with(f.cx, |wb, _| wb.last_task_for_test()).unwrap();
    assert_eq!((label.as_str(), command.as_str()), ("run main", "cargo run --manifest-path Cargo.toml --target aarch64-unknown-linux-musl"));

    // The library has a test: its mark, and the same kind of list as with a server.
    let lib = crate::ui::local_path(&f.root.join("src/lib.rs"));
    f.wb.update_in(f.cx, |wb, window, cx| wb.open_file_in_root(lib.clone(), window, cx));
    let wb = f.wb.clone();
    wait_ticking(f.cx, "lib.rs", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).as_deref() == Some(lib.as_str()));
    let ed = editor(&mut f);
    assert_eq!(ed.read_with(f.cx, |e, _| e.run_mark_lines()), [9]);
    ed.update(f.cx, |e, cx| e.set_cursor_line_col_for_test(10, 8, cx));
    f.run("Rust: Run Runnable at Cursor");
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the test task", |cx| wb.read_with(cx, |wb, _| wb.last_task_for_test()).is_some_and(|(l, _)| l.starts_with("test")));
    let (_, command) = f.wb.read_with(f.cx, |wb, _| wb.last_task_for_test()).unwrap();
    assert_eq!(command, "cargo test --manifest-path Cargo.toml tests::adds_up --target aarch64-unknown-linux-musl -- --exact");
}

/// The editor keeps the modifiers of rust-analyzer's semantic tokens (here the real ones for the demo library): the
/// `unsafe` block and what is dereferenced in it, by line, UTF-16 column and length.
#[gpui::test]
fn the_editor_keeps_the_unsafe_modifier_of_semantic_tokens(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let sample: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{}/testdata/rust-analyzer/semantic-tokens.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    let scenario = json!({
        "capabilities": { "semanticTokensProvider": { "legend": { "tokenTypes": sample["types"], "tokenModifiers": sample["mods"] }, "full": true } },
        "responses": { "textDocument/semanticTokens/full": { "result": { "data": sample["data"] } } },
    });
    let Some(mut f) = fixture_with(cx, "modifiers", scenario, json!({}), DEMO_LIB) else { return };
    let ed = editor(&mut f);
    ed.update(f.cx, |e, cx| e.schedule_lsp_extras(0, cx));
    wait_ticking(f.cx, "the marks", |cx| !ed.read_with(cx, |e, _| e.modifier_marks_for_test()).is_empty());
    assert_eq!(ed.read_with(f.cx, |e, _| e.modifier_marks_for_test()), [(6, 4, 6, crate::lsp::MOD_UNSAFE), (6, 13, 1, crate::lsp::MOD_UNSAFE)]);
}

/// *Rust: Select Target…* lists the default and the installed targets (this machine's at least), and the choice is written
/// to `rust.target` and applies at once; choosing the default clears it again.
#[gpui::test]
fn the_target_picker_writes_rust_target(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = fixture(cx, "target", json!({}), json!({})) else { return };
    let Ok(version) = std::process::Command::new("rustc").arg("-vV").output() else { return };
    let Some(host) = crate::rust_toolchain::target::parse_host(&String::from_utf8_lossy(&version.stdout)) else { return };

    f.run("Rust: Select Target…");
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the list", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some());
    let labels = f.wb.read_with(f.cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert_eq!(labels[0], "Default: build for this machine");
    assert!(labels.contains(&host), "the machine's own target is listed: {labels:?} / {host}");
    let at = labels.iter().position(|l| *l == host).unwrap();
    for _ in 0..at {
        f.cx.simulate_keystrokes("down");
    }
    f.cx.simulate_keystrokes("enter");
    f.cx.run_until_parked();
    assert_eq!(f.cx.update(|_, cx| cx.global::<crate::settings::Settings>().rust_build.target.clone()), Some(host.clone()));
    let written = std::fs::read_to_string(f.root.parent().unwrap().join("config/settings.jsonc")).unwrap();
    assert!(written.contains(&format!("\"rust.target\":\"{host}\"")) || written.contains(&format!("\"rust.target\": \"{host}\"")), "{written}");

    // Again, choosing the default: the setting goes.
    f.run("Rust: Select Target…");
    let wb = f.wb.clone();
    wait_ticking(f.cx, "the list again", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some());
    let current = f.wb.read_with(f.cx, |wb, cx| wb.overlay_details(cx)).unwrap();
    assert!(current.iter().any(|d| d == "current"), "the chosen target is marked: {current:?}");
    f.cx.simulate_keystrokes("enter");
    f.cx.run_until_parked();
    assert_eq!(f.cx.update(|_, cx| cx.global::<crate::settings::Settings>().rust_build.target.clone()), None);
}

/// Renaming a function keeps the line count, but the places rust-analyzer gave belong to the old text: the marks fall back
/// to the text at once, and a click asks the server again instead of running what the old answer said.
#[cfg(unix)]
#[gpui::test]
fn an_edit_in_a_line_drops_the_old_runnables_and_a_click_asks_again(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let Some(mut f) = runnables_fixture(cx, "stale", json!({})) else { return };
    let ed = editor(&mut f);
    ed.update(f.cx, |e, cx| e.schedule_lsp_extras(0, cx));
    wait_ticking(f.cx, "the marks", |cx| ed.read_with(cx, |e, _| e.runnables().is_some()));
    let asked = f.fake.received("experimental/runnables").len();
    let at = DEMO_LIB.find("adds_up").unwrap();
    let lines_before = ed.read_with(f.cx, |e, _| e.text().lines().count());
    ed.update(f.cx, |e, cx| e.insert_at_for_test(at, "renamed_", cx));
    assert_eq!(ed.read_with(f.cx, |e, _| e.text().lines().count()), lines_before, "the line count did not change");
    assert!(ed.read_with(f.cx, |e, _| e.runnables().is_none()), "the old answer is gone with the edit");
    assert_eq!(ed.read_with(f.cx, |e, _| e.run_mark_lines()), [14, 19, 25], "the marks come from the text meanwhile (tests only)");

    ed.update(f.cx, |e, cx| e.press_run_mark(14, cx));
    let fake = &f.fake;
    wait_ticking(f.cx, "a new request", |_| fake.received("experimental/runnables").len() > asked);
}

/// A server that is still loading answers `experimental/runnables` with an empty list. The marks do not disappear for it:
/// the text stands in, and when the server reports that it is ready the file is asked about again (found in the real app).
#[gpui::test]
fn an_empty_answer_while_loading_keeps_the_marks_and_ready_asks_again(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let scenario = json!({
        "capabilities": { "experimental": { "runnables": { "kinds": ["cargo"] } } },
        "responses": { "experimental/runnables": { "result": [] } },
        "after": { "experimental/runnables": [{ "method": "experimental/serverStatus", "params": { "health": "ok", "quiescent": false } }] },
    });
    let Some(mut f) = fixture_with(cx, "loading", scenario, json!({}), DEMO_LIB) else { return };
    let ed = editor(&mut f);
    ed.update(f.cx, |e, cx| e.schedule_lsp_extras(0, cx));
    let fake = &f.fake;
    wait_ticking(f.cx, "the empty answer", |_| !fake.received("experimental/runnables").is_empty());
    assert!(ed.read_with(f.cx, |e, _| e.runnables().is_none()));
    assert_eq!(ed.read_with(f.cx, |e, _| e.run_mark_lines()), [14, 19, 25], "the text stands in");
}
