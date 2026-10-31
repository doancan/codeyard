//! A keystroke never waits (#142): not for a completion provider that does not answer, and not for a reparse of the
//! whole file. Typing stray words between the items of a large Rust file makes tree-sitter recover from the error
//! across all of it, tens of milliseconds per key; that reparse finishes in the background. Nor for a frame drawn for
//! a completion answer that shows nothing.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use futures::channel::{mpsc, oneshot};
use gpui::{Entity, EntityInputHandler, Focusable, TestAppContext, VisualTestContext};

use crate::editor::EditorView;
use crate::editor::ext_lang::{ExtAnswer, ExtAsker, ExtLanguage, ExtQuery, ExtReply};
use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

const BIG: &str = include_str!("../testdata/big.rs");

fn file(name: &str, content: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("codeyard-keystroke-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("big.rs");
    std::fs::write(&f, content).unwrap();
    f
}

fn setup(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let (wb, cx) = cx.add_window_view(|window, cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

fn open(wb: &Entity<Workbench>, path: &std::path::Path, cx: &mut VisualTestContext) -> Entity<EditorView> {
    let p = path.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&p, window, cx));
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| wb.active_editor_entity()).expect("editor opened")
}

/// Type `ch` the way the platform does, without running anything else: what happens before the key returns.
fn type_char(ed: &Entity<EditorView>, ch: &str, cx: &mut VisualTestContext) {
    ed.update_in(cx, |e, window, cx| e.replace_text_in_range(None, ch, window, cx));
}

