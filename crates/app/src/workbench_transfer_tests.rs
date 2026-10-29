use super::*;
use crate::qa_workbench_tests::{fresh, setup};
use gpui::TestAppContext;

fn copy_job(name: &str) -> (PathBuf, Job) {
    let root = fresh(name);
    std::fs::create_dir(root.join("target")).unwrap();
    let source = root.join("source.txt");
    std::fs::write(&source, "source data").unwrap();
    let job = Job::CopyIn { items: vec![source], dest_dir: ui::local_path(&root.join("target")) };
    (root, job)
}

#[test]
fn cancellation_words_in_connection_and_uncertain_errors_remain_failures() {
    for error in [
        host::HostError::Disconnected("connection to cancelled-project was lost".into()),
        host::HostError::Uncertain("copy cancelled after some destination files changed".into()),
        host::HostError::Op(proto::Error::PermissionDenied("/cancelled-project".into())),
    ] {
        let failure = partial(1, 2, std::path::Path::new("cancelled-project"), error.into());
        assert!(!failure.cancelled);
        assert_eq!(failure.partial.unwrap().1, 1);
    }
}

#[gpui::test]
fn a_filename_containing_cancelled_cannot_hide_a_transfer_error(cx: &mut TestAppContext) {
    let (root, mut job) = copy_job("transfer-cancelled-filename");
    if let Job::CopyIn { items, .. } = &mut job {
        items.push(root.join("cancelled-project.txt"));
    }
    let run = TransferRun::new("two files".into(), true);
    let error = execute(&job, Arc::new(host::local()), &run).unwrap_err();
    assert!(error.partial.is_some(), "an ordinary failure must retain the completed item count");
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| wb.finish_transfer(&job, Err(error), cx));
    assert!(wb.read_with(cx, |wb, _| matches!(wb.notice, Some(Notice::Error(_)))), "a filename is not evidence of cancellation");
    assert_eq!(std::fs::read_to_string(root.join("target/source.txt")).unwrap(), "source data");
}

#[gpui::test]
fn a_user_cancelled_transfer_still_reports_cancellation(cx: &mut TestAppContext) {
    let (root, job) = copy_job("transfer-user-cancelled");
    let run = TransferRun::new("one file".into(), true);
    run.cancel.store(true, Ordering::Relaxed);
    let result = execute(&job, Arc::new(host::local()), &run);
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, cx| wb.finish_transfer(&job, result, cx));
    assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "Transfer cancelled.")));
    assert!(!root.join("target/source.txt").exists());
}

#[gpui::test]
fn a_failed_destination_probe_cannot_start_a_transfer(cx: &mut TestAppContext) {
    let (root, mut job) = copy_job("transfer-probe-error");
    if let Job::CopyIn { dest_dir, .. } = &mut job {
        dest_dir.push('\0');
    }
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.begin(job, window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.remote.transfer.is_none()), "a failed conflict check must stop before starting the worker");
    assert!(wb.read_with(cx, |wb, _| matches!(wb.notice, Some(Notice::Error(_)))));
    assert!(!root.join("target/source.txt").exists());
}

#[gpui::test]
fn a_remote_permission_error_cannot_bypass_the_conflict_check(cx: &mut TestAppContext) {
    let (root, _) = copy_job("transfer-probe-permission");
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, mut resp_w) = std::io::pipe().unwrap();
    std::thread::spawn(move || {
        let mut requests = std::io::BufReader::new(req_r);
        while let Ok(Some(proto::Frame { id, body })) = proto::read_frame::<proto::Request>(&mut requests) {
            let response = match body {
                proto::Request::Hello { .. } => agent::handle(body),
                proto::Request::Stat { path } => proto::Response::Error(proto::Error::PermissionDenied(path)),
                proto::Request::Bye => break,
                _ => proto::Response::Done,
            };
            if proto::write_frame(&mut resp_w, &proto::Frame { id, body: response }).is_err() {
                break;
            }
        }
    });
    let host = Arc::new(host::RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "probe fixture").unwrap());
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.remote.host = host;
        // Invalid alias makes the old code's upload worker fail locally, without a network connection.
        wb.begin(Job::Upload { alias: "-invalid-fixture".into(), items: vec![root.join("source.txt")], remote_dir: "/target".into() }, window, cx);
    });
    crate::qa_integration_tests::wait_secs(cx, 5, "probe completion", |cx| wb.read_with(cx, |wb, _| wb.notice.is_some() || wb.remote.transfer.is_some()));
    assert!(wb.read_with(cx, |wb, _| wb.remote.transfer.is_none()));
    assert!(wb.read_with(cx, |wb, _| matches!(wb.notice, Some(Notice::Error(_)))));
}

