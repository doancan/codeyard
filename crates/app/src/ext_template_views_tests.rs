//! #182 end to end: a view made with `cyx new view`, in Rust and in TypeScript, is built and packaged by `cyx`,
//! installed, started in the real codeyard-exthost and drawn by the window; a press on its button reaches the extension
//! and the count it draws changes. Before #182 the template's tree was refused here with "unknown field `type`".
//!
//! Needs `cyx` and `codeyard-exthost` built (`cargo build -p cyx -p ext-host`), the wasm32-wasip2 target for Rust, and
//! Node.js 22 with network access (npm) for TypeScript. Without them the test is skipped with a note, unless
//! CODEYARD_REQUIRE_WASM is set (CI's Linux app job), where a skip is a failure.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

const VIEW: &str = "you.counter.main";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// `cyx` beside codeyard-exthost (both are built into target/<profile>/), or `CODEYARD_CYX`.
fn cyx() -> Option<PathBuf> {
    std::env::var_os("CODEYARD_CYX").map(PathBuf::from).or_else(|| {
        let exthost = crate::ext_runtime::exthost_path()?;
        let p = exthost.with_file_name(if cfg!(windows) { "cyx.exe" } else { "cyx" });
        p.is_file().then_some(p)
    })
}

fn wasm_target() -> bool {
    let out = Command::new("rustc").args(["--print", "target-libdir", "--target", "wasm32-wasip2"]).output();
    out.ok().filter(|o| o.status.success()).is_some_and(|o| Path::new(String::from_utf8_lossy(&o.stdout).trim()).is_dir())
}

fn node() -> bool {
    let out = Command::new("node").arg("--version").output();
    out.ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().trim_start_matches('v').split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major >= 22)
}

/// `cyx`, or why this test cannot run here.
fn toolchain(ts: bool) -> Option<PathBuf> {
    let missing = if crate::ext_runtime::code_runs_here(&"tests").is_err() {
        Some("codeyard-exthost is not built (cargo build -p ext-host) or the sandbox is missing")
    } else if cyx().is_none() {
        Some("cyx is not built (cargo build -p cyx, or set CODEYARD_CYX)")
    } else if !ts && !wasm_target() {
        Some("the wasm32-wasip2 target is not installed (rustup target add wasm32-wasip2)")
    } else if ts && !node() {
        Some("Node.js 22 or newer is not on PATH")
    } else {
        None
    };
    match missing {
        Some(why) if std::env::var_os("CODEYARD_REQUIRE_WASM").is_some() => panic!("CODEYARD_REQUIRE_WASM is set, but {why}"),
        Some(why) => {
            eprintln!("skipped: {why}");
            None
        }
        None => cyx(),
    }
}

fn run(dir: &Path, program: &Path, args: &[&str]) {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        // One target folder for the Rust builds of these tests and of cyx's, so the SDK is compiled once.
        .env("CARGO_TARGET_DIR", repo().join("target/cyx-wasm-tests"))
        .output()
        .unwrap_or_else(|e| panic!("{} {args:?}: {e}", program.display()));
    assert!(out.status.success(), "{} {args:?} failed:\n{}\n{}", program.display(), String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

/// `cyx new view counter` (with the SDK of this checkout), then `cyx package`: the package's bytes.
fn package(cyx: &Path, root: &Path, ts: bool) -> Vec<u8> {
    let sdk = repo().to_string_lossy().into_owned();
    let mut args = vec!["new", "view", "counter", "--publisher", "you", "--sdk", &sdk];
    if ts {
        args.extend(["--lang", "ts"]);
    }
    run(root, cyx, &args);
    let dir = root.join("counter");
    if ts {
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        run(&dir, Path::new(npm), &["install", "--ignore-scripts", "--no-audit", "--no-fund"]);
    } else {
        // The example's lock file (the same template), so the build needs no registry lookups.
        std::fs::copy(repo().join("examples/extensions/hello-view/Cargo.lock"), dir.join("Cargo.lock")).unwrap();
    }
    run(&dir, cyx, &["package"]);
    std::fs::read(dir.join("you.counter-0.1.0.cyx")).expect("cyx package wrote the package")
}

fn setup(cx: &mut TestAppContext, cfg: PathBuf) -> (Entity<Workbench>, &mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(cfg), window, cx));
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    (wb, cx)
}

/// Run the window until the view's `text` elements are `want` (the exthost is a real process), or fail with the view's
/// error.
fn wait_for(wb: &Entity<Workbench>, cx: &mut VisualTestContext, want: &[&str]) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        cx.run_until_parked();
        let texts = wb.read_with(cx, |wb, _| wb.ext_view_texts_for_test(VIEW));
        if texts == want {
            return;
        }
        if let Some(e) = wb.read_with(cx, |wb, _| wb.ext_view_error_for_test(VIEW)) {
            panic!("the view shows an error instead of {want:?}: {e}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {want:?}; the view shows {:?}", wb.read_with(cx, |wb, _| wb.ext_view_texts_for_test(VIEW)));
}

fn press_enter(cx: &mut VisualTestContext) {
    let keystroke = gpui::Keystroke::parse("enter").unwrap();
    cx.update(|window, cx| {
        window.dispatch_event(gpui::PlatformInput::KeyDown(gpui::KeyDownEvent { keystroke: keystroke.clone(), is_held: false }), cx);
        window.dispatch_event(gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }), cx);
    });
    cx.run_until_parked();
}

fn the_template_view_draws(cx: &mut TestAppContext, ts: bool) {
    let Some(cyx) = toolchain(ts) else { return };
    let name = if ts { "ts" } else { "rust" };
    let root = std::env::temp_dir().join(format!("codeyard-template-view-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cfg = root.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let bytes = package(&cyx, &root, ts);
    crate::ext::store(&cfg).install(&bytes, ext_core::Source::File { path: "/downloads/you.counter-0.1.0.cyx".into() }).expect("installs");
    let (wb, cx) = setup(cx, cfg);
    wb.update_in(cx, |wb, window, cx| wb.open_ext_view_for_test(VIEW, window, cx));
    wait_for(&wb, cx, &["0"]);
    // Drawn whole: the heading and the button too, which a screen reader reaches as well.
    let access = wb.update_in(cx, |wb, window, _| wb.ext_view_access_for_test(window));
    assert!(access.iter().any(|(_, label, _)| label == "Clicks"), "{access:?}");
    assert!(access.iter().any(|(_, label, act)| label == "Add one" && *act), "{access:?}");
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_focus_order_for_test(VIEW)), ["add"]);
    // The button is the view's only control: Enter on it sends {"click": {"path": ["add"]}}.
    wb.update_in(cx, |wb, window, cx| wb.focus_ext_view_for_test(VIEW, window, cx));
    press_enter(cx);
    wait_for(&wb, cx, &["1"]);
    drop(wb);
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui::test]
fn a_rust_view_from_cyx_new_view_draws_in_codeyard(cx: &mut TestAppContext) {
    the_template_view_draws(cx, false);
}

#[gpui::test]
fn a_typescript_view_from_cyx_new_view_draws_in_codeyard(cx: &mut TestAppContext) {
    the_template_view_draws(cx, true);
}
