use super::*;
use serde_json::json;

fn registry(tasks: Value) -> Registry {
    Registry::parse(&json!({"version":"2.0.0","tasks":tasks}).to_string(), Platform::Linux).unwrap()
}
fn labels(registry: &Registry, ids: impl IntoIterator<Item = usize>) -> Vec<String> {
    ids.into_iter().map(|id| registry.tasks[id].label.clone()).collect()
}

#[test]
fn task_plan_preserves_process_arguments_platform_precedence_and_unset_environment() {
    let text = json!({
        "options":{"env":{"GLOBAL":"base","KEEP":"value"},"cwd":"${workspaceFolder}"},
        "windows":{"options":{"env":{"GLOBAL":"windows"}}},
        "tasks":[{"label":"build","type":"process","command":"a program","args":["şablon $() ; x",{"value":"literal value","quoting":"strong"}],
            "options":{"env":{"OWN":"local"}},"windows":{"command":"build.exe","options":{"env":{"GLOBAL":null}}}}]
    })
    .to_string();
    let registry = Registry::parse(&text, Platform::Windows).unwrap();
    let task = &registry.tasks[0];
    assert_eq!(task.kind, Kind::Process);
    assert_eq!(task.command.as_ref().unwrap().value, "build.exe");
    assert_eq!(task.args[0].value, "şablon $() ; x");
    assert_eq!(task.args[1].quoting, Some(Quoting::Strong));
    assert_eq!(task.options.env["GLOBAL"], None);
    assert_eq!(task.options.env["KEEP"].as_deref(), Some("value"));
    assert_eq!(task.options.env["OWN"].as_deref(), Some("local"));
    assert_eq!(Registry::parse(&text, Platform::Mac).unwrap().tasks[0].command.as_ref().unwrap().value, "a program");
}

#[test]
fn task_plan_parallel_dependencies_share_a_prerequisite_without_serializing_siblings() {
    let registry = registry(json!([
        {"label":"shared","command":"shared"},{"label":"a","command":"a","dependsOn":"shared"},
        {"label":"b","command":"b","dependsOn":"shared"},{"label":"all","dependsOn":["a","b"]}
    ]));
    let plan = registry.plan("all", false).unwrap();
    assert_eq!(labels(&registry, plan.order), ["shared", "a", "b", "all"]);
    assert_eq!(plan.waits[&1], BTreeSet::from([0]));
    assert_eq!(plan.waits[&2], BTreeSet::from([0]));
}

#[test]
fn task_plan_sequence_delays_the_later_subtree_and_reuses_shared_dependencies() {
    let registry = registry(json!([
        {"label":"shared","command":"shared"}, {"label":"first","command":"first","dependsOn":"shared"},
        {"label":"later-child","command":"child","dependsOn":"shared"},
        {"label":"later","command":"later","dependsOn":["shared","later-child"]},
        {"label":"all","dependsOrder":"sequence","dependsOn":["first","later"]}
    ]));
    let plan = registry.plan("all", false).unwrap();
    assert!(plan.waits[&2].contains(&1), "the next dependency's child must wait for the prior dependency too");
    assert!(!plan.waits[&0].contains(&1), "an already shared prerequisite must not wait for its dependent");
    assert_eq!(labels(&registry, plan.order), ["shared", "first", "later-child", "later", "all"]);
}

#[test]
fn task_plan_rejects_original_cycles_and_conflicting_sequence_barriers_before_execution() {
    let cycle = registry(json!([{"label":"a","command":"a","dependsOn":"b"},{"label":"b","command":"b","dependsOn":"a"}]));
    assert!(matches!(cycle.plan("a", false), Err(Error::Cycle(path)) if path.first() == path.last() && path.len() == 3));
    let conflicting = registry(json!([
        {"label":"x","command":"x"},{"label":"y","command":"y"},
        {"label":"a","dependsOrder":"sequence","dependsOn":["x","y"]},
        {"label":"b","dependsOrder":"sequence","dependsOn":["y","x"]},
        {"label":"all","dependsOn":["a","b"]}
    ]));
    assert!(matches!(conflicting.plan("all", false), Err(Error::Cycle(_))));
}

