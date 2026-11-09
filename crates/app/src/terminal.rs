//! Integrated terminal (S08): VT parsing with alacritty_terminal, PTY on the agent
//! (local or remote, same protocol). Colors are derived from the theme.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, Processor, StdSyncHandler};
use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, Entity, EventEmitter, FocusHandle, Focusable, FontStyle, FontWeight, GlobalElementId,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, ScrollWheelEvent, SharedString, Style, TextRun, Window, actions, div,
    fill, point, prelude::*, px, relative, size,
};
use host::{Host, PtyEvent};

use crate::ui::{MONO_FONT, Palette, c, palette};

use colors::{mix, resolve};

actions!(
    terminal,
    [
        TermPaste,
        TermCopy,
        TermClear,
        TermReset,
        TermSelectAll,
        TermFind,
        TermFindNext,
        TermFindPrevious,
        TermScrollPageUp,
        TermScrollPageDown,
        TermScrollToTop,
        TermScrollToBottom,
        TermPrevCommand,
        TermNextCommand,
        TermCopyLastOutput
    ]
);

#[path = "terminal_attach.rs"]
mod attach;
#[path = "terminal_clear.rs"]
mod clear;
#[path = "terminal_colors.rs"]
mod colors;
#[path = "terminal_find.rs"]
mod find;
#[path = "terminal_marks.rs"]
mod marks;
#[path = "terminal_modes.rs"]
mod modes;

pub use marks::{Activity, Finished};

#[path = "terminal_mouse.rs"]
mod mouse;
use mouse::selects_text;
#[path = "terminal_restore.rs"]
mod restore;
#[path = "terminal_revival.rs"]
mod revival;
#[path = "terminal_scroll.rs"]
mod scroll;
#[path = "terminal_selection.rs"]
mod selection;

#[cfg(test)]
#[path = "terminal_scroll_tests.rs"]
mod scroll_tests;

#[cfg(test)]
#[path = "terminal_redraw_tests.rs"]
mod redraw_tests;

pub enum TerminalEvent {
    TitleChanged,
    /// A command started or finished (shell integration): the terminal list shows it.
    ActivityChanged,
    /// The program rang the bell or sent a notification (OSC 9/777), with its message if it had one.
    Attention(Option<String>),
    /// A command that ran for 10 seconds or more ended: the line it was typed on and its exit code.
    LongCommandFinished {
        command: String,
        code: Option<i32>,
    },
    Exited,
    /// A key was pressed after the shell exited.
    CloseRequested,
    /// A paste of several lines waits for the user's yes (`paste_confirmed` sends it).
    ConfirmPaste {
        data: Vec<u8>,
        lines: usize,
    },
    /// An image could not be transferred to the terminal's host.
    ImagePasteFailed(String),
    /// ⌘-click: URL or file location.
    OpenLink(crate::links::Link),
    /// Right click (window coordinates): copy, paste, select all, find, clear.
    ContextMenu(Point<Pixels>),
    /// The terminal daemon gave the token to take this terminal back after a restart: save it.
    Kept,
}

/// Most scrollback lines (`terminal.scrollback`): 50,000 lines of 200 columns already take about 200 MB per terminal,
/// times up to eight terminals, so the settings say so above `SCROLLBACK_COMFORT`.
pub const SCROLLBACK_MAX: usize = 200_000;

/// Scrollback up to here (the default) costs little; more is allowed, with a warning about memory and speed.
pub const SCROLLBACK_COMFORT: usize = 50_000;

/// What a new terminal runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    /// Program (`None`: the user's shell).
    pub shell: Option<String>,
    /// Arguments (`None`: a login shell).
    pub args: Option<Vec<String>>,
    pub env: Vec<(String, String)>,
    pub color: Option<String>,
}

/// Upper bound (bytes) on terminal output not yet parsed.
const MAX_BACKLOG: usize = 32 * 1024 * 1024;
/// The window hears of a changed title at most this often.
const TITLE_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// Pending-output counter between the bridge thread and the UI: a chunk over the limit is dropped (see `admit`).
pub(crate) struct OutputBacklog {
    limit: usize,
    pending: std::sync::atomic::AtomicUsize,
    dropped: std::sync::atomic::AtomicBool,
}

impl Default for OutputBacklog {
    fn default() -> Self {
        Self::with_limit(MAX_BACKLOG)
    }
}

impl OutputBacklog {
    /// A backlog that admits at most `limit` bytes not yet consumed.
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self { limit, pending: Default::default(), dropped: Default::default() }
    }

    /// Accept the new `len` bytes of output (`true`) or drop it (`false`, which raises the "dropped" flag).
    pub(crate) fn admit(&self, len: usize) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        if self.pending.load(Relaxed).saturating_add(len) > self.limit {
            self.dropped.store(true, Relaxed);
            return false;
        }
        self.pending.fetch_add(len, Relaxed);
        true
    }

    /// The UI parsed a chunk.
    pub(crate) fn consumed(&self, len: usize) {
        self.pending.fetch_sub(len, std::sync::atomic::Ordering::Relaxed);
    }

    /// `true` once if output was dropped (so the user can be told).
    pub(crate) fn take_dropped(&self) -> bool {
        self.dropped.swap(false, std::sync::atomic::Ordering::Relaxed)
    }
}

