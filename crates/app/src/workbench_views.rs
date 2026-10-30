//! Workbench: where views and terminals live. A side view can move to the secondary side bar (on the right of the
//! editor) and back; asking for a moved view (its activity bar icon, a command) opens it there. A terminal can move
//! into a window of its own and back: the same terminal view, so the same process and input, with one owner at a time;
//! closing that window returns the terminal to the panel instead of ending it.

use super::*;
use gpui::{Bounds, WindowBounds, WindowOptions};

use crate::terminal::TerminalView;

/// A terminal shown in a window of its own.
pub(super) struct Detached {
    pub(super) term: Entity<TerminalView>,
    pub(super) window: gpui::WindowHandle<DetachedTerminal>,
}

/// The size a terminal's own window opens at.
pub(super) const DETACHED_SIZE: gpui::Size<gpui::Pixels> = gpui::size(px(720.), px(440.));

/// Where a window of its own is, to open it in the same place next time: the frame's top-left corner and the size of
/// its content. A window opens with its content at the given size (on macOS the title bar is added around it), so
/// saving the frame's size would make the window taller at every start.
pub(super) fn reopen_bounds(window: &Window) -> gpui::Bounds<gpui::Pixels> {
    gpui::Bounds::new(window.bounds().origin, window.viewport_size())
}

/// The root view of a detached terminal's window.
pub(super) struct DetachedTerminal {
    term: Entity<TerminalView>,
}

impl DetachedTerminal {
    /// The terminal it shows (accessibility).
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub(super) fn term(&self) -> Entity<TerminalView> {
        self.term.clone()
    }
}

impl Render for DetachedTerminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        div().size_full().bg(c(p.bg)).p(px(theme::SPACE[3])).child(self.term.clone())
    }
}

/// The id a view is saved under.
pub(super) fn view_id(panel: Panel) -> &'static str {
    match panel {
        Panel::Explorer => "explorer",
        Panel::Search => "search",
        Panel::Scm => "scm",
        Panel::Debug => "debug",
        Panel::Testing => "testing",
        Panel::Remote => "remote",
        Panel::Containers => "containers",
        Panel::Extensions => "extensions",
        Panel::ExtView => "extension-view",
    }
}

pub(super) fn view_of(id: &str) -> Option<Panel> {
    [Panel::Explorer, Panel::Search, Panel::Scm, Panel::Debug, Panel::Testing, Panel::Remote, Panel::Containers, Panel::Extensions, Panel::ExtView]
        .into_iter()
        .find(|p| view_id(*p) == id)
}

