//! Native agent terminals and account preferences in the Agents panel.

use std::collections::HashSet;
use std::fs::File;
use std::path::PathBuf;

use gpui::{Div, FontWeight, Stateful};

#[path = "workbench_agent_library.rs"]
mod library;

use super::antigravity_accounts::{Accounts, Login};
use super::claude_store::{HistoryMode, Project, Provider, Session, new_session_id};
use super::*;
use crate::{
    a11y_controls::Control,
    i18n::{t, tf},
    terminal::Launch,
};

const PROFILE_BUTTON: &str = "agent-account-picker";
const HISTORY_BUTTON: &str = "agent-history-mode";
const PROVIDER_BUTTON: &str = "agent-provider";
const MAX_PROFILES: u32 = 64;

#[derive(Default)]
pub(super) struct AgentPanel {
    requested_folder: Option<PathBuf>,
    chosen_folder: Option<PathBuf>,
    project: Option<Project>,
    provider: Provider,
    profile: u32,
    profile_count: u32,
    mode: HistoryMode,
    sessions: Vec<Session>,
    recoverable: Vec<Session>,
    selected: Option<String>,
    scan: u64,
    loading: bool,
    rescan_pending: bool,
    installed: Option<Vec<Provider>>,
    checking_cli: bool,
    terminal: Option<Entity<TerminalView>>,
    running: Option<Running>,
    stopping: bool,
    switch_to: Option<u32>,
    message: Option<String>,
    library: library::Library,
    transferring: bool,
    handoff: Option<library::Handoff>,
}

struct Running {
    _lock: File,
    _observer: Subscription,
    _history: Option<gpui::Task<()>>,
    conversation: bool,
    known_sessions: Option<HashSet<String>>,
    antigravity: Option<AntigravityRun>,
}

enum AntigravityRun {
    Session(Accounts, u32),
    Login(Login),
}

struct AgentLaunch {
    project: Project,
    executable: PathBuf,
    lock: File,
    start: Start,
    antigravity: Option<AntigravityRun>,
}

enum Start {
    SignIn,
    AddAccount,
    New,
    Resume(String),
    Recover(String),
}

fn profile_label(profile: u32) -> String {
    match profile {
        0 => t("Personal").to_owned(),
        1 => t("Work").to_owned(),
        _ => tf("Account {}", &[&(profile + 1)]),
    }
}

fn mode_label(mode: HistoryMode) -> &'static str {
    match mode {
        HistoryMode::Private => t("Private sessions"),
        HistoryMode::Shared => t("Shared sessions"),
    }
}

fn panel_button(id: impl Into<gpui::ElementId>, p: &Palette) -> Stateful<Div> {
    div()
        .id(id)
        .min_h(px(30.))
        .px(px(theme::SPACE[3]))
        .flex()
        .items_center()
        .gap(px(theme::SPACE[2]))
        .cursor_pointer()
        .tab_index(0)
        .border_1()
        .border_color(c(p.border))
        .rounded(px(theme::radius::MD))
        .hover(|d| d.bg(c(p.surface_hover)))
        .focus(|d| d.border_color(c(p.focus_ring)))
}

fn profile_key(project: &Project) -> String {
    if project.provider == Provider::Antigravity { "agents.antigravity.profile".into() } else { format!("{}.profile", project.state_key()) }
}

fn profile_count_key(provider: Provider) -> String {
    format!("agents.{}.accounts", provider.key())
}

