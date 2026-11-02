//! Hover and completion from code extensions end to end (#142): the probe extension (crates/ext-host/testdata/probe.wasm)
//! runs in a real codeyard-exthost next to a scripted language server (test_lsp.rs). Skipped when codeyard-exthost is
//! not built (`cargo test --workspace` always builds it) or Python 3 is missing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::editor::EditorView;
use crate::test_lsp::FakeServer;
use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe.wasm");

const MANIFEST: &str = r#"[extension]
id = "you.probe"
version = "1.0.0"
name = "Probe"
license = "MIT"

[code]
entry = "Cargo.toml"

# The probe imports these (for the exthost's own tests); nothing here uses them.
[permissions]
read = ["a.txt"]
net = ["api.example.com"]
auth = ["github"]
secrets = true
why = "Tests every part of the host."

[[hover]]
languages = ["toml"]

[[completion]]
languages = ["toml"]
trigger-characters = ["."]
"#;

fn exthost_missing() -> bool {
    !crate::ext_runtime::code_runs_for_test()
}

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-exthover-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    crate::ui::canonical_for_test(d)
}

fn install(cfg: &Path) {
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: MANIFEST.as_bytes().to_vec(), executable: false },
        ext_pkg::Entry { path: "Cargo.toml".into(), data: Vec::new(), executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: PROBE.to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).expect("the probe passes the import check");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "/downloads/probe.cyx".into() }).expect("installs");
}

/// A window with the probe installed; `settings` is the user's settings.jsonc.
fn setup<'a>(cx: &'a mut TestAppContext, cfg: &Path, settings: &str) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    install(cfg);
    std::fs::write(cfg.join("settings.jsonc"), settings).unwrap();
    std::fs::write(cfg.join("keybindings.jsonc"), "[]").unwrap();
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let dir = cfg.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&gpui::Focusable::focus_handle(wb, cx)));
    cx.run_until_parked();
    (wb, cx)
}

fn open(wb: &Entity<Workbench>, path: &Path, cx: &mut VisualTestContext) -> Entity<EditorView> {
    let p = path.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&p, window, cx));
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| wb.active_editor_entity()).expect("editor opened")
}

/// The exthost and the language server are real processes: run the window's tasks until `done`, up to 20 s.
fn wait_for(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20 * crate::test_sshd::timeout_scale());
    while Instant::now() < deadline {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out: {what}");
}

/// Let real time pass with the window running (to show that something does not happen).
fn settle(cx: &mut VisualTestContext, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The completion list as (label, extension) pairs; empty when it is closed.
fn listed(ed: &Entity<EditorView>, cx: &mut VisualTestContext) -> Vec<(String, Option<String>)> {
    ed.read_with(cx, |e, _| e.completion_sources_for_test()).unwrap_or_default()
}

fn has(list: &[(String, Option<String>)], label: &str, source: Option<&str>) -> bool {
    list.iter().any(|(l, s)| l == label && s.as_deref() == source)
}

/// Show the hover for the character at `ix` and let the hover delay pass.
fn hover_at(ed: &Entity<EditorView>, ix: usize, cx: &mut VisualTestContext) {
    ed.update(cx, |e, cx| e.hover_at_for_test(ix, cx));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(500));
}

/// A language server for TOML that describes every position and offers one completion.
fn language_server(cfg: &Path) -> Option<(FakeServer, String)> {
    let scenario = serde_json::json!({
        "capabilities": { "hoverProvider": true, "completionProvider": { "triggerCharacters": ["."] } },
        "responses": {
            "textDocument/hover": { "result": { "contents": { "kind": "markdown", "value": "LSP: a key" } } },
            "textDocument/completion": { "result": [{ "label": "lsp-item", "detail": "from the server" }] },
        },
    });
    let fake = FakeServer::new(&cfg.join("server"), &scenario)?;
    let (command, args) = fake.command();
    let settings = serde_json::json!({ "lsp.enabled": true, "lsp.servers": { "toml": { "command": command, "args": args } } }).to_string();
    Some((fake, settings))
}

/// AC1: an extension's hover shows beside the language server's for the same position, under the extension's name;
/// its completions join the server's in one list, each tagged with the extension.
#[gpui::test]
fn extension_hover_and_completion_join_the_language_server(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("lsp");
    let Some((_fake, settings)) = language_server(&cfg) else { return };
    let (wb, cx) = setup(cx, &cfg, &settings);
    let dir = config_dir("lsp-files");
    let file = dir.join("a.toml");
    std::fs::write(&file, "name = 1\n").unwrap();
    let ed = open(&wb, &file, cx);
    wait_for(cx, "the language server", |cx| ed.read_with(cx, |e, _| e.lsp_client().is_some()));

    hover_at(&ed, 2, cx);
    wait_for(cx, "both hovers", |cx| ed.read_with(cx, |e, _| e.hover_text().is_some_and(|t| t.contains("LSP: a key")) && !e.ext_hovers_for_test().is_empty()));
    assert_eq!(ed.read_with(cx, |e, _| e.ext_hovers_for_test()), [("you.probe".to_string(), "Probe hover at 0:2".to_string())]);

    // Completion: a word, then a trigger character the extension declared.
    ed.update(cx, |e, cx| e.set_cursor_for_test(9, cx));
    cx.simulate_input("i");
    wait_for(cx, "the merged list", |cx| {
        let l = listed(&ed, cx);
        has(&l, "lsp-item", None) && has(&l, "probe-item", Some("you.probe"))
    });
    assert!(!has(&listed(&ed, cx), "probe-dot", Some("you.probe")), "probe-dot comes only after a dot");
    cx.simulate_keystrokes("escape");
    cx.simulate_input(".");
    wait_for(cx, "the trigger character's items", |cx| has(&listed(&ed, cx), "probe-dot", Some("you.probe")));
}