#[test]
fn task_plan_reports_missing_dependencies_and_requires_watch_readiness_only_when_needed() {
    let registry = registry(json!([
        {"label":"missing","dependsOn":"unknown"}, {"label":"watch","command":"watch","isBackground":true},
        {"label":"ready","command":"watch","isBackground":true,"problemMatcher":{"background":{"endsPattern":"^Ready$"}}},
        {"label":"all","dependsOn":"watch"}, {"label":"use-ready","dependsOn":"ready"}
    ]));
    assert_eq!(registry.plan("missing", false).err(), Some(Error::Missing { task: "missing".into(), dependency: "unknown".into() }));
    assert!(registry.plan("watch", false).is_ok());
    assert_eq!(registry.plan("watch", true).err(), Some(Error::Readiness("watch".into())));
    assert_eq!(registry.plan("all", false).err(), Some(Error::Readiness("watch".into())));
    assert!(registry.plan("use-ready", false).is_ok());
}

#[test]
fn task_inputs_preserve_secrets_pick_values_and_command_arguments_without_evaluating_them() {
    let parsed = Registry::parse(
        &json!({"tasks":[{"label":"run","command":"${input:secret}"}],"inputs":[
            {"id":"secret","type":"promptString","description":"Password","password":true},
            {"id":"target","type":"pickString","description":"Target","options":["local",{"label":"Remote server","value":"ssh"}],"default":"ssh"},
            {"id":"command","type":"command","command":"registered.command","args":{"key":"value"}}
        ]})
        .to_string(),
        Platform::Linux,
    )
    .unwrap();
    assert!(matches!(parsed.inputs["secret"], Input::Prompt { password: true, .. }));
    assert!(
        matches!(&parsed.inputs["target"],Input::Pick {options,default,..} if options[1] == ("Remote server".into(),"ssh".into()) && default.as_deref() == Some("ssh"))
    );
    assert!(matches!(&parsed.inputs["command"],Input::Command {command,args} if command == "registered.command" && args["key"] == "value"));
}

#[test]
fn task_parse_rejects_ambiguous_labels_invalid_types_counts_and_nul_arguments() {
    for tasks in [
        json!([{"label":"x","command":"a"},{"label":"x","command":"b"}]),
        json!([{"label":"x","command":"a","type":17}]),
        json!([{"label":"x","command":"a","args":["a\u{0}b"]}]),
        json!([{"label":"x","command":"a","runOptions":{"instanceLimit":0}}]),
        json!([{"label":"x","command":"a","dependsOrder":"sometimes"}]),
        json!([{"label":"x","command":"a","options":{"env":{"A=B":"bad"}}}]),
    ] {
        assert!(Registry::parse(&json!({"tasks":tasks}).to_string(), Platform::Linux).is_err());
    }
    assert!(matches!(Registry::parse(&" ".repeat(MAX_FILE + 1), Platform::Linux), Err(Error::Limit)));
}

#[test]
fn task_parse_bounds_expanded_defaults_before_amplifying_a_small_configuration() {
    let value = json!({"args":["x".repeat(60000)],"tasks":(0..256).map(|i| json!({"label":format!("task-{i}"),"command":"run"})).collect::<Vec<_>>()});
    assert!(matches!(Registry::parse(&value.to_string(), Platform::Linux), Err(Error::Limit)));
}

#[test]
fn task_parse_validates_string_pick_options_and_schema_versions() {
    for option in ["contains\0nul".into(), "x".repeat(65537)] {
        let value = json!({"tasks":[],"inputs":[{"id":"target","type":"pickString","description":"Target","options":[option]}]});
        assert!(Registry::parse(&value.to_string(), Platform::Linux).is_err());
    }
    for version in [json!("3.0.0"), json!(2), json!("0.1.0")] {
        assert!(Registry::parse(&json!({"version":version,"tasks":[]}).to_string(), Platform::Linux).is_err());
    }
}

#[test]
fn task_parse_supports_a_single_global_command_and_rejects_invalid_task_lists() {
    let parsed = Registry::parse(r#"{"version":"2.0.0","command":"build","options":{"cwd":"${workspaceFolder}"}}"#, Platform::Linux).unwrap();
    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.tasks[0].label, "build");
    assert_eq!(parsed.tasks[0].options.cwd.as_deref(), Some("${workspaceFolder}"));
    assert!(Registry::parse(r#"{"command":"build","tasks":17}"#, Platform::Linux).is_err());
}
