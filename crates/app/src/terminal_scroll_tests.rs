use super::*;
use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, point};

/// A remote host with a deterministic transfer response and recorded PTY input.
struct ImagePasteHost {
    local: Arc<dyn Host>,
    inputs: std::sync::Mutex<Vec<Vec<u8>>>,
    uploads: std::sync::Mutex<Vec<Vec<u8>>>,
    fail: bool,
}

impl ImagePasteHost {
    fn new(fail: bool) -> Arc<Self> {
        Arc::new(Self { local: Arc::new(host::local()), inputs: Default::default(), uploads: Default::default(), fail })
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
    fn pty_spawn(&self, c: u16, r: u16, d: Option<String>, s: Option<String>) -> host::Result<host::Pty> {
        self.local.pty_spawn(c, r, d, s)
    }
    fn pty_input(&self, _: u32, data: Vec<u8>) {
        self.inputs.lock().unwrap().push(data);
    }
    fn pty_resize(&self, _: u32, _: u16, _: u16) {}
    fn pty_kill(&self, _: u32) {}
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
