use super::*;
use crate::picker::{Pick, PickerEvent};
use crate::qa_integration_tests::{git_in, repo};
use gpui::TestAppContext;
use std::sync::Arc;

#[gpui::test]
fn a_pull_checkout_cannot_follow_a_remote_repointed_while_its_menu_is_open(cx: &mut TestAppContext) {
    let root = repo("repointed-pull-action");
    let original = repo("repointed-pull-original");
    let replacement = repo("repointed-pull-replacement");
    git_in(&original, &["update-ref", "refs/pull/42/head", "HEAD"]);
    std::fs::write(replacement.join("different-repository.txt"), "unrelated code\n").unwrap();
    git_in(&replacement, &["add", "-A"]);
    git_in(&replacement, &["commit", "-q", "-m", "different repository"]);
    git_in(&replacement, &["update-ref", "refs/pull/42/head", "HEAD"]);
    git_in(&root, &["remote", "add", "origin", &crate::ui::local_path(&original)]);
    let starting_head = git_in(&root, &["rev-parse", "HEAD"]);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
        wb.pick_pull_request(wb.remote.host.clone(), crate::ui::local_path(&root), wb.pull_remote_for_test("origin".into()), pull_fixture(), window, cx);
    });
    git_in(&root, &["remote", "set-url", "origin", &crate::ui::local_path(&replacement)]);
    wb.update_in(cx, |wb, window, cx| wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx));
    cx.run_until_parked();
    assert_eq!(git_in(&root, &["rev-parse", "HEAD"]), starting_head, "the PR selection cannot check out unrelated code from a repointed remote");
    assert!(git_in(&root, &["branch", "--list", "pr-42"]).is_empty());
    assert!(!root.join("different-repository.txt").exists());
    assert!(
        wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Error(message)) if message.contains("changed"))),
        "explain why the old selection was refused"
    );
}

#[gpui::test]
fn a_pull_list_keeps_its_resolved_url_when_an_instead_of_rule_changes(cx: &mut TestAppContext) {
    let root = repo("repointed-pull-list");
    let original = repo("repointed-pull-list-original");
    let replacement = repo("repointed-pull-list-replacement");
    for source in [&original, &replacement] {
        git_in(source, &["update-ref", "refs/pull/42/head", "HEAD"]);
    }
    let original_rule = format!("url.{}.insteadOf", crate::ui::local_path(&original));
    let replacement_rule = format!("url.{}.insteadOf", crate::ui::local_path(&replacement));
    git_in(&root, &["config", &original_rule, "cy-pr-test:repo"]);
    git_in(&root, &["remote", "add", "origin", "cy-pr-test:repo"]);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        let root = crate::ui::local_path(&root);
        wb.vcs.status = Some(Arc::new(git::Status { root: root.clone(), ..Default::default() }));
        let picker = cx.new(|cx| {
            Picker::new(
                vec![PickSource::List { prefix: "", items: vec![PickItem { label: "#42".into(), detail: None, keys: None }] }],
                "Pull requests",
                "",
                None,
                cx,
            )
        });
        wb.show_overlay(
            picker,
            OverlayKind::PullRequests { host: wb.remote.host.clone(), root, remote: wb.pull_remote_for_test("origin".into()), pulls: vec![pull_fixture()] },
            window,
            cx,
        );
    });
    git_in(&root, &["config", "--unset-all", &original_rule]);
    git_in(&root, &["config", &replacement_rule, "cy-pr-test:repo"]);
    wb.update_in(cx, |wb, window, cx| {
        wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx);
        wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx);
    });
    cx.run_until_parked();
    assert_eq!(git_in(&root, &["config", "remote.origin.url"]).trim(), "cy-pr-test:repo", "the alias itself never changed");
    assert!(git_in(&root, &["branch", "--list", "pr-42"]).is_empty(), "the second menu must keep the first list's resolved URL");
    assert!(!root.join(".git/FETCH_HEAD").exists(), "refuse before fetching any PR");
    assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Error(message)) if message.contains("changed"))));
}

