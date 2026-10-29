use super::*;
use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext, point};

/// A remote host with deterministic spawn/transfer responses and recorded PTY input and cleanup.
struct ImagePasteHost {
    local: Arc<dyn Host>,
    inputs: std::sync::Mutex<Vec<Vec<u8>>>,
    uploads: std::sync::Mutex<Vec<Vec<u8>>>,
    killed: std::sync::Mutex<Vec<u32>>,
    fail: bool,
}

impl ImagePasteHost {
    fn new(fail: bool) -> Arc<Self> {
        Arc::new(Self { local: Arc::new(host::local()), inputs: Default::default(), uploads: Default::default(), killed: Default::default(), fail })
    }
}

impl Host for ImagePasteHost {
    fn info(&self) -> &proto::HostInfo {
        self.local.info()
    }
    fn label(&self) -> String {
        "ssh://fixture".into()
    }
    fn is_remote(&self) -> bool {
        true
    }
    fn call(&self, request: proto::Request) -> host::Result<proto::Response> {
        self.local.call(request)
    }
    fn search(&self, q: proto::SearchQuery) -> host::Result<host::SearchStream> {
        self.local.search(q)
    }
    fn hash_tree(&self, r: &str, i: Vec<String>, g: bool, l: u32, o: Option<Vec<String>>) -> host::Result<host::HashStream> {
        self.local.hash_tree(r, i, g, l, o)
    }
    fn cancel(&self, id: u32) {
        self.local.cancel(id)
    }
    fn pty_spawn(&self, _: u16, _: u16, _: Option<String>, _: Option<String>) -> host::Result<host::Pty> {
        Ok(host::Pty { id: 7, events: std::sync::mpsc::channel().1 })
    }
    fn pty_input(&self, _: u32, data: Vec<u8>) {
        self.inputs.lock().unwrap().push(data);
    }
    fn pty_resize(&self, _: u32, _: u16, _: u16) {}
    fn pty_kill(&self, id: u32) {
        self.killed.lock().unwrap().push(id);
    }
    fn watch(&self, _: Vec<String>) {}
    fn fs_events(&self) -> std::sync::mpsc::Receiver<host::FsChanges> {
        self.local.fs_events()
    }
    fn status_events(&self) -> std::sync::mpsc::Receiver<host::ConnStatus> {
        self.local.status_events()
    }
    fn proc_spawn(&self, p: &str, a: Vec<String>, c: Option<String>) -> host::Result<host::Proc> {
        self.local.proc_spawn(p, a, c)
    }
    fn proc_input(&self, _: u32, _: Vec<u8>) {}
    fn proc_kill(&self, _: u32) {}
    fn exec(&self, _: &str, _: &[&str], _: Option<&str>, stdin: Option<Vec<u8>>) -> host::Result<host::ExecOutput> {
        self.uploads.lock().unwrap().push(stdin.unwrap());
        Ok(if self.fail {
            host::ExecOutput { code: Some(1), stdout: vec![], stderr: b"disk full".to_vec() }
        } else {
            host::ExecOutput { code: Some(0), stdout: b"/tmp/codeyard-paste-Abc123XY.png".to_vec(), stderr: vec![] }
        })
    }
}

#[gpui::test]
fn image_paste_survives_the_shell_finishing_startup_during_upload(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let host = ImagePasteHost::new(false);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(host.clone(), "test".into(), cx));
    term.update(cx, |term, cx| {
        term.starting = true;
        term.paste_image(gpui::Image::from_bytes(gpui::ImageFormat::Png, b"\x89PNG\r\n\x1a\nimage".to_vec()), cx);
        // The spawn reply arrives before the upload reply.
        term.pty = Some(7);
        term.starting = false;
        term.feed(b"\x1b[?2004h");
    });
    cx.run_until_parked();
    assert_eq!(*host.inputs.lock().unwrap(), vec![b"\x1b[200~/tmp/codeyard-paste-Abc123XY.png\x1b[201~".to_vec()]);
}

#[gpui::test]
fn closing_a_terminal_during_image_upload_suppresses_the_late_error(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let host = ImagePasteHost::new(true);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(host.clone(), "test".into(), cx));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let captured = errors.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&term, move |_, event: &TerminalEvent, _| {
            if let TerminalEvent::ImagePasteFailed(error) = event {
                captured.borrow_mut().push(error.clone());
            }
        })
    });
    term.update(cx, |term, cx| {
        term.pty = Some(7);
        term.paste_image(gpui::Image::from_bytes(gpui::ImageFormat::Png, b"\x89PNG\r\n\x1a\nimage".to_vec()), cx);
        term.kill();
    });
    cx.run_until_parked();
    assert!(host.inputs.lock().unwrap().is_empty());
    assert!(errors.borrow().is_empty(), "a closed terminal must not raise late upload notices");
}

