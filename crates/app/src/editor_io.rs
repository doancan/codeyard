//! Editor: saving, syncing with disk, encoding, reloading.

use super::*;

/// How long after the conflict banner appears a save still counts as an accident.
const CONFLICT_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

impl EditorView {
    pub(super) fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.save_now(cx);
    }

    /// Save. With a language server, first the `editor.codeActionsOnSave` actions (one request per
    /// kind, syncing the document in between), then `editor.formatOnSave` runs: with the extension chosen to format
    /// the file (`editor.formatters`), else with the language server. Each step is skipped if it does not answer in
    /// time (2 s for a server, the provider deadline for an extension): the file is saved in every case.
    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        // The watcher can raise the banner an instant before a habitual Cmd+S lands; only a press after the user had time to read it overwrites.
        if self.conflict && self.conflict_at.is_some_and(|at| at.elapsed() < CONFLICT_GRACE) {
            return;
        }
        // Cmd+S again while the conflict banner is open: overwrite on purpose.
        let force = self.conflict;
        let (kinds, format_on_save) = {
            let s = cx.global::<Settings>();
            (
                self.folder.as_ref().and_then(|f| f.code_actions_on_save.clone()).unwrap_or_else(|| s.code_actions_on_save.clone()),
                self.scoped(|f| f.format_on_save, s.format_on_save),
            )
        };
        let ext_format = self.ext.save_formatter.clone().filter(|_| format_on_save);
        if (self.lsp.is_none() && ext_format.is_none()) || (kinds.is_empty() && !format_on_save) {
            return self.write_to_disk(force, cx);
        }
        cx.spawn(async move |this, cx| {
            for kind in kinds {
                let Ok(Some((client, params, version))) =
                    this.update(cx, |e, _| e.code_action_on_save_request(&kind).map(|(c, p)| (c, p, e.lsp_buffer_version())))
                else {
                    continue;
                };
                let edit = cx.background_executor().spawn(async move { source_action_edit(&client, params, &kind) }).await;
                let Some(edit) = edit else { continue };
                this.update(cx, |e, cx| {
                    let (files, _) = crate::lsp::parse_workspace_edit(&edit);
                    if let Some((_, edits)) = files.into_iter().find(|(p, _)| *p == e.path) {
                        // If typing continued while the server was answering, the positions now point elsewhere: do not apply, save as is.
                        e.apply_lsp_edits_if_unchanged(version, &edits, cx);
                    }
                })
                .ok();
            }
            let asked = match &ext_format {
                Some(f) => this.update(cx, |e, _| e.format_job()).ok().and_then(|job| {
                    let version = job.version;
                    f(job, cx).map(|answer| (version, answer))
                }),
                None => None,
            };
            if let Some((version, answer)) = asked {
                if let Ok(edits) = answer.await {
                    this.update(cx, |e, cx| e.apply_lsp_edits_if_unchanged(version, &edits, cx)).ok();
                }
            } else if format_on_save {
                let format = this.update(cx, |e, _| e.format_request().map(|(c, p)| (c, p, e.lsp_buffer_version()))).ok().flatten();
                if let Some((client, params, version)) = format {
                    let result = cx
                        .background_executor()
                        .spawn(async move { client.request("textDocument/formatting", params, std::time::Duration::from_secs(2)) })
                        .await;
                    if let Ok(v) = result {
                        let edits: Vec<(crate::lsp::Range16, String)> = v
                            .as_array()
                            .map(|a| a.iter().map(|x| (crate::lsp::parse_range(&x["range"]), x["newText"].as_str().unwrap_or_default().to_string())).collect())
                            .unwrap_or_default();
                        this.update(cx, |e, cx| e.apply_lsp_edits_if_unchanged(version, &edits, cx)).ok();
                    }
                }
            }
            this.update(cx, |e, cx| e.write_to_disk(force, cx)).ok();
        })
        .detach();
    }

    /// `textDocument/codeAction` request (whole document) for a single action kind to run on save.
    pub(super) fn code_action_on_save_request(&mut self, kind: &str) -> Option<(Arc<crate::lsp::Client>, serde_json::Value)> {
        self.sync_lsp();
        let client = self.lsp.clone()?;
        let end = self.buffer.lsp_position(self.buffer.len_chars());
        let params = serde_json::json!({
            "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": end.0, "character": end.1 } },
            "context": { "diagnostics": [], "only": [kind], "triggerKind": 2 },
        });
        Some((client, params))
    }

    /// Edits computed against the buffer (replace in files): a single undo step.
    pub fn apply_buffer_edits(&mut self, f: impl FnOnce(&Buffer) -> Vec<(Range<usize>, String)>, cx: &mut Context<Self>) -> usize {
        let edits = f(&self.buffer);
        let n = edits.len();
        if n > 0 {
            self.buffer.apply_edits(edits);
            self.after_edit(cx);
        }
        n
    }

    /// The text as it reads in the editor (`\n` line ends, no BOM). Structural search parses unsaved files from it.
    pub fn buffer_text(&self) -> String {
        self.buffer.rope().to_string()
    }

    /// Lines (for the preview in the references list).
    pub fn text_lines(&self) -> Vec<String> {
        (0..self.buffer.len_lines()).map(|l| self.buffer.line(l)).collect()
    }

    /// The whole text (tests; extensions reading an open document).
    pub fn text(&self) -> String {
        String::from_utf8(self.buffer.to_bytes()).unwrap_or_default()
    }

    /// On-screen position of a character offset (per the last paint; for tests).
    #[cfg(test)]
    pub fn point_for(&self, ix: usize) -> Option<Point<Pixels>> {
        let (v, x) = self.screen_pos(ix)?;
        Some(point(x + px(1.), v.bounds.top() + v.bounds.size.height / 2.))
    }

    #[cfg(test)]
    pub fn hscroll(&self) -> f32 {
        self.hscroll / px(1.)
    }

    #[cfg(test)]
    pub fn cursors(&self) -> Vec<usize> {
        self.buffer.cursors()
    }

    /// QA: all selection ranges (empty selection = cursor).
    #[cfg(test)]
    pub fn qa_selections(&self) -> Vec<Range<usize>> {
        self.buffer.selections()
    }

    /// QA: the selected text of each cursor (empty selection = empty string).
    #[cfg(test)]
    pub fn selected_text_all(&self) -> Vec<String> {
        self.buffer.selected_texts()
    }

    /// First and last buffer line visible in the last paint.
    pub fn visible_lines(&self) -> Option<(usize, usize)> {
        Some((self.visible.first()?.line, self.visible.last()?.line))
    }

    /// Accessibility scroll action: move the view `pages` screens down (up when negative), the caret stays.
    pub fn a11y_scroll(&mut self, pages: isize, cx: &mut Context<Self>) {
        let Some((first, last)) = self.visible_lines() else { return };
        let page = (last - first).max(1) as isize;
        let max = self.buffer.len_lines().saturating_sub(1) as isize;
        let row = self.row_of_line((first as isize + pages * page).clamp(0, max) as usize);
        self.scroll.scroll_to_item_strict(row, ScrollStrategy::Top);
        cx.notify();
    }

    #[cfg(test)]
    pub fn disk_state(&self) -> (bool, bool) {
        (self.conflict, self.deleted_on_disk)
    }

    // --- Session and backup ---

    pub fn version(&self) -> u64 {
        self.buffer.version()
    }

    /// The document's exact state: buffer epoch, undo version and a hash of its text.
    pub fn doc_identity(&self) -> super::DocIdentity {
        super::DocIdentity { epoch: self.epoch, version: self.buffer.version(), hash: proto::content_hash(self.buffer_text().as_bytes()) }
    }

    /// File was opened with mixed line endings (saving normalizes to a single style).
    pub fn has_mixed_line_endings(&self) -> bool {
        self.buffer.has_mixed_line_endings()
    }

    /// Name of the line-ending style to use when saving.
    pub fn line_ending_name(&self) -> &'static str {
        match self.buffer.line_ending() {
            buffer::LineEnding::Lf => "LF",
            buffer::LineEnding::Crlf => "CRLF",
            buffer::LineEnding::Cr => "CR",
        }
    }

    pub fn disk_hash(&self) -> u64 {
        self.disk_hash
    }

    pub fn content_bytes(&self) -> Vec<u8> {
        self.buffer.to_bytes()
    }

    /// Cursor and the topmost visible line.
    pub fn view(&self) -> state::View {
        let (line, column) = self.buffer.cursor_line_col();
        let line_h = self.visible.first().map_or(px(20.), |v| v.bounds.size.height);
        let top = -self.scroll.0.borrow().base_handle.offset().y;
        let row = (top / line_h).max(0.) as usize;
        state::View { line, column, scroll_line: self.display.line_at_row(row).0 }
    }

    /// Snapshot a group's primary cursor and scroll without changing its input home.
    pub fn view_for_slot(&mut self, slot: usize) -> state::View {
        let home = self.slot;
        self.enter(slot);
        let view = self.view();
        self.enter(home);
        view
    }

    /// Runs `f` in the `slot` view (its cursors, find bar and popups) without changing the input home.
    #[cfg_attr(not(any(test, target_os = "macos", windows)), allow(dead_code))]
    pub fn in_slot<R>(&mut self, slot: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let home = self.slot;
        self.enter(slot);
        let r = f(self);
        self.enter(home);
        r
    }

    pub fn set_view_for_slot(&mut self, slot: usize, view: state::View) {
        let home = self.slot;
        self.enter(slot);
        self.set_view(view);
        self.enter(home);
    }

    pub fn set_view(&mut self, v: state::View) {
        let line = v.line.min(self.buffer.len_lines().saturating_sub(1));
        let at = self.buffer.line_to_char(line) + v.column.min(self.buffer.line_len_chars(line));
        self.buffer.set_cursor(at, false);
        let row = self.row_of_line(v.scroll_line.min(self.buffer.len_lines().saturating_sub(1)));
        self.scroll.scroll_to_item_strict(row, ScrollStrategy::Top);
    }

    /// Put the unsaved content from the backup over the on-disk version: the file
    /// becomes dirty, undo returns to the on-disk version. If the disk also changed
    /// after the backup, that is a conflict: let the user decide.
    pub fn apply_backup(&mut self, bytes: &[u8], base_hash: u64, cx: &mut Context<Self>) {
        let Ok(text) = std::str::from_utf8(bytes) else { return };
        // The backup is taken with `to_bytes()`: it contains the BOM and the file's own line endings. It must enter the rope with LF and no BOM
        // (same rule as `Buffer::from_str`); otherwise the restored text would start with a real U+FEFF and the next save
        // would write a second BOM, and for CR-only files raw `\r` characters would remain in the rope.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let text = match self.buffer.line_ending() {
            buffer::LineEnding::Cr => text.replace('\r', "\n"),
            _ => text.replace("\r\n", "\n"),
        };
        let n = self.buffer.len_chars();
        self.buffer.replace(0..n, &text, false);
        self.buffer.set_cursor(0, false);
        self.sync_syntax();
        if base_hash != self.disk_hash {
            self.flag_conflict();
        }
        cx.notify();
    }

    fn flag_conflict(&mut self) {
        if !self.conflict {
            self.conflict_at = Some(std::time::Instant::now());
        }
        self.conflict = true;
    }

    /// Tests: let the grace period after a conflict pass without sleeping.
    #[cfg(test)]
    pub fn expire_conflict_grace(&mut self) {
        self.conflict_at = None;
    }

    pub fn mark_deleted_on_disk(&mut self) {
        self.deleted_on_disk = true;
    }

    pub fn uses_host(&self, host: &Arc<dyn Host>) -> bool {
        std::ptr::addr_eq(Arc::as_ptr(&self.host), Arc::as_ptr(host))
    }

    /// Whether `host` is the same machine as this file's (this computer, or the same SSH host), even through another
    /// connection (another window's).
    pub fn on_same_machine(&self, host: &Arc<dyn Host>) -> bool {
        self.uses_host(host) || (self.host.is_remote() == host.is_remote() && (!host.is_remote() || self.host.label() == host.label()))
    }

    /// The watcher reported that the file changed externally.
    pub fn on_disk_change(&mut self, kind: proto::ChangeKind, cx: &mut Context<Self>) {
        if kind == proto::ChangeKind::Removed {
            self.deleted_on_disk = true;
            cx.notify();
            return;
        }
        if self.saving {
            return; // our own save
        }
        self.deleted_on_disk = false;
        let host = self.host.clone();
        let path = self.path.clone();
        let fallback = cx.global::<Settings>().fallback_encoding;
        let task = cx.background_executor().spawn(async move { load(host.as_ref(), &path, fallback) });
        cx.spawn(async move |this, cx| {
            let Ok(loaded) = task.await else { return };
            this.update(cx, |this, cx| {
                if loaded.hash == this.disk_hash || this.saving {
                    return; // same content (our own save, or only the timestamp changed)
                }
                if this.is_dirty() {
                    this.flag_conflict(); // let the user decide
                } else {
                    this.replace_buffer(loaded);
                    this.resync_lsp();
                    this.schedule_lsp_extras(300, cx);
                    this.schedule_git_marks(0, Some(cx));
                }
                cx.emit(EditorEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Load the on-disk version; the cursor stays at the same line/column.
    pub(super) fn replace_buffer(&mut self, loaded: Loaded) {
        let (line, col) = self.buffer.cursor_line_col();
        let Loaded { mut buffer, hash, syntax, encoding, rules } = loaded;
        if syntax.is_some() {
            buffer.track_edits();
        }
        let line = line.min(buffer.len_lines().saturating_sub(1));
        let at = buffer.line_to_char(line) + col.min(buffer.line_len_chars(line));
        buffer.set_cursor(at, false);
        self.saved_version = buffer.version();
        self.saved_line_ending = buffer.line_ending();
        self.buffer = buffer;
        self.epoch = super::next_epoch();
        self.syntax = syntax;
        self.encoding = encoding;
        self.rules = rules;
        self.apply_language_override();
        // Document changed: the column box and the line-based data from the server are stale.
        self.column = None;
        self.column_drag = false;
        self.reset_lsp_data();
        self.disk_hash = hash;
        self.conflict = false;
        self.marked = None;
        // The new buffer's version starts from scratch: do not mix it up with the old version.
        self.conflicts_version = u64::MAX;
        self.refresh_conflicts();
    }

    /// Auto save: if dirty and there is no conflict.
    pub fn autosave_now(&mut self, cx: &mut Context<Self>) {
        if self.is_dirty() && !self.conflict && !self.saving {
            self.write_to_disk(false, cx);
        }
    }

    /// A build must never use the explicit second-save overwrite behavior of a conflicted editor.
    pub fn save_for_build(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if let Some(error) = self.build_save_error().filter(|_| self.conflict || self.deleted_on_disk) {
            return Err(error);
        }
        if !self.saving {
            self.error = None;
            self.save_now(cx);
        }
        Ok(())
    }

    pub fn build_save_error(&self) -> Option<String> {
        if self.conflict {
            Some(crate::i18n::t("Resolve the file conflict before debugging.").to_string())
        } else if self.deleted_on_disk {
            Some(crate::i18n::t("This file was deleted on disk. Save it explicitly before debugging.").to_string())
        } else {
            self.error.as_ref().map(|error| error.to_string())
        }
    }

    /// Tests: what the banner above the editor says ("conflict", "deleted" or the error text).
    #[cfg(test)]
    pub fn status_for_test(&self) -> Option<String> {
        if self.conflict {
            Some("conflict".into())
        } else if self.deleted_on_disk {
            Some("deleted".into())
        } else {
            self.error.as_ref().map(|e| e.to_string())
        }
    }

    pub(super) fn write_to_disk(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let s = cx.global::<Settings>();
        let ec = s.editorconfig;
        let (trim, final_nl) = (
            ec.then_some(self.rules.trim_trailing_whitespace).flatten().unwrap_or(self.scoped(|f| f.trim_trailing_whitespace, s.trim_trailing_whitespace)),
            ec.then_some(self.rules.insert_final_newline).flatten().unwrap_or(self.scoped(|f| f.insert_final_newline, s.insert_final_newline)),
        );
        if let Some(eol) = ec.then_some(self.rules.eol).flatten()
            && self.buffer.line_ending() != eol
        {
            self.buffer.set_line_ending(eol);
        }
        if trim {
            self.buffer.trim_trailing_whitespace();
        }
        if final_nl {
            self.buffer.ensure_final_newline();
        }
        if trim || final_nl {
            self.sync_syntax();
        }
        let data = match self.encoded() {
            Ok(d) => d,
            Err(ch) => {
                self.error = Some(
                    crate::i18n::tf(
                        "Not saved: {0} (U+{1}) cannot be written in {2}. Remove it, or save a copy as UTF-8.",
                        &[&format!("\"{ch}\""), &format!("{:04X}", ch as u32), &self.encoding.label()],
                    )
                    .into(),
                );
                cx.notify();
                return;
            }
        };
        self.saving = true;
        self.error = None;
        let version = self.buffer.version();
        let line_ending = self.buffer.line_ending();
        // A file known to be deleted is saved back as new; otherwise the agent refuses to recreate a vanished file.
        let expect = (!force && !self.deleted_on_disk).then_some(self.disk_hash);
        let (known, fallback) = (self.disk_hash, cx.global::<Settings>().fallback_encoding);
        let host = self.host.clone();
        let path = self.path.clone();
        let task = cx.background_executor().spawn(async move {
            // A forced write destroys whatever is on disk now, which the buffer never saw: read it in the same step so nothing can slip in between.
            let replaced = force
                .then(|| host.read(&path).ok())
                .flatten()
                .filter(|(_, hash)| *hash != known)
                .and_then(|(bytes, _)| crate::encoding::decode(&bytes, fallback))
                .map(|(text, _)| text);
            (host.write(&path, data, expect), replaced)
        });
        cx.spawn(async move |this, cx| {
            let (result, replaced) = task.await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(hash) => {
                        if let Some(text) = replaced {
                            cx.emit(EditorEvent::Overwrote(text));
                        }
                        this.disk_hash = hash;
                        this.saved_version = version;
                        this.saved_line_ending = line_ending;
                        this.conflict = false;
                        this.deleted_on_disk = false;
                        if let Some(c) = this.lsp.as_ref().filter(|c| c.wants_save()) {
                            c.notify("textDocument/didSave", serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&this.path) } }));
                        }
                        cx.emit(EditorEvent::Saved);
                    }
                    Err(HostError::Op(proto::Error::Conflict { .. })) => this.flag_conflict(),
                    Err(e) => this.error = Some(crate::i18n::tf("Could not save: {}", &[&e]).into()),
                }
                cx.emit(EditorEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Bytes to write to disk, in the file's encoding.
    pub(super) fn encoded(&self) -> Result<Vec<u8>, char> {
        let bytes = self.buffer.to_bytes();
        match self.encoding {
            Encoding::Utf8 => Ok(bytes),
            enc => crate::encoding::encode(std::str::from_utf8(&bytes).unwrap_or_default(), enc),
        }
    }

    /// Bytes for Save As (an explanation if there is a character with no equivalent in the encoding).
    pub fn encoded_bytes(&self) -> Result<Vec<u8>, String> {
        self.encoded().map_err(|ch| crate::i18n::tf("Not saved: {0} cannot be written in {1}.", &[&format!("\"{ch}\""), &self.encoding.label()]))
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Discard local changes, load the on-disk version.
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        let host = self.host.clone();
        let path = self.path.clone();
        let fallback = cx.global::<Settings>().fallback_encoding;
        let task = cx.background_executor().spawn(async move { load(host.as_ref(), &path, fallback) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(Loaded { mut buffer, hash, syntax, encoding, rules }) => {
                        if syntax.is_some() {
                            buffer.track_edits();
                        }
                        this.saved_version = buffer.version();
                        this.saved_line_ending = buffer.line_ending();
                        this.buffer = buffer;
                        this.epoch = super::next_epoch();
                        this.syntax = syntax;
                        this.encoding = encoding;
                        this.rules = rules;
                        this.apply_language_override();
                        this.indent = indent_with_rules(&this.buffer, &this.rules, cx.global::<Settings>().editorconfig, this.indent_prefs(cx));
                        this.column = None;
                        this.column_drag = false;
                        this.reset_lsp_data();
                        this.disk_hash = hash;
                        this.conflict = false;
                        this.error = None;
                        this.resync_lsp();
                        this.schedule_lsp_extras(300, cx);
                        this.schedule_git_marks(0, Some(cx));
                    }
                    Err(e) => this.error = Some(crate::i18n::tf("Could not load: {}", &[&e]).into()),
                }
                cx.emit(EditorEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn index_for_position(&self, pos: Point<Pixels>) -> Option<usize> {
        let first = self.visible.first()?;
        let last = self.visible.last()?;
        let v =
            self.visible.iter().find(|v| pos.y >= v.bounds.top() && pos.y < v.bounds.bottom()).unwrap_or(if pos.y < first.bounds.top() { first } else { last });
        let x = pos.x - v.text_left;
        let byte = if x <= px(0.) { 0 } else { v.layout.closest_index_for_x(x).min(v.text.len()) };
        let ix = v.start + v.text[..byte].chars().count();
        // A click at the end of a wrapped segment stays on that screen line (not at the start of the next one).
        let line_end = self.buffer.line_to_char(v.line) + self.buffer.line_len_chars(v.line);
        let seg_end = v.start + v.text.chars().count();
        Some(if ix == seg_end && seg_end < line_end && ix > v.start { ix - 1 } else { ix })
    }
}
