//! The update dialog, its status bar item and the rules around them, against a scripted backend: no network, no installer, no
//! real quit. The core (`update.rs`, `update_install.rs`) has its own tests; here only what the workbench decides is checked.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext};

use crate::qa_integration_tests::wait_ticking;
use crate::qa_workbench_tests::{fresh, open, open_in, project, setup, setup_persistent, setup_with_config};
use crate::ui;
use crate::update::{Asset, Downloaded, Progress, Release, UpdateError};
use crate::update_backend::UpdateBackend;
use crate::update_install::{InstallKind, InstallPlan, ManualReason};
use crate::workbench::Workbench;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const APP: &str = "/Applications/Codeyard.app";

fn release(version: &str) -> Release {
    Release {
        version: version.into(),
        notes: "## What is new\n\n- Faster **start**\n- A [link](https://example.com)\n".into(),
        asset: Some(Asset { name: format!("Codeyard-{version}-macos-arm64.dmg"), url: "https://example.invalid/x.dmg".into(), sha256: "0".repeat(64) }),
    }
}

fn mac_plan() -> InstallPlan {
    InstallPlan::MacApp { app: PathBuf::from(APP) }
}

fn network() -> UpdateError {
    UpdateError::Network("curl: (6) Could not resolve host".into())
}

enum Download {
    /// Reports progress and returns the file.
    Done,
    Fail(UpdateError),
    /// Reports progress until cancelled, then answers like the real download.
    UntilCancelled,
    /// Waits for `release_download`, then fails.
    FailWhenReleased(UpdateError),
}

/// A backend whose answers the test scripts. Every call is written to `calls`, in order.
struct Fake {
    checks: Mutex<VecDeque<Result<Option<Release>, UpdateError>>>,
    downloads: Mutex<VecDeque<Download>>,
    installs: Mutex<VecDeque<Result<InstallPlan, UpdateError>>>,
    /// While set, `stage_and_install` waits: the swap is in progress.
    hold_install: AtomicBool,
    kind: Mutex<InstallKind>,
    now: AtomicU64,
    relaunch_fails: AtomicBool,
    calls: Mutex<Vec<String>>,
    /// A download that was cancelled has returned and cleaned up.
    cleaned_up: AtomicBool,
    release_download: AtomicBool,
}

impl Fake {
    fn new() -> Arc<Fake> {
        Arc::new(Fake {
            checks: Mutex::default(),
            downloads: Mutex::default(),
            installs: Mutex::default(),
            hold_install: AtomicBool::new(false),
            kind: Mutex::new(InstallKind::MacBundle { app: PathBuf::from(APP) }),
            now: AtomicU64::new(1_800_000_000),
            relaunch_fails: AtomicBool::new(false),
            calls: Mutex::default(),
            cleaned_up: AtomicBool::new(false),
            release_download: AtomicBool::new(false),
        })
    }

    fn check_returns(&self, r: Result<Option<Release>, UpdateError>) {
        self.checks.lock().unwrap().push_back(r);
    }

    fn download_does(&self, d: Download) {
        self.downloads.lock().unwrap().push_back(d);
    }

    fn install_returns(&self, r: Result<InstallPlan, UpdateError>) {
        self.installs.lock().unwrap().push_back(r);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, call: &str) -> usize {
        self.calls().iter().filter(|c| *c == call).count()
    }

    fn log(&self, call: &str) {
        self.calls.lock().unwrap().push(call.into());
    }

    /// Waits about 100 ms, or until `stop` says so.
    fn nap(&self, stop: &dyn Fn() -> bool) {
        for _ in 0..20 {
            if stop() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn advance(&self, by: Duration) {
        self.now.fetch_add(by.as_secs(), Ordering::SeqCst);
    }
}

impl UpdateBackend for Fake {
    fn version(&self) -> &str {
        "0.1.0"
    }

    fn check(&self) -> Result<Option<Release>, UpdateError> {
        self.log("check");
        self.checks.lock().unwrap().pop_front().unwrap_or_else(|| Err(UpdateError::Network("unscripted check".into())))
    }

    fn download(&self, release: &Release, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Downloaded, UpdateError> {
        self.log("download");
        let script = self.downloads.lock().unwrap().pop_front().unwrap_or(Download::Fail(UpdateError::Network("unscripted download".into())));
        match script {
            Download::Done => {
                progress(Progress { done: 5_000_000, total: Some(10_000_000) });
                progress(Progress { done: 10_000_000, total: Some(10_000_000) });
                Ok(Downloaded { release: release.clone(), path: PathBuf::from("/nonexistent/installer.dmg") })
            }
            Download::Fail(e) => Err(e),
            Download::UntilCancelled => {
                // The real download reports about ten times a second; more would only make the test window redraw for nothing.
                let mut done = 0;
                while !cancel.load(Ordering::Relaxed) {
                    done += 1_000_000;
                    progress(Progress { done: done.min(9_000_000), total: Some(10_000_000) });
                    self.nap(&|| cancel.load(Ordering::Relaxed));
                }
                self.cleaned_up.store(true, Ordering::SeqCst);
                Err(UpdateError::Cancelled)
            }
            Download::FailWhenReleased(e) => {
                progress(Progress { done: 1_000_000, total: Some(10_000_000) });
                while !self.release_download.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e)
            }
        }
    }

    fn stage_and_install(&self, _: &Downloaded, kind: &InstallKind) -> Result<InstallPlan, UpdateError> {
        self.log(&format!("install {}", if matches!(kind, InstallKind::WindowsInstalled) { "windows" } else { "mac" }));
        while self.hold_install.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.installs.lock().unwrap().pop_front().unwrap_or(Ok(mac_plan()))
    }

    fn discard(&self, _: Downloaded) {
        self.log("discard");
    }

    fn relaunch(&self, _: &InstallPlan) -> io::Result<()> {
        self.log("relaunch");
        if self.relaunch_fails.load(Ordering::SeqCst) { Err(io::Error::other("no /usr/bin/open")) } else { Ok(()) }
    }

    fn install_kind(&self) -> InstallKind {
        self.kind.lock().unwrap().clone()
    }

    fn now(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(self.now.load(Ordering::SeqCst))
    }

    fn quit(&self, _: &mut App) {
        self.log("quit");
    }
}

type Rig<'a> = (Entity<Workbench>, &'a mut VisualTestContext, Arc<Fake>);

fn rig(cx: &mut TestAppContext) -> Rig<'_> {
    let (wb, cx) = setup(cx);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    (wb, cx, fake)
}

fn state(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.update_state_for_test())
}

fn dialog(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<crate::workbench::DialogView> {
    wb.read_with(cx, |wb, cx| wb.update_dialog_for_test(cx))
}

fn press(wb: &Entity<Workbench>, cx: &mut VisualTestContext, label: &str) {
    wb.update_in(cx, |wb, window, cx| wb.press_update_button_for_test(label, window, cx));
    cx.run_until_parked();
}

fn manual_check(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    run_command(wb, cx, "Help: Check for Updates…");
}

fn run_command(wb: &Entity<Workbench>, cx: &mut VisualTestContext, label: &str) {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
}

fn wait_state(wb: &Entity<Workbench>, cx: &mut VisualTestContext, want: &str) {
    wait_ticking(cx, want, |cx| state(wb, cx) == want);
}

fn focused_button(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, cx| wb.read(cx).focused_update_button_for_test(window, cx))
}

fn notes_focused(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| wb.read(cx).notes_focused_for_test(window))
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<(bool, String)> {
    wb.read_with(cx, |wb, _| wb.notice_for_test())
}

fn set_policy(cx: &mut VisualTestContext, policy: crate::update::Policy) {
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().update_check = policy);
}

/// A check the user asks for, answered with `release`, ends in the offer.
fn offered(wb: &Entity<Workbench>, cx: &mut VisualTestContext, fake: &Fake, version: &str) {
    fake.check_returns(Ok(Some(release(version))));
    manual_check(wb, cx);
    wait_state(wb, cx, &format!("available {version}"));
}

// --- The command and the dialog's states ---

#[gpui::test]
fn check_for_updates_opens_the_dialog_in_the_checking_state_whatever_the_policy(cx: &mut TestAppContext) {
    for policy in [crate::update::Policy::Ask, crate::update::Policy::Daily, crate::update::Policy::Manual] {
        let (wb, cx, fake) = rig(cx);
        set_policy(cx, policy);
        fake.check_returns(Ok(None));
        manual_check(&wb, cx);
        let d = dialog(&wb, cx).expect("the dialog opens at once");
        assert_eq!((d.title.as_str(), d.buttons.as_slice()), ("Checking for updates…", ["Cancel".to_string()].as_slice()), "{policy:?}");
        wait_state(&wb, cx, "up-to-date");
        assert_eq!(fake.count("check"), 1);
    }
}

/// The dialog drawn in the window: the dialog and each of its buttons have the frame they are drawn at (the buttons
/// in the dialog's footer), and AccessKit's hit test finds each button at its centre.
#[gpui::test]
fn the_dialog_and_its_buttons_have_their_painted_frames(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "9.0.0");
    cx.run_until_parked();
    let update = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx)).to_tree();
    let tree = accesskit_consumer::Tree::new(update, true);
    let state = tree.state();
    let dialog = state.root().children().find(|n| n.role() == accesskit::Role::Dialog).expect("the dialog is in the tree");
    let outer = dialog.data().bounds().expect("the dialog's frame");
    let buttons: Vec<_> = dialog.children().filter(|n| n.role() == accesskit::Role::Button).collect();
    assert!(buttons.len() >= 2, "{} buttons", buttons.len());
    for b in buttons {
        let r = b.data().bounds().unwrap_or_else(|| panic!("{:?} has a frame", b.label()));
        // (The test platform's wide stand-in font can push the last button past the dialog's right edge.)
        assert!(r.x0 >= outer.x0 && r.x0 < outer.x1 && r.y0 >= outer.y0 && r.y1 <= outer.y1, "{r:?} in {outer:?}");
        let at = accesskit::Point::new((r.x0 + r.x1) / 2., (r.y0 + r.y1) / 2.);
        assert_eq!(state.root().node_at_point(at, &|_| accesskit_consumer::FilterResult::Include).map(|n| n.id()), Some(b.id()), "{:?}", b.label());
    }
}

#[gpui::test]
fn up_to_date_says_which_version_you_have(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    fake.check_returns(Ok(None));
    manual_check(&wb, cx);
    wait_state(&wb, cx, "up-to-date");
    let d = dialog(&wb, cx).unwrap();
    assert_eq!((d.title.as_str(), d.subtitle.as_deref()), ("Codeyard is up to date", Some("You have 0.1.0, the latest version.")));
    assert_eq!((d.buttons, d.primary.as_deref()), (vec!["Close".to_string()], Some("Close")));
    press(&wb, cx, "Close");
    assert!((dialog(&wb, cx).is_none(), state(&wb, cx)) == (true, "idle".into()), "closing forgets the answer");
}

