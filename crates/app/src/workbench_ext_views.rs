//! Workbench: views of code extensions (docs/plugin-architecture.md §5.3, docs/extensions/reference/ui.md). A view sits
//! in the sidebar (its own activity bar button), in the bottom panel (a tab) or in the editor area (a page tab).
//! Opening one starts its extension (`view:<id>`) and sends `request.render`; the JSON tree that comes back is checked
//! by `ext_ui::decode_json`, diffed by key against the tree on screen, and the input state (typed text, ticks, choices,
//! selected and expanded rows) stays here, so a re-render never loses what the user did. `ui.invalidate` asks for the
//! tree again; what the user does goes back as `request.ui-event`. Drawing is in `ext_ui_render.rs`, screen reader
//! nodes in `a11y_ext_views.rs`.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use ext_api::value::{self as v, LogLevel};
use ext_manifest::ViewLocation;
use ext_proto::ToHost;
use ext_ui::{Element, Event as UiEvent, Key, KeyPath, Kind, Node, PageToken, Selection, Value};
use futures::StreamExt;
use gpui::{Subscription, UniformListScrollHandle};

use super::*;
use crate::input::{FieldEvent, TextField};

#[path = "workbench_ext_view_images.rs"]
mod images;

/// The pause in typing after which a text input sends `change`.
const CHANGE_PAUSE: Duration = Duration::from_millis(400);

/// A view as its extension's manifest declares it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Declared {
    /// `publisher.name.local`.
    pub(super) id: String,
    pub(super) extension: String,
    pub(super) local: String,
    pub(super) title: String,
    pub(super) location: ViewLocation,
    /// The manifest's `when`: the view is offered only while it holds.
    pub(super) when: Option<String>,
}

impl Declared {
    /// Shown on every surface of the view, so an extension cannot pose as Codeyard or as another publisher.
    pub(super) fn publisher(&self) -> &str {
        self.extension.split('.').next().unwrap_or(&self.extension)
    }
}

/// The views of the active code extensions, in manifest order.
pub(super) fn declared_views() -> Vec<Declared> {
    let reg = crate::ext::current();
    let mut out = Vec::new();
    for e in reg.active.iter().filter(|e| e.manifest.code.is_some()) {
        for d in &e.manifest.contributions.views {
            let local = d.id.strip_prefix(e.id()).and_then(|r| r.strip_prefix('.')).unwrap_or(&d.id).to_string();
            out.push(Declared { id: d.id.clone(), extension: e.id().to_string(), local, title: d.title.clone(), location: d.location, when: d.when.clone() });
        }
    }
    out
}

/// One open view: the tree on screen and everything the host keeps for it.
pub(super) struct HostedView {
    pub(super) decl: Declared,
    pub(super) tree: Option<Node>,
    pub(super) state: ext_ui::State,
    /// Why the last render was refused or failed; shown in place of the view.
    pub(super) error: Option<String>,
    in_flight: bool,
    /// Invalidated while a render was on its way: ask once more when it lands.
    again: bool,
    /// Opened before its first tree arrived: the keyboard goes into it when the tree does.
    focus_first: bool,
    pub(super) fields: HashMap<KeyPath, (Entity<TextField>, Subscription)>,
    /// Interactive elements and row containers, so Tab reaches them and a screen reader can focus them.
    pub(super) focus: HashMap<KeyPath, FocusHandle>,
    /// The keyboard row of each list, table and tree.
    pub(super) cursor: HashMap<KeyPath, usize>,
    pub(super) scroll: HashMap<KeyPath, UniformListScrollHandle>,
    /// Parsed `markdown` sources and decoded `image` bytes, by a hash of their content.
    pub(super) markdown: HashMap<u64, Rc<Vec<crate::markdown::Block>>>,
    pub(super) images: HashMap<u64, Option<Arc<gpui::Image>>>,
    /// `image` elements with an `https` URL, fetched through the extension's `net` permission.
    pub(super) remote: images::RemoteImages,
    /// Bumped on every keystroke in a text input; a pending `change` is sent only if it is still the latest.
    typing: u64,
    /// Operations of the last diff (tests).
    pub(super) last_ops: usize,
}

impl HostedView {
    fn new(decl: Declared) -> Self {
        HostedView {
            decl,
            tree: None,
            state: ext_ui::State::new(),
            error: None,
            in_flight: false,
            again: false,
            focus_first: false,
            fields: HashMap::new(),
            focus: HashMap::new(),
            cursor: HashMap::new(),
            scroll: HashMap::new(),
            markdown: HashMap::new(),
            images: HashMap::new(),
            remote: images::RemoteImages::default(),
            typing: 0,
            last_ops: 0,
        }
    }

    /// The node at `path` (keys from the root, unkeyed elements skipped).
    pub(super) fn find(&self, path: &[Key]) -> Option<&Node> {
        find(self.tree.as_ref()?, path)
    }
}

