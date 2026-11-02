//! Editor: what code extensions put on a document (docs/plugin-architecture.md §5.1). Decorations are drawn with tones,
//! never colours of the extension's choosing, so themes and high contrast keep working. The formatter that runs on save
//! is chosen and asked by the Workbench; the editor only waits for its edits, as it does for a language server. Hover and
//! completion are in editor_ext_lang.rs. Code lenses and inlay hints (#143) show at the end of their line with the
//! language server's; the editor reports the lines it draws, and a click on a lens, to the Workbench
//! (workbench_ext_lenses.rs).

use std::rc::Rc;

use ext_api::value::{DecorationPlace, Tone};
use futures::channel::oneshot;

use super::*;

/// Hover and completion from extensions.
#[path = "editor_ext_lang.rs"]
pub mod lang;

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

/// A code lens an extension gave, with its command: shown at the end of its line, run on click.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtLens {
    pub extension: String,
    pub line: usize,
    pub title: String,
    /// As the extension named it: its own (local or full id), a dependency's, or `core:…`.
    pub command: String,
    /// The command's arguments as JSON.
    pub args: String,
}

/// An inlay hint an extension gave, shown at the end of its line.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtHint {
    pub extension: String,
    pub line: usize,
    pub character: u32,
    pub label: String,
    pub tooltip: Option<String>,
}

/// What an editor tells the Workbench about extension lenses and hints.
#[derive(Clone, Debug)]
pub enum ExtEvent {
    /// The lines in view changed (a scroll, a resize, the first frame): ask for them.
    View,
    /// The user clicked a lens (or ran "Run Code Lens on This Line" on it).
    RunLens(ExtLens),
}

/// The editor's share of extension contributions, set by the Workbench.
#[derive(Default)]
pub struct ExtMarks {
    pub decorations: Vec<LineDecoration>,
    pub save_formatter: Option<SaveFormatter>,
    pub lang: lang::ExtLangState,
    /// Lenses and hints, each kind for the text as it was when it had `lens_lines` / `hint_lines` lines: once the line
    /// count changes they are stale and not shown until the providers answer again.
    pub lenses: Vec<ExtLens>,
    pub hints: Vec<ExtHint>,
    pub lens_lines: usize,
    pub hint_lines: usize,
    /// The lines last drawn (first..end), reported when they change.
    pub view: std::ops::Range<usize>,
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

