//! "Settings" page: categories, search, toggles, option buttons and
//! number/text fields. It does not write values itself: it reports them to the workbench
//! with `SettingsEvent::Write` (written to `settings.jsonc` with comments preserved, applied immediately).

use std::collections::HashMap;

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window, div, prelude::*, px};
use serde_json::Value;

use crate::input::{FieldEvent, TextField};
use crate::settings::Settings;
use crate::ui::{self, c, icon, palette, text};
use crate::workbench::settings_ui::{CATALOG, Entry, Kind, current, parse_setting_input};

pub enum SettingsEvent {
    /// (key, value); `null`: revert to the default.
    Write(String, Value),
    PickTheme,
    PickIconTheme,
    OpenJson,
    OpenKeybindings,
    ResetAll,
}

/// The page as assistive technology reads it ([`SettingsPage::access_view`]).
#[derive(Clone, PartialEq, Debug)]
pub struct AccessPage {
    pub query: String,
    /// (label, count, selected) in `CATEGORIES` order.
    pub categories: Vec<(&'static str, usize, bool)>,
    pub heading: String,
    /// Where the search field, each category, and the "Open settings.jsonc", "Keyboard Shortcuts" and "File icon
    /// theme…" buttons were painted.
    pub search_bounds: Option<accesskit::Rect>,
    pub category_bounds: Vec<Option<accesskit::Rect>>,
    pub button_bounds: [Option<accesskit::Rect>; 4],
    pub rows: Vec<AccessSetting>,
    /// Visible settings (`rows` holds the first ones).
    pub total: usize,
    /// Why there are no rows.
    pub empty: Option<&'static str>,
    /// `Some(None)`: the search field; `Some(Some(key))`: that setting's field.
    pub focused: Option<Option<&'static str>>,
}

/// A setting's row: its title, description, value and error.
#[derive(Clone, PartialEq, Debug)]
pub struct AccessSetting {
    pub key: &'static str,
    pub title: String,
    pub about: String,
    pub control: AccessControl,
    pub modified: bool,
    pub error: Option<String>,
    pub source: Option<&'static str>,
    /// The row's painted, visible bounds (physical pixels); `None` while it is scrolled out of view.
    pub bounds: Option<accesskit::Rect>,
    /// Where its Reset button and each of its options were painted.
    pub reset_bounds: Option<accesskit::Rect>,
    pub option_bounds: Vec<Option<accesskit::Rect>>,
}

/// A part of the page whose painted bounds the accessibility tree reports.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum SetFrame {
    Row(&'static str),
    Reset(&'static str),
    Option(&'static str, usize),
    Search,
    Category(usize),
    Json,
    Keys,
    Icons,
    ResetAll,
}

#[derive(Clone, PartialEq, Debug)]
pub enum AccessControl {
    /// The theme button and the theme it shows.
    Theme(String),
    Toggle(bool),
    /// The options and the one chosen.
    Choice(&'static [&'static str], Option<String>),
    /// The text in a number or text field.
    Field(String),
}

/// The categories of the page, in the order of its list. Everything about *writing* languages (the interface language, spelling)
/// is under "Language & Region" and everything about *programming* languages (language servers, schemas, `.http`) under
/// "Development".
pub const CATEGORIES: &[&str] = &[
    "Commonly Used",
    "Modified",
    "Modules",
    "Editor",
    "Workbench",
    "Files",
    "Explorer",
    "Search",
    "Git",
    "Terminal",
    "Window",
    "Language & Region",
    "Development",
    "Remote & Security",
];

/// Keys of the "Commonly Used" category.
const COMMON: &[&str] = &[
    "modules.containers.enabled",
    "modules.webPreview.enabled",
    "theme",
    "editor.fontSize",
    "editor.fontFamily",
    "editor.tabSize",
    "editor.wordWrap",
    "editor.minimap.enabled",
    "files.autoSave",
    "editor.formatOnSave",
    "workbench.iconTheme",
    "explorer.compactFolders",
    "window.startup",
    "window.restoreSession",
    "terminal.fontSize",
    "terminal.lineHeight",
    "ui.scale",
];

/// The category of a setting (from the key prefix).
pub fn category_of(key: &str) -> &'static str {
    match key {
        "ui.language" => return "Language & Region",
        "localHistory.remote" => return "Remote & Security",
        "localHistory.enabled" | "compare.folders.maxFiles" => return "Files",
        "diagnostics.logLevel" => return "Workbench",
        _ => {}
    }
    match key.split('.').next().unwrap_or("") {
        "editor" | "data" | "diffEditor" => "Editor",
        "workbench" | "theme" | "ui" | "extensions" | "keymap" => "Workbench",
        "spelling" => "Language & Region",
        "lsp" | "json" | "emmet" | "http" | "rust" => "Development",
        "ports" | "security" => "Remote & Security",
        "files" => "Files",
        "explorer" => "Explorer",
        "search" => "Search",
        "git" => "Git",
        "terminal" => "Terminal",
        "window" | "update" => "Window",
        "modules" | "webPreview" => "Modules",
        _ => "Workbench",
    }
}

/// "editor.fontSize" → "Editor: Font Size"; "workbench.statusBar.visible" → "Workbench › Status Bar: Visible".
pub fn title_of(key: &str) -> String {
    let words = |s: &str| {
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if i == 0 {
                out.extend(ch.to_uppercase());
            } else if ch.is_uppercase() {
                out.push(' ');
                out.push(ch);
            } else {
                out.push(ch);
            }
        }
        out
    };
    let parts: Vec<&str> = key.split('.').collect();
    match parts.as_slice() {
        [one] => words(one),
        [head, rest @ ..] => {
            let (last, mid) = rest.split_last().unwrap();
            let mut t = words(head);
            for m in mid {
                t.push_str(" › ");
                t.push_str(&words(m));
            }
            format!("{t}: {}", words(last))
        }
        [] => String::new(),
    }
}

