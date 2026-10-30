//! Run and Debug with the scripted adapter (`dap::tests::write_fake_adapter`): breakpoints that persist, that can be
//! disabled or deactivated, that the adapter verifies or rejects, and function breakpoints. Skipped without python3.

use std::path::{Path, PathBuf};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::qa_integration_tests::wait_secs;
use crate::qa_workbench_tests::{open, project, restart_with, setup_persistent};
use crate::workbench::Workbench;

fn have_python() -> bool {
    if std::process::Command::new("python3").arg("-V").output().is_err() {
        crate::test_sshd::skip_missing("python3");
        return false;
    }
    true
}

/// A project with a 12-line `main.c` and a launch configuration that runs the scripted adapter; returns the log path.
fn fake_project(name: &str) -> (PathBuf, PathBuf) {
    let dir = project(name);
    let log = dir.join("dap.log");
    let script = crate::dap::tests::write_fake_adapter(&dir);
    std::fs::write(dir.join("main.c"), (1..=12).map(|i| format!("int line{i};\n")).collect::<String>()).unwrap();
    std::fs::create_dir_all(dir.join(".codeyard")).unwrap();
    std::fs::write(
        dir.join(".codeyard/launch.jsonc"),
        format!(
            r#"{{ "configurations": [ {{ "name": "Fake", "type": "custom", "request": "launch", "program": "/bin/true",
                "source": "${{workspaceFolder}}/main.c", "debugAdapter": {{ "command": "{}", "args": ["{}"] }} }} ] }}"#,
            script.display(),
            log.display()
        ),
    )
    .unwrap();
    (dir, log)
}

fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, label: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
}

fn open_main(wb: &Entity<Workbench>, dir: &Path, cx: &mut VisualTestContext) -> (String, Entity<crate::editor::EditorView>) {
    let main = dir.join("main.c").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    cx.run_until_parked();
    (main, wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap())
}

fn log_lines(log: &Path, prefix: &str) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().filter(|l| l.starts_with(prefix)).map(str::to_string).collect()
}

#[gpui::test]
fn breakpoints_keep_their_state_across_a_restart(cx: &mut TestAppContext) {
    let home = project("dbg-persist-home");
    let (dir, _) = fake_project("dbg-persist");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    let (main, ed) = open_main(&wb, &dir, cx);
    ed.update(cx, |e, cx| {
        e.toggle_breakpoint_at(3, cx);
        e.toggle_breakpoint_at(6, cx);
    });
    wb.update(cx, |wb, cx| wb.set_breakpoint_enabled_for_test(&main, 6, false, cx));
    wb.update(cx, |wb, cx| wb.add_function_breakpoint_for_test("helper", cx));
    // Lines added above move the breakpoints; the saved ones move with them.
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "// one\n// two\n", cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.breakpoints_for_test(&main)), vec![(5, true, "?"), (8, false, "?")]);

    let wb = restart_with(wb, cx, &home, crate::settings::Startup::Continue);
    wait_secs(cx, 10, "the folder to come back", |cx| wb.read_with(cx, |wb, cx| wb.root_for_test(cx)).is_some());
    assert_eq!(wb.read_with(cx, |wb, _| wb.breakpoints_for_test(&main)), vec![(5, true, "?"), (8, false, "?")], "saved with the project");
    let (_, ed) = open_main(&wb, &dir, cx);
    let shown: Vec<(usize, bool)> = ed.read_with(cx, |e, _| e.breakpoints().into_iter().map(|(l, m)| (l, !m.disabled)).collect());
    assert_eq!(shown, vec![(5, true), (8, false)], "the editor shows the disabled one as disabled");
}

