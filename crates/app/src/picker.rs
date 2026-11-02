//! QuickPick: floating list at the top, filtered as you type. Holds several sources;
//! the query prefix picks the active one ("" files, ">" commands,
//! ":" line). The theme picker uses a single source.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, BoxShadow, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle, MouseButton, Pixels, Point, ScrollStrategy,
    SharedString, StyledText, UTF16Selection, UniformListScrollHandle, Window, actions, canvas, div, point, prelude::*, px, uniform_list,
};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::fuzzy;
use crate::ui::{c, ca, layout, palette, text};

#[cfg(test)]
#[path = "picker_tests.rs"]
mod tests;

actions!(picker, [PickPrev, PickNext, PickConfirm, PickDismiss, PickBackspace, PickSelectAll, PickPaste, PickCopy, PickCut]);

#[derive(Clone)]
pub struct PickItem {
    pub label: SharedString,
    pub detail: Option<SharedString>,
    /// Shortcut ("⌘K ⌘T"): shown as key chips on the right of the row.
    pub keys: Option<SharedString>,
}

pub enum Source {
    /// Short list (commands, themes): `fuzzy::score`.
    List { prefix: &'static str, items: Vec<PickItem> },
    /// Relative file paths: nucleo, path-aware. The list arrives in the background.
    Paths { prefix: &'static str, paths: Arc<Vec<String>>, folded: Arc<Vec<String>>, loading: bool, truncated: bool },
    /// Line number (at most `max`; 0 = no open editor).
    Line { prefix: &'static str, max: usize },
    /// Free text (like a file name). `verb`: "Create file" → `Create file “name”`.
    /// `path`: may contain "/" (a path in a subfolder).
    Input { verb: &'static str, path: bool },
    /// Hidden input (password): masked on screen, may be submitted empty.
    Secret { verb: &'static str },
    /// Results come from outside (the language server's symbols): `PickerEvent::Query` fires on every
    /// query change, results arrive via `set_remote`. Order is kept,
    /// results of the old query are also filtered locally. `status`: row shown while the list is empty.
    Remote { prefix: &'static str, items: Vec<PickItem>, status: SharedString },
    /// Folder path input (remote "Open Folder"): the query is the path itself; the first row opens
    /// the typed path (`Pick::Input`), below it the subfolders from outside (not filtered locally, the caller
    /// already filtered by the typed prefix). `PickerEvent::Query(path)` on every query change.
    Dirs { items: Vec<PickItem>, status: SharedString },
}

impl Source {
    pub fn paths(prefix: &'static str) -> Self {
        Source::Paths { prefix, paths: Arc::default(), folded: Arc::default(), loading: true, truncated: false }
    }

    fn prefix(&self) -> &'static str {
        match self {
            Source::List { prefix, .. } | Source::Paths { prefix, .. } | Source::Line { prefix, .. } | Source::Remote { prefix, .. } => prefix,
            Source::Input { .. } | Source::Secret { .. } | Source::Dirs { .. } => "",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Item {
        source: usize,
        index: usize,
    },
    /// Line (1-based) and optional column ("12:5").
    Line(usize, Option<usize>),
    /// Input confirmed; read the text with `Picker::query`.
    Input,
}

pub enum PickerEvent {
    /// Selection changed (live preview).
    Preview(Pick),
    Confirm(Pick),
    Dismiss,
    /// Active source is `Remote` and the query changed (prefix stripped, trimmed).
    Query(String),
}

/// What the accessibility tree (a11y.rs) shows of a picker.
#[derive(Clone, PartialEq, Debug)]
pub struct PickerA11y {
    pub placeholder: String,
    pub message: Option<String>,
    /// The query, masked for a secret.
    pub query: String,
    /// (label, detail) of the first rows.
    pub rows: Vec<(String, Option<String>)>,
    pub rows_total: usize,
    pub selected: Option<usize>,
    /// Shown instead of rows when there are none.
    pub empty: Option<String>,
    /// Where the picker, its query box, its list and each of the first rows were painted (physical pixels).
    pub bounds: Option<accesskit::Rect>,
    pub input: Option<accesskit::Rect>,
    pub list: Option<accesskit::Rect>,
    pub frames: Vec<Option<accesskit::Rect>>,
}

/// A part of the picker whose painted bounds the accessibility tree reports.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum PickFrame {
    Picker,
    Input,
    List,
    Row(usize),
}

struct Row {
    pick: Option<Pick>,
    label: SharedString,
    detail: Option<SharedString>,
    /// Character indices to highlight in the label.
    hits: Vec<usize>,
    /// File row: file icon before the name.
    file: bool,
    keys: Option<SharedString>,
}

pub struct Picker {
    sources: Vec<Source>,
    /// Description above the query box (e.g. ssh's prompt).
    message: Option<SharedString>,
    /// Extra search text per source (in item order): checked if the label doesn't match (e.g. the English name of a translated command).
    aliases: std::collections::HashMap<usize, Vec<SharedString>>,
    placeholder: SharedString,
    query: String,
    /// Initial text selected in a name input: the first keystroke or delete replaces it.
    initial_selected: bool,
    marked: Option<Range<usize>>,
    rows: Vec<Row>,
    selected: usize,
    matcher: Matcher,
    /// Number of recently opened files at the head of the file list (ranked first).
    recent: usize,
    /// With several folders open: the first folder's name, so its files are found as `folder/path` like the others'.
    first_folder: Option<String>,
    /// ":line(:column)" in the query (jump there when a file is picked).
    goto: Option<(usize, Option<usize>)>,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Where its parts were painted in the last frame (accessibility).
    painted: crate::painted::WindowBounds<PickFrame>,
}

impl EventEmitter<PickerEvent> for Picker {}

impl Focusable for Picker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

const MAX_ROWS: usize = 12;
const MAX_PATH_RESULTS: usize = 200;
const ROW_H: f32 = 24.;

impl Picker {
    pub fn new(sources: Vec<Source>, placeholder: &str, query: &str, initial: Option<Pick>, cx: &mut Context<Self>) -> Self {
        let mut p = Picker {
            sources,
            message: None,
            aliases: std::collections::HashMap::new(),
            placeholder: placeholder.to_string().into(),
            query: query.to_string(),
            initial_selected: false,
            marked: None,
            rows: Vec::new(),
            selected: 0,
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            recent: 0,
            first_folder: None,
            goto: None,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            painted: Default::default(),
        };
        p.initial_selected = !p.query.is_empty() && p.sources.iter().any(|s| matches!(s, Source::Input { .. }));
        p.refilter();
        if let Some(initial) = initial {
            p.selected = p.rows.iter().position(|r| r.pick == Some(initial)).unwrap_or(0);
        }
        p.scroll.scroll_to_item(p.selected, ScrollStrategy::Center);
        p
    }

    pub fn with_message(mut self, message: impl Into<SharedString>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Extra search text for the items in `source` (`aliases[i]` = alias of item i); only the label gets highlighted.
    pub fn with_aliases(mut self, source: usize, aliases: Vec<SharedString>) -> Self {
        self.aliases.insert(source, aliases);
        self.refilter();
        self
    }

    fn masked(&self) -> bool {
        self.sources.iter().any(|s| matches!(s, Source::Secret { .. }))
    }

    /// File list arrived (or was refreshed).
    pub fn set_paths(&mut self, source: usize, new_paths: Vec<String>, was_truncated: bool, cx: &mut Context<Self>) {
        if let Some(Source::Paths { paths, folded, loading, truncated, .. }) = self.sources.get_mut(source) {
            // An extra folder's files are `folder › path`; they are searched as `folder / path`, so `folder/path` (as
            // typed) finds them. The separator keeps its length, so the highlighted positions stay where they are.
            // The first folder's files are searched as `first / path` when there are several folders.
            let first = self.first_folder.as_ref().map(|f| format!("{f} / "));
            let searched = |p: &String| match (&first, p.contains(crate::workbench::ROOT_SEP)) {
                (_, true) => p.replacen(crate::workbench::ROOT_SEP, " / ", 1),
                (Some(first), false) => format!("{first}{p}"),
                (None, false) => p.clone(),
            };
            *folded = Arc::new(new_paths.iter().map(|p| searched(p).chars().map(fuzzy::fold).collect()).collect());
            *paths = Arc::new(new_paths);
            *loading = false;
            *truncated = was_truncated;
            self.refilter();
            cx.notify();
        }
    }

    /// Results from outside (and the status row shown when empty).
    pub fn set_remote(&mut self, source: usize, new_items: Vec<PickItem>, new_status: impl Into<SharedString>, cx: &mut Context<Self>) {
        if let Some(Source::Remote { items, status, .. }) = self.sources.get_mut(source) {
            *items = new_items;
            *status = new_status.into();
            self.refilter();
            cx.notify();
        }
    }

    /// Subfolder list for the folder input (and the status shown when empty).
    pub fn set_dirs(&mut self, new_items: Vec<PickItem>, new_status: impl Into<SharedString>, cx: &mut Context<Self>) {
        let keep = self.selected;
        if let Some(Source::Dirs { items, status }) = self.sources.first_mut() {
            *items = new_items;
            *status = new_status.into();
            self.refilter();
            self.selected = keep.min(self.rows.len().saturating_sub(1));
            cx.notify();
        }
    }

    /// The program changes the query (descending into a folder, completion); a `Query` event goes out.
    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.marked = None;
        self.initial_selected = false;
        self.query_changed(cx);
    }

    /// Query of the active source if it is `Remote`.
    fn remote_query(&self) -> Option<String> {
        let si = self.active_source()?;
        match &self.sources[si] {
            Source::Dirs { .. } => Some(self.query.clone()),
            Source::Remote { prefix, .. } => Some(self.query[prefix.len()..].trim().to_string()),
            _ => None,
        }
    }

    /// The first `n` of the file list are the recently opened ones.
    pub fn set_recent(&mut self, n: usize) {
        self.recent = n;
    }

    /// Several folders are open and `name` is the first one's (call before `set_paths`).
    pub fn set_first_folder(&mut self, name: Option<String>) {
        self.first_folder = name;
    }

    /// ":line(:column)" in the query.
    pub fn goto(&self) -> Option<(usize, Option<usize>)> {
        self.goto
    }

    /// Labels of the visible rows (for tests).
    #[cfg(test)]
    pub fn labels(&self) -> Vec<String> {
        self.rows.iter().map(|r| r.label.to_string()).collect()
    }

    #[cfg(test)]
    pub fn details(&self) -> Vec<String> {
        self.rows.iter().map(|r| r.detail.as_ref().map_or(String::new(), |d| d.to_string())).collect()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// The query as the screen shows it: dots for a secret.
    pub fn shown_query(&self) -> String {
        if self.masked() { "•".repeat(self.query.chars().count()) } else { self.query.clone() }
    }

    /// For the accessibility tree (a11y.rs): at most `limit` rows, with where they were painted in `window`.
    pub fn a11y_view(&self, limit: usize, window: &Window) -> PickerA11y {
        let frame = |k: PickFrame| self.painted.physical(&k, window);
        PickerA11y {
            bounds: frame(PickFrame::Picker),
            input: frame(PickFrame::Input),
            list: frame(PickFrame::List),
            frames: (0..self.rows.len().min(limit)).map(|i| frame(PickFrame::Row(i))).collect(),
            placeholder: self.placeholder.to_string(),
            message: self.message.as_ref().map(|m| m.to_string()),
            query: self.shown_query(),
            rows: self.rows.iter().take(limit).map(|r| (r.label.to_string(), r.detail.as_ref().map(|d| d.to_string()))).collect(),
            rows_total: self.rows.len(),
            selected: (!self.rows.is_empty()).then_some(self.selected),
            empty: self.rows.is_empty().then(|| self.empty_message().to_string()),
        }
    }

    /// Accessibility action: select row `ix`, and pick it if `confirm`.
    pub fn a11y_choose(&mut self, ix: usize, confirm: bool, cx: &mut Context<Self>) {
        if ix >= self.rows.len() {
            return;
        }
        self.select(ix, cx);
        if confirm && let Some(pick) = self.rows[self.selected].pick {
            cx.emit(PickerEvent::Confirm(pick));
        }
    }

    /// Path from the file source (after Confirm).
    pub fn path_at(&self, source: usize, index: usize) -> Option<String> {
        match self.sources.get(source)? {
            Source::Paths { paths, .. } => paths.get(index).cloned(),
            _ => None,
        }
    }

    /// Source whose prefix is the longest match for the query.
    fn active_source(&self) -> Option<usize> {
        self.sources.iter().enumerate().filter(|(_, s)| self.query.starts_with(s.prefix())).max_by_key(|(_, s)| s.prefix().len()).map(|(i, _)| i)
    }

    fn refilter(&mut self) {
        self.rows.clear();
        self.selected = 0;
        let Some(si) = self.active_source() else { return };
        let q = self.query[self.sources[si].prefix().len()..].trim().to_string();
        match &self.sources[si] {
            Source::List { items, .. } => {
                let alias = self.aliases.get(&si);
                let mut scored: Vec<(i32, usize, Vec<usize>)> = items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, it)| match fuzzy::score(&q, &it.label) {
                        Some((s, hits)) => Some((s, i, hits)),
                        None => alias.and_then(|a| a.get(i)).and_then(|a| fuzzy::score(&q, a)).map(|(s, _)| (s, i, Vec::new())),
                    })
                    .collect();
                if !q.is_empty() {
                    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                }
                self.rows = scored
                    .into_iter()
                    .map(|(_, i, hits)| Row {
                        pick: Some(Pick::Item { source: si, index: i }),
                        label: items[i].label.clone(),
                        detail: items[i].detail.clone(),
                        hits,
                        file: false,
                        keys: items[i].keys.clone(),
                    })
                    .collect();
            }
            Source::Paths { paths, folded, .. } => {
                let (paths, folded) = (paths.clone(), folded.clone());
                // "lib.rs:12" or "lib.rs:12:5": open the file, go to that position.
                let (q, goto) = match q.rsplit_once(':').filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
                    Some((head, tail)) => match head.rsplit_once(':').filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
                        Some((file, line)) => (file.to_string(), Some((line.parse().unwrap_or(1), tail.parse().ok()))),
                        None => (head.to_string(), Some((tail.parse().unwrap_or(1), None))),
                    },
                    None => (q.clone(), None),
                };
                self.goto = goto;
                let recent = self.recent;
                let picks: Vec<(usize, Vec<usize>)> = if q.is_empty() {
                    (0..paths.len().min(MAX_PATH_RESULTS)).map(|i| (i, Vec::new())).collect()
                } else {
                    let folded_q: String = q.chars().map(fuzzy::fold).collect();
                    let pattern = Pattern::parse(&folded_q, CaseMatching::Ignore, Normalization::Smart);
                    let mut buf = Vec::new();
                    // Score only first (fast), then highlight positions for the best ones.
                    let mut scored: Vec<(u32, usize)> =
                        folded.iter().enumerate().filter_map(|(i, f)| pattern.score(Utf32Str::new(f, &mut buf), &mut self.matcher).map(|s| (s, i))).collect();
                    // Matching recents first (like VS Code), then score, then shorter path.
                    scored.sort_unstable_by(|a, b| (b.1 < recent).cmp(&(a.1 < recent)).then(b.0.cmp(&a.0)).then(paths[a.1].len().cmp(&paths[b.1].len())));
                    scored.truncate(MAX_PATH_RESULTS);
                    scored
                        .into_iter()
                        .map(|(_, i)| {
                            let mut idx = Vec::new();
                            pattern.indices(Utf32Str::new(&folded[i], &mut buf), &mut self.matcher, &mut idx);
                            idx.sort_unstable();
                            idx.dedup();
                            (i, idx.into_iter().map(|x| x as usize).collect())
                        })
                        .collect()
                };
                self.rows = picks
                    .into_iter()
                    .map(|(i, hits)| {
                        let path = &paths[i];
                        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
                        // Highlight spans the whole searched text (with the first folder's name in front, if added);
                        // the label is only the file name.
                        let name_start = folded[i].chars().count() - name.chars().count();
                        Row {
                            pick: Some(Pick::Item { source: si, index: i }),
                            label: name.to_string().into(),
                            detail: (!dir.is_empty()).then(|| dir.to_string().into()),
                            hits: hits.into_iter().filter(|&h| h >= name_start).map(|h| h - name_start).collect(),
                            file: true,
                            keys: None,
                        }
                    })
                    .collect();
            }
            Source::Remote { items, status, .. } => {
                self.rows = items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, it)| {
                        let (_, hits) = fuzzy::score(&q, &it.label)?;
                        Some(Row {
                            pick: Some(Pick::Item { source: si, index: i }),
                            label: it.label.clone(),
                            detail: it.detail.clone(),
                            hits,
                            file: false,
                            keys: None,
                        })
                    })
                    .collect();
                if self.rows.is_empty() {
                    self.rows.push(Row { pick: None, label: status.clone(), detail: None, hits: Vec::new(), file: false, keys: None });
                }
            }
            Source::Dirs { items, status } => {
                let path = self.query.trim();
                if !path.is_empty() {
                    self.rows.push(Row {
                        pick: Some(Pick::Input),
                        label: crate::i18n::tf("Open “{}”", &[&path]).into(),
                        detail: None,
                        hits: Vec::new(),
                        file: false,
                        keys: None,
                    });
                }
                for (i, it) in items.iter().enumerate() {
                    self.rows.push(Row {
                        pick: Some(Pick::Item { source: si, index: i }),
                        label: it.label.clone(),
                        detail: it.detail.clone(),
                        hits: Vec::new(),
                        file: false,
                        keys: None,
                    });
                }
                if items.is_empty() {
                    self.rows.push(Row { pick: None, label: status.clone(), detail: None, hits: Vec::new(), file: false, keys: None });
                }
            }
            Source::Secret { verb } => {
                self.rows.push(Row { pick: Some(Pick::Input), label: (*verb).into(), detail: None, hits: Vec::new(), file: false, keys: None });
            }
            Source::Input { verb, path } => {
                let name = self.query.trim();
                let bad_path = *path && (name.ends_with('/') || name.split('/').any(|p| p == "." || p == ".."));
                self.rows.push(if name.is_empty() {
                    Row { pick: None, label: crate::i18n::t("Type a name").into(), detail: None, hits: Vec::new(), file: false, keys: None }
                } else if bad_path {
                    Row {
                        pick: None,
                        label: crate::i18n::t("Paths can't end with / or contain . or .. parts").into(),
                        detail: None,
                        hits: Vec::new(),
                        file: false,
                        keys: None,
                    }
                } else if !*path && (name.contains('/') || name == "." || name == "..") {
                    Row {
                        pick: None,
                        label: crate::i18n::t("Names can't contain / or be . or ..").into(),
                        detail: None,
                        hits: Vec::new(),
                        file: false,
                        keys: None,
                    }
                } else {
                    Row { pick: Some(Pick::Input), label: format!("{verb} “{name}”").into(), detail: None, hits: Vec::new(), file: false, keys: None }
                });
            }
            Source::Line { max, .. } => {
                let max = *max;
                let (line, col) = parse_position(&q);
                self.rows.push(match (max, line) {
                    (0, _) => Row { pick: None, label: crate::i18n::t("Open a file first").into(), detail: None, hits: Vec::new(), file: false, keys: None },
                    (_, Some(n)) if n >= 1 => Row {
                        pick: Some(Pick::Line(n.min(max), col)),
                        label: match col {
                            Some(c) => crate::i18n::tf("Go to line {0}, column {1}", &[&n.min(max), &c]),
                            None => crate::i18n::tf("Go to line {}", &[&n.min(max)]),
                        }
                        .into(),
                        detail: Some(crate::i18n::tf("of {}", &[&max]).into()),
                        hits: Vec::new(),
                        file: false,
                        keys: None,
                    },
                    _ => Row {
                        pick: None,
                        label: crate::i18n::tf("Type a line number between 1 and {}", &[&max]).into(),
                        detail: None,
                        hits: Vec::new(),
                        file: false,
                        keys: None,
                    },
                });
            }
        }
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = ix.min(self.rows.len() - 1);
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Top);
        if let Some(pick) = self.rows[self.selected].pick {
            cx.emit(PickerEvent::Preview(pick));
        }
        cx.notify();
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        self.refilter();
        if let Some(q) = self.remote_query() {
            cx.emit(PickerEvent::Query(q));
        }
        if let Some(pick) = self.rows.first().and_then(|r| r.pick) {
            cx.emit(PickerEvent::Preview(pick));
        }
        cx.notify();
    }

