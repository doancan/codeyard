//! Workbench: side views in windows of their own. "View: Move View into New Window" draws the open side view
//! (Explorer, Search, Source Control, Run and Debug, Testing, Remote Explorer, Extensions, or a code extension's side
//! bar view) in an OS window of its own.
//! It is the same view with the same state, drawn there instead of in a side bar: the workbench keeps owning it. The
//! window carries the workbench's actions, so commands and shortcuts work there, and its menus and pickers open there
//! while it is in use. Asking for the view (its activity bar icon, its command) brings that window to the front.
//! "View: Move View Back to Side Bar" or closing the window puts it back where it was (primary or secondary side
//! bar). The windows belong to the folder like editor windows: they are saved with its session, restored with it, and
//! close (their views going back) when the folder or the connection changes or the main window closes. An extension's
//! view keeps its tree and input state (it is the same hosted view); its window closes when the extension goes away
//! and gives the view back to the side bar when the extension's host stops after a crash.

use super::*;
use gpui::{Bounds, Pixels, WeakEntity, WindowBounds, WindowOptions};

gpui::actions!(workbench, [MoveViewIntoNewWindow, MoveViewBackToSideBar]);

/// The size a view's own window opens at.
const SIZE: gpui::Size<Pixels> = gpui::size(px(360.), px(640.));
/// State key of the saved windows, per workspace.
const SAVED_KEY: &str = "view_windows";
const VERSION: u8 = 1;
const MAX_TEXT: usize = 16 * 1024;
/// At most this many saved windows, and the longest extension view id kept.
const MAX_WINDOWS: usize = 32;
const MAX_EXT_ID: usize = 512;
/// Views that can have a window of their own.
const WINDOWABLE: [Panel; 8] = [Panel::Explorer, Panel::Search, Panel::Scm, Panel::Debug, Panel::Testing, Panel::Remote, Panel::Extensions, Panel::ExtView];

/// Which view a window shows: a built-in one, or (`Panel::ExtView`) the extension view `ext`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ViewRef {
    pub(super) panel: Panel,
    pub(super) ext: Option<String>,
}

/// A side view drawn in a window of its own, the side bar it came from and where its window was last.
pub(super) struct ViewWindowEntry {
    pub(super) view: ViewRef,
    secondary: bool,
    pub(super) window: gpui::WindowHandle<ViewWindow>,
    bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
}

/// What a view's window shows, drawn as a cached view: it is drawn again only when what it reads changes, not with
/// every change of the workbench. Source Control and Search are views of their own and read nothing else; the other
/// views are drawn from the workbench's state, so they redraw with it, as they do in a side bar.
pub(super) struct ViewBody {
    view: ViewRef,
    home: WeakEntity<Workbench>,
    scm: Entity<crate::scm::ScmPanel>,
    search: Entity<SearchPanel>,
    /// Frames drawn (tests: an unrelated change must not draw it again).
    #[cfg(test)]
    pub(super) renders: usize,
}

impl Render for ViewBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        let p = *palette(cx);
        let body = match self.view.panel {
            Panel::Scm => self.scm.clone().into_any_element(),
            Panel::Search => div()
                .flex()
                .flex_col()
                .size_full()
                .child(view_explorer_glue::view_header(crate::i18n::upper(crate::i18n::t("Search")), &p))
                .child(div().flex_1().min_h_0().child(self.search.clone()))
                .into_any_element(),
            panel => match (self.home.upgrade(), &self.view.ext) {
                // The same hosted view as in the side bar: its tree, input state and the extension's updates.
                (Some(home), Some(id)) => home.update(cx, |wb, cx| wb.ext_view_frame(id, &p, cx)),
                (Some(home), None) => home.update(cx, |wb, cx| wb.side_view(panel, &p, cx)),
                (None, _) => div().into_any_element(),
            },
        };
        div().size_full().flex().flex_col().child(body)
    }
}

/// The root view of a side view's own window.
pub(super) struct ViewWindow {
    view: ViewRef,
    body: Entity<ViewBody>,
    home: WeakEntity<Workbench>,
    focus: FocusHandle,
    bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    /// Set once the window exists: its first frame is drawn while the workbench is still opening it.
    ready: bool,
    /// What had the keyboard at the last frame.
    last_focus: Option<FocusHandle>,
    _subs: Vec<Subscription>,
}

