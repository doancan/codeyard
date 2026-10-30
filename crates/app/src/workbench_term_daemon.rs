//! Workbench: saving the panel's terminals and bringing them back, with terminals that outlive the app.
//!
//! With `window.restoreSession` on (Unix, local folders), a window runs its terminals on the terminal daemon: the same
//! binary started as `--daemon` on a socket in the state folder (owner-only, like the SSH daemon's). The window opens a
//! session there with a random secret id. The daemon keeps the session's shells when the app quits, crashes or is
//! killed, buffers their output (the last 256 KB each) and ends them if no window takes the session back within its
//! grace period (5 minutes); with no sessions left it exits after 10 minutes. At the next start the window takes the
//! session back and each saved terminal with its token (the daemon's instance id and the terminal's secret): those are
//! the same processes, shown with "Reconnected to terminal session". Anything else (a refused or missing token, a daemon
//! that restarted, restore off, SSH) comes back as a new shell as before, and nothing saved is ever sent as input. One
//! window owns a session at a time: a session already used by another window of this app is not taken over, and a newer
//! connection to a session stops the older one from typing into it.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::*;
use crate::terminal_session::{SavedTerminal, SavedTerminals};

/// The terminal daemon's socket, set at startup (never in tests, which opt in per window).
static SOCKET: OnceLock<PathBuf> = OnceLock::new();
/// Sessions of the windows of this process: two windows never share one.
static IN_USE: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Use the terminal daemon on `socket` (called once by `main`).
pub fn enable_terminal_daemon(socket: PathBuf) {
    let _ = SOCKET.set(socket);
}

/// The socket the app uses: `terminals/d.sock` in the state folder.
pub fn terminal_daemon_socket() -> Option<PathBuf> {
    state_path()?.parent().map(|d| d.join("terminals").join("d.sock"))
}

/// A window's connection to its session on the terminal daemon.
pub(super) struct TermDaemon {
    pub(super) host: Arc<host::RemoteHost>,
    pub(super) session: String,
    workspace: i64,
}

