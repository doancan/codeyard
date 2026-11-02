use std::collections::BTreeMap;

use serde_json::Value;
use task_engine::model::{Platform, Quoting};
use task_engine::schedule::{Failure, State};
use task_engine::variables::Context;

use super::*;

fn with_context<R>(platform: Platform, results: &[(&str, &str)], f: impl FnOnce(&Context<'_>) -> R) -> R {
    let folders = BTreeMap::new();
    let environment = BTreeMap::from([("HOME".to_string(), "/home/u".to_string())]);
    let config = BTreeMap::<String, Value>::new();
    let results: BTreeMap<String, String> = results.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let context = Context {
        platform,
        root: "/work/app",
        folders: &folders,
        file: Some("/work/app/src/main.rs"),
        cursor: None,
        selected: None,
        cwd: "/work/app",
        home: "/home/u",
        executable: "codeyard",
        default_build: None,
        environment: &environment,
        config: &config,
        results: &results,
    };
    f(&context)
}

fn spawn(text: &str, label: &str, platform: Platform, results: &[(&str, &str)]) -> Result<Spawn, String> {
    let prepared = prepare(text, platform, label)?;
    let task = &prepared.registry.tasks[prepared.plan.root];
    with_context(platform, results, |c| spawn_for(task, c))
}

#[test]
fn process_tasks_keep_every_argument_as_one_word() {
    let text = r#"{ "version": "2.0.0", "tasks": [ { "label": "p", "type": "process", "command": "tool",
        "args": ["two words", "'q' \"d\" $HOME; *", "", "${file}"], "options": { "env": { "A": "${userHome}/x", "B": null } } } ] }"#;
    let s = spawn(text, "p", Platform::Linux, &[]).unwrap();
    assert_eq!(
        s.command,
        proto::TaskCommand::Process {
            program: "tool".into(),
            args: vec!["two words".into(), "'q' \"d\" $HOME; *".into(), String::new(), "/work/app/src/main.rs".into()],
        }
    );
    assert_eq!(s.cwd.as_deref(), Some("/work/app"), "the folder is the default working directory");
    assert_eq!(s.env, vec![("A".into(), Some("/home/u/x".into())), ("B".into(), None)]);
}

#[test]
fn shell_tasks_quote_arguments_for_their_shell() {
    let posix = r#"{ "version": "2.0.0", "tasks": [ { "label": "s", "type": "shell", "command": "echo",
        "args": ["plain", "a b", "it's", { "value": "$HOME x", "quoting": "weak" }, { "value": "a b", "quoting": "escape" }] } ] }"#;
    let s = spawn(posix, "s", Platform::Linux, &[]).unwrap();
    assert_eq!(s.command, proto::TaskCommand::Shell { shell: None, args: None, line: r#"echo plain 'a b' 'it'\''s' "$HOME x" a\ b"#.into() });

    let s = spawn(
        r#"{ "version": "2.0.0", "tasks": [ { "label": "w", "type": "shell", "command": "echo", "args": ["a \"b\"", "x&y"] } ] }"#,
        "w",
        Platform::Windows,
        &[],
    )
    .unwrap();
    assert_eq!(s.command, proto::TaskCommand::Shell { shell: None, args: None, line: r#"echo "a ""b""" "x&y""#.into() });

    let pwsh = r#"{ "version": "2.0.0", "tasks": [ { "label": "p", "type": "shell", "command": "Write-Output", "args": ["it's $x"],
        "options": { "shell": { "executable": "pwsh", "args": ["-NoProfile", "-Command"] } } } ] }"#;
    let s = spawn(pwsh, "p", Platform::Windows, &[]).unwrap();
    assert_eq!(
        s.command,
        proto::TaskCommand::Shell {
            shell: Some("pwsh".into()),
            args: Some(vec!["-NoProfile".into(), "-Command".into()]),
            line: "Write-Output 'it''s $x'".into()
        }
    );
}

