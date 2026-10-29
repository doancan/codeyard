//! Task runs through the workbench and the local agent: real processes, inputs, Stop, Problems and preLaunchTask.
#![cfg(unix)]

use std::path::Path;

use super::*;
use crate::qa_integration_tests::{setup_with, wait_ticking};
use crate::settings::Settings;

fn project(name: &str, tasks: &str) -> std::path::PathBuf {
    let dir = crate::qa_workbench_tests::fresh(name);
    std::fs::create_dir_all(dir.join(".codeyard")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.join(".codeyard/tasks.jsonc"), tasks).unwrap();
    dir
}

fn find(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext, label: &str) -> Task {
    let found = std::rc::Rc::new(std::cell::RefCell::new(None));
    let (slot, label) = (found.clone(), label.to_string());
    wb.update(cx, |wb, cx| {
        wb.detect_tasks(cx, move |_, tasks, _, _| {
            *slot.borrow_mut() = Some(tasks.into_iter().find(|t| t.label == label));
        })
    });
    wait_ticking(cx, "tasks to be found", |_| found.borrow().is_some());
    found.borrow_mut().take().flatten().expect("task in the file")
}

fn notice(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| match &wb.notice {
        Some(Notice::Error(m)) => format!("error: {m}"),
        Some(Notice::Info(m)) => format!("info: {m}"),
        None => String::new(),
    })
}

fn last_run_finished(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> bool {
    wb.read_with(cx, |wb, _| wb.jobs.runs.last().is_some_and(|r| r.run.finished()))
}

fn terminal_text(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext, label: &str) -> String {
    wb.read_with(cx, |wb, cx| {
        let live = wb.jobs.runs.last().unwrap();
        let node = live.run.prepared.registry.tasks.iter().position(|t| t.label == label).unwrap();
        live.terms.get(&node).map(|t| t.read(cx).scrollback_text()).unwrap_or_default()
    })
}

fn start(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext, dir: &Path, label: &str) {
    crate::qa_workbench_tests::open(wb, dir, cx);
    let task = find(wb, cx, label);
    wb.update_in(cx, |wb, window, cx| wb.start_task_run(task, After::Nothing, window, cx));
}

#[gpui::test]
fn a_task_graph_runs_its_dependency_first_with_literal_arguments_and_problems(cx: &mut gpui::TestAppContext) {
    let dir = project(
        "task-run-graph",
        r#"{ "version": "2.0.0", "tasks": [
            { "label": "prep", "type": "process", "command": "/bin/sh",
              "args": ["-c", "printf 'prep [%s]\\n' \"$1\"; echo 'src/a.rs:1: error: boom'", "sh", "two words; $HOME"],
              "problemMatcher": { "owner": "t", "pattern": { "regexp": "^(.*):(\\d+): (error): (.*)$", "file": 1, "line": 2, "severity": 3, "message": 4 } } },
            { "label": "all", "type": "shell", "command": "echo \"done-${input:who}\"", "options": { "shell": { "executable": "/bin/sh", "args": ["-c"] } },
              "dependsOn": "prep" } ],
          "inputs": [ { "id": "who", "type": "promptString", "description": "Who?" } ] }"#,
    );
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, ..Settings::default() });
    start(&wb, cx, &dir, "all");
    wait_ticking(cx, "the input question", |cx| wb.read_with(cx, |wb, _| wb.jobs.pending.as_ref().is_some_and(|p| p.left.len() == 1)));
    wb.update_in(cx, |wb, window, cx| {
        wb.close_overlay(window, cx);
        wb.answer_task_input("Ada; rm -rf ~".into(), window, cx)
    });
    wait_ticking(cx, "the run to finish", |cx| last_run_finished(&wb, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.jobs.runs.last().unwrap().run.outcome().clone()), State::Succeeded);
    assert!(terminal_text(&wb, cx, "prep").contains("prep [two words; $HOME]"), "{}", terminal_text(&wb, cx, "prep"));
    assert!(terminal_text(&wb, cx, "all").contains("done-Ada; rm -rf ~"), "the answer stays one value: {}", terminal_text(&wb, cx, "all"));
    let problems = wb.read_with(cx, |wb, _| wb.problems());
    assert!(problems.iter().any(|(p, n)| p.ends_with("/src/a.rs") && *n == 1), "{problems:?}");
}

