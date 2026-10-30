//! Workbench: saving a window's terminals and bringing them back, with terminals that outlive the app.
//!
//! A window's terminals run in an agent daemon session that can outlive the app: for a local folder (with
//! `window.restoreSession`) the terminal daemon, the same binary started as `--daemon` on an owner-only socket in the
//! state folder (on Windows, a named pipe only this user can open); for an SSH folder the agent daemon on the server, which already keeps sessions across dropped
//! connections. The session id is a random secret saved with the window's terminals, and each terminal has a token (the
//! daemon's instance id and the terminal's own secret, never a process id).
//!
//! A session is kept only when the user is expected to come back: when the app quits (every window is told so), crashes
//! or is killed, and for `terminal.persistentSessionTimeout` minutes (0: not at all). Leaving on purpose (closing the
//! window, Close Folder, another folder, another connection) ends it and its shells at once. At the next start the
//! window takes the session back and each saved terminal, in the panel, in an editor group or in a window of its own,
//! with its token: those are the same processes, shown with "Reconnected to terminal session". Anything else (a refused
//! or missing token, a restarted daemon, restore off) comes back as a new shell as before, and nothing saved is ever
//! sent as input. One window owns a session: another window of this app never shares it, the local daemon refuses
//! another app instance while the window's connection lives, and a newer connection stops an older one from typing.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::*;
use crate::terminal_session::{SavedPty, SavedTerminal, SavedTerminals};

/// The terminal daemon's socket, set at startup (never in tests, which opt in per window).
static SOCKET: OnceLock<PathBuf> = OnceLock::new();
/// Sessions of the windows of this process: two windows never share one.
static IN_USE: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Use the terminal daemon on `socket` (called once by `main`).
#[cfg(any(unix, windows))]
pub fn enable_terminal_daemon(socket: PathBuf) {
    let _ = SOCKET.set(socket);
}

/// The socket the app uses: `terminals/d.sock` in the state folder.
#[cfg(unix)]
pub fn terminal_daemon_socket() -> Option<PathBuf> {
    state_path()?.parent().map(|d| d.join("terminals").join("d.sock"))
}

/// The named pipe the app uses on Windows: this user's, for this state folder.
#[cfg(windows)]
pub fn terminal_daemon_socket() -> Option<PathBuf> {
    let scope = state_path()?.parent()?.to_string_lossy().into_owned();
    agent::daemon::pipe_name(&scope).ok().map(PathBuf::from)
}

/// A window's connection to its session on the terminal daemon.
pub(super) struct TermDaemon {
    pub(super) host: Arc<host::RemoteHost>,
    pub(super) session: String,
    workspace: i64,
    /// The app is quitting: leave the session to the daemon for the next start. Otherwise (the window closes, another
    /// folder opens, Close Folder) the session ends with this value and its shells with it.
    keep: bool,
}

impl Drop for TermDaemon {
    fn drop(&mut self) {
        if !self.keep {
            self.host.end_session();
        }
        release(&self.session);
    }
}

/// A session id this window holds (an SSH folder's): released when the window lets it go.
pub(super) struct Claim(String);

impl Drop for Claim {
    fn drop(&mut self) {
        release(&self.0);
    }
}

/// Claim `session` for this window; `false` if another window of this process has it.
fn claim(session: &str) -> bool {
    IN_USE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(HashSet::new).insert(session.to_string())
}

fn release(session: &str) {
    if let Some(set) = IN_USE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut() {
        set.remove(session);
    }
}

/// A new exclusive session id: the prefix and 128 random bits as hex. Exclusive, so another app instance cannot take a
/// live window's terminals over (the daemon refuses it while this window's connection lives).
fn new_session() -> String {
    format!("{}{}", proto::EXCLUSIVE_SESSION, host::random_token())
}

/// Where a saved terminal was: the panel's `n`th, or the `n`th outside the panel (`workbench_term_restore.rs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Slot {
    Panel(usize),
    Moved(usize),
}

/// Terminals taken back: where each was, its events, and the bytes of output it lost meanwhile.
pub(super) type Back = Vec<(Slot, host::Pty, u64)>;

/// What taking a session back gave.
struct Taken {
    /// The local daemon connection (`None` for an SSH folder, whose connection the window already has).
    daemon: Option<TermDaemon>,
    back: Back,
    /// The saved session was in use by another app instance: new shells instead.
    in_use: bool,
}

/// Why a session could not be opened.
enum Refused {
    /// Another live connection (another Codeyard instance) holds it.
    InUse,
    Failed,
}

