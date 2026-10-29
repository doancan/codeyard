use super::*;
use crate::model::{Platform, Registry};
use serde_json::{Value, json};

fn schedule(tasks: Value, root: &str, parallelism: usize) -> Scheduler {
    let registry = Registry::parse(&json!({"tasks":tasks}).to_string(), Platform::Linux).unwrap();
    let plan = registry.plan(root, false).unwrap();
    Scheduler::new(&registry, plan, parallelism)
}

#[test]
fn task_schedule_reserves_shared_prerequisites_once_and_runs_parallel_branches() {
    let mut s = schedule(
        json!([
            {"label":"shared","command":"shared"},{"label":"a","command":"a","dependsOn":"shared"},
            {"label":"b","command":"b","dependsOn":"shared"},{"label":"all","dependsOn":["a","b"]}
        ]),
        "all",
        2,
    );
    assert_eq!(s.advance().start, [0]);
    assert!(s.advance().start.is_empty());
    assert!(s.exited(0, Some(0)));
    assert_eq!(s.advance().start, [1, 2]);
    assert!(!s.exited(0, Some(1)), "a duplicate exit cannot rewrite a finished prerequisite");
    s.exited(1, Some(0));
    s.exited(2, Some(0));
    assert_eq!(s.advance(), Changes::default());
    assert_eq!(s.root_state(), &State::Succeeded);
    assert_eq!(s.live(), 0);
}

#[test]
fn task_schedule_sequence_holds_the_entire_later_subtree_until_the_first_finishes() {
    let mut s = schedule(
        json!([
            {"label":"first","command":"first"},{"label":"child","command":"child"},
            {"label":"later","command":"later","dependsOn":"child"},
            {"label":"all","dependsOn":["first","later"],"dependsOrder":"sequence"}
        ]),
        "all",
        8,
    );
    assert_eq!(s.advance().start, [0]);
    s.exited(0, Some(0));
    assert_eq!(s.advance().start, [1]);
    s.exited(1, Some(0));
    assert_eq!(s.advance().start, [2]);
    s.exited(2, Some(0));
    s.advance();
    assert_eq!(s.root_state(), &State::Succeeded);
}

#[test]
fn task_schedule_watch_readiness_releases_a_start_slot_but_retains_process_ownership() {
    let mut s = schedule(
        json!([
            {"label":"watch","command":"watch","isBackground":true,"problemMatcher":"$tsc-watch"},
            {"label":"serve","command":"serve","dependsOn":"watch"}
        ]),
        "serve",
        1,
    );
    assert_eq!(s.advance().start, [0]);
    assert!(s.advance().start.is_empty());
    assert!(!s.ready(1));
    assert!(s.ready(0));
    assert_eq!(s.advance().start, [1]);
    assert_eq!(s.live(), 2);
    assert_eq!(s.cancel(), [0, 1]);
    assert!(s.cancel().is_empty());
    assert!(!s.ready(0));
    assert!(!s.exited(1, Some(0)));
    assert_eq!(s.root_state(), &State::Cancelled);
}

#[test]
fn task_schedule_watch_exit_before_readiness_stops_dependents_even_with_a_zero_exit() {
    let mut s = schedule(
        json!([
            {"label":"watch","command":"watch","isBackground":true,"problemMatcher":"$tsc-watch"},
            {"label":"use","command":"use","dependsOn":"watch"}
        ]),
        "use",
        8,
    );
    s.advance();
    s.exited(0, Some(0));
    assert!(s.advance().start.is_empty());
    assert_eq!(s.state(0), Some(&State::Failed(Failure::BeforeReady)));
    assert_eq!(s.root_state(), &State::Failed(Failure::Dependency(0)));
}

#[test]
fn task_schedule_watch_failure_cancels_running_transitive_dependents_and_keeps_unrelated_work() {
    let mut s = schedule(
        json!([
            {"label":"watch","command":"watch","isBackground":true,"problemMatcher":"$tsc-watch"},
            {"label":"use","command":"use","dependsOn":"watch"},{"label":"independent","command":"other"},
            {"label":"all","dependsOn":["use","independent"]}
        ]),
        "all",
        8,
    );
    assert_eq!(s.advance().start, [0, 2]);
    s.ready(0);
    assert_eq!(s.advance().start, [1]);
    s.exited(0, Some(9));
    let changes = s.advance();
    assert_eq!(changes.stop, [1]);
    assert!(changes.start.is_empty());
    assert_eq!(s.state(1), Some(&State::Failed(Failure::Dependency(0))));
    assert_eq!(s.state(2), Some(&State::Running));
    assert_eq!(s.cancel(), [2]);
}

#[test]
fn task_schedule_caps_parallel_startup_and_reports_watch_capacity_without_deadlocking() {
    let mut s = schedule(
        json!(
            (0..40)
                .map(|i| json!({"label":format!("watch-{i}"),"command":"watch","isBackground":true,"problemMatcher":"$tsc-watch"}))
                .chain([json!({"label":"all","dependsOn":(0..32).map(|i|format!("watch-{i}")).collect::<Vec<_>>(),"command":"build"})])
                .collect::<Vec<_>>()
        ),
        "all",
        3,
    );
    loop {
        let changes = s.advance();
        assert!(changes.start.len() <= 3);
        if changes.start.is_empty() {
            break;
        }
        for id in changes.start {
            assert!(s.ready(id));
        }
    }
    assert_eq!(s.live(), 32);
    assert_eq!(s.root_state(), &State::Failed(Failure::Capacity));
    assert_eq!(s.cancel().len(), 32);
}