pub(super) fn find<'a>(root: &'a Node, path: &[Key]) -> Option<&'a Node> {
    let mut stack: Vec<(&Node, usize)> = vec![(root, 0)];
    while let Some((n, depth)) = stack.pop() {
        let depth = match &n.key {
            Some(k) if path.get(depth) == Some(k) => depth + 1,
            Some(_) => continue,
            None => depth,
        };
        if depth == path.len() && n.key.is_some() {
            return Some(n);
        }
        stack.extend(n.children.iter().map(|c| (c, depth)));
    }
    None
}

pub(super) fn content_hash(x: &impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    x.hash(&mut h);
    h.finish()
}

/// The image format from its first bytes.
pub(super) fn image_format(b: &[u8]) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat::*;
    Some(match b {
        [0x89, b'P', b'N', b'G', ..] => Png,
        [0xff, 0xd8, 0xff, ..] => Jpeg,
        [b'G', b'I', b'F', b'8', ..] => Gif,
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Webp,
        _ if b.starts_with(b"<svg") || b.starts_with(b"<?xml") => Svg,
        _ => return None,
    })
}

/// Views of code extensions in this window.
#[derive(Default)]
pub(super) struct ViewsState {
    pub(super) hosted: Vec<HostedView>,
    /// The view shown for `Panel::ExtView` and `Bottom::ExtView`.
    pub(super) sidebar: Option<String>,
    pub(super) bottom: Option<String>,
    /// Render requests sent; a theme change must not add one.
    pub(super) renders: u64,
    /// Declared views whose `when` is false: not offered, and closed if open. Worked out each frame.
    pub(super) hidden: HashSet<String>,
    /// The select whose menu is open: view, path and its option keys.
    choosing: Option<(String, KeyPath, Vec<Key>)>,
    /// Rows the last frame built per row container (tests: only the visible ones).
    pub(super) built_rows: HashMap<KeyPath, usize>,
    /// Tests: requests and events are recorded, not sent.
    #[cfg(test)]
    pub(super) offline: bool,
    #[cfg(test)]
    pub(super) sent: Vec<(String, String)>,
    /// The background colour of the theme the last view frame was drawn with.
    #[cfg(test)]
    pub(super) drawn_with: std::cell::Cell<u32>,
}

impl Workbench {
    pub(super) fn ext_view(&self, id: &str) -> Option<&HostedView> {
        self.cfg.exts.views.hosted.iter().find(|h| h.decl.id == id)
    }

    fn ext_view_mut(&mut self, id: &str) -> Option<&mut HostedView> {
        self.cfg.exts.views.hosted.iter_mut().find(|h| h.decl.id == id)
    }

    /// "Open View: …" in the palette, one per declared view whose `when` holds.
    pub(super) fn ext_view_commands(&self) -> Vec<Command> {
        declared_views()
            .into_iter()
            .filter(|d| !self.cfg.exts.views.hidden.contains(&d.id))
            .map(|d| Command { label: crate::i18n::tf("Open View: {0} ({1})", &[&d.title, &d.publisher()]), run: Run::ExtensionView(d.id) })
            .collect()
    }

    /// Show a view in its place and ask its extension for the tree the first time.
    pub(super) fn open_ext_view(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(decl) = declared_views().into_iter().find(|d| d.id == id) else { return };
        if !self.ext_view_when_holds(&decl, cx) {
            return;
        }
        match decl.location {
            ViewLocation::Sidebar => {
                self.chrome.panel = Some(Panel::ExtView);
                self.cfg.exts.views.sidebar = Some(decl.id.clone());
            }
            ViewLocation::Bottom => {
                self.chrome.panel_open = true;
                self.chrome.bottom = Bottom::ExtView;
                self.cfg.exts.views.bottom = Some(decl.id.clone());
            }
            ViewLocation::Editor => {
                // The consent sheet waits for the user's answer; no extension page takes its place.
                if self.cfg.exts.pending.is_some() && matches!(self.tabs.page, Some(Page::Extension)) {
                    self.notice = Some(Notice::Info(crate::i18n::t("Answer the install sheet first.").into()));
                    cx.notify();
                    return;
                }
                self.tabs.page = Some(Page::ExtView(decl.id.clone()));
                self.tabs._page_sub = None;
                self.tabs.page_active = true;
                self.tabs.diff_active = false;
                self.tabs.large_active = false;
            }
        }
        if self.ext_view(id).is_none() {
            self.cfg.exts.views.hosted.push(HostedView::new(decl));
        }
        if self.ext_view(id).is_some_and(|h| h.tree.is_some()) {
            self.focus_ext_view(id, window, cx);
        } else if let Some(h) = self.ext_view_mut(id) {
            h.focus_first = true;
            if !h.in_flight {
                self.request_ext_view_render(id, window, cx);
            }
        }
        cx.notify();
    }