#[gpui::test]
fn disabled_and_deactivated_breakpoints_are_not_sent_and_results_come_back(cx: &mut TestAppContext) {
    if !have_python() {
        return;
    }
    let home = project("dbg-send-home");
    let (dir, log) = fake_project("dbg-send");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    let (main, ed) = open_main(&wb, &dir, cx);
    ed.update(cx, |e, cx| {
        e.toggle_breakpoint_at(4, cx);
        e.toggle_breakpoint_at(6, cx);
        e.toggle_breakpoint_at(11, cx); // line 12: the adapter has no code there
    });
    wb.update(cx, |wb, cx| wb.set_breakpoint_enabled_for_test(&main, 6, false, cx));
    wb.update(cx, |wb, cx| {
        wb.add_function_breakpoint_for_test("helper", cx);
        wb.add_function_breakpoint_for_test("missing", cx);
    });

    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 10, "stopped on the first breakpoint", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint"));
    let sent = log_lines(&log, "setBreakpoints");
    assert!(sent[0].contains("\"lines\": [5, 12]"), "the disabled line is not sent: {sent:?}");
    assert!(log_lines(&log, "setFunctionBreakpoints")[0].contains("helper"));
    wait_secs(cx, 10, "results", |cx| wb.read_with(cx, |wb, _| wb.breakpoints_for_test(&main)) == vec![(4, true, "yes"), (6, false, "?"), (11, true, "no")]);
    let rings: Vec<bool> =
        ed.read_with(cx, |e, _| e.breakpoints().into_iter().map(|(_, m)| matches!(m.verified, crate::debug_model::Verified::No(_))).collect());
    assert_eq!(rings, vec![false, false, true], "the editor knows which one the adapter rejected");

    // Deactivated: an empty list goes to the adapter, the list stays.
    run(&wb, cx, "Debug: Deactivate/Activate Breakpoints");
    wait_secs(cx, 10, "an empty list sent", |_| log_lines(&log, "setBreakpoints").last().is_some_and(|l| l.contains("\"lines\": []")));
    assert_eq!(wb.read_with(cx, |wb, _| wb.breakpoints_for_test(&main)).len(), 3);
    run(&wb, cx, "Debug: Deactivate/Activate Breakpoints");
    wait_secs(cx, 10, "sent again", |_| log_lines(&log, "setBreakpoints").last().is_some_and(|l| l.contains("\"lines\": [5, 12]")));

    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 10, "ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
    assert!(wb.read_with(cx, |wb, _| wb.breakpoints_for_test(&main)).iter().all(|(.., v)| *v == "?"), "no session, no verdict");
}

#[gpui::test]
fn the_run_and_debug_view_shows_the_paused_program(cx: &mut TestAppContext) {
    if !have_python() {
        return;
    }
    let home = project("dbg-view-home");
    let (dir, _) = fake_project("dbg-view");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.show_panel_named("debug", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some("debug"));
    wait_secs(cx, 10, "the configuration selector", |cx| wb.read_with(cx, |wb, _| wb.debug_catalog_for_test()) == vec!["Fake".to_string()]);
    let (_, ed) = open_main(&wb, &dir, cx);
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(4, cx));
    assert!(!wb.read_with(cx, |wb, _| wb.toolbar_shown_for_test()));

    wb.update_in(cx, |wb, window, cx| wb.start_selected_for_test(window, cx));
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 10, "paused", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint" && !s.1.is_empty()));
    assert!(wb.read_with(cx, |wb, _| wb.toolbar_shown_for_test()), "the floating toolbar shows while the session lives");
    let (vars, stack) = wb.read_with(cx, |wb, _| wb.run_debug_rows_for_test());
    assert_eq!(vars, vec!["Locals: ", "  x: 50", "  point: {x, y}"]);
    assert_eq!(stack, vec!["thread:main*", "frame:main*", "frame:caller", "thread:worker"], "two threads: a tree with the paused one open");
    wb.update(cx, |wb, cx| wb.toggle_variable_for_test("point", cx));
    wait_secs(cx, 10, "the child values", |cx| wb.read_with(cx, |wb, _| wb.run_debug_rows_for_test().0.len()) == 5);
    assert_eq!(wb.read_with(cx, |wb, _| wb.run_debug_rows_for_test().0)[3], "    x: 1");

    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 10, "ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
    assert!(!wb.read_with(cx, |wb, _| wb.toolbar_shown_for_test()));
}

