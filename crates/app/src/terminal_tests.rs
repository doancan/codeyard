use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

use super::*;
use crate::ui::text;

#[test]
fn the_terminal_font_size_setting_sets_the_cell_size() {
    let (_, base) = metrics(text::CODE.0, LINE_HEIGHT);
    assert_eq!(base, (text::CODE.0 * LINE_HEIGHT).round(), "rows are 1.2 × the font size by default");
    let (size, line) = metrics(20., LINE_HEIGHT);
    assert_eq!((size, line), (20., 24.));
    assert!(line > base && line.fract() == 0., "a bigger font gets taller, whole-pixel rows: {line}");
    assert_eq!(metrics(2., LINE_HEIGHT).0, 6., "absurd sizes are clamped");
    assert_eq!(metrics(500., LINE_HEIGHT).0, 48.);
    assert_eq!(metrics(13., 1.5).1, 20., "terminal.lineHeight widens the rows");
    assert_eq!(metrics(13., 0.2).1, 13., "never shorter than the font");
    assert_eq!(metrics(13., f32::NAN).1, 16., "a broken value falls back to the default");
}

#[test]
fn a_terminal_that_cannot_keep_up_drops_output_instead_of_growing_without_bound() {
    let b = OutputBacklog::default();
    assert!(b.admit(MAX_BACKLOG - 10));
    assert!(b.admit(10), "exactly at the limit is fine");
    assert!(!b.admit(1), "over the limit is dropped");
    assert!(!b.admit(1_000_000));
    assert!(b.take_dropped(), "the user is told once");
    assert!(!b.take_dropped());
    // After the UI catches up, output flows again.
    b.consumed(MAX_BACKLOG);
    assert!(b.admit(4096));
}

/// Fault injection, slow consumer, through the real gate: 256 MB of output arrives while the UI reads nothing. At most
/// `MAX_BACKLOG` bytes are held, the UI is told output was dropped, and the exit event still arrives after the output.
#[test]
fn a_flood_behind_a_stalled_terminal_is_capped_and_the_exit_is_kept() {
    let (tx, rx) = std::sync::mpsc::channel::<PtyEvent>();
    let backlog = Arc::new(OutputBacklog::default());
    let gate = backlog.clone();
    let mut out = crate::ui::bridge_with(rx, move |ev| match ev {
        PtyEvent::Output(d) => gate.admit(d.len()),
        PtyEvent::Exit(_) => true,
    });
    let chunk = vec![b'y'; 64 * 1024];
    for _ in 0..4096 {
        tx.send(PtyEvent::Output(chunk.clone())).unwrap();
    }
    tx.send(PtyEvent::Exit(Some(0))).unwrap();
    drop(tx);
    // The UI reads nothing until the gate has started refusing output (it must, 256 MB > 32 MB).
    let start = std::time::Instant::now();
    while !backlog.dropped.load(std::sync::atomic::Ordering::Relaxed) && start.elapsed() < std::time::Duration::from_secs(30) {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(backlog.dropped.load(std::sync::atomic::Ordering::Relaxed), "the gate never refused anything");
    let peak = backlog.pending.load(std::sync::atomic::Ordering::Relaxed);
    assert!(peak <= MAX_BACKLOG, "{peak} bytes were held while the UI stalled");
    let (mut held, mut exit) = (0usize, None);
    while exit.is_none() && start.elapsed() < std::time::Duration::from_secs(30) {
        match out.try_recv() {
            Ok(PtyEvent::Output(d)) => {
                held += d.len();
                backlog.consumed(d.len());
            }
            Ok(PtyEvent::Exit(code)) => exit = Some(code),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(2)),
        }
    }
    assert_eq!(exit, Some(Some(0)), "the exit event is never dropped");
    assert!(held >= MAX_BACKLOG - chunk.len(), "output up to the cap is delivered ({held})");
    assert!(backlog.take_dropped(), "the user is told");
}

