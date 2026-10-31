//! End-to-end tests: real keystrokes and platform text input (the IME path) on
//! GPUI's test platform. No operating-system permission required.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Entity, Focusable, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use host::{Host, RemoteHost};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

fn tmp(name: &str, content: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("dosya.txt");
    std::fs::write(&file, content).unwrap();
    file
}

fn setup(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    setup_with_settings(cx, crate::settings::Settings { lsp_enabled: false, ..Default::default() })
}

fn setup_with_settings(cx: &mut TestAppContext, settings: crate::settings::Settings) -> (Entity<Workbench>, &mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(settings);
    });
    let (wb, cx) = cx.add_window_view(|window, cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx));
    // Same as main.rs: focus starts on the workbench.
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

fn open(wb: &Entity<Workbench>, path: &std::path::Path, cx: &mut VisualTestContext) {
    let p = path.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&p, window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn type_and_save_keeps_crlf(cx: &mut TestAppContext) {
    let file = tmp("crlf", b"satir bir\r\nsatir iki\r\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);

    cx.simulate_input("Merhaba ğüşİı ");
    cx.simulate_keystrokes("down end enter");
    cx.simulate_input("üç");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();

    let saved = std::fs::read_to_string(&file).unwrap();
    assert_eq!(saved, "Merhaba ğüşİı satir bir\r\nsatir iki\r\nüç\r\n");
    assert!(!wb.read_with(cx, |wb, cx| wb.any_dirty(cx)), "clean after saving");
}

#[gpui::test]
fn windows_1254_file_is_saved_back_the_same_way(cx: &mut TestAppContext) {
    // "Işık\r\n" in Windows-1254: ş=0xFE, ı=0xFD.
    let file = tmp("cp1254", b"I\xfe\xfdk\r\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let editor = wb.read_with(cx, |wb, _| wb.active_editor_entity()).expect("should open");
    assert_eq!(editor.read_with(cx, |e, _| e.encoding()), crate::encoding::Encoding::Windows1254);

    cx.simulate_input("ğ");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read(&file).unwrap(), b"\xf0I\xfe\xfdk\r\n", "same encoding and line ending");

    // Character with no equivalent in the encoding: saving is refused and the file is left intact.
    cx.simulate_input("😀");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read(&file).unwrap(), b"\xf0I\xfe\xfdk\r\n");
    assert!(editor.read_with(cx, |e, _| e.is_dirty()), "kaydedilmedi");
}

#[gpui::test]
fn undo_selection_and_line_cut(cx: &mut TestAppContext) {
    let file = tmp("undo", b"alfa\nbeta\ngama\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);

    cx.simulate_keystrokes("down cmd-x"); // cut with no selection: the whole line
    cx.simulate_input("x");
    cx.simulate_keystrokes("cmd-z cmd-z cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alfa\nbeta\ngama\n");

    cx.simulate_keystrokes("shift-right shift-right");
    cx.simulate_input("BE");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alfa\nBEta\ngama\n");
}

#[gpui::test]
fn external_change_is_a_conflict_not_an_overwrite(cx: &mut TestAppContext) {
    let file = tmp("conflict", b"ilk\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);

    std::fs::write(&file, "başkası değiştirdi\n").unwrap();
    cx.simulate_input("benim ");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "başkası değiştirdi\n", "must not overwrite silently");
    assert!(wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));

    // Save again once the user has had time to read the banner: deliberate overwrite.
    wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap().update(cx, |e, _| e.expire_conflict_grace());
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "benim ilk\n");
}

#[gpui::test]
fn remote_host_same_flow(cx: &mut TestAppContext) {
    let file = tmp("remote", b"uzak dosya\n");
    let (wb, cx) = setup(cx);

    // The agent runs on a separate thread over two pipes: the same protocol as ssh's stdio.
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    std::thread::spawn(move || agent::serve(req_r, resp_w));
    let remote: Arc<dyn Host> = Arc::new(RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "ssh://test").unwrap());

    let dir = file.parent().unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| {
        wb.attach_host(remote, "test".into(), window, cx);
        wb.set_root(dir, cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.window_title().as_deref(), Some(format!("{} — ssh://test", file.parent().unwrap().file_name().unwrap().to_string_lossy()).as_str()));

    // Open the file from the tree with the keyboard.
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-down");
    cx.simulate_input("// uzaktan eklendi\n");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "uzak dosya\n// uzaktan eklendi\n");
}

#[gpui::test]
fn binary_and_missing_files_show_message(cx: &mut TestAppContext) {
    let file = tmp("binary", b"\x00\x01\x02ELF");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let (opened, error) = wb.read_with(cx, |wb, _| wb.open_state());
    assert_eq!(opened, 0);
    assert!(error.unwrap().contains("binary"));
}

#[gpui::test]
fn theme_picker_preview_confirm_and_escape(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    let active = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.global::<ActiveTheme>().0);
    let fjord = theme::themes::builtin().into_iter().find(|t| t.id == "fjord").unwrap().palette;
    let ember = theme::themes::builtin().into_iter().find(|t| t.id == "ember").unwrap().palette;

    cx.simulate_keystrokes("cmd-k cmd-t");
    assert!(wb.read_with(cx, |wb, _| wb.theme_picker_open()), "⌘K ⌘T should open the picker");
    cx.simulate_input("fjord");
    assert_eq!(active(cx), fjord, "first match is previewed live while typing");
    cx.simulate_keystrokes("enter");
    assert_eq!(wb.read_with(cx, |wb, _| wb.current_theme_id().to_string()), "fjord");
    assert_eq!(active(cx), fjord);

    // Preview, then Esc: the previous theme comes back.
    cx.simulate_keystrokes("cmd-k cmd-t");
    cx.simulate_input("ember");
    assert_eq!(active(cx), ember);
    cx.simulate_keystrokes("escape");
    assert_eq!(active(cx), fjord);
    assert_eq!(wb.read_with(cx, |wb, _| wb.current_theme_id().to_string()), "fjord");
}

#[gpui::test]
fn theme_picker_from_editor_returns_focus(cx: &mut TestAppContext) {
    let file = tmp("picker-focus", b"x\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);

    cx.simulate_keystrokes("cmd-k cmd-t");
    assert!(wb.read_with(cx, |wb, _| wb.theme_picker_open()));
    cx.simulate_input("pap");
    cx.simulate_keystrokes("escape");
    assert!(!wb.read_with(cx, |wb, _| wb.theme_picker_open()));

    // Focus returned to the editor: typing goes to the file, not the picker.
    cx.simulate_input("y");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "yx\n");
}

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(dir.join("ignored.txt"), "gizli-içerik").unwrap();
    std::fs::write(dir.join("README.md"), "# proje\n").unwrap();
    std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    let lib: String = (1..=10).map(|i| format!("// satır {i}\n")).collect();
    std::fs::write(dir.join("src/lib.rs"), lib).unwrap();
    dir
}

#[gpui::test]
fn quick_open_finds_file_and_skips_ignored(cx: &mut TestAppContext) {
    let dir = project("quickopen");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);

    cx.simulate_keystrokes("cmd-p");
    cx.run_until_parked(); // the file list arrives in the background
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()));
    cx.simulate_input("ignored");
    cx.simulate_keystrokes("enter");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), None, "file from .gitignore should not be listed");

    cx.simulate_keystrokes("escape cmd-p");
    cx.run_until_parked();
    cx.simulate_input("slib");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let path = wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap();
    assert!(path.ends_with("src/lib.rs"), "{path}");
}

#[gpui::test]
fn command_palette_runs_command_and_go_to_line(cx: &mut TestAppContext) {
    let dir = project("palette");
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("src/lib.rs"), cx);

    // Ctrl+G: go to line.
    cx.simulate_keystrokes("ctrl-g");
    cx.simulate_input("7");
    cx.simulate_keystrokes("enter");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)), Some((7, 1)));

    // Editor command from the palette: focus must return to the editor so Save reaches it.
    cx.simulate_input("x");
    cx.simulate_keystrokes("cmd-shift-p");
    cx.simulate_input("editor save");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let saved = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
    assert!(saved.contains("x// satır 7"), "{saved}");

    // The command opens another overlay.
    cx.simulate_keystrokes("cmd-shift-p");
    cx.simulate_input("color theme");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.theme_picker_open()));
}

fn single_file_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "eski ad\n").unwrap();
    dir
}

#[gpui::test]
fn new_file_is_created_and_opened(cx: &mut TestAppContext) {
    let dir = single_file_dir("newfile");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let d = dir.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.start_create(d, false, window, cx));
    cx.simulate_input("yeni.rs");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(dir.join("yeni.rs").is_file());
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("/yeni.rs"));
}

#[gpui::test]
fn rename_with_f2_updates_open_tab(cx: &mut TestAppContext) {
    let dir = single_file_dir("rename");
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("a.txt"), cx);
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("down f2 backspace backspace backspace backspace backspace");
    cx.simulate_input("b.txt");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!dir.join("a.txt").exists() && dir.join("b.txt").is_file());
    let path = wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap();
    assert!(path.ends_with("/b.txt"), "open tab should follow the new path: {path}");

    // Saving writes to the new location.
    wb.update_in(cx, |wb, window, cx| wb.focus_active_editor(window, cx));
    cx.simulate_input("+");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "+eski ad\n");
}

#[gpui::test]
fn delete_asks_first_and_closes_tab(cx: &mut TestAppContext) {
    let dir = single_file_dir("delete");
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("a.txt"), cx);
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("down cmd-backspace");
    assert!(cx.has_pending_prompt(), "should ask before deleting");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(dir.join("a.txt").exists(), "cancelling must not delete");

    cx.simulate_keystrokes("cmd-backspace");
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();
    assert!(!dir.join("a.txt").exists());
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), None, "deleted file's tab should close");
}

/// A real shell: output comes from real threads, so loop while waiting.
fn wait_for(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    let start = std::time::Instant::now();
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(10 * crate::test_sshd::timeout_scale()), "timed out: {what}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[gpui::test]
fn terminal_runs_commands_and_closes(cx: &mut TestAppContext) {
    let dir = single_file_dir("terminal");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));

    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal should open");
    // The command is typed with real keys while the terminal is focused (including Turkish characters).
    cx.simulate_keystrokes("e c h o space ğ ü ş - $ ( ( 6 * 7 ) ) enter");
    wait_for(cx, "komut çıktısı", |cx| term.read_with(cx, |t, _| t.screen_text().contains("ğüş-42")));

    // Mouse selection: drag across an output line, then double-click a word; ⌘C copies to the clipboard.
    let row = term.read_with(cx, |t, _| t.screen_text().lines().position(|l| l == "ğüş-42")).expect("output line");
    let at: Vec<_> = term.read_with(cx, |t, _| (0..5).map(|col| t.cell_center(row, col)).collect());
    // Just inside the outer halves: a press exactly on a cell's centre is on neither side, and float noise picks one.
    let (from, to) = (gpui::point(at[0].x - gpui::px(1.), at[0].y), gpui::point(at[4].x + gpui::px(1.), at[4].y));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    assert_eq!(term.read_with(cx, |t, _| t.selected_text()).as_deref(), Some("ğüş-4"));
    cx.simulate_event(gpui::MouseDownEvent { button: MouseButton::Left, position: at[1], modifiers: Modifiers::none(), click_count: 2, first_mouse: false });
    cx.simulate_event(gpui::MouseUpEvent { button: MouseButton::Left, position: at[1], modifiers: Modifiers::none(), click_count: 2 });
    assert_eq!(term.read_with(cx, |t, _| t.selected_text()).as_deref(), Some("ğüş-42"), "double click: word");
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-c" } else { "ctrl-shift-c" });
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("ğüş-42"));
    // A single click clears the selection.
    cx.simulate_click(at[2], Modifiers::none());
    assert_eq!(term.read_with(cx, |t, _| t.selected_text()), None);

    // `terminal.fontSize` changes the rows on the next frame.
    let row_h = |cx: &mut VisualTestContext| term.read_with(cx, |t, _| t.cell_center(1, 0).y - t.cell_center(0, 0).y);
    let before = row_h(cx);
    cx.update(|_, cx| gpui::BorrowAppContext::update_global::<crate::settings::Settings, _>(cx, |s, _| s.terminal_font_size += 8.));
    term.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert!(row_h(cx) > before, "taller rows after a bigger font: {:?} -> {:?}", before, row_h(cx));

    // The shell starts in the opened folder.
    cx.simulate_keystrokes("p w d enter");
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    wait_for(cx, "pwd", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l.ends_with(&name))));

    cx.simulate_keystrokes("e x i t enter");
    wait_for(cx, "kabuk çıkışı", |cx| term.read_with(cx, |t, _| t.has_exited()));
    cx.simulate_keystrokes("x"); // any key: close the tab
    assert!(wb.read_with(cx, |wb, _| wb.active_terminal()).is_none());
}

#[gpui::test]
fn find_in_files_streams_results_and_opens_match(cx: &mut TestAppContext) {
    let dir = project("search");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());

    cx.simulate_keystrokes("cmd-shift-f");
    cx.simulate_input("SATIR 7"); // case-insensitive, including Turkish İ/ı
    cx.simulate_keystrokes("enter");
    wait_for(cx, "arama", |cx| search.read_with(cx, |s, _| !s.is_searching()));
    assert_eq!(search.read_with(cx, |s, _| s.result_summary()), [("src/lib.rs".to_string(), vec![7])]);

    search.update(cx, |s, cx| s.open_first(cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("src/lib.rs"));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_selection(cx)).as_deref(), Some("satır 7"));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)).map(|(l, _)| l), Some(7));
}

/// Search: F4 / ⇧F4 to move between results, ↑/↓ query history, excluding files,
/// limiting to a folder.
#[gpui::test]
fn search_steps_through_results_remembers_queries_and_limits_to_a_folder(cx: &mut TestAppContext) {
    let dir = project("search-nav");
    std::fs::write(dir.join("a.txt"), "needle bir\nikinci needle\n").unwrap();
    std::fs::write(dir.join("b.txt"), "needle\n").unwrap();
    std::fs::write(dir.join("src/c.txt"), "needle altta\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    cx.simulate_keystrokes("cmd-shift-f");
    cx.simulate_input("needle");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "arama", |cx| search.read_with(cx, |s, _| !s.is_searching()));
    let files: Vec<String> = search.read_with(cx, |s, _| s.result_summary().into_iter().map(|(p, _)| p).collect());
    assert_eq!(files, ["a.txt", "b.txt", "src/c.txt"]);
    assert_eq!(search.read_with(cx, |s, _| s.history_for_test()), ["needle"]);

    // F4: first match, then the following ones; ⇧F4 goes back; going back from the start wraps to the end.
    cx.simulate_keystrokes("f4");
    assert_eq!(search.read_with(cx, |s, _| s.current_for_test()), Some((0, 0)));
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("a.txt"));
    cx.simulate_keystrokes("f4 f4");
    assert_eq!(search.read_with(cx, |s, _| s.current_for_test()), Some((1, 0)));
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("b.txt"));
    cx.simulate_keystrokes("shift-f4");
    assert_eq!(search.read_with(cx, |s, _| s.current_for_test()), Some((0, 1)));
    cx.simulate_keystrokes("shift-f4 shift-f4");
    assert_eq!(search.read_with(cx, |s, _| s.current_for_test()), Some((2, 0)), "wraps from the start back to the end");

    // History: second query, then ↑ ↑ ↓ ↓ (the field must be focused).
    wb.update_in(cx, |wb, window, cx| wb.search_for("", window, cx));
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("zzz");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "ikinci arama", |cx| search.read_with(cx, |s, _| !s.is_searching()));
    assert_eq!(search.read_with(cx, |s, _| s.history_for_test()), ["needle", "zzz"]);
    let query = |cx: &mut VisualTestContext| search.read_with(cx, |s, cx| s.state_for_test(cx).0);
    cx.simulate_keystrokes("up");
    assert_eq!(query(cx), "zzz");
    cx.simulate_keystrokes("up");
    assert_eq!(query(cx), "needle");
    cx.simulate_keystrokes("down");
    assert_eq!(query(cx), "zzz");
    cx.simulate_keystrokes("down");
    assert_eq!(query(cx), "");

    // Excluding a file: the counts update.
    cx.simulate_input("needle");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "üçüncü arama", |cx| search.read_with(cx, |s, _| !s.is_searching() && s.summary_for_test().is_some()));
    assert_eq!(search.read_with(cx, |s, _| s.summary_for_test()).map(|x| x.0), Some(4));
    search.update(cx, |s, cx| s.dismiss_for_test(0, cx));
    assert_eq!(search.read_with(cx, |s, _| s.summary_for_test()), Some((2, 2)));
    assert_eq!(search.read_with(cx, |s, _| s.results_for_test().len()), 2);

    // Limit to a folder: src/ only.
    wb.update_in(cx, |wb, window, cx| {
        wb.search_panel().update(cx, |s, cx| s.limit_to("src/**", window, cx));
    });
    wait_for(cx, "klasör araması", |cx| search.read_with(cx, |s, _| !s.is_searching() && s.summary_for_test().is_some()));
    assert_eq!(search.read_with(cx, |s, cx| s.include_text(cx)), "src/**");
    let files: Vec<String> = search.read_with(cx, |s, _| s.result_summary().into_iter().map(|(p, _)| p).collect());
    assert_eq!(files, ["src/c.txt"]);
}

#[gpui::test]
fn find_in_files_skips_gitignored(cx: &mut TestAppContext) {
    let dir = project("search-ignore");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    cx.simulate_keystrokes("cmd-shift-f");
    cx.simulate_input("gizli-içerik"); // only occurs in the file listed in .gitignore
    cx.simulate_keystrokes("enter");
    wait_for(cx, "arama", |cx| search.read_with(cx, |s, _| !s.is_searching()));
    assert!(search.read_with(cx, |s, _| s.result_summary()).is_empty());
}

fn setup_with_config<'a>(cx: &'a mut TestAppContext, config: &std::path::Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings::default());
    });
    let dir = config.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-config-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The Welcome and Settings pages open as tabs; settings fields are written to the file.
#[gpui::test]
fn welcome_and_settings_pages_open_as_tabs_and_write_settings(cx: &mut TestAppContext) {
    let cfg = config_dir("pages");
    let dir = project("pages");
    let (wb, cx) = setup_with_config(cx, &cfg);
    open(&wb, &dir, cx);

    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Help: Welcome", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("welcome", true)));
    // When a file opens the page stays behind (its tab remains) and the editor comes to the front.
    open(&wb, &dir.join("README.md"), cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("welcome", false)));

    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Open Settings", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("settings", true)));
    let page = wb.read_with(cx, |wb, _| wb.settings_page_for_test()).unwrap();
    let keys = page.read_with(cx, |p, cx| p.visible_keys(cx));
    assert!(keys.contains(&"theme") && keys.contains(&"editor.fontSize"), "{keys:?}");
    assert!(!keys.contains(&"search.maxResults"), "not in the shared settings");
    page.update(cx, |p, cx| p.set_query("max results", cx));
    assert_eq!(page.read_with(cx, |p, cx| p.visible_keys(cx)), ["search.maxResults"]);

    // Number field: a valid value is written to the file and applied; an invalid value shows an error.
    page.update(cx, |p, cx| p.submit_field("editor.fontSize", "99", cx));
    assert!(page.read_with(cx, |p, _| p.error_for_test("editor.fontSize")).is_some());
    assert_eq!(cx.read_global(|s: &crate::settings::Settings, _| s.editor_font_size), theme::text::CODE.0);
    page.update(cx, |p, cx| p.submit_field("editor.fontSize", "15", cx));
    cx.run_until_parked();
    assert_eq!(cx.read_global(|s: &crate::settings::Settings, _| s.editor_font_size), 15.);
    assert!(std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap().contains("\"editor.fontSize\": 15"));
    assert!(page.read_with(cx, |p, _| p.error_for_test("editor.fontSize")).is_none());

    // Close: the tab goes away, the editor remains.
    wb.update(cx, |wb, cx| wb.close_page_for_test(cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), None);
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("README.md"));
}

/// Case conversions and block comment apply to the selected text.
#[gpui::test]
fn case_transforms_and_block_comment_work_on_the_selection(cx: &mut TestAppContext) {
    let dir = tmp("case", b"");
    let file = dir.with_file_name("kod.rs");
    std::fs::write(&file, "let some_value_here = HTTPServer;\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let text = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).text())).unwrap();
    let run = |cx: &mut VisualTestContext, cmd: &str| {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(cmd, window, cx));
    };
    // Select the word without double-click: works with the cursor inside it and no selection.
    cx.simulate_keystrokes("cmd-up right right right right right");
    run(cx, "Editor: Transform to camelCase");
    assert_eq!(text(cx), "let someValueHere = HTTPServer;\n");
    run(cx, "Editor: Transform to snake_case");
    assert_eq!(text(cx), "let some_value_here = HTTPServer;\n");
    run(cx, "Editor: Transform to CONSTANT_CASE");
    assert_eq!(text(cx), "let SOME_VALUE_HERE = HTTPServer;\n");
    run(cx, "Editor: Transform to kebab-case");
    assert_eq!(text(cx), "let some-value-here = HTTPServer;\n");

    // Block comment: wrap the selection; running it again removes it.
    cx.simulate_keystrokes("cmd-a");
    run(cx, "Editor: Toggle Block Comment");
    assert_eq!(text(cx), "/* let some-value-here = HTTPServer; */\n");
    run(cx, "Editor: Toggle Block Comment");
    assert_eq!(text(cx), "let some-value-here = HTTPServer;\n");
}

/// Comparisons, last edit location, scratch file, and running the selected text in the terminal.
#[gpui::test]
fn compare_last_edit_scratch_and_run_selected_text(cx: &mut TestAppContext) {
    let cfg = config_dir("extras");
    let dir = project("extras");
    std::fs::write(dir.join("run.sh"), "echo run-$((20+22))\nsecond line\n").unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    open(&wb, &dir, cx);
    open(&wb, &dir.join("src/lib.rs"), cx);
    let run = |cx: &mut VisualTestContext, cmd: &str| {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(cmd, window, cx));
    };

    // No unsaved changes: no diff opens, an info message is shown.
    run(cx, "File: Compare Active File with Saved");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.diff_view().is_none()));
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).is_some_and(|n| n.contains("No unsaved changes")));
    // Editing opens the comparison; the last edit location is recorded.
    cx.simulate_input("// yeni ");
    assert_eq!(wb.read_with(cx, |wb, _| wb.last_edit_for_test()).map(|(p, l, _)| (p.ends_with("lib.rs"), l)), Some((true, 1)));
    run(cx, "File: Compare Active File with Saved");
    wait_ticking(cx, "kaydedilmişle diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    wb.update_in(cx, |wb, window, cx| wb.close_diff_for_test(window, cx));

    // Switch to another file, then return to the last edit.
    open(&wb, &dir.join("README.md"), cx);
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("README.md"));
    run(cx, "Go: Last Edit Location");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("lib.rs"));

    // Compare with the clipboard.
    cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("pano içeriği\n".into())));
    run(cx, "File: Compare Active File with Clipboard");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    wb.update_in(cx, |wb, window, cx| wb.close_diff_for_test(window, cx));

    // Scratch file: in the settings folder; the same name gets numbered the second time.
    wb.update_in(cx, |wb, window, cx| wb.create_scratch_for_test("notes.md", window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("scratches/notes.md"));
    wb.update_in(cx, |wb, window, cx| wb.create_scratch_for_test("notes.md", window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("scratches/notes 2.md"));

    // No selection: the cursor's line is run in the terminal.
    open(&wb, &dir.join("run.sh"), cx);
    cx.simulate_keystrokes("cmd-up");
    run(cx, "Terminal: Run Selected Text");
    let term = wait_term(cx, &wb);
    wait_ticking(cx, "terminal çıktısı", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l == "run-42")));
}

fn wait_term(cx: &mut VisualTestContext, wb: &Entity<Workbench>) -> Entity<crate::terminal::TerminalView> {
    let mut found = None;
    wait_ticking(cx, "terminal", |cx| {
        found = wb.read_with(cx, |wb, _| wb.active_terminal());
        found.is_some()
    });
    found.unwrap()
}

/// Markdown preview: opens only for .md files and updates as you type.
#[gpui::test]
fn markdown_preview_follows_the_editor(cx: &mut TestAppContext) {
    let dir = project("md");
    std::fs::write(dir.join("doc.md"), "# Başlık\n\nmerhaba **dünya**\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("src/main.rs"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), None, "no preview for a rust file");

    open(&wb, &dir.join("doc.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("markdown", true)));
    let blocks = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.markdown_blocks_for_test(cx)).unwrap();
    assert_eq!(blocks(cx).len(), 2);
    assert!(matches!(&blocks(cx)[0], crate::markdown::Block::Heading(1, _)));

    // Escape restores the source editor, keeping the preview available.
    cx.simulate_keystrokes("escape");
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("markdown", false)));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("markdown", true)));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("markdown", false)));

    // Type in the restored editor: the preview sees the new block.
    cx.simulate_keystrokes("cmd-down");
    cx.simulate_input("- madde bir\n- madde iki\n");
    assert_eq!(blocks(cx).len(), 3);
    assert!(matches!(&blocks(cx)[2], crate::markdown::Block::List { items, .. } if items.len() == 2));
}

/// Markdown and standalone image tabs reject the same unsafe resources.
#[gpui::test]
fn markdown_images_obey_the_same_pixel_limit_as_image_tabs(cx: &mut TestAppContext) {
    let dir = project("md-image-limit");
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(30_000u32.to_be_bytes());
    png.extend(30_000u32.to_be_bytes());
    std::fs::write(dir.join("huge.png"), png).unwrap();
    std::fs::write(dir.join("ok.svg"), br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"/>"#).unwrap();
    std::fs::write(dir.join("doc.md"), "![huge](huge.png)\n\n![ok](ok.svg)\n\n![external](https://example.invalid/image.png)\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("doc.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    cx.run_until_parked();
    let images = wb.read_with(cx, |wb, cx| wb.markdown_images_for_test(cx)).unwrap();
    assert!(images["huge.png"].as_ref().is_err_and(|e| e.contains("too large")), "oversized image must not reach the decoder: {images:?}");
    assert_eq!(images["ok.svg"], Ok(()));
    assert!(images["https://example.invalid/image.png"].is_err(), "external resources stay offline");
}

/// The preview must overflow its viewport and respond to real wheel input.
#[gpui::test]
fn markdown_preview_scrolls_long_documents(cx: &mut TestAppContext) {
    let dir = project("md-scroll");
    let document = (0..100).map(|i| format!("## Section {i}\n\nParagraph {i}: some readable content.\n\n")).collect::<String>();
    std::fs::write(dir.join("long.md"), document).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("long.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    cx.run_until_parked();
    let scroll = wb.read_with(cx, |wb, cx| wb.markdown_scroll_for_test(cx)).unwrap();
    assert!(scroll.max_offset().height > gpui::px(1000.), "document has scrollable height: {:?}", scroll.max_offset());
    let at = scroll.bounds().center();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-300.))),
        modifiers: Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.run_until_parked();
    assert!(scroll.offset().y < gpui::px(-100.), "wheel moves the document: {:?}", scroll.offset());
}

