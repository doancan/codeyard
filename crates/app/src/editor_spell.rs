//! Editor: spell check. The workbench wires up the checker (`set_speller`); misspelled words are computed while a line is painted
//! (cached per word) and shown with a wavy underline. In prose files (Markdown, text)
//! everything is checked, in code only comments.

use syntax::Role;

use super::*;

/// Diagnostic severity code for a misspelled word (for color choice): separate from real diagnostics (1-4).
pub(super) const SPELLING_SEVERITY: u8 = 9;

impl EditorView {
    pub fn set_speller(&mut self, speller: Option<Arc<spell::Speller>>, cx: &mut Context<Self>) {
        self.spell = speller;
        cx.notify();
    }

    /// Character offset of the cursor.
    pub fn cursor_char(&self) -> usize {
        self.buffer.cursor()
    }

    #[cfg(test)]
    pub fn has_speller(&self) -> bool {
        self.spell.is_some()
    }

    /// Misspelled words in a line segment (byte range, within `text`). `spans`: syntax runs
    /// (absolute bytes); `abs_start`: byte start of the segment in the document.
    pub(super) fn spelling_in_segment(
        &self,
        line: usize,
        spans: Option<&[(Range<usize>, Role)]>,
        abs_start: usize,
        text: &str,
        on: (bool, bool),
    ) -> Vec<Range<usize>> {
        let (spelling, comments) = on;
        let Some(speller) = &self.spell else { return Vec::new() };
        if !spelling || speller.is_empty() || text.len() > 4000 {
            return Vec::new();
        }
        let prose = spell::prose_extension(&self.path);
        if (!prose && !comments) || (spans.is_none() && !prose) {
            return Vec::new();
        }
        // Markdown fenced code blocks: inactive tabs may drop the syntax tree, so we track the fence ourselves.
        if prose && spell::is_markdown(&self.path) && self.in_code_fence(line) {
            return Vec::new();
        }
        let runs = match spans {
            Some(sp) => syntax::line_runs(abs_start, text.len(), sp),
            None => vec![(text.len(), None)],
        };
        // In prose: plain text, comments and headings; code spans, links, strings are skipped. In code: comments.
        let mut allowed: Vec<Range<usize>> = Vec::new();
        let mut at = 0;
        for (len, role) in runs {
            let ok = matches!((prose, role), (true, None | Some(Role::Comment | Role::Keyword)) | (false, Some(Role::Comment)));
            if ok {
                allowed.push(at..at + len);
            }
            at += len;
        }
        if allowed.is_empty() {
            return Vec::new();
        }
        // Markdown: `inline code` is a separate language so it gives no syntax role; we skip the backtick spans ourselves.
        let code: Vec<Range<usize>> = if prose && spell::is_markdown(&self.path) { inline_code_spans(text) } else { Vec::new() };
        speller.misspelled(text, &|i| allowed.iter().any(|r| r.contains(&i)) && !code.iter().any(|r| r.contains(&i)))
    }

    /// In Markdown, is the line inside a ``` / ~~~ fence (the fence itself included). Computed once per document version.
    fn in_code_fence(&self, line: usize) -> bool {
        let version = self.buffer.version();
        let mut cache = self.fence_cache.borrow_mut();
        if cache.as_ref().is_none_or(|(v, _)| *v != version) {
            let n = self.buffer.len_lines();
            let mut flags = vec![false; n];
            if n <= 200_000 {
                let mut open: Option<String> = None;
                for (flag, text) in flags.iter_mut().zip(self.buffer.rope().lines()) {
                    let marker = fence_marker(text.chars());
                    match (&open, marker) {
                        (None, Some(m)) => {
                            open = Some(m.to_string());
                            *flag = true;
                        }
                        (Some(o), Some(m)) if o == m => {
                            open = None;
                            *flag = true;
                        }
                        (Some(_), _) => *flag = true,
                        _ => {}
                    }
                }
            }
            *cache = Some((version, flags));
        }
        cache.as_ref().is_some_and(|(_, f)| f.get(line).copied().unwrap_or(false))
    }