/// The text a terminal event wants on the clipboard, if `allowed`.
fn clipboard_text(ev: TermEvent, allowed: bool) -> Option<String> {
    match ev {
        TermEvent::ClipboardStore(_, s) if allowed => Some(s),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// What the terminal sends back to the app (title, replies to the PTY) is queued.
#[derive(Clone, Default)]
struct Listener(Rc<RefCell<Vec<TermEvent>>>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        self.0.borrow_mut().push(event);
    }
}

pub struct TerminalView {
    /// Folder it was started in (tests read it).
    #[cfg_attr(not(test), allow(dead_code, reason = "only tests read it"))]
    cwd: Option<String>,
    host: Arc<dyn Host>,
    pty: Option<u32>,
    /// A close request must also cancel a shell which has not finished starting.
    closed: Cell<bool>,
    starting: bool,
    /// Input sent before the shell started (task command): written once the pty is up.
    pending_input: Vec<u8>,
    /// Fixed title (task name): the shell's title updates don't change it.
    title_locked: bool,
    /// When the window was last told the title changed, and whether a later telling is waiting: a program that animates its
    /// title (a spinner) would otherwise rebuild the whole window for every frame of it.
    title_told: std::time::Instant,
    title_waiting: bool,
    /// Program and arguments it was started with (`None`: the user's shell), kept so a restart can start the same.
    launched: (Option<String>, Option<Vec<String>>),
    /// Started with a shell (not an output-only terminal): saved for the next start from the moment it opens.
    has_shell: bool,
    /// The token to take this terminal back after a restart (a terminal kept by the terminal daemon).
    token: Option<crate::terminal_session::SavedPty>,
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    events: Listener,
    pub title: SharedString,
    exit: Option<Option<i32>>,
    error: Option<SharedString>,
    size: TermSize,
    /// Grid origin and cell width from the last paint (for mouse → cell).
    grid: (Point<Pixels>, Pixels),
    selecting: bool,
    /// Programs may erase the scrollback (`terminal.allowClearHistory`).
    allow_clear_history: bool,
    /// The start of an erase sequence that a read ended in the middle of (terminal_clear.rs).
    clear_carry: Vec<u8>,
    /// "Hold Shift to select": shown for a moment when a drag went to a program that reads the mouse; when it last showed.
    shift_hint: bool,
    shift_hint_last: Option<std::time::Instant>,
    /// The left button as a program that reads the mouse sees it (terminal_mouse.rs).
    program_mouse: mouse::ProgramMouse,
    /// Link hovered while ⌘ is held: (screen row, columns).
    hover_link: Option<(usize, std::ops::Range<usize>)>,
    /// Where the link under the pointer (OSC 8: any text can stand for any address) really goes.
    hover_uri: Option<SharedString>,
    /// The find bar, while it is open.
    find: Option<find::FindState>,
    /// Font size and line height from `terminal.fontSize` (set on every render, so zoom and setting changes apply at once).
    metrics: (f32, f32),
    /// Scrollback lines in effect.
    scrollback: usize,
    wheel: scroll::WheelScroll,
    /// Mark colour in the terminal list (a theme role from `terminal_profiles::COLORS`).
    pub color: Option<String>,
    /// Shell integration: sequences in the output and the commands they mark.
    osc: crate::terminal_osc::Scanner,
    marks: marks::Marks,
    focus_handle: FocusHandle,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    /// Folder the terminal was started in (a split terminal opens in the same place).
    pub fn start_dir(&self) -> Option<String> {
        self.cwd.clone()
    }

    #[cfg(test)]
    pub fn cwd_for_test(&self) -> Option<String> {
        self.start_dir()
    }

    pub fn new(host: Arc<dyn Host>, cwd: Option<String>, shell: Option<String>, cx: &mut Context<Self>) -> Self {
        Self::new_with(host, cwd, Launch { shell, ..Launch::default() }, cx)
    }

    /// A terminal running `launch` (a profile or a picked shell).
    pub fn new_with(host: Arc<dyn Host>, cwd: Option<String>, launch: Launch, cx: &mut Context<Self>) -> Self {
        Self::start(host, cwd, launch, false, cx)
    }

    /// A terminal that only shows output given to it (test runs): no shell, keys go nowhere.
    pub fn output_only(host: Arc<dyn Host>, title: String, cx: &mut Context<Self>) -> Self {
        let mut view = Self::blank(host, None, Some("info".into()), cx);
        view.title = title.into();
        view.title_locked = true;
        view
    }

    /// Output for an output-only terminal (line breaks become what a terminal expects).
    pub fn append(&mut self, data: &[u8], cx: &mut Context<Self>) {
        let mut out = Vec::with_capacity(data.len() + data.len() / 16);
        for &b in data {
            if b == b'\n' {
                out.push(b'\r');
            }
            out.push(b);
        }
        self.feed(&out);
        cx.notify();
    }

    /// Clears an output-only terminal for a new run.
    pub fn reset_output(&mut self, cx: &mut Context<Self>) {
        // Our own clear, not a program's: it takes the history along.
        // Screen first, then the history: the other way round the cleared screen would end up in it.
        self.feed_chunk(b"\x1b[H\x1b[2J\x1b[3J");
        cx.notify();
    }

    /// The view without a shell yet.
    fn blank(host: Arc<dyn Host>, cwd: Option<String>, color: Option<String>, cx: &mut Context<Self>) -> Self {
        let size = TermSize { cols: 100, rows: 24 };
        let events = Listener::default();
        // Scrollback from settings (default 2,000; 10,000 × 200 columns can reach ~45 MB).
        let scrollback = cx.global::<crate::settings::Settings>().terminal_scrollback.min(SCROLLBACK_MAX);
        let config = Config { scrolling_history: scrollback, ..Config::default() };
        let term = Term::new(config, &size, events.clone());
        TerminalView {
            title: "terminal".into(),
            cwd,
            host,
            pty: None,
            closed: Cell::new(false),
            starting: false,
            pending_input: Vec::new(),
            title_locked: false,
            title_told: std::time::Instant::now() - TITLE_EVERY,
            title_waiting: false,
            launched: (None, None),
            has_shell: false,
            token: None,
            term,
            parser: Processor::new(),
            events,
            exit: None,
            error: None,
            size,
            grid: (Point::default(), px(8.)),
            selecting: false,
            allow_clear_history: cx.global::<crate::settings::Settings>().terminal_allow_clear_history,
            clear_carry: Vec::new(),
            shift_hint: false,
            shift_hint_last: None,
            program_mouse: Default::default(),
            find: None,
            hover_link: None,
            hover_uri: None,
            metrics: {
                let s = cx.global::<crate::settings::Settings>();
                metrics(s.terminal_font_size, s.terminal_line_height)
            },
            scrollback,
            wheel: Default::default(),
            color,
            osc: Default::default(),
            marks: Default::default(),
            focus_handle: cx.focus_handle(),
        }
    }

    /// Bytes from the program. A full-screen clear (`CSI 2 J`) wipes the screen without pushing it into the history
    /// (terminal_clear.rs); everything else goes to the parser as it comes.
    fn feed_chunk(&mut self, data: &[u8]) {
        // TUIs that redraw in place erase the selected lines; the selection follows its text (terminal_selection.rs).
        let kept = self.keep_selection();
        // Each shell-integration sequence is noted where it ends, with the cursor where the parser left it.
        let mut at = 0;
        for (end, ev) in self.osc.scan(data) {
            self.parser.advance(&mut self.term, &data[at..end]);
            at = end;
            self.on_osc(ev);
        }
        self.parser.advance(&mut self.term, &data[at..]);
        self.restore_selection(kept, selection::scroll_bound(data, self.term.screen_lines()));
        // Deleting lines (`CSI M`, which full-screen programs send all the time) while the view is scrolled back can leave the
        // parser's view offset past the end of the history, and drawing that panics: bring it back inside.
        let grid = self.term.grid();
        let (offset, history) = (grid.display_offset(), grid.history_size());
        if offset > history {
            self.term.scroll_display(alacritty_terminal::grid::Scroll::Delta(history as i32 - offset as i32));
        }
        self.update_mark_shift();
        // New output can push lines into the history, so a remembered match would point at the wrong cells.
        self.forget_find_match();
    }

    /// A program that began a synchronized update (`CSI ? 2026 h`) and never ended it must not freeze the screen: the parser
    /// holds the bytes back until the end comes, and only the caller can say that the time is up. Applies what was held once
    /// it is; otherwise returns how long until then (`None`: no update pending).
    pub(super) fn release_expired_sync(&mut self) -> Option<std::time::Duration> {
        let deadline = self.parser.sync_timeout().sync_timeout()?;
        let left = deadline.checked_duration_since(std::time::Instant::now());
        if left.is_none() {
            self.parser.stop_sync(&mut self.term);
        }
        left
    }

    /// A title from the program: nothing happens when it is the same, and the window is told at most every `TITLE_EVERY`
    /// (the first change at once, a quicker one after the pause).
    fn set_title(&mut self, title: SharedString, cx: &mut Context<Self>) {
        if self.title == title {
            return;
        }
        self.title = title;
        if self.title_told.elapsed() >= TITLE_EVERY {
            self.title_told = std::time::Instant::now();
            cx.emit(TerminalEvent::TitleChanged);
        } else if !std::mem::replace(&mut self.title_waiting, true) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(TITLE_EVERY).await;
                this.update(cx, |t, cx| {
                    t.title_waiting = false;
                    t.title_told = std::time::Instant::now();
                    cx.emit(TerminalEvent::TitleChanged);
                })
                .ok();
            })
            .detach();
        }
    }

    fn drain_events(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.marks.activity_changed) {
            cx.emit(TerminalEvent::ActivityChanged);
        }
        let events: Vec<TermEvent> = std::mem::take(&mut *self.events.0.borrow_mut());
        for ev in events {
            match ev.clone() {
                // The terminal is answering a query (like the cursor position): write it back to the shell.
                TermEvent::PtyWrite(s) => self.write(s.into_bytes()),
                TermEvent::Title(_) | TermEvent::ResetTitle if self.title_locked => {}
                TermEvent::Title(t) => self.set_title(t.into(), cx),
                TermEvent::ResetTitle => self.set_title("terminal".into(), cx),
                // OSC 52: as the setting says; by default right after a copy in the program (terminal_mouse.rs).
                TermEvent::ClipboardStore(..) => {
                    let allowed = self.takes_clipboard_write(&cx.global::<crate::settings::Settings>().terminal_allow_clipboard_write);
                    if let Some(s) = clipboard_text(ev, allowed) {
                        cx.write_to_clipboard(ClipboardItem::new_string(s))
                    }
                }
                TermEvent::Bell => self.ask_attention(None),
                _ => {}
            }
        }
        if let Some(message) = self.marks.attention.take() {
            if cx.global::<crate::settings::Settings>().terminal_notifications {
                cx.emit(TerminalEvent::Attention(message));
            } else if self.unseen_finish() == Some(Finished::Attention) {
                self.mark_seen();
            }
        }
        if let Some((command, code)) = self.marks.long_finish.take()
            && cx.global::<crate::settings::Settings>().terminal_notifications
        {
            cx.emit(TerminalEvent::LongCommandFinished { command, code });
        }
    }

    fn on_exit(&mut self, code: Option<i32>, cx: &mut Context<Self>) {
        if self.exit.is_none() {
            // Nothing reads the mouse any more (the program is gone, or the SSH connection dropped under it): without this the
            // clicks go nowhere and the last output cannot be selected and copied.
            self.feed(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?2004l\x1b[?1l\x1b>");
            self.selecting = false;
            self.exit = Some(code);
            cx.emit(TerminalEvent::Exited);
            cx.notify();
        }
    }

    /// Host and terminal id, to ask what runs in it (`None` until the shell has started).
    pub fn pty_handle(&self) -> Option<(Arc<dyn Host>, u32)> {
        self.pty.map(|id| (self.host.clone(), id))
    }

    pub fn has_exited(&self) -> bool {
        self.exit.is_some()
    }

    fn write(&mut self, data: Vec<u8>) {
        if self.exit.is_some() || self.closed.get() {
            return;
        }
        match self.pty {
            Some(id) => self.host.pty_input(id, data),
            None if self.starting => self.pending_input.extend_from_slice(&data),
            None => {}
        }
    }

    /// Send text to the shell (once it starts, if it hasn't yet).
    pub fn send(&mut self, text: &str) {
        self.write(text.as_bytes().to_vec());
    }

    /// Type `text` and press Enter after each of its lines. Enter is a carriage return, as the key sends it: every shell
    /// takes it, and the Windows console does not take a line feed as Enter.
    pub fn send_lines(&mut self, text: &str) {
        let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').map(|l| l.trim_end_matches('\r')).collect();
        self.send(&format!("{}\r", lines.join("\r")));
    }

    pub fn set_fixed_title(&mut self, title: String) {
        self.title = title.into();
        self.title_locked = true;
    }

    /// Close the shell (when the tab is closed).
    pub fn kill(&self) {
        if self.closed.replace(true) {
            return;
        }
        if let Some(id) = self.pty {
            self.host.pty_kill(id);
        }
    }

    /// View size changed: update the grid and the PTY.
    fn set_size(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if (cols, rows) == (self.size.cols, self.size.rows) {
            return;
        }
        self.size = TermSize { cols, rows };
        self.term.resize(self.size);
        if let Some(id) = self.pty {
            self.host.pty_resize(id, cols as u16, rows as u16);
        }
    }

    /// All text including scrollback. Wrapped lines are joined into one (to extract problems from task
    /// output: a long compiler line must not be split).
    pub fn scrollback_text(&self) -> String {
        let grid = self.term.grid();
        let cols = grid.columns();
        let mut out = String::new();
        for i in grid.topmost_line().0..=grid.bottommost_line().0 {
            let row = &grid[Line(i)];
            let text: String = (0..cols).map(|c| &row[Column(c)]).filter(|cell| !cell.flags.contains(Flags::WIDE_CHAR_SPACER)).map(|cell| cell.c).collect();
            if row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
                out.push_str(&text);
            } else {
                out.push_str(text.trim_end());
                out.push('\n');
            }
        }
        out
    }

    /// Text on screen (for tests).
    #[cfg(test)]
    pub fn hover_link(&self) -> Option<(usize, std::ops::Range<usize>)> {
        self.hover_link.clone()
    }

    #[cfg(test)]
    pub fn screen_text(&self) -> String {
        self.a11y_view().0.join("\n")
    }

    /// For the accessibility tree (a11y.rs): the rows on screen (trailing blanks trimmed) and the cursor (row, column),
    /// if it is on screen.
    /// For the accessibility tree: how far the view is scrolled back and how many lines of history there are.
    pub fn a11y_scroll(&self) -> (usize, usize) {
        let grid = self.term.grid();
        (grid.display_offset(), grid.history_size())
    }

    /// A page through the history from the accessibility tree (up: older lines), as Shift+Page Up/Down do.
    pub fn a11y_scroll_page(&mut self, up: bool, cx: &mut Context<Self>) {
        self.term.scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
        cx.notify();
    }

    pub fn a11y_view(&self) -> (Vec<String>, Option<(usize, usize)>) {
        let content = self.term.renderable_content();
        let offset = content.display_offset as i32;
        let mut rows = vec![String::new(); self.size.rows];
        for cell in content.display_iter {
            let row = (cell.point.line.0 + offset) as usize;
            if row < rows.len() && !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                rows[row].push(cell.c);
            }
        }
        let cur_row = content.cursor.point.line.0 + offset;
        let cursor = (cur_row >= 0 && (cur_row as usize) < rows.len()).then_some((cur_row as usize, content.cursor.point.column.0));
        (rows.into_iter().map(|r| r.trim_end().to_string()).collect(), cursor)
    }

    /// For the accessibility tree: where the characters of `rows` (as `a11y_view` returns them) were drawn in the last
    /// paint, one cell each (two for a wide character); `starts` is each row's first character in the exposed text.
    /// Empty before the first paint.
    pub fn a11y_layout(&self, rows: &[String], starts: &[usize]) -> Vec<crate::painted::TextLine> {
        let (origin, cell_w) = self.grid;
        if cell_w <= px(0.) {
            return Vec::new();
        }
        let line_h = self.line_h();
        let content = self.term.renderable_content();
        let offset = content.display_offset as i32;
        let mut cells: Vec<Vec<(usize, usize)>> = vec![Vec::new(); rows.len()];
        for cell in content.display_iter {
            let row = (cell.point.line.0 + offset) as usize;
            if row < cells.len() && !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                cells[row].push((cell.point.column.0, if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 }));
            }
        }
        rows.iter()
            .zip(starts)
            .zip(cells)
            .enumerate()
            .map(|(r, ((text, start), cells))| {
                let n = text.chars().count().min(cells.len());
                let x = |col: usize| origin.x + cell_w * col as f32;
                let end = x(cells[..n].last().map_or(0, |(col, w)| col + w));
                let cells = cells[..n].iter().map(|&(col, w)| (x(col), x(col + w))).collect();
                let top = origin.y + line_h * r as f32;
                crate::painted::TextLine { start: *start, cells, end, top, bottom: top + line_h, line_break: px(0.) }
            })
            .collect()
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Typing in the find bar: those keys bubble up here but are not for the shell.
        if self.find_has_focus(window, cx) {
            return;
        }
        // On Windows and Linux, Ctrl+C copies selected terminal text. Without a
        // selection it must still reach the program as the interrupt character.
        let k = &ev.keystroke;
        if !cfg!(target_os = "macos")
            && k.key == "c"
            && k.modifiers.control
            && !k.modifiers.shift
            && !k.modifiers.alt
            && !k.modifiers.platform
            && let Some(text) = self.term.selection_to_string().filter(|s| !s.is_empty())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.term.selection = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // The shell is gone: a key closes the tab, except those that read the output left behind (the copy above, paging).
        if self.exit.is_some() || self.error.is_some() {
            let reading = matches!(k.key.as_str(), "pageup" | "pagedown" | "home" | "end");
            if !k.modifiers.platform && !reading {
                cx.stop_propagation();
                cx.emit(TerminalEvent::CloseRequested);
            }
            return;
        }
        // A remote program cannot read the local image clipboard itself.
        // Preserve Ctrl+V for programs when there is no image on the clipboard.
        if k.key == "v"
            && k.modifiers.control
            && !k.modifiers.shift
            && !k.modifiers.alt
            && !k.modifiers.platform
            && self.host.is_remote()
            && let Some(image) = cx.read_from_clipboard().filter(|item| item.text().is_none_or(|t| t.trim().is_empty())).and_then(Self::clipboard_image)
        {
            cx.stop_propagation();
            self.paste_image(image, cx);
            return;
        }
        let Some(bytes) = key_to_bytes(&ev.keystroke, *self.term.mode()) else { return };
        // Ctrl+C with a selection inside the program copies it there (Claude Code), over SSH through OSC 52.
        if k.key == "c" && k.modifiers.control && !k.modifiers.alt && !k.modifiers.platform {
            self.program_mouse.note_copy_key();
        }
        cx.stop_propagation();
        self.term.selection = None;
        self.term.scroll_display(Scroll::Bottom);
        self.write(bytes);
        cx.notify();
    }

    /// Height of one terminal row.
    fn line_h(&self) -> Pixels {
        px(self.metrics.1)
    }

    /// Window position → grid cell + which half of the cell.
    fn grid_point(&self, pos: Point<Pixels>) -> (GridPoint, Side) {
        let (origin, cell_w) = self.grid;
        let line_h = self.line_h();
        let x = ((pos.x - origin.x) / cell_w).max(0.);
        let row = ((pos.y - origin.y) / line_h).floor().clamp(0., (self.size.rows - 1) as f32) as i32;
        let col = (x.floor() as usize).min(self.size.cols - 1);
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
        let offset = self.term.grid().display_offset() as i32;
        (GridPoint::new(Line(row - offset), Column(col)), side)
    }

    /// The address of a link the program marked (OSC 8) under the position, whether or not it opens.
    fn marked_link_uri(&self, pos: Point<Pixels>) -> Option<String> {
        self.uri_at(self.grid_point(pos).0)
    }

    fn uri_at(&self, at: GridPoint) -> Option<String> {
        let uri = self.term.grid()[at.line][at.column].hyperlink()?.uri().to_string();
        Some(uri.chars().filter(|c| !c.is_control()).take(300).collect())
    }

    /// Link at the position: (screen row, columns, link). The row is read cell by cell.
    fn link_under(&self, pos: Point<Pixels>) -> Option<(usize, std::ops::Range<usize>, crate::links::Link)> {
        let (at, _) = self.grid_point(pos);
        // `iter_from` skips the starting cell; read the row directly.
        let row = &self.term.grid()[at.line];
        let screen_row = (at.line.0 + self.term.grid().display_offset() as i32).max(0) as usize;
        // A link the program marked (OSC 8): its target, over the cells that carry it. Only web addresses open.
        if let Some(h) = row[at.column].hyperlink().filter(|h| h.uri().starts_with("https://") || h.uri().starts_with("http://")) {
            let same = |c: usize| row[Column(c)].hyperlink().is_some_and(|x| x == h);
            let start = (0..=at.column.0).rev().take_while(|&c| same(c)).last().unwrap_or(at.column.0);
            let end = (at.column.0..self.size.cols).take_while(|&c| same(c)).last().unwrap_or(at.column.0) + 1;
            return Some((screen_row, start..end, crate::links::Link::Url(h.uri().to_string())));
        }
        let cells: Vec<char> =
            (0..self.size.cols).map(|c| &row[Column(c)]).map(|cell| if cell.flags.contains(Flags::WIDE_CHAR_SPACER) { '\0' } else { cell.c }).collect();
        let (cols, link) = crate::links::link_at(&cells, at.column.0)?;
        Some((screen_row, cols, link))
    }

    /// Drag: character, double click: word, triple click: line. Shift extends. ⌘-click: link.
    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        if ev.modifiers.secondary()
            && !ev.modifiers.shift
            && let Some((_, _, link)) = self.link_under(ev.position)
        {
            self.hover_link = None;
            cx.emit(TerminalEvent::OpenLink(link));
            cx.notify();
            return;
        }
        // The program wants the mouse: the click goes to it (⇧ still selects text, like VS Code).
        if !selects_text(&ev.modifiers) && self.report_press(ev.position, &ev.modifiers, ev.click_count) {
            return;
        }
        let (at, side) = self.grid_point(ev.position);
        match (&mut self.term.selection, ev.modifiers.shift) {
            (Some(sel), true) => sel.update(at, side),
            _ => {
                let ty = match ev.click_count {
                    2 => SelectionType::Semantic,
                    n if n >= 3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                self.term.selection = Some(Selection::new(ty, at, side));
            }
        }
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Links are underlined while ⌘ is held (redraws only on change).
        let hover = (ev.modifiers.secondary() && ev.pressed_button.is_none()).then(|| self.link_under(ev.position).map(|(r, c, _)| (r, c))).flatten();
        if hover != self.hover_link {
            self.hover_link = hover;
            cx.notify();
        }
        let uri = self.marked_link_uri(ev.position).map(SharedString::from);
        if uri != self.hover_uri {
            self.hover_uri = uri;
            cx.notify();
        }
        // Drags and moves the program follows (Claude Code selects text this way).
        if !self.selecting && self.report_motion(ev) {
            return;
        }
        if !self.selecting || ev.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            if ev.pressed_button == Some(MouseButton::Left) && !selects_text(&ev.modifiers) && self.term.mode().intersects(TermMode::MOUSE_MODE) {
                self.show_shift_hint(cx);
            }
            return;
        }
        let (at, side) = self.grid_point(ev.position);
        if let Some(sel) = &mut self.term.selection {
            sel.update(at, side);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, ev: &gpui::MouseUpEvent, cx: &mut Context<Self>) {
        if !selects_text(&ev.modifiers) && !self.selecting && self.report_release(ev.position, &ev.modifiers) {
            return;
        }
        let was_selecting = std::mem::take(&mut self.selecting);
        // Single click without dragging: don't leave an empty selection.
        if self.term.selection.as_ref().is_some_and(|s| s.is_empty()) {
            self.term.selection = None;
        } else if was_selecting {
            self.copy_on_select(cx);
        }
    }

    fn on_other_button_up(&mut self, ev: &gpui::MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.report_other_button(ev.button, false, ev.position, &ev.modifiers);
    }

    #[cfg(test)]
    pub fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string()
    }

    /// Center of the cell (row, column) in window coordinates (for tests).
    #[cfg(test)]
    pub fn cell_center(&self, row: usize, col: usize) -> Point<Pixels> {
        let (origin, cell_w) = self.grid;
        point(origin.x + cell_w * (col as f32 + 0.5), origin.y + self.line_h() * (row as f32 + 0.5))
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let line_h = self.line_h();
        let mode = *self.term.mode();
        let sensitivity = cx.global::<crate::settings::Settings>().terminal_scroll_sensitivity;
        let lines = self.wheel.lines(ev, line_h / px(1.), sensitivity, mode);
        cx.stop_propagation();
        if lines == 0 {
            return;
        }
        // The program wants the mouse: wheel buttons (64 up, 65 down).
        if mode.intersects(TermMode::MOUSE_MODE) {
            let button = if lines > 0 { 64 } else { 65 };
            for _ in 0..lines.unsigned_abs().min(10) {
                if let Some(bytes) = self.mouse_report(button, true, ev.position, &ev.modifiers) {
                    self.write(bytes);
                }
            }
            return;
        }
        // Full-screen program (less, man, git pager): the wheel becomes ↑/↓ (xterm "alternate scroll").
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let arrow: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.write(arrow.repeat(lines.unsigned_abs().min(10) as usize));
            return;
        }
        self.term.scroll_display(Scroll::Delta(lines));
        cx.notify();
    }

    fn clipboard_image(item: ClipboardItem) -> Option<gpui::Image> {
        item.into_entries().find_map(|entry| match entry {
            gpui::ClipboardEntry::Image(image) => Some(image),
            _ => None,
        })
    }

    fn paste_image(&mut self, image: gpui::Image, cx: &mut Context<Self>) {
        if self.closed.get() || self.exit.is_some() || self.error.is_some() || (self.pty.is_none() && !self.starting) {
            return;
        }
        let host = self.host.clone();
        let pty = self.pty;
        let task = cx.background_executor().spawn(async move { crate::terminal_image_paste::save_remote_image(host.as_ref(), image) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                // None -> Some is a normal startup transition. A different
                // existing PTY, or a closed/failed terminal, cannot accept this paste.
                if this.closed.get()
                    || this.exit.is_some()
                    || this.error.is_some()
                    || (pty.is_some() && this.pty != pty)
                    || (this.pty.is_none() && !this.starting)
                {
                    return;
                }
                match result {
                    Ok(path) => {
                        let bracketed = this.term.mode().contains(TermMode::BRACKETED_PASTE);
                        let data = crate::terminal_paste::prepare(&path, bracketed, crate::terminal_paste::Sanitize::Auto).data;
                        this.paste_confirmed(data);
                    }
                    Err(error) => cx.emit(TerminalEvent::ImagePasteFailed(error)),
                }
            })
            .ok();
        })
        .detach();
    }

    fn paste(&mut self, _: &TermPaste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else { return };
        // Text wins: a spreadsheet or a document puts a picture of the selection on the clipboard beside its text.
        if self.host.is_remote()
            && item.text().is_none_or(|t| t.trim().is_empty())
            && let Some(image) = Self::clipboard_image(item.clone())
        {
            self.paste_image(image, cx);
            return;
        }
        let Some(text) = item.text() else { return };
        let bracketed = self.term.mode().contains(TermMode::BRACKETED_PASTE);
        let settings = cx.global::<crate::settings::Settings>();
        let (policy, warn) = (settings.terminal_sanitize_paste, settings.terminal_warn_multiline);
        let p = crate::terminal_paste::prepare(&text, bracketed, policy);
        if crate::terminal_paste::needs_confirm(&p, bracketed, warn) {
            cx.emit(TerminalEvent::ConfirmPaste { data: p.data, lines: p.lines });
            return;
        }
        self.paste_confirmed(p.data);
    }

    /// Send a prepared paste.
    pub fn paste_confirmed(&mut self, data: Vec<u8>) {
        self.term.scroll_display(Scroll::Bottom);
        self.write(data);
    }

    fn copy(&mut self, _: &TermCopy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.term.selection_to_string() {
            cx.write_to_clipboard(ClipboardItem::new_string(s));
        }
    }

    fn clear(&mut self, _: &TermClear, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_all(cx);
    }

    /// Select the screen and the whole scrollback (then ⌘C copies everything).
    fn select_all(&mut self, _: &TermSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let top = GridPoint::new(self.term.topmost_line(), Column(0));
        let bottom = GridPoint::new(self.term.bottommost_line(), self.term.last_column());
        let mut selection = Selection::new(SelectionType::Simple, top, Side::Left);
        selection.update(bottom, Side::Right);
        self.term.selection = Some(selection);
        cx.notify();
    }

    fn scroll_page_up(&mut self, _: &TermScrollPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_by(Scroll::PageUp, cx);
    }

    fn scroll_page_down(&mut self, _: &TermScrollPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_by(Scroll::PageDown, cx);
    }

    fn scroll_to_top(&mut self, _: &TermScrollToTop, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_by(Scroll::Top, cx);
    }

    fn scroll_to_bottom(&mut self, _: &TermScrollToBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_by(Scroll::Bottom, cx);
    }

    /// Clear scrollback and ask the shell to redraw the screen (Ctrl+L).
    pub fn clear_all(&mut self, cx: &mut Context<Self>) {
        self.term.grid_mut().clear_history();
        self.forget_marks();
        self.write(b"\x0c".to_vec());
        cx.notify();
    }
}

