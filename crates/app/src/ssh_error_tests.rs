//! Negative cases when working over SSH, using a real (local, temporary) `sshd`: bad name, authentication
//! failure, the server going away, the bridge dropping. Expected: the reason is stated, unsaved work is not lost,
//! the window stays usable. Tests are skipped if sshd or the agent is missing.

#![cfg(unix)]

use std::time::Duration;

use gpui::{TestAppContext, VisualTestContext};

use crate::qa_integration_tests::ssh_workbench;
use crate::qa_integration_tests::{c_server, formatting_server, notice, setup_with, ssh_open_file, wait_secs};
use crate::qa_workbench_tests::{open, project, setup};
use crate::workbench::Workbench;

fn tick(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_secs(1));
}

fn conn(wb: &gpui::Entity<Workbench>, cx: &mut VisualTestContext) -> String {
    wb.read_with(cx, |w, _| w.conn_for_test())
}

/// Process ids of the server-side bridge processes (`agent --attach`) of this fixture's sshd.
fn bridge_pids(sshd: &crate::test_sshd::Sshd) -> Vec<String> {
    let pattern = format!("{}/remote/agent.*--attach", sshd.dir.display());
    let out = std::process::Command::new("pgrep").arg("-f").arg(pattern).output().map(|o| o.stdout).unwrap_or_default();
    String::from_utf8_lossy(&out).split_whitespace().map(String::from).collect()
}

/// Kills the bridge and waits until the app is connected again through a new one. The "Reconnected" notice cannot be
/// the signal: anything that reports right after it (a language server that is not installed) replaces it, and on a
/// slow runner the poll never sees it.
fn drop_the_bridge_and_wait_for_the_reconnect(wb: &gpui::Entity<Workbench>, cx: &mut VisualTestContext, sshd: &crate::test_sshd::Sshd) {
    let before = bridge_pids(sshd);
    assert!(!before.is_empty(), "no bridge process to kill: the pattern matches nothing");
    for pid in &before {
        let _ = std::process::Command::new("kill").arg(pid).status();
    }
    wait_secs(cx, 60, "reconnected through a new bridge", |cx| {
        tick(cx);
        conn(wb, cx) == "connected" && bridge_pids(sshd).iter().any(|p| !before.contains(p))
    });
}

/// Nonexistent name: the connection fails with the reason (name doesn't resolve); local work goes on.
#[gpui::test]
fn an_unknown_host_name_fails_with_a_reason_and_local_work_continues(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("err-badhost") else { return };
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |w, window, cx| w.connect("no-such-host.invalid".into(), window, cx));
    wait_secs(cx, 40, "the connection to fail", |cx| {
        tick(cx);
        conn(&wb, cx).starts_with("failed")
    });
    let why = conn(&wb, cx).to_lowercase();
    assert!(
        why.contains("resolve") || why.contains("hostname") || why.contains("name or service") || why.contains("not known"),
        "the reason is shown: {why:?}"
    );
    // The window did not die: a local folder opens.
    let dir = project("err-badhost-local");
    open(&wb, &dir, cx);
    assert!(wb.read_with(cx, |w, cx| w.root_for_test(cx)).is_some(), "local folders still open after a failed connection");
}

/// Rejected key: "permission denied" is stated plainly, no password dialog.
#[gpui::test]
fn an_authentication_failure_says_permission_denied(cx: &mut TestAppContext) {
    let Some(sshd) = crate::test_sshd::start("err-auth") else { return };
    sshd.add_alias_without_key("no-key-host");
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |w, window, cx| w.connect("no-key-host".into(), window, cx));
    wait_secs(cx, 40, "the connection to fail", |cx| {
        tick(cx);
        conn(&wb, cx).starts_with("failed")
    });
    let why = conn(&wb, cx).to_lowercase();
    assert!(why.contains("permission denied") || why.contains("authentication") || why.contains("publickey"), "{why:?}");
}

/// Shortens the reconnect window, restored at test end (process-wide env var).
struct ShortReconnectWindow;

impl ShortReconnectWindow {
    fn new() -> Self {
        // SAFETY: sshd tests run sequentially (test_sshd::TURN); the variable exists only for the duration of this test.
        unsafe { std::env::set_var("CODEYARD_RECONNECT_WINDOW_SECS", "4") };
        Self
    }
}

impl Drop for ShortReconnectWindow {
    fn drop(&mut self) {
        // SAFETY: see new.
        unsafe { std::env::remove_var("CODEYARD_RECONNECT_WINDOW_SECS") };
    }
}

