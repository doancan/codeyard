//! Editor: JSON/YAML schema completion and hover. For JSON and YAML files without a language server the workbench
//! wires up the schema (`set_schema`); suggestions and descriptions come from the `schema` crate, the text is in editor memory
//! (works on remote SSH files too).

use super::*;

/// Schema bound to the file, and its format.
pub struct SchemaBinding {
    pub set: Arc<schema::validate::SchemaSet>,
    pub format: schema::ide::Format,
}

/// Everything needed to compute the hover text in the background.
pub(super) struct SchemaHoverInput {
    binding: Arc<SchemaBinding>,
    text: String,
    offset: usize,
}

impl SchemaHoverInput {
    pub(super) fn run(self) -> Option<String> {
        schema::ide::hover(&self.binding.set, self.binding.format, &self.text, self.offset).map(|(t, ..)| t)
    }
}

impl EditorView {
    pub fn set_schema(&mut self, binding: Option<Arc<SchemaBinding>>) {
        self.schema = binding;
    }

    /// Snapshot of the document (cheap clone): schema check, merge editor.
    pub fn rope_snapshot(&self) -> buffer::Rope {
        self.buffer.rope().clone()
    }

    #[cfg(test)]
    pub fn has_schema(&self) -> bool {
        self.schema.is_some()
    }

    /// Input for hover: if a schema is bound, there's no language server and the document is a reasonable size.
    pub(super) fn schema_hover_input(&self, ix: usize, has_lsp: bool) -> Option<SchemaHoverInput> {
        let binding = self.schema.clone().filter(|_| !has_lsp)?;
        let rope = self.buffer.rope();
        if rope.len_bytes() > schema::ide::MAX_BYTES {
            return None;
        }
        Some(SchemaHoverInput { binding, text: rope.to_string(), offset: rope.char_to_byte(ix.min(rope.len_chars())) })
    }

    /// As you type: open or filter suggestions on a key/value character; open value suggestions after `: `.
    pub(super) fn after_typed_schema(&mut self, typed: &str, cx: &mut Context<Self>) {
        let mut chars = typed.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            self.completion = None;
            return;
        };
        if self.buffer.cursor_count() > 1 || !cx.global::<Settings>().quick_suggestions {
            self.completion = None;
            return;
        }
        let cursor = self.buffer.cursor();
        let after_colon = ch == ' ' && cursor >= 2 && self.buffer.text_range(cursor - 2..cursor) == ": ";
        if Self::is_word(ch) || ch == '-' || ch == '"' || after_colon {
            match &self.completion {
                Some(_) if Self::is_word(ch) || ch == '-' => self.refilter(cx),
                _ => self.request_schema_completion(cx),
            }
        } else {
            self.completion = None;
        }
    }

    pub(super) fn request_schema_completion(&mut self, cx: &mut Context<Self>) {
        let Some(binding) = self.schema.clone() else { return };
        let rope = self.buffer.rope();
        if rope.len_bytes() > schema::ide::MAX_BYTES {
            return;
        }
        let (text, offset, version) = (rope.to_string(), rope.char_to_byte(self.buffer.cursor()), self.buffer.version());
        let task = cx.background_executor().spawn(async move {
            let found = schema::ide::complete(&binding.set, binding.format, &text, offset);
            (found, text)
        });
        self.completion_task = Some(cx.spawn(async move |this, cx| {
            let (found, text) = task.await;
            this.update(cx, |e, cx| {
                // If typing happened meanwhile the positions are stale: don't open in vain, the next key asks again.
                if e.buffer.version() != version {
                    return;
                }
                let Some(found) = found else {
                    e.completion = None;
                    cx.notify();
                    return;
                };
                let pos = |b: usize| schema::doc::position(&text, b);
                let items: Vec<crate::lsp::CompletionItem> = found
                    .items
                    .iter()
                    .map(|i| crate::lsp::CompletionItem {
                        label: i.label.clone(),
                        detail: i.detail.clone(),
                        kind: i.kind,
                        filter: i.label.clone(),
                        sort: i.sort.clone(),
                        insert: i.insert.clone(),
                        cursor: i.cursor,
                        stops: Vec::new(),
                        range: Some((pos(i.start), pos(i.end))),
                        extra: Vec::new(),
                        doc: i.doc.clone(),
                        resolve: None,
                        source: None,
                    })
                    .collect();
                let start = text[..found.filter_start].chars().count();
                e.open_completion(items, start, false, cx);
            })
            .ok();
        }));
    }
}

#[cfg(test)]
impl EditorView {
    pub fn replace_all_text_for_test(&mut self, text: &str, cx: &mut Context<Self>) {
        let len = self.buffer.len_chars();
        self.buffer.replace_all(vec![(0..len, text.to_string())]);
        self.after_edit(cx);
    }

    /// 0-based line and character column.
    pub fn set_cursor_line_col_for_test(&mut self, line: usize, col: usize, cx: &mut Context<Self>) {
        let at = self.buffer.line_to_char(line) + col;
        self.buffer.set_cursor(at, false);
        cx.notify();
    }

    /// Hover text from the schema (relative to the character at the line start; 0-based line).
    pub fn schema_hover_for_test(&self, line: usize) -> Option<String> {
        let ix = self.buffer.line_to_char(line);
        self.schema_hover_input(ix, false)?.run()
    }
}
