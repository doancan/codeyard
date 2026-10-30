//! Accounts, secrets and HTTPS of code extensions in a window, end to end: the probe extension (a real component in
//! codeyard-exthost) signs in, calls the API as the user and stores secrets, against a server in memory and an
//! in-memory keychain. The token must never be in a frame sent to the exthost. Skipped when codeyard-exthost is not
//! built (`cargo build -p ext-host`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ext_fetch::tests::Fake;
use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe.wasm");
const TOKEN: &str = "gho_FRAME_TOKEN_7f3a91";

const MANIFEST: &str = r#"[extension]
id = "you.probe"
version = "1.0.0"
name = "Probe"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
read = ["a.txt"]
net = ["api.example.com"]
auth = ["github", "acme"]
secrets = true
why = "Tests accounts."

[[commands]]
id = "auth"
title = "Probe: Auth"

[[commands]]
id = "authfetch"
title = "Probe: Auth Fetch"

[[commands]]
id = "authfetch-other"
title = "Probe: Auth Fetch Other"

[[commands]]
id = "auth-acme"
title = "Probe: Auth Acme"

[[commands]]
id = "authfetch-acme"
title = "Probe: Auth Fetch Acme"

[[commands]]
id = "netfetch"
title = "Probe: Net Fetch"

[[commands]]
id = "secret-set"
title = "Probe: Secret Set"

[[commands]]
id = "secret-get"
title = "Probe: Secret Get"

[[oauth]]
id = "acme"
name = "Acme"
authorize-url = "https://acme.example.com/oauth/authorize"
token-url = "https://acme.example.com/oauth/token"
client-id = "cy-123"
scopes = ["read"]
hosts = ["api.acme.example.com"]
account-url = "https://api.acme.example.com/me"
"#;

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extaccounts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn install(cfg: &Path) {
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: MANIFEST.as_bytes().to_vec(), executable: false },
        ext_pkg::Entry { path: "Cargo.toml".into(), data: Vec::new(), executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: PROBE.to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).expect("the probe passes the import check");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "/downloads/probe.cyx".into() }).expect("installs");
}

fn setup<'a>(cx: &'a mut TestAppContext, cfg: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let dir = cfg.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    (wb, cx)
}

/// Run the window until `done`, or fail after 20 s.
fn until(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn answer(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.extension_answer_for_test("you.probe")).unwrap_or_default()
}

