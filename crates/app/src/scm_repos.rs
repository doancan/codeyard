//! Source Control: the workspace's repositories. With more than one, a list at the top shows each one's branch, sync
//! counts and pending changes; the selected one is what the rest of the panel shows and what every operation targets.

use gpui::{Context, SharedString, div, prelude::*, px};

use super::{ScmEvent, ScmPanel};
use crate::i18n::{t, tf};
use crate::ui::{c, icon, palette, text};

/// One repository as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoRow {
    /// Work tree: the repository's identity in the workbench.
    pub root: String,
    /// Folder name.
    pub label: String,
    /// Where it is, relative to the workspace folder it was found in (`None` for a workspace folder itself).
    pub location: Option<String>,
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: usize,
    /// Status could not be read (untrusted, broken repository): shown instead of the counts.
    pub error: Option<String>,
    /// Not read yet.
    pub loading: bool,
    pub selected: bool,
}

impl RepoRow {
    /// What a screen reader says for the row (and the tooltip).
    pub fn description(&self) -> String {
        let place = self.location.clone().unwrap_or_else(|| self.label.clone());
        if let Some(e) = &self.error {
            return tf("Repository {0}: {1}", &[&place, e]);
        }
        let branch = self.branch.clone().unwrap_or_else(|| t("detached HEAD").into());
        tf("Repository {0}, branch {1}, changes: {2}, to push: {3}, to pull: {4}", &[&place, &branch, &self.changes, &self.ahead, &self.behind])
    }
}

impl ScmPanel {
    pub fn set_repos(&mut self, rows: Vec<RepoRow>, truncated: bool, cx: &mut Context<Self>) {
        if self.repos != rows || self.repos_truncated != truncated {
            self.repos = rows;
            self.repos_truncated = truncated;
            cx.notify();
        }
    }

    pub fn repos(&self) -> &[RepoRow] {
        &self.repos
    }

    /// The list; only shown when there is a choice to make.
    pub(super) fn repos_section(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.repos.len() < 2 {
            return None;
        }
        let p = *palette(cx);
        let rows = self.repos.iter().enumerate().map(|(ix, r)| {
            let root = r.root.clone();
            let tip: SharedString = r.description().into();
            let counts = match (&r.error, r.loading) {
                (Some(_), _) => t("unavailable").to_string(),
                (None, true) => String::new(),
                (None, false) => {
                    let mut parts = Vec::new();
                    if r.ahead > 0 || r.behind > 0 {
                        parts.push(crate::git::counts_label(r.ahead, r.behind));
                    }
                    if r.changes > 0 {
                        parts.push(r.changes.to_string());
                    }
                    parts.join("  ")
                }
            };
            div()
                .id(("scm-repo", ix))
                .h(px(crate::ui::layout::ROW_H))
                .px(px(theme::SPACE[6]))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .cursor_pointer()
                .when(r.selected, |d| d.bg(c(p.surface_active)))
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(icon("git-branch", 14., if r.selected { p.accent } else { p.text_muted }))
                .child(div().flex_none().text_color(c(p.text)).child(r.label.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(text::XS.0))
                        .text_color(c(p.text_muted))
                        .child(r.branch.clone().unwrap_or_else(|| t("detached HEAD").into())),
                )
                .child(div().flex_none().text_size(px(text::XS.0)).text_color(c(if r.error.is_some() { p.danger } else { p.text_muted })).child(counts))
                .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.clone())).into())
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ScmEvent::SelectRepo(root.clone()))))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .pb(px(theme::SPACE[3]))
                .child(
                    div()
                        .px(px(theme::SPACE[6]))
                        .pb(px(theme::SPACE[2]))
                        .text_size(px(text::XXS.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(c(p.text_muted))
                        .child(crate::i18n::upper(t("Repositories"))),
                )
                .children(rows)
                .when(self.repos_truncated, |d| {
                    d.child(
                        div()
                            .px(px(theme::SPACE[6]))
                            .text_size(px(text::XS.0))
                            .text_color(c(p.text_muted))
                            .child(tf("Only the first {} repositories are listed.", &[&crate::git::repos::MAX_REPOS])),
                    )
                })
                .into_any_element(),
        )
    }
}
