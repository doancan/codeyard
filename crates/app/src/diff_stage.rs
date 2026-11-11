//! Staging from Source Control's diff: lines are picked in the diff (a click on a line's gutter, Shift-click for a
//! range, Space for the change under F7) and Enter, or the header's button, stages them (Index ↔ Working Tree) or
//! unstages them (HEAD ↔ Staged). A change's own button does the whole change. The diff is read again afterwards; when
//! the index or the file changed since it was shown, nothing is written and the diff is reloaded.

use std::sync::Arc;

use gpui::{Context, SharedString, Window, actions};
use host::Host;

use crate::diff::DiffView;
use crate::git::{self, Picked};
use crate::i18n::{t, tn};

actions!(diff, [ApplyPicked, PickChange, ClearPicked]);

/// Something the diff did to the repository: Source Control reads its state again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffEvent {
    Changed,
}

/// Where a Source Control diff stages to, and which way.
#[derive(Clone)]
pub struct Stager {
    pub host: Arc<dyn Host>,
    pub root: String,
    /// HEAD ↔ Staged: picked lines are unstaged. Otherwise (Index ↔ Working Tree) they are staged.
    pub staged: bool,
}

/// The two texts of a Source Control diff as the view shows them (no BOM, LF): `HEAD` and the index, or the index and
/// the working file. `orig` is the path in `HEAD` (a renamed file's old path).
pub fn load_texts(host: &dyn Host, root: &str, rel: &str, orig: &str, staged: bool) -> (Option<String>, Option<String>) {
    if staged {
        (git::show(host, root, "HEAD", orig), git::show(host, root, "", rel))
    } else {
        let disk = host.read(&format!("{}/{rel}", root.trim_end_matches('/'))).ok().map(|(d, _)| normalize(&String::from_utf8_lossy(&d)));
        (git::show(host, root, "", rel), disk)
    }
}

fn normalize(text: &str) -> String {
    text.trim_start_matches('\u{feff}').replace("\r\n", "\n")
}

/// Stage or unstage `picked` as the view showed them (`old`, `new`), then read the diff again.
fn run(stager: &Stager, rel: &str, old: &str, new: &str, picked: &Picked) -> (Result<git::Taken, String>, (Option<String>, Option<String>)) {
    let host = stager.host.as_ref();
    let result = if stager.staged {
        git::unstage_lines(host, &stager.root, rel, old, new, picked)
    } else {
        // The working file must still be what the diff shows, byte for byte once normalized, and UTF-8.
        let path = format!("{}/{rel}", stager.root.trim_end_matches('/'));
        match host.read(&path).map_err(|e| e.to_string()).and_then(|(bytes, _)| {
            String::from_utf8(bytes).map_err(|_| t("Staging changes needs a UTF-8 file (this one is stored in another encoding)").to_string())
        }) {
            Ok(disk) if normalize(&disk) != new => Err(git::stale_diff_message()),
            Ok(disk) => git::stage_lines(host, &stager.root, rel, &normalize(&disk), Some(old), |_| picked.clone()),
            Err(e) => Err(e),
        }
    };
    (result, load_texts(host, &stager.root, rel, rel, stager.staged))
}

impl DiffView {
    /// Staging from this diff: Source Control's diffs only.
    pub fn with_staging(mut self, stager: Stager) -> Self {
        self.stage = Some(stager);
        self
    }

    /// Lines can be picked here: a Source Control diff of text that is lined up exactly (staging works on the exact
    /// rows, so it is off while whitespace is ignored) and read without loss (replacement characters would be staged
    /// in place of the file's bytes).
    pub fn can_stage(&self) -> bool {
        self.stage.is_some() && !self.ws.ignores() && self.unsupported.is_none() && !self.invalid_bytes
    }

    /// Whether lines are unstaged here (HEAD ↔ Staged) rather than staged.
    pub fn unstages(&self) -> bool {
        self.stage.as_ref().is_some_and(|s| s.staged)
    }

    fn is_changed(&self, row: usize) -> bool {
        matches!(self.rows.get(row), Some(git::DiffRow::Changed(..)))
    }

    /// A click on a changed line: Shift picks every changed line from the last one clicked; otherwise the line is
    /// picked or unpicked.
    pub fn click_row(&mut self, row: usize, shift: bool, cx: &mut Context<Self>) {
        if !self.can_stage() || !self.is_changed(row) || self.busy {
            return;
        }
        self.outcome = None;
        match self.anchor.filter(|_| shift) {
            Some(anchor) => {
                let (lo, hi) = (anchor.min(row), anchor.max(row));
                let all: Vec<usize> = (lo..=hi).filter(|&r| self.is_changed(r)).collect();
                self.picked.extend(all);
            }
            None => {
                if !self.picked.remove(&row) {
                    self.picked.insert(row);
                }
                self.anchor = Some(row);
            }
        }
        self.current = self.block_of(row).unwrap_or(self.current);
        cx.notify();
    }