/// Take back, on `host`, the terminals whose tokens it accepts; then end the session's terminals no window can reach
/// any more (listed, not taken back, and not newer than the saved ones: a terminal opened since belongs to this window).
fn adopt_all(host: &dyn Host, tokens: &[(Slot, SavedPty)]) -> Back {
    let mut back = Vec::new();
    for (slot, token) in tokens {
        let r = host.pty_adopt(token.pty, token.instance.clone(), token.secret.clone());
        if let Ok((pty, dropped)) = r {
            back.push((*slot, pty, dropped));
        }
    }
    let newest = tokens.iter().map(|(_, t)| t.pty).max().unwrap_or(0);
    for id in host.pty_list().unwrap_or_default() {
        if id <= newest && !back.iter().any(|(_, p, _)| p.id == id) {
            host.pty_kill(id);
        }
    }
    back
}

/// Connect to (starting if needed) the local daemon and open or take back `session`. Blocking.
fn take_session(socket: &std::path::Path, session: String, tokens: &[(Slot, SavedPty)], workspace: i64) -> Result<Taken, Refused> {
    let (transport, close) = connect(socket).ok_or(Refused::Failed)?;
    let (host, resumed) = match host::RemoteHost::attach_session(transport, close, &session, "Local") {
        Ok(v) => v,
        Err(host::HostError::Op(proto::Error::PermissionDenied(_))) => return Err(Refused::InUse),
        Err(_) => return Err(Refused::Failed),
    };
    let host = Arc::new(host);
    let back = if resumed { adopt_all(host.as_ref(), tokens) } else { Vec::new() };
    Ok(Taken { daemon: Some(TermDaemon { host, session, workspace, keep: false }), back, in_use: false })
}

/// A connection to the daemon on `socket` (started if none answers), and what ends it at once.
#[cfg(unix)]
fn connect(socket: &std::path::Path) -> Option<(host::Transport, Box<dyn FnOnce() + Send>)> {
    let stream = agent::daemon::connect_or_start(socket).ok()?;
    let closer = stream.try_clone().ok()?;
    let transport = host::Transport::pipes(Box::new(stream.try_clone().ok()?), Box::new(stream));
    Some((transport, Box::new(move || drop(closer.shutdown(std::net::Shutdown::Both)))))
}

/// A connection to the daemon's named pipe (started if none answers), and what ends it at once.
#[cfg(windows)]
fn connect(pipe: &std::path::Path) -> Option<(host::Transport, Box<dyn FnOnce() + Send>)> {
    let pipe = agent::daemon::connect_or_start(pipe).ok()?;
    let closer = pipe.clone();
    let transport = host::Transport::pipes(Box::new(pipe.clone()), Box::new(pipe));
    Some((transport, Box::new(move || closer.shutdown())))
}

/// No terminal daemon on other systems (`terminal_daemon_for` never asks for one there).
#[cfg(not(any(unix, windows)))]
fn connect(_: &std::path::Path) -> Option<(host::Transport, Box<dyn FnOnce() + Send>)> {
    None
}

/// The saved session (retried briefly while its previous owner's connection is still closing: a killed app's socket
/// can take a moment to be noticed), else a new one. Blocking.
fn open_session(socket: &std::path::Path, saved: Option<String>, tokens: &[(Slot, SavedPty)], ws: i64) -> Option<Taken> {
    let mut in_use = false;
    if let Some(session) = saved {
        for attempt in 0..10 {
            match take_session(socket, session.clone(), tokens, ws) {
                Ok(taken) => return Some(taken),
                Err(Refused::InUse) if attempt < 9 => std::thread::sleep(std::time::Duration::from_millis(100)),
                Err(Refused::InUse) => in_use = true,
                Err(Refused::Failed) => break,
            }
        }
        release(&session);
    }
    let session = Some(new_session()).filter(|s| claim(s))?;
    match take_session(socket, session.clone(), &[], ws) {
        Ok(taken) => Some(Taken { in_use, ..taken }),
        Err(_) => {
            release(&session);
            None
        }
    }
}

pub(super) fn saved_terminals_key(ws: i64) -> String {
    format!("terminals:{ws}")
}

/// Where an SSH folder's session id is kept, so connecting to it after a restart can resume the session.
fn remote_session_key(alias: &str, path: &str) -> String {
    format!("remote_session:{alias}\t{path}")
}

impl Workbench {
    /// Where new terminals of this window run, and whether they can be taken back after a restart (they get a token):
    /// the local terminal daemon session when the window has one, else the folder's host (an SSH folder's agent daemon
    /// session can be taken back; the in-process agent cannot).
    pub(super) fn terminal_host(&self) -> (Arc<dyn Host>, bool) {
        match &self.persist.term_daemon {
            Some(d) if Some(d.workspace) == self.persist.workspace_id && !self.remote.host.is_remote() => (d.host.clone(), true),
            _ => (self.remote.host.clone(), self.remote.host.is_remote() && self.remote.host.session().is_some()),
        }
    }