    /// Misspelled word at the cursor (or immediately left of it): (character range, word).
    pub fn misspelling_at(&self, ix: usize, cx: &App) -> Option<(Range<usize>, String)> {
        let s = cx.global::<Settings>();
        let on = (s.spelling, s.spelling_comments);
        let rope = self.buffer.rope();
        let line = self.buffer.char_to_line(ix.min(self.buffer.len_chars()));
        let text = self.buffer.line(line);
        let line_byte = rope.line_to_byte(line);
        let spans = self.syntax.as_ref().map(|sx| sx.highlights(rope, line_byte..line_byte + text.len()));
        let line_start = self.buffer.line_to_char(line);
        self.spelling_in_segment(line, spans.as_deref(), line_byte, &text, on).into_iter().find_map(|r| {
            let (a, b) = (line_start + text[..r.start].chars().count(), line_start + text[..r.end].chars().count());
            (a <= ix && ix <= b).then(|| (a..b, text[r].to_string()))
        })
    }

    /// Replace the word (single undo step).
    pub fn replace_range_with(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.buffer.replace_all(vec![(range.clone(), text.to_string())]);
        self.buffer.set_cursor(range.start + text.chars().count(), false);
        self.after_edit(cx);
    }

    #[cfg(test)]
    pub fn misspelled_words_for_test(&self, cx: &App) -> Vec<String> {
        let s = cx.global::<Settings>();
        let on = (s.spelling, s.spelling_comments);
        let rope = self.buffer.rope();
        (0..self.buffer.len_lines())
            .flat_map(|line| {
                let text = self.buffer.line(line);
                let line_byte = rope.line_to_byte(line);
                let spans = self.syntax.as_ref().map(|sx| sx.highlights(rope, line_byte..line_byte + text.len()));
                self.spelling_in_segment(line, spans.as_deref(), line_byte, &text, on).into_iter().map(|r| text[r].to_string()).collect::<Vec<_>>()
            })
            .collect()
    }
}

/// The fence a line opens or closes: ``` or ~~~ after any indentation. Reads the rope in place, without copying the line.
fn fence_marker(chars: impl Iterator<Item = char>) -> Option<&'static str> {
    let mut chars = chars.skip_while(|c| c.is_whitespace());
    match (chars.next(), chars.next(), chars.next()) {
        (Some('`'), Some('`'), Some('`')) => Some("```"),
        (Some('~'), Some('~'), Some('~')) => Some("~~~"),
        _ => None,
    }
}

/// Ranges between double backticks (bytes, quotes included); an unclosed quote is ignored.
fn inline_code_spans(text: &str) -> Vec<Range<usize>> {
    let ticks: Vec<usize> = text.match_indices('`').map(|(i, _)| i).collect();
    (0..ticks.len() / 2).map(|i| ticks[2 * i]..ticks[2 * i + 1] + 1).collect()
}

impl EditorView {
    /// Request lines in a `.http` / `.rest` file (0-based); parsed once per document version.
    pub(super) fn http_request_lines(&self) -> Vec<usize> {
        if !crate::http_client::is_http_file(&self.path) {
            return Vec::new();
        }
        let version = self.buffer.version();
        let mut cache = self.http_lines.borrow_mut();
        if cache.as_ref().is_none_or(|(v, _)| *v != version) {
            let text = self.buffer.rope().to_string();
            *cache = Some((version, crate::http_client::parse(&text).requests.iter().map(|r| r.line).collect()));
        }
        cache.as_ref().map(|(_, l)| l.clone()).unwrap_or_default()
    }

    /// Host the file lives on (request body files are read from here).
    pub fn host_arc(&self) -> Arc<dyn host::Host> {
        self.host.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{fence_marker, inline_code_spans};

    #[test]
    fn fence_markers_after_indentation() {
        assert_eq!(fence_marker("  ```rust\n".chars()), Some("```"));
        assert_eq!(fence_marker("\t~~~".chars()), Some("~~~"));
        assert_eq!(fence_marker("``".chars()), None);
        assert_eq!(fence_marker("a ```".chars()), None);
        assert_eq!(fence_marker("".chars()), None);
    }

    #[test]
    fn inline_code_spans_pair_up_backticks() {
        assert_eq!(inline_code_spans("a `b` c `d`"), [2..5, 8..11]);
        assert_eq!(inline_code_spans("open ` never closed"), Vec::<std::ops::Range<usize>>::new());
        assert_eq!(inline_code_spans("`a` `b"), [0..3]);
    }
}
