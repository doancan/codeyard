//! Pull requests from a code extension in the Pull Requests view, end to end (#145): the probe extension (a real
//! component in codeyard-exthost) provides the merge requests of a repository remote it claims, calling its forge
//! with `auth.fetch` against a server in memory. AC1: its requests appear in the view and their details, files,
//! comments and checkout work. AC2: no frame sent to the exthost carries the account's token. Skipped when
//! codeyard-exthost is not built (`cargo build -p ext-host`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ext_fetch::tests::Fake;
use crate::qa_integration_tests::{git_in, repo_with_remote};
use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe.wasm");

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
auth = ["acme"]
secrets = true
why = "Lists merge requests."

[[commands]]
id = "auth-acme"
title = "Probe: Auth Acme"

[[pull-requests]]
id = "acme"
label = "Acme"
hosts = ["git.acme.example.com"]
auth = "acme"

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

const API: &str = "https://api.acme.example.com/repos/team/app/pulls";

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extpulls-{name}-{}", std::process::id()));
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

/// Run the window until `done`, or fail after 30 s.
fn until(cx: &mut VisualTestContext, what: &str, done: impl Fn(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn frames_contain(needle: &str) -> bool {
    let frames = crate::ext_runtime::SENT_FOR_TEST.lock().unwrap();
    let has = |b: &[u8]| b.windows(needle.len()).any(|w| w == needle.as_bytes());
    frames.iter().any(|f| has(f)) || has(&frames.concat())
}

fn labels(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap_or_default()
}

fn notice(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default()
}

/// Choose the row labelled `label` in the open picker.
fn choose(wb: &Entity<Workbench>, cx: &mut VisualTestContext, label: &str) {
    let wb2 = wb.clone();
    let want = label.to_string();
    until(cx, label, move |cx| labels(&wb2, cx).contains(&want));
    wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(label.into(), cx));
    assert_eq!(labels(wb, cx).first().map(String::as_str), Some(label));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
}

/// Open the Pull Requests view with its palette command.
fn show_view(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| {
        wb.focus_tree(window, cx);
        wb.run_command_for_test("GitHub: Pull Requests…", window, cx)
    });
}

/// Open the view and pick the probe's request `!7`.
fn pick_request(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    show_view(wb, cx);
    choose(wb, cx, "!7 Fix the parser");
}