#[gpui::test]
fn a_failed_dependency_stops_the_run_and_says_why(cx: &mut gpui::TestAppContext) {
    let dir = project(
        "task-run-fail",
        r#"{ "version": "2.0.0", "tasks": [ { "label": "bad", "command": "exit 3" }, { "label": "after", "command": "touch should-not-exist", "dependsOn": "bad" } ] }"#,
    );
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, ..Settings::default() });
    start(&wb, cx, &dir, "after");
    wait_ticking(cx, "the run to finish", |cx| last_run_finished(&wb, cx));
    assert!(!dir.join("should-not-exist").exists(), "a dependent of a failed task never starts");
    let notice = notice(&wb, cx);
    assert!(notice.starts_with("error: ") && notice.contains("after") && notice.contains("dependency"), "{notice}");
}

#[gpui::test]
fn stop_ends_the_task_and_everything_it_started(cx: &mut gpui::TestAppContext) {
    let dir =
        project("task-run-stop", r#"{ "version": "2.0.0", "tasks": [ { "label": "serve", "command": "sleep 300 & echo $! > child.pid; echo up; wait" } ] }"#);
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, ..Settings::default() });
    start(&wb, cx, &dir, "serve");
    wait_ticking(cx, "the child process", |_| std::fs::read_to_string(dir.join("child.pid")).is_ok_and(|s| s.ends_with('\n')));
    let child: i32 = std::fs::read_to_string(dir.join("child.pid")).unwrap().trim().parse().unwrap();
    wb.update(cx, |wb, cx| wb.terminate_tasks(cx));
    wait_ticking(cx, "the run to stop", |cx| last_run_finished(&wb, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.jobs.runs.last().unwrap().run.outcome().clone()), State::Cancelled);
    // SAFETY: signal 0 only asks whether the process exists.
    wait_ticking(cx, "the child to be gone", |_| unsafe { libc::kill(child, 0) } != 0);
}

#[gpui::test]
fn dismissing_an_input_starts_nothing(cx: &mut gpui::TestAppContext) {
    let dir = project(
        "task-run-cancel-input",
        r#"{ "version": "2.0.0", "tasks": [ { "label": "a", "command": "touch ran-${input:x}" } ],
            "inputs": [ { "id": "x", "type": "pickString", "description": "x", "options": ["1", "2"] } ] }"#,
    );
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, ..Settings::default() });
    start(&wb, cx, &dir, "a");
    wait_ticking(cx, "the question", |cx| wb.read_with(cx, |wb, _| matches!(wb.overlay.as_ref().map(|o| &o.kind), Some(OverlayKind::TaskInput(_)))));
    wb.update_in(cx, |wb, window, cx| {
        wb.close_overlay(window, cx);
    });
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.jobs.pending.is_none() && wb.jobs.runs.is_empty()));
    assert_eq!(notice(&wb, cx), "info: Task input cancelled");
    assert!(!dir.join("ran-1").exists() && !dir.join("ran-2").exists());
}

