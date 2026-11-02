//! The Search view for assistive technology: its fields, options, Replace All, the status line (an error kept apart, so
//! it can be announced at once) and the results as a tree of files and matches, with where each part was painted. The
//! actions go through the view's own paths: typing in a field (then searching, as typing does), a toggle, a click on a
//! row or a row's button. A row is named by its file (and line and column for a match), so an action from an outdated
//! tree acts only on a row that is still a result.

use std::collections::HashMap;

use super::*;

/// A search option and the toggle that shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SearchOption {
    Case,
    Word,
    Regex,
    Preserve,
    Structural,
}

impl SearchOption {
    fn of(id: &str) -> Option<SearchOption> {
        Some(match id {
            "s-case" => SearchOption::Case,
            "s-word" => SearchOption::Word,
            "s-regex" => SearchOption::Regex,
            "s-preserve" => SearchOption::Preserve,
            "s-structural" => SearchOption::Structural,
            _ => return None,
        })
    }
}

/// A row of the results: a file (by its result path) or a match (path, line, column).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AccessKey {
    File(String),
    Match(String, u32, u32),
}

/// A field of the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AccessField {
    Query,
    Replace,
    Include,
    Exclude,
}

/// A button of a file row.
pub(crate) const REPLACE_IN_FILE: u8 = 1;
pub(crate) const DISMISS_FILE: u8 = 2;