impl ViewWindow {
    /// This window (`me`) is in use: its menus and pickers open here.
    fn in_use(&self, me: gpui::AnyWindowHandle, cx: &mut App) {
        self.home.update(cx, |wb, cx| wb.use_view_window(me, cx)).ok();
    }

    /// The view and the workbench (accessibility).
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub(super) fn parts(&self) -> (ViewRef, WeakEntity<Workbench>) {
        (self.view.clone(), self.home.clone())
    }
}

impl Render for ViewWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.bounds.set(Some(window.bounds()));
        // Keys always reach the workbench's actions here, even when what has the keyboard is not drawn here (the view's
        // own target, as the Explorer's tree with no folder open). A focus set since the last frame (a menu that just
        // opened) is left alone: it is drawn in this frame.
        let focused = window.focused(cx);
        if self.ready && window.context_stack().is_empty() && focused.as_ref() == self.last_focus.as_ref() {
            window.focus(&self.focus);
        }
        self.last_focus = focused;
        let p = *palette(cx);
        let me = window.window_handle();
        let root = div().id("view-window");
        // The actions and the menu or picker layer come from the workbench (cheap, drawn with it); the view itself is
        // the cached body.
        let (root, body, layers) = match self.home.upgrade().filter(|_| self.ready) {
            Some(home) => home.update(cx, |wb, cx| {
                let layers: Vec<AnyElement> = if wb.chrome.view_host == Some(me) {
                    wb.menu_layer(0., cx).into_iter().chain(wb.picker_layer(theme::SPACE[4], cx)).collect()
                } else {
                    Vec::new()
                };
                let body = gpui::AnyView::from(self.body.clone()).cached(gpui::StyleRefinement::default().size_full());
                (wb.with_actions(root, cx), Some(body), layers)
            }),
            None => (root, None, Vec::new()),
        };
        root.key_context("ViewWindow")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(c(p.surface))
            .text_color(c(p.text))
            .font_family(cx.global::<Settings>().ui_font.clone().map_or(SharedString::from(ui::UI_FONT), SharedString::from))
            .text_size(px(text::SM.0))
            .line_height(px(text::SM.1))
            // Input here makes this the window menus and pickers open in.
            .capture_key_down(cx.listener(|this, _, window, cx| this.in_use(window.window_handle(), cx)))
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| this.in_use(window.window_handle(), cx)))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    // Before the workbench's own: ⌘W and "Move View Back" put this view back in its side bar.
                    .on_action(cx.listener(|this, _: &CloseEditor, window, cx| this.go_back(window.window_handle(), cx)))
                    .on_action(cx.listener(|this, _: &MoveViewBackToSideBar, window, cx| this.go_back(window.window_handle(), cx)))
                    .children(body),
            )
            .children(layers)
    }
}

impl ViewWindow {
    fn go_back(&self, me: gpui::AnyWindowHandle, cx: &mut App) {
        self.home.update(cx, |wb, cx| wb.view_window_returned(me, true, cx)).ok();
    }
}

/// The saved windows: which view each showed, whether it came from the secondary side bar, and its bounds.
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedViewWindows {
    version: u8,
    windows: Vec<SavedViewWindow>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedViewWindow {
    view: String,
    /// The extension view's id, for `extension-view`.
    #[serde(default)]
    ext: Option<String>,
    #[serde(default)]
    secondary: bool,
    #[serde(default)]
    at: Option<[f32; 4]>,
}

impl SavedViewWindows {
    /// The saved windows, if this version wrote them and they are within bounds; anything else is ignored whole.
    /// Unknown or repeated views are dropped.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        if text.len() > MAX_TEXT {
            return None;
        }
        let mut saved: Self = serde_json::from_str(text).ok()?;
        if saved.version != VERSION || saved.windows.len() > MAX_WINDOWS {
            return None;
        }
        let mut seen = Vec::new();
        saved.windows.retain(|w| match w.view_ref() {
            Some(v) if !seen.contains(&v) => {
                seen.push(v);
                true
            }
            _ => false,
        });
        Some(saved)
    }
}