    fn prev(&mut self, _: &PickPrev, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.rows.len().max(1);
        self.select((self.selected + n - 1) % n, cx);
    }

    fn next(&mut self, _: &PickNext, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.rows.len().max(1);
        self.select((self.selected + 1) % n, cx);
    }

    fn confirm(&mut self, _: &PickConfirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(pick) = self.rows.get(self.selected).and_then(|r| r.pick) {
            cx.emit(PickerEvent::Confirm(pick));
        }
    }

    fn dismiss(&mut self, _: &PickDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PickerEvent::Dismiss);
    }

    fn backspace(&mut self, _: &PickBackspace, _: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.initial_selected) {
            self.query.clear();
            self.marked = None;
            self.query_changed(cx);
        } else if self.query.pop().is_some() {
            self.marked = None;
            self.query_changed(cx);
        }
    }

    fn select_all(&mut self, _: &PickSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.initial_selected = !self.query.is_empty();
        self.marked = None;
        cx.notify();
    }

    fn paste(&mut self, _: &PickPaste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &PickCopy, _: &mut Window, cx: &mut Context<Self>) {
        if self.initial_selected && !self.masked() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(self.query.clone()));
        }
    }

    fn cut(&mut self, _: &PickCut, window: &mut Window, cx: &mut Context<Self>) {
        if self.initial_selected && !self.masked() {
            self.copy(&PickCopy, window, cx);
            self.backspace(&PickBackspace, window, cx);
        }
    }

    fn utf16_len(&self) -> usize {
        self.query.encode_utf16().count()
    }

    fn byte_of_utf16(&self, cu: usize) -> usize {
        let mut units = 0;
        for (b, ch) in self.query.char_indices() {
            if units >= cu {
                return b;
            }
            units += ch.len_utf16();
        }
        self.query.len()
    }

    fn empty_message(&self) -> &'static str {
        match self.active_source().map(|i| &self.sources[i]) {
            Some(Source::Paths { loading: true, .. }) => crate::i18n::t("Indexing files…"),
            Some(Source::Paths { paths, .. }) if paths.is_empty() => crate::i18n::t("No files in this folder"),
            _ => crate::i18n::t("No matches"),
        }
    }

    fn footer(&self) -> Option<&'static str> {
        match self.active_source().map(|i| &self.sources[i]) {
            Some(Source::Paths { truncated: true, .. }) => Some(crate::i18n::t("Showing the first 100,000 files of a very large folder")),
            _ => None,
        }
    }
}

