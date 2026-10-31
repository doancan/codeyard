//! Updates and version status (#69): a revoked version stops after one index fetch, a yanked one goes back without a
//! restart, more permissions wait for the card, `autoUpdate: off` downloads nothing, code waits 48 h unless the user
//! presses Update Now, another publisher or a new native program needs the full sheet, and the pre-release channel is
//! opt-in per extension.
//! The registry is a folder (`ext_core::testing::TestRegistry`); nothing touches the network.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ext_core::testing::{DirFetch, TestRegistry, github};
use ext_core::update::Standing;
use ext_core::yard::Status;
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ext_updates::Offer;
use crate::market_tests::setup;
use crate::workbench::Workbench;

const THEME: &str = r##"{"name":"x","colors":{"editor.background":"#101418","editor.foreground":"#d0d6dc"}}"##;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extupd-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A theme; `extra` is added to the manifest (permissions).
fn package(version: &str, extra: &str) -> Vec<u8> {
    let manifest = format!(
        "[extension]\nid = \"you.night\"\nversion = \"{version}\"\nname = \"Night\"\nlicense = \"MIT\"\n[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n{extra}"
    );
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.into_bytes(), executable: false },
        ext_pkg::Entry { path: "t.json".into(), data: THEME.as_bytes().to_vec(), executable: false },
    ];
    ext_pkg::write(entries).unwrap().0
}

struct Yard {
    r: TestRegistry,
    fetch: Arc<DirFetch>,
    cfg: PathBuf,
}

impl Yard {
    /// A registry with `you.night` at each of `versions`, in the user's settings with `auto` (or the default), and
    /// the first version installed from it.
    fn new(root: &Path, name: &str, versions: &[&str], auto: Option<&str>) -> Yard {
        let now = crate::ext_registry::now();
        let mut r = TestRegistry::new(&root.join("yard"), name, now);
        for v in versions {
            r.publish(&package(v, ""), &github(7, 42), now);
        }
        let cfg = root.join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let mut settings = serde_json::json!({ "extensions.registries": [r.setting()] });
        if let Some(a) = auto {
            settings["extensions.autoUpdate"] = a.into();
        }
        std::fs::write(cfg.join("settings.jsonc"), settings.to_string()).unwrap();
        let fetch = Arc::new(DirFetch::new(&r));
        crate::ext_registry::add_fetch_for_test(fetch.clone());
        let y = Yard { r, fetch, cfg };
        y.install(&format!("you.night@{}", versions[0]));
        y
    }

    fn install(&self, spec: &str) {
        let regs =
            crate::ext_registry::registries(&crate::settings::parse_layers(&std::fs::read_to_string(self.cfg.join("settings.jsonc")).unwrap(), &[]).unwrap().0);
        crate::ext_registry::install_from_command_line(&self.cfg, &regs, spec).unwrap();
    }

    /// What the registry did since the client last asked becomes visible on the next check (the cached index is a day
    /// old).
    fn a_day_later(&self) {
        let _ = std::fs::remove_file(crate::ext_registry::cache_root(&self.cfg).join(&self.r.name).join("state.json"));
    }

    fn installed(&self) -> String {
        crate::ext::store(&self.cfg).locked().unwrap()[&ext_manifest::ExtId::parse("you.night").unwrap()].version.to_string()
    }

    /// Packages downloaded so far (not the index, the signature or a page).
    fn packages(&self) -> Vec<String> {
        self.fetch.requests.lock().unwrap().iter().filter(|u| u.ends_with(".cyx")).cloned().collect()
    }
}

fn check(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.check_extension_updates_for_test(window, cx));
    cx.run_until_parked();
}