    /// The block (index into `blocks`) a row is in.
    pub(crate) fn block_of(&self, row: usize) -> Option<usize> {
        self.blocks.iter().position(|b| b.contains(&row))
    }

    /// Space: pick every line of the change under F7, or unpick it when it is all picked.
    fn pick_change(&mut self, _: &PickChange, _: &mut Window, cx: &mut Context<Self>) {
        let Some(block) = self.blocks.get(self.current).cloned() else { return };
        if !self.can_stage() || self.busy {
            return;
        }
        self.outcome = None;
        if block.clone().all(|r| self.picked.contains(&r)) {
            block.for_each(|r| {
                self.picked.remove(&r);
            });
        } else {
            self.picked.extend(block);
        }
        cx.notify();
    }

    pub(crate) fn clear_picked(&mut self, _: &ClearPicked, _: &mut Window, cx: &mut Context<Self>) {
        if !self.picked.is_empty() || self.outcome.is_some() {
            self.picked.clear();
            self.anchor = None;
            self.outcome = None;
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    /// Enter: stage or unstage the picked lines, or the change under F7 when none is picked.
    fn apply_picked(&mut self, _: &ApplyPicked, _: &mut Window, cx: &mut Context<Self>) {
        let rows = if self.picked.is_empty() { self.blocks.get(self.current).cloned().map(|b| b.collect()).unwrap_or_default() } else { self.picked.clone() };
        self.apply(rows, cx);
    }

    /// Stage or unstage one whole change (its button).
    pub(crate) fn apply_block(&mut self, block: usize, cx: &mut Context<Self>) {
        let Some(rows) = self.blocks.get(block).cloned() else { return };
        self.current = block;
        self.apply(rows.collect(), cx);
    }

    /// The header's button: the picked lines.
    pub(crate) fn apply_selection(&mut self, cx: &mut Context<Self>) {
        let rows = self.picked.clone();
        self.apply(rows, cx);
    }

    fn apply(&mut self, rows: Picked, cx: &mut Context<Self>) {
        let Some(stager) = self.stage.clone() else { return };
        if self.busy || rows.is_empty() {
            return;
        }
        self.busy = true;
        self.outcome = None;
        let (rel, old, new) = (self.path.clone(), self.old_text.clone(), self.new_text.clone());
        let unstage = stager.staged;
        let task = cx.background_executor().spawn(async move { run(&stager, &rel, &old, &new, &rows) });
        cx.spawn(async move |this, cx| {
            let (result, (old, new)) = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                let wrote = matches!(result, Ok(t) if t.lines > 0);
                this.outcome = Some(match result {
                    Ok(t) if t.lines == 0 => (SharedString::from(t_nothing(unstage)), false),
                    Ok(t) if unstage => (tn("Unstaged {} line", "Unstaged {} lines", t.lines, &[]).into(), false),
                    Ok(t) => (tn("Staged {} line", "Staged {} lines", t.lines, &[]).into(), false),
                    Err(e) => (e.into(), true),
                });
                this.reload(old.as_deref().unwrap_or(""), new.as_deref().unwrap_or(""));
                if wrote {
                    cx.emit(DiffEvent::Changed);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// New texts after an operation: picks are dropped, the view stays where it was and F7 stays on the nearest change.
    fn reload(&mut self, old: &str, new: &str) {
        self.set_texts(old, new);
        self.picked.clear();
        self.anchor = None;
        self.current = self.current.min(self.blocks.len().saturating_sub(1));
    }

    pub(crate) fn register_stage_actions(&self, d: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        use gpui::InteractiveElement as _;
        if !self.can_stage() {
            return d;
        }
        d.on_action(cx.listener(Self::apply_picked)).on_action(cx.listener(Self::pick_change)).on_action(cx.listener(Self::clear_picked))
    }
}

fn t_nothing(unstage: bool) -> &'static str {
    if unstage { t("Nothing to unstage in these lines") } else { t("Nothing to stage in these lines") }
}

impl gpui::EventEmitter<DiffEvent> for DiffView {}

#[cfg(test)]
#[path = "diff_stage_tests.rs"]
mod tests;
