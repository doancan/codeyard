use super::*;
use crate::language_service_install::tests::{FakeHost, Fixture, Outcome};
use gpui::{Entity, TestAppContext, VisualTestContext};

fn wait(page: &Entity<LanguageServicesPage>, cx: &mut VisualTestContext, predicate: impl Fn(&LanguageServicesPage) -> bool) {
    let start = std::time::Instant::now();
    loop {
        cx.run_until_parked();
        if page.read_with(cx, |page, _| predicate(page)) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "language service page did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui::test]
fn managed_service_page_repairs_an_invalid_installation_from_the_keyboard(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let fixture = Fixture::new();
    let host = FakeHost::new(&fixture, Outcome::Success);
    let root = installer::destination(host.as_ref()).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(format!("{root}/active.json"), "previous active generation").unwrap();
    let owner = host.clone();
    let (page, cx) = cx.add_window_view(move |window, cx| LanguageServicesPage::new(owner, window, cx));
    page.update_in(cx, |page, window, cx| window.focus(&page.focus_handle(cx)));
    wait(&page, cx, |page| matches!(page.status, Status::Failed(Error::InvalidInstallation)));
    cx.simulate_keystrokes("enter");
    wait(&page, cx, |page| matches!(page.status, Status::Available(Some(_))));
    assert!(installer::discover(host.as_ref(), &root).unwrap().is_some());
}

#[gpui::test]
fn managed_service_page_escape_cancels_the_process_and_cleans_staging(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let fixture = Fixture::new();
    let host = FakeHost::new(&fixture, Outcome::Cancel);
    let root = installer::destination(host.as_ref()).unwrap();
    let owner = host.clone();
    let (page, cx) = cx.add_window_view(move |window, cx| LanguageServicesPage::new(owner, window, cx));
    page.update_in(cx, |page, window, cx| window.focus(&page.focus_handle(cx)));
    wait(&page, cx, |page| matches!(page.status, Status::Available(None)));
    cx.simulate_keystrokes("enter");
    wait(&page, cx, |page| page.progress.snapshot().stage == Stage::Packages && host.waiting_for_cancel());
    cx.simulate_keystrokes("escape");
    wait(&page, cx, |page| matches!(page.status, Status::Failed(Error::Cancelled)));
    assert_eq!(host.kill_count.load(Ordering::Relaxed), 1);
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
}