#[gpui::test]
fn an_extensions_merge_requests_appear_and_its_token_never_reaches_it(cx: &mut TestAppContext) {
    if crate::ext_runtime::exthost_path().is_none() {
        eprintln!("skipped: codeyard-exthost is not built (cargo build -p ext-host)");
        return;
    }
    let cfg = config_dir("view");
    install(&cfg);
    // The repository: `origin` is a local bare repository nobody claims; `acme` is on the probe's host.
    let (dir, bare, other) = repo_with_remote("extpulls");
    let dir = std::fs::canonicalize(dir).unwrap();
    git_in(&other, &["checkout", "-q", "-b", "topic"]);
    std::fs::write(other.join("pr.txt"), "from the request\n").unwrap();
    git_in(&other, &["add", "-A"]);
    git_in(&other, &["commit", "-q", "-m", "request"]);
    git_in(&other, &["push", "-q", "origin", "HEAD:refs/pull/7/head"]);
    git_in(&dir, &["remote", "add", "acme", "https://git.acme.example.com/team/app.git"]);

    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, workspace_trust: false, ..Default::default() });
    });
    let config = cfg.clone();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(config), window, cx));
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    crate::qa_workbench_tests::open(&wb, &dir, cx);

    let server = Arc::new(
        Fake::default()
            .route("https://acme.example.com/oauth/token", 200, r#"{"access_token":"acme_SECRET_at","token_type":"bearer"}"#)
            .route("https://api.acme.example.com/me", 200, r#"{"preferred_username":"ada"}"#)
            .route(&format!("{API}/7/comments"), 201, "{}")
            .route(&format!("{API}/7"), 200, "Fixes **the parser**.")
            .route(API, 200, "7|Fix the parser|ada|topic|main\n"),
    );
    let keychain = wb.update(cx, |wb, _| wb.use_test_accounts(server.clone(), "gho_unused"));

    // Sign in to Acme once (the browser flow, answered by the fake server).
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Auth Acme", window, cx));
    until(cx, "the sign-in prompt", |cx| cx.has_pending_prompt());
    cx.simulate_prompt_answer("Sign In…");
    cx.run_until_parked();
    let state = wb.read_with(cx, |wb, _| wb.sign_in_state_for_test("you.probe")).expect("a sign-in waits");
    let back = format!("codeyard://ext/you.probe/callback?code=the-code&state={state}");
    assert!(wb.update_in(cx, |wb, window, cx| wb.extension_callback_for_test(&back, window, cx)));
    until(cx, "the account", |_| keychain.keys().len() == 1);

    // AC1: the provider's request is a row of the view, named with the provider and the extension.
    show_view(&wb, cx);
    let wb2 = wb.clone();
    until(cx, "the list", move |cx| labels(&wb2, cx).contains(&"!7 Fix the parser".to_string()));
    let details = wb.read_with(cx, |wb, cx| wb.overlay_details(cx)).unwrap();
    assert!(details[0].starts_with("Acme · ada · topic → main") && details[0].ends_with("you.probe"), "{details:?}");
    // Screen readers reach it like GitHub's rows: an option of the picker, with its detail.
    let snap = wb.update_in(cx, |wb, window, cx| wb.a11y_snapshot(window, cx));
    let rows = snap.picker.expect("the view is in the accessibility tree").list.rows;
    assert!(rows.iter().any(|(label, detail)| label == "!7 Fix the parser" && detail.as_deref().is_some_and(|d| d.contains("Acme"))), "{rows:?}");
    // AC2: the app signed the forge call; the token never went to the exthost.
    let listed = server.sent().into_iter().find(|s| s.url == API).expect("the provider listed through auth.fetch");
    assert_eq!(listed.authorization.as_deref(), Some("Bearer acme_SECRET_at"));
    assert_eq!(listed.hosts, ["api.acme.example.com"], "only the provider's declared hosts");
    assert!(frames_contain("git.acme.example.com"), "the tap sees the frames (the query went to the exthost)");
    assert!(!frames_contain("acme_SECRET"), "no frame to an exthost carries the token");
    cx.simulate_keystrokes("escape");

    // Details and comments: a Markdown page.
    pick_request(&wb, cx);
    choose(&wb, cx, "Show details and comments");
    let wb2 = wb.clone();
    until(cx, "the details page", move |cx| wb2.read_with(cx, |wb, _| wb.page_for_test()) == Some(("markdown", true)));
    let page = wb.read_with(cx, |wb, cx| wb.pull_page_for_test(cx)).unwrap();
    for want in ["# !7 Request 7", "Fixes **the parser**.", "- ✓ ci", "## Comments (0)"] {
        assert!(page.contains(want), "{want}\n{page}");
    }

    // A changed file as a diff: the target branch's text against the source branch's.
    pick_request(&wb, cx);
    choose(&wb, cx, "Show changed files");
    choose(&wb, cx, "a.txt");
    let wb2 = wb.clone();
    until(cx, "the diff", move |cx| wb2.read_with(cx, |wb, _| wb.diff_view()).is_some());
    let (path, summary) = wb.read_with(cx, |wb, cx| wb.diff_view().map(|d| (d.read(cx).path.clone(), d.read(cx).summary()))).unwrap();
    assert!(path.starts_with("a.txt (!7 Fix the parser)"), "{path}");
    assert_eq!(summary.0, 1, "one added line");

    // A comment: a user action, posted through auth.fetch, then listed.
    pick_request(&wb, cx);
    choose(&wb, cx, "Add comment…");
    wb.update(cx, |wb, cx| wb.set_overlay_query_for_test("Looks good".into(), cx));
    cx.simulate_keystrokes("enter");
    let wb2 = wb.clone();
    until(cx, "the comment", move |cx| notice(&wb2, cx).contains("Comment added"));
    let posted = server.sent().into_iter().find(|s| s.url.ends_with("/comments")).unwrap();
    assert_eq!((posted.url.as_str(), posted.body.as_slice()), (format!("{API}/7/comments").as_str(), b"Looks good".as_slice()));
    assert_eq!(posted.authorization.as_deref(), Some("Bearer acme_SECRET_at"));

    // Checkout: the provider says what to fetch; git runs on the workspace's host. (The remote now points at the bare
    // repository holding the request's ref, since the test has no forge to fetch from.)
    pick_request(&wb, cx);
    git_in(&dir, &["remote", "set-url", "acme", bare.to_str().unwrap()]);
    choose(&wb, cx, "Check out");
    let wb2 = wb.clone();
    until(cx, "the checkout", move |cx| notice(&wb2, cx).contains("Checked out"));
    assert_eq!(notice(&wb, cx), "Checked out !7 Fix the parser as pr-7");
    assert_eq!(git_in(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "pr-7");
    assert!(dir.join("pr.txt").exists());
    assert!(!frames_contain("acme_SECRET"));

    for p in [&dir, &bare, &other] {
        let _ = std::fs::remove_dir_all(p);
    }
}
