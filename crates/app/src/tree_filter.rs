//! The explorer's filter: a box above the tree that leaves only the names holding the typed text, with the folders that lead to
//! them. It looks through the whole workspace, not only the folders opened so far: the file list comes from the host (the same
//! list Go to File uses) and is arranged once on a worker thread (`tree_universe.rs`); each keystroke only scans the names.

use gpui::{AnyElement, Entity, Subscription, Task};

use super::universe::{Matches, Universe};
use super::*;
use crate::input::{FieldEvent, TextField};

/// Most files read from the host for the filter.
const FILE_LIMIT: u32 = 100_000;

/// What the box below the field says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterStatus {
    pub hits: usize,
    /// The list of all files is still being read.
    pub searching: bool,
    /// The list could not be read: only the folders opened so far are searched.
    pub opened_only: bool,
    /// The host stopped listing at the limit.
    pub truncated: bool,
}

#[derive(Default)]
pub(super) struct FilterState {
    /// The box; the filter is open while it exists.
    field: Option<Entity<TextField>>,
    _sub: Option<Subscription>,
    query: Option<matching::Query>,
    /// Every file of the workspace, as the host lists them (below the root).
    files: Option<Arc<Vec<String>>>,
    truncated: bool,
    files_failed: bool,
    universe: Option<Arc<Universe>>,
    matches: Matches,
    /// Bumped whenever what a running worker would deliver is out of date (another root, a new file list): its result is dropped.
    generation: u64,
    /// A worker is arranging names; another round is wanted when it ends.
    building: bool,
    dirty: bool,
    _task: Option<Task<()>>,
}

/// ⌘F in the tree opens the filter, Esc closes it. `cyx` lists these as built-in keys too (its `BINDING_SOURCES`).
pub fn key_bindings() -> [gpui::KeyBinding; 2] {
    let at = Some("FileTree");
    [gpui::KeyBinding::new("secondary-f", ToggleFilter, at), gpui::KeyBinding::new("escape", ClearFilter, at)]
}

impl FilterState {
    pub(super) fn is_active(&self) -> bool {
        self.query.is_some()
    }

    /// The folder changed or was read again: list all files again with the next filter run. A worker still running was started
    /// for the old state, so it is dropped and its result with it.
    pub(super) fn forget_files(&mut self) {
        self.files = None;
        self.files_failed = false;
        self.truncated = false;
        self.universe = None;
        self.matches = Matches::default();
        self.generation += 1;
        self.building = false;
        self.dirty = false;
        self._task = None;
    }
}

impl FileTree {
    /// What the box holds, while it is open.
    #[cfg(test)]
    pub fn filter_text(&self, cx: &App) -> Option<String> {
        self.filter.field.as_ref().map(|f| f.read(cx).text().to_string())
    }

    #[cfg(test)]
    pub fn filter_open(&self) -> bool {
        self.filter.field.is_some()
    }

    pub fn filter_status(&self) -> FilterStatus {
        FilterStatus {
            hits: self.filter.matches.hits,
            searching: self.filter.building || self.filter.universe.is_none(),
            opened_only: self.filter.files_failed,
            truncated: self.filter.truncated,
        }
    }

    /// The text being filtered for, to highlight in names.
    fn filter_query(&self) -> Option<&matching::Query> {
        self.filter.query.as_ref()
    }

