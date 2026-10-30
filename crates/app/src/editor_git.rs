//! Editor: git markers, hunk stage / revert, merge conflicts, jump to change.

use super::*;
use crate::i18n::{t, tn};

impl EditorView {
    /// Fetch the last change of the cursor's line (author, time, subject) in the background; shown
    /// as dim text at the end of the line (`git.inlineBlame`).
    pub(super) fn schedule_blame(&mut self, cx: &mut Context<Self>) {
        if !cx.global::<Settings>().inline_blame {
            self.blame = None;
            self.blame_task = None;
            return;
        }
        let Some(root) = self.git_root.clone() else { return };
        let Some(rel) = self.path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).map(str::to_string) else { return };
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let version = self.buffer.version();
        if self.blame.as_ref().is_some_and(|(l, v, _)| *l == line && *v == version) {
            return;
        }
        if self.blame.as_ref().is_some_and(|(l, ..)| *l != line) {
            self.blame = None;
        }
        if self.buffer.line(line).trim().is_empty() {
            self.blame_task = None;
            return;
        }
        let (host, rope, dirty) = (self.host.clone(), self.buffer.rope().clone(), self.is_dirty());
        self.blame_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(350)).await;
            let result =
                cx.background_executor().spawn(async move { crate::git::blame_line(host.as_ref(), &root, &rel, line, dirty.then(|| rope.to_string())) }).await;
            this.update(cx, |this, cx| {
                if let Ok(e) = result {
                    let text = if e.hash.chars().all(|c| c == '0') {
                        crate::i18n::t("You · Uncommitted changes").to_string()
                    } else {
                        format!("{}, {} · {}", e.author, e.when, e.subject)
                    };
                    this.blame = Some((line, version, text.into()));
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Repo (or HEAD) changed: fetch the HEAD content again. `None`: outside a repo.
    /// `changed`: git sees this file as modified; if not and the buffer is clean,
    /// the HEAD content is the buffer itself.
    pub fn set_git_root(&mut self, root: Option<String>, changed: bool, cx: &mut Context<Self>) {
        if self.scm_owned {
            return;
        }
        self.git_root = root.clone();
        self.blame = None;
        self.schedule_blame(cx);
        let rel = root.as_deref().and_then(|r| self.path.strip_prefix(&format!("{}/", r.trim_end_matches('/'))).map(str::to_string));
        let (Some(root), Some(rel)) = (root, rel) else {
            self.git_base = None;
            self.git_marks.clear();
            cx.notify();
            return;
        };
        if !changed && !self.is_dirty() {
            self.git_base = Some(self.buffer.rope().clone());
            self.git_marks.clear();
            cx.notify();
            return;
        }
        let host = self.host.clone();
        let task = cx.background_executor().spawn(async move { crate::git::show(host.as_ref(), &root, "HEAD", &rel) });
        self.git_task = Some(cx.spawn(async move |this, cx| {
            let base = task.await;
            this.update(cx, |e, cx| {
                if e.scm_owned {
                    return;
                }
                // Untracked or new file: not in HEAD, so no marker (as VS Code does).
                e.git_base = base.map(|b| buffer::Rope::from_str(&b));
                e.git_marks.clear();
                e.schedule_git_marks(0, Some(cx));
            })
            .ok();
        }));
    }

    /// An extension's source control provider owns this file (#144): `base` is its original text (none for a new file:
    /// no marks). Git's blame and hunk actions are off for it.
    pub fn set_scm_base(&mut self, base: Option<String>, cx: &mut Context<Self>) {
        self.take_scm(cx);
        self.git_base = base.map(|b| buffer::Rope::from_str(&b.replace("\r\n", "\n")));
        self.git_marks.clear();
        self.schedule_git_marks(0, Some(cx));
        cx.notify();
    }

    /// As `set_scm_base`, with the file as saved: the provider does not list it as changed.
    pub fn set_scm_base_saved(&mut self, cx: &mut Context<Self>) {
        self.take_scm(cx);
        if !self.is_dirty() {
            self.git_base = Some(self.buffer.rope().clone());
            self.git_marks.clear();
            cx.notify();
        }
    }

    fn take_scm(&mut self, cx: &mut Context<Self>) {
        if !self.scm_owned {
            self.scm_owned = true;
            self.git_root = None;
            self.git_task = None;
            self.schedule_blame(cx);
        }
    }

    /// The provider no longer owns the file: no marks until git's are read.
    pub fn release_scm_base(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.scm_owned) {
            self.git_base = None;
            self.git_marks.clear();
            cx.notify();
        }
    }

    /// Compute line markers in the background (after waiting `delay` ms, so not on every keystroke while typing).
    pub(super) fn schedule_git_marks(&mut self, delay: u64, cx: Option<&mut Context<Self>>) {
        let (Some(base), Some(cx)) = (self.git_base.clone(), cx) else { return };
        let text = self.buffer.rope().clone();
        self.git_task = Some(cx.spawn(async move |this, cx| {
            if delay > 0 {
                cx.background_executor().timer(std::time::Duration::from_millis(delay)).await;
            }
            // If equal (comparing without flattening) there's no diff; otherwise convert to text and compare.
            let marks = cx
                .background_executor()
                .spawn(async move { if base == text { Vec::new() } else { crate::git::line_marks(&base.to_string(), &text.to_string()) } })
                .await;
            this.update(cx, |e, cx| {
                e.git_marks = marks;
                cx.notify();
            })
            .ok();
        }));
    }

    #[cfg(test)]
    pub fn git_marks(&self) -> Vec<Option<crate::git::LineMark>> {
        self.git_marks.clone()
    }

    /// Stage the changed hunks at the cursor (selection) (like `git add -p`, but per hunk,
    /// not per line: even if the selection covers part of a hunk, the whole hunk goes in).
    pub(super) fn stage_hunk(&mut self, _: &StageHunk, _: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.git_root.clone() else { return cx.emit(EditorEvent::Message(t("Not in a git repository").into())) };
        let Some(rel) = self.path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).map(str::to_string) else {
            return cx.emit(EditorEvent::Message(t("This file is outside the repository root").into()));
        };
        if self.encoding != Encoding::Utf8 {
            return cx.emit(EditorEvent::Message(t("Staging changes needs a UTF-8 file").into()));
        }
        // Keep two quick calls from reading the same index and overwriting each other's write.
        if self.stage_busy {
            return cx.emit(EditorEvent::Message(t("Still staging the previous change…").into()));
        }
        self.stage_busy = true;
        let (lo, hi) = self.cursor_lines();
        let (host, text) = (self.host.clone(), self.buffer.rope().to_string());
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { crate::git::stage_hunks(host.as_ref(), &root, &rel, &text, lo, hi) }).await;
            this.update(cx, |e, cx| {
                e.stage_busy = false;
                cx.emit(EditorEvent::Message(match result {
                    Ok(0) => t("No change here to stage").into(),
                    Ok(n) => tn("Staged {} change", "Staged {} changes", n, &[]),
                    Err(e) => e,
                }))
            })
            .ok();
        })
        .detach();
    }

    /// Revert the changed hunks at the cursor (selection) to their HEAD state (undo brings them back).
    pub(super) fn revert_hunk(&mut self, _: &RevertHunk, _: &mut Window, cx: &mut Context<Self>) {
        let Some(base) = self.git_base.as_ref().map(|b| b.to_string()) else { return cx.emit(EditorEvent::Message(t("No git changes to revert").into())) };
        let text = self.buffer.rope().to_string();
        if self.encoding != Encoding::Utf8 || crate::git::has_lone_cr(&base) || crate::git::has_lone_cr(&text) {
            return cx.emit(EditorEvent::Message(t("Reverting a change is not supported in this file (non-UTF-8 or CR-only line endings)").into()));
        }
        let (lo, hi) = self.cursor_lines();
        let chosen = crate::git::hunks_touching(&crate::git::hunks(&base, &text), lo, hi);
        if chosen.is_empty() {
            return cx.emit(EditorEvent::Message(t("No change here to revert").into()));
        }
        let base_lines: Vec<&str> = base.split_inclusive('\n').collect();
        let parts = chosen
            .iter()
            .map(|h| {
                let from = self.buffer.line_to_char(h.new.start.min(self.buffer.len_lines()));
                let to = self.buffer.line_to_char(h.new.end.min(self.buffer.len_lines()));
                (from..to, base_lines[h.old.start.min(base_lines.len())..h.old.end.min(base_lines.len())].concat())
            })
            .collect();
        self.buffer.replace_all(parts);
        self.after_edit(cx);
    }

    /// Alt+F3 / Shift+Alt+F3: go to the first line of the next / previous git hunk (wraps).
    pub(super) fn go_to_change(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let marks = &self.git_marks;
        let starts: Vec<usize> = (0..marks.len()).filter(|&l| marks[l].is_some() && (l == 0 || marks[l - 1].is_none())).collect();
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let target = if forward {
            starts.iter().copied().find(|&l| l > line).or(starts.first().copied())
        } else {
            starts.iter().rev().copied().find(|&l| l < line).or(starts.last().copied())
        };
        match target {
            Some(l) => self.go_to_line(l + 1, window, cx),
            None => cx.emit(EditorEvent::Message(t("No changes in this file").into())),
        }
    }

    pub(super) fn next_change(&mut self, _: &NextChange, window: &mut Window, cx: &mut Context<Self>) {
        self.go_to_change(true, window, cx);
    }

    pub(super) fn previous_change(&mut self, _: &PreviousChange, window: &mut Window, cx: &mut Context<Self>) {
        self.go_to_change(false, window, cx);
    }

    /// Edit and paint path: immediate for small files; for large ones (a flagged 32 MB file
    /// would cost ~27 ms per keystroke) in the background, rope copy is O(1). The result is written if the document didn't change.
    pub(super) fn refresh_conflicts_soon(&mut self, cx: &mut Context<Self>) {
        const SYNC_MAX_BYTES: usize = 4 * 1024 * 1024;
        let v = self.buffer.version();
        if v == self.conflicts_version {
            return;
        }
        if self.buffer.rope().len_bytes() <= SYNC_MAX_BYTES {
            return self.refresh_conflicts();
        }
        if self.conflicts_pending == v {
            return; // a scan for this version is already running
        }
        // `conflicts_version` is updated when the result arrives: commands scan synchronously meanwhile.
        self.conflicts_pending = v;
        let rope = self.buffer.rope().clone();
        let scan = cx.background_executor().spawn(async move { crate::conflict::find(&rope) });
        self.conflicts_task = Some(cx.spawn(async move |this, cx| {
            let found = scan.await;
            this.update(cx, |e, cx| {
                if e.buffer.version() == v {
                    e.conflicts = found;
                    e.conflicts_version = v;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// If the document changed, find conflicts again now (commands: always an up-to-date list).
    pub(super) fn refresh_conflicts(&mut self) {
        let v = self.buffer.version();
        if v != self.conflicts_version {
            self.conflicts_version = v;
            self.conflicts = crate::conflict::find(self.buffer.rope());
        }
    }

    /// Background of a conflict line: current side green, incoming blue, markers dark.
    pub(super) fn conflict_tint(&self, line: usize, p: &Palette) -> Option<gpui::Rgba> {
        let i = self.conflicts.partition_point(|c| c.end < line);
        let c = self.conflicts.get(i).filter(|c| c.start <= line)?;
        let (color, alpha) = if line == c.start {
            (p.success, 0.32)
        } else if c.current_lines().contains(&line) {
            (p.success, 0.13)
        } else if line == c.end {
            (p.info, 0.32)
        } else if c.incoming_lines().contains(&line) {
            (p.info, 0.13)
        } else if Some(line) == c.base {
            (p.text_muted, 0.22)
        } else if line == c.sep {
            (p.text_muted, 0.16)
        } else {
            (p.text_muted, 0.07) // common ancestor (diff3)
        };
        Some(ui::ca(color, alpha))
    }

    /// Summary of visible lines: (start of first segment, segment count, top of first line).
    pub(super) fn visible_sig(&self) -> (usize, usize, Pixels) {
        self.visible.first().map_or((usize::MAX, 0, px(0.)), |v| (v.start, self.visible.len(), v.bounds.top()))
    }

    /// Is anything positioned by line layout (conflict links, popups).
    pub(super) fn wants_anchored(&self) -> bool {
        !self.conflicts.is_empty()
            || self.completion.is_some()
            || self.signature.is_some()
            || self.hover.is_some()
            || self.color_picker.is_some()
            || !self.http_request_lines().is_empty()
    }

    /// Conflict count in the last paint (status bar).
    pub fn conflict_count(&self) -> usize {
        self.conflicts.len()
    }

    /// For the merge editor: resolve conflict number `ix`.
    pub fn resolve_conflict_at(&mut self, ix: usize, choice: crate::conflict::Choice, cx: &mut Context<Self>) {
        self.resolve_conflict(ix, choice, cx);
    }

    /// For the merge editor: resolve all in the same direction.
    pub fn resolve_all_conflicts(&mut self, choice: crate::conflict::Choice, cx: &mut Context<Self>) {
        self.accept_all(choice, cx);
    }

    /// Resolve conflict number `ix`: a single undo step, cursor at the start of the resolved text.
    pub(super) fn resolve_conflict(&mut self, ix: usize, choice: crate::conflict::Choice, cx: &mut Context<Self>) {
        self.refresh_conflicts();
        let Some(c) = self.conflicts.get(ix).cloned() else { return };
        let rope = self.buffer.rope();
        let (range, text) = (crate::conflict::char_range(rope, &c), crate::conflict::resolution(rope, &c, choice));
        let start = range.start;
        self.buffer.replace(range, &text, false);
        self.buffer.set_cursor(start, false);
        self.refresh_conflicts();
        self.after_edit(cx);
    }

    /// The conflict containing the cursor (tell the user if none).
    pub(super) fn conflict_at_cursor(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        self.refresh_conflicts();
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let ix = self.conflicts.iter().position(|c| c.contains_line(line));
        if ix.is_none() {
            cx.emit(EditorEvent::Message(if self.conflicts.is_empty() {
                t("No merge conflicts in this file").into()
            } else {
                t("Put the cursor inside a merge conflict").into()
            }));
        }
        ix
    }

    pub(super) fn accept_at_cursor(&mut self, choice: crate::conflict::Choice, cx: &mut Context<Self>) {
        if let Some(ix) = self.conflict_at_cursor(cx) {
            self.resolve_conflict(ix, choice, cx);
        }
    }

    pub(super) fn accept_current_change(&mut self, _: &AcceptCurrentChange, _: &mut Window, cx: &mut Context<Self>) {
        self.accept_at_cursor(crate::conflict::Choice::Current, cx);
    }

    pub(super) fn accept_incoming_change(&mut self, _: &AcceptIncomingChange, _: &mut Window, cx: &mut Context<Self>) {
        self.accept_at_cursor(crate::conflict::Choice::Incoming, cx);
    }

    pub(super) fn accept_both_changes(&mut self, _: &AcceptBothChanges, _: &mut Window, cx: &mut Context<Self>) {
        self.accept_at_cursor(crate::conflict::Choice::Both, cx);
    }

    /// Resolve all conflicts in the same direction: a single undo step.
    pub(super) fn accept_all(&mut self, choice: crate::conflict::Choice, cx: &mut Context<Self>) {
        self.refresh_conflicts();
        if self.conflicts.is_empty() {
            cx.emit(EditorEvent::Message(t("No merge conflicts in this file").into()));
            return;
        }
        let rope = self.buffer.rope();
        let parts: Vec<(Range<usize>, String)> =
            self.conflicts.iter().map(|c| (crate::conflict::char_range(rope, c), crate::conflict::resolution(rope, c, choice))).collect();
        let n = parts.len();
        self.buffer.replace_all(parts);
        self.refresh_conflicts();
        self.after_edit(cx);
        cx.emit(EditorEvent::Message(tn("Resolved {} conflict", "Resolved {} conflicts", n, &[])));
    }

    pub(super) fn accept_all_current(&mut self, _: &AcceptAllCurrent, _: &mut Window, cx: &mut Context<Self>) {
        self.accept_all(crate::conflict::Choice::Current, cx);
    }

    pub(super) fn accept_all_incoming(&mut self, _: &AcceptAllIncoming, _: &mut Window, cx: &mut Context<Self>) {
        self.accept_all(crate::conflict::Choice::Incoming, cx);
    }

    pub(super) fn next_conflict(&mut self, _: &NextConflict, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_conflict(true, cx);
    }

    pub(super) fn previous_conflict(&mut self, _: &PreviousConflict, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_conflict(false, cx);
    }

    /// To the start of the next/previous conflict (wraps around).
    pub fn jump_conflict(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.refresh_conflicts();
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let starts: Vec<usize> = self.conflicts.iter().map(|c| c.start).collect();
        let target = if forward {
            starts.iter().copied().find(|&s| s > line).or(starts.first().copied())
        } else {
            // If we're inside, it's the previous one's start, not this block's.
            let here = self.conflicts.iter().find(|c| c.contains_line(line)).map_or(line, |c| c.start);
            starts.iter().rev().copied().find(|&s| s < here).or(starts.last().copied())
        };
        let Some(target) = target else {
            cx.emit(EditorEvent::Message(t("No merge conflicts in this file").into()));
            return;
        };
        self.buffer.set_cursor(self.buffer.line_to_char(target), false);
        self.after_move(cx);
    }

    pub(super) fn next_problem(&mut self, _: &NextProblem, window: &mut Window, cx: &mut Context<Self>) {
        self.jump_problem(true, window, cx);
    }

    pub(super) fn previous_problem(&mut self, _: &PreviousProblem, window: &mut Window, cx: &mut Context<Self>) {
        self.jump_problem(false, window, cx);
    }

    /// F8 / Shift+F8: go to the next/previous diagnostic in the file (wraps), show its description.
    pub(super) fn jump_problem(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut at: Vec<usize> = self.diagnostics.iter().map(|d| self.buffer.from_lsp_position(d.start.0, d.start.1)).collect();
        at.sort_unstable();
        at.dedup();
        let cursor = self.buffer.cursor();
        let target = if forward {
            at.iter().copied().find(|&a| a > cursor).or(at.first().copied())
        } else {
            at.iter().rev().copied().find(|&a| a < cursor).or(at.last().copied())
        };
        let Some(target) = target else {
            cx.emit(EditorEvent::Message(t("No problems in this file").into()));
            return;
        };
        self.buffer.set_cursor(target, false);
        self.after_move(cx);
        // Open the description once the line is painted.
        cx.on_next_frame(window, |this, window, cx| this.show_hover(&ShowHover, window, cx));
    }

    /// Return to the on-disk version (unsaved changes are lost; not undoable).
    pub(super) fn revert_file(&mut self, _: &RevertFile, _: &mut Window, cx: &mut Context<Self>) {
        self.reload(cx);
    }
}
