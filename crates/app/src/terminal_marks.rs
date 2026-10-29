//! Command marks from shell integration (OSC 133) and the shell's folder (OSC 7): where each prompt and command output
//! starts, how the command ended, and what the terminal does with that (gutter dots, jumping between commands, copying
//! the last output, opening a split where the shell is).
//!
//! Marks are kept as absolute lines (history lines plus the screen line). While the history is not full, a line's
//! absolute number never changes. Once it is full, old lines fall off the top and every line moves up; the shift is
//! found again from the newest command's own text, so marks stay on their lines without hooking into the parser.

use std::collections::VecDeque;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::term::cell::Flags;

use super::TerminalView;
use crate::terminal_osc::OscEvent;

/// Most commands remembered per terminal.
const MAX_MARKS: usize = 1000;
/// How far (lines) the shift is looked for when the history is full.
const SHIFT_SEARCH: i64 = 5000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CmdMark {
    /// Line of the prompt (OSC 133;A).
    pub prompt: i64,
    /// First line of the command's output (OSC 133;C), and the text of the line above it (the command as typed).
    pub output: Option<(i64, String)>,
    /// Line after the output and the exit code (OSC 133;D).
    pub end: Option<(i64, Option<i32>)>,
}

#[derive(Debug, Default)]
pub(super) struct Marks {
    pub list: VecDeque<CmdMark>,
    /// Lines that fell off the top since the history filled (subtracted from recorded lines).
    pub shift: i64,
    /// Folder from OSC 7.
    pub cwd: Option<String>,
    /// When the running command started (OSC 133;C).
    pub started: Option<std::time::Instant>,
    /// A long command finished; cleared once the terminal is looked at.
    pub unseen: Option<Finished>,
    /// A command started or ended since the app was last told.
    pub activity_changed: bool,
    /// A program asked for the user (bell, OSC 9/777) since the app was last told: the message, if it sent one.
    pub attention: Option<Option<String>>,
    /// A command that ran for `NOTIFY_RUN` or longer ended since the app was last told: the line it was typed on, its code.
    pub long_finish: Option<(String, Option<i32>)>,
}

/// How a command ended, for its gutter dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkState {
    Running,
    Succeeded,
    Failed,
}

/// Commands that ran at least this long tell the terminal list when they finish out of sight.
const NOTABLE_RUN: std::time::Duration = std::time::Duration::from_secs(3);
/// Commands that ran at least this long also say so in a notice and on the taskbar when nobody watched them end.
const NOTIFY_RUN: std::time::Duration = std::time::Duration::from_secs(10);

/// What a terminal is doing, for its row in the terminal list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    /// At the prompt (or without shell integration).
    Idle,
    /// A command runs: the line it was typed on.
    Running(String),
    /// The last command failed with this exit code.
    Failed(i32),
}

/// A long command that finished while nobody looked at its terminal, or a program that asked for the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finished {
    Succeeded,
    Failed,
    /// The bell, or a notification (OSC 9/777): Claude Code or Codex waiting for an answer.
    Attention,
}

impl TerminalView {
    fn history(&self) -> i64 {
        self.term.grid().history_size() as i64
    }

    /// Absolute line of the cursor.
    fn cursor_abs(&self) -> i64 {
        self.history() + self.term.grid().cursor.point.line.0 as i64
    }