/// Every search word may match a different part of the key, title or translated description.
pub fn matches_query(key: &str, about: &str, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let help = crate::settings_help::help_for(key).unwrap_or_default().to_lowercase();
    let searchable = [key.to_lowercase(), title_of(key).to_lowercase(), about.to_lowercase(), crate::i18n::t(about).to_lowercase(), help];
    q.split_whitespace().all(|word| searchable.iter().any(|part| part.contains(word)))
}

pub struct SettingsPage {
    query: Entity<TextField>,
    category: usize,
    /// Recompose from the actual editor pane, including split editors and UI zoom.
    compact: bool,
    /// Fields for number and text settings.
    fields: HashMap<&'static str, Entity<TextField>>,
    /// Last committed text shown in each field; differing text is an unsaved draft.
    synced_fields: HashMap<&'static str, String>,
    errors: HashMap<&'static str, String>,
    /// Settings whose "?" explanation is open.
    help_open: std::collections::HashSet<&'static str>,
    /// Where each setting's row was painted in the last frame (accessibility).
    bounds: crate::painted::RowBounds<SetFrame>,
    focus_handle: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsPage {}

impl Focusable for SettingsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// A category name in the UI language. The id (`CATEGORIES`) stays English; only the displayed text is translated.
/// "Git" is a term and is not translated.
pub fn category_label(name: &str) -> &'static str {
    use crate::i18n::t;
    match name {
        "Commonly Used" => t("Commonly Used"),
        "Modified" => t("Modified"),
        "Modules" => t("Modules"),
        "Editor" => t("Editor"),
        "Workbench" => t("Workbench"),
        "Files" => t("Files"),
        "Explorer" => t("Explorer"),
        "Search" => t("Search"),
        "Terminal" => t("Terminal"),
        "Window" => t("Window"),
        "Language & Region" => t("Language & Region"),
        "Development" => t("Development"),
        "Remote & Security" => t("Remote & Security"),
        _ => "Git",
    }
}

/// Readable name of a theme id ("codeyard-light" → "Codeyard Light"); an unknown id is returned as is.
fn theme_label(id: &str) -> String {
    if id == "auto" {
        return crate::i18n::t("Auto (follow system)").into();
    }
    theme::themes::builtin().into_iter().find(|t| t.id == id).map_or_else(|| id.to_string(), |t| t.name)
}

/// Where the value in effect comes from, when it is not the user's own settings file: the workspace file or the folder.
pub fn source_note(key: &str, cx: &App) -> Option<&'static str> {
    match cx.try_global::<crate::settings::Sources>()?.of(key) {
        crate::settings::Scope::User => None,
        crate::settings::Scope::Workspace => Some(crate::i18n::t("Set by the workspace file")),
        crate::settings::Scope::Folder => Some(crate::i18n::t("Set by this folder's settings")),
    }
}

/// The title a setting's row shows.
fn row_title(key: &str) -> String {
    match key {
        "modules.containers.enabled" => crate::i18n::t("Container Environments").to_string(),
        "modules.webPreview.enabled" => crate::i18n::t("Web Preview").to_string(),
        _ => title_of(key),
    }
}

fn shown(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_f64().map_or(n.to_string(), |f| if f.fract() == 0. { format!("{}", f as i64) } else { format!("{f}") }),
        o => o.to_string(),
    }
}