    /// The lenses (line, title) and hints (line, label) shown, by line.
    #[cfg(test)]
    pub fn ext_lenses_for_test(&self) -> (Vec<(usize, String)>, Vec<(usize, String)>) {
        let lines = self.buffer.len_lines();
        let mut lenses: Vec<(usize, String)> = self.ext.lenses.iter().filter(|_| self.ext.lens_lines == lines).map(|l| (l.line, l.title.clone())).collect();
        let mut hints: Vec<(usize, String)> = self.ext.hints.iter().filter(|_| self.ext.hint_lines == lines).map(|h| (h.line, h.label.clone())).collect();
        lenses.sort();
        hints.sort();
        (lenses, hints)
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

    /// The lines drawn this frame: a change is reported, so the Workbench asks for their lenses and hints.
    pub(super) fn note_ext_view(&mut self, view: std::ops::Range<usize>, cx: &mut Context<Self>) {
        if self.ext.view != view {
            self.ext.view = view;
            cx.emit(EditorEvent::Extension(ExtEvent::View));
        }
    }

    /// The lines in view when last drawn (empty before the first frame).
    pub fn ext_view(&self) -> std::ops::Range<usize> {
        self.ext.view.clone()
    }

    /// Replace `extension`'s lenses (asked for the current text).
    pub fn set_ext_lenses(&mut self, extension: &str, lenses: Vec<ExtLens>, cx: &mut Context<Self>) {
        let lines = self.buffer.len_lines();
        if self.ext.lens_lines != lines {
            self.ext.lenses.clear();
            self.ext.lens_lines = lines;
        }
        self.ext.lenses.retain(|l| l.extension != extension);
        self.ext.lenses.extend(lenses);
        cx.notify();
    }

    /// Add a lens that was resolved after the others came.
    pub fn add_ext_lens(&mut self, lens: ExtLens, cx: &mut Context<Self>) {
        if self.ext.lens_lines == self.buffer.len_lines() && !self.ext.lenses.contains(&lens) {
            self.ext.lenses.push(lens);
            cx.notify();
        }
    }

    /// Replace `extension`'s hints (asked for the current text).
    pub fn set_ext_hints(&mut self, extension: &str, hints: Vec<ExtHint>, cx: &mut Context<Self>) {
        let lines = self.buffer.len_lines();
        if self.ext.hint_lines != lines {
            self.ext.hints.clear();
            self.ext.hint_lines = lines;
        }
        self.ext.hints.retain(|h| h.extension != extension);
        self.ext.hints.extend(hints);
        cx.notify();
    }

    /// Remove the lenses and hints of `extension`, or of every extension.
    pub fn clear_ext_lenses(&mut self, extension: Option<&str>, cx: &mut Context<Self>) {
        let before = self.ext.lenses.len() + self.ext.hints.len();
        self.ext.lenses.retain(|l| extension.is_some_and(|e| l.extension != e));
        self.ext.hints.retain(|h| extension.is_some_and(|e| h.extension != e));
        if self.ext.lenses.len() + self.ext.hints.len() != before {
            cx.notify();
        }
    }

    /// Keep only the lenses of `lens_ids` and the hints of `hint_ids` (the extensions that still provide them here).
    pub fn keep_ext_lenses_of(&mut self, lens_ids: &[String], hint_ids: &[String], cx: &mut Context<Self>) {
        let before = self.ext.lenses.len() + self.ext.hints.len();
        self.ext.lenses.retain(|l| lens_ids.contains(&l.extension));
        self.ext.hints.retain(|h| hint_ids.contains(&h.extension));
        if self.ext.lenses.len() + self.ext.hints.len() != before {
            cx.notify();
        }
    }

    /// How many lenses and hints are shown (performance scenarios).
    pub fn ext_marks_count(&self) -> usize {
        let lines = self.buffer.len_lines();
        (if self.ext.lens_lines == lines { self.ext.lenses.len() } else { 0 }) + (if self.ext.hint_lines == lines { self.ext.hints.len() } else { 0 })
    }

    /// Lenses shown on `line` (none while they are stale).
    pub fn ext_lenses_on(&self, line: usize) -> impl Iterator<Item = &ExtLens> {
        let fresh = self.ext.lens_lines == self.buffer.len_lines();
        self.ext.lenses.iter().filter(move |l| fresh && l.line == line)
    }

    /// Hints shown on `line`, in column order (none while they are stale).
    pub fn ext_hints_on(&self, line: usize) -> Vec<&ExtHint> {
        let fresh = self.ext.hint_lines == self.buffer.len_lines();
        let mut out: Vec<&ExtHint> = self.ext.hints.iter().filter(|h| fresh && h.line == line).collect();
        out.sort_by_key(|h| h.character);
        out
    }

    /// The tooltip lines of extension lenses and hints on `line`: each names its extension.
    pub fn ext_eol_tooltip(&self, line: usize) -> Option<String> {
        let mut out: Vec<String> = self.ext_lenses_on(line).map(|l| crate::i18n::tf("{0} (from {1})", &[&l.title, &l.extension])).collect();
        for h in self.ext_hints_on(line) {
            let text = match &h.tooltip {
                Some(t) => format!("{}: {t}", h.label),
                None => h.label.clone(),
            };
            out.push(crate::i18n::tf("{0} (from {1})", &[&text, &h.extension]));
        }
        (!out.is_empty()).then(|| out.join("\n"))
    }

    /// Run the first extension lens on `line`; `false` when there is none.
    pub fn click_ext_lens(&mut self, line: usize, cx: &mut Context<Self>) -> bool {
        let Some(lens) = self.ext_lenses_on(line).next().cloned() else { return false };
        cx.emit(EditorEvent::Extension(ExtEvent::RunLens(lens)));
        true
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