    /// Text of an absolute line (as recorded, before the shift), if it is still in the terminal.
    fn line_text(&self, abs: i64) -> Option<String> {
        let line = abs - self.history();
        if line < -self.history() || line >= self.term.screen_lines() as i64 {
            return None;
        }
        let row = &self.term.grid()[Line(line as i32)];
        let text: String = (0..self.term.columns()).map(|c| &row[Column(c)]).filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER)).map(|c| c.c).collect();
        Some(text.trim_end().to_string())
    }

    pub(super) fn on_osc(&mut self, ev: OscEvent) {
        let at = self.cursor_abs() + self.marks.shift;
        match ev {
            OscEvent::Cwd(path) => self.marks.cwd = Some(path),
            OscEvent::Notify { title, body } => {
                let text = match title {
                    Some(title) if !body.is_empty() => format!("{title}: {body}"),
                    Some(title) => title,
                    None => body,
                };
                self.ask_attention(Some(text));
            }
            OscEvent::PromptStart => {
                self.marks.list.push_back(CmdMark { prompt: at, ..CmdMark::default() });
                if self.marks.list.len() > MAX_MARKS {
                    self.marks.list.pop_front();
                }
            }
            OscEvent::CommandStart => {
                let typed = self.line_text(self.cursor_abs() - 1).unwrap_or_default();
                if let Some(m) = self.marks.list.back_mut().filter(|m| m.output.is_none()) {
                    m.output = Some((at, typed));
                    self.marks.started = Some(std::time::Instant::now());
                    self.marks.activity_changed = true;
                }
            }
            // Only a command seen starting has an end (bash reports one after every prompt).
            OscEvent::CommandEnd(code) => {
                if let Some(m) = self.marks.list.back_mut().filter(|m| m.output.is_some() && m.end.is_none()) {
                    m.end = Some((at, code));
                    self.marks.activity_changed = true;
                    let typed = m.output.as_ref().map(|(_, typed)| typed.trim().to_string()).unwrap_or_default();
                    let ran = self.marks.started.take().map(|t| t.elapsed()).unwrap_or_default();
                    if ran >= NOTABLE_RUN && self.marks.unseen != Some(Finished::Attention) {
                        self.marks.unseen = Some(if matches!(code, Some(c) if c != 0) { Finished::Failed } else { Finished::Succeeded });
                    }
                    if ran >= NOTIFY_RUN {
                        self.marks.long_finish = Some((typed, code));
                    }
                }
            }
        }
    }

    /// Re-finds the shift once the history is full: the newest command's line is looked for where it should be and
    /// above it. Marks that scrolled out are dropped.
    pub(super) fn update_mark_shift(&mut self) {
        let full = self.history() as usize >= self.scrollback && self.scrollback > 0;
        if !full || self.marks.list.is_empty() {
            return;
        }
        let Some((out, typed)) = self.marks.list.iter().rev().find_map(|m| m.output.clone()) else { return };
        if typed.is_empty() {
            return;
        }
        let expected = out - 1 - self.marks.shift;
        if self.line_text(expected).as_deref() != Some(typed.as_str())
            && let Some(k) = (1..=SHIFT_SEARCH).find(|k| self.line_text(expected - k).as_deref() == Some(typed.as_str()))
        {
            self.marks.shift += k;
        }
        let top = self.marks.shift;
        while self.marks.list.front().is_some_and(|m| m.prompt < top) {
            self.marks.list.pop_front();
        }
    }

    /// Prompts on screen: (screen row, state) for the gutter.
    pub(super) fn visible_marks(&self, rows: usize) -> Vec<(usize, MarkState)> {
        let offset = self.term.grid().display_offset() as i64;
        let base = self.history() + self.marks.shift - offset;
        self.marks
            .list
            .iter()
            .filter_map(|m| {
                let row = m.prompt - base;
                let state = match (&m.output, m.end) {
                    (None, _) => return None,
                    (Some(_), None) => MarkState::Running,
                    (Some(_), Some((_, Some(0)))) => MarkState::Succeeded,
                    (Some(_), Some((_, None))) => MarkState::Succeeded,
                    (Some(_), Some(_)) => MarkState::Failed,
                };
                (0..rows as i64).contains(&row).then_some((row as usize, state))
            })
            .collect()
    }

    /// Scroll so the previous (or next) command's prompt is at the top. Returns whether there was one.
    pub(super) fn jump_to_command(&mut self, previous: bool) -> bool {
        let offset = self.term.grid().display_offset() as i64;
        let top = self.history() - offset + self.marks.shift;
        let prompts = self.marks.list.iter().filter(|m| m.output.is_some()).map(|m| m.prompt);
        let target = if previous { prompts.filter(|p| *p < top).max() } else { prompts.filter(|p| *p > top).min() };
        let Some(target) = target else { return false };
        let want = (self.history() + self.marks.shift - target).clamp(0, self.history());
        self.term.scroll_display(alacritty_terminal::grid::Scroll::Delta((want - offset) as i32));
        true
    }

    /// Output of the last command that finished.
    pub(super) fn last_output(&self) -> Option<String> {
        let m = self.marks.list.iter().rev().find(|m| m.end.is_some() && m.output.is_some())?;
        let (start, _) = m.output.as_ref()?;
        let (end, _) = m.end?;
        let (first, last) = (start - self.marks.shift - self.history(), end - 1 - self.marks.shift - self.history());
        // A program can clear the history or reset the terminal after the command ran, so the recorded lines may no longer
        // exist: only what is still inside the grid is read.
        let last = last.min(self.term.screen_lines() as i64 - 1);
        if last < first || first < -self.history() {
            return Some(String::new());
        }
        let from = GridPoint::new(Line(first as i32), Column(0));
        let to = GridPoint::new(Line(last as i32), self.term.last_column());
        Some(self.term.bounds_to_string(from, to).trim_end().to_string())
    }

    /// What the terminal is doing now: the newest command decides (a prompt with nothing typed yet keeps the one before).
    pub fn activity(&self) -> Activity {
        let Some(m) = self.marks.list.iter().rev().find(|m| m.output.is_some()) else { return Activity::Idle };
        match (&m.output, m.end) {
            (Some((_, typed)), None) => Activity::Running(typed.trim().to_string()),
            (_, Some((_, Some(code)))) if code != 0 => Activity::Failed(code),
            _ => Activity::Idle,
        }
    }

    /// A program asked for the user: the terminal list flags it until it is looked at. A later finish does not hide it.
    pub(super) fn ask_attention(&mut self, message: Option<String>) {
        self.marks.unseen = Some(Finished::Attention);
        // A bell after a notification keeps the notification's message.
        match &mut self.marks.attention {
            Some(Some(_)) if message.is_none() => {}
            pending => *pending = Some(message),
        }
    }

    /// A long command finished while the terminal was out of sight.
    pub fn unseen_finish(&self) -> Option<Finished> {
        self.marks.unseen
    }

    /// The terminal was looked at: forget the unseen finish.
    pub fn mark_seen(&mut self) -> bool {
        self.marks.unseen.take().is_some()
    }

    /// The shell's folder as it last said (OSC 7).
    pub fn live_cwd(&self) -> Option<String> {
        self.marks.cwd.clone()
    }

    /// Commands seen so far (prompts with a command), for tests.
    #[cfg(test)]
    pub fn command_count_for_test(&self) -> usize {
        self.marks.list.iter().filter(|m| m.output.is_some()).count()
    }

    #[cfg(test)]
    pub fn last_output_for_test(&self) -> Option<String> {
        self.last_output()
    }

    #[cfg(test)]
    pub fn scroll_for_test(&mut self, lines: i32) {
        self.term.scroll_display(alacritty_terminal::grid::Scroll::Delta(lines));
    }

    #[cfg(test)]
    pub fn jump_for_test(&mut self, previous: bool) {
        self.jump_to_command(previous);
    }

    #[cfg(test)]
    pub fn visible_marks_for_test(&self) -> Vec<(usize, MarkState)> {
        self.visible_marks(self.term.screen_lines())
    }

    /// Pretend the running command started `secs` seconds earlier.
    #[cfg(test)]
    pub fn backdate_command_for_test(&mut self, secs: u64) {
        self.marks.started = self.marks.started.map(|t| t - std::time::Duration::from_secs(secs));
    }

    #[cfg(test)]
    pub fn feed_for_test(&mut self, data: &[u8]) {
        self.feed(data);
    }

    /// Like `feed_for_test`, then tells the app what the output asked for (bell, notifications), as the output pump does.
    #[cfg(test)]
    pub fn feed_and_report_for_test(&mut self, data: &[u8], cx: &mut gpui::Context<Self>) {
        self.feed(data);
        self.drain_events(cx);
    }
}