impl SettingsPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new(crate::i18n::tr("Search settings"), cx));
        let mut subs = vec![cx.observe(&query, |_, _, cx| cx.notify())];
        let mut fields = HashMap::new();
        for e in CATALOG.iter().filter(|e| matches!(e.kind, Kind::Number { .. } | Kind::Text)) {
            let field = cx.new(|cx| TextField::new(crate::i18n::tr("default"), cx));
            let (key, kind) = (e.key, e.kind);
            subs.push(cx.subscribe(&field, move |this, f, ev: &FieldEvent, cx| {
                if matches!(ev, FieldEvent::Changed) {
                    this.errors.remove(key);
                    return cx.notify();
                }
                if !matches!(ev, FieldEvent::Submit) {
                    return;
                }
                let text = f.read(cx).text().to_string();
                match parse_setting_input(key, kind, &text) {
                    Ok(v) => {
                        this.errors.remove(key);
                        f.update(cx, |field, cx| field.set_text(&shown(&v), cx));
                        cx.emit(SettingsEvent::Write(key.to_string(), v));
                    }
                    Err(msg) => {
                        this.errors.insert(key, msg);
                    }
                }
                cx.notify();
            }));
            fields.insert(e.key, field);
        }
        let mut page = SettingsPage {
            query,
            category: 0,
            compact: true,
            fields,
            synced_fields: HashMap::new(),
            errors: HashMap::new(),
            help_open: Default::default(),
            bounds: Default::default(),
            focus_handle: cx.focus_handle(),
            _subs: subs,
        };
        page.sync_fields(cx);
        page
    }

    /// Opens or closes the "?" explanation of a setting.
    pub fn toggle_help(&mut self, key: &'static str, cx: &mut Context<Self>) {
        if !self.help_open.remove(key) {
            self.help_open.insert(key);
        }
        cx.notify();
    }

    /// The paragraphs of the explanation: the written help (in the UI language), then the facts the page works out itself.
    fn help_text(&self, key: &str, kind: Option<Kind>, default: &Value) -> Vec<String> {
        let mut out: Vec<String> = crate::settings_help::help_for(key).map(|h| h.split('\n').map(str::to_string).collect()).unwrap_or_default();
        out.extend(crate::settings_help::facts(key, kind, default));
        out
    }

    /// Fill the fields from the current settings (when the settings file changes).
    pub fn sync_fields(&mut self, cx: &mut Context<Self>) {
        let s = cx.global::<Settings>().clone();
        for (key, field) in &self.fields {
            let text = shown(&current(&s, key));
            let draft = field.read(cx).text();
            if self.synced_fields.get(key).is_some_and(|previous| previous != draft) && draft != text {
                continue;
            }
            if draft != text {
                field.update(cx, |f, cx| f.set_text(&text, cx));
            }
            self.synced_fields.insert(key, text);
        }
    }

    /// Called only after an explicit reset has succeeded or when resetting one field.
    pub fn discard_drafts(&mut self) {
        self.synced_fields.clear();
        self.errors.clear();
    }

    fn has_draft(&self, key: &str, cx: &App) -> bool {
        self.fields.get(key).zip(self.synced_fields.get(key)).is_some_and(|(field, previous)| field.read(cx).text() != previous)
    }

    pub fn focus_search(&self, window: &mut Window, cx: &App) {
        window.focus(&self.query.focus_handle(cx));
    }

    /// Filter the page as if `q` had been typed into its search field.
    pub fn set_query(&mut self, q: &str, cx: &mut Context<Self>) {
        self.query.update(cx, |f, cx| f.set_text(q, cx));
    }

    /// Shows a category (the search is cleared), as clicking it does.
    pub fn select_category(&mut self, i: usize, cx: &mut Context<Self>) {
        self.category = i.min(CATEGORIES.len() - 1);
        self.query.update(cx, |q, cx| q.set_text("", cx));
        cx.notify();
    }

    /// Focuses a number or text setting's field.
    pub fn focus_field(&self, key: &str, window: &mut Window, cx: &App) {
        if let Some(f) = self.fields.get(key) {
            window.focus(&f.focus_handle(cx));
        }
    }

    /// Puts `text` into a number or text setting's field and submits it as Enter does (checked, then written).
    pub fn submit_field(&mut self, key: &str, text: &str, cx: &mut Context<Self>) {
        if let Some(f) = self.fields.get(key).cloned() {
            f.update(cx, |f, cx| {
                f.set_text(text, cx);
                cx.emit(FieldEvent::Submit);
            });
        }
    }

    #[cfg(test)]
    pub fn error_for_test(&self, key: &str) -> Option<String> {
        self.errors.get(key).cloned()
    }

    /// Every setting (key, description): theme + catalog.
    fn all_entries() -> Vec<(&'static str, &'static str)> {
        let mut keys: Vec<(&'static str, &'static str)> = vec![("theme", "Color theme of the whole window and the editor")];
        keys.extend(CATALOG.iter().map(|e| (e.key, e.about)));
        keys
    }

    /// Whether it differs from the default (against the effective settings, not a restart).
    fn is_modified(s: &Settings, key: &str) -> bool {
        let (now, default) =
            if key == "theme" { (Value::String(s.theme.clone()), Value::String("auto".into())) } else { (current(s, key), current(&Settings::default(), key)) };
        now != default && !(now.is_null() && default.is_null())
    }

    /// Number of settings per category (the counter in the sidebar).
    fn category_count(cat: &str, s: &Settings) -> usize {
        Self::all_entries()
            .into_iter()
            .filter(|(k, _)| match cat {
                "Commonly Used" => COMMON.contains(k),
                "Modified" => Self::is_modified(s, k),
                _ => category_of(k) == cat,
            })
            .count()
    }

    /// Visible setting keys: all categories when searching, otherwise the selected category.
    pub fn visible_keys(&self, cx: &App) -> Vec<&'static str> {
        let query = self.query.read(cx).text().to_string();
        let searching = !query.trim().is_empty();
        let modified_only = query.split_whitespace().any(|word| word.eq_ignore_ascii_case("@modified"));
        let words = query.split_whitespace().filter(|word| !word.eq_ignore_ascii_case("@modified")).collect::<Vec<_>>().join(" ");
        let cat = CATEGORIES[self.category];
        let s = cx.global::<Settings>();
        let mut keys: Vec<_> = Self::all_entries()
            .into_iter()
            .filter(|(k, about)| {
                if searching {
                    (!modified_only || Self::is_modified(s, k)) && matches_query(k, about, &words)
                } else if cat == "Commonly Used" {
                    COMMON.contains(k)
                } else if cat == "Modified" {
                    Self::is_modified(s, k)
                } else {
                    category_of(k) == cat
                }
            })
            .map(|(k, _)| k)
            .collect();
        if !searching && cat == "Commonly Used" {
            keys.sort_by_key(|key| *key != "modules.containers.enabled");
        } else if searching {
            // Put settings named by the query before incidental mentions in descriptions.
            keys.sort_by_key(|key| !matches_query(key, "", &words));
        }
        keys
    }

    /// What assistive technology is told about the page: the same categories, rows, values and errors as on screen,
    /// at most `max` rows.
    pub fn access_view(&self, max: usize, window: &Window, cx: &App) -> AccessPage {
        let s = cx.global::<Settings>();
        let query = self.query.read(cx).text().to_string();
        let searching = !query.trim().is_empty();
        let keys = self.visible_keys(cx);
        let rows = keys
            .iter()
            .take(max)
            .map(|&key| {
                let entry = CATALOG.iter().find(|e| e.key == key);
                let about = entry.map_or("Color theme of the whole window and the editor", |e| e.about);
                let now = if key == "theme" { Value::String(s.theme.clone()) } else { current(s, key) };
                let control = match entry.map(|e| e.kind) {
                    None => AccessControl::Theme(theme_label(&shown(&now))),
                    Some(Kind::Bool) => AccessControl::Toggle(now.as_bool().unwrap_or(false)),
                    Some(Kind::Choice(opts)) => AccessControl::Choice(opts, now.as_str().map(str::to_string)),
                    Some(Kind::Number { .. } | Kind::Text) => {
                        AccessControl::Field(self.fields.get(key).map(|f| f.read(cx).text().to_string()).unwrap_or_default())
                    }
                };
                AccessSetting {
                    key,
                    title: row_title(key),
                    about: {
                        let mut about = crate::i18n::t(about).to_string();
                        if self.help_open.contains(key) {
                            about = format!("{about} {}", self.help_text(key, entry.map(|e| e.kind), &current(&Settings::default(), key)).join(" "));
                        }
                        if self.has_draft(key, cx) {
                            about = format!("{about} {}", crate::i18n::t("Press Enter to apply"));
                        }
                        about
                    },
                    control,
                    modified: Self::is_modified(s, key),
                    error: self.errors.get(key).cloned(),
                    source: source_note(key, cx),
                    bounds: crate::painted::physical(&self.bounds, &SetFrame::Row(key), window),
                    reset_bounds: crate::painted::physical(&self.bounds, &SetFrame::Reset(key), window),
                    option_bounds: match entry.map(|e| e.kind) {
                        Some(Kind::Choice(opts)) => {
                            (0..opts.len()).map(|k| crate::painted::physical(&self.bounds, &SetFrame::Option(key, k), window)).collect()
                        }
                        _ => Vec::new(),
                    },
                }
            })
            .collect();
        let focused = if self.query.focus_handle(cx).is_focused(window) {
            Some(None)
        } else {
            self.fields.iter().find(|(_, f)| f.focus_handle(cx).is_focused(window)).map(|(key, _)| Some(*key))
        };
        AccessPage {
            categories: CATEGORIES
                .iter()
                .enumerate()
                .map(|(i, name)| (category_label(name), Self::category_count(name, s), i == self.category && !searching))
                .collect(),
            heading: if searching {
                crate::i18n::tn("{} result", "{} results", keys.len(), &[])
            } else {
                category_label(CATEGORIES[self.category]).to_string()
            },
            empty: keys.is_empty().then(|| {
                if CATEGORIES[self.category] == "Modified" && !searching {
                    crate::i18n::t("Nothing changed: every setting has its default value.")
                } else {
                    crate::i18n::t("No setting matches.")
                }
            }),
            query,
            rows,
            total: keys.len(),
            focused,
            search_bounds: crate::painted::physical(&self.bounds, &SetFrame::Search, window),
            category_bounds: (0..CATEGORIES.len()).map(|i| crate::painted::physical(&self.bounds, &SetFrame::Category(i), window)).collect(),
            button_bounds: [SetFrame::Json, SetFrame::Keys, SetFrame::Icons, SetFrame::ResetAll].map(|f| crate::painted::physical(&self.bounds, &f, window)),
        }
    }

    fn entry_view(&self, key: &'static str, ix: usize, s: &Settings, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = *palette(cx);
        let entry: Option<&Entry> = CATALOG.iter().find(|e| e.key == key);
        let about = entry.map_or(crate::i18n::tr("Color theme of the whole window and the editor"), |e| e.about);
        let now = if key == "theme" { Value::String(s.theme.clone()) } else { current(s, key) };
        let default = if key == "theme" { Value::String("auto".into()) } else { current(&Settings::default(), key) };
        let modified = now != default && !(now.is_null() && default.is_null());
        let control: gpui::AnyElement = match (key, entry.map(|e| e.kind)) {
            ("theme", _) => div()
                .flex()
                .child(
                    div()
                        .id(("set-theme", ix))
                        .h(px(32.))
                        .px(px(theme::SPACE[4]))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[3]))
                        .rounded(px(theme::radius::SM))
                        .bg(c(p.surface_hover))
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_active)))
                        .child(icon("palette", 14., p.accent))
                        .child(div().text_size(px(text::SM.0)).child(theme_label(&shown(&now))))
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::PickTheme))),
                )
                .into_any_element(),
            (_, Some(Kind::Bool)) => {
                let on = now.as_bool().unwrap_or(false);
                div()
                    .id(("sw", ix))
                    .flex_none()
                    .w(px(36.))
                    .h(px(20.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(c(if on { p.accent } else { p.surface_active }))
                    .border_1()
                    .border_color(c(if on { p.accent } else { p.border_input }))
                    .relative()
                    .child(div().absolute().top(px(2.)).left(px(if on { 18. } else { 2. })).size(px(14.)).rounded_full().bg(c(if on {
                        p.on_accent
                    } else {
                        p.text_muted
                    })))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(SettingsEvent::Write(key.to_string(), Value::Bool(!on)))))
                    .into_any_element()
            }
            (_, Some(Kind::Choice(opts))) => div()
                .flex()
                .flex_wrap()
                .gap(px(theme::SPACE[2]))
                .children(opts.iter().enumerate().map(|(oi, o)| {
                    let active = now.as_str() == Some(*o);
                    let value = (*o).to_string();
                    div()
                        .id((SharedString::from(format!("opt-{key}")), oi))
                        .relative()
                        .child(crate::painted::probe(&self.bounds, SetFrame::Option(key, oi)))
                        .h(px(32.))
                        .px(px(theme::SPACE[4]))
                        .flex()
                        .items_center()
                        .rounded(px(theme::radius::SM))
                        .text_size(px(text::XS.0))
                        .cursor_pointer()
                        .when(active, |d| d.bg(c(p.accent)).text_color(c(p.on_accent)))
                        .when(!active, |d| d.bg(c(p.surface_hover)).text_color(c(p.text)).hover(|d| d.bg(c(p.surface_active))))
                        .child(*o)
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SettingsEvent::Write(key.to_string(), Value::String(value.clone())))))
                }))
                .into_any_element(),
            (_, Some(Kind::Number { .. } | Kind::Text)) => match self.fields.get(key) {
                Some(f) => div()
                    .flex()
                    .items_center()
                    .w(px(if matches!(entry.map(|e| e.kind), Some(Kind::Text)) { 480. } else { 180. }))
                    .max_w_full()
                    .min_h(px(32.))
                    .bg(c(p.surface))
                    .border_1()
                    .border_color(c(if self.errors.contains_key(key) { p.danger } else { p.border_input }))
                    .rounded(px(theme::radius::SM))
                    .child(f.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            },
            _ => div().into_any_element(),
        };
        let help_open = self.help_open.contains(key);
        let help_lines = if help_open { self.help_text(key, entry.map(|e| e.kind), &default) } else { Vec::new() };
        let reset = (modified || self.has_draft(key, cx)).then(|| {
            div()
                .id(("reset", ix))
                .relative()
                .child(crate::painted::probe(&self.bounds, SetFrame::Reset(key)))
                .flex_none()
                .cursor_pointer()
                .text_size(px(text::XXS.0))
                .text_color(c(p.accent))
                .hover(|d| d.underline())
                .child(crate::i18n::t("Reset"))
                .tooltip(|_, cx| {
                    cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Back to the default (removes the value from settings.jsonc)").into())).into()
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.synced_fields.remove(key);
                    this.errors.remove(key);
                    cx.emit(SettingsEvent::Write(key.to_string(), Value::Null));
                }))
        });
        div()
            .id(("setting", ix))
            .debug_selector(move || format!("setting-row-{key}"))
            .relative()
            .child(crate::painted::probe(&self.bounds, SetFrame::Row(key)))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(10.))
            .py(px(24.))
            .border_b_1()
            .border_color(c(p.border))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .children(modified.then(|| div().flex_none().size(px(6.)).rounded_full().bg(c(p.accent))))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_size(px(text::SM.0)).child(row_title(key)))
                    .child(
                        div()
                            .id(("help", ix))
                            .debug_selector(move || format!("setting-help-{key}"))
                            .flex_none()
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_hover)))
                            .when(help_open, |d| d.bg(c(p.surface_active)))
                            .child(icon("circle-help", 15., if help_open { p.accent } else { p.text_muted }))
                            .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Explain this setting").into())).into())
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_help(key, cx))),
                    )
                    .children(reset),
            )
            .child(
                div()
                    .debug_selector(move || format!("setting-description-{key}"))
                    .text_size(px(text::SM.0))
                    .line_height(px(20.))
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::t(about).to_string()),
            )
            .when(help_open, |d| {
                d.child(
                    div()
                        .debug_selector(move || format!("setting-help-text-{key}"))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .p(px(theme::SPACE[5]))
                        .rounded(px(theme::radius::MD))
                        .bg(c(p.surface_hover))
                        .border_l_2()
                        .border_color(c(p.accent))
                        .text_size(px(text::SM.0))
                        .line_height(px(20.))
                        .text_color(c(p.text))
                        .children(help_lines.iter().map(|l| div().child(l.clone()))),
                )
            })
            .child(div().font_family(ui::MONO_FONT).text_size(px(text::XXS.0)).text_color(c(p.line_number)).child(key.to_string()))
            .children(
                source_note(key, cx)
                    .map(|note| div().debug_selector(move || format!("setting-source-{key}")).text_size(px(text::XS.0)).text_color(c(p.accent)).child(note)),
            )
            .child(
                div()
                    .debug_selector(move || format!("setting-control-{key}"))
                    .w_full()
                    .min_w_0()
                    .pt(px(4.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.))
                    .child(control)
                    .when(self.has_draft(key, cx), |d| {
                        d.child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(crate::i18n::t("Press Enter to apply")))
                    }),
            )
            .when_some(self.errors.get(key).cloned(), |d, e| d.child(div().text_size(px(text::XS.0)).text_color(c(p.danger)).child(e)))
            .into_any_element()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let s = cx.global::<Settings>().clone();
        let keys = self.visible_keys(cx);
        let searching = !self.query.read(cx).text().trim().is_empty();
        self.bounds.borrow_mut().clear();
        let rows: Vec<gpui::AnyElement> = keys.iter().enumerate().map(|(i, k)| self.entry_view(k, i, &s, cx)).collect();
        let compact = self.compact;
        let entity = cx.weak_entity();
        let nav = CATEGORIES.iter().enumerate().map(|(i, name)| {
            let active = i == self.category && !searching;
            let count = Self::category_count(name, &s);
            let hot = *name == "Modified" && count > 0;
            div()
                .id(("set-cat", i))
                .relative()
                .child(crate::painted::probe(&self.bounds, SetFrame::Category(i)))
                .flex_none()
                .h(px(34.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(12.))
                .rounded(px(theme::radius::MD))
                .cursor_pointer()
                .text_size(px(text::SM.0))
                .when(active, |d| d.bg(c(p.surface_active)).text_color(c(p.text)))
                .when(!active, |d| d.text_color(c(p.text_muted)).hover(|d| d.bg(c(p.surface_hover))))
                .child(div().when(!compact, |d| d.flex_1()).child(category_label(name)))
                .child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(if hot { p.accent } else { p.line_number })).child(count.to_string()))
                .on_click(cx.listener(move |this, _, _, cx| this.select_category(i, cx)))
        });
        let heading =
            if searching { crate::i18n::tn("{} result", "{} results", rows.len(), &[]) } else { category_label(CATEGORIES[self.category]).to_string() };
        let tools = [
            ("set-open-json", "Open settings.jsonc", "file-json", SetFrame::Json),
            ("set-open-keys", "Keyboard Shortcuts", "keyboard", SetFrame::Keys),
            ("set-icon-theme", "File icon theme…", "folder-tree", SetFrame::Icons),
            ("set-reset-all", "Reset All Settings…", "rotate-ccw", SetFrame::ResetAll),
        ]
        .into_iter()
        .map(|(id, label, glyph, frame)| {
            div()
                .id(id)
                .relative()
                .child(crate::painted::probe(&self.bounds, frame))
                .h(px(if compact { 28. } else { 30. }))
                .px(px(if compact { 6. } else { 10. }))
                .flex()
                .items_center()
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .hover(|d| d.bg(c(p.surface_hover)).text_color(c(p.text)))
                .text_size(px(text::XS.0))
                .text_color(c(p.text_muted))
                .when(compact, |d| d.child(icon(glyph, 14., p.text_muted)))
                .when(!compact, |d| d.child(crate::i18n::t(label)))
                .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t(label).into())).into())
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(match frame {
                        SetFrame::Json => SettingsEvent::OpenJson,
                        SetFrame::Keys => SettingsEvent::OpenKeybindings,
                        SetFrame::Icons => SettingsEvent::PickIconTheme,
                        _ => SettingsEvent::ResetAll,
                    });
                }))
        });
        div()
            .id("settings-page")
            .debug_selector(|| "settings-page".to_string())
            .relative()
            .child(
                gpui::canvas(
                    |bounds, _, _| bounds.size.width < px(760.),
                    move |_, measured, _, cx| {
                        if compact != measured {
                            let entity = entity.clone();
                            cx.defer(move |cx| {
                                entity
                                    .update(cx, |page, cx| {
                                        if page.compact != measured {
                                            page.compact = measured;
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                            });
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .key_context("SettingsPage")
            .track_focus(&self.focus_handle)
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(if compact { 12. } else { 16. }))
                    .px(px(if compact { 12. } else { 32. }))
                    .py(px(if compact { 12. } else { 24. }))
                    .border_b_1()
                    .border_color(c(p.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(icon("settings", if compact { 18. } else { 20. }, p.text_muted))
                                    .child(
                                        div()
                                            .text_size(px(if compact { text::LG.0 } else { text::XL.0 }))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(crate::i18n::t("Settings")),
                                    ),
                            )
                            .child(div().min_w_0().flex().flex_wrap().justify_end().gap(px(if compact { 0. } else { 4. })).children(tools)),
                    )
                    .child(
                        crate::ui::input_box("settings-search-box", &p, p.surface, self.query.focus_handle(cx).is_focused(window))
                            .relative()
                            .child(crate::painted::probe(&self.bounds, SetFrame::Search))
                            .w_full()
                            .min_h(px(36.))
                            .child(crate::ui::search_icon(&p))
                            .child(div().flex_1().min_w_0().child(self.query.clone())),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .when(compact, |d| d.flex_col())
                    .child(
                        div()
                            .id("settings-categories")
                            .debug_selector(|| "settings-categories".to_string())
                            .flex_none()
                            .min_w_0()
                            .flex()
                            .gap(px(4.))
                            .p(px(if compact { 8. } else { 12. }))
                            .when(compact, |d| d.w_full().overflow_x_scroll().border_b_1())
                            .when(!compact, |d| d.w(px(208.)).h_full().flex_col().overflow_y_scroll().border_r_1())
                            .border_color(c(p.border))
                            .children(nav),
                    )
                    .child(
                        div()
                            .id("settings-list")
                            .debug_selector(|| "settings-list".to_string())
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .overflow_y_scroll()
                            .px(px(if compact { 20. } else { 32. }))
                            .py(px(28.))
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(880.))
                                    .mx_auto()
                                    .child(div().pb(px(4.)).text_size(px(text::LG.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(heading))
                                    .children(rows)
                                    .when(keys.is_empty(), |d| {
                                        d.child(div().pt(px(24.)).text_color(c(p.text_muted)).child(if CATEGORIES[self.category] == "Modified" && !searching {
                                            crate::i18n::t("Nothing changed: every setting has its default value.")
                                        } else {
                                            crate::i18n::t("No setting matches.")
                                        }))
                                    }),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn settings_reflow_with_pane_width_and_zoom(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (page, cx) = cx.add_window_view(|_, cx| SettingsPage::new(cx));
        page.update(cx, |page, cx| page.set_query("terminal font", cx));
        for scale in [1., 2.] {
            cx.update(|_, cx| cx.set_ui_scale(scale));
            for width in [480., 800., 1280.] {
                cx.simulate_resize(gpui::size(px(width), px(1000.)));
                cx.run_until_parked();
                let pane = cx.debug_bounds("settings-page").expect("settings pane");
                assert_eq!(page.read_with(cx, |page, _| page.compact), pane.size.width < px(760.));
                let view = page.update_in(cx, |page, window, cx| page.access_view(256, window, cx));
                let search = view.search_bounds.expect("visible search");
                let physical_width = cx.update(|window, _| f64::from(pane.size.width / px(1.)) * f64::from(window.scale_factor()));
                assert!(search.x0 >= 0. && search.x1 <= physical_width + 0.5, "search fits at {width}/{scale}: {search:?}");
                for bounds in view.button_bounds {
                    let b = bounds.expect("utility action visible");
                    assert!(b.x0 >= 0. && b.x1 <= physical_width + 0.5, "utility fits at {width}/{scale}: {b:?}");
                }
                let description = cx.debug_bounds("setting-description-terminal.fontSize").expect("description");
                let control = cx.debug_bounds("setting-control-terminal.fontSize").expect("control");
                assert!(control.top() >= description.bottom(), "controls sit below readable descriptions");
                assert!(control.left() >= pane.left() && control.right() <= pane.right(), "control stays within pane: {control:?}");
                assert!(control.bottom() <= pane.bottom(), "first control is visible at {width}/{scale}: {control:?}");
                let list = cx.debug_bounds("settings-list").expect("settings list");
                assert!(list.size.height >= px(180.), "header leaves useful content space at {width}/{scale}: {list:?}");
                let nav = cx.debug_bounds("settings-categories").expect("categories");
                if pane.size.width < px(760.) {
                    assert!(nav.size.height < px(70.), "compact categories form a horizontal strip");
                } else {
                    assert_eq!(nav.size.width, px(208.));
                }
            }
        }
    }

    #[gpui::test]
    fn compact_settings_keep_content_space_with_workbench_chrome_at_two_hundred_percent(cx: &mut gpui::TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        cx.update(|_, cx| cx.set_ui_scale(2.));
        cx.simulate_resize(gpui::size(px(1280.), px(800.)));
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Open Settings", window, cx));
        cx.run_until_parked();
        let pane = cx.debug_bounds("settings-page").expect("settings pane");
        let list = cx.debug_bounds("settings-list").expect("scrollable settings");
        assert!(list.size.height >= px(120.), "the header leaves scrollable content in a short pane: {pane:?} {list:?}");
        let page = wb.read_with(cx, |wb, _| wb.settings_page_for_test().unwrap());
        let view = page.update_in(cx, |page, window, cx| page.access_view(256, window, cx));
        for b in view.button_bounds {
            let b = b.expect("utility action stays visible");
            assert!(b.width() >= 24. && b.height() >= 24., "usable utility action: {b:?}");
        }
    }

    #[test]
    fn container_module_is_findable_without_search() {
        assert_eq!(category_of("modules.containers.enabled"), "Modules");
        assert!(CATEGORIES.contains(&"Modules"));
        assert!(COMMON.contains(&"modules.containers.enabled"));
        assert!(COMMON.contains(&"modules.webPreview.enabled"));
    }

    /// The interface language and spelling (languages people write) and language servers, schemas and Rust builds (programming
    /// languages) are in different categories, each translated.
    #[gpui::test]
    fn writing_languages_and_programming_languages_have_their_own_categories(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (page, cx) = cx.add_window_view(|_, cx| SettingsPage::new(cx));
        let keys_of = |cx: &mut gpui::VisualTestContext, name: &str| {
            page.update(cx, |page, cx| {
                page.category = CATEGORIES.iter().position(|c| *c == name).unwrap();
                cx.notify();
            });
            page.read_with(cx, |page, cx| page.visible_keys(cx))
        };
        let region = keys_of(cx, "Language & Region");
        assert!(region.contains(&"ui.language") && region.contains(&"spelling.enabled"), "{region:?}");
        assert!(!region.contains(&"lsp.enabled"), "{region:?}");
        let programming = keys_of(cx, "Development");
        for key in ["lsp.enabled", "json.validate.enable", "http.runOn", "rust.profile", "rust.testRunner", "rust.inlayHints.types"] {
            assert!(programming.contains(&key), "{key} in {programming:?}");
        }
        assert!(!programming.contains(&"ui.language"), "{programming:?}");
        assert!(CATEGORIES.iter().all(|c| !c.contains("Remote & Languages")));
        let _tr = crate::i18n::LangGuard::set("tr");
        assert_eq!((category_label("Language & Region"), category_label("Development")), ("Dil ve Bölge", "Geliştirme"));
    }

    /// The "?" opens the explanation in the interface language, with the facts under it; a second press closes it.
    #[gpui::test]
    fn the_question_mark_opens_and_closes_the_explanation_in_the_ui_language(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (page, cx) = cx.add_window_view(|_, cx| SettingsPage::new(cx));
        page.update(cx, |page, cx| page.set_query("ui.language", cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("setting-help-ui.language").is_some(), "every row has the button");
        assert!(cx.debug_bounds("setting-help-text-ui.language").is_none(), "closed at first");
        page.update(cx, |page, cx| page.toggle_help("ui.language", cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("setting-help-text-ui.language").is_some());
        let spoken = cx.update(|window, cx| page.read(cx).access_view(50, window, cx));
        assert!(spoken.rows[0].about.contains("The language of menus") && spoken.rows[0].about.contains("Default: "), "{}", spoken.rows[0].about);
        {
            let _tr = crate::i18n::LangGuard::set("tr");
            let spoken = cx.update(|window, cx| page.read(cx).access_view(50, window, cx));
            assert!(spoken.rows[0].about.contains("Menülerin") && spoken.rows[0].about.contains("Varsayılan: "), "{}", spoken.rows[0].about);
        }
        page.update(cx, |page, cx| page.toggle_help("ui.language", cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("setting-help-text-ui.language").is_none());
    }

    /// The help is searched too: a word that only the written explanation has finds the setting.
    #[gpui::test]
    fn search_finds_a_setting_by_its_help_text(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (page, cx) = cx.add_window_view(|_, cx| SettingsPage::new(cx));
        page.update(cx, |page, cx| page.set_query("nextest installed", cx));
        assert!(page.read_with(cx, |page, cx| page.visible_keys(cx)).contains(&"rust.testRunner"));
    }

    #[gpui::test]
    fn container_module_toggle_appears_first_and_has_a_translated_category(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (page, cx) = cx.add_window_view(|_, cx| SettingsPage::new(cx));
        let keys = page.read_with(cx, |page, cx| page.visible_keys(cx));
        assert_eq!(keys.first().copied(), Some("modules.containers.enabled"));
        page.update(cx, |page, cx| {
            page.category = CATEGORIES.iter().position(|name| *name == "Modules").unwrap();
            cx.notify();
        });
        assert_eq!(
            page.read_with(cx, |page, cx| page.visible_keys(cx)),
            [
                "modules.containers.enabled",
                "modules.webPreview.enabled",
                "webPreview.port",
                "webPreview.host",
                "webPreview.liveReload",
                "webPreview.reloadInterval",
                "webPreview.root"
            ]
        );
        let _tr = crate::i18n::LangGuard::set("tr");
        assert_eq!(category_label("Modules"), "Modüller");
        page.update(cx, |page, cx| page.set_query("konteyner", cx));
        assert!(page.read_with(cx, |page, cx| page.visible_keys(cx)).contains(&"modules.containers.enabled"));
        page.update(cx, |page, cx| page.set_query("yenile", cx));
        assert!(page.read_with(cx, |page, cx| page.visible_keys(cx)).contains(&"webPreview.liveReload"));
    }

    #[test]
    fn titles_are_readable() {
        assert_eq!(title_of("editor.fontSize"), "Editor: Font Size");
        assert_eq!(title_of("workbench.statusBar.visible"), "Workbench › Status Bar: Visible");
        assert_eq!(title_of("theme"), "Theme");
        assert_eq!(title_of("files.autoSaveDelay"), "Files: Auto Save Delay");
    }

    #[test]
    fn every_catalog_entry_lands_in_a_category() {
        for e in CATALOG {
            assert!(CATEGORIES.contains(&category_of(e.key)), "{}", e.key);
        }
        for k in COMMON {
            assert!(*k == "theme" || CATALOG.iter().any(|e| e.key == *k), "{k} is missing");
        }
    }

    #[test]
    fn modified_means_different_from_the_default() {
        let d = Settings::default();
        assert!(!SettingsPage::is_modified(&d, "editor.minimap.enabled"));
        let mut s = d.clone();
        s.minimap = !d.minimap;
        assert!(SettingsPage::is_modified(&s, "editor.minimap.enabled"));
        assert!(!SettingsPage::is_modified(&s, "editor.wordWrap"));
        assert_eq!(SettingsPage::category_count("Modified", &d), 0);
        assert_eq!(SettingsPage::category_count("Modified", &s), 1);
        s.theme = "nord".into();
        assert!(SettingsPage::is_modified(&s, "theme"));
        assert_eq!(SettingsPage::category_count("Modified", &s), 2);
    }

    #[test]
    fn category_counts_add_up_to_the_catalog() {
        let s = Settings::default();
        let total: usize = CATEGORIES.iter().filter(|c| **c != "Commonly Used" && **c != "Modified").map(|c| SettingsPage::category_count(c, &s)).sum();
        assert_eq!(total, CATALOG.len() + 1, "every setting (and the theme) is in exactly one category");
        assert_eq!(SettingsPage::category_count("Commonly Used", &s), COMMON.len());
    }

    #[test]
    fn theme_labels_are_readable() {
        assert_eq!(theme_label("auto"), "Auto (follow system)");
        assert_eq!(theme_label("codeyard-light"), "Codeyard Light");
        assert_eq!(theme_label("user:mine"), "user:mine");
    }

    #[test]
    fn search_looks_at_key_title_and_description() {
        assert!(matches_query("editor.fontSize", "Font size of the editor", "font"));
        assert!(matches_query("editor.minimap.enabled", "Show the minimap", "MINIMAP"));
        assert!(matches_query("files.autoSave", "Save files automatically", "automatic"));
        assert!(!matches_query("editor.fontSize", "Font size", "terminal"));
        assert!(matches_query("x.y", "", "  "));
    }

    #[test]
    fn settings_search_combines_words_across_fields_in_any_order() {
        assert!(matches_query("terminal.fontSize", "Font size of the terminal", "terminal font"));
        assert!(matches_query("terminal.fontSize", "Font size of the terminal", " SIZE\tterminal  font "));
        assert!(matches_query("files.autoSaveDelay", "Delay for afterDelay (ms)", "files delay"));
        assert!(!matches_query("terminal.fontSize", "Font size of the terminal", "terminal minimap"));
        let _tr = crate::i18n::LangGuard::set("tr");
        assert!(matches_query("terminal.fontSize", "Font size of the terminal", "terminal boyutu"));
    }

    #[gpui::test]
    fn modified_search_filters_the_actual_settings_and_combines_with_words(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (page, cx) = cx.add_window_view(|_, cx| SettingsPage::new(cx));
        cx.update(|_, cx| cx.global_mut::<Settings>().terminal_font_size = 18.);
        for query in ["@modified", "@modified terminal font", " FONT\t@MODIFIED terminal "] {
            page.update(cx, |page, cx| page.set_query(query, cx));
            assert_eq!(page.read_with(cx, |page, cx| page.visible_keys(cx)), ["terminal.fontSize"], "{query}");
        }
        page.update(cx, |page, cx| page.set_query("@modified minimap", cx));
        assert!(page.read_with(cx, |page, cx| page.visible_keys(cx)).is_empty());
        page.update(cx, |page, cx| page.set_query("terminal font", cx));
        assert_eq!(page.read_with(cx, |page, cx| page.visible_keys(cx)).first(), Some(&"terminal.fontSize"));
    }

    #[gpui::test]
    fn settings_reload_preserves_drafts_and_updates_untouched_fields(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (page, cx) = cx.add_window_view(|_, cx| SettingsPage::new(cx));
        page.update(cx, |page, cx| page.fields["editor.fontSize"].update(cx, |f, cx| f.set_text("18", cx)));
        cx.update(|_, cx| {
            let settings = cx.global_mut::<Settings>();
            settings.minimap = !settings.minimap;
            settings.terminal_font_size = 22.;
        });
        page.update(cx, |page, cx| page.sync_fields(cx));
        page.read_with(cx, |page, cx| {
            assert_eq!(page.fields["editor.fontSize"].read(cx).text(), "18");
            assert!(page.has_draft("editor.fontSize", cx));
            assert_eq!(page.fields["terminal.fontSize"].read(cx).text(), "22");
            assert!(!page.has_draft("terminal.fontSize", cx));
        });
        // An acknowledged save makes that field clean; future changes can sync normally.
        cx.update(|_, cx| cx.global_mut::<Settings>().editor_font_size = 18.);
        page.update(cx, |page, cx| page.sync_fields(cx));
        assert!(!page.read_with(cx, |page, cx| page.has_draft("editor.fontSize", cx)));
        cx.update(|_, cx| cx.global_mut::<Settings>().editor_font_size = 20.);
        page.update(cx, |page, cx| page.sync_fields(cx));
        assert_eq!(page.read_with(cx, |page, cx| page.fields["editor.fontSize"].read(cx).text().to_string()), "20");
    }

    #[gpui::test]
    fn explicit_settings_reset_discards_unsaved_drafts(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (page, cx) = cx.add_window_view(|_, cx| SettingsPage::new(cx));
        page.update(cx, |page, cx| {
            page.fields["editor.fontSize"].update(cx, |f, cx| f.set_text("invalid draft", cx));
            page.sync_fields(cx);
            assert_eq!(page.fields["editor.fontSize"].read(cx).text(), "invalid draft");
            page.discard_drafts();
            page.sync_fields(cx);
            assert_eq!(page.fields["editor.fontSize"].read(cx).text(), shown(&current(cx.global::<Settings>(), "editor.fontSize")));
        });
    }
}
