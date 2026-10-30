//! Rust workflow regressions: discovery, target arguments, honest results and debug boundaries.

#[test]
fn rust_hardening_discovers_parameterized_async_test_attributes_and_raw_identifiers() {
    let source = "#[tokio::test(flavor = \"current_thread\")]\nasync fn asynchronous() {}\n\n#[test]\nfn r#type() {}\n";
    let cases = crate::testrun::cases("src/lib.rs", source, None);
    assert_eq!(cases.iter().map(|c| c.run.id.as_str()).collect::<Vec<_>>(), ["asynchronous", "type"]);
}

#[test]
fn rust_hardening_duplicate_test_names_cannot_overwrite_a_failure_with_a_pass() {
    use crate::testexp::{Kind, Outcome, parse_results};
    for output in ["test tests::same ... FAILED\ntest tests::same ... ok\n", "test tests::same ... ok\ntest tests::same ... FAILED\n"] {
        assert_eq!(parse_results(Kind::Rust, output)["tests::same"].outcome, Outcome::Failed, "{output}");
    }
}

#[test]
fn rust_hardening_test_artifacts_are_nonempty_and_unique() {
    let artifact =
        |path: &str| serde_json::json!({"reason":"compiler-artifact","target":{"name":"api","kind":["test"]},"profile":{"test":true},"executable":path});
    let output = format!("{}\n{}\n{}\n", artifact(""), artifact("/custom output/api-123"), artifact("/custom output/api-123"));
    assert_eq!(crate::testexp::test_executables(&output), [("api".to_string(), "test".to_string(), "/custom output/api-123".to_string())]);
}

#[test]
fn rust_hardening_unreported_tests_cannot_keep_an_earlier_pass_after_a_failed_build() {
    use crate::testexp::{Job, Kind, Outcome, outcomes};
    let job = Job { kind: Kind::Rust, command: "cargo test --lib".into(), scope: vec![("src/lib.rs".into(), "tests::one".into())] };
    let result = outcomes(&job, "error: could not compile the crate\n", Some(101));
    assert_eq!(result.len(), 1, "the current failed run must replace the earlier result");
    assert_eq!(result[0].1.outcome, Outcome::Failed);
    assert!(result[0].1.message.is_some());
}

#[test]
fn rust_hardening_debug_build_preserves_target_features_and_manifest_arguments() {
    assert_eq!(
        crate::testexp::cargo_select_args(
            "cargo test --manifest-path 'member with spaces/Cargo.toml' --bin worker --features 'cli,serde' --no-default-features tests::works -- --exact"
        )
        .unwrap(),
        ["--manifest-path", "member with spaces/Cargo.toml", "--bin", "worker", "--features", "cli,serde", "--no-default-features"]
    );
}

#[gpui::test]
fn rust_hardening_debug_at_cursor_does_not_run_a_test_above_a_helper(cx: &mut gpui::TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::qa_workbench_tests::fresh("rust-hardening-debug-helper");
    let file = dir.join("lib.rs");
    std::fs::write(&file, "#[test]\nfn works() {}\n\nfn helper() {\n    let _x = 1;\n}\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().lsp_enabled = false);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(4, 4, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Test: Debug Test at Cursor", window, cx));
    assert!(wb.read_with(cx, |wb, _| wb.notice_for_test()).is_some_and(|(_, text)| text == "No test at the cursor: put it inside a test function."));
}

#[gpui::test]
fn rust_hardening_failed_real_test_build_reaches_problems_and_never_starts_a_session(cx: &mut gpui::TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::qa_workbench_tests::fresh("rust-hardening-build-error");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"broken-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();
    let file = dir.join("src/lib.rs");
    std::fs::write(&file, "#[test]\nfn broken() {\n    let _n: u32 = \"wrong type\";\n}\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    cx.update(|_, cx| {
        let settings = cx.global_mut::<crate::settings::Settings>();
        settings.lsp_enabled = false;
        settings.workspace_trust = false;
    });
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(2, 4, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Test: Debug Test at Cursor", window, cx));
    crate::qa_integration_tests::wait_ticking(cx, "the compiler error", |cx| {
        wb.read_with(cx, |wb, _| wb.notice_for_test()).is_some_and(|(error, text)| error && text.contains("Rust tests did not build"))
    });
    assert!(wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_none());
    assert!(wb.read_with(cx, |wb, _| wb.problems()).iter().any(|(path, count)| path.ends_with("/src/lib.rs") && *count > 0));
}