/// Run a palette command and wait for the probe's answer to it.
fn run(wb: &Entity<Workbench>, cx: &mut VisualTestContext, label: &str) -> String {
    let before = answer(wb, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    let wb2 = wb.clone();
    until(cx, move |cx| answer(&wb2, cx) != before || cx.has_pending_prompt(), label);
    answer(wb, cx)
}

/// Whether any frame sent to an exthost, or all of them end to end, holds `needle`.
fn frames_contain(needle: &str) -> bool {
    let frames = crate::ext_runtime::SENT_FOR_TEST.lock().unwrap();
    let has = |b: &[u8]| b.windows(needle.len()).any(|w| w == needle.as_bytes());
    frames.iter().any(|f| has(f)) || has(&frames.concat())
}

fn skip() -> bool {
    if crate::ext_runtime::exthost_path().is_none() {
        eprintln!("skipped: codeyard-exthost is not built (cargo build -p ext-host)");
        return true;
    }
    false
}

#[gpui::test]
fn an_extension_calls_github_as_the_user_without_the_token(cx: &mut TestAppContext) {
    if skip() {
        return;
    }
    let cfg = config_dir("github");
    install(&cfg);
    let (wb, cx) = setup(cx, &cfg);
    let server = Arc::new(Fake::default().route("https://api.github.com/user/repos", 200, r#"[{"name":"codeyard"}]"#).route(
        "https://api.github.com/user",
        200,
        r#"{"login":"ada-lovelace-1815"}"#,
    ));
    let keychain = wb.update(cx, |wb, _| wb.use_test_accounts(server.clone(), TOKEN));

    // First use asks, in a system dialog that names the publisher and the account.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Auth", window, cx));
    until(cx, |cx| cx.has_pending_prompt(), "the account prompt");
    let (title, detail) = cx.pending_prompt().unwrap();
    assert!(title.contains("Probe") && title.contains("@ada-lovelace-1815"), "{title}");
    assert!(detail.contains("you, installed from /downloads/probe.cyx") && detail.contains("api.github.com"), "{detail}");
    cx.simulate_prompt_answer("Allow");
    let wb2 = wb.clone();
    until(cx, move |cx| answer(&wb2, cx) == "account github ada-lovelace-1815", "the account");
    assert_eq!(keychain.keys().len(), 1, "the account is in the keychain");
    // AC1: the request is signed by the app; the extension gets the answer.
    assert_eq!(run(&wb, cx, "Probe: Auth Fetch"), r#"done body [{"name":"codeyard"}]"#);
    // A second session does not ask again.
    assert_eq!(run(&wb, cx, "Probe: Auth"), "account github ada-lovelace-1815");
    assert!(!cx.has_pending_prompt());
    let sent = server.sent();
    let last = sent.last().unwrap();
    assert_eq!((last.url.as_str(), last.authorization.as_deref()), ("https://api.github.com/user/repos", Some(format!("Bearer {TOKEN}").as_str())));
    assert!(frames_contain("ada-lovelace-1815"), "the tap sees the frames (the account name went to the exthost)");
    assert!(!frames_contain(TOKEN), "no frame to an exthost carries the token");

    // AC2: a host outside GitHub's is not signed, and not sent at all.
    let n = server.sent().len();
    let denied = run(&wb, cx, "Probe: Auth Fetch Other");
    assert!(denied.starts_with("denied") && denied.contains("api.example.com"), "{denied}");
    assert_eq!(server.sent().len(), n);

    // AC4: signing out removes the token from the keychain; the next request is refused.
    assert_eq!(wb.read_with(cx, |wb, _| wb.accounts_known_for_test()), [("you.probe".to_string(), "github".to_string(), "ada-lovelace-1815".to_string())]);
    wb.update(cx, |wb, cx| wb.sign_out_for_test("you.probe", "github", cx));
    let wb2 = wb.clone();
    until(cx, move |cx| wb2.read_with(cx, |wb, _| wb.accounts_known_for_test()).is_empty(), "the sign-out");
    assert!(keychain.keys().is_empty());
    assert!(!keychain.contains_value(TOKEN));
    assert!(run(&wb, cx, "Probe: Auth Fetch").contains("not signed in to GitHub"));
    assert!(!frames_contain(TOKEN));
}

#[gpui::test]
fn an_oauth_sign_in_comes_back_only_to_its_extension(cx: &mut TestAppContext) {
    if skip() {
        return;
    }
    let cfg = config_dir("oauth");
    install(&cfg);
    let (wb, cx) = setup(cx, &cfg);
    let server = Arc::new(
        Fake::default()
            .route("https://acme.example.com/oauth/token", 200, r#"{"access_token":"acme_SECRET_at","token_type":"bearer","refresh_token":"acme_SECRET_rt"}"#)
            .route("https://api.acme.example.com/me", 200, r#"{"preferred_username":"ada"}"#)
            .route("https://api.acme.example.com/deployments", 200, r#"{"deployments":1}"#),
    );
    let keychain = wb.update(cx, |wb, _| wb.use_test_accounts(server.clone(), TOKEN));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Auth Acme", window, cx));
    until(cx, |cx| cx.has_pending_prompt(), "the sign-in prompt");
    assert!(cx.pending_prompt().unwrap().0.contains("Acme"));
    cx.simulate_prompt_answer("Sign In…");
    cx.run_until_parked();
    let opened = cx.opened_url().expect("the browser opens the sign-in page");
    assert!(opened.starts_with("https://acme.example.com/oauth/authorize?") && opened.contains("code_challenge_method=S256"), "{opened}");
    let state = wb.read_with(cx, |wb, _| wb.sign_in_state_for_test("you.probe")).expect("a sign-in waits");
    assert!(opened.contains(&format!("state={state}")));

    // AC3: another extension's address with this state, or a wrong state, completes nothing.
    let stolen = format!("codeyard://ext/you.other/callback?code=evil&state={state}");
    assert!(wb.update_in(cx, |wb, window, cx| wb.extension_callback_for_test(&stolen, window, cx)));
    let forged = "codeyard://ext/you.probe/callback?code=evil&state=guess";
    assert!(!wb.update_in(cx, |wb, window, cx| wb.extension_callback_for_test(forged, window, cx)));
    cx.run_until_parked();
    assert!(server.sent().is_empty(), "no code was traded");
    assert!(wb.read_with(cx, |wb, _| wb.sign_in_state_for_test("you.probe")).is_some(), "the real sign-in still waits");

    let good = format!("codeyard://ext/you.probe/callback?code=the-code&state={state}");
    assert!(wb.update_in(cx, |wb, window, cx| wb.extension_callback_for_test(&good, window, cx)));
    let wb2 = wb.clone();
    until(cx, move |cx| answer(&wb2, cx) == "account acme ada", "the account");
    let traded = String::from_utf8(server.sent()[0].body.clone()).unwrap();
    assert!(traded.contains("code=the-code") && traded.contains("code_verifier="), "{traded}");
    assert_eq!(keychain.keys().len(), 1);

    assert_eq!(run(&wb, cx, "Probe: Auth Fetch Acme"), r#"done body {"deployments":1}"#);
    assert_eq!(server.sent().last().unwrap().authorization.as_deref(), Some("Bearer acme_SECRET_at"));
    assert!(!frames_contain("acme_SECRET"), "neither token reaches the exthost");
}

#[gpui::test]
fn secrets_and_net_fetch_work_through_the_broker(cx: &mut TestAppContext) {
    if skip() {
        return;
    }
    let cfg = config_dir("secrets");
    install(&cfg);
    let (wb, cx) = setup(cx, &cfg);
    let server = Arc::new(Fake::default().route("https://api.example.com/status", 200, "up"));
    let keychain = wb.update(cx, |wb, _| wb.use_test_accounts(server.clone(), TOKEN));
    assert_eq!(run(&wb, cx, "Probe: Secret Get"), "text None");
    assert_eq!(run(&wb, cx, "Probe: Secret Set"), "done");
    assert_eq!(run(&wb, cx, "Probe: Secret Get"), "text Some(\"s3cr3t\")");
    assert!(keychain.keys()[0].ends_with("/you.probe/secret/api-key"));
    assert_eq!(run(&wb, cx, "Probe: Net Fetch"), "done body up");
    assert_eq!(server.sent()[0].authorization, None);
}
