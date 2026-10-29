//! Editor: language server binding: diagnostics, hover, go to definition, hierarchy, formatting.

use super::*;
use crate::i18n::{t, tf};

impl EditorView {
    /// Connect to the server: open the document (full text), after that only changes are sent.
    pub fn set_lsp(&mut self, client: Option<Arc<crate::lsp::Client>>, lang: &'static str, cx: &mut Context<Self>) {
        if let (Some(old), true) = (&self.lsp, client.is_none()) {
            old.notify("textDocument/didClose", serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) } }));
        }
        self.lsp = client;
        self.lsp_lang = lang;
        // The server's diagnostics are sent; those from task output ("task") are independent of the server.
        self.diagnostics.retain(|d| d.source.as_deref() == Some("task"));
        self.reset_lsp_data();
        self.extras_task = None;
        self.hl_task = None;
        self.lsp_reported.clear();
        if let Some(c) = &self.lsp {
            self.buffer.track_lsp();
            self.buffer.take_lsp_changes();
            self.lsp_version = 1;
            c.notify(
                "textDocument/didOpen",
                serde_json::json!({ "textDocument": {
                    "uri": crate::lsp::path_to_uri(&self.path), "languageId": lang, "version": 1, "text": self.buffer.rope().to_string()
                } }),
            );
            self.schedule_lsp_extras(800, cx);
        }
        cx.notify();
    }

    /// Dim text at the line end: code lenses first, then hints.
    pub(super) fn eol_hint(&self, line: usize) -> Option<String> {
        // Values from the stopped program replace the hints (both on one line would be crowded).
        if let Some(v) = self.debug_values.get(&line) {
            return Some(v.clone());
        }
        let lens = self.lenses.get(&line).map(|l| l.iter().map(|x| x.title.as_str()).collect::<Vec<_>>().join(" | "));
        match (lens, self.inlays.get(&line)) {
            (Some(l), Some(h)) => Some(format!("{l}   {h}")),
            (Some(l), None) => Some(l),
            (None, Some(h)) => Some(h.clone()),
            (None, None) => None,
        }
    }

    /// Run the first code lens on the cursor's line.
    pub(super) fn run_code_lens(&mut self, _: &RunCodeLens, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let Some(lens) = self.lenses.get(&line).and_then(|l| l.first()).cloned() else {
            return cx.emit(EditorEvent::Message(t("No code lens on this line").into()));
        };
        let Some(client) = self.lsp.clone() else { return };
        let name = lens.command["command"].as_str().unwrap_or_default().to_string();
        let args = lens.command["arguments"].clone();
        // Reference-list lenses (`showReferences`): the client opens the location list.
        if name.ends_with("showReferences") || name.ends_with("show-references") {
            let locs = crate::lsp::parse_locations(&args[2]);
            if !locs.is_empty() {
                return cx.emit(EditorEvent::ShowLocations { locs, what: "references" });
            }
        }
        let task = cx.background_executor().spawn(async move {
            client.request(
                "workspace/executeCommand",
                serde_json::json!({ "command": name, "arguments": if args.is_array() { args } else { serde_json::json!([]) } }),
                std::time::Duration::from_secs(15),
            )
        });
        cx.spawn(async move |this, cx| {
            if let Err(m) = task.await {
                this.update(cx, |_, cx| cx.emit(EditorEvent::Message(m))).ok();
            }
        })
        .detach();
    }

    /// Symbol highlight: if the cursor is on a word, request `documentHighlight` from the server (after 250 ms).
    pub(super) fn schedule_symbol_highlight(&mut self, cx: &mut Context<Self>) {
        let cursor = self.buffer.cursor();
        // If the cursor is still inside the currently highlighted symbol, leave it as is.
        if !self.symbol_hl.iter().any(|r| r.start <= cursor && cursor <= r.end) {
            self.symbol_hl.clear();
        }
        let Some(client) = self.lsp.clone().filter(|c| c.supports("documentHighlightProvider")) else {
            self.hl_task = None;
            return;
        };
        if self.buffer.word_at(cursor).is_none() || self.buffer.cursor_count() != 1 {
            self.symbol_hl.clear();
            self.hl_task = None;
            return;
        }
        self.hl_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
            let Ok(Some(params)) = this.update(cx, |e, _| {
                e.sync_lsp();
                let (l, c) = e.buffer.lsp_position(e.buffer.cursor());
                Some(serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&e.path) }, "position": { "line": l, "character": c } }))
            }) else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { client.request("textDocument/documentHighlight", params, std::time::Duration::from_secs(4)) })
                .await;
            this.update(cx, |e, cx| {
                if let Ok(v) = result {
                    let mut ranges: Vec<Range<usize>> = v
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|h| crate::lsp::parse_range(&h["range"]))
                                .map(|((l0, c0), (l1, c1))| e.buffer.from_lsp_position(l0, c0)..e.buffer.from_lsp_position(l1, c1))
                                .filter(|r| r.start < r.end)
                                .collect()
                        })
                        .unwrap_or_default();
                    ranges.sort_by_key(|r| r.start);
                    e.symbol_hl = ranges;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Linked tag: if the cursor is on an HTML/XML/JSX tag name, mark the start of its counterpart.
    pub(super) fn update_linked_tag(&mut self, cx: &mut Context<Self>) {
        self.buffer.set_marks(LINK_MARKS, Vec::new());
        if !crate::linked_tag::applies_to(&self.path)
            || !cx.global::<Settings>().linked_editing
            || self.buffer.cursor_count() != 1
            || self.buffer.selection().is_some()
        {
            return;
        }
        let cursor = self.buffer.cursor();
        // Cheap pre-check: if there's no tag name on the same line, don't build a large window.
        let (line, col) = self.buffer.cursor_line_col();
        let line_chars: Vec<char> = self.buffer.line(line).chars().collect();
        if crate::linked_tag::name_at(&line_chars, col).is_none() {
            return;
        }
        let from = cursor.saturating_sub(60_000);
        let to = (cursor + 60_000).min(self.buffer.len_chars());
        let chars: Vec<char> = self.buffer.rope().slice(from..to).chars().collect();
        let Some((s, e, close)) = crate::linked_tag::name_at(&chars, cursor - from) else { return };
        let Some(p) = crate::linked_tag::partner(&chars, s, e, close) else { return };
        // The mark sits on the `<` / `/` before the name, not on the name itself (so typing at the start doesn't push it).
        self.buffer.set_marks(LINK_MARKS, vec![s + from - 1, p + from - 1]);
    }

    /// Mirror the typed tag name to its counterpart (VS Code `editor.linkedEditing`). The marks are
    /// on the `<` / `/` before the name, not at its start: typing at the start of the name doesn't push the mark away.
    pub(super) fn sync_linked_tag(&mut self) {
        let marks = self.buffer.marks(LINK_MARKS).to_vec();
        let [ga, gb] = marks[..] else { return };
        let cursor = self.buffer.cursor();
        let from = ga.min(gb).saturating_sub(2);
        let to = (ga.max(gb) + 100_000).min(self.buffer.len_chars());
        if to <= from || ga.max(gb) >= self.buffer.len_chars() {
            return;
        }
        let chars: Vec<char> = self.buffer.rope().slice(from..to).chars().collect();
        // Do the marks still point at the start of a tag name (a deletion may have swallowed the counterpart)?
        let name_start = |g: usize| {
            let i = g - from;
            match chars.get(i) {
                Some('<') if chars.get(i + 1) != Some(&'/') => Some(i + 1),
                Some('/') if i > 0 && chars[i - 1] == '<' => Some(i + 1),
                _ => None,
            }
        };
        let (Some(ra), Some(rb)) = (name_start(ga), name_start(gb)) else {
            self.buffer.set_marks(LINK_MARKS, Vec::new());
            return;
        };
        let (ea, eb) = (crate::linked_tag::name_end(&chars, ra), crate::linked_tag::name_end(&chars, rb));
        let (a, b) = (ra + from, rb + from);
        // If the names overlap or the cursor is outside the name, do nothing.
        if (a < b && ea + from >= b) || (b < a && eb + from >= a) || cursor < a || cursor > ea + from {
            return;
        }
        let (name_a, name_b) = (&chars[ra..ea], &chars[rb..eb]);
        if name_a != name_b {
            self.buffer.apply_edits(vec![(b..b + (eb - rb), name_a.iter().collect())]);
            // The typed letter and its mirror are a single undo step: undo/redo applies both together.
            self.buffer.chain_last_undo();
        }
    }

    /// Drop line-based data from the server and cancel in-flight requests (the document changed or
    /// the server went away). `lsp_epoch` bumps: stale requests don't apply their results.
    pub(super) fn reset_lsp_data(&mut self) {
        self.lsp_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (self.inlays, self.lsp_folds, self.links) = Default::default();
        (self.lenses, self.sem) = Default::default();
        self.symbol_hl.clear();
        self.display_key = None;
    }

    /// Folding ranges, line-end hints, links, code lenses and semantic colors from the language
    /// server. Waits `delay_ms` (not on every keystroke while typing); a new call cancels the previous one.
    pub fn schedule_lsp_extras(&mut self, delay_ms: u64, cx: &mut Context<Self>) {
        use std::sync::atomic::Ordering::SeqCst;
        let Some(client) = self.lsp.clone() else {
            self.extras_task = None;
            return;
        };
        // On a large file, hints only around the cursor; whole-document requests are skipped (expensive).
        let big = self.buffer.len_lines() > 3000;
        let s = cx.global::<Settings>();
        let want_inlay = s.inlay_hints && client.supports("inlayHintProvider");
        let want_fold = !big && client.supports("foldingRangeProvider");
        let want_links = !big && client.supports("documentLinkProvider");
        let want_lens = !big && s.code_lens && client.supports("codeLensProvider");
        let want_sem = !big && s.semantic_highlighting && client.supports("semanticTokensProvider");
        if !(want_inlay || want_fold || want_links || want_lens || want_sem) {
            self.extras_task = None;
            return;
        }
        let epoch = self.lsp_epoch.clone();
        let mine = epoch.fetch_add(1, SeqCst) + 1;
        self.extras_task = Some(cx.spawn(async move |this, cx| {
            if delay_ms > 0 {
                cx.background_executor().timer(std::time::Duration::from_millis(delay_ms)).await;
            }
            // Send accumulated edits to the server first, then compute the request parameters.
            let Ok(Some((uri, range, lines))) = this.update(cx, |e, _| {
                e.sync_lsp();
                let lines = e.buffer.len_lines();
                let (from, to) = if lines <= 3000 {
                    (0, e.buffer.len_chars())
                } else {
                    let l = e.buffer.char_to_line(e.buffer.cursor());
                    (e.buffer.line_to_char(l.saturating_sub(300)), e.buffer.line_to_char((l + 300).min(lines)))
                };
                let (a, b) = (e.buffer.lsp_position(from), e.buffer.lsp_position(to));
                let range = serde_json::json!({ "start": { "line": a.0, "character": a.1 }, "end": { "line": b.0, "character": b.1 } });
                Some((crate::lsp::path_to_uri(&e.path), range, lines))
            }) else {
                return;
            };
            let guard = epoch.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let t = std::time::Duration::from_secs(8);
                    let doc = serde_json::json!({ "uri": uri });
                    let live = || guard.load(SeqCst) == mine;
                    let mut errors: Vec<String> = Vec::new();
                    let mut ask = |method: &'static str, params: serde_json::Value| -> Option<serde_json::Value> {
                        if !live() {
                            return None; // document changed: no needless request
                        }
                        match client.request(method, params, t) {
                            Ok(v) => Some(v),
                            Err(e) => {
                                errors.push(format!("{method}: {e}"));
                                None
                            }
                        }
                    };
                    // A failed request doesn't keep the old data: an empty result is written.
                    let hints = want_inlay.then(|| {
                        ask("textDocument/inlayHint", serde_json::json!({ "textDocument": doc, "range": range }))
                            .map(|v| crate::lsp::parse_inlay_hints(&v))
                            .unwrap_or_default()
                    });
                    let folds = want_fold.then(|| {
                        ask("textDocument/foldingRange", serde_json::json!({ "textDocument": doc }))
                            .map(|v| crate::lsp::parse_folding_ranges(&v))
                            .unwrap_or_default()
                    });
                    let links = want_links.then(|| {
                        ask("textDocument/documentLink", serde_json::json!({ "textDocument": doc }))
                            .map(|v| crate::lsp::parse_document_links(&v))
                            .unwrap_or_default()
                    });
                    let lenses = want_lens.then(|| {
                        let (mut done, todo) = ask("textDocument/codeLens", serde_json::json!({ "textDocument": doc }))
                            .map(|v| crate::lsp::parse_code_lenses(&v))
                            .unwrap_or_default();
                        // Resolve lenses without commands: at most 10 (each is a separate request).
                        for item in todo.into_iter().take(10) {
                            if let Some(r) = ask("codeLens/resolve", item) {
                                done.extend(crate::lsp::parse_code_lenses(&serde_json::json!([r])).0);
                            }
                        }
                        done
                    });
                    let sem = want_sem.then(|| {
                        let legend = client.capabilities["semanticTokensProvider"]["legend"]["tokenTypes"].as_array().cloned().unwrap_or_default();
                        let legend: Vec<String> = legend.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect();
                        ask("textDocument/semanticTokens/full", serde_json::json!({ "textDocument": doc }))
                            .map(|v| crate::lsp::decode_semantic_tokens(&v["data"], &legend))
                            .unwrap_or_default()
                    });
                    (hints, folds, links, lenses, sem, errors)
                })
                .await;
            this.update(cx, |e, cx| {
                // If the document changed or the server went away meanwhile, the result is stale: drop it.
                if e.lsp_epoch.load(SeqCst) != mine {
                    return;
                }
                let (hints, folds, links, lenses, sem, errors) = result;
                e.lsp_lines = lines;
                for err in errors {
                    e.log_lsp_failure(err, cx);
                }
                if let Some(h) = hints {
                    let mut by_line: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
                    for hint in h {
                        let slot = by_line.entry(hint.line).or_default();
                        if !slot.is_empty() {
                            slot.push_str("  ");
                        }
                        slot.push_str(&hint.label);
                    }
                    e.inlays = by_line;
                }
                if let Some(f) = folds {
                    e.lsp_folds = f;
                    e.display_key = None;
                }
                if let Some(l) = lenses {
                    let mut by_line: std::collections::HashMap<usize, Vec<crate::lsp::Lens>> = std::collections::HashMap::new();
                    for lens in l {
                        by_line.entry(lens.line).or_default().push(lens);
                    }
                    e.lenses = by_line;
                }
                if let Some(tokens) = sem {
                    let mut by_line: std::collections::HashMap<usize, Vec<(u32, u32, Role)>> = std::collections::HashMap::new();
                    for t in tokens {
                        let role = match t.kind.as_str() {
                            "function" | "method" | "macro" | "decorator" => Role::Function,
                            "type" | "class" | "struct" | "enum" | "interface" | "typeParameter" | "namespace" => Role::Type,
                            "enumMember" => Role::Number,
                            _ => continue,
                        };
                        by_line.entry(t.line).or_default().push((t.start, t.len, role));
                    }
                    e.sem = by_line;
                }
                if let Some(l) = links {
                    e.links =
                        l.into_iter().map(|(((l0, c0), (l1, c1)), t)| (e.buffer.from_lsp_position(l0, c0)..e.buffer.from_lsp_position(l1, c1), t)).collect();
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// An LSP request failed: write it to the Output panel (at most once per method).
    pub(super) fn log_lsp_failure(&mut self, text: String, cx: &mut Context<Self>) {
        let method = text.split(':').next().unwrap_or_default().to_string();
        if self.lsp_reported.insert(method) {
            cx.emit(EditorEvent::Log(format!("{}: {text}", self.name)));
        }
    }

    /// Cmd-click: if there's a `documentLink` at the position, open its target (file or URL). Returns `true` if opened.
    pub(super) fn open_link_at(&mut self, ix: usize, cx: &mut Context<Self>) -> bool {
        let Some((_, target)) = self.links.iter().find(|(r, _)| r.contains(&ix)).cloned() else { return false };
        // `file:///x.ts#12,5` or `#L12`: the fragment gives the position.
        let (base, frag) = target.split_once('#').map_or((target.as_str(), ""), |(b, f)| (b, f));
        let (line, col) = {
            let f = frag.trim_start_matches('L');
            let mut it = f.split([',', ':', 'C']).filter(|p| !p.is_empty());
            let l = it.next().and_then(|p| p.trim_start_matches('L').parse::<u32>().ok()).map_or(0, |l| l.saturating_sub(1));
            let c = it.next().and_then(|p| p.parse::<u32>().ok()).map_or(0, |c| c.saturating_sub(1));
            (l, c)
        };
        if let Some(path) = crate::lsp::uri_to_path(base) {
            cx.emit(EditorEvent::OpenLocation { path, line, col });
        } else if target.starts_with("http://") || target.starts_with("https://") {
            cx.open_url(&target);
        } else {
            cx.emit(EditorEvent::Message(tf("Cannot open this link: {}", &[&target.chars().take(80).collect::<String>()])));
        }
        true
    }

    #[cfg(test)]
    pub fn lens_and_semantic_for_test(&self) -> (Vec<(usize, String)>, usize) {
        let mut l: Vec<(usize, String)> = self.lenses.iter().flat_map(|(k, v)| v.iter().map(move |x| (*k, x.title.clone()))).collect();
        l.sort();
        (l, self.sem.len())
    }

    #[cfg(test)]
    pub fn symbol_highlights_for_test(&self) -> Vec<Range<usize>> {
        self.symbol_hl.clone()
    }

    #[cfg(test)]
    pub fn lsp_extras_for_test(&self) -> (std::collections::HashMap<usize, String>, Vec<(usize, usize)>, usize) {
        (self.inlays.clone(), self.lsp_folds.clone(), self.links.len())
    }

    /// LSP (line, UTF-16 column) -> character index.
    pub fn lsp_char_index(&self, line: u32, col: u32) -> usize {
        self.buffer.from_lsp_position(line, col)
    }

    /// Text in a character range (`None` if the range is invalid).
    pub fn text_in(&self, r: Range<usize>) -> Option<String> {
        (r.start <= r.end && r.end <= self.buffer.len_chars()).then(|| self.buffer.rope().slice(r).to_string())
    }

    #[cfg(test)]
    pub fn peek_for_test(&self) -> Option<(u32, u32)> {
        self.peek.as_ref().map(|p| (p.line, p.col))
    }

    pub fn lsp_client(&self) -> Option<&Arc<crate::lsp::Client>> {
        self.lsp.as_ref()
    }

    /// For a language server request: (client, cursor, selection range, word under the cursor).
    /// Pending changes are sent first (so the request sees the current text).
    pub fn lsp_context(&mut self) -> Option<(Arc<crate::lsp::Client>, (u32, u32), crate::lsp::Range16, Option<String>)> {
        self.sync_lsp();
        let client = self.lsp.clone()?;
        let r = self.buffer.selection_or_cursor();
        let cursor = self.buffer.lsp_position(self.buffer.cursor());
        let range = (self.buffer.lsp_position(r.start), self.buffer.lsp_position(r.end));
        let word = self.buffer.word_at(self.buffer.cursor()).map(|w| self.buffer.text_range(w));
        Some((client, cursor, range, word))
    }

    /// Diagnostics intersecting the cursor (as context for a code action request).
    pub fn diagnostics_at_cursor(&self) -> Vec<serde_json::Value> {
        let line = self.buffer.cursor_line_col().0 as u32;
        self.diagnostics
            .iter()
            .filter(|d| d.start.0 <= line && line <= d.end.0)
            .map(|d| {
                serde_json::json!({
                    "range": { "start": { "line": d.start.0, "character": d.start.1 }, "end": { "line": d.end.0, "character": d.end.1 } },
                    "severity": d.severity, "message": d.message, "source": d.source,
                })
            })
            .collect()
    }

    /// Language server edits (formatting, rename): a single undo step.
    pub fn apply_lsp_edits(&mut self, edits: &[(crate::lsp::Range16, String)], cx: &mut Context<Self>) {
        if edits.is_empty() {
            return;
        }
        let parts = edits
            .iter()
            .map(|(((l0, c0), (l1, c1)), t)| (self.buffer.from_lsp_position(*l0, *c0)..self.buffer.from_lsp_position(*l1, *c1), t.clone()))
            .collect();
        self.buffer.apply_edits(parts);
        self.after_edit(cx);
    }

    /// The selection (or the cursor as an empty range) in UTF-16 positions, for extensions.
    pub fn lsp_selection(&self) -> crate::lsp::Range16 {
        let head = self.buffer.cursor();
        let r = self.buffer.selection().unwrap_or(head..head);
        (self.buffer.lsp_position(r.start), self.buffer.lsp_position(r.end))
    }

    /// Select `range` (UTF-16 positions), for extensions.
    pub fn set_lsp_selection(&mut self, range: crate::lsp::Range16, cx: &mut Context<Self>) {
        let ((l0, c0), (l1, c1)) = range;
        let (a, b) = (self.buffer.from_lsp_position(l0, c0), self.buffer.from_lsp_position(l1, c1));
        self.buffer.set_selection(a.min(b)..a.max(b), b < a);
        cx.notify();
    }

    /// Document version for language server requests (see `apply_lsp_edits_if_unchanged`).
    pub fn lsp_buffer_version(&self) -> u64 {
        self.buffer.version()
    }

    /// Applies if `version` (`buffer.version()` when the request was sent) is still current; if the document changed meanwhile the server's
    /// lines/columns would point elsewhere and corrupt the text, so it is dropped. Returns `true` if applied.
    pub fn apply_lsp_edits_if_unchanged(&mut self, version: u64, edits: &[(crate::lsp::Range16, String)], cx: &mut Context<Self>) -> bool {
        if self.buffer.version() != version {
            return false;
        }
        self.apply_lsp_edits(edits, cx);
        true
    }

    /// Formatting request (with the tab settings); applied when the result arrives.
    pub fn format_request(&mut self) -> Option<(Arc<crate::lsp::Client>, serde_json::Value)> {
        self.sync_lsp();
        let client = self.lsp.clone()?;
        let spaces = self.indent != "\t";
        let params = serde_json::json!({
            "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) },
            "options": { "tabSize": self.indent_unit(), "insertSpaces": spaces, "trimTrailingWhitespace": true },
        });
        Some((client, params))
    }

    /// Format the selection (the cursor's line if there is none): `textDocument/rangeFormatting`.
    pub fn format_selection_request(&mut self) -> Option<(Arc<crate::lsp::Client>, serde_json::Value)> {
        let (client, mut params) = self.format_request()?;
        let r = self.buffer.selection_or_cursor();
        let line = self.buffer.char_to_line(r.start);
        let (from, to) =
            if r.is_empty() { (self.buffer.line_to_char(line), self.buffer.line_to_char(line) + self.buffer.line_len_chars(line)) } else { (r.start, r.end) };
        let (a, b) = (self.buffer.lsp_position(from), self.buffer.lsp_position(to));
        params["range"] = serde_json::json!({ "start": { "line": a.0, "character": a.1 }, "end": { "line": b.0, "character": b.1 } });
        Some((client, params))
    }

    /// Report accumulated edits to the server (order is kept: requests come after).
    pub(super) fn sync_lsp(&mut self) {
        let Some(c) = &self.lsp else { return };
        let changes = self.buffer.take_lsp_changes();
        if changes.is_empty() {
            return;
        }
        self.lsp_version += 1;
        let content: Vec<_> = changes
            .into_iter()
            .map(|ch| {
                serde_json::json!({
                    "range": { "start": { "line": ch.start.0, "character": ch.start.1 }, "end": { "line": ch.end.0, "character": ch.end.1 } },
                    "text": ch.text,
                })
            })
            .collect();
        c.notify(
            "textDocument/didChange",
            serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&self.path), "version": self.lsp_version }, "contentChanges": content }),
        );
    }

    /// Buffer changed entirely (reloaded from disk): send the full text.
    pub(super) fn resync_lsp(&mut self) {
        let Some(c) = &self.lsp else { return };
        self.buffer.track_lsp();
        self.buffer.take_lsp_changes();
        self.lsp_version += 1;
        c.notify(
            "textDocument/didChange",
            serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&self.path), "version": self.lsp_version },
                "contentChanges": [{ "text": self.buffer.rope().to_string() }] }),
        );
    }

    /// The diagnostics covering character `ix` (an empty range counts as one character), most severe first.
    pub(super) fn diagnostics_at(&self, ix: usize) -> Vec<crate::lsp::Diagnostic> {
        let mut out: Vec<crate::lsp::Diagnostic> = self
            .diagnostics
            .iter()
            .filter(|d| {
                let (a, b) = (self.buffer.from_lsp_position(d.start.0, d.start.1), self.buffer.from_lsp_position(d.end.0, d.end.1));
                (a..b.max(a + 1)).contains(&ix)
            })
            .cloned()
            .collect();
        out.sort_by_key(|d| d.severity);
        out
    }

    /// The most severe diagnostic under the cursor (for *Problems: Copy Full Diagnostic*).
    pub fn diagnostic_at_cursor(&self) -> Option<crate::lsp::Diagnostic> {
        self.diagnostics_at(self.buffer.cursor()).into_iter().next()
    }

    pub fn set_lsp_root(&mut self, root: Option<String>) {
        self.lsp_root = root;
    }

    pub fn set_diagnostics(&mut self, diagnostics: Vec<crate::lsp::Diagnostic>, cx: &mut Context<Self>) {
        self.diagnostics = diagnostics;
        cx.notify();
    }

    #[cfg(test)]
    pub fn diagnostics(&self) -> &[crate::lsp::Diagnostic] {
        &self.diagnostics
    }

    /// Byte ranges of the diagnostics on this line (an empty range counts as one character).
    pub(super) fn line_diagnostics(&self, line: usize, text: &str) -> Vec<(Range<usize>, u8)> {
        let n = text.chars().count();
        self.diagnostics
            .iter()
            .filter(|d| d.start.0 as usize <= line && d.end.0 as usize >= line)
            .map(|d| {
                let col = |(l, c): (u32, u32)| -> usize {
                    if (l as usize) < line {
                        0
                    } else if (l as usize) > line {
                        n
                    } else {
                        self.buffer.from_lsp_position(l, c) - self.buffer.line_to_char(line)
                    }
                };
                let (mut a, mut b) = (col(d.start), col(d.end));
                if a >= b {
                    // Empty range: mark one character (the last one if at the line end).
                    a = a.min(n.saturating_sub(1));
                    b = (a + 1).min(n.max(1));
                }
                (byte_of(text, a)..byte_of(text, b), d.severity)
            })
            .filter(|(r, _)| r.start < r.end)
            .collect()
    }

    // --- Completion ---

    /// When the mouse rests on a word (half a second), show info.
    pub(super) fn schedule_hover(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !cx.global::<Settings>().hover_enabled {
            self.hover = None;
            self.hover_task = None;
            return;
        }
        let Some(ix) = self.index_for_position(position).filter(|&ix| self.over_text(position, ix)) else {
            self.hover = None;
            self.hover_task = None;
            return;
        };
        let word = self.buffer.word_at(ix);
        if let Some(h) = &self.hover {
            if word.as_ref().is_some_and(|w| w.contains(&h.at)) || h.at == ix {
                return; // same word: leave it
            }
            self.hover = None;
            cx.notify();
        }
        let mut diagnostics = self.diagnostics_at(ix);
        if let Some((_, word)) = self.misspelling_at(ix, cx) {
            let message = tf("“{}” is not in the dictionary. Quick Fix shows suggestions.", &[&word]);
            diagnostics.push(crate::lsp::Diagnostic { severity: 3, message, ..Default::default() });
        }
        let client = self.lsp.clone();
        let debug = self.debug_eval.clone().zip(self.debug_expression_at(ix));
        let schema_hover = self.schema_hover_input(ix, client.is_some());
        if client.is_none() && diagnostics.is_empty() && debug.is_none() && schema_hover.is_none() {
            self.hover_task = None;
            return;
        }
        let (line, col) = self.buffer.lsp_position(ix);
        let uri = crate::lsp::path_to_uri(&self.path);
        self.hover_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(450)).await;
            // If the program is stopped: the expression's value (DAP `evaluate`, context "hover") goes before the server's description.
            let value = match debug {
                Some(((session, frame), expr)) => {
                    cx.background_executor()
                        .spawn(async move {
                            session.evaluate(Some(frame), &expr, "hover").ok().filter(|(v, _)| !v.is_empty()).map(|(v, _)| format!("{expr} = {v}"))
                        })
                        .await
                }
                None => None,
            };
            let text = match client {
                Some(c) => {
                    let params = serde_json::json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": col } });
                    cx.background_executor()
                        .spawn(async move { c.request("textDocument/hover", params, std::time::Duration::from_secs(5)) })
                        .await
                        .ok()
                        .and_then(|v| crate::lsp::hover_text(&v))
                }
                None => match schema_hover {
                    Some(input) => cx.background_executor().spawn(async move { input.run() }).await,
                    None => None,
                },
            };
            let text = match (value, text) {
                (Some(v), Some(t)) => Some(format!("{v}\n\n{t}")),
                (Some(v), None) => Some(v),
                (None, t) => t,
            };
            this.update(cx, |e, cx| {
                if text.is_some() || !diagnostics.is_empty() {
                    e.hover = Some(Hover { at: ix, diagnostics, text, position });
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Is the position over text in the line (not whitespace to the right of the line end).
    pub(super) fn over_text(&self, position: Point<Pixels>, ix: usize) -> bool {
        self.visible
            .iter()
            .find(|v| position.y >= v.bounds.top() && position.y < v.bounds.bottom())
            .is_some_and(|v| ix >= v.start && ix <= v.start + v.text.chars().count() && position.x - v.text_left < v.layout.x_for_index(v.text.len()))
    }

    pub(super) fn show_hover(&mut self, _: &ShowHover, _: &mut Window, cx: &mut Context<Self>) {
        let Some((v, x)) = self.screen_pos(self.buffer.cursor()) else { return };
        let at = point(x + px(1.), v.bounds.top() + v.bounds.size.height / 2.);
        self.hover = None;
        self.schedule_hover(at, cx);
    }

    pub(super) fn go_to_definition(&mut self, _: &GoToDefinition, _: &mut Window, cx: &mut Context<Self>) {
        self.definition_at(self.buffer.cursor(), cx);
    }

    pub(super) fn go_to_type_definition(&mut self, _: &GoToTypeDefinition, _: &mut Window, cx: &mut Context<Self>) {
        self.locate(self.buffer.cursor(), "textDocument/typeDefinition", "type definition", cx);
    }

    pub(super) fn go_to_implementation(&mut self, _: &GoToImplementation, _: &mut Window, cx: &mut Context<Self>) {
        self.locate(self.buffer.cursor(), "textDocument/implementation", "implementations", cx);
    }

    pub(super) fn definition_at(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.locate(ix, "textDocument/definition", "definition", cx);
    }

    /// LSP request returning locations: a single result jumps there, several are listed.
    pub(super) fn locate(&mut self, ix: usize, method: &'static str, what: &'static str, cx: &mut Context<Self>) {
        let Some(client) = self.lsp.clone() else {
            cx.emit(EditorEvent::Message(t("No language server for this file").into()));
            return;
        };
        self.sync_lsp();
        let (line, col) = self.buffer.lsp_position(ix);
        let params = serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) }, "position": { "line": line, "character": col } });
        let task = cx.background_executor().spawn(async move { client.request(method, params, std::time::Duration::from_secs(10)) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |_, cx| match result.map(|v| crate::lsp::parse_locations(&v)) {
                Ok(locs) if locs.len() == 1 || (what == "definition" && !locs.is_empty()) => {
                    let (path, (line, col)) = locs[0].clone();
                    cx.emit(EditorEvent::OpenLocation { path, line, col });
                }
                Ok(locs) if !locs.is_empty() => cx.emit(EditorEvent::ShowLocations { locs, what }),
                Ok(_) => cx.emit(EditorEvent::Message(crate::lsp::none_found(what))),
                Err(e) => cx.emit(EditorEvent::Message(e)),
            })
            .ok();
        })
        .detach();
    }

    #[cfg(test)]
    pub fn completion_labels(&self) -> Option<Vec<String>> {
        self.completion.as_ref().map(|c| c.shown.iter().map(|&i| c.items[i].label.clone()).collect())
    }

    /// Number of popups (those whose position can be computed; for tests).
    #[cfg(test)]
    pub fn popup_count(&mut self, cx: &mut Context<Self>) -> usize {
        let p = *palette(cx);
        self.popups(&p, MONO_FONT.into(), 14., 20., cx).len()
    }

    #[cfg(test)]
    pub fn blame_for_test(&self) -> Option<(usize, String)> {
        self.blame.as_ref().map(|(l, _, t)| (*l, t.to_string()))
    }

    #[cfg(test)]
    pub fn hover_text(&self) -> Option<String> {
        self.hover.as_ref().map(|h| {
            let mut parts: Vec<String> = h.diagnostics.iter().map(|d| d.message.clone()).collect();
            parts.extend(h.text.clone());
            parts.join("\n")
        })
    }

    /// Alt+F12: show the definition in a small card without going to the target file.
    pub(super) fn peek_definition(&mut self, _: &PeekDefinition, _: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.lsp.clone() else {
            return cx.emit(EditorEvent::Message(t("No language server for this file").into()));
        };
        self.sync_lsp();
        let (line, col) = self.buffer.lsp_position(self.buffer.cursor());
        let params = serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) }, "position": { "line": line, "character": col } });
        let host = self.host.clone();
        self.peek_seq += 1;
        let seq = self.peek_seq;
        // Definition in the same file: read from the unsaved buffer (the lines on disk may have shifted).
        let (own_path, own_text) = (self.path.clone(), self.buffer.rope().to_string());
        let task = cx.background_executor().spawn(async move {
            let locs = crate::lsp::parse_locations(&client.request("textDocument/definition", params, std::time::Duration::from_secs(10))?);
            let Some((path, (l, c))) = locs.first().cloned() else { return Ok(None) };
            let data = if path == own_path { own_text.into_bytes() } else { host.read(&path).map_err(|e| e.to_string())?.0 };
            let text = String::from_utf8_lossy(&data);
            let first = (l as usize).saturating_sub(3);
            let lines: Vec<String> = text.lines().skip(first).take(16).map(str::to_string).collect();
            Ok::<_, String>(Some(Peek { path, line: l, col: c, first, lines, more: locs.len() - 1 }))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |e, cx| match result {
                // If the cursor moved or another peek was requested, the result is stale.
                _ if e.peek_seq != seq => {}
                Ok(Some(p)) => {
                    e.peek = Some(p);
                    cx.notify();
                }
                Ok(None) => cx.emit(EditorEvent::Message(crate::lsp::none_found("definition"))),
                Err(m) => cx.emit(EditorEvent::Message(m)),
            })
            .ok();
        })
        .detach();
    }

    /// Call / type hierarchy: first the cursor's item (`prepare`), then its neighbors via `method`;
    /// results open in the location list.
    pub(super) fn hierarchy(
        &mut self,
        capability: &'static str,
        prepare: &'static str,
        method: &'static str,
        key: Option<&'static str>,
        what: &'static str,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.lsp.clone().filter(|c| c.supports(capability)) else {
            return cx.emit(EditorEvent::Message(crate::lsp::unsupported(what)));
        };
        self.sync_lsp();
        let (line, col) = self.buffer.lsp_position(self.buffer.cursor());
        let params = serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) }, "position": { "line": line, "character": col } });
        let task = cx.background_executor().spawn(async move {
            let t = std::time::Duration::from_secs(15);
            let items = client.request(prepare, params, t)?;
            let Some(item) = items.as_array().and_then(|a| a.first()).cloned() else { return Ok(Vec::new()) };
            let result = client.request(method, serde_json::json!({ "item": item }), t)?;
            Ok::<_, String>(crate::lsp::parse_hierarchy(&result, key))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |_, cx| match result {
                Ok(locs) if !locs.is_empty() => cx.emit(EditorEvent::ShowLocations { locs, what }),
                Ok(_) => cx.emit(EditorEvent::Message(crate::lsp::none_found(what))),
                Err(m) => cx.emit(EditorEvent::Message(m)),
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn show_incoming_calls(&mut self, _: &ShowIncomingCalls, _: &mut Window, cx: &mut Context<Self>) {
        self.hierarchy("callHierarchyProvider", "textDocument/prepareCallHierarchy", "callHierarchy/incomingCalls", Some("from"), "incoming calls", cx);
    }

    pub(super) fn show_outgoing_calls(&mut self, _: &ShowOutgoingCalls, _: &mut Window, cx: &mut Context<Self>) {
        self.hierarchy("callHierarchyProvider", "textDocument/prepareCallHierarchy", "callHierarchy/outgoingCalls", Some("to"), "outgoing calls", cx);
    }

    pub(super) fn show_supertypes(&mut self, _: &ShowSupertypes, _: &mut Window, cx: &mut Context<Self>) {
        self.hierarchy("typeHierarchyProvider", "textDocument/prepareTypeHierarchy", "typeHierarchy/supertypes", None, "supertypes", cx);
    }

    pub(super) fn show_subtypes(&mut self, _: &ShowSubtypes, _: &mut Window, cx: &mut Context<Self>) {
        self.hierarchy("typeHierarchyProvider", "textDocument/prepareTypeHierarchy", "typeHierarchy/subtypes", None, "subtypes", cx);
    }

    /// Send a formatting request; when the reply arrives, apply the edits if the document hasn't changed
    /// (edits computed against old positions must not corrupt other text).
    pub(super) fn apply_lsp_format(&mut self, method: &'static str, client: Arc<crate::lsp::Client>, params: serde_json::Value, cx: &mut Context<Self>) {
        let epoch = self.lsp_epoch.load(std::sync::atomic::Ordering::SeqCst);
        let task = cx.background_executor().spawn(async move { client.request(method, params, std::time::Duration::from_secs(2)) });
        cx.spawn(async move |this, cx| {
            let Ok(v) = task.await else { return };
            this.update(cx, |e, cx| {
                if e.lsp_epoch.load(std::sync::atomic::Ordering::SeqCst) != epoch {
                    return;
                }
                let edits: Vec<(crate::lsp::Range16, String)> = v
                    .as_array()
                    .map(|a| a.iter().map(|x| (crate::lsp::parse_range(&x["range"]), x["newText"].as_str().unwrap_or_default().to_string())).collect())
                    .unwrap_or_default();
                e.apply_lsp_edits(&edits, cx);
            })
            .ok();
        })
        .detach();
    }
}