/// The saved arrangement: `ids` of the views in the secondary side bar (unknown or repeated ids are dropped, so a
/// damaged value restores what it can).
pub(super) fn parse_secondary(text: &str) -> Vec<Panel> {
    let mut out = Vec::new();
    for v in text.split(',').filter_map(|id| view_of(id.trim())) {
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

impl Workbench {
    /// Whether `panel` is on screen, in either side bar.
    pub(super) fn view_shown(&self, panel: Panel) -> bool {
        self.chrome.panel == Some(panel) || self.chrome.secondary == Some(panel) || self.in_view_window(panel)
    }

    /// Asking for a view (`chrome.panel = Some(view)`) that lives in the secondary side bar opens it there; the primary
    /// side bar keeps the view it showed before.
    pub(super) fn place_views(&mut self) {
        match self.chrome.panel {
            Some(view) if self.chrome.in_secondary.contains(&view) => {
                self.chrome.secondary = Some(view);
                self.chrome.panel = self.chrome.last_primary.filter(|v| !self.chrome.in_secondary.contains(v));
            }
            // A view with a window of its own is not kept as the side bar's view (`place_window_views` sends it there).
            Some(view) if !self.in_view_window(view) => self.chrome.last_primary = Some(view),
            _ => {}
        }
        if self.chrome.secondary.is_some_and(|v| !self.chrome.in_secondary.contains(&v)) {
            self.chrome.secondary = None;
        }
    }

    pub(super) fn save_views(&self) {
        if let Some(s) = &self.persist.state {
            let ids: Vec<&str> = self.chrome.in_secondary.iter().map(|v| view_id(*v)).collect();
            let _ = s.set("secondary_views", &ids.join(","));
            let _ = s.set("secondary_open", self.chrome.secondary.map(view_id).unwrap_or(""));
        }
    }

    /// "View: Move View to Secondary Side Bar": the view of the primary side bar moves to the right of the editor.
    pub(super) fn move_view_to_secondary(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.chrome.panel else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a view in the side bar first.").into()));
            return cx.notify();
        };
        if !self.chrome.in_secondary.contains(&view) {
            self.chrome.in_secondary.push(view);
        }
        self.chrome.panel = None;
        self.chrome.secondary = Some(view);
        self.chrome.last_primary = None;
        self.save_views();
        cx.notify();
    }

    /// "View: Move View to Primary Side Bar": the view of the secondary side bar goes back to the left.
    pub(super) fn move_view_to_primary(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.chrome.secondary else {
            self.notice = Some(Notice::Info(crate::i18n::t("The secondary side bar shows no view.").into()));
            return cx.notify();
        };
        self.chrome.in_secondary.retain(|v| *v != view);
        self.chrome.secondary = None;
        self.chrome.panel = Some(view);
        self.save_views();
        cx.notify();
    }

    /// "View: Toggle Secondary Side Bar".
    pub(super) fn toggle_secondary(&mut self, cx: &mut Context<Self>) {
        self.chrome.secondary = match self.chrome.secondary {
            Some(_) => None,
            None => self.chrome.secondary_last.or_else(|| self.chrome.in_secondary.first().copied()),
        };
        if self.chrome.secondary.is_none() && self.chrome.in_secondary.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("No view is in the secondary side bar. Use View: Move View to Secondary Side Bar.").into()));
        }
        if let Some(v) = self.chrome.secondary {
            self.chrome.secondary_last = Some(v);
        }
        self.save_views();
        cx.notify();
    }

    /// "Terminal: Move Terminal into New Window": the active terminal leaves the panel for a window of its own.
    pub(super) fn detach_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(term) = self.terminals.get(self.active_terminal).cloned() else {
            self.notice = Some(Notice::Info(crate::i18n::t("There is no terminal to move.").into()));
            return cx.notify();
        };
        let at = window.bounds();
        let bounds = Bounds::new(at.origin + gpui::point(px(40.), px(40.)), DETACHED_SIZE);
        match self.open_detached(term.clone(), bounds, cx) {
            Ok(handle) => {
                // One owner: the panel lets go of the terminal only once its window exists.
                self.remove_terminal_from_panel(&term, window);
                self.chrome.detached.push(Detached { term, window: handle });
                self.persist_session(cx);
            }
            Err(e) => self.notice = Some(Notice::Error(crate::i18n::tf("The terminal could not move to a new window: {}", &[&e]).into())),
        }
        cx.notify();
    }

    /// A window of its own for `term` at `bounds`; closing it returns the terminal to this workbench.
    pub(super) fn open_detached(
        &mut self,
        term: Entity<TerminalView>,
        bounds: Bounds<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> gpui::Result<gpui::WindowHandle<DetachedTerminal>> {
        let title = term.read(cx).title.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions { title: Some(title), appears_transparent: false, traffic_light_position: None }),
            // Windows: the UI Automation adapter must be attached before the window is first shown.
            show: !cfg!(windows),
            ..Default::default()
        };
        let home = cx.entity().downgrade();
        let t = term;
        let opened = cx.open_window(options, move |window, cx| {
            let view = cx.new(|_| DetachedTerminal { term: t.clone() });
            let focus = t.read(cx).focus_handle(cx);
            window.focus(&focus);
            // Closing the window returns the terminal to its workbench (it is not ended).
            let back = t.clone();
            window.on_window_should_close(cx, move |_, cx| {
                if let Some(wb) = home.upgrade() {
                    wb.update(cx, |wb, cx| wb.terminal_returned(&back, cx));
                }
                true
            });
            view
        });
        if let Ok(handle) = &opened {
            #[cfg(any(target_os = "macos", windows))]
            a11y::platform::attach_with((*handle).into(), a11y::windows_access::TERMINAL_WINDOW, cx);
            #[cfg(windows)]
            handle.update(cx, |_, window, _| window.activate_window()).ok();
        }
        opened
    }

    /// The panel's list without `term`, without ending it.
    pub(super) fn remove_terminal_from_panel(&mut self, term: &Entity<TerminalView>, window: &mut Window) {
        let Some(ix) = self.terminals.iter().position(|t| t == term) else { return };
        self.terminals.remove(ix);
        if self.term_split.as_ref().is_some_and(|(l, r)| l == term || r == term) {
            self.term_split = None;
        }
        if ix < self.active_terminal {
            self.active_terminal -= 1;
        }
        if self.terminals.is_empty() {
            self.chrome.panel_open = false;
            window.focus(&self.focus_handle);
        } else {
            self.active_terminal = self.active_terminal.min(self.terminals.len() - 1);
        }
    }

    /// A detached terminal's window closed: the terminal is back in the panel, still running.
    pub(super) fn terminal_returned(&mut self, term: &Entity<TerminalView>, cx: &mut Context<Self>) {
        let before = self.chrome.detached.len();
        self.chrome.detached.retain(|d| &d.term != term);
        if self.chrome.detached.len() == before || self.terminals.contains(term) {
            return;
        }
        self.terminals.push(term.clone());
        self.active_terminal = self.terminals.len() - 1;
        self.chrome.bottom = Bottom::Terminal;
        self.chrome.panel_open = true;
        self.persist_session(cx);
        cx.notify();
    }

    /// "Terminal: Move Terminals Back to Panel": every detached terminal returns and its window closes.
    pub(super) fn return_detached_terminals(&mut self, cx: &mut Context<Self>) {
        for d in std::mem::take(&mut self.chrome.detached) {
            if !self.terminals.contains(&d.term) {
                self.terminals.push(d.term.clone());
            }
            d.window.update(cx, |_, window, _| window.remove_window()).ok();
        }
        if let Some(last) = self.terminals.len().checked_sub(1) {
            self.active_terminal = last;
            self.chrome.bottom = Bottom::Terminal;
            self.chrome.panel_open = true;
        }
        self.persist_session(cx);
        cx.notify();
    }

    /// The connection changed or the window closes: detached terminals belong to it and end with it.
    pub(super) fn close_detached_terminals(&mut self, cx: &mut App) {
        for d in std::mem::take(&mut self.chrome.detached) {
            d.term.read(cx).kill();
            d.window.update(cx, |_, window, _| window.remove_window()).ok();
        }
    }
}

#[cfg(test)]
#[path = "workbench_views_tests.rs"]
mod tests;