#[gpui::test]
fn typing_before_the_shell_starts_is_delivered_after_startup(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let host = ImagePasteHost::new(false);
    let (_term, cx) = cx.add_window_view(|window, cx| {
        let mut term = TerminalView::new(host.clone(), None, None, cx);
        for key in ["x", "enter"] {
            term.on_key_down(&KeyDownEvent { keystroke: gpui::Keystroke::parse(key).unwrap(), is_held: false }, window, cx);
        }
        term
    });
    cx.run_until_parked();
    assert_eq!(*host.inputs.lock().unwrap(), vec![b"x\r".to_vec()]);
}

#[gpui::test]
fn an_open_terminal_receives_commands_queued_during_startup(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let host = ImagePasteHost::new(false);
    let _term = cx.new(|cx| {
        let mut term = TerminalView::new(host.clone(), None, None, cx);
        term.send("queued command");
        term
    });
    cx.run_until_parked();
    assert!(host.killed.lock().unwrap().is_empty());
    assert_eq!(*host.inputs.lock().unwrap(), vec![b"queued command".to_vec()]);
}

#[gpui::test]
fn closing_a_terminal_before_spawn_finishes_kills_the_late_shell(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let host = ImagePasteHost::new(false);
    let term = cx.new(|cx| {
        let mut term = TerminalView::new(host.clone(), None, None, cx);
        term.send("queued command");
        term.kill();
        term
    });
    cx.run_until_parked();
    assert_eq!(*host.killed.lock().unwrap(), vec![7]);
    assert!(host.inputs.lock().unwrap().is_empty(), "a cancelled shell must not receive queued commands");
    assert!(term.read_with(cx, |term, _| term.pty.is_none()));
}

#[gpui::test]
fn dropping_a_terminal_before_spawn_finishes_kills_the_late_shell(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let host = ImagePasteHost::new(false);
    let term = cx.new(|cx| TerminalView::new(host.clone(), None, None, cx));
    drop(term);
    cx.run_until_parked();
    assert_eq!(*host.killed.lock().unwrap(), vec![7]);
}

#[gpui::test]
fn ctrl_v_transfers_a_remote_clipboard_image_as_bracketed_path(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let host = ImagePasteHost::new(false);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(host.clone(), "test".into(), cx));
    term.update_in(cx, |t, window, cx| {
        t.pty = Some(7);
        t.feed(b"\x1b[?2004h");
        t.focus_handle.focus(window);
        cx.notify();
    });
    let data = b"\x89PNG\r\n\x1a\nimage".to_vec();
    let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, data.clone());
    cx.write_to_clipboard(ClipboardItem::new_image(&image));
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-v");
    cx.run_until_parked();
    assert_eq!(*host.uploads.lock().unwrap(), vec![data]);
    assert_eq!(*host.inputs.lock().unwrap(), vec![b"\x1b[200~/tmp/codeyard-paste-Abc123XY.png\x1b[201~".to_vec()]);
}

#[cfg(unix)]
#[gpui::test]
fn clipboard_image_reaches_a_real_ssh_terminal_program(cx: &mut TestAppContext) {
    use std::time::{Duration, Instant};

    let Some(sshd) = crate::test_sshd::start("terminal-image-paste") else { return };
    let host: Arc<dyn Host> = Arc::new(host::ssh::connect(crate::test_sshd::ALIAS, &crate::bundled_agents::discover(), |_| {}).unwrap());
    let destination = sshd.dir.join("pasted-image.png");
    let script = "stty -echo; printf '__IMAGE_READY__\\n'; IFS= read -r image; \
        case \"$image\" in /tmp/codeyard-paste-????????.png) ;; *) exit 17;; esac; \
        trap 'rm -f -- \"$image\"' EXIT; cat -- \"$image\" > \"$1\" && printf '__IMAGE_SAVED__\\n'";
    let pty = host
        .pty_spawn_ex(host::PtySpec {
            cols: 80,
            rows: 24,
            cwd: Some(crate::ui::local_path(&sshd.dir)),
            shell: Some("/bin/sh".into()),
            args: Some(vec!["-c".into(), script.into(), "codeyard-image-fixture".into(), crate::ui::local_path(&destination)]),
            ..Default::default()
        })
        .unwrap();
    let wait_for = |marker: &[u8]| {
        let deadline = Instant::now() + Duration::from_secs(10 * crate::test_sshd::timeout_scale());
        let mut output = Vec::new();
        while !output.windows(marker.len()).any(|part| part == marker) {
            assert!(Instant::now() < deadline, "remote image fixture did not report {}", String::from_utf8_lossy(marker));
            match pty.events.recv_timeout(Duration::from_millis(100)) {
                Ok(PtyEvent::Output(data)) => output.extend(data),
                Ok(PtyEvent::Exit(code)) => panic!("remote image fixture exited early: {code:?}: {}", String::from_utf8_lossy(&output)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("remote image fixture disconnected: {error}"),
            }
        }
    };
    wait_for(b"__IMAGE_READY__");
    crate::qa_workbench_tests::globals(cx);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(host.clone(), "owned SSH image fixture".into(), cx));
    term.update_in(cx, |term, window, _| {
        term.pty = Some(pty.id);
        term.focus_handle.focus(window);
    });
    let data = b"\x89PNG\r\n\x1a\nowned image payload".to_vec();
    let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, data.clone());
    cx.write_to_clipboard(ClipboardItem::new_image(&image));
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-v");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_for(b"__IMAGE_SAVED__");
    let saved = host.exec("cat", &["--", &crate::ui::local_path(&destination)], None, None).unwrap();
    assert!(saved.ok());
    assert_eq!(saved.stdout, data);
    term.update(cx, |term, _| term.pty = None);
    host.pty_kill(pty.id);
}