#[test]
fn osc_52_reaches_the_clipboard_only_when_allowed() {
    let events = Listener::default();
    let mut term = Term::new(Config::default(), &TermSize { cols: 80, rows: 24 }, events.clone());
    let mut parser = Processor::<StdSyncHandler>::default();
    parser.advance(&mut term, b"\x1b]52;c;Zm9v\x07");
    let queued = std::mem::take(&mut *events.0.borrow_mut());
    assert!(queued.iter().any(|e| matches!(e, TermEvent::ClipboardStore(..))), "the terminal reports the store request");
    let texts = |allowed| queued.iter().cloned().filter_map(|e| clipboard_text(e, allowed)).collect::<Vec<_>>();
    assert!(texts(false).is_empty(), "refused: nothing is written");
    assert_eq!(texts(true), ["foo"]);
}

#[test]
fn bridge_with_discards_what_the_gate_refuses() {
    let (tx, rx) = std::sync::mpsc::channel::<u32>();
    let mut out = crate::ui::bridge_with(rx, |n| n % 2 == 0);
    for i in 0..6 {
        tx.send(i).unwrap();
    }
    drop(tx);
    let got: Vec<u32> = futures::executor::block_on(async {
        use futures::StreamExt;
        out.by_ref().collect().await
    });
    assert_eq!(got, [0, 2, 4]);
}
use gpui::Keystroke;

fn bytes(k: &str, mode: TermMode) -> Option<Vec<u8>> {
    key_to_bytes(&Keystroke::parse(k).unwrap(), mode)
}

fn mac(k: &str, mode: TermMode) -> Option<&'static [u8]> {
    macos_line_editing(&Keystroke::parse(k).unwrap(), mode)
}

#[test]
fn macos_line_editing_keys_are_readline_keys() {
    let m = TermMode::empty();
    assert_eq!(mac("cmd-left", m), Some(&b"\x01"[..]), "start of line");
    assert_eq!(mac("cmd-right", m), Some(&b"\x05"[..]), "end of line");
    assert_eq!(mac("alt-left", m), Some(&b"\x1bb"[..]), "word left");
    assert_eq!(mac("alt-right", m), Some(&b"\x1bf"[..]), "word right");
    assert_eq!(mac("cmd-backspace", m), Some(&b"\x15"[..]), "delete to line start");
    assert_eq!(mac("cmd-delete", m), Some(&b"\x0b"[..]), "delete to line end");
    assert_eq!(mac("alt-delete", m), Some(&b"\x1bd"[..]), "delete word right");
    // Selections and other combinations keep their xterm meaning; ⌘C / ⌘V stay with the app.
    for k in ["shift-cmd-left", "alt-shift-left", "ctrl-left", "cmd-c", "cmd-up"] {
        assert_eq!(mac(k, m), None, "{k}");
    }
    // vim, less: Home / End (Ctrl+A would increment a number in vim); the word keys stay plain xterm keys.
    assert_eq!(mac("cmd-left", TermMode::ALT_SCREEN), Some(&b"\x1b[H"[..]));
    assert_eq!(mac("cmd-right", TermMode::ALT_SCREEN), Some(&b"\x1b[F"[..]));
    assert_eq!(mac("cmd-left", TermMode::ALT_SCREEN | TermMode::APP_CURSOR), Some(&b"\x1bOH"[..]));
    assert_eq!(mac("cmd-backspace", TermMode::ALT_SCREEN), None);
    assert_eq!(mac("alt-left", TermMode::ALT_SCREEN), None);
}

#[test]
fn keys_map_to_xterm_sequences() {
    let m = TermMode::empty();
    assert_eq!(bytes("enter", m).unwrap(), b"\r");
    assert_eq!(bytes("ctrl-c", m).unwrap(), [3]);
    assert_eq!(bytes("up", m).unwrap(), b"\x1b[A");
    assert_eq!(bytes("up", TermMode::APP_CURSOR).unwrap(), b"\x1bOA", "vim/less uygulama modu");
    assert_eq!(bytes("shift-right", m).unwrap(), b"\x1b[1;2C");
    assert_eq!(bytes("ctrl-delete", m).unwrap(), b"\x1b[3;5~");
    assert_eq!(bytes("alt-backspace", m).unwrap(), b"\x1b\x7f");
    assert!(bytes("cmd-v", m).is_none(), "⌘ keys are left to the app");
}