#[gpui::test]
fn an_available_release_shows_its_notes_and_the_four_choices(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    *fake.kind.lock().unwrap() = InstallKind::MacBundle { app: PathBuf::from(APP) };
    offered(&wb, cx, &fake, "0.2.0");
    let d = dialog(&wb, cx).unwrap();
    assert_eq!((d.title.as_str(), d.subtitle.as_deref()), ("Codeyard 0.2.0 is available", Some("You have 0.1.0")));
    assert_eq!(d.buttons, ["Skip This Version", "Release Page", "Later", "Install and Restart"]);
    assert_eq!(d.primary.as_deref(), Some("Install and Restart"));
    assert_eq!(wb.read_with(cx, |wb, _| wb.update_status_for_test()), Some(("Update 0.2.0".into(), "Codeyard 0.2.0 is available. Click for details.".into())));
}

#[gpui::test]
fn a_copy_that_cannot_update_itself_gets_a_download_page_button_and_the_reason(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    *fake.kind.lock().unwrap() = InstallKind::Manual(ManualReason::NotInstalled);
    offered(&wb, cx, &fake, "0.2.0");
    let d = dialog(&wb, cx).unwrap();
    assert_eq!(d.buttons, ["Skip This Version", "Later", "Download Page"]);
    assert_eq!(d.primary.as_deref(), Some("Download Page"));
    assert!(d.text.contains("This copy isn't installed as an app, so download the new version yourself."), "{}", d.text);
    // A release without an installer for this system says so instead.
    let (wb, cx, fake) = rig(cx);
    let mut r = release("0.2.0");
    r.asset = None;
    fake.check_returns(Ok(Some(r)));
    manual_check(&wb, cx);
    wait_state(&wb, cx, "available 0.2.0");
    let d = dialog(&wb, cx).unwrap();
    assert_eq!(d.primary.as_deref(), Some("Download Page"));
    assert!(d.text.contains("no installer for this system"), "{}", d.text);
    press(&wb, cx, "Download Page");
    assert_eq!(cx.opened_url().as_deref(), Some(format!("{}/releases/tag/v0.2.0", env!("CARGO_PKG_REPOSITORY")).as_str()));
}

#[gpui::test]
fn the_release_page_button_opens_this_release_in_the_browser(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    press(&wb, cx, "Release Page");
    assert_eq!(cx.opened_url().as_deref(), Some(format!("{}/releases/tag/v0.2.0", env!("CARGO_PKG_REPOSITORY")).as_str()));
    assert!(dialog(&wb, cx).is_some(), "the dialog stays; the page is only a place to read more");
}

#[gpui::test]
fn install_downloads_installs_and_waits_for_the_restart(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Done);
    fake.install_returns(Ok(mac_plan()));
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "ready 0.2.0");
    assert_eq!(fake.calls(), ["check", "download", "install mac", "discard"]);
    let d = dialog(&wb, cx).unwrap();
    assert_eq!(d.title, "Update installed");
    assert!(d.text.contains("Restart Codeyard to finish."), "{}", d.text);
    assert_eq!((d.buttons, d.primary.as_deref()), (vec!["Later".to_string(), "Restart Now".to_string()], Some("Restart Now")));
    assert_eq!(wb.read_with(cx, |wb, _| wb.state_value_for_test("update.installed")).as_deref(), Some("0.2.0"));
    assert_eq!(fake.count("quit"), 0, "nothing quits until the user says so");
}

#[gpui::test]
fn downloading_shows_progress_and_the_size_in_megabytes(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::UntilCancelled);
    press(&wb, cx, "Install and Restart");
    wait_ticking(cx, "some progress", |cx| dialog(&wb, cx).is_some_and(|d| d.text.contains(" of 9.5 MB")));
    let d = dialog(&wb, cx).unwrap();
    assert_eq!(d.title, "Downloading Codeyard 0.2.0");
    assert!(matches!(d.progress, Some(Some(f)) if f > 0. && f <= 1.), "{:?}", d.progress);
    assert_eq!(d.buttons, ["Cancel", "Hide"]);
    press(&wb, cx, "Cancel");
}

#[gpui::test]
fn cancelling_a_download_returns_to_the_offer_without_an_error(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::UntilCancelled);
    press(&wb, cx, "Install and Restart");
    wait_ticking(cx, "downloading", |cx| dialog(&wb, cx).is_some_and(|d| d.text.contains(" of ")));
    press(&wb, cx, "Cancel");
    wait_state(&wb, cx, "available 0.2.0");
    assert!(fake.cleaned_up.load(Ordering::SeqCst));
    assert_eq!(notice(&wb, cx), None, "cancelling is not a failure");
    assert_eq!(dialog(&wb, cx).unwrap().title, "Codeyard 0.2.0 is available");
}

#[gpui::test]
fn closing_the_dialog_keeps_the_download_going_and_the_status_item_shows_it(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::UntilCancelled);
    press(&wb, cx, "Install and Restart");
    wait_ticking(cx, "downloading", |cx| dialog(&wb, cx).is_some_and(|d| d.text.contains(" of ")));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(dialog(&wb, cx).is_none(), "Esc hides the dialog");
    assert_eq!(state(&wb, cx), "downloading 0.2.0", "…and the download goes on");
    wait_ticking(cx, "percent in the status bar", |cx| {
        wb.read_with(cx, |wb, _| wb.update_status_for_test()).is_some_and(|(label, _)| label.starts_with("Updating… ") && label.ends_with('%'))
    });
    let (_, tip) = wb.read_with(cx, |wb, _| wb.update_status_for_test()).unwrap();
    assert_eq!(tip, "Downloading Codeyard 0.2.0. Click for details.");
    // The item reopens the dialog on the progress, and a second check shows it instead of starting another one.
    wb.update_in(cx, |wb, window, cx| wb.open_update_dialog_for_test(window, cx));
    assert_eq!(dialog(&wb, cx).unwrap().title, "Downloading Codeyard 0.2.0");
    press(&wb, cx, "Hide");
    manual_check(&wb, cx);
    assert_eq!(dialog(&wb, cx).unwrap().title, "Downloading Codeyard 0.2.0");
    assert_eq!(fake.count("check"), 1, "one update operation at a time");
    press(&wb, cx, "Cancel");
    wait_state(&wb, cx, "available 0.2.0");
}

#[gpui::test]
fn the_install_step_cannot_be_cancelled_and_says_what_it_does(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Done);
    fake.hold_install.store(true, Ordering::SeqCst);
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "installing 0.2.0");
    let d = dialog(&wb, cx).unwrap();
    assert_eq!(d.title, "Installing Codeyard 0.2.0");
    assert!(d.text.contains("Verifying and installing…"), "{}", d.text);
    assert_eq!(d.buttons, ["Hide"], "there is no Cancel once the swap may have started");
    assert_eq!(d.progress, Some(None), "indeterminate");
    fake.hold_install.store(false, Ordering::SeqCst);
    wait_state(&wb, cx, "ready 0.2.0");
}

// --- Errors ---

#[gpui::test]
fn a_network_error_offers_try_again_and_the_release_page(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    fake.check_returns(Err(network()));
    manual_check(&wb, cx);
    wait_state(&wb, cx, "error network");
    let d = dialog(&wb, cx).unwrap();
    assert_eq!(d.title, "Couldn't check for updates");
    assert!(d.text.contains("couldn't reach the update server") && d.text.contains("Could not resolve host"), "plain lead, English detail: {}", d.text);
    assert_eq!(d.buttons, ["Release Page", "Close", "Try Again"]);
    // Try Again is a new check; this time it works.
    fake.check_returns(Ok(None));
    press(&wb, cx, "Try Again");
    wait_state(&wb, cx, "up-to-date");
    assert_eq!(fake.count("check"), 2);
    // Without a release the page is the list of releases.
    fake.check_returns(Err(network()));
    press(&wb, cx, "Close");
    manual_check(&wb, cx);
    wait_state(&wb, cx, "error network");
    press(&wb, cx, "Release Page");
    assert_eq!(cx.opened_url().as_deref(), Some(format!("{}/releases/latest", env!("CARGO_PKG_REPOSITORY")).as_str()));
}

#[gpui::test]
fn a_failed_download_retries_the_download_not_the_check(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Fail(network()));
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "error network");
    assert_eq!(dialog(&wb, cx).unwrap().title, "Couldn't download the update");
    fake.download_does(Download::Done);
    press(&wb, cx, "Try Again");
    wait_state(&wb, cx, "ready 0.2.0");
    assert_eq!(fake.count("check"), 1, "the release is already known");
    assert_eq!(fake.count("download"), 2);
}

#[gpui::test]
fn an_untrusted_download_installs_nothing_and_only_points_to_the_release_page(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Fail(UpdateError::HashMismatch("Codeyard-0.2.0-macos-arm64.dmg".into())));
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "error untrusted");
    let d = dialog(&wb, cx).unwrap();
    assert_eq!(d.title, "The update couldn't be verified");
    assert!(d.text.contains("Nothing was installed") && d.text.contains("signed checksums"), "{}", d.text);
    assert!(d.text.contains("does not match its signed checksum"), "the technical text stays visible: {}", d.text);
    assert_eq!((d.buttons, d.primary.as_deref()), (vec!["Close".to_string(), "Release Page".to_string()], Some("Release Page")));
    assert_eq!(fake.count("install mac"), 0);
}

#[gpui::test]
fn an_install_error_says_so_and_its_status_item_goes_when_the_dialog_is_closed(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Done);
    fake.install_returns(Err(UpdateError::NoWritePermission(PathBuf::from("/Applications"))));
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "error install");
    let d = dialog(&wb, cx).unwrap();
    assert_eq!(d.title, "The update couldn't be installed");
    assert!(d.text.contains("cannot write to /Applications"), "{}", d.text);
    assert_eq!(fake.count("discard"), 1, "the downloaded file is not left behind");
    assert_eq!(wb.read_with(cx, |wb, _| wb.update_status_for_test()).map(|s| s.0), Some("Update failed".into()), "while the error is on screen");
    press(&wb, cx, "Close");
    assert_eq!(wb.read_with(cx, |wb, _| wb.update_status_for_test()), None, "read and closed: the red item goes away");
    assert_eq!(state(&wb, cx), "idle");
    // …and a background check is not held back by the error that was dismissed: it finds the release again.
    fake.check_returns(Ok(Some(release("0.2.0"))));
    wb.update_in(cx, |wb, window, cx| wb.background_check_for_test(window, cx));
    wait_state(&wb, cx, "available 0.2.0");
}