#[test]
fn quoting_round_trips_through_a_real_posix_shell() {
    if cfg!(windows) {
        return;
    }
    for value in ["a b", "it's", "$HOME", "`x`", "\\", "*", "", "\"q\"", "é ü"] {
        for q in [None, Some(Quoting::Strong), Some(Quoting::Escape)] {
            let word = quote(value, q, Family::Posix);
            let out = std::process::Command::new("/bin/sh").args(["-c", &format!("printf %s {word}")]).output().unwrap();
            if value.is_empty() && q == Some(Quoting::Escape) {
                continue;
            }
            assert_eq!(String::from_utf8_lossy(&out.stdout), value, "{word} ({q:?})");
        }
    }
}

#[test]
fn inputs_are_listed_once_and_unknown_or_command_inputs_are_refused() {
    let text = r#"{ "version": "2.0.0",
        "tasks": [ { "label": "a", "command": "echo ${input:name} ${input:name}", "dependsOn": "b" }, { "label": "b", "command": "echo ${input:mode}" } ],
        "inputs": [ { "id": "name", "type": "promptString", "description": "Name" }, { "id": "mode", "type": "pickString", "description": "Mode", "options": ["x", "y"] } ] }"#;
    let p = prepare(text, Platform::Linux, "a").unwrap();
    assert_eq!(inputs_needed(&p).unwrap(), vec!["mode".to_string(), "name".to_string()], "in the order the tasks run");
    let s = with_context(Platform::Linux, &[("input:name", "Ada"), ("input:mode", "x")], |c| spawn_for(&p.registry.tasks[p.plan.root], c)).unwrap();
    assert_eq!(s.command, proto::TaskCommand::Shell { shell: None, args: None, line: "echo Ada Ada".into() });

    let unknown = prepare(r#"{ "version": "2.0.0", "tasks": [ { "label": "a", "command": "echo ${input:nope}" } ] }"#, Platform::Linux, "a").unwrap();
    assert!(inputs_needed(&unknown).unwrap_err().contains("nope"));
    let command = prepare(r#"{ "version": "2.0.0", "tasks": [ { "label": "a", "command": "echo ${command:x}" } ] }"#, Platform::Linux, "a").unwrap();
    assert!(inputs_needed(&command).is_err());
}

#[test]
fn values_in_a_shell_line_stay_one_word_wherever_they_land() {
    let text = r#"{ "version": "2.0.0", "tasks": [ { "label": "a", "type": "shell", "command": "cat ${input:f} '${input:f}' \"${input:f}\"" } ],
        "inputs": [ { "id": "f", "type": "promptString", "description": "f" } ] }"#;
    let evil = "a;rm -rf ~;$(id)'x\".txt";
    let s = spawn(text, "a", Platform::Linux, &[("input:f", evil)]).unwrap();
    let proto::TaskCommand::Shell { line, .. } = s.command else { panic!() };
    if !cfg!(windows) {
        let out = std::process::Command::new("/bin/sh").args(["-c", &line.replacen("cat", "printf '%s\\n'", 1)]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{evil}\n{evil}\n{evil}\n"), "{line}");
    }
    let s = spawn(text, "a", Platform::Windows, &[("input:f", "x\" & del y")]).unwrap();
    let proto::TaskCommand::Shell { line, .. } = s.command else { panic!() };
    // cmd.exe has no single quotes: the value between them is quoted on its own.
    assert_eq!(line, r#"cat "x"" & del y" '"x"" & del y"' "x"" & del y""#);
}

#[test]
fn a_value_is_expanded_once() {
    let text = r#"{ "version": "2.0.0", "tasks": [ { "label": "a", "type": "process", "command": "echo", "args": ["${input:x}"] } ],
        "inputs": [ { "id": "x", "type": "promptString", "description": "x" } ] }"#;
    let s = spawn(text, "a", Platform::Linux, &[("input:x", "${userHome}")]).unwrap();
    assert_eq!(s.command, proto::TaskCommand::Process { program: "echo".into(), args: vec!["${userHome}".into()] });
}

#[test]
fn cycles_and_missing_dependencies_are_reported_before_anything_runs() {
    let cycle =
        r#"{ "version": "2.0.0", "tasks": [ { "label": "a", "command": "x", "dependsOn": "b" }, { "label": "b", "command": "y", "dependsOn": "a" } ] }"#;
    assert!(prepare(cycle, Platform::Linux, "a").err().unwrap().starts_with("Task dependency cycle: "));
    let missing = r#"{ "version": "2.0.0", "tasks": [ { "label": "a", "command": "x", "dependsOn": "zzz" } ] }"#;
    assert!(prepare(missing, Platform::Linux, "a").err().unwrap().contains("zzz"));
}

const GRAPH: &str = r#"{ "version": "2.0.0", "tasks": [
    { "label": "all", "dependsOn": ["one", "two"], "dependsOrder": "sequence" },
    { "label": "one", "command": "a", "dependsOn": "shared" },
    { "label": "two", "command": "b", "dependsOn": "shared" },
    { "label": "shared", "command": "s" } ] }"#;

