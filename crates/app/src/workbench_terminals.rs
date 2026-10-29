//! Workbench: opening, splitting and closing terminals; profiles, names and colours; the list of terminals.

use super::*;
use crate::terminal::Launch;
use crate::terminal_profiles::{COLORS, Profile};

#[cfg(test)]
#[path = "workbench_terminal_context_tests.rs"]
mod context_tests;

gpui::actions!(
    workbench,
    [
        KillAllTerminals,
        KillOtherTerminals,
        RenameTerminal,
        ChangeTerminalColor,
        NewTerminalWithProfile,
        SelectDefaultTerminalProfile,
        ConfigureTerminalSettings,
        FocusNextTerminal,
        FocusPreviousTerminal
    ]
);

/// Height of a row in the terminal list.
const TERM_ROW_H: f32 = 28.;

/// The launch menu's profile and shell entries (the default profile ticked) and what each starts: profiles first, then the
/// machine's shells that no profile already runs.
fn launch_menu_entries(profiles: &[Profile], shells: &[String], default: Option<&str>) -> (Vec<crate::menu::MenuItem>, Vec<Launch>) {
    use crate::menu::MenuItem;
    let mut items = Vec::new();
    let mut launches = Vec::new();
    for p in profiles {
        items.push(MenuItem::new(&p.name).checked(default == Some(p.name.as_str())));
        launches.push(launch_of(p));
    }
    for sh in shells.iter().filter(|sh| !profiles.iter().any(|p| &p.path == *sh)) {
        items.push(MenuItem::new(crate::ui::file_name(sh)));
        launches.push(Launch { shell: Some(sh.clone()), ..Launch::default() });
    }
    (items, launches)
}

/// What a row of the terminal list shows for one terminal.
struct TermRow {
    label: String,
    mark: Option<String>,
    exited: bool,
    activity: crate::terminal::Activity,
    /// A long command finished while the terminal was out of sight.
    unseen: Option<crate::terminal::Finished>,
    cwd: Option<String>,
}

impl TermRow {
    /// `visible`: the terminal is on screen, so a finish it reports has been seen.
    fn of(t: &TerminalView, visible: bool) -> Self {
        let exited = t.has_exited();
        TermRow {
            label: if exited { crate::i18n::tf("{} (exited)", &[&t.title]) } else { t.title.to_string() },
            mark: t.color.clone(),
            exited,
            activity: if exited { crate::terminal::Activity::Idle } else { t.activity() },
            unseen: if visible { None } else { t.unseen_finish() },
            cwd: t.live_cwd().or_else(|| t.start_dir()),
        }
    }

    /// Beside the glyph: bars while a command runs, a dot for a finish not yet seen or a failed last command.
    fn indicator(&self, i: usize, p: &Palette) -> Option<AnyElement> {
        use crate::terminal::{Activity, Finished};
        let dot = |color: u32| div().size(px(7.)).flex_none().rounded_full().bg(c(color)).into_any_element();
        match (&self.activity, self.unseen) {
            (Activity::Running(_), _) => Some(ui::activity_bars(("term-activity", i), 12., p.accent)),
            (_, Some(Finished::Attention)) => Some(dot(p.warning)),
            (_, Some(Finished::Failed)) => Some(dot(p.danger)),
            (_, Some(Finished::Succeeded)) => Some(dot(p.success)),
            (Activity::Failed(_), None) => Some(dot(p.danger)),
            (Activity::Idle, None) => None,
        }
    }

    fn tooltip(&self) -> String {
        use crate::terminal::Activity;
        let mut lines = vec![self.label.clone()];
        if let Some(cwd) = &self.cwd {
            lines.push(cwd.clone());
        }
        match &self.activity {
            Activity::Running(cmd) if !cmd.is_empty() => lines.push(crate::i18n::tf("Running: {}", &[cmd])),
            Activity::Running(_) => lines.push(crate::i18n::t("A command is running").into()),
            Activity::Failed(code) => lines.push(crate::i18n::tf("The last command exited with code {}", &[code])),
            Activity::Idle => {}
        }
        lines.join("\n")
    }
}

