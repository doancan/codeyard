//! Workbench: opening, splitting and closing terminals.

use super::*;

impl Workbench {
    pub(super) fn toggle_panel(&mut self, _: &TogglePanel, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel_open = !self.chrome.panel_open;
        if self.chrome.panel_open {
            if self.terminals.is_empty() {
                self.new_terminal(&NewTerminal, window, cx);
                return;
            }
            self.focus_terminal(window, cx);
        } else {
            window.focus(&self.focus_handle);
        }
        cx.notify();
    }

    /// Ctrl+`: open a terminal if there is none; focus it if there is; close the panel if it is focused.
    pub(super) fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.terminals.get(self.active_terminal).is_some_and(|t| t.focus_handle(cx).is_focused(window));
        if focused {
            self.chrome.panel_open = false;
            if let Some(ed) = self.active_editor() {
                window.focus(&ed.focus_handle(cx));
            } else {
                window.focus(&self.focus_handle);
            }
            cx.notify();
        } else if self.terminals.is_empty() {
            self.new_terminal(&NewTerminal, window, cx);
        } else {
            self.chrome.panel_open = true;
            self.focus_terminal(window, cx);
        }
    }

    pub(super) fn new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.active_root(cx);
        self.new_terminal_in(cwd, window, cx);
    }

    /// New terminal in the given folder.
    pub(super) fn new_terminal_in(&mut self, cwd: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        // The number of processes is limited: after eight terminals no new one is opened.
        if self.terminals.len() >= MAX_TERMINALS {
            self.notice = Some(Notice::Info(crate::i18n::tf("Close a terminal first: at most {} can be open.", &[&MAX_TERMINALS]).into()));
            cx.notify();
            return;
        }
        let host = self.remote.host.clone();
        let shell = self.shell.clone().or_else(|| cx.global::<Settings>().terminal_shell.clone());
        let term = cx.new(|cx| TerminalView::new(host, cwd, shell, cx));
        let weak = term.downgrade();
        self._subs.push(cx.subscribe_in(&term, window, move |this, _, event: &TerminalEvent, window, cx| match event {
            TerminalEvent::CloseRequested => {
                if let Some(ix) = this.terminals.iter().position(|t| t.downgrade() == weak) {
                    this.close_terminal(ix, window, cx);
                }
            }
            TerminalEvent::TitleChanged | TerminalEvent::Exited => cx.notify(),
            TerminalEvent::OpenLink(link) => this.open_link(link.clone(), window, cx),
            TerminalEvent::ContextMenu(position) => {
                if let Some(ix) = this.terminals.iter().position(|t| t.downgrade() == weak) {
                    this.active_terminal = ix;
                }
                this.open_terminal_menu(*position, window, cx);
            }
        }));
        self.terminals.push(term);
        self.active_terminal = self.terminals.len() - 1;
        self.chrome.panel_open = true;
        self.focus_terminal(window, cx);
    }

    /// Split the active terminal: a new terminal in the same folder goes next to it.
    pub(super) fn split_terminal(&mut self, _: &SplitTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.terminals.len();
        let prev = self.terminals.get(self.active_terminal).cloned();
        let cwd = prev.as_ref().and_then(|t| t.read(cx).start_dir()).or_else(|| self.roots.primary.read(cx).root().map(str::to_string));
        self.new_terminal_in(cwd, window, cx);
        if self.terminals.len() > before {
            self.term_split = prev;
            cx.notify();
        }
    }

    pub(super) fn kill_terminal(&mut self, _: &KillTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminals.is_empty() {
            self.close_terminal(self.active_terminal, window, cx);
        }
    }

    pub(super) fn close_terminal(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let term = self.terminals.remove(ix);
        term.read(cx).kill();
        if self.term_split.as_ref() == Some(&term) {
            self.term_split = None;
        }
        if self.terminals.is_empty() {
            self.chrome.panel_open = false;
            match self.active_editor() {
                Some(ed) => window.focus(&ed.focus_handle(cx)),
                None => window.focus(&self.focus_handle),
            }
        } else {
            self.active_terminal = self.active_terminal.min(self.terminals.len() - 1);
            if self.term_split.as_ref() == self.terminals.get(self.active_terminal) {
                self.term_split = None; // the split is meaningless if the peer became active
            }
            self.focus_terminal(window, cx);
        }
        cx.notify();
    }

    /// Connection changed: the terminals belong to the old machine, close them.
    pub(super) fn close_all_terminals(&mut self, cx: &mut Context<Self>) {
        self.term_split = None;
        for t in self.terminals.drain(..) {
            t.read(cx).kill();
        }
        self.chrome.panel_open = false;
    }

    pub(super) fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.bottom = Bottom::Terminal;
        if let Some(t) = self.terminals.get(self.active_terminal) {
            window.focus(&t.focus_handle(cx));
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn set_shell(&mut self, shell: &str) {
        self.shell = Some(shell.to_string());
    }

    #[cfg(test)]
    pub fn terminal_count(&self) -> usize {
        self.terminals.len()
    }

    #[cfg(test)]
    pub fn split_partner(&self) -> Option<Entity<TerminalView>> {
        self.term_split.clone()
    }

    /// Right click in a terminal: the terminal has focus, so the clipboard and find actions go to it.
    fn open_terminal_menu(&mut self, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        use crate::terminal::{TermCopy, TermFind, TermPaste, TermSelectAll};
        let entries: Vec<(&str, Box<dyn gpui::Action>, bool)> = vec![
            (crate::i18n::t("Copy"), Box::new(TermCopy), false),
            (crate::i18n::t("Paste"), Box::new(TermPaste), false),
            (crate::i18n::t("Select All"), Box::new(TermSelectAll), false),
            (crate::i18n::t("Find"), Box::new(TermFind), true),
            (crate::i18n::t("Clear Terminal"), Box::new(ClearTerminal), true),
            (crate::i18n::t("New Terminal"), Box::new(NewTerminal), true),
            (crate::i18n::t("Kill Terminal"), Box::new(KillTerminal), false),
        ];
        self.show_action_menu(entries, position, window, cx);
    }

    pub fn active_terminal(&self) -> Option<Entity<TerminalView>> {
        self.terminals.get(self.active_terminal).cloned()
    }
}
