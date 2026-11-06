//! Source Control: what Git is in the middle of (a merge, a rebase, a cherry-pick), and the view of a folder that is not a
//! repository yet. Kept out of `scm.rs` for its size budget.

use gpui::{Context, SharedString, Window, div, prelude::*, px};

use super::{ScmEvent, ScmPanel};
use crate::git::{Mark, Status};
use crate::i18n::t;
use crate::repos::OpState;
use crate::ui::{self, c, icon, text};

impl ScmPanel {
    /// A folder that is not a repository: the two ways to get one, like the Source Control view of VS Code.
    pub(super) fn not_a_repository_actions(&self, p: &ui::Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *p;
        let button = |id: &'static str, glyph: &'static str, label: &'static str, primary: bool, event: fn() -> ScmEvent| {
            div()
                .id(id)
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(theme::SPACE[2]))
                .rounded(px(theme::radius::SM))
                .bg(c(if primary { p.accent } else { p.surface_active }))
                .text_color(c(if primary { p.on_accent } else { p.text }))
                .cursor_pointer()
                .hover(|d| d.opacity(0.9))
                .child(icon(glyph, 14., if primary { p.on_accent } else { p.text }))
                .child(label)
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(event())))
        };
        div()
            .px(px(theme::SPACE[6]))
            .pt(px(theme::SPACE[4]))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[3]))
            .child(button("scm-init", "git-branch", t("Initialize Repository"), true, || ScmEvent::InitRepository))
            .child(button("scm-clone", "git-branch", t("Clone Repository…"), false, || ScmEvent::CloneRepository))
    }

    /// "Merge in progress · 2 conflicts"; `None` when git is not in the middle of anything.
    pub(super) fn operation_summary(&self, st: &Status) -> Option<String> {
        let label = st.op.label()?;
        let conflicts = st.changes.iter().filter(|c| c.unstaged == Some(Mark::Conflict)).count();
        Some(if conflicts == 0 { label.to_string() } else { format!("{label} · {}", conflict_count(conflicts)) })
    }

    pub fn commit_button_for_test(&self, cx: &gpui::App) -> Option<(String, bool)> {
        self.status.as_ref().map(|st| self.commit_state(st, cx))
    }

    pub fn result_for_test(&self) -> Option<String> {
        self.result.as_ref().map(|(message, _)| message.to_string())
    }

    /// "Merge in progress · 2 conflicts" with what to do next: abort, or (a rebase) continue or abort. A half-done operation is
    /// easy to forget about, and the way out used to be a palette command only.
    pub(super) fn operation_banner(&self, st: &Status, p: &ui::Palette) -> Option<impl IntoElement + use<>> {
        let summary: SharedString = self.operation_summary(st)?.into();
        let p = *p;
        let dispatch = |action: Box<dyn gpui::Action>| {
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                cx.stop_propagation();
                window.dispatch_action(action.boxed_clone(), cx);
            }
        };
        let link = |id: &'static str, text_label: &'static str, action: Box<dyn gpui::Action>| {
            div()
                .id(id)
                .px(px(theme::SPACE[2]))
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .text_color(c(p.accent))
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(text_label)
                .on_click(dispatch(action))
        };
        let actions = match st.op {
            OpState::Merging => vec![link("scm-op-abort", t("Abort"), Box::new(crate::workbench::GitAbortMerge))],
            OpState::Rebasing => vec![
                link("scm-op-continue", t("Continue"), Box::new(crate::workbench::GitContinueRebase)),
                link("scm-op-abort", t("Abort"), Box::new(crate::workbench::GitAbortRebase)),
            ],
            _ => Vec::new(),
        };
        Some(
            div()
                .id("scm-operation")
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .text_size(px(text::XS.0))
                .child(icon("alert", 14., p.warning))
                .child(div().flex_1().min_w_0().text_color(c(p.text)).child(summary))
                .children(actions),
        )
    }
}

/// "1 conflict" / "3 conflicts".
fn conflict_count(n: usize) -> String {
    crate::i18n::tn("{} conflict", "{} conflicts", n, &[])
}