#[gpui::test]
fn remote_image_transfer_failure_reports_error_without_pasting_a_path(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let host = ImagePasteHost::new(true);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(host.clone(), "test".into(), cx));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let captured = errors.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&term, move |_, event: &TerminalEvent, _| {
            if let TerminalEvent::ImagePasteFailed(error) = event {
                captured.borrow_mut().push(error.clone());
            }
        })
    });
    term.update_in(cx, |t, window, cx| {
        t.pty = Some(7);
        t.focus_handle.focus(window);
        cx.notify();
    });
    let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, b"\x89PNG\r\n\x1a\nimage".to_vec());
    cx.write_to_clipboard(ClipboardItem::new_image(&image));
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-v");
    cx.run_until_parked();
    assert!(host.inputs.lock().unwrap().is_empty());
    assert!(errors.borrow().iter().any(|error| error.contains("disk full")));
}

#[gpui::test]
fn ctrl_v_with_text_still_reaches_the_remote_program(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let host = ImagePasteHost::new(false);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(host.clone(), "test".into(), cx));
    term.update_in(cx, |t, window, cx| {
        t.pty = Some(7);
        t.focus_handle.focus(window);
        cx.notify();
    });
    cx.write_to_clipboard(ClipboardItem::new_string("text".into()));
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-v");
    cx.run_until_parked();
    assert!(host.uploads.lock().unwrap().is_empty());
    assert_eq!(*host.inputs.lock().unwrap(), vec![vec![22]]);
}

#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn ctrl_c_copies_a_terminal_selection(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(Arc::new(host::local()), "test".into(), cx));
    term.update_in(cx, |t, window, cx| {
        t.feed(b"hello");
        let mut selection = Selection::new(SelectionType::Simple, GridPoint::new(Line(0), Column(0)), Side::Left);
        selection.update(GridPoint::new(Line(0), Column(4)), Side::Right);
        t.term.selection = Some(selection);
        t.focus_handle.focus(window);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(term.read_with(cx, |t, _| t.selected_text()).as_deref(), Some("hello"));
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("hello"));
}

/// Claude Code and Codex erase and rewrite their lines on every frame; the selection must survive to be copied.
#[gpui::test]
fn a_selection_survives_a_tui_redrawing_its_lines(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(Arc::new(host::local()), "test".into(), cx));
    term.update(cx, |t, _| {
        t.feed(b"hello\r\nspinner |");
        let mut selection = Selection::new(SelectionType::Simple, GridPoint::new(Line(0), Column(0)), Side::Left);
        selection.update(GridPoint::new(Line(0), Column(4)), Side::Right);
        t.term.selection = Some(selection);
        // A frame: cursor up, erase the lines, write them again.
        t.feed(b"\x1b[1A\r\x1b[2Khello\r\n\x1b[2Kspinner /");
    });
    assert_eq!(term.read_with(cx, |t, _| t.selected_text()).as_deref(), Some("hello"));
    // Leaving for the alternate screen still drops it: the cells it named are gone.
    term.update(cx, |t, _| t.feed(b"\x1b[?1049h"));
    assert_eq!(term.read_with(cx, |t, _| t.selected_text()), None);
}

/// Small trackpad events must add up, rather than each being rounded away.
#[gpui::test]
fn terminal_wheel_preserves_small_trackpad_movements(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(Arc::new(host::local()), "test".into(), cx));
    term.update(cx, |t, cx| {
        t.feed("line\r\n".repeat(200).as_bytes());
        cx.notify();
    });
    cx.run_until_parked();
    let line_h = term.read_with(cx, |t, _| t.line_h());
    for _ in 0..8 {
        term.update_in(cx, |t, window, cx| {
            t.on_scroll(
                &ScrollWheelEvent {
                    position: t.cell_center(5, 5),
                    delta: ScrollDelta::Pixels(point(px(0.), line_h / 8.)),
                    modifiers: Modifiers::none(),
                    touch_phase: TouchPhase::Moved,
                },
                window,
                cx,
            );
        });
    }
    assert!(term.read_with(cx, |t, _| t.term.grid().display_offset()) >= 1, "a line of trackpad motion must scroll the terminal");
}