    /// The box was opened with the keyboard or the header button: show it, or take focus into it; when it already has focus, close it.
    pub fn toggle_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.filter.field.clone() {
            None => self.open_filter(window, cx),
            Some(field) if field.focus_handle(cx).is_focused(window) => self.close_filter(window, cx),
            Some(field) => window.focus(&field.focus_handle(cx)),
        }
    }

    fn open_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = cx.new(|cx| TextField::new(crate::i18n::tr("Filter files by name"), cx));
        self.filter._sub = Some(cx.subscribe_in(&field, window, |this, field, ev: &FieldEvent, window, cx| match ev {
            FieldEvent::Changed => {
                let text = field.read(cx).text().to_string();
                this.set_filter_text(&text, cx);
                cx.emit(TreeEvent::FilterChanged(text));
            }
            FieldEvent::Submit => this.focus_first_match(window, cx),
            FieldEvent::Escape if field.read(cx).text().is_empty() => this.close_filter(window, cx),
            FieldEvent::Escape => field.update(cx, |f, cx| f.set_text("", cx)),
        }));
        window.focus(&field.focus_handle(cx));
        self.filter.field = Some(field);
        cx.notify();
    }

    /// Escape in the tree: the filter goes away and the tree shows where the selected entry is.
    pub(super) fn clear_filter_action(&mut self, _: &ClearFilter, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter.field.is_some() {
            self.close_filter(window, cx);
        } else {
            cx.propagate();
        }
    }

    pub(super) fn toggle_filter_action(&mut self, _: &ToggleFilter, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_filter(window, cx);
    }

    /// Close the box, show the whole tree again with the selected entry revealed, and give the tree the focus.
    pub fn close_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keep = self.selected_row().map(|r| r.path.clone());
        self.filter.field = None;
        self.filter._sub = None;
        self.set_filter_text("", cx);
        cx.emit(TreeEvent::FilterChanged(String::new()));
        window.focus(&self.focus_handle);
        if let Some(path) = keep {
            self.pending_reveal = None;
            self.reveal(&path, cx);
        }
        cx.notify();
    }

    /// Empty the box and keep it open, with the focus in it.
    fn clear_filter_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.filter.field.clone() {
            field.update(cx, |f, cx| f.set_text("", cx));
            window.focus(&field.focus_handle(cx));
        }
    }

    /// Enter in the box: the first file left takes the selection and the focus, so ↵ opens it.
    fn focus_first_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.rows.iter().position(|r| !r.is_dir) else { return };
        window.focus(&self.focus_handle);
        self.select(ix, cx);
    }

    /// The text to filter by (also sent to the other folders' trees, which have no box of their own).
    pub fn set_filter_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let query = matching::Query::new(text);
        if query == self.filter.query {
            return;
        }
        let was_active = self.filter.is_active();
        self.filter.query = query;
        if !self.filter.is_active() {
            self.filter.universe = None;
            self.filter.matches = Matches::default();
            self.filter.files = None;
            self.filter.truncated = false;
            self.filter.files_failed = false;
        } else if was_active {
            self.apply_filter();
        } else {
            self.refresh_filter(cx);
        }
        self.rebuild();
        cx.notify();
    }

    fn apply_filter(&mut self) {
        if let (Some(universe), Some(query)) = (&self.filter.universe, &self.filter.query) {
            self.filter.matches = universe.matching(query);
        }
    }

    /// The rows of the filtered tree. Until the names are arranged, the folders already opened are searched through the loaded entries.
    pub(super) fn filtered_rows(&self) -> Vec<Row> {
        let (Some(root), Some(universe)) = (&self.root, &self.filter.universe) else { return Vec::new() };
        self.filter
            .matches
            .shown
            .iter()
            .map(|&at| {
                let item = universe.item(at);
                Row {
                    path: ui::join(root, &item.rel),
                    name: item.name().to_string().into(),
                    depth: item.depth,
                    is_dir: item.is_dir,
                    loading: false,
                    error: None,
                    chain: false,
                    nest: nesting::Role::None,
                }
            })
            .collect()
    }

    /// Arrange the names again (the filter was just typed into, or the tree's folders changed). One worker at a time; changes that
    /// arrive meanwhile cause one more round.
    pub(super) fn refresh_filter(&mut self, cx: &mut Context<Self>) {
        if !self.filter.is_active() {
            return;
        }
        let Some(root) = self.root.clone() else { return };
        if self.filter.building {
            self.filter.dirty = true;
            return;
        }
        self.filter.building = true;
        self.filter.dirty = false;
        let loaded = self.loaded_entries(&root);
        let (host, sort) = (self.host.clone(), self.opts.sort);
        let excludes = cx.global::<crate::settings::Settings>().files_exclude.clone();
        let known = (!self.filter.files_failed).then(|| self.filter.files.clone()).flatten().map(|files| (files, self.filter.truncated));
        let task = cx.background_executor().spawn(async move {
            let (files, truncated, failed) = match known {
                Some((files, truncated)) => (files, truncated, false),
                None => match host.list_files(&root, FILE_LIMIT) {
                    Ok(list) => {
                        (Arc::new(list.paths.into_iter().filter(|p| !crate::globs::is_excluded(&excludes, p)).collect::<Vec<_>>()), list.truncated, false)
                    }
                    Err(_) => (Arc::new(Vec::new()), false, true),
                },
            };
            let all = files.iter().map(|p| (p.clone(), false)).chain(loaded);
            (Universe::build(all, sort), files, truncated, failed)
        });
        let generation = self.filter.generation;
        self.filter._task = Some(cx.spawn(async move |this, cx| {
            let (universe, files, truncated, failed) = task.await;
            this.update(cx, |this, cx| {
                // Another root or a new file list was asked for meanwhile: this answer is for something else.
                if this.filter.generation != generation {
                    return;
                }
                this.filter.building = false;
                if !this.filter.is_active() {
                    return;
                }
                this.filter.universe = Some(Arc::new(universe));
                this.filter.files = Some(files);
                this.filter.truncated = truncated;
                this.filter.files_failed = failed;
                this.apply_filter();
                this.rebuild();
                if this.filter.dirty {
                    this.refresh_filter(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Every entry of the folders read so far, below the root (folders opened since the file list was made are in it too).
    fn loaded_entries(&self, root: &str) -> Vec<(String, bool)> {
        let prefix = format!("{}/", root.trim_end_matches('/'));
        let mut out = Vec::new();
        for (dir, load) in &self.children {
            let Load::Loaded(entries) = load else { continue };
            let Some(rel) = (if dir == root { Some("") } else { dir.strip_prefix(&prefix) }) else { continue };
            out.extend(entries.iter().map(|e| (if rel.is_empty() { e.name.clone() } else { format!("{rel}/{}", e.name) }, e.is_dir())));
        }
        out
    }

    /// A name with the filtered text marked.
    pub(super) fn highlighted_name(&self, name: &SharedString, p: &ui::Palette) -> AnyElement {
        let range = self.filter_query().and_then(|q| q.find(name));
        match range {
            None => div().child(name.clone()).into_any_element(),
            Some(r) => div()
                .flex()
                .child(name[..r.start].to_string())
                .child(div().bg(ui::ca(p.accent, 0.32)).rounded(px(2.)).child(name[r.clone()].to_string()))
                .child(name[r.end..].to_string())
                .into_any_element(),
        }
    }

    /// The filter box with its status line, drawn above the list while the filter is open.
    pub(super) fn filter_bar(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let field = self.filter.field.clone()?;
        let p = *palette(cx);
        let focused = field.focus_handle(cx).is_focused(window);
        let text = field.read(cx).text().to_string();
        let status = self.filter_status();
        let line = match (&self.filter.query, &status) {
            (None, _) => None,
            (Some(_), s) if s.searching && s.hits == 0 => Some(crate::i18n::t("Searching…").to_string()),
            (Some(_), s) if s.hits == 0 => Some(crate::i18n::t("No matches").to_string()),
            (Some(_), s) => Some(crate::i18n::tn("{} match", "{} matches", s.hits, &[])),
        };
        let note = if status.opened_only && self.filter.query.is_some() {
            Some(crate::i18n::t("The file list could not be read: only opened folders are searched.").to_string())
        } else if status.truncated && self.filter.query.is_some() {
            Some(crate::i18n::tf("Only the first {} files are searched.", &[&FILE_LIMIT]))
        } else {
            None
        };
        let surface = crate::i18n::t("Explorer");
        let clear_label = crate::i18n::t("Clear filter");
        let field_label = crate::i18n::t("Filter files by name");
        let clear = ui::icon_button("exp-filter-clear", "x", clear_label, &p)
            .relative()
            .child(crate::a11y_controls::control(
                crate::a11y_controls::Control::button(surface, "exp-filter-clear", clear_label, |_, _| {})
                    .on_entity(cx.entity().downgrade(), |this: &mut Self, w, cx| this.clear_filter_text(w, cx)),
            ))
            .on_click(cx.listener(|this, _, w, cx| this.clear_filter_text(w, cx)));
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE[1]))
                .px(px(theme::SPACE[4]))
                .pb(px(theme::SPACE[2]))
                .child(
                    ui::input_box("exp-filter-box", &p, p.bg, focused)
                        .relative()
                        .child(ui::search_icon(&p))
                        .child(
                            div().flex_1().min_w_0().relative().child(field.clone()).child(crate::a11y_controls::control(
                                crate::a11y_controls::Control::new(surface, "exp-filter-field", accesskit::Role::SearchInput, field_label)
                                    .value(text.clone())
                                    .focus(field.focus_handle(cx)),
                            )),
                        )
                        .when(!text.is_empty(), |d| d.child(clear)),
                )
                .when_some(line, |d, line| d.child(div().text_size(px(crate::ui::text::XS.0)).text_color(c(p.text_muted)).child(line)))
                .when_some(note, |d, note| d.child(div().text_size(px(crate::ui::text::XS.0)).text_color(c(p.text_muted)).child(note)))
                .into_any_element(),
        )
    }
}
