//! Workbench: opening, splitting and closing terminals; profiles, names and colours; the list of terminals.

use super::*;
use crate::terminal::Launch;
use crate::terminal_profiles::{COLORS, Profile};

gpui::actions!(workbench, [KillAllTerminals, RenameTerminal, ChangeTerminalColor, NewTerminalWithProfile, SelectDefaultTerminalProfile]);

/// A terminal's mark colour.
fn mark_color(role: Option<&str>, p: &Palette) -> u32 {
    match role {
        Some("accent") => p.accent,
        Some("success") => p.success,
        Some("warning") => p.warning,
        Some("danger") => p.danger,
        Some("info") => p.info,
        _ => p.text_muted,
    }
}

fn launch_of(profile: &Profile) -> Launch {
    Launch { shell: Some(profile.path.clone()), args: profile.args.clone(), env: profile.env.clone(), color: profile.color.clone() }
}

impl Workbench {
    /// Profiles for the machine the terminals run on (an SSH host's own operating system).
    fn terminal_profiles(&self, cx: &App) -> Vec<Profile> {
        let os = crate::terminal_profiles::os_key(&self.remote.host.info().os);
        cx.global::<Settings>().terminal_profiles.get(os).cloned().unwrap_or_default()
    }

    /// What a new terminal runs: the default profile, else `terminal.shell`, else the user's shell.
    fn default_launch(&self, cx: &App) -> Launch {
        let settings = cx.global::<Settings>();
        if let Some(p) = settings.terminal_default_profile.as_deref().and_then(|name| self.terminal_profiles(cx).into_iter().find(|p| p.name == name)) {
            return launch_of(&p);
        }
        Launch { shell: self.shell.clone().or_else(|| settings.terminal_shell.clone()), ..Launch::default() }
    }
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
        let launch = self.default_launch(cx);
        self.new_terminal_launch(cwd, launch, window, cx);
    }

    /// New terminal in the given folder running `launch`.
    pub(super) fn new_terminal_launch(&mut self, cwd: Option<String>, launch: Launch, window: &mut Window, cx: &mut Context<Self>) {
        // The number of processes is limited: after eight terminals no new one is opened.
        if self.terminals.len() >= MAX_TERMINALS {
            self.notice = Some(Notice::Info(crate::i18n::tf("Close a terminal first: at most {} can be open.", &[&MAX_TERMINALS]).into()));
            cx.notify();
            return;
        }
        let host = self.remote.host.clone();
        let term = cx.new(|cx| TerminalView::new_with(host, cwd, launch, cx));
        let weak = term.downgrade();
        self._subs.push(cx.subscribe_in(&term, window, move |this, source, event: &TerminalEvent, window, cx| match event {
            TerminalEvent::CloseRequested => {
                // The shell has exited: nothing runs any more, close without asking.
                if let Some(ix) = this.terminals.iter().position(|t| t.downgrade() == weak) {
                    this.close_terminal(ix, window, cx);
                }
            }
            TerminalEvent::TitleChanged | TerminalEvent::Exited => cx.notify(),
            TerminalEvent::ImagePasteFailed(error) => {
                this.notice = Some(Notice::Error(crate::i18n::tf("Could not paste: {}", &[error]).into()));
                cx.notify();
            }
            TerminalEvent::ConfirmPaste { data, lines } => {
                let (term, data) = (source.clone(), data.clone());
                let title = crate::i18n::tn("Paste {} line into the terminal?", "Paste {} lines into the terminal?", *lines, &[]);
                let detail = crate::i18n::t("The shell may run each line as soon as it arrives.");
                this.confirm(window, &title, detail, crate::i18n::t("Paste"), cx, move |_, cx| term.update(cx, |t, _| t.paste_confirmed(data)));
            }
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

    /// Split the active terminal: a new terminal in the folder its shell is in now goes next to it.
    pub(super) fn split_terminal(&mut self, _: &SplitTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let prev = self.terminals.get(self.active_terminal).cloned();
        let start = prev.as_ref().and_then(|t| t.read(cx).start_dir()).or_else(|| self.roots.primary.read(cx).root().map(str::to_string));
        // The folder the shell last reported (shell integration), else the one its process is in.
        let said = prev.as_ref().and_then(|t| t.read(cx).live_cwd());
        let handle = prev.as_ref().and_then(|t| t.read(cx).pty_handle()).filter(|_| said.is_none());
        let ask = cx.background_executor().spawn(async move { said.or_else(|| handle.and_then(|(host, id)| host.pty_status(id).cwd)) });
        cx.spawn_in(window, async move |this, cx| {
            let cwd = ask.await.or(start);
            this.update_in(cx, |this, window, cx| {
                let before = this.terminals.len();
                this.new_terminal_in(cwd, window, cx);
                if this.terminals.len() > before {
                    this.term_split = prev;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn kill_terminal(&mut self, _: &KillTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.terminals.get(self.active_terminal).cloned() {
            self.request_close(vec![t], window, cx);
        }
    }

    pub(super) fn kill_all_terminals(&mut self, _: &KillAllTerminals, window: &mut Window, cx: &mut Context<Self>) {
        let all = self.terminals.clone();
        self.request_close(all, window, cx);
    }

    /// Close terminals, asking first when a program other than the shell runs in one (`terminal.confirmOnClose`).
    pub(super) fn request_close(&mut self, terms: Vec<Entity<TerminalView>>, window: &mut Window, cx: &mut Context<Self>) {
        let mode = cx.global::<Settings>().terminal_confirm_close.clone();
        let handles: Vec<_> = terms.iter().filter(|t| !t.read(cx).has_exited()).filter_map(|t| t.read(cx).pty_handle()).collect();
        let check = cx.background_executor().spawn(async move {
            if mode == "never" {
                return (mode, Vec::new());
            }
            let busy: Vec<String> = handles
                .into_iter()
                .filter_map(|(host, id)| {
                    let s = host.pty_status(id);
                    (s.busy == Some(true)).then(|| s.name.unwrap_or_else(|| crate::i18n::t("a program").into()))
                })
                .collect();
            (mode, busy)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (mode, busy) = check.await;
            this.update_in(cx, |this, window, cx| {
                let close = move |this: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>| {
                    for t in &terms {
                        if let Some(ix) = this.terminals.iter().position(|x| x == t) {
                            this.close_terminal(ix, window, cx);
                        }
                    }
                };
                if busy.is_empty() && mode != "always" {
                    return close(this, window, cx);
                }
                let title = match busy.as_slice() {
                    [] => crate::i18n::t("Close the terminal?").to_string(),
                    [one] => crate::i18n::tf("“{}” is still running in this terminal. Close it anyway?", &[one]),
                    many => crate::i18n::tf("{} terminals have programs running. Close them anyway?", &[&many.len()]),
                };
                let detail = crate::i18n::t("Closing a terminal ends the programs running in it.");
                let answer = window.prompt(gpui::PromptLevel::Warning, &title, Some(detail), &[crate::i18n::t("Close"), crate::i18n::t("Cancel")], cx);
                cx.spawn_in(window, async move |this, cx| {
                    if answer.await == Ok(0) {
                        this.update_in(cx, |this, window, cx| close(this, window, cx)).ok();
                    }
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }

    /// "Terminal: New Terminal with Profile…": the profiles, then the machine's other shells.
    pub(super) fn new_terminal_with_profile(&mut self, _: &NewTerminalWithProfile, window: &mut Window, cx: &mut Context<Self>) {
        let profiles = self.terminal_profiles(cx);
        let host = self.remote.host.clone();
        let shells = cx.background_executor().spawn(async move { host.list_shells() });
        cx.spawn_in(window, async move |this, cx| {
            let shells = shells.await;
            this.update_in(cx, |this, window, cx| {
                let mut items: Vec<PickItem> =
                    profiles.iter().map(|p| PickItem { label: p.name.clone().into(), detail: Some(p.path.clone().into()), keys: None }).collect();
                let mut launches: Vec<Launch> = profiles.iter().map(launch_of).collect();
                for sh in shells.into_iter().filter(|sh| !profiles.iter().any(|p| &p.path == sh)) {
                    items.push(PickItem { label: crate::ui::file_name(&sh).to_string().into(), detail: Some(sh.clone().into()), keys: None });
                    launches.push(Launch { shell: Some(sh), ..Launch::default() });
                }
                if launches.is_empty() {
                    this.notice = Some(Notice::Info(crate::i18n::t("No profiles or shells were found on this machine.").into()));
                    return cx.notify();
                }
                if this.overlay.is_some() {
                    this.close_overlay(window, cx);
                }
                let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Open a terminal with…"), "", None, cx));
                this.show_overlay(picker, OverlayKind::TerminalLaunch(launches), window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// "Terminal: Select Default Profile…".
    pub(super) fn select_default_terminal_profile(&mut self, _: &SelectDefaultTerminalProfile, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<String> = self.terminal_profiles(cx).into_iter().map(|p| p.name).collect();
        if names.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("No terminal profiles yet: add them under terminal.profiles in settings.jsonc.").into()));
            return cx.notify();
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let current = cx.global::<Settings>().terminal_default_profile.clone();
        let items = names
            .iter()
            .map(|n| PickItem { label: n.clone().into(), detail: (current.as_ref() == Some(n)).then(|| crate::i18n::t("default").into()), keys: None })
            .collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Default terminal profile"), "", None, cx));
        self.show_overlay(picker, OverlayKind::TerminalDefaultProfile(names), window, cx);
    }

    /// "Terminal: Rename…".
    pub(super) fn rename_terminal(&mut self, _: &RenameTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.terminals.get(self.active_terminal) else { return };
        let title = t.read(cx).title.to_string();
        self.show_input(InputPurpose::RenameTerminal, crate::i18n::t("Rename"), crate::i18n::t("terminal name"), &title, window, cx);
    }

    pub(super) fn finish_rename_terminal(&mut self, name: String, cx: &mut Context<Self>) {
        let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
        if let (Some(t), false) = (self.terminals.get(self.active_terminal), name.trim().is_empty()) {
            t.update(cx, |t, cx| {
                t.set_fixed_title(name.trim().to_string());
                cx.notify();
            });
        }
    }

    /// "Terminal: Change Color…".
    pub(super) fn change_terminal_color(&mut self, _: &ChangeTerminalColor, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminals.is_empty() {
            return;
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let items = COLORS.iter().map(|c| PickItem { label: (*c).into(), detail: None, keys: None }).collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Terminal colour"), "", None, cx));
        self.show_overlay(picker, OverlayKind::TerminalColor, window, cx);
    }

    pub(super) fn set_terminal_color(&mut self, color: &str, cx: &mut Context<Self>) {
        if let Some(t) = self.terminals.get(self.active_terminal) {
            t.update(cx, |t, _| t.color = (color != "muted").then(|| color.to_string()));
            cx.notify();
        }
    }

    /// The list of terminals beside the active one (when there are several): mark, name and close.
    pub(super) fn terminal_list(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        (self.terminals.len() > 1).then(|| {
            div()
                .id("terminal-list")
                .w(px(180.))
                .flex_none()
                .h_full()
                .border_l_1()
                .border_color(c(p.border))
                .overflow_y_scroll()
                .children(self.terminals.iter().enumerate().map(|(i, t)| {
                    let active = i == self.active_terminal;
                    let term = t.clone();
                    let t = t.read(cx);
                    let label = if t.has_exited() { crate::i18n::tf("{} (exited)", &[&t.title]) } else { t.title.to_string() };
                    let mark = mark_color(t.color.as_deref(), p);
                    let group: SharedString = format!("term-row-{i}").into();
                    div()
                        .id(("term-tab", i))
                        .group(group.clone())
                        .w_full()
                        .h(px(layout::ROW_H))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[2]))
                        .px(px(theme::SPACE[4]))
                        .cursor_pointer()
                        .text_size(px(text::XS.0))
                        .when(active, |d| d.bg(c(p.surface_active)))
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .text_color(c(if active { p.text } else { p.text_muted }))
                        .child(icon(
                            "terminal",
                            14.,
                            if t.color.is_some() {
                                mark
                            } else if active {
                                p.text
                            } else {
                                p.text_muted
                            },
                        ))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(label))
                        .child(div().id(("term-close", i)).opacity(0.).group_hover(group, |s| s.opacity(1.)).child(icon("x", 12., p.text_muted)).on_click(
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.request_close(vec![term.clone()], window, cx);
                            }),
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.active_terminal = i;
                            this.chrome.bottom = Bottom::Terminal;
                            this.focus_terminal(window, cx);
                        }))
                        .into_any_element()
                }))
                .into_any_element()
        })
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

    pub(crate) fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            (crate::i18n::t("Copy Last Command Output"), Box::new(crate::terminal::TermCopyLastOutput), false),
            (crate::i18n::t("Clear Terminal"), Box::new(ClearTerminal), true),
            (crate::i18n::t("New Terminal"), Box::new(NewTerminal), true),
            (crate::i18n::t("Rename…"), Box::new(RenameTerminal), false),
            (crate::i18n::t("Change Color…"), Box::new(ChangeTerminalColor), false),
            (crate::i18n::t("Kill Terminal"), Box::new(KillTerminal), true),
            (crate::i18n::t("Kill All Terminals"), Box::new(KillAllTerminals), false),
        ];
        self.show_action_menu(entries, position, window, cx);
    }

    pub fn active_terminal(&self) -> Option<Entity<TerminalView>> {
        self.terminals.get(self.active_terminal).cloned()
    }
}