fn node(run: &Run, label: &str) -> usize {
    run.prepared.registry.tasks.iter().position(|t| t.label == label).unwrap()
}

#[test]
fn a_shared_prerequisite_runs_once_and_sequence_order_is_kept() {
    let mut run = Run::new(1, prepare(GRAPH, Platform::Linux, "all").unwrap()).unwrap();
    let (shared, one, two) = (node(&run, "shared"), node(&run, "one"), node(&run, "two"));
    assert_eq!(run.advance().start, vec![shared]);
    assert!(run.advance().start.is_empty(), "nothing starts twice");
    run.started(shared, 10);
    assert_eq!(run.exited(shared, Some(0)).start, vec![one]);
    run.started(one, 11);
    assert_eq!(run.exited(one, Some(0)).start, vec![two]);
    run.started(two, 12);
    run.exited(two, Some(0));
    assert_eq!(run.outcome(), &State::Succeeded);
    assert!(run.finished());
}

#[test]
fn a_failure_stops_the_dependents_and_a_late_event_changes_nothing() {
    let mut run = Run::new(1, prepare(GRAPH, Platform::Linux, "all").unwrap()).unwrap();
    let shared = node(&run, "shared");
    run.advance();
    run.started(shared, 10);
    run.exited(shared, Some(2));
    assert_eq!(run.state(shared), Some(&State::Failed(Failure::Exit(2))));
    assert!(matches!(run.outcome(), State::Failed(Failure::Dependency(_))));
    assert!(run.exited(shared, Some(0)).start.is_empty(), "a second exit does not revive the node");
    assert_eq!(run.state(shared), Some(&State::Failed(Failure::Exit(2))));
    assert!(run.finished());
}

#[test]
fn a_spawn_failure_fails_the_task_like_a_missing_command() {
    let mut run = Run::new(1, prepare(GRAPH, Platform::Linux, "all").unwrap()).unwrap();
    let shared = node(&run, "shared");
    run.advance();
    run.spawn_failed(shared);
    assert_eq!(run.state(shared), Some(&State::Failed(Failure::Exit(127))));
    assert!(run.finished());
}

const WATCH: &str = r#"{ "version": "2.0.0", "tasks": [
    { "label": "serve", "command": "start", "dependsOn": "watch" },
    { "label": "watch", "command": "w", "isBackground": true, "problemMatcher": {
        "owner": "x", "fileLocation": "absolute",
        "pattern": { "regexp": "^(.*):(\\d+): (error|warning): (.*)$", "file": 1, "line": 2, "severity": 3, "message": 4 },
        "background": { "activeOnStart": true, "beginsPattern": "^build start", "endsPattern": "^build done" } } } ] }"#;

#[test]
fn a_background_task_releases_its_dependents_only_on_its_end_pattern() {
    let mut run = Run::new(1, prepare(WATCH, Platform::Linux, "serve").unwrap()).unwrap();
    let (watch, serve) = (node(&run, "watch"), node(&run, "serve"));
    assert_eq!(run.advance().start, vec![watch]);
    run.started(watch, 5);
    let e = run.output(watch, "build start\n/a/b.rs:3: err".as_bytes());
    assert!(e.start.is_empty() && !e.problems, "half a line is not a problem yet");
    let e = run.output(watch, "or: bad thing\nbuild do".as_bytes());
    assert!(e.problems && e.start.is_empty());
    let e = run.output(watch, b"ne\n");
    assert_eq!(e.start, vec![serve], "ready after the end pattern");
    let problems = run.problems();
    assert_eq!(problems.len(), 1);
    assert_eq!((problems[0].1.file.as_str(), problems[0].1.line, problems[0].1.message.as_str()), ("/a/b.rs", 3, "bad thing"));
    // A new compilation clears the old problems.
    assert!(run.output(watch, b"build start\n").problems);
    assert!(run.problems().is_empty());
}

