//! Editor: as you type: completion, signature, snippets and Emmet.

use super::*;

impl EditorView {
    pub(super) fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    /// Start of the word to the left of the cursor.
    pub(super) fn word_start(&self, cursor: usize) -> usize {
        let rope = self.buffer.rope();
        let mut i = cursor;
        while i > 0 && Self::is_word(rope.char(i - 1)) {
            i -= 1;
        }
        i
    }

    /// Open, filter or close the list based on the typed text (single cursor only).
    pub(super) fn after_typed(&mut self, typed: &str, cx: &mut Context<Self>) {
        // Extensions are asked on their own (editor_ext_lang.rs): their items join whichever list opens.
        self.ext_after_typed(typed, cx);
        let Some(client) = self.lsp.clone() else {
            return if self.schema.is_some() { self.after_typed_schema(typed, cx) } else { self.after_typed_words(typed, cx) };
        };
        let mut chars = typed.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            self.completion = None;
            return;
        };
        if self.buffer.cursor_count() > 1 {
            self.completion = None;
            return;
        }
        if self.scoped(|f| f.format_on_type, cx.global::<Settings>().format_on_type) {
            self.format_typed(ch, client.clone(), cx);
        }
        // Signature help: request when a call opens or on a new argument, dismiss when it closes.
        match ch {
            ')' => self.signature = None,
            '(' | ',' if client.capabilities["signatureHelpProvider"].is_object() => self.request_signature(cx),
            // While open, refresh on every key (the active parameter changes; the first response may be stale).
            _ if self.signature.is_some() => self.request_signature(cx),
            _ => {}
        }
        if !cx.global::<Settings>().quick_suggestions {
            self.completion = None;
            return;
        }
        if Self::is_word(ch) {
            match &self.completion {
                Some(c) if c.incomplete => self.request_completion(None, cx),
                Some(_) => self.refilter(cx),
                None => self.request_completion(None, cx),
            }
        } else if client.is_trigger(ch) {
            self.request_completion(Some(ch), cx);
        } else {
            self.completion = None;
        }
    }

    /// No language server: words from the document after two letters (while the list is open it is only filtered).
    pub(super) fn after_typed_words(&mut self, typed: &str, cx: &mut Context<Self>) {
        let mut chars = typed.chars();
        let one_word_char = matches!((chars.next(), chars.next()), (Some(ch), None) if Self::is_word(ch));
        if !one_word_char || self.buffer.cursor_count() > 1 || !cx.global::<Settings>().word_based_suggestions || !cx.global::<Settings>().quick_suggestions {
            self.completion = None;
            return;
        }
        // The word start is searched at most 100 characters back: in a giant single line (minified,
        // base64) do not walk to the line start on every key; a "word" of that length is not a suggestion candidate.
        const MAX_WORD: usize = 100;
        let cursor = self.buffer.cursor();
        let rope = self.buffer.rope();
        let typed = (1..=MAX_WORD + 1).take_while(|&n| n <= cursor && Self::is_word(rope.char(cursor - n))).count();
        // A list of extension items only still gets the document's words.
        let words_shown = self.completion.as_ref().is_some_and(|c| c.items.iter().any(|i| i.source.is_none()));
        match &self.completion {
            Some(_) if words_shown || !(2..=MAX_WORD).contains(&typed) => self.refilter(cx),
            _ if (2..=MAX_WORD).contains(&typed) => self.request_word_completion(cx),
            _ => {}
        }
    }

    /// Word list: from ~1 MB around the cursor, in the background. The list stays cached
    /// (not rescanned on every key): the words rarely change over a few keystrokes.
    pub(super) fn request_word_completion(&mut self, cx: &mut Context<Self>) {
        const AROUND: usize = 512 * 1024;
        const STALE_AFTER_EDITS: u64 = 200;
        let cursor = self.buffer.cursor();
        let start = self.word_start(cursor);
        let version = self.buffer.version();
        let open = move |e: &mut Self, items: Vec<crate::lsp::CompletionItem>, cx: &mut Context<Self>| e.open_completion(items, start, false, cx);
        if let Some((v, at, words)) = &self.word_cache
            && version.saturating_sub(*v) < STALE_AFTER_EDITS
            && cursor.abs_diff(*at) < AROUND / 4
        {
            let items = words.to_vec();
            return open(self, items, cx);
        }
        let rope = self.buffer.rope().clone();
        let task = cx.background_executor().spawn(async move {
            let from = cursor.saturating_sub(AROUND);
            let to = (cursor + AROUND).min(rope.len_chars());
            let text = rope.slice(from..to).to_string();
            let at = text.char_indices().nth(cursor - from).map_or(text.len(), |(b, _)| b);
            crate::lsp::buffer_words(&text, at, 2000)
        });
        self.completion_task = Some(cx.spawn(async move |this, cx| {
            let items = task.await;
            this.update(cx, |e, cx| {
                e.word_cache = Some((version, cursor, items.clone().into()));
                // If the cursor moved to another word in the meantime, do not open the stale list.
                if e.buffer.cursor() < start || e.word_start(e.buffer.cursor()) != start {
                    return;
                }
                open(e, items, cx);
            })
            .ok();
        }));
    }

    pub(super) fn request_completion(&mut self, trigger: Option<char>, cx: &mut Context<Self>) {
        let Some(client) = self.lsp.clone() else {
            if self.schema.is_some() {
                return self.request_schema_completion(cx);
            }
            if trigger.is_none() && cx.global::<Settings>().word_based_suggestions {
                self.request_word_completion(cx);
            }
            return;
        };
        self.sync_lsp(); // changes first, then the request (same pipe, in order)
        let (line, col) = self.buffer.lsp_position(self.buffer.cursor());
        let context = match trigger {
            Some(t) => serde_json::json!({ "triggerKind": 2, "triggerCharacter": t.to_string() }),
            None => serde_json::json!({ "triggerKind": 1 }),
        };
        let params = serde_json::json!({
            "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) },
            "position": { "line": line, "character": col },
            "context": context,
        });
        // The response can be thousands of items: parse in the background too (so typing is not blocked).
        let task = cx.background_executor().spawn(async move {
            client.request("textDocument/completion", params, std::time::Duration::from_secs(5)).map(|r| crate::lsp::parse_completions(&r))
        });
        self.completion_task = Some(cx.spawn(async move |this, cx| {
            let Ok((items, incomplete)) = task.await else { return };
            this.update(cx, |e, cx| {
                let start = e.word_start(e.buffer.cursor());
                e.open_completion(items, start, incomplete, cx);
            })
            .ok();
        }));
    }

    /// Filter by the typed prefix: fuzzy match score, then the server's order.
    pub(super) fn refilter(&mut self, cx: &mut Context<Self>) {
        let cursor = self.buffer.cursor();
        let Some(c) = &mut self.completion else { return };
        if cursor < c.start {
            self.completion = None;
            cx.notify();
            return;
        }
        let prefix = self.buffer.text_range(c.start..cursor);
        let mut scored: Vec<(i32, usize)> = c
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| if prefix.is_empty() { Some((0, i)) } else { crate::fuzzy::score(&prefix, &it.filter).map(|(s, _)| (s, i)) })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| c.items[a.1].sort.cmp(&c.items[b.1].sort)));
        c.shown = scored.into_iter().map(|(_, i)| i).take(200).collect();
        c.selected = 0;
        c.prefix = prefix.clone();
        if c.shown.is_empty() || (c.shown.len() == 1 && c.items[c.shown[0]].insert == prefix) {
            self.completion = None; // no match, or already fully typed
        }
        cx.notify();
    }

    pub(crate) fn request_signature(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.lsp.clone() else { return };
        self.sync_lsp();
        let (line, col) = self.buffer.lsp_position(self.buffer.cursor());
        let params = serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) }, "position": { "line": line, "character": col } });
        let task = cx.background_executor().spawn(async move { client.request("textDocument/signatureHelp", params, std::time::Duration::from_secs(3)) });
        self.signature_task = Some(cx.spawn(async move |this, cx| {
            let Ok(v) = task.await else { return };
            this.update(cx, |e, cx| {
                e.signature = crate::lsp::parse_signature(&v);
                cx.notify();
            })
            .ok();
        }));
    }

    #[cfg(test)]
    pub fn signature_text(&self) -> Option<String> {
        self.signature.as_ref().map(|(l, r)| match r {
            Some(r) => format!("{}[{}]{}", &l[..r.start], &l[r.clone()], &l[r.end..]),
            None => l.clone(),
        })
    }

    pub(super) fn trigger_completion(&mut self, _: &TriggerCompletion, _: &mut Window, cx: &mut Context<Self>) {
        self.ask_ext_completion(None, cx);
        self.request_completion(None, cx);
    }

    pub(super) fn completion_next(&mut self, _: &CompletionNext, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = &mut self.completion {
            c.selected = (c.selected + 1) % c.shown.len().max(1);
            cx.notify();
        }
    }

    pub(super) fn completion_prev(&mut self, _: &CompletionPrev, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = &mut self.completion {
            let n = c.shown.len().max(1);
            c.selected = (c.selected + n - 1) % n;
            cx.notify();
        }
    }

    pub(super) fn completion_dismiss(&mut self, _: &CompletionDismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.completion = None;
        self.end_ext_completion();
        cx.notify();
    }

    /// Apply the selected suggestion: main edit + additional edits (imports) as a single undo step.
    pub(super) fn completion_accept(&mut self, _: &CompletionAccept, _: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.completion.take() else { return };
        self.end_ext_completion();
        let Some(item) = c.shown.get(c.selected).map(|&i| c.items[i].clone()) else { return };
        let cursor = self.buffer.cursor();
        let main = match item.range {
            // The server's range is relative to the request time: let text typed since then change it too.
            Some((a, b)) => self.buffer.from_lsp_position(a.0, a.1)..self.buffer.from_lsp_position(b.0, b.1).max(cursor),
            None => c.start..cursor,
        };
        let mut parts: Vec<(Range<usize>, String)> = vec![(main.clone(), item.insert.clone())];
        for ((a, b), text) in &item.extra {
            let r = self.buffer.from_lsp_position(a.0, a.1)..self.buffer.from_lsp_position(b.0, b.1);
            if r.end <= main.start || r.start >= main.end {
                parts.push((r, text.clone()));
            }
        }
        parts.sort_by_key(|(r, _)| r.start);
        // Start of the main edit in the final document: shifts by the difference of the edits before it.
        let shift: isize = parts.iter().take_while(|(r, _)| r.start < main.start).map(|(r, t)| t.chars().count() as isize - r.len() as isize).sum();
        self.buffer.replace_all(parts);
        let start = (main.start as isize + shift) as usize;
        // Snippet: mark the tab stops (they shift as you type), select the first placeholder.
        if item.stops.len() > 1 || item.stops.first().is_some_and(|(_, r)| !r.is_empty()) {
            let marks = item.stops.iter().flat_map(|(_, r)| [start + r.start, start + r.end]).collect();
            self.buffer.set_marks(SNIPPET_MARKS, marks);
            self.snippet_stop = Some(0);
            self.select_snippet_stop(0);
        } else {
            let at = start + item.cursor.unwrap_or_else(|| item.insert.chars().count());
            self.buffer.set_cursor(at, false);
        }
        self.after_edit(cx);
        self.resolve_completion_edits(&item, cx);
    }

    /// If the item carries no additional edits (auto import) but the server supports
    /// `completionItem/resolve`: resolve after insertion and apply the returned edits.
    pub(super) fn resolve_completion_edits(&mut self, item: &crate::lsp::CompletionItem, cx: &mut Context<Self>) {
        let (Some(client), Some(raw)) = (self.lsp.clone(), item.resolve.clone()) else { return };
        if !item.extra.is_empty() || client.capabilities["completionProvider"]["resolveProvider"].as_bool() != Some(true) {
            return;
        }
        // Let the server see the current document; if the document changed before the response arrived the positions are stale: not applied.
        self.sync_lsp();
        let version = self.buffer.version();
        let task = cx.background_executor().spawn(async move { client.request("completionItem/resolve", (*raw).clone(), std::time::Duration::from_secs(5)) });
        cx.spawn(async move |this, cx| {
            let Ok(v) = task.await else { return };
            if this.update(cx, |e, _| e.buffer.version() != version).unwrap_or(true) {
                return;
            }
            let edits: Vec<(crate::lsp::Range16, String)> = v["additionalTextEdits"]
                .as_array()
                .map(|a| a.iter().map(|x| (crate::lsp::parse_range(&x["range"]), x["newText"].as_str().unwrap_or_default().to_string())).collect())
                .unwrap_or_default();
            if !edits.is_empty() {
                this.update(cx, |e, cx| e.apply_lsp_edits(&edits, cx)).ok();
            }
        })
        .detach();
    }

    /// Format JSON/JSONC without a language server (`minify`: single line). `Ok(false)`: already that way.
    pub fn format_json(&mut self, minify: bool, cx: &mut Context<Self>) -> Result<bool, String> {
        let old = self.buffer.rope().to_string();
        let new = if minify { crate::jsonfmt::minify(&old)? } else { crate::jsonfmt::format(&old, &self.indent)? };
        let new = if minify { new } else { new.trim_end_matches('\n').to_string() + if old.ends_with('\n') || !old.contains('\n') { "\n" } else { "" } };
        if new == old {
            return Ok(false);
        }
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let len = self.buffer.len_chars();
        self.buffer.replace_all(vec![(0..len, new)]);
        let line = line.min(self.buffer.len_lines().saturating_sub(1));
        let at = self.buffer.line_to_char(line);
        self.buffer.set_cursor(at, false);
        self.after_edit(cx);
        Ok(true)
    }

    /// Tab: expand the Emmet abbreviation before the cursor (in HTML/JSX/CSS files).
    pub(super) fn try_emmet(&mut self, cx: &mut Context<Self>) -> bool {
        if !cx.global::<Settings>().emmet || self.buffer.selection().is_some() || self.buffer.cursor_count() != 1 {
            return false;
        }
        let ext = self.path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        let (css, jsx) = match ext.as_str() {
            "html" | "htm" | "xhtml" | "vue" | "php" | "svelte" | "xml" => (false, false),
            "jsx" | "tsx" => (false, true),
            "css" | "scss" | "less" => (true, false),
            _ => return false,
        };
        let (line, col) = self.buffer.cursor_line_col();
        let chars: Vec<char> = self.buffer.line(line).chars().take(col).collect();
        let (mut start, mut depth) = (chars.len(), 0usize);
        while start > 0 {
            let c = chars[start - 1];
            match c {
                ']' | '}' | ')' => depth += 1,
                '[' | '{' | '(' if depth == 0 => break,
                '[' | '{' | '(' => depth -= 1,
                _ if depth == 0 && (c.is_whitespace() || matches!(c, '<' | '"' | '\'' | '`' | ';' | ',')) => break,
                _ => {}
            }
            start -= 1;
        }
        let abbr: String = chars[start..].iter().collect();
        // Inside quotes (attribute value, string) do not expand: if the number of quotes is odd we are inside.
        let before = &chars[..start];
        let quotes = |q: char| before.iter().filter(|&&c| c == q).count();
        if !css && (quotes('"') % 2 == 1 || quotes('\'') % 2 == 1) {
            return false;
        }
        let expanded = if css {
            crate::emmet::expand_css(&abbr)
        } else {
            let first: String = abbr.chars().take_while(|c| !".#>+*[{(^".contains(*c)).collect();
            let special = first.len() < abbr.len() || abbr == "!";
            // In code files (JSX/PHP/Vue/Svelte) only those starting with a known tag: ordinary expressions like `this.state`
            // do not expand.
            let strict = jsx || matches!(ext.as_str(), "php" | "vue" | "svelte");
            let ok = if strict { first.is_empty() || crate::emmet::is_known_tag(&first) } else { true };
            let known = crate::emmet::is_known_tag(&abbr);
            if ok && (special || known) { crate::emmet::expand_html(&abbr, jsx) } else { None }
        };
        let Some(body) = expanded else { return false };
        let from = self.buffer.line_to_char(line) + start;
        self.expand_snippet_at(&body, from..from + abbr.chars().count(), cx);
        true
    }

    /// Expand the snippet body in place of `range`: following lines take the current indentation, `\t` becomes the indent
    /// unit, the first stop is selected; without `$0` the cursor ends at the end.
    pub(super) fn expand_snippet_at(&mut self, body: &str, range: Range<usize>, cx: &mut Context<Self>) {
        let line = self.buffer.char_to_line(range.start);
        let lead: String = self.buffer.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let indented = body.replace('\t', &self.indent).replace('\n', &format!("\n{lead}"));
        let (text, mut stops) = crate::lsp::parse_snippet(&indented);
        let len = text.chars().count();
        if !stops.iter().any(|(n, _)| *n == 0) {
            stops.push((0, len..len));
        }
        let start = range.start;
        self.buffer.replace_all(vec![(range, text)]);
        self.end_snippet();
        if stops.len() > 1 || stops.first().is_some_and(|(_, r)| !r.is_empty()) {
            let marks = stops.iter().flat_map(|(_, r)| [start + r.start, start + r.end]).collect();
            self.buffer.set_marks(SNIPPET_MARKS, marks);
            self.snippet_stop = Some(0);
            self.select_snippet_stop(0);
        } else {
            self.buffer.set_cursor(start + stops.first().map_or(len, |(_, r)| r.start), false);
        }
        self.after_edit(cx);
    }

    /// "Insert Snippet…": in place of the selection (or the cursor if none).
    pub fn insert_snippet(&mut self, body: &str, cx: &mut Context<Self>) {
        let range = self.buffer.selections().first().cloned().unwrap_or_else(|| {
            let c = self.buffer.cursor();
            c..c
        });
        self.expand_snippet_at(body, range, cx);
    }

    /// Tab: if the word immediately left of the cursor is a snippet prefix, expands it.
    pub(super) fn try_expand_prefix(&mut self, cx: &mut Context<Self>) -> bool {
        if self.buffer.cursor_count() > 1 || self.buffer.selected_text().is_some() {
            return false;
        }
        let cursor = self.buffer.cursor();
        let rope = self.buffer.rope();
        if cursor < rope.len_chars() && Self::is_word(rope.char(cursor)) {
            return false;
        }
        let n = (1..=40).take_while(|&k| k <= cursor && Self::is_word(rope.char(cursor - k))).count();
        if n == 0 {
            return false;
        }
        let prefix: String = (cursor - n..cursor).map(|i| rope.char(i)).collect();
        let Some(snip) = crate::snippets::find(&self.path, &prefix) else { return false };
        self.expand_snippet_at(&snip.body, cursor - n..cursor, cx);
        true
    }

    pub(super) fn select_snippet_stop(&mut self, i: usize) {
        let marks = self.buffer.marks(SNIPPET_MARKS).to_vec();
        if let (Some(&a), Some(&b)) = (marks.get(2 * i), marks.get(2 * i + 1)) {
            self.buffer.set_selection(a..b, false);
        }
    }

    /// Tab/Shift+Tab inside a snippet: next/previous stop. The session ends at the last stop.
    /// If there is no session, `false` (normal Tab).
    pub(super) fn snippet_jump(&mut self, forward: bool) -> bool {
        let Some(cur) = self.snippet_stop else { return false };
        let count = self.buffer.marks(SNIPPET_MARKS).len() / 2;
        let next = if forward { cur + 1 } else { cur.saturating_sub(1) };
        if next >= count {
            self.end_snippet();
            return false;
        }
        self.snippet_stop = Some(next);
        self.select_snippet_stop(next);
        if next + 1 == count {
            self.end_snippet(); // last stop ($0): stay there
        }
        true
    }

    pub(super) fn end_snippet(&mut self) {
        self.snippet_stop = None;
        self.buffer.set_marks(SNIPPET_MARKS, Vec::new());
    }

    // --- Hover and go to definition ---
}
