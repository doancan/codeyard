//! Source Control's quick actions under the Commit button: Amend (the last commit takes the staged changes and, if the box has one,
//! a new message) and Stash (park the changes). Submodule of `scm.rs`: reads the panel's private fields.

use gpui::{Context, IntoElement, Window, div, prelude::*, px};

use super::ScmPanel;
use crate::a11y_controls::{Control, press_on};
use crate::git::Status;
use crate::graph::{GraphCommit, RefKind};
use crate::i18n::t;
use crate::ui::{Palette, c, icon, text};

/// Whether the last commit is also somewhere else: on a remote branch, or on the upstream (a commit the graph does not count as
/// "not pushed yet"). Amending it rewrites what others may have.
pub(super) fn is_shared(status: &Status, last: &GraphCommit) -> bool {
    last.refs.iter().any(|r| r.kind == RefKind::Remote) || (status.upstream.is_some() && !last.outgoing)
}

impl ScmPanel {
    /// The Amend and Stash buttons. Amend needs a commit to amend, Stash something to stash; neither runs while another
    /// operation of the panel does, or while conflicts wait.
    pub(super) fn quick_actions(&self, st: &Status, has_last: bool, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let conflicts = st.changes.iter().any(|c| c.unstaged == Some(crate::git::Mark::Conflict));
        let idle = !self.busy() && !conflicts;
        let button = |id: &'static str, glyph: &'static str, label: &'static str, tip: &'static str, enabled: bool| {
            let el = div()
                .id(id)
                .debug_selector(move || id.to_string())
                .h(px(22.))
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(theme::SPACE[2]))
                .rounded(px(theme::radius::SM))
                .border_1()
                .border_color(c(p.border))
                .text_size(px(text::XS.0))
                .text_color(c(p.text_muted))
                .overflow_hidden()
                .whitespace_nowrap()
                .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(c(p.surface_hover))).active(|d| d.bg(c(p.surface_active))))
                .when(!enabled, |d| d.opacity(0.5))
                .child(icon(glyph, 12., p.text_muted))
                .child(div().min_w_0().overflow_hidden().text_ellipsis().child(label))
                .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.into())).into());
            (el, enabled)
        };
        let (amend, amend_on) = button(
            "scm-quick-amend",
            "pencil",
            t("Amend"),
            t("Add the staged changes to the last commit (and give it the message above, if there is one)"),
            idle && has_last,
        );
        let (stash, stash_on) =
            button("scm-quick-stash", "file-archive", "Stash", t("Put the changes aside. Hold Shift to take new files too"), idle && !st.changes.is_empty());
        let amend = press_on(amend, Control::button(t("Source Control"), "scm-quick-amend", t("Amend"), |_, _| {}), cx, move |this: &mut Self, window, cx| {
            if amend_on {
                this.amend_pressed(window, cx);
            }
        });
        let stash = press_on(stash, Control::button(t("Source Control"), "scm-quick-stash", "Stash", |_, _| {}), cx, move |this: &mut Self, window, cx| {
            if stash_on {
                this.stash_pressed(window.modifiers().shift, cx);
            }
        });
        div().flex().gap(px(theme::SPACE[3])).child(amend).child(stash)
    }

    /// Amend. A commit that others may have is asked about first: amending it changes its hash, and the branch then has to be
    /// force-pushed.
    fn amend_pressed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shared = match (&self.status, self.last_commit()) {
            (Some(st), Some(last)) => is_shared(st, &last),
            _ => false,
        };
        if !shared {
            return self.commit_amend(cx);
        }
        let answer = crate::questions::git(
            t("Amend a commit that is already pushed?"),
            t("Amending replaces the commit with a new one. The branch then differs from the remote, and updating it takes a force push."),
            t("Amend"),
        )
        .ask(window, cx);
        cx.spawn(async move |this, cx| {
            if answer.await == Some(0) {
                this.update(cx, |this, cx| this.commit_amend(cx)).ok();
            }
        })
        .detach();
    }

    /// Stash. New files go along when asked (Shift), or when they are all there is to stash.
    fn stash_pressed(&mut self, with_new_files: bool, cx: &mut Context<Self>) {
        let only_new = self.status.as_ref().is_some_and(|st| st.changes.iter().all(|c| c.unstaged == Some(crate::git::Mark::Untracked) && c.staged.is_none()));
        self.stash(with_new_files || only_new, cx);
    }

    /// The commit the checked-out branch is at, as the graph has it.
    fn last_commit(&self) -> Option<GraphCommit> {
        self.graph.as_ref().and_then(|g| g.commits.iter().find(|c| c.refs.iter().any(|r| r.kind == RefKind::Head)).cloned())
    }
}

#[cfg(test)]
#[path = "scm_quick_tests.rs"]
mod tests;