#[gpui::test]
fn a_background_check_that_fails_stays_quiet(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    fake.check_returns(Err(network()));
    wb.update_in(cx, |wb, window, cx| wb.background_check_for_test(window, cx));
    wait_ticking(cx, "the check ran", |_| fake.count("check") == 1);
    wait_state(&wb, cx, "idle");
    assert_eq!((notice(&wb, cx), dialog(&wb, cx).is_none()), (None, true), "stderr only: nobody asked");
}

#[gpui::test]
fn a_download_that_fails_after_the_dialog_was_hidden_tells_the_user(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::FailWhenReleased(network()));
    press(&wb, cx, "Install and Restart");
    wait_ticking(cx, "downloading", |cx| state(&wb, cx) == "downloading 0.2.0");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    fake.release_download.store(true, Ordering::SeqCst);
    wait_state(&wb, cx, "error network");
    assert_eq!(notice(&wb, cx), Some((true, "Couldn't download the update".into())));
    assert!(dialog(&wb, cx).is_none(), "the dialog does not jump back in front of the user's work");
    assert_eq!(wb.read_with(cx, |wb, _| wb.update_status_for_test()).map(|s| s.0), Some("Update failed".into()));
}

// --- When something is looked for ---

#[gpui::test]
fn policy_ask_and_manual_never_make_a_request(cx: &mut TestAppContext) {
    for policy in [crate::update::Policy::Ask, crate::update::Policy::Manual] {
        let (wb, cx, fake) = rig(cx);
        set_policy(cx, policy);
        for _ in 0..3 {
            wb.update_in(cx, |wb, window, cx| wb.check_if_due_for_test(window, cx));
            fake.advance(DAY * 3);
        }
        // The startup timers too: the whole first minute, and then days.
        wb.update_in(cx, |wb, window, cx| wb.schedule_update_checks_for_test(window, cx));
        for _ in 0..40 {
            cx.executor().advance_clock(Duration::from_secs(5));
            cx.run_until_parked();
        }
        cx.executor().advance_clock(DAY);
        cx.run_until_parked();
        assert_eq!(fake.calls(), Vec::<String>::new(), "{policy:?} made a request by itself");
    }
}

#[gpui::test]
fn policy_daily_checks_when_a_day_has_passed_and_records_only_an_answer(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    set_policy(cx, crate::update::Policy::Daily);
    // Never checked: due. A network failure is not an answer: it is not recorded, the next start tries again.
    fake.check_returns(Err(network()));
    wb.update_in(cx, |wb, window, cx| wb.check_if_due_for_test(window, cx));
    wait_ticking(cx, "first check", |_| fake.count("check") == 1);
    wait_state(&wb, cx, "idle");
    assert_eq!(wb.read_with(cx, |wb, _| wb.state_value_for_test("update.last_check")), None);
    // Up to date counts.
    fake.check_returns(Ok(None));
    wb.update_in(cx, |wb, window, cx| wb.check_if_due_for_test(window, cx));
    wait_ticking(cx, "second check", |_| fake.count("check") == 2);
    wait_ticking(cx, "recorded", |cx| wb.read_with(cx, |wb, _| wb.state_value_for_test("update.last_check")).is_some());
    assert_eq!(wb.read_with(cx, |wb, _| wb.state_value_for_test("update.last_check")), Some("1800000000000".into()));
    // Within a day: not due. After a day: due again.
    fake.advance(DAY - Duration::from_secs(1));
    wb.update_in(cx, |wb, window, cx| wb.check_if_due_for_test(window, cx));
    cx.run_until_parked();
    assert_eq!(fake.count("check"), 2);
    fake.advance(Duration::from_secs(1));
    fake.check_returns(Ok(None));
    wb.update_in(cx, |wb, window, cx| wb.check_if_due_for_test(window, cx));
    wait_ticking(cx, "third check", |_| fake.count("check") == 3);
}

#[gpui::test]
fn the_startup_timers_check_daily_after_fifteen_seconds_and_not_before(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    set_policy(cx, crate::update::Policy::Daily);
    fake.check_returns(Ok(None));
    wb.update_in(cx, |wb, window, cx| wb.schedule_update_checks_for_test(window, cx));
    cx.executor().advance_clock(Duration::from_secs(14));
    cx.run_until_parked();
    assert_eq!(fake.count("check"), 0, "the window gets a head start");
    cx.executor().advance_clock(Duration::from_secs(2));
    wait_ticking(cx, "the daily check", |_| fake.count("check") == 1);
}

#[gpui::test]
fn a_found_release_in_the_background_shows_one_notice_per_version_and_a_status_item_but_no_dialog(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    let background = |wb: &Entity<Workbench>, cx: &mut VisualTestContext, fake: &Fake, version: &str| {
        let calls = fake.count("check");
        fake.check_returns(Ok(Some(release(version))));
        wb.update_in(cx, |wb, window, cx| wb.background_check_for_test(window, cx));
        wait_ticking(cx, "the check ran", |_| fake.count("check") == calls + 1);
        wait_state(wb, cx, &format!("available {version}"));
    };
    background(&wb, cx, &fake, "0.2.0");
    assert_eq!(notice(&wb, cx), Some((false, "Codeyard 0.2.0 is available.".into())));
    assert!(dialog(&wb, cx).is_none(), "a background check never takes over the window");
    assert!(wb.read_with(cx, |wb, _| wb.update_status_for_test()).is_some());
    // The same version found again (the next day, the next start): the item stays, the notice does not come back.
    wb.update(cx, |wb, cx| wb.set_notice_for_test("something else", false, cx));
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    background(&wb, cx, &fake, "0.2.0");
    assert_eq!(notice(&wb, cx), Some((false, "something else".into())));
    // A newer version is news again.
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    background(&wb, cx, &fake, "0.3.0");
    assert_eq!(notice(&wb, cx), Some((false, "Codeyard 0.3.0 is available.".into())));
}

// --- Skipping ---

#[gpui::test]
fn skipping_a_version_silences_background_paths_but_a_manual_check_still_offers_it(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    press(&wb, cx, "Skip This Version");
    assert_eq!((state(&wb, cx), dialog(&wb, cx).is_none()), ("idle".to_string(), true));
    assert_eq!(wb.read_with(cx, |wb, _| wb.state_value_for_test("update.skipped")).as_deref(), Some("0.2.0"));
    assert!(wb.read_with(cx, |wb, _| wb.update_status_for_test()).is_none(), "the item goes away");
    // A background check finds 0.2.0 again: nothing, not even the notice.
    wb.update(cx, |wb, cx| wb.set_notice_for_test("x", false, cx));
    fake.check_returns(Ok(Some(release("0.2.0"))));
    wb.update_in(cx, |wb, window, cx| wb.background_check_for_test(window, cx));
    wait_ticking(cx, "second check", |_| fake.count("check") == 2);
    wait_state(&wb, cx, "idle");
    assert_eq!(notice(&wb, cx), Some((false, "x".into())), "no notice for a skipped version");
    // The user asking offers the skipped one regardless.
    fake.check_returns(Ok(Some(release("0.2.0"))));
    manual_check(&wb, cx);
    wait_state(&wb, cx, "available 0.2.0");
    assert_eq!(dialog(&wb, cx).unwrap().title, "Codeyard 0.2.0 is available");
    // A newer version is not skipped.
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    fake.check_returns(Ok(Some(release("0.3.0"))));
    wb.update_in(cx, |wb, window, cx| wb.background_check_for_test(window, cx));
    wait_state(&wb, cx, "available 0.3.0");
    assert_eq!(notice(&wb, cx), Some((false, "Codeyard 0.3.0 is available.".into())));
}

// --- Later after installing ---

#[gpui::test]
fn later_after_an_install_does_not_offer_the_same_version_again(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Done);
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "ready 0.2.0");
    press(&wb, cx, "Later");
    assert!(dialog(&wb, cx).is_none());
    assert_eq!(wb.read_with(cx, |wb, _| wb.update_status_for_test()).map(|s| s.0), Some("Restart to update".into()));
    // Another window, or the next session of the old process: the check finds 0.2.0, which is already on disk.
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    set_policy(cx, crate::update::Policy::Daily);
    fake.check_returns(Ok(Some(release("0.2.0"))));
    wb.update_in(cx, |wb, window, cx| wb.background_check_for_test(window, cx));
    wait_state(&wb, cx, "ready 0.2.0");
    assert_eq!(notice(&wb, cx).filter(|(_, m)| m.contains("is available")), None);
    // Asking by hand shows the restart button, not the install button.
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    fake.check_returns(Ok(Some(release("0.2.0"))));
    manual_check(&wb, cx);
    wait_state(&wb, cx, "ready 0.2.0");
    assert_eq!(dialog(&wb, cx).unwrap().primary.as_deref(), Some("Restart Now"));
    assert_eq!(fake.count("download"), 1);
}

// --- The question ---

fn settings_dir(name: &str, text: &str) -> PathBuf {
    let dir = fresh(name);
    std::fs::write(dir.join("settings.jsonc"), text).unwrap();
    dir
}

const COMMENTED: &str = "// my settings\n{\n  // keep this comment\n  \"editor.fontSize\": 15,\n}\n";