/// What of the view has a frame in the accessibility tree.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AccessFrame {
    Field(AccessField),
    Option(SearchOption),
    ReplaceAll,
    Globs,
    Status,
    Row(AccessKey),
    Button(AccessKey, u8),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AccessRow {
    pub key: AccessKey,
    /// File name, or the match's line text.
    pub label: String,
    /// The file's folder, or "line 3, column 5".
    pub detail: String,
    /// A file: its match count and whether its matches are shown.
    pub count: usize,
    pub expanded: bool,
    /// The match last opened (F4 steps from it).
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AccessView {
    pub structural: bool,
    pub query: String,
    pub replace: String,
    /// Include and exclude globs, while their fields are shown.
    pub globs: Option<(String, String)>,
    pub options: Vec<(SearchOption, bool)>,
    /// The status line ("3 results in 2 files", "Searching…"); `error`: a search or pattern error.
    pub status: Option<String>,
    pub error: Option<String>,
    pub can_replace: bool,
    /// The exposed rows (from row `first` of `total`).
    pub rows: Vec<AccessRow>,
    pub first: usize,
    pub total: usize,
    pub focus: Option<AccessField>,
    pub frames: HashMap<AccessFrame, accesskit::Rect>,
}

impl SearchPanel {
    /// The status line and whether it is an error (`None` while there is nothing to say).
    pub(super) fn status_line(&self, cx: &App) -> Option<(String, bool)> {
        match &self.status {
            Status::Idle if self.root.is_none() => Some((crate::i18n::t("Open a folder to search in it.").into(), false)),
            Status::Idle => None,
            Status::Searching => Some((crate::i18n::t("Searching…").into(), false)),
            Status::Done { matches: 0, files, .. } => Some((crate::i18n::tn("No results in {} file.", "No results in {} files.", *files as usize, &[]), false)),
            Status::Done { files: _, matches, truncated } => {
                let n = self.results.len();
                let mut s = if n == 1 {
                    crate::i18n::tn("{} result in 1 file", "{} results in 1 file", *matches as usize, &[])
                } else {
                    crate::i18n::tn("{0} result in {1} files", "{0} results in {1} files", *matches as usize, &[&n])
                };
                if *truncated {
                    s = crate::i18n::tf("{0} (stopped at {1})", &[&s, &cx.global::<crate::settings::Settings>().search_max_results]);
                }
                Some((s, false))
            }
            Status::Failed(e) => Some((e.to_string(), true)),
        }
    }

    /// Records where `frame` is laid out (also while it is hidden until the pointer is over its row).
    pub(super) fn probe(&self, frame: AccessFrame) -> impl IntoElement + use<> {
        crate::painted::layout_probe(&self.painted, frame)
    }

    /// Records where the toggle `id` is laid out.
    pub(super) fn option_probe(&self, id: &str) -> Option<impl IntoElement + use<>> {
        SearchOption::of(id).map(|o| self.probe(AccessFrame::Option(o)))
    }

    fn option(&self, o: SearchOption) -> bool {
        match o {
            SearchOption::Case => self.case_sensitive,
            SearchOption::Word => self.whole_word,
            SearchOption::Regex => self.regex,
            SearchOption::Preserve => self.preserve_case,
            SearchOption::Structural => self.st.on,
        }
    }

    /// The view as the accessibility tree shows it: at most `max` rows, around the current match while there is one,
    /// otherwise from the top of what is on screen.
    pub(crate) fn access_view(&self, max: usize, window: &Window, cx: &App) -> AccessView {
        let field = |f: &Entity<TextField>| f.read(cx).text().to_string();
        let total = self.rows.len();
        let current = self.current.and_then(|c| self.rows.iter().position(|r| matches!(r, Row::Match(a, b) if (*a, *b) == c)));
        let top = self.scroll.0.borrow().base_handle.offset().y;
        let visible = (-f32::from(top) / layout::ROW_H).max(0.) as usize;
        let first = crate::workbench::a11y::window_start(total, max, current, Some((visible, visible)));
        let rows = self.rows[first..(first + max).min(total)]
            .iter()
            .filter_map(|row| match *row {
                Row::File(fi) => {
                    let f = self.results.get(fi)?;
                    let (dir, name) = f.path.rsplit_once('/').unwrap_or(("", &f.path));
                    Some(AccessRow {
                        key: AccessKey::File(f.path.clone()),
                        label: name.to_string(),
                        detail: dir.to_string(),
                        count: f.matches.len(),
                        expanded: !f.collapsed,
                        current: false,
                    })
                }
                Row::Match(fi, mi) => {
                    let f = self.results.get(fi)?;
                    let m = f.matches.get(mi)?;
                    Some(AccessRow {
                        key: AccessKey::Match(f.path.clone(), m.line, m.column),
                        label: m.preview.trim().to_string(),
                        detail: crate::i18n::tf("line {0}, column {1}", &[&m.line, &(m.column + 1)]),
                        count: 0,
                        expanded: false,
                        current: self.current == Some((fi, mi)),
                    })
                }
            })
            .collect();
        let (status, error) = match self.status_line(cx) {
            Some((s, true)) => (None, Some(s)),
            other => (other.map(|(s, _)| s), None),
        };
        let error = error.or_else(|| self.st.error.clone().filter(|_| self.st.on));
        let focused = |f: &Entity<TextField>| f.focus_handle(cx).is_focused(window);
        let focus = [
            (AccessField::Query, self.active_query()),
            (AccessField::Replace, &self.replace),
            (AccessField::Include, &self.include),
            (AccessField::Exclude, &self.exclude),
        ]
        .into_iter()
        .find(|(_, f)| focused(f))
        .map(|(k, _)| k);
        let mut options: Vec<SearchOption> =
            if self.st.on { vec![] } else { vec![SearchOption::Case, SearchOption::Word, SearchOption::Regex, SearchOption::Preserve] };
        options.push(SearchOption::Structural);
        let scale = window.scale_factor();
        AccessView {
            structural: self.st.on,
            query: field(self.active_query()),
            replace: field(&self.replace),
            globs: self.show_globs.then(|| (field(&self.include), field(&self.exclude))),
            options: options.into_iter().map(|o| (o, self.option(o))).collect(),
            status,
            error,
            can_replace: matches!(self.status, Status::Done { matches, .. } if matches > 0),
            rows,
            first,
            total,
            focus,
            frames: self.painted.borrow().iter().filter_map(|(k, b)| Some((k.clone(), crate::painted::physical_scaled(*b, scale)?))).collect(),
        }
    }

    fn field(&self, which: AccessField) -> &Entity<TextField> {
        match which {
            AccessField::Query => self.active_query(),
            AccessField::Replace => &self.replace,
            AccessField::Include => &self.include,
            AccessField::Exclude => &self.exclude,
        }
    }

    /// The screen reader writes a field: as typing it, so the query, include and exclude fields search again.
    pub(crate) fn a11y_set_field(&mut self, which: AccessField, text: &str, cx: &mut Context<Self>) {
        if matches!(which, AccessField::Include | AccessField::Exclude) && !self.show_globs {
            return;
        }
        self.field(which).clone().update(cx, |f, cx| f.set_text(text, cx));
        if which != AccessField::Replace {
            self.start(cx);
        }
        cx.notify();
    }

    pub(crate) fn a11y_focus_field(&self, which: AccessField, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.field(which).focus_handle(cx));
    }

    /// A toggle pressed from the tree, as a click on it: only from the state the tree showed.
    pub(crate) fn a11y_toggle(&mut self, o: SearchOption, was: bool, cx: &mut Context<Self>) {
        if self.option(o) != was || (self.st.on && o != SearchOption::Structural) {
            return;
        }
        match o {
            SearchOption::Case => self.case_sensitive = !self.case_sensitive,
            SearchOption::Word => self.whole_word = !self.whole_word,
            SearchOption::Regex => self.regex = !self.regex,
            SearchOption::Preserve => self.preserve_case = !self.preserve_case,
            SearchOption::Structural => self.st.on = !self.st.on,
        }
        self.start(cx);
    }

    /// "Files to include / exclude" pressed from the tree, from the state it showed.
    pub(crate) fn a11y_toggle_globs(&mut self, was: bool, cx: &mut Context<Self>) {
        if self.show_globs == was {
            self.show_globs = !self.show_globs;
            self.start(cx);
        }
    }

    /// Scroll action on the results: `pages` of 100 rows down (up when negative), which moves the exposed range.
    pub(crate) fn a11y_scroll(&mut self, pages: isize, cx: &mut Context<Self>) {
        let top = (-f32::from(self.scroll.0.borrow().base_handle.offset().y) / layout::ROW_H).max(0.) as isize;
        let last = self.rows.len().saturating_sub(1) as isize;
        let row = (top + pages * 100).clamp(0, last.max(0)) as usize;
        self.scroll.scroll_to_item_strict(row, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    fn file_index(&self, path: &str) -> Option<usize> {
        self.results.iter().position(|f| f.path == path)
    }

    /// A file row: show or hide its matches (`open`: the state wanted; `None` toggles from `was`).
    pub(crate) fn a11y_expand(&mut self, path: &str, open: bool, cx: &mut Context<Self>) {
        let Some(fi) = self.file_index(path) else { return };
        if self.results[fi].collapsed == open {
            self.results[fi].collapsed = !open;
            self.rebuild_rows();
            cx.notify();
        }
    }

    /// A match row: open it as a click does (it becomes the current match). Nothing when it is no longer a result.
    pub(crate) fn a11y_open(&mut self, path: &str, line: u32, column: u32, cx: &mut Context<Self>) -> bool {
        let Some(fi) = self.file_index(path) else { return false };
        let Some(mi) = self.results[fi].matches.iter().position(|m| m.line == line && m.column == column) else { return false };
        self.current = Some((fi, mi));
        self.open(fi, mi, cx);
        cx.notify();
        true
    }

    /// A file row's button: Replace in this file, or Dismiss it from the results.
    pub(crate) fn a11y_file_button(&mut self, path: &str, button: u8, cx: &mut Context<Self>) {
        let Some(fi) = self.file_index(path) else { return };
        match button {
            REPLACE_IN_FILE => self.replace_in_file(fi, cx),
            DISMISS_FILE => self.dismiss_file(fi, cx),
            _ => {}
        }
    }
}