/// List text retains the document width when placed inside the scrolling column.
#[gpui::test]
fn markdown_list_items_keep_their_width(cx: &mut TestAppContext) {
    let dir = project("md-list-layout");
    let item = "**No extensions.** There is no extension API and no store. Features that people usually install an extension for are built in. ".repeat(4);
    std::fs::write(dir.join("list.md"), format!("# Title\n\nIntroduction.\n\n- {item}\n- Second item.\n\nAfter the list.\n")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("list.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    cx.run_until_parked();
    let first = cx.debug_bounds("md2-item-0").unwrap();
    let row = cx.debug_bounds("md2-row-0").unwrap();
    let second = cx.debug_bounds("md2-row-1").unwrap();
    assert!(first.size.width > gpui::px(300.), "list text must use document width: {first:?}");
    assert!(first.size.height > gpui::px(44.), "long list text wraps to multiple readable lines: {first:?}");
    assert!(row.bottom() <= second.top(), "list rows must not overlap: {row:?}, {second:?}");
}

/// A click can begin and end before another frame is painted; links still need to open the local file.
#[gpui::test]
fn markdown_links_open_local_files_with_one_fast_left_click(cx: &mut TestAppContext) {
    let dir = project("md-link-click");
    std::fs::write(dir.join("README.md"), "# Title\n\n[a local document](child.md)\n").unwrap();
    std::fs::write(dir.join("child.md"), "# Linked document\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("README.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    cx.run_until_parked();
    let layouts = crate::markdown::text_layouts_for_test();
    let link = layouts.iter().find(|layout| layout.text() == "a local document").unwrap();
    let at = link.position_for_index(2).unwrap() + gpui::point(gpui::px(1.), gpui::px(1.));
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_editor_entity().unwrap().read(cx).name.to_string()), "README.md", "right-click does not follow a link");
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(gpui::point(gpui::px(0.), gpui::px(0.)), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_editor_entity().unwrap().read(cx).name.to_string()), "README.md", "releasing outside cancels the click");
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        wb.read_with(cx, |wb, cx| wb.active_editor_entity().unwrap().read(cx).name.to_string()),
        "README.md",
        "a cancelled gesture cannot follow a link on a later release"
    );
    // One window update deliberately allows no intervening paint between press and release.
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::none(), click_count: 1 }),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_editor_entity().unwrap().read(cx).name.to_string()), "child.md");
}

/// Guard actual shaped text, not just its parent bounds: narrow glyph wrapping can overflow a wide parent.
#[gpui::test]
fn markdown_long_paragraphs_wrap_inside_the_document(cx: &mut TestAppContext) {
    let dir = project("md-paragraph-layout");
    let paragraph = "The document remains readable while scrolling. ".repeat(20);
    let document =
        format!("# Title\n\n[Jump to details](#details)\n\n{}\n\n## Details\n\nEnd.\n", (0..8).map(|_| paragraph.clone()).collect::<Vec<_>>().join("\n\n"));
    std::fs::write(dir.join("README.md"), document).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("README.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    cx.run_until_parked();
    for width in [1280., 2560.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
        cx.run_until_parked();
        let document = cx.debug_bounds("markdown-document").unwrap();
        let layouts = crate::markdown::text_layouts_for_test();
        for text in layouts.iter().filter(|layout| layout.text().starts_with("The document remains")) {
            assert!(text.bounds().right() <= document.right(), "paragraph must not overflow the document: {:?}, {document:?}", text.bounds());
            let shaped = text.line_layout_for_index(0).unwrap();
            assert!(
                shaped.width() <= text.bounds().size.width + gpui::px(1.),
                "glyph wrapping must fit the assigned width, also after resize: {:?} > {:?}",
                shaped.width(),
                text.bounds()
            );
        }
    }
}

/// Guard actual shaped text, not just its parent bounds: narrow glyph wrapping can overflow a wide parent.
#[gpui::test]
fn markdown_readme_lists_shape_at_readable_width(cx: &mut TestAppContext) {
    let dir = project("md-readme-layout");
    std::fs::write(dir.join("README.md"), include_str!("../../../README.md")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("README.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Markdown: Open Preview", window, cx));
    cx.run_until_parked();
    let check = || {
        let layouts = crate::markdown::text_layouts_for_test();
        let list = layouts.iter().find(|layout| layout.text().starts_with("Complete core, optional extensions.")).expect("README list paragraph rendered");
        assert!(list.wrapped_text().lines().all(|line| line.chars().count() > 1), "list must not wrap character by character: {:?}", list.wrapped_text());
        assert!(list.wrapped_text().lines().count() < 12, "list should wrap to a few lines: {:?}", list.wrapped_text());
    };
    check();
    let scroll = wb.read_with(cx, |wb, cx| wb.markdown_scroll_for_test(cx)).unwrap();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll.bounds().center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-400.))),
        modifiers: Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.run_until_parked();
    check();
}

/// OUTLINE: the active file's symbols; updates as you type (with a short delay).
#[gpui::test]
fn outline_lists_symbols_of_the_active_file(cx: &mut TestAppContext) {
    let dir = project("outline");
    std::fs::write(dir.join("src/main.rs"), "fn main() {}\n\nstruct Point;\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("src/main.rs"), cx);
    wait_ticking(cx, "anahat", |cx| wb.read_with(cx, |wb, _| wb.outline_for_test() == [("main".to_string(), 0), ("Point".to_string(), 2)]));
    // README: heading.
    open(&wb, &dir.join("README.md"), cx);
    wait_ticking(cx, "md anahat", |cx| wb.read_with(cx, |wb, _| wb.outline_for_test() == [("proje".to_string(), 0)]));
    // Typing a new function updates the outline.
    open(&wb, &dir.join("src/main.rs"), cx);
    wb.update_in(cx, |wb, window, cx| wb.focus_active_editor(window, cx));
    cx.simulate_keystrokes("cmd-down");
    cx.simulate_input("fn extra() {}\n");
    cx.executor().advance_clock(std::time::Duration::from_millis(600));
    wait_ticking(cx, "güncel anahat", |cx| wb.read_with(cx, |wb, _| wb.outline_for_test().iter().any(|(n, _)| n == "extra")));
}

/// Local history: one copy per save; shown in TIMELINE and compared with the editor.
#[gpui::test]
fn local_history_keeps_a_copy_per_save(cx: &mut TestAppContext) {
    let dir = project("localhist");
    let data = std::env::temp_dir().join(format!("codeyard-data-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| wb.set_data_dir(data.clone()));
    open(&wb, &dir, cx);
    open(&wb, &dir.join("src/main.rs"), cx);
    cx.simulate_input("// bir\n");
    cx.simulate_keystrokes("cmd-s");
    wait_ticking(cx, "ilk kopya", |cx| wb.read_with(cx, |wb, _| wb.timeline_local_for_test() == 1));
    cx.simulate_input("// iki\n");
    cx.simulate_keystrokes("cmd-s");
    wait_ticking(cx, "ikinci kopya", |cx| wb.read_with(cx, |wb, _| wb.timeline_local_for_test() == 2));
    // The newest copy (0) is the same as the editor: the previous one (1) differs → the diff opens.
    wb.update_in(cx, |wb, window, cx| wb.open_local_history_for_test(1, window, cx));
    assert!(wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
}

/// Clicking the empty space below the tree clears the selection; a new file goes to the root.
#[gpui::test]
fn blank_space_in_the_tree_selects_the_root_for_new_files(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("blank")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "kök", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx).contains(&"src".to_string())));
    let main = dir.join("src/main.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_in_root(&main, window, cx));
    cx.run_until_parked();
    let src = dir.join("src").to_string_lossy().into_owned();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_target_for_test(cx)), Some(src), "selected file's folder");
    wb.update(cx, |wb, cx| wb.tree_clear_selection_for_test(cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_target_for_test(cx)), Some(dir.to_string_lossy().into_owned()), "blank area: root");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("File: New File…", window, cx));
    cx.simulate_input("rootfile.txt");
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "dosya", |_| dir.join("rootfile.txt").exists());
    assert!(!dir.join("src/rootfile.txt").exists());
}

/// `workbench.colorCustomizations` overrides the theme's color; removing it restores the old color.
#[gpui::test]
fn color_customizations_override_the_theme_and_associations_change_the_language(cx: &mut TestAppContext) {
    let cfg = config_dir("colors");
    let accent_before = theme::DARK.accent;
    std::fs::write(cfg.join("settings.jsonc"), r##"{ "theme": "midnight", "workbench.colorCustomizations": { "accent": "#ff8800", "[midnight]": { "text": "#eeeeee" } }, "files.associations": { "*.tpl": "html" } }"##).unwrap();
    let dir = project("colors");
    std::fs::write(dir.join("page.tpl"), "<p>merhaba</p>\n").unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    let p = cx.read_global(|t: &ActiveTheme, _| t.0);
    assert_eq!(p.accent, 0xff8800);
    assert_eq!(p.text, 0xeeeeee, "theme-specific color applies too");
    assert_ne!(accent_before, 0xff8800);
    // File association: a .tpl file is highlighted as HTML.
    open(&wb, &dir.join("page.tpl"), cx);
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).language())), Some("HTML"));
    // Delete the setting: colors return to the theme.
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "theme": "midnight" }"#).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    assert_eq!(cx.read_global(|t: &ActiveTheme, _| t.0).accent, accent_before);
    crate::editor::set_associations(Vec::new());
}

/// Snippets: prefix + Tab expands, Tab stops are navigated; the user snippet overrides the embedded one.
#[gpui::test]
fn snippets_expand_on_tab_and_walk_the_tab_stops(cx: &mut TestAppContext) {
    let _guard = crate::snippets::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("snip", b"");
    let file = dir.with_file_name("snip.rs");
    std::fs::write(&file, "").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.with_file_name(""), cx);
    open(&wb, &file, cx);
    let text = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).text())).unwrap();

    cx.simulate_input("fn");
    cx.simulate_keystrokes("tab");
    assert_eq!(text(cx), "fn name() -> () {\n    \n}", "expanded; first stop (name) selected");
    cx.simulate_input("hello");
    cx.simulate_keystrokes("tab");
    cx.simulate_input("x: u8");
    assert_eq!(text(cx), "fn hello(x: u8) -> () {\n    \n}");

    // Lines added later keep the indentation; expand the prefix inside a nested block.
    cx.simulate_keystrokes("escape cmd-down");
    cx.simulate_input("\nfor");
    cx.simulate_keystrokes("tab");
    cx.simulate_input("x");
    cx.simulate_keystrokes("tab");
    cx.simulate_input("xs");
    assert!(text(cx).ends_with("\nfor x in xs {\n    \n}"), "{}", text(cx));

    // If the prefix isn't found, Tab inserts indentation (first an open snippet session ends with Esc).
    cx.simulate_keystrokes("escape cmd-down");
    cx.simulate_input("\nzzzz");
    cx.simulate_keystrokes("tab");
    assert!(text(cx).ends_with("zzzz    "), "{:?}", text(cx));

    // The user snippet overrides the embedded one.
    crate::snippets::set_user(vec![crate::snippets::Snippet {
        lang: "rust".into(),
        prefix: "fn".into(),
        body: "// benim ${1:fn}".into(),
        description: String::new(),
        user: true,
        origin: None,
    }]);
    cx.simulate_keystrokes("escape cmd-down");
    cx.simulate_input("\nfn");
    cx.simulate_keystrokes("tab");
    assert!(text(cx).ends_with("\n// benim fn"), "{:?}", text(cx));
    crate::snippets::set_user(Vec::new());
}

/// Inline blame: author and subject of the cursor line; an unsaved line shows "Uncommitted".
#[gpui::test]
fn inline_blame_shows_the_author_of_the_cursor_line(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("blame") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_for(cx, "git", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some()));
    let main = dir.join("src/main.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_in_root(&main, window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_ticking(cx, "blame", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(400));
        ed.read_with(cx, |e, _| e.blame_for_test().is_some_and(|(l, t)| l == 0 && t.contains("Test,") && t.ends_with("· ilk")))
    });
    // Change the line (without saving): "Uncommitted".
    wb.update_in(cx, |wb, window, cx| wb.focus_active_editor(window, cx));
    cx.simulate_input("// ");
    wait_ticking(cx, "uncommitted", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(400));
        ed.read_with(cx, |e, _| e.blame_for_test().is_some_and(|(_, t)| t.contains("Uncommitted")))
    });
}

/// Shortcuts reference page: lists commands and their keys, searches, runs a command on click.
#[gpui::test]
fn shortcuts_reference_lists_searches_and_runs_commands(cx: &mut TestAppContext) {
    let dir = project("shortcuts");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("README.md"), cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Help: Keyboard Shortcuts Reference", window, cx));
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("shortcuts", true)));
    let page = wb.read_with(cx, |wb, _| wb.shortcuts_page_for_test()).unwrap();
    assert!(page.read_with(cx, |p, cx| p.visible_for_test(cx)).len() > 100);
    page.update(cx, |p, cx| p.set_query("block comment", cx));
    assert_eq!(page.read_with(cx, |p, cx| p.visible_for_test(cx)), ["Editor: Toggle Block Comment"]);
    // Also searchable by key.
    page.update(cx, |p, cx| p.set_query(if cfg!(target_os = "macos") { "⌃G" } else { "Ctrl+G" }, cx));
    assert!(page.read_with(cx, |p, cx| p.visible_for_test(cx)).contains(&"Go: Line…".to_string()));
    // Click closes the page; the command runs on the next frame after the editor draws (the test platform draws none).
    page.update(cx, |_, cx| cx.emit(crate::shortcuts_page::ShortcutsEvent::Run("View: Toggle Zen Mode".into())));
    cx.run_until_parked();
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), None);
}

/// JSON: serverless formatting (key order and comments preserved) and minification.
#[gpui::test]
fn json_documents_format_and_minify_without_a_language_server(cx: &mut TestAppContext) {
    let dir = project("json");
    std::fs::write(dir.join("a.json"), "{\"z\":1,\"a\":[1,2],\"o\":{}} \n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &dir.join("a.json"), cx);
    let text = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).text())).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Format Document", window, cx));
    cx.run_until_parked();
    assert_eq!(text(cx), "{\n    \"z\": 1,\n    \"a\": [\n        1,\n        2\n    ],\n    \"o\": {}\n}\n");
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).is_some_and(|n| n.contains("Formatted")));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Format Document", window, cx));
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).is_some_and(|n| n.contains("Already formatted")));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("JSON: Minify Document", window, cx));
    assert_eq!(text(cx).trim_end(), "{\"z\":1,\"a\":[1,2],\"o\":{}}");
    // Broken JSON: left untouched, an error is reported.
    wb.update_in(cx, |wb, window, cx| wb.focus_active_editor(window, cx));
    cx.simulate_keystrokes("cmd-up");
    cx.simulate_input("[");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Format Document", window, cx));
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).is_some_and(|n| n.contains("Could not format")));
    assert!(text(cx).starts_with("[{"));
}

/// "Save Current Colors as User Theme": colors plus customizations go to the themes folder and get selected.
#[gpui::test]
fn current_colors_can_be_saved_as_a_user_theme(cx: &mut TestAppContext) {
    let cfg = config_dir("savetheme");
    std::fs::write(cfg.join("settings.jsonc"), r##"{ "theme": "midnight", "workbench.colorCustomizations": { "accent": "#12ab34" } }"##).unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    wb.update_in(cx, |wb, window, cx| wb.save_theme_for_test("My  Fancy Theme!", window, cx));
    let file = cfg.join("themes/my-fancy-theme.json");
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(json["name"], "My  Fancy Theme!");
    assert_eq!(json["appearance"], "dark");
    assert_eq!(json["color"]["accent"], "#12ab34");
    assert!(json["color"].as_object().unwrap().len() >= 30, "all roles written");
    // The theme is in the list and selected; the file can be read back.
    assert_eq!(wb.read_with(cx, |wb, _| wb.current_theme_id().to_string()), "user:my-fancy-theme");
    let (themes, errors) = theme::themes::load_dir(&cfg.join("themes"));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(themes[0].palette.accent, 0x12ab34);
}

/// Palette: the most recently run command is at the top of the list the next time it opens.
#[gpui::test]
fn command_palette_puts_recently_used_commands_first(cx: &mut TestAppContext) {
    let dir = project("recentcmd");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("cmd-shift-p");
    cx.simulate_input("reverse lines");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-shift-p");
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert_eq!(labels[0], "Editor: Reverse Lines", "{:?}", &labels[..3.min(labels.len())]);
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_details(cx)).unwrap()[0], "recently used");
}

/// #186: the Pull Requests view is forge-neutral; its old GitHub name and "merge requests" still find it.
#[gpui::test]
fn command_palette_finds_the_pull_requests_view_by_any_forge_word(cx: &mut TestAppContext) {
    let dir = project("pullcmd");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    for query in ["pull requests", "merge requests", "github pull"] {
        cx.simulate_keystrokes("cmd-shift-p");
        cx.simulate_input(query);
        let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
        assert_eq!(labels.first().map(String::as_str), Some("Pull Requests: Show…"), "{query}: {:?}", &labels[..3.min(labels.len())]);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
    }
}

/// Fuzz: random commands + key sequences (many file types, pages, overlays, split groups).
/// Must not crash in any order; at each step Esc closes the overlays.
#[gpui::test]
fn torture_random_commands_and_keys_never_panic(cx: &mut TestAppContext) {
    let _guard = crate::snippets::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = config_dir("torture");
    let dir = std::fs::canonicalize(git_repo("torture").unwrap_or_else(|| project("torture"))).unwrap();
    std::fs::write(dir.join("a.json"), "{\"k\":[1,2,{\"x\":null}]}\n").unwrap();
    std::fs::write(dir.join("b.md"), "# T\n\n- a\n- b\n\n| x | y |\n|---|---|\n| 1 | 2 |\n").unwrap();
    std::fs::write(dir.join("c.py"), "def f():\n    return 1\n\nclass K:\n    pass\n").unwrap();
    std::fs::write(dir.join("d.txt"), "plain text\nline two\n").unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    open(&wb, &dir, cx);
    wait_for(cx, "ağaç", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx).len() > 3));
    let skip = [
        "Quit",
        "Open Folder",
        "Import",
        "New Window",
        "Clone",
        "Terminal",
        "Run ",
        "Restart",
        "Reveal",
        "Tasks",
        "Discard",
        "Delete",
        "Close Folder",
        "Connect",
        "Pull",
        "Push",
        "Fetch",
        "Sync",
        "Welcome",
        "Unlock",
        "Open Workspace",
        "Add Folder",
        "Install",
        "Start Debugging",
    ];
    let labels: Vec<String> = wb.read_with(cx, |wb, _| wb.command_labels_for_test()).into_iter().filter(|l| !skip.iter().any(|s| l.contains(s))).collect();
    assert!(labels.len() > 100, "{}", labels.len());
    let files = ["a.json", "b.md", "c.py", "d.txt", "README.md", "src/main.rs", "src/lib.rs"];
    let keys = [
        "escape",
        "enter",
        "tab",
        "shift-tab",
        "up",
        "down",
        "left",
        "right",
        "backspace",
        "cmd-z",
        "cmd-shift-z",
        "cmd-a",
        "cmd-c",
        "cmd-v",
        "cmd-x",
        "cmd-d",
        "cmd-/",
        "cmd-f",
        "cmd-b",
        "cmd-up",
        "cmd-down",
        "alt-up",
        "alt-down",
        "cmd-s",
        "cmd-p",
        "cmd-shift-p",
        "cmd-shift-f",
        "f4",
        "shift-f4",
        "cmd-e",
        "cmd-\\",
        "cmd-k z",
        "cmd-,",
    ];
    let words = ["fn", "for", "x", "( ", "\"", "ğüş", "\n", "{", "def", "# ", "- ", "😀", "    ", "$1"];
    let seeds: u64 = std::env::var("TORTURE_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    for seed in 1..=seeds {
        let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
        let mut next = |m: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % m as u64) as usize
        };
        for _step in 0..120 {
            match next(6) {
                0 | 1 => {
                    let l = labels[next(labels.len())].clone();
                    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(&l, window, cx));
                }
                2 => cx.simulate_keystrokes(keys[next(keys.len())]),
                3 => cx.simulate_input(words[next(words.len())]),
                4 => {
                    let f = dir.join(files[next(files.len())]).to_string_lossy().into_owned();
                    wb.update_in(cx, |wb, window, cx| wb.open_in_root(&f, window, cx));
                }
                _ => {
                    cx.simulate_keystrokes("escape");
                    let k = keys[next(keys.len())];
                    cx.simulate_keystrokes(k);
                }
            }
            cx.run_until_parked();
            // If an overlay stayed open, close it: the next step should be a real edit.
            if wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some() {
                cx.simulate_keystrokes("escape");
            }
        }
        cx.simulate_keystrokes("escape escape");
    }
    crate::snippets::set_user(Vec::new());
    crate::editor::set_associations(Vec::new());
}

/// Text tools: encoding conversions act on the selection, line tools on selected lines (whole document without one).
#[gpui::test]
fn text_tools_work_on_selections_and_lines(cx: &mut TestAppContext) {
    let dir = tmp("texttools", b"");
    let file = dir.with_file_name("araclar.txt");
    std::fs::write(&file, "item10\nItem2\n\n\nitem1\nhello world\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.with_file_name(""), cx);
    open(&wb, &file, cx);
    let text = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).text())).unwrap();
    let run = |cx: &mut VisualTestContext, cmd: &str| wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(cmd, window, cx));
    // Whole document: collapse blank lines to one, then natural sort (the blank line comes first).
    run(cx, "Editor: Collapse Blank Lines");
    assert_eq!(text(cx), "item10\nItem2\n\nitem1\nhello world\n");
    run(cx, "Editor: Remove Blank Lines");
    assert_eq!(text(cx), "item10\nItem2\nitem1\nhello world\n");
    run(cx, "Editor: Sort Lines (Natural)");
    assert_eq!(text(cx), "hello world\nitem1\nItem2\nitem10\n");
    run(cx, "Editor: Sort Lines (Case-Insensitive)");
    assert_eq!(text(cx), "hello world\nitem1\nitem10\nItem2\n", "harf harf: item1, item10, Item2");

    // Base64 / URL on a selection: round trip; invalid input is left untouched.
    wb.update_in(cx, |wb, window, cx| wb.focus_active_editor(window, cx));
    let original = text(cx);
    cx.simulate_keystrokes("cmd-a");
    run(cx, "Editor: Base64 Encode");
    assert_ne!(text(cx), original);
    cx.simulate_keystrokes("cmd-a");
    run(cx, "Editor: Base64 Decode");
    assert_eq!(text(cx), original);
    cx.simulate_keystrokes("cmd-a");
    run(cx, "Editor: URL Encode");
    assert!(text(cx).starts_with("hello%20world%0Aitem1"), "{}", text(cx));
    cx.simulate_keystrokes("cmd-a");
    run(cx, "Editor: URL Decode");
    assert_eq!(text(cx), original);
    cx.simulate_keystrokes("cmd-a");
    run(cx, "Editor: Base64 Decode"); // plain text is not Base64
    assert_eq!(text(cx), original);
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).is_some_and(|n| n.contains("Not valid Base64")));
}

#[gpui::test]
fn settings_file_is_applied(cx: &mut TestAppContext) {
    let cfg = config_dir("apply");
    std::fs::write(
        cfg.join("settings.jsonc"),
        r#"{
          // yorumlar serbest
          "theme": "ember",
          "files.exclude": [".git", "*.md"],
          "editor.tabSize": 2,
          "editor.insertSpaces": true,
          "files.trimTrailingWhitespace": true,
          "files.insertFinalNewline": true,
        }"#,
    )
    .unwrap();
    let dir = project("settings");
    let (wb, cx) = setup_with_config(cx, &cfg);
    let ember = theme::themes::builtin().into_iter().find(|t| t.id == "ember").unwrap().palette;
    assert_eq!(cx.update(|_, cx| cx.global::<ActiveTheme>().0), ember);

    open(&wb, &dir, cx);
    let names = wb.read_with(cx, |wb, cx| wb.tree_names(cx));
    assert!(!names.contains(&"README.md".to_string()), "*.md gizlenmeli: {names:?}");
    assert!(names.contains(&"src".to_string()));

    open(&wb, &dir.join("src/main.rs"), cx);
    cx.simulate_keystrokes("cmd-down tab");
    cx.simulate_input("x   ");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    // Tab = 2 spaces; saving strips trailing whitespace and adds a final newline.
    assert_eq!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap(), "fn main() {}\n  x\n");
}

#[gpui::test]
fn user_keybindings_and_live_reload(cx: &mut TestAppContext) {
    let cfg = config_dir("keys");
    let m = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    std::fs::write(cfg.join("keybindings.jsonc"), format!(r#"[ {{ "key": "{m}-1", "command": "workbench.quickOpen" }}, {{ "key": "{m}-2", "command": "editor.sav" }}, {{ "key": "ctrl-ö", "command": "workbench.commandPalette" }} ]"#)).unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    let notice = wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default();
    assert!(notice.contains("did you mean \"editor.save\""), "{notice}");

    cx.simulate_keystrokes("cmd-1");
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()), "user shortcut should work");
    cx.simulate_keystrokes("escape");
    // A Turkish keyboard key can also be bound (README example).
    cx.simulate_keystrokes("ctrl-ö");
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()), "ctrl-ö should be bindable");
    cx.simulate_keystrokes("escape");

    // Modify the file: the once-a-second check picks it up, the theme changes immediately.
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "theme": "paper" }"#).unwrap();
    cx.executor().advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    let paper = theme::themes::builtin().into_iter().find(|t| t.id == "paper").unwrap().palette;
    assert_eq!(cx.update(|_, cx| cx.global::<ActiveTheme>().0), paper);
}

#[gpui::test]
fn theme_picker_writes_settings_file_keeping_comments(cx: &mut TestAppContext) {
    let cfg = config_dir("persist");
    std::fs::write(cfg.join("settings.jsonc"), "{\n  // benim yorumum\n  \"theme\": \"auto\",\n  \"editor.fontSize\": 14,\n}\n").unwrap();
    let (_wb, cx) = setup_with_config(cx, &cfg);
    cx.simulate_keystrokes("cmd-k cmd-t");
    cx.simulate_input("graphite");
    cx.simulate_keystrokes("enter");
    let text = std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap();
    assert_eq!(text, "{\n  // benim yorumum\n  \"theme\": \"graphite\",\n  \"editor.fontSize\": 14,\n}\n");
}

#[gpui::test]
fn external_changes_reload_clean_files_and_flag_dirty_ones(cx: &mut TestAppContext) {
    let dir = project("watch");
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("src/lib.rs"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    cx.simulate_keystrokes("down down"); // cursor on line 3
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(200)); // let the watch set be sent

    // A clean file changed externally: it is silently reloaded, the cursor stays in place.
    std::fs::write(dir.join("src/lib.rs"), "bir\niki\nüç\n").unwrap();
    wait_for(cx, "yeniden yükleme", |cx| ed.read_with(cx, |e, _| e.text() == "bir\niki\nüç\n"));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)), Some((3, 1)));

    // A dirty file changed externally: conflict banner, disk is left alone.
    cx.simulate_input("benim ");
    std::fs::write(dir.join("src/lib.rs"), "başkası\n").unwrap();
    wait_for(cx, "çakışma", |cx| ed.read_with(cx, |e, _| e.disk_state().0));
    assert_eq!(std::fs::read_to_string(dir.join("src/lib.rs")).unwrap(), "başkası\n");
}

#[gpui::test]
fn own_saves_are_not_external_changes(cx: &mut TestAppContext) {
    let dir = project("own-save");
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("src/main.rs"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(200));
    cx.simulate_input("// a\n");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    // Wait longer than one polling round.
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(2500) {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(ed.read_with(cx, |e, _| e.disk_state()), (false, false));
    assert!(!wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
}

#[gpui::test]
fn tree_follows_external_creates_and_deletes(cx: &mut TestAppContext) {
    let dir = project("watch-tree");
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("src/main.rs"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(200));

    std::fs::write(dir.join("src/yeni.rs"), "").unwrap();
    wait_for(cx, "ağaçta yeni dosya", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx).contains(&"yeni.rs".to_string())));

    std::fs::remove_file(dir.join("src/main.rs")).unwrap();
    wait_for(cx, "silindi uyarısı", |cx| ed.read_with(cx, |e, _| e.disk_state().1));
    cx.simulate_keystrokes("cmd-s");
    wait_for(cx, "yeniden oluştu", |_| dir.join("src/main.rs").exists());
}

/// A workbench with a persistent database and backup folder (restart tests).
fn setup_persistent<'a>(cx: &'a mut TestAppContext, home: &std::path::Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings::default());
    });
    let db = home.join("state.db");
    let data = home.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| {
        let mut wb = Workbench::with_state(state::State::open(&db).ok(), None, window, cx);
        wb.set_data_dir(data);
        wb
    });
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