/// The query box is only appended to; `marked` is kept for IME composition.
impl EntityInputHandler for Picker {
    fn text_for_range(&mut self, r: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let (a, b) = (self.byte_of_utf16(r.start), self.byte_of_utf16(r.end));
        actual.replace(r);
        Some(self.query[a..b].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let n = self.utf16_len();
        Some(UTF16Selection { range: if self.initial_selected { 0..n } else { n..n }, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| {
            let to16 = |b: usize| self.query[..b].encode_utf16().count();
            to16(m.start)..to16(m.end)
        })
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, new: &str, _: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.initial_selected) {
            self.query.clear();
        }
        let r = range.map(|r| self.byte_of_utf16(r.start)..self.byte_of_utf16(r.end)).or(self.marked.take()).unwrap_or(self.query.len()..self.query.len());
        self.query.replace_range(r, &new.replace(['\r', '\n'], ""));
        self.marked = None;
        self.query_changed(cx);
    }

    fn replace_and_mark_text_in_range(&mut self, range: Option<Range<usize>>, new: &str, _sel: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.initial_selected) {
            self.query.clear();
        }
        let r = range.map(|r| self.byte_of_utf16(r.start)..self.byte_of_utf16(r.end)).or(self.marked.clone()).unwrap_or(self.query.len()..self.query.len());
        let start = r.start;
        self.query.replace_range(r, new);
        self.marked = (!new.is_empty()).then(|| start..start + new.len());
        self.query_changed(cx);
    }

