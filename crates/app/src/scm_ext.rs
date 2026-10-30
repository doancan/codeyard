//! Source Control for folders a code extension's provider claims (`[[scm]]`, #144): its head, input box and groups of
//! changed files, drawn like git's. Every button runs one of the extension's commands; the workbench does that
//! (workbench_ext_scm.rs) and pushes the provider's state back here with `set_ext_sources`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ext_api::value::{FileState, ScmAction, ScmGroup, ScmResource, ScmState};
use gpui::{AnyElement, Context, Entity, Focusable, SharedString, Subscription, Window, div, prelude::*, px};

use super::{ScmEvent, ScmPanel};
use crate::git::Mark;
use crate::i18n::t;
use crate::input::{FieldEvent, TextField};
use crate::ui::{self, c, icon, layout, palette, text};

/// What the panel shows for one claimed folder.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtSource {
    /// The folder, a path on the window's host: the key of everything else.
    pub root: String,
    /// The provider's label and the folder's name.
    pub title: String,
    /// The provider's state, already checked; none until the first answer.
    pub state: Option<Arc<ScmState>>,
    pub error: Option<String>,
    /// A button's command runs.
    pub busy: bool,
    /// The provider's message about the input box's text.
    pub validation: Option<String>,
}

/// Something the user did in a provider's section.
#[derive(Clone, Debug, PartialEq)]
pub enum ExtEvent {
    /// A button: run `command` with these files and the box's text. `accept`: the box's own button (or Enter).
    Run { root: String, command: String, uris: Vec<String>, message: String, accept: bool },
    /// The box's text changed.
    Typed { root: String, text: String },
    /// Show a file's changes.
    Open { root: String, uri: String },
    /// The Refresh button.
    Refresh,
}

/// The panel's side: the sources and each one's input box.
#[derive(Default)]
pub(crate) struct ExtSources {
    pub(crate) list: Vec<ExtSource>,
    fields: HashMap<String, (Entity<TextField>, Subscription)>,
    /// (root, group id) of the closed groups.
    closed: HashSet<(String, String)>,
}

/// A file state as git's marks show it; `None` for an ignored file (muted, `I`).
pub(crate) fn mark(state: FileState) -> Option<Mark> {
    match state {
        FileState::Modified => Some(Mark::Modified),
        FileState::Added => Some(Mark::Added),
        FileState::Deleted => Some(Mark::Deleted),
        FileState::Renamed => Some(Mark::Renamed),
        FileState::Untracked => Some(Mark::Untracked),
        FileState::Conflicted => Some(Mark::Conflict),
        FileState::Ignored => None,
    }
}

/// The state in words, for tooltips and screen readers.
pub(crate) fn describe(state: FileState) -> &'static str {
    mark(state).map_or(t("Ignored"), Mark::describe)
}

/// The path of a resource relative to its folder (the uri when it is not under it).
pub(crate) fn relative(root: &str, uri: &str) -> String {
    let path = crate::ext_broker::normalize(uri).unwrap_or_else(|| uri.to_string());
    path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).map(str::to_string).unwrap_or(path)
}

/// Groups as shown: those that hide when empty and are empty are left out.
pub(crate) fn shown_groups(state: &ScmState) -> impl Iterator<Item = &ScmGroup> {
    state.groups.iter().filter(|g| !(g.hide_when_empty && g.resources.is_empty()))
}