    /// `ui.invalidate(view)` from an extension: draw it again if it is open. A view that was never opened is not
    /// started by it.
    pub(super) fn on_ext_view_invalidate(&mut self, extension: &str, local: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.cfg.exts.views.hosted.iter().find(|h| h.decl.extension == extension && h.decl.local == local).map(|h| h.decl.id.clone()) else {
            return;
        };
        self.request_ext_view_render(&id, window, cx);
    }

    /// Send `request.render`; at most one is on its way per view, and an invalidation meanwhile asks once more.
    pub(super) fn request_ext_view_render(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view_mut(id) else { return };
        if h.in_flight {
            h.again = true;
            return;
        }
        h.in_flight = true;
        let decl = h.decl.clone();
        self.cfg.exts.views.renders += 1;
        #[cfg(test)]
        if self.cfg.exts.views.offline {
            return;
        }
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == decl.extension).cloned() else { return };
        if self.cfg.exts.code.disabled.contains(&decl.extension) {
            let msg = crate::i18n::tf("{} crashed three times and is turned off until Codeyard restarts.", &[&decl.extension]);
            self.ext_view_rendered(id, Err(msg), window, cx);
            return;
        }
        let (req, answer) = self.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).add(&decl.extension);
        let msg = ToHost::Handle { id: req, deadline_ms: 0, request: v::Request::Render { view: decl.local.clone() } };
        self.send_to_extension(&active, &format!("view:{}", decl.local), Some(msg), window, cx);
        let answer = crate::ui::bridge(answer);
        let view = id.to_string();
        cx.spawn_in(window, async move |this, cx| {
            let mut answer = answer;
            let decoded = match answer.next().await {
                Some(Ok(v::Response::View(json))) => cx.background_executor().spawn(async move { ext_ui::decode_json(&json).map_err(|e| e.to_string()) }).await,
                Some(Ok(_)) => Err(crate::i18n::t("render must answer with response.view").to_string()),
                Some(Err(e)) => Err(super::code_glue::explain(&e)),
                None => Err(crate::i18n::t("the extension stopped").to_string()),
            };
            this.update_in(cx, |this, window, cx| this.ext_view_rendered(&view, decoded, window, cx)).ok();
        })
        .detach();
    }

    fn ext_view_rendered(&mut self, id: &str, result: Result<Node, String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view_mut(id) else { return };
        h.in_flight = false;
        let again = std::mem::take(&mut h.again);
        match result {
            Ok(tree) => self.set_ext_view_tree(id, tree, window, cx),
            Err(e) => {
                let (ext, local) = (h.decl.extension.clone(), h.decl.local.clone());
                h.error = Some(e.clone());
                self.log_extension(&ext, LogLevel::Warn, format!("view {local}: {e}"));
            }
        }
        if again {
            self.request_ext_view_render(id, window, cx);
        }
        cx.notify();
    }

    /// A checked tree arrived: diff it by key into the one on screen, bring the input state along, and prepare what
    /// drawing needs (focus handles, text fields, parsed Markdown, decoded images).
    pub(super) fn set_ext_view_tree(&mut self, id: &str, new: Node, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view_mut(id) else { return };
        h.error = None;
        match h.tree.as_mut() {
            Some(old) => {
                let ops = ext_ui::diff(old, &new);
                h.last_ops = ops.len();
                h.state.sync(Some(old), &new);
                if ext_ui::apply(old, ops).is_err() {
                    *old = new;
                }
            }
            None => {
                h.state.sync(None, &new);
                h.last_ops = 1;
                h.tree = Some(new);
            }
        }
        self.prepare_ext_view(id, window, cx);
        if self.ext_view_mut(id).is_some_and(|h| std::mem::take(&mut h.focus_first)) {
            self.focus_ext_view(id, window, cx);
        }
        cx.notify();
    }

    fn prepare_ext_view(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view_mut(id) else { return };
        let Some(tree) = h.tree.take() else { return };
        let mut live: HashSet<KeyPath> = HashSet::new();
        let (mut md, mut imgs, mut urls) = (HashSet::new(), HashSet::new(), Vec::new());
        let mut new_fields = Vec::new();
        let mut stack: Vec<(&Node, KeyPath, bool)> = vec![(&tree, Vec::new(), false)];
        while let Some((n, mut path, in_tree)) = stack.pop() {
            if let Some(k) = &n.key {
                path.push(k.clone());
            }
            let kind = n.kind();
            let keyed = n.key.is_some();
            if keyed && (kind.is_interactive() && !(kind == Kind::Tree && in_tree)) {
                live.insert(path.clone());
                h.focus.entry(path.clone()).or_insert_with(|| cx.focus_handle().tab_stop(true));
            }
            if keyed && kind.has_rows() && !in_tree {
                h.scroll.entry(path.clone()).or_default();
                let rows = if kind == Kind::Tree { super::ext_ui_glue::tree_rows(n, h.state.get(&path)).len() } else { n.children.len() };
                let c = h.cursor.entry(path.clone()).or_default();
                *c = (*c).min(rows.saturating_sub(1));
            }
            match &n.element {
                Element::TextInput { placeholder, multiline, .. } if keyed => {
                    let text = h.state.get(&path).map(|s| s.text.clone()).unwrap_or_default();
                    match h.fields.get(&path) {
                        Some((f, _)) if f.read(cx).text() != text => f.update(cx, |f, cx| f.set_text(&text, cx)),
                        Some(_) => {}
                        None => new_fields.push((path.clone(), placeholder.clone(), *multiline, text)),
                    }
                }
                Element::Markdown { source } => {
                    let k = content_hash(source);
                    md.insert(k);
                    h.markdown.entry(k).or_insert_with(|| Rc::new(crate::markdown::parse(source)));
                }
                Element::Image { source: ext_ui::ImageSource::Bytes(b), .. } => {
                    let k = content_hash(b);
                    imgs.insert(k);
                    h.images.entry(k).or_insert_with(|| image_format(b).map(|f| Arc::new(gpui::Image::from_bytes(f, b.clone()))));
                }
                Element::Image { source: ext_ui::ImageSource::Url(u), .. } => urls.push(u.as_str().to_string()),
                _ => {}
            }
            let below_tree = in_tree || kind == Kind::Tree;
            stack.extend(n.children.iter().map(|c| (c, path.clone(), below_tree)));
        }
        h.focus.retain(|p, _| live.contains(p));
        h.fields.retain(|p, _| live.contains(p));
        h.cursor.retain(|p, _| live.contains(p));
        h.scroll.retain(|p, _| live.contains(p));
        h.markdown.retain(|k, _| md.contains(k));
        h.images.retain(|k, _| imgs.contains(k));
        h.remote.want(urls);
        h.tree = Some(tree);
        for (path, placeholder, multiline, text) in new_fields {
            let field = cx.new(|cx| {
                let f = TextField::new(&placeholder, cx);
                if multiline { f.multiline() } else { f }
            });
            field.update(cx, |f, cx| f.set_text(&text, cx));
            let (view, p) = (id.to_string(), path.clone());
            let sub = cx.subscribe_in(&field, window, move |this, field, ev: &FieldEvent, window, cx| {
                let text = field.read(cx).text().to_string();
                this.on_ext_view_field(&view, &p, text, ev, window, cx);
            });
            let Some(h) = self.ext_view_mut(id) else { return };
            h.focus.insert(path.clone(), field.focus_handle(cx).tab_stop(true));
            h.fields.insert(path, (field, sub));
        }
    }

    fn on_ext_view_field(&mut self, id: &str, path: &KeyPath, text: String, ev: &FieldEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view_mut(id) else { return };
        let Some(state) = h.state.get_mut(path) else { return };
        match ev {
            FieldEvent::Changed if state.text != text => {
                state.text = text.clone();
                h.typing += 1;
                let typing = h.typing;
                let (view, path) = (id.to_string(), path.clone());
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(CHANGE_PAUSE).await;
                    this.update_in(cx, |this, window, cx| {
                        if this.ext_view(&view).is_some_and(|h| h.typing == typing) {
                            this.send_ext_view_event(&view, UiEvent::Change { path, value: Value::Text(text) }, window, cx);
                        }
                    })
                    .ok();
                })
                .detach();
            }
            FieldEvent::Submit => {
                let multiline = matches!(h.find(path).map(|n| &n.element), Some(Element::TextInput { multiline: true, .. }));
                if !multiline {
                    h.typing += 1;
                    self.send_ext_view_event(id, UiEvent::Submit { path: path.clone(), text }, window, cx);
                }
            }
            _ => {}
        }
    }

    /// Send what the user did to the extension (`request.ui-event`). It counts as a user action, like a command, so the
    /// extension may open a link in answer.
    pub(super) fn send_ext_view_event(&mut self, id: &str, event: UiEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(decl) = self.ext_view(id).map(|h| h.decl.clone()) else { return };
        let json = event.to_json();
        #[cfg(test)]
        {
            self.cfg.exts.views.sent.push((id.to_string(), json.clone()));
            if self.cfg.exts.views.offline {
                return;
            }
        }
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == decl.extension).cloned() else { return };
        if self.cfg.exts.code.disabled.contains(&decl.extension) {
            return;
        }
        let (req, answer) = self.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).add(&decl.extension);
        self.cfg.exts.code.user_action.insert(decl.extension.clone(), req);
        let msg = ToHost::Handle { id: req, deadline_ms: 0, request: v::Request::UiEvent { view: decl.local.clone(), event_json: json } };
        self.send_to_extension(&active, &format!("view:{}", decl.local), Some(msg), window, cx);
        let answer = crate::ui::bridge(answer);
        cx.spawn_in(window, async move |this, cx| {
            let mut answer = answer;
            let result = answer.next().await;
            this.update(cx, |this, _| {
                if this.cfg.exts.code.user_action.get(&decl.extension) == Some(&req) {
                    this.cfg.exts.code.user_action.remove(&decl.extension);
                }
                if let Some(Err(e)) = result {
                    this.log_extension(&decl.extension, LogLevel::Warn, format!("view {}: {}", decl.local, super::code_glue::explain(&e)));
                }
            })
            .ok();
        })
        .detach();
    }

    /// A button was pressed.
    pub(super) fn ext_view_click(&mut self, id: &str, path: KeyPath, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = matches!(self.ext_view(id).and_then(|h| h.find(&path)).map(|n| &n.element), Some(Element::Button { disabled: false, .. }));
        if enabled {
            self.send_ext_view_event(id, UiEvent::Click { path }, window, cx);
        }
    }

    pub(super) fn ext_view_toggle(&mut self, id: &str, path: KeyPath, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view_mut(id) else { return };
        if !matches!(h.find(&path).map(|n| &n.element), Some(Element::Checkbox { disabled: false, .. })) {
            return;
        }
        let Some(s) = h.state.get_mut(&path) else { return };
        s.checked = !s.checked;
        let checked = s.checked;
        self.send_ext_view_event(id, UiEvent::Change { path, value: Value::Checked(checked) }, window, cx);
        cx.notify();
    }

    pub(super) fn ext_view_choose(&mut self, id: &str, path: KeyPath, choice: Key, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view_mut(id) else { return };
        let Some(s) = h.state.get_mut(&path) else { return };
        if s.choice.as_ref() == Some(&choice) {
            return;
        }
        s.choice = Some(choice.clone());
        self.send_ext_view_event(id, UiEvent::Change { path, value: Value::Choice(choice) }, window, cx);
        cx.notify();
    }

    /// Up and Down on a focused select step through its options.
    pub(super) fn ext_view_step_choice(&mut self, id: &str, path: KeyPath, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view(id) else { return };
        let Some(Element::Select { options, disabled: false, .. }) = h.find(&path).map(|n| &n.element) else { return };
        let now = h.state.get(&path).and_then(|s| s.choice.as_ref()).and_then(|c| options.iter().position(|o| &o.key == c));
        let next = match now {
            None => 0,
            Some(i) => (i as isize + step).clamp(0, options.len() as isize - 1) as usize,
        };
        if let Some(o) = options.get(next) {
            let key = o.key.clone();
            self.ext_view_choose(id, path, key, window, cx);
        }
    }

    /// A select was clicked: its options in a menu Codeyard draws.
    pub(super) fn ext_view_open_select(&mut self, id: &str, path: KeyPath, at: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view(id) else { return };
        let Some(Element::Select { options, disabled: false, .. }) = h.find(&path).map(|n| &n.element) else { return };
        let chosen = h.state.get(&path).and_then(|s| s.choice.clone());
        let items = options.iter().map(|o| MenuItem::new(&o.label).checked(chosen.as_ref() == Some(&o.key))).collect();
        let commands = (0..options.len()).map(MenuCmd::ExtViewChoice).collect();
        let keys = options.iter().map(|o| o.key.clone()).collect();
        self.cfg.exts.views.choosing = Some((id.to_string(), path, keys));
        let target = MenuTarget { path: String::new(), is_dir: false, tab: None };
        self.show_menu(items, commands, Vec::new(), target, at, window, cx);
    }

    pub(super) fn ext_view_choice_from_menu(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, path, keys)) = self.cfg.exts.views.choosing.take() else { return };
        if let Some(k) = keys.get(ix) {
            self.ext_view_choose(&id, path, k.clone(), window, cx);
        }
    }

    /// A link: with a URL it opens after the user confirms the address; without, the extension hears of it.
    pub(super) fn ext_view_link(&mut self, id: &str, path: KeyPath, window: &mut Window, cx: &mut Context<Self>) {
        match self.ext_view(id).and_then(|h| h.find(&path)).map(|n| n.element.clone()) {
            Some(Element::Link { url: Some(url), .. }) => self.confirm_open_link(url.as_str().to_string(), window, cx),
            Some(Element::Link { url: None, .. }) => self.send_ext_view_event(id, UiEvent::Link { path }, window, cx),
            _ => {}
        }
    }

    /// Select a row of a list, table or tree (`row`: its key). `toggle` adds or removes it in a many-select.
    pub(super) fn ext_view_select_row(&mut self, id: &str, path: KeyPath, row: Key, toggle: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view_mut(id) else { return };
        let selection = match h.find(&path).map(|n| &n.element) {
            Some(Element::List { selection, .. } | Element::Table { selection, .. } | Element::Tree { selection, .. }) => *selection,
            _ => return,
        };
        let Some(s) = h.state.get_mut(&path) else { return };
        let before = s.selected.clone();
        match selection {
            Selection::None => return,
            Selection::Many if toggle => {
                if !s.selected.remove(&row) {
                    s.selected.insert(row);
                }
            }
            _ => {
                s.selected.clear();
                s.selected.insert(row);
            }
        }
        if s.selected != before {
            let rows = s.selected.iter().cloned().collect();
            self.send_ext_view_event(id, UiEvent::Select { path, rows }, window, cx);
        }
        cx.notify();
    }

    /// Expand or collapse a tree item. An item with no children yet and a `more` token asks for its first page.
    pub(super) fn ext_view_expand(&mut self, id: &str, path: KeyPath, item: Key, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view_mut(id) else { return };
        let Some(tree) = h.find(&path) else { return };
        let Some(node) = find_item(tree, &item) else { return };
        let load = match &node.element {
            Element::Tree { more: Some(page), .. } if node.children.is_empty() => Some(page.clone()),
            _ => None,
        };
        if node.children.is_empty() && load.is_none() {
            return;
        }
        let Some(s) = h.state.get_mut(&path) else { return };
        if open {
            s.expanded.insert(item.clone());
        } else {
            s.expanded.remove(&item);
        }
        if let (true, Some(page)) = (open, load) {
            self.send_ext_view_event(id, UiEvent::Expand { path, item: Some(item), page }, window, cx);
        }
        cx.notify();
    }

    /// The "Load more" row of a tree level.
    pub(super) fn ext_view_load_more(&mut self, id: &str, path: KeyPath, item: Option<Key>, page: PageToken, window: &mut Window, cx: &mut Context<Self>) {
        self.send_ext_view_event(id, UiEvent::Expand { path, item, page }, window, cx);
    }

    /// Move the keyboard row of a list, table or tree and keep it on screen.
    pub(super) fn ext_view_move_cursor(&mut self, id: &str, path: &KeyPath, to: usize, cx: &mut Context<Self>) {
        let Some(h) = self.ext_view_mut(id) else { return };
        h.cursor.insert(path.clone(), to);
        if let Some(s) = h.scroll.get(path) {
            s.scroll_to_item(to, gpui::ScrollStrategy::Top);
        }
        cx.notify();
    }

    /// Focus the first element of a view that takes the keyboard (opening a view lands in it). Never while the
    /// consent sheet or a picker waits for the user: a view's render must not take the keyboard from them.
    pub(super) fn focus_ext_view(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.cfg.exts.pending.is_some() || self.overlay.is_some() || self.menu.is_some() {
            return;
        }
        let Some(h) = self.ext_view(id) else { return };
        let Some(tree) = &h.tree else { return };
        if let Some(f) = super::ext_ui_glue::focus_order(tree).into_iter().find_map(|p| h.focus.get(&p)) {
            window.focus(f);
            cx.notify();
        }
    }

    /// The panel tabs of the bottom views. The publisher is in the tooltip and on the view itself.
    pub(super) fn ext_view_panel_tabs(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let shown = (self.chrome.bottom == Bottom::ExtView).then_some(self.cfg.exts.views.bottom.as_deref()).flatten();
        declared_views()
            .into_iter()
            .filter(|d| d.location == ViewLocation::Bottom && !self.cfg.exts.views.hidden.contains(&d.id))
            .enumerate()
            .map(|(i, d)| {
                let active = shown == Some(d.id.as_str());
                let tip = crate::i18n::tf("{0}, from {1}", &[&d.title, &d.extension]);
                div()
                    .id(("ext-view-tab", i))
                    .debug_selector(move || format!("ext-view-tab-{i}"))
                    .h(px(layout::VIEW_HEADER_H - 8.))
                    .flex()
                    .items_center()
                    .px(px(theme::SPACE[3]))
                    .cursor_pointer()
                    .rounded(px(theme::radius::SM))
                    .when(active, |e| e.bg(c(p.surface_active)))
                    .text_color(c(if active { p.text } else { p.text_muted }))
                    .when(!active, |e| e.hover(|e| e.bg(c(p.surface_hover)).text_color(c(p.text))))
                    .child(d.title.clone())
                    .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone().into())).into())
                    .on_click(cx.listener(move |this, _, window, cx| this.open_ext_view(&d.id, window, cx)))
                    .into_any_element()
            })
            .collect()
    }

    /// The activity bar buttons of the sidebar views.
    pub(super) fn ext_view_activity_items(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let shown = (self.chrome.panel == Some(Panel::ExtView)).then_some(self.cfg.exts.views.sidebar.as_deref()).flatten();
        declared_views()
            .into_iter()
            .filter(|d| d.location == ViewLocation::Sidebar && !self.cfg.exts.views.hidden.contains(&d.id))
            .enumerate()
            .map(|(i, d)| {
                let selected = shown == Some(d.id.as_str());
                let tip = crate::i18n::tf("{0}, from {1}", &[&d.title, &d.extension]);
                div()
                    .id(("act-ext-view", i))
                    .debug_selector(move || format!("act-ext-view-{i}"))
                    .flex_none()
                    .w(px(layout::ACTIVITYBAR_W))
                    .h(px(layout::ACTIVITYBAR_W))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(layout::ACTIVITYBAR_W - 8.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::radius::MD))
                            .when(selected, |e| e.bg(c(p.surface_active)))
                            .when(!selected, |e| e.hover(|e| e.bg(c(p.surface_hover))))
                            .child(icon("plug", layout::ICON_LG, if selected { p.text } else { p.text_muted })),
                    )
                    .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone().into())).into())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.chrome.panel == Some(Panel::ExtView) && this.cfg.exts.views.sidebar.as_deref() == Some(d.id.as_str()) {
                            this.chrome.panel = None;
                            cx.notify();
                        } else {
                            this.open_ext_view(&d.id, window, cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// The page tab's title.
    pub(super) fn ext_view_title(&self, id: &str) -> String {
        declared_views().into_iter().find(|d| d.id == id).map_or_else(|| id.to_string(), |d| d.title)
    }

    /// After a settings reload, and when a view's `when` turns false: views whose extension is gone, or whose `when`
    /// is false, are closed.
    pub(super) fn sync_ext_views(&mut self, cx: &mut Context<Self>) {
        let hidden = &self.cfg.exts.views.hidden;
        let declared: HashSet<String> = declared_views().into_iter().map(|d| d.id).filter(|id| !hidden.contains(id)).collect();
        let views = &mut self.cfg.exts.views;
        views.hosted.retain(|h| declared.contains(&h.decl.id));
        if views.sidebar.as_ref().is_some_and(|s| !declared.contains(s)) {
            views.sidebar = None;
            if self.chrome.panel == Some(Panel::ExtView) {
                self.chrome.panel = Some(Panel::Explorer);
            }
        }
        if views.bottom.as_ref().is_some_and(|s| !declared.contains(s)) {
            views.bottom = None;
            if self.chrome.bottom == Bottom::ExtView {
                self.chrome.bottom = Bottom::Problems;
            }
        }
        if matches!(&self.tabs.page, Some(Page::ExtView(id)) if !declared.contains(id)) {
            self.close_page(cx);
        }
    }
}

impl Workbench {
    /// Whether a view's `when` holds now. A view without one is always offered; one that does not parse (the manifest
    /// check refuses those) never is. Keys are what this window shows: `editor` when a file is open in an editor tab,
    /// `editor.language` its language, `terminal` when the bottom panel shows the terminal, `file-tree`, `scm` and
    /// `search` when that view is in a side bar, `panel` when the bottom panel is open. `ext.<key>` is not set yet.
    pub(super) fn ext_view_when_holds(&self, d: &Declared, cx: &App) -> bool {
        use ext_manifest::when::Value as W;
        let Some(src) = d.when.as_deref() else { return true };
        let Ok(expr) = ext_manifest::when::parse(src) else { return false };
        let lookup = |key: &str| -> Option<W> {
            Some(match key {
                "editor" => W::Bool(self.active_editor().is_some()),
                "editor.language" => W::Str(self.active_editor().map(|e| super::providers_glue::language_id(&e.read(cx).path)).unwrap_or_default()),
                "terminal" => W::Bool(self.chrome.panel_open && self.chrome.bottom == Bottom::Terminal),
                "file-tree" => W::Bool(self.view_shown(Panel::Explorer)),
                "scm" => W::Bool(self.view_shown(Panel::Scm)),
                "search" => W::Bool(self.view_shown(Panel::Search)),
                "panel" => W::Bool(self.chrome.panel_open),
                "workspace.trusted" => W::Bool(self.all_roots(cx).iter().all(|r| self.is_trusted_root(r, cx))),
                _ => return None,
            })
        };
        expr.eval(&lookup)
    }

    /// Each frame: work out which views' `when` is false, close those that are open, and let the views on screen fetch
    /// their URL images.
    pub(super) fn sync_ext_views_frame(&mut self, cx: &mut Context<Self>) {
        let declared = declared_views();
        if declared.iter().any(|d| d.when.is_some()) || !self.cfg.exts.views.hidden.is_empty() {
            let hidden: HashSet<String> = declared.iter().filter(|d| !self.ext_view_when_holds(d, cx)).map(|d| d.id.clone()).collect();
            let close = self.cfg.exts.views.hosted.iter().any(|h| hidden.contains(&h.decl.id));
            self.cfg.exts.views.hidden = hidden;
            if close {
                self.sync_ext_views(cx);
            }
        }
        self.fetch_ext_view_images(cx);
    }

    /// The views drawn now: the side bar's, the bottom panel's and the editor area's, when each is showing.
    pub(super) fn ext_views_on_screen(&self) -> Vec<String> {
        let views = &self.cfg.exts.views;
        let mut out = Vec::new();
        if self.view_shown(Panel::ExtView) {
            out.extend(views.sidebar.clone());
        }
        if self.chrome.panel_open && self.chrome.bottom == Bottom::ExtView {
            out.extend(views.bottom.clone());
        }
        if let (Some(Page::ExtView(id)), true) = (&self.tabs.page, self.tabs.page_active) {
            out.push(id.clone());
        }
        out
    }
}

/// The item with key `item` anywhere in `tree`.
pub(super) fn find_item<'a>(tree: &'a Node, item: &Key) -> Option<&'a Node> {
    let mut stack: Vec<&Node> = tree.children.iter().collect();
    while let Some(n) = stack.pop() {
        if n.key.as_ref() == Some(item) {
            return Some(n);
        }
        stack.extend(&n.children);
    }
    None
}

