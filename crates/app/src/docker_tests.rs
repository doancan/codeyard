//! Hermetic tests: no installed Docker, network or personal configuration is required.
use super::docker::*;
use crate::container_engine::Engine;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[test]
fn container_engines_are_platform_specific_and_do_not_require_docker() {
    let mac = Engine::detect("macos", |engine| engine == Engine::Apple);
    assert_eq!(mac[0].engine, Engine::Apple);
    assert!(mac[0].installed);
    assert!(!mac.iter().find(|c| c.engine == Engine::Docker).unwrap().installed);
    let windows = Engine::detect("windows", |engine| engine == Engine::Wslc);
    assert_eq!(windows[0].engine, Engine::Wslc);
    assert!(windows[0].installed);
    assert!(!windows.iter().any(|c| c.engine == Engine::Apple));
    assert_eq!(Engine::candidates("linux"), [Engine::Podman, Engine::Docker]);
    assert!(Engine::detect("windows", |_| false).iter().all(|c| !c.installed));
}

#[test]
fn native_container_commands_and_targets_never_invoke_docker() {
    for engine in [Engine::Apple, Engine::Wslc, Engine::Podman] {
        let list = engine.list(engine.label());
        assert_eq!(list.program, engine.program());
        assert!(!list.args.contains(&"--context".into()));
        assert!(list.args.contains(&"json".into()));
        assert_eq!(engine.alias(engine.label(), "web-1").unwrap(), format!("container-runtime:{}/web-1", engine.id()));
        assert!(engine.alias(engine.label(), "--privileged").is_err());
    }
    assert!(Engine::Apple.logs("local", "web").args.contains(&"-n".into()));
    assert_eq!(Engine::Wslc.list("local").args[..2], ["container", "list"]);
}

#[test]
fn native_container_json_parsers_follow_each_runtime_format() {
    let apple = r#"[{"id":"web","configuration":{"id":"web","image":{"reference":"alpine:latest"}},"status":{"state":"running"}}]"#;
    let rows = Engine::Apple.containers(apple).unwrap();
    assert_eq!((rows[0].id.as_str(), rows[0].image.as_str(), rows[0].state.as_str()), ("web", "alpine:latest", "running"));
    let wslc = "{\"ID\":\"a\",\"Names\":\"one\",\"Image\":\"alpine\",\"State\":\"running\"}\n{\"ID\":\"b\",\"Names\":\"two\",\"Image\":\"ubuntu\",\"State\":\"stopped\"}\n";
    let rows = Engine::Wslc.containers(wslc).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].state, "exited");
    assert!(Engine::Wslc.containers("").unwrap().is_empty());
    let podman = r#"[{"Id":"abc","Names":["web"],"Image":"alpine","State":"running","Ports":[]}]"#;
    assert_eq!(Engine::Podman.containers(podman).unwrap()[0].name, "web");
    assert!(Engine::Apple.containers("not json").is_err());
}

#[gpui::test]
fn container_engine_picker_opens_without_launching_docker(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().docker_enabled = true);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Remote: Connect to Dev Container…", window, cx));
    cx.run_until_parked();
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert!(labels.iter().any(|label| label == "Docker"));
    assert!(labels.iter().any(|label| label == "Podman"));
    assert!(wb.read_with(cx, |wb, _| wb.output_lines()).is_empty());
}

