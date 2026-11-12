//! The Search view's scope row (workspace, open editors, changed files, current folder) and how a scope limits a search.

use gpui::AnyElement;
use theme::Palette;

use super::*;
use crate::a11y_controls::{Control, press_on, said};
use crate::search::scope::{Limit, Scope};

fn label(scope: Scope) -> &'static str {
    match scope {
        Scope::Workspace => crate::i18n::tr("Workspace"),
        Scope::OpenEditors => crate::i18n::tr("Open Editors"),
        Scope::ChangedFiles => crate::i18n::tr("Changed Files"),
        Scope::CurrentFolder => crate::i18n::tr("Current Folder"),
    }
}

impl SearchPanel {
    pub fn scope(&self) -> Scope {
        self.scope
    }

    /// Limits the search to `scope` (the workbench names its files and the search runs again).
    pub fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        // The old scope's files mean nothing here; the workbench is asked for the new ones.
        self.scope_paths.clear();
        cx.emit(SearchEvent::ScopeChanged);
        self.schedule(cx);
        cx.notify();
    }

    /// The files the workbench named for the scope (absolute paths; for the current folder, the folder). A change runs
    /// the search again.
    pub fn set_scope_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        if self.scope_paths == paths {
            return;
        }
        self.scope_paths = paths;
        if self.scope.needs_paths() {
            self.schedule(cx);
        }
        cx.notify();
    }

    /// The include globs for the folder `root`: the scope's when it limits the search, else the typed ones (while the
    /// filter is shown). `None` when the scope names no file in this folder, so there is nothing to search.
    pub(super) fn include_for(&self, root: &str, cx: &App) -> Option<Vec<String>> {
        self.include_with(root, scope::limit_for(self.scope, root, &self.scope_paths), cx)
    }

    /// As `include_for`, for a Search Editor or a replacement: only globs that match exactly the scope's files.
    pub(super) fn include_exact_for(&self, root: &str, cx: &App) -> Option<Vec<String>> {
        self.include_with(root, scope::limit_exact(self.scope, root, &self.scope_paths), cx)
    }

    fn include_with(&self, _root: &str, limit: Limit, cx: &App) -> Option<Vec<String>> {
        match limit {
            Limit::Everything => {
                let typed = self.include.read(cx).text().split(',').map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect();
                Some(if self.show_globs { typed } else { Vec::new() })
            }
            Limit::Only(globs) if globs.is_empty() => None,
            Limit::Only(globs) => Some(globs),
        }
    }

    /// Whether the scope lists `abs` (a result of the agent, which matched globs, not names).
    pub(super) fn scope_admits(&self, abs: &str) -> bool {
        scope::admits(self.scope, &self.scope_paths, abs)
    }

    /// The scope limits the search to some files, so the typed "Files to include" is not used.
    pub(super) fn scope_replaces_include(&self) -> bool {
        self.root.as_deref().is_some_and(|root| !matches!(scope::limit_for(self.scope, root, &self.scope_paths), Limit::Everything))
    }

    /// The scope names no file in the first folder: a Search Editor or a replacement would search everywhere.
    pub(super) fn scope_is_empty(&self, cx: &App) -> bool {
        self.root.as_deref().is_some_and(|root| self.include_for(root, cx).is_none())
    }

    /// What the scope means for the first folder, in words (`None` for the whole workspace).
    pub(crate) fn scope_note(&self) -> Option<String> {
        let root = self.root.as_deref()?;
        let limit = scope::limit_for(self.scope, root, &self.scope_paths);
        let n = match &limit {
            Limit::Only(globs) => globs.len(),
            Limit::Everything => 0,
        };
        let note = match (self.scope, limit) {
            (Scope::Workspace, _) => return None,
            (Scope::OpenEditors, _) if n == 0 => crate::i18n::t("No file is open in this folder.").to_string(),
            (Scope::OpenEditors, _) => crate::i18n::tn("Only the {} open file.", "Only the {} open files.", n, &[]),
            (Scope::ChangedFiles, _) if n == 0 => crate::i18n::t("No changed files.").to_string(),
            (Scope::ChangedFiles, _) => crate::i18n::tn("Only the {} changed file.", "Only the {} changed files.", n, &[]),
            (Scope::CurrentFolder, Limit::Only(globs)) if n == 0 || globs.is_empty() => {
                crate::i18n::t("The current folder is not inside this folder.").to_string()
            }
            (Scope::CurrentFolder, Limit::Only(globs)) => {
                crate::i18n::tf("Only in the folder {}.", &[&globs[0].trim_start_matches('/').trim_end_matches("/**")])
            }
            (Scope::CurrentFolder, Limit::Everything) => crate::i18n::t("The current folder is the workspace.").to_string(),
        };
        Some(note)
    }

    /// Four chips to choose what to search in, and a line that says what the choice means now.
    pub(super) fn scope_row(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let surface = crate::i18n::t("Search");
        let chips = Scope::ALL.into_iter().enumerate().map(|(i, scope)| {
            let (on, name) = (self.scope == scope, crate::i18n::t(label(scope)));
            let el = div()
                .id(("s-scope", i))
                .h(px(20.))
                .flex_none()
                .flex()
                .items_center()
                .px(px(theme::SPACE[2]))
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .text_size(px(text::XS.0))
                .when(on, |d| d.bg(c(p.surface_active)).text_color(c(p.text)))
                .when(!on, |d| d.text_color(c(p.text_muted)).hover(|d| d.bg(c(p.surface_hover)).text_color(c(p.text))))
                .child(name.to_string());
            press_on(el, Control::new(surface, format!("scope-{i}"), accesskit::Role::Button, name).toggled(on), cx, move |this: &mut Self, _, cx| {
                this.set_scope(scope, cx)
            })
        });
        div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[1]))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(theme::SPACE[1]))
                    .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(crate::i18n::t("Search in:")))
                    .children(chips),
            )
            .children(self.scope_note().map(|note| {
                let note = if self.scope_replaces_include() && !self.include.read(cx).text().trim().is_empty() {
                    crate::i18n::tf("{0} Files to include is not used.", &[&note])
                } else {
                    note
                };
                div()
                    .relative()
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .child(said(surface, "scope-note", accesskit::Role::Label, note.clone()))
                    .child(note)
            }))
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "search_scope_tests.rs"]
mod tests;
