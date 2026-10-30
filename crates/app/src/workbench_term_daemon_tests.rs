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

/// The window closes on purpose: its terminal daemon session ends.
fn close(wb: Entity<Workbench>, cx: &mut gpui::VisualTestContext) {
    drop(wb);
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
}

/// The app quits (what `on_app_quit` does to every window, then the windows go): the session stays for the next start.
fn quit(wb: Entity<Workbench>, cx: &mut gpui::VisualTestContext) {
    wb.update(cx, |wb, cx| wb.keep_terminals_for_restart(cx));
    close(wb, cx);
}

fn alive(pid: &str) -> bool {
    // SAFETY: signal 0 only checks whether the process exists.
    unsafe { libc::kill(pid.parse().unwrap(), 0) == 0 }
}

/// A window with one terminal on the daemon, and its shell's pid.
fn window_with_terminal<'a>(
    cx: &'a mut gpui::TestAppContext,
    home: &std::path::Path,
    dir: &std::path::Path,
    sock: &std::path::Path,
) -> (Entity<Workbench>, &'a mut gpui::VisualTestContext, String) {
    let (wb, vcx) = window(cx, home, dir, sock);
    wb.update_in(vcx, |wb, window, cx| wb.new_terminal_launch(None, crate::terminal::Launch::default(), window, cx));
    wait_ticking(vcx, "its token is saved", |cx| wb.read_with(cx, |wb, cx| wb.terminals[0].read(cx).token().is_some()));
    let pid = shell_pid(&wb, vcx, &dir.join("pid-1"));
    (wb, vcx, pid)
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
        quit(wb, vcx);
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
        quit(wb, vcx);
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

/// Closing the window (or leaving the folder) is deliberate: the shell ends at once, as without the daemon.
#[gpui::test]
fn closing_the_window_or_the_folder_ends_its_terminals_at_once(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("termd-close-home");
    let dir = crate::qa_workbench_tests::project("termd-close");
    let other = crate::qa_workbench_tests::project("termd-close-other");
    let sock = daemon("close");
    let (wb, vcx, pid) = window_with_terminal(cx, &home, &dir, &sock);
    close(wb, vcx);
    wait_ticking(vcx, "the closed window's shell ended", |_| !alive(&pid));

    // Opening another folder in the window ends the first folder's terminals too.
    let (wb, vcx, pid) = window_with_terminal(cx, &home, &dir, &sock);
    crate::qa_workbench_tests::open(&wb, &other, vcx);
    wait_ticking(vcx, "the left folder's shell ended", |_| !alive(&pid));
}

/// Another Codeyard instance holds the window's session: the window does not take it over. It starts new shells and
/// says why, and the other instance keeps its terminal.
#[gpui::test]
fn a_session_held_by_another_instance_is_not_taken_over(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("termd-held-home");
    let dir = crate::qa_workbench_tests::project("termd-held");
    let sock = daemon("held");
    let (wb, vcx, pid) = window_with_terminal(cx, &home, &dir, &sock);
    let session = wb.read_with(vcx, |wb, _| wb.persist.term_daemon.as_ref().unwrap().session.clone());
    quit(wb, vcx);
    // "Another instance" takes the session (as it would after its own restart) and keeps its connection.
    let stream = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    let closer = stream.try_clone().unwrap();
    let transport = host::Transport::pipes(Box::new(stream.try_clone().unwrap()), Box::new(stream));
    let close_other = Box::new(move || drop(closer.shutdown(std::net::Shutdown::Both)));
    let (other, resumed) = host::RemoteHost::attach_session(transport, close_other, &session, "Local").unwrap();
    assert!(resumed);

    let (wb, cx) = window(cx, &home, &dir, &sock);
    wait_ticking(cx, "a new shell", |cx| wb.read_with(cx, |wb, cx| wb.terminals.len() == 1 && wb.terminals[0].read(cx).pty_handle().is_some()));
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default();
    assert_eq!(notice, "Terminal session could not be restored: another Codeyard window has them; started new shells");
    assert_ne!(wb.read_with(cx, |wb, _| wb.persist.term_daemon.as_ref().map(|d| d.session.clone())), Some(session), "a session of its own");
    assert!(alive(&pid), "the other instance keeps its terminal");
    other.end_session();
}

/// The pid a terminal's shell writes to `file`.
fn pid_of(term: &Entity<crate::terminal::TerminalView>, cx: &mut gpui::VisualTestContext, file: &std::path::Path) -> String {
    let line = format!("echo $$ > {}\n", file.display());
    term.update(cx, |t, _| t.send(&line));
    wait_ticking(cx, "the shell wrote its pid", |_| std::fs::read_to_string(file).is_ok_and(|s| s.ends_with('\n')));
    std::fs::read_to_string(file).unwrap().trim().to_string()
}

/// Every window of the app goes, as at quit (the main one marked to keep its terminals first).
fn quit_all(wb: Entity<Workbench>, cx: &mut gpui::VisualTestContext) {
    wb.update(cx, |wb, cx| {
        wb.persist_session(cx);
        wb.keep_terminals_for_restart(cx);
    });
    drop(wb);
    cx.update(|window, _| window.remove_window());
    // The terminal windows go too, as every window does when the app quits.
    for w in cx.cx.windows() {
        let _ = gpui::AppContext::update_window(&mut cx.cx, w, |_, window, _| window.remove_window());
    }
    cx.run_until_parked();
}

/// A terminal moved into an editor group and one in a window of its own come back to their places as the same shells,
/// next to the panel's own.
#[gpui::test]
fn terminals_outside_the_panel_are_taken_back_into_their_places(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("termd-moved-home");
    let dir = crate::qa_workbench_tests::project("termd-moved");
    let sock = daemon("moved");
    let (in_group, in_window, in_panel) = {
        let (wb, vcx) = window(cx, &home, &dir, &sock);
        for _ in 0..3 {
            wb.update_in(vcx, |wb, window, cx| wb.new_terminal_launch(None, crate::terminal::Launch::default(), window, cx));
        }
        wait_ticking(vcx, "their tokens", |cx| wb.read_with(cx, |wb, cx| wb.terminals.iter().all(|t| t.read(cx).token().is_some())));
        let terms = wb.read_with(vcx, |wb, _| wb.terminals.clone());
        let pids =
            (pid_of(&terms[0], vcx, &dir.join("pid-group")), pid_of(&terms[1], vcx, &dir.join("pid-window")), pid_of(&terms[2], vcx, &dir.join("pid-panel")));
        // The second goes to a window of its own, the first into the editor area; the third stays in the panel.
        wb.update_in(vcx, |wb, window, cx| {
            wb.active_terminal = 1;
            wb.detach_terminal(window, cx);
        });
        wait_ticking(vcx, "the terminal window", |cx| wb.read_with(cx, |wb, _| wb.chrome.detached.len() == 1));
        wb.update_in(vcx, |wb, window, cx| {
            wb.active_terminal = 0;
            wb.move_terminal_into_editor(window, cx);
        });
        wait_ticking(vcx, "in the editor area", |cx| wb.read_with(cx, |wb, _| wb.editor_terminal_count() == 1 && wb.terminals.len() == 1));
        quit_all(wb, vcx);
        pids
    };
    let (wb, cx) = window(cx, &home, &dir, &sock);
    wait_ticking(cx, "all back in their places", |cx| {
        wb.read_with(cx, |wb, _| wb.editor_terminal_count() == 1 && wb.chrome.detached.len() == 1 && wb.terminals.len() == 1)
    });
    let (group_term, window_term, panel_term) =
        wb.read_with(cx, |wb, _| (wb.editor_terminals_for_test()[0].clone(), wb.chrome.detached[0].term.clone(), wb.terminals[0].clone()));
    for term in [&group_term, &window_term, &panel_term] {
        let shown = cx.update(|_, cx| term.read(cx).scrollback_text() + &term.read(cx).screen_text());
        assert!(shown.contains("Reconnected to terminal session") && !shown.contains("Previous session ended"), "{shown}");
    }
    assert_eq!(pid_of(&group_term, cx, &dir.join("pid-group-2")), in_group, "the editor group's terminal is the same shell");
    assert_eq!(pid_of(&window_term, cx, &dir.join("pid-window-2")), in_window, "the window's terminal is the same shell");
    assert_eq!(pid_of(&panel_term, cx, &dir.join("pid-panel-2")), in_panel, "the panel's terminal is the same shell");
}

/// `terminal.persistentSessionTimeout: 0`: quitting does not keep the terminals; the next start has a new shell.
#[gpui::test]
fn a_timeout_of_zero_does_not_keep_terminals(cx: &mut gpui::TestAppContext) {
    let home = crate::qa_workbench_tests::fresh("termd-zero-home");
    let dir = crate::qa_workbench_tests::project("termd-zero");
    let sock = daemon("zero");
    let (wb, vcx, pid) = window_with_terminal(cx, &home, &dir, &sock);
    vcx.update(|_, cx| {
        let mut s = cx.global::<Settings>().clone();
        s.terminal_persistent_session_timeout = 0;
        cx.set_global(s);
    });
    quit(wb, vcx);
    wait_ticking(vcx, "the shell ended with the app", |_| !alive(&pid));
}

/// A window of the SSH folder `path` on the test host, connected with the folder's saved session if there is one.
fn ssh_window<'a>(cx: &'a mut gpui::TestAppContext, home: &std::path::Path, path: &str) -> (Entity<Workbench>, &'a mut gpui::VisualTestContext) {
    let (wb, vcx) = crate::qa_workbench_tests::setup_persistent(cx, home);
    restore_on(vcx);
    let p = path.to_string();
    wb.update_in(vcx, |wb, window, cx| {
        wb.remote.pending_path = Some(p);
        wb.connect(crate::test_sshd::ALIAS.into(), window, cx);
    });
    crate::qa_integration_tests::wait_secs(vcx, 60, "connected", |cx| wb.read_with(cx, |wb, cx| wb.remote.host.is_remote() && wb.root(cx).is_some()));
    (wb, vcx)
}