#[gpui::test]
fn a_completed_background_fetch_error_only_appears_on_its_source_host_and_repository(cx: &mut TestAppContext) {
    let root = repo("autofetch-error-context");
    let other = repo("autofetch-error-other");
    git_in(&root, &["remote", "add", "origin", &crate::ui::local_path(&root.join("missing.git"))]);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update(cx, |wb, cx| {
        cx.set_global(crate::settings::Settings { git_autofetch: true, workspace_trust: false, ..Default::default() });
        wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
        assert!(wb.autofetch_for_test(true, cx));
    });
    cx.run_until_parked();
    let original_host = wb.read_with(cx, |wb, _| wb.remote.host.clone());
    assert!(wb.read_with(cx, |wb, _| wb.sync_tooltip_for_test()).contains("The last background fetch failed"));
    for change_host in [false, true] {
        wb.update(cx, |wb, _| {
            wb.remote.host = if change_host { Arc::new(host::local()) } else { original_host.clone() };
            wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(if change_host { &root } else { &other }), ..Default::default() }));
        });
        assert!(
            !wb.read_with(cx, |wb, _| wb.sync_tooltip_for_test()).contains("The last background fetch failed"),
            "a completed failure belongs to its source connection and repository"
        );
    }
    wb.update(cx, |wb, _| {
        wb.remote.host = original_host;
        wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
    });
    assert!(wb.read_with(cx, |wb, _| wb.sync_tooltip_for_test()).contains("The last background fetch failed"));
    git_in(&root, &["remote", "remove", "origin"]);
    wb.update(cx, |wb, cx| assert!(wb.autofetch_for_test(true, cx)));
    cx.run_until_parked();
    assert!(!wb.read_with(cx, |wb, _| wb.sync_tooltip_for_test()).contains("The last background fetch failed"), "a successful fetch clears the error");
}

fn sync_picker_repo(name: &str) -> std::path::PathBuf {
    let root = repo(name);
    let remote = crate::ui::local_path(&root.join("upstream.git"));
    git_in(&root, &["init", "-q", "--bare", &remote]);
    git_in(&root, &["remote", "add", "origin", &remote]);
    git_in(&root, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
    root
}

#[gpui::test]
fn current_sync_target_pickers_still_run_pull_and_push(cx: &mut TestAppContext) {
    for pull in [false, true] {
        let root = sync_picker_repo(&format!("current-sync-target-{pull}"));
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
            wb.show_sync_target_picker(pull, window, cx);
        });
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| {
            wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx);
            assert!(wb.sync_running_for_test());
        });
        cx.run_until_parked();
        assert!(!wb.read_with(cx, |wb, _| wb.sync_running_for_test()));
        assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.contains(if pull { "Pulled" } else { "Pushed" }))));
        assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()));
    }
}

#[gpui::test]
fn pending_sync_target_lists_cannot_open_after_their_context_changes(cx: &mut TestAppContext) {
    let root = sync_picker_repo("stale-sync-target-list");
    for pull in [false, true] {
        for change_host in [false, true] {
            for fail in [false, true] {
                let (wb, cx) = crate::qa_workbench_tests::setup(cx);
                wb.update_in(cx, |wb, window, cx| {
                    let path = if fail { root.join("missing") } else { root.clone() };
                    wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&path), ..Default::default() }));
                    wb.show_sync_target_picker(pull, window, cx);
                    if change_host {
                        wb.remote.host = Arc::new(host::local());
                    } else {
                        wb.vcs.status = None;
                    }
                    wb.notice = Some(Notice::Info("new context".into()));
                });
                cx.run_until_parked();
                assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()), "a stale sync target list cannot open a picker");
                assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")));
            }
        }
    }
}

#[gpui::test]
fn visible_sync_targets_cannot_start_an_operation_after_their_context_changes(cx: &mut TestAppContext) {
    let root = sync_picker_repo("stale-sync-target-selection");
    let other = repo("stale-sync-target-other");
    for pull in [false, true] {
        for change_host in [false, true] {
            let (wb, cx) = crate::qa_workbench_tests::setup(cx);
            wb.update_in(cx, |wb, window, cx| {
                wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
                wb.show_sync_target_picker(pull, window, cx);
            });
            cx.run_until_parked();
            assert!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).is_some_and(|rows| !rows.is_empty())));
            wb.update_in(cx, |wb, window, cx| {
                if change_host {
                    wb.remote.host = Arc::new(host::local());
                } else {
                    wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&other), ..Default::default() }));
                }
                wb.notice = Some(Notice::Info("new context".into()));
                wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx);
                assert!(!wb.sync_running_for_test(), "an old target must not start Git on the new context");
            });
            cx.run_until_parked();
            assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()));
            assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")));
        }
    }
}

#[gpui::test]
fn old_sync_results_release_the_guard_without_changing_the_new_context(cx: &mut TestAppContext) {
    check_old_sync_result(cx, false);
}

#[gpui::test]
fn old_autofetch_results_release_the_guard_without_changing_the_new_context(cx: &mut TestAppContext) {
    check_old_sync_result(cx, true);
}