/// macOS line editing, as in Terminal.app: ⌘← / ⌘→ go to the start / end of the line, ⌥← / ⌥→ move by word,
/// ⌘⌫ deletes to the start of the line, ⌘⌦ to its end, ⌥⌦ deletes the next word. They are the readline / zle keys
/// (Ctrl+A, Ctrl+E, Esc b, Esc f, Ctrl+U, Ctrl+K, Esc d), so they work in bash, zsh, fish and most REPLs. Full-screen programs
/// (vim, less) get the plain xterm keys instead: Ctrl+A there would mean something else.
pub fn macos_line_editing(k: &gpui::Keystroke, mode: TermMode) -> Option<&'static [u8]> {
    let m = &k.modifiers;
    if m.shift || m.control {
        return None;
    }
    // Full-screen programs get Home / End instead of readline's Ctrl+A / Ctrl+E.
    if mode.contains(TermMode::ALT_SCREEN) {
        let app = mode.contains(TermMode::APP_CURSOR);
        return match (k.key.as_str(), m.platform, m.alt) {
            ("left", true, false) => Some(if app { b"\x1bOH" } else { b"\x1b[H" }),
            ("right", true, false) => Some(if app { b"\x1bOF" } else { b"\x1b[F" }),
            _ => None,
        };
    }
    Some(match (k.key.as_str(), m.platform, m.alt) {
        ("left", true, false) => b"\x01",
        ("right", true, false) => b"\x05",
        ("backspace", true, false) => b"\x15",
        ("delete", true, false) => b"\x0b",
        ("left", false, true) => b"\x1bb",
        ("right", false, true) => b"\x1bf",
        ("delete", false, true) => b"\x1bd",
        _ => return None,
    })
}