/// "Quit" the app and reopen it in the same window with the same database
/// (the root view changes: it is rendered and receives keys).
fn restart(wb: Entity<Workbench>, cx: &mut VisualTestContext, home: &std::path::Path) -> Entity<Workbench> {
    assert!(wb.update(cx, |wb, cx| wb.prepare_quit(cx)), "hot exit: soru sormadan kapanabilmeli");
    drop(wb);
    let db = home.join("state.db");
    let data = home.to_path_buf();
    let new = cx.update(|window, cx| {
        window.replace_root(cx, |window, cx| {
            let mut w = Workbench::with_state(state::State::open(&db).ok(), None, window, cx);
            w.set_data_dir(data);
            w
        })
    });
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().startup = crate::settings::Startup::Continue);
    new.update_in(cx, |w, window, cx| {
        window.focus(&w.focus_handle(cx));
        w.restore_last(window, cx);
    });
    cx.run_until_parked();
    new
}

fn home_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-home-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[gpui::test]
fn session_restores_tabs_active_tab_and_cursor(cx: &mut TestAppContext) {
    let home = home_dir("session");
    let dir = project("session");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir.join("src/main.rs"), cx);
    open(&wb, &dir.join("src/lib.rs"), cx);
    cx.simulate_keystrokes("down down down right right"); // line 4, column 3
    cx.run_until_parked();

    let wb2 = restart(wb, cx, &home);
    let paths: Vec<String> = wb2.read_with(cx, |w, cx| w.open_paths(cx));
    assert_eq!(paths.len(), 2, "{paths:?}");
    assert!(paths[0].ends_with("src/main.rs") && paths[1].ends_with("src/lib.rs"), "order should be kept: {paths:?}");
    assert!(wb2.read_with(cx, |w, cx| w.active_path(cx)).unwrap().ends_with("src/lib.rs"));
    assert_eq!(wb2.read_with(cx, |w, cx| w.active_cursor(cx)), Some((4, 3)));
}

#[gpui::test]
fn hot_exit_keeps_unsaved_changes_across_restart(cx: &mut TestAppContext) {
    let home = home_dir("hotexit");
    let dir = project("hotexit");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir.join("src/main.rs"), cx);
    cx.simulate_input("// kaydedilmedi\n");
    cx.executor().advance_clock(std::time::Duration::from_secs(2)); // backup timer
    cx.run_until_parked();

    let wb2 = restart(wb, cx, &home);
    assert_eq!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap(), "fn main() {}\n", "disk ellenmez");
    let ed = wb2.read_with(cx, |w, _| w.active_editor_entity()).expect("tab should come back");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "// kaydedilmedi\nfn main() {}\n");
    assert!(wb2.read_with(cx, |w, cx| w.any_dirty(cx)), "should come back unsaved");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap(), "// kaydedilmedi\nfn main() {}\n");
}

#[gpui::test]
fn backup_conflicts_if_disk_changed_and_discard_is_final(cx: &mut TestAppContext) {
    let home = home_dir("backup-conflict");
    let dir = project("backup-conflict");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir.join("src/main.rs"), cx);
    cx.simulate_input("benim ");
    cx.executor().advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    // The file was changed by someone else while the app was closed.
    std::fs::write(dir.join("src/main.rs"), "başkası\n").unwrap();

    let wb2 = restart(wb, cx, &home);
    let ed = wb2.read_with(cx, |w, _| w.active_editor_entity()).unwrap();
    assert!(ed.read_with(cx, |e, _| e.disk_state()).0, "conflict banner");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "benim fn main() {}\n", "user's content is kept");

    // Close deliberately ("Don't Save"): it does not come back on reopen.
    cx.simulate_keystrokes("cmd-w");
    cx.simulate_prompt_answer("Don't Save");
    cx.run_until_parked();
    let wb3 = restart(wb2, cx, &home);
    assert_eq!(wb3.read_with(cx, |w, cx| w.open_paths(cx)), Vec::<String>::new());
}

#[gpui::test]
fn preview_tab_is_reused_until_edited(cx: &mut TestAppContext) {
    let dir = project("preview");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let p = |f: &str| dir.join(f).to_string_lossy().into_owned();
    wb.update_in(cx, |w, window, cx| w.preview_file(&p("README.md"), window, cx));
    cx.run_until_parked();
    wb.update_in(cx, |w, window, cx| w.preview_file(&p("src/main.rs"), window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |w, cx| w.tab_flags(cx)), [("main.rs".to_string(), true, false)], "preview is a single tab");

    cx.simulate_input("x"); // edit: becomes persistent
    wb.update_in(cx, |w, window, cx| w.preview_file(&p("src/lib.rs"), window, cx));
    cx.run_until_parked();
    let flags = wb.read_with(cx, |w, cx| w.tab_flags(cx));
    assert_eq!(flags, [("main.rs".to_string(), false, false), ("lib.rs".to_string(), true, false)]);
}

#[gpui::test]
fn pin_and_bulk_close_keep_pinned_and_dirty_tabs(cx: &mut TestAppContext) {
    let home = home_dir("tabs");
    // Canonical paths, as in the tree and ⌘P (/var → /private/var symlink).
    let dir = std::fs::canonicalize(project("tabs")).unwrap();
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    let open_tab = |cx: &mut VisualTestContext, f: &str| {
        let p = dir.join(f).to_string_lossy().into_owned();
        wb.update_in(cx, |w, window, cx| w.open_in_root(&p, window, cx));
        cx.run_until_parked();
    };
    for f in ["README.md", "src/main.rs", "src/lib.rs", ".gitignore"] {
        open_tab(cx, f);
    }
    // Pin lib.rs: it moves to the left.
    wb.update_in(cx, |w, window, cx| w.tab_command(2, "pin", window, cx));
    let names = |cx: &mut VisualTestContext| wb.read_with(cx, |w, cx| w.tab_flags(cx)).into_iter().map(|(n, _, _)| n).collect::<Vec<_>>();
    assert_eq!(names(cx), ["lib.rs", "README.md", "main.rs", ".gitignore"]);

    // Make main.rs dirty, then close those that are not in .gitignore.
    open_tab(cx, "src/main.rs");
    cx.simulate_input("kirli ");
    wb.update_in(cx, |w, window, cx| w.tab_command(3, "close-others", window, cx));
    assert_eq!(names(cx), ["lib.rs", "main.rs", ".gitignore"], "pinned and dirty tabs stay");
    assert!(wb.read_with(cx, |w, _| w.notice_text()).unwrap_or_default().contains("unsaved"));

    // Pinned state is stored in the session.
    let wb2 = restart(wb, cx, &home);
    let flags = wb2.read_with(cx, |w, cx| w.tab_flags(cx));
    assert!(flags.iter().any(|(n, _, pinned)| n == "lib.rs" && *pinned), "{flags:?}");
}

#[cfg(unix)]
#[gpui::test]
fn ssh_prompts_ask_in_window(cx: &mut TestAppContext) {
    use crate::workbench::{AskKind, AskRequest};
    let (wb, cx) = setup(cx);
    let ask = |kind, prompt: &str| {
        let (tx, rx) = std::sync::mpsc::channel();
        (AskRequest { kind, prompt: prompt.into() }, tx, rx)
    };
    // Two questions at once: one is shown, the other is queued.
    let (key_q, key_tx, key_rx) = ask(
        AskKind::HostKey,
        "The authenticity of host 'node-dev' can't be established.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?",
    );
    let (pw_q, pw_tx, pw_rx) = ask(AskKind::Secret, "deploy@node-dev's password:");
    wb.update_in(cx, |wb, window, cx| {
        wb.ask(key_q, key_tx, window, cx);
        wb.ask(pw_q, pw_tx, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.pending_asks()), (true, 1));
    cx.simulate_keystrokes("enter"); // first option: trust and connect
    assert_eq!(key_rx.try_recv().unwrap().as_deref(), Some("yes"));
    // The next password question opens automatically; spaces are preserved.
    assert_eq!(wb.read_with(cx, |wb, _| wb.pending_asks()), (true, 0));
    cx.simulate_input(" gizli şifre ");
    cx.simulate_keystrokes("enter");
    assert_eq!(pw_rx.try_recv().unwrap().as_deref(), Some(" gizli şifre "));
    assert_eq!(wb.read_with(cx, |wb, _| wb.pending_asks()), (false, 0));

    // Esc: give up (ssh counts it as a failure). "No": the host key is rejected.
    let (q, tx, rx) = ask(AskKind::Secret, "Enter passphrase for key '/k':");
    wb.update_in(cx, |wb, window, cx| wb.ask(q, tx, window, cx));
    cx.simulate_keystrokes("escape");
    assert_eq!(rx.try_recv().unwrap(), None);
    let (q, tx, rx) = ask(AskKind::HostKey, "continue connecting (yes/no)?");
    wb.update_in(cx, |wb, window, cx| wb.ask(q, tx, window, cx));
    cx.simulate_keystrokes("down enter");
    assert_eq!(rx.try_recv().unwrap().as_deref(), Some("no"));
}

#[gpui::test]
fn multi_cursor_editing(cx: &mut TestAppContext) {
    let file = tmp("multi", b"alfa\nbeta\ngama\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let text = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.text());

    // ⌥⌘↓ twice: three cursors; typing goes to all of them, as a single undo step.
    cx.simulate_keystrokes("alt-cmd-down alt-cmd-down");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_count()), 3);
    cx.simulate_input("->");
    assert_eq!(text(cx), "->alfa\n->beta\n->gama\n");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(cx), "alfa\nbeta\ngama\n");

    // Select every line, copy, paste at the line ends: each cursor gets its own line.
    cx.simulate_keystrokes("shift-end cmd-c end cmd-v");
    assert_eq!(text(cx), "alfaalfa\nbetabeta\ngamagama\n");
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("alfa\nbeta\ngama"));

    // Esc: back to a single cursor.
    cx.simulate_keystrokes("escape");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_count()), 1);
    cx.simulate_keystrokes("cmd-z");

    // ⌘D: select the word, add the following occurrences, replace all.
    let file = tmp("multi2", b"foo bar foo\nfoo\n");
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let text = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.text());
    assert_eq!(text(cx), "foo bar foo\nfoo\n");
    cx.simulate_keystrokes("cmd-up cmd-d cmd-d cmd-d");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_count()), 3);
    cx.simulate_input("baz");
    assert_eq!(text(cx), "baz bar baz\nbaz\n");
    cx.simulate_keystrokes("escape cmd-z");
    assert_eq!(text(cx), "foo bar foo\nfoo\n");

    // ⌥+click: add a cursor; a second ⌥+click at the same spot removes it.
    cx.simulate_keystrokes("cmd-up");
    let at = ed.read_with(cx, |e, _| e.point_for(8)).expect("line should be drawn");
    cx.simulate_click(at, Modifiers::alt());
    assert_eq!(ed.read_with(cx, |e, _| e.cursors()), [0, 8]);
    cx.simulate_input(">");
    assert_eq!(text(cx), ">foo bar >foo\nfoo\n");
    let at = ed.read_with(cx, |e, _| e.point_for(10)).unwrap();
    cx.simulate_click(at, Modifiers::alt());
    assert_eq!(ed.read_with(cx, |e, _| e.cursors()), [1]);
}

#[gpui::test]
fn find_and_replace_in_file(cx: &mut TestAppContext) {
    let file = tmp("find", "İğne iğne\nfoo iğne\n".as_bytes());
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let text = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.text());
    let status = |cx: &mut VisualTestContext| {
        let bar = ed.read_with(cx, |e, _| e.find_bar()).expect("find bar open");
        bar.read_with(cx, |b, _| b.status_text().map(str::to_string))
    };

    // ⌘F + typing: case-insensitive with Turkish casing, the first match from the cursor is selected.
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_input("iğne");
    assert_eq!(status(cx).as_deref(), Some("1 of 3"));
    cx.simulate_keystrokes("enter");
    assert_eq!(status(cx).as_deref(), Some("2 of 3"));
    cx.simulate_keystrokes("enter enter");
    assert_eq!(status(cx).as_deref(), Some("1 of 3"), "wraps to the start at the end");
    cx.simulate_keystrokes("shift-enter");
    assert_eq!(status(cx).as_deref(), Some("3 of 3"));
    // While in the bar, Tab/⌘Z don't go to the buffer.
    cx.simulate_keystrokes("tab cmd-z");
    assert_eq!(text(cx), "İğne iğne\nfoo iğne\n");
    // Case-sensitive: only those typed in lowercase.
    cx.simulate_keystrokes("alt-c");
    assert_eq!(status(cx).as_deref(), Some("2 of 2"), "the selected match was the lowercase third one");
    cx.simulate_keystrokes("alt-c");

    // ⌥⌘F: replace field; ↵ replaces one, ⌥⌘↵ replaces all.
    cx.simulate_keystrokes("alt-cmd-f");
    cx.simulate_input("iplik");
    cx.simulate_keystrokes("enter"); // the selected (third) one is replaced, moves on to the next (wraps to the start)
    assert_eq!(text(cx), "İğne iğne\nfoo iplik\n");
    assert_eq!(status(cx).as_deref(), Some("1 of 2"));
    cx.simulate_keystrokes("alt-cmd-enter");
    assert_eq!(text(cx), "iplik iplik\nfoo iplik\n");
    assert_eq!(status(cx).as_deref(), Some("Replaced 2"));
    // Esc closes; replace-all is a single undo step.
    cx.simulate_keystrokes("escape");
    assert!(ed.read_with(cx, |e, _| e.find_bar()).is_none());
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(cx), "İğne iğne\nfoo iplik\n");

    // Regex + groups; an invalid pattern is shown as an error.
    cx.simulate_keystrokes("cmd-f alt-r");
    let bar = ed.read_with(cx, |e, _| e.find_bar()).unwrap();
    bar.update(cx, |b, cx| b.set_query("(", cx));
    cx.run_until_parked();
    assert!(status(cx).is_some_and(|s| !s.contains("of")), "error: {:?}", status(cx));
    bar.update(cx, |b, cx| {
        b.set_query(r"(\w+) (\w+)$", cx);
        b.set_replacement("$2 $1", cx);
    });
    cx.run_until_parked();
    assert!(status(cx).is_some_and(|s| s.ends_with(" of 2")), "{:?}", status(cx));
    cx.simulate_keystrokes("alt-cmd-f alt-cmd-enter");
    assert_eq!(text(cx), "iğne İğne\niplik foo\n");
}

#[gpui::test]
fn word_keys_clicks_and_undo_to_clean(cx: &mut TestAppContext) {
    let file = tmp("words", "let şeker_2 = foo(bar);\nikinci satır\n".as_bytes());
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let text = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.text());

    cx.simulate_keystrokes("alt-right alt-right alt-shift-left");
    assert_eq!(ed.read_with(cx, |e, _| e.selected_text()).as_deref(), Some("şeker_2"));
    cx.simulate_keystrokes("right alt-backspace");
    assert_eq!(text(cx), "let  = foo(bar);\nikinci satır\n");
    assert!(ed.read_with(cx, |e, _| e.is_dirty()));
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(cx), "let şeker_2 = foo(bar);\nikinci satır\n");
    assert!(!ed.read_with(cx, |e, _| e.is_dirty()), "clean again after undo returns to the saved state");

    // Double-click: word; triple-click: line.
    let at = ed.read_with(cx, |e, _| e.point_for(32)).unwrap(); // inside "satır"
    cx.simulate_event(gpui::MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::none(), click_count: 2, first_mouse: false });
    cx.simulate_event(gpui::MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::none(), click_count: 2 });
    assert_eq!(ed.read_with(cx, |e, _| e.selected_text()).as_deref(), Some("satır"));
    cx.simulate_event(gpui::MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::none(), click_count: 3, first_mouse: false });
    cx.simulate_event(gpui::MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::none(), click_count: 3 });
    assert_eq!(ed.read_with(cx, |e, _| e.selected_text()).as_deref(), Some("ikinci satır\n"));
}

/// A small committed repo; `None` if `git` is missing (the test is skipped).
pub(crate) fn git_repo(name: &str) -> Option<PathBuf> {
    let dir = project(name);
    let run = |args: &[&str]| crate::test_git::git().args(args).current_dir(&dir).output().ok().filter(|o| o.status.success());
    run(&["init", "-q"])?;
    run(&["add", "-A"])?;
    run(&["commit", "-q", "-m", "ilk"])?;
    Some(dir)
}

#[gpui::test]
fn git_status_marks_tree_and_gutter(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("gitstatus") else { return };
    let dir = std::fs::canonicalize(dir).unwrap(); // the path is canonicalized when the folder is opened
    std::fs::write(dir.join("src/lib.rs"), (1..=10).map(|i| if i == 3 { "// değişti\n".to_string() } else { format!("// satır {i}\n") }).collect::<String>())
        .unwrap();
    std::fs::write(dir.join("yeni.txt"), "yeni\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_for(cx, "git durumu", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|g| g.changes.len() == 2)));
    let st = wb.read_with(cx, |wb, _| wb.git_status()).unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    let marks = st.marks();
    let abs = |p: &str| dir.join(p).to_string_lossy().into_owned();
    assert_eq!(marks.get(&abs("src/lib.rs")), Some(&crate::git::Mark::Modified));
    assert_eq!(marks.get(&abs("src")), Some(&crate::git::Mark::Modified), "folder mark");
    assert_eq!(marks.get(&abs("yeni.txt")), Some(&crate::git::Mark::Untracked));

    // Gutter: changed line; line added by typing; state refreshes on save.
    open(&wb, &dir.join("src/lib.rs"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    use crate::git::LineMark;
    wait_for(cx, "gutter", |cx| ed.read_with(cx, |e, _| e.git_marks().get(2).copied().flatten() == Some(LineMark::Modified)));
    cx.simulate_keystrokes("cmd-down");
    cx.simulate_input("// eklendi\n");
    cx.executor().advance_clock(std::time::Duration::from_millis(300));
    wait_for(cx, "eklenen satır", |cx| ed.read_with(cx, |e, _| e.git_marks().get(10).copied().flatten() == Some(LineMark::Added)));

    // External commit (as from a terminal): .git/index is watched, the marks go away.
    let ok = crate::test_git::git().args(["commit", "-qam", "ikinci"]).current_dir(&dir).status().unwrap().success();
    assert!(ok);
    wait_for(cx, "commit sonrası", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|g| g.changes.len() == 1))
    });
}

/// The graph's labels follow refs that move without HEAD moving: a new branch checked out on the same commit (with a slash
/// in its name, which must stay a local branch) and a new tag.
#[gpui::test]
fn source_control_graph_follows_refs_that_do_not_move_head(cx: &mut TestAppContext) {
    use crate::graph::RefKind;
    let Some(dir) = git_repo("scmgraphrefs") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    let refs = |cx: &mut VisualTestContext| {
        scm.read_with(cx, |s, _| s.graph_commit(0).map(|c| c.refs.into_iter().map(|r| (r.name, r.kind)).collect::<Vec<_>>()).unwrap_or_default())
    };
    wait_ticking(cx, "graph", |cx| refs(cx).contains(&("main".into(), RefKind::Head)));
    git_in(&dir, &["checkout", "-q", "-b", "feature/x"]);
    wait_ticking(cx, "HEAD label moved to the new branch", |cx| refs(cx).contains(&("feature/x".into(), RefKind::Head)));
    assert!(refs(cx).contains(&("main".into(), RefKind::Local)), "main is a local branch now: {:?}", refs(cx));
    git_in(&dir, &["branch", "fix/y"]);
    git_in(&dir, &["tag", "v9"]);
    wait_ticking(cx, "new branch and tag", |cx| {
        let r = refs(cx);
        r.contains(&("fix/y".into(), RefKind::Local)) && r.contains(&("v9".into(), RefKind::Tag))
    });
}

/// The GRAPH "…" menu: view and filters are checked items that change the panel; the remote actions are only offered
/// when the repository has a remote.
#[gpui::test]
fn source_control_graph_menu_toggles_view_and_filters(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("scmgraphmenu") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "graph", |cx| !scm.read_with(cx, |s, _| s.graph_subjects()).is_empty());
    let open_menu = |cx: &mut VisualTestContext| {
        scm.update(cx, |_, cx| cx.emit(crate::scm::ScmEvent::GraphMenu(gpui::point(gpui::px(10.), gpui::px(10.)))));
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).expect("graph menu")
    };
    let labels = open_menu(cx);
    for want in ["Graph", "List", "Days", "All Branches", "Hide Merge Commits", "Search Commits", "Create Branch…", "Reload history"] {
        assert!(labels.iter().any(|l| l == want), "{want} missing in {labels:?}");
    }
    assert!(!labels.iter().any(|l| l == "Push"), "no remote, no Push: {labels:?}");
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Hide Merge Commits", cx));
    cx.run_until_parked();
    assert!(scm.read_with(cx, |s, cx| s.graph_menu_state(cx).hide_merges));
    open_menu(cx);
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Days", cx));
    cx.run_until_parked();
    assert_eq!(scm.read_with(cx, |s, cx| s.graph_menu_state(cx).view), crate::graph::GraphView::Dates);
    open_menu(cx);
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("All Branches", cx));
    cx.run_until_parked();
    assert!(!scm.read_with(cx, |s, cx| s.graph_menu_state(cx).all_branches));
}

/// Source control graph: branches, merge, tag; clicking a commit shows its files.
#[gpui::test]
fn source_control_graph_shows_branches_and_opens_commit_files(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("scmgraph") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    git_in(&dir, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(dir.join("f.txt"), "f\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "feature work"]);
    git_in(&dir, &["checkout", "-q", "main"]);
    std::fs::write(dir.join("g.txt"), "g\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "main work"]);
    git_in(&dir, &["merge", "-q", "--no-ff", "feature", "-m", "Merge feature"]);
    git_in(&dir, &["tag", "v1"]);

    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "grafik", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() == 4));
    let subjects = scm.read_with(cx, |s, _| s.graph_subjects());
    assert_eq!(subjects[0], "Merge feature", "newest first");
    assert_eq!(subjects[3], "ilk", "root at the bottom");
    assert!(subjects.contains(&"feature work".to_string()) && subjects.contains(&"main work".to_string()));
    assert_eq!(scm.read_with(cx, |s, _| s.graph_cols()), 2, "two branches, two lanes");
    assert!(scm.read_with(cx, |s, _| s.identity_for_test()).is_some(), "git identity read (configured or not)");
    let merge = scm.read_with(cx, |s, _| s.graph_commit(0)).unwrap();
    assert_eq!(merge.parents.len(), 2);
    assert!(merge.refs.iter().any(|r| r.name == "v1" && r.kind == crate::graph::RefKind::Tag));
    assert!(merge.refs.iter().any(|r| r.name == "main" && r.kind == crate::graph::RefKind::Head));

    // External new commit: the graph refreshes when HEAD changes.
    std::fs::write(dir.join("h.txt"), "h\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "sonradan"]);
    wait_ticking(cx, "yeni commit grafikte", |cx| scm.read_with(cx, |s, _| s.graph_subjects().first().is_some_and(|x| x == "sonradan")));

    // Click the "main work" commit: its file is listed, selecting it opens a side-by-side diff.
    let ix = scm.read_with(cx, |s, _| s.graph_subjects().iter().position(|x| x == "main work")).unwrap();
    let commit = scm.read_with(cx, |s, _| s.graph_commit(ix)).unwrap();
    scm.update(cx, |_, cx| cx.emit(crate::scm::ScmEvent::OpenCommit(commit)));
    wait_ticking(cx, "commit dosyaları", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some_and(|l| l == ["g.txt"]));
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "commit diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
}

/// Graph: commit details, search filter, branch scope, and commit operations (branch, tag,
/// cherry-pick, revert, reset).
#[gpui::test]
fn source_control_graph_details_filter_scope_and_commit_operations(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("scmgraph2") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    git_in(&dir, &["config", "user.name", "Test"]);
    git_in(&dir, &["config", "user.email", "t@example.com"]);
    git_in(&dir, &["checkout", "-q", "-b", "side"]);
    std::fs::write(dir.join("s.txt"), "s\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "side work"]);
    git_in(&dir, &["checkout", "-q", "main"]);
    std::fs::write(dir.join("g.txt"), "g1\ng2\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "main work\n\nlong body"]);

    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-shift-g");
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "grafik", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() == 3));

    // Details: message body, file and line counts.
    let find = |cx: &mut VisualTestContext, subject: &str| {
        let ix = scm.read_with(cx, |s, _| s.graph_subjects().iter().position(|x| x == subject)).unwrap();
        scm.read_with(cx, |s, _| s.graph_commit(ix)).unwrap()
    };
    let main_work = find(cx, "main work");
    scm.update(cx, |s, cx| s.select_commit(main_work.clone(), cx));
    wait_ticking(cx, "ayrıntı", |cx| scm.read_with(cx, |s, _| s.detail_for_test().is_some()));
    let d = scm.read_with(cx, |s, _| s.detail_for_test()).unwrap();
    assert_eq!(d.message, "main work\n\nlong body");
    assert_eq!(d.files.len(), 1);
    assert_eq!((d.files[0].path.as_str(), d.files[0].status), ("g.txt", 'A'));
    assert_eq!(d.totals(), (2, 0));
    assert_eq!(scm.read_with(cx, |s, _| s.selected_hash()), Some(main_work.hash.clone()));

    // Search: matches subject, author, and branch name.
    scm.update(cx, |s, cx| s.set_query_for_test("side", cx));
    assert_eq!(scm.read_with(cx, |s, cx| s.visible_subjects(cx)), ["side work"]);
    scm.update(cx, |s, cx| s.set_query_for_test("", cx));
    assert_eq!(scm.read_with(cx, |s, cx| s.visible_subjects(cx)).len(), 3);

    // Scope: only the current branch does not show "side work".
    scm.update(cx, |s, cx| s.set_scope_for_test(true, cx));
    wait_ticking(cx, "dal kapsamı", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() == 2));
    assert!(!scm.read_with(cx, |s, _| s.graph_subjects()).contains(&"side work".to_string()));
    scm.update(cx, |s, cx| s.set_scope_for_test(false, cx));
    wait_ticking(cx, "tüm dallar", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() == 3));

    // Operations.
    let op = |cx: &mut VisualTestContext, hash: &str, op: crate::graph::CommitOp| {
        let hash = hash.to_string();
        wb.update_in(cx, |wb, window, cx| wb.scm_panel().update(cx, |s, cx| s.commit_op(hash, op, window, cx)));
        wait_ticking(cx, "işlem bitti", |cx| !scm.read_with(cx, |s, _| s.is_busy()));
    };
    use crate::graph::{CommitOp, ResetMode};
    op(cx, &main_work.hash, CommitOp::Tag("t1".into()));
    assert!(git_in(&dir, &["tag"]).lines().any(|l| l == "t1"));

    let side = find(cx, "side work");
    op(cx, &side.hash, CommitOp::CherryPick);
    assert_eq!(git_in(&dir, &["log", "-1", "--format=%s"]).trim(), "side work", "cherry-pick adds a new commit on HEAD");
    assert!(dir.join("s.txt").exists());

    op(cx, "HEAD", CommitOp::Revert);
    assert_eq!(git_in(&dir, &["log", "-1", "--format=%s"]).trim(), "Revert \"side work\"");
    assert!(!dir.join("s.txt").exists());

    op(cx, &main_work.hash, CommitOp::Reset(ResetMode::Mixed));
    assert_eq!(git_in(&dir, &["rev-parse", "HEAD"]).trim(), main_work.hash);

    op(cx, &main_work.hash, CommitOp::Branch("from-main-work".into()));
    assert_eq!(git_in(&dir, &["branch", "--show-current"]).trim(), "from-main-work");

    op(cx, &main_work.hash, CommitOp::Checkout);
    assert_eq!(git_in(&dir, &["branch", "--show-current"]).trim(), "", "detached HEAD");

    // A conflicting cherry-pick is rolled back: no half-finished state is left behind.
    git_in(&dir, &["checkout", "-q", "main"]);
    std::fs::write(dir.join("s.txt"), "farklı\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "çakışma kaynağı"]);
    op(cx, &side.hash, CommitOp::CherryPick);
    assert!(scm.read_with(cx, |s, _| s.result_text()).is_some_and(|t| t.contains("undone")));
    assert!(!dir.join(".git/CHERRY_PICK_HEAD").exists());
}

/// Explorer: expand/collapse a folder with its subfolders, open folders persist; TIMELINE shows the active file's
/// commits.
#[gpui::test]
fn explorer_recursive_expand_persists_and_timeline_follows_the_file(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("explorer2") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    git_in(&dir, &["config", "user.name", "Test"]);
    git_in(&dir, &["config", "user.email", "t@example.com"]);
    std::fs::create_dir_all(dir.join("a/b/c")).unwrap();
    std::fs::create_dir_all(dir.join("a/d")).unwrap();
    std::fs::write(dir.join("a/b/c/deep.txt"), "1\n").unwrap();
    std::fs::write(dir.join("a/b/side.txt"), "s\n").unwrap();
    std::fs::write(dir.join("a/d/other.txt"), "o\n").unwrap();
    std::fs::write(dir.join("a/top.txt"), "t\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "add tree"]);
    std::fs::write(dir.join("a/b/c/deep.txt"), "1\n2\n").unwrap();
    git_in(&dir, &["commit", "-q", "-am", "edit deep"]);
    let other = project("explorer2-other");

    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "kök", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx).contains(&"a".to_string())));
    let a = crate::ui::local_path(&dir.join("a"));
    wb.update(cx, |wb, cx| wb.tree_expand_for_test(&a, true, cx));
    wait_ticking(cx, "hepsi açıldı", |cx| {
        wb.read_with(cx, |wb, cx| {
            let n = wb.tree_names(cx);
            n.contains(&"deep.txt".to_string()) && n.contains(&"other.txt".to_string()) && n.contains(&"top.txt".to_string())
        })
    });
    let saved = wb.read_with(cx, |wb, _| wb.saved_tree_for_test()).unwrap_or_default();
    assert!(saved.lines().any(|l| l == "a/d") && saved.lines().any(|l| l == "a/b/c"), "open folders saved: {saved:?}");

    // Switching to another folder and back restores the open folders.
    open(&wb, &other, cx);
    assert!(!wb.read_with(cx, |wb, cx| wb.tree_names(cx)).contains(&"deep.txt".to_string()));
    open(&wb, &dir, cx);
    wait_ticking(cx, "geri yüklendi", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx).contains(&"deep.txt".to_string())));

    // Timeline: the active file's commits (newest to oldest); the diff is clickable.
    open(&wb, &dir.join("a/b/c/deep.txt"), cx);
    wait_ticking(cx, "timeline", |cx| wb.read_with(cx, |wb, _| wb.timeline_subjects() == ["edit deep", "add tree"]));
    wb.update_in(cx, |wb, window, cx| wb.open_timeline_entry_for_test(0, window, cx));
    wait_ticking(cx, "timeline diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    // Switching to README shows its history (a single commit: "ilk").
    open(&wb, &dir.join("README.md"), cx);
    wait_ticking(cx, "timeline README", |cx| wb.read_with(cx, |wb, _| wb.timeline_subjects() == ["ilk"]));

    // Close the folder: its subfolders close too; the section state is written to the persisted text.
    wb.update(cx, |wb, cx| wb.tree_expand_for_test(&a, false, cx));
    wait_ticking(cx, "kapandı", |cx| wb.read_with(cx, |wb, cx| !wb.tree_names(cx).contains(&"deep.txt".to_string())));
    wb.update(cx, |wb, _| wb.section_for_test("timeline", false, Some(222.)));
    let text = wb.read_with(cx, |wb, _| wb.sections_text_for_test());
    assert!(text.contains("timeline:0:222"), "{text}");
}