impl Drop for TermDaemon {
    fn drop(&mut self) {
        release(&self.session);
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

/// 128 random bits as hex: a new session id.
fn new_session() -> Option<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// What taking the session back gave: the connection, and the saved terminals (by index) that are the same processes,
/// with the bytes of output each lost meanwhile.
struct Taken {
    daemon: TermDaemon,
    back: Vec<(usize, host::Pty, u64)>,
}

/// Connect to (starting if needed) the daemon and open or take back `session`; take back the saved terminals whose
/// tokens it accepts, and end the session's other terminals (no window could ever reach them again). Blocking.
#[cfg(unix)]
fn take_session(socket: &std::path::Path, session: String, saved: &[(usize, crate::terminal_session::SavedPty)], workspace: i64) -> Option<Taken> {
    let stream = agent::daemon::connect_or_start(socket).ok()?;
    let closer = stream.try_clone().ok()?;
    let transport = host::Transport::pipes(Box::new(stream.try_clone().ok()?), Box::new(stream));
    let close = Box::new(move || drop(closer.shutdown(std::net::Shutdown::Both)));
    let ids: Vec<u32> = saved.iter().map(|(_, t)| t.pty).collect();
    let (host, resumed, ptys) = host::RemoteHost::attach_session(transport, close, &session, &ids, "Local").ok()?;
    let host = Arc::new(host);
    let mut back = Vec::new();
    for pty in ptys {
        let Some((ix, token)) = saved.iter().find(|(_, t)| t.pty == pty.id) else { continue };
        // A refused token: its buffered output is dropped with `pty`, unread.
        if let Ok((_, dropped)) = host.pty_attach(pty.id, token.instance.clone(), token.secret.clone()) {
            back.push((*ix, pty, dropped));
        }
    }
    if resumed {
        for id in host.pty_list().unwrap_or_default() {
            if !back.iter().any(|(_, p, _)| p.id == id) {
                host.pty_kill(id);
            }
        }
    }
    Some(Taken { daemon: TermDaemon { host, session, workspace }, back })
}

#[cfg(not(unix))]
fn take_session(_: &std::path::Path, _: String, _: &[(usize, crate::terminal_session::SavedPty)], _: i64) -> Option<Taken> {
    None
}

pub(super) fn saved_terminals_key(ws: i64) -> String {
    format!("terminals:{ws}")
}

impl Workbench {
    /// Where new terminals of this window run: its terminal daemon session when it has one, else the folder's host.
    pub(super) fn terminal_host(&self) -> (Arc<dyn Host>, bool) {
        match &self.persist.term_daemon {
            Some(d) if Some(d.workspace) == self.persist.workspace_id && !self.remote.host.is_remote() => (d.host.clone(), true),
            _ => (self.remote.host.clone(), false),
        }
    }

    /// The socket of the terminal daemon this window may use: restore on, a local folder, Unix.
    fn terminal_daemon_for(&self, cx: &App) -> Option<PathBuf> {
        if !cfg!(unix) || !cx.global::<Settings>().restore_session || self.remote.host.is_remote() {
            return None;
        }
        self.persist.term_socket.clone().or_else(|| SOCKET.get().cloned())
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
        let session = self.persist.term_daemon.as_ref().filter(|d| d.workspace == ws).map(|d| d.session.clone());
        let saved = SavedTerminals { version: 1, active, terminals, session };
        if let Some(json) = saved.to_json() {
            let _ = s.set(&saved_terminals_key(ws), &json);
        }
    }

    /// Bring the saved terminals back: the ones the terminal daemon still runs are taken back as they are, the others
    /// start again as new shells in their folders. Nothing saved is sent to any of them as input: a saved history is
    /// shown as output, followed by a line saying the previous session ended.
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
        // The window's daemon session: the saved one unless another window of this app already has it.
        let daemon = self.terminal_daemon_for(cx).and_then(|socket| {
            self.persist.term_daemon = None;
            let session = saved.session.clone().filter(|s| claim(s)).or_else(|| new_session().filter(|s| claim(s)))?;
            let tokens: Vec<_> = if saved.session.as_deref() == Some(session.as_str()) {
                saved.terminals.iter().enumerate().filter_map(|(ix, t)| Some((ix, t.token.clone()?))).collect()
            } else {
                Vec::new()
            };
            Some(cx.background_executor().spawn(async move {
                let taken = take_session(&socket, session.clone(), &tokens, ws);
                if taken.is_none() {
                    release(&session);
                }
                taken
            }))
        });
        let host = self.remote.host.clone();
        let cwds: Vec<Option<String>> = saved.terminals.iter().map(|t| t.cwd.clone()).collect();
        // Which saved folders still exist, asked off the UI thread (over SSH each is a round trip).
        let present = cx
            .background_executor()
            .spawn(async move { cwds.iter().map(|c| c.as_ref().is_none_or(|c| host.stat(c).is_ok_and(|m| m.kind == proto::Kind::Dir))).collect::<Vec<_>>() });
        let asked = (self.remote.host.clone(), self.persist.workspace_id);
        cx.spawn_in(window, async move |this, cx| {
            let taken = match daemon {
                Some(task) => task.await,
                None => None,
            };
            let present = present.await;
            this.update_in(cx, |this, window, cx| {
                // Another folder or connection by now: these terminals belonged to the previous one.
                if !Arc::ptr_eq(&asked.0, &this.remote.host) || asked.1 != this.persist.workspace_id || !this.terminals.is_empty() {
                    return;
                }
                let mut back = Vec::new();
                if let Some(taken) = taken {
                    this.persist.term_daemon = Some(taken.daemon);
                    back = taken.back;
                }
                if saved.terminals.is_empty() {
                    return;
                }
                this.revive_terminals(&saved, back, present, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Put the saved terminals in the panel: the ones taken back, the others as new shells.
    fn revive_terminals(
        &mut self,
        saved: &SavedTerminals,
        mut back: Vec<(usize, host::Pty, u64)>,
        present: Vec<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel_open = self.chrome.panel_open;
        let fallback = self.root(cx);
        let mut reconnected = 0;
        for (ix, (t, here)) in saved.terminals.iter().zip(present).enumerate() {
            if let Some(at) = back.iter().position(|(i, _, _)| *i == ix) {
                let (_, pty, dropped) = back.swap_remove(at);
                let (host, _) = self.terminal_host();
                let term = cx.new(|cx| crate::terminal::TerminalView::reattached(host, pty, t, dropped, cx));
                self.adopt_terminal(&term, window, cx);
                self.terminals.push(term);
                reconnected += 1;
                continue;
            }
            self.revive_as_new_shell(t, here, fallback.clone(), window, cx);
        }
        if reconnected > 0 {
            self.notice = Some(Notice::Info(crate::i18n::t("Reconnected to terminal session").into()));
        }
        self.active_terminal = saved.active.min(self.terminals.len().saturating_sub(1));
        // Restoring does not open the panel on its own.
        self.chrome.panel_open = panel_open;
        self.persist_terminals(cx);
        cx.notify();
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

    /// Use a terminal daemon on `socket` in this window (tests; the app sets it once for every window).
    #[cfg(test)]
    pub(crate) fn use_terminal_daemon_for_test(&mut self, socket: PathBuf) {
        self.persist.term_socket = Some(socket);
    }
}

#[cfg(all(test, unix))]
#[path = "workbench_term_daemon_tests.rs"]
mod tests;
