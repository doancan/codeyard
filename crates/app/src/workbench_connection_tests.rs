//! Drive the actual asynchronous connection path without an SSH server or engine.
use super::*;
use futures::channel::oneshot;

fn pending(wb: &mut Workbench, alias: &str, window: &mut Window, cx: &mut Context<Workbench>) -> oneshot::Sender<host::Result<Arc<dyn Host>>> {
    let (tx, rx) = oneshot::channel();
    wb.connect_using(alias.into(), window, cx, |_| async move { rx.await.expect("test supplies the connection result") });
    tx
}

fn settle(cx: &mut gpui::VisualTestContext) {
    // Deliver the background result and foreground callback, including delayed redraws.
    for _ in 0..5 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
    }
}

#[gpui::test]
fn connection_attempt_container_success_retains_the_original_workspace(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let source: Arc<dyn Host> = crate::container_monitor::tests::FixtureHost::new();
    let target: Arc<dyn Host> = crate::container_monitor::tests::FixtureHost::new();
    let root = ui::local_path(&crate::qa_workbench_tests::fresh("attempt-source"));
    let inside = ui::local_path(&crate::qa_workbench_tests::fresh("attempt-inside"));
    let alias = host::ssh::ssh_docker_target("fixture", "default", "web-id").unwrap();
    cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
    let tx = wb.update_in(cx, |wb, window, cx| {
        wb.remote.host = source.clone();
        wb.remote.conn = Conn::Connected { alias: "fixture".into() };
        wb.remote.docker.monitor.source = Some(source.clone());
        wb.remote.docker.monitor.host = Some("fixture".into());
        wb.remote.docker.monitor.paused = true;
        wb.set_root(root.clone(), cx);
        wb.remote.pending_path = Some(inside);
        pending(wb, &alias, window, cx)
    });
    assert!(wb.read_with(cx, |wb, _| matches!(wb.remote.conn, Conn::Connecting { .. })));
    assert!(tx.send(Ok(target)).is_ok());
    settle(cx);
    wb.read_with(cx, |wb, _| {
        let saved = wb.remote.docker.monitor.return_workspace.as_ref().expect("real Connecting -> Connected path must retain the source");
        assert!(Arc::ptr_eq(&saved.host, &source));
        assert_eq!(saved.root.as_deref(), Some(root.as_str()));
    });
    wb.update_in(cx, |wb, window, cx| wb.return_from_container(window, cx));
    assert!(wb.read_with(cx, |wb, cx| Arc::ptr_eq(&wb.remote.host, &source) && wb.root(cx).as_deref() == Some(root.as_str())));
    wb.update(cx, |wb, _| wb.remote.docker.monitor.clear());
}

#[gpui::test]
fn connection_attempt_late_success_does_not_replace_a_local_recent(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let root = ui::local_path(&crate::qa_workbench_tests::fresh("attempt-local"));
    let tx = wb.update_in(cx, |wb, window, cx| pending(wb, "fixture", window, cx));
    wb.update_in(cx, |wb, window, cx| wb.open_recent(RecentWorkspace { host: None, path: root.clone(), last_opened_at: 0 }, window, cx));
    assert!(tx.send(Ok(crate::container_monitor::tests::FixtureHost::new())).is_ok());
    settle(cx);
    wb.read_with(cx, |wb, cx| {
        assert!(!wb.remote.host.is_remote(), "an abandoned result must not switch the host");
        assert!(matches!(wb.remote.conn, Conn::Local));
        assert_eq!(wb.root(cx).as_deref(), Some(root.as_str()));
    });
}

#[gpui::test]
fn connection_attempt_late_failure_does_not_overwrite_a_new_attempt(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let first = wb.update_in(cx, |wb, window, cx| pending(wb, "fixture", window, cx));
    wb.update(cx, |wb, cx| wb.disconnect(cx));
    let second = wb.update_in(cx, |wb, window, cx| pending(wb, "fixture", window, cx));
    assert!(first.send(Err(host::HostError::Disconnected("obsolete failure".into()))).is_ok());
    settle(cx);
    assert!(wb.read_with(cx, |wb, _| matches!(&wb.remote.conn, Conn::Connecting { alias, .. } if alias == "fixture")));
    assert!(wb.read_with(cx, |wb, _| wb.notice.is_none()));
    assert!(second.send(Err(host::HostError::Disconnected("current failure".into()))).is_ok());
    settle(cx);
    assert!(wb.read_with(cx, |wb, _| matches!(&wb.remote.conn, Conn::Failed { message, .. } if message.contains("current failure"))));
}

