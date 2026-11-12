//! Editor: the semantic token modifiers the server sent (`unsafe`, `mutable`, consumed), kept by line and drawn as
//! underlines and italics over the colours (see `editor_overlay::modifier_runs`).

use std::ops::Range;

use super::*;
use crate::lsp::SemToken;
use crate::rust_toolchain::hints::Modifiers;

#[derive(Default)]
pub(super) struct ModifierMarks {
    /// line → (start, length (UTF-16), flags)
    by_line: std::collections::HashMap<usize, Vec<(u32, u32, u8)>>,
}

impl ModifierMarks {
    pub(super) fn clear(&mut self) {
        self.by_line.clear();
    }
}

impl EditorView {
    /// Keeps the tokens that carry a modifier the editor knows.
    pub(super) fn set_modifier_marks(&mut self, tokens: &[SemToken]) {
        self.modmarks.by_line.clear();
        for t in tokens.iter().filter(|t| t.mods != 0) {
            self.modmarks.by_line.entry(t.line).or_default().push((t.start, t.len, t.mods));
        }
    }

    /// The marks of line `ix` that are drawn, as byte ranges within the part of the line shown (`full[b0..b1]`).
    pub(super) fn modifier_spans(&self, ix: usize, full: &str, (b0, b1): (usize, usize), shown: Modifiers) -> Vec<(Range<usize>, u8)> {
        let Some(marks) = self.modmarks.by_line.get(&ix) else { return Vec::new() };
        marks
            .iter()
            .filter_map(|&(start, len, flags)| {
                let flags = shown.shown(flags);
                let a = editor_decor::utf16_to_byte(full, start).max(b0);
                let b = editor_decor::utf16_to_byte(full, start.saturating_add(len)).min(b1);
                (flags != 0 && a < b).then(|| (a - b0..b - b0, flags))
            })
            .collect()
    }

    #[cfg(test)]
    pub fn modifier_marks_for_test(&self) -> Vec<(usize, u32, u32, u8)> {
        let mut all: Vec<_> = self.modmarks.by_line.iter().flat_map(|(line, v)| v.iter().map(move |&(s, l, f)| (*line, s, l, f))).collect();
        all.sort();
        all
    }
}