fn status(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> crate::ext_updates::Status {
    wb.read_with(cx, |wb, _| wb.extension_update_for_test("you.night")).expect("checked")
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> (bool, String) {
    wb.read_with(cx, |wb, _| wb.notice_for_test()).unwrap_or_default()
}

fn active_version() -> Option<String> {
    crate::ext::current().active.iter().find(|e| e.id() == "you.night").map(|e| e.manifest.version.to_string())
}

/// AC1 (exit criterion 4 is measured in #70): a version revoked in the registry stops on the next index fetch, with
/// the registry's reason, stays stopped offline and after a restart, and runs again once a fixed version replaces it.
#[gpui::test]
fn a_revoked_version_stops_within_one_index_fetch(cx: &mut TestAppContext) {
    let root = dir("revoke");
    let mut y = Yard::new(&root, "revoke", &["1.0.0"], None);
    let (wb, cx) = setup(cx, &y.cfg);
    assert_eq!(active_version().as_deref(), Some("1.0.0"));
    let now = crate::ext_registry::now();
    y.r.set_status("you.night", "1.0.0", Status::Revoked { reason: "Sends your files to a stranger.".into() }, now);
    y.a_day_later();
    let before = y.fetch.count();
    check(&wb, cx);
    let index_files = y.fetch.requests.lock().unwrap()[before..].to_vec();
    assert!(index_files.iter().all(|u| u.contains("/index.json") || u.contains("/root.json")), "one index fetch, nothing else: {index_files:?}");
    assert_eq!(active_version(), None, "stopped");
    let (error, text) = notice(&wb, cx);
    assert!(error && text.contains("revoked") && text.contains("Sends your files to a stranger."), "{text}");
    let (badge, line) = wb.read_with(cx, |wb, _| wb.extension_state_for_test("you.night")).unwrap();
    assert_eq!(badge, "Stopped");
    assert!(line.unwrap().contains("Sends your files to a stranger."));
    // It stays stopped without the network and after a restart: the revocation is kept on disk.
    let problems = crate::ext::reload(Some(&y.cfg));
    assert_eq!(active_version(), None);
    assert!(problems.iter().any(|p| p.contains("revoked")), "{problems:?}");

    // A fixed version replaces it like any update.
    y.r.publish(&package("1.0.1", ""), &github(7, 42), now);
    y.a_day_later();
    check(&wb, cx);
    assert_eq!(y.installed(), "1.0.1");
    assert_eq!(active_version().as_deref(), Some("1.0.1"));
    assert!(notice(&wb, cx).1.contains("updated to version 1.0.1"), "{:?}", notice(&wb, cx));
}

/// AC2: a yanked installed version goes back to the previous one at once, from the store, without a restart.
#[gpui::test]
fn a_yanked_version_goes_back_without_a_restart(cx: &mut TestAppContext) {
    let root = dir("yank");
    let mut y = Yard::new(&root, "yank", &["1.0.0", "1.1.0"], None);
    y.install("you.night@1.1.0");
    let (wb, cx) = setup(cx, &y.cfg);
    assert_eq!(active_version().as_deref(), Some("1.1.0"));
    y.r.set_status("you.night", "1.1.0", Status::Yanked, crate::ext_registry::now());
    y.a_day_later();
    let downloads = y.packages().len();
    check(&wb, cx);
    assert_eq!(y.installed(), "1.0.0");
    assert_eq!(active_version().as_deref(), Some("1.0.0"), "applied in this session");
    assert_eq!(y.packages().len(), downloads, "the previous version came from the store");
    assert!(notice(&wb, cx).1.contains("went back to version 1.0.0"), "{:?}", notice(&wb, cx));
    // Nothing to do on the next check: 1.1.0 is hidden from updates.
    check(&wb, cx);
    assert_eq!((y.installed(), status(&wb, cx).offer), ("1.0.0".to_string(), None));
}

/// AC3: an update that asks for more is not applied until the user agrees on the card, which lists only what is new;
/// the package is not even downloaded before that.
#[gpui::test]
fn an_update_that_adds_a_permission_waits_for_the_card(cx: &mut TestAppContext) {
    let root = dir("perm");
    let mut y = Yard::new(&root, "perm", &["1.0.0"], None);
    let (wb, cx) = setup(cx, &y.cfg);
    y.r.publish(&package("1.1.0", "[permissions]\nclipboard = \"read\"\n"), &github(7, 42), crate::ext_registry::now());
    y.a_day_later();
    for _ in 0..2 {
        check(&wb, cx);
    }
    let s = status(&wb, cx);
    let Some(Offer::MorePermissions { version, added }) = &s.offer else { panic!("{s:?}") };
    assert_eq!(version.to_string(), "1.1.0");
    assert_eq!(Workbench::permission_lines_for_test(added), ["Read the clipboard"]);
    assert_eq!(y.installed(), "1.0.0");
    assert_eq!(y.packages().len(), 1, "only the first install: {:?}", y.packages());
    let (badge, _) = wb.read_with(cx, |wb, _| wb.extension_state_for_test("you.night")).unwrap();
    assert_eq!(badge, "Update needs your OK");
    let n = notice(&wb, cx).1;
    assert_eq!(n.matches("waits for your OK").count(), 1, "announced once, not on every check: {n}");

    wb.update_in(cx, |wb, window, cx| wb.accept_extension_update_for_test("you.night", window, cx));
    cx.run_until_parked();
    assert_eq!(y.installed(), "1.1.0");
    assert_eq!(active_version().as_deref(), Some("1.1.0"));
}

/// #146: an update that adds a native program never installs by itself or from the card, even when the card's
/// permissions were accepted: the full sheet asks for the id to be typed, as a first install does (§6.3).
#[gpui::test]
fn an_update_that_adds_a_native_program_needs_the_id_typed(cx: &mut TestAppContext) {
    let Some(here) = ext_manifest::this_target() else { return };
    let root = dir("native");
    let mut y = Yard::new(&root, "native-update", &["1.0.0"], None);
    let (wb, cx) = setup(cx, &y.cfg);
    let now = crate::ext_registry::now();
    y.r.publish(&ext_core::testing::native_package("you.night", "1.1.0", here), &github(7, 42), now);
    // Cleared by the registry, so the check would install it at once if nothing else stopped it.
    y.r.edit_release("you.night", "1.1.0", now, |r| r.immediate = true);
    y.a_day_later();
    check(&wb, cx);
    let s = status(&wb, cx);
    let Some(Offer::NativePrograms { version, added }) = &s.offer else { panic!("{s:?}") };
    assert_eq!(version.to_string(), "1.1.0");
    assert_eq!(added.run, ["./bin/hello"]);
    assert_eq!(y.installed(), "1.0.0");
    let (badge, _) = wb.read_with(cx, |wb, _| wb.extension_state_for_test("you.night")).unwrap();
    assert_eq!(badge, "Update needs your OK");
    // Agreeing to the new permissions as the card would is not enough.
    let regs = crate::ext_registry::registries(&crate::settings::parse_layers(&std::fs::read_to_string(y.cfg.join("settings.jsonc")).unwrap(), &[]).unwrap().0);
    let id = ext_manifest::ExtId::parse("you.night").unwrap();
    let offer = crate::ext_updates::update_now(&y.cfg, &regs, &id, Some(added), now).unwrap();
    assert!(matches!(offer, Some(Offer::NativePrograms { .. })), "{offer:?}");
    assert_eq!(y.installed(), "1.0.0");
    // Update Now opens the full sheet; it installs only once the id is typed.
    wb.update_in(cx, |wb, window, cx| wb.update_extension_now_for_test("you.night", window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.consent_source_for_test()).is_some(), "the full sheet: {:?}", notice(&wb, cx));
    assert!(wb.update_in(cx, |wb, window, cx| wb.consent_typing_focused_for_test(window, cx)), "the id field takes the keyboard");
    for typed in ["", "you.nigh"] {
        assert!(!wb.update_in(cx, |wb, _, cx| wb.type_consent_id_for_test(typed, cx)));
        wb.update_in(cx, |wb, window, cx| wb.answer_consent_for_test(true, window, cx));
        cx.run_until_parked();
        assert_eq!(y.installed(), "1.0.0", "installed with {typed:?}");
        assert!(notice(&wb, cx).1.contains("Type you.night"), "{:?}", notice(&wb, cx));
    }
    assert!(wb.update_in(cx, |wb, _, cx| wb.type_consent_id_for_test("you.night", cx)));
    wb.update_in(cx, |wb, window, cx| wb.answer_consent_for_test(true, window, cx));
    cx.run_until_parked();
    assert_eq!(y.installed(), "1.1.0");
}

/// AC4: with `autoUpdate: off` no update is downloaded (`notify` the same); the version shows, and Update Now
/// installs it.
#[gpui::test]
fn with_auto_update_off_no_update_is_downloaded(cx: &mut TestAppContext) {
    for auto in ["off", "notify"] {
        let root = dir(&format!("off-{auto}"));
        let mut y = Yard::new(&root, &format!("off-{auto}"), &["1.0.0"], Some(auto));
        let (wb, cx) = setup(cx, &y.cfg);
        y.r.publish(&package("1.1.0", ""), &github(7, 42), crate::ext_registry::now());
        y.a_day_later();
        let downloads = y.packages().len();
        check(&wb, cx);
        assert_eq!(status(&wb, cx).offer, Some(Offer::Available { version: ext_manifest::Version::parse("1.1.0").unwrap() }), "{auto}");
        assert_eq!(y.packages().len(), downloads, "{auto}: nothing downloaded");
        assert!(y.fetch.requests.lock().unwrap().iter().all(|u| !u.contains(".details.json")), "{auto}: not even the page");
        assert_eq!(y.installed(), "1.0.0");
        let (badge, _) = wb.read_with(cx, |wb, _| wb.extension_state_for_test("you.night")).unwrap();
        assert_eq!(badge, "Update available");
        wb.update_in(cx, |wb, window, cx| wb.update_extension_now_for_test("you.night", window, cx));
        cx.run_until_parked();
        assert_eq!(y.installed(), "1.1.0", "{auto}");
    }
}

/// A version that runs code waits 48 h after it is published; Update Now skips the wait, and a fix the registry
/// cleared installs at once.
#[gpui::test]
fn code_waits_48_hours_unless_the_user_or_the_registry_says_now(cx: &mut TestAppContext) {
    let root = dir("wait");
    let mut y = Yard::new(&root, "wait", &["1.0.0"], None);
    let (wb, cx) = setup(cx, &y.cfg);
    let now = crate::ext_registry::now();
    // The index says it runs code (this test's package does not, so it can install here).
    y.r.publish(&package("1.1.0", ""), &github(7, 42), now);
    y.r.edit_release("you.night", "1.1.0", now, |r| r.code = true);
    y.a_day_later();
    check(&wb, cx);
    let Some(Offer::Waiting { ready_at, .. }) = status(&wb, cx).offer else { panic!("{:?}", status(&wb, cx)) };
    assert!(ready_at >= now + ext_core::update::CODE_WAIT_SECS);
    assert_eq!(y.installed(), "1.0.0");
    wb.update_in(cx, |wb, window, cx| wb.update_extension_now_for_test("you.night", window, cx));
    cx.run_until_parked();
    assert_eq!(y.installed(), "1.1.0");

    y.r.publish(&package("1.2.0", ""), &github(7, 42), now);
    y.r.edit_release("you.night", "1.2.0", now, |r| {
        r.code = true;
        r.immediate = true;
    });
    y.a_day_later();
    check(&wb, cx);
    assert_eq!(y.installed(), "1.2.0", "an advisory fix the registry cleared");
}

/// Another publisher for the same id never updates by itself: Review opens the full sheet, which says so.
#[gpui::test]
fn another_publisher_needs_the_full_sheet(cx: &mut TestAppContext) {
    let root = dir("publisher");
    let mut y = Yard::new(&root, "publisher", &["1.0.0"], None);
    let (wb, cx) = setup(cx, &y.cfg);
    let now = crate::ext_registry::now();
    y.r.set_publisher("you.night", &github(8, 1), now);
    y.r.publish(&package("1.1.0", ""), &github(8, 1), now);
    y.a_day_later();
    check(&wb, cx);
    assert!(matches!(status(&wb, cx).offer, Some(Offer::NewPublisher { .. })), "{:?}", status(&wb, cx));
    assert_eq!(y.installed(), "1.0.0");
    wb.update_in(cx, |wb, window, cx| wb.update_extension_now_for_test("you.night", window, cx));
    cx.run_until_parked();
    let (_, warnings) = wb.read_with(cx, |wb, _| wb.consent_source_for_test()).expect("the full sheet");
    assert!(warnings.iter().any(|w| w.contains("Another publisher")), "{warnings:?}");
    assert_eq!(y.installed(), "1.0.0", "until the user installs it there");
}

/// Pre-releases come only to an extension the user opted in on its page; the choice is kept in the lock.
#[gpui::test]
fn the_pre_release_channel_is_opt_in_per_extension(cx: &mut TestAppContext) {
    let root = dir("beta");
    let mut y = Yard::new(&root, "beta", &["1.0.0"], None);
    let (wb, cx) = setup(cx, &y.cfg);
    y.r.publish(&package("1.1.0-beta.1", ""), &github(7, 42), crate::ext_registry::now());
    y.a_day_later();
    check(&wb, cx);
    assert_eq!((y.installed(), status(&wb, cx).offer), ("1.0.0".to_string(), None));
    wb.update_in(cx, |wb, window, cx| wb.set_extension_pre_release_for_test("you.night", true, window, cx));
    cx.run_until_parked();
    assert_eq!(y.installed(), "1.1.0-beta.1", "the check after the change installs it");
    assert!(crate::ext::store(&y.cfg).locked().unwrap()[&ext_manifest::ExtId::parse("you.night").unwrap()].pre_release);
}

/// A deprecated version keeps working and its page names the replacement.
#[gpui::test]
fn a_deprecated_version_names_its_replacement(cx: &mut TestAppContext) {
    let root = dir("deprecate");
    let mut y = Yard::new(&root, "deprecate", &["1.0.0"], None);
    let (wb, cx) = setup(cx, &y.cfg);
    let replacement = ext_manifest::ExtId::parse("you.dawn").unwrap();
    y.r.set_status("you.night", "1.0.0", Status::Deprecated { replacement: Some(replacement.clone()) }, crate::ext_registry::now());
    y.a_day_later();
    check(&wb, cx);
    assert_eq!(status(&wb, cx).standing, Standing::Deprecated { replacement: Some(replacement) });
    assert_eq!(active_version().as_deref(), Some("1.0.0"));
}

/// `codeyard --update-extensions`: the person asked now, so the registry is asked again although its index was
/// fetched today, and a revoked version is stopped (recorded in revoked.json) without a window.
#[test]
fn update_extensions_asks_now_and_stops_a_revoked_version() {
    let root = dir("check-now");
    let mut y = Yard::new(&root, "checknow", &["1.0.0"], None);
    let now = crate::ext_registry::now();
    y.r.set_status("you.night", "1.0.0", Status::Revoked { reason: "Canary.".into() }, now);
    let regs = crate::ext_registry::registries(&crate::settings::parse_layers(&std::fs::read_to_string(y.cfg.join("settings.jsonc")).unwrap(), &[]).unwrap().0);
    // The daily check still uses this morning's copy and sees nothing.
    let daily = crate::ext_updates::check(&y.cfg, &regs, ext_core::update::AutoUpdate::Install, now);
    assert!(daily.stopped.is_empty(), "{:?}", daily.stopped);
    let report = crate::ext_updates::check_now(&y.cfg, &regs, ext_core::update::AutoUpdate::Install, now);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(report.stopped.iter().any(|l| l.contains("Canary.")), "{:?}", report.stopped);
    assert!(crate::ext::store(&y.cfg).revoked().values().any(|r| r.version.to_string() == "1.0.0"));
}

/// A public registry ships only with real ceremony keys: the check that runs when the app compiles.
#[test]
fn the_public_registry_refuses_placeholder_keys() {
    use crate::ext_registry::public_problem;
    let a = "3f9c2a71b8e04d56a1c7e9f02b4d68a35c1e7f90a2b4c6d8e0f1a3b5c7d9e1f3";
    let b = "0a1b2c3d4e5f60718293a4b5c6d7e8f90123456789abcdef0fedcba987654321";
    let c = "e7d6c5b4a3928170f6e5d4c3b2a19087f6e5d4c3b2a1908776655443322110ff";
    assert_eq!(public_problem(None), None, "no public registry before the ceremony");
    assert_eq!(public_problem(Some(("yard.codeyard.dev", "https://yard.codeyard.dev", &[a, b, c], 2))), None);
    let zeros = "0000000000000000000000000000000000000000000000000000000000000000";
    for bad in [
        Some(("yard.codeyard.dev", "https://yard.codeyard.dev", &[a, b, zeros][..], 2)),
        Some(("yard.codeyard.dev", "https://yard.codeyard.dev", &[a, b, "<C>"][..], 2)),
        Some(("yard.codeyard.dev", "https://yard.codeyard.dev", &[a, b, &a.to_uppercase()][..], 2)),
        Some(("yard.codeyard.dev", "https://yard.codeyard.dev", &[a, a, c][..], 2)),
        Some(("yard.codeyard.dev", "https://yard.codeyard.dev", &[a, b, c][..], 1)),
        Some(("yard.codeyard.dev", "https://yard.codeyard.dev", &[a, b, c][..], 4)),
        Some(("yard.codeyard.dev", "http://yard.codeyard.dev", &[a, b, c][..], 2)),
        Some(("yard.example.dev", "https://yard.example.dev", &[a, b, c][..], 2)),
    ] {
        assert!(public_problem(bad).is_some(), "{bad:?}");
    }
}