#[test]
fn alt_letters_are_the_meta_key_unless_alt_composed_another_character() {
    let m = TermMode::empty();
    // readline, emacs, vim: Alt+f, Alt+b, Alt+d.
    for (k, want) in [("alt-f", b"\x1bf".as_slice()), ("alt-b", b"\x1bb"), ("alt-d", b"\x1bd"), ("alt-shift-f", b"\x1bF")] {
        let mut key = Keystroke::parse(k).unwrap();
        key.key_char = Some(if k.contains("shift") { want[1..].to_ascii_uppercase() } else { want[1..].to_vec() }.iter().map(|b| *b as char).collect());
        assert_eq!(key_to_bytes(&key, m).unwrap(), want, "{k}");
    }
    // ⌥ on macOS composed another character: it is typed as it is.
    let mut key = Keystroke::parse("alt-f").unwrap();
    key.key_char = Some("ƒ".into());
    assert_eq!(key_to_bytes(&key, m).unwrap(), "ƒ".as_bytes());
}

#[test]
fn space_keys_are_sent_to_the_terminal() {
    let m = TermMode::empty();
    assert_eq!(bytes("space", m), Some(b" ".to_vec()));
    assert_eq!(bytes("shift-space", m), Some(b" ".to_vec()));
    assert_eq!(bytes("ctrl-space", m), Some(vec![0]));
}

#[test]
fn windows_turkish_characters_are_sent_as_utf8() {
    for ch in ['ı', 'İ', 'ş', 'ğ', 'ü', 'ö', 'ç', '@', '{', '}', '[', ']', '\\', '|', '€'] {
        // GPUI resolves AltGr to a text character before dispatching the key.
        let mut key = Keystroke::parse("a").unwrap();
        key.key_char = Some(ch.to_string());
        assert_eq!(key_to_bytes(&key, TermMode::empty()).unwrap(), ch.to_string().as_bytes());
    }
}

#[test]
fn ansi_cube_and_grayscale() {
    let p = theme::DARK;
    let o = alacritty_terminal::term::color::Colors::default();
    assert_eq!(resolve(&p, Color::Indexed(196), &o), 0xff0000);
    assert_eq!(resolve(&p, Color::Indexed(232), &o), 0x080808);
    assert_eq!(resolve(&p, Color::Spec(Rgb { r: 1, g: 2, b: 3 }), &o), 0x010203);
    assert_eq!(resolve(&p, Color::Named(NamedColor::Red), &o), p.danger);
}

/// What a program like Claude Code sends: many redraws of a screen of long, multi-coloured, wrapping lines.
fn tui_stream(frames: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for frame in 0..frames {
        out.extend_from_slice(b"\x1b[40A");
        for row in 0..40 {
            out.extend_from_slice(b"\x1b[2K");
            for word in 0..60 {
                let color = 31 + (word + row + frame) % 7;
                out.extend_from_slice(format!("\x1b[{color}mword{word:02}\x1b[0m ").as_bytes());
            }
            out.extend_from_slice(b"\r\n");
        }
    }
    out
}