/// Server vanishes while connected: "reconnecting" first, then a permanent-loss notice saying
/// the work is safe; saving errors out instead of hanging, nothing is lost;
/// "Reconnect" with an unsaved file gives guidance, not a dead end.
#[gpui::test]
fn the_server_going_away_keeps_unsaved_work_and_says_what_happened(cx: &mut TestAppContext) {
    let Some(mut sshd) = crate::test_sshd::start("err-gone") else { return };
    let _window = ShortReconnectWindow::new();
    let dir = project("err-gone-repo");
    let (wb, cx) = ssh_workbench(cx, &dir);
    let ed = ssh_open_file(&wb, cx, &dir.join("src/main.rs"));
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "// unsaved\n", cx));
    assert!(ed.read_with(cx, |e, _| e.is_dirty()));

    sshd.stop_server();
    wait_secs(cx, 60, "the window to notice the connection dropped", |cx| {
        tick(cx);
        conn(&wb, cx) != "connected"
    });
    let first = notice(&wb, cx).to_lowercase();
    assert!(first.contains("dropped") || first.contains("reconnecting") || first.contains("lost"), "the first message says what is happening: {first:?}");

    wait_secs(cx, 90, "reconnecting to give up", |cx| {
        tick(cx);
        conn(&wb, cx).starts_with("failed")
    });
    let said = notice(&wb, cx);
    assert!(said.contains("Lost the connection to"), "{said:?}");
    assert!(said.contains("unsaved changes are kept"), "the user is told their work is safe: {said:?}");

    // Try to save: it does not hang, the error is visible, the change is not lost.
    ed.update(cx, |e, cx| e.save_now(cx));
    wait_secs(cx, 60, "the save attempt to end", |cx| {
        tick(cx);
        ed.read_with(cx, |e, _| e.status_for_test().is_some())
    });
    let status = ed.read_with(cx, |e, _| e.status_for_test()).unwrap_or_default();
    assert!(status.contains("Could not save"), "{status:?}");
    assert!(ed.read_with(cx, |e, _| e.is_dirty()), "the edit is still there");
    assert!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap().starts_with("fn main"), "nothing half-written reached the server");

    // "Reconnect" with an unsaved file: refused, but with guidance (no "save first" since saving can't work).
    wb.update_in(cx, |w, window, cx| w.reconnect_lost_for_test(window, cx));
    let help = notice(&wb, cx);
    assert!(help.contains("Save a Copy to This Computer"), "{help:?}");
    assert!(!help.starts_with("Save or close"), "no dead-end advice while the connection is down: {help:?}");
}

/// Only the bridge drops (server alive): auto-reconnect, edit kept, save works afterwards.
#[gpui::test]
fn a_dropped_bridge_reconnects_and_the_edit_can_be_saved(cx: &mut TestAppContext) {
    let Some(sshd) = crate::test_sshd::start("err-bridge") else { return };
    let dir = project("err-bridge-repo");
    let (wb, cx) = ssh_workbench(cx, &dir);
    let ed = ssh_open_file(&wb, cx, &dir.join("src/main.rs"));
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "// kept\n", cx));

    drop_the_bridge_and_wait_for_the_reconnect(&wb, cx, &sshd);
    assert!(ed.read_with(cx, |e, _| e.is_dirty()), "the unsaved edit survived the reconnect");
    ed.update(cx, |e, cx| e.save_now(cx));
    wait_secs(cx, 30, "the save to reach the server", |cx| {
        tick(cx);
        std::fs::read_to_string(dir.join("src/main.rs")).is_ok_and(|t| t.starts_with("// kept"))
    });
    assert!(!ed.read_with(cx, |e, _| e.is_dirty()), "clean after a successful save");
}

/// A forwarded port survives a dropped bridge: it waits while reconnecting and comes back on the same local port
/// (the old spec is cancelled first, so the surviving shared connection does not keep a second copy).
#[gpui::test]
fn a_forwarded_port_comes_back_on_the_same_local_port_after_a_reconnect(cx: &mut TestAppContext) {
    use std::io::{Read, Write};
    let Some(sshd) = crate::test_sshd::start("err-fwd") else { return };
    let dir = project("err-fwd-repo");
    let (wb, cx) = ssh_workbench(cx, &dir);
    let app = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let remote = app.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in app.incoming().flatten() {
            let _ = s.write_all(b"still-here");
        }
    });
    wb.update(cx, |w, cx| w.forward_port(remote, cx));
    let local =
        |wb: &gpui::Entity<Workbench>, cx: &mut VisualTestContext| wb.read_with(cx, |w, _| w.forwards().iter().find(|f| f.remote == remote).map(|f| f.local));
    wait_secs(cx, 30, "the forward", |cx| {
        tick(cx);
        local(&wb, cx).is_some()
    });
    let before = local(&wb, cx).unwrap();

    drop_the_bridge_and_wait_for_the_reconnect(&wb, cx, &sshd);
    wait_secs(cx, 30, "the forward to come back", |cx| {
        tick(cx);
        local(&wb, cx).is_some()
    });
    assert_eq!(local(&wb, cx), Some(before), "the same local port");
    assert_eq!(wb.read_with(cx, |w, _| w.forwards().len()), 1, "no second forward");
    let mut got = String::new();
    std::net::TcpStream::connect(("127.0.0.1", before)).unwrap().read_to_string(&mut got).unwrap();
    assert_eq!(got, "still-here");
}

