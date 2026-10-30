//! Terminals that outlive the window: a window's terminal runs on a terminal daemon (here on a thread of the test, as
//! the real one is a process of its own), the window goes away as when the app quits or is killed, and the next window
//! of that folder takes the same shell back; a token that does not match brings a new shell instead.

use super::*;
use crate::qa_integration_tests::wait_ticking;

fn restore_on(cx: &mut gpui::VisualTestContext) {
    cx.update(|_, cx| {
        let mut s = cx.global::<Settings>().clone();
        s.restore_session = true;
        s.terminal_persistent_scrollback = 0;
        cx.set_global(s);
    });
}

fn text(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> String {
    wb.read_with(cx, |wb, cx| wb.terminals.first().map(|t| t.read(cx).scrollback_text()).unwrap_or_default())
}

fn daemon(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("codeyard-app-termd-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sock = dir.join("d.sock");
    let s2 = sock.clone();
    std::thread::spawn(move || agent::daemon::run(&s2));
    sock
}

/// A window of `dir` on the terminal daemon `sock`, with its terminals restored.
fn window<'a>(
    cx: &'a mut gpui::TestAppContext,
    home: &std::path::Path,
    dir: &std::path::Path,
    sock: &std::path::Path,
) -> (Entity<Workbench>, &'a mut gpui::VisualTestContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup_persistent(cx, home);
    restore_on(cx);
    let sock = sock.to_path_buf();
    wb.update(cx, |wb, _| wb.use_terminal_daemon_for_test(sock));
    crate::qa_workbench_tests::open(&wb, dir, cx);
    wait_ticking(cx, "the window's daemon session", |cx| wb.read_with(cx, |wb, _| wb.persist.term_daemon.is_some()));
    (wb, cx)
}

/// The pid the shell writes to `file`.
fn shell_pid(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext, file: &std::path::Path) -> String {
    let line = format!("echo $$ > {}\n", file.display());
    wb.update(cx, |wb, cx| wb.terminals[0].update(cx, |t, _| t.send(&line)));
    wait_ticking(cx, "the shell wrote its pid", |_| std::fs::read_to_string(file).is_ok_and(|s| s.ends_with('\n')));
    std::fs::read_to_string(file).unwrap().trim().to_string()
}

fn close(wb: Entity<Workbench>, cx: &mut gpui::VisualTestContext) {
    drop(wb);
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
}

#[gpui::test]
fn a_terminal_outlives_its_window_and_the_next_window_takes_the_same_shell_back(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("termd-home");
    let dir = crate::qa_workbench_tests::project("termd");
    let sock = daemon("reattach");
    let first = {
        let (wb, vcx) = window(cx, &home, &dir, &sock);
        wb.update_in(vcx, |wb, window, cx| wb.new_terminal_launch(None, crate::terminal::Launch::default(), window, cx));
        wait_ticking(vcx, "its token is saved", |cx| wb.read_with(cx, |wb, cx| wb.terminals[0].read(cx).token().is_some()));
        let pid = shell_pid(&wb, vcx, &dir.join("pid-1"));
        // Output while no window is attached is kept for the next one.
        wb.update(vcx, |wb, cx| wb.terminals[0].update(cx, |t, _| t.send("sleep 0.5; echo printed-while-closed\n")));
        close(wb, vcx);
        pid
    };
    std::thread::sleep(std::time::Duration::from_millis(900));

    let (wb, cx) = window(cx, &home, &dir, &sock);
    wait_ticking(cx, "the terminal taken back", |cx| wb.read_with(cx, |wb, _| wb.terminals.len() == 1));
    wait_ticking(cx, "the buffered output", |cx| text(&wb, cx).contains("printed-while-closed"));
    let shown = text(&wb, cx);
    assert!(shown.contains("Reconnected to terminal session") && !shown.contains("Previous session ended"), "{shown}");
    assert!(wb.read_with(cx, |wb, _| wb.notice_for_test()).is_some_and(|(_, t)| t == "Reconnected to terminal session"), "announced");
    assert_eq!(shell_pid(&wb, cx, &dir.join("pid-2")), first, "the same shell process");
}

#[gpui::test]
fn a_token_that_does_not_match_brings_a_new_shell_and_ends_the_old_one(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("termd-stale-home");
    let dir = crate::qa_workbench_tests::project("termd-stale");
    let sock = daemon("stale");
    let first = {
        let (wb, vcx) = window(cx, &home, &dir, &sock);
        wb.update_in(vcx, |wb, window, cx| wb.new_terminal_launch(None, crate::terminal::Launch::default(), window, cx));
        wait_ticking(vcx, "its token is saved", |cx| wb.read_with(cx, |wb, cx| wb.terminals[0].read(cx).token().is_some()));
        let pid = shell_pid(&wb, vcx, &dir.join("pid-1"));
        // The saved secret is replaced (a stale or forged token).
        wb.update(vcx, |wb, cx| {
            let key = saved_terminals_key(wb.persist.workspace_id.unwrap());
            let s = wb.persist.state.as_ref().unwrap();
            let mut saved = crate::terminal_session::SavedTerminals::parse(&s.get(&key).unwrap(), 0).unwrap();
            saved.terminals[0].token.as_mut().unwrap().secret = "0".repeat(32);
            let _ = s.set(&key, &saved.to_json().unwrap());
            let _ = cx;
        });
        close(wb, vcx);
        pid
    };
    let (wb, cx) = window(cx, &home, &dir, &sock);
    wait_ticking(cx, "a new shell", |cx| wb.read_with(cx, |wb, cx| wb.terminals.len() == 1 && wb.terminals[0].read(cx).pty_handle().is_some()));
    let shown = text(&wb, cx);
    assert!(shown.contains("Previous session ended; started a new shell") && !shown.contains("Reconnected"), "{shown}");
    assert_ne!(shell_pid(&wb, cx, &dir.join("pid-2")), first, "not the old process");
    // The old shell was in the session but no window can reach it: it is ended.
    let old: i32 = first.parse().unwrap();
    wait_ticking(cx, "the unreachable shell ended", |_| {
        // SAFETY: signal 0 only checks whether the process exists.
        unsafe { libc::kill(old, 0) != 0 }
    });
}