#[gpui::test]
fn source_control_stage_diff_commit(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("scm") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    std::fs::write(dir.join("README.md"), "# proje\nikinci satır\n").unwrap();
    std::fs::write(dir.join("yeni.txt"), "yeni\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let changes = |cx: &mut VisualTestContext| {
        wb.read_with(cx, |wb, _| wb.git_status().map(|g| g.changes.iter().map(|c| (c.path.clone(), c.staged, c.unstaged)).collect::<Vec<_>>()))
    };
    wait_for(cx, "durum", |cx| changes(cx).is_some_and(|c| c.len() == 2));

    // ⌃⇧G: the panel opens, focus is in the message field.
    cx.simulate_keystrokes("ctrl-shift-g");
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    use crate::git::Mark;

    // Diff: index ↔ working tree, one line added.
    wb.update_in(cx, |wb, window, cx| wb.open_diff("README.md".into(), false, window, cx));
    wait_for(cx, "diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    let view = wb.read_with(cx, |wb, _| wb.diff_view()).unwrap();
    assert_eq!(view.read_with(cx, |v, _| v.summary()), (1, 0, 1), "+1 −0, 1 blok");

    // Stage: README moves to the index.
    scm.update(cx, |s, cx| s.stage(vec!["README.md".into()], cx));
    wait_ticking(cx, "stage", |cx| changes(cx).is_some_and(|c| c.iter().any(|(p, st, _)| p == "README.md" && *st == Some(Mark::Modified))));

    // Commit (only what is staged); the untracked file remains.
    scm.update(cx, |s, cx| s.set_message("README güncellendi", cx));
    cx.simulate_keystrokes("ctrl-shift-g enter"); // back to the panel (focus in the message), ↵ commit
    wait_ticking(cx, "commit", |cx| changes(cx).is_some_and(|c| c.len() == 1) && !scm.read_with(cx, |s, _| s.is_busy()));
    assert!(
        scm.read_with(cx, |s, _| s.result_text()).is_some_and(|r| r.starts_with("Committed ") && r.ends_with("README güncellendi")),
        "{:?}",
        scm.read_with(cx, |s, _| s.result_text())
    );
    let log = crate::test_git::git().args(["log", "-1", "--format=%s"]).current_dir(&dir).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "README güncellendi");

    // An empty message is rejected; discard deletes the untracked file.
    scm.update(cx, |s, cx| s.commit(cx));
    assert_eq!(scm.read_with(cx, |s, _| s.result_text()).as_deref(), Some("Type a commit message first"));
    let untracked = wb.read_with(cx, |wb, _| wb.git_status().unwrap().changes.clone());
    scm.update(cx, |s, cx| s.discard(untracked, cx));
    wait_ticking(cx, "discard", |cx| changes(cx).is_some_and(|c| c.is_empty()));
    assert!(!dir.join("yeni.txt").exists());
}

fn has_clangd() -> bool {
    std::process::Command::new("clangd").arg("--version").output().is_ok_and(|o| o.status.success())
}

#[gpui::test]
fn language_server_diagnostics_follow_edits(cx: &mut TestAppContext) {
    if !has_clangd() {
        crate::test_sshd::skip_missing("clangd");
        return;
    }
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-lsp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(dir).unwrap();
    std::fs::write(dir.join("main.c"), "int main(void) {\n    return yok;\n}\n").unwrap();
    let (wb, cx) = setup_with_settings(cx, crate::settings::Settings { lsp_enabled: true, ..Default::default() });
    open(&wb, &dir.join("main.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_for(cx, "tanı", |cx| ed.read_with(cx, |e, _| !e.diagnostics().is_empty()));
    let d = ed.read_with(cx, |e, _| e.diagnostics()[0].clone());
    assert_eq!((d.severity, d.start.0), (1, 1), "{d:?}");
    assert_eq!(wb.read_with(cx, |wb, _| wb.problems()), vec![(dir.join("main.c").to_string_lossy().into_owned(), 1)]);

    // ⇧⌘M: Problems panel; instead of clicking the row, the same path: go to the location.
    cx.simulate_keystrokes("cmd-shift-m");

    // "yok" → "0": the incremental change goes to the server, the diagnostic goes away.
    cx.simulate_keystrokes("down end left alt-shift-left");
    cx.simulate_input("0");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "int main(void) {\n    return 0;\n}\n");
    wait_for(cx, "tanı kalktı", |cx| ed.read_with(cx, |e, _| e.diagnostics().is_empty()));
    assert!(wb.read_with(cx, |wb, _| wb.problems()).is_empty());

    // Undo: the diagnostic comes back (undo is also sent incrementally).
    cx.simulate_keystrokes("cmd-z");
    wait_for(cx, "tanı geri", |cx| ed.read_with(cx, |e, _| !e.diagnostics().is_empty()));
}

#[gpui::test]
fn language_server_completion_hover_definition(cx: &mut TestAppContext) {
    if !has_clangd() {
        crate::test_sshd::skip_missing("clangd");
        return;
    }
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-lsp2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(dir).unwrap();
    let src = "struct nokta { int xeksen; int yeksen; };\n/* iki sayıyı toplar */\nint topla(int a, int b) { return a + b; }\nint main(void) {\n    struct nokta n = {1, 2};\n    return topla(n.xeksen, 0);\n}\n";
    std::fs::write(dir.join("main.c"), src).unwrap();
    let (wb, cx) = setup_with_settings(cx, crate::settings::Settings { lsp_enabled: true, ..Default::default() });
    open(&wb, &dir.join("main.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    wait_for(cx, "sunucu bağlandı", |cx| ed.read_with(cx, |e, _| e.lsp_client().is_some()));

    // Go to definition: go to the "topla(" call, F12 → the definition line (2).
    cx.simulate_keystrokes("cmd-down up up right right right right right right right right right right right right");
    cx.simulate_keystrokes("f12");
    wait_for(cx, "tanım", |cx| ed.read_with(cx, |e, _| e.cursor_position().0 == 3));

    // Hover: the signature of the symbol under the cursor.
    cx.simulate_keystrokes("cmd-k cmd-i");
    wait_for(cx, "hover", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500)); // delayed display
        ed.read_with(cx, |e, _| e.hover_text().is_some_and(|t| t.contains("topla")))
    });

    // Completion: "n." on a new line → fields; "y" filters; ↵ accepts.
    cx.simulate_keystrokes("cmd-down up up end enter");
    cx.simulate_input("n.");
    wait_for(cx, "öneriler", |cx| ed.read_with(cx, |e, _| e.completion_labels().is_some_and(|l| l.len() >= 2)));
    assert_eq!(ed.update(cx, |e, cx| e.popup_count(cx)), 1, "list is at a drawable position");
    cx.simulate_input("y");
    wait_for(cx, "süzüldü", |cx| ed.read_with(cx, |e, _| e.completion_labels().is_some_and(|l| l.first().is_some_and(|f| f.contains("yeksen")))));
    cx.simulate_keystrokes("enter");
    assert!(ed.read_with(cx, |e, _| e.completion_labels()).is_none(), "closes on accept");
    assert!(ed.read_with(cx, |e, _| e.text()).contains("    n.yeksen\n"), "{}", ed.read_with(cx, |e, _| e.text()));
    // One undo step: the suggestion and the typed prefix must revert together (the prefix was a separate typing step).
    cx.simulate_keystrokes("cmd-z");
    assert!(ed.read_with(cx, |e, _| e.text()).contains("    n.y\n"));
}

#[gpui::test]
fn split_editor_groups(cx: &mut TestAppContext) {
    let dir = project("split");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let dir = std::fs::canonicalize(dir).unwrap();
    let open_in = |cx: &mut VisualTestContext, f: &str| {
        let p = dir.join(f).to_string_lossy().into_owned();
        wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p, window, cx));
        cx.run_until_parked();
    };
    for f in ["README.md", "src/main.rs", "src/lib.rs"] {
        open_in(cx, f);
    }
    let groups = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.groups(cx));
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

    // ⌘\: the active file (lib.rs) also opens in a new group on the right, focus on the right.
    cx.simulate_keystrokes("cmd-\\");
    assert_eq!(groups(cx), (s(&["README.md", "main.rs", "lib.rs"]), Some(s(&["lib.rs"])), false));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).map(|p| p.ends_with("lib.rs")), Some(true));

    // What is typed on the right also appears in the same document in the left group (single document).
    let lib = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert_eq!(lib.read_with(cx, |e, _| e.view_count()), 2);
    cx.simulate_input("x");
    assert!(wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
    cx.simulate_keystrokes("cmd-1");
    assert!(groups(cx).2);
    let left = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert_eq!(left, lib, "same editor in the left group too");
    assert!(lib.read_with(cx, |e, _| e.text()).starts_with('x'));
    cx.simulate_keystrokes("cmd-z");

    // Move the tab (⌃⌘→): main.rs goes right and leaves the left group. (groups: left, right, focus on left?)
    open_in(cx, "src/main.rs");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Move Editor into Other Group", window, cx));
    cx.run_until_parked();
    assert_eq!(groups(cx), (s(&["README.md", "lib.rs"]), Some(s(&["lib.rs", "main.rs"])), false));

    // Opening a file that is already open in its own group switches to it (no new tab).
    open_in(cx, "src/lib.rs");
    assert_eq!(groups(cx), (s(&["README.md", "lib.rs"]), Some(s(&["lib.rs", "main.rs"])), false));

    // lib.rs closes on the right; the document lives on in the left.
    cx.simulate_keystrokes("cmd-w");
    assert_eq!(groups(cx), (s(&["README.md", "lib.rs"]), Some(s(&["main.rs"])), false));
    assert_eq!(lib.read_with(cx, |e, _| e.view_count()), 1);

    // When a group's last tab closes, the group closes.
    cx.simulate_keystrokes("cmd-w");
    assert_eq!(groups(cx), (s(&["README.md", "lib.rs"]), None, true));

    // One tab: ⌘\ opens the same file on the right too; moving it (⌃⌘←) closes the right group.
    cx.simulate_keystrokes("cmd-w"); // lib.rs
    cx.simulate_keystrokes("cmd-\\");
    assert_eq!(groups(cx), (s(&["README.md"]), Some(s(&["README.md"])), false));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Move Editor into Other Group", window, cx));
    cx.run_until_parked();
    assert_eq!(groups(cx), (s(&["README.md"]), None, true));
}

#[gpui::test]
fn same_file_in_two_groups_has_its_own_cursor(cx: &mut TestAppContext) {
    let file = tmp("twoviews", b"bir\niki\nuc\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let text = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.text());

    cx.simulate_keystrokes("down"); // left: line 2
    cx.simulate_keystrokes("cmd-\\"); // the right starts at the same place
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()), (2, 1));
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.painted_lines()), vec![4, 4], "both groups drawn");
    cx.simulate_keystrokes("cmd-down");
    cx.simulate_input("son");
    cx.simulate_keystrokes("cmd-1");
    // The left view's cursor is in place: the start of line 2.
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()), (2, 1));
    cx.simulate_input(">");
    cx.simulate_keystrokes("cmd-up");
    cx.simulate_input("bas ");
    assert_eq!(text(cx), "bas bir\n>iki\nuc\nson");
    // The right view's cursor followed the left's typing: still at the end.
    cx.simulate_keystrokes("cmd-2");
    cx.simulate_input("!");
    assert_eq!(text(cx), "bas bir\n>iki\nuc\nson!");

    // Each group has its own find bar.
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_input("iki");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-1");
    assert!(!ed.read_with(cx, |e, _| e.find_open()), "no find bar in the left group");
    cx.simulate_keystrokes("cmd-2");
    assert!(ed.read_with(cx, |e, _| e.find_open()), "the one on the right stayed open");

    // Closing in one group asks nothing and the document stays dirty in the other; saving cleans both.
    cx.simulate_keystrokes("cmd-2 cmd-w");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.groups(cx)).1, None);
    assert!(ed.read_with(cx, |e, _| e.is_dirty()));
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "bas bir\n>iki\nuc\nson!");
    assert!(!wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
}

#[gpui::test]
fn split_via_palette_keeps_tabs(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("split2")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let groups = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.groups(cx));
    cx.simulate_keystrokes("cmd-p");
    cx.simulate_input("main.rs");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-shift-p");
    cx.simulate_input("split editor");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-shift-e");
    cx.run_until_parked();
    cx.simulate_mouse_move(gpui::point(gpui::px(900.), gpui::px(150.)), None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(groups(cx), (vec!["main.rs".to_string()], Some(vec!["main.rs".to_string()]), false));
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some("explorer"), "mouse movement doesn't change the panel");
}

/// Real ssh (a temporary local sshd): connect, then a port opened
/// on the server is forwarded automatically, reachable locally, and closed when stopped.
#[cfg(unix)]
#[gpui::test]
fn ssh_ports_are_forwarded_automatically(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("ports") else { return };
    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    let tick = |cx: &mut VisualTestContext| cx.executor().advance_clock(std::time::Duration::from_secs(5));
    wait_for(cx, "bağlantı ve ilk tarama", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });

    // A dev server opened "on the server" after connecting.
    let app = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let remote = app.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in app.incoming().flatten() {
            use std::io::Write;
            let _ = s.write_all(b"merhaba-ui");
        }
    });
    wait_for(cx, "kendiliğinden yönlendirme", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, _| wb.forwards().iter().any(|f| f.remote == remote))
    });
    // The server is this machine, so our own local port shows up as a "new port"; it must not be forwarded again.
    for _ in 0..3 {
        tick(cx);
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // Other programs and parallel tests open ports on this machine too, and those are rightly forwarded; only the loop matters:
    // our port forwarded once, and its local end never forwarded back.
    let forwards = wb.read_with(cx, |wb, _| wb.forwards());
    let local = forwards.iter().find(|f| f.remote == remote).unwrap().local;
    assert_eq!(forwards.iter().filter(|f| f.remote == remote).count(), 1, "{forwards:?}");
    assert!(local == remote || !forwards.iter().any(|f| f.remote == local), "the local end was forwarded again: {forwards:?}");
    let mut got = String::new();
    use std::io::Read;
    std::net::TcpStream::connect(("127.0.0.1", local)).unwrap().read_to_string(&mut got).unwrap();
    assert_eq!(got, "merhaba-ui");

    // Stop: the local port closes and isn't reopened on the next scan.
    wb.update(cx, |wb, cx| wb.stop_forward(remote, cx));
    wait_for(cx, "port kapandı", |cx| {
        cx.run_until_parked();
        std::net::TcpStream::connect(("127.0.0.1", local)).is_err()
    });
    tick(cx);
    cx.run_until_parked();
    assert!(!wb.read_with(cx, |wb, _| wb.forwards()).iter().any(|f| f.remote == remote), "a stopped port doesn't come back on its own");
}

/// A port the user forwarded in a project, with its label, is forwarded again when the project opens after a restart.
#[cfg(unix)]
#[gpui::test]
fn a_forwarded_port_comes_back_with_its_project(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("ports-saved") else { return };
    let home = crate::qa_workbench_tests::project("ports-saved-home");
    let dir = crate::qa_workbench_tests::project("ports-saved-repo");
    let (wb, cx) = crate::qa_workbench_tests::setup_persistent(cx, &home);
    let tick = |cx: &mut VisualTestContext| cx.executor().advance_clock(std::time::Duration::from_secs(5));
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    wait_for(cx, "connected", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    open(&wb, &dir, cx);
    let app = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let remote = app.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in app.incoming().flatten() {
            use std::io::Write;
            let _ = s.write_all(b"saved");
        }
    });
    wb.update(cx, |wb, cx| wb.forward_port(remote, cx));
    wait_for(cx, "forwarded", |cx| {
        cx.run_until_parked();
        wb.read_with(cx, |wb, _| wb.forwards().iter().any(|f| f.remote == remote))
    });
    wb.update(cx, |wb, cx| wb.set_forward_label_for_test(remote, "api", cx));

    let wb = crate::qa_workbench_tests::restart_with(wb, cx, &home, crate::settings::Startup::Continue);
    wait_for(cx, "forwarded again after the restart", |cx| {
        tick(cx);
        wb.read_with(cx, |wb, _| wb.forwards().iter().any(|f| f.remote == remote))
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.forward_label_for_test(remote)).as_deref(), Some("api"));
    let local = wb.read_with(cx, |wb, _| wb.forwards()).into_iter().find(|f| f.remote == remote).unwrap().local;
    let mut got = String::new();
    use std::io::Read;
    std::net::TcpStream::connect(("127.0.0.1", local)).unwrap().read_to_string(&mut got).unwrap();
    assert_eq!(got, "saved");
}

/// Pasting several lines into a shell without bracketed paste asks first; a marker hidden in the text never reaches the
/// shell; the answer decides whether anything is sent.
#[cfg(unix)]
#[gpui::test]
fn a_multi_line_paste_asks_first_and_hidden_markers_are_removed(cx: &mut TestAppContext) {
    let dir = single_file_dir("paste");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal");
    let paste = |cx: &mut VisualTestContext, text: &str| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_string()));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-v" } else { "ctrl-shift-v" });
        cx.run_until_parked();
    };
    paste(cx, "echo one-\x1b[201~two\n");
    wait_for(cx, "the single line ran", |cx| term.read_with(cx, |t, _| t.screen_text().contains("one-two")));
    assert!(!cx.has_pending_prompt(), "one line does not ask");

    paste(cx, "echo first-line\necho second-line\n");
    assert!(cx.has_pending_prompt(), "several lines ask first");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    assert!(!term.read_with(cx, |t, _| t.screen_text().contains("first-line")), "nothing was sent");

    paste(cx, "echo first-line\necho second-line\n");
    cx.simulate_prompt_answer("Paste");
    // The output line may follow a prompt (dash echoes both typed lines before running them).
    let ran = |l: &str| l.trim_end().ends_with("second-line") && !l.contains("echo");
    wait_for(cx, "both lines ran", |cx| term.read_with(cx, |t, _| t.screen_text().lines().filter(|l| ran(l)).count() == 1));
}

/// Closing a terminal asks only while a program runs in it; a split opens where the shell is now; a profile's
/// arguments and environment reach the shell.
#[cfg(unix)]
#[gpui::test]
fn terminals_ask_before_killing_a_program_split_where_the_shell_is_and_follow_profiles(cx: &mut TestAppContext) {
    let dir = single_file_dir("term-profiles");
    let elsewhere = std::fs::canonicalize(std::env::temp_dir()).unwrap().to_string_lossy().into_owned();
    let mut settings = crate::settings::Settings::default();
    let profile = crate::terminal_profiles::Profile {
        name: "plain".into(),
        path: "/bin/sh".into(),
        args: Some(Vec::new()),
        env: vec![("PROFILE_ENV_PROBE".into(), "from-profile".into())],
        color: Some("info".into()),
    };
    settings.terminal_profiles.insert(crate::terminal_profiles::os_key(std::env::consts::OS).into(), vec![profile]);
    settings.terminal_default_profile = Some("plain".into());
    let (wb, cx) = crate::qa_integration_tests::setup_with(cx, settings);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal");
    let screen = |cx: &mut VisualTestContext, t: &gpui::Entity<crate::terminal::TerminalView>| t.read_with(cx, |t, _| t.screen_text());
    cx.simulate_input("echo \"$PROFILE_ENV_PROBE\"\n");
    wait_for(cx, "the profile's environment", |cx| screen(cx, &term).lines().any(|l| l == "from-profile"));
    assert_eq!(term.read_with(cx, |t, _| t.color.clone()).as_deref(), Some("info"));

    // The shell moves; the split follows it.
    cx.simulate_input(&format!("cd {elsewhere}\n"));
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(300));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Split", window, cx));
    wait_for(cx, "the split", |cx| wb.read_with(cx, |wb, _| wb.terminal_count()) == 2);
    let second = wb.read_with(cx, |wb, _| wb.active_terminal()).unwrap();
    assert_eq!(second.read_with(cx, |t, _| t.cwd_for_test()).as_deref(), Some(elsewhere.as_str()));

    // An idle shell closes without a question; one running `sleep` asks and stays open on Cancel.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Close", window, cx));
    wait_for(cx, "closed without asking", |cx| wb.read_with(cx, |wb, _| wb.terminal_count()) == 1);
    assert!(!cx.has_pending_prompt());
    wb.update_in(cx, |wb, window, cx| wb.focus_terminal(window, cx));
    cx.simulate_input("sleep 30\n");
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(300));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Close", window, cx));
    wait_for(cx, "the question", |cx| cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.terminal_count()), 1, "still open");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Kill All Terminals", window, cx));
    wait_for(cx, "the question", |cx| cx.has_pending_prompt());
    cx.simulate_prompt_answer("Close");
    wait_for(cx, "all closed", |cx| wb.read_with(cx, |wb, _| wb.terminal_count()) == 0);
}

/// Shell integration sequences mark commands: their count, the last output and how they ended.
#[gpui::test]
fn command_marks_follow_the_shell_integration_sequences(cx: &mut TestAppContext) {
    let dir = single_file_dir("marks");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/cat"));
    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal");
    let feed = |cx: &mut VisualTestContext, s: &str| term.update(cx, |t, _| t.feed_for_test(s.as_bytes()));
    feed(cx, "\x1b]7;file://box/srv/app\x07\x1b]133;A\x07$ ls\r\n\x1b]133;C\x07a.txt\r\nb.txt\r\n\x1b]133;D;0\x07");
    feed(cx, "\x1b]133;A\x07$ false\r\n\x1b]133;C\x07");
    assert_eq!(term.read_with(cx, |t, _| t.command_count_for_test()), 2);
    assert_eq!(term.read_with(cx, |t, _| t.last_output_for_test()).as_deref(), Some("a.txt\nb.txt"), "the finished command's output");
    feed(cx, "\x1b]133;D;1\x07\x1b]133;A\x07$ ");
    assert_eq!(term.read_with(cx, |t, _| t.last_output_for_test()).as_deref(), Some(""), "the last command printed nothing");
    assert_eq!(term.read_with(cx, |t, _| t.live_cwd()).as_deref(), Some("/srv/app"));
    // A prompt without a command (an empty line) is not counted.
    feed(cx, "\r\n\x1b]133;A\x07$ ");
    assert_eq!(term.read_with(cx, |t, _| t.command_count_for_test()), 2);
}

/// The terminal list's activity comes from the same sequences: a running command, a failed one, and a long command that
/// finished out of sight until the terminal is looked at.
#[gpui::test]
fn terminal_activity_follows_the_shell_integration_sequences(cx: &mut TestAppContext) {
    use crate::terminal::{Activity, Finished};
    let dir = single_file_dir("activity");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/cat"));
    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal");
    let feed = |cx: &mut VisualTestContext, s: &str| term.update(cx, |t, _| t.feed_for_test(s.as_bytes()));
    let state = |cx: &mut VisualTestContext| term.read_with(cx, |t, _| (t.activity(), t.unseen_finish()));
    assert_eq!(state(cx), (Activity::Idle, None), "no command yet");
    feed(cx, "\x1b]133;A\x07$ make\r\n\x1b]133;C\x07");
    assert_eq!(state(cx), (Activity::Running("$ make".into()), None));
    term.update(cx, |t, _| t.backdate_command_for_test(10));
    feed(cx, "\x1b]133;D;2\x07\x1b]133;A\x07$ ");
    assert_eq!(state(cx), (Activity::Failed(2), Some(Finished::Failed)), "a long command failed");
    assert!(term.update(cx, |t, _| t.mark_seen()));
    // A quick command says nothing when it ends, and success clears the failure.
    feed(cx, "ls\r\n\x1b]133;C\x07a\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ ");
    assert_eq!(state(cx), (Activity::Idle, None));
}