#[gpui::test]
fn rust_hardening_detected_workspace_target_reaches_dap_with_the_real_cargo_artifact(cx: &mut gpui::TestAppContext) {
    let python = if cfg!(windows) { "python" } else { "python3" };
    if std::process::Command::new(python).arg("-V").output().is_err() {
        crate::test_sshd::skip_missing(python);
        return;
    }
    let dir = crate::qa_workbench_tests::fresh("rust-hardening-workspace-dap");
    std::fs::create_dir_all(dir.join("member/src")).unwrap();
    std::fs::create_dir_all(dir.join(".cargo")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"member\"]\nresolver = \"3\"\n").unwrap();
    std::fs::write(dir.join(".cargo/config.toml"), "[build]\ntarget-dir = \"custom output\"\n").unwrap();
    std::fs::write(
        dir.join("member/Cargo.toml"),
        "[package]\nname = \"different-package-name\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[[bin]]\nname = \"worker\"\npath = \"src/main.rs\"\n",
    )
    .unwrap();
    let source = dir.join("member/src/main.rs");
    std::fs::write(&source, "fn main() { println!(\"debug target\"); }\n").unwrap();
    let script = crate::dap::tests::write_fake_adapter(&dir);
    let log = dir.join("dap.log");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    cx.update(|_, cx| {
        let settings = cx.global_mut::<crate::settings::Settings>();
        settings.lsp_enabled = false;
        settings.workspace_trust = false;
    });
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &source, cx);
    let editor = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    editor.update(cx, |ed, cx| ed.insert_at_for_test(0, "// saved before build\n", cx));
    let pending = std::sync::Arc::new(std::sync::Mutex::new(None));
    let out = pending.clone();
    wb.update(cx, |wb, cx| {
        let task = wb.detected_debug_configs_for_test(cx).unwrap();
        cx.spawn(async move |_, _| *out.lock().unwrap() = Some(task.await.0)).detach();
    });
    crate::qa_integration_tests::wait_ticking(cx, "Cargo target discovery", |_| pending.lock().unwrap().is_some());
    let mut configs = pending.lock().unwrap().take().unwrap();
    assert_eq!(configs.len(), 1);
    let mut config = configs.remove(0);
    assert_eq!(config.cargo_target.as_ref().unwrap().name, "worker");
    config.adapter = Ok(crate::debug_config::Adapter {
        id: "lldb-dap".into(),
        command: python.into(),
        args: vec![script.to_string_lossy().into_owned(), log.to_string_lossy().into_owned()],
        connect: None,
    });
    config.args["source"] = serde_json::json!(source.to_string_lossy());
    wb.update(cx, |wb, cx| wb.start_debug_config_for_test(config, cx));
    let launch = || std::fs::read_to_string(&log).unwrap_or_default().lines().find(|l| l.starts_with("launch ")).map(str::to_string);
    crate::qa_integration_tests::wait_ticking(cx, "the launch request", |_| launch().is_some());
    let args: serde_json::Value = serde_json::from_str(launch().unwrap().trim_start_matches("launch ")).unwrap();
    let program = args["program"].as_str().unwrap();
    assert!(program.contains("custom output") && program.contains("worker"), "{program}");
    assert!(std::path::Path::new(program).is_file());
    assert!(std::fs::read_to_string(&source).unwrap().starts_with("// saved before build\n"));
    cx.simulate_keystrokes("shift-f5");
    cx.run_until_parked();
}

fn dirty_debug_fixture<'a>(
    cx: &'a mut gpui::TestAppContext,
    name: &str,
) -> (gpui::Entity<crate::workbench::Workbench>, &'a mut gpui::VisualTestContext, std::path::PathBuf, std::path::PathBuf, crate::debug_config::LaunchConfig) {
    let dir = crate::qa_workbench_tests::fresh(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let manifest = "[package]\nname = \"save-guard\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
    std::fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    let file = dir.join("src/main.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    cx.update(|_, cx| {
        let settings = cx.global_mut::<crate::settings::Settings>();
        settings.lsp_enabled = false;
        settings.workspace_trust = false;
    });
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &file, cx);
    let editor = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    editor.update(cx, |ed, cx| ed.insert_at_for_test(0, "// unsaved\n", cx));
    let config = crate::debug_config::detect(Some(manifest), false, None, dir.to_str().unwrap()).remove(0);
    (wb, cx, dir, file, config)
}