#[gpui::test]
fn the_question_explains_the_choice_briefly_and_has_two_buttons(cx: &mut TestAppContext) {
    let (wb, cx, _fake) = rig(cx);
    assert!(wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    let d = dialog(&wb, cx).expect("the question is on screen");
    assert_eq!(d.title, "Check for updates automatically?");
    assert!(d.text.contains("daily") && d.text.contains("Settings"), "the choice and where to change it remain clear: {}", d.text);
    assert!(d.text.chars().count() < 120, "keep the title and explanation concise: {}", d.text);
    assert_eq!((d.buttons, d.primary.as_deref()), (vec!["Only When I Ask".to_string(), "Check Daily".to_string()], Some("Check Daily")));
}

#[gpui::test]
fn check_daily_writes_the_setting_keeps_the_comments_and_checks_now(cx: &mut TestAppContext) {
    let dir = settings_dir("update-ask-daily", COMMENTED);
    let (wb, cx) = setup_with_config(cx, &dir);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    fake.check_returns(Ok(None));
    assert!(wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    press(&wb, cx, "Check Daily");
    let text = std::fs::read_to_string(dir.join("settings.jsonc")).unwrap();
    assert!(text.contains("\"update.check\": \"daily\""), "{text}");
    assert!(text.contains("// my settings") && text.contains("// keep this comment") && text.contains("\"editor.fontSize\": 15"), "{text}");
    assert_eq!(wb.read_with(cx, |_, cx| cx.global::<crate::settings::Settings>().update_check), crate::update::Policy::Daily);
    assert!(dialog(&wb, cx).is_none());
    wait_ticking(cx, "the first daily check", |_| fake.count("check") == 1);
    // Decided: it is never asked again.
    assert!(wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    assert!(dialog(&wb, cx).is_none());
}

#[gpui::test]
fn only_when_i_ask_writes_manual_and_makes_no_request(cx: &mut TestAppContext) {
    let dir = settings_dir("update-ask-manual", COMMENTED);
    let (wb, cx) = setup_with_config(cx, &dir);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    assert!(wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    press(&wb, cx, "Only When I Ask");
    let text = std::fs::read_to_string(dir.join("settings.jsonc")).unwrap();
    assert!(text.contains("\"update.check\": \"manual\"") && text.contains("// keep this comment"), "{text}");
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    assert_eq!(fake.calls(), Vec::<String>::new());
}

/// Esc is not an answer. Nothing is written and nothing is sent; the question comes back, but after 30 days, not at every start.
#[gpui::test]
fn esc_on_the_question_writes_nothing_and_snoozes_it_for_thirty_days(cx: &mut TestAppContext) {
    let dir = settings_dir("update-ask-esc", COMMENTED);
    let (wb, cx) = setup_with_config(cx, &dir);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    assert!(wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(dialog(&wb, cx).is_none(), "Esc closes it");
    assert_eq!(std::fs::read_to_string(dir.join("settings.jsonc")).unwrap(), COMMENTED, "the user's file is untouched");
    assert_eq!(wb.read_with(cx, |_, cx| cx.global::<crate::settings::Settings>().update_check), crate::update::Policy::Ask);
    assert_eq!(fake.calls(), Vec::<String>::new());
    // The next starts within the month do not ask…
    fake.advance(DAY * 29);
    assert!(wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    assert!(dialog(&wb, cx).is_none());
    // …and the one after it does.
    fake.advance(DAY * 2);
    assert!(wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    assert_eq!(dialog(&wb, cx).unwrap().title, "Check for updates automatically?");
}

#[gpui::test]
fn the_question_waits_for_a_quiet_moment_and_ignores_enter_just_after_it_appears(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    // The window is in the background, or a picker is open (quick open, the palette…): not now.
    assert!(!wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(false, window, cx)));
    wb.update_in(cx, |wb, window, cx| wb.open_palette_for_test(window, cx));
    assert!(!wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    assert!(dialog(&wb, cx).is_none());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    fake.advance(Duration::from_secs(4)); // the Esc was a key too: see the test for typing below
    assert!(wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    // A key typed into the editor a moment before cannot answer it.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(dialog(&wb, cx).is_some(), "Enter is ignored for the first moments");
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(dialog(&wb, cx).is_none(), "afterwards it presses the primary button");
}

// --- Keyboard ---

#[gpui::test]
fn tab_walks_the_buttons_and_enter_presses_the_focused_one(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    // Focus starts on the primary button; Tab goes to the release notes (before the buttons), then to "Skip This Version".
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert!(notes_focused(&wb, cx) && focused_button(&wb, cx).is_none(), "the notes come first in the tab order");
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(focused_button(&wb, cx).as_deref(), Some("Skip This Version"));
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(focused_button(&wb, cx).as_deref(), Some("Release Page"));
    cx.simulate_keystrokes("shift-tab shift-tab shift-tab");
    cx.run_until_parked();
    assert_eq!(focused_button(&wb, cx).as_deref(), Some("Install and Restart"), "shift-tab wraps backwards over the notes");
    cx.simulate_keystrokes("shift-tab");
    cx.run_until_parked();
    assert_eq!(focused_button(&wb, cx).as_deref(), Some("Later"));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(dialog(&wb, cx).is_none(), "Enter pressed Later");
    assert_eq!(state(&wb, cx), "available 0.2.0", "Later leaves the offer in the status bar");
}

#[gpui::test]
fn enter_installs_and_esc_cancels_a_check(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::UntilCancelled);
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "downloading", |cx| state(&wb, cx) == "downloading 0.2.0");
    press(&wb, cx, "Cancel");
    wait_state(&wb, cx, "available 0.2.0");
    // A check in progress: Esc is Cancel, and its answer is dropped.
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    fake.check_returns(Ok(Some(release("0.2.0"))));
    manual_check(&wb, cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!((state(&wb, cx), dialog(&wb, cx).is_none()), ("idle".to_string(), true));
    cx.executor().advance_clock(Duration::from_secs(1));
    std::thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(state(&wb, cx), "idle", "the late answer of a cancelled check changes nothing");
}

#[gpui::test]
fn focus_returns_to_where_it_was_when_the_dialog_closes(cx: &mut TestAppContext) {
    let dir = project("update-focus");
    let (wb, cx, fake) = rig(cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    cx.simulate_input("a");
    offered(&wb, cx, &fake, "0.2.0");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_input("b");
    let text = wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).text()));
    assert_eq!(text.as_deref(), Some("abfn main() {}\n"), "typing goes to the editor again");
}

// --- Restart and quit ---

#[gpui::test]
fn restart_now_starts_the_new_version_before_it_quits(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Done);
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "ready 0.2.0");
    press(&wb, cx, "Restart Now");
    assert_eq!(fake.calls().iter().rev().take(2).rev().cloned().collect::<Vec<_>>(), ["relaunch", "quit"], "{:?}", fake.calls());
}

#[gpui::test]
fn a_restart_that_cannot_start_the_new_version_does_not_quit(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Done);
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "ready 0.2.0");
    fake.relaunch_fails.store(true, Ordering::SeqCst);
    press(&wb, cx, "Restart Now");
    assert_eq!(fake.count("quit"), 0, "the editor stays");
    let (error, message) = notice(&wb, cx).unwrap();
    assert!(error && message.contains("no /usr/bin/open") && message.contains("open Codeyard again"), "{message}");
}

#[gpui::test]
fn restarting_with_unsaved_work_and_hot_exit_off_asks_first(cx: &mut TestAppContext) {
    let dir = project("update-restart-ask");
    let (wb, cx, fake) = rig(cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().hot_exit = false);
    cx.simulate_input("// unsaved\n");
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Done);
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "ready 0.2.0");
    press(&wb, cx, "Restart Now");
    let (title, detail) = cx.pending_prompt().expect("the unsaved-changes question");
    assert_eq!(title, "You have unsaved changes");
    assert_eq!(detail, "Restarting loses everything since the last save.");
    assert_eq!(fake.count("relaunch") + fake.count("quit"), 0);
    crate::question::cancel_keeps_focus(&wb, cx, "Cancel");
    cx.run_until_parked();
    assert_eq!(fake.count("quit"), 0, "Cancel keeps everything as it was");
    press(&wb, cx, "Restart Now");
    cx.simulate_prompt_answer("Restart without saving");
    cx.run_until_parked();
    assert_eq!(fake.calls().iter().rev().take(2).rev().cloned().collect::<Vec<_>>(), ["relaunch", "quit"]);
}

/// The restart path goes through hot exit: what was typed and not saved is there after the restart.
#[gpui::test]
fn unsaved_edits_survive_the_restart_after_an_update(cx: &mut TestAppContext) {
    let home = fresh("update-hot-exit-home");
    let dir = project("update-hot-exit");
    let (wb, cx) = setup_persistent(cx, &home);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    open(&wb, &dir.join("src/main.rs"), cx);
    cx.simulate_input("// not saved\n");
    cx.executor().advance_clock(Duration::from_secs(2)); // the backup timer
    cx.run_until_parked();
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Done);
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "ready 0.2.0");
    press(&wb, cx, "Restart Now");
    assert_eq!(fake.count("quit"), 1, "no question: hot exit is on");
    // The new process opens the same state.
    let wb2 = crate::qa_workbench_tests::restart_with(wb, cx, &home, crate::settings::Startup::Continue);
    assert_eq!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap(), "fn main() {}\n", "the file on disk is untouched");
    let editor = wb2.read_with(cx, |wb, _| wb.active_editor_entity()).expect("the tab comes back");
    assert_eq!(editor.read_with(cx, |e, _| e.text()), "// not saved\nfn main() {}\n");
    assert!(wb2.read_with(cx, |wb, cx| wb.any_dirty(cx)), "still unsaved");
}

/// Windows has no Ready state: the setup program is already running, so the app leaves at once through the normal quit path.
#[gpui::test]
fn on_windows_the_installer_ends_with_a_restart_not_a_ready_state(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    *fake.kind.lock().unwrap() = InstallKind::WindowsInstalled;
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Done);
    fake.hold_install.store(true, Ordering::SeqCst);
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "installing 0.2.0");
    assert!(dialog(&wb, cx).unwrap().text.contains("Installing — Codeyard will restart."));
    fake.install_returns(Ok(InstallPlan::WindowsInstaller));
    fake.hold_install.store(false, Ordering::SeqCst);
    wait_ticking(cx, "the app quits", |_| fake.count("quit") == 1);
    assert_eq!(fake.calls(), ["check", "download", "install windows", "relaunch", "quit"], "the setup file is kept: it is running");
}

#[gpui::test]
fn quitting_during_a_download_cancels_it_and_waits_for_the_clean_up(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::UntilCancelled);
    press(&wb, cx, "Install and Restart");
    wait_ticking(cx, "downloading", |cx| dialog(&wb, cx).is_some_and(|d| d.text.contains(" of ")));
    let exited = wb.update(cx, |wb, _| wb.cancel_update_for_quit_for_test()).expect("a download was running");
    wait_ticking(cx, "the download thread is done", |_| exited.load(Ordering::SeqCst));
    assert!(fake.cleaned_up.load(Ordering::SeqCst), "it stopped the transfer and removed what it had");
    assert_eq!(fake.count("install mac"), 0, "nothing is installed after a quit");
}

#[gpui::test]
fn quitting_while_the_app_is_being_replaced_asks_first(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Done);
    fake.hold_install.store(true, Ordering::SeqCst);
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "installing 0.2.0");
    cx.simulate_keystrokes("cmd-q");
    cx.run_until_parked();
    let (title, _) = cx.pending_prompt().expect("the question");
    assert_eq!(title, "Codeyard is still installing an update");
    crate::question::cancel_keeps_focus(&wb, cx, "Keep Waiting");
    cx.run_until_parked();
    assert_eq!(fake.count("quit"), 0);
    cx.simulate_keystrokes("cmd-q");
    cx.run_until_parked();
    cx.simulate_prompt_answer("Quit Anyway");
    cx.run_until_parked();
    assert_eq!(fake.count("quit"), 1);
    fake.hold_install.store(false, Ordering::SeqCst);
}