/// Several terminals: the split pair stays together whichever of the two is active and comes back after visiting another
/// terminal; next and previous go round the list; closing one before the active terminal keeps the active one.
#[gpui::test]
fn terminals_cycle_and_keep_their_split_pair(cx: &mut TestAppContext) {
    let dir = single_file_dir("term-list");
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/cat"));
    cx.simulate_keystrokes("ctrl-`");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: New", window, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Split", window, cx));
    wait_for(cx, "the split", |cx| wb.read_with(cx, |wb, _| wb.terminal_count()) == 3);
    let all = wb.read_with(cx, |wb, _| wb.terminals_for_test());
    let active = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.active_terminal()).unwrap();
    let partner = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.split_partner());
    assert_eq!((active(cx), partner(cx)), (all[2].clone(), Some(all[1].clone())));
    wb.update_in(cx, |wb, window, cx| wb.select_terminal_for_test(1, window, cx));
    assert_eq!(partner(cx), Some(all[2].clone()), "the other half of the pair is still beside it");
    wb.update_in(cx, |wb, window, cx| wb.select_terminal_for_test(0, window, cx));
    assert_eq!(partner(cx), None, "a terminal outside the pair stands alone");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Focus Previous Terminal", window, cx));
    assert_eq!((active(cx), partner(cx)), (all[2].clone(), Some(all[1].clone())), "round to the last one, and the pair is back");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Focus Next Terminal", window, cx));
    assert_eq!(active(cx), all[0]);
    // Close the first terminal while the last is active: the last stays active.
    wb.update_in(cx, |wb, window, cx| wb.select_terminal_for_test(2, window, cx));
    wb.update_in(cx, |wb, window, cx| wb.close_terminal_for_test(0, window, cx));
    assert_eq!(active(cx), all[2]);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Terminal: Kill Other Terminals", window, cx));
    wait_for(cx, "the others closed", |cx| wb.read_with(cx, |wb, _| wb.terminal_count()) == 1);
    assert_eq!(active(cx), all[2]);
}

/// The New Terminal button's menu: the profiles (the default ticked), then the machine's shells no profile runs.
#[gpui::test]
fn the_launch_menu_offers_profiles_and_shells(cx: &mut TestAppContext) {
    let mut settings = crate::settings::Settings::default();
    let profile = |name: &str, path: &str| crate::terminal_profiles::Profile { name: name.into(), path: path.into(), args: None, env: Vec::new(), color: None };
    settings
        .terminal_profiles
        .insert(crate::terminal_profiles::os_key(std::env::consts::OS).into(), vec![profile("Login", "/bin/zsh"), profile("Plain", "/bin/sh")]);
    settings.terminal_default_profile = Some("Plain".into());
    let (wb, cx) = setup_with_settings(cx, settings);
    let shells = ["/bin/zsh".to_string(), "/bin/bash".to_string()];
    let menu = wb.read_with(cx, |wb, cx| wb.launch_menu_for_test(&shells, cx));
    assert_eq!(menu, [("Login".to_string(), Some(false)), ("Plain".to_string(), Some(true)), ("bash".to_string(), None)]);
}

/// Scrolled back while a program with a scroll region (vim, less, tmux) keeps printing: the view stays inside the history
/// and the screen can be drawn (the terminal parser moved the view past the history's end, and drawing that panicked).
#[gpui::test]
fn output_in_a_scroll_region_while_scrolled_back_does_not_break_drawing(cx: &mut TestAppContext) {
    let dir = single_file_dir("scrolled-dl");
    let (wb, cx) = setup_with_settings(cx, crate::settings::Settings { terminal_scrollback: 50, ..Default::default() });
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/cat"));
    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal");
    term.update(cx, |t, _| t.feed_for_test(format!("{}\x1b[5;10r", "line\r\n".repeat(50)).as_bytes()));
    term.update(cx, |t, _| t.scroll_for_test(24));
    for _ in 0..4 {
        term.update(cx, |t, _| t.feed_for_test(b"\n\n\n\n\n\n\n\n\x1b[3M"));
        let _ = term.read_with(cx, |t, _| t.screen_text());
    }
}

/// A program can print anything: random sequences mixed with command marks, screen and history clears, resizes of the
/// scroll region and wide characters. Marks, jumping between commands and copying the last output must survive all of it.
#[gpui::test]
fn command_marks_survive_hostile_output(cx: &mut TestAppContext) {
    let dir = single_file_dir("marks-fuzz");
    let (wb, cx) = setup_with_settings(cx, crate::settings::Settings { terminal_scrollback: 50, ..Default::default() });
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/cat"));
    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal");
    let pieces: &[&str] = &[
        "\x1b]133;A\x07$ ",
        "\x1b]133;C\x07",
        "\x1b]133;D;0\x07",
        "\x1b]133;D;1\x07",
        "\x1b]7;file://h/tmp\x07",
        "\x1b[2J",
        "\x1b[3J",
        "\x1b[H",
        "\x1bc",
        "\x1b[5;10r",
        "\x1b[r",
        "\x1b[?1049h",
        "\x1b[?1049l",
        "\x1b[10S",
        "\x1b[10T",
        "\x1b[3L",
        "\x1b[3M",
        "中文😀",
        "\r\n",
        "\n\n\n\n\n\n\n\n",
        "line of output\r\n",
    ];
    let mut x = 0x9E3779B97F4A7C15u64;
    let mut next = move |m: usize| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % m as u64) as usize
    };
    for _ in 0..6000 {
        let mut chunk = String::new();
        for _ in 0..(1 + next(8)) {
            chunk.push_str(pieces[next(pieces.len())]);
        }
        term.update(cx, |t, _| t.feed_for_test(chunk.as_bytes()));
        match next(6) {
            0 => drop(term.read_with(cx, |t, _| t.last_output_for_test())),
            1 => term.update(cx, |t, _| t.jump_for_test(next(2) == 0)),
            2 => drop(term.read_with(cx, |t, _| t.visible_marks_for_test())),
            3 => drop(term.read_with(cx, |t, _| t.scrollback_text())),
            4 => drop(term.read_with(cx, |t, _| (t.screen_text(), t.live_cwd()))),
            _ => {}
        }
    }
}

/// A real zsh with the integration scripts: its commands and exit codes are marked, and it reports its folder.
#[cfg(unix)]
#[gpui::test]
fn zsh_reports_commands_and_its_folder_with_shell_integration(cx: &mut TestAppContext) {
    if !std::path::Path::new("/bin/zsh").exists() {
        return;
    }
    let dir = single_file_dir("zsh-int");
    let user = dir.join("zdot");
    std::fs::create_dir_all(&user).unwrap();
    let mut settings = crate::settings::Settings::default();
    let profile = crate::terminal_profiles::Profile {
        name: "zsh".into(),
        path: "/bin/zsh".into(),
        args: None,
        env: vec![("CODEYARD_USER_ZDOTDIR".into(), user.to_string_lossy().into_owned())],
        color: None,
    };
    settings.terminal_profiles.insert(crate::terminal_profiles::os_key(std::env::consts::OS).into(), vec![profile]);
    settings.terminal_default_profile = Some("zsh".into());
    let (wb, cx) = crate::qa_integration_tests::setup_with(cx, settings);
    open(&wb, &dir, cx);
    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal");
    let elsewhere = std::fs::canonicalize(std::env::temp_dir()).unwrap().to_string_lossy().into_owned();
    cx.simulate_input(&format!("cd {elsewhere}; echo marked-output\n"));
    wait_for(cx, "the first command", |cx| {
        term.read_with(cx, |t, _| t.command_count_for_test()) >= 1 && t_out(&term, cx).is_some_and(|o| o.contains("marked-output"))
    });
    assert_eq!(term.read_with(cx, |t, _| t.live_cwd()).as_deref(), Some(elsewhere.as_str()), "OSC 7 from the prompt hook");
    cx.simulate_input("false\n");
    wait_for(cx, "the second command", |cx| term.read_with(cx, |t, _| t.command_count_for_test()) >= 2 && t_out(&term, cx).is_some_and(|o| o.is_empty()));
}

#[cfg(unix)]
fn t_out(term: &gpui::Entity<crate::terminal::TerminalView>, cx: &mut VisualTestContext) -> Option<String> {
    term.read_with(cx, |t, _| t.last_output_for_test())
}

/// Explorer: single-subfolder chains as one row, sort setting, hierarchy line, type-to-search.
#[gpui::test]
fn explorer_compacts_chains_sorts_and_types_ahead(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-explorer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/main/java")).unwrap();
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("src/main/java/App.java"), "class App {}\n").unwrap();
    std::fs::write(dir.join("src/main/java/Util.java"), "class Util {}\n").unwrap();
    std::fs::write(dir.join("docs/a.md"), "a\n").unwrap();
    std::fs::write(dir.join("docs/b.md"), "b\n").unwrap();
    std::fs::write(dir.join("zeta.txt"), "z\n").unwrap();
    std::fs::write(dir.join("Alpha.rs"), "a\n").unwrap();
    std::fs::write(dir.join("beta.txt"), "b\n").unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let names = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.tree_names(cx));
    wait_for(cx, "kök", |cx| names(cx).len() == 5);
    assert_eq!(names(cx), ["docs", "src", "Alpha.rs", "beta.txt", "zeta.txt"], "folders first, natural order");

    // When src opens the chain is a single row: "src/main/java", its files one level deeper.
    let src = crate::ui::local_path(&dir.join("src"));
    wb.update(cx, |wb, cx| wb.tree_reveal_dir_for_test(&src, cx));
    wait_for(cx, "zincir", |cx| names(cx).contains(&"src/main/java".to_string()) && names(cx).contains(&"Util.java".to_string()));
    assert_eq!(names(cx), ["docs", "src/main/java", "App.java", "Util.java", "Alpha.rs", "beta.txt", "zeta.txt"]);

    // When compaction is turned off every folder gets its own row.
    wb.update(cx, |wb, cx| wb.tree_options_for_test(false, "default", cx));
    wait_for(cx, "sıkıştırma kapalı", |cx| names(cx).contains(&"src".to_string()));
    assert!(names(cx).contains(&"main".to_string()) && names(cx).contains(&"java".to_string()), "{:?}", names(cx));
    wb.update(cx, |wb, cx| wb.tree_options_for_test(true, "default", cx));
    wait_for(cx, "sıkıştırma açık", |cx| names(cx).contains(&"src/main/java".to_string()));

    // Sorting: files first; mixed; by extension.
    wb.update(cx, |wb, cx| wb.tree_options_for_test(true, "filesFirst", cx));
    assert_eq!(names(cx), ["Alpha.rs", "beta.txt", "zeta.txt", "docs", "src/main/java", "App.java", "Util.java"]);
    wb.update(cx, |wb, cx| wb.tree_options_for_test(true, "mixed", cx));
    assert_eq!(names(cx), ["Alpha.rs", "beta.txt", "docs", "src/main/java", "App.java", "Util.java", "zeta.txt"]);
    wb.update(cx, |wb, cx| wb.tree_options_for_test(true, "type", cx));
    assert_eq!(names(cx), ["docs", "src/main/java", "App.java", "Util.java", "Alpha.rs", "beta.txt", "zeta.txt"], ".java < .rs < .txt");
    wb.update(cx, |wb, cx| wb.tree_options_for_test(true, "default", cx));

    // Type-to-search: tree focused, "z" → zeta.txt, "b" → beta.txt; the selected folder gets a stronger guide line.
    cx.simulate_keystrokes("cmd-shift-e");
    cx.simulate_input("z");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_selected_for_test(cx)), Some(crate::ui::local_path(&dir.join("zeta.txt"))));
    std::thread::sleep(std::time::Duration::from_millis(1100)); // new search
    cx.simulate_input("b");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_selected_for_test(cx)), Some(crate::ui::local_path(&dir.join("beta.txt"))));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    cx.simulate_input("al");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_selected_for_test(cx)), Some(crate::ui::local_path(&dir.join("Alpha.rs"))), "büyük/küçük harf fark etmez");

    // Hierarchy: with an open folder selected, its children's line (column 0, rows 2-3) is emphasized;
    // with a file inside it selected, the same folder's line is emphasized too.
    cx.simulate_keystrokes("up up up"); // Alpha.rs → Util.java → App.java → src/main/java
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_selected_for_test(cx)), Some(crate::ui::local_path(&dir.join("src/main/java"))));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_guide_for_test(cx)), Some((0, 2, 3)));
    cx.simulate_keystrokes("down");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_guide_for_test(cx)), Some((0, 2, 3)));
    cx.simulate_keystrokes("up up"); // docs (closed folder, at the root): no line
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_guide_for_test(cx)), None);
}

/// Real ssh: the folder picker opens on connect; type a path, descend into a subfolder, open.
/// Esc stays in the home directory; ⌘O opens the same picker on the remote host; a bad path errors.
#[cfg(unix)]
#[gpui::test]
fn ssh_connect_asks_which_folder_to_open(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("folder") else { return };
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("codeyard-e2e-pick-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("alpha/inner")).unwrap();
    std::fs::create_dir_all(base.join("beta")).unwrap();
    std::fs::write(base.join("alpha/readme.txt"), "x").unwrap();
    let base_s = base.to_string_lossy().into_owned();

    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    wait_ticking(cx, "bağlantı ve klasör seçici", |cx| wb.read_with(cx, |wb, _| wb.remote_folder_open()));
    let home = wb.read_with(cx, |wb, cx| wb.root_for_test(cx)).expect("home directory open");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_query(cx)), Some(format!("{}/", home.trim_end_matches('/'))));

    // Type the path: the list shows only matching folders (no files).
    cx.update(|_, cx| wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(format!("{base_s}/"), cx)));
    wait_ticking(cx, "alt klasörler", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).is_some_and(|l| l.contains(&"alpha/".to_string()))));
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert_eq!(labels[0], format!("Open “{base_s}/”"));
    assert_eq!(&labels[1..], ["alpha/", "beta/"]);

    // The prefix filters.
    cx.simulate_input("be");
    wait_ticking(cx, "süzülmüş liste", |cx| {
        wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).is_some_and(|l| l.get(1).is_some_and(|x| x == "beta/") && l.len() == 2))
    });

    // Select alpha/: it descends into it (the path becomes "…/alpha/"), inner folders are listed.
    cx.update(|_, cx| wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(format!("{base_s}/"), cx)));
    wait_ticking(cx, "liste", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).is_some_and(|l| l.len() == 3)));
    cx.simulate_keystrokes("down enter");
    wait_ticking(cx, "enter alpha", |cx| wb.read_with(cx, |wb, cx| wb.overlay_query(cx)) == Some(format!("{base_s}/alpha/")));
    wait_ticking(cx, "alpha'nın klasörleri", |cx| {
        wb.read_with(cx, |wb, cx| wb.overlay_labels(cx).is_some_and(|l| l.get(1).is_some_and(|x| x == "inner/") && l.len() == 2))
    });

    // The first row (Enter) opens the typed path.
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "folder opened", |cx| wb.read_with(cx, |wb, cx| wb.root_for_test(cx)) == Some(format!("{base_s}/alpha")));
    assert!(!wb.read_with(cx, |wb, _| wb.remote_folder_open()));
    wait_ticking(cx, "ağaç", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).contains(&"readme.txt".to_string()));

    // ⌘O opens the picker on the remote host at the open folder; a nonexistent path errors.
    cx.simulate_keystrokes("cmd-o");
    wait_ticking(cx, "⌘O seçici", |cx| wb.read_with(cx, |wb, _| wb.remote_folder_open()));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_query(cx)), Some(format!("{base_s}/alpha/")));
    cx.update(|_, cx| wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(format!("{base_s}/yok"), cx)));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "hata", |cx| wb.read_with(cx, |wb, _| wb.notice_text()).is_some_and(|t| t.contains("Can't open")));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.root_for_test(cx)), Some(format!("{base_s}/alpha")), "root doesn't change");

    // Esc: the picker closes, the open folder remains.
    cx.simulate_keystrokes("cmd-o escape");
    cx.run_until_parked();
    assert!(!wb.read_with(cx, |wb, _| wb.remote_folder_open()));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.root_for_test(cx)), Some(format!("{base_s}/alpha")));
    let _ = std::fs::remove_dir_all(&base);
}

#[gpui::test]
fn line_commands_pairs_and_zoom(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-lines-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.rs");
    std::fs::write(&file, "fn a() {}\nlet x = 1;\nlet y = 2;\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let text = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.text());

    // ⌘/ comment, again: remove.
    cx.simulate_keystrokes("down cmd-/");
    assert_eq!(text(cx), "fn a() {}\n// let x = 1;\nlet y = 2;\n");
    cx.simulate_keystrokes("cmd-/");
    assert_eq!(text(cx), "fn a() {}\nlet x = 1;\nlet y = 2;\n");
    // ⌥↓ move, ⇧⌥↓ duplicate, ⇧⌘K delete.
    cx.simulate_keystrokes("alt-down");
    assert_eq!(text(cx), "fn a() {}\nlet y = 2;\nlet x = 1;\n");
    cx.simulate_keystrokes("shift-alt-down");
    assert_eq!(text(cx), "fn a() {}\nlet y = 2;\nlet x = 1;\nlet x = 1;\n");
    cx.simulate_keystrokes("cmd-shift-k");
    assert_eq!(text(cx), "fn a() {}\nlet y = 2;\nlet x = 1;\n");
    // ⌘L twice: two lines selected; Tab indents, ⇧Tab outdents.
    cx.simulate_keystrokes("cmd-up down cmd-l cmd-l tab");
    assert_eq!(text(cx), "fn a() {}\n    let y = 2;\n    let x = 1;\n");
    cx.simulate_keystrokes("shift-tab");
    assert_eq!(text(cx), "fn a() {}\nlet y = 2;\nlet x = 1;\n");

    // Enter between {}: closing brace drops a line, cursor indented in the middle; typed bracket is paired.
    cx.simulate_keystrokes("cmd-up end left enter");
    assert_eq!(text(cx), "fn a() {\n    \n}\nlet y = 2;\nlet x = 1;\n");
    cx.simulate_input("f(");
    assert!(text(cx).contains("    f()\n"), "{}", text(cx));
    cx.simulate_input("1)");
    assert!(text(cx).contains("    f(1)\n"), "stepped over the closing bracket: {}", text(cx));

    // ⌘= / ⌘0: interface scale; the editor font preference itself is untouched (the window scales it once).
    let size = |cx: &mut VisualTestContext| cx.update(|_, cx| (cx.global::<crate::settings::Settings>().editor_font_size, cx.ui_scale()));
    let (base, _) = size(cx);
    cx.simulate_keystrokes("cmd-= cmd-=");
    assert_eq!(size(cx), (base, 1.2));
    cx.simulate_keystrokes("cmd-0");
    assert_eq!(size(cx), (base, 1.));
}

#[gpui::test]
fn language_server_rename_references_symbols_fix_format_signature(cx: &mut TestAppContext) {
    if !has_clangd() {
        crate::test_sshd::skip_missing("clangd");
        return;
    }
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-lsp3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(dir).unwrap();
    std::fs::write(dir.join("lib.h"), "int topla(int a, int b);\n").unwrap();
    std::fs::write(dir.join("main.c"), "#include \"lib.h\"\nint main(void){int x=topla(1,2);return x}\n").unwrap();
    std::fs::write(dir.join("lib.c"), "#include \"lib.h\"\nint topla(int a, int b) { return a + b; }\n").unwrap();
    // clangd's project index for cross-file references/rename.
    let d = dir.display();
    std::fs::write(
        dir.join("compile_commands.json"),
        format!(r#"[{{"directory":"{d}","file":"{d}/main.c","arguments":["cc","-I.","-c","main.c"]}},{{"directory":"{d}","file":"{d}/lib.c","arguments":["cc","-I.","-c","lib.c"]}}]"#),
    )
    .unwrap();
    let (wb, cx) = setup_with_settings(cx, crate::settings::Settings { lsp_enabled: true, ..Default::default() });
    open(&wb, &dir.join("main.c"), cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let text = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.text());
    wait_for(cx, "tanı", |cx| ed.read_with(cx, |e, _| !e.diagnostics().is_empty()));
    let labels = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx));

    // F8: go to next problem (missing ';' on line 2).
    cx.simulate_keystrokes("f8");
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()).0, 2);
    cx.simulate_keystrokes("escape cmd-up");

    // ⌘. : quick fix for the missing ';'.
    cx.simulate_keystrokes("down end");
    cx.simulate_keystrokes("cmd-.");
    wait_for(cx, "hızlı düzeltmeler", |cx| labels(cx).is_some_and(|l| l.iter().any(|t| t.contains(';'))));
    let ix = labels(cx).unwrap().iter().position(|t| t.contains(';')).unwrap();
    for _ in 0..ix {
        cx.simulate_keystrokes("down");
    }
    cx.simulate_keystrokes("enter");
    wait_for(cx, "düzeltme uygulandı", |cx| text(cx).contains("return x;"));

    // ⌘⇧O: symbols.
    cx.simulate_keystrokes("cmd-shift-o");
    wait_for(cx, "semboller", |cx| labels(cx).is_some_and(|l| l.iter().any(|t| t.trim() == "main")));
    cx.simulate_keystrokes("escape");

    // ⇧F12: references (main.c and lib.h).
    let col = text(cx).lines().nth(1).unwrap().find("topla").unwrap() + 1;
    ed.update_in(cx, |e, window, cx| e.reveal(2, col, 0, window, cx));
    cx.simulate_keystrokes("shift-f12");
    // The index is built in the background: ask again until found.
    wait_for(cx, "referanslar", |cx| {
        if labels(cx).is_some_and(|l| l.len() >= 3) {
            return true;
        }
        cx.simulate_keystrokes("escape shift-f12");
        std::thread::sleep(std::time::Duration::from_millis(300));
        false
    });
    assert!(labels(cx).unwrap().iter().any(|l| l.contains("return a + b")), "{:?}", labels(cx));
    cx.simulate_keystrokes("escape");

    // F2: rename; the open file changes in the buffer, the closed lib.h on disk.
    cx.simulate_keystrokes("f2");
    cx.simulate_input("toplam");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "yeniden adlandırma", |cx| text(cx).contains("toplam(1,2)"));
    wait_for(cx, "kapalı dosya diskte", |cx| {
        cx.run_until_parked();
        std::fs::read_to_string(dir.join("lib.h")).unwrap().contains("toplam(") && std::fs::read_to_string(dir.join("lib.c")).unwrap().contains("toplam(")
    });

    // ⇧⌥F: format (single undo step).
    let before = text(cx);
    cx.simulate_keystrokes("shift-alt-f");
    wait_for(cx, "biçimlendirme", |cx| text(cx) != before);
    assert!(text(cx).contains("int main(void) {"), "{}", text(cx));
    cx.simulate_keystrokes("escape cmd-z");
    assert_eq!(text(cx), before);

    // Signature help: after typing "toplam(" the active parameter is the first argument.
    let col = text(cx).lines().nth(1).unwrap().find('{').unwrap() + 1;
    ed.update_in(cx, |e, window, cx| e.reveal(2, col, 0, window, cx));
    cx.simulate_input("toplam(");
    let mut last = None;
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(15) {
        ed.update(cx, |e, cx| e.request_signature(cx));
        std::thread::sleep(std::time::Duration::from_millis(300));
        cx.run_until_parked();
        last = ed.read_with(cx, |e, _| e.signature_text());
        if last.as_deref().is_some_and(|s| s.contains("[int a]")) {
            break;
        }
    }
    let _ = last; // the first response may arrive before the server's cache is refreshed: it is requested again
    wait_for(cx, "imza", |cx| ed.read_with(cx, |e, _| e.signature_text()).is_some_and(|s| s.contains("[int a]")));
    cx.simulate_input("1,");
    wait_for(cx, "ikinci parametre", |cx| ed.read_with(cx, |e, _| e.signature_text()).is_some_and(|s| s.contains("[int b]")));
    cx.simulate_keystrokes("escape");

    // ⌘T: symbols in the project; goes to the definition in the closed lib.c.
    cx.simulate_keystrokes("cmd-t");
    assert_eq!(labels(cx).unwrap(), vec!["Type to search symbols in the workspace"]);
    cx.simulate_input("topl");
    wait_for(cx, "proje sembolleri", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(200));
        labels(cx).is_some_and(|l| l.iter().any(|t| t == "toplam"))
    });
    let details = wb.read_with(cx, |wb, cx| wb.overlay_details(cx)).unwrap();
    let ix = details.iter().position(|d| d.ends_with("lib.c:2")).unwrap_or_else(|| panic!("{details:?}"));
    for _ in 0..ix {
        cx.simulate_keystrokes("down");
    }
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let path = wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).path.clone())).unwrap();
    assert!(path.ends_with("lib.c"), "{path}");

    // If there is no match, it says so.
    cx.simulate_keystrokes("cmd-t");
    cx.simulate_input("zzqqyok");
    wait_for(cx, "boş sonuç", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(200));
        labels(cx).is_some_and(|l| l == ["No symbols matching “zzqqyok”"])
    });
}

#[gpui::test]
fn replace_in_files_preserve_case(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("replace-case")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    std::fs::write(dir.join("README.md"), "oldName OldName OLDNAME old_name Old_Name\n").unwrap();
    let q = proto::SearchQuery {
        root: dir.to_string_lossy().into_owned(),
        pattern: "old".into(),
        regex: false,
        case_sensitive: false,
        whole_word: false,
        include: vec![],
        exclude: vec![],
        max_results: 2000,
    };
    let q2 = proto::SearchQuery { pattern: "oldname".into(), ..q.clone() };
    let files = vec![dir.join("README.md").to_string_lossy().into_owned()];
    wb.update(cx, |wb, cx| wb.replace_in_files_with(files.clone(), q2, "newname".into(), true, cx));
    wait_for(cx, "disk", |cx| {
        cx.run_until_parked();
        std::fs::read_to_string(dir.join("README.md")).unwrap().contains("NEWNAME")
    });
    assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "newname Newname NEWNAME old_name Old_Name\n");
    // Underscored name: the parts follow their own format.
    let q3 = proto::SearchQuery { pattern: "old_name".into(), ..q };
    wb.update(cx, |wb, cx| wb.replace_in_files_with(files, q3, "new_name".into(), true, cx));
    wait_for(cx, "disk2", |cx| {
        cx.run_until_parked();
        std::fs::read_to_string(dir.join("README.md")).unwrap().contains("New_Name")
    });
    assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "newname Newname NEWNAME new_name New_Name\n");
}

#[gpui::test]
fn replace_in_files_open_and_closed(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("replace")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let lib = dir.join("src/lib.rs");
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(lib.to_string_lossy().into_owned(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    std::fs::write(dir.join("README.md"), "# proje\nsatır 7 burada\n").unwrap();

    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    cx.simulate_keystrokes("cmd-shift-f");
    cx.simulate_input(r"satır (\d+)");
    search.update(cx, |s, cx| {
        s.set_regex(true, cx);
        s.set_replacement("line $1", cx);
    });
    wait_for(cx, "arama", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        search.read_with(cx, |s, _| !s.is_searching() && s.result_summary().len() == 2)
    });
    // The UI path reviews in a Search Editor (search_editor_tests); this drives the shared apply path directly.
    let files = vec![lib.to_string_lossy().into_owned(), dir.join("README.md").to_string_lossy().into_owned()];
    let q = proto::SearchQuery {
        root: dir.to_string_lossy().into_owned(),
        pattern: r"satır (\d+)".into(),
        regex: true,
        case_sensitive: false,
        whole_word: false,
        include: vec![],
        exclude: vec![],
        max_results: 2000,
    };
    wb.update(cx, |wb, cx| wb.replace_in_files(files, q, "line $1".into(), cx));
    wait_for(cx, "diskteki dosya", |cx| {
        cx.run_until_parked();
        std::fs::read_to_string(dir.join("README.md")).unwrap().contains("line 7 burada")
    });
    let t = ed.read_with(cx, |e, _| e.text());
    assert!(t.starts_with("// line 1\n// line 2\n"), "{t}");
    assert!(ed.read_with(cx, |e, _| e.is_dirty()), "open file not saved, dirty");
    cx.simulate_keystrokes("escape");
    ed.update_in(cx, |e, window, cx| window.focus(&e.focus_handle(cx)));
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), (1..=10).map(|i| format!("// satır {i}\n")).collect::<String>(), "single undo step");
}

