//! Integrated terminal (S08): VT parsing with alacritty_terminal, PTY on the agent
//! (local or remote, same protocol). Colors are derived from the theme.

use std::cell::RefCell;
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

#[path = "terminal_colors.rs"]
mod colors;
#[path = "terminal_find.rs"]
mod find;
#[path = "terminal_marks.rs"]
mod marks;

pub use marks::{Activity, Finished};

#[path = "terminal_scroll.rs"]
mod scroll;

#[cfg(test)]
#[path = "terminal_scroll_tests.rs"]
mod scroll_tests;

pub enum TerminalEvent {
    TitleChanged,
    /// A command started or finished (shell integration): the terminal list shows it.
    ActivityChanged,
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
}

/// Most scrollback lines (`terminal.scrollback`): 50,000 lines of 200 columns already take about 200 MB, times up to
/// eight terminals.
pub const SCROLLBACK_MAX: usize = 50_000;

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
    /// Input sent before the shell started (task command): written once the pty is up.
    pending_input: Vec<u8>,
    /// Fixed title (task name): the shell's title updates don't change it.
    title_locked: bool,
    /// Program and arguments it was started with (`None`: the user's shell), kept so a restart can start the same.
    launched: (Option<String>, Option<Vec<String>>),
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
    /// Link hovered while ⌘ is held: (screen row, columns).
    hover_link: Option<(usize, std::ops::Range<usize>)>,
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
    /// What a restart needs to bring this terminal back as a new shell (`history_lines` 0: no history). Output-only
    /// terminals (task and test output) have no shell and are not saved.
    pub fn saved(&self, history_lines: usize) -> Option<crate::terminal_session::SavedTerminal> {
        self.pty.as_ref()?;
        let history = if history_lines > 0 { crate::terminal_session::sanitize_history(&self.scrollback_text(), history_lines) } else { None };
        Some(crate::terminal_session::SavedTerminal {
            cwd: self.cwd.clone(),
            shell: self.launched.0.clone(),
            args: self.launched.1.clone(),
            color: self.color.clone(),
            title: self.title_locked.then(|| self.title.to_string()),
            history,
        })
    }

    /// Text shown in the terminal as output (a restored history, a notice): never sent to the program.
    pub fn show_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.append(text.as_bytes(), cx);
    }

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
        let Launch { shell, args, env, color } = launch;
        let mut view = Self::blank(host.clone(), cwd.clone(), color, cx);
        view.launched = (shell.clone(), args.clone());
        let (cols, rows) = (view.size.cols as u16, view.size.rows as u16);
        let integration = cx.global::<crate::settings::Settings>().terminal_shell_integration;
        let spawn = cx.background_executor().spawn(async move {
            match (&args, env.is_empty(), integration) {
                (None, true, false) => host.pty_spawn(cols, rows, cwd, shell),
                _ => host.pty_spawn_ex(host::PtySpec { cols, rows, cwd, shell, args, env, integration }),
            }
        });
        cx.spawn(async move |this, cx| {
            let pty = match spawn.await {
                Ok(p) => p,
                Err(e) => {
                    this.update(cx, |t, cx| {
                        t.error = Some(crate::i18n::tf("Could not start a shell: {}", &[&e]).into());
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let id = pty.id;
            let ok = this
                .update(cx, |t, cx| {
                    t.pty = Some(id);
                    // Report it if the view got its size in the meantime.
                    t.host.pty_resize(id, t.size.cols as u16, t.size.rows as u16);
                    let pending = std::mem::take(&mut t.pending_input);
                    if !pending.is_empty() {
                        t.host.pty_input(id, pending);
                    }
                    cx.notify();
                })
                .is_ok();
            if !ok {
                return;
            }
            // Output pump: wait, then parse whatever piled up in one go (no frame per chunk).
            use futures::StreamExt;
            // If the UI lags (e.g. `yes` output), pending output must not grow without bound: whatever exceeds 32 MB is dropped and a note
            // is left (the terminal's scrollback is only 2,000 lines anyway). The exit event is never dropped.
            let backlog = Arc::new(OutputBacklog::default());
            let gate = backlog.clone();
            let mut events = crate::ui::bridge_with(pty.events, move |ev| match ev {
                PtyEvent::Output(d) => gate.admit(d.len()),
                PtyEvent::Exit(_) => true,
            });
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                    if batch.len() > 256 {
                        break;
                    }
                }
                let mut exited = false;
                let alive = this
                    .update(cx, |t, cx| {
                        for ev in batch {
                            match ev {
                                PtyEvent::Output(data) => {
                                    backlog.consumed(data.len());
                                    t.feed(&data);
                                    if backlog.take_dropped() {
                                        t.feed(b"\r\n\x1b[2m[output was dropped: the terminal could not keep up]\x1b[0m\r\n");
                                    }
                                }
                                PtyEvent::Exit(code) => {
                                    t.on_exit(code, cx);
                                    exited = true;
                                }
                            }
                        }
                        t.drain_events(cx);
                        cx.notify();
                    })
                    .is_ok();
                if !alive || exited {
                    return;
                }
            }
            this.update(cx, |t, cx| t.on_exit(None, cx)).ok();
        })
        .detach();
        view
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
        self.feed(b"\x1b[3J\x1b[H\x1b[2J");
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
            pending_input: Vec::new(),
            title_locked: false,
            launched: (None, None),
            term,
            parser: Processor::new(),
            events,
            exit: None,
            error: None,
            size,
            grid: (Point::default(), px(8.)),
            selecting: false,
            find: None,
            hover_link: None,
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

    fn feed(&mut self, data: &[u8]) {
        // Each shell-integration sequence is noted where it ends, with the cursor where the parser left it.
        let mut at = 0;
        for (end, ev) in self.osc.scan(data) {
            self.parser.advance(&mut self.term, &data[at..end]);
            at = end;
            self.on_osc(ev);
        }
        self.parser.advance(&mut self.term, &data[at..]);
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
                TermEvent::Title(t) => {
                    self.title = t.into();
                    cx.emit(TerminalEvent::TitleChanged);
                }
                TermEvent::ResetTitle => {
                    self.title = "terminal".into();
                    cx.emit(TerminalEvent::TitleChanged);
                }
                // OSC 52: only when the setting allows it (a program could otherwise replace what the user copied).
                TermEvent::ClipboardStore(..) => {
                    let allowed = cx.global::<crate::settings::Settings>().terminal_allow_clipboard_write;
                    if let Some(s) = clipboard_text(ev, allowed) {
                        cx.write_to_clipboard(ClipboardItem::new_string(s))
                    }
                }
                _ => {}
            }
        }
    }

    fn on_exit(&mut self, code: Option<i32>, cx: &mut Context<Self>) {
        if self.exit.is_none() {
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

    fn write(&self, data: Vec<u8>) {
        if let (Some(id), false) = (self.pty, self.exit.is_some()) {
            self.host.pty_input(id, data);
        }
    }

    /// Send text to the shell (once it starts, if it hasn't yet).
    pub fn send(&mut self, text: &str) {
        match self.pty {
            Some(_) => self.write(text.as_bytes().to_vec()),
            None => self.pending_input.extend_from_slice(text.as_bytes()),
        }
    }

    pub fn set_fixed_title(&mut self, title: String) {
        self.title = title.into();
        self.title_locked = true;
    }

    /// Close the shell (when the tab is closed).
    pub fn kill(&self) {
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

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Typing in the find bar: those keys bubble up here but are not for the shell.
        if self.find_has_focus(window, cx) {
            return;
        }
        if self.exit.is_some() || self.error.is_some() {
            if !ev.keystroke.modifiers.platform {
                cx.stop_propagation();
                cx.emit(TerminalEvent::CloseRequested);
            }
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
        // A remote program cannot read the local image clipboard itself.
        // Preserve Ctrl+V for programs when there is no image on the clipboard.
        if k.key == "v"
            && k.modifiers.control
            && !k.modifiers.shift
            && !k.modifiers.alt
            && !k.modifiers.platform
            && self.host.is_remote()
            && let Some(image) = cx.read_from_clipboard().and_then(Self::clipboard_image)
        {
            cx.stop_propagation();
            self.paste_image(image, cx);
            return;
        }
        let Some(bytes) = key_to_bytes(&ev.keystroke, *self.term.mode()) else { return };
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

    /// xterm sequence if the program wants the mouse (htop, vim `mouse=a`, tmux): SGR or the legacy
    /// format. `button`: 0 left, 64/65 wheel up/down. `None` if it doesn't.
    fn mouse_report(&self, button: u8, pressed: bool, pos: Point<Pixels>, mods: &gpui::Modifiers) -> Option<Vec<u8>> {
        let mode = *self.term.mode();
        if !mode.intersects(TermMode::MOUSE_MODE) {
            return None;
        }
        let (origin, cell_w) = self.grid;
        let line_h = self.line_h();
        let col = (((pos.x - origin.x) / cell_w).max(0.).floor() as usize).min(self.size.cols.saturating_sub(1)) + 1;
        let row = ((pos.y - origin.y) / line_h).max(0.).floor().min((self.size.rows - 1) as f32) as usize + 1;
        let mut b = button;
        if mods.shift {
            b += 4;
        }
        if mods.alt {
            b += 8;
        }
        if mods.control {
            b += 16;
        }
        Some(if mode.contains(TermMode::SGR_MOUSE) {
            format!("\x1b[<{b};{col};{row}{}", if pressed { 'M' } else { 'm' }).into_bytes()
        } else {
            // Legacy format: release is "3", coordinates are bytes offset by 32 (at most 223).
            let b = if pressed { b } else { 3 };
            vec![0x1b, b'[', b'M', 32 + b, (32 + col.min(223)) as u8, (32 + row.min(223)) as u8]
        })
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
        if !ev.modifiers.shift
            && let Some(bytes) = self.mouse_report(0, true, ev.position, &ev.modifiers)
        {
            self.write(bytes);
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
        if !self.selecting || ev.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            return;
        }
        let (at, side) = self.grid_point(ev.position);
        if let Some(sel) = &mut self.term.selection {
            sel.update(at, side);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, ev: &gpui::MouseUpEvent) {
        if !ev.modifiers.shift
            && !self.selecting
            && let Some(bytes) = self.mouse_report(0, false, ev.position, &ev.modifiers)
        {
            self.write(bytes);
            return;
        }
        self.selecting = false;
        // Single click without dragging: don't leave an empty selection.
        if self.term.selection.as_ref().is_some_and(|s| s.is_empty()) {
            self.term.selection = None;
        }
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
        let host = self.host.clone();
        let pty = self.pty;
        let task = cx.background_executor().spawn(async move { crate::terminal_image_paste::save_remote_image(host.as_ref(), image) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.pty != pty || this.exit.is_some() {
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
        if self.host.is_remote()
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
    if mode.contains(TermMode::ALT_SCREEN) || m.shift || m.control {
        return None;
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
            typed.into_bytes()
        }
    };
    Some(bytes)
}

// --- Drawing ---

struct Run {
    col: usize,
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
            let extend = runs
                .last_mut()
                .filter(|r| !wide && r.fg == fg && r.bold == bold && r.italic == italic && r.underline == underline && r.col + r.text.chars().count() == col);
            match extend {
                Some(r) => r.text.push(ch),
                None => runs.push(Run { col, text: ch.to_string(), fg, bold, italic, underline }),
            }
            if wide {
                runs.push(Run { col: col + 2, text: String::new(), fg, bold, italic, underline });
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
        let Some(frame) = frame.take() else { return };
        let p = self.palette;
        let line_h = frame.line_h;
        let origin = grid_origin(bounds);
        let cell = |row: usize, col: usize| point(origin.x + frame.cell_w * col as f32, origin.y + line_h * row as f32);
        for (row, a, b, color) in &frame.backgrounds {
            window.paint_quad(fill(Bounds::new(cell(*row, *a), size(frame.cell_w * (b - a) as f32, line_h)), c(*color)));
        }
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        for (row, runs) in frame.rows.iter().enumerate() {
            for run in runs.iter().filter(|r| !r.text.trim().is_empty()) {
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
                let shaped = window.text_system().shape_line(run.text.clone().into(), font_size, &[tr], None);
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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    // A program that reads the mouse (htop, vim with mouse=a) keeps the right button; Shift still opens the menu.
                    if this.term.mode().intersects(TermMode::MOUSE_MODE) && !ev.modifiers.shift {
                        return;
                    }
                    window.focus(&this.focus_handle);
                    cx.emit(TerminalEvent::ContextMenu(ev.position));
                }),
            )
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .when(self.hover_link.is_some(), |d| d.cursor(gpui::CursorStyle::PointingHand))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, ev, _, _| this.on_mouse_up(ev)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, ev, _, _| this.on_mouse_up(ev)))
            .relative()
            .size_full()
            .bg(c(p.bg))
            .font_family(MONO_FONT)
            .text_size(px(self.metrics.0))
            .line_height(px(self.metrics.1))
            .text_color(c(p.text))
            .child(TerminalElement { view: cx.entity(), palette: p, focused })
            .children(find_bar)
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
mod tests {
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
    fn osc_52_reaches_the_clipboard_only_when_the_setting_allows_it() {
        let events = Listener::default();
        let mut term = Term::new(Config::default(), &TermSize { cols: 80, rows: 24 }, events.clone());
        let mut parser = Processor::<StdSyncHandler>::default();
        parser.advance(&mut term, b"\x1b]52;c;Zm9v\x07");
        let queued = std::mem::take(&mut *events.0.borrow_mut());
        assert!(queued.iter().any(|e| matches!(e, TermEvent::ClipboardStore(..))), "the terminal reports the store request");
        let texts = |allowed| queued.iter().cloned().filter_map(|e| clipboard_text(e, allowed)).collect::<Vec<_>>();
        assert!(texts(false).is_empty(), "off by default: nothing is written");
        assert_eq!(texts(true), ["foo"]);
        assert!(!crate::settings::Settings::default().terminal_allow_clipboard_write);
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
        // vim, less: plain xterm keys (Ctrl+A would increment a number in vim).
        assert_eq!(mac("cmd-left", TermMode::ALT_SCREEN), None);
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
}