impl Workbench {
    /// Check local executable paths without starting any agent processes.
    pub(super) fn detect_agent_clis(&mut self, cx: &mut Context<Self>) {
        if self.chrome.agents.checking_cli {
            return;
        }
        self.chrome.agents.checking_cli = true;
        let scan = cx.background_executor().spawn(async move { Provider::ALL.into_iter().filter(|provider| provider.executable().is_some()).collect() });
        cx.spawn(async move |this, cx| {
            let installed = scan.await;
            this.update(cx, |this, cx| {
                let state = &mut this.chrome.agents;
                state.checking_cli = false;
                if state.installed.as_ref() != Some(&installed) {
                    state.installed = Some(installed);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Open the Agents tab and choose the current workspace unless a project was explicitly picked.
    pub(super) fn open_agents(&mut self, cx: &mut Context<Self>) {
        self.chrome.bottom = Bottom::Agents;
        self.chrome.panel_open = true;
        self.ensure_agent_search(cx);
        self.detect_agent_clis(cx);
        if self.chrome.agents.running.is_none() && !self.chrome.agents.transferring {
            self.load_agent_project(cx);
        }
        cx.notify();
    }

    fn agent_folder(&self, cx: &App) -> Option<PathBuf> {
        if self.remote.host.is_remote() {
            return None;
        }
        self.chrome.agents.chosen_folder.clone().or_else(|| self.active_root(cx).map(PathBuf::from))
    }

    pub(super) fn sync_agent_project(&mut self, cx: &mut Context<Self>) {
        if self.chrome.panel_open
            && self.chrome.bottom == Bottom::Agents
            && self.chrome.agents.running.is_none()
            && !self.chrome.agents.transferring
            && self.chrome.agents.requested_folder != self.agent_folder(cx)
        {
            self.load_agent_project(cx);
        }
    }

    pub(super) fn agent_terminal(&self) -> Option<Entity<TerminalView>> {
        self.chrome.agents.terminal.clone()
    }

    pub(super) fn select_agent_provider(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if self.chrome.agents.running.is_some() || self.chrome.agents.transferring || self.chrome.agents.provider == provider {
            return;
        }
        self.chrome.agents.provider = provider;
        self.load_agent_project(cx);
    }

    fn load_agent_project(&mut self, cx: &mut Context<Self>) {
        let folder = self.agent_folder(cx);
        self.chrome.agents.requested_folder.clone_from(&folder);
        let Some(data) = self.persist.data_dir.as_deref() else {
            let state = &mut self.chrome.agents;
            state.scan = state.scan.wrapping_add(1);
            state.loading = false;
            state.project = None;
            state.sessions.clear();
            state.recoverable.clear();
            state.message = Some(t("Codeyard local storage is unavailable.").to_owned());
            cx.notify();
            return;
        };
        let provider = self.chrome.agents.provider;
        let project = match folder.as_deref().map(|folder| Project::new(data, folder, provider)).transpose() {
            Ok(project) => project,
            Err(error) => {
                let state = &mut self.chrome.agents;
                state.scan = state.scan.wrapping_add(1);
                state.loading = false;
                state.project = None;
                state.sessions.clear();
                state.recoverable.clear();
                state.message = Some(tf("Could not open the {0} project: {1}", &[&provider.label(), &error]));
                cx.notify();
                return;
            }
        };
        let state = &mut self.chrome.agents;
        if state.project != project {
            state.terminal = None;
            state.selected = None;
            state.sessions.clear();
            state.recoverable.clear();
        }
        state.project = project;
        state.message = None;
        let persistence = self.persist.state.as_ref();
        state.profile_count = if provider.uses_local_profiles() {
            let default = if provider == Provider::Antigravity { 1 } else { 2 };
            persistence.and_then(|db| db.get(&profile_count_key(provider))).and_then(|n| n.parse().ok()).unwrap_or(default).clamp(default, MAX_PROFILES)
        } else {
            1
        };
        if let Some(project) = &state.project {
            let key = project.state_key();
            state.mode = if state.provider == Provider::Claude && persistence.and_then(|db| db.get(&format!("{key}.history"))).as_deref() == Some("shared") {
                HistoryMode::Shared
            } else {
                HistoryMode::Private
            };
            state.profile =
                persistence.and_then(|db| db.get(&profile_key(project))).and_then(|p| p.parse().ok()).filter(|p| *p < state.profile_count).unwrap_or(0);
        }
        self.load_agent_pins();
        self.refresh_agent_sessions(cx);
    }

    fn choose_agent_project(&mut self, cx: &mut Context<Self>) {
        if self.chrome.agents.running.is_some() || self.chrome.agents.transferring {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some(t("Choose project…").into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    if this.chrome.agents.running.is_none() && !this.chrome.agents.transferring {
                        this.chrome.agents.chosen_folder = Some(path);
                        this.load_agent_project(cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn refresh_agent_sessions(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.chrome.agents;
        state.scan = state.scan.wrapping_add(1);
        state.rescan_pending = false;
        let scan = state.scan;
        let Some(project) = state.project.clone() else {
            state.loading = false;
            cx.notify();
            return;
        };
        state.loading = true;
        let (profile, mode) = (state.profile, state.mode);
        let load = cx
            .background_executor()
            .spawn(async move { Ok::<_, std::io::Error>((project.sessions(profile, mode)?, project.recoverable_sessions(profile, mode)?)) });
        cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                let state = &mut this.chrome.agents;
                if state.scan != scan {
                    return;
                }
                state.loading = false;
                match result {
                    Ok((sessions, recoverable)) => {
                        if let Some(known) = state.running.as_mut().and_then(|run| run.known_sessions.as_mut())
                            && let Some(newest) = sessions.iter().find(|session| !known.contains(&session.id))
                        {
                            state.selected = Some(newest.id.clone());
                            if let Some(run) = &mut state.running {
                                run.known_sessions = None;
                            }
                        }
                        state.sessions = sessions;
                        state.recoverable = recoverable;
                    }
                    Err(error) => state.message = Some(tf("Could not read {0} sessions: {1}", &[&state.provider.label(), &error])),
                }
                if state.rescan_pending {
                    this.refresh_agent_sessions(cx);
                } else {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn watch_agent_history(project: &Project, profile: u32, mode: HistoryMode, session: Option<&str>, cx: &mut Context<Self>) -> gpui::Task<()> {
        let project = project.clone();
        let session = session.map(str::to_owned);
        cx.spawn(async move |this, cx| {
            let watcher = Arc::new(agent::watch::Watcher::default());
            let watch = watcher.clone();
            let target = project.clone();
            let current = session.clone();
            cx.background_executor()
                .spawn(async move {
                    watch.set(target.watch_paths(profile, mode, current.as_deref()));
                })
                .await;
            // The CLI can persist a session before the watcher establishes its baseline.
            if this.update(cx, |this, cx| this.refresh_agent_sessions(cx)).is_err() {
                return;
            }
            loop {
                cx.background_executor().timer(agent::watch::INTERVAL).await;
                let watch = watcher.clone();
                let target = project.clone();
                let changed = cx
                    .background_executor()
                    .spawn(async move {
                        let changed = !watch.poll().is_empty();
                        if changed && target.provider != Provider::Claude {
                            watch.set(target.watch_paths(profile, mode, None));
                        }
                        changed
                    })
                    .await;
                if changed
                    && this
                        .update(cx, |this, cx| {
                            if this.chrome.agents.loading {
                                this.chrome.agents.rescan_pending = true;
                            } else {
                                this.refresh_agent_sessions(cx);
                            }
                        })
                        .is_err()
                {
                    return;
                }
            }
        })
    }

    fn save_agent_value(&mut self, key: &str, value: &str, cx: &mut Context<Self>) -> bool {
        let Some(db) = &self.persist.state else {
            self.agent_problem(t("Codeyard local storage is unavailable.").to_owned(), cx);
            return false;
        };
        if let Err(error) = db.set(key, value) {
            self.agent_problem(tf("Could not save agent settings: {}", &[&error]), cx);
            return false;
        }
        true
    }

    pub(super) fn select_agent_profile(&mut self, profile: u32, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.chrome.agents;
        if profile >= state.profile_count || profile == state.profile || state.stopping || state.transferring {
            return;
        }
        if state.running.is_none() {
            self.change_agent_profile(profile, cx);
            return;
        }
        let terminal = state.terminal.as_ref().map(Entity::entity_id);
        let detail = if state.provider == Provider::Antigravity {
            t("The running terminal will stop. Antigravity will use the selected Google account for its next session.")
        } else if state.mode == HistoryMode::Shared {
            t("The running terminal will stop. Shared conversations can continue with the selected account.")
        } else {
            t("The running terminal will stop. This conversation stays with its current account.")
        };
        let answer = crate::question::Question::new(gpui::PromptLevel::Warning, tf("Switch {} account?", &[&state.provider.label()]))
            .detail(detail)
            .button(t("Switch account"))
            .cancel(t("Cancel"))
            .ask(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Some(0) {
                return;
            }
            this.update_in(cx, |this, _, cx| {
                if this.chrome.agents.terminal.as_ref().map(Entity::entity_id) != terminal {
                    return;
                }
                if this.chrome.agents.running.is_some() {
                    this.chrome.agents.switch_to = Some(profile);
                    this.stop_agent_terminal(cx);
                } else {
                    this.change_agent_profile(profile, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn change_agent_profile(&mut self, profile: u32, cx: &mut Context<Self>) {
        let state = &mut self.chrome.agents;
        let Some(project) = state.project.clone() else { return };
        if project.provider != Provider::Antigravity {
            self.apply_agent_profile(profile, cx);
            return;
        }
        state.transferring = true;
        let count = state.profile_count;
        let selected_project = project.clone();
        let operation = cx.background_executor().spawn(async move {
            let _lock = project.lock()?;
            project.antigravity_accounts().activate(profile, count)
        });
        cx.spawn(async move |this, cx| {
            let result = operation.await;
            this.update(cx, |this, cx| {
                let state = &mut this.chrome.agents;
                if !state.transferring || state.project.as_ref() != Some(&selected_project) {
                    return;
                }
                state.transferring = false;
                match result {
                    Ok(()) => {
                        this.apply_agent_profile(profile, cx);
                    }
                    Err(error) => this.agent_problem(tf("Could not switch the Antigravity account: {}", &[&error]), cx),
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn apply_agent_profile(&mut self, profile: u32, cx: &mut Context<Self>) -> bool {
        let Some(project) = &self.chrome.agents.project else { return false };
        if !self.save_agent_value(&profile_key(project), &profile.to_string(), cx) {
            return false;
        }
        let state = &mut self.chrome.agents;
        state.profile = profile;
        state.recoverable.clear();
        state.terminal = None;
        state.message = None;
        if state.mode == HistoryMode::Private {
            state.selected = None;
            state.sessions.clear();
            state.recoverable.clear();
        }
        self.load_agent_pins();
        self.refresh_agent_sessions(cx);
        true
    }

    pub(super) fn select_agent_history(&mut self, mode: HistoryMode, cx: &mut Context<Self>) {
        let state = &self.chrome.agents;
        if state.provider != Provider::Claude || state.running.is_some() || state.transferring || state.mode == mode {
            return;
        }
        let Some(project) = &state.project else { return };
        if let Err(error) = project.prepare(state.profile, mode) {
            self.agent_problem(tf("Could not prepare agent storage: {}", &[&error]), cx);
            return;
        }
        if !self.save_agent_value(&format!("{}.history", project.state_key()), mode.key(), cx) {
            return;
        }
        let state = &mut self.chrome.agents;
        state.mode = mode;
        state.sessions.clear();
        state.recoverable.clear();
        state.selected = None;
        state.terminal = None;
        state.message = None;
        self.load_agent_pins();
        self.refresh_agent_sessions(cx);
    }

    pub(super) fn add_agent_account_from_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.agents.provider == Provider::Antigravity {
            if self.chrome.agents.provider.uses_local_profiles() && self.chrome.agents.profile_count < MAX_PROFILES {
                self.start_agent(Start::AddAccount, window, cx);
            }
        } else {
            self.add_agent_account(cx);
        }
    }

    pub(super) fn add_agent_account(&mut self, cx: &mut Context<Self>) {
        let state = &self.chrome.agents;
        if state.provider == Provider::Antigravity || state.running.is_some() || state.transferring || state.profile_count >= MAX_PROFILES {
            return;
        }
        let profile = state.profile_count;
        if self.save_agent_value(&profile_count_key(state.provider), &(profile + 1).to_string(), cx) {
            self.chrome.agents.profile_count = profile + 1;
            self.apply_agent_profile(profile, cx);
        }
    }

    fn agent_problem(&mut self, message: String, cx: &mut Context<Self>) {
        self.chrome.agents.message = Some(message);
        cx.notify();
    }

    pub(super) fn sign_in_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_agent(Start::SignIn, window, cx);
    }

    fn start_agent(&mut self, start: Start, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.chrome.agents;
        if state.running.is_some()
            || state.transferring
            || self.remote.host.is_remote()
            || (state.provider != Provider::Claude && state.loading && matches!(start, Start::New))
        {
            return;
        }
        let Some(project) = state.project.clone() else { return };
        let provider = project.provider;
        let Some(executable) = provider.executable() else {
            self.agent_problem(tf("{} is not installed or could not be found on this computer.", &[&provider.label()]), cx);
            return;
        };
        let profile = state.profile;
        let count = state.profile_count;
        let lock = match project.lock() {
            Ok(lock) => lock,
            Err(error) => {
                self.agent_problem(tf("Could not open the {0} terminal: {1}", &[&provider.label(), &error]), cx);
                return;
            }
        };
        let mut launch = AgentLaunch { project, executable, lock, start, antigravity: None };
        if provider != Provider::Antigravity || !provider.uses_local_profiles() {
            self.launch_agent(launch, window, cx);
            return;
        }

        self.chrome.agents.transferring = true;
        let operation = cx.background_executor().spawn(async move {
            let accounts = launch.project.antigravity_accounts();
            let run = match launch.start {
                Start::SignIn => accounts.begin_login(profile, profile, count).map(AntigravityRun::Login),
                Start::AddAccount => accounts.begin_login(profile, count, count).map(AntigravityRun::Login),
                _ => accounts.activate(profile, count).map(|()| AntigravityRun::Session(accounts, profile)),
            }?;
            launch.antigravity = Some(run);
            Ok::<_, std::io::Error>(launch)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = operation.await;
            this.update_in(cx, |this, window, cx| {
                let state = &mut this.chrome.agents;
                if !state.transferring || state.provider != Provider::Antigravity {
                    return;
                }
                state.transferring = false;
                match result {
                    Ok(launch) if state.project.as_ref() == Some(&launch.project) => this.launch_agent(launch, window, cx),
                    Ok(_) => cx.notify(),
                    Err(error) => this.agent_problem(tf("Could not open the Antigravity account: {}", &[&error]), cx),
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn launch_agent(&mut self, launch: AgentLaunch, window: &mut Window, cx: &mut Context<Self>) {
        let AgentLaunch { project, executable, lock, start, antigravity } = launch;
        let state = &self.chrome.agents;
        let provider = project.provider;
        let (profile, mode) = (state.profile, state.mode);
        let mut env = match project.prepare(profile, mode) {
            Ok(env) => env,
            Err(error) => {
                self.agent_problem(tf("Could not prepare agent storage: {}", &[&error]), cx);
                return;
            }
        };
        // Inherited credentials would take priority over the chosen account.
        env.extend(provider.credentials_env().iter().map(|key| ((*key).to_owned(), String::new())));
        let start = if let Start::Recover(id) = start {
            if let Err(error) = project.recover_session(profile, mode, &id) {
                self.agent_problem(tf("Could not recover the Claude Code session: {}", &[&error]), cx);
                return;
            }
            Start::Resume(id)
        } else {
            start
        };
        let conversation = !matches!(start, Start::SignIn | Start::AddAccount);
        let known_sessions = (provider != Provider::Claude && conversation && !matches!(start, Start::Resume(_)))
            .then(|| state.sessions.iter().map(|session| session.id.clone()).collect());
        let (args, selected) = if provider != Provider::Claude {
            let mut args = Vec::new();
            if provider == Provider::Codex {
                args.push("--no-daemon".to_owned());
            }
            let selected = match start {
                Start::SignIn | Start::AddAccount => {
                    if provider == Provider::Codex {
                        args.push("login".into());
                    } else {
                        args.extend(["acp".to_owned(), "--login".to_owned()]);
                    }
                    state.selected.clone()
                }
                Start::New => None,
                Start::Resume(id) if state.sessions.iter().any(|session| session.id == id) => {
                    let resume = if provider == Provider::Codex { "resume" } else { "--conversation" };
                    args.extend([resume.to_owned(), id.clone()]);
                    Some(id)
                }
                _ => {
                    self.agent_problem(t("This session is unavailable for the selected account and project.").to_owned(), cx);
                    self.refresh_agent_sessions(cx);
                    return;
                }
            };
            (args, selected)
        } else {
            let mut args = Vec::new();
            let selected = match start {
                Start::SignIn => {
                    args.extend(["auth".to_owned(), "login".to_owned()]);
                    state.selected.clone()
                }
                command => {
                    args.extend(["--settings".to_owned(), project.settings(profile, mode)]);
                    let session = match command {
                        Start::New => match new_session_id() {
                            Ok(id) => {
                                args.extend(["--session-id".to_owned(), id.clone()]);
                                id
                            }
                            Err(_) => {
                                self.agent_problem(t("Could not create a Claude Code session ID.").to_owned(), cx);
                                return;
                            }
                        },
                        Start::Resume(id) => {
                            let Some(path) = project.session_path(profile, mode, &id) else {
                                self.agent_problem(t("This session is unavailable for the selected account and project.").to_owned(), cx);
                                self.refresh_agent_sessions(cx);
                                return;
                            };
                            args.extend(["--resume".to_owned(), ui::local_path(&path)]);
                            id
                        }
                        _ => return,
                    };
                    Some(session)
                }
            };
            (args, selected)
        };
        let launch = Launch { shell: Some(executable.to_string_lossy().into_owned()), args: Some(args), env, color: None };
        let cwd = ui::local_path(&project.folder);
        let host = self.remote.host.clone();
        let terminal = cx.new(|cx| TerminalView::new_with(host, Some(cwd), launch, cx));
        let observer = cx.observe_in(&terminal, window, |this, terminal, window, cx| {
            let view = terminal.read(cx);
            if view.has_exited() || view.launch_failed() {
                this.agent_terminal_finished(view.exit_code() == Some(0), window, cx);
            } else if this.chrome.agents.stopping && view.pty_handle().is_some() {
                view.kill();
            }
        });
        let history = conversation.then(|| Self::watch_agent_history(&project, profile, mode, selected.as_deref(), cx));
        let state = &mut self.chrome.agents;
        state.selected = selected;
        state.message = None;
        state.running = Some(Running { _lock: lock, _observer: observer, _history: history, conversation, known_sessions, antigravity });
        state.terminal = Some(terminal.clone());
        window.focus(&terminal.focus_handle(cx));
        cx.notify();
    }

    fn agent_terminal_finished(&mut self, success: bool, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.chrome.agents;
        let Some(running) = state.running.take() else { return };
        let conversation = running.conversation;
        state.stopping = false;
        if running.antigravity.is_some() {
            self.finish_antigravity(running, success, cx);
            return;
        }
        drop(running);
        if let Some(handoff) = state.handoff.take() {
            self.begin_agent_handoff(handoff, window, cx);
            return;
        }
        let Some(profile) = state.switch_to.take() else {
            self.refresh_agent_sessions(cx);
            return;
        };
        let resume = state.selected.clone().filter(|id| {
            conversation && state.mode == HistoryMode::Shared && state.project.as_ref().is_some_and(|project| project.has_session(profile, state.mode, id))
        });
        if self.apply_agent_profile(profile, cx)
            && let Some(id) = resume
        {
            self.start_agent(Start::Resume(id), window, cx);
        }
    }

    fn finish_antigravity(&mut self, mut running: Running, success: bool, cx: &mut Context<Self>) {
        let Some(account) = running.antigravity.take() else { return };
        let lock = running._lock;
        let state = &mut self.chrome.agents;
        state.transferring = true;
        let project = state.project.clone();
        let switch_to = state.switch_to.take();
        let operation = cx.background_executor().spawn(async move {
            let _lock = lock;
            match account {
                AntigravityRun::Login(login) => login.finish(success),
                AntigravityRun::Session(accounts, profile) => accounts.refresh(profile).map(|()| None),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = operation.await;
            this.update(cx, |this, cx| {
                let state = &mut this.chrome.agents;
                if state.project != project || !state.transferring {
                    return;
                }
                state.transferring = false;
                match result {
                    Ok(Some(profile)) => {
                        if profile == state.profile_count {
                            let count = profile + 1;
                            if !this.save_agent_value(&profile_count_key(Provider::Antigravity), &count.to_string(), cx) {
                                return;
                            }
                            this.chrome.agents.profile_count = count;
                        }
                        this.apply_agent_profile(profile, cx);
                    }
                    Ok(None) => {
                        if let Some(profile) = switch_to {
                            this.change_agent_profile(profile, cx);
                        } else {
                            this.refresh_agent_sessions(cx);
                        }
                    }
                    Err(error) => this.agent_problem(tf("Could not save the Antigravity account: {}", &[&error]), cx),
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn recover_agent_session(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.chrome.agents;
        if state.mode == HistoryMode::Private {
            self.start_agent(Start::Recover(id), window, cx);
            return;
        }
        let project = state.project.clone();
        let profile = state.profile;
        let answer = crate::question::Question::new(gpui::PromptLevel::Info, t("Recover this session into shared history?"))
            .detail(t("Other Claude Code accounts will be able to open this conversation for this project."))
            .button(t("Recover session"))
            .cancel(t("Cancel"))
            .ask(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Some(0) {
                this.update_in(cx, |this, window, cx| {
                    let state = &this.chrome.agents;
                    if state.project == project && state.profile == profile && state.mode == HistoryMode::Shared {
                        this.start_agent(Start::Recover(id), window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn stop_agent_terminal(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.chrome.agents;
        if state.running.is_some()
            && let Some(terminal) = &state.terminal
        {
            state.stopping = true;
            if terminal.read(cx).pty_handle().is_some() {
                terminal.read(cx).kill();
            }
            cx.notify();
        }
    }

    pub(super) fn close_agent_terminal(&mut self, cx: &App) {
        let state = &mut self.chrome.agents;
        if let Some(terminal) = state.terminal.take() {
            terminal.read(cx).kill();
        }
        state.running = None;
        state.stopping = false;
        state.switch_to = None;
        state.handoff = None;
        state.transferring = false;
    }

    fn open_agent_menu(&mut self, key: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.chrome.agents;
        if state.stopping
            || state.transferring
            || self.remote.host.is_remote()
            || (state.running.is_some() && key != PROFILE_BUTTON)
            || (state.project.is_none() && key != PROVIDER_BUTTON)
        {
            return;
        }
        let (items, commands) = if key == PROVIDER_BUTTON {
            let items = Provider::ALL.map(|provider| {
                let label = if state.installed.as_ref().is_some_and(|installed| !installed.contains(&provider)) {
                    tf("{} is not installed", &[&provider.label()])
                } else {
                    provider.label().to_owned()
                };
                MenuItem::new(&label).checked(provider == state.provider)
            });
            (items.into(), Provider::ALL.map(MenuCmd::AgentProvider).into())
        } else if key == PROFILE_BUTTON {
            let mut items: Vec<_> = (0..state.profile_count).map(|id| MenuItem::new(&profile_label(id)).checked(id == state.profile)).collect();
            let mut commands: Vec<_> = (0..state.profile_count).map(MenuCmd::AgentProfile).collect();
            if !state.provider.uses_local_profiles() {
                return;
            }
            let can_sign_in = state.running.is_none();
            items.push(MenuItem::new(t("Sign in to this account…")).enabled(can_sign_in).separator());
            commands.push(MenuCmd::AgentSignIn);
            if state.profile_count < MAX_PROFILES {
                items.push(MenuItem::new(t("Add account")).enabled(state.running.is_none()));
                commands.push(MenuCmd::AgentAddAccount);
            }
            (items, commands)
        } else {
            let modes = [HistoryMode::Private, HistoryMode::Shared];
            (modes.map(|mode| MenuItem::new(mode_label(mode)).checked(mode == state.mode)).into(), modes.map(MenuCmd::AgentHistory).into())
        };
        let position = crate::a11y_controls::bounds_of(cx, window, t("Agents"), key).map(|b| b.bottom_right()).unwrap_or_else(|| window.mouse_position());
        self.show_menu(items, commands, Vec::new(), MenuTarget { path: String::new(), is_dir: false, tab: None }, position, window, cx);
        if let Some(menu) = &self.menu {
            menu.menu.update(cx, |menu, _| menu.set_anchor(gpui::Corner::TopRight));
        }
    }

    fn agent_button(
        &self,
        id: &'static str,
        label: String,
        glyph: &'static str,
        menu: bool,
        p: &Palette,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        self.pressable(
            panel_button(id, p)
                .debug_selector(move || id.into())
                .text_color(c(p.text))
                .font_weight(FontWeight::MEDIUM)
                .child(icon(glyph, 14., p.text_muted))
                .child(div().truncate().child(label.clone()))
                .when(menu, |d| d.child(icon("chevron-down", 12., p.text_muted))),
            Control::new(t("Agents"), id, accesskit::Role::Button, label),
            cx,
            run,
        )
    }

    pub(super) fn agent_panel_view(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.chrome.agents;
        let busy = state.running.is_some() || state.transferring;
        let pending = state.running.as_ref().is_some_and(|run| run.conversation)
            && state.selected.as_ref().is_none_or(|id| !state.sessions.iter().any(|session| session.id == *id));
        let available = state.project.is_some() && !self.remote.host.is_remote();
        let compact = self.chrome.panel_area.is_some_and(|area| {
            let width = area.size.width / px(1.);
            let width = if self.chrome.panel_dock.beside() && !self.chrome.panel_max { width.min(self.chrome.panel_w) } else { width };
            width < 680.
        });
        let identities = || div().flex().flex_wrap().gap(px(theme::SPACE[2]));
        let bars = identities()
            .when(state.running.is_some() && !state.stopping, |d| {
                d.child(self.agent_button("agent-stop", t("Stop terminal").into(), "circle-slash-2", false, p, cx, |this, _, cx| this.stop_agent_terminal(cx)))
            })
            .when(state.stopping, |d| d.child(tf("Stopping {}…", &[&state.provider.label()])))
            .when(state.transferring, |d| d.child(t("Preparing the session on the selected account…")))
            .when(available && !busy && state.provider == Provider::Claude, |d| {
                d.child(self.agent_button(HISTORY_BUTTON, mode_label(state.mode).into(), "history", true, p, cx, |this, w, cx| {
                    this.open_agent_menu(HISTORY_BUTTON, w, cx)
                }))
            })
            .when(available && !busy && state.provider == Provider::Codex, |d| {
                d.child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(t("Private sessions")))
            })
            .when(available && !state.stopping && !state.transferring && state.provider.uses_local_profiles(), |d| {
                d.child(self.agent_button(PROFILE_BUTTON, profile_label(state.profile), "key-round", true, p, cx, |this, w, cx| {
                    this.open_agent_menu(PROFILE_BUTTON, w, cx)
                }))
            })
            .when(available && !state.provider.uses_local_profiles(), |d| {
                d.child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(t("System account")))
            });

        let mut sessions = div()
            .id("agent-sessions")
            .flex_none()
            .min_w_0()
            .flex()
            .gap(px(theme::SPACE[2]))
            .p(px(theme::SPACE[3]))
            .bg(c(p.surface))
            .border_color(c(p.border))
            .when(compact, |d| d.items_center().border_b_1().overflow_x_scroll())
            .when(!compact, |d| d.w(px(248.)).flex_col().border_r_1().overflow_y_scroll());
        if !busy && !self.remote.host.is_remote() {
            sessions = sessions.child(self.agent_button(PROVIDER_BUTTON, state.provider.label().into(), "terminal", true, p, cx, |this, window, cx| {
                this.open_agent_menu(PROVIDER_BUTTON, window, cx)
            }));
        }
        if !busy && !self.remote.host.is_remote() {
            let folder_label = state
                .project
                .as_ref()
                .and_then(|project| project.folder.file_name())
                .map_or_else(|| t("Choose project…").to_owned(), |name| name.to_string_lossy().into_owned());
            sessions =
                sessions.child(self.agent_button("agent-project", folder_label, "folder-open-solid", true, p, cx, |this, _, cx| this.choose_agent_project(cx)));
        }
        if !busy && available && (state.provider == Provider::Claude || !state.loading) {
            sessions = sessions.child(
                self.agent_button("agent-new-session", t("New session").into(), "plus", false, p, cx, |this, w, cx| this.start_agent(Start::New, w, cx)),
            );
        }
        if available && let Some(field) = &state.library.field {
            sessions = sessions.child(
                ui::input_box("agent-search", p, p.bg, false).when(compact, |box_| box_.flex_none().w(px(160.))).child(ui::search_icon(p)).child(field.clone()),
            );
        }
        if pending {
            sessions = sessions.child(
                div()
                    .id("agent-session-pending")
                    .relative()
                    .min_h(px(30.))
                    .px(px(theme::SPACE[3]))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .rounded(px(theme::radius::MD))
                    .bg(c(p.surface_active))
                    .child(icon("history", 13., p.accent))
                    .child(t("New session"))
                    .child(crate::a11y_controls::said(t("Agents"), "agent-session-pending", accesskit::Role::Label, t("New session"))),
            );
        }
        let (mut pinned, mut recent) = (Vec::new(), Vec::new());
        for session in &state.sessions {
            if !state.library.matches(session) {
                continue;
            }
            if state.library.pins.contains(&session.id) {
                pinned.push(session);
            } else {
                recent.push(session);
            }
        }
        if !pinned.is_empty() {
            if !compact {
                sessions = sessions.child(library::section(t("Pinned"), p));
            }
            sessions = sessions.children(pinned.into_iter().map(|session| self.agent_session_row(session, compact, p, cx)));
        }
        if !recent.is_empty() {
            if !compact {
                sessions = sessions.child(library::section(t("Sessions"), p));
            }
            sessions = sessions.children(recent.into_iter().map(|session| self.agent_session_row(session, compact, p, cx)));
        } else if !state.library.query.is_empty() && !state.sessions.iter().any(|session| state.library.matches(session)) {
            sessions = sessions.child(library::section(t("No matching sessions"), p));
        }
        if !busy && !state.recoverable.is_empty() {
            sessions = sessions.child(div().px(px(theme::SPACE[3])).text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(t("Recoverable sessions")));
            for (index, session) in state.recoverable.iter().enumerate() {
                if !state.library.matches(session) {
                    continue;
                }
                let label = session.title.clone();
                let id = session.id.clone();
                sessions = sessions.child(
                    self.pressable(
                        panel_button(("agent-recover", index), p)
                            .when(compact, |d| d.max_w(px(210.)))
                            .child(icon("refresh-cw", 13., p.text_muted))
                            .child(div().truncate().child(label.clone())),
                        Control::new(t("Agents"), format!("agent-recover-{index}"), accesskit::Role::Button, label),
                        cx,
                        move |this, w, cx| this.recover_agent_session(id.clone(), w, cx),
                    ),
                );
            }
        }
        let title = state
            .sessions
            .iter()
            .find(|session| Some(session.id.as_str()) == state.selected.as_deref())
            .map_or_else(|| if pending { t("New session") } else { state.provider.label() }, |session| session.title.as_str());
        let header = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(theme::SPACE[3]))
            .p(px(theme::SPACE[3]))
            .border_b_1()
            .border_color(c(p.border))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().truncate().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(title.to_owned()))
                    .when_some(state.project.as_ref(), |d, project| {
                        let path = ui::local_path(&project.folder);
                        let workspace = self.active_root(cx).is_some_and(|root| root == path) && self.vcs.scm_root.as_ref().is_none_or(|root| root == &path);
                        let branch = workspace.then_some(self.vcs.branch.as_ref()).flatten();
                        let location = branch.map_or_else(|| path.clone(), |branch| format!("{path} · {branch}"));
                        d.child(div().truncate().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(location))
                    }),
            )
            .child(bars);
        let mut content = div().flex_1().min_h_0().min_w_0().flex().flex_col();
        if let Some(message) = &state.message {
            content = content.child(div().p(px(theme::SPACE[3])).text_color(c(p.text_muted)).child(message.clone()));
        }
        if let Some(terminal) = &state.terminal {
            content = content.child(div().flex_1().min_h_0().min_w_0().pt(px(4.)).pl(px(8.)).child(terminal.clone()));
        } else if self.remote.host.is_remote() {
            content = content.child(div().p(px(theme::SPACE[4])).text_color(c(p.text_muted)).child(t("Agent profiles are available in local windows.")));
        } else if state.message.is_none() {
            content = content.child(div().p(px(theme::SPACE[4])).text_color(c(p.text_muted)).child(if available {
                if state.provider == Provider::Antigravity && state.sessions.is_empty() {
                    t("Start a session to sign in or open Antigravity.")
                } else {
                    t("Select a session to continue, or start a new one.")
                }
            } else {
                t("Choose a project to start an agent.")
            }));
        }
        div()
            .id("agents-panel")
            .debug_selector(|| "agents-panel".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .when(compact, |d| d.flex_col())
            .text_size(px(text::SM.0))
            .text_color(c(p.text))
            .bg(c(p.bg))
            .child(sessions)
            .child(div().flex_1().min_w_0().min_h_0().flex().flex_col().child(header).child(content))
    }
}

#[cfg(test)]
#[gpui::test]
fn agent_history_preferences_follow_the_selected_project(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let data = crate::qa_workbench_tests::fresh("agent-panel-data");
    let first = crate::qa_workbench_tests::fresh("agent-project-first");
    let second = crate::qa_workbench_tests::fresh("agent-project-second");
    wb.update(cx, |wb, cx| {
        wb.set_data_dir(data);
        wb.chrome.agents.chosen_folder = Some(first.clone());
        wb.open_agents(cx);
    });
    cx.run_until_parked();
    let history = cx.debug_bounds(HISTORY_BUTTON).expect("history selector");
    cx.simulate_click(history.center(), gpui::Modifiers::none());
    assert_eq!(wb.read_with(cx, |wb, cx| wb.menu_labels(cx)), Some(vec!["Private sessions".into(), "Shared sessions".into()]));
    cx.simulate_keystrokes("escape");
    #[cfg(unix)]
    wb.update(cx, |wb, cx| {
        assert_eq!(wb.chrome.agents.mode, HistoryMode::Private);
        wb.select_agent_history(HistoryMode::Shared, cx);
        wb.add_agent_account(cx);
        assert_eq!(wb.chrome.agents.profile, 2);
        wb.chrome.agents.chosen_folder = Some(second);
        wb.open_agents(cx);
        assert_eq!(wb.chrome.agents.mode, HistoryMode::Private);
        assert_eq!(wb.chrome.agents.profile, 0);
        wb.chrome.agents.chosen_folder = Some(first);
        wb.open_agents(cx);
        assert_eq!(wb.chrome.agents.mode, HistoryMode::Shared);
        assert_eq!(wb.chrome.agents.profile, 2);
    });
    #[cfg(not(unix))]
    let _ = second;
}

#[cfg(test)]
#[gpui::test]
fn agent_history_updates_while_the_conversation_is_open(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let data = crate::qa_workbench_tests::fresh("agent-live-history-data");
    let folder = crate::qa_workbench_tests::fresh("agent-live-history-project");
    let project = Project::new(&data, &folder, Provider::Claude).expect("test project");
    let id = new_session_id().expect("session ID");
    project.prepare(0, HistoryMode::Private).expect("history directory");
    let paths = project.watch_paths(0, HistoryMode::Private, Some(&id));
    let _watch = wb.update(cx, |wb, cx| {
        wb.set_data_dir(data);
        wb.chrome.agents.chosen_folder = Some(folder);
        wb.open_agents(cx);
        Workbench::watch_agent_history(&project, 0, HistoryMode::Private, Some(&id), cx)
    });
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.chrome.agents.sessions.is_empty()));

    let transcript = &paths[1].1;
    std::fs::write(transcript, "").expect("Claude creates a transcript");
    cx.executor().advance_clock(agent::watch::INTERVAL);
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.chrome.agents.sessions.len()), 1);

    std::fs::write(transcript, "{\"type\":\"user\",\"message\":{\"content\":\"Live session title\"}}\n").expect("Claude saves the first turn");
    cx.executor().advance_clock(agent::watch::INTERVAL);
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.chrome.agents.sessions.first().map(|session| session.title.clone())).as_deref(), Some("Live session title"),);
}

#[cfg(test)]
#[gpui::test]
fn agent_session_search_and_pins_follow_the_account(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let data = crate::qa_workbench_tests::fresh("agent-library-data");
    let folder = crate::qa_workbench_tests::fresh("agent-library-project");
    wb.update(cx, |wb, cx| {
        wb.set_data_dir(data);
        wb.chrome.agents.chosen_folder = Some(folder);
        wb.open_agents(cx);
    });
    cx.run_until_parked();
    wb.update_in(cx, |wb, window, cx| {
        let field = wb.chrome.agents.library.field.as_ref().expect("session search");
        window.focus(&field.focus_handle(cx));
    });
    cx.simulate_input("  AUTH  ");
    wb.update(cx, |wb, cx| {
        let auth = Session { id: "a".into(), title: "Fix authentication".into(), path: None };
        let readme = Session { id: "b".into(), title: "Update the README".into(), path: None };
        let state = &mut wb.chrome.agents;
        assert!(state.library.matches(&auth));
        assert!(!state.library.matches(&readme));
        state.sessions = vec![auth, readme];
        wb.toggle_agent_pin("a", cx);
        assert!(wb.chrome.agents.library.pins.contains("a"));
        assert!(wb.apply_agent_profile(1, cx));
        assert!(wb.chrome.agents.library.pins.is_empty());
        assert!(wb.apply_agent_profile(0, cx));
        assert!(wb.chrome.agents.library.pins.contains("a"));
    });
}

#[cfg(test)]
#[gpui::test]
fn agent_provider_switch_keeps_separate_profiles(cx: &mut gpui::TestAppContext) {
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    let data = crate::qa_workbench_tests::fresh("agent-provider-data");
    let folder = crate::qa_workbench_tests::fresh("agent-provider-project");
    wb.update(cx, |wb, cx| {
        wb.set_data_dir(data);
        wb.chrome.agents.chosen_folder = Some(folder);
        wb.open_agents(cx);
        wb.add_agent_account(cx);
    });
    cx.run_until_parked();
    wb.update(cx, |wb, _| wb.chrome.agents.installed = Some(vec![Provider::Claude, Provider::Codex]));
    let picker = cx.debug_bounds(PROVIDER_BUTTON).expect("provider selector");
    cx.simulate_click(picker.center(), gpui::Modifiers::none());
    assert_eq!(wb.read_with(cx, |wb, cx| wb.menu_labels(cx)), Some(vec!["Claude Code".into(), "Codex".into(), "Antigravity CLI is not installed".into()]));
    cx.simulate_keystrokes("down down enter");
    wb.update(cx, |wb, cx| {
        assert_eq!(wb.chrome.agents.provider, Provider::Codex);
        assert_eq!(wb.chrome.agents.profile, 0);
        assert_eq!(wb.chrome.agents.mode, HistoryMode::Private);
        assert_eq!(wb.chrome.agents.profile_count, 2);
        wb.select_agent_provider(Provider::Claude, cx);
        assert_eq!(wb.chrome.agents.profile, 2);
        assert_eq!(wb.chrome.agents.profile_count, 3);
    });
}