/// A closed file with mixed line endings keeps the endings of the lines the replacement did not touch.
#[gpui::test]
fn replace_in_closed_files_keeps_mixed_line_endings(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("replace-eol")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let mixed = dir.join("mixed.txt");
    std::fs::write(&mixed, "x = 1\nfoo\r\nbar\r\n").unwrap();
    let q = proto::SearchQuery {
        root: dir.to_string_lossy().into_owned(),
        pattern: "x".into(),
        regex: false,
        case_sensitive: true,
        whole_word: true,
        include: vec![],
        exclude: vec![],
        max_results: 2000,
    };
    wb.update(cx, |wb, cx| wb.replace_in_files(vec![mixed.to_string_lossy().into_owned()], q, "y".into(), cx));
    wait_for(cx, "diskteki dosya", |cx| {
        cx.run_until_parked();
        std::fs::read(&mixed).unwrap().starts_with(b"y = 1")
    });
    assert_eq!(std::fs::read(&mixed).unwrap(), b"y = 1\nfoo\r\nbar\r\n");
}

#[gpui::test]
fn import_vscode_settings_and_shortcuts(cx: &mut TestAppContext) {
    let cfg = config_dir("vsimport");
    let vs = cfg.join("vscode-user");
    std::fs::create_dir_all(&vs).unwrap();
    std::fs::write(
        vs.join("settings.json"),
        r#"{ "editor.fontSize": 16, "files.autoSave": "afterDelay", "workbench.colorTheme": "Paper", "eklenti.ayarı": 1 }"#,
    )
    .unwrap();
    std::fs::write(vs.join("keybindings.json"), r#"[ { "key": "cmd+k cmd+c", "command": "editor.action.commentLine", "when": "editorTextFocus" }, { "key": "f5", "command": "workbench.action.debug.start" } ]"#).unwrap();
    std::fs::write(cfg.join("settings.jsonc"), "{\n  // benim yorumum\n  \"editor.fontSize\": 13,\n}\n").unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    let msg = wb.update_in(cx, |wb, _, _| wb.import_vscode_from(&vs)).unwrap();
    assert!(msg.contains("Imported 3 settings and 1 shortcuts") && msg.contains("1 settings and 1 shortcuts have no equivalent"), "{msg}");
    let text = std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap();
    assert!(text.contains("// benim yorumum") && text.contains("\"editor.fontSize\": 16") && text.contains("\"theme\": \"paper\""), "{text}");
    // Settings apply live; the imported shortcut works (⌘K ⌘C: comment).
    cx.executor().advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(cx.update(|_, cx| cx.global::<crate::settings::Settings>().editor_font_size), 16.);
    let file = tmp("vsimport-file", b"let x = 1;\n");
    let file = file.with_file_name("a.rs");
    std::fs::write(&file, "let x = 1;\n").unwrap();
    open(&wb, &file, cx);
    cx.simulate_keystrokes("cmd-k cmd-c");
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "// let x = 1;\n");
    // Importing again doesn't duplicate shortcuts.
    wb.update_in(cx, |wb, _, _| wb.import_vscode_from(&vs)).unwrap();
    let keys = std::fs::read_to_string(cfg.join("keybindings.jsonc")).unwrap();
    assert_eq!(keys.matches("editor.toggleComment").count(), 1, "{keys}");
}

#[gpui::test]
fn large_file_opens_read_only_in_chunks(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-large-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("dev.log");
    {
        use std::io::Write;
        let mut w = std::io::BufWriter::new(std::fs::File::create(&file).unwrap());
        for i in 0..800_000 {
            writeln!(w, "2026-10-22 12:00:00 INFO istek {i:07} işlendi ğüşİı ok").unwrap();
        }
    }
    assert!(std::fs::metadata(&file).unwrap().len() > proto::MAX_FILE, "should be above the limit");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    wait_for(cx, "büyük dosya görünümü", |cx| wb.read_with(cx, |wb, cx| wb.large_view().is_some_and(|v| v.read(cx).line_count() > 0)));
    let view = wb.read_with(cx, |wb, _| wb.large_view()).unwrap();
    assert_eq!(view.read_with(cx, |v, _| v.line_count()), 800_001);
    wait_for(cx, "ilk satırlar", |cx| view.read_with(cx, |v, _| v.loaded_line(0)).is_some());
    assert_eq!(view.read_with(cx, |v, _| v.loaded_line(0)).unwrap(), "2026-10-22 12:00:00 INFO istek 0000000 işlendi ğüşİı ok");

    // ⌃G: line 500000 (through the palette, the same path as the editor).
    cx.simulate_keystrokes("ctrl-g");
    cx.simulate_input("500000");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "uzak satır", |cx| view.read_with(cx, |v, _| v.loaded_line(499_999)).is_some());
    assert!(view.read_with(cx, |v, _| v.loaded_line(499_999)).unwrap().contains("istek 0499999"));
    assert!(view.read_with(cx, |v, _| v.chunk_count()) <= 64, "bounded chunk in memory");

    // ⌘F: search runs on the agent; the single match deep in the file is found and highlighted.
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_input("istek 0612345");
    wait_ticking(cx, "tek eşleşme", |cx| view.read_with(cx, |v, _| v.find_summary()) == Some((1, Some(0), false)));
    assert_eq!(view.read_with(cx, |v, _| v.highlighted_line()), Some(612_345));
    // 10 matches: ↵ next, ⇧↵ previous, wraps from start to end.
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("istek 000000");
    wait_ticking(cx, "on eşleşme", |cx| view.read_with(cx, |v, _| v.find_summary()) == Some((10, Some(0), false)));
    assert_eq!(view.read_with(cx, |v, _| v.highlighted_line()), Some(0));
    cx.simulate_keystrokes("enter");
    assert_eq!(view.read_with(cx, |v, _| v.highlighted_line()), Some(1));
    cx.simulate_keystrokes("shift-enter shift-enter");
    assert_eq!(view.read_with(cx, |v, _| v.highlighted_line()), Some(9), "wraps to the start");
    cx.simulate_keystrokes("escape");
    assert_eq!(view.read_with(cx, |v, _| v.find_summary()), None);
    assert_eq!(view.read_with(cx, |v, _| v.highlighted_line()), None);
    cx.simulate_keystrokes("cmd-w");
    assert!(wb.read_with(cx, |wb, _| wb.large_view()).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn long_lines_scroll_horizontally(cx: &mut TestAppContext) {
    let long: String = (0..400).map(|i| format!("k{i} ")).collect();
    let file = tmp("hscroll", format!("kısa\n{long}\nson\n").as_bytes());
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let hs = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.hscroll());
    cx.run_until_parked();
    assert_eq!(hs(cx), 0.);
    cx.simulate_keystrokes("down end");
    cx.run_until_parked();
    assert!(hs(cx) > 500., "line end should be visible: {}", hs(cx));
    // Click while scrolled: the character at the visible position.
    let cursor = ed.read_with(cx, |e, _| e.cursors()[0]);
    let at = ed.read_with(cx, |e, _| e.point_for(cursor - 10)).unwrap();
    cx.simulate_click(at, Modifiers::none());
    assert_eq!(ed.read_with(cx, |e, _| e.cursors()[0]), cursor - 10);
    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    assert_eq!(hs(cx), 0., "line start: scroll resets");
    // ⇧+wheel: horizontal.
    let pos = ed.read_with(cx, |e, _| e.point_for(5)).unwrap();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: pos,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-200.))),
        modifiers: Modifiers::shift(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    assert!(hs(cx) > 100., "{}", hs(cx));
}

/// ⌘T with rust-analyzer: goes to a function in a closed file.
#[gpui::test]
fn workspace_symbols_find_rust_functions(cx: &mut TestAppContext) {
    if !std::process::Command::new("rust-analyzer").arg("--version").output().is_ok_and(|o| o.status.success()) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-ra-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let dir = std::fs::canonicalize(dir).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"ra_deneme\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("src/main.rs"), "mod yardim;\nfn main() { yardim::hesapla_toplam(1); }\n").unwrap();
    std::fs::write(dir.join("src/yardim.rs"), "pub struct Kutu;\n\npub fn hesapla_toplam(x: i32) -> i32 {\n    x + 1\n}\n").unwrap();
    // `setup` turns language servers off for every other test; this one needs rust-analyzer running.
    let (wb, cx) = setup_with_settings(cx, crate::settings::Settings { lsp_enabled: true, ..Default::default() });
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(dir.join("src/main.rs").to_string_lossy().into_owned(), window, cx));
    cx.run_until_parked();

    cx.simulate_keystrokes("cmd-t");
    cx.simulate_input("hesapla");
    let labels = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap_or_default();
    // Returns empty while the server is indexing: refresh the query until found.
    let start = std::time::Instant::now();
    while !labels(cx).iter().any(|l| l == "hesapla_toplam") {
        assert!(start.elapsed() < std::time::Duration::from_secs(90), "timed out: {:?}", labels(cx));
        cx.simulate_keystrokes("backspace");
        cx.simulate_input("a");
        cx.executor().advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(500));
        cx.run_until_parked();
    }
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (path, pos) = wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| (e.read(cx).path.clone(), e.read(cx).cursor_position()))).unwrap();
    assert!(path.ends_with("src/yardim.rs"), "{path}");
    assert_eq!(pos.0, 3, "function's line");
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn word_wrap_rows_cursor_and_click(cx: &mut TestAppContext) {
    let long = "kelime ".repeat(60); // 420 characters
    let file = tmp("wrap", format!("{long}\nikinci\n").as_bytes());
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let rows = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.row_counts());
    assert_eq!(rows(cx), (3, 3), "identical when off");

    cx.simulate_keystrokes("alt-z");
    cx.run_until_parked();
    cx.run_until_parked(); // the width is learned on the first draw
    let (r, l) = rows(cx);
    assert!(r > l, "long line spans several screen rows: {r}");
    assert!(ed.read_with(cx, |e, _| e.rows_fit()), "no segment overflows");

    // ↓ moves to the next segment of the same line; one ↓ per segment to reach the second line.
    cx.simulate_keystrokes("down");
    let (line, col) = ed.read_with(cx, |e, _| e.cursor_position());
    assert_eq!(line, 1);
    assert!(col > 1, "start of the second segment: {col}");
    for _ in 0..r - l {
        cx.simulate_keystrokes("down");
    }
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()).0, 2);

    // Clicking a scrolled segment hits the correct character.
    let target = 300;
    let at = ed.read_with(cx, |e, _| e.point_for(target)).expect("drawn");
    cx.simulate_click(at, Modifiers::none());
    assert_eq!(ed.read_with(cx, |e, _| e.cursors()), vec![target]);

    cx.simulate_keystrokes("alt-z");
    cx.run_until_parked();
    assert_eq!(rows(cx), (3, 3));
}

#[gpui::test]
fn folding_hides_lines_and_follows_edits(cx: &mut TestAppContext) {
    let file = tmp("fold", b"fn a() {\n    let x = 1;\n    let y = 2;\n}\nfn b() {\n    z();\n}\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let rows = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.row_counts()).0;
    let pos = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.cursor_position());
    assert_eq!(rows(cx), 8);

    // Fold inside: cursor at the end of the heading, two lines hidden.
    cx.simulate_keystrokes("down alt-cmd-[");
    cx.run_until_parked();
    assert_eq!(rows(cx), 6);
    assert_eq!(pos(cx), (1, 9));
    // ↓ skips hidden lines, ↑ returns.
    cx.simulate_keystrokes("down");
    assert_eq!(pos(cx).0, 4);
    cx.simulate_keystrokes("up");
    assert_eq!(pos(cx).0, 1);

    cx.simulate_keystrokes("alt-cmd-]");
    cx.run_until_parked();
    assert_eq!(rows(cx), 8);

    // Fold all; ⌃G to a hidden line unfolds that fold.
    cx.simulate_keystrokes("cmd-k cmd-0");
    cx.run_until_parked();
    assert_eq!(rows(cx), 5);
    wb.update_in(cx, |wb, window, cx| {
        let ed = wb.active_editor_entity().unwrap();
        ed.update(cx, |e, cx| e.go_to_line(6, window, cx));
    });
    cx.run_until_parked();
    assert_eq!(rows(cx), 6, "b unfolded (1 row), a stayed folded (2 rows)");
    cx.simulate_keystrokes("cmd-k cmd-j");
    cx.run_until_parked();
    assert_eq!(rows(cx), 8);

    // When a line is added above, the fold moves along with its heading.
    ed.update(cx, |e, cx| e.toggle_fold_for_test(0, cx));
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.fold_headers()), vec![0]);
    cx.simulate_keystrokes("cmd-up");
    cx.simulate_input("// not\n");
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.fold_headers()), vec![1]);
    assert_eq!(rows(cx), 7);

    // Folds are per view: opening in the right group leaves it folded in the left group.
    cx.simulate_keystrokes("cmd-\\");
    ed.update(cx, |e, cx| e.toggle_fold_for_test(1, cx));
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.fold_headers()), Vec::<usize>::new());
    cx.simulate_keystrokes("cmd-1");
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.fold_headers()), vec![1]);
}

#[gpui::test]
fn selection_and_line_commands(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("codeyard-e2e-cmds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.rs");
    std::fs::write(&file, "fn main() {\n    let ad = topla(1, 2);\n    ad\n}\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let text = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.text());
    let sel = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.selected_text());

    // ⌃⇧⌘→: word → argument list → call …; ⌃⇧⌘← goes back.
    let at = text(cx).find("1, 2").unwrap();
    ed.update_in(cx, |e, window, cx| e.reveal(2, at - 12, 0, window, cx));
    // These keys aren't bound on Linux and Windows ("rebind"): the same job from the command palette.
    let expand = |cx: &mut VisualTestContext, label: &str| wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    expand(cx, "Editor: Expand Selection");
    cx.run_until_parked();
    assert_eq!(sel(cx).as_deref(), Some("1"));
    expand(cx, "Editor: Expand Selection");
    cx.run_until_parked();
    assert_eq!(sel(cx).as_deref(), Some("(1, 2)"));
    expand(cx, "Editor: Expand Selection");
    cx.run_until_parked();
    assert_eq!(sel(cx).as_deref(), Some("topla(1, 2)"));
    expand(cx, "Editor: Shrink Selection");
    cx.run_until_parked();
    assert_eq!(sel(cx).as_deref(), Some("(1, 2)"));

    // ⇧⌘L: all occurrences of "ad"; convert to uppercase.
    let ad = text(cx).find("ad ").unwrap();
    ed.update_in(cx, |e, window, cx| e.reveal(2, ad - 12, 0, window, cx));
    cx.simulate_keystrokes("cmd-shift-l");
    assert_eq!(ed.read_with(cx, |e, _| e.cursors().len()), 2);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Transform to Uppercase", window, cx));
    assert!(text(cx).contains("let AD = ") && text(cx).contains("    AD\n"), "{}", text(cx));

    // ⌃J: join lines.
    cx.simulate_keystrokes("escape cmd-up");
    cx.simulate_keystrokes("ctrl-j");
    assert!(text(cx).starts_with("fn main() { let AD"), "{}", text(cx));
}

#[gpui::test]
fn navigation_history_reopen_new_file_and_save_as(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("nav")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let active = |cx: &mut VisualTestContext| {
        wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| (crate::ui::file_name(&e.read(cx).path).to_string(), e.read(cx).cursor_position())))
    };
    let open_in = |cx: &mut VisualTestContext, f: &str| {
        let p = dir.join(f).to_string_lossy().into_owned();
        wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p, window, cx));
        cx.run_until_parked();
    };
    open_in(cx, "src/lib.rs");
    // ⌃G 7: a jump within the same file is also recorded in history.
    cx.simulate_keystrokes("ctrl-g");
    cx.simulate_input("7");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    open_in(cx, "src/main.rs");
    assert_eq!(active(cx).unwrap().0, "main.rs");

    // ⌃-: to lib.rs line 7, again ⌃-: to lib.rs line 1; ⌃⇧- forward.
    cx.simulate_keystrokes("ctrl--");
    cx.run_until_parked();
    assert_eq!(active(cx).unwrap(), ("lib.rs".into(), (7, 1)));
    cx.simulate_keystrokes("ctrl--");
    cx.run_until_parked();
    assert_eq!(active(cx).unwrap(), ("lib.rs".into(), (1, 1)));
    cx.simulate_keystrokes("ctrl-shift--");
    cx.run_until_parked();
    assert_eq!(active(cx).unwrap(), ("lib.rs".into(), (7, 1)));

    // ⌘W then ⇧⌘T: comes back in the same place.
    cx.simulate_keystrokes("cmd-w");
    cx.run_until_parked();
    assert_ne!(active(cx).map(|a| a.0), Some("lib.rs".into()));
    cx.simulate_keystrokes("cmd-shift-t");
    cx.run_until_parked();
    assert_eq!(active(cx).unwrap(), ("lib.rs".into(), (7, 1)));

    // ⌘N: in the active file's folder, with a sub-path.
    cx.simulate_keystrokes("cmd-n");
    cx.simulate_input("alt/yeni.rs");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap(), vec!["Create file “alt/yeni.rs”"]);
    std::fs::create_dir_all(dir.join("src/alt")).unwrap();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(dir.join("src/alt/yeni.rs").is_file());
    assert_eq!(active(cx).unwrap().0, "yeni.rs");

    // ⇧⌘S: content goes to the new file, the old one stays the same on disk, the tab is on the new file.
    open_in(cx, "src/main.rs");
    cx.simulate_input("// değişti\n");
    cx.simulate_keystrokes("cmd-shift-s");
    cx.run_until_parked();
    for _ in 0.."src/main.rs".len() {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_input("src/kopya.rs");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("src/kopya.rs")).unwrap(), "// değişti\nfn main() {}\n");
    assert_eq!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap(), "fn main() {}\n");
    assert_eq!(active(cx).unwrap().0, "kopya.rs");
    assert!(!wb.read_with(cx, |wb, cx| wb.any_dirty(cx)));
    // Doesn't overwrite an existing file.
    cx.simulate_keystrokes("cmd-shift-s");
    cx.run_until_parked();
    for _ in 0.."src/kopya.rs".len() {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_input("README.md");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "# proje\n");
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).unwrap().contains("already exists"));
}

#[gpui::test]
fn active_file_is_revealed_in_tree(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("reveal")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    assert!(!wb.read_with(cx, |wb, cx| wb.tree_names(cx)).contains(&"lib.rs".to_string()), "src starts collapsed");
    let lib = dir.join("src/lib.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(lib.clone(), window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, cx| wb.tree_names(cx)).contains(&"lib.rs".to_string()), "src expanded");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_selected(cx)), Some(lib));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Files: Collapse Folders in Explorer", window, cx));
    assert!(!wb.read_with(cx, |wb, cx| wb.tree_names(cx)).contains(&"lib.rs".to_string()));
}

/// Like `wait_for`, but also advances the test clock: the delayed git refresh
/// scheduled by the file watcher can replace the immediate one.
fn wait_ticking(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    wait_for(cx, what, |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        done(cx)
    });
}

/// Git command in a temp directory (identity and default branch fixed for tests).
fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
    let out = crate::test_git::git().args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[gpui::test]
fn git_branches_push_pull_against_a_local_remote(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("gitsync") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let bare = dir.with_extension("origin.git");
    let _ = std::fs::remove_dir_all(&bare);
    git_in(dir.parent().unwrap(), &["init", "-q", "--bare", bare.to_str().unwrap()]);
    git_in(&dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git_in(&dir, &["push", "-q", "-u", "origin", "main"]);

    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let status = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.git_status());
    let notice = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default();
    let run = |cx: &mut VisualTestContext, label: &str| wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    let refresh = |cx: &mut VisualTestContext| wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "git durumu", |cx| status(cx).is_some());

    // Local commit → 1 ahead → Push → on the remote.
    std::fs::write(dir.join("README.md"), "# proje\nyeni satır\n").unwrap();
    git_in(&dir, &["commit", "-q", "-am", "yerel"]);
    refresh(cx);
    wait_ticking(cx, "1 önde", |cx| status(cx).is_some_and(|s| s.ahead == 1));
    run(cx, "Git: Push");
    wait_ticking(cx, "push", |cx| notice(cx) == "Pushed");
    assert!(git_in(&bare, &["log", "--format=%s", "-1"]).contains("yerel"));

    // New branch (the name may contain /), then go back to main from the picker.
    run(cx, "Git: Create Branch…");
    cx.simulate_input("ozellik/x");
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "yeni dal", |cx| status(cx).is_some_and(|s| s.branch.as_deref() == Some("ozellik/x")));
    run(cx, "Git: Checkout to…");
    wait_ticking(cx, "dal listesi", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some_and(|l| l.iter().any(|t| t == "main")));
    cx.simulate_input("main");
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "main", |cx| status(cx).is_some_and(|s| s.branch.as_deref() == Some("main")));

    // Someone else commits on the remote → Fetch → 1 behind → Sync (pull) → file updated.
    let other = dir.with_extension("other");
    let _ = std::fs::remove_dir_all(&other);
    git_in(dir.parent().unwrap(), &["clone", "-q", bare.to_str().unwrap(), other.to_str().unwrap()]);
    std::fs::write(other.join("README.md"), "# proje\nyeni satır\nuzaktan\n").unwrap();
    git_in(&other, &["commit", "-q", "-am", "uzak"]);
    git_in(&other, &["push", "-q"]);
    run(cx, "Git: Fetch");
    wait_ticking(cx, "1 geride", |cx| status(cx).is_some_and(|s| s.behind == 1));
    run(cx, "Git: Sync");
    // Sync pulls at once: no question unless git.confirmSync is on.
    assert!(!cx.has_pending_prompt());
    // The file watcher schedules a delayed refresh: advance the test clock.
    wait_ticking(cx, "pull", |cx| status(cx).is_some_and(|s| s.behind == 0) && notice(cx) == "Pulled");
    assert!(std::fs::read_to_string(dir.join("README.md")).unwrap().contains("uzaktan"));
    let _ = std::fs::remove_dir_all(&other);
    let _ = std::fs::remove_dir_all(&bare);
}

#[gpui::test]
fn images_open_in_a_preview(cx: &mut TestAppContext) {
    // 1×1 PNG.
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84,
        120, 218, 99, 100, 96, 248, 95, 15, 0, 2, 135, 1, 128, 235, 71, 186, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let dir = std::fs::canonicalize(project("image")).unwrap();
    std::fs::write(dir.join("logo.png"), PNG).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let p = dir.join("logo.png").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p, window, cx));
    cx.run_until_parked();
    let view = wb.read_with(cx, |wb, _| wb.large_view()).expect("preview opened");
    assert!(view.read_with(cx, |v, _| v.is_image() && v.image_loaded()));
    assert_eq!(wb.read_with(cx, |wb, _| wb.open_state()).1, None, "no binary-file error");
}

/// A tiny PNG that announces 30,000 × 30,000 pixels (3.6 GB once decoded) is refused before it is decoded.
#[gpui::test]
fn an_image_too_large_to_decode_is_refused(cx: &mut TestAppContext) {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(30_000u32.to_be_bytes());
    png.extend(30_000u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    let dir = std::fs::canonicalize(project("huge-image")).unwrap();
    std::fs::write(dir.join("bomb.png"), &png).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let p = dir.join("bomb.png").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p, window, cx));
    cx.run_until_parked();
    let view = wb.read_with(cx, |wb, _| wb.large_view()).expect("preview opened");
    assert!(view.read_with(cx, |v, _| !v.image_loaded()), "nothing was decoded");
    let error = view.read_with(cx, |v, _| v.error_for_test());
    assert!(error.as_deref().is_some_and(|e| e.contains("30000 × 30000")), "{error:?}");
}

#[gpui::test]
fn change_line_endings_and_indentation(cx: &mut TestAppContext) {
    let file = tmp("eol", b"fn a() {\n    if x {\n        y();\n    }\n}\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let run = |cx: &mut VisualTestContext, label: &str| wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));

    // Choose CRLF: the file is dirty, CRLF on disk after saving.
    run(cx, "Editor: Change End of Line Sequence");
    cx.simulate_input("CRLF");
    cx.simulate_keystrokes("enter");
    assert!(ed.read_with(cx, |e, _| e.is_dirty()));
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read(&file).unwrap(), b"fn a() {\r\n    if x {\r\n        y();\r\n    }\r\n}\r\n");
    assert!(!ed.read_with(cx, |e, _| e.is_dirty()));

    // Convert indentation to tabs (4 spaces = 1 tab), single undo step.
    run(cx, "Editor: Change Indentation");
    cx.simulate_input("convert indentation to tabs");
    cx.simulate_keystrokes("enter");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "fn a() {\r\n\tif x {\r\n\t\ty();\r\n\t}\r\n}\r\n");
    assert_eq!(ed.read_with(cx, |e, _| e.indent_label()), "Tabs");
    cx.simulate_keystrokes("cmd-z");
    assert!(ed.read_with(cx, |e, _| e.text()).contains("\r\n        y();"));
}

#[gpui::test]
fn new_window_opens_an_empty_workbench(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings::default());
        // So tests don't touch the user's state.
        let dir = std::env::temp_dir().join(format!("codeyard-e2e-windows-{}", std::process::id()));
        // SAFETY: only this test reads it (other tests keep the state in memory).
        unsafe {
            std::env::set_var("CODEYARD_STATE_DIR", dir.join("state"));
            std::env::set_var("CODEYARD_CONFIG_DIR", dir.join("config"));
        }
    });
    let a = cx.update(crate::open_workbench_window).expect("ilk pencere");
    let b = cx.update(crate::open_workbench_window).expect("ikinci pencere");
    assert_ne!(a.window_id(), b.window_id());
    assert_eq!(cx.update(|cx| cx.windows().len()), 2);
    cx.run_until_parked();
    let open = b.update(cx, |wb, _, cx| wb.open_paths(cx)).unwrap();
    assert!(open.is_empty(), "new window opens empty");

    // Picking in B the folder open in A doesn't open a second copy: B stays empty, A comes to the front.
    let dir = std::fs::canonicalize(project("windows-same")).unwrap().to_string_lossy().into_owned();
    a.update(cx, |wb, window, cx| wb.open_path(&dir, window, cx)).unwrap();
    cx.run_until_parked();
    let recent = state::RecentWorkspace { host: None, path: dir.clone(), last_opened_at: 0 };
    b.update(cx, |wb, window, cx| wb.open_recent(recent, window, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(b.update(cx, |wb, _, cx| wb.root_for_test(cx)).unwrap(), None);
    assert_eq!(a.update(cx, |wb, _, cx| wb.root_for_test(cx)).unwrap().as_deref(), Some(dir.as_str()));
}

#[gpui::test]
fn projects_switcher_and_favorites(cx: &mut TestAppContext) {
    let cfg = config_dir("projects");
    let a = std::fs::canonicalize(project("proj-a")).unwrap();
    let b = std::fs::canonicalize(project("proj-b")).unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    let run = |cx: &mut VisualTestContext, label: &str| wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    let labels = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap_or_default();

    // Save A by name.
    open(&wb, &a, cx);
    run(cx, "Projects: Add Folder to Projects…");
    for _ in 0..30 {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_input("Ana Proje");
    cx.simulate_keystrokes("enter");
    let saved = crate::workbench::projects::parse_projects(&std::fs::read_to_string(cfg.join("projects.jsonc")).unwrap());
    assert_eq!(saved.len(), 1);
    assert_eq!((saved[0].name.as_str(), saved[0].path.as_str()), ("Ana Proje", a.to_str().unwrap()));

    // Switch to B; in ⌃R the saved project is first, choosing it returns to A.
    open(&wb, &b, cx);
    cx.simulate_keystrokes("ctrl-r");
    let l = labels(cx);
    assert_eq!(l.first().map(String::as_str), Some("★ Ana Proje"), "{l:?}");
    cx.simulate_input("ana");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.root_for_test(cx)), Some(a.to_string_lossy().into_owned()));

    // Favorites: the active file → in the list; renaming in the tree makes the path follow.
    let lib = a.join("src/lib.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(lib.clone(), window, cx));
    cx.run_until_parked();
    run(cx, "Favorites: Toggle Active File");
    assert_eq!(wb.read_with(cx, |wb, _| wb.favorite_list()), vec!["src/lib.rs".to_string()]);
    cx.simulate_keystrokes("cmd-w");
    run(cx, "Favorites: Show Favorites…");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), Some(lib.clone()));
    wb.update_in(cx, |wb, window, cx| wb.focus_tree(window, cx));
    cx.simulate_keystrokes("f2");
    for _ in 0.."lib.rs".len() {
        cx.simulate_keystrokes("backspace");
    }
    cx.simulate_input("kutu.rs");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.favorite_list()), vec!["src/kutu.rs".to_string()]);
}

