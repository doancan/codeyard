//! Workbench: terminals in the editor area. A terminal of the panel can move into the focused editor group as a tab:
//! the same terminal view, so the same process and input, with one owner at a time. Closing that tab returns the
//! terminal to the panel still running (like closing a detached terminal's window); a connection change or the window
//! closing ends it like a panel terminal.

use super::*;

use crate::terminal::TerminalView;

#[path = "workbench_term_restore.rs"]
pub(super) mod restore;

/// A terminal tab being dragged to another editor group.
#[derive(Clone)]
pub(super) struct DraggedTerminalTab {
    pub(super) term: Entity<TerminalView>,
    /// The group it was in when the drag started.
    pub(super) from: GroupKey,
    title: SharedString,
}

impl Render for DraggedTerminalTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        div()
            .px(px(theme::SPACE[4]))
            .py(px(theme::SPACE[1]))
            .rounded(px(theme::radius::SM))
            .bg(c(p.bg))
            .border_1()
            .border_color(c(p.accent))
            .text_size(px(text::SM.0))
            .text_color(c(p.text))
            .child(self.title.clone())
    }
}

impl Workbench {
    /// A terminal tab dropped on the tab bar of group `to`: it moves there (the same terminal, the same process) and
    /// comes to the front. A drop from a group that no longer exists or no longer holds it changes nothing.
    pub(super) fn drop_terminal_tab(&mut self, term: Entity<TerminalView>, from: GroupKey, to: GroupKey, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.contains_key(from) || !self.tabs.contains_key(to) || !self.tabs.group(from.id).is_some_and(|g| g.terminals.contains(&term)) {
            return;
        }
        if from != to {
            let source = self.tabs.group_mut(from.id).expect("validated group");
            source.terminals.retain(|t| *t != term);
            if matches!(&source.page, Some(Page::Terminal(t)) if *t == term) {
                source.page = None;
                source.page_active = false;
            }
            self.tabs.group_mut(to.id).expect("validated group").terminals.push(term.clone());
            self.tabs.remove_empty(from);
        }
        if self.tabs.focus(to) {
            self.sync_slots(cx);
            self.show_editor_terminal(term, window, cx);
        }
        self.persist_session(cx);
    }

    /// The terminal in front of the focused group, if any.
    pub(super) fn front_editor_terminal(&self) -> Option<Entity<TerminalView>> {
        match &self.tabs.page {
            Some(Page::Terminal(term)) if self.tabs.page_active => Some(term.clone()),
            _ => None,
        }
    }

    /// Whether some editor group holds `term`.
    pub(super) fn in_editor_area(&self, term: &Entity<TerminalView>) -> bool {
        self.tabs.terminals.contains(term) || self.tabs.groups.values().any(|g| g.terminals.contains(term))
    }

    /// How many terminals the editor groups hold.
    pub(super) fn editor_terminal_count(&self) -> usize {
        self.tabs.terminals.len() + self.tabs.groups.values().map(|g| g.terminals.len()).sum::<usize>()
    }