impl ScmPanel {
    /// The workbench's sources replace the panel's: input boxes come and go with their folders, and a provider's
    /// `value` replaces the box's text when it changes.
    pub fn set_ext_sources(&mut self, list: Vec<ExtSource>, cx: &mut Context<Self>) {
        let ext = &mut self.ext;
        ext.fields.retain(|root, _| list.iter().any(|s| &s.root == root));
        for s in &list {
            let placeholder = s.state.as_ref().map(|st| st.input.placeholder.clone()).unwrap_or_default();
            let value = s.state.as_ref().and_then(|st| st.input.value.clone());
            let before = ext.list.iter().find(|o| o.root == s.root).and_then(|o| o.state.as_ref()).and_then(|st| st.input.value.clone());
            if !ext.fields.contains_key(&s.root) {
                let field = cx.new(|cx| TextField::new("", cx).multiline());
                let root = s.root.clone();
                let sub = cx.subscribe(&field, move |this, field, ev: &FieldEvent, cx| {
                    let text = field.read(cx).text().to_string();
                    match ev {
                        FieldEvent::Changed => cx.emit(ScmEvent::Ext(ExtEvent::Typed { root: root.clone(), text })),
                        FieldEvent::Submit => this.ext_accept(&root, cx),
                        FieldEvent::Escape => {}
                    }
                });
                ext.fields.insert(s.root.clone(), (field, sub));
            }
            if let Some((field, _)) = ext.fields.get(&s.root) {
                field.update(cx, |f, cx| {
                    f.set_placeholder(&placeholder);
                    if let Some(v) = value.as_ref().filter(|v| before.as_ref() != Some(*v)) {
                        f.set_text(v, cx);
                    }
                });
            }
        }
        if ext.list != list {
            ext.list = list;
            cx.notify();
        }
    }

    pub fn ext_sources(&self) -> &[ExtSource] {
        &self.ext.list
    }

    /// The input box's text of a claimed folder.
    pub fn ext_message(&self, root: &str, cx: &gpui::App) -> String {
        self.ext.fields.get(root).map(|(f, _)| f.read(cx).text().to_string()).unwrap_or_default()
    }

    pub fn set_ext_message(&mut self, root: &str, text: &str, cx: &mut Context<Self>) {
        if let Some((f, _)) = self.ext.fields.get(root) {
            f.update(cx, |f, cx| f.set_text(text, cx));
        }
    }

    pub(crate) fn focus_ext_message(&self, root: &str, window: &mut Window, cx: &gpui::App) {
        if let Some((f, _)) = self.ext.fields.get(root) {
            window.focus(&f.focus_handle(cx));
        }
    }

    /// The box's button (or Enter in the box): its command, with every file the provider lists.
    pub(crate) fn ext_accept(&mut self, root: &str, cx: &mut Context<Self>) {
        let Some(source) = self.ext.list.iter().find(|s| s.root == root) else { return };
        let Some(state) = source.state.clone().filter(|_| !source.busy) else { return };
        let Some(accept) = &state.input.accept else { return };
        let uris = state.groups.iter().flat_map(|g| g.resources.iter().map(|r| r.uri.clone())).collect();
        let message = self.ext_message(root, cx);
        cx.emit(ScmEvent::Ext(ExtEvent::Run { root: root.to_string(), command: accept.command.clone(), uris, message, accept: true }));
    }

    /// A button on a file or a group header.
    pub(crate) fn ext_run(&mut self, root: &str, action: &ScmAction, uris: Vec<String>, cx: &mut Context<Self>) {
        if self.ext.list.iter().any(|s| s.root == root && s.busy) {
            return;
        }
        let message = self.ext_message(root, cx);
        cx.emit(ScmEvent::Ext(ExtEvent::Run { root: root.to_string(), command: action.command.clone(), uris, message, accept: false }));
    }

    pub(crate) fn ext_group_open(&self, root: &str, group: &str) -> bool {
        !self.ext.closed.contains(&(root.to_string(), group.to_string()))
    }

    pub(crate) fn toggle_ext_group(&mut self, root: &str, group: &str, cx: &mut Context<Self>) {
        let key = (root.to_string(), group.to_string());
        if !self.ext.closed.remove(&key) {
            self.ext.closed.insert(key);
        }
        cx.notify();
    }

    /// Git's part of the panel gives way when its repository is a folder a provider claims (or holds one).
    pub(crate) fn git_hidden(&self) -> bool {
        let Some(st) = &self.status else { return false };
        let git = format!("{}/", st.root.trim_end_matches('/'));
        self.ext.list.iter().any(|s| {
            let root = format!("{}/", s.root.trim_end_matches('/'));
            root == git || root.starts_with(&git) || git.starts_with(&root)
        })
    }