#[gpui::test]
fn bookmarks_follow_edits_and_persist(cx: &mut TestAppContext) {
    let home = std::env::temp_dir().join(format!("codeyard-e2e-bm-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let dir = std::fs::canonicalize(project("bookmarks")).unwrap();
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    let lib = dir.join("src/lib.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(lib.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let marks = |cx: &mut VisualTestContext, ed: &Entity<crate::editor::EditorView>| ed.read_with(cx, |e, _| e.bookmark_lines());

    // Bookmarks on lines 3 and 7; adding two lines at the top shifts both.
    cx.simulate_keystrokes("down down alt-cmd-k down down down down alt-cmd-k");
    assert_eq!(marks(cx, &ed), vec![2, 6]);
    cx.simulate_keystrokes("cmd-up");
    cx.simulate_input("// a\n// b\n");
    assert_eq!(marks(cx, &ed), vec![4, 8]);
    // Next/previous: wraps to the start at the end.
    cx.simulate_keystrokes("alt-cmd-l");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()).0, 5);
    cx.simulate_keystrokes("alt-cmd-l alt-cmd-l");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()).0, 5, "sarma");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();

    // List: two bookmarks, with their line texts.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Bookmarks: List…", window, cx));
    cx.run_until_parked();
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert_eq!(labels, vec!["// satır 3", "// satır 7"]);
    cx.simulate_keystrokes("escape");

    // Back in place after restart.
    let wb2 = restart(wb, cx, &home);
    cx.run_until_parked();
    let ed2 = wb2.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert_eq!(marks(cx, &ed2), vec![4, 8]);
}

#[cfg(unix)]
#[gpui::test]
fn tasks_from_makefile_run_in_a_named_terminal(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("tasks")).unwrap();
    std::fs::write(dir.join("Makefile"), "selam:\n\t@echo görev-çalıştı-$$((40+2))\n.PHONY: selam\n").unwrap();
    let (wb, cx) = setup(cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Tasks: Run Task…", window, cx));
    wait_for(cx, "görev listesi", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some_and(|l| l.iter().any(|t| t == "make: selam")));
    cx.simulate_input("selam");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal opened");
    assert_eq!(term.read_with(cx, |t, _| t.title.to_string()), "make: selam");
    wait_for(cx, "görev çıktısı", |cx| term.read_with(cx, |t, _| t.screen_text().contains("görev-çalıştı-42")));
    // Run again: new terminal.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Tasks: Rerun Last Task", window, cx));
    cx.run_until_parked();
    assert_ne!(wb.read_with(cx, |wb, _| wb.active_terminal()), Some(term));
}

#[gpui::test]
fn scrollbar_click_and_drag(cx: &mut TestAppContext) {
    let text: String = (1..=600).map(|i| format!("satır {i}\n")).collect();
    let file = tmp("scrollbar", text.as_bytes());
    let (wb, cx) = setup(cx);
    // The scrollbar is measured with the minimap off (the minimap handles its own clicks).
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().minimap = false);
    open(&wb, &file, cx);
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    let first = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.first_visible_line()).unwrap_or(0);
    assert_eq!(first(cx), 0);
    // The test platform doesn't draw the scheduled frame by itself; for a real user
    // mouse movement before the click does that.
    cx.simulate_mouse_move(gpui::point(gpui::px(400.), gpui::px(300.)), None, Modifiers::none());
    cx.run_until_parked();
    // Click in the middle: the document jumps to its middle.
    let mid = ed.read_with(cx, |e, _| e.scrollbar_point(0.5)).expect("bar present");
    cx.simulate_mouse_down(mid, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(mid, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    let after_click = first(cx);
    assert!((200..400).contains(&after_click), "jumped to the middle: {after_click}");
    // Drag to the bottom: reaches the end. The cursor stays in place (only the view scrolls).
    let top = ed.read_with(cx, |e, _| e.scrollbar_point(0.5)).unwrap();
    let bottom = ed.read_with(cx, |e, _| e.scrollbar_point(1.0)).unwrap();
    cx.simulate_mouse_down(top, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(bottom, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(bottom, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert!(first(cx) >= 500, "dragged to the end: {}", first(cx));
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()), (1, 1));
}

#[gpui::test]
fn editor_context_menu_runs_editor_actions(cx: &mut TestAppContext) {
    let file = tmp("ctxmenu", b"alfa beta\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);

    cx.simulate_keystrokes("shift-alt-right"); // "alfa" selected
    wb.update_in(cx, |wb, window, cx| wb.open_editor_menu_for_test(window, cx));
    let labels = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).expect("menu didn't open");
    for l in ["Go to Definition", "Find All References", "Rename Symbol", "Cut", "Copy", "Paste", "Command Palette…"] {
        assert!(labels.iter().any(|x| x == l), "{l} missing: {labels:?}");
    }
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Cut", cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).is_none(), "menu should close on select");
    let text = wb.read_with(cx, |wb, cx| wb.active_editor_entity().unwrap().read(cx).text());
    assert_eq!(text, " beta\n");
    // Focus returned to the editor: typing goes to the editor.
    cx.simulate_keystrokes("end");
    wb.update_in(cx, |wb, window, cx| wb.open_editor_menu_for_test(window, cx));
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Paste", cx));
    cx.run_until_parked();
    let text = wb.read_with(cx, |wb, cx| wb.active_editor_entity().unwrap().read(cx).text());
    assert_eq!(text, " betaalfa\n");
}

/// End-to-end workflow over real ssh: pick a folder, git status and graph, compacted explorer,
/// TODO search (agent), context menu (duplicate, compare, terminal), edit-save.
#[cfg(unix)]
#[gpui::test]
fn ssh_workflow_git_graph_explorer_search_and_menu(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("workflow") else { return };
    let repo = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("codeyard-e2e-sshwf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(repo.join("deep/er/est")).unwrap();
    std::fs::write(repo.join("deep/er/est/note.txt"), "// TODO: remote todo\n").unwrap();
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    git_in(&repo, &["add", "-A"]);
    git_in(&repo, &["commit", "-q", "-m", "ilk"]);
    git_in(&repo, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.join("f.txt"), "f\n").unwrap();
    git_in(&repo, &["add", "-A"]);
    git_in(&repo, &["commit", "-q", "-m", "feature work"]);
    git_in(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("g.txt"), "g\n").unwrap();
    git_in(&repo, &["add", "-A"]);
    git_in(&repo, &["commit", "-q", "-m", "main work"]);
    git_in(&repo, &["merge", "-q", "--no-ff", "feature", "-m", "Merge feature"]);
    // Current state: one staged, one modified, one untracked.
    std::fs::write(repo.join("staged.txt"), "s\n").unwrap();
    git_in(&repo, &["add", "staged.txt"]);
    std::fs::write(repo.join("a.txt"), "a changed\n").unwrap();
    std::fs::write(repo.join("untracked.txt"), "u\n").unwrap();
    let repo_s = repo.to_string_lossy().into_owned();

    let (wb, cx) = setup(cx);
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    wait_ticking(cx, "klasör seçici", |cx| wb.read_with(cx, |wb, _| wb.remote_folder_open()));
    cx.update(|_, cx| wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(format!("{repo_s}/"), cx)));
    wait_ticking(cx, "liste", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some_and(|l| l.iter().any(|x| x == "deep/")));
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "folder opened", |cx| wb.read_with(cx, |wb, cx| wb.root_for_test(cx)).as_deref() == Some(repo_s.as_str()));

    // Git status is read remotely: staged, modified and untracked land in the right groups.
    wait_ticking(cx, "git durumu", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|g| g.changes.len() == 3)));
    let st = wb.read_with(cx, |wb, _| wb.git_status()).unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    use crate::git::Mark;
    let get = |p: &str| st.changes.iter().find(|c| c.path == p).map(|c| (c.staged, c.unstaged));
    assert_eq!(get("staged.txt"), Some((Some(Mark::Added), None)));
    assert_eq!(get("a.txt"), Some((None, Some(Mark::Modified))));
    assert_eq!(get("untracked.txt"), Some((None, Some(Mark::Untracked))));

    // Source control: the graph shows the remote repo; clicking a commit shows its files.
    cx.simulate_keystrokes("ctrl-shift-g");
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "uzak grafik", |cx| scm.read_with(cx, |s, _| s.graph_subjects().len() == 4));
    let subjects = scm.read_with(cx, |s, _| s.graph_subjects());
    assert_eq!(subjects[0], "Merge feature");
    assert_eq!(scm.read_with(cx, |s, _| s.graph_cols()), 2);
    let ix = subjects.iter().position(|x| x == "feature work").unwrap();
    let commit = scm.read_with(cx, |s, _| s.graph_commit(ix)).unwrap();
    scm.update(cx, |_, cx| cx.emit(crate::scm::ScmEvent::OpenCommit(commit)));
    wait_ticking(cx, "uzak commit dosyaları", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some_and(|l| l == ["f.txt"]));
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "uzak commit diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));

    // Explorer: single-subfolder chain as one row; type-to-search works remotely too.
    cx.simulate_keystrokes("cmd-shift-e");
    let names = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.tree_names(cx));
    let deep = repo.join("deep").to_string_lossy().into_owned();
    wb.update(cx, |wb, cx| wb.tree_reveal_dir_for_test(&deep, cx));
    wait_ticking(cx, "uzak zincir", |cx| names(cx).contains(&"deep/er/est".to_string()) && names(cx).contains(&"note.txt".to_string()));
    cx.simulate_input("g");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.tree_selected_for_test(cx)), Some(repo.join("g.txt").to_string_lossy().into_owned()));

    // TODO search runs on the agent.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Search: Find TODO Comments", window, cx));
    wait_ticking(cx, "uzak arama", |cx| wb.read_with(cx, |wb, cx| wb.search_summary_for_test(cx)).is_some_and(|s| s.0 >= 1));
    let rows = wb.read_with(cx, |wb, cx| wb.search_results_for_test(cx));
    assert_eq!(rows, [("deep/er/est/note.txt".to_string(), 1)]);

    // Context menu on a remote file: duplicate (cp on the server), compare, terminal.
    let menu_on = |cx: &mut VisualTestContext, path: &std::path::Path, is_dir: bool| {
        let ev =
            crate::tree::TreeEvent::ContextMenu { path: path.to_string_lossy().into_owned(), is_dir, position: gpui::point(gpui::px(100.), gpui::px(100.)) };
        wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(ev, window, cx));
    };
    menu_on(cx, &repo.join("a.txt"), false);
    let labels = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).unwrap();
    assert!(!labels.iter().any(|l| l == "Reveal in Finder"), "no Finder on a remote file: {labels:?}");
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Duplicate", cx));
    wait_ticking(cx, "uzak kopya", |_| repo.join("a copy.txt").exists());
    menu_on(cx, &repo.join("a.txt"), false);
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Select for Compare", cx));
    menu_on(cx, &repo.join("g.txt"), false);
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Compare with Selected", cx));
    wait_ticking(cx, "uzak karşılaştırma", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    menu_on(cx, &repo.join("deep"), true);
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Open in Integrated Terminal", cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.terminal_cwds_for_test(cx)), [Some(deep.clone())]);

    // Edit and save: the file on the server changes; the change shows up in git status.
    wb.update_in(cx, |wb, window, cx| wb.open_in_root(&repo.join("g.txt").to_string_lossy(), window, cx));
    wait_ticking(cx, "uzak dosya", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some_and(|p| p.ends_with("g.txt")));
    cx.simulate_keystrokes("cmd-down");
    cx.simulate_input("eklendi\n");
    cx.simulate_keystrokes("cmd-s");
    wait_ticking(cx, "uzak kayıt", |_| std::fs::read_to_string(repo.join("g.txt")).is_ok_and(|t| t.contains("eklendi")));
    wait_ticking(cx, "kayıttan sonra durum", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|g| g.changes.iter().any(|c| c.path == "g.txt"))));
    let _ = std::fs::remove_dir_all(&repo);
}

/// On restart the last remote folder opens without asking; a connection error is reported and can be
/// retried after fixing; another folder can be chosen while connected.
#[cfg(unix)]
#[gpui::test]
fn ssh_restore_failure_and_reopen(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("restore") else { return };
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("codeyard-e2e-sshrestore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("one")).unwrap();
    std::fs::create_dir_all(base.join("two")).unwrap();
    std::fs::write(base.join("one/a.txt"), "1\n").unwrap();
    std::fs::write(base.join("two/b.txt"), "2\n").unwrap();
    let one = base.join("one").to_string_lossy().into_owned();
    let two = base.join("two").to_string_lossy().into_owned();
    let home = home_dir("sshrestore");

    let (wb, cx) = setup_persistent(cx, &home);
    // A nonexistent server: the error is reported, the picker doesn't open, the app stays local.
    wb.update_in(cx, |wb, window, cx| wb.connect("codeyard-no-such-host".into(), window, cx));
    wait_ticking(cx, "bağlantı hatası", |cx| wb.read_with(cx, |wb, _| wb.conn_for_test()).starts_with("failed"));
    assert!(!wb.read_with(cx, |wb, _| wb.remote_folder_open()));

    // The right server: the picker opens; a folder is chosen.
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    wait_ticking(cx, "seçici", |cx| wb.read_with(cx, |wb, _| wb.remote_folder_open()));
    cx.update(|_, cx| wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(format!("{one}/"), cx)));
    wait_ticking(cx, "liste", |cx| wb.read_with(cx, |wb, cx| wb.overlay_query(cx)).is_some_and(|q| q == format!("{one}/")));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "one açıldı", |cx| wb.read_with(cx, |wb, cx| wb.root_for_test(cx)).as_deref() == Some(one.as_str()));
    wait_ticking(cx, "ağaç", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).contains(&"a.txt".to_string()));

    // ⌘O while connected: starting path is the open folder; switch to another folder.
    cx.simulate_keystrokes("cmd-o");
    wait_ticking(cx, "⌘O seçici", |cx| wb.read_with(cx, |wb, _| wb.remote_folder_open()));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_query(cx)), Some(format!("{one}/")));
    cx.update(|_, cx| wb.update(cx, |wb, cx| wb.set_overlay_query_for_test(two.clone(), cx)));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "two açıldı", |cx| wb.read_with(cx, |wb, cx| wb.root_for_test(cx)).as_deref() == Some(two.as_str()));
    wait_ticking(cx, "two ağacı", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)) == ["b.txt"]);

    // Restart: the last remote folder (two) opens without asking.
    let wb2 = restart(wb, cx, &home);
    wait_ticking(cx, "oturum uzakta geri geldi", |cx| wb2.read_with(cx, |wb, cx| wb.root_for_test(cx)).as_deref() == Some(two.as_str()));
    assert!(!wb2.read_with(cx, |wb, _| wb.remote_folder_open()), "no prompt when a saved folder exists");
    assert_eq!(wb2.read_with(cx, |wb, _| wb.conn_for_test()), "connected");
    wait_ticking(cx, "geri gelen ağaç", |cx| wb2.read_with(cx, |wb, cx| wb.tree_names(cx)) == ["b.txt"]);
    let _ = std::fs::remove_dir_all(&base);
}

/// ⌘E: open editors by most recent use; Enter jumps to the previous one. Zen mode saves/restores the layout.
/// TODO search: regex and case-sensitive, finds files containing TODO.
#[gpui::test]
fn editor_switcher_zen_mode_and_todo_search(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("switcher")).unwrap();
    std::fs::write(dir.join("src/main.rs"), "fn main() {\n    // TODO: finish\n}\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), "// todo lowercase is not a marker\n// FIXME: broken\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    // Open README → main.rs → lib.rs in that order: most recent use is lib, main, README.
    for f in ["README.md", "src/main.rs", "src/lib.rs"] {
        wb.update_in(cx, |wb, window, cx| wb.open_in_root(&dir.join(f).to_string_lossy(), window, cx));
        cx.run_until_parked();
    }
    cx.simulate_keystrokes("cmd-e");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)), Some(vec!["lib.rs".into(), "main.rs".into(), "README.md".into()]));
    cx.simulate_keystrokes("enter"); // the first row is the current one; Enter jumps to the previous one (main.rs)
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("src/main.rs"));
    cx.simulate_keystrokes("cmd-e enter"); // again: back to lib.rs
    assert!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap().ends_with("src/lib.rs"));
    cx.simulate_keystrokes("cmd-e escape");
    assert!(wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_none());

    // Zen: the sidebar closes and comes back when reopened.
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some("explorer"));
    cx.simulate_keystrokes("cmd-k z");
    assert!(wb.read_with(cx, |wb, _| wb.zen_for_test()));
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), None);
    cx.simulate_keystrokes("cmd-k z");
    assert!(!wb.read_with(cx, |wb, _| wb.zen_for_test()));
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some("explorer"));

    // TODO search.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Search: Find TODO Comments", window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.panel_for_test()), Some("search"));
    let (query, regex, case) = wb.read_with(cx, |wb, cx| wb.search_state_for_test(cx));
    assert!(regex && case, "regex and case-sensitive");
    assert!(query.contains("TODO") && query.contains("FIXME"));
    wait_for(cx, "arama", |cx| wb.read_with(cx, |wb, cx| wb.search_summary_for_test(cx)).is_some_and(|s| s.0 == 2));
    let (matches, files) = wb.read_with(cx, |wb, cx| wb.search_summary_for_test(cx)).unwrap();
    let rows = wb.read_with(cx, |wb, cx| wb.search_results_for_test(cx));
    assert_eq!(matches, 2, "TODO and FIXME; lowercase todo doesn't count: {rows:?}");
    let _ = files; // number of files scanned (including non-matching ones)
    assert_eq!(rows.len(), 2, "{rows:?}");
}

/// Text commands (palette and shortcut): title case, duplicate, trim whitespace, reverse, deduplicate, replace.
#[gpui::test]
fn editor_text_commands(cx: &mut TestAppContext) {
    let file = tmp("textcmds", b"hello wORLD it's  \nx\ny\nx\nabcd\n");
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let run = |cx: &mut VisualTestContext, label: &str| wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    let text = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.active_editor_entity().unwrap().read(cx).text());

    // Title Case: selected line.
    cx.simulate_keystrokes("cmd-up shift-end");
    run(cx, "Editor: Transform to Title Case");
    assert_eq!(text(cx).lines().next().unwrap(), "Hello World It's  ");
    // Trailing whitespace.
    run(cx, "Editor: Trim Trailing Whitespace");
    assert_eq!(text(cx).lines().next().unwrap(), "Hello World It's");

    // Duplicate selection: "Hello" selected → "HelloHello".
    cx.simulate_keystrokes("cmd-up shift-right shift-right shift-right shift-right shift-right");
    cx.simulate_keystrokes("cmd-shift-d");
    assert_eq!(text(cx).lines().next().unwrap(), "HelloHello World It's");
    // No selection: duplicate the line.
    cx.simulate_keystrokes("down home");
    run(cx, "Editor: Duplicate Selection");
    assert_eq!(text(cx).lines().collect::<Vec<_>>()[1..4], ["x", "x", "y"]);

    // Reverse lines (x x y x → x y x x) and deduplicate.
    cx.simulate_keystrokes("cmd-up down shift-down shift-down shift-down shift-down");
    run(cx, "Editor: Reverse Lines");
    assert_eq!(text(cx).lines().collect::<Vec<_>>()[1..5], ["x", "y", "x", "x"]);
    cx.simulate_keystrokes("cmd-a");
    run(cx, "Editor: Delete Duplicate Lines");
    assert_eq!(text(cx).lines().collect::<Vec<_>>(), ["HelloHello World It's", "x", "y", "abcd"]);

    // ⌃T (macOS): the two characters at the cursor swap.
    #[cfg(target_os = "macos")]
    {
        cx.simulate_keystrokes("cmd-down up end"); // end of the "abcd" line
        cx.simulate_keystrokes("ctrl-t");
        assert_eq!(text(cx).lines().last(), Some("abdc"), "swaps the two characters before the line end");
        cx.simulate_keystrokes("left ctrl-t"); // c|d → d and c swap again
        assert_eq!(text(cx).lines().last(), Some("abcd"));
    }
}

/// Editor decorations: rendering stays sound with them on: ruler, whitespace marks, relative numbers, block cursor,
/// bracket colors, color preview; sticky headings follow scope.
#[gpui::test]
fn editor_decorations_render_and_sticky_scroll_follows_scope(cx: &mut TestAppContext) {
    let cfg = config_dir("decor");
    std::fs::write(
        cfg.join("settings.jsonc"),
        r#"{ "editor.rulers": [20, 40], "editor.renderWhitespace": "all", "editor.lineNumbers": "relative",
             "editor.cursorStyle": "block", "editor.bracketPairColorization": true, "editor.colorDecorators": true,
             "editor.stickyScroll.enabled": true }"#,
    )
    .unwrap();
    let mut src = String::from("mod a {\n    fn f() {\n        let c = \"#1e90ff\"; // (x\n");
    for i in 0..150 {
        src.push_str(&format!("        call({i}, [1, {{2}}]);\t\n"));
    }
    src.push_str("    }\n}\n");
    let file = tmp("decor", src.as_bytes());
    let (wb, cx) = setup_with_config(cx, &cfg);
    open(&wb, &file, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.sticky_for_test()), Vec::<usize>::new(), "no heading at the top");
    // Brackets: depth is 0 at the top, 2 inside mod a { fn f() {, calls leave it unchanged.
    let depths = ed.read_with(cx, |e, _| e.bracket_depths_for_test());
    assert_eq!(depths[0], 0);
    assert_eq!(depths[2], 2);
    assert_eq!(depths[100], 2);
    assert_eq!(*depths.last().unwrap(), 0);

    // Deep in the function body, the enclosing `mod a` and `fn f` stick to the top.
    ed.update_in(cx, |e, window, cx| e.go_to_line(120, window, cx));
    for _ in 0..3 {
        cx.run_until_parked();
        ed.update(cx, |_, cx| cx.notify()); // scrolling doesn't request a new frame by itself
    }
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.sticky_for_test()), [0, 1]);
    // Typing keeps rendering sound (cache refreshes with the document).
    cx.simulate_input("(");
    cx.run_until_parked();
    let d2 = ed.read_with(cx, |e, _| e.bracket_depths_for_test());
    assert!(d2.len() >= depths.len());
}

/// Explorer context menu: duplicate, open side by side, compare, open a terminal in a folder.
#[gpui::test]
fn explorer_menu_duplicates_compares_and_opens_terminals(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("explorermenu")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_for(cx, "ağaç", |cx| wb.read_with(cx, |wb, cx| wb.tree_names(cx)).contains(&"README.md".to_string()));
    let menu_on = |cx: &mut VisualTestContext, path: &std::path::Path, is_dir: bool| {
        let ev = crate::tree::TreeEvent::ContextMenu { path: crate::ui::local_path(path), is_dir, position: gpui::point(gpui::px(100.), gpui::px(100.)) };
        wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(ev, window, cx));
    };
    let readme = dir.join("README.md");
    menu_on(cx, &readme, false);
    let labels = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).expect("menu didn't open");
    for l in ["Duplicate", "Open to the Side", "Select for Compare", "Open in Integrated Terminal", "Reveal in Finder"] {
        if l == "Reveal in Finder" && !cfg!(target_os = "macos") {
            continue;
        }
        assert!(labels.iter().any(|x| x == l), "{l} missing: {labels:?}");
    }
    assert!(!labels.iter().any(|x| x == "Compare with Selected"), "select a file first");

    // Duplicate: "README copy.md" is created; the copy clipboard is unaffected.
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Duplicate", cx));
    wait_for(cx, "kopya", |_| dir.join("README copy.md").exists());
    assert_eq!(std::fs::read_to_string(dir.join("README copy.md")).unwrap(), "# proje\n");

    // Compare: README is selected, "Compare with Selected" appears on main.rs.
    menu_on(cx, &readme, false);
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Select for Compare", cx));
    menu_on(cx, &dir.join("src/main.rs"), false);
    let labels = wb.read_with(cx, |wb, cx| wb.menu_labels(cx)).unwrap();
    assert!(labels.iter().any(|x| x == "Compare with Selected"), "{labels:?}");
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Compare with Selected", cx));
    wait_for(cx, "karşılaştırma", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));

    // Terminal in folder: on a file it uses the parent folder, on a folder itself.
    menu_on(cx, &dir.join("src"), true);
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Open in Integrated Terminal", cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.terminal_cwds_for_test(cx)), [Some(crate::ui::local_path(&dir.join("src")))]);
    menu_on(cx, &dir.join("src/lib.rs"), false);
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Open in Integrated Terminal", cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.terminal_cwds_for_test(cx)).last(), Some(&Some(crate::ui::local_path(&dir.join("src")))));

    // Open side by side: a second group is created.
    menu_on(cx, &readme, false);
    wb.update_in(cx, |wb, _, cx| wb.select_menu_for_test("Open to the Side", cx));
    cx.run_until_parked();
    let groups = wb.read_with(cx, |wb, cx| wb.groups(cx));
    assert!(groups.1.as_ref().is_some_and(|g| g.contains(&"README.md".to_string())), "{groups:?}");
}

#[gpui::test]
fn git_amend_undo_stash_and_discard_all(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("gitmore") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    let run = |cx: &mut VisualTestContext, label: &str| wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    let idle = |cx: &mut VisualTestContext| !scm.read_with(cx, |s, _| s.is_busy());
    let result = |cx: &mut VisualTestContext| scm.read_with(cx, |s, _| s.result_text()).unwrap_or_default();
    let changes = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.git_status().map(|g| g.changes.len()));
    wait_ticking(cx, "durum", |cx| changes(cx) == Some(0));

    // Commit, then amend with a changed message: the commit count stays the same.
    std::fs::write(dir.join("README.md"), "# proje\nbir\n").unwrap();
    git_in(&dir, &["commit", "-q", "-am", "ikinci"]);
    scm.update(cx, |s, cx| s.set_message("ikinci (düzeltildi)", cx));
    run(cx, "Git: Commit (Amend)");
    cx.run_until_parked();
    wait_ticking(cx, "amend", |cx| idle(cx) && result(cx).starts_with("Amended"));
    assert_eq!(git_in(&dir, &["log", "--format=%s"]).lines().collect::<Vec<_>>(), ["ikinci (düzeltildi)", "ilk"]);

    // Undo Last Commit: the changes stay staged, the message returns to the box.
    run(cx, "Git: Undo Last Commit");
    cx.run_until_parked();
    wait_ticking(cx, "undo", |cx| idle(cx) && result(cx).starts_with("Undid"));
    assert_eq!(git_in(&dir, &["log", "--format=%s"]).trim(), "ilk");
    assert_eq!(git_in(&dir, &["diff", "--cached", "--name-only"]).trim(), "README.md");
    assert_eq!(scm.read_with(cx, |s, cx| s.message_text(cx)), "ikinci (düzeltildi)");

    // Stash (with untracked) → clean; Pop Latest → back.
    std::fs::write(dir.join("yeni.txt"), "yeni\n").unwrap();
    run(cx, "Git: Stash (Include Untracked)");
    cx.run_until_parked();
    wait_ticking(cx, "stash", |cx| idle(cx) && changes(cx) == Some(0));
    assert!(!dir.join("yeni.txt").exists());
    run(cx, "Git: Stash");
    cx.run_until_parked();
    wait_ticking(cx, "boş stash", |cx| idle(cx) && !result(cx).is_empty());
    assert_eq!(result(cx), "There are no local changes to stash");
    run(cx, "Git: Pop Latest Stash");
    cx.run_until_parked();
    wait_ticking(cx, "pop", |cx| idle(cx) && changes(cx) == Some(2));
    assert!(dir.join("yeni.txt").exists());

    // Discard All: asks first; tracked files come back, untracked ones are deleted.
    git_in(&dir, &["reset", "-q"]);
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "unstaged", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|g| g.staged().next().is_none() && g.changes.len() == 2)));
    run(cx, "Git: Discard All Changes");
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "should ask before discarding");
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "# proje\nbir\n", "cancel keeps tracked edits");
    assert_eq!(std::fs::read_to_string(dir.join("yeni.txt")).unwrap(), "yeni\n", "cancel keeps untracked files");
    assert!(git_in(&dir, &["diff", "--cached", "--name-only"]).is_empty(), "cancel leaves the index alone");
    run(cx, "Git: Discard All Changes");
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard");
    wait_ticking(cx, "discard all", |cx| idle(cx) && changes(cx) == Some(0));
    assert!(!dir.join("yeni.txt").exists());
    assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "# proje\n");
}