/// An SSH folder: after a restart, the window resumes its session on the host's agent daemon and takes its terminal
/// back, the same process; the window it replaced was gone (no `Bye`), so the session was kept.
#[gpui::test]
fn an_ssh_folders_terminal_is_taken_back_after_a_restart(cx: &mut gpui::TestAppContext) {
    let Some(sshd) = crate::test_sshd::start("term-reattach-ssh") else { return };
    let home = crate::qa_workbench_tests::fresh("termd-ssh-home");
    let dir = sshd.dir.join("project");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let first = {
        let (wb, vcx) = ssh_window(cx, &home, &path);
        wb.update_in(vcx, |wb, window, cx| wb.new_terminal_launch(None, crate::terminal::Launch::default(), window, cx));
        crate::qa_integration_tests::wait_secs(vcx, 30, "its token", |cx| {
            wb.read_with(cx, |wb, cx| wb.terminals.first().is_some_and(|t| t.read(cx).token().is_some()))
        });
        let term = wb.read_with(vcx, |wb, _| wb.terminals[0].clone());
        let pid = pid_of(&term, vcx, &dir.join("pid-1"));
        term.update(vcx, |t, _| t.send("sleep 1; echo printed-while-away\n"));
        // The command reached the host (the shell echoed it) before the app goes.
        crate::qa_integration_tests::wait_secs(vcx, 30, "the command echoed", |cx| {
            cx.update(|_, cx| term.read(cx).scrollback_text() + &term.read(cx).screen_text()).contains("sleep 1")
        });
        // Nothing of the window may outlive it here: its connection must go as a quitting app's does.
        drop(term);
        quit_all(wb, vcx);
        pid
    };
    std::thread::sleep(std::time::Duration::from_secs(1));
    let (wb, cx) = ssh_window(cx, &home, &path);
    crate::qa_integration_tests::wait_secs(cx, 30, "the terminal taken back", |cx| text(&wb, cx).contains("printed-while-away"));
    let shown = text(&wb, cx);
    assert!(shown.contains("Reconnected to terminal session") && !shown.contains("Previous session ended"), "{shown}");
    let term = wb.read_with(cx, |wb, _| wb.terminals[0].clone());
    assert_eq!(pid_of(&term, cx, &dir.join("pid-2")), first, "the same shell process on the host");
}

