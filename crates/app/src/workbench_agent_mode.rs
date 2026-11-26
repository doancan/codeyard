//! Agent mode: one window for every open project. The editor area is replaced by a list of the projects (every open window),
//! the selected project's terminals and, on the right, its source control, diff, pull requests and files.
//!
//! A project stays in its own window with its own terminals; this mode only reads those windows and draws their terminals,
//! so changing project never stops anything (see docs/agent-mode.md).

use std::time::Duration;

use super::agent_panel::{AgentSummary, profile_label};
use super::claude_store::Provider;
use super::notifications_glue::model::{NoticeAction, Notification, Severity};
use super::*;
use crate::a11y_controls::Control;
use crate::agent_accounts::{Limits, Mark};
use crate::agent_limits::{self, Limit};
use crate::agent_usage;
use crate::i18n::{t, tf, tn};
use crate::terminal::{Activity, Finished};
use gpui::{Div, WeakEntity};

/// Which projects the list shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Scope {
    All,
    Local,
    Ssh,
}

impl Scope {
    /// Whether a project on an SSH host (`ssh`) or on this computer is listed.
    fn includes(self, ssh: bool) -> bool {
        match self {
            Self::All => true,
            Self::Local => !ssh,
            Self::Ssh => ssh,
        }
    }
}

/// The tabs of the right panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RightTab {
    Git,
    Diff,
    Pulls,
    Files,
}

/// Agent mode while it is on.
pub(super) struct AgentMode {
    /// The project (its window) whose terminals are shown; `None` is the first project of the list.
    selected: Option<gpui::WindowId>,
    /// The terminal tab of the selected project.
    tab: usize,
    scope: Scope,
    /// The terminal this window decides the size of while it is shown (weak: the project owns the terminal).
    leading: Option<WeakEntity<TerminalView>>,
    /// The tab of the right panel, and whether the panel is open (its icon rail is always there).
    right: RightTab,
    right_open: bool,
    /// The diff last asked for, with the project it belongs to (shown under Diff).
    diff: Option<(gpui::WindowId, Entity<crate::diff::DiffView>)>,
    /// The pull request panel of each project that was looked at (it keeps its list while another project is shown).
    pulls: std::collections::HashMap<gpui::WindowId, Entity<super::pulls_panel::PullsPanel>>,
    /// Tokens each Claude Code account spent in the last five hours, read every minute off the UI thread.
    usage: std::collections::HashMap<u32, u64>,
    usage_at: Option<std::time::Instant>,
    usage_task: Option<gpui::Task<()>>,
    /// The account list under the terminal tabs is open.
    account_menu: bool,
    /// How many projects the Dock badge says need the person.
    badge: usize,
    /// What each project needed at the last look, to say when that changes.
    seen: std::collections::HashMap<gpui::WindowId, Status>,
    /// The project whose views are drawn here now. Its window knows, so its menus and pickers open here.
    hosting: Option<gpui::WindowId>,
    /// Redraws once a second so that the other projects' states stay current without being observed one by one.
    _refresh: gpui::Task<()>,
}

/// What a project needs from the person, most urgent first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Status {
    /// A program waits for an answer (the bell or a notification).
    Attention,
    /// The tool printed a usage-limit message.
    Limit,
    /// A command or the agent launcher's session runs.
    Running,
    /// The launcher's session ended.
    Done,
    /// At the prompt, or no terminal.
    Idle,
}

impl Status {
    fn needs_you(self) -> bool {
        matches!(self, Self::Attention | Self::Limit)
    }

    fn label(self) -> SharedString {
        match self {
            Self::Attention => t("Needs you").into(),
            Self::Limit => t("Limit reached").into(),
            Self::Running => t("Running").into(),
            Self::Done => t("Done").into(),
            Self::Idle => t("Idle").into(),
        }
    }

    fn color(self, p: &Palette) -> u32 {
        match self {
            Self::Attention => p.warning,
            Self::Limit => p.danger,
            Self::Running => p.accent,
            Self::Done => p.success,
            Self::Idle => p.text_muted,
        }
    }
}

/// What agent mode reads from one project window.
struct Peer {
    id: gpui::WindowId,
    name: String,
    /// The SSH host, `None` for this computer.
    host: Option<String>,
    branch: Option<String>,
    changes: usize,
    /// The agent launcher's terminal first, then the panel's terminals.
    terminals: Vec<Entity<TerminalView>>,
    /// The first terminal is the launcher's session (it is ended from the Agents panel, not closed as a tab).
    launcher: bool,
    status: Status,
    last_line: String,
    limit: Option<Limit>,
    /// The agent launcher's session of the project, when its terminal is the one shown first.
    agent: Option<AgentSummary>,
}

impl Peer {
    fn extra_terminals(&self) -> usize {
        self.terminals.len().saturating_sub(1)
    }
}

/// One account in the list at the foot of the sidebar.
struct AccountRow {
    provider: Provider,
    profile: u32,
    limited: Option<Mark>,
    /// Tokens spent in the last five hours (Claude Code only) and that as a share of the busiest account's.
    tokens: Option<u64>,
    share: f32,
    /// The share of `agents.claudeTokenBudget`, when one is set.
    percent: Option<u32>,
}

/// What an account's use says: "Available" when nothing is known, else its tokens and, with a budget, their share of it.
fn usage_text(row: &AccountRow) -> SharedString {
    match (row.tokens, row.percent) {
        (Some(tokens), Some(percent)) => tf("{} tokens · {}%", &[&short_tokens(tokens), &percent]).into(),
        (Some(tokens), None) => tf("{} tokens", &[&short_tokens(tokens)]).into(),
        (None, _) => t("Available").into(),
    }
}

/// "950", "12k", "1.3M".
fn short_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// The status of a project from its main terminal. `launcher` is whether that terminal is the agent launcher's session.
fn status_of(exited: bool, limit: bool, unseen: Option<Finished>, activity: &Activity, launcher: bool) -> Status {
    if exited {
        return if launcher { Status::Done } else { Status::Idle };
    }
    if limit {
        Status::Limit
    } else if unseen == Some(Finished::Attention) {
        Status::Attention
    } else if launcher || matches!(activity, Activity::Running(_)) {
        Status::Running
    } else {
        Status::Idle
    }
}

/// The list as drawn: *Needs you* first, then this computer, then one group per SSH host (in the order the windows were found).
fn grouped<'a>(visible: &[&'a Peer]) -> Vec<(SharedString, Vec<&'a Peer>)> {
    let mut groups: Vec<(SharedString, Vec<&Peer>)> = Vec::new();
    let urgent: Vec<&Peer> = visible.iter().copied().filter(|peer| peer.status.needs_you()).collect();
    if !urgent.is_empty() {
        groups.push((t("Attention").into(), urgent));
    }
    let local: Vec<&Peer> = visible.iter().copied().filter(|peer| !peer.status.needs_you() && peer.host.is_none()).collect();
    if !local.is_empty() {
        groups.push((t("This computer").into(), local));
    }
    let mut hosts: Vec<&str> = Vec::new();
    for peer in visible.iter().filter(|peer| !peer.status.needs_you()) {
        if let Some(host) = peer.host.as_deref()
            && !hosts.contains(&host)
        {
            hosts.push(host);
        }
    }
    for host in hosts {
        let members: Vec<&Peer> = visible.iter().copied().filter(|peer| !peer.status.needs_you() && peer.host.as_deref() == Some(host)).collect();
        groups.push((tf("SSH host {}", &[&host]).into(), members));
    }
    groups
}

