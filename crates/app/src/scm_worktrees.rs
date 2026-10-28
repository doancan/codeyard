//! Source Control's worktree inventory: branch and path, current checkout, and same-host navigation.

use std::sync::Arc;

use gpui::{Context, SharedString, div, prelude::*, px};

use super::{ScmEvent, ScmPanel};
use crate::git::{self, Worktree};
use crate::i18n::{t, tf};
use crate::ui::{self, c, icon, layout, palette, text};

#[derive(Default)]
pub(super) struct Worktrees {
    pub loaded: bool,
    current_path: Option<String>,
    entries: Vec<Worktree>,
    error: Option<String>,
    task: Option<gpui::Task<()>>,
}

impl Worktrees {
    pub(super) fn reset(&mut self) {
        self.task = None;
        self.loaded = false;
        self.current_path = None;
        self.entries.clear();
        self.error = None;
    }
}

impl ScmPanel {
    /// Called by the visible panel's git refresh, including when HEAD and refs did not change.
    pub fn refresh_worktrees(&mut self, cx: &mut Context<Self>) {
        if self.worktrees.task.is_some() || !self.sections.is_open("worktrees", true) {
            return;
        }
        let Some(root) = self.status.as_ref().map(|s| s.root.clone()) else { return };
        let first_load = !self.worktrees.loaded;
        self.worktrees.loaded = true;
        let host = self.host.clone();
        let source = host.clone();
        let dir = root.clone();
        let current_path = self.worktrees.current_path.clone();
        let task = cx.background_executor().spawn(async move {
            let entries = git::worktree_entries(source.as_ref(), &dir)?;
            // Status preserves the opened path, which can be a symlink on an SSH host. Git lists canonical paths.
            let current_path = match current_path {
                Some(path) => path,
                None => {
                    let path = git::git_ok(source.as_ref(), &dir, &["rev-parse", "--show-toplevel"])?;
                    path.strip_suffix('\n').unwrap_or(&path).to_string()
                }
            };
            Ok((entries, current_path))
        });
        self.worktrees.task = Some(cx.spawn(async move |this, cx| {
            let timeout = cx.background_executor().timer(std::time::Duration::from_secs(15));
            let result = match futures::future::select(task, timeout).await {
                futures::future::Either::Left((result, _)) => result,
                futures::future::Either::Right(_) => Err(t("Listing worktrees timed out. Refresh to try again.").into()),
            };
            this.update(cx, |this, cx| {
                if !Arc::ptr_eq(&this.host, &host) || this.status.as_ref().is_none_or(|s| s.root != root) {
                    return;
                }
                this.worktrees.task = None;
                let changed = match result {
                    Ok((entries, current_path)) => {
                        let changed = this.worktrees.entries != entries || this.worktrees.error.is_some();
                        this.worktrees.entries = entries;
                        this.worktrees.current_path = Some(current_path);
                        this.worktrees.error = None;
                        changed
                    }
                    Err(error) => {
                        let changed = this.worktrees.error.as_ref() != Some(&error);
                        this.worktrees.error = Some(error);
                        changed
                    }
                };
                if changed || first_load {
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn toggle_worktrees(&mut self, cx: &mut Context<Self>) {
        self.sections.toggle("worktrees", true);
        self.refresh_worktrees(cx);
        self.emit_sections(cx);
        cx.notify();
    }

    fn open_worktree(&self, path: &str, cx: &mut Context<Self>) {
        let Some(st) = &self.status else { return };
        if self.worktrees.error.is_none()
            && self.worktrees.current_path.as_deref() != Some(path)
            && self.worktrees.entries.iter().any(|entry| entry.path == path && !entry.bare && entry.prunable.is_none())
        {
            cx.emit(ScmEvent::OpenWorktree { root: st.root.clone(), path: path.into() });
        }
    }

    pub(super) fn worktree_section(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *palette(cx);
        let open = self.sections.is_open("worktrees", true);
        let header = crate::sections::header("scm-worktree-header", t("Worktrees"), Some(self.worktrees.entries.len()), open, &p)
            .debug_selector(|| "scm-worktree-header".into())
            .tab_index(0)
            .focus(|d| d.bg(c(p.surface_active)))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_worktrees(cx)))
            .child(ui::icon_button("scm-worktree-add", "plus", t("Add worktree"), &p).tab_index(0).focus(|d| d.bg(c(p.surface_active))).on_click(cx.listener(
                |_, _, _, cx| {
                    cx.stop_propagation();
                    cx.emit(ScmEvent::AddWorktree);
                },
            )))
            .child(
                ui::icon_button("scm-worktree-refresh", "refresh-cw", t("Refresh"), &p)
                    .debug_selector(|| "scm-worktree-refresh".into())
                    .tab_index(0)
                    .focus(|d| d.bg(c(p.surface_active)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.sections.set_open("worktrees", true);
                        this.emit_sections(cx);
                        this.refresh_worktrees(cx);
                        cx.notify();
                    })),
            );
        div().flex().flex_col().min_h_0().child(header).when(open, |d| {
            let mut body = div().id("scm-worktrees").debug_selector(|| "scm-worktrees".into()).min_h_0().max_h(px(layout::ROW_H * 8.)).overflow_y_scroll();
            if let Some(error) = &self.worktrees.error {
                body = body.child(
                    div()
                        .debug_selector(|| "scm-worktree-error".into())
                        .px(px(theme::SPACE[6]))
                        .py(px(theme::SPACE[3]))
                        .text_size(px(text::XS.0))
                        .text_color(c(p.danger))
                        .child(tf("Could not list worktrees: {}", &[&error.lines().next().unwrap_or(error)])),
                );
            } else if self.worktrees.entries.is_empty() {
                body = body.child(div().px(px(theme::SPACE[6])).py(px(theme::SPACE[3])).text_color(c(p.text_muted)).child(if self.worktrees.task.is_some() {
                    t("Loading…")
                } else {
                    t("No worktrees found.")
                }));
            }
            body = body.children(self.worktrees.entries.iter().enumerate().map(|(i, entry)| self.worktree_row(i, entry, &p, cx)));
            d.child(body)
        })
    }

    fn worktree_row(&self, index: usize, entry: &Worktree, p: &ui::Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let current = self.worktrees.current_path.as_deref() == Some(entry.path.as_str());
        let available = !entry.bare && entry.prunable.is_none() && self.worktrees.error.is_none();
        let title = entry.branch.as_deref().unwrap_or_else(|| if entry.bare { t("Bare repository") } else { t("detached HEAD") });
        let badge = if current {
            Some(t("Current"))
        } else if !available {
            Some(t("Unavailable"))
        } else {
            None
        };
        let path = entry.path.clone();
        let label = path.escape_debug().to_string();
        let tip: SharedString = [
            title.to_string(),
            label.clone(),
            entry.head.clone().unwrap_or_default(),
            entry.locked.as_ref().map(|reason| tf("Worktree locked: {}", &[reason])).unwrap_or_default(),
            entry.prunable.clone().unwrap_or_default(),
            if available && !current { t("Open worktree in this window").into() } else { String::new() },
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .into();
        div()
            .id(SharedString::from(format!("scm-worktree-{}", entry.path)))
            .debug_selector(move || format!("scm-worktree-row-{index}"))
            .w_full()
            .h(px(layout::ROW_H * 2.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[3]))
            .px(px(theme::SPACE[6]))
            .border_1()
            .border_color(ui::ca(p.focus_ring, 0.))
            .when(current, |d| d.bg(c(p.surface_active)))
            .when(available, |d| d.tab_index(0).cursor_pointer().hover(|d| d.bg(c(p.surface_hover))).focus(|d| d.border_color(c(p.focus_ring))))
            .child(icon(if entry.locked.is_some() { "lock" } else { "git-branch" }, 14., p.text_muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[3]))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(text::SM.0)).text_color(c(p.text)).child(title.to_string()))
                            .children(badge.map(|b| div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(b))),
                    )
                    .child(div().truncate().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(label)),
            )
            .when(!self.menu_open, |d| d.tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.clone())).into()))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.open_worktree(&path, cx);
            }))
    }
}

#[cfg(test)]
#[path = "scm_worktree_tests.rs"]
mod tests;