/// Key → terminal bytes (xterm sequences). ⌘ keys are left to the app, except the macOS line editing keys.
pub fn key_to_bytes(k: &gpui::Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = &k.modifiers;
    if cfg!(target_os = "macos")
        && let Some(bytes) = macos_line_editing(k, mode)
    {
        return Some(bytes.to_vec());
    }
    if m.platform {
        return None;
    }
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    // xterm modifier code: 1 + shift + 2·alt + 4·ctrl
    let modcode = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let csi = |final_byte: char| -> Vec<u8> {
        if modcode > 1 {
            format!("\x1b[1;{modcode}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> { if modcode > 1 { format!("\x1b[{n};{modcode}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() } };
    let bytes = match k.key.as_str() {
        "enter" => b"\r".to_vec(),
        "backspace" if m.alt => b"\x1b\x7f".to_vec(),
        "backspace" if m.control => b"\x08".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "up" => csi('A'),
        "down" => csi('B'),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if m.control => vec![0],
        "space" => b" ".to_vec(),
        key if m.control && key.chars().count() == 1 => {
            let ch = key.chars().next()?.to_ascii_lowercase();
            match ch {
                'a'..='z' => vec![ch as u8 - b'a' + 1],
                '[' => vec![0x1b],
                '\\' => vec![0x1c],
                ']' => vec![0x1d],
                '/' | '_' | '-' => vec![0x1f],
                '2' | '@' => vec![0],
                _ => return None,
            }
        }
        _ => {
            // Typed character (ğ, ş... on a Turkish keyboard; includes ones produced with ⌥ on macOS).
            let typed = k.key_char.clone().or_else(|| (k.key.chars().count() == 1).then(|| k.key.clone()))?;
            // Alt+letter is the meta key of readline, emacs and vim: ESC first. Not when Alt composed another character (⌥ on
            // macOS, AltGr on a Turkish keyboard), which is typed as it is.
            let plain = k.key.chars().count() == 1 && typed.to_lowercase() == k.key;
            let mut bytes = typed.into_bytes();
            if m.alt && !m.control && !m.platform && plain {
                bytes.insert(0, 0x1b);
            }
            bytes
        }
    };
    Some(bytes)
}

// --- Drawing ---

struct Run {
    col: usize,
    /// The column after the text's last character.
    end: usize,
    text: String,
    fg: u32,
    bold: bool,
    italic: bool,
    underline: bool,
}

struct Frame {
    rows: Vec<Vec<Run>>,
    backgrounds: Vec<(usize, usize, usize, u32)>, // (row, start column, end, color)
    cursor: Option<(usize, usize, CursorShape)>,
    /// Link to underline: (row, start column, end).
    link: Option<(usize, usize, usize)>,
    /// Command marks on screen: (row, how it ended).
    marks: Vec<(usize, marks::MarkState)>,
    cell_w: Pixels,
    line_h: Pixels,
}

/// Font size and row height for a terminal font size: the row keeps the design scale's size-to-line ratio
/// (whole pixels, so rows don't drift apart while painting).
/// Default row height as a multiple of the font size. Tighter than the editor's 1.5: terminal output (tables, box drawing,
/// progress bars) reads as one block.
pub(crate) const LINE_HEIGHT: f32 = 1.2;

/// Font size and whole-pixel row height for `terminal.fontSize` and `terminal.lineHeight`.
pub(crate) fn metrics(font_size: f32, line_height: f32) -> (f32, f32) {
    let size = font_size.clamp(6., 48.);
    let factor = if line_height.is_finite() { line_height.clamp(1., 2.) } else { LINE_HEIGHT };
    (size, (size * factor).round().max(size.ceil()))
}

struct TerminalElement {
    view: Entity<TerminalView>,
    palette: Palette,
    focused: bool,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// Space between the panel's edges and the text grid. The left one also holds the command marks.
const PAD_X: f32 = 14.;
const PAD_Y: f32 = 8.;

fn grid_origin(bounds: Bounds<Pixels>) -> gpui::Point<Pixels> {
    point(bounds.left() + px(PAD_X), bounds.top() + px(PAD_Y))
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<Frame>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Frame> {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let probe = TextRun { len: 1, font: style.font(), color: c(self.palette.text).into(), background_color: None, underline: None, strikethrough: None };
        let cell_w = window.text_system().shape_line("0".into(), font_size, &[probe], None).width;
        let line_h = self.view.read(cx).line_h();
        let cols = ((bounds.size.width - px(PAD_X * 2.)) / cell_w).floor().max(2.) as usize;
        let rows = ((bounds.size.height - px(PAD_Y * 2.)) / line_h).floor().max(1.) as usize;
        let origin = grid_origin(bounds);
        self.view.update(cx, |v, _| {
            v.set_size(cols, rows);
            v.grid = (origin, cell_w);
        });

        let p = self.palette;
        let view = self.view.read(cx);
        let content = view.term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset as i32;
        let selection = content.selection;
        // Find bar open: every match on screen gets a tint (the current one is the selection).
        let found = view.find.as_ref().and_then(|f| f.regex()).map(|mut regex| find::visible_matches(&view.term, &mut regex, offset, rows)).unwrap_or_default();
        let current = view.find.as_ref().and_then(|f| f.current());
        let (found_bg, current_bg) = (mix(p.bg, p.warning, 0.22), mix(p.bg, p.warning, 0.45));
        let mut frame = Frame {
            rows: (0..rows).map(|_| Vec::new()).collect(),
            backgrounds: Vec::new(),
            cursor: None,
            link: view.hover_link.clone().map(|(r, c)| (r, c.start, c.end)),
            marks: view.visible_marks(rows),
            cell_w,
            line_h,
        };
        // A run starts with room for a typical stretch of text (never more than the rest of its row), not for a whole row:
        // a full-screen program's frame has dozens of short runs per row.
        const RUN_ROOM: usize = 48;
        let cols_hint = cols;
        for cell in content.display_iter {
            let row = (cell.point.line.0 + offset) as usize;
            let col = cell.point.column.0;
            if row >= rows || cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let (mut fg, mut bg) = (resolve(&p, cell.fg, colors), resolve(&p, cell.bg, colors));
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.flags.contains(Flags::DIM) {
                fg = mix(fg, p.bg, 0.4);
            }
            if cell.flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            if current.as_ref().is_some_and(|m| m.contains(&cell.point)) {
                bg = current_bg;
            } else if selection.is_some_and(|s| s.contains(cell.point)) {
                bg = p.selection;
            } else if found.iter().any(|m| m.contains(&cell.point)) {
                bg = found_bg;
            }
            if bg != p.bg {
                let end = if cell.flags.contains(Flags::WIDE_CHAR) { col + 2 } else { col + 1 };
                match frame.backgrounds.last_mut() {
                    Some((r, _, e, color)) if *r == row && *e == col && *color == bg => *e = end,
                    _ => frame.backgrounds.push((row, col, end, bg)),
                }
            }
            let ch = if cell.c == '\0' || cell.c == '\t' { ' ' } else { cell.c };
            let (bold, italic, underline) =
                (cell.flags.contains(Flags::BOLD), cell.flags.contains(Flags::ITALIC), cell.flags.intersects(Flags::ALL_UNDERLINES));
            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            let runs = &mut frame.rows[row];
            // A wide character gets its own run: grid alignment must not break.
            let extend = runs.last_mut().filter(|r| !wide && r.fg == fg && r.bold == bold && r.italic == italic && r.underline == underline && r.end == col);
            match extend {
                Some(r) => {
                    r.text.push(ch);
                    r.end += 1;
                }
                None => {
                    let mut text = String::with_capacity(cols_hint.saturating_sub(col).min(RUN_ROOM));
                    text.push(ch);
                    runs.push(Run { col, end: col + 1, text, fg, bold, italic, underline });
                }
            }
            if wide {
                runs.push(Run { col: col + 2, end: col + 2, text: String::new(), fg, bold, italic, underline });
            }
        }
        let cur = content.cursor;
        let cur_row = cur.point.line.0 + offset;
        if cur.shape != CursorShape::Hidden && cur_row >= 0 && (cur_row as usize) < rows && view.exit.is_none() {
            frame.cursor = Some((cur_row as usize, cur.point.column.0, cur.shape));
        }
        Some(frame)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        frame: &mut Option<Frame>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(mut frame) = frame.take() else { return };
        let p = self.palette;
        let line_h = frame.line_h;
        let origin = grid_origin(bounds);
        let cell = |row: usize, col: usize| point(origin.x + frame.cell_w * col as f32, origin.y + line_h * row as f32);
        for (row, a, b, color) in &frame.backgrounds {
            window.paint_quad(fill(Bounds::new(cell(*row, *a), size(frame.cell_w * (b - a) as f32, line_h)), c(*color)));
        }
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        for (row, runs) in std::mem::take(&mut frame.rows).into_iter().enumerate() {
            for run in runs.into_iter().filter(|r| !r.text.trim().is_empty()) {
                let mut font = style.font();
                if run.bold {
                    font.weight = FontWeight::BOLD;
                }
                if run.italic {
                    font.style = FontStyle::Italic;
                }
                let tr = TextRun {
                    len: run.text.len(),
                    font,
                    color: c(run.fg).into(),
                    background_color: None,
                    underline: run.underline.then(|| gpui::UnderlineStyle { color: Some(c(run.fg).into()), thickness: px(1.), wavy: false }),
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(run.text.into(), font_size, &[tr], None);
                let _ = shaped.paint(cell(row, run.col), line_h, window, cx);
            }
        }
        // Command marks in the left margin: green when it succeeded, red when it failed, grey while it runs.
        for (row, state) in &frame.marks {
            let color = match state {
                marks::MarkState::Succeeded => p.success,
                marks::MarkState::Failed => p.danger,
                marks::MarkState::Running => p.text_muted,
            };
            let d = px(5.);
            let at = point(bounds.left() + px((PAD_X - 5.) / 2.), origin.y + line_h * *row as f32 + (line_h - d) / 2.);
            window.paint_quad(fill(Bounds::new(at, size(d, d)), c(color)).corner_radii(d / 2.));
        }
        if let Some((row, a, b)) = frame.link {
            let at = cell(row, a);
            window.paint_quad(fill(Bounds::new(point(at.x, at.y + line_h - px(2.)), size(frame.cell_w * (b - a) as f32, px(1.))), c(p.accent)));
        }
        if let Some((row, col, shape)) = frame.cursor {
            let at = cell(row, col);
            let color = c(p.accent);
            let quad = match (shape, self.focused) {
                (_, false) | (CursorShape::HollowBlock, _) => gpui::outline(Bounds::new(at, size(frame.cell_w, line_h)), color, gpui::BorderStyle::Solid),
                (CursorShape::Beam, true) => fill(Bounds::new(at, size(px(2.), line_h)), color),
                (CursorShape::Underline, true) => fill(Bounds::new(point(at.x, at.y + line_h - px(2.)), size(frame.cell_w, px(2.))), color),
                _ => fill(Bounds::new(at, size(frame.cell_w, line_h)), crate::ui::ca(p.accent, 0.6)),
            };
            window.paint_quad(quad);
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        self.metrics = {
            let s = cx.global::<crate::settings::Settings>();
            metrics(s.terminal_font_size, s.terminal_line_height)
        };
        self.allow_clear_history = cx.global::<crate::settings::Settings>().terminal_allow_clear_history;
        // A changed `terminal.scrollback` applies to open terminals too.
        let scrollback = cx.global::<crate::settings::Settings>().terminal_scrollback.min(SCROLLBACK_MAX);
        if scrollback != self.scrollback {
            self.scrollback = scrollback;
            self.term.set_options(Config { scrolling_history: scrollback, ..Config::default() });
        }
        let focused = self.focus_handle.is_focused(window);
        let find_focused = self.find_has_focus(window, cx);
        let find_bar = self.find_bar(&p, find_focused, cx);
        let exit_line = self.exit.map(|code| match code {
            Some(c) => crate::i18n::tf("Process exited with code {}. Press any key to close.", &[&c]),
            None => crate::i18n::t("Process ended. Press any key to close.").to_string(),
        });
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::reset))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_previous))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .on_action(cx.listener(Self::scroll_to_top))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_action(cx.listener(|this, _: &TermPrevCommand, _, cx| {
                if this.jump_to_command(true) {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &TermNextCommand, _, cx| {
                if this.jump_to_command(false) {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &TermCopyLastOutput, _, cx| {
                if let Some(text) = this.last_output() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            // Middle click pastes; a program that reads the mouse gets it instead (Shift or ⌥ pastes anyway).
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle);
                    if selects_text(&ev.modifiers) || !this.report_other_button(MouseButton::Middle, true, ev.position, &ev.modifiers) {
                        this.paste(&TermPaste, window, cx);
                    }
                }),
            )
            .on_drop(cx.listener(|this, d: &gpui::ExternalPaths, _, cx| this.drop_paths(d.paths(), cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    // Ours by default, also while a program reads the mouse (Claude Code, htop, vim with mouse=a, over SSH too):
                    // otherwise there is no way to copy or paste with the mouse in those programs. `terminal.rightClick:
                    // "program"` gives it to such a program instead, and Shift or ⌥ still opens the menu.
                    window.focus(&this.focus_handle);
                    let to_program = cx.global::<crate::settings::Settings>().terminal_right_click == "program" && !selects_text(&ev.modifiers);
                    if !(to_program && this.report_other_button(MouseButton::Right, true, ev.position, &ev.modifiers)) {
                        cx.emit(TerminalEvent::ContextMenu(ev.position));
                    }
                }),
            )
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .when(self.hover_link.is_some(), |d| d.cursor(gpui::CursorStyle::PointingHand))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.hover_uri.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, ev, _, cx| this.on_mouse_up(ev, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, ev, _, cx| this.on_mouse_up(ev, cx)))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_other_button_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::on_other_button_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_other_button_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_other_button_up))
            .relative()
            .size_full()
            .bg(c(p.bg))
            .font_family(MONO_FONT)
            .text_size(px(self.metrics.0))
            .line_height(px(self.metrics.1))
            .text_color(c(p.text))
            .child(TerminalElement { view: cx.entity(), palette: p, focused })
            .children(find_bar)
            // Scrolled back into the history: a button to get back to the live screen.
            .when(self.term.grid().display_offset() > 0 && !self.term.mode().contains(TermMode::ALT_SCREEN), |d| {
                d.child(
                    div()
                        .id("terminal-to-bottom")
                        .absolute()
                        .bottom(px(theme::SPACE[3]))
                        .right(px(theme::SPACE[5]))
                        .px(px(theme::SPACE[3]))
                        .py(px(theme::SPACE[1]))
                        .rounded(px(theme::radius::SM))
                        .bg(c(p.surface_raised))
                        .text_color(c(p.text))
                        .text_size(px(crate::ui::text::XS.0))
                        .cursor(gpui::CursorStyle::PointingHand)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.scroll_by(Scroll::Bottom, cx);
                            }),
                        )
                        .child(crate::i18n::t("Jump to Bottom")),
                )
            })
            .when_some(self.hover_uri.clone(), |d, uri| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(theme::SPACE[3]))
                        .left(px(theme::SPACE[5]))
                        .max_w_full()
                        .px(px(theme::SPACE[3]))
                        .py(px(theme::SPACE[1]))
                        .rounded(px(theme::radius::SM))
                        .bg(c(p.surface_raised))
                        .text_color(c(p.text))
                        .text_size(px(crate::ui::text::XS.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(uri),
                )
            })
            .when(self.shift_hint, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(theme::SPACE[2]))
                        .right(px(theme::SPACE[4]))
                        .px(px(theme::SPACE[3]))
                        .py(px(theme::SPACE[1]))
                        .rounded(px(theme::radius::SM))
                        .bg(c(p.surface_raised))
                        .text_color(c(p.text))
                        .text_size(px(crate::ui::text::XS.0))
                        .child(crate::i18n::t("Hold Shift or ⌥ to select text")),
                )
            })
            .when_some(self.error.clone().or(exit_line.map(Into::into)), |d, msg| {
                d.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .px(px(theme::SPACE[5]))
                        .py(px(theme::SPACE[2]))
                        .bg(c(p.surface_raised))
                        .text_color(c(p.text_muted))
                        .child(msg),
                )
            })
    }
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