fn check_old_sync_result(cx: &mut TestAppContext, background: bool) {
    for change_host in [false, true] {
        for fail in [false, true] {
            let root = repo(&format!("stale-sync-{background}-{change_host}-{fail}"));
            let remote = if fail { root.join("missing.git") } else { root.clone() };
            git_in(&root, &["remote", "add", "origin", &crate::ui::local_path(&remote)]);
            let (wb, cx) = crate::qa_workbench_tests::setup(cx);
            wb.update(cx, |wb, cx| {
                cx.set_global(crate::settings::Settings { git_autofetch: true, workspace_trust: false, ..Default::default() });
                wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
                if background {
                    assert!(wb.autofetch_for_test(true, cx));
                } else {
                    wb.git_sync(vec![git::SyncOp::Fetch], cx);
                }
                assert!(wb.sync_running_for_test());
                if change_host {
                    wb.remote.host = Arc::new(host::local());
                } else {
                    wb.vcs.status = None;
                }
                wb.notice = Some(Notice::Info("new context".into()));
            });
            cx.run_until_parked();
            assert!(!wb.read_with(cx, |wb, _| wb.sync_running_for_test()), "the old operation must release the window's guard");
            assert!(wb.read_with(cx, |wb, _| wb.autofetch_error_for_test()).is_none(), "old fetch errors must not attach to the new context");
            assert!(
                wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")),
                "old sync results must not replace the new context's notice"
            );
            if !fail {
                assert!(
                    !git_in(&root, &["for-each-ref", "--format=%(refname)", "refs/remotes/origin"]).trim().is_empty(),
                    "the operation still finishes against the original repository"
                );
            }
        }
    }
}

fn pull_fixture() -> crate::github::Pull {
    crate::github::Pull {
        number: 42,
        title: "Fixture pull request".into(),
        author: "fixture".into(),
        head: "topic".into(),
        base: "main".into(),
        draft: false,
        updated: String::new(),
        url: "https://github.com/example/repo/pull/42".into(),
        review: None,
        checks: None,
    }
}

#[gpui::test]
fn the_current_pull_request_picker_carries_its_context_through_checkout(cx: &mut TestAppContext) {
    let root = repo("current-pull-action");
    let origin = repo("current-pull-action-origin");
    git_in(&origin, &["update-ref", "refs/pull/42/head", "HEAD"]);
    git_in(&root, &["remote", "add", "origin", &crate::ui::local_path(&origin)]);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let source_host = wb.read_with(cx, |wb, _| wb.remote.host.clone());
    wb.update_in(cx, |wb, window, cx| {
        let root = crate::ui::local_path(&root);
        wb.vcs.status = Some(Arc::new(git::Status { root: root.clone(), ..Default::default() }));
        let picker = cx.new(|cx| {
            Picker::new(
                vec![PickSource::List { prefix: "", items: vec![PickItem { label: "#42".into(), detail: None, keys: None }] }],
                "Pull requests",
                "",
                None,
                cx,
            )
        });
        wb.show_overlay(
            picker,
            OverlayKind::PullRequests { host: source_host.clone(), root, remote: wb.pull_remote_for_test("origin".into()), pulls: vec![pull_fixture()] },
            window,
            cx,
        );
        wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx);
    });
    assert!(wb.read_with(cx, |wb, _| matches!(wb.overlay.as_ref().map(|overlay| &overlay.kind), Some(OverlayKind::PullRequestActions { host, root: source, .. }) if Arc::ptr_eq(host, &source_host) && source == &crate::ui::local_path(&root))));
    wb.update_in(cx, |wb, window, cx| wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx));
    cx.run_until_parked();
    assert_eq!(git_in(&root, &["branch", "--show-current"]).trim(), "pr-42");
    assert!(wb.read_with(cx, |wb, _| Arc::ptr_eq(&wb.remote.host, &source_host)));
}

#[gpui::test]
fn an_old_pull_request_load_error_cannot_replace_the_new_context_notice(cx: &mut TestAppContext) {
    let root = repo("stale-pull-list"); // No GitHub remote: the provider returns locally without an HTTP request.
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    for change_host in [false, true] {
        wb.update_in(cx, |wb, window, cx| {
            wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
            wb.show_pull_requests(&crate::workbench::ShowPullRequests, window, cx);
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.vcs.status = None;
            }
            wb.notice = Some(Notice::Info("new context".into()));
        });
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")));
        assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()));
    }
}