/// The adapter's extras: a 250-element value paged by 100, a 45-frame stack paged by 20, Set Value, completions and
/// Restart Frame.
#[gpui::test]
fn values_pages_stack_pages_set_value_completion_and_restart_frame(cx: &mut TestAppContext) {
    if !have_python() {
        return;
    }
    let home = project("dbg-depth-home");
    let (dir, _) = fake_project("dbg-depth");
    let launch =
        std::fs::read_to_string(dir.join(".codeyard/launch.jsonc")).unwrap().replace("\"request\": \"launch\",", "\"request\": \"launch\", \"extras\": true,");
    std::fs::write(dir.join(".codeyard/launch.jsonc"), launch).unwrap();
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    let (_, ed) = open_main(&wb, &dir, cx);
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(4, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 10, "paused", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint" && !s.1.is_empty()));
    let rows = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.run_debug_rows_for_test());

    // The stack: the first 20 frames, then "Load More".
    let frames = |cx: &mut VisualTestContext| rows(cx).1.into_iter().filter(|r| r.starts_with("frame:") || r == "more").collect::<Vec<_>>();
    assert_eq!(frames(cx).len(), 21, "{:?}", frames(cx));
    assert_eq!(frames(cx).last().map(String::as_str), Some("more"));
    wb.update(cx, |wb, cx| wb.load_more_frames(cx));
    wait_secs(cx, 10, "40 frames", |cx| frames(cx).len() == 41);
    wb.update(cx, |wb, cx| wb.load_more_frames(cx));
    wait_secs(cx, 10, "all 45, no more", |cx| frames(cx).len() == 45 && frames(cx).last().is_some_and(|r| r != "more"));

    // A big value: its named field and the first 100 elements, then "… 150 more".
    wb.update(cx, |wb, cx| wb.toggle_variable_for_test("big", cx));
    wait_secs(cx, 10, "first page", |cx| rows(cx).0.iter().any(|r| r.contains("… 150 more")));
    let vars = rows(cx).0;
    assert!(vars.contains(&"    length: 250".to_string()) && vars.contains(&"    [99]: 9801".to_string()) && !vars.iter().any(|r| r.contains("[100]")));
    let big = wb.read_with(cx, |wb, _| wb.reference_for_test("big")).unwrap();
    wb.update(cx, |wb, cx| wb.load_more_variables(big, cx));
    wait_secs(cx, 10, "second page", |cx| rows(cx).0.iter().any(|r| r.contains("… 50 more")));

    // Set Value goes to the adapter and the row shows the new value.
    wb.update(cx, |wb, cx| wb.set_variable_for_test("x", "7", cx));
    wait_secs(cx, 10, "the new value", |cx| rows(cx).0.contains(&"  x: 7".to_string()));

    // Tab in the console: one completion fills in; several are listed.
    wb.update(cx, |wb, cx| wb.set_console_input_for_test("1 + poi", cx));
    wb.update(cx, |wb, cx| wb.console_complete(cx));
    wait_secs(cx, 10, "completed", |cx| wb.read_with(cx, |wb, cx| wb.console_input_for_test(cx)) == "1 + point");
    wb.update(cx, |wb, cx| wb.set_console_input_for_test("p", cx));
    wb.update(cx, |wb, cx| wb.console_complete(cx));
    wait_secs(cx, 10, "listed", |cx| snap(cx).is_some_and(|s| s.3.iter().any(|l| l == "point  print")));

    // History: ↑ brings back earlier inputs.
    wb.update(cx, |wb, cx| wb.evaluate_for_test("1 + 1", cx));
    wb.update(cx, |wb, cx| wb.evaluate_for_test("2 + 2", cx));
    wb.update(cx, |wb, cx| wb.console_history(true, cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.console_input_for_test(cx)), "2 + 2");
    wb.update(cx, |wb, cx| wb.console_history(true, cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.console_input_for_test(cx)), "1 + 1");
    wb.update(cx, |wb, cx| wb.console_history(false, cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.console_input_for_test(cx)), "2 + 2");

    // Restart Frame: the adapter reports a stop for it.
    wb.update(cx, |wb, cx| wb.restart_frame(0, cx));
    wait_secs(cx, 10, "restarted", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:restart"));
    run(&wb, cx, "Debug: Stop");
}

/// Writes launch.jsonc with a configuration per (name, extra JSON fields), all running the scripted adapter.
fn write_launch(dir: &Path, log: &Path, configs: &[(&str, &str)], rest: &str) {
    let script = dir.join("fake-dap.py");
    let configs: Vec<String> = configs
        .iter()
        .map(|(name, extra)| {
            format!(
                r#"{{ "name": "{name}", "type": "custom", "request": "launch", "program": "/bin/true", "source": "${{workspaceFolder}}/main.c",
                   "debugAdapter": {{ "command": "{}", "args": ["{}"] }} {extra} }}"#,
                script.display(),
                log.display()
            )
        })
        .collect();
    std::fs::write(dir.join(".codeyard/launch.jsonc"), format!(r#"{{ "configurations": [ {} ] {rest} }}"#, configs.join(", "))).unwrap();
}

#[gpui::test]
fn run_without_debugging_sends_no_breakpoints_and_the_post_debug_task_runs(cx: &mut TestAppContext) {
    if !have_python() {
        return;
    }
    let home = project("dbg-nodebug-home");
    let (dir, log) = fake_project("dbg-nodebug");
    let marker = dir.join("post-task-ran");
    std::fs::write(dir.join(".codeyard/tasks.jsonc"), format!(r#"[ {{ "label": "after", "command": "touch {}" }} ]"#, marker.display())).unwrap();
    write_launch(&dir, &log, &[("Fake", r#", "postDebugTask": "after""#)], "");
    let (wb, cx) = setup_persistent(cx, &home);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    open(&wb, &dir, cx);
    let (_, ed) = open_main(&wb, &dir, cx);
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(4, cx));
    run(&wb, cx, "Debug: Run Without Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 10, "ran to the end", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
    assert!(snap(cx).unwrap().3.iter().any(|l| l.contains("ran without debugging")));
    let launch = log_lines(&log, "launch");
    assert!(launch[0].contains("\"noDebug\": true"), "{launch:?}");
    assert!(log_lines(&log, "setBreakpoints").is_empty(), "no breakpoints for a run without debugging");
    wait_secs(cx, 10, "the post-debug task", |_| marker.exists());
}

#[gpui::test]
fn run_to_cursor_stops_there_once(cx: &mut TestAppContext) {
    if !have_python() {
        return;
    }
    let home = project("dbg-cursor-home");
    let (dir, log) = fake_project("dbg-cursor");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    let (main, ed) = open_main(&wb, &dir, cx);
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(2, cx));
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 10, "paused on line 3", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint" && s.2 == Some((main.clone(), 2))));
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(7, 0, cx));
    run(&wb, cx, "Debug: Run to Cursor");
    wait_secs(cx, 10, "stopped at the cursor", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint" && s.2 == Some((main.clone(), 7))));
    wait_secs(cx, 10, "the temporary breakpoint is taken back", |_| log_lines(&log, "setBreakpoints").last().is_some_and(|l| l.contains("\"lines\": [3]")));
    assert_eq!(wb.read_with(cx, |wb, _| wb.breakpoints_for_test(&main)).len(), 1, "never listed");
    run(&wb, cx, "Debug: Stop");
}

#[gpui::test]
fn a_compound_starts_its_sessions_together_and_stop_all_stops_them(cx: &mut TestAppContext) {
    if !have_python() {
        return;
    }
    let home = project("dbg-compound-home");
    let (dir, log) = fake_project("dbg-compound");
    write_launch(&dir, &log, &[("One", ""), ("Two", "")], r#", "compounds": [ { "name": "Both", "configurations": ["One", "Two"], "stopAll": true } ]"#);
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.show_panel_named("debug", window, cx));
    wait_secs(cx, 10, "the catalog", |cx| wb.read_with(cx, |wb, _| wb.debug_catalog_for_test()) == vec!["One", "Two", "Both"]);
    wb.update_in(cx, |wb, window, cx| wb.start_named_for_test("Both", window, cx));
    let stack = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.run_debug_rows_for_test().1);
    wait_secs(cx, 10, "two sessions", |cx| stack(cx).iter().filter(|r| r.starts_with("session:")).count() == 2);
    wait_secs(cx, 10, "both paused", |cx| stack(cx).iter().filter(|r| r.contains("paused on")).count() == 2);
    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 10, "both ended", |cx| stack(cx).iter().filter(|r| r.contains("(ended)")).count() == 2);
}

/// An adapter (or whatever answered on its port) asking to run a program in a terminal that was never offered is refused.
#[gpui::test]
fn a_terminal_that_was_not_offered_is_refused(cx: &mut TestAppContext) {
    if !have_python() {
        return;
    }
    let home = project("dbg-sneak-home");
    let (dir, log) = fake_project("dbg-sneak");
    let marker = dir.join("terminal-ran");
    write_launch(&dir, &log, &[("Fake", &format!(r#", "sneak": true, "marker": "{}""#, marker.display()))], "");
    let (wb, cx) = setup_persistent(cx, &home);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    open(&wb, &dir, cx);
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    // The adapter asks for the terminal before it stops, so by then the answer was given.
    wait_secs(cx, 10, "started", |cx| snap(cx).is_some_and(|s| s.0.starts_with("stopped")));
    assert!(log_lines(&log, "initialize")[0].contains("\"supportsRunInTerminalRequest\": false"));
    std::thread::sleep(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert!(!marker.exists(), "a terminal request that was not offered must not run anything");
    run(&wb, cx, "Debug: Stop");
}

#[gpui::test]
fn inputs_are_asked_before_the_start_and_the_program_can_run_in_a_terminal(cx: &mut TestAppContext) {
    if !have_python() {
        return;
    }
    let home = project("dbg-inputs-home");
    let (dir, log) = fake_project("dbg-inputs");
    let marker = dir.join("terminal-ran");
    write_launch(
        &dir,
        &log,
        &[("Fake", &format!(r#", "args": ["${{input:level}}", "${{input:name}}"], "console": "integratedTerminal", "marker": "{}""#, marker.display()))],
        r#", "inputs": [ { "id": "name", "type": "promptString", "description": "Your name" }, { "id": "level", "type": "pickString", "description": "Level", "options": ["low", "high"] } ]"#,
    );
    let (wb, cx) = setup_persistent(cx, &home);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    open(&wb, &dir, cx);
    run(&wb, cx, "Debug: Start Debugging");
    // First question (in order of use): pick a level; then type a name.
    wait_secs(cx, 10, "the level question", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some_and(|l| l == ["low", "high"]));
    wb.update_in(cx, |wb, window, cx| wb.answer_launch_input_for_test("high", window, cx));
    wb.update_in(cx, |wb, window, cx| wb.answer_launch_input_for_test("Ada", window, cx));
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 10, "started", |cx| snap(cx).is_some_and(|s| s.0.starts_with("stopped")));
    let launch = log_lines(&log, "launch");
    assert!(launch[0].contains("\"args\": [\"high\", \"Ada\"]"), "{launch:?}");
    assert!(log_lines(&log, "initialize")[0].contains("\"supportsRunInTerminalRequest\": true"));
    wait_secs(cx, 10, "the program ran in a terminal", |_| std::fs::read_to_string(&marker).is_ok_and(|t| t.trim() == "ran-in-terminal"));
    run(&wb, cx, "Debug: Stop");
}

/// "Break on Value Change" on a variable asks the adapter for the value's id (in that variable's scope) and sets a
/// data breakpoint; it is listed, turned off and on (a disabled one is not sent, nor any while breakpoints are
/// deactivated), removed, and belongs to its session only.
#[gpui::test]
fn data_breakpoints_from_a_variable_are_listed_toggled_removed_and_end_with_the_session(cx: &mut TestAppContext) {
    if !have_python() {
        return;
    }
    let home = project("dbg-data-home");
    let (dir, log) = fake_project("dbg-data");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    run(&wb, cx, "Debug: Start Debugging");
    let snap = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.debug_snapshot());
    wait_secs(cx, 10, "stopped", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint"));
    wait_secs(cx, 10, "the variables", |cx| wb.read_with(cx, |wb, _| wb.reference_for_test("x")).is_some());
    wb.update_in(cx, |wb, window, cx| wb.open_variable_menu_for_test("x", window, cx));
    let labels = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).expect("the variable's menu");
    assert!(labels.iter().any(|l| l == "Break on Value Change"), "{labels:?}");
    wb.update(cx, |wb, cx| wb.select_menu_for_test("Break on Value Change", cx));
    let states = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.data_breakpoint_states_for_test());
    wait_secs(cx, 10, "the data breakpoint", |cx| states(cx) == vec![("x (4 bytes)".to_string(), true)]);
    assert!(log_lines(&log, "dataBreakpointInfo")[0].contains("\"variablesReference\": 10"), "asked in the variable's own scope");
    assert!(log_lines(&log, "setDataBreakpoints").last().unwrap().contains("var:x"));

    wb.update(cx, |wb, cx| wb.set_data_breakpoint_enabled_for_test(0, false, cx));
    wait_secs(cx, 10, "an empty list sent", |_| log_lines(&log, "setDataBreakpoints").last().is_some_and(|l| l.contains("\"breakpoints\": []")));
    assert_eq!(states(cx), vec![("x (4 bytes)".to_string(), false)], "still listed, turned off");
    wb.update(cx, |wb, cx| wb.set_data_breakpoint_enabled_for_test(0, true, cx));
    wait_secs(cx, 10, "sent again", |_| log_lines(&log, "setDataBreakpoints").last().is_some_and(|l| l.contains("var:x")));
    run(&wb, cx, "Debug: Deactivate/Activate Breakpoints");
    wait_secs(cx, 10, "none while deactivated", |_| log_lines(&log, "setDataBreakpoints").last().is_some_and(|l| l.contains("\"breakpoints\": []")));
    run(&wb, cx, "Debug: Deactivate/Activate Breakpoints");
    wait_secs(cx, 10, "back when active", |_| log_lines(&log, "setDataBreakpoints").last().is_some_and(|l| l.contains("var:x")));

    wb.update(cx, |wb, cx| wb.remove_data_breakpoint_for_test(0, cx));
    assert!(states(cx).is_empty());
    wait_secs(cx, 10, "removed", |_| log_lines(&log, "setDataBreakpoints").last().is_some_and(|l| l.contains("\"breakpoints\": []")));

    // A new one ends with its session: the next run starts without it.
    wb.update(cx, |wb, cx| wb.add_data_breakpoint_for_test("x", cx));
    wait_secs(cx, 10, "set again", |cx| states(cx).len() == 1);
    run(&wb, cx, "Debug: Stop");
    wait_secs(cx, 10, "ended", |cx| snap(cx).is_some_and(|s| s.0 == "ended"));
    run(&wb, cx, "Debug: Start Debugging");
    wait_secs(cx, 10, "stopped again", |cx| snap(cx).is_some_and(|s| s.0 == "stopped:breakpoint"));
    assert!(states(cx).is_empty(), "data breakpoints are not carried into another session");
    run(&wb, cx, "Debug: Stop");
}
