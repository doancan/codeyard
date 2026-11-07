//! The mouse while a program reads it (Claude Code, Codex, tmux, vim `mouse=a`, htop): presses, releases, drags and moves
//! go to the program in the xterm format it asked for, and the program may then put what it selected on the clipboard.
//!
//! Programs that select text themselves (Claude Code's fullscreen view, tmux, vim) follow a drag through motion reports
//! (`?1002h`, `?1003h`) and copy when the button is let go. Over SSH they cannot reach this computer's clipboard and send
//! OSC 52 instead. `terminal.allowClipboardWrite: "copy"` takes such a write only right after a copy gesture in that terminal
//! (a drag or a double-click let go in the program, or Ctrl+C), so output that carries OSC 52 on its own, such as a file
//! printed with `cat`, still cannot replace what is on the clipboard.

use std::time::{Duration, Instant};

use alacritty_terminal::term::TermMode;
use gpui::{Modifiers, MouseButton, MouseMoveEvent, Pixels, Point};

use super::TerminalView;

/// How long after a copy gesture a program's clipboard write is taken: over SSH it comes back after a round trip.
const COPY_WINDOW: Duration = Duration::from_secs(5);

/// The left button as the program sees it.
#[derive(Default)]
pub(super) struct ProgramMouse {
    /// The cell and click count of the press the program got: moves are drags until the release.
    press: Option<((usize, usize), usize)>,
    /// The cell the last report was for: a move inside the same cell is not reported again.
    last: Option<(usize, usize)>,
    /// When the user last let go of a drag or a double-click in the program, or pressed Ctrl+C in it.
    copy_gesture: Option<Instant>,
}

impl ProgramMouse {
    pub(super) fn note_copy_key(&mut self) {
        self.copy_gesture = Some(Instant::now());
    }
}

/// Shift or ⌥ held: the drag selects text even while the program reads the mouse (Terminal.app and iTerm use ⌥ for this).
pub(super) fn selects_text(m: &Modifiers) -> bool {
    m.shift || m.alt
}

/// Whether a program's clipboard write (OSC 52) is taken under `terminal.allowClipboardWrite`.
pub(super) fn clipboard_write_allowed(setting: &str, copy_gesture: Option<Instant>, now: Instant) -> bool {
    match setting {
        "always" => true,
        "copy" => copy_gesture.is_some_and(|t| now.saturating_duration_since(t) <= COPY_WINDOW),
        _ => false,
    }
}

impl TerminalView {
    /// The 1-based cell under `pos`, held inside the grid (a drag may leave it).
    fn mouse_cell(&self, pos: Point<Pixels>) -> (usize, usize) {
        let (origin, cell_w) = self.grid;
        let line_h = self.line_h();
        let col = (((pos.x - origin.x) / cell_w).max(0.).floor() as usize).min(self.size.cols.saturating_sub(1)) + 1;
        let row = ((pos.y - origin.y) / line_h).max(0.).floor().min((self.size.rows - 1) as f32) as usize + 1;
        (col, row)
    }

    /// One xterm report: SGR or the legacy format. `button`: 0 left, 3 none, +32 for a move, 64/65 wheel up/down.
    fn mouse_bytes(&self, button: u8, pressed: bool, (col, row): (usize, usize), mods: &Modifiers) -> Vec<u8> {
        let b = button + if mods.shift { 4 } else { 0 } + if mods.alt { 8 } else { 0 } + if mods.control { 16 } else { 0 };
        if self.term.mode().contains(TermMode::SGR_MOUSE) {
            format!("\x1b[<{b};{col};{row}{}", if pressed { 'M' } else { 'm' }).into_bytes()
        } else {
            // Legacy format: release is "3", coordinates are bytes offset by 32 (at most 223).
            let b = if pressed { b } else { 3 };
            vec![0x1b, b'[', b'M', 32 + b, (32 + col.min(223)) as u8, (32 + row.min(223)) as u8]
        }
    }

    /// The report for `button` if the program wants the mouse (htop, vim `mouse=a`, tmux, Claude Code). `None` if it doesn't.
    pub(super) fn mouse_report(&self, button: u8, pressed: bool, pos: Point<Pixels>, mods: &Modifiers) -> Option<Vec<u8>> {
        self.term.mode().intersects(TermMode::MOUSE_MODE).then(|| self.mouse_bytes(button, pressed, self.mouse_cell(pos), mods))
    }

    /// The left button went down: it goes to the program if it reads the mouse.
    pub(super) fn report_press(&mut self, pos: Point<Pixels>, mods: &Modifiers, click_count: usize) -> bool {
        let Some(bytes) = self.mouse_report(0, true, pos, mods) else { return false };
        let cell = self.mouse_cell(pos);
        self.program_mouse.press = Some((cell, click_count));
        self.program_mouse.last = Some(cell);
        self.write(bytes);
        true
    }

    /// The left button came up. A drag or a double-click let go is a copy the program may make (OSC 52).
    pub(super) fn report_release(&mut self, pos: Point<Pixels>, mods: &Modifiers) -> bool {
        let press = self.program_mouse.press.take();
        self.program_mouse.last = None;
        let Some(bytes) = self.mouse_report(0, false, pos, mods) else { return false };
        if press.is_some_and(|(at, clicks)| at != self.mouse_cell(pos) || clicks >= 2) {
            self.program_mouse.copy_gesture = Some(Instant::now());
        }
        self.write(bytes);
        true
    }

    /// A move the program asked for: a drag while its button is held (`?1002h` or `?1003h`), any move with `?1003h`.
    /// `false` when the program does not follow it (it only reads clicks, or nothing).
    pub(super) fn report_motion(&mut self, ev: &MouseMoveEvent) -> bool {
        let mode = *self.term.mode();
        let held = self.program_mouse.press.is_some() && ev.pressed_button == Some(MouseButton::Left);
        if ev.pressed_button.is_none() {
            self.program_mouse.press = None;
        }
        let wanted = if held {
            mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
        } else {
            ev.pressed_button.is_none() && mode.contains(TermMode::MOUSE_MOTION)
        };
        if !wanted {
            return false;
        }
        let cell = self.mouse_cell(ev.position);
        if self.program_mouse.last != Some(cell) {
            self.program_mouse.last = Some(cell);
            // A move adds 32 to the button: 0 (left) while dragging, 3 (none) otherwise.
            let bytes = self.mouse_bytes(if held { 32 } else { 35 }, true, cell, &ev.modifiers);
            self.write(bytes);
        }
        true
    }

    /// A program's clipboard write (OSC 52) is taken now, under `terminal.allowClipboardWrite`.
    pub(super) fn takes_clipboard_write(&self, setting: &str) -> bool {
        clipboard_write_allowed(setting, self.program_mouse.copy_gesture, Instant::now())
    }
}