/// Parsing a long TUI stream must stay fast, and feeding it in one piece must not be slower than in the chunks the pty
/// delivers it in (the pump batches the chunks of one turn).
#[gpui::test]
fn feeding_a_tui_stream_is_fast(cx: &mut gpui::TestAppContext) {
    let data = tui_stream(300);
    cx.update(|cx| cx.set_global(crate::settings::Settings::default()));
    let make = |cx: &mut gpui::TestAppContext| {
        let view = cx.new(|cx| TerminalView::output_only(std::sync::Arc::new(host::local()), "bench".into(), cx));
        view.update(cx, |t, _| t.set_size(200, 50));
        view
    };
    let chunked = make(cx);
    let start = std::time::Instant::now();
    chunked.update(cx, |t, _| data.chunks(16 * 1024).for_each(|c| t.feed(c)));
    let per_chunk = start.elapsed();
    let whole = make(cx);
    let start = std::time::Instant::now();
    whole.update(cx, |t, _| t.feed(&data));
    let in_one = start.elapsed();
    eprintln!("BENCH feed {} KB: chunked {per_chunk:?}, one piece {in_one:?}", data.len() / 1024);
    let (a, b) = (chunked.read_with(cx, |t, _| t.scrollback_text()), whole.read_with(cx, |t, _| t.scrollback_text()));
    assert!(a == b, "the pump feeds a turn's chunks as one piece: the screen must come out the same");
    assert!(per_chunk < std::time::Duration::from_secs(5 * crate::test_sshd::timeout_scale()), "{per_chunk:?}");
}

/// A program that animates its title (a spinner) must not make the window rebuild for every frame: the first change is told
/// at once, the ones after it are gathered, and only the last title is kept.
#[gpui::test]
fn an_animated_title_tells_the_window_at_most_every_quarter_second(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| cx.set_global(crate::settings::Settings::default()));
    let view = cx.new(|cx| TerminalView::output_only(std::sync::Arc::new(host::local()), "t".into(), cx));
    let told = std::rc::Rc::new(std::cell::Cell::new(0));
    let counter = told.clone();
    let _sub = cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &TerminalEvent, _| {
            if matches!(event, TerminalEvent::TitleChanged) {
                counter.set(counter.get() + 1)
            }
        })
    });
    view.update(cx, |t, cx| {
        for frame in "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".chars().cycle().take(100) {
            t.set_title(format!("{frame} working").into(), cx);
        }
        t.set_title("working".into(), cx);
        t.set_title("working".into(), cx);
    });
    assert_eq!(told.get(), 1, "one telling, not one per frame");
    cx.executor().advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert_eq!(told.get(), 2, "the changes since are told once the pause is over");
    assert_eq!(view.read_with(cx, |t, _| t.title.to_string()), "working");
    view.update(cx, |t, cx| t.set_title("working".into(), cx));
    assert_eq!(told.get(), 2, "the same title is not news");
}

/// A program that begins a synchronized update and never ends it must not freeze the screen: after the time limit what it drew
/// is applied, and until then the parser holds it back.
#[gpui::test]
fn a_synchronized_update_that_never_ends_is_shown_after_its_time_limit(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| cx.set_global(crate::settings::Settings::default()));
    let view = cx.new(|cx| TerminalView::output_only(std::sync::Arc::new(host::local()), "t".into(), cx));
    view.update(cx, |t, _| {
        t.feed(b"\x1b[?2026hdrawn in a frame");
        assert!(!t.screen_text().contains("drawn in a frame"), "held back while the update is open");
        let left = t.release_expired_sync().expect("an update is pending");
        assert!(left <= std::time::Duration::from_millis(150), "{left:?}");
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    view.update(cx, |t, _| {
        assert_eq!(t.release_expired_sync(), None, "nothing pending once it is applied");
        assert!(t.screen_text().contains("drawn in a frame"));
    });
}

#[test]
fn clipboard_writes_follow_the_setting_and_the_copy_gesture() {
    use super::mouse::clipboard_write_allowed;
    let now = std::time::Instant::now();
    let just = Some(now);
    let stale = now.checked_sub(std::time::Duration::from_secs(6));
    assert!(clipboard_write_allowed("copy", just, now));
    assert!(!clipboard_write_allowed("copy", stale, now), "a copy long ago does not open the clipboard");
    assert!(!clipboard_write_allowed("copy", None, now));
    assert!(clipboard_write_allowed("always", None, now));
    assert!(!clipboard_write_allowed("off", just, now));
    assert_eq!(crate::settings::Settings::default().terminal_allow_clipboard_write, "copy");
}