#[gpui::test]
fn a_stale_pull_request_row_cannot_open_its_action_menu(cx: &mut TestAppContext) {
    let root = repo("stale-pull-row");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
        let picker = cx.new(|cx| {
            Picker::new(
                vec![PickSource::List { prefix: "", items: vec![PickItem { label: "#42".into(), detail: None, keys: None }] }],
                "Pull requests",
                "",
                None,
                cx,
            )
        });
        wb.show_overlay(
            picker,
            OverlayKind::PullRequests {
                host: wb.remote.host.clone(),
                root: crate::ui::local_path(&root),
                remote: wb.pull_remote_for_test("origin".into()),
                pulls: vec![pull_fixture()],
            },
            window,
            cx,
        );
        wb.remote.host = Arc::new(host::local());
        wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx);
    });
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()), "the old PR list cannot become a menu bound to the new host");
}

#[gpui::test]
fn a_stale_pull_checkout_action_cannot_create_a_branch_in_another_context(cx: &mut TestAppContext) {
    for change_host in [false, true] {
        let root = repo(&format!("stale-pull-action-{change_host}"));
        let other = repo(&format!("stale-pull-action-other-{change_host}"));
        let origin = repo(&format!("stale-pull-action-origin-{change_host}"));
        git_in(&origin, &["update-ref", "refs/pull/42/head", "HEAD"]);
        for target in [&root, &other] {
            git_in(target, &["remote", "add", "origin", &crate::ui::local_path(&origin)]);
        }
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
            wb.pick_pull_request(wb.remote.host.clone(), crate::ui::local_path(&root), wb.pull_remote_for_test("origin".into()), pull_fixture(), window, cx);
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&other), ..Default::default() }));
            }
            wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index: 0 }), window, cx);
        });
        cx.run_until_parked();
        for target in [&root, &other] {
            assert!(git_in(target, &["branch", "--list", "pr-42"]).is_empty(), "the old PR cannot be checked out in the new context");
        }
    }
}

#[gpui::test]
fn editing_a_remote_url_without_changes_preserves_the_configured_value(cx: &mut TestAppContext) {
    let root = repo("remote-url-with-spaces");
    let url = crate::ui::local_path(&root.join("local repositories/çalışma.git"));
    for url in [url.as_str(), " /srv/repo with spaces.git ", "/srv/repo\twith-tab.git", "/srv/first\nsecond.git\n"] {
        git_in(&root, &["config", "remote.origin.url", url]);
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            let root = crate::ui::local_path(&root);
            wb.vcs.status = Some(Arc::new(git::Status { root: root.clone(), ..Default::default() }));
            wb.run_ref_op(root, BranchOp::SetRemoteUrl, "origin".into(), window, cx);
        });
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_query(cx)), Some(url.to_string()), "the initial value cannot truncate a remote URL");
        wb.update_in(cx, |wb, window, cx| wb.on_overlay_event(&PickerEvent::Confirm(Pick::Input), window, cx));
        cx.run_until_parked();
        let saved = git_in(&root, &["config", "remote.origin.url"]);
        assert_eq!(saved.strip_suffix('\n').unwrap(), url);
    }
}

#[gpui::test]
fn a_remote_url_read_cannot_open_an_editor_after_its_context_changes(cx: &mut TestAppContext) {
    let root = repo("stale-remote-url");
    git_in(&root, &["remote", "add", "origin", "https://example.invalid/original.git"]);
    for change_host in [false, true] {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            let root = crate::ui::local_path(&root);
            wb.vcs.status = Some(Arc::new(git::Status { root: root.clone(), ..Default::default() }));
            wb.run_ref_op(root, BranchOp::SetRemoteUrl, "origin".into(), window, cx);
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.vcs.status = None;
            }
            wb.notice = None;
        });
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()), "the old URL must not become a form bound to the new context");
        assert!(wb.read_with(cx, |wb, _| wb.notice.is_none()));
    }
}

#[gpui::test]
fn a_failed_remote_url_read_is_an_error_instead_of_an_empty_editor(cx: &mut TestAppContext) {
    let root = repo("failed-remote-url");
    let moved = root.with_file_name(format!("{}-moved", root.file_name().unwrap().to_string_lossy()));
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        let path = crate::ui::local_path(&root);
        wb.vcs.status = Some(Arc::new(git::Status { root: path.clone(), ..Default::default() }));
        wb.run_ref_op(path, BranchOp::SetRemoteUrl, "origin".into(), window, cx);
        std::fs::rename(&root, &moved).unwrap();
    });
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()), "a failed metadata read must not offer a blank replacement URL");
    assert!(wb.read_with(cx, |wb, _| matches!(wb.notice, Some(Notice::Error(_)))));
    std::fs::rename(moved, root).unwrap();
}