// --- Other windows, language, accessibility ---

#[gpui::test]
fn a_second_window_shows_the_update_that_is_already_running_instead_of_starting_another(cx: &mut TestAppContext) {
    // Real windows have the workbench as their root view (the `setup` helper wraps it), which is what other windows look for.
    crate::qa_workbench_tests::globals(cx);
    let open_window = |cx: &mut TestAppContext| {
        cx.update(|app| {
            app.open_window(gpui::WindowOptions::default(), |window, cx| cx.new(|cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx)))
                .expect("a window")
        })
    };
    let (first, second) = (open_window(cx), open_window(cx));
    let (fake, other) = (Fake::new(), Fake::new());
    let state_of = |cx: &mut TestAppContext| first.read_with(cx, |wb, _| wb.update_state_for_test()).unwrap();
    let wait = |cx: &mut TestAppContext, want: &str| {
        let start = std::time::Instant::now();
        while state_of(cx) != want {
            assert!(start.elapsed() < Duration::from_secs(10), "timeout: {want}, now {}", state_of(cx));
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    first
        .update(cx, |wb, window, cx| {
            window.focus(&wb.focus_handle(cx));
            wb.set_update_backend_for_test(fake.clone());
            fake.check_returns(Ok(Some(release("0.2.0"))));
            wb.run_command_for_test("Help: Check for Updates…", window, cx);
        })
        .unwrap();
    wait(cx, "available 0.2.0");
    fake.download_does(Download::UntilCancelled);
    first.update(cx, |wb, window, cx| wb.press_update_button_for_test("Install and Restart", window, cx)).unwrap();
    wait(cx, "downloading 0.2.0");
    first.update(cx, |wb, window, cx| wb.press_update_button_for_test("Hide", window, cx)).unwrap();
    // The second window asks for a check: nothing starts there, the first window shows what is going on.
    second
        .update(cx, |wb, window, cx| {
            window.focus(&wb.focus_handle(cx));
            wb.set_update_backend_for_test(other.clone());
            wb.run_command_for_test("Help: Check for Updates…", window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(other.calls(), Vec::<String>::new());
    assert_eq!(first.read_with(cx, |wb, cx| wb.update_dialog_for_test(cx)).unwrap().map(|d| d.title).as_deref(), Some("Downloading Codeyard 0.2.0"));
    assert_eq!(second.read_with(cx, |wb, cx| wb.update_dialog_for_test(cx)).unwrap(), None, "and the second window has no dialog of its own");
    first.update(cx, |wb, window, cx| wb.press_update_button_for_test("Cancel", window, cx)).unwrap();
    wait(cx, "available 0.2.0");
}

#[gpui::test]
fn the_dialog_and_the_status_item_follow_the_interface_language(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    let _tr = crate::i18n::LangGuard::set("tr");
    offered(&wb, cx, &fake, "0.2.0");
    let d = dialog(&wb, cx).unwrap();
    assert_ne!(d.title, "Codeyard 0.2.0 is available");
    assert!(d.title.contains("0.2.0"), "{}", d.title);
    assert!(d.buttons.iter().all(|b| !["Skip This Version", "Release Page", "Later", "Install and Restart"].contains(&b.as_str())), "{:?}", d.buttons);
    let (_, tip) = wb.read_with(cx, |wb, _| wb.update_status_for_test()).unwrap();
    assert!(!tip.starts_with("Codeyard 0.2.0 is available"), "{tip}");
}

#[gpui::test]
fn the_accessibility_tree_has_a_modal_dialog_with_progress_and_buttons(cx: &mut TestAppContext) {
    use accesskit::Role;
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::UntilCancelled);
    press(&wb, cx, "Install and Restart");
    wait_ticking(cx, "downloading", |cx| dialog(&wb, cx).is_some_and(|d| d.text.contains(" of ")));
    let snapshot = cx.update(|window, cx| wb.read(cx).a11y_snapshot(window, cx));
    let tree = accesskit_consumer::Tree::new(snapshot.to_tree(), true);
    let state = tree.state();
    let dialog = state.root().children().find(|n| n.role() == Role::Dialog).expect("a dialog node");
    assert_eq!(dialog.label().as_deref(), Some("Downloading Codeyard 0.2.0"));
    assert!(dialog.is_modal());
    let kids: Vec<_> = dialog.children().collect();
    let bar = kids.iter().find(|n| n.role() == Role::ProgressIndicator).expect("progress");
    assert!(bar.numeric_value().is_some_and(|v| (0. ..=100.).contains(&v)) && bar.max_numeric_value() == Some(100.));
    let labels: Vec<_> = kids.iter().filter(|n| n.role() == Role::Button).filter_map(|n| n.label()).collect();
    assert_eq!(labels, ["Cancel", "Hide"]);
    assert_eq!(state.focus().map(|n| n.role()), Some(Role::Button), "the focused button is the focus of the tree");
    // The status bar names the update too.
    assert!(snapshot.status.iter().any(|s| s == "Downloading Codeyard 0.2.0. Click for details."), "{:?}", snapshot.status);
    press(&wb, cx, "Cancel");
}

// --- Replays, retries and what a failure says ---

#[gpui::test]
fn a_list_older_than_one_seen_before_is_not_offered_but_equal_and_newer_ones_are(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.4.0");
    assert_eq!(wb.read_with(cx, |wb, _| wb.state_value_for_test("update.highest_seen")).as_deref(), Some("0.4.0"));
    // Somebody plays an old, genuinely signed list back (the real latest is 0.4.0): not an upgrade, whoever asks.
    press(&wb, cx, "Later");
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    fake.check_returns(Ok(Some(release("0.2.0"))));
    manual_check(&wb, cx);
    wait_state(&wb, cx, "up-to-date");
    assert_eq!(wb.read_with(cx, |wb, _| wb.state_value_for_test("update.highest_seen")).as_deref(), Some("0.4.0"), "never lowered");
    press(&wb, cx, "Close");
    fake.check_returns(Ok(Some(release("0.3.0"))));
    wb.update_in(cx, |wb, window, cx| wb.background_check_for_test(window, cx));
    wait_ticking(cx, "the background check ran", |_| fake.count("check") == 3);
    wait_state(&wb, cx, "idle");
    assert_eq!(notice(&wb, cx).filter(|(_, m)| m.contains("is available")), None);
    // The same version again, and a newer one, are fine.
    fake.check_returns(Ok(Some(release("0.4.0"))));
    manual_check(&wb, cx);
    wait_state(&wb, cx, "available 0.4.0");
    press(&wb, cx, "Later");
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    fake.check_returns(Ok(Some(release("0.5.0"))));
    manual_check(&wb, cx);
    wait_state(&wb, cx, "available 0.5.0");
    assert_eq!(wb.read_with(cx, |wb, _| wb.state_value_for_test("update.highest_seen")).as_deref(), Some("0.5.0"));
}

#[gpui::test]
fn after_failed_checks_the_daily_check_is_retried_three_times_a_day_at_most(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    set_policy(cx, crate::update::Policy::Daily);
    let hours = |h: u64| Duration::from_secs(h * 3600);
    let look = |cx: &mut VisualTestContext, expected_checks: usize| {
        fake.check_returns(Err(network()));
        wb.update_in(cx, |wb, window, cx| wb.check_if_due_for_test(window, cx));
        if fake.count("check") < expected_checks {
            wait_ticking(cx, "a check", |_| fake.count("check") == expected_checks);
        }
        std::thread::sleep(Duration::from_millis(60)); // a check that must not happen has no event to wait for
        cx.run_until_parked();
        assert_eq!(fake.count("check"), expected_checks, "after {:?}", fake.now());
    };
    // The app looks every three hours while it runs.
    for (expected, hour) in [(1, 0), (2, 3), (3, 6), (3, 9), (3, 12), (3, 21)] {
        fake.advance(hours(hour) - hours(fake_hours(&fake)));
        look(cx, expected);
    }
    // 24 hours after the first failure the day is over.
    fake.advance(hours(24 - 21));
    look(cx, 4);
}

/// Hours between the epoch the fake clock starts at and now, so that the test above can step to absolute hours.
fn fake_hours(fake: &Fake) -> u64 {
    (fake.now.load(Ordering::SeqCst) - 1_800_000_000) / 3600
}

#[gpui::test]
fn a_copy_that_cannot_update_itself_says_why(cx: &mut TestAppContext) {
    let reasons = [
        (ManualReason::DevelopmentBuild, "development build"),
        (ManualReason::NotInstalled, "isn't installed as an app"),
        (ManualReason::ReadOnlyFolder(PathBuf::from("/Applications")), "can't replace itself in /Applications: your account can't write there"),
        (ManualReason::Portable, "portable version"),
        (ManualReason::OtherSystem, "can't update itself on this system"),
    ];
    for (reason, text) in reasons {
        let (wb, cx, fake) = rig(cx);
        *fake.kind.lock().unwrap() = InstallKind::Manual(reason.clone());
        offered(&wb, cx, &fake, "0.2.0");
        let d = dialog(&wb, cx).unwrap();
        assert!(d.text.contains(text), "{reason:?}: {}", d.text);
        assert_eq!(d.primary.as_deref(), Some("Download Page"));
        let others = d.text.matches("Download the new version").count() + d.text.matches("download the new version").count();
        assert_eq!(others, 1, "one sentence, the right one: {}", d.text);
    }
}

#[gpui::test]
fn every_kind_of_failure_says_what_to_do_about_it(cx: &mut TestAppContext) {
    // (the error of a manual check, title, words in the lead, buttons)
    type Case = (UpdateError, &'static str, &'static str, &'static [&'static str]);
    let cases: [Case; 4] = [
        (UpdateError::Unsigned, "The update couldn't be verified", "isn't signed, so Codeyard can't verify it", &["Close", "Release Page"]),
        (UpdateError::NoRelease, "Couldn't check for updates", "no release with a checksum list", &["Close", "Release Page"]),
        (
            UpdateError::Tls("curl: (60) SSL certificate problem".into()),
            "Couldn't check for updates",
            "date and time are right",
            &["Release Page", "Close", "Try Again"],
        ),
        (
            UpdateError::Http { file: "SHA256SUMS.txt".into(), status: 503 },
            "Couldn't check for updates",
            "couldn't reach the update server",
            &["Release Page", "Close", "Try Again"],
        ),
    ];
    for (error, title, lead, buttons) in cases {
        let (wb, cx, fake) = rig(cx);
        fake.check_returns(Err(error));
        manual_check(&wb, cx);
        wait_ticking(cx, "an error", |cx| state(&wb, cx).starts_with("error"));
        let d = dialog(&wb, cx).unwrap();
        assert_eq!((d.title.as_str(), d.buttons.as_slice()), (title, &buttons.iter().map(|b| b.to_string()).collect::<Vec<_>>()[..]), "{}", d.text);
        assert!(d.text.contains(lead), "{}", d.text);
    }
}

#[gpui::test]
fn a_full_disk_can_be_retried_and_a_failed_rollback_says_where_the_old_app_is(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    fake.download_does(Download::Fail(UpdateError::NoSpace));
    press(&wb, cx, "Install and Restart");
    wait_state(&wb, cx, "error install");
    let d = dialog(&wb, cx).unwrap();
    assert!(d.text.contains("isn't enough free disk space") && d.buttons.contains(&"Try Again".to_string()), "{}", d.text);
    fake.download_does(Download::Done);
    fake.install_returns(Err(UpdateError::RollbackFailed {
        app: PathBuf::from(APP),
        backup: PathBuf::from("/Applications/.Codeyard.old-1.app"),
        cause: io::Error::other("busy"),
    }));
    press(&wb, cx, "Try Again");
    wait_state(&wb, cx, "error install");
    let d = dialog(&wb, cx).unwrap();
    assert!(d.text.contains("may be at /Applications/.Codeyard.old-1.app") && d.text.contains("move it back to /Applications/Codeyard.app"), "{}", d.text);
    assert!(!d.text.contains("download it from the release page"), "the app is not as it was: {}", d.text);
}

// --- The question and typing ---

#[gpui::test]
fn the_question_waits_until_nobody_is_typing(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    cx.simulate_keystrokes("a");
    cx.run_until_parked();
    assert!(!wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)), "a key was just pressed");
    assert!(dialog(&wb, cx).is_none());
    fake.advance(Duration::from_secs(2));
    assert!(!wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    cx.simulate_keystrokes("b");
    fake.advance(Duration::from_secs(2));
    assert!(!wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)), "the clock starts again at every key");
    fake.advance(Duration::from_secs(2));
    assert!(wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    assert!(dialog(&wb, cx).is_some());
}