#[gpui::test]
fn explorer_copy_cut_paste_and_drag_move(cx: &mut TestAppContext) {
    use crate::tree::TreeEvent;
    let dir = std::fs::canonicalize(project("clipboard")).unwrap();
    let d = |p: &str| crate::ui::local_path(&dir.join(p));
    std::fs::create_dir_all(dir.join("hedef")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let root = crate::ui::local_path(&dir);
    let ev = |cx: &mut VisualTestContext, e: TreeEvent| {
        wb.update_in(cx, |wb, window, cx| wb.tree_event_for_test(e, window, cx));
        cx.run_until_parked();
    };

    // Copy + paste into the same folder: "README copy.md", then "README copy 2.md"; the copy opens.
    ev(cx, TreeEvent::Copy(d("README.md")));
    ev(cx, TreeEvent::Paste { dir: root.clone() });
    ev(cx, TreeEvent::Paste { dir: root.clone() });
    assert_eq!(std::fs::read_to_string(dir.join("README copy.md")).unwrap(), "# proje\n");
    assert!(dir.join("README copy 2.md").exists());
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), Some(d("README copy 2.md")));
    // To another folder: the name is kept as is if there is no conflict.
    ev(cx, TreeEvent::Copy(d("README.md")));
    ev(cx, TreeEvent::Paste { dir: d("src") });
    assert!(dir.join("src/README.md").exists() && !dir.join("src/README copy.md").exists());
    // A folder can't be copied into itself.
    ev(cx, TreeEvent::Copy(d("src")));
    ev(cx, TreeEvent::Paste { dir: d("src") });
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default().contains("into itself"));

    // Cut + paste: it is moved; the open tab follows the new path.
    ev(cx, TreeEvent::Cut(d("README copy 2.md")));
    ev(cx, TreeEvent::Paste { dir: d("hedef") });
    assert!(!dir.join("README copy 2.md").exists());
    assert!(dir.join("hedef/README copy 2.md").exists());
    let tabs = wb.read_with(cx, |wb, cx| wb.open_paths(cx));
    assert!(tabs.contains(&d("hedef/README copy 2.md")) && !tabs.contains(&d("README copy 2.md")), "{tabs:?}");

    // Drag and drop: move to a folder; dropping into itself or where it already is does nothing.
    ev(cx, TreeEvent::Move { from: d("src/lib.rs"), to_dir: d("hedef") });
    assert!(cx.has_pending_prompt(), "should ask before drag-moving (explorer.confirmDragAndDrop)");
    assert!(dir.join("src/lib.rs").exists(), "not moved before confirmation");
    cx.simulate_prompt_answer("Move");
    cx.run_until_parked();
    assert!(dir.join("hedef/lib.rs").exists() && !dir.join("src/lib.rs").exists());
    ev(cx, TreeEvent::Move { from: d("src"), to_dir: d("src") });
    ev(cx, TreeEvent::Move { from: d("hedef"), to_dir: d("hedef/iç") });
    ev(cx, TreeEvent::Move { from: d("src/main.rs"), to_dir: d("src") });
    assert!(dir.join("src/main.rs").exists());
    // A file with the same name at the destination isn't overwritten.
    std::fs::write(dir.join("hedef/main.rs"), "başka").unwrap();
    ev(cx, TreeEvent::Move { from: d("src/main.rs"), to_dir: d("hedef") });
    cx.simulate_prompt_answer("Move");
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(dir.join("hedef/main.rs")).unwrap(), "başka");
    assert!(dir.join("src/main.rs").exists());
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default().contains("already exists"));
}

#[gpui::test]
fn settings_and_shortcuts_pickers_write_the_files(cx: &mut TestAppContext) {
    let cfg = config_dir("settings-ui");
    std::fs::write(cfg.join("settings.jsonc"), "{\n  // benim yorumum\n  \"editor.fontSize\": 13,\n}\n").unwrap();
    let (wb, cx) = setup_with_config(cx, &cfg);
    let run = |cx: &mut VisualTestContext, label: &str| wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
    let labels = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap_or_default();
    let settings = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.global::<crate::settings::Settings>().clone());

    // Boolean: choosing toggles it, applies immediately, the comment stays.
    run(cx, "Preferences: Settings");
    cx.run_until_parked();
    assert!(labels(cx).iter().any(|l| l == "editor.minimap.enabled"));
    cx.simulate_input("minimap");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!settings(cx).minimap, "minimap on by default; selecting turns it off");
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()).as_deref(), Some("editor.minimap.enabled: false"));
    // Right after, another setting: the earlier message doesn't hide the new one.
    run(cx, "Preferences: Settings");
    cx.run_until_parked();
    cx.simulate_input("editor.indentGuides");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.notice_text()).as_deref(), Some("editor.indentGuides: false"));
    let text = std::fs::read_to_string(cfg.join("settings.jsonc")).unwrap();
    assert!(text.contains("// benim yorumum") && text.contains("\"editor.minimap.enabled\": false"), "{text}");

    // Number: via input; out-of-range is rejected.
    run(cx, "Preferences: Settings");
    cx.run_until_parked();
    cx.simulate_input("editor.fontSize");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("backspace backspace");
    cx.simulate_input("16");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(settings(cx).editor_font_size, 16.);
    run(cx, "Preferences: Settings");
    cx.run_until_parked();
    cx.simulate_input("editor.fontSize");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("backspace backspace");
    cx.simulate_input("99");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(settings(cx).editor_font_size, 16.);
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default().contains("between"));

    // Option: sub-list.
    run(cx, "Preferences: Settings");
    cx.run_until_parked();
    cx.simulate_input("files.autoSave");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(labels(cx), ["off", "afterDelay", "onFocusChange"]);
    cx.simulate_input("focus");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(settings(cx).auto_save, crate::settings::AutoSave::OnFocusChange);

    // Shortcut: choosing the command adds a line to keybindings.jsonc and opens the file.
    run(cx, "Preferences: Keyboard Shortcuts");
    cx.run_until_parked();
    cx.simulate_input("git.stashPopLatest");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let keys = std::fs::read_to_string(cfg.join("keybindings.jsonc")).unwrap();
    assert!(keys.contains(r#"{ "key": "", "command": "git.stashPopLatest" },"#), "{keys}");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), Some(crate::ui::local_path(&cfg.join("keybindings.jsonc"))));
}

#[gpui::test]
fn tabs_reorder_and_move_between_groups_by_dragging(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("tabdrag")).unwrap();
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    for f in ["README.md", "src/main.rs", "src/lib.rs"] {
        let p = dir.join(f).to_string_lossy().into_owned();
        wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p, window, cx));
        cx.run_until_parked();
    }
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let groups = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, cx| wb.groups(cx));
    let drop = |cx: &mut VisualTestContext, from_left: bool, name: &str, to_left: bool, before: Option<&str>| {
        wb.update_in(cx, |wb, window, cx| wb.drop_tab_for_test(from_left, name, to_left, before, window, cx));
        cx.run_until_parked();
    };
    assert_eq!(groups(cx).0, s(&["README.md", "main.rs", "lib.rs"]));

    // Same group: lib.rs to the front; README to the end.
    drop(cx, true, "lib.rs", true, Some("README.md"));
    assert_eq!(groups(cx).0, s(&["lib.rs", "README.md", "main.rs"]));
    drop(cx, true, "README.md", true, None);
    assert_eq!(groups(cx).0, s(&["lib.rs", "main.rs", "README.md"]));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), Some(dir.join("README.md").to_string_lossy().into_owned()));

    // To the other group: while split, main.rs to the start of the right group.
    cx.simulate_keystrokes("cmd-\\"); // README also opens on the right
    drop(cx, true, "main.rs", false, Some("README.md"));
    assert_eq!(groups(cx), (s(&["lib.rs", "README.md"]), Some(s(&["main.rs", "README.md"])), false));
    // From right to left, to the end.
    drop(cx, false, "main.rs", true, None);
    assert_eq!(groups(cx), (s(&["lib.rs", "README.md", "main.rs"]), Some(s(&["README.md"])), true));
}

#[gpui::test]
fn merge_conflicts_resolve_navigate_and_guard_staging(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("merge") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let base: String = (1..=12).map(|i| format!("satır {i}\n")).collect();
    std::fs::write(dir.join("a.txt"), &base).unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "taban"]);
    git_in(&dir, &["checkout", "-q", "-b", "özellik"]);
    std::fs::write(dir.join("a.txt"), base.replace("satır 2\n", "özellik 2\n").replace("satır 10\n", "özellik 10\n")).unwrap();
    git_in(&dir, &["commit", "-q", "-am", "özellik"]);
    git_in(&dir, &["checkout", "-q", "main"]);
    std::fs::write(dir.join("a.txt"), base.replace("satır 2\n", "ana 2\n").replace("satır 10\n", "ana 10\n")).unwrap();
    git_in(&dir, &["commit", "-q", "-am", "ana"]);
    let merge = crate::test_git::git().args(["merge", "-q", "özellik"]).current_dir(&dir).output().unwrap();
    assert!(!merge.status.success(), "conflict expected");

    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let conflict_marked = |cx: &mut VisualTestContext| {
        wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|g| g.changes.iter().any(|c| c.path == "a.txt" && c.unstaged == Some(crate::git::Mark::Conflict))))
    };
    wait_ticking(cx, "çakışma durumu", conflict_marked);

    // While markers remain, "resolved" (stage) is rejected; the file stays conflicted.
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    scm.update(cx, |s, cx| s.stage(vec!["a.txt".into()], cx));
    wait_ticking(cx, "stage reddi", |cx| !scm.read_with(cx, |s, _| s.is_busy()) && scm.read_with(cx, |s, _| s.result_text()).is_some());
    assert!(scm.read_with(cx, |s, _| s.result_text()).unwrap().contains("still has merge conflict markers"));
    assert!(conflict_marked(cx));

    let p = dir.join("a.txt").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p, window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.conflict_count()), 2);
    let run = |cx: &mut VisualTestContext, label: &str| {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    };
    let line = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.cursor_position().0);
    let text = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.text());

    // Navigation: from the start to the first block, then the second, then wraps to the start; and back.
    run(cx, "Merge Conflict: Next Conflict");
    assert_eq!(line(cx), 2, "first block on line 2 (1-based)");
    run(cx, "Merge Conflict: Next Conflict");
    let second = line(cx);
    assert!(second > 6, "{second}");
    run(cx, "Merge Conflict: Next Conflict");
    assert_eq!(line(cx), 2, "wraps to the start at the end");
    run(cx, "Merge Conflict: Previous Conflict");
    assert_eq!(line(cx), second);

    // Cursor in the second block: Incoming → only that block is resolved.
    run(cx, "Merge Conflict: Accept Incoming");
    assert_eq!(ed.read_with(cx, |e, _| e.conflict_count()), 1);
    assert!(text(cx).contains("özellik 10\n") && !text(cx).contains("ana 10\n"));
    // Single undo step: the block comes back.
    cx.simulate_keystrokes("cmd-z");
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.conflict_count()), 2);
    assert!(text(cx).contains("ana 10\n") && text(cx).contains("özellik 10\n"));

    // Cursor outside a conflict: Accept Current does nothing, and says so.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Go: Line…", window, cx));
    cx.run_until_parked();
    cx.simulate_input("1");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let before = text(cx);
    run(cx, "Merge Conflict: Accept Current");
    assert_eq!(text(cx), before);
    assert!(wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default().contains("inside a merge conflict"));

    // Resolve everything with the current side, save, mark resolved, commit.
    run(cx, "Merge Conflict: Accept All Current");
    assert_eq!(ed.read_with(cx, |e, _| e.conflict_count()), 0);
    assert_eq!(text(cx), base.replace("satır 2\n", "ana 2\n").replace("satır 10\n", "ana 10\n"));
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    scm.update(cx, |s, cx| s.stage(vec!["a.txt".into()], cx));
    wait_ticking(cx, "çözüldü", |cx| !scm.read_with(cx, |s, _| s.is_busy()) && !conflict_marked(cx));
    git_in(&dir, &["commit", "-q", "--no-edit"]);
    assert!(git_in(&dir, &["log", "-1", "--format=%P"]).split_whitespace().count() == 2, "merge commit");
}

#[gpui::test]
fn conflict_markers_written_from_outside_are_picked_up(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("conflict-reload")).unwrap();
    let f = dir.join("b.txt");
    std::fs::write(&f, "bir\niki\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    let p = f.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p, window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert_eq!(ed.read_with(cx, |e, _| e.conflict_count()), 0);
    // Write from outside as git merge does: the clean file is reloaded.
    std::fs::write(&f, "<<<<<<< HEAD\nbir\n=======\nBİR\n>>>>>>> dal\niki\n").unwrap();
    wait_ticking(cx, "yeniden yükleme", |cx| ed.read_with(cx, |e, _| e.text()).starts_with("<<<<<<<"));
    assert_eq!(ed.read_with(cx, |e, _| e.conflict_count()), 1, "reloaded buffer should show the conflict");
}

#[gpui::test]
fn file_history_follows_renames_and_blame_sees_unsaved_lines(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("history") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    std::fs::write(dir.join("a.rs"), "fn bir() {}\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "a eklendi"]);
    std::fs::write(dir.join("a.rs"), "fn bir() {}\nfn iki() {}\n").unwrap();
    git_in(&dir, &["commit", "-q", "-am", "iki eklendi"]);
    git_in(&dir, &["mv", "a.rs", "b.rs"]);
    git_in(&dir, &["commit", "-q", "-m", "b olarak adlandırıldı"]);
    std::fs::write(dir.join("b.rs"), "fn bir() {}\nfn iki() {}\nfn üç() {}\n").unwrap();
    git_in(&dir, &["commit", "-q", "-am", "üç eklendi"]);

    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_ticking(cx, "git", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some()));
    let p = dir.join("b.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p, window, cx));
    cx.run_until_parked();
    let run = |cx: &mut VisualTestContext, label: &str| wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(label, window, cx));

    run(cx, "Git: View File History");
    wait_ticking(cx, "geçmiş", |cx| wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).is_some());
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).unwrap();
    assert_eq!(labels, ["üç eklendi", "b olarak adlandırıldı", "iki eklendi", "a eklendi"]);
    // The commit before the rename: the change at the old path (+1 line).
    cx.simulate_input("iki eklendi");
    cx.simulate_keystrokes("enter");
    wait_ticking(cx, "commit diff", |cx| wb.read_with(cx, |wb, _| wb.diff_view().is_some()));
    let view = wb.read_with(cx, |wb, _| wb.diff_view()).unwrap();
    assert_eq!(view.read_with(cx, |v, _| v.summary()), (1, 0, 1));
    assert!(view.read_with(cx, |v, _| v.label.to_string()).contains("iki eklendi"));

    // Blame: add an unsaved line at the top; "fn iki" (now line 3) still maps to the right commit.
    let p = dir.join("b.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(p, window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-up");
    cx.simulate_input("// yeni\n");
    cx.simulate_keystrokes("down");
    let notice = |cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.notice_text()).unwrap_or_default();
    run(cx, "Git: Blame Line");
    wait_ticking(cx, "blame", |cx| notice(cx).starts_with("Line 3"));
    assert!(notice(cx).contains("iki eklendi") && notice(cx).contains("Test"), "{}", notice(cx));
    cx.simulate_keystrokes("cmd-up");
    run(cx, "Git: Blame Line");
    wait_ticking(cx, "blame yeni", |cx| notice(cx).starts_with("Line 1"));
    assert_eq!(notice(cx), "Line 1: not committed yet");
}

#[gpui::test]
fn terminal_cmd_click_opens_file_locations(cx: &mut TestAppContext) {
    let dir = std::fs::canonicalize(project("termlink")).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wb.update(cx, |wb, _| wb.set_shell("/bin/sh"));
    cx.simulate_keystrokes("ctrl-`");
    let term = wb.read_with(cx, |wb, _| wb.active_terminal()).expect("terminal");
    // Like compiler output: "  --> src/lib.rs:3:2" and a nonexistent file.
    cx.simulate_input("printf '  --> src/lib.rs:3:2\\nyok/olmayan.rs:9\\n'\n");
    wait_for(cx, "çıktı", |cx| term.read_with(cx, |t, _| t.screen_text().lines().any(|l| l == "  --> src/lib.rs:3:2")));
    let row = term.read_with(cx, |t, _| t.screen_text().lines().position(|l| l == "  --> src/lib.rs:3:2")).unwrap();
    let on_link = term.read_with(cx, |t, _| t.cell_center(row, 10));

    // Hover with ⌘ held: the link is underlined (only as long as the link).
    cx.simulate_mouse_move(on_link, None, Modifiers::secondary_key());
    assert_eq!(term.read_with(cx, |t, _| t.hover_link()), Some((row, 6..20)));
    cx.simulate_mouse_move(on_link, None, Modifiers::none());
    assert_eq!(term.read_with(cx, |t, _| t.hover_link()), None, "underline goes away when ⌘ is released");

    // Clicking without ⌘ opens nothing.
    cx.simulate_click(on_link, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), None);

    // ⌘-click: the file opens at line 3, column 2.
    cx.simulate_click(on_link, Modifiers::secondary_key());
    wait_for(cx, "dosya açıldı", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some());
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), Some(dir.join("src/lib.rs").to_string_lossy().into_owned()));
    wait_for(cx, "konum", |cx| wb.read_with(cx, |wb, cx| wb.active_cursor(cx)) == Some((3, 2)));

    // A file not in the project: it says so, nothing opens.
    let row2 = term.read_with(cx, |t, _| t.screen_text().lines().position(|l| l == "yok/olmayan.rs:9")).unwrap();
    let missing = term.read_with(cx, |t, _| t.cell_center(row2, 3));
    cx.simulate_click(missing, Modifiers::secondary_key());
    wait_for(cx, "mesaj", |cx| wb.read_with(cx, |wb, _| wb.notice_text()).is_some_and(|n| n.contains("olmayan.rs")));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.open_paths(cx)).len(), 1);
}

#[gpui::test]
fn stage_and_revert_change_at_cursor(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("hunkcmd") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let lib = dir.join("src/lib.rs");
    let edited: String = (1..=10).map(|i| if i == 3 || i == 9 { format!("// DEĞİŞTİ {i}\n") } else { format!("// satır {i}\n") }).collect();
    std::fs::write(&lib, &edited).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &lib, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    use crate::git::LineMark;
    wait_for(cx, "gutter", |cx| ed.read_with(cx, |e, _| e.git_marks().get(2).copied().flatten() == Some(LineMark::Modified)));
    // Go to line 3, stage only that block: line 3 is changed in the index, line 9 is not.
    cx.simulate_keystrokes("ctrl-g");
    cx.simulate_input("3");
    cx.simulate_keystrokes("enter");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Stage Change at Cursor", window, cx));
    let staged = || {
        let o = crate::test_git::git().args(["show", ":src/lib.rs"]).current_dir(&dir).output().unwrap();
        String::from_utf8(o.stdout).unwrap()
    };
    wait_for(cx, "index", |_| staged().contains("DEĞİŞTİ 3"));
    assert!(!staged().contains("DEĞİŞTİ 9"), "only the cursor's block is staged");
    // Undo: go to line 9, revert the block to HEAD; undo brings it back.
    cx.simulate_keystrokes("ctrl-g");
    cx.simulate_input("9");
    cx.simulate_keystrokes("enter");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Revert Change at Cursor", window, cx));
    let text = ed.read_with(cx, |e, _| e.text());
    assert!(text.contains("// satır 9\n") && text.contains("DEĞİŞTİ 3"), "{text}");
    cx.simulate_keystrokes("cmd-z");
    assert!(ed.read_with(cx, |e, _| e.text()).contains("DEĞİŞTİ 9"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn next_change_wraps_and_close_saved_keeps_dirty(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("nextchange") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let lib = dir.join("src/lib.rs");
    let edited: String = (1..=10).map(|i| if i == 3 || i == 8 { format!("// X{i}\n") } else { format!("// satır {i}\n") }).collect();
    std::fs::write(&lib, &edited).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    open(&wb, &lib, cx);
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    use crate::git::LineMark;
    wait_for(cx, "gutter", |cx| ed.read_with(cx, |e, _| e.git_marks().get(7).copied().flatten() == Some(LineMark::Modified)));
    cx.simulate_keystrokes("alt-f3");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)).map(|c| c.0), Some(3));
    cx.simulate_keystrokes("alt-f3");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)).map(|c| c.0), Some(8));
    cx.simulate_keystrokes("alt-f3");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)).map(|c| c.0), Some(3), "wraps to the start at the end");
    cx.simulate_keystrokes("shift-alt-f3");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)).map(|c| c.0), Some(8), "wraps around");

    // Saved tabs close, the dirty one remains.
    open(&wb, &dir.join("README.md"), cx);
    cx.simulate_input("x");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Close Saved Editors", window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).path.ends_with("README.md"))), Some(true));
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn git_branch_commands_through_the_palette(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("branchcmds") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let git = |args: &[&str]| crate::test_git::git().args(args).current_dir(&dir).output().unwrap();
    git(&["branch", "side"]);
    let (wb, cx) = setup(cx);
    open(&wb, &dir, cx);
    wait_for(cx, "git durumu", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some()));
    // Rename: the picker opens, "side" is selected, the new name is entered.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Rename Branch…", window, cx));
    wait_for(cx, "dal listesi", |cx| wb.read_with(cx, |wb, cx| wb.overlay_query(cx).is_some()));
    cx.simulate_input("side");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("renamed");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "yeniden adlandırma", |_| String::from_utf8_lossy(&git(&["branch", "--list", "renamed"]).stdout).contains("renamed"));
    assert!(!String::from_utf8_lossy(&git(&["branch", "--list", "side"]).stdout).contains("side"));
    // The tag is created at HEAD.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Create Tag…", window, cx));
    cx.simulate_input("v9");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "tag", |_| String::from_utf8_lossy(&git(&["tag", "--list"]).stdout).contains("v9"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn search_query_opens_in_a_search_editor_tab(cx: &mut TestAppContext) {
    let dir = project("searchtab");
    let (wb, cx) = setup_with_config(cx, &dir.join("cfg"));
    open(&wb, &dir, cx);
    let search = wb.read_with(cx, |wb, _| wb.search_panel());
    cx.simulate_keystrokes("cmd-shift-f");
    cx.simulate_input("SATIR 7");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "arama", |cx| search.read_with(cx, |s, _| !s.is_searching()));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Search: Open Search Editor", window, cx));
    let ed = wb.read_with(cx, |wb, _| wb.active_search_editor_for_test()).expect("a Search Editor tab");
    wait_for(cx, "sonuç sekmesi", |cx| ed.read_with(cx, |e, _| e.doc.state == crate::search_doc::RunState::Complete));
    let text = ed.read_with(cx, |e, _| e.doc.as_text());
    assert!(text.contains("# Query: SATIR 7") && text.contains("src/lib.rs:\n  7: // satır 7"), "{text}");
}

#[gpui::test]
fn open_as_hex_creates_a_dump_tab(cx: &mut TestAppContext) {
    let dir = project("hexcmd");
    std::fs::write(dir.join("blob.txt"), b"AB\x00\x01").unwrap();
    let (wb, cx) = setup_with_config(cx, &dir.join("cfg"));
    open(&wb, &dir.join("blob.txt"), cx);
    // A file containing a NUL byte doesn't open as text: an error page, hex from there.
    assert!(wb.read_with(cx, |wb, _| wb.open_error_text()).is_some_and(|m| m.contains("binary file")));
    wb.update_in(cx, |wb, window, cx| wb.open_hex_of_for_test(dir.join("blob.txt").to_string_lossy().into_owned(), window, cx));
    wait_for(cx, "hex sekmesi", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some_and(|p| p.ends_with(".hex.txt")));
    let text = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap().read_with(cx, |e, _| e.text());
    assert!(text.starts_with("00000000  41 42 00 01"), "{text}");
    assert!(text.contains("|AB..|"));
}

#[gpui::test]
fn open_with_picker_lists_alternatives(cx: &mut TestAppContext) {
    let dir = project("openwith");
    let (wb, cx) = setup_with_config(cx, &dir.join("cfg"));
    open(&wb, &dir.join("README.md"), cx);
    cx.simulate_input("x"); // dirty: differs from the saved version
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("File: Open Active File With…", window, cx));
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, cx| wb.overlay_query(cx).is_some()));
    cx.simulate_input("Compare");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.diff_view().is_some()), "comparison with the saved version opened");
}

// --- git-refs ---

#[gpui::test]
fn git_tag_with_a_message_is_annotated_and_open_file_head_shows_the_commit(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("refscmds") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("README.md"), cx);
    wait_for(cx, "git durumu", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some()));
    // The first word is the tag, the rest its message.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Create Tag…", window, cx));
    cx.simulate_input("v1 first release");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "açıklamalı etiket", |_| git_in(&dir, &["tag", "-n1", "-l", "v1"]).contains("first release"));
    assert_eq!(git_in(&dir, &["cat-file", "-t", "v1"]).trim(), "tag");
    // The committed text of the file, not the edited one.
    std::fs::write(dir.join("README.md"), "# değişti\n").unwrap();
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Open File (HEAD)", window, cx));
    wait_for(cx, "HEAD sekmesi", |cx| wb.read_with(cx, |wb, cx| wb.active_path(cx)).is_some_and(|p| p.contains("README (HEAD)")));
    let text = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap().read_with(cx, |e, _| e.text());
    assert_eq!(text, "# proje\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn git_add_to_gitignore_and_named_stash_through_the_palette(cx: &mut TestAppContext) {
    let Some(dir) = git_repo("refsignore") else { return };
    let dir = std::fs::canonicalize(dir).unwrap();
    std::fs::write(dir.join("secret.env"), "k=1\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("secret.env"), cx);
    wait_for(cx, "git durumu", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|g| g.changes.iter().any(|c| c.path == "secret.env"))));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Add to .gitignore", window, cx));
    // The fixture already has a .gitignore: the entry is appended to it.
    wait_for(cx, ".gitignore", |_| std::fs::read_to_string(dir.join(".gitignore")).is_ok_and(|t| t == "ignored.txt\n/secret.env\n"));
    wait_for(cx, "durum yenilendi", |cx| wb.read_with(cx, |wb, _| wb.git_status().is_some_and(|g| g.changes.iter().all(|c| c.path != "secret.env"))));
    // A named stash of a tracked edit.
    std::fs::write(dir.join("README.md"), "# yarım\n").unwrap();
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Git: Stash with Name…", window, cx));
    cx.simulate_input("yarım iş");
    cx.simulate_keystrokes("enter");
    wait_for(cx, "stash", |_| git_in(&dir, &["stash", "list"]).contains("yarım iş"));
    assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "# proje\n");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Focus rings follow the last input: a key shows them, a click hides them again.
#[gpui::test]
fn focus_rings_show_after_keys_and_hide_after_clicks(cx: &mut TestAppContext) {
    let (_wb, cx) = setup(cx);
    assert!(!cx.update(|_, cx| crate::ui::keyboard_focus(cx)));
    cx.simulate_keystrokes("tab");
    assert!(cx.update(|_, cx| crate::ui::keyboard_focus(cx)), "a key press switches to keyboard focus");
    cx.simulate_click(gpui::point(gpui::px(400.), gpui::px(300.)), gpui::Modifiers::none());
    assert!(!cx.update(|_, cx| crate::ui::keyboard_focus(cx)), "a click switches back");
}