#[gpui::test]
fn a_removed_remote_cannot_open_an_empty_url_editor(cx: &mut TestAppContext) {
    let root = repo("removed-remote-url");
    git_in(&root, &["remote", "add", "origin", "https://example.invalid/original.git"]);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        let path = crate::ui::local_path(&root);
        wb.vcs.status = Some(Arc::new(git::Status { root: path.clone(), ..Default::default() }));
        wb.run_ref_op(path, BranchOp::SetRemoteUrl, "origin".into(), window, cx);
        git_in(&root, &["remote", "remove", "origin"]);
    });
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()));
    assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Error(message)) if message.contains("origin"))));
}

#[gpui::test]
fn the_current_remote_url_is_shown_and_can_be_updated(cx: &mut TestAppContext) {
    let root = repo("current-remote-url");
    git_in(&root, &["remote", "add", "origin", "https://example.invalid/original.git"]);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        let path = crate::ui::local_path(&root);
        wb.vcs.status = Some(Arc::new(git::Status { root: path.clone(), ..Default::default() }));
        wb.run_ref_op(path, BranchOp::SetRemoteUrl, "origin".into(), window, cx);
    });
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_query(cx)), Some("https://example.invalid/original.git".into()));
    wb.update_in(cx, |wb, window, cx| {
        wb.set_overlay_query_for_test("https://example.invalid/updated.git".into(), cx);
        wb.on_overlay_event(&PickerEvent::Confirm(Pick::Input), window, cx);
    });
    cx.run_until_parked();
    assert_eq!(git_in(&root, &["remote", "get-url", "origin"]).trim(), "https://example.invalid/updated.git");
}

#[gpui::test]
fn a_git_delete_confirmation_cannot_mutate_after_its_context_changes(cx: &mut TestAppContext) {
    for change_host in [false, true] {
        let root = repo(&format!("stale-git-confirm-{change_host}"));
        let other = repo(&format!("stale-git-confirm-other-{change_host}"));
        git_in(&root, &["tag", "keep-tag"]);
        git_in(&other, &["tag", "keep-tag"]);
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            let root = crate::ui::local_path(&root);
            wb.vcs.status = Some(Arc::new(git::Status { root: root.clone(), ..Default::default() }));
            wb.run_ref_op(root, BranchOp::DeleteTag, "keep-tag".into(), window, cx);
        });
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        wb.update(cx, |wb, _| {
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&other), ..Default::default() }));
            }
        });
        cx.simulate_prompt_answer("Delete");
        cx.run_until_parked();
        for repo in [&root, &other] {
            assert!(git_in(repo, &["tag", "--list", "keep-tag"]).contains("keep-tag"), "an old confirmation cannot delete tags through a new context");
        }
    }
}

#[gpui::test]
fn a_late_branch_delete_preflight_cannot_open_a_prompt_in_the_new_context(cx: &mut TestAppContext) {
    let root = repo("stale-delete-preflight");
    git_in(&root, &["branch", "keep-branch"]);
    for change_host in [false, true] {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            let root = crate::ui::local_path(&root);
            wb.vcs.status = Some(Arc::new(git::Status { root: root.clone(), ..Default::default() }));
            wb.run_branch_op(root, BranchOp::Delete, "keep-branch".into(), window, cx);
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.vcs.status = None;
            }
        });
        cx.run_until_parked();
        assert!(!cx.has_pending_prompt(), "an old commit-count result cannot ask the new connection to delete a branch");
        assert!(git_in(&root, &["branch", "--list", "keep-branch"]).contains("keep-branch"));
    }
}

#[gpui::test]
fn a_git_delete_confirmation_in_the_current_context_still_runs(cx: &mut TestAppContext) {
    let root = repo("current-git-confirm");
    git_in(&root, &["tag", "delete-tag"]);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        let root = crate::ui::local_path(&root);
        wb.vcs.status = Some(Arc::new(git::Status { root: root.clone(), ..Default::default() }));
        wb.run_ref_op(root, BranchOp::DeleteTag, "delete-tag".into(), window, cx);
    });
    cx.run_until_parked();
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    assert!(git_in(&root, &["tag", "--list", "delete-tag"]).is_empty());
}