#[test]
fn a_background_task_that_ends_before_it_is_ready_cannot_release_anything() {
    let mut run = Run::new(1, prepare(WATCH, Platform::Linux, "serve").unwrap()).unwrap();
    let watch = node(&run, "watch");
    run.advance();
    run.started(watch, 5);
    let e = run.exited(watch, Some(0));
    assert!(e.start.is_empty());
    assert_eq!(run.state(watch), Some(&State::Failed(Failure::BeforeReady)));
}

#[test]
fn cancel_stops_each_live_process_once_and_nothing_starts_after_it() {
    let mut run = Run::new(1, prepare(WATCH, Platform::Linux, "serve").unwrap()).unwrap();
    let watch = node(&run, "watch");
    run.advance();
    run.started(watch, 5);
    assert_eq!(run.cancel(), vec![5]);
    assert!(run.cancel().is_empty(), "asked once");
    assert_eq!(run.unconfirmed(), 1);
    assert!(run.output(watch, b"build done\n").start.is_empty(), "readiness after Stop starts nothing");
    run.exited(watch, None);
    assert_eq!(run.unconfirmed(), 0);
    assert_eq!(run.outcome(), &State::Cancelled);
    assert!(run.finished());
}

#[test]
fn a_malformed_matcher_fails_before_anything_runs() {
    let text = r#"{ "version": "2.0.0", "tasks": [ { "label": "a", "command": "x", "problemMatcher": { "pattern": { "regexp": "(", "file": 1 } } } ] }"#;
    assert!(Run::new(1, prepare(text, Platform::Linux, "a").unwrap()).err().unwrap().contains("regular expression is invalid"));
    let group = r#"{ "version": "2.0.0", "tasks": [ { "label": "a", "command": "x", "problemMatcher": { "pattern": { "regexp": "^x$", "file": 1 } } } ] }"#;
    let message = Run::new(1, prepare(group, Platform::Linux, "a").unwrap()).err().unwrap();
    assert!(message.ends_with("a field refers to a group the regular expression does not have"), "{message}");
}

