//! Editor: indentation. The unit (tab or n spaces) from EditorConfig, the folder and the global settings, converting a file
//! between tabs and spaces, the labels the status bar shows, and how many indent guides a line gets.

use super::*;

impl EditorView {
    /// Settings (including project settings) changed: re-resolve the indent unit.
    pub fn refresh_indent(&mut self, cx: &mut Context<Self>) {
        self.indent = indent_with_rules(&self.buffer, &self.rules, cx.global::<Settings>().editorconfig, self.indent_prefs(cx));
        self.display_key = None;
        cx.notify();
    }

    /// (indent is spaces?, width): the folder's setting if there is one, otherwise the global setting.
    pub(super) fn indent_prefs(&self, cx: &App) -> (Option<bool>, Option<usize>) {
        let s = cx.global::<Settings>();
        let f = self.folder.as_deref();
        (f.and_then(|f| f.insert_spaces).or(s.insert_spaces), f.and_then(|f| f.tab_size).or(s.tab_size))
    }

    /// Folder setting (files in additional folders): the indent is re-resolved when it changes.
    pub fn set_folder_settings(&mut self, folder: Option<Arc<crate::folder_settings::FolderSettings>>, cx: &mut Context<Self>) {
        if self.folder == folder {
            return;
        }
        self.folder = folder;
        self.refresh_indent(cx);
    }

    /// Indent unit: `None` is tab, `Some(n)` is n spaces (for text written from now on).
    pub fn set_indent(&mut self, spaces: Option<usize>, cx: &mut Context<Self>) {
        self.indent = spaces.map_or("\t".to_string(), |n| " ".repeat(n));
        self.display_key = None; // tab width affects scrolling
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    /// Convert the indentation at line starts to tabs or spaces (single undo step).
    pub fn convert_indentation(&mut self, to_tabs: bool, cx: &mut Context<Self>) {
        let unit = self.indent_unit();
        let mut edits = Vec::new();
        for l in 0..self.buffer.len_lines() {
            let line = self.buffer.line(l);
            let lead: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            if lead.is_empty() {
                continue;
            }
            let width: usize = lead.chars().map(|c| if c == '\t' { unit } else { 1 }).sum();
            let new = if to_tabs { format!("{}{}", "\t".repeat(width / unit), " ".repeat(width % unit)) } else { " ".repeat(width) };
            if new != lead {
                let start = self.buffer.line_to_char(l);
                edits.push((start..start + lead.chars().count(), new));
            }
        }
        self.indent = if to_tabs { "\t".into() } else { " ".repeat(unit) };
        if !edits.is_empty() {
            self.buffer.apply_edits(edits);
            self.after_edit(cx);
        }
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    /// The indentation as the English key the indent picker compares against.
    pub fn indent_key(&self) -> String {
        if self.indent == "\t" { "Tabs".into() } else { format!("{} spaces", self.indent.len()) }
    }

    pub fn indent_label(&self) -> String {
        if self.indent == "\t" { crate::i18n::t("Tabs").into() } else { crate::i18n::tn("{} space", "{} spaces", self.indent.len(), &[]) }
    }

    /// Column width of one indent level. A tab is 4 columns in the text painter.
    #[cfg(test)]
    pub fn indent_unit_for_test(&self) -> usize {
        self.indent_unit()
    }

    pub(super) fn indent_unit(&self) -> usize {
        if self.indent == "\t" { TAB_COLS } else { self.indent.len().max(1) }
    }

    /// Number of guide lines. On an empty line, the smaller of the surrounding non-empty lines
    /// (so a guide doesn't break on an empty line inside a block).
    pub(super) fn guide_levels(&self, ix: usize) -> usize {
        let unit = self.indent_unit();
        if let Some(l) = indent_level(&self.buffer.line(ix), unit) {
            return l;
        }
        let n = self.buffer.len_lines();
        let up = (ix.saturating_sub(50)..ix).rev().find_map(|i| indent_level(&self.buffer.line(i), unit));
        let down = (ix + 1..(ix + 50).min(n)).find_map(|i| indent_level(&self.buffer.line(i), unit));
        up.unwrap_or(0).min(down.unwrap_or(0))
    }
}