/// Screen row whose text is `text`.
fn row_of(t: &TerminalView, text: &str) -> usize {
    t.screen_text().lines().position(|l| l == text).unwrap_or_else(|| panic!("{text:?} on screen"))
}

/// Select columns `from..=to` of screen row `row` directly.
fn select(t: &mut TerminalView, row: usize, from: usize, to: usize) {
    let line = Line(row as i32 - t.term.grid().display_offset() as i32);
    let mut selection = Selection::new(SelectionType::Simple, GridPoint::new(line, Column(from)), Side::Left);
    selection.update(GridPoint::new(line, Column(to)), Side::Right);
    t.term.selection = Some(selection);
}

/// A redraw that also scrolls (Claude Code adding a message below) keeps the selection on its text; when the text itself
/// changes, the selection goes.
#[gpui::test]
fn a_selection_follows_its_text_when_a_redraw_scrolls(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(Arc::new(host::local()), "test".into(), cx));
    cx.run_until_parked();
    term.update(cx, |t, _| {
        let rows = t.size.rows;
        for i in 0..rows + 5 {
            t.feed(format!("line {i}\r\n").as_bytes());
        }
        let row = row_of(t, "line 12");
        select(t, row, 0, 6);
        // One frame: rewrite the selected line in place, then two new lines at the bottom scroll everything up.
        t.feed(format!("\x1b[{};1H\x1b[2Kline 12\x1b[{rows};1H\r\nnew a\r\nnew b", row + 1).as_bytes());
        assert_eq!(t.selected_text().as_deref(), Some("line 12"));
        assert_eq!(t.term.selection.as_ref().and_then(|s| s.to_range(&t.term)).map(|r| r.start.line), Some(Line(row as i32 - 2)), "two lines up");
        let row = row_of(t, "line 12");
        t.feed(format!("\x1b[{};1H\x1b[2Kline 99", row + 1).as_bytes());
        assert_eq!(t.selected_text(), None, "the text it named is gone");
    });
}

/// `terminal.copyOnSelection`: "agents" copies only while Claude Code or Codex runs, "always" copies every selection.
#[gpui::test]
fn a_mouse_selection_is_copied_while_an_agent_runs(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(Arc::new(host::local()), "zsh".into(), cx));
    term.update(cx, |t, _| t.feed(b"hello world"));
    cx.run_until_parked();
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
    let drag = |cx: &mut VisualTestContext| {
        let (from, to) = term.read_with(cx, |t, _| (t.cell_center(0, 0), t.cell_center(0, 4)));
        let (from, to) = (point(from.x - px(1.), from.y), point(to.x + px(1.), to.y));
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        cx.read_from_clipboard().and_then(|c| c.text())
    };
    assert_eq!(drag(cx).as_deref(), Some("before"), "a plain shell keeps Ctrl+C for copying");
    term.update(cx, |t, _| t.title = "✳ Claude Code".into());
    assert_eq!(drag(cx).as_deref(), Some("hello"));
    term.update(cx, |t, _| t.title = "zsh".into());
    cx.update(|_, cx| cx.update_global::<crate::settings::Settings, _>(|s, _| s.terminal_copy_on_selection = "always".into()));
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
    assert_eq!(drag(cx).as_deref(), Some("hello"));
}

#[test]
fn agents_are_known_by_their_program_name() {
    for name in ["claude", "Codex", "claude.exe", "codex.cmd"] {
        assert!(selection::is_agent(name), "{name}");
    }
    for name in ["node", "claudette", "zsh", ""] {
        assert!(!selection::is_agent(name), "{name}");
    }
}

/// A drag that goes to a program reading the mouse says that Shift selects.
#[gpui::test]
fn a_drag_into_a_mouse_program_hints_at_shift(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (term, cx) = cx.add_window_view(|_, cx| TerminalView::output_only(Arc::new(host::local()), "test".into(), cx));
    term.update(cx, |t, _| t.feed(b"\x1b[?1000hhello"));
    cx.run_until_parked();
    let (from, to) = term.read_with(cx, |t, _| (t.cell_center(0, 0), t.cell_center(0, 4)));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    assert!(term.read_with(cx, |t, _| t.shift_hint));
    assert_eq!(term.read_with(cx, |t, _| t.selected_text()), None, "the drag went to the program");
}