// --- Release notes ---

fn release_with_notes(notes: String) -> Release {
    Release { notes, ..release("0.2.0") }
}

fn offered_notes(wb: &Entity<Workbench>, cx: &mut VisualTestContext, fake: &Fake, notes: String) {
    fake.check_returns(Ok(Some(release_with_notes(notes))));
    manual_check(wb, cx);
    wait_state(wb, cx, "available 0.2.0");
}

fn dialog_node_description(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    use accesskit::Role;
    let snapshot = cx.update(|window, cx| wb.read(cx).a11y_snapshot(window, cx));
    let tree = accesskit_consumer::Tree::new(snapshot.to_tree(), true);
    let state = tree.state();
    let dialog = state.root().children().find(|n| n.role() == Role::Dialog).expect("a dialog node");
    dialog.description().unwrap_or_default()
}

#[gpui::test]
fn screen_readers_get_the_release_notes_with_a_pointer_to_the_rest(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    offered(&wb, cx, &fake, "0.2.0");
    let spoken = dialog_node_description(&wb, cx);
    assert!(spoken.contains("What is new") && spoken.contains("Faster start") && spoken.contains("A link"), "{spoken}");
    assert!(!spoken.contains("**") && !spoken.contains("]("), "text, not Markdown: {spoken}");
    assert!(!spoken.contains("read the rest"), "short notes are given whole");
    // Long notes: the first 8 KB and where to read on.
    let long: String = (0..2000).map(|i| format!("- Change number {i} of the release\n")).collect();
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    offered_notes(&wb, cx, &fake, long);
    let spoken = dialog_node_description(&wb, cx);
    assert!(spoken.contains("Change number 0 of"), "{}", &spoken[..80.min(spoken.len())]);
    assert!(!spoken.contains("Change number 1999 of"));
    assert!(spoken.len() < 8 * 1024 + 200, "{} bytes", spoken.len());
    let page = format!("{}/releases/tag/v0.2.0", env!("CARGO_PKG_REPOSITORY"));
    assert!(spoken.ends_with(&format!("The release notes are longer: read the rest at {page}")), "{}", &spoken[spoken.len() - 120..]);
}

#[gpui::test]
fn huge_release_notes_are_drawn_only_up_to_a_limit_and_say_so(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    // 256 KiB, the most the updater accepts, in the shape that makes the most blocks.
    let huge = "x\n\n".repeat(256 * 1024 / 3);
    offered_notes(&wb, cx, &fake, huge);
    let (blocks, last) = wb.read_with(cx, |wb, _| wb.update_notes_for_test());
    assert!(blocks <= 64 * 1024 / 3 + 2, "{blocks} blocks");
    assert_eq!(last, "The release notes are longer: read the rest on GitHub.");
    // A note just under the limit is shown whole, without the line.
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    offered_notes(&wb, cx, &fake, "y\n\n".repeat(64 * 1024 / 3 - 1));
    let (blocks, last) = wb.read_with(cx, |wb, _| wb.update_notes_for_test());
    assert_eq!((blocks, last.as_str()), (64 * 1024 / 3 - 1, "y"));
}

#[gpui::test]
fn the_notes_can_be_read_with_the_keyboard(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    let notes: String = (0..80).map(|i| format!("Paragraph {i} of the notes.\n\n")).collect();
    offered_notes(&wb, cx, &fake, notes);
    cx.run_until_parked();
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert!(notes_focused(&wb, cx));
    let scroll = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.notes_scroll_for_test());
    let (top, max) = scroll(&wb, cx);
    assert_eq!(top, 0.);
    assert!(max > 100., "there is something to scroll: {max}");
    for (keys, check) in [
        ("down", Box::new(|y: f32, _: f32| y < 0.) as Box<dyn Fn(f32, f32) -> bool>),
        ("pagedown", Box::new(|y, _| y < -100.)),
        ("end", Box::new(|y, max| y == -max)),
        ("pageup", Box::new(|y, max| y > -max)),
        ("home", Box::new(|y, _| y == 0.)),
        ("up", Box::new(|y, _| y == 0.)),
    ] {
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        let (y, max) = scroll(&wb, cx);
        assert!(check(y, max), "{keys}: offset {y} of {max}");
    }
    // Other keys are left alone, and the dialog is still there.
    cx.simulate_keystrokes("x");
    assert!(dialog(&wb, cx).is_some());
}

// --- More than one window ---

fn two_windows(cx: &mut TestAppContext) -> (gpui::WindowHandle<Workbench>, gpui::WindowHandle<Workbench>, Arc<Fake>) {
    crate::qa_workbench_tests::globals(cx);
    let open_window = |cx: &mut TestAppContext| {
        cx.update(|app| {
            app.open_window(gpui::WindowOptions::default(), |window, cx| cx.new(|cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx)))
                .expect("a window")
        })
    };
    let (first, second) = (open_window(cx), open_window(cx));
    let fake = Fake::new();
    for w in [first, second] {
        w.update(cx, |wb, window, cx| {
            window.focus(&wb.focus_handle(cx));
            wb.set_update_backend_for_test(fake.clone());
        })
        .unwrap();
    }
    (first, second, fake)
}