impl SavedViewWindow {
    /// The view it names: a built-in view without an id, or an extension view with a sane one.
    fn view_ref(&self) -> Option<ViewRef> {
        let panel = views_glue::view_of(&self.view).filter(|v| WINDOWABLE.contains(v))?;
        let ext = self.ext.clone().filter(|id| !id.is_empty() && id.len() <= MAX_EXT_ID && !id.contains('\0'));
        ((panel == Panel::ExtView) == ext.is_some()).then_some(ViewRef { panel, ext })
    }
}

impl Workbench {
    /// The view a side bar would show for `panel` (for `Panel::ExtView`, the extension view of the side bar).
    fn view_ref(&self, panel: Panel) -> ViewRef {
        let ext = (panel == Panel::ExtView).then(|| self.cfg.exts.views.sidebar.clone()).flatten();
        ViewRef { panel, ext }
    }

    /// Whether the view a side bar would show for `panel` is drawn in a window of its own.
    pub(super) fn in_view_window(&self, panel: Panel) -> bool {
        let view = self.view_ref(panel);
        self.chrome.view_windows.iter().any(|w| w.view == view)
    }

    /// The extension views in windows of their own.
    #[cfg_attr(not(any(test, target_os = "macos", windows)), allow(dead_code))]
    pub(super) fn ext_views_in_windows(&self) -> Vec<String> {
        self.chrome.view_windows.iter().filter_map(|w| w.view.ext.clone()).collect()
    }

    /// The window of the view in use, if one is.
    pub(super) fn view_host_window(&self) -> Option<gpui::AnyWindowHandle> {
        let host = self.chrome.view_host?;
        self.chrome.view_windows.iter().any(|w| gpui::AnyWindowHandle::from(w.window) == host).then_some(host)
    }

    /// The view window `handle` is in use: menus and pickers open there.
    pub(super) fn use_view_window(&mut self, handle: gpui::AnyWindowHandle, cx: &mut Context<Self>) {
        let known = self.chrome.view_windows.iter().any(|w| gpui::AnyWindowHandle::from(w.window) == handle);
        if known && self.chrome.view_host != Some(handle) {
            self.chrome.editor_host = None;
            self.chrome.view_host = Some(handle);
            cx.notify();
        }
    }

    /// A menu or picker asked for in another window while a view window is in use opens in that view window (a
    /// context menu of its tree, a picker of its command): `f` runs there instead, and `true` is returned.
    pub(super) fn reroute_to_view_window(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> bool {
        let Some(handle) = self.view_host_window().filter(|h| *h != window.window_handle()) else { return false };
        let wb = cx.entity();
        cx.defer(move |cx| {
            handle.update(cx, |_, window, cx| wb.update(cx, |wb, cx| f(wb, window, cx))).ok();
        });
        true
    }

    /// The keyboard target of a view, when it has one.
    fn view_focus(&self, panel: Panel, cx: &App) -> Option<FocusHandle> {
        match panel {
            Panel::Explorer => Some(self.roots.primary.focus_handle(cx)),
            Panel::Scm => Some(self.scm.focus_handle(cx)),
            Panel::Search => Some(self.search.focus_handle(cx)),
            _ => None,
        }
    }

    /// "View: Move View into New Window": the open side view (primary side bar first) gets a window of its own.
    pub(super) fn view_into_new_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let windowable = |v: &Panel| WINDOWABLE.contains(v) && (*v != Panel::ExtView || self.cfg.exts.views.sidebar.is_some());
        let (panel, secondary) = match (self.chrome.panel, self.chrome.secondary) {
            (Some(v), _) if windowable(&v) => (v, false),
            (_, Some(v)) if windowable(&v) => (v, true),
            _ => {
                self.notice = Some(Notice::Info(crate::i18n::t("Open a view in a side bar first.").into()));
                return cx.notify();
            }
        };
        let at = window.bounds();
        let bounds = Bounds::new(at.origin + gpui::point(px(40.), px(40.)), SIZE);
        if let Err(e) = self.show_in_view_window(self.view_ref(panel), secondary, bounds, cx) {
            self.notice = Some(Notice::Error(e.into()));
        }
        // The keyboard stays with the main window's editor (the view's own target is in the new window now).
        if let Some(ed) = self.tab_editor() {
            window.focus(&ed.focus_handle(cx));
        } else {
            window.focus(&self.focus_handle);
        }
        self.persist_view_windows();
        cx.notify();
    }

