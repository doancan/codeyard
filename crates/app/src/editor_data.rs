//! What the data lens needs from the editor: the text to work on, the cursor's place in it, and a way to put generated
//! text back.

use std::ops::Range;

use super::*;

/// The part of the document a data command works on.
pub struct DataSource {
    /// The selection, or the whole document when nothing is selected.
    pub text: String,
    /// Character range of the selection; `None` when `text` is the whole document.
    pub range: Option<Range<usize>>,
}

impl EditorView {
    pub fn data_source(&self) -> DataSource {
        match self.buffer.selection() {
            Some(range) => DataSource { text: self.buffer.rope().slice(range.clone()).to_string(), range: Some(range) },
            None => DataSource { text: self.buffer.rope().to_string(), range: None },
        }
    }

    pub fn data_len_bytes(&self) -> usize {
        self.buffer.rope().len_bytes()
    }

    /// The whole text (line breaks are LF in the buffer) and the cursor as a byte offset into it.
    pub fn data_text_and_cursor(&self) -> (String, usize) {
        let rope = self.buffer.rope();
        (rope.to_string(), rope.char_to_byte(self.buffer.cursor()))
    }

    /// Replace `range` (the whole document when `None`) with generated text; one undo step.
    pub fn replace_with_generated(&mut self, range: Option<Range<usize>>, text: &str, cx: &mut Context<Self>) {
        let range = range.unwrap_or(0..self.buffer.len_chars());
        let start = range.start;
        self.buffer.replace_all(vec![(range, text.to_string())]);
        self.buffer.set_cursor(start, false);
        self.after_edit(cx);
    }
}