#[gpui::test]
fn generic_git_task_results_and_callbacks_cannot_cross_contexts(cx: &mut TestAppContext) {
    for change_host in [false, true] {
        for failed in [false, true] {
            let root = repo(&format!("stale-git-task-{change_host}-{failed}"));
            let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let refreshed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let worker_ran = ran.clone();
            let after_ran = refreshed.clone();
            let (wb, cx) = crate::qa_workbench_tests::setup(cx);
            wb.update(cx, |wb, cx| {
                let root = crate::ui::local_path(&root);
                wb.vcs.status = Some(Arc::new(git::Status { root: root.clone(), ..Default::default() }));
                wb.git_task_then(
                    "working",
                    cx,
                    move |host| {
                        worker_ran.store(true, std::sync::atomic::Ordering::Relaxed);
                        if failed {
                            return Err("old operation failed".into());
                        }
                        git::create_branch_from(host, &root, "completed-old-task", "HEAD").map(|_| "old operation succeeded".into())
                    },
                    move |_, _| {
                        after_ran.store(true, std::sync::atomic::Ordering::Relaxed);
                    },
                );
                if change_host {
                    wb.remote.host = Arc::new(host::local());
                } else {
                    wb.vcs.status = None;
                }
                wb.notice = Some(Notice::Info("new context".into()));
            });
            cx.run_until_parked();
            assert!(ran.load(std::sync::atomic::Ordering::Relaxed), "the already started operation still runs on its original host");
            if !failed {
                assert!(git_in(&root, &["branch", "--list", "completed-old-task"]).contains("completed-old-task"));
            }
            assert!(!refreshed.load(std::sync::atomic::Ordering::Relaxed), "a stale completion cannot refresh the new context");
            assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")));
        }
    }
}

#[gpui::test]
fn generic_git_task_in_the_current_context_keeps_its_result_and_callback(cx: &mut TestAppContext) {
    let root = repo("current-git-task");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    for has_repository in [false, true] {
        for failed in [false, true] {
            let refreshed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let after_ran = refreshed.clone();
            wb.update(cx, |wb, cx| {
                wb.vcs.status = has_repository.then(|| Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
                wb.git_task_then(
                    "working",
                    cx,
                    move |_| if failed { Err("current error".into()) } else { Ok("current success".into()) },
                    move |_, _| {
                        after_ran.store(true, std::sync::atomic::Ordering::Relaxed);
                    },
                );
            });
            cx.run_until_parked();
            assert!(refreshed.load(std::sync::atomic::Ordering::Relaxed));
            assert!(wb.read_with(cx, |wb, _| match &wb.notice {
                Some(Notice::Info(message)) if !failed => message.as_ref() == "current success",
                Some(Notice::Error(message)) if failed => message.as_ref() == "current error",
                _ => false,
            }));
        }
    }
}

#[gpui::test]
fn current_git_name_forms_still_create_branches_and_tags(cx: &mut TestAppContext) {
    for form in ["branch", "branch-from", "tag"] {
        let root = repo(&format!("current-git-input-{form}"));
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            let path = crate::ui::local_path(&root);
            let status = Arc::new(git::Status { root: path.clone(), ..Default::default() });
            wb.vcs.status = Some(status.clone());
            wb.scm.update(cx, |scm, cx| scm.set_status(wb.remote.host.clone(), Some(status), cx));
            let purpose = match form {
                "branch" => InputPurpose::CreateBranch { root: path },
                "branch-from" => InputPurpose::CreateBranchFrom { root: path, start: "HEAD".into() },
                _ => InputPurpose::CommitTag { hash: "HEAD".into(), repo: wb.scm.read(cx).target(None).ok() },
            };
            wb.show_input(purpose, "Create", "Name", "", window, cx);
            wb.set_overlay_query_for_test("current-input".into(), cx);
            wb.on_overlay_event(&PickerEvent::Confirm(Pick::Input), window, cx);
        });
        cx.run_until_parked();
        let created = if form == "tag" { git_in(&root, &["tag", "--list", "current-input"]) } else { git_in(&root, &["branch", "--list", "current-input"]) };
        assert!(created.contains("current-input"), "the current {form} form must retain its normal behavior");
    }
}

#[gpui::test]
fn ordinary_file_name_input_does_not_require_a_git_repository(cx: &mut TestAppContext) {
    let root = crate::qa_workbench_tests::fresh("non-git-input");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.show_input(InputPurpose::Create { dir: crate::ui::local_path(&root), is_dir: false }, "Create", "File name", "", window, cx);
        wb.set_overlay_query_for_test("created.txt".into(), cx);
        wb.on_overlay_event(&PickerEvent::Confirm(Pick::Input), window, cx);
    });
    cx.run_until_parked();
    assert!(root.join("created.txt").is_file());
}