#[gpui::test]
fn rust_hardening_debug_save_never_overwrites_an_external_change(cx: &mut gpui::TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (wb, cx, dir, file, config) = dirty_debug_fixture(cx, "rust-hardening-save-conflict");
    std::fs::write(&file, "// external change\nfn main() {}\n").unwrap();
    wb.update(cx, |wb, cx| wb.start_debug_config_for_test(config, cx));
    crate::qa_integration_tests::wait_ticking(cx, "debug cancelled on save conflict", |cx| {
        wb.read_with(cx, |wb, _| wb.notice_for_test()).is_some_and(|(error, message)| error && message.contains("Could not save files before debugging"))
    });
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "// external change\nfn main() {}\n");
    assert!(wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_none());
    assert!(!dir.join("target").exists(), "the build must not start after a failed save");
}

#[gpui::test]
fn rust_hardening_stop_cancels_a_debug_start_waiting_for_saves(cx: &mut gpui::TestAppContext) {
    let (wb, cx, dir, _file, config) = dirty_debug_fixture(cx, "rust-hardening-stop-during-save");
    wb.update(cx, |wb, cx| wb.start_debug_config_for_test(config, cx));
    cx.simulate_keystrokes("shift-f5");
    cx.executor().advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_none());
    assert!(!dir.join("target").exists());
}

#[gpui::test]
fn rust_hardening_stop_cancels_a_pending_real_test_build_before_its_reply(cx: &mut gpui::TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = crate::qa_workbench_tests::fresh("rust-hardening-stop-during-build");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"cancel-stop\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();
    let file = dir.join("src/lib.rs");
    std::fs::write(&file, "#[test]\nfn works() {}\n").unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    cx.update(|_, cx| {
        let settings = cx.global_mut::<crate::settings::Settings>();
        settings.lsp_enabled = false;
        settings.workspace_trust = false;
    });
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    crate::qa_workbench_tests::open_in(&wb, &file, cx);
    let editor = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    editor.update(cx, |ed, cx| ed.set_cursor_line_col_for_test(1, 4, cx));
    wb.update_in(cx, |wb, window, cx| {
        wb.run_command_for_test("Test: Debug Test at Cursor", window, cx);
        wb.run_command_for_test("Debug: Stop", window, cx);
    });
    crate::qa_integration_tests::wait_ticking(cx, "the cancelled progress notice", |cx| {
        wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some()
            || wb.read_with(cx, |wb, _| wb.notice_for_test()).is_none_or(|(_, text)| text != "Building the tests…")
    });
    assert!(wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_none(), "Stop must reject the late build result");
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test());
    assert!(notice.is_none(), "the cancelled progress notice must clear: {notice:?}");
}

#[gpui::test]
fn rust_hardening_cancelled_launch_does_not_report_a_late_prelaunch_error(cx: &mut gpui::TestAppContext) {
    let dir = crate::qa_workbench_tests::fresh("rust-hardening-cancelled-prelaunch");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let mut config = crate::debug_config::detect(Some("[package]\nname = \"cancelled\"\n"), false, None, dir.to_str().unwrap()).remove(0);
    config.adapter = Ok(crate::debug_config::Adapter { id: "lldb-dap".into(), command: "unused-adapter".into(), args: Vec::new(), connect: None });
    config.pre_launch = Some("exit 1".into());
    wb.update_in(cx, |wb, window, cx| {
        wb.start_debug_config_for_test(config, cx);
        wb.run_command_for_test("Debug: Stop", window, cx);
    });
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.debug_snapshot()).unwrap().0, "ended");
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test());
    assert!(notice.is_none(), "an ended session must ignore its late failure: {notice:?}");
}

/// A virtual workspace with two differently named binaries; `extra` must be on for the program to build.
fn two_target_workspace(name: &str) -> std::path::PathBuf {
    let dir = crate::qa_workbench_tests::fresh(name);
    std::fs::create_dir_all(dir.join("app/src/bin")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\nresolver = \"3\"\n[profile.fast]\ninherits = \"release\"\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app-pkg\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[features]\nextra = []\n").unwrap();
    std::fs::write(dir.join("app/src/bin/server.rs"), "#[cfg(feature = \"extra\")]\nfn main() {}\n").unwrap();
    std::fs::write(dir.join("app/src/bin/client.rs"), "fn main() {}\n").unwrap();
    dir
}