#[test]
fn docker_module_is_off_by_default_and_projects_cannot_enable_it() {
    use crate::settings::*;
    assert!(!Settings::default().docker_enabled);
    let layers = [("repo", r#"{"modules.containers.enabled":true}"#, false)];
    for trusted in [false, true] {
        let (s, warnings) = parse_layers_trusted("{}", &layers, trusted).unwrap();
        assert!(!s.docker_enabled);
        assert!(!warnings.is_empty());
        let (s, _) = parse_layers_trusted(r#"{"modules.containers.enabled":true}"#, &[], trusted).unwrap();
        assert!(s.docker_enabled);
    }
}

#[test]
fn docker_targets_pin_context_and_user_and_reject_options() {
    assert_eq!(target("remote", "abc123", Some("1000:1000")).unwrap(), "docker-context:remote/abc123/1000:1000");
    for s in ["", "--help", "a/b", "a\nb", "x;whoami", "a\0b"] {
        assert!(target(s, "abc", None).is_err());
        assert!(target("default", s, None).is_err());
        assert!(target("default", "abc", Some(s)).is_err());
    }
}

#[test]
fn docker_ssh_context_routes_operations_and_reconnects_without_local_docker() {
    let context =
        DockerContext { engine: Engine::Docker, ssh: Some("linux-build".into()), name: "default".into(), endpoint: "unix:///var/run/docker.sock".into() };
    for command in [context.engine.list(&context.name), context.engine.logs(&context.name, "web"), context.engine.command(&context.name, &["start", "web"])] {
        assert_eq!(context.route(command).ssh.as_deref(), Some("linux-build"));
    }
    let alias = context.container_alias("web").unwrap();
    assert!(is_target(&alias));
    assert_eq!(host::ssh::ssh_docker_host(&alias).as_deref(), Some("linux-build"));
}

#[test]
fn docker_structured_output_keeps_health_and_stopped_containers() {
    let text = r#"{"ID":"abc","Names":"web","Image":"alpine","State":"running","Status":"Up 1 minute (healthy)","Ports":"127.0.0.1:3000->3000/tcp"}
{"ID":"def","Names":"db","Image":"postgres","State":"exited","Status":"Exited (1)"}"#;
    let rows = json_lines::<Container>(text).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].status.contains("healthy"));
    assert_eq!(rows[1].state, "exited");
    assert!(json_lines::<Container>("not json").is_err());
}

#[test]
fn docker_devcontainer_result_is_checked_before_connecting() {
    let good = "log\n{\"outcome\":\"success\",\"containerId\":\"abc\",\"remoteWorkspaceFolder\":\"/workspaces/proje\",\"remoteUser\":\"dev\"}\n";
    let result = opened(good).unwrap();
    assert_eq!(result.folder, "/workspaces/proje");
    for bad in
        ["", "{}", r#"{"outcome":"error","description":"build failed"}"#, r#"{"outcome":"success","containerId":"abc","remoteWorkspaceFolder":"relative"}"#]
    {
        assert!(opened(bad).is_err(), "{bad}");
    }
}

#[test]
fn docker_project_discovery_supports_jsonc_and_preserves_paths_as_arguments() {
    let dir = crate::qa_workbench_tests::fresh("docker-project ö boşluk");
    std::fs::create_dir(dir.join(".devcontainer")).unwrap();
    std::fs::write(dir.join(".devcontainer/devcontainer.json"), "{ // comment\n\"name\":\"Proje\",\"image\":\"alpine\",\"postCreateCommand\":\"echo ready\",}")
        .unwrap();
    let p = Project::discover(&dir).unwrap();
    assert_eq!(p.name, "Proje");
    assert!(p.summary.contains("postCreateCommand"));
    let cmd = p.command("test-remote", true);
    assert!(cmd.args.contains(&p.root.to_string_lossy().into_owned()));
    assert!(cmd.args.contains(&"--remove-existing-container".into()));
    assert_eq!(cmd.context.as_deref(), Some("test-remote"));
    assert!(!p.command("default", false).args.contains(&"--remove-existing-container".into()));
}

#[test]
fn docker_cancellation_prevents_process_launch() {
    let cmd = Command { program: "this-program-must-never-run".into(), args: vec![], context: None, ssh: None, cwd: None, timeout: Duration::from_secs(1) };
    assert_eq!(run(&cmd, &AtomicBool::new(true), |_| {}).unwrap_err(), "Operation cancelled");
    let mut state = State::default();
    let flag = std::sync::Arc::new(AtomicBool::new(false));
    state.cancel = Some(flag.clone());
    state.cancel();
    assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
}

#[cfg(windows)]
#[test]
fn docker_process_runner_drains_both_pipes_and_times_out() {
    let mut cmd = Command {
        ssh: None,
        program: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
        args: vec!["--exact".into(), "docker_tests::docker_process_fixture_stdio".into(), "--ignored".into(), "--nocapture".into()],
        context: None,
        cwd: None,
        timeout: Duration::from_secs(10),
    };
    let mut logs = Vec::new();
    assert!(run(&cmd, &AtomicBool::new(false), |l| logs.push(l)).unwrap().contains("result"));
    assert!(logs.iter().any(|l| l.contains("progress")));
    cmd.args[1] = "docker_tests::docker_process_fixture_failure".into();
    assert!(run(&cmd, &AtomicBool::new(false), |_| {}).unwrap_err().contains("setup failed"));
    cmd.args[1] = "docker_tests::docker_process_fixture_sleep".into();
    cmd.timeout = Duration::from_millis(100);
    assert_eq!(run(&cmd, &AtomicBool::new(false), |_| {}).unwrap_err(), "Operation timed out");
}

// Owned child-process fixtures avoid depending on a cold PowerShell startup. The runner's
// success deadline and its 100 ms timeout assertion remain unchanged.
#[cfg(windows)]
#[test]
#[ignore = "child process fixture for docker_process_runner_drains_both_pipes_and_times_out"]
fn docker_process_fixture_stdio() {
    use std::io::Write;
    std::io::stdout().write_all(b"result\n").unwrap();
    std::io::stdout().flush().unwrap();
    std::io::stderr().write_all(b"progress\n").unwrap();
    std::io::stderr().flush().unwrap();
}

#[cfg(windows)]
#[test]
#[ignore = "child process fixture for docker_process_runner_drains_both_pipes_and_times_out"]
fn docker_process_fixture_failure() {
    use std::io::Write;
    std::io::stdout().write_all(b"setup failed\n").unwrap();
    std::io::stdout().flush().unwrap();
    std::process::exit(7);
}

#[cfg(windows)]
#[test]
#[ignore = "child process fixture for docker_process_runner_drains_both_pipes_and_times_out"]
fn docker_process_fixture_sleep() {
    std::thread::sleep(Duration::from_secs(20));
}

#[gpui::test]
fn docker_disabled_actions_do_not_launch_processes(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    for command in ["Remote: Connect to Dev Container…", "Docker: Open Project in Dev Container…", "Docker: Show Containers and Services…"] {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(command, window, cx));
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.output_lines()).is_empty());
    }
}