#[gpui::test]
fn stale_branch_name_forms_cannot_create_refs_after_their_context_changes(cx: &mut TestAppContext) {
    for from_revision in [false, true] {
        for change_host in [false, true] {
            let root = repo(&format!("stale-branch-input-{from_revision}-{change_host}"));
            let other = repo(&format!("stale-branch-input-other-{from_revision}-{change_host}"));
            let (wb, cx) = crate::qa_workbench_tests::setup(cx);
            wb.update_in(cx, |wb, window, cx| {
                let path = crate::ui::local_path(&root);
                wb.vcs.status = Some(Arc::new(git::Status { root: path.clone(), ..Default::default() }));
                let purpose =
                    if from_revision { InputPurpose::CreateBranchFrom { root: path, start: "HEAD".into() } } else { InputPurpose::CreateBranch { root: path } };
                wb.show_input(purpose, "Create", "Branch name", "", window, cx);
                wb.set_overlay_query_for_test("feature/old-input".into(), cx);
                if change_host {
                    wb.remote.host = Arc::new(host::local());
                } else {
                    wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&other), ..Default::default() }));
                }
                wb.on_overlay_event(&PickerEvent::Confirm(Pick::Input), window, cx);
            });
            cx.run_until_parked();
            for repo in [&root, &other] {
                assert!(git_in(repo, &["branch", "--list", "feature/old-input"]).is_empty(), "a stale form cannot create a branch in either repository");
            }
        }
    }
}

#[gpui::test]
fn an_old_tag_name_form_cannot_use_the_new_source_control_context(cx: &mut TestAppContext) {
    let root = repo("stale-tag-input");
    let other = repo("stale-tag-input-other");
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        let status = Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() });
        wb.vcs.status = Some(status.clone());
        wb.scm.update(cx, |scm, cx| scm.set_status(wb.remote.host.clone(), Some(status), cx));
        let repo = wb.scm.read(cx).target(None).ok();
        wb.show_input(InputPurpose::CommitTag { hash: "HEAD".into(), repo }, "Create", "Tag name", "", window, cx);
        wb.set_overlay_query_for_test("old-input-tag".into(), cx);
        let status = Arc::new(git::Status { root: crate::ui::local_path(&other), ..Default::default() });
        wb.vcs.status = Some(status.clone());
        wb.scm.update(cx, |scm, cx| scm.set_status(wb.remote.host.clone(), Some(status), cx));
        wb.on_overlay_event(&PickerEvent::Confirm(Pick::Input), window, cx);
    });
    cx.run_until_parked();
    assert!(git_in(&root, &["tag", "--list", "old-input-tag"]).is_empty());
    assert!(git_in(&other, &["tag", "--list", "old-input-tag"]).is_empty(), "a graph tag input cannot act on the newly selected repository");
}

#[gpui::test]
fn pending_checkout_picker_cannot_open_after_its_context_changes(cx: &mut TestAppContext) {
    let root = repo("stale-checkout-list");
    let path = crate::ui::local_path(&root);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    for change_host in [false, true] {
        wb.update_in(cx, |wb, window, cx| {
            wb.vcs.status = Some(Arc::new(git::Status { root: path.clone(), ..Default::default() }));
            wb.show_branch_picker(window, cx);
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.vcs.status = None;
            }
        });
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()), "a stale branch list cannot open in the new context");
    }
}

#[gpui::test]
fn a_stale_checkout_selection_cannot_switch_a_branch(cx: &mut TestAppContext) {
    let root = repo("stale-checkout-selection");
    git_in(&root, &["branch", "feature/context"]);
    let original = git_in(&root, &["branch", "--show-current"]);
    for change_host in [false, true] {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
            wb.show_branch_picker(window, cx);
        });
        cx.run_until_parked();
        let index = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).unwrap().iter().position(|name| name == "feature/context").unwrap());
        wb.update_in(cx, |wb, window, cx| {
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.vcs.status = None;
            }
            wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index }), window, cx);
        });
        cx.run_until_parked();
        assert_eq!(git_in(&root, &["branch", "--show-current"]), original, "a stale row cannot mutate the old repository through the new context");
    }
}

#[gpui::test]
fn an_old_checkout_result_cannot_replace_the_new_context_notice(cx: &mut TestAppContext) {
    for change_host in [false, true] {
        for failed_checkout in [false, true] {
            let root = repo(&format!("stale-checkout-result-{change_host}-{failed_checkout}"));
            git_in(&root, &["branch", "feature/context"]);
            let original = git_in(&root, &["branch", "--show-current"]);
            let (wb, cx) = crate::qa_workbench_tests::setup(cx);
            wb.update(cx, |wb, cx| {
                let root = crate::ui::local_path(&root);
                wb.vcs.status = Some(Arc::new(git::Status { root: root.clone(), ..Default::default() }));
                wb.git_checkout(root, if failed_checkout { "missing-branch" } else { "feature/context" }.into(), false, false, cx);
                if change_host {
                    wb.remote.host = Arc::new(host::local());
                } else {
                    wb.vcs.status = None;
                }
                wb.notice = Some(Notice::Info("new context".into()));
            });
            cx.run_until_parked();
            assert_eq!(
                git_in(&root, &["branch", "--show-current"]).trim(),
                if failed_checkout { original.trim() } else { "feature/context" },
                "the already started operation finishes on its original host"
            );
            assert!(
                wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")),
                "a late result cannot overwrite the new context's message"
            );
        }
    }
}