/// Discovers the targets, offers both in the picker and debugs `server` with the chosen options; returns the program
/// the adapter was asked to launch.
fn debug_server_with_options(wb: &gpui::Entity<crate::workbench::Workbench>, cx: &mut gpui::VisualTestContext, dir: &std::path::Path, python: &str) -> String {
    cx.update(|_, cx| {
        cx.global_mut::<crate::settings::Settings>().rust_build = crate::cargo_project::BuildOptions {
            features: vec!["extra".into()],
            no_default_features: false,
            profile: Some("fast".into()),
            target_dir: Some("chosen out".into()),
        };
    });
    let pending = std::sync::Arc::new(std::sync::Mutex::new(None));
    let out = pending.clone();
    wb.update(cx, |wb, cx| {
        let task = wb.detected_debug_configs_for_test(cx).unwrap();
        cx.spawn(async move |_, _| *out.lock().unwrap() = Some(task.await.0)).detach();
    });
    crate::qa_integration_tests::wait_ticking(cx, "Cargo target discovery", |_| pending.lock().unwrap().is_some());
    let configs = pending.lock().unwrap().take().unwrap();
    let names: Vec<_> = configs.iter().filter_map(|c| c.cargo_target.as_ref()).map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["client", "server"]);
    let rows = wb.update_in(cx, |wb, window, cx| wb.debug_picker_rows_for_test(configs.clone(), window, cx));
    assert_eq!(rows.len(), 2, "a picker offers the targets: {rows:?}");
    assert!(rows.iter().all(|(_, detail)| detail.ends_with("fast · features: extra · output: chosen out")), "{rows:?}");
    let mut config = configs.into_iter().find(|c| c.cargo_target.as_ref().is_some_and(|t| t.name == "server")).unwrap();
    let script = crate::dap::tests::write_fake_adapter(dir);
    let log = dir.join("dap.log");
    config.adapter = Ok(crate::debug_config::Adapter {
        id: "lldb-dap".into(),
        command: python.into(),
        args: vec![script.to_string_lossy().into_owned(), log.to_string_lossy().into_owned()],
        connect: None,
    });
    wb.update(cx, |wb, cx| wb.start_debug_config_for_test(config, cx));
    let launch = || std::fs::read_to_string(&log).unwrap_or_default().lines().find(|l| l.starts_with("launch ")).map(str::to_string);
    crate::qa_integration_tests::wait_ticking(cx, "the launch request", |cx| launch().is_some() || wb.read_with(cx, |wb, _| wb.notice_for_test()).is_some());
    let Some(line) = launch() else { panic!("no launch: {:?}", wb.read_with(cx, |wb, _| wb.notice_for_test())) };
    let args: serde_json::Value = serde_json::from_str(line.trim_start_matches("launch ")).unwrap();
    cx.simulate_keystrokes("shift-f5");
    cx.run_until_parked();
    args["program"].as_str().unwrap().to_string()
}

fn fake_adapter_python() -> Option<&'static str> {
    let python = if cfg!(windows) { "python" } else { "python3" };
    let ok = std::process::Command::new(python).arg("-V").output().is_ok();
    if !ok {
        crate::test_sshd::skip_missing(python);
    }
    ok.then_some(python)
}

#[gpui::test]
fn rust_debug_builds_the_picked_target_with_the_chosen_features_profile_and_output_folder(cx: &mut gpui::TestAppContext) {
    let Some(python) = fake_adapter_python() else { return };
    let dir = two_target_workspace("rust-debug-build-options");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().lsp_enabled = false);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let program = debug_server_with_options(&wb, cx, &dir, python);
    let expected = dir.join("chosen out").join("fast").join(if cfg!(windows) { "server.exe" } else { "server" });
    assert_eq!(std::path::Path::new(&program), expected);
}

#[gpui::test]
fn rust_debug_build_options_apply_on_the_ssh_host(cx: &mut gpui::TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("rust-debug-build-options-ssh") else { return };
    let Some(python) = fake_adapter_python() else { return };
    let dir = std::fs::canonicalize(two_target_workspace("rust-debug-build-options-ssh")).unwrap();
    let (wb, cx) = crate::qa_integration_tests::ssh_workbench(cx, &dir);
    let program = debug_server_with_options(&wb, cx, &dir, python);
    assert_eq!(std::path::Path::new(&program), dir.join("chosen out/fast/server"), "built on the server in its workspace folder");
}