    /// Takes `view` out of its side bar and opens its window at `bounds`. The side bar lets go of the view only once
    /// the window exists.
    fn show_in_view_window(&mut self, view: ViewRef, secondary: bool, bounds: Bounds<Pixels>, cx: &mut Context<Self>) -> Result<(), String> {
        if self.chrome.view_windows.iter().any(|w| w.view == view) {
            return Ok(());
        }
        let panel = view.panel;
        let title = self.view_window_title(&view);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions { title: Some(title.clone().into()), appears_transparent: false, traffic_light_position: None }),
            window_min_size: Some(gpui::size(px(200.), px(160.))),
            // Windows: the UI Automation adapter must be attached before the window is first shown.
            show: !cfg!(windows),
            ..Default::default()
        };
        let home = cx.entity().downgrade();
        let shared = std::rc::Rc::new(std::cell::Cell::new(Some(bounds)));
        let cell = shared.clone();
        let target = self.view_focus(panel, cx);
        let (scm, search) = (self.scm.clone(), self.search.clone());
        let ext = view.ext.clone();
        let opened = cx.open_window(options, move |window, cx| {
            // Closing the window puts the view back in its side bar.
            let back = home.clone();
            window.on_window_should_close(cx, move |window, cx| {
                let me = window.window_handle();
                back.update(cx, |wb, cx| wb.view_window_returned(me, false, cx)).ok();
                true
            });
            window.set_window_title(&title);
            let body = cx.new(|_| ViewBody {
                view: ViewRef { panel, ext: ext.clone() },
                home: home.clone(),
                scm,
                search,
                #[cfg(test)]
                renders: 0,
            });
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(target.as_ref().unwrap_or(&focus));
                let subs = vec![
                    cx.observe_window_bounds(window, |this: &mut ViewWindow, window, _| this.bounds.set(Some(window.bounds()))),
                    cx.observe_window_activation(window, |this: &mut ViewWindow, window, cx| {
                        if window.is_window_active() {
                            this.in_use(window.window_handle(), cx);
                        }
                    }),
                ];
                // Once the view is drawn here, the keyboard goes into it.
                let target = target.clone();
                cx.defer_in(window, move |this: &mut ViewWindow, window, cx| {
                    this.ready = true;
                    window.focus(target.as_ref().unwrap_or(&this.focus));
                    cx.notify();
                });
                ViewWindow { view: ViewRef { panel, ext }, body, home, focus, bounds: cell, ready: false, last_focus: None, _subs: subs }
            })
        });
        match opened {
            Ok(handle) => {
                #[cfg(any(target_os = "macos", windows))]
                a11y::platform::attach_with(handle.into(), a11y::windows_access::VIEW_WINDOW, cx);
                #[cfg(windows)]
                handle.update(cx, |_, window, _| window.activate_window()).ok();
                // The side bars let go of it. An extension view leaves the side bar's extension slot; other extension
                // views still open there.
                let shown_there = self.view_ref(panel) == view;
                if shown_there {
                    if self.chrome.panel == Some(panel) {
                        self.chrome.panel = None;
                    }
                    if self.chrome.secondary == Some(panel) {
                        self.chrome.secondary = None;
                    }
                }
                match &view.ext {
                    Some(id) if self.cfg.exts.views.sidebar.as_ref() == Some(id) => self.cfg.exts.views.sidebar = None,
                    Some(_) => {}
                    None => self.chrome.in_secondary.retain(|v| *v != panel),
                }
                self.chrome.view_windows.push(ViewWindowEntry { view, secondary, window: handle, bounds: shared });
                self.save_views();
                self.use_view_window(handle.into(), cx);
                Ok(())
            }
            Err(e) => Err(crate::i18n::tf("The view could not move to a new window: {}", &[&e.to_string()])),
        }
    }

    /// The view's window closed, or "View: Move View Back to Side Bar": it is back in the side bar it came from, and
    /// shown there. `close`: the window is still open and closes now.
    pub(super) fn view_window_returned(&mut self, handle: gpui::AnyWindowHandle, close: bool, cx: &mut Context<Self>) {
        let Some(ix) = self.chrome.view_windows.iter().position(|w| gpui::AnyWindowHandle::from(w.window) == handle) else { return };
        let entry = self.chrome.view_windows.remove(ix);
        if self.chrome.view_host == Some(handle) {
            self.chrome.view_host = None;
        }
        let panel = entry.view.panel;
        if let Some(id) = entry.view.ext.clone() {
            self.cfg.exts.views.sidebar = Some(id);
        }
        if close {
            let handle = entry.window;
            cx.defer(move |cx| {
                handle.update(cx, |_, window, _| window.remove_window()).ok();
            });
        }
        if entry.secondary {
            if !self.chrome.in_secondary.contains(&panel) {
                self.chrome.in_secondary.push(panel);
            }
            self.chrome.secondary = Some(panel);
            self.chrome.secondary_last = Some(panel);
        } else {
            self.chrome.panel = Some(panel);
        }
        self.save_views();
        self.persist_view_windows();
        cx.notify();
    }

    /// "View: Move View Back to Side Bar" in the main window: every view window's view goes back.
    pub(super) fn return_view_windows(&mut self, cx: &mut Context<Self>) {
        if self.chrome.view_windows.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("No view is in a window of its own.").into()));
            return cx.notify();
        }
        let handles: Vec<gpui::AnyWindowHandle> = self.chrome.view_windows.iter().map(|w| w.window.into()).collect();
        for handle in handles {
            self.view_window_returned(handle, true, cx);
        }
    }

    /// The window of the view `panel` (for extension views, the side bar's), if it has one.
    pub(super) fn view_window_of(&self, panel: Panel) -> Option<gpui::AnyWindowHandle> {
        let view = self.view_ref(panel);
        self.chrome.view_windows.iter().find(|w| w.view == view).map(|w| w.window.into())
    }

    /// A window's title: the view's name, and for an extension view its title and publisher.
    pub(super) fn view_window_title(&self, view: &ViewRef) -> String {
        match &view.ext {
            Some(id) => match ext_views_glue::declared_views().into_iter().find(|d| d.id == *id) {
                Some(d) => crate::i18n::tf("{0}, from {1}", &[&d.title, &d.extension]),
                None => id.clone(),
            },
            None => docks_glue::view_name(view.panel),
        }
    }

    /// Extension views whose extension is gone (uninstalled, turned off): their windows close; there is nothing to
    /// give back.
    pub(super) fn close_gone_ext_view_windows(&mut self, declared: &std::collections::HashSet<String>, cx: &mut Context<Self>) {
        let gone: Vec<gpui::AnyWindowHandle> =
            self.chrome.view_windows.iter().filter(|w| w.view.ext.as_ref().is_some_and(|id| !declared.contains(id))).map(|w| w.window.into()).collect();
        for handle in gone {
            self.chrome.view_windows.retain(|w| gpui::AnyWindowHandle::from(w.window) != handle);
            if self.chrome.view_host == Some(handle) {
                self.chrome.view_host = None;
            }
            cx.defer(move |cx| {
                handle.update(cx, |_, window, _| window.remove_window()).ok();
            });
        }
        self.persist_view_windows();
    }

    /// An extension's host stopped after a crash: its views' windows give them back to the side bar, which shows
    /// what happened and starts the extension again when the view is drawn.
    pub(super) fn return_ext_view_windows(&mut self, extension: &str, cx: &mut Context<Self>) {
        let declared = ext_views_glue::declared_views();
        let of_it: Vec<gpui::AnyWindowHandle> = self
            .chrome
            .view_windows
            .iter()
            .filter(|w| w.view.ext.as_ref().is_some_and(|id| declared.iter().any(|d| d.id == *id && d.extension == extension)))
            .map(|w| w.window.into())
            .collect();
        for handle in of_it {
            self.view_window_returned(handle, true, cx);
        }
    }

    /// Asked for (activity bar icon, command), a view that has a window of its own is not opened in a side bar: its
    /// window comes to the front with the keyboard in the view. Called while drawing, after the side bars are placed.
    pub(super) fn place_window_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.chrome.panel.filter(|p| self.in_view_window(*p)) else { return };
        let Some(handle) = self.view_window_of(panel) else { return };
        if panel == Panel::ExtView {
            // The side bar's extension slot was pointed at it; the view stays in its window.
            self.cfg.exts.views.sidebar = None;
        }
        self.chrome.panel = self.chrome.last_primary.filter(|v| *v != panel && !self.in_view_window(*v) && !self.chrome.in_secondary.contains(v));
        // The command may have given the keyboard to the view in this window, where it is not drawn.
        if self.view_focus(panel, cx).is_some_and(|f| f.is_focused(window)) {
            match self.tab_editor() {
                Some(ed) => window.focus(&ed.focus_handle(cx)),
                None => window.focus(&self.focus_handle),
            }
        }
        let Some(entry) = self.chrome.view_windows.iter().find(|w| gpui::AnyWindowHandle::from(w.window) == handle) else { return };
        let (root, target, wb) = (entry.window, self.view_focus(panel, cx), cx.entity());
        cx.defer(move |cx| {
            root.update(cx, |root, window, cx| {
                window.activate_window();
                window.focus(target.as_ref().unwrap_or(&root.focus));
                wb.update(cx, |wb, cx| wb.use_view_window(handle, cx));
            })
            .ok();
        });
    }

    /// The folder or the connection changes, or the main window closes: the windows close and their views go back to
    /// their side bars (the arrangement was saved with the session).
    pub(super) fn close_view_windows(&mut self, cx: &mut App) {
        self.chrome.view_host = None;
        for entry in std::mem::take(&mut self.chrome.view_windows) {
            if entry.secondary && entry.view.ext.is_none() && !self.chrome.in_secondary.contains(&entry.view.panel) {
                self.chrome.in_secondary.push(entry.view.panel);
            }
            let handle = entry.window;
            cx.defer(move |cx| {
                handle.update(cx, |_, window, _| window.remove_window()).ok();
            });
        }
    }

    /// Save which views have windows and where those are (with the session).
    pub(super) fn persist_view_windows(&self) {
        let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) else { return };
        let windows: Vec<SavedViewWindow> = self
            .chrome
            .view_windows
            .iter()
            .map(|w| SavedViewWindow {
                view: views_glue::view_id(w.view.panel).to_string(),
                ext: w.view.ext.clone(),
                secondary: w.secondary,
                at: w.bounds.get().map(|b| [b.origin.x, b.origin.y, b.size.width, b.size.height].map(f32::from)),
            })
            .collect();
        if let Ok(json) = serde_json::to_string(&SavedViewWindows { version: VERSION, windows }) {
            let _ = s.set(&format!("{SAVED_KEY}:{ws}"), &json);
        }
    }

    /// The windows saved for workspace `ws`; `None` when there are none or the value is damaged.
    pub(super) fn saved_view_windows(&self, ws: i64) -> Option<SavedViewWindows> {
        let text = self.persist.state.as_ref()?.get(&format!("{SAVED_KEY}:{ws}"))?;
        SavedViewWindows::parse(&text)
    }

    /// The saved windows open again, each on the display that shows most of it (or the first one, made to fit). Only
    /// when no view window is open.
    pub(super) fn restore_view_windows(&mut self, saved: Option<SavedViewWindows>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(saved) = saved.filter(|_| self.chrome.view_windows.is_empty()) else { return };
        let mut displays: Vec<Bounds<Pixels>> = Vec::new();
        for d in cx.primary_display().into_iter().chain(cx.displays()).map(|d| d.bounds()) {
            if !displays.contains(&d) {
                displays.push(d);
            }
        }
        let declared = ext_views_glue::declared_views();
        for w in saved.windows {
            let Some(view) = w.view_ref() else { continue };
            // An extension view comes back only while its extension is there and declares it for a side bar.
            if let Some(id) = &view.ext {
                let Some(d) = declared.iter().find(|d| d.id == *id && d.location == ext_manifest::ViewLocation::Sidebar) else { continue };
                self.ensure_ext_view(d.clone(), window, cx);
            }
            let bounds = term_tabs_glue::restore::place_window(w.at, &displays)
                .unwrap_or_else(|| Bounds::new(window.bounds().origin + gpui::point(px(40.), px(40.)), SIZE));
            let _ = self.show_in_view_window(view, w.secondary, bounds, cx);
        }
        self.chrome.view_host = None;
        cx.notify();
    }
}

#[cfg(test)]
#[path = "workbench_view_windows_tests.rs"]
mod tests;
