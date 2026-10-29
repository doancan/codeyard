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
}

pub const CATEGORIES: &[&str] =
    &["Commonly Used", "Modified", "Modules", "Editor", "Workbench", "Files", "Explorer", "Search", "Git", "Terminal", "Window", "Remote & Languages"];

/// Keys of the "Commonly Used" category.
const COMMON: &[&str] = &[
    "modules.containers.enabled",
    "modules.webPreview.enabled",
    "theme",
    "ui.scale",
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
];

/// The category of a setting (from the key prefix).
pub fn category_of(key: &str) -> &'static str {
    if key == "ui.scale" {
        return "Window";
    }
    match key.split('.').next().unwrap_or("") {
        "editor" | "data" => "Editor",
        "workbench" | "theme" => "Workbench",
        "files" => "Files",
        "explorer" => "Explorer",
        "search" => "Search",
        "git" => "Git",
        "terminal" => "Terminal",
        "window" | "update" => "Window",
        "modules" | "webPreview" => "Modules",
        _ => "Remote & Languages",
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

/// Whether it matches the search (key, title, description).
pub fn matches_query(key: &str, about: &str, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || key.to_lowercase().contains(&q)
        || title_of(key).to_lowercase().contains(&q)
        || about.to_lowercase().contains(&q)
        || crate::i18n::t(about).to_lowercase().contains(&q)
}

pub struct SettingsPage {
    query: Entity<TextField>,
    category: usize,
    /// Fields for number and text settings.
    fields: HashMap<&'static str, Entity<TextField>>,
    errors: HashMap<&'static str, String>,
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
        "Remote & Languages" => t("Remote & Languages"),
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
                if !matches!(ev, FieldEvent::Submit) {
                    return;
                }
                let text = f.read(cx).text().to_string();
                match parse_setting_input(key, kind, &text) {
                    Ok(v) => {
                        this.errors.remove(key);
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
        let mut page = SettingsPage { query, category: 0, fields, errors: HashMap::new(), focus_handle: cx.focus_handle(), _subs: subs };
        page.sync_fields(cx);
        page
    }

    /// Fill the fields from the current settings (when the settings file changes).
    pub fn sync_fields(&mut self, cx: &mut Context<Self>) {
        let s = cx.global::<Settings>().clone();
        for (key, field) in &self.fields {
            let text = shown(&current(&s, key));
            if field.read(cx).text() != text {
                field.update(cx, |f, cx| f.set_text(&text, cx));
            }
        }
    }

    pub fn focus_search(&self, window: &mut Window, cx: &App) {
        window.focus(&self.query.focus_handle(cx));
    }

    /// Filter the page as if `q` had been typed into its search field.
    pub fn set_query(&mut self, q: &str, cx: &mut Context<Self>) {
        self.query.update(cx, |f, cx| f.set_text(q, cx));
    }

    #[cfg(test)]
    pub fn submit_field_for_test(&mut self, key: &'static str, text: &str, cx: &mut Context<Self>) {
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
        let cat = CATEGORIES[self.category];
        let s = cx.global::<Settings>();
        let mut keys: Vec<_> = Self::all_entries()
            .into_iter()
            .filter(|(k, about)| {
                if searching {
                    matches_query(k, about, &query)
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
        }
        keys
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
                        .h(px(26.))
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
                .gap(px(theme::SPACE[1]))
                .children(opts.iter().enumerate().map(|(oi, o)| {
                    let active = now.as_str() == Some(*o);
                    let value = (*o).to_string();
                    div()
                        .id((SharedString::from(format!("opt-{key}")), oi))
                        .h(px(24.))
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
                    .w(px(200.))
                    .bg(c(p.bg))
                    .border_1()
                    .border_color(c(if self.errors.contains_key(key) { p.danger } else { p.border_input }))
                    .rounded(px(theme::radius::SM))
                    .child(f.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            },
            _ => div().into_any_element(),
        };
        let reset = modified.then(|| {
            div()
                .id(("reset", ix))
                .flex_none()
                .cursor_pointer()
                .text_size(px(text::XXS.0))
                .text_color(c(p.accent))
                .hover(|d| d.underline())
                .child(crate::i18n::t("Reset"))
                .tooltip(|_, cx| {
                    cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Back to the default (removes the value from settings.jsonc)").into())).into()
                })
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(SettingsEvent::Write(key.to_string(), Value::Null))))
        });
        div()
            .id(("setting", ix))
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[2]))
            .py(px(theme::SPACE[5]))
            .border_b_1()
            .border_color(c(p.border))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(theme::SPACE[8]))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(theme::SPACE[1]))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(theme::SPACE[3]))
                                    .children(modified.then(|| div().flex_none().size(px(6.)).rounded_full().bg(c(p.accent))))
                                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_size(px(text::SM.0)).child(
                                        if key == "modules.containers.enabled" {
                                            crate::i18n::t("Container Environments").to_string()
                                        } else if key == "modules.webPreview.enabled" {
                                            crate::i18n::t("Web Preview").to_string()
                                        } else {
                                            title_of(key)
                                        },
                                    ))
                                    .children(reset),
                            )
                            .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(crate::i18n::t(about).to_string()))
                            .child(div().font_family(ui::MONO_FONT).text_size(px(text::XXS.0)).text_color(c(p.line_number)).child(key.to_string()))
                            .children(source_note(key, cx).map(|note| {
                                div().debug_selector(move || format!("setting-source-{key}")).text_size(px(text::XS.0)).text_color(c(p.accent)).child(note)
                            })),
                    )
                    .child(div().w(px(240.)).flex_none().flex().justify_end().child(control)),
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
        let searching = !self.query.read(cx).text().is_empty();
        let rows: Vec<gpui::AnyElement> = keys.iter().enumerate().map(|(i, k)| self.entry_view(k, i, &s, cx)).collect();
        let nav = CATEGORIES.iter().enumerate().map(|(i, name)| {
            let active = i == self.category && !searching;
            let count = Self::category_count(name, &s);
            let hot = *name == "Modified" && count > 0;
            div()
                .id(("set-cat", i))
                .h(px(28.))
                .px(px(theme::SPACE[5]))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .text_size(px(text::SM.0))
                .when(active, |d| d.bg(c(p.surface_active)).text_color(c(p.text)))
                .when(!active, |d| d.text_color(c(p.text_muted)).hover(|d| d.bg(c(p.surface_hover))))
                .child(div().flex_1().child(category_label(name)))
                .child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(if hot { p.accent } else { p.line_number })).child(count.to_string()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.category = i;
                    this.query.update(cx, |q, cx| q.set_text("", cx));
                    cx.notify();
                }))
        });
        let heading =
            if searching { crate::i18n::tn("{} result", "{} results", rows.len(), &[]) } else { category_label(CATEGORIES[self.category]).to_string() };
        div()
            .id("settings-page")
            .key_context("SettingsPage")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[5]))
                    .px(px(theme::SPACE[9]))
                    .py(px(theme::SPACE[5]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .child(icon("settings", 20., p.accent))
                    .child(div().text_size(px(text::LG.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(crate::i18n::t("Settings")))
                    .child(
                        crate::ui::input_box("settings-search-box", &p, p.surface, self.query.focus_handle(cx).is_focused(window))
                            .flex_1()
                            .max_w(px(520.))
                            .child(crate::ui::search_icon(&p))
                            .child(div().flex_1().min_w_0().child(self.query.clone())),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("set-open-json")
                            .h(px(26.))
                            .px(px(theme::SPACE[4]))
                            .flex()
                            .items_center()
                            .rounded(px(theme::radius::SM))
                            .bg(c(p.surface_hover))
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_active)))
                            .text_size(px(text::XS.0))
                            .child(crate::i18n::t("Open settings.jsonc"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::OpenJson))),
                    )
                    .child(
                        div()
                            .id("set-open-keys")
                            .h(px(26.))
                            .px(px(theme::SPACE[4]))
                            .flex()
                            .items_center()
                            .rounded(px(theme::radius::SM))
                            .bg(c(p.surface_hover))
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_active)))
                            .text_size(px(text::XS.0))
                            .child(crate::i18n::t("Keyboard Shortcuts"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::OpenKeybindings))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(200.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(px(theme::SPACE[1]))
                            .p(px(theme::SPACE[4]))
                            .border_r_1()
                            .border_color(c(p.border))
                            .children(nav)
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("set-icon-theme")
                                    .h(px(28.))
                                    .px(px(theme::SPACE[5]))
                                    .flex()
                                    .items_center()
                                    .gap(px(theme::SPACE[3]))
                                    .rounded(px(theme::radius::SM))
                                    .cursor_pointer()
                                    .text_size(px(text::XS.0))
                                    .text_color(c(p.text_muted))
                                    .hover(|d| d.bg(c(p.surface_hover)))
                                    .child(icon("folder-tree", 14., p.text_muted))
                                    .child(crate::i18n::t("File icon theme…"))
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::PickIconTheme))),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-list")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .px(px(theme::SPACE[9]))
                            .py(px(theme::SPACE[5]))
                            .child(div().pb(px(theme::SPACE[3])).text_size(px(text::LG.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(heading))
                            .children(rows)
                            .when(keys.is_empty(), |d| {
                                d.child(div().pt(px(theme::SPACE[5])).text_color(c(p.text_muted)).child(
                                    if CATEGORIES[self.category] == "Modified" && !searching {
                                        crate::i18n::t("Nothing changed: every setting has its default value.")
                                    } else {
                                        crate::i18n::t("No setting matches.")
                                    },
                                ))
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_module_is_findable_without_search() {
        assert_eq!(category_of("modules.containers.enabled"), "Modules");
        assert!(CATEGORIES.contains(&"Modules"));
        assert!(COMMON.contains(&"modules.containers.enabled"));
        assert!(COMMON.contains(&"modules.webPreview.enabled"));
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
}
