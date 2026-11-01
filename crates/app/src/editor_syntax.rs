//! Editor: the syntax tree follows the buffer (#215). Every buffer edit (typing with any number of cursors, paste,
//! undo and redo, edits from a language server, an extension or an agent, a reload) is recorded at the buffer's single
//! splice point and handed to the tree as a tree-sitter edit, so the tree is always reparsed from the old one. A reparse
//! that does not fit `SYNTAX_BUDGET` finishes in the background; the edits made meanwhile are coalesced into the next
//! background reparse. A tree that lands redraws the editor only when something on screen changes color, and not in
//! the middle of typing: the next key's frame shows it anyway.

use super::*;

/// A tree that lands while keys are coming waits this long after the last edit before it asks for a frame of its own,
/// so that frame does not hold up the next key. A key that comes first draws the new colors itself.
const QUIET_AFTER_EDIT: std::time::Duration = std::time::Duration::from_millis(120);

/// The background side of the syntax tree.
#[derive(Default)]
pub(super) struct ParseState {
    /// A reparse too long for a keystroke, finishing in the background.
    task: Option<gpui::Task<()>>,
    /// A frame for new colors, waiting for a pause in typing (`QUIET_AFTER_EDIT`).
    repaint: Option<gpui::Task<()>>,
    /// When the tree last took an edit (the executor's clock, so tests can move it), to tell when typing paused.
    last_edit: Option<std::time::Instant>,
    /// Frames asked for by `repaint_when_quiet` (tests tell them from frames other things ask for).
    #[cfg(test)]
    repaints: usize,
    /// A test's budget in place of `SYNTAX_BUDGET`: whether a reparse fits 4 ms depends on the machine.
    #[cfg(test)]
    budget: Option<std::time::Duration>,
}

impl EditorView {
    /// Apply the buffer's edits to the syntax tree, reparse the changed part. A reparse that takes longer than
    /// `SYNTAX_BUDGET` (an edit that makes tree-sitter recover from an error across the whole file) finishes in the
    /// background: the key's frame is drawn with the old tree, moved by the edit, and the new one shows when ready.
    pub(super) fn sync_syntax(&mut self, cx: &mut Context<Self>) {
        self.sync_lsp();
        self.sync_tree(cx);
    }

    /// `sync_syntax` without the language server. Painting calls it too, so colors never come from a tree that has not
    /// seen an edit, whichever way the edit reached the buffer.
    pub(super) fn sync_tree(&mut self, cx: &mut Context<Self>) {
        let Some(syntax) = &mut self.syntax else { return };
        let edits = self.buffer.take_edits();
        if edits.is_empty() {
            return;
        }
        for e in &edits {
            syntax.edit(&syntax::Edit {
                start_byte: e.start_byte,
                old_end_byte: e.old_end_byte,
                new_end_byte: e.new_end_byte,
                start: e.start_point,
                old_end: e.old_end_point,
                new_end: e.new_end_point,
            });
        }
        self.parse.last_edit = Some(cx.background_executor().now());
        #[cfg(test)]
        let budget = self.parse.budget.unwrap_or(SYNTAX_BUDGET);
        #[cfg(not(test))]
        let budget = SYNTAX_BUDGET;
        if let Some(job) = syntax.reparse_within(self.buffer.rope(), budget) {
            self.parse_in_background(job, cx);
        }
    }

    fn parse_in_background(&mut self, job: syntax::ParseJob, cx: &mut Context<Self>) {
        let parsed = cx.background_executor().spawn(async move { job.run() });
        self.parse.task = Some(cx.spawn(async move |this, cx| {
            let parsed = parsed.await;
            this.update(cx, |e, cx| {
                let Some(syntax) = &mut e.syntax else { return };
                // An answer for a tree that no longer waits for it (reloaded, language changed) is dropped.
                let Some(landed) = syntax.finish(parsed, e.buffer.rope()) else { return };
                // Edits made while it ran are reparsed on top of it, in the background again.
                if let Some(job) = landed.next {
                    e.parse_in_background(job, cx);
                }
                if landed.repaint {
                    e.repaint_when_quiet(cx);
                }
            })
            .ok();
        }));
    }

    /// New colors on screen: draw them once typing pauses, unless a frame has drawn them by then.
    fn repaint_when_quiet(&mut self, cx: &mut Context<Self>) {
        if self.parse.repaint.is_some() {
            return;
        }
        self.parse.repaint = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(since) = this.read_with(cx, |e, cx| e.parse.last_edit.map(|t| cx.background_executor().now().saturating_duration_since(t))) else {
                    return;
                };
                let wait = QUIET_AFTER_EDIT.saturating_sub(since.unwrap_or(QUIET_AFTER_EDIT));
                if wait.is_zero() {
                    break;
                }
                cx.background_executor().timer(wait).await;
            }
            this.update(cx, |e, cx| {
                e.parse.repaint = None;
                if e.syntax.as_ref().is_some_and(|s| !s.is_shown()) {
                    #[cfg(test)]
                    {
                        e.parse.repaints += 1;
                    }
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// The syntax tree is being rebuilt off the UI thread.
    #[cfg(test)]
    pub fn parsing_syntax_for_test(&self) -> bool {
        self.syntax.as_ref().is_some_and(|s| s.parsing())
    }

    /// The colored spans in `bytes`, as the text is painted.
    #[cfg(test)]
    pub fn highlights_for_test(&self, bytes: Range<usize>) -> Vec<(Range<usize>, Role)> {
        self.syntax.as_ref().map(|s| s.highlights(self.buffer.rope(), bytes)).unwrap_or_default()
    }

    /// A frame for new colors waits for typing to pause.
    #[cfg(test)]
    pub fn syntax_repaint_waiting_for_test(&self) -> bool {
        self.parse.repaint.is_some()
    }

    /// Reparse within `budget` on a key instead of `SYNTAX_BUDGET`.
    #[cfg(test)]
    pub fn set_syntax_budget_for_test(&mut self, budget: std::time::Duration) {
        self.parse.budget = Some(budget);
    }

    /// Frames asked for once typing paused, so far.
    #[cfg(test)]
    pub fn syntax_repaints_for_test(&self) -> usize {
        self.parse.repaints
    }

    /// A frame drew text colored by the current tree.
    #[cfg(test)]
    pub fn syntax_shown_for_test(&self) -> bool {
        self.syntax.as_ref().is_some_and(|s| s.is_shown())
    }

    /// How long new colors wait for typing to pause.
    #[cfg(test)]
    pub const SYNTAX_QUIET_FOR_TEST: std::time::Duration = QUIET_AFTER_EDIT;
}