/// AC2: typing with a slow completion provider never waits for it. Leaving the word cancels its request; a request
/// that misses the deadline times out, and the answer that comes after it is dropped and noted in the log, never shown.
#[gpui::test]
fn a_slow_completion_provider_never_holds_up_typing(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("slow");
    let (wb, cx) = setup(cx, &cfg, "{}");
    let dir = config_dir("slow-files");
    let file = dir.join("slow.toml");
    std::fs::write(&file, "slow = 1\n").unwrap();
    let ed = open(&wb, &file, cx);
    // The first word starts the extension, which sits on the request.
    cx.simulate_input("x");
    wait_for(cx, "the extension to start", |cx| wb.read_with(cx, |wb, _| wb.extension_running_for_test("you.probe")));
    let t0 = Instant::now();
    cx.simulate_input("y z");
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "xy zslow = 1\n");
    assert!(t0.elapsed() < Duration::from_secs(1), "typing waited for the provider: {:?}", t0.elapsed());
    // The space left the word: that request was cancelled, and the probe's answer to the cancel is dropped.
    settle(cx, 300);
    assert!(!listed(&ed, cx).iter().any(|(_, s)| s.is_some()), "{:?}", listed(&ed, cx));

    // "z" asked again; the deadline passes while the probe sits on it.
    cx.executor().advance_clock(Duration::from_millis(ext_manifest::limits::PROVIDER_DEADLINE_MILLIS + 50));
    wait_for(cx, "the late answer in the log", |cx| {
        wb.read_with(cx, |wb, _| wb.extension_log_for_test("you.probe")).iter().any(|l| l.contains("after its") && l.contains("dropped"))
    });
    settle(cx, 200);
    assert!(!has(&listed(&ed, cx), "probe-late", Some("you.probe")), "a late answer is never shown");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "xy zslow = 1\n");
}

/// AC3: turning the extension off or uninstalling it removes its hover text and completion items at once.
#[gpui::test]
fn uninstalling_the_extension_removes_its_items_at_once(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let cfg = config_dir("uninstall");
    let (wb, cx) = setup(cx, &cfg, "{}");
    let dir = config_dir("uninstall-files");
    let file = dir.join("a.toml");
    std::fs::write(&file, "name = 1\n").unwrap();
    let ed = open(&wb, &file, cx);
    let show = |cx: &mut VisualTestContext| {
        hover_at(&ed, 2, cx);
        wait_for(cx, "the extension's hover", |cx| !ed.read_with(cx, |e, _| e.ext_hovers_for_test()).is_empty());
        ed.update(cx, |e, cx| e.set_cursor_for_test(9, cx));
        cx.simulate_input("p");
        wait_for(cx, "the extension's items", |cx| has(&listed(&ed, cx), "probe-item", Some("you.probe")));
    };
    show(cx);
    wb.update_in(cx, |wb, window, cx| wb.set_extension_enabled_for_test("you.probe", false, window, cx));
    cx.run_until_parked();
    assert!(ed.read_with(cx, |e, _| e.ext_hovers_for_test()).is_empty(), "the hover text went with it");
    assert!(!listed(&ed, cx).iter().any(|(_, s)| s.is_some()), "its items went with it");

    wb.update_in(cx, |wb, window, cx| wb.set_extension_enabled_for_test("you.probe", true, window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape backspace");
    show(cx);
    wb.update_in(cx, |wb, window, cx| wb.uninstall_extension_for_test("you.probe", window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Uninstall");
    cx.run_until_parked();
    assert!(ed.read_with(cx, |e, _| e.ext_hovers_for_test()).is_empty(), "the hover text went with it");
    assert!(!listed(&ed, cx).iter().any(|(_, s)| s.is_some()), "its items went with it");
    // And it is not asked again.
    cx.simulate_input("q");
    settle(cx, 300);
    assert!(!listed(&ed, cx).iter().any(|(_, s)| s.is_some()));
}

/// SSH parity: over SSH the document goes to the extension from the editor, as it is on screen (no read on the host),
/// and its hover and completions show the same way.
#[gpui::test]
fn extension_hover_and_completion_work_over_ssh(cx: &mut TestAppContext) {
    if exthost_missing() {
        return;
    }
    let Some(_sshd) = crate::test_sshd::start("ext-hover-ssh") else { return };
    let cfg = config_dir("ssh");
    let root = cfg.join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.toml"), "name = 1\n").unwrap();
    let (wb, cx) = setup(cx, &cfg, "{}");
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 30, "connected", |cx| {
        cx.executor().advance_clock(Duration::from_secs(1));
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    let r = root.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&r, window, cx));
    cx.run_until_parked();
    let ed = crate::qa_integration_tests::ssh_open_file(&wb, cx, &root.join("a.toml"));
    hover_at(&ed, 2, cx);
    wait_for(cx, "the extension's hover", |cx| !ed.read_with(cx, |e, _| e.ext_hovers_for_test()).is_empty());
    ed.update(cx, |e, cx| e.set_cursor_for_test(9, cx));
    cx.simulate_input("p");
    wait_for(cx, "the extension's items", |cx| has(&listed(&ed, cx), "probe-item", Some("you.probe")));
}