/// Language server comes back after a reconnect (a restart attempt during the outage must not
/// mark it permanently failed or raise a second error notification).
#[gpui::test]
fn the_language_server_comes_back_after_a_reconnect(cx: &mut TestAppContext) {
    let Some(sshd) = crate::test_sshd::start("err-lsp") else { return };
    let dir = project("err-lsp-repo");
    std::fs::write(dir.join("a.c"), "int main(void) { return 0; }\n").unwrap();
    let script = formatting_server(&dir);
    let settings = crate::settings::Settings { lsp_servers: c_server(&script), ..crate::settings::Settings::default() };
    let (wb, cx) = setup_with(cx, settings);
    wb.update_in(cx, |w, window, cx| w.connect(crate::test_sshd::ALIAS.into(), window, cx));
    wait_secs(cx, 30, "connected", |cx| {
        tick(cx);
        wb.read_with(cx, |w, _| w.ports_ready())
    });
    open(&wb, &dir, cx);
    let ed = ssh_open_file(&wb, cx, &dir.join("a.c"));
    wait_secs(cx, 30, "the language server to start", |cx| {
        tick(cx);
        ed.read_with(cx, |e, _| e.lsp_client().is_some())
    });

    drop_the_bridge_and_wait_for_the_reconnect(&wb, cx, &sshd);
    wait_secs(cx, 30, "the language server to come back", |cx| {
        tick(cx);
        ed.read_with(cx, |e, _| e.lsp_client().is_some())
    });
}

/// A file open on the server is deleted: the tab and content stay, the band says "deleted" (it is recreated on save).
#[gpui::test]
fn a_file_deleted_on_the_server_keeps_its_tab_and_says_so(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("err-deleted") else { return };
    let dir = project("err-deleted-repo");
    let (wb, cx) = ssh_workbench(cx, &dir);
    let ed = ssh_open_file(&wb, cx, &dir.join("src/lib.rs"));
    std::fs::remove_file(dir.join("src/lib.rs")).unwrap();
    wait_secs(cx, 30, "the deleted-on-server banner", |cx| {
        tick(cx);
        ed.read_with(cx, |e, _| e.status_for_test().as_deref() == Some("deleted"))
    });
    assert!(wb.read_with(cx, |w, cx| w.open_paths(cx)).iter().any(|p| p.ends_with("src/lib.rs")), "the tab stays open");
    assert!(ed.read_with(cx, |e, _| e.text().contains("// line 1")), "the content stays readable and copyable");
}

/// File changed on the server and edited locally: saving must not overwrite silently; a conflict band
/// appears and the server version is left as is.
#[gpui::test]
fn saving_over_a_file_changed_on_the_server_raises_a_conflict_instead_of_overwriting(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("err-conflict") else { return };
    let dir = project("err-conflict-repo");
    let (wb, cx) = ssh_workbench(cx, &dir);
    let ed = ssh_open_file(&wb, cx, &dir.join("src/lib.rs"));
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "// mine\n", cx));
    std::fs::write(dir.join("src/lib.rs"), "// theirs\n").unwrap();
    // A file-watch event may raise a band; what is actually tested is saving.
    ed.update(cx, |e, cx| e.save_now(cx));
    wait_secs(cx, 30, "the conflict banner", |cx| {
        tick(cx);
        ed.read_with(cx, |e, _| e.status_for_test().as_deref() == Some("conflict"))
    });
    assert_eq!(std::fs::read_to_string(dir.join("src/lib.rs")).unwrap(), "// theirs\n", "their version is untouched");
    assert!(ed.read_with(cx, |e, _| e.is_dirty()), "my edit is still in the tab");
}