/// The last row with text.
fn last_line(rows: &[String]) -> String {
    rows.iter().rev().map(|r| r.trim()).find(|r| !r.is_empty()).unwrap_or_default().chars().take(120).collect()
}

impl Workbench {
    pub(super) fn toggle_agent_mode(&mut self, _: &ToggleAgentMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.agent_mode.is_some() {
            self.leave_agent_mode(cx);
            return;
        }
        // Once a second, drawn or not: a window that is covered or in the background may not be drawn at all, and that is when
        // a project that starts waiting has to be noticed.
        let refresh = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update_in(cx, |this, window, cx| this.agent_mode_tick(window, cx)).is_err() {
                    return;
                }
            }
        });
        // The way it was left: the project, the filter and the right tab.
        let (selected, scope, right, right_open) =
            self.chrome.agent_last.take().map_or((None, Scope::All, RightTab::Git, true), |(id, scope, right, open)| (Some(id), scope, right, open));
        self.chrome.agent_mode = Some(AgentMode {
            selected,
            tab: 0,
            scope,
            leading: None,
            right,
            right_open,
            diff: None,
            pulls: Default::default(),
            usage: Default::default(),
            usage_at: None,
            usage_task: None,
            account_menu: false,
            badge: 0,
            seen: Default::default(),
            hosting: None,
            _refresh: refresh,
        });
        // Typing goes to the terminal shown, as soon as the mode opens.
        let peers = self.agent_mode_peers(window, cx);
        let shown = peers
            .iter()
            .filter(|peer| scope.includes(peer.host.is_some()))
            .find(|peer| Some(peer.id) == selected)
            .or_else(|| peers.iter().find(|peer| scope.includes(peer.host.is_some())));
        if let Some(peer) = shown {
            self.agent_mode_select(peer, 0, window, cx);
        }
        cx.notify();
    }

    /// The project after (`step` 1) or before (`-1`) the one shown, in the order of the list; with `urgent` only a project that
    /// needs the person counts. Does nothing outside agent mode or when there is no such project.
    fn agent_mode_step(&mut self, step: isize, urgent: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((selected, scope)) = self.chrome.agent_mode.as_ref().map(|mode| (mode.selected, mode.scope)) else { return };
        let peers = self.agent_mode_peers(window, cx);
        let visible: Vec<&Peer> = peers.iter().filter(|peer| scope.includes(peer.host.is_some())).collect();
        let order: Vec<&Peer> = grouped(&visible).into_iter().flat_map(|(_, members)| members).collect();
        // The project shown is the selected one, or the first of the list when none was chosen (as the view reads it).
        let current =
            visible.iter().find(|peer| Some(peer.id) == selected).or(visible.first()).and_then(|shown| order.iter().position(|peer| peer.id == shown.id));
        let Some(current) = current else { return };
        let len = order.len() as isize;
        let next = (1..len).map(|n| order[(current as isize + step * n).rem_euclid(len) as usize]).find(|peer| !urgent || peer.status.needs_you());
        if let Some(peer) = next {
            self.agent_mode_select(peer, 0, window, cx);
        }
    }

    pub(super) fn agent_mode_next_project(&mut self, _: &AgentModeNextProject, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_mode_step(1, false, window, cx);
    }

    pub(super) fn agent_mode_previous_project(&mut self, _: &AgentModePreviousProject, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_mode_step(-1, false, window, cx);
    }

    pub(super) fn agent_mode_next_attention(&mut self, _: &AgentModeNextAttention, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_mode_step(1, true, window, cx);
    }

    /// Closes terminal `term` of the project of window `id` (a tab of agent mode) and shows the tab before it.
    fn agent_mode_close_terminal(&mut self, id: gpui::WindowId, term: &Entity<TerminalView>, tab: usize, window: &mut Window, cx: &mut Context<Self>) {
        let close = |wb: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>| {
            if let Some(ix) = wb.terminals.iter().position(|t| t == term) {
                wb.close_terminal(ix, window, cx);
            }
        };
        if id == window.window_handle().window_id() {
            close(self, window, cx);
        } else if let Some(handle) = self.agent_mode_handle(id, cx) {
            handle.update(cx, |wb, window, cx| close(wb, window, cx)).ok();
        }
        if let Some(peer) = self.agent_mode_peers(window, cx).into_iter().find(|peer| peer.id == id) {
            self.agent_mode_select(&peer, tab.saturating_sub(1), window, cx);
        }
    }

    /// What agent mode notes about the projects without being drawn: limits seen, the Dock count, notices and banners, usage.
    fn agent_mode_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let peers = self.agent_mode_peers(window, cx);
        self.agent_mode_account_state(&peers, window, cx);
        cx.notify();
    }

    /// Back to the editor: what was shown goes back to its own window (the terminal's size, the project's menus), the Dock badge goes.
    pub(super) fn leave_agent_mode(&mut self, cx: &mut Context<Self>) {
        let Some(mode) = self.chrome.agent_mode.take() else { return };
        self.chrome.agent_last = mode.selected.map(|id| (id, mode.scope, mode.right, mode.right_open));
        if let Some(term) = mode.leading.and_then(|weak| weak.upgrade()) {
            term.update(cx, |t, cx| t.lead_size(None, cx));
        }
        self.agent_mode_release(mode.hosting, cx);
        crate::dock_attention::set_badge(0);
        cx.notify();
    }

    fn agent_mode_peer(&self, id: gpui::WindowId, cx: &App) -> Peer {
        let launcher = self.agent_terminal();
        let mut terminals: Vec<Entity<TerminalView>> = launcher.iter().cloned().collect();
        terminals.extend(self.terminals.iter().filter(|term| !launcher.iter().any(|l| l == *term)).cloned());
        let summary = launcher.as_ref().and_then(|_| self.agent_summary());
        let (status, last, limit) = match terminals.first() {
            None => (Status::Idle, String::new(), None),
            Some(main) => {
                let main = main.read(cx);
                let rows = main.a11y_view().0;
                // A limit message the person has said to ignore (the account was marked available) is not one.
                let quiet = summary
                    .as_ref()
                    .is_some_and(|agent| cx.try_global::<Limits>().is_some_and(|limits| limits.is_quiet(agent.provider.key(), agent.profile, now_secs())));
                let limit = agent_limits::detect(&rows).filter(|_| !quiet);
                let status = status_of(main.has_exited(), limit.is_some(), main.unseen_finish(), &main.activity(), launcher.is_some());
                (status, last_line(&rows), limit)
            }
        };
        Peer {
            id,
            name: self.roots.primary.read(cx).root().map(|r| ui::file_name(r).to_string()).unwrap_or_else(|| "Codeyard".to_string()),
            host: self.remote.host.is_remote().then(|| self.remote.host.label()),
            branch: self.vcs.branch.as_ref().map(ToString::to_string),
            changes: self.vcs.status.as_ref().map_or(0, |s| s.changes.len()),
            launcher: launcher.is_some(),
            terminals,
            status,
            last_line: last,
            limit,
            agent: summary,
        }
    }

    /// Every open project window, this window first. A window being updated right now cannot be read and is left out.
    fn agent_mode_peers(&self, window: &Window, cx: &mut Context<Self>) -> Vec<Peer> {
        let me = window.window_handle().window_id();
        let mut out = vec![self.agent_mode_peer(me, cx)];
        for handle in cx.windows().into_iter().filter_map(|w| w.downcast::<Workbench>()) {
            if handle.window_id() != me
                && let Ok(peer) = handle.read_with(cx, |wb, cx| wb.agent_mode_peer(handle.window_id(), cx))
            {
                out.push(peer);
            }
        }
        out
    }

    /// Opens a plain terminal in the project of window `id` and shows it.
    fn agent_mode_new_terminal(&mut self, id: gpui::WindowId, window: &mut Window, cx: &mut Context<Self>) {
        let me = window.window_handle().window_id();
        let count = if id == me {
            self.new_terminal_in(None, window, cx);
            self.agent_terminal().iter().count() + self.terminals.len()
        } else {
            let Some(handle) = cx.windows().into_iter().filter_map(|w| w.downcast::<Workbench>()).find(|h| h.window_id() == id) else { return };
            handle
                .update(cx, |wb, window, cx| {
                    wb.new_terminal_in(None, window, cx);
                    wb.agent_terminal().iter().count() + wb.terminals.len()
                })
                .unwrap_or(0)
        };
        if let Some(mode) = self.chrome.agent_mode.as_mut() {
            mode.selected = Some(id);
            mode.tab = count.saturating_sub(1);
        }
        cx.notify();
    }

    /// A button of agent mode: clicked as usual, and named for the screen reader with what pressing it does.
    fn agent_mode_button(
        &self,
        el: gpui::Stateful<Div>,
        key: impl Into<String>,
        label: impl Into<String>,
        selected: Option<bool>,
        cx: &mut Context<Self>,
        press: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<Div> {
        let mut control = Control::new(t("Agent mode"), key, accesskit::Role::Button, label);
        if let Some(on) = selected {
            control = control.selected(on);
        }
        self.pressable(el.relative(), control, cx, press)
    }

    fn agent_mode_handle(&self, id: gpui::WindowId, cx: &App) -> Option<gpui::WindowHandle<Workbench>> {
        cx.windows().into_iter().filter_map(|w| w.downcast::<Workbench>()).find(|h| h.window_id() == id)
    }

    /// This project's views are shown in the window `hub` (`None`: no longer).
    pub(super) fn hosted_by(&mut self, hub: Option<gpui::AnyWindowHandle>, cx: &mut Context<Self>) {
        if self.chrome.hub_host == hub {
            return;
        }
        let before = std::mem::replace(&mut self.chrome.hub_host, hub);
        if hub.is_none() && self.chrome.view_host == before {
            self.chrome.view_host = None;
        }
        cx.notify();
    }

    /// Input in the hub's right panel: this project's menus and pickers open in the hub's window.
    fn use_hub_window(&mut self, hub: gpui::AnyWindowHandle, cx: &mut Context<Self>) {
        if self.chrome.hub_host == Some(hub) && self.chrome.view_host != Some(hub) {
            self.chrome.editor_host = None;
            self.chrome.view_host = Some(hub);
            cx.notify();
        }
    }

    /// The hub's window closed without letting go: its menus and pickers must open here again.
    pub(super) fn drop_closed_hub(&mut self, cx: &mut Context<Self>) {
        if let Some(hub) = self.chrome.hub_host
            && !cx.windows().contains(&hub)
        {
            self.hosted_by(None, cx);
        }
    }

    /// The hub window a diff asked for here goes to (not this window's own).
    pub(super) fn agent_mode_host_of_diff(&self, window: &Window) -> Option<gpui::WindowHandle<Workbench>> {
        self.chrome.hub_host.filter(|hub| *hub != window.window_handle()).and_then(|hub| hub.downcast::<Workbench>())
    }

    /// A diff arrived from the project of window `owner`: the Diff tab shows it.
    pub(super) fn agent_mode_show_diff(&mut self, owner: gpui::WindowId, view: Entity<crate::diff::DiffView>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mode) = self.chrome.agent_mode.as_mut() else { return };
        mode.diff = Some((owner, view.clone()));
        mode.right = RightTab::Diff;
        mode.right_open = true;
        mode.selected = Some(owner);
        window.focus(&view.focus_handle(cx));
        cx.notify();
    }

    /// Stops hosting the views of project `id` (it no longer opens menus and pickers here).
    fn agent_mode_release(&mut self, id: Option<gpui::WindowId>, cx: &mut Context<Self>) {
        if let Some(handle) = id.and_then(|id| self.agent_mode_handle(id, cx)) {
            handle.update(cx, |wb, _, cx| wb.hosted_by(None, cx)).ok();
        }
    }

    /// Keeps the hosted project in step with the one shown: the one left lets go, the new one learns where to open its menus.
    fn agent_mode_host(&mut self, shown: Option<gpui::WindowId>, window: &Window, cx: &mut Context<Self>) {
        let me = window.window_handle().window_id();
        let shown = shown.filter(|id| *id != me);
        let Some(mode) = self.chrome.agent_mode.as_mut() else { return };
        if mode.hosting == shown {
            return;
        }
        let before = std::mem::replace(&mut mode.hosting, shown);
        let hub = window.window_handle();
        self.agent_mode_release(before, cx);
        if let Some(handle) = shown.and_then(|id| self.agent_mode_handle(id, cx)) {
            handle.update(cx, |wb, _, cx| wb.hosted_by(Some(hub), cx)).ok();
        }
    }

    /// One of the project's side views, drawn here though the project's window owns it. `None` when the window is gone.
    fn agent_mode_side_view(&mut self, id: gpui::WindowId, panel: Panel, window: &Window, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if id == window.window_handle().window_id() {
            return Some(self.side_view(panel, p, cx));
        }
        let handle = self.agent_mode_handle(id, cx)?;
        let p = *p;
        handle.update(cx, |wb, _, cx| wb.side_view(panel, &p, cx)).ok()
    }

    /// A project that starts waiting, hits a limit or finishes while the person is not looking at it gets a notice whose button
    /// opens it here. What a project was at the first look is not news.
    fn agent_mode_say_changes(&mut self, peers: &[Peer], active: bool) {
        let Some(mode) = self.chrome.agent_mode.as_mut() else { return };
        let shown = mode.hosting.or(mode.selected);
        let mut news = Vec::new();
        for peer in peers {
            let before = mode.seen.insert(peer.id, peer.status);
            let watching = active && Some(peer.id) == shown;
            let said = match (before, peer.status) {
                (Some(old), new) if old != new && !watching => match new {
                    Status::Attention => Some(tf("{} needs you", &[&peer.name])),
                    Status::Limit => Some(tf("{}: limit reached", &[&peer.name])),
                    Status::Done => Some(tf("{} finished", &[&peer.name])),
                    Status::Running | Status::Idle => None,
                },
                _ => None,
            };
            news.extend(said.map(|message| (peer.id, message, peer.last_line.clone())));
        }
        mode.seen.retain(|id, _| peers.iter().any(|peer| peer.id == *id));
        for (id, message, last_line) in news {
            // Out of sight, the system says it too (Notification Center); its banner brings the project up.
            if !active {
                crate::system_notifications::post(&message, &last_line, id.as_u64());
            }
            self.notify_with(Notification::new(Severity::Info, NoticeSource::Terminal, message).action(NoticeAction::AgentProject(id.as_u64())));
        }
    }

    /// A banner of Notification Center was clicked: agent mode, on the project it names.
    pub(super) fn open_clicked_banner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for project in crate::system_notifications::take_opened() {
            self.open_agent_project(gpui::WindowId::from(project), window, cx);
        }
    }

    /// The button of such a notice: agent mode, on that project's main terminal.
    pub(super) fn open_agent_project(&mut self, id: gpui::WindowId, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.agent_mode.is_none() {
            self.toggle_agent_mode(&ToggleAgentMode, window, cx);
        }
        if let Some(mode) = self.chrome.agent_mode.as_mut() {
            mode.scope = Scope::All;
            mode.selected = Some(id);
            mode.tab = 0;
        }
        if let Some(peer) = self.agent_mode_peers(window, cx).into_iter().find(|peer| peer.id == id) {
            self.agent_mode_select(&peer, 0, window, cx);
        }
        cx.notify();
    }

    /// What the windows' terminals say about accounts, noted where everything can see it: a limit message marks its account, the
    /// five-hour usage of the Claude Code accounts is read again every minute, and the Dock says how many projects need the person.
    fn agent_mode_account_state(&mut self, peers: &[Peer], window: &Window, cx: &mut Context<Self>) {
        let now = now_secs();
        for peer in peers {
            if let (Some(limit), Some(agent)) = (&peer.limit, &peer.agent) {
                cx.default_global::<Limits>().mark(agent.provider.key(), agent.profile, limit.resets.clone(), now);
            }
        }
        let needs = peers.iter().filter(|peer| peer.status.needs_you()).count();
        let active = window.is_window_active();
        self.agent_mode_say_changes(peers, active);
        let claude_dirs =
            peers.iter().filter_map(|peer| peer.agent.as_ref()).find(|agent| agent.provider == Provider::Claude).map(|agent| agent.profile_dirs.clone());
        let Some(mode) = self.chrome.agent_mode.as_mut() else { return };
        if needs != mode.badge {
            if needs > mode.badge && !active {
                crate::dock_attention::bounce();
            }
            crate::dock_attention::set_badge(needs);
            mode.badge = needs;
        }
        let due = mode.usage_task.is_none() && mode.usage_at.is_none_or(|at| at.elapsed() > Duration::from_secs(60));
        if let (true, Some(dirs)) = (due, claude_dirs) {
            let read = cx.background_executor().spawn(async move {
                let now = now_secs();
                dirs.iter()
                    .enumerate()
                    .map(|(profile, dir)| (profile as u32, agent_usage::claude_tokens(dir, now)))
                    .collect::<std::collections::HashMap<_, _>>()
            });
            mode.usage_task = Some(cx.spawn(async move |this, cx| {
                let usage = read.await;
                this.update(cx, |this, cx| {
                    if let Some(mode) = this.chrome.agent_mode.as_mut() {
                        mode.usage = usage;
                        mode.usage_at = Some(std::time::Instant::now());
                        mode.usage_task = None;
                    }
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    /// The accounts of every tool that has a session in some project, with their state.
    fn agent_mode_accounts(&self, peers: &[Peer], cx: &App) -> Vec<AccountRow> {
        let now = now_secs();
        let limits = cx.try_global::<Limits>();
        let usage = self.chrome.agent_mode.as_ref().map(|mode| &mode.usage);
        let mut rows = Vec::new();
        for provider in Provider::ALL {
            let Some(count) = peers.iter().filter_map(|peer| peer.agent.as_ref()).find(|agent| agent.provider == provider).map(|agent| agent.profile_count)
            else {
                continue;
            };
            let tokens = |profile: u32| (provider == Provider::Claude).then(|| usage.and_then(|usage| usage.get(&profile).copied())).flatten();
            let busiest = (0..count).filter_map(tokens).max().unwrap_or(0).max(1);
            // With a budget the bar is a share of it; without one, of the busiest account's use.
            let budget = (provider == Provider::Claude).then(|| cx.global::<crate::settings::Settings>().agents_claude_token_budget).filter(|b| *b > 0);
            rows.extend((0..count).map(|profile| AccountRow {
                provider,
                profile,
                limited: limits.and_then(|limits| limits.limited(provider.key(), profile, now)).cloned(),
                tokens: tokens(profile),
                share: tokens(profile).map_or(0., |tokens| tokens as f32 / budget.unwrap_or(busiest) as f32),
                percent: budget.and_then(|budget| tokens(profile).map(|tokens| (tokens as f64 / budget as f64 * 100.).round() as u32)),
            }));
        }
        rows
    }

    /// The accounts at the foot of the sidebar: whether each can be used, and how much the Claude Code ones were used lately.
    fn agent_mode_account_list(&self, accounts: &[AccountRow], p: &Palette) -> gpui::Stateful<Div> {
        let mut list = div()
            .id("agent-mode-accounts")
            .flex_none()
            .max_h(px(220.))
            .overflow_y_scroll()
            .border_t_1()
            .border_color(c(p.border))
            .px(px(theme::SPACE[4]))
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(4.));
        let mut last = None;
        for row in accounts {
            if last != Some(row.provider) {
                list = list.child(
                    div().mt(px(2.)).text_size(px(text::XS.0)).text_color(c(p.text_muted)).font_weight(gpui::FontWeight::SEMIBOLD).child(row.provider.label()),
                );
                last = Some(row.provider);
            }
            let (state, color): (SharedString, u32) = match &row.limited {
                Some(mark) => (mark.resets.clone().map_or_else(|| t("Limit reached").into(), |resets| tf("Limit reached · {}", &[&resets]).into()), p.danger),
                None => (usage_text(row), p.text_muted),
            };
            list = list.child(
                div()
                    .debug_selector({
                        let key = format!("agent-mode-account-{}-{}", row.provider.key(), row.profile);
                        move || key.clone()
                    })
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(div().flex_1().truncate().child(profile_label(row.profile)))
                            .child(div().text_size(px(text::XS.0)).text_color(c(color)).child(state)),
                    )
                    .children(row.tokens.map(|_| {
                        let fill = if row.limited.is_some() { p.danger } else { p.accent };
                        div()
                            .h(px(3.))
                            .w_full()
                            .rounded_full()
                            .bg(c(p.border))
                            .child(div().h_full().w(gpui::relative(row.share.clamp(0., 1.))).rounded_full().bg(c(fill)))
                    })),
            );
        }
        if accounts.is_empty() {
            list = list.hidden();
        }
        list
    }

    /// Shown when the project's agent hit a limit: continue on the account that is free and least used.
    fn agent_mode_limit_banner(&mut self, peer: &Peer, accounts: &[AccountRow], p: &Palette, cx: &mut Context<Self>) -> Option<gpui::Stateful<Div>> {
        let (agent, limit) = (peer.agent.as_ref()?, peer.limit.as_ref()?);
        let now = now_secs();
        let used = |profile: u32| accounts.iter().find(|row| row.provider == agent.provider && row.profile == profile).and_then(|row| row.tokens);
        let best = cx
            .try_global::<Limits>()
            .and_then(|limits| limits.best_other(agent.provider.key(), agent.profile, agent.profile_count, now, &used))
            .or_else(|| {
                // Nothing is known about the others yet: every other account is a candidate.
                (0..agent.profile_count).find(|profile| *profile != agent.profile).filter(|_| cx.try_global::<Limits>().is_none())
            });
        let current = profile_label(agent.profile);
        let reset = limit.resets.clone();
        let message = match (&reset, best) {
            (Some(resets), _) => tf("{} reached its limit; it resets {}.", &[&current, resets]),
            (None, _) => tf("{} reached its limit.", &[&current]),
        };
        let id = peer.id;
        let mut banner = div()
            .id("agent-mode-limit")
            .debug_selector(|| "agent-mode-limit".into())
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(theme::SPACE[5]))
            .py(px(6.))
            .border_t_1()
            .border_color(c(p.danger))
            .bg(crate::ui::ca(p.danger, 0.12))
            .child(div().flex_1().child(message));
        banner = match best {
            Some(profile) => banner.child({
                let label = tf("Continue with {}", &[&profile_label(profile)]);
                let button = div()
                    .id("agent-mode-continue")
                    .debug_selector(|| "agent-mode-continue".into())
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(theme::radius::MD))
                    .border_1()
                    .border_color(c(p.border))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(label.clone());
                self.agent_mode_button(button, "continue", label, None, cx, move |this, window, cx| this.agent_mode_continue_on(id, profile, window, cx))
            }),
            None => banner.child(div().text_color(c(p.text_muted)).child(t("No other account is free."))),
        };
        Some(banner)
    }

    /// Runs `f` on the workbench of project `id` with this window, so that dialogs and menus it opens belong to what the person sees.
    fn agent_mode_on_owner(
        &mut self,
        id: gpui::WindowId,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>),
    ) {
        if id == window.window_handle().window_id() {
            return f(self, window, cx);
        }
        if let Some(handle) = self.agent_mode_handle(id, cx)
            && let Ok(owner) = handle.entity(cx)
        {
            owner.update(cx, |wb, cx| f(wb, window, cx));
        }
    }

    /// The account button's choice: the agent session of the project moves to that account (asked first, as in the Agents panel).
    fn agent_mode_switch_account(&mut self, id: gpui::WindowId, profile: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_mode_on_owner(id, window, cx, move |wb, window, cx| wb.select_agent_profile(profile, window, cx));
    }

    /// The limit bar's choice: the conversation goes on, where it stopped, on the other account.
    fn agent_mode_continue_on(&mut self, id: gpui::WindowId, profile: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_mode_on_owner(id, window, cx, move |wb, window, cx| match wb.agent_summary() {
            Some(AgentSummary { private_history: true, session: Some(session), .. }) => wb.request_agent_handoff(&session, profile, window, cx),
            _ => wb.select_agent_profile(profile, window, cx),
        });
    }

    pub(super) fn mark_agent_account_limited(&mut self, _: &MarkAgentAccountLimited, _: &mut Window, cx: &mut Context<Self>) {
        self.mark_agent_account(true, cx);
    }

    pub(super) fn mark_agent_account_available(&mut self, _: &MarkAgentAccountAvailable, _: &mut Window, cx: &mut Context<Self>) {
        self.mark_agent_account(false, cx);
    }

    /// "Agents: Mark Account as Limited / Available": the account of the project shown (or of this window's own) by hand.
    pub(super) fn mark_agent_account(&mut self, limited: bool, cx: &mut Context<Self>) {
        let hosted = self.chrome.agent_mode.as_ref().and_then(|mode| mode.hosting);
        let summary = match hosted.and_then(|id| self.agent_mode_handle(id, cx)) {
            Some(handle) => handle.read_with(cx, |wb, _| wb.agent_summary()).ok().flatten(),
            None => self.agent_summary(),
        };
        let Some(agent) = summary else {
            self.notify(NoticeSource::App, Notice::Info(t("There is no agent session in this project to mark.").into()));
            return cx.notify();
        };
        let (key, now) = (agent.provider.key(), now_secs());
        if limited {
            cx.default_global::<Limits>().mark_by_hand(key, agent.profile, now);
        } else {
            cx.default_global::<Limits>().clear(key, agent.profile, now);
        }
        let account = format!("{} · {}", agent.provider.label(), profile_label(agent.profile));
        let message = if limited { tf("{} is marked as limited.", &[&account]) } else { tf("{} is marked as available.", &[&account]) };
        self.notify(NoticeSource::App, Notice::Info(message.into()));
        cx.notify();
    }

    /// The project's pull request panel, made when first asked for and read again when its list is old.
    fn agent_mode_pulls(&mut self, id: gpui::WindowId, window: &Window, cx: &mut Context<Self>) -> Option<Entity<super::pulls_panel::PullsPanel>> {
        if let Some(panel) = self.chrome.agent_mode.as_ref()?.pulls.get(&id).cloned() {
            panel.update(cx, |panel, cx| panel.reload_if_stale(cx));
            return Some(panel);
        }
        let (host, root) = if id == window.window_handle().window_id() {
            (self.remote.host.clone(), self.git_status().map(|status| status.root.clone()))
        } else {
            self.agent_mode_handle(id, cx)?.update(cx, |wb, _, _| (wb.remote.host.clone(), wb.git_status().map(|status| status.root.clone()))).ok()?
        };
        let panel = cx.new(|cx| super::pulls_panel::PullsPanel::new(host, root, std::sync::Arc::new(crate::github::SystemRunner), cx));
        cx.subscribe_in(&panel, window, move |this, _, event: &super::pulls_panel::PullsEvent, window, cx| this.agent_mode_pull_event(id, event, window, cx))
            .detach();
        self.chrome.agent_mode.as_mut()?.pulls.insert(id, panel.clone());
        Some(panel)
    }

    /// A checkout asked for in the panel runs in the project's own window (the repository is its).
    fn agent_mode_pull_event(&mut self, id: gpui::WindowId, event: &super::pulls_panel::PullsEvent, window: &Window, cx: &mut Context<Self>) {
        let super::pulls_panel::PullsEvent::Checkout { remote, pull } = event;
        let (remote, pull) = (remote.clone(), pull.clone());
        if id == window.window_handle().window_id() {
            return self.run_pull_action(remote, pull, super::pulls_glue::PullAction::Checkout, cx);
        }
        if let Some(handle) = self.agent_mode_handle(id, cx) {
            handle.update(cx, |wb, _, cx| wb.run_pull_action(remote, pull, super::pulls_glue::PullAction::Checkout, cx)).ok();
        }
    }

    /// The menu and picker layers of the hosted project, drawn in this window (where its views are).
    fn agent_mode_layers(&mut self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(id) = self.chrome.agent_mode.as_ref().and_then(|m| m.hosting) else { return Vec::new() };
        let Some(handle) = self.agent_mode_handle(id, cx) else { return Vec::new() };
        let hub = window.window_handle();
        handle
            .update(cx, |wb, _, cx| {
                if wb.chrome.view_host == Some(hub) {
                    wb.menu_layer(0., cx).into_iter().chain(wb.picker_layer(theme::SPACE[4], cx)).collect()
                } else {
                    Vec::new()
                }
            })
            .unwrap_or_default()
    }

    /// This window decides the size of the terminal it shows; the one shown before gets its own window's size back. Two windows
    /// drawing one terminal would otherwise resize it to their own areas in turn, without end.
    fn agent_mode_lead(&mut self, shown: Option<&Entity<TerminalView>>, window: &Window, cx: &mut Context<Self>) {
        let me = window.window_handle().window_id();
        let Some(mode) = self.chrome.agent_mode.as_mut() else { return };
        let unchanged = match (&mode.leading, shown) {
            (Some(weak), Some(term)) => weak.entity_id() == term.entity_id(),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return;
        }
        if let Some(old) = mode.leading.take().and_then(|weak| weak.upgrade()) {
            old.update(cx, |t, cx| t.lead_size(None, cx));
        }
        if let Some(term) = shown {
            term.update(cx, |t, cx| t.lead_size(Some(me), cx));
            mode.leading = Some(term.downgrade());
        }
    }

    fn agent_mode_select(&mut self, peer: &Peer, tab: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mode) = self.chrome.agent_mode.as_mut() {
            mode.selected = Some(peer.id);
            mode.tab = tab;
        }
        if let Some(term) = peer.terminals.get(tab) {
            term.update(cx, |t, _| {
                t.mark_seen();
            });
            window.focus(&term.focus_handle(cx));
        }
        cx.notify();
    }

    pub(super) fn agent_mode_view(&mut self, window: &mut Window, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let peers = self.agent_mode_peers(window, cx);
        let (selected, tab, scope) = self.chrome.agent_mode.as_ref().map_or((None, 0, Scope::All), |m| (m.selected, m.tab, m.scope));
        let shown = |peer: &&Peer| scope.includes(peer.host.is_some());
        let visible: Vec<&Peer> = peers.iter().filter(shown).collect();
        let current = visible.iter().position(|peer| Some(peer.id) == selected).or_else(|| (!visible.is_empty()).then_some(0));
        let current_peer = current.map(|ix| visible[ix]);
        let tab = current_peer.map_or(0, |peer| tab.min(peer.terminals.len().saturating_sub(1)));
        // Looking at the main terminal answers its attention request.
        if window.is_window_active()
            && tab == 0
            && let Some(peer) = current_peer
            && peer.status == Status::Attention
            && let Some(term) = peer.terminals.first()
        {
            term.update(cx, |t, _| {
                t.mark_seen();
            });
        }

        self.agent_mode_lead(current_peer.and_then(|peer| peer.terminals.get(tab)), window, cx);
        if let Some(mode) = self.chrome.agent_mode.as_mut() {
            // A closed project's panel (and its list) goes with it.
            mode.pulls.retain(|id, _| peers.iter().any(|peer| peer.id == *id));
        }
        self.agent_mode_account_state(&peers, window, cx);
        let accounts = self.agent_mode_accounts(&peers, cx);
        self.agent_mode_host(current_peer.map(|peer| peer.id), window, cx);
        let list = self.agent_mode_list(&visible, current_peer.map(|peer| peer.id), scope, &accounts, p, cx);
        let center = self.agent_mode_center(current_peer, tab, &accounts, p, cx);
        let right = self.agent_mode_right(current_peer, window, p, cx);
        let layers = self.agent_mode_layers(window, cx);
        div()
            .id("agent-mode")
            // The keys of agent mode (next project, the right tabs) work here and leave the editor's own keys alone.
            .key_context("AgentMode")
            .debug_selector(|| "agent-mode".into())
            .relative()
            .size_full()
            .flex()
            .bg(c(p.surface))
            .child(list)
            .child(center)
            .child(right)
            .children(layers)
            .into_any_element()
    }

    fn agent_mode_list(
        &mut self,
        visible: &[&Peer],
        current: Option<gpui::WindowId>,
        scope: Scope,
        accounts: &[AccountRow],
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let scopes = [(Scope::All, t("All")), (Scope::Local, t("Local")), (Scope::Ssh, t("SSH"))];
        let list = div().w(px(240.)).flex_none().min_h_0().flex().flex_col().border_r_1().border_color(c(p.border)).bg(c(p.surface)).child(
            div().flex().gap(px(2.)).p(px(theme::SPACE[3])).children(scopes.into_iter().enumerate().map(|(ix, (value, label))| {
                let button = div()
                    .id(("agent-mode-scope", ix))
                    .flex_1()
                    .py(px(4.))
                    .flex()
                    .justify_center()
                    .rounded(px(theme::radius::MD))
                    .cursor_pointer()
                    .text_color(c(if value == scope { p.text } else { p.text_muted }))
                    .when(value == scope, |d| d.bg(c(p.surface_active)))
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(label);
                self.agent_mode_button(button, format!("scope-{ix}"), label, Some(value == scope), cx, move |this, _, cx| {
                    if let Some(mode) = this.chrome.agent_mode.as_mut() {
                        mode.scope = value;
                    }
                    cx.notify();
                })
            })),
        );
        let mut body =
            div().id("agent-mode-list").flex_1().min_h_0().overflow_y_scroll().px(px(theme::SPACE[3])).pb(px(theme::SPACE[3])).flex().flex_col().gap(px(2.));
        if visible.is_empty() {
            body = body.child(div().p(px(8.)).text_color(c(p.text_muted)).child(t("No projects are open here.")));
        }
        let groups = grouped(visible);
        let mut row_ix = 0usize;
        for (title, members) in groups {
            body = body.child(
                div()
                    .mt(px(theme::SPACE[4]))
                    .mb(px(2.))
                    .px(px(4.))
                    .flex()
                    .gap(px(6.))
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title)
                    .child(members.len().to_string()),
            );
            for peer in members {
                let id = peer.id;
                let selected = Some(id) == current;
                let extra = peer.extra_terminals();
                // A project shown under its host only while it needs the person (the group says it otherwise); two projects of one
                // name are told apart by their host and branch.
                let twin = visible.iter().filter(|other| other.name == peer.name).count() > 1;
                let ssh_chip = if twin {
                    Some([peer.host.clone(), peer.branch.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ")).filter(|chip| !chip.is_empty())
                } else {
                    peer.host.clone().filter(|_| peer.status.needs_you())
                };
                let ix = row_ix;
                let label = format!("{}, {}", peer.name, peer.status.label());
                let row = div()
                    .id(("agent-mode-peer", ix))
                    .debug_selector(move || format!("agent-mode-peer-{ix}"))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(theme::radius::MD))
                    .cursor_pointer()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .when(selected, |d| d.bg(c(p.surface_active)))
                    .when(!selected, |d| d.hover(|s| s.bg(c(p.surface_hover))))
                    .child(div().size(px(8.)).flex_none().rounded_full().bg(c(peer.status.color(p))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .gap(px(6.))
                                    .child(div().truncate().font_weight(gpui::FontWeight::SEMIBOLD).child(peer.name.clone()))
                                    .children(ssh_chip.map(|host| div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(host))),
                            )
                            .child(div().truncate().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(if peer.last_line.is_empty() {
                                peer.status.label()
                            } else {
                                SharedString::from(peer.last_line.clone())
                            })),
                    )
                    .when(extra > 0, |d| d.child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(format!("+{extra}"))));
                body = body.child(self.agent_mode_button(row, format!("project-{ix}"), label, Some(selected), cx, move |this, window, cx| {
                    let peers = this.agent_mode_peers(window, cx);
                    if let Some(peer) = peers.iter().find(|peer| peer.id == id) {
                        this.agent_mode_select(peer, 0, window, cx);
                    }
                }));
                row_ix += 1;
            }
        }
        let close = div()
            .id("agent-mode-close")
            .px(px(theme::SPACE[5]))
            .py(px(6.))
            .border_t_1()
            .border_color(c(p.border))
            .cursor_pointer()
            .text_color(c(p.text_muted))
            .hover(|d| d.text_color(c(p.text)))
            .child(t("Back to editor"));
        let close = self.agent_mode_button(close, "close", t("Back to editor"), None, cx, |this, _, cx| this.leave_agent_mode(cx));
        list.child(body).child(self.agent_mode_account_list(accounts, p)).child(close)
    }

    fn agent_mode_center(&mut self, peer: Option<&Peer>, tab: usize, accounts: &[AccountRow], p: &Palette, cx: &mut Context<Self>) -> Div {
        let Some(peer) = peer else {
            return div().flex_1().flex().items_center().justify_center().text_color(c(p.text_muted)).child(t("No projects are open here."));
        };
        let id = peer.id;
        let mut tabs = div().h(px(34.)).flex_none().flex().items_end().gap(px(2.)).px(px(8.)).border_b_1().border_color(c(p.border));
        for (ix, term) in peer.terminals.iter().enumerate() {
            let title = term.read(cx).title.clone();
            let title = if title.is_empty() { t("Terminal").into() } else { title };
            let tab_label = title.to_string();
            let button = div()
                .id(("agent-mode-tab", ix))
                .px(px(10.))
                .py(px(6.))
                .flex()
                .gap(px(6.))
                .items_center()
                .cursor_pointer()
                .text_color(c(if ix == tab { p.text } else { p.text_muted }))
                .when(ix == tab, |d| d.border_b_2().border_color(c(p.accent)))
                .when(ix == 0, |d| d.child(div().size(px(7.)).rounded_full().bg(c(peer.status.color(p)))))
                .child(title);
            let shown = term.clone();
            let tab_button = self.agent_mode_button(button, format!("tab-{ix}"), tab_label.clone(), Some(ix == tab), cx, move |this, window, cx| {
                if let Some(mode) = this.chrome.agent_mode.as_mut() {
                    mode.tab = ix;
                }
                shown.update(cx, |t, _| {
                    t.mark_seen();
                });
                window.focus(&shown.focus_handle(cx));
                cx.notify();
            });
            // The launcher's session is ended from the Agents panel; every other tab can be closed here.
            let closable = !(ix == 0 && peer.launcher);
            let close = closable.then(|| {
                let cross = div()
                    .id(("agent-mode-tab-close", ix))
                    .px(px(4.))
                    .py(px(6.))
                    .cursor_pointer()
                    .text_color(c(p.text_muted))
                    .hover(|d| d.text_color(c(p.text)))
                    .tooltip(|_, cx| cx.new(|_| Tooltip(t("Close terminal").into())).into())
                    .child("×");
                let term = term.clone();
                self.agent_mode_button(cross, format!("tab-close-{ix}"), tf("Close {}", &[&tab_label]), None, cx, move |this, window, cx| {
                    this.agent_mode_close_terminal(id, &term, ix, window, cx)
                })
            });
            tabs = tabs.child(div().flex().items_end().child(tab_button).children(close));
        }
        let plus = div()
            .id("agent-mode-new-terminal")
            .px(px(8.))
            .py(px(6.))
            .cursor_pointer()
            .text_color(c(p.text_muted))
            .hover(|d| d.text_color(c(p.text)))
            .tooltip(|_, cx| cx.new(|_| Tooltip(t("New terminal").into())).into())
            .child("+");
        tabs = tabs.child(
            self.agent_mode_button(plus, "new-terminal", t("New terminal"), None, cx, move |this, window, cx| this.agent_mode_new_terminal(id, window, cx)),
        );
        let menu_open = self.chrome.agent_mode.as_ref().is_some_and(|m| m.account_menu);
        if let Some(agent) = &peer.agent {
            let label = format!("{} · {}", agent.provider.label(), profile_label(agent.profile));
            let button = div()
                .id("agent-mode-account")
                .debug_selector(|| "agent-mode-account".into())
                .mb(px(4.))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(theme::radius::MD))
                .border_1()
                .border_color(c(p.border))
                .cursor_pointer()
                .text_size(px(text::XS.0))
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(format!("{label} ▾"));
            tabs = tabs.child(div().flex_1()).child(self.agent_mode_button(button, "account", label, Some(menu_open), cx, |this, _, cx| {
                if let Some(mode) = this.chrome.agent_mode.as_mut() {
                    mode.account_menu = !mode.account_menu;
                }
                cx.notify();
            }));
        }
        let terminal = match peer.terminals.get(tab) {
            Some(term) => div()
                .debug_selector(|| "agent-mode-terminal".into())
                .flex_1()
                .min_h_0()
                .p(px(theme::SPACE[3]))
                .bg(c(p.bg))
                .child(term.clone())
                .into_any_element(),
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(p.text_muted))
                .child(t("No terminal yet. Press + to open one."))
                .into_any_element(),
        };
        let status_bar = div()
            .h(px(26.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(theme::SPACE[5]))
            .border_t_1()
            .border_color(c(p.border))
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .child(div().size(px(7.)).rounded_full().bg(c(peer.status.color(p))))
            .child(peer.status.label())
            .children(peer.branch.clone().map(|b| format!("· {b}")))
            .child(div().flex_1())
            .child(tn("{} terminal", "{} terminals", peer.terminals.len(), &[]));
        let banner = self.agent_mode_limit_banner(peer, accounts, p, cx);
        let menu = peer.agent.as_ref().filter(|_| menu_open).map(|agent| {
            let id = peer.id;
            div()
                .id("agent-mode-account-menu")
                .absolute()
                .top(px(34.))
                .right(px(8.))
                .w(px(260.))
                .p(px(4.))
                .rounded(px(theme::radius::MD))
                .border_1()
                .border_color(c(p.border))
                .bg(c(p.surface))
                .shadow_md()
                .occlude()
                .children((0..agent.profile_count).map(|profile| {
                    let row = accounts.iter().find(|row| row.provider == agent.provider && row.profile == profile);
                    let state: SharedString = match row.and_then(|row| row.limited.as_ref()) {
                        Some(mark) => mark.resets.clone().map_or_else(|| t("Limit reached").into(), |resets| tf("Limit reached · {}", &[&resets]).into()),
                        None => row.map_or_else(|| t("Available").into(), usage_text),
                    };
                    let row_label = format!("{}, {}", profile_label(profile), state);
                    let row = div()
                        .id(("agent-mode-account-row", profile as usize))
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(theme::radius::MD))
                        .cursor_pointer()
                        .flex()
                        .gap(px(8.))
                        .when(profile == agent.profile, |d| d.bg(c(p.surface_active)))
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(div().flex_1().child(profile_label(profile)))
                        .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(state));
                    self.agent_mode_button(row, format!("account-{profile}"), row_label, Some(profile == agent.profile), cx, move |this, window, cx| {
                        if let Some(mode) = this.chrome.agent_mode.as_mut() {
                            mode.account_menu = false;
                        }
                        this.agent_mode_switch_account(id, profile, window, cx);
                    })
                }))
        });
        div().flex_1().min_w_0().min_h_0().relative().flex().flex_col().child(tabs).child(terminal).children(banner).child(status_bar).children(menu)
    }

    /// The project's source control, diff and files, drawn by the views of the project's own window.
    fn agent_mode_right(&mut self, peer: Option<&Peer>, window: &Window, p: &Palette, cx: &mut Context<Self>) -> Div {
        let Some(peer) = peer else { return div() };
        let id = peer.id;
        let (active, open, diff) = self
            .chrome
            .agent_mode
            .as_ref()
            .map_or((RightTab::Git, true, None), |m| (m.right, m.right_open, m.diff.clone().filter(|(owner, _)| *owner == id).map(|(_, view)| view)));
        let tabs = [
            (RightTab::Git, t("Git"), "git-branch", peer.changes),
            (RightTab::Diff, t("Diff"), "git-compare", 0),
            (RightTab::Pulls, t("Pull requests"), "git-pull-request", 0),
            (RightTab::Files, t("Files"), "files", 0),
        ];
        // The rail at the edge, top to bottom: the icon of the tab shown opens it, and again closes the panel.
        let rail = div()
            .id("agent-mode-rail")
            .debug_selector(|| "agent-mode-rail".into())
            .w(px(40.))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.))
            .py(px(8.))
            .border_l_1()
            .border_color(c(p.border))
            .children(tabs.iter().enumerate().map(|(ix, (tab, label, glyph, badge))| {
                let (tab, shown, badge) = (*tab, open && *tab == active, *badge);
                let tip: SharedString = label.to_string().into();
                let button = div()
                    .id(("agent-mode-right-tab", ix))
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::radius::MD))
                    .cursor_pointer()
                    .when(shown, |d| d.bg(c(p.surface_active)))
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into())
                    .child(icon(glyph, 18., if shown { p.text } else { p.text_muted }))
                    .when(badge > 0, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(1.))
                                .right(px(0.))
                                .min_w(px(16.))
                                .h(px(16.))
                                .px(px(3.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(c(p.accent))
                                .text_color(c(p.surface))
                                .text_size(px(text::XS.0))
                                .child(badge.min(99).to_string()),
                        )
                    });
                self.agent_mode_button(button, format!("right-tab-{ix}"), *label, Some(shown), cx, move |this, _, cx| this.agent_mode_show_right(tab, cx))
            }));
        let panel = open.then(|| {
            let title = tabs.iter().find(|(tab, ..)| *tab == active).map_or_else(|| SharedString::from(""), |(_, label, ..)| label.to_string().into());
            let close = div()
                .id("agent-mode-right-close")
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::radius::MD))
                .cursor_pointer()
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(icon("x", 14., p.text_muted));
            let close = self.agent_mode_button(close, "right-close", t("Close panel"), None, cx, |this, _, cx| {
                if let Some(mode) = this.chrome.agent_mode.as_mut() {
                    mode.right_open = false;
                }
                cx.notify();
            });
            let header = div()
                .h(px(34.))
                .flex_none()
                .flex()
                .items_center()
                .justify_between()
                .px(px(12.))
                .border_b_1()
                .border_color(c(p.border))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                .child(close);
            let body = match active {
                RightTab::Git => self.agent_mode_side_view(id, Panel::Scm, window, p, cx),
                RightTab::Pulls => self.agent_mode_pulls(id, window, cx).map(IntoElement::into_any_element),
                RightTab::Files => self.agent_mode_side_view(id, Panel::Explorer, window, p, cx),
                RightTab::Diff => Some(match diff {
                    Some(view) => div().size_full().child(view).into_any_element(),
                    None => div().p(px(16.)).text_color(c(p.text_muted)).child(t("Pick a changed file in Git to see its diff here.")).into_any_element(),
                }),
            };
            let body = div()
                .id("agent-mode-right-body")
                .flex_1()
                .min_h_0()
                .relative()
                .overflow_hidden()
                // Input here makes this window the one the project's menus and pickers open in.
                .capture_any_mouse_down(cx.listener(|this, _, window, cx| this.agent_mode_use_owner(window, cx)))
                .capture_key_down(cx.listener(|this, _, window, cx| this.agent_mode_use_owner(window, cx)))
                .children(body);
            div().w(px(420.)).flex_none().min_h_0().flex().flex_col().border_l_1().border_color(c(p.border)).child(header).child(body)
        });
        div().flex_none().min_h_0().flex().children(panel).child(rail)
    }

    /// The tab `tab` of the right panel: shown (the panel opens), or, when it is the one shown, the panel closes.
    fn agent_mode_show_right(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        if let Some(mode) = self.chrome.agent_mode.as_mut() {
            mode.right_open = !(mode.right_open && mode.right == tab);
            mode.right = tab;
        }
        cx.notify();
    }

    pub(super) fn agent_mode_show_git(&mut self, _: &AgentModeShowGit, _: &mut Window, cx: &mut Context<Self>) {
        self.agent_mode_show_right(RightTab::Git, cx);
    }

    pub(super) fn agent_mode_show_diff_tab(&mut self, _: &AgentModeShowDiff, _: &mut Window, cx: &mut Context<Self>) {
        self.agent_mode_show_right(RightTab::Diff, cx);
    }

    pub(super) fn agent_mode_show_pulls(&mut self, _: &AgentModeShowPulls, _: &mut Window, cx: &mut Context<Self>) {
        self.agent_mode_show_right(RightTab::Pulls, cx);
    }

    pub(super) fn agent_mode_show_files(&mut self, _: &AgentModeShowFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.agent_mode_show_right(RightTab::Files, cx);
    }

    fn agent_mode_use_owner(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(id) = self.chrome.agent_mode.as_ref().and_then(|m| m.hosting) else { return };
        let hub = window.window_handle();
        if let Some(handle) = self.agent_mode_handle(id, cx) {
            handle.update(cx, |wb, _, cx| wb.use_hub_window(hub, cx)).ok();
        }
    }
}

#[cfg(test)]
#[path = "workbench_agent_mode_tests.rs"]
mod tests;