    /// One section per claimed folder.
    pub(crate) fn ext_sections(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let list = self.ext.list.clone();
        list.iter().enumerate().map(|(i, s)| self.ext_section(i, s, window, cx)).collect()
    }

    fn ext_section(&self, i: usize, s: &ExtSource, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = *palette(cx);
        let root = s.root.clone();
        let mut col = div().id(SharedString::from(format!("scm-ext-{i}"))).flex().flex_col().pb(px(theme::SPACE[3])).child(
            div()
                .px(px(theme::SPACE[6]))
                .h(px(layout::ROW_H))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .text_size(px(text::XS.0))
                .child(icon("git-branch", 14., p.text_muted))
                .child(div().flex_none().text_color(c(p.text)).child(s.title.clone()))
                .children(
                    s.state
                        .as_ref()
                        .and_then(|st| st.head.clone())
                        .map(|h| div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(c(p.text_muted)).child(h)),
                )
                .when(s.busy, |d| d.child(ui::spinner(SharedString::from(format!("scm-ext-busy-{i}")), 14., p.text_muted))),
        );
        let Some(state) = s.state.clone() else {
            let note = s.error.clone().unwrap_or_else(|| t("Asking the source control provider…").to_string());
            return col
                .child(
                    div().px(px(theme::SPACE[6])).text_size(px(text::XS.0)).text_color(c(if s.error.is_some() { p.danger } else { p.text_muted })).child(note),
                )
                .into_any_element();
        };
        if let (Some(accept), Some((field, _))) = (&state.input.accept, self.ext.fields.get(&s.root)) {
            let focused = field.focus_handle(cx).is_focused(window);
            let field_box = ui::input_box(SharedString::from(format!("scm-ext-box-{i}")), &p, p.bg, focused).child(field.clone());
            let can = !s.busy && s.validation.is_none();
            let r = root.clone();
            col = col.child(
                div()
                    .px(px(theme::SPACE[6]))
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[3]))
                    .child(field_box)
                    .children(s.validation.clone().map(|v| div().text_size(px(text::XS.0)).text_color(c(p.warning)).child(v)))
                    .child(
                        div()
                            .id(SharedString::from(format!("scm-ext-accept-{i}")))
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(theme::SPACE[2]))
                            .rounded(px(theme::radius::SM))
                            .bg(c(p.accent))
                            .text_color(c(p.on_accent))
                            .cursor_pointer()
                            .when(!can, |d| d.opacity(0.5))
                            .hover(|d| d.opacity(0.9))
                            .child(action_icon(accept.icon.as_deref().unwrap_or("check"), p.on_accent))
                            .child(accept.title.clone())
                            .on_click(cx.listener(move |this, _, _, cx| this.ext_accept(&r, cx))),
                    ),
            );
        }
        if let Some(e) = &s.error {
            col = col.child(div().px(px(theme::SPACE[6])).text_size(px(text::XS.0)).text_color(c(p.danger)).child(e.clone()));
        }
        let mut any = false;
        for (g, group) in shown_groups(&state).enumerate() {
            any |= !group.resources.is_empty();
            col = col.child(self.ext_group_header(i, g, &root, group, cx));
            if self.ext_group_open(&root, &group.id) {
                for (r, res) in group.resources.iter().enumerate() {
                    col = col.child(self.ext_row(i, g, r, &root, group, res, cx));
                }
            }
        }
        if !any {
            col = col.child(div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).child(t("No changes.")));
        }
        col.into_any_element()
    }

    fn ext_group_header(&self, i: usize, g: usize, root: &str, group: &ScmGroup, cx: &mut Context<Self>) -> AnyElement {
        let p = *palette(cx);
        let el: SharedString = format!("scm-ext-{i}-g{g}").into();
        let open = self.ext_group_open(root, &group.id);
        let uris: Vec<String> = group.resources.iter().map(|r| r.uri.clone()).collect();
        let (r1, id) = (root.to_string(), group.id.clone());
        let mut buttons = Vec::new();
        for (k, a) in group.group_actions.iter().enumerate() {
            let (r, a2, u) = (root.to_string(), a.clone(), uris.clone());
            buttons.push(hover_button(format!("{el}-a{k}"), a, &p).invisible().group_hover(el.clone(), |s| s.visible()).on_click(cx.listener(
                move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.ext_run(&r, &a2, u.clone(), cx);
                },
            )));
        }
        div()
            .id(el.clone())
            .group(el.clone())
            .w_full()
            .h(px(layout::ROW_H))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .px(px(theme::SPACE[4]))
            .text_size(px(text::XS.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(c(p.text_muted))
            .cursor_pointer()
            .child(icon(if open { "chevron-down" } else { "chevron-right" }, 14., p.text_muted))
            .child(div().flex_1().child(group.label.clone()))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_ext_group(&r1, &id, cx)))
            .children(buttons)
            .when(!group.resources.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(theme::SPACE[2]))
                        .rounded(px(theme::radius::LG))
                        .bg(c(p.surface_active))
                        .text_color(c(p.text))
                        .child(group.resources.len().to_string()),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn ext_row(&self, i: usize, g: usize, r: usize, root: &str, group: &ScmGroup, res: &ScmResource, cx: &mut Context<Self>) -> AnyElement {
        let p = *palette(cx);
        let rel = relative(root, &res.uri);
        let name = ui::file_name(&rel).to_string();
        let dir = rel.rsplit_once('/').map(|(d, _)| d.to_string());
        let color = mark(res.state).map_or(p.text_muted, |m| m.color(&p));
        let letter = mark(res.state).map_or("I", Mark::letter);
        let el: SharedString = format!("scm-ext-{i}-g{g}-r{r}").into();
        let mut buttons = Vec::new();
        for (k, a) in group.resource_actions.iter().enumerate() {
            let (rt, a2, u) = (root.to_string(), a.clone(), vec![res.uri.clone()]);
            buttons.push(hover_button(format!("{el}-a{k}"), a, &p).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.ext_run(&rt, &a2, u.clone(), cx);
            })));
        }
        let tip = match &res.detail {
            Some(d) => format!("{rel} · {} · {d}", describe(res.state)),
            None => format!("{rel} · {}", describe(res.state)),
        };
        let (rt, uri) = (root.to_string(), res.uri.clone());
        div()
            .id(el.clone())
            .group(el.clone())
            .w_full()
            .h(px(layout::ROW_H))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .pl(px(theme::SPACE[6]))
            .pr(px(theme::SPACE[4]))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_hover)))
            .child(crate::icon_theme::file_icon_el(&name, layout::ICON, &p))
            .child(div().flex_shrink().min_w_0().overflow_hidden().text_color(c(color)).when(res.state == FileState::Deleted, |d| d.line_through()).child(name))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .children(dir)
                    .children(res.detail.clone().map(|d| format!(" · {d}"))),
            )
            .child(div().flex().invisible().group_hover(el.clone(), |s| s.visible()).children(buttons))
            .child(div().w(px(12.)).flex_none().text_size(px(text::XS.0)).text_color(c(color)).child(letter))
            .when(!self.menu_open, |d| d.tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.clone().into())).into()))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ScmEvent::Ext(ExtEvent::Open { root: rt.clone(), uri: uri.clone() }))))
            .into_any_element()
    }
}

/// An icon the provider named, or a neutral one this version does not have.
fn action_icon(name: &str, color: u32) -> gpui::Svg {
    icon(if ui::has_icon(name) { name } else { "circle-dashed" }, 14., color)
}

fn hover_button(id: String, a: &ScmAction, p: &ui::Palette) -> gpui::Stateful<gpui::Div> {
    let tip: SharedString = a.title.clone().into();
    let hover = p.surface_active;
    div()
        .id(SharedString::from(id))
        .size(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::radius::SM))
        .cursor_pointer()
        .hover(move |d| d.bg(c(hover)))
        .child(action_icon(a.icon.as_deref().unwrap_or("circle-dashed"), p.text_muted))
        .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.clone())).into())
}