#[gpui::test]
fn a_current_checkout_picker_switches_the_branch_and_reports_success(cx: &mut TestAppContext) {
    let root = repo("current-checkout-selection");
    git_in(&root, &["branch", "feature/context"]);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        // Source Control knows the repository too: the picker is pinned to it.
        let status = Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() });
        wb.vcs.status = Some(status.clone());
        wb.scm.update(cx, |scm, cx| scm.set_status(wb.remote.host.clone(), Some(status), cx));
        wb.show_branch_picker(window, cx);
    });
    cx.run_until_parked();
    let original_host = wb.read_with(cx, |wb, _| wb.remote.host.clone());
    let index = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).unwrap().iter().position(|name| name == "feature/context").unwrap());
    wb.update_in(cx, |wb, window, cx| wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index }), window, cx));
    cx.run_until_parked();
    assert_eq!(git_in(&root, &["branch", "--show-current"]).trim(), "feature/context");
    assert!(wb.read_with(cx, |wb, _| Arc::ptr_eq(&wb.remote.host, &original_host)));
    assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.contains("feature/context"))));
}

#[gpui::test]
fn pending_worktree_picker_is_discarded_when_its_host_or_repository_changes(cx: &mut TestAppContext) {
    let root = repo("stale-worktree-list");
    let path = crate::ui::local_path(&root);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    for change_host in [false, true] {
        for failed_list in [false, true] {
            wb.update_in(cx, |wb, window, cx| {
                wb.vcs.status = Some(Arc::new(git::Status { root: if failed_list { format!("{path}/missing") } else { path.clone() }, ..Default::default() }));
                wb.notice = None;
                wb.show_branch_op_picker(BranchOp::OpenWorktree, window, cx);
                if change_host {
                    wb.remote.host = Arc::new(host::local());
                } else {
                    wb.vcs.status = None;
                }
            });
            cx.run_until_parked();
            assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()), "a stale listing cannot open a picker in the new context");
            assert!(wb.read_with(cx, |wb, _| wb.notice.is_none()), "a stale listing cannot report errors in the new context");
        }
    }
}

#[gpui::test]
fn a_visible_worktree_picker_cannot_navigate_after_its_host_or_repository_changes(cx: &mut TestAppContext) {
    let root = repo("stale-worktree-selection");
    let linked = root.join("linked");
    git_in(&root, &["worktree", "add", "-q", "-b", "linked", linked.to_str().unwrap()]);
    for change_host in [false, true] {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
            wb.show_branch_op_picker(BranchOp::OpenWorktree, window, cx);
        });
        cx.run_until_parked();
        let index = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).unwrap().iter().position(|path| path.ends_with("/linked")).unwrap());
        wb.update_in(cx, |wb, window, cx| {
            if change_host {
                wb.remote.host = Arc::new(host::local());
            } else {
                wb.vcs.status = None;
            }
            wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index }), window, cx);
        });
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, cx| wb.root(cx).is_none()), "the old host's path cannot become the new workspace root");
        assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()), "a stale picker closes after confirmation");
    }
}

#[gpui::test]
fn a_current_worktree_picker_opens_the_checkout_on_its_original_host(cx: &mut TestAppContext) {
    let root = repo("current-worktree-selection");
    let linked = root.join("linked");
    git_in(&root, &["worktree", "add", "-q", "-b", "linked", linked.to_str().unwrap()]);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.vcs.status = Some(Arc::new(git::Status { root: crate::ui::local_path(&root), ..Default::default() }));
        wb.show_branch_op_picker(BranchOp::OpenWorktree, window, cx);
    });
    cx.run_until_parked();
    let original = wb.read_with(cx, |wb, _| wb.remote.host.clone());
    let index = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).unwrap().iter().position(|path| path.ends_with("/linked")).unwrap());
    wb.update_in(cx, |wb, window, cx| wb.on_overlay_event(&PickerEvent::Confirm(Pick::Item { source: 0, index }), window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.root(cx)), Some(crate::ui::local_path(&linked)));
    assert!(wb.read_with(cx, |wb, _| Arc::ptr_eq(&wb.remote.host, &original)));
}