    /// The socket of the terminal daemon this window may use: restore on, a local folder, Unix.
    fn terminal_daemon_for(&self, cx: &App) -> Option<PathBuf> {
        if !cfg!(any(unix, windows)) || !cx.global::<Settings>().restore_session || self.remote.host.is_remote() {
            return None;
        }
        self.persist.term_socket.clone().or_else(|| SOCKET.get().cloned())
    }

    /// The window's session, where its terminals are kept (local daemon or SSH folder).
    fn terminal_session(&self) -> Option<String> {
        match &self.persist.term_daemon {
            Some(d) if Some(d.workspace) == self.persist.workspace_id => Some(d.session.clone()),
            _ if self.remote.host.is_remote() => self.remote.host.session(),
            _ => None,
        }
    }

    /// The session to resume when connecting to `alias` for `path` (the one this window's terminals were kept in), if
    /// no other window of this app has it.
    pub(super) fn claim_remote_session(&mut self, alias: &str, path: Option<&str>) -> Option<String> {
        self.persist.remote_claim = None;
        let saved = self.persist.state.as_ref()?.get(&remote_session_key(alias, path?)).filter(|s| !s.is_empty())?;
        let session = Some(saved).filter(|s| claim(s))?;
        self.persist.remote_claim = Some(Claim(session.clone()));
        Some(session)
    }