/// The first window is in the middle of replacing the app.
fn installing_in_first(cx: &mut TestAppContext, first: gpui::WindowHandle<Workbench>, fake: &Fake) {
    first
        .update(cx, |wb, window, cx| {
            fake.check_returns(Ok(Some(release("0.2.0"))));
            wb.run_command_for_test("Help: Check for Updates…", window, cx);
        })
        .unwrap();
    let wait = |cx: &mut TestAppContext, want: &str| {
        let start = std::time::Instant::now();
        while first.read_with(cx, |wb, _| wb.update_state_for_test()).unwrap() != want {
            assert!(start.elapsed() < Duration::from_secs(10), "timeout: {want}");
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait(cx, "available 0.2.0");
    fake.download_does(Download::Done);
    fake.hold_install.store(true, Ordering::SeqCst);
    first.update(cx, |wb, window, cx| wb.press_update_button_for_test("Install and Restart", window, cx)).unwrap();
    wait(cx, "installing 0.2.0");
}

#[gpui::test]
fn quitting_asks_in_every_window_while_any_window_is_replacing_the_app(cx: &mut TestAppContext) {
    let (first, second, fake) = two_windows(cx);
    installing_in_first(cx, first, &fake);
    // Cmd-Q in the OTHER window.
    second.update(cx, |wb, window, cx| wb.confirm_quit(window, cx)).unwrap();
    cx.run_until_parked();
    let (title, _) = cx.pending_prompt().expect("the question, although this window is not the one installing");
    assert_eq!(title, "Codeyard is still installing an update");
    cx.simulate_prompt_answer("Keep Waiting");
    cx.run_until_parked();
    assert_eq!(fake.count("quit"), 0);
    second.update(cx, |wb, window, cx| wb.confirm_quit(window, cx)).unwrap();
    cx.run_until_parked();
    cx.simulate_prompt_answer("Quit Anyway");
    cx.run_until_parked();
    assert_eq!(fake.count("quit"), 1);
    fake.hold_install.store(false, Ordering::SeqCst);
}

#[gpui::test]
fn the_window_that_is_installing_cannot_be_closed_and_the_others_can(cx: &mut TestAppContext) {
    let (first, second, fake) = two_windows(cx);
    installing_in_first(cx, first, &fake);
    let held = second.update(cx, |wb, window, cx| wb.hold_close_for_install(window, cx)).unwrap();
    assert!(!held, "a window with nothing to do with the install closes as usual");
    let held = first.update(cx, |wb, window, cx| wb.hold_close_for_install(window, cx)).unwrap();
    assert!(held, "this window will receive the result: it stays");
    assert!(!cx.has_pending_prompt(), "not a quit: no question, a notice");
    let notice = first.read_with(cx, |wb, _| wb.notice_for_test()).unwrap();
    assert_eq!(notice, Some((false, "This window is installing an update. You can close it in a few seconds.".into())));
    // It finishes, the version is recorded and the window may close.
    fake.hold_install.store(false, Ordering::SeqCst);
    let start = std::time::Instant::now();
    while first.read_with(cx, |wb, _| wb.update_state_for_test()).unwrap() != "ready 0.2.0" {
        assert!(start.elapsed() < Duration::from_secs(10), "timeout");
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(first.read_with(cx, |wb, _| wb.state_value_for_test("update.installed")).unwrap().as_deref(), Some("0.2.0"));
    assert!(!first.update(cx, |wb, window, cx| wb.hold_close_for_install(window, cx)).unwrap());
}

#[gpui::test]
fn the_reasons_failures_and_notes_line_are_turkish_too(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    let _tr = crate::i18n::LangGuard::set("tr");
    *fake.kind.lock().unwrap() = InstallKind::Manual(ManualReason::ReadOnlyFolder(PathBuf::from("/Applications")));
    fake.check_returns(Ok(Some(release_with_notes("x\n\n".repeat(40_000)))));
    manual_check(&wb, cx);
    wait_state(&wb, cx, "available 0.2.0");
    let d = dialog(&wb, cx).unwrap();
    assert!(d.text.contains("/Applications içinde kendini değiştiremez"), "{}", d.text);
    assert_eq!(wb.read_with(cx, |wb, _| wb.update_status_for_test()).unwrap().0, "Güncelleme 0.2.0");
    let (_, last) = wb.read_with(cx, |wb, _| wb.update_notes_for_test());
    assert_eq!(last, "Sürüm notları daha uzun: devamını GitHub'da okuyun.");
    press(&wb, cx, "Sonra");
    wb.update(cx, |wb, _| wb.reset_update_for_test());
    for (error, words) in [
        (UpdateError::Unsigned, "imzalı olmadığı için"),
        (UpdateError::NoRelease, "sağlama toplamı listesi olan bir sürüm yok"),
        (UpdateError::Tls("x".into()), "ve saatinizin doğru"),
        (UpdateError::NoSpace, "boş disk alanı"),
    ] {
        fake.check_returns(Err(error));
        manual_check(&wb, cx);
        wait_ticking(cx, "an error", |cx| state(&wb, cx).starts_with("error"));
        let d = dialog(&wb, cx).unwrap();
        assert!(d.text.contains(words), "{words}: {}", d.text);
        press(&wb, cx, "Kapat");
    }
}

// --- The windows come back after the restart ---

use crate::update_restart::{self, Folder, Handover};

fn ready(wb: &Entity<Workbench>, cx: &mut VisualTestContext, fake: &Fake) {
    offered(wb, cx, fake, "0.2.0");
    fake.download_does(Download::Done);
    press(wb, cx, "Install and Restart");
    wait_state(wb, cx, "ready 0.2.0");
}

/// The window the old process had, as the new one finds it: a fresh workbench on the same state folder.
fn new_process(cx: &mut VisualTestContext, home: &std::path::Path) -> Entity<Workbench> {
    let (db, data) = (home.join("state.db"), home.to_path_buf());
    let wb = cx.update(|window, cx| {
        window.replace_root(cx, |window, cx| {
            let mut w = Workbench::with_state(state::State::open(&db).ok(), None, window, cx);
            w.set_data_dir(data);
            w
        })
    });
    wb.update_in(cx, |w, window, cx| window.focus(&w.focus_handle(cx)));
    wb
}

/// What `update_restart::reopen_windows` does for the window the process started with (the other windows are in the two-window tests).
fn land(wb: &Entity<Workbench>, cx: &mut VisualTestContext, h: &Handover) {
    let Folder { host, path } = h.windows[0].clone();
    wb.update_in(cx, |wb, window, cx| wb.reopen(host, path, window, cx));
    cx.run_until_parked();
}

fn fake_now(fake: &Fake) -> SystemTime {
    fake.now()
}

#[gpui::test]
fn restarting_leaves_the_open_folder_for_the_new_version(cx: &mut TestAppContext) {
    let home = fresh("restart-handover-home");
    let dir = std::fs::canonicalize(project("restart-handover")).unwrap();
    let (wb, cx) = setup_persistent(cx, &home);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    open(&wb, &dir, cx);
    ready(&wb, cx, &fake);
    assert!(!home.join("update-restart.json").exists(), "nothing is left before the restart");
    press(&wb, cx, "Restart Now");
    assert_eq!(fake.count("quit"), 1);
    let h = update_restart::take(&home, "0.2.0", fake_now(&fake)).unwrap().expect("the file the new version reads");
    assert_eq!(h, Handover { to: "0.2.0".into(), windows: vec![Folder { host: None, path: ui::local_path(&dir) }], focused: 0 });
}

#[gpui::test]
fn a_restart_with_no_folder_open_leaves_nothing(cx: &mut TestAppContext) {
    let home = fresh("restart-handover-none");
    let (wb, cx) = setup_persistent(cx, &home);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    ready(&wb, cx, &fake);
    press(&wb, cx, "Restart Now");
    assert_eq!(fake.count("quit"), 1);
    assert!(!home.join("update-restart.json").exists());
}

#[gpui::test]
fn a_restart_that_cannot_start_the_new_version_takes_the_file_back(cx: &mut TestAppContext) {
    let home = fresh("restart-handover-fails");
    let dir = project("restart-handover-fails-dir");
    let (wb, cx) = setup_persistent(cx, &home);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    open(&wb, &dir, cx);
    ready(&wb, cx, &fake);
    fake.relaunch_fails.store(true, Ordering::SeqCst);
    press(&wb, cx, "Restart Now");
    assert_eq!(fake.count("quit"), 0);
    assert!(!home.join("update-restart.json").exists(), "a later start must not pick up a restart that did not happen");
}

/// The whole path: unsaved edits and the tab are saved by the restart, the handover names the folder, and the new process, which
/// would show the start page on its own (`window.startup` is `welcome`), lands in the folder with the edit still unsaved.
#[gpui::test]
fn after_the_restart_the_folder_and_its_unsaved_edit_are_back_though_startup_is_welcome(cx: &mut TestAppContext) {
    let home = fresh("restart-land-home");
    let dir = std::fs::canonicalize(project("restart-land")).unwrap();
    let (wb, cx) = setup_persistent(cx, &home);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    cx.simulate_input("// not saved\n");
    cx.executor().advance_clock(Duration::from_secs(2)); // the backup timer
    cx.run_until_parked();
    ready(&wb, cx, &fake);
    press(&wb, cx, "Restart Now");
    assert_eq!(fake.count("quit"), 1, "no question: hot exit is on");
    let handover = update_restart::take(&home, "0.2.0", fake_now(&fake)).unwrap();

    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().startup = crate::settings::Startup::Welcome);
    let first = new_process(cx, &home);
    // Without the handover this start is the old behaviour: the start page.
    first.update_in(cx, |wb, window, cx| wb.restore_last(window, cx));
    cx.run_until_parked();
    assert_eq!(first.read_with(cx, |wb, cx| wb.root_for_test(cx)), None);

    land(&first, cx, &handover.expect("handover"));
    assert_eq!(first.read_with(cx, |wb, cx| wb.root_for_test(cx)), Some(ui::local_path(&dir)));
    let text = first.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).text()));
    assert_eq!(text.as_deref(), Some("// not saved\nfn main() {}\n"));
    assert!(first.read_with(cx, |wb, cx| wb.any_dirty(cx)), "still unsaved");
    assert_eq!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap(), "fn main() {}\n");
}

/// What comes back inside the window follows the user's settings, and the dialog says no more than that. One test window per case:
/// a restart prepares every window of the app, so the windows of an earlier case would take part.
fn session_case(cx: &mut TestAppContext, restore: bool, hot: bool, tab: bool, edit: bool) {
    let n = format!("restore={restore} hotExit={hot}");
    let home = fresh(&format!("restart-settings-home-{restore}-{hot}"));
    let dir = std::fs::canonicalize(project(&format!("restart-settings-{restore}-{hot}"))).unwrap();
    let (wb, cx) = setup_persistent(cx, &home);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    // With hot exit off, an unsaved edit would make the restart ask first: nothing is typed then.
    if hot {
        cx.simulate_input("// edit\n");
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
    }
    cx.update(|_, cx| {
        let s = cx.global_mut::<crate::settings::Settings>();
        (s.restore_session, s.hot_exit) = (restore, hot);
    });
    ready(&wb, cx, &fake);
    let said = dialog(&wb, cx).unwrap().text;
    press(&wb, cx, "Restart Now");
    assert_eq!(fake.count("quit"), 1, "{n}");
    let h = update_restart::take(&home, "0.2.0", fake_now(&fake)).unwrap().unwrap();
    let first = new_process(cx, &home);
    land(&first, cx, &h);
    assert_eq!(first.read_with(cx, |wb, cx| wb.root_for_test(cx)), Some(ui::local_path(&dir)), "{n}: the folder always comes back");
    let tabs = first.read_with(cx, |wb, cx| wb.open_paths(cx));
    let edit_back = first.read_with(cx, |wb, cx| wb.any_dirty(cx));
    assert_eq!((!tabs.is_empty(), edit_back), (tab, edit), "{n}: {tabs:?}");
    // The dialog promised exactly that.
    assert!(said.contains("Codeyard will reopen your windows."), "{n}: {said}");
    assert_eq!(said.contains("Your open files and unsaved changes come back"), edit, "{n}: {said}");
    assert_eq!(said.contains("Your open files come back"), tab && !edit, "{n}: {said}");
}

#[gpui::test]
fn after_a_restart_tabs_and_unsaved_edits_come_back_and_the_dialog_says_so(cx: &mut TestAppContext) {
    session_case(cx, true, true, true, true);
}

#[gpui::test]
fn with_hot_exit_off_the_tabs_come_back_and_the_dialog_does_not_mention_unsaved_edits(cx: &mut TestAppContext) {
    session_case(cx, true, false, true, false);
}

#[gpui::test]
fn without_session_restore_only_the_folder_comes_back_and_the_dialog_says_only_that(cx: &mut TestAppContext) {
    session_case(cx, false, true, false, false);
}

#[gpui::test]
fn without_session_restore_and_hot_exit_only_the_folder_comes_back(cx: &mut TestAppContext) {
    session_case(cx, false, false, false, false);
}

#[gpui::test]
fn the_ready_dialog_promises_nothing_when_there_is_nothing_to_reopen(cx: &mut TestAppContext) {
    let home = fresh("restart-dialog-none");
    let (wb, cx) = setup_persistent(cx, &home);
    let fake = Fake::new();
    wb.update(cx, |wb, _| wb.set_update_backend_for_test(fake.clone()));
    ready(&wb, cx, &fake);
    let said = dialog(&wb, cx).unwrap().text;
    assert!(said.contains("Restart Codeyard to finish.") && !said.contains("come back") && !said.contains("reopen"), "{said}");
}

#[gpui::test]
fn the_ready_dialog_promises_nothing_when_no_hand_over_can_be_written(cx: &mut TestAppContext) {
    let dir = project("restart-dialog-nodata");
    let (wb, cx, fake) = rig(cx); // an in-memory state: no state folder to leave the file in
    open(&wb, &dir, cx);
    ready(&wb, cx, &fake);
    let said = dialog(&wb, cx).unwrap().text;
    assert!(!said.contains("reopen") && !said.contains("come back"), "{said}");
}