#[cfg(test)]
impl Workbench {
    /// Record requests and events instead of sending them, so a test can hand the view its tree.
    pub fn ext_views_offline_for_test(&mut self) {
        self.cfg.exts.views.offline = true;
    }

    pub fn open_ext_view_for_test(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_ext_view(id, window, cx);
    }

    /// The extension answered `render` with `json`.
    pub fn ext_view_render_for_test(&mut self, id: &str, json: &str, window: &mut Window, cx: &mut Context<Self>) {
        let result = ext_ui::decode_json(json).map_err(|e| e.to_string());
        self.ext_view_rendered(id, result, window, cx);
    }

    pub fn ext_view_invalidate_for_test(&mut self, extension: &str, local: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.on_ext_view_invalidate(extension, local, window, cx);
    }

    pub fn ext_view_renders_for_test(&self) -> u64 {
        self.cfg.exts.views.renders
    }

    pub fn ext_view_events_for_test(&self) -> Vec<String> {
        self.cfg.exts.views.sent.iter().map(|(_, e)| e.clone()).collect()
    }

    pub fn ext_view_error_for_test(&self, id: &str) -> Option<String> {
        self.ext_view(id).and_then(|h| h.error.clone())
    }

    pub fn ext_view_last_ops_for_test(&self, id: &str) -> usize {
        self.ext_view(id).map_or(0, |h| h.last_ops)
    }