/// A completion provider that is asked and never answers: typing goes on, each key is drawn, and nothing waits for it.
#[gpui::test]
fn typing_never_waits_for_a_completion_provider_that_never_answers(cx: &mut TestAppContext) {
    let f = file("silent", "fn main() {\n    \n}\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    // The provider keeps every request open for good.
    type Open = (mpsc::UnboundedSender<(String, ExtAnswer)>, oneshot::Receiver<()>);
    let asked: Rc<RefCell<Vec<(ExtQuery, Open)>>> = Rc::default();
    let log = asked.clone();
    let ask: ExtAsker = Rc::new(move |query: ExtQuery, _: &mut gpui::AsyncApp| {
        let (tx, answers) = mpsc::unbounded();
        let (alive, gone) = oneshot::channel();
        log.borrow_mut().push((query, (tx, gone)));
        Some(ExtReply { answers, _alive: alive })
    });
    ed.update(cx, |e, _| e.set_ext_language(Some(ExtLanguage { hover: false, completion: Some(vec!['.']), ask })));
    ed.update(cx, |e, cx| e.set_cursor_for_test(16, cx));
    cx.run_until_parked();
    let notified = Rc::new(RefCell::new(0usize));
    let count = notified.clone();
    let _watch = cx.update(|_, cx| cx.observe(&ed, move |_, _| *count.borrow_mut() += 1));

    for (i, ch) in ["a", "b", " ", "c", "."].into_iter().enumerate() {
        let before = *notified.borrow();
        type_char(&ed, ch, cx);
        // The key is in the text and the editor asked to be drawn before anything else ran.
        assert_eq!(ed.read_with(cx, |e, _| e.text()), format!("fn main() {{\n    {}\n}}\n", ["a", "ab", "ab ", "ab c", "ab c."][i]));
        assert!(*notified.borrow() > before, "key {ch:?} did not ask for a frame");
        cx.run_until_parked();
    }
    // A new word, the trigger character and the first word: three requests, all still open, and the list never
    // waited for them.
    assert_eq!(asked.borrow().len(), 3, "a, c and '.' ask");
    assert!(ed.read_with(cx, |e, _| e.completion_sources_for_test()).is_none_or(|l| l.iter().all(|(_, s)| s.is_none())));
}

/// Stray words at the top of a 10 000-line Rust file: tree-sitter reparses the whole file on some of these keys. The
/// key does not wait for that; the tree catches up in the background and ends as a full parse of the text.
#[gpui::test]
fn a_keystroke_never_waits_for_a_reparse_of_the_whole_file(cx: &mut TestAppContext) {
    let f = file("reparse", BIG);
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    ed.update(cx, |e, cx| e.set_cursor_for_test(0, cx));
    let mut deferred = 0;
    for ch in "ab ab ab ab ab a".split("").filter(|s| !s.is_empty()) {
        type_char(&ed, ch, cx);
        if ed.read_with(cx, |e, _| e.parsing_syntax_for_test()) {
            deferred += 1;
        }
    }
    assert!(deferred > 0, "every reparse ran on the keystroke");
    cx.run_until_parked();
    assert!(!ed.read_with(cx, |e, _| e.parsing_syntax_for_test()));
    let text = ed.read_with(cx, |e, _| e.text());
    assert!(text.starts_with("ab ab ab ab ab a/// "));
    let rope = buffer::Rope::from_str(&text);
    let full = syntax::Syntax::new(syntax::language_for_path("big.rs").unwrap(), &rope).unwrap();
    let range = 0..rope.line_to_byte(200);
    assert_eq!(ed.read_with(cx, |e, _| e.highlights_for_test(range.clone())), full.highlights(&rope, range));
}

fn item(label: &str) -> crate::lsp::CompletionItem {
    crate::lsp::CompletionItem {
        label: label.into(),
        detail: None,
        kind: 6,
        filter: label.into(),
        sort: label.into(),
        insert: label.into(),
        cursor: None,
        stops: Vec::new(),
        range: None,
        extra: Vec::new(),
        doc: None,
        resolve: None,
        source: None,
    }
}

/// A completion answer none of whose items matches the word typed so far changes nothing on screen, so it asks for no
/// frame: in a large file a frame takes the UI thread for a long while, and a key that comes meanwhile waits for it
/// (`typing.keystroke_frame_completion_ms`). An answer that matches opens the list.
#[gpui::test]
fn a_completion_answer_that_shows_nothing_draws_no_frame(cx: &mut TestAppContext) {
    let f = file("unmatched", "fn main() {\n    \n}\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    let asked: Rc<RefCell<Vec<mpsc::UnboundedSender<(String, ExtAnswer)>>>> = Rc::default();
    let log = asked.clone();
    let ask: ExtAsker = Rc::new(move |_: ExtQuery, _: &mut gpui::AsyncApp| {
        let (tx, answers) = mpsc::unbounded();
        let (alive, _) = oneshot::channel();
        log.borrow_mut().push(tx);
        Some(ExtReply { answers, _alive: alive })
    });
    ed.update(cx, |e, _| e.set_ext_language(Some(ExtLanguage { hover: false, completion: Some(Vec::new()), ask })));
    ed.update(cx, |e, cx| e.set_cursor_for_test(16, cx));
    cx.run_until_parked();
    let notified = Rc::new(RefCell::new(0usize));
    let count = notified.clone();
    let _watch = cx.update(|_, cx| cx.observe(&ed, move |_, _| *count.borrow_mut() += 1));
    let answer = |n: usize, labels: &[&str], cx: &mut VisualTestContext| {
        *notified.borrow_mut() = 0;
        let items = labels.iter().map(|l| item(l)).collect();
        asked.borrow()[n].unbounded_send(("you.ext".into(), ExtAnswer::Completion { items, incomplete: false })).unwrap();
        cx.run_until_parked();
        *notified.borrow()
    };

    type_char(&ed, "a", cx);
    cx.run_until_parked();
    assert_eq!(asked.borrow().len(), 1);
    assert_eq!(answer(0, &["perf_item_0", "perf_item_1"], cx), 0, "an answer that shows nothing drew a frame");
    assert!(ed.read_with(cx, |e, _| e.completion_sources_for_test()).is_none());

    type_char(&ed, " ", cx);
    type_char(&ed, "p", cx);
    cx.run_until_parked();
    assert_eq!(asked.borrow().len(), 2);
    assert!(answer(1, &["perf_item_0", "other"], cx) > 0, "the list opened without a frame");
    assert_eq!(ed.read_with(cx, |e, _| e.completion_sources_for_test()), Some(vec![("perf_item_0".to_string(), Some("you.ext".to_string()))]));
}