/// Single surface: "Connect to SSH Host…" opens the workspace switcher (no separate server panel); a failed
/// connect there states the reason and "Reconnect" retries.
#[gpui::test]
fn connecting_by_name_from_the_switcher_reports_a_failure_with_its_reason(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("err-switcher-name") else { return };
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |w, window, cx| w.run_command_for_test("Remote: Connect to Host or Switch Workspace…", window, cx));
    cx.run_until_parked();
    let labels = wb.read_with(cx, |w, cx| w.overlay_labels(cx)).unwrap_or_default();
    assert!(labels.iter().any(|l| l == "Connect to SSH host…") && labels.iter().any(|l| l == "Open Folder…"), "{labels:?}");
    assert!(labels.iter().any(|l| l == crate::test_sshd::ALIAS), "the hosts of ~/.ssh/config are in the same list: {labels:?}");
    assert_eq!(wb.read_with(cx, |w, _| w.panel_for_test()), Some("explorer"), "no separate remote panel opened");
    cx.simulate_input("Connect to SSH");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_input("no-such-host.invalid");
    cx.simulate_keystrokes("enter");
    wait_secs(cx, 40, "the connection to fail", |cx| {
        tick(cx);
        conn(&wb, cx).starts_with("failed")
    });
    let said = notice(&wb, cx);
    assert!(said.starts_with("Could not connect to no-such-host.invalid:"), "{said:?}");
    // The same switcher offers to retry the failed connection.
    wb.update_in(cx, |w, window, cx| w.run_command_for_test("Remote: Connect to Host or Switch Workspace…", window, cx));
    cx.run_until_parked();
    let labels = wb.read_with(cx, |w, cx| w.overlay_labels(cx)).unwrap_or_default();
    assert!(labels.iter().any(|l| l == "Reconnect to no-such-host.invalid"), "{labels:?}");
    assert!(labels.iter().any(|l| l.starts_with("✖ ") || l == crate::test_sshd::ALIAS), "{labels:?}");
}

/// While connected, the switcher offers "Disconnect" and marks the server with ●; Disconnect returns to this computer.
#[gpui::test]
fn the_switcher_offers_disconnect_while_connected_and_returns_to_this_computer(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("err-switcher-disconnect") else { return };
    let dir = project("err-switcher-repo");
    let (wb, cx) = ssh_workbench(cx, &dir);
    wb.update_in(cx, |w, window, cx| w.run_command_for_test("Remote: Connect to Host or Switch Workspace…", window, cx));
    cx.run_until_parked();
    let labels = wb.read_with(cx, |w, cx| w.overlay_labels(cx)).unwrap_or_default();
    assert_eq!(labels.first().map(String::as_str), Some(format!("Disconnect from {}", crate::test_sshd::ALIAS).as_str()), "{labels:?}");
    assert!(labels.iter().any(|l| *l == format!("● {}", crate::test_sshd::ALIAS)), "{labels:?}");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(conn(&wb, cx), "local", "Disconnect returns to this computer");
}

/// An SSH recent that can no longer be reached: the failure is explained and the window asks whether to remove
/// it from Recent; "Remove from Recent" does.
#[gpui::test]
fn an_unreachable_ssh_recent_asks_to_be_removed(cx: &mut TestAppContext) {
    use crate::settings::Startup;
    let Some(_sshd) = crate::test_sshd::start("err-recent-ssh") else { return };
    let home = crate::qa_workbench_tests::fresh("home-recent-ssh");
    let (wb, cx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
    // Write an SSH folder into the history (as if it had once been connected to), then restart.
    wb.read_with(cx, |w, _| w.record_recent_for_test(Some("no-such-host.invalid"), "/srv/app"));
    let wb = crate::qa_workbench_tests::restart_with(wb, cx, &home, Startup::Welcome);
    assert!(wb.read_with(cx, |w, _| w.recent_for_test()).iter().any(|(h, p, _)| h.as_deref() == Some("no-such-host.invalid") && p == "/srv/app"));
    let r = state::RecentWorkspace { host: Some("no-such-host.invalid".into()), path: "/srv/app".into(), last_opened_at: 0 };
    wb.update_in(cx, |w, window, cx| w.open_recent(r, window, cx));
    wait_secs(cx, 40, "the failed connection to ask about the recent", |cx| {
        tick(cx);
        cx.has_pending_prompt()
    });
    assert!(conn(&wb, cx).starts_with("failed"));
    cx.simulate_prompt_answer("Remove from Recent");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |w, _| w.recent_for_test()).iter().all(|(h, _, _)| h.as_deref() != Some("no-such-host.invalid")));
}
