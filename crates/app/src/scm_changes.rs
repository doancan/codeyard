//! The change list of a single repository: one uniform row per group header or file, so only the visible rows are
//! built (thousands of generated changes stay within the frame budget). The scroll handle lives on the panel, so a
//! status refresh keeps the position.

use std::sync::Arc;

use gpui::{Context, div, prelude::*, px, uniform_list};

use super::ScmPanel;
use super::repos_ui::Group;
use crate::git::Status;
use crate::ui::{c, layout, palette, text};

/// One row of the single-repository list. `File` indexes `Status::changes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeLine {
    Group(Group, usize),
    File(Group, usize),
    /// "No changes."
    Empty,
}

/// Rows of `st`: Merge and Staged Changes only when they hold something, Changes always; files of open groups only.
pub(crate) fn change_lines(st: &Status, open: impl Fn(&str) -> bool) -> Vec<ChangeLine> {
    let mut out = Vec::new();
    for g in Group::ALL {
        let files: Vec<usize> = st.changes.iter().enumerate().filter(|(_, ch)| g.holds(ch)).map(|(i, _)| i).collect();
        if files.is_empty() && g != Group::Changes {
            continue;
        }
        out.push(ChangeLine::Group(g, files.len()));
        if open(g.id()) {
            out.extend(files.into_iter().map(|i| ChangeLine::File(g, i)));
        }
    }
    if st.changes.is_empty() {
        out.push(ChangeLine::Empty);
    }
    out
}

impl ScmPanel {
    pub(super) fn change_list(&self, st: Arc<Status>, fit: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let lines = Arc::new(change_lines(&st, |id| self.sections.is_open(id, true)));
        let n = lines.len();
        uniform_list(
            "scm-list",
            n,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range.filter_map(|ix| Some(this.change_line(ix, lines.get(ix).copied()?, &st, cx))).collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.change_scroll.clone())
        .debug_selector(|| "scm-list".into())
        // Nothing to commit: the list takes only its lines and the graph gets the room (until its height is set by hand).
        .when(fit, |d| d.h(px(layout::ROW_H * n as f32)))
        .when(!fit, |d| d.flex_1().min_h(px(72.)))
    }

    fn change_line(&self, ix: usize, line: ChangeLine, st: &Status, cx: &mut Context<Self>) -> gpui::AnyElement {
        match line {
            ChangeLine::Group(g, n) => self.group_header(g.id(), g.label(), n, g.staged(), None, cx).into_any_element(),
            ChangeLine::File(g, i) => match st.changes.get(i) {
                Some(change) => self.row(ix, change, g.staged(), None, cx).into_any_element(),
                None => div().h(px(layout::ROW_H)).into_any_element(),
            },
            ChangeLine::Empty => {
                let p = *palette(cx);
                div()
                    .h(px(layout::ROW_H))
                    .flex()
                    .items_center()
                    .px(px(theme::SPACE[6]))
                    .text_size(px(text::SM.0))
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::t("No changes."))
                    .into_any_element()
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn change_scroll_top(&self) -> f32 {
        -f32::from(self.change_scroll.0.borrow().base_handle.offset().y)
    }

    #[cfg(test)]
    pub(crate) fn scroll_changes_to(&self, ix: usize) {
        self.change_scroll.scroll_to_item_strict(ix, gpui::ScrollStrategy::Top);
    }
}

#[cfg(test)]
#[path = "scm_changes_tests.rs"]
mod tests;