/// The window is in the background: flash its taskbar button until it is brought forward (Windows). Elsewhere GPUI has
/// no way to ask yet; the terminal list and the Terminal tab still show it. Test windows have no platform window.
fn request_attention(window: &Window) {
    #[cfg(all(windows, not(test)))]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx};
        let Some(RawWindowHandle::Win32(h)) = HasWindowHandle::window_handle(window).ok().map(|h| h.as_raw()) else { return };
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd: HWND(h.hwnd.get() as *mut std::ffi::c_void),
            dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        // SAFETY: the HWND is the live window GPUI holds while `window` is borrowed.
        let _ = unsafe { FlashWindowEx(&info) };
    }
    #[cfg(any(not(windows), test))]
    let _ = window;
}

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
            TerminalEvent::TitleChanged | TerminalEvent::Exited | TerminalEvent::ActivityChanged => cx.notify(),
            TerminalEvent::Attention(message) => this.terminal_wants_attention(source, message.clone(), window, cx),
            TerminalEvent::LongCommandFinished { command, code } => this.long_command_finished(source, command, *code, window, cx),
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
                if let Some(ix) = this.terminals.iter().position(|t| t.downgrade() == weak)
                    && ix != this.active_terminal
                {
                    this.select_terminal(ix, window, cx);
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
                    this.term_split = prev.zip(this.terminals.last().cloned());
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
        let context = super::types::InputContext::Workspace { host: host.clone(), root: self.root(cx) };
        let shells = cx.background_executor().spawn(async move { host.list_shells() });
        cx.spawn_in(window, async move |this, cx| {
            let shells = shells.await;
            this.update_in(cx, |this, window, cx| {
                if !context.matches(this, cx) {
                    return;
                }
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
                this.show_overlay(picker, OverlayKind::TerminalLaunch { context, launches }, window, cx);
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
        let context = super::types::InputContext::Workspace { host: self.remote.host.clone(), root: self.root(cx) };
        self.show_overlay(picker, OverlayKind::TerminalDefaultProfile { context, names }, window, cx);
    }

    /// "Terminal: Rename…".
    pub(super) fn rename_terminal(&mut self, _: &RenameTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.terminals.get(self.active_terminal) else { return };
        let title = t.read(cx).title.to_string();
        let terminal = t.entity_id();
        self.show_input(InputPurpose::RenameTerminal { terminal }, crate::i18n::t("Rename"), crate::i18n::t("terminal name"), &title, window, cx);
    }

    pub(super) fn finish_rename_terminal(&mut self, terminal: gpui::EntityId, name: String, cx: &mut Context<Self>) {
        let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
        if let (Some(t), false) = (self.terminals.iter().find(|t| t.entity_id() == terminal), name.trim().is_empty()) {
            t.update(cx, |t, cx| {
                t.set_fixed_title(name.trim().to_string());
                cx.notify();
            });
        }
    }

    /// "Terminal: Change Color…".
    pub(super) fn change_terminal_color(&mut self, _: &ChangeTerminalColor, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminals.get(self.active_terminal).map(|t| t.entity_id()) else { return };
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let items = COLORS.iter().map(|c| PickItem { label: (*c).into(), detail: None, keys: None }).collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Terminal colour"), "", None, cx));
        self.show_overlay(picker, OverlayKind::TerminalColor { terminal }, window, cx);
    }

    pub(super) fn set_terminal_color(&mut self, terminal: gpui::EntityId, color: &str, cx: &mut Context<Self>) {
        if let Some(t) = self.terminals.iter().find(|t| t.entity_id() == terminal) {
            t.update(cx, |t, _| t.color = (color != "muted").then(|| color.to_string()));
            cx.notify();
        }
    }

    /// The list of terminals beside the active one (when there are several): mark, activity, name and close. Its left edge
    /// drags to resize it; a double click renames a terminal and a middle click closes it.
    pub(super) fn terminal_list(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.terminals.len() < 2 {
            return None;
        }
        let resizing = self.chrome.resizing_term_list;
        let shown = self.shown_split();
        let pair = self.term_split.clone().filter(|(l, r)| self.terminals.contains(l) && self.terminals.contains(r));
        let rows = self.terminals.iter().enumerate().map(|(i, t)| {
            let active = i == self.active_terminal;
            // The two split terminals carry the split glyph; they are on screen together when one of them is active.
            let paired = pair.as_ref().is_some_and(|(l, r)| l == t || r == t);
            let on_screen = active || shown.as_ref().is_some_and(|(l, r)| l == t || r == t);
            let row = TermRow::of(t.read(cx), on_screen);
            let term = t.clone();
            let group: SharedString = format!("term-row-{i}").into();
            let (fg, glyph_color) = match (row.exited, active) {
                (true, _) => (p.text_muted, p.text_muted),
                (false, true) => (p.text, row.mark.as_deref().map_or(p.text, |m| mark_color(Some(m), p))),
                (false, false) => (p.text_muted, row.mark.as_deref().map_or(p.text_muted, |m| mark_color(Some(m), p))),
            };
            let tip: SharedString = row.tooltip().into();
            div()
                .id(("term-tab", i))
                .group(group.clone())
                .relative()
                .w_full()
                .h(px(TERM_ROW_H))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .pl(px(theme::SPACE[4]))
                .pr(px(theme::SPACE[2]))
                .cursor_pointer()
                .text_size(px(text::SM.0))
                .when(active, |d| d.bg(c(p.surface_active)))
                .hover(|d| d.bg(c(p.surface_hover)))
                .text_color(c(fg))
                // The active row carries the accent on its left edge, like the active editor tab.
                .when(active, |d| d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(c(p.accent))))
                .child(icon(if paired { "tb-split" } else { "terminal" }, layout::ICON, glyph_color))
                .children(row.indicator(i, p))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().when(row.exited, |d| d.italic()).child(row.label))
                .child(
                    div()
                        .id(("term-close", i))
                        .size(px(20.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::radius::SM))
                        .opacity(0.)
                        .when(active, |d| d.opacity(1.))
                        .group_hover(group, |s| s.opacity(1.))
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(icon("tb-close", 14., p.text_muted))
                        .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Kill Terminal").into())).into())
                        .on_click(cx.listener({
                            let term = term.clone();
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.request_close(vec![term.clone()], window, cx);
                            }
                        })),
                )
                .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into())
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener({
                        let term = term.clone();
                        move |this, _, window, cx| this.request_close(vec![term.clone()], window, cx)
                    }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                        if ev.click_count == 2 {
                            this.select_terminal(i, window, cx);
                            this.rename_terminal(&RenameTerminal, window, cx);
                        }
                    }),
                )
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                    if ev.click_count() < 2 {
                        this.select_terminal(i, window, cx);
                    }
                }))
                .into_any_element()
        });
        Some(
            div()
                .relative()
                .w(px(self.chrome.term_list_w))
                .flex_none()
                .h_full()
                .border_l_1()
                .border_color(c(p.border))
                .child(div().id("terminal-list").size_full().overflow_y_scroll().children(rows))
                .child(
                    div()
                        .id("terminal-list-sash")
                        .absolute()
                        .occlude()
                        .left(px(-layout::SASH / 2.))
                        .top_0()
                        .bottom_0()
                        .w(px(layout::SASH))
                        .cursor(gpui::CursorStyle::ResizeLeftRight)
                        .child(crate::ui_sash::highlight(p.accent, resizing))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.chrome.resizing_term_list = true;
                                cx.notify();
                            }),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Make terminal `ix` the active one and focus it. What was on screen before, and what is now, counts as seen.
    pub(super) fn select_terminal(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.terminals.len() {
            return;
        }
        self.mark_visible_terminals_seen(cx);
        self.active_terminal = ix;
        self.chrome.bottom = Bottom::Terminal;
        self.focus_terminal(window, cx);
    }

    /// The terminals on screen (the active one and its split partner) have been looked at.
    fn mark_visible_terminals_seen(&mut self, cx: &mut Context<Self>) {
        let visible: Vec<_> = match self.shown_split() {
            Some((l, r)) => vec![l, r],
            None => self.terminals.get(self.active_terminal).cloned().into_iter().collect(),
        };
        for t in visible {
            t.update(cx, |t, _| t.mark_seen());
        }
    }

    /// "Terminal: Focus Next Terminal" / "Focus Previous Terminal": round the list.
    pub(super) fn focus_next_terminal(&mut self, _: &FocusNextTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_terminal(1, window, cx);
    }

    pub(super) fn focus_previous_terminal(&mut self, _: &FocusPreviousTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_terminal(-1, window, cx);
    }

    fn cycle_terminal(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.terminals.len();
        if n == 0 {
            return;
        }
        let ix = (self.active_terminal as isize + step).rem_euclid(n as isize) as usize;
        self.chrome.panel_open = true;
        self.select_terminal(ix, window, cx);
    }

    /// "Terminal: Kill Other Terminals": every terminal but the active one (asking first if programs run in them).
    pub(super) fn kill_other_terminals(&mut self, _: &KillOtherTerminals, window: &mut Window, cx: &mut Context<Self>) {
        let keep = self.terminals.get(self.active_terminal).cloned();
        let others: Vec<_> = self.terminals.iter().filter(|t| Some(*t) != keep.as_ref()).cloned().collect();
        if !others.is_empty() {
            self.request_close(others, window, cx);
        }
    }

    /// "Terminal: Configure Terminal Settings": the settings page, filtered to the terminal.
    pub(super) fn configure_terminal_settings(&mut self, _: &ConfigureTerminalSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_page(&OpenSettingsPage, window, cx);
        if let Some(Page::Settings(page)) = self.tabs.page.clone() {
            page.update(cx, |page, cx| page.set_query("terminal.", cx));
        }
    }

    /// The menu beside the New Terminal button: start one with a profile or another shell, split, tasks and settings.
    pub(super) fn open_terminal_launch_menu(&mut self, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let profiles = self.terminal_profiles(cx);
        let default = cx.global::<Settings>().terminal_default_profile.clone();
        let host = self.remote.host.clone();
        let context = super::types::InputContext::Workspace { host: host.clone(), root: self.root(cx) };
        let shells = cx.background_executor().spawn(async move { host.list_shells() });
        cx.spawn_in(window, async move |this, cx| {
            let shells = shells.await;
            this.update_in(cx, |this, window, cx| {
                if !context.matches(this, cx) {
                    return;
                }
                use crate::menu::MenuItem;
                let (items, launches) = launch_menu_entries(&profiles, &shells, default.as_deref());
                let mut items: Vec<MenuItem> = items;
                let mut commands: Vec<MenuCmd> = (0..launches.len()).map(MenuCmd::TerminalLaunch).collect();
                let mut actions: Vec<Box<dyn gpui::Action>> = Vec::new();
                let key = |a: &dyn gpui::Action| Command { label: String::new(), run: Run::Action(a.boxed_clone()) }.shortcut(cx);
                let fixed: Vec<(&str, Box<dyn gpui::Action>, bool)> = vec![
                    (crate::i18n::t("New Terminal"), Box::new(NewTerminal), !launches.is_empty()),
                    (crate::i18n::t("Split Terminal"), Box::new(SplitTerminal), false),
                    (crate::i18n::t("New Terminal with Profile…"), Box::new(NewTerminalWithProfile), false),
                    (crate::i18n::t("Run Task…"), Box::new(RunTask), true),
                    (crate::i18n::t("Configure Tasks…"), Box::new(ConfigureTasks), false),
                    (crate::i18n::t("Select Default Profile…"), Box::new(SelectDefaultTerminalProfile), true),
                    (crate::i18n::t("Configure Terminal Settings"), Box::new(ConfigureTerminalSettings), false),
                ];
                for (label, action, sep) in fixed {
                    let item = MenuItem::new(label).shortcut(key(action.as_ref()));
                    items.push(if sep { item.separator() } else { item });
                    commands.push(MenuCmd::Dispatch(actions.len()));
                    actions.push(action);
                }
                this.term_launches = launches;
                let target = MenuTarget { path: String::new(), is_dir: false, tab: None };
                this.show_menu(items, commands, actions, target, position, window, cx);
                if let Some(state) = &mut this.menu {
                    state.context = Some(context);
                    state.menu.update(cx, |menu, _| menu.set_anchor(gpui::Corner::TopRight));
                }
            })
            .ok();
        })
        .detach();
    }

    /// An entry of the launch menu was chosen.
    pub(super) fn launch_from_menu(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(launch) = self.term_launches.get(ix).cloned() {
            let cwd = self.active_root(cx);
            self.new_terminal_launch(cwd, launch, window, cx);
        }
    }

    pub(super) fn close_terminal(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let term = self.terminals.remove(ix);
        term.read(cx).kill();
        if self.term_split.as_ref().is_some_and(|(l, r)| *l == term || *r == term) {
            self.term_split = None;
        }
        // Closing a terminal before the active one moves the active one up a place.
        if ix < self.active_terminal {
            self.active_terminal -= 1;
        }
        if self.terminals.is_empty() {
            self.chrome.panel_open = false;
            match self.active_editor() {
                Some(ed) => window.focus(&ed.focus_handle(cx)),
                None => window.focus(&self.focus_handle),
            }
        } else {
            self.active_terminal = self.active_terminal.min(self.terminals.len() - 1);
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
        self.mark_visible_terminals_seen(cx);
        cx.notify();
    }

    /// Whether terminal `t` is on screen now: the panel shows the terminals and it is the active one or beside it.
    fn terminal_on_screen(&self, t: &Entity<TerminalView>) -> bool {
        self.chrome.panel_open
            && self.chrome.bottom == Bottom::Terminal
            && (self.terminals.get(self.active_terminal) == Some(t) || self.shown_split().is_some_and(|(l, r)| &l == t || &r == t))
    }

    /// The Terminal tab's badge: terminals out of sight whose program asked for the user.
    pub(crate) fn terminals_asking(&self, cx: &App) -> usize {
        self.terminals.iter().filter(|t| !self.terminal_on_screen(t) && t.read(cx).unseen_finish() == Some(crate::terminal::Finished::Attention)).count()
    }

    /// A long command ended. Nothing to say when its terminal is on screen in the active window; otherwise a notice (when
    /// it is out of sight) and the taskbar (when the window is in the background). The list's dot comes from the terminal.
    fn long_command_finished(&mut self, t: &Entity<TerminalView>, command: &str, code: Option<i32>, window: &mut Window, cx: &mut Context<Self>) {
        let active = window.is_window_active();
        let on_screen = self.terminal_on_screen(t);
        if active && on_screen {
            return;
        }
        if !on_screen {
            let what = match code {
                Some(code) if code != 0 && command.is_empty() => crate::i18n::tf("A command failed with code {}", &[&code]),
                Some(code) if code != 0 => crate::i18n::tf("Failed with code {}: {}", &[&code, &command]),
                _ if command.is_empty() => crate::i18n::t("A command finished").to_string(),
                _ => crate::i18n::tf("Finished: {}", &[&command]),
            };
            let title = t.read(cx).title.to_string();
            self.notice = Some(Notice::Info(crate::i18n::tf("{}: {}", &[&title, &what]).into()));
        }
        if !active {
            request_attention(window);
        }
        cx.notify();
    }

    /// The bell or a notification (Claude Code, Codex waiting for an answer). Seen at once when its terminal is on screen
    /// in the active window; otherwise the list and the tab are flagged, a message shows as a notice, and an inactive
    /// window asks for the user from the taskbar.
    fn terminal_wants_attention(&mut self, t: &Entity<TerminalView>, message: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let active = window.is_window_active();
        if active && self.terminal_on_screen(t) {
            t.update(cx, |t, _| t.mark_seen());
            return cx.notify();
        }
        if let Some(message) = message.filter(|_| !self.terminal_on_screen(t)) {
            let title = t.read(cx).title.to_string();
            self.notice = Some(Notice::Info(crate::i18n::tf("{}: {}", &[&title, &message]).into()));
        }
        if !active {
            request_attention(window);
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

    /// The split pair, when the active terminal is one of the two.
    pub(super) fn shown_split(&self) -> Option<(Entity<TerminalView>, Entity<TerminalView>)> {
        let active = self.terminals.get(self.active_terminal)?;
        self.term_split.clone().filter(|(l, r)| (l == active || r == active) && self.terminals.contains(l) && self.terminals.contains(r))
    }

    #[cfg(test)]
    pub fn terminals_for_test(&self) -> Vec<Entity<TerminalView>> {
        self.terminals.clone()
    }

    #[cfg(test)]
    pub fn close_terminal_for_test(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.close_terminal(ix, window, cx);
    }

    #[cfg(test)]
    pub fn select_terminal_for_test(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.select_terminal(ix, window, cx);
    }

    /// The labels of the New Terminal button's menu, as it would open now.
    #[cfg(test)]
    pub fn launch_menu_for_test(&self, shells: &[String], cx: &App) -> Vec<(String, Option<bool>)> {
        let default = cx.global::<Settings>().terminal_default_profile.clone();
        let (items, _) = launch_menu_entries(&self.terminal_profiles(cx), shells, default.as_deref());
        items.into_iter().map(|i| (i.label.to_string(), i.checked)).collect()
    }

    /// The terminal shown beside the active one.
    #[cfg(test)]
    pub fn split_partner(&self) -> Option<Entity<TerminalView>> {
        let active = self.terminals.get(self.active_terminal)?;
        self.shown_split().map(|(l, r)| if &l == active { r } else { l })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, path: &str) -> Profile {
        Profile { name: name.into(), path: path.into(), args: None, env: Vec::new(), color: None }
    }

    #[test]
    fn the_launch_menu_lists_profiles_then_the_other_shells() {
        let profiles = [profile("Work", "/bin/zsh"), profile("Plain", "/bin/sh")];
        let shells = ["/bin/bash".to_string(), "/bin/zsh".to_string(), "/opt/homebrew/bin/fish".to_string()];
        let (items, launches) = launch_menu_entries(&profiles, &shells, Some("Plain"));
        let labels: Vec<(&str, Option<bool>)> = items.iter().map(|i| (i.label.as_ref(), i.checked)).collect();
        assert_eq!(labels, [("Work", Some(false)), ("Plain", Some(true)), ("bash", None), ("fish", None)], "zsh already has a profile");
        assert_eq!(launches.len(), labels.len());
        assert_eq!(launches[2].shell.as_deref(), Some("/bin/bash"));
        assert_eq!(launches[0], launch_of(&profiles[0]));
    }

    #[test]
    fn a_row_shows_what_its_terminal_is_doing() {
        use crate::terminal::{Activity, Finished};
        let row = |activity, unseen| TermRow { label: "zsh".into(), mark: None, exited: false, activity, unseen, cwd: Some("/srv/app".into()) };
        let p = theme::DARK;
        assert!(row(Activity::Idle, None).indicator(0, &p).is_none(), "nothing to say");
        assert!(row(Activity::Running("$ make".into()), None).indicator(0, &p).is_some());
        assert!(row(Activity::Idle, Some(Finished::Succeeded)).indicator(0, &p).is_some());
        assert!(row(Activity::Failed(2), None).indicator(0, &p).is_some());
        assert_eq!(row(Activity::Running("$ make".into()), None).tooltip(), "zsh\n/srv/app\nRunning: $ make");
        assert_eq!(row(Activity::Failed(2), None).tooltip(), "zsh\n/srv/app\nThe last command exited with code 2");
    }
}