    /// "Terminal: Move Terminal into Editor Area": the active terminal of the panel becomes a tab of the focused group.
    pub(super) fn move_terminal_into_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(term) = self.terminals.get(self.active_terminal).cloned() else {
            self.notice = Some(Notice::Info(crate::i18n::t("There is no terminal to move.").into()));
            return cx.notify();
        };
        // One owner: the group holds the terminal before the panel lets go of it.
        if !self.in_editor_area(&term) {
            self.tabs.terminals.push(term.clone());
        }
        self.remove_terminal_from_panel(&term, window);
        self.show_editor_terminal(term, window, cx);
        self.persist_session(cx);
    }

    /// Bring a terminal tab of the focused group to the front, with the keyboard in it.
    pub(super) fn show_editor_terminal(&mut self, term: Entity<TerminalView>, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel_max = false;
        window.focus(&term.focus_handle(cx));
        self.tabs.page = Some(Page::Terminal(term));
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        self.refresh_active_flags(cx);
        cx.notify();
    }

    /// "Terminal: Move Terminal Back to Panel": the terminal in front of the focused group returns to the panel.
    pub(super) fn move_terminal_back_to_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(term) = self.front_editor_terminal() else {
            self.notice = Some(Notice::Info(crate::i18n::t("No terminal is in front in the editor area.").into()));
            return cx.notify();
        };
        self.return_editor_terminal(&term, window, cx);
    }

    /// Take `term` out of every group (and out of the front page); whether a group held it.
    fn take_editor_terminal(&mut self, term: &Entity<TerminalView>) -> bool {
        let mut found = false;
        for g in std::iter::once(&mut self.tabs.current).chain(self.tabs.groups.values_mut()) {
            let before = g.terminals.len();
            g.terminals.retain(|t| t != term);
            found |= g.terminals.len() != before;
            if matches!(&g.page, Some(Page::Terminal(t)) if t == term) {
                g.page = None;
                g.page_active = false;
            }
        }
        found
    }

    /// A terminal tab closes or moves back: the terminal returns to the panel, still running. Closing a tab never ends
    /// a live process.
    pub(super) fn return_editor_terminal(&mut self, term: &Entity<TerminalView>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.in_editor_area(term) {
            return;
        }
        // One owner: the panel holds the terminal before the group lets go of it.
        if !self.terminals.contains(term) {
            self.terminals.push(term.clone());
        }
        self.take_editor_terminal(term);
        self.active_terminal = self.terminals.iter().position(|t| t == term).unwrap_or(0);
        self.chrome.panel_open = true;
        self.refresh_active_flags(cx);
        self.focus_terminal(window, cx);
        self.persist_session(cx);
    }

    /// The editor groups are about to be replaced (another folder opens, the folder closes): their terminals return to
    /// the panel, still running, as panel terminals do not end with a folder.
    pub(super) fn return_all_editor_terminals(&mut self) {
        let mut back = Vec::new();
        for g in std::iter::once(&mut self.tabs.current).chain(self.tabs.groups.values_mut()) {
            back.append(&mut g.terminals);
            if matches!(g.page, Some(Page::Terminal(_))) {
                g.page = None;
                g.page_active = false;
            }
        }
        if back.is_empty() {
            return;
        }
        for t in back {
            if !self.terminals.contains(&t) {
                self.terminals.push(t);
            }
        }
        self.active_terminal = self.terminals.len() - 1;
        self.chrome.bottom = Bottom::Terminal;
        self.chrome.panel_open = true;
    }

    /// The shell of a terminal tab exited and a key was pressed: the tab closes (nothing runs any more).
    pub(super) fn close_editor_terminal(&mut self, term: &Entity<TerminalView>, cx: &mut Context<Self>) {
        if self.take_editor_terminal(term) {
            term.read(cx).kill();
            cx.notify();
        }
    }

    /// The connection changed or the window closes: terminals in the editor area end like panel terminals.
    pub(super) fn close_editor_terminals(&mut self, cx: &mut App) {
        for g in std::iter::once(&mut self.tabs.current).chain(self.tabs.groups.values_mut()) {
            for t in g.terminals.drain(..) {
                t.read(cx).kill();
            }
            if matches!(g.page, Some(Page::Terminal(_))) {
                g.page = None;
                g.page_active = false;
            }
        }
    }

    /// The terminal the accessibility tree shows: the focused one of the panel and the editor area, else the one in
    /// front of the focused group, else the panel's (when the panel shows terminals). With its place and count.
    pub(super) fn a11y_terminal(&self, window: &Window, cx: &App) -> Option<(Entity<TerminalView>, usize, usize)> {
        let editor = self.front_editor_terminal().map(|t| {
            let ix = self.tabs.terminals.iter().position(|x| *x == t).unwrap_or(0);
            (t, ix, self.tabs.terminals.len())
        });
        let panel = self
            .active_terminal()
            .filter(|_| self.chrome.panel_open && self.chrome.bottom == Bottom::Terminal)
            .map(|t| (t, self.active_terminal, self.terminals.len()));
        match (editor, panel) {
            (Some(e), Some(p)) => Some(if p.0.focus_handle(cx).contains_focused(window, cx) { p } else { e }),
            (e, p) => e.or(p),
        }
    }

    /// A terminal in front of an editor group.
    pub(super) fn editor_terminal_view(&self, term: Entity<TerminalView>, p: &Palette) -> AnyElement {
        div().size_full().bg(c(p.bg)).p(px(theme::SPACE[3])).child(term).into_any_element()
    }

    /// The tabs of the terminals in group `key`.
    pub(super) fn editor_terminal_tabs(&self, key: GroupKey, state: &Group, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        for (ix, term) in state.terminals.iter().enumerate() {
            let active = state.page_active && matches!(&state.page, Some(Page::Terminal(t)) if t == term);
            let (title, exited) = {
                let t = term.read(cx);
                (t.title.clone(), t.has_exited())
            };
            let (show, close, middle) = (term.clone(), term.clone(), term.clone());
            let dragged = DraggedTerminalTab { term: term.clone(), from: key, title: title.clone() };
            let accent = p.accent;
            let tab = div()
                .id(SharedString::from(format!("tab-terminal-{}-{ix}", key.id)))
                .debug_selector(move || format!("tab-terminal-{}-{ix}", key.id))
                .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<DraggedTerminalTab>(move |s, _, _, _| s.border_l_2().border_color(c(accent)))
                .relative()
                .children((key.id == self.tabs.layout.focused()).then(|| a11y::part_frame(a11y::Part::TermTab(ix), cx)))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .max_w(px(260.))
                .pl(px(theme::SPACE[5]))
                .pr(px(theme::SPACE[2]))
                .border_r_1()
                .border_color(c(p.border))
                .cursor_pointer()
                .text_color(c(if active { p.text } else { p.text_muted }))
                .when(!active, |d| d.hover(|d| d.bg(c(p.surface_hover))))
                .when(active, |d| d.bg(c(p.bg)).child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(c(p.accent))))
                .child(icon("terminal", layout::ICON, if exited { p.text_muted } else { p.accent }))
                .child(div().overflow_hidden().whitespace_nowrap().child(title))
                .child(
                    div()
                        .id(SharedString::from(format!("tab-terminal-close-{}-{ix}", key.id)))
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::radius::SM))
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(icon("x", 14., p.text_muted))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            if this.ensure_group(key) {
                                this.return_editor_terminal(&close, window, cx);
                            }
                        })),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| {
                        if this.ensure_group(key) {
                            this.return_editor_terminal(&middle, window, cx);
                        }
                    }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.ensure_group(key) {
                        this.show_editor_terminal(show.clone(), window, cx);
                    }
                }));
            out.push(tab.into_any_element());
        }
        out
    }

    #[cfg(test)]
    pub(super) fn editor_terminals_for_test(&self) -> Vec<Entity<TerminalView>> {
        let mut out = self.tabs.terminals.clone();
        for g in self.tabs.groups.values() {
            out.extend(g.terminals.iter().cloned());
        }
        out
    }
}