#[gpui::test]
fn docker_activity_button_opens_full_width_overview_when_enabled(cx: &mut gpui::TestAppContext) {
    let config = crate::qa_workbench_tests::fresh("docker-activity-config");
    let path = config.join("settings.jsonc");
    std::fs::write(&path, r#"{"modules.containers.enabled":false}"#).unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &config);
    cx.run_until_parked();
    assert!(cx.debug_bounds("act-containers").is_none());

    std::fs::write(&path, r#"{"modules.containers.enabled":true}"#).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    assert!(cx.update(|_, cx| cx.global::<crate::settings::Settings>().docker_enabled));
    let at = cx.debug_bounds("act-containers").expect("enabled module has a left activity button").center();
    cx.simulate_click(at, gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), None);
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("containers", true)));
    assert!(cx.debug_bounds("container-dashboard").is_some());
    assert!(cx.debug_bounds("container-button-Container actions…").is_some());
    assert!(wb.read_with(cx, |wb, _| wb.output_lines()).is_empty(), "opening the overview must not launch Docker");

    std::fs::write(&path, r#"{"modules.containers.enabled":false}"#).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    assert!(!cx.update(|_, cx| cx.global::<crate::settings::Settings>().docker_enabled));
    // GPUI retains old debug bounds across frames. Verify the removed button no
    // longer responds at its former position instead of reading that stale map.
    cx.simulate_click(at, gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some("explorer"));
}
