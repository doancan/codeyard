use super::*;
use crate::language_service_install::tests::{FakeHost, Fixture, Outcome};
use gpui::{TestAppContext, VisualTestContext, size};

fn attach(wb: &Entity<Workbench>, host: Arc<dyn Host>, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| {
        let page = cx.new(|cx| crate::language_services_page::LanguageServicesPage::new(host, window, cx));
        window.focus(&page.focus_handle(cx));
        wb.tabs.page = Some(Page::LanguageServices(page));
        wb.tabs.page_active = true;
        wb.tabs.diff_active = false;
        wb.tabs.large_active = false;
        cx.notify();
    });
}

fn snapshot(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Snapshot {
    wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx))
}

fn wait(wb: &Entity<Workbench>, cx: &mut VisualTestContext, predicate: impl Fn(&AccessView) -> bool) -> Snapshot {
    let started = std::time::Instant::now();
    loop {
        cx.run_until_parked();
        let view = snapshot(wb, cx);
        if view.services.as_ref().is_some_and(&predicate) {
            return view;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "setup accessibility did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn act(wb: &Entity<Workbench>, cx: &mut VisualTestContext, snap: &Snapshot, id: NodeId, action: Action) {
    let request = ActionRequest { action, target_tree: TreeId::ROOT, target_node: id, data: None };
    wb.update_in(cx, |wb, window, cx| wb.a11y_action_for_test(&request, snap, window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn managed_service_accessibility_controls_have_visible_bounds_and_scroll_without_moving_the_install_button(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let host = FakeHost::new(&fixture, Outcome::Success);
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    attach(&wb, host, cx);
    wait(&wb, cx, |view| view.can_install);
    for width in [480., 1280.] {
        cx.simulate_resize(size(px(width), px(600.)));
        cx.run_until_parked();
        let snap = snapshot(&wb, cx);
        let page = snap.services.as_ref().unwrap();
        let button = page.bounds[3].expect("painted install bounds");
        let window = page.bounds[0].expect("painted page bounds");
        assert!(button.width() > 0. && button.height() > 0.);
        assert!(
            button.x0 >= window.x0 && button.x1 <= window.x1 && button.y0 >= window.y0 && button.y1 <= window.y1,
            "install must stay in view at {width}: {button:?} within {window:?}"
        );
        let tree = accesskit_consumer::Tree::new(snap.to_tree(), true);
        assert!(tree.state().root().children().any(|group| group.role() == Role::Group
            && group.children().any(|node| node.role() == Role::Button && node.label().as_deref() == Some(page.install_label.as_str()))));
        act(&wb, cx, &snap, SERVICES, Action::ScrollDown);
        assert_eq!(snapshot(&wb, cx).services.unwrap().bounds[3], Some(button), "scrolling details must keep the installation control visible");
    }
}

#[gpui::test]
fn managed_service_accessibility_actions_cancel_and_reject_completed_or_replaced_page_requests(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let host = FakeHost::new(&fixture, Outcome::Cancel);
    let root = crate::language_service_install::destination(host.as_ref()).unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    attach(&wb, host.clone(), cx);
    let before = wait(&wb, cx, |view| view.can_install);
    act(&wb, cx, &before, SERVICES_INSTALL, Action::Click);
    let installing = wait(&wb, cx, |view| view.can_cancel && host.waiting_for_cancel());
    assert_eq!(installing.to_tree().focus, SERVICES_CANCEL);
    act(&wb, cx, &installing, SERVICES_CANCEL, Action::Click);
    wait(&wb, cx, |view| view.can_install);
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
    let starts = host.install_starts();
    act(&wb, cx, &before, SERVICES_INSTALL, Action::Click);
    assert_eq!(host.install_starts(), starts, "a request captured before a completed installation must not reinstall");
    let other = Fixture::new();
    let replacement = FakeHost::new(&other, Outcome::Success);
    attach(&wb, replacement.clone(), cx);
    wait(&wb, cx, |view| view.can_install);
    act(&wb, cx, &before, SERVICES_INSTALL, Action::Click);
    assert_eq!(replacement.install_starts(), 0, "a stale page must not target its replacement");
}
