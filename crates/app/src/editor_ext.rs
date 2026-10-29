//! Editor: what code extensions put on a document (docs/plugin-architecture.md §5.1). Decorations are drawn with tones,
//! never colours of the extension's choosing, so themes and high contrast keep working. The formatter that runs on save
//! is chosen and asked by the Workbench; the editor only waits for its edits, as it does for a language server.

use std::rc::Rc;

use ext_api::value::{DecorationPlace, Tone};
use futures::channel::oneshot;

use super::*;

/// What the Workbench needs to ask a formatter: the document as it is now.
pub struct FormatJob {
    pub path: String,
    pub text: String,
    /// `buffer.version()` when the job was taken: edits apply only if the document did not change since.
    pub version: u64,
    pub tab_size: u32,
    pub insert_spaces: bool,
}

/// Asks an extension to format the document before it is written: `None` when no extension formats this file (the
/// language server, if any, does). The answer is the edits, or nothing when the formatter failed, was too slow or
/// the user moved on.
pub type SaveFormatter = Rc<dyn Fn(FormatJob, &mut gpui::AsyncApp) -> Option<oneshot::Receiver<Vec<(crate::lsp::Range16, String)>>>>;

/// A decoration resolved to lines (decorations are drawn per line).
#[derive(Clone, Debug, PartialEq)]
pub struct LineDecoration {
    pub first: usize,
    pub last: usize,
    pub place: DecorationPlace,
    pub tone: Tone,
    pub text: Option<String>,
    pub hover: Option<String>,
}

/// The editor's share of extension contributions, set by the Workbench.
#[derive(Default)]
pub struct ExtMarks {
    pub decorations: Vec<LineDecoration>,
    pub save_formatter: Option<SaveFormatter>,
}

/// The palette colour of a tone.
pub fn tone_color(p: &Palette, tone: Tone) -> u32 {
    match tone {
        Tone::Neutral => p.text,
        Tone::Accent => p.accent,
        Tone::Success => p.success,
        Tone::Warning => p.warning,
        Tone::Danger => p.danger,
        Tone::Muted => p.text_muted,
    }
}

impl EditorView {
    /// Replace the extension decorations shown on this document.
    pub fn set_ext_decorations(&mut self, decorations: Vec<LineDecoration>, cx: &mut Context<Self>) {
        if self.ext.decorations != decorations {
            self.ext.decorations = decorations;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn ext_decorations(&self) -> &[LineDecoration] {
        &self.ext.decorations
    }

    pub fn set_save_formatter(&mut self, formatter: Option<SaveFormatter>) {
        self.ext.save_formatter = formatter;
    }

    /// The most urgent tone of a decoration at `place` on `line` (danger before warning before the rest).
    pub(super) fn ext_tone(&self, line: usize, place: DecorationPlace) -> Option<Tone> {
        let rank = |t: Tone| match t {
            Tone::Danger => 0,
            Tone::Warning => 1,
            Tone::Success => 2,
            Tone::Accent => 3,
            Tone::Neutral => 4,
            Tone::Muted => 5,
        };
        self.ext.decorations.iter().filter(|d| d.place == place && d.first <= line && line <= d.last).map(|d| d.tone).min_by_key(|t| rank(*t))
    }

    /// Text after the end of `line`: the first `after` decoration ending there, with its tone and hover text.
    pub(super) fn ext_after(&self, line: usize) -> Option<(String, Tone, Option<String>)> {
        let d = self.ext.decorations.iter().find(|d| d.place == DecorationPlace::After && d.last == line && d.text.is_some())?;
        Some((d.text.clone().unwrap_or_default(), d.tone, d.hover.clone()))
    }

    /// The selection (or the cursor) as a zero-based line and UTF-16 column range.
    pub fn selection_range16(&self) -> crate::lsp::Range16 {
        let r = self.buffer.selection_or_cursor();
        (self.buffer.lsp_position(r.start), self.buffer.lsp_position(r.end))
    }

    /// The document for a formatter.
    pub fn format_job(&self) -> FormatJob {
        FormatJob {
            path: self.path.clone(),
            text: self.buffer_text(),
            version: self.buffer.version(),
            tab_size: self.indent_unit() as u32,
            insert_spaces: self.indent != "\t",
        }
    }
}