    /// Rows built by the last frame for each row container, by its key path joined with `/`.
    pub fn ext_view_built_rows_for_test(&self) -> HashMap<String, usize> {
        let join = |p: &KeyPath| p.iter().map(Key::as_str).collect::<Vec<_>>().join("/");
        self.cfg.exts.views.built_rows.iter().map(|(p, n)| (join(p), *n)).collect()
    }

    /// The text the host keeps for the text input at `path`.
    pub fn ext_view_text_for_test(&self, id: &str, path: &[&str]) -> Option<String> {
        let path: KeyPath = path.iter().map(|k| Key::from(*k)).collect();
        self.ext_view(id)?.state.get(&path).map(|s| s.text.clone())
    }

    /// The key path of the element that has keyboard focus in view `id`.
    pub fn ext_view_focused_for_test(&self, id: &str, window: &Window) -> Option<String> {
        let h = self.ext_view(id)?;
        h.focus.iter().find(|(_, f)| f.is_focused(window)).map(|(p, _)| p.iter().map(Key::as_str).collect::<Vec<_>>().join("/"))
    }

    pub fn ext_view_shown_for_test(&self) -> (Option<String>, Option<String>, Option<String>) {
        let page = match &self.tabs.page {
            Some(Page::ExtView(id)) if self.tabs.page_active => Some(id.clone()),
            _ => None,
        };
        let side = (self.chrome.panel == Some(Panel::ExtView)).then(|| self.cfg.exts.views.sidebar.clone()).flatten();
        let bottom = (self.chrome.panel_open && self.chrome.bottom == Bottom::ExtView).then(|| self.cfg.exts.views.bottom.clone()).flatten();
        (side, bottom, page)
    }