/// The host side of an SSH restart, without a window: a connection kept for the next start (no `Bye`) leaves its shell
/// and that shell's output to the host's agent daemon; a new connection to the same session adopts the shell with its
/// token and gets what it printed meanwhile.
#[test]
fn an_ssh_session_kept_for_a_restart_hands_its_shell_to_the_next_connection() {
    let Some(sshd) = crate::test_sshd::start("term-adopt-ssh") else { return };
    let bundle = crate::bundled_agents::discover();
    let session = host::random_token();
    let marker = sshd.dir.join("adopt-marker");
    let read_until = |pty: &host::Pty, want: &str| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut seen = String::new();
        while !seen.contains(want) {
            assert!(std::time::Instant::now() < deadline, "waited for {want:?}, got {seen:?}");
            if let Ok(host::PtyEvent::Output(d)) = pty.events.recv_timeout(std::time::Duration::from_millis(200)) {
                seen.push_str(&String::from_utf8_lossy(&d));
            }
        }
    };
    let spec = || host::PtySpec { cols: 80, rows: 24, shell: Some("/bin/sh".into()), ..Default::default() };
    let (id, token) = {
        let first = host::ssh::connect_session(crate::test_sshd::ALIAS, &bundle, Some(session.clone()), |_| {}).unwrap();
        let pty = first.pty_spawn_ex(spec()).unwrap();
        let token = first.pty_token(pty.id).unwrap();
        first.pty_input(pty.id, format!("echo $$ > {}; sleep 1; echo printed-while-away\n", marker.display()).into_bytes());
        read_until(&pty, "sleep 1");
        first.keep_for(300);
        first.keep_session();
        (pty.id, token)
    };
    std::thread::sleep(std::time::Duration::from_secs(2));
    let second = host::ssh::connect_session(crate::test_sshd::ALIAS, &bundle, Some(session), |_| {}).unwrap();
    let (back, _) = second.pty_adopt(id, token.0, token.1).unwrap();
    read_until(&back, "printed-while-away");
    let pid = std::fs::read_to_string(&marker).unwrap().trim().to_string();
    second.pty_input(id, b"echo still-the-same-$$\n".to_vec());
    read_until(&back, &format!("still-the-same-{pid}"));
}