/// cmd.exe words follow the usual Windows argument rules: quotes doubled, backslashes before a quote or the end doubled,
/// and `%` taken out of the quotes as `^%` so `%NAME%` is not expanded (weak quoting keeps variables live).
#[test]
fn cmd_words_survive_the_windows_argument_rules_and_percent_expansion() {
    assert_eq!(quote(r"C:\dir\", Some(Quoting::Strong), Family::Cmd), r#""C:\dir\\""#);
    assert_eq!(quote(r"C:\a b\", None, Family::Cmd), r#""C:\a b\\""#);
    assert_eq!(quote(r#"a\"b"#, None, Family::Cmd), r#""a\\""b""#);
    assert_eq!(quote("%PATH%", None, Family::Cmd), r#"""^%"PATH"^%"""#);
    assert_eq!(quote("%PATH%", Some(Quoting::Weak), Family::Cmd), r#""%PATH%""#);
    assert_eq!(quote(r"x\%y", None, Family::Cmd), r#""x\\"^%"y""#);
    assert_eq!(quote("", None, Family::Cmd), r#""""#);
    // Inside the user's own quotes the same rules hold.
    assert_eq!(place("50% \"x\"", crate::workbench::tasks::Quote::Double, Family::Cmd), r#"50"^%" ""x"""#);
}

/// Words every Windows shell must hand to a program unchanged.
#[cfg(windows)]
const WINDOWS_VALUES: &[&str] = &[
    "plain",
    "a b",
    "it's",
    "\"q\"",
    "say \"hi\" now",
    "%PATH%",
    "100%",
    "%",
    "a^b",
    "x&y",
    "a|b",
    "<in>",
    "a>b",
    "(paren)",
    "semi;colon",
    "Türkçe ıİşğ é ü",
    r"C:\dir\",
    r"C:\a b\",
    r"\\server\share\",
    r#"back\"slash"#,
    "",
];

/// Values PowerShell 5.1 (`powershell.exe`) cannot pass to a program unchanged: its legacy native-argument passing
/// drops empty arguments and does not escape quotes or trailing backslashes. PowerShell 7 (`pwsh`) passes them all.
#[cfg(windows)]
fn legacy_powershell_breaks(value: &str) -> bool {
    value.is_empty() || value.contains('"') || value.ends_with('\\')
}

/// Run `command` as a task (through the agent, as the app does) and read the arguments the program received.
#[cfg(windows)]
fn received(command: proto::TaskCommand) -> Vec<String> {
    use std::io::Read;
    let (mut task, mut out) = agent::proc::spawn_task(&command, None, &[]).unwrap();
    let mut text = String::new();
    out.read_to_string(&mut text).unwrap();
    assert_eq!(task.wait(), Some(0), "{command:?}: {text}");
    let line = text.lines().rev().find(|l| l.starts_with('[')).unwrap_or_else(|| panic!("{command:?}: no argument list in {text:?}"));
    serde_json::from_str(line).unwrap()
}

/// A task that runs Python with `values` as its arguments; Python prints the list it got as JSON.
#[cfg(windows)]
fn echo_task(kind: &str, values: &[&str], shell: Option<(&str, &[&str])>, quoting: Option<&str>) -> proto::TaskCommand {
    const ECHO: &str = "import sys,json;print(json.dumps(sys.argv[1:]))";
    let mut args = vec![serde_json::json!("-c"), serde_json::json!(ECHO)];
    args.extend(values.iter().map(|v| match quoting {
        Some(q) => serde_json::json!({ "value": v, "quoting": q }),
        None => serde_json::json!(v),
    }));
    let mut task = serde_json::json!({ "label": "echo", "type": kind, "command": "python", "args": args });
    if let Some((exe, shell_args)) = shell {
        task["options"] = serde_json::json!({ "shell": { "executable": exe, "args": shell_args } });
    }
    let text = serde_json::json!({ "version": "2.0.0", "tasks": [task] }).to_string();
    spawn(&text, "echo", Platform::Windows, &[]).unwrap().command
}

/// Whether `program` can be started (PowerShell 7 is not on every machine).
#[cfg(windows)]
fn available(program: &str) -> bool {
    std::process::Command::new(program).args(["-NoProfile", "-Command", "exit 0"]).output().is_ok_and(|o| o.status.success())
}

/// A process task gets every argument as it was; a cmd.exe shell task too, with automatic and strong quoting.
#[cfg(windows)]
#[test]
fn windows_task_arguments_reach_the_program_unchanged_through_cmd() {
    let want: Vec<String> = WINDOWS_VALUES.iter().map(|v| v.to_string()).collect();
    assert_eq!(received(echo_task("process", WINDOWS_VALUES, None, None)), want, "process task");
    assert_eq!(received(echo_task("shell", WINDOWS_VALUES, None, None)), want, "cmd.exe, automatic quoting");
    assert_eq!(received(echo_task("shell", WINDOWS_VALUES, None, Some("strong"))), want, "cmd.exe, strong quoting");
}

/// PowerShell shell tasks: PowerShell 7 hands every value over unchanged; Windows PowerShell 5.1 every value its own
/// argument passing can carry.
#[cfg(windows)]
#[test]
fn windows_task_arguments_reach_the_program_unchanged_through_powershell() {
    let shell_args: &[&str] = &["-NoProfile", "-Command"];
    if available("pwsh") {
        let want: Vec<String> = WINDOWS_VALUES.iter().map(|v| v.to_string()).collect();
        assert_eq!(received(echo_task("shell", WINDOWS_VALUES, Some(("pwsh", shell_args)), None)), want, "pwsh");
    } else {
        eprintln!("SKIPPED pwsh: PowerShell 7 is not installed");
    }
    let values: Vec<&str> = WINDOWS_VALUES.iter().copied().filter(|v| !legacy_powershell_breaks(v)).collect();
    let want: Vec<String> = values.iter().map(|v| v.to_string()).collect();
    assert_eq!(received(echo_task("shell", &values, Some(("powershell", shell_args)), None)), want, "powershell.exe 5.1");
}