#[gpui::test]
fn debugging_waits_for_the_pre_launch_task_and_never_starts_after_it_fails(cx: &mut gpui::TestAppContext) {
    let dir = project("task-run-prelaunch", r#"{ "version": "2.0.0", "tasks": [ { "label": "build", "command": "echo building; exit 1" } ] }"#);
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, ..Settings::default() });
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let build = find(&wb, cx, "build");
    let launch = r#"{ "configurations": [ { "name": "Run", "type": "lldb", "request": "launch", "program": "/bin/true", "preLaunchTask": "build" } ] }"#;
    let root = dir.to_string_lossy().into_owned();
    let config = crate::debug_config::parse_launch_json(launch, &root, None, &[build]).remove(0);
    assert_eq!(config.pre_launch_task.as_deref(), Some("build"));
    assert!(config.pre_launch.is_none(), "a task from a task file is not run as a plain command");
    wb.update(cx, |wb, cx| wb.start_debug(config, cx));
    wait_ticking(cx, "the preLaunchTask run to finish", |cx| last_run_finished(&wb, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.dbg.sessions.is_empty()), "no debug session after a failed preLaunchTask");
    let notice = notice(&wb, cx);
    assert!(notice.starts_with("error: ") && notice.contains("debugging was not started"), "{notice}");
}

#[gpui::test]
fn quitting_stops_running_tasks_before_the_app_ends(cx: &mut gpui::TestAppContext) {
    let dir = project("task-run-quit", r#"{ "version": "2.0.0", "tasks": [ { "label": "serve", "command": "echo $$ > leader.pid; exec sleep 300" } ] }"#);
    let (wb, cx) = setup_with(cx, Settings { lsp_enabled: false, ..Settings::default() });
    start(&wb, cx, &dir, "serve");
    wait_ticking(cx, "the task to run", |_| std::fs::read_to_string(dir.join("leader.pid")).is_ok_and(|s| s.ends_with('\n')));
    let leader: i32 = std::fs::read_to_string(dir.join("leader.pid")).unwrap().trim().parse().unwrap();
    wb.update(cx, |wb, _| wb.stop_task_runs_for_quit());
    // Synchronous: the process is gone before the quit handler returns, without running the executor.
    let gone = (0..100).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(10));
        // SAFETY: signal 0 only asks whether the process exists (a reaped leader is gone).
        (unsafe { libc::kill(leader, 0) }) != 0
    });
    assert!(gone, "the task outlived the quit");
}

/// Over SSH the graph runs on the server: literal arguments, a dependency first, and Stop ends the remote process tree.
#[gpui::test]
fn a_task_graph_runs_and_stops_on_the_ssh_host(cx: &mut gpui::TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("task-run-ssh") else { return };
    let dir = project(
        "task-run-ssh",
        r#"{ "version": "2.0.0", "tasks": [
            { "label": "prep", "type": "process", "command": "/bin/sh", "args": ["-c", "printf '%s' \"$1\" > arg.txt", "sh", "two words; $HOME"] },
            { "label": "serve", "command": "sleep 300 & echo $! > child.pid; wait", "dependsOn": "prep" } ] }"#,
    );
    let (wb, cx) = crate::qa_integration_tests::ssh_workbench(cx, &dir);
    assert!(wb.read_with(cx, |wb, _| wb.remote.host.is_remote()));
    let task = find(&wb, cx, "serve");
    wb.update_in(cx, |wb, window, cx| wb.start_task_run(task, After::Nothing, window, cx));
    wait_ticking(cx, "the remote child process", |_| std::fs::read_to_string(dir.join("child.pid")).is_ok_and(|s| s.ends_with('\n')));
    assert_eq!(std::fs::read_to_string(dir.join("arg.txt")).unwrap(), "two words; $HOME", "the argument arrived literally on the server");
    let child: i32 = std::fs::read_to_string(dir.join("child.pid")).unwrap().trim().parse().unwrap();
    wb.update(cx, |wb, cx| wb.terminate_tasks(cx));
    wait_ticking(cx, "the run to stop", |cx| last_run_finished(&wb, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.jobs.runs.last().unwrap().run.outcome().clone()), State::Cancelled);
    // SAFETY: signal 0 only asks whether the process exists.
    wait_ticking(cx, "the remote child to be gone", |_| (unsafe { libc::kill(child, 0) }) != 0);
}
