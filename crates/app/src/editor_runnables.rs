//! Editor: the gutter marks that start a run or a debug session. They come from rust-analyzer's runnables; without a
//! server (or before it answers) the project's test finder and the entry point (`fn main`) stand in.

use super::*;
use crate::lsp::{Runnable, RunnableCommand};
use crate::rust_toolchain::runnables as run;

/// Files with more lines than this get no marks from reading the text (the scan is linear in the file).
const MAX_SCANNED_LINES: usize = 20_000;

#[derive(Default)]
pub(super) struct RunMarks {
    /// What the server offered; `None` until it answered, or when it cannot.
    server: Option<Arc<Vec<Runnable>>>,
    /// Marks found by reading the text, valid for one buffer version.
    scanned: std::cell::RefCell<(Option<u64>, Vec<u32>)>,
}

impl RunMarks {
    /// The server's answer no longer fits the text (lines moved): the text-based marks stand in until it answers again.
    pub(super) fn reset(&mut self) {
        self.server = None;
    }
}

impl EditorView {
    /// The runnables the server offered for this file, if it did.
    pub fn runnables(&self) -> Option<Arc<Vec<Runnable>>> {
        self.run.server.clone()
    }

    pub(super) fn set_runnables(&mut self, list: Option<Vec<Runnable>>) {
        // A server still loading answers with nothing: that is not a file without runnables, the text stands in.
        self.run.server = list.filter(|l| !l.is_empty()).map(Arc::new);
    }

    /// 0-based lines with a mark, sorted.
    pub fn run_mark_lines(&self) -> Vec<u32> {
        if self.language() != "Rust" {
            return Vec::new();
        }
        if let Some(list) = &self.run.server {
            let mut lines: Vec<u32> = list.iter().filter(|r| matches!(r.command, RunnableCommand::Cargo { .. })).filter_map(Runnable::line).collect();
            lines.sort_unstable();
            lines.dedup();
            return lines;
        }
        if self.buffer.len_lines() > MAX_SCANNED_LINES {
            return Vec::new();
        }
        let version = self.buffer.version();
        let mut cache = self.run.scanned.borrow_mut();
        if cache.0 != Some(version) {
            *cache = (Some(version), run::mark_lines(&self.buffer.rope().to_string()));
        }
        cache.1.clone()
    }

    /// A press on the mark of `line`.
    pub fn press_run_mark(&mut self, line: usize, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::RunAt(line as u32));
    }

    /// The cursor as the server counts it: (line, UTF-16 column).
    pub fn cursor_lsp_position(&self) -> (u32, u32) {
        self.buffer.lsp_position(self.buffer.cursor())
    }
}