#[gpui::test]
fn connection_attempt_same_folder_cancels_a_pending_connection(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let root = ui::local_path(&crate::qa_workbench_tests::fresh("attempt-same-folder"));
    wb.update(cx, |wb, cx| wb.set_root(root.clone(), cx));
    let tx = wb.update_in(cx, |wb, window, cx| pending(wb, "fixture", window, cx));
    wb.update(cx, |wb, cx| wb.set_root(root.clone(), cx));
    assert!(tx.send(Ok(crate::container_monitor::tests::FixtureHost::new())).is_ok());
    settle(cx);
    assert!(wb.read_with(cx, |wb, cx| matches!(wb.remote.conn, Conn::Local) && wb.root(cx).as_deref() == Some(root.as_str())));
}

#[gpui::test]
fn connection_attempt_container_failure_or_disable_keeps_ssh(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let source: Arc<dyn Host> = crate::container_monitor::tests::FixtureHost::new();
    let root = ui::local_path(&crate::qa_workbench_tests::fresh("attempt-preserved"));
    let alias = host::ssh::ssh_docker_target("fixture", "default", "web-id").unwrap();
    cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
    wb.update(cx, |wb, cx| {
        wb.remote.host = source.clone();
        wb.remote.conn = Conn::Connected { alias: "fixture".into() };
        wb.set_root(root.clone(), cx);
    });
    let tx = wb.update_in(cx, |wb, window, cx| pending(wb, &alias, window, cx));
    assert!(tx.send(Err(host::HostError::Disconnected("container stopped".into()))).is_ok());
    settle(cx);
    wb.read_with(cx, |wb, cx| {
        assert!(matches!(&wb.remote.conn, Conn::Connected { alias } if alias == "fixture"));
        assert!(Arc::ptr_eq(&wb.remote.host, &source));
        assert_eq!(wb.root(cx).as_deref(), Some(root.as_str()));
        assert!(wb.remote.docker.monitor.return_workspace.is_none());
        assert!(wb.notice.is_some());
    });
    let tx = wb.update_in(cx, |wb, window, cx| pending(wb, &alias, window, cx));
    cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = false);
    assert!(tx.send(Ok(crate::container_monitor::tests::FixtureHost::new())).is_ok());
    settle(cx);
    wb.read_with(cx, |wb, _| {
        assert!(matches!(&wb.remote.conn, Conn::Connected { alias } if alias == "fixture"));
        assert!(Arc::ptr_eq(&wb.remote.host, &source));
        assert!(wb.remote.docker.monitor.return_workspace.is_none());
        assert!(wb.remote.connection_origin.is_none());
    });
}

#[gpui::test]
fn connection_attempt_same_alias_late_success_cannot_win(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let first = wb.update_in(cx, |wb, window, cx| pending(wb, "fixture", window, cx));
    wb.update(cx, |wb, cx| wb.disconnect(cx));
    let root = ui::local_path(&crate::qa_workbench_tests::fresh("attempt-current"));
    let second = wb.update_in(cx, |wb, window, cx| {
        wb.remote.pending_path = Some(root.clone());
        pending(wb, "fixture", window, cx)
    });
    assert!(first.send(Ok(crate::container_monitor::tests::FixtureHost::new())).is_ok());
    settle(cx);
    assert!(wb.read_with(cx, |wb, _| matches!(wb.remote.conn, Conn::Connecting { .. }) && !wb.remote.host.is_remote()));
    let current: Arc<dyn Host> = crate::container_monitor::tests::FixtureHost::new();
    assert!(second.send(Ok(current.clone())).is_ok());
    settle(cx);
    assert!(wb.read_with(cx, |wb, cx| Arc::ptr_eq(&wb.remote.host, &current) && wb.root(cx).as_deref() == Some(root.as_str())));
}

#[gpui::test]
fn connection_attempt_preserves_edits_made_while_connecting(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let root = crate::qa_workbench_tests::project("attempt-dirty");
    crate::qa_workbench_tests::open(&wb, &root, cx);
    crate::qa_workbench_tests::open_in(&wb, &root.join("src/main.rs"), cx);
    let tx = wb.update_in(cx, |wb, window, cx| pending(wb, "fixture", window, cx));
    cx.simulate_input("// edited during connection\n");
    assert!(wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
    assert!(tx.send(Ok(crate::container_monitor::tests::FixtureHost::new())).is_ok());
    settle(cx);
    wb.read_with(cx, |wb, cx| {
        assert!(matches!(wb.remote.conn, Conn::Local));
        assert!(!wb.remote.host.is_remote());
        assert!(wb.any_dirty(cx));
        assert!(wb.active_editor().unwrap().read(cx).text().contains("// edited during connection"));
        assert_eq!(wb.root(cx), Some(ui::local_path(&root)));
    });
}