fn open_test_window(cx: &mut TestAppContext) -> gpui::WindowHandle<Workbench> {
    cx.update(|app| {
        app.open_window(gpui::WindowOptions::default(), |window, cx| cx.new(|cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx)))
            .expect("a window")
    })
}

#[gpui::test]
fn every_window_with_a_folder_is_left_for_the_new_version_and_the_one_in_front_is_marked(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let home = fresh("restart-two-home");
    let canon = |name: &str| ui::local_path(&std::fs::canonicalize(project(name)).unwrap());
    let (a, b, c) = (canon("restart-two-a"), canon("restart-two-b"), canon("restart-two-c"));
    let (first, second, third, _no_folder) = (open_test_window(cx), open_test_window(cx), open_test_window(cx), open_test_window(cx));
    let fake = Fake::new();
    for (w, dir) in [(first, &a), (second, &b), (third, &c)] {
        w.update(cx, |wb, window, cx| wb.open_path(dir, window, cx)).unwrap();
    }
    cx.run_until_parked();
    first
        .update(cx, |wb, window, cx| {
            wb.set_data_dir(home.clone());
            wb.set_update_backend_for_test(fake.clone());
            window.focus(&wb.focus_handle(cx));
            fake.check_returns(Ok(Some(release("0.2.0"))));
            wb.run_command_for_test("Help: Check for Updates…", window, cx);
        })
        .unwrap();
    let state_of = |cx: &mut TestAppContext| first.read_with(cx, |wb, _| wb.update_state_for_test()).unwrap();
    let wait = |cx: &mut TestAppContext, want: &str| {
        let start = std::time::Instant::now();
        while state_of(cx) != want {
            assert!(start.elapsed() < Duration::from_secs(10), "timeout: {want}, now {}", state_of(cx));
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait(cx, "available 0.2.0");
    fake.download_does(Download::Done);
    first.update(cx, |wb, window, cx| wb.press_update_button_for_test("Install and Restart", window, cx)).unwrap();
    wait(cx, "ready 0.2.0");
    // The user is in the second window when they press the button in the first one's dialog.
    second.update(cx, |_, window, _| window.activate_window()).unwrap();
    let said = first.read_with(cx, |wb, cx| wb.update_dialog_for_test(cx)).unwrap().unwrap().text;
    assert!(said.contains("Codeyard will reopen your windows."), "{said}");
    first.update(cx, |wb, window, cx| wb.press_update_button_for_test("Restart Now", window, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(fake.count("quit"), 1);
    let h = update_restart::take(&home, "0.2.0", fake_now(&fake)).unwrap().unwrap();
    let folders: Vec<_> = h.windows.iter().map(|f| f.path.clone()).collect();
    assert_eq!(folders, [a.clone(), b.clone(), c.clone()], "all three folders in window order; the empty window is not there");
    assert_eq!(h.focused, 1, "the window that was in front");

    // The new process: one window to start with, the others made for it.
    let fresh_first = open_test_window(cx);
    let mut made = Vec::new();
    cx.update(|app| {
        update_restart::reopen_windows(
            &h,
            fresh_first,
            &mut |app| {
                let w = app
                    .open_window(gpui::WindowOptions::default(), |window, cx| {
                        cx.new(|cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx))
                    })
                    .ok();
                made.extend(w);
                w
            },
            app,
        )
    });
    cx.run_until_parked();
    let roots: Vec<_> = std::iter::once(fresh_first).chain(made.iter().copied()).map(|w| w.read_with(cx, |wb, cx| wb.root_for_test(cx)).unwrap()).collect();
    assert_eq!(roots, [Some(a), Some(b), Some(c)]);
}

#[gpui::test]
fn a_folder_that_is_gone_is_said_so_and_the_other_windows_still_open(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let ok = std::fs::canonicalize(project("restart-gone-ok")).unwrap().to_string_lossy().into_owned();
    let h = Handover {
        to: "0.2.0".into(),
        windows: vec![Folder { host: None, path: "/no/such/folder-codeyard".into() }, Folder { host: None, path: ok.clone() }],
        focused: 1,
    };
    let first = open_test_window(cx);
    let mut made = Vec::new();
    cx.update(|app| {
        update_restart::reopen_windows(
            &h,
            first,
            &mut |app| {
                let w = app
                    .open_window(gpui::WindowOptions::default(), |window, cx| {
                        cx.new(|cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx))
                    })
                    .ok();
                made.extend(w);
                w
            },
            app,
        )
    });
    cx.run_until_parked();
    let (error, message) = first.read_with(cx, |wb, _| wb.notice_for_test()).unwrap().expect("a notice");
    assert!(!error && message.contains("no longer exists"), "{message}");
    // `canonicalize` gives `\\?\C:\…` on Windows; the workbench keeps the plain `C:/…` form of the same folder.
    let plain = |p: &str| p.trim_start_matches(r"\\?\").replace('\\', "/");
    let root = made[0].read_with(cx, |wb, cx| wb.root_for_test(cx)).unwrap().unwrap();
    assert_eq!(plain(&root), plain(&ok));
}

// --- The dialog in a window of its own (macOS) ---

fn windows_of(cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| cx.windows().len())
}

fn update_window(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<gpui::AnyWindowHandle> {
    wb.read_with(cx, |wb, cx| wb.update_window_for_test(cx))
}

/// Lets the render that opens the window (and the window's own first frame) happen.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
    }
    cx.run_until_parked();
}

#[gpui::test]
fn the_dialog_opens_in_a_window_of_its_own_once_and_closes_with_the_dialog(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    wb.update(cx, |wb, _| wb.use_update_window_for_test());
    assert_eq!(windows_of(cx), 1);
    fake.check_returns(Ok(None));
    manual_check(&wb, cx);
    settle(cx);
    assert_eq!(windows_of(cx), 2, "the editor window and the dialog's");
    let first = update_window(&wb, cx).expect("the dialog's window");
    // Every state change draws the editor again: still the one window.
    wait_state(&wb, cx, "up-to-date");
    settle(cx);
    assert_eq!(windows_of(cx), 2);
    assert!(update_window(&wb, cx) == Some(first), "the same window");
    press(&wb, cx, "Close");
    settle(cx);
    assert_eq!((windows_of(cx), update_window(&wb, cx).is_none()), (1, true), "closing the dialog closes its window");
}

#[gpui::test]
fn the_overlay_stays_where_there_is_no_window_for_the_dialog(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    fake.check_returns(Ok(None));
    manual_check(&wb, cx);
    wait_state(&wb, cx, "up-to-date");
    settle(cx);
    assert_eq!((windows_of(cx), update_window(&wb, cx).is_none()), (1, true));
    assert!(dialog(&wb, cx).is_some());
}

#[gpui::test]
fn esc_in_the_dialogs_window_does_what_esc_does_in_the_card(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    wb.update(cx, |wb, _| wb.use_update_window_for_test());
    offered(&wb, cx, &fake, "0.2.0");
    settle(cx);
    assert!(update_window(&wb, cx).is_some());
    wb.update_in(cx, |wb, window, cx| wb.update_escape_for_test(window, cx));
    settle(cx);
    assert!(dialog(&wb, cx).is_none(), "Esc is Later");
    assert!(update_window(&wb, cx).is_none());
    assert_eq!(state(&wb, cx), "available 0.2.0", "the offer stays in the status bar");
}

#[gpui::test]
fn the_keyboard_starts_on_the_primary_button_of_the_dialogs_window(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    wb.update(cx, |wb, _| wb.use_update_window_for_test());
    offered(&wb, cx, &fake, "0.2.0");
    settle(cx);
    let own = update_window(&wb, cx).expect("the dialog's window");
    let focused = cx.update(|_, cx| own.update(cx, |_, window, cx| wb.read(cx).focused_update_button_for_test(window, cx)).unwrap());
    assert_eq!(focused.as_deref(), Some("Install and Restart"));
}

#[gpui::test]
fn the_dialogs_window_does_not_outlive_its_editor_window(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    wb.update(cx, |wb, _| wb.use_update_window_for_test());
    offered(&wb, cx, &fake, "0.2.0");
    settle(cx);
    assert_eq!(windows_of(cx), 2);
    // Closes the editor window the way the platform does: from outside, while nothing is being drawn.
    cx.update(|window, app| {
        let host = window.window_handle();
        app.defer(move |app| {
            app.update_window(host, |_, window, _| window.remove_window()).ok();
        });
    });
    cx.run_until_parked();
    let left = wb.read_with(cx, |_, app| app.windows().len());
    assert_eq!(left, 0, "the dialog's window is gone with the workbench it belongs to");
}

#[gpui::test]
fn a_key_pressed_in_the_dialogs_window_reaches_the_editor_window(cx: &mut TestAppContext) {
    let dir = settings_dir("update-window-enter", COMMENTED);
    let (wb, cx) = setup_with_config(cx, &dir);
    let fake = Fake::new();
    wb.update(cx, |wb, _| {
        wb.set_update_backend_for_test(fake.clone());
        wb.use_update_window_for_test();
    });
    fake.check_returns(Ok(None));
    assert!(wb.update_in(cx, |wb, window, cx| wb.ask_update_policy_for_test(true, window, cx)));
    settle(cx);
    cx.executor().advance_clock(Duration::from_secs(1));
    settle(cx);
    let own = update_window(&wb, cx).expect("the question is in a window of its own");
    // The real path of a click or a key in that window: the platform delivers it from outside any update, the dialog's window asks
    // the editor's window to run it, and that window's workbench must not be leased twice.
    cx.cx.simulate_keystrokes(own, "enter");
    settle(cx);
    let text = std::fs::read_to_string(dir.join("settings.jsonc")).unwrap();
    assert!(text.contains("\"update.check\": \"daily\""), "Enter is the primary button, Check Daily: {text}");
    assert!(dialog(&wb, cx).is_none());
}

#[gpui::test]
fn the_red_button_of_the_dialogs_window_answers_like_esc(cx: &mut TestAppContext) {
    let (wb, cx, fake) = rig(cx);
    wb.update(cx, |wb, _| wb.use_update_window_for_test());
    offered(&wb, cx, &fake, "0.2.0");
    settle(cx);
    // The platform asks "may this window close?" from outside any update; the answer comes from the editor window's workbench.
    assert!(update_window(&wb, cx).is_some());
    let host = cx.update(|window, _| window.window_handle().downcast::<Workbench>()).expect("the editor window");
    assert!(cx.cx.update(|app| Workbench::dialog_may_close_for_test(host, app)), "Esc is Later: the dialog is gone, so the window may go");
    settle(cx);
    assert!(dialog(&wb, cx).is_none());
    assert_eq!(state(&wb, cx), "available 0.2.0", "the offer stays in the status bar");
}