    /// Every `text` element's text, in order.
    pub fn ext_view_texts_for_test(&self, id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let Some(tree) = self.ext_view(id).and_then(|h| h.tree.as_ref()) else { return out };
        let mut stack = vec![tree];
        while let Some(n) = stack.pop() {
            if let Element::Text { text, .. } = &n.element {
                out.push(text.clone());
            }
            stack.extend(n.children.iter().rev());
        }
        out
    }

    /// What a screen reader gets for the views on screen: (role, label, has an action) per node.
    pub fn ext_view_access_for_test(&self, window: &Window) -> Vec<(String, String, bool)> {
        self.a11y_ext_views(window)
            .into_iter()
            .flat_map(|v| v.nodes.into_iter().map(|n| (format!("{:?}", n.role), n.label, n.act != super::a11y::ExtViewAct::None)))
            .collect()
    }

    /// The tab stops of view `id` in order, as key paths joined with `/`.
    pub fn ext_view_focus_order_for_test(&self, id: &str) -> Vec<String> {
        let Some(tree) = self.ext_view(id).and_then(|h| h.tree.as_ref()) else { return Vec::new() };
        super::ext_ui_glue::focus_order(tree).iter().map(|p| p.iter().map(Key::as_str).collect::<Vec<_>>().join("/")).collect()
    }

    pub fn ext_view_drawn_with_for_test(&self) -> u32 {
        self.cfg.exts.views.drawn_with.get()
    }

    pub fn focus_ext_view_for_test(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_ext_view(id, window, cx);
    }
}