#[gpui::test]
fn obsolete_transfer_completion_cannot_clear_a_newer_transfer(cx: &mut TestAppContext) {
    let (_root, job) = copy_job("transfer-obsolete-result");
    let (wb, cx) = setup(cx);
    let old = wb.update(cx, |wb, cx| {
        let host = wb.remote.host.clone();
        wb.run_job(job, &host, cx);
        wb.remote.transfer.clone().unwrap()
    });
    crate::qa_integration_tests::wait_secs(cx, 5, "copy worker completion", |_| old.result.lock().unwrap().is_some());
    let current = TransferRun::new("new transfer".into(), true);
    wb.update(cx, |wb, _| {
        wb.remote.transfer = Some(current.clone());
        wb.notice = None;
    });
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.remote.transfer.as_ref().is_some_and(|run| Arc::ptr_eq(run, &current))));
    assert!(wb.read_with(cx, |wb, _| wb.notice.is_none()));
    assert!(old.cancel.load(Ordering::Relaxed));
}

#[gpui::test]
fn transfer_completion_from_an_old_host_cannot_change_the_new_workspace(cx: &mut TestAppContext) {
    let (_root, job) = copy_job("transfer-old-host-result");
    let (wb, cx) = setup(cx);
    let old = wb.update(cx, |wb, cx| {
        let host = wb.remote.host.clone();
        wb.run_job(job, &host, cx);
        wb.remote.transfer.clone().unwrap()
    });
    crate::qa_integration_tests::wait_secs(cx, 5, "copy worker completion", |_| old.result.lock().unwrap().is_some());
    wb.update(cx, |wb, _| {
        wb.remote.host = Arc::new(host::local());
        wb.notice = None;
    });
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.notice.is_none()));
    assert!(wb.read_with(cx, |wb, _| wb.remote.transfer.is_none()));
    assert!(old.cancel.load(Ordering::Relaxed));
}

#[gpui::test]
fn pending_transfer_probe_cannot_replace_an_active_transfer(cx: &mut TestAppContext) {
    let (root, job) = copy_job("transfer-probe-busy");
    let (wb, cx) = setup(cx);
    let active = TransferRun::new("already running".into(), true);
    wb.update_in(cx, |wb, window, cx| {
        wb.begin(job, window, cx);
        wb.remote.transfer = Some(active.clone());
    });
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.remote.transfer.as_ref().is_some_and(|run| Arc::ptr_eq(run, &active))));
    assert!(!root.join("target/source.txt").exists());
}

#[gpui::test]
fn pending_transfer_probe_is_discarded_when_the_host_changes(cx: &mut TestAppContext) {
    let (root, job) = copy_job("transfer-probe-host");
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| {
        wb.begin(job, window, cx);
        wb.remote.host = Arc::new(host::local());
    });
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.remote.transfer.is_none()));
    assert!(!root.join("target/source.txt").exists());
}

#[gpui::test]
fn transfer_confirmation_cannot_start_on_a_different_host(cx: &mut TestAppContext) {
    let (root, job) = copy_job("transfer-confirm-host");
    std::fs::write(root.join("target/source.txt"), "keep original").unwrap();
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.begin(job, window, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    wb.update(cx, |wb, _| wb.remote.host = Arc::new(host::local()));
    cx.simulate_prompt_answer("Replace");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.remote.transfer.is_none()));
    assert_eq!(std::fs::read_to_string(root.join("target/source.txt")).unwrap(), "keep original");
}