    fn bounds_for_range(&mut self, _: Range<usize>, b: gpui::Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<gpui::Bounds<Pixels>> {
        Some(b)
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Render for Picker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let entity = cx.entity();
        self.painted.begin(window);
        let focus = self.focus_handle.clone();
        let rows = self.rows.len().min(MAX_ROWS);
        let selected = self.selected;
        let footer = self.footer();

        div()
            .key_context("Picker")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::prev))
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .relative()
            .child(self.painted.probe(PickFrame::Picker))
            .w(px(layout::PALETTE_W))
            .flex()
            .flex_col()
            .p(px(theme::SPACE[3]))
            .gap(px(theme::SPACE[2]))
            .bg(c(p.surface_raised))
            .border_1()
            .border_color(c(p.border))
            .rounded(px(theme::radius::LG))
            .shadow(vec![BoxShadow { color: ca(0x000000, 0.45).into(), offset: point(px(0.), px(6.)), blur_radius: px(20.), spread_radius: px(0.) }])
            .text_size(px(text::SM.0))
            .when_some(self.message.clone(), |d, m| d.child(div().px(px(theme::SPACE[2])).text_color(c(p.text)).whitespace_normal().child(m)))
            // Query box
            .child(
                div()
                    .relative()
                    .h(px(32.))
                    .px(px(theme::SPACE[3]))
                    .flex()
                    .items_center()
                    .bg(c(p.bg))
                    .border_1()
                    .border_color(c(p.accent))
                    .rounded(px(theme::radius::SM))
                    .map(|d| {
                        let caret = div().w(px(1.5)).h(px(16.)).bg(c(p.accent));
                        if self.query.is_empty() {
                            // Cursor at the start, placeholder dimmed.
                            d.child(caret).child(div().text_color(c(p.text_muted)).child(self.placeholder.clone()))
                        } else {
                            let shown = self.shown_query();
                            if self.initial_selected {
                                d.text_color(c(p.text)).child(div().bg(c(p.selection)).child(shown)).child(caret)
                            } else {
                                d.text_color(c(p.text)).child(shown).child(caret)
                            }
                        }
                    })
                    .child(self.painted.probe(PickFrame::Input))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, cx| {
                                window.handle_input(&focus, gpui::ElementInputHandler::new(bounds, entity), cx);
                            },
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .child(if self.rows.is_empty() {
                div()
                    .px(px(theme::SPACE[4]))
                    .w_full()
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .text_color(c(p.text_muted))
                    .child(self.empty_message())
                    .into_any_element()
            } else {
                let list = uniform_list(
                    "picker-rows",
                    self.rows.len(),
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        range
                            .map(|ix| {
                                let row = &this.rows[ix];
                                let highlights = fuzzy::hit_ranges(&row.label, &row.hits).into_iter().map(|r| {
                                    (r, HighlightStyle { color: Some(c(p.accent).into()), font_weight: Some(FontWeight::BOLD), ..Default::default() })
                                });
                                div()
                                    .id(ix)
                                    .relative()
                                    .child(this.painted.probe(PickFrame::Row(ix)))
                                    .w_full()
                                    .h(px(ROW_H))
                                    .flex()
                                    .items_center()
                                    .gap(px(theme::SPACE[4]))
                                    .px(px(theme::SPACE[4]))
                                    .rounded(px(theme::radius::SM))
                                    .text_color(c(if row.pick.is_some() { p.text } else { p.text_muted }))
                                    .when(ix == selected, |d| d.bg(c(p.surface_active)))
                                    .hover(|d| d.bg(c(p.surface_hover)))
                                    .when(row.file, |d| d.child(crate::icon_theme::file_icon_el(&row.label, 14., &p)))
                                    .child(
                                        div()
                                            .flex_none()
                                            .max_w(px(layout::PALETTE_W * 0.6))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .child(StyledText::new(row.label.clone()).with_highlights(highlights)),
                                    )
                                    .when_some(row.detail.clone(), |d, det| {
                                        d.child(
                                            div()
                                                .flex_1()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_size(px(text::XS.0))
                                                .text_color(c(p.text_muted))
                                                .child(det),
                                        )
                                    })
                                    .when_some(row.keys.clone(), |d, keys| {
                                        // A chord ("⌘K ⌘T") is split on spaces: one chip per keypress.
                                        d.child(div().flex_1()).child(div().flex_none().flex().gap(px(theme::SPACE[1])).children(keys.split(' ').map(|k| {
                                            div()
                                                .px(px(theme::SPACE[2]))
                                                .rounded(px(theme::radius::SM))
                                                .border_1()
                                                .border_color(c(p.border))
                                                .bg(c(p.surface))
                                                .text_size(px(text::XS.0))
                                                .text_color(c(p.text_muted))
                                                .child(k.to_string())
                                        })))
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.selected = ix;
                                            if let Some(pick) = this.rows.get(ix).and_then(|r| r.pick) {
                                                cx.emit(PickerEvent::Confirm(pick));
                                            }
                                        }),
                                    )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.scroll.clone())
                .h(px(rows as f32 * ROW_H));
                div().relative().child(list).child(self.painted.probe(PickFrame::List)).into_any_element()
            })
            .when_some(footer, |d, f| d.child(div().px(px(theme::SPACE[4])).text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(f)))
    }
}

/// "12", "12:5" or "12,5" → (line, column).
fn parse_position(q: &str) -> (Option<usize>, Option<usize>) {
    let mut it = q.split([':', ',']).map(str::trim);
    let line = it.next().and_then(|s| s.parse().ok());
    let col = it.next().and_then(|s| s.parse().ok()).filter(|&c: &usize| c >= 1);
    (line, col)
}