    /// Save the panel's terminals for the next start (with `window.restoreSession`; history only with
    /// `terminal.persistentScrollback`). Output-only terminals (tasks, tests) are not saved.
    pub(super) fn persist_terminals(&self, cx: &App) {
        let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) else { return };
        let settings = cx.global::<Settings>();
        if !settings.restore_session {
            let _ = s.set(&saved_terminals_key(ws), "");
            return;
        }
        let lines = settings.terminal_persistent_scrollback;
        let mut active = 0;
        let mut terminals = Vec::new();
        for (ix, t) in self.terminals.iter().enumerate() {
            if let Some(saved) = t.read(cx).saved(lines) {
                if ix == self.active_terminal {
                    active = terminals.len();
                }
                terminals.push(saved);
            }
        }
        let session = self.terminal_session();
        // Kept under what the next start reopens: the workspace file when one is open (Recents and a restart reopen
        // that), else the folder.
        let opened = self.roots.workspace.as_ref().map(|w| w.path.clone()).or_else(|| self.root(cx));
        if let (Conn::Connected { alias }, Some(opened), Some(session)) = (&self.remote.conn, opened, &session)
            && self.remote.host.is_remote()
        {
            let _ = s.set(&remote_session_key(alias, &opened), session);
        }
        let saved = SavedTerminals { version: 1, active, terminals, session };
        if let Some(json) = saved.to_json() {
            let _ = s.set(&saved_terminals_key(ws), &json);
        }
    }

    /// Bring the saved terminals back: the ones still running in the window's session are taken back as they are (in
    /// the panel here; those outside the panel when `restore_moved_terminals` places them), the others start again as
    /// new shells in their folders. Nothing saved is sent to any of them as input.
    pub(super) fn restore_terminals(&mut self, ws: i64, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.global::<Settings>().restore_session || !self.terminals.is_empty() {
            return;
        }
        let text = self.persist.state.as_ref().and_then(|s| s.get(&saved_terminals_key(ws))).filter(|t| !t.is_empty());
        let lines = cx.global::<Settings>().terminal_persistent_scrollback;
        let saved = match text.map(|t| SavedTerminals::parse(&t, lines)) {
            None => SavedTerminals::default(),
            Some(Some(saved)) => saved,
            Some(None) => {
                self.notice =
                    Some(Notice::Error(crate::i18n::tf("Terminal session could not be restored: {}", &[&crate::i18n::t("the saved state is damaged")]).into()));
                cx.notify();
                SavedTerminals::default()
            }
        };
        let keep_secs = cx.global::<Settings>().terminal_persistent_session_timeout * 60;
        // Every saved token of the window: the panel's and those of terminals outside the panel.
        let moved = self.saved_moved_terminals(ws).map(|m| m.tokens()).unwrap_or_default();
        let tokens: Vec<(Slot, SavedPty)> = saved
            .terminals
            .iter()
            .enumerate()
            .filter_map(|(ix, t)| Some((Slot::Panel(ix), t.token.clone()?)))
            .chain(moved.into_iter().map(|(ix, t)| (Slot::Moved(ix), t)))
            .collect();
        let taking = if let Some(socket) = self.terminal_daemon_for(cx) {
            // The local daemon: the saved session unless another window of this app already has it.
            self.persist.term_daemon = None;
            let session = saved.session.clone().filter(|s| claim(s));
            let tokens = if session.is_some() { tokens } else { Vec::new() };
            Some(cx.background_executor().spawn(async move {
                let taken = open_session(&socket, session, &tokens, ws);
                if let Some(d) = taken.as_ref().and_then(|t| t.daemon.as_ref()) {
                    d.host.keep_for(keep_secs);
                }
                taken
            }))
        } else if self.remote.host.is_remote() && self.remote.host.session().is_some() {
            // An SSH folder: its connection resumed the saved session if it could; take its terminals back on it.
            let host = self.remote.host.clone();
            host.keep_for(keep_secs);
            let ours = saved.session.is_some() && saved.session == host.session();
            let tokens = if ours { tokens } else { Vec::new() };
            Some(cx.background_executor().spawn(async move { Some(Taken { daemon: None, back: adopt_all(host.as_ref(), &tokens), in_use: false }) }))
        } else {
            None
        };
        self.persist.term_restoring = taking.is_some();
        self.persist.panel_restoring = true;
        let host = self.remote.host.clone();
        let cwds: Vec<Option<String>> = saved.terminals.iter().map(|t| t.cwd.clone()).collect();
        // Which saved folders still exist, asked off the UI thread (over SSH each is a round trip).
        let present = cx
            .background_executor()
            .spawn(async move { cwds.iter().map(|c| c.as_ref().is_none_or(|c| host.stat(c).is_ok_and(|m| m.kind == proto::Kind::Dir))).collect::<Vec<_>>() });
        let asked = (self.remote.host.clone(), self.persist.workspace_id);
        cx.spawn_in(window, async move |this, cx| {
            let taken = match taking {
                Some(task) => task.await,
                None => None,
            };
            let present = present.await;
            this.update_in(cx, |this, window, cx| {
                this.persist.term_restoring = false;
                this.persist.panel_restoring = false;
                // Another folder or connection by now: these terminals belonged to the previous one.
                if !Arc::ptr_eq(&asked.0, &this.remote.host) || asked.1 != this.persist.workspace_id || !this.terminals.is_empty() {
                    this.persist.pending_moved = None;
                    this.open_wanted_terminal(window, cx);
                    return;
                }
                let (mut back, mut in_use) = (Vec::new(), false);
                if let Some(taken) = taken {
                    if let Some(daemon) = taken.daemon {
                        this.persist.term_daemon = Some(daemon);
                    }
                    (back, in_use) = (taken.back, taken.in_use);
                }
                let moved_back: Vec<(usize, host::Pty, u64)> = back
                    .extract_if(.., |(slot, _, _)| matches!(slot, Slot::Moved(_)))
                    .filter_map(|(slot, pty, dropped)| if let Slot::Moved(ix) = slot { Some((ix, pty, dropped)) } else { None })
                    .collect();
                let reconnected = back.len() + moved_back.len();
                if !saved.terminals.is_empty() {
                    this.revive_terminals(&saved, back, present, window, cx);
                }
                // Terminals outside the panel waited for the session: place them now, or keep what was taken back for
                // when their places are restored.
                match this.persist.pending_moved.take() {
                    Some(moved) => this.place_moved_terminals(moved, moved_back, window, cx),
                    None => this.persist.moved_back = moved_back,
                }
                if reconnected > 0 {
                    this.notice = Some(Notice::Info(crate::i18n::t("Reconnected to terminal session").into()));
                }
                if in_use {
                    let why = crate::i18n::t("another Codeyard window has them; started new shells");
                    this.notice = Some(Notice::Info(crate::i18n::tf("Terminal session could not be restored: {}", &[&why]).into()));
                }
                this.open_wanted_terminal(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The terminal asked for while the restore ran: a new one if none came back, else the panel's active one in front.
    pub(super) fn open_wanted_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.persist.terminal_wanted) {
            return;
        }
        if self.terminals.is_empty() {
            self.new_terminal(&NewTerminal, window, cx);
        } else {
            self.chrome.panel_open = true;
            self.chrome.bottom = Bottom::Terminal;
            self.focus_terminal(window, cx);
        }
    }

    /// Put the saved terminals in the panel: the ones taken back, the others as new shells.
    fn revive_terminals(&mut self, saved: &SavedTerminals, mut back: Back, present: Vec<bool>, window: &mut Window, cx: &mut Context<Self>) {
        let panel_open = self.chrome.panel_open;
        let fallback = self.root(cx);
        for (ix, (t, here)) in saved.terminals.iter().zip(present).enumerate() {
            if let Some(at) = back.iter().position(|(slot, _, _)| *slot == Slot::Panel(ix)) {
                let (_, pty, dropped) = back.swap_remove(at);
                let term = self.reattached_terminal(pty, t, dropped, window, cx);
                self.terminals.push(term);
                continue;
            }
            self.revive_as_new_shell(t, here, fallback.clone(), window, cx);
        }
        self.active_terminal = saved.active.min(self.terminals.len().saturating_sub(1));
        // Restoring does not open the panel on its own.
        self.chrome.panel_open = panel_open;
        self.persist_terminals(cx);
        cx.notify();
    }

    /// A view for a terminal taken back from the window's session, listened to like any other.
    pub(super) fn reattached_terminal(
        &mut self,
        pty: host::Pty,
        saved: &SavedTerminal,
        dropped: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<crate::terminal::TerminalView> {
        let (host, _) = self.terminal_host();
        let term = cx.new(|cx| crate::terminal::TerminalView::reattached(host, pty, saved, dropped, cx));
        self.adopt_terminal(&term, window, cx);
        term
    }

    /// A saved terminal that cannot be taken back: a new shell in its folder (or the workspace folder), its saved
    /// history shown as output and a line saying what happened.
    fn revive_as_new_shell(&mut self, t: &SavedTerminal, here: bool, fallback: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = if here { t.cwd.clone() } else { fallback };
        let before = self.terminals.len();
        let launch = crate::terminal::Launch { shell: t.shell.clone(), args: t.args.clone(), env: Vec::new(), color: t.color.clone() };
        self.new_terminal_launch(cwd, launch, window, cx);
        let Some(term) = self.terminals.get(before).cloned() else { return };
        let mut shown = String::new();
        if let Some(history) = &t.history {
            shown.push_str(history);
            shown.push('\n');
        }
        let status = if here {
            crate::i18n::t("Previous session ended; started a new shell").to_string()
        } else {
            crate::i18n::tf(
                "Terminal session could not be restored: {}",
                &[&crate::i18n::tf("the folder {} no longer exists; started a new shell in the workspace folder", &[&t.cwd.clone().unwrap_or_default()])],
            )
        };
        shown.push_str(&format!("\x1b[2m{status}\x1b[0m\n"));
        term.update(cx, |term, cx| {
            if let Some(title) = &t.title {
                term.set_fixed_title(title.clone());
            }
            // Output only: shown in the terminal, never sent to the shell.
            term.append(shown.as_bytes(), cx);
        });
    }

    /// "Terminal: Clear Saved Terminal History": the saved histories of this folder are removed now (terminals are still
    /// restored; history is saved again at the next save only while `terminal.persistentScrollback` is above 0).
    pub(super) fn clear_saved_terminal_history(&mut self, cx: &mut Context<Self>) {
        let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) else { return };
        if let Some(mut saved) = s.get(&saved_terminals_key(ws)).and_then(|t| SavedTerminals::parse(&t, 0)) {
            for t in &mut saved.terminals {
                t.history = None;
            }
            if let Some(json) = saved.to_json() {
                let _ = s.set(&saved_terminals_key(ws), &json);
            }
        }
        self.notice = Some(Notice::Info(crate::i18n::t("Saved terminal history cleared").into()));
        cx.notify();
    }

    /// The app is quitting: this window's terminals stay in their session for `terminal.persistentSessionTimeout`
    /// minutes, for the next start (with 0, they end with the app as when the window is closed).
    pub fn keep_terminals_for_restart(&mut self, cx: &App) {
        let secs = cx.global::<Settings>().terminal_persistent_session_timeout * 60;
        if secs == 0 || !cx.global::<Settings>().restore_session {
            return;
        }
        if let Some(d) = self.persist.term_daemon.as_mut() {
            d.host.keep_for(secs);
            d.keep = true;
            self.persist.keeping_terminals = true;
        }
        if self.remote.host.is_remote() && self.remote.host.session().is_some() {
            self.remote.host.keep_for(secs);
            self.remote.host.keep_session();
            self.persist.keeping_terminals = true;
        }
    }

    /// Use a terminal daemon on `socket` in this window (tests; the app sets it once for every window).
    #[cfg(all(test, unix))]
    pub(crate) fn use_terminal_daemon_for_test(&mut self, socket: PathBuf) {
        self.persist.term_socket = Some(socket);
    }
}

#[cfg(all(test, unix))]
#[path = "workbench_term_daemon_tests.rs"]
mod tests;
