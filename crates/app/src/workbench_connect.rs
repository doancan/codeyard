//! Workbench: remote connection, workspace trust and recents.

use super::*;

impl Workbench {
    /// Running Docker containers or WSL distros: the selected one becomes the target connected through the agent.
    pub(super) fn show_remote_targets(&mut self, wsl: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !wsl {
            self.docker_contexts(docker_glue::Action::Attach, window, cx);
            return;
        }
        let task = cx.background_executor().spawn(async move {
            if wsl {
                host::ssh::list_wsl()
                    .map(|v| v.into_iter().map(|d| (format!("wsl:{d}"), d, crate::i18n::t("WSL distribution").to_string())).collect::<Vec<_>>())
            } else {
                host::ssh::list_containers().map(|v| v.into_iter().map(|(n, i)| (format!("docker:{n}"), n, i)).collect::<Vec<_>>())
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(list) if !list.is_empty() => {
                    let items = list.iter().map(|(_, n, d)| PickItem { label: n.clone().into(), detail: Some(d.clone().into()), keys: None }).collect();
                    let aliases = list.into_iter().map(|(a, ..)| a).collect();
                    let title = if wsl { crate::i18n::t("Connect to which WSL distribution?") } else { crate::i18n::t("Connect to which running container?") };
                    let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], title, "", None, cx));
                    this.show_overlay(picker, OverlayKind::RemoteTargets(aliases), window, cx);
                }
                Ok(_) => {
                    this.notice =
                        Some(Notice::Info(if wsl { crate::i18n::t("No WSL distributions found.") } else { crate::i18n::t("No running containers.") }.into()));
                    cx.notify();
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(e.lines().next().unwrap_or(crate::i18n::t("could not list")).to_string().into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn connect(&mut self, alias: String, window: &mut Window, cx: &mut Context<Self>) {
        let bundle = self.remote.bundle.clone();
        let target = alias.clone();
        // The folder's saved session, so the terminals kept in it can be taken back (restore on).
        let pending = self.remote.pending_path.clone();
        let session = cx.global::<Settings>().restore_session.then(|| self.claim_remote_session(&alias, pending.as_deref())).flatten();
        self.connect_using(alias, window, cx, move |progress| async move {
            let remote = if target == LOCAL_AGENT_ALIAS {
                host::ssh::spawn_local_agent(&bundle)
            } else {
                host::ssh::connect_session(&target, &bundle, session, |s| progress.store(s as u8, Ordering::Relaxed))
            }?;
            Ok(Arc::new(remote) as Arc<dyn Host>)
        });
    }

    fn connect_using<F>(&mut self, alias: String, window: &mut Window, cx: &mut Context<Self>, connector: impl FnOnce(Arc<AtomicU8>) -> F)
    where
        F: std::future::Future<Output = host::Result<Arc<dyn Host>>> + Send + 'static,
    {
        if crate::docker::is_target(&alias) && !self.docker_available(cx) {
            self.remote.pending_path = None;
            self.persist.opening_recent = None;
            return;
        }
        if crate::docker::is_target(&alias) && !self.container_target_ready(&alias, window, cx) {
            self.remote.pending_path = None;
            self.persist.opening_recent = None;
            return;
        }
        if matches!(self.remote.conn, Conn::Connecting { .. }) || self.refuse_if_dirty(cx) {
            return;
        }
        let return_workspace = if crate::docker::is_target(&alias)
            && let Conn::Connected { alias: source } = &self.remote.conn
            && matches!(host::ssh::target_of(source), host::ssh::Target::Ssh(_))
            && self.remote.host.is_remote()
        {
            Some(crate::container_monitor::ReturnWorkspace { host: self.remote.host.clone(), alias: source.clone(), root: self.root(cx) })
        } else {
            None
        };
        // Own request metadata: another UI action must not change this attempt's folder.
        let pending_path = self.remote.pending_path.take();
        let opening_recent = self.persist.opening_recent.take();
        let terminal_target = self.remote.docker.monitor.browser.terminal_target.take();
        let step = Arc::new(AtomicU8::new(Step::Probing as u8));
        self.remote.connection_origin = Some(std::mem::replace(&mut self.remote.conn, Conn::Connecting { alias: alias.clone(), step: step.clone() }));
        self.notice = None;
        let task = cx.background_executor().spawn(connector(step.clone()));
        // Refresh the step indicator (while connecting).
        let progress = step.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let still = this
                    .update(cx, |this, cx| {
                        let current = matches!(&this.remote.conn, Conn::Connecting { step, .. } if Arc::ptr_eq(step, &progress));
                        if current {
                            cx.notify();
                        }
                        current
                    })
                    .unwrap_or(false);
                if !still {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if !matches!(&this.remote.conn, Conn::Connecting { step: current, .. } if Arc::ptr_eq(current, &step)) {
                    return;
                }
                if crate::docker::is_target(&alias) && !cx.global::<Settings>().docker_enabled {
                    this.cancel_pending_connection();
                    cx.notify();
                    return;
                }
                // Files can become dirty while the network request is in flight.
                if result.is_ok() && this.refuse_if_dirty(cx) {
                    this.cancel_pending_connection();
                    cx.notify();
                    return;
                }
                let origin = this.remote.connection_origin.take().unwrap_or(Conn::Local);
                match result {
                    Ok(remote) => {
                        this.remote.pending_path = pending_path;
                        this.persist.opening_recent = opening_recent;
                        this.remote.docker.monitor.browser.terminal_target = terminal_target;
                        if this.remote.docker.monitor.return_workspace.is_none() {
                            this.remote.docker.monitor.return_workspace = return_workspace;
                        }
                        this.attach_host(remote, alias, window, cx);
                    }
                    Err(e) => {
                        let message: SharedString = e.to_string().into();
                        if let Some(s) = &this.persist.state {
                            let _ = s.record_host_error(&alias, &message);
                        }
                        this.remote.pending_path = None;
                        // Reason and "Reconnect" button are in the toast; the red indicator also stays in the status bar.
                        let why = crate::i18n::tf("Could not connect to {}: {}", &[&alias, &message]);
                        this.notice = Some(Notice::Error(why.clone().into()));
                        // Connection started from a recent: if it can't be reached anymore, ask whether to remove it from the list.
                        if let Some(r) = opening_recent {
                            this.ask_forget_unreachable(r, why, window, cx);
                        }
                        this.persist.opening_recent = None;
                        this.remote.conn = if this.remote.host.is_remote() { origin } else { Conn::Failed { alias, message } };
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Abandon the attempt without abandoning the workspace that is still open.
    pub(super) fn cancel_pending_connection(&mut self) {
        if matches!(self.remote.conn, Conn::Connecting { .. }) {
            self.remote.conn = self.remote.connection_origin.take().unwrap_or(Conn::Local);
            self.remote.pending_path = None;
            self.persist.opening_recent = None;
            self.remote.docker.monitor.browser.terminal_target = None;
        }
    }

    /// Connection established: switch the host, open the remote home directory (or the pending folder).
    pub fn attach_host(&mut self, host: Arc<dyn Host>, alias: String, window: &mut Window, cx: &mut Context<Self>) {
        if crate::docker::is_target(&alias) && !cx.global::<Settings>().docker_enabled {
            self.remote.pending_path = None;
            self.remote.conn = Conn::Local;
            return;
        }
        self.stop_web_preview(cx);
        let info = host.info().clone();
        if crate::docker::is_target(&alias)
            && self.remote.docker.monitor.return_workspace.is_none()
            && let Conn::Connected { alias: source } = &self.remote.conn
            && matches!(host::ssh::target_of(source), host::ssh::Target::Ssh(_))
            && self.remote.host.is_remote()
        {
            self.remote.docker.monitor.return_workspace =
                Some(crate::container_monitor::ReturnWorkspace { host: self.remote.host.clone(), alias: source.clone(), root: self.root(cx) });
        }
        self.close_all_terminals(cx);
        // Save with the old host: persist_session ignores editors belonging to another host.
        self.flush_backups(cx);
        self.persist_session(cx);
        self.persist.workspace_id = None;
        self.remote.host = host;
        self.listen_fs(cx);
        self.listen_status(alias.clone(), cx);
        self.remote.conn = Conn::Connected { alias: alias.clone() };
        if let Some(s) = self.persist.state.as_ref().filter(|_| alias != LOCAL_AGENT_ALIAS) {
            let _ = s.record_host_ok(&alias, info.version, &format!("{}-{}", info.arch, info.os));
        }
        self.shutdown_lsp(cx); // servers were running on the old host
        self.attach_ports(&alias, cx);
        let chosen = self.remote.pending_path.take();
        // A workspace file (from Recents or a restart): the home folder first, then the file opens its own folders.
        let workspace_file = chosen.clone().filter(|p| crate::workspace_file::is_workspace_file(p));
        let chosen = if workspace_file.is_some() { None } else { chosen };
        // From a recent: does the folder still exist on the server? If not, offer to remove it and open the folder picker.
        if let (Some(r), Some(path)) = (self.persist.opening_recent.take(), chosen.clone().or_else(|| workspace_file.clone())) {
            let host = self.remote.host.clone();
            let is_file = workspace_file.is_some();
            let check = cx
                .background_executor()
                .spawn(async move { if is_file { host.stat(&path).map(|_| ()) } else { host.read_dir(&path).map(|_| ()) }.map_err(|e| e.to_string()) });
            cx.spawn_in(window, async move |this, cx| {
                if let Err(why) = check.await {
                    this.update_in(cx, |this, window, cx| {
                        this.ask_forget_unreachable(r, crate::i18n::tf("The folder could not be read on the server: {}", &[&why]), window, cx);
                        this.open_remote_folder(window, cx);
                    })
                    .ok();
                }
            })
            .detach();
        }
        let ask_folder = chosen.is_none() && workspace_file.is_none() && alias != LOCAL_AGENT_ALIAS;
        self.replace_root(chosen.unwrap_or(info.home), cx);
        // If the folder could not be replaced (unsaved files), Search Editors of the old connection must say so.
        self.sync_search_editors(cx);
        if let Some(file) = workspace_file {
            self.open_workspace_file(&file, window, cx);
        }
        self.chrome.panel = Some(Panel::Explorer);
        self.sync_container_monitor(cx);
        if self.remote.docker.monitor.browser.terminal_target.as_ref() == Some(&alias) {
            self.remote.docker.monitor.browser.terminal_target = None;
            self.open_terminal(window, cx);
        }
        window.focus(&self.roots.primary.focus_handle(cx));
        // No folder chosen (new connection): ask which folder to open, like VS Code;
        // Esc leaves it at the home directory.
        if ask_folder {
            self.open_remote_folder(window, cx);
        }
    }

    /// State of the remote connection: drop, reconnect, giving up.
    pub(super) fn listen_status(&mut self, alias: String, cx: &mut Context<Self>) {
        let source = self.remote.host.clone();
        let rx = self.remote.host.status_events();
        self.status_task = Some(cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            let mut rx = ui::bridge(rx);
            while let Some(status) = rx.next().await {
                let alias = alias.clone();
                let ok = this
                    .update(cx, |this, cx| {
                        if !Arc::ptr_eq(&this.remote.host, &source) {
                            return;
                        }
                        // Source-host status still matters, but cannot replace an in-flight target.
                        let pending = if matches!(this.remote.conn, Conn::Connecting { .. }) {
                            Some(std::mem::replace(&mut this.remote.conn, this.remote.connection_origin.take().unwrap_or(Conn::Local)))
                        } else {
                            None
                        };
                        match status {
                            host::ConnStatus::Reconnecting { attempt, .. } => {
                                if attempt == 1 {
                                    this.notice = Some(Notice::Info(crate::i18n::tf("The connection to {} dropped. Reconnecting…", &[&alias]).into()));
                                }
                                this.remote.conn = Conn::Reconnecting { alias, attempt };
                                this.ports_interrupted();
                            }
                            host::ConnStatus::Connected { resumed } => {
                                this.remote.conn = Conn::Connected { alias };
                                this.ports_reconnected(cx);
                                this.reattach_lsp(cx);
                                this.notice = Some(if resumed {
                                    Notice::Info(crate::i18n::t("Reconnected. Terminals kept running.").into())
                                } else {
                                    Notice::Error(crate::i18n::t("Reconnected, but the server session had expired: terminals were closed.").into())
                                });
                            }
                            host::ConnStatus::Lost(e) => {
                                // Permanent loss: say why, that the work is safe, and what to do.
                                let dirty = this.any_dirty(cx);
                                let lost = if dirty {
                                    crate::i18n::tf("Lost the connection to {}: {}. Your open files stay here and unsaved changes are kept, but they cannot be saved until you reconnect (File: Save a Copy to This Computer… keeps one locally).", &[&alias, &e])
                                } else {
                                    crate::i18n::tf("Lost the connection to {}: {}. Your open files stay here.", &[&alias, &e])
                                };
                                this.notice = Some(Notice::Error(lost.into()));
                                this.remote.conn = Conn::Failed { alias, message: crate::i18n::tf("Connection lost: {}", &[&e]).into() };
                                // Forwards wait: "Reconnect" brings them back on the same local ports.
                                this.ports_interrupted();
                                this.stop_ports();
                            }
                        }
                        if let Some(pending) = pending {
                            this.remote.connection_origin = Some(std::mem::replace(&mut this.remote.conn, pending));
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        }));
    }

    /// "Reconnect" (the notification button): re-establish the dropped connection, returning to the open folder.
    pub(super) fn reconnect_lost(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Conn::Failed { alias, .. } = &self.remote.conn else { return };
        let alias = alias.clone();
        self.remote.pending_path = self.root(cx);
        self.connect(alias, window, cx);
    }

    pub(super) fn disconnect(&mut self, cx: &mut Context<Self>) {
        if self.refuse_if_dirty(cx) {
            return;
        }
        self.cancel_pending_connection();
        self.remote.pending_path = None;
        self.persist.opening_recent = None;
        self.invalidate_file_operation();
        self.flush_backups(cx);
        self.persist_session(cx);
        self.shutdown_lsp(cx);
        self.stop_web_preview(cx);
        self.cancel_all_forwards();
        self.stop_ports();
        self.close_all_terminals(cx);
        self.remote.host = Arc::new(host::local());
        self.listen_fs(cx);
        self.status_task = None;
        self.remote.conn = Conn::Local;
        self.remote.docker.monitor.clear();
        self.folder_task = None;
        self.persist.workspace_id = None;
        self.persist.pending_restore = None;
        self.persist.backup_task = None;
        self.persist.backed_up.clear();
        self.roots.extra_roots.clear();
        self.roots.extra_trees.clear();
        self.roots.pending_extra_roots.clear();
        self.roots.workspace = None;
        self.roots.workspace_request += 1;
        self.persist.pending_restore_workspace = None;
        self.cfg.folder_texts.clear();
        self.roots.favorites.clear();
        self.roots.file_index = None;
        self.roots.file_clip = None;
        self.cfg.dirty |= !self.cfg.project.is_empty();
        self.cfg.project.clear();
        self.cfg.folders.clear();
        self.vcs.branch = None;
        self.vcs.status = None;
        self.vcs.refresh = None;
        self.vcs.refreshing = false;
        self.vcs.refresh_again = false;
        self.vcs.scm_root = None;
        self.reset_repos(cx);
        self.vcs.tree_marks.clear();
        self.scm.update(cx, |s, cx| s.set_status(self.remote.host.clone(), None, cx));
        let h = self.remote.host.clone();
        self.search.update(cx, |s, cx| s.set_context(h, None, cx));
        self.close_editor_windows(cx);
        self.close_view_windows(cx);
        self.tabs = Default::default();
        self.tabs.active = None;
        self.tabs.opening = None;
        self.tabs.open_error = None;
        self.tabs.diff = None;
        self.tabs.diff_active = false;
        self.tabs.large = None;
        self.tabs.large_active = false;
        self.tabs.page = None;
        self.tabs.page_active = false;
        self.tabs._page_sub = None;
        let host = self.remote.host.clone();
        self.roots.primary.update(cx, |t, cx| t.set_root(host, None, cx));
        cx.notify();
    }

    /// What the window holds, as Recents names it: (ssh alias, folder path), or the workspace file it was opened through.
    pub(crate) fn workspace_key(&self, cx: &App) -> Option<(Option<String>, String)> {
        let root = self.root(cx)?;
        let path = self.roots.workspace.as_ref().filter(|w| Arc::ptr_eq(&w.host, &self.remote.host)).map_or(root, |w| w.path.clone());
        Some((self.host_alias(), path))
    }

    /// The SSH alias of the window's host (`None` on this computer).
    pub(crate) fn host_alias(&self) -> Option<String> {
        match &self.remote.conn {
            Conn::Connected { alias } | Conn::Reconnecting { alias, .. } if self.remote.host.is_remote() => Some(alias.clone()),
            _ => None,
        }
    }

    /// Upgrade migration: language servers now start only in trusted folders. Folders opened before (servers already
    /// ran) count as trusted once; newly opened ones get the trust prompt.
    pub(crate) fn migrate_trust(state: &State) {
        const KEY: &str = "trust_migrated_v1";
        if state.get(KEY).is_some() {
            return;
        }
        for r in state.recent_workspaces(500).unwrap_or_default() {
            let _ = state.set(&format!("trust:{}:{}", r.host.as_deref().unwrap_or(""), r.path), "1");
        }
        let _ = state.set(KEY, "1");
    }

    /// Key of the trust record: the same path on a different server is a different folder.
    pub(super) fn trust_key(&self, root: &str) -> String {
        let alias = match &self.remote.conn {
            Conn::Connected { alias } | Conn::Reconnecting { alias, .. } if self.remote.host.is_remote() => alias.as_str(),
            _ => "",
        };
        format!("trust:{alias}:{root}")
    }

    /// Is the folder's author trusted? With the setting off, every folder is trusted.
    pub(super) fn is_trusted_root(&self, root: &str, cx: &App) -> bool {
        !cx.global::<Settings>().workspace_trust
            || self.cfg.trusted.contains(&self.trust_key(root))
            || self.persist.state.as_ref().is_some_and(|s| s.get(&self.trust_key(root)).is_some())
    }

    /// Trust the folder: stored permanently, project settings (terminal.shell, lsp.servers) are reapplied.
    pub(super) fn trust_root(&mut self, root: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.trust_roots(&[root.to_string()], ui::file_name(root), window, cx);
    }

    /// Trust several folders in one step (a workspace: its file's folder and its folders), named `name` in the notice.
    fn trust_roots(&mut self, roots: &[String], name: &str, window: &mut Window, cx: &mut Context<Self>) {
        for root in roots {
            self.cfg.trusted.insert(self.trust_key(root));
            if let Some(s) = &self.persist.state {
                let _ = s.set(&self.trust_key(root), "1");
            }
            // Start the language servers held back by the missing trust.
            self.lsp.restricted_notified.remove(root);
        }
        self.reload_config(window, cx);
        self.restart_lsp(cx);
        self.notice = Some(Notice::Info(crate::i18n::tf("Trusted “{}”", &[&name]).into()));
        cx.notify();
    }

    /// What answering yes about `root` trusts: the whole open workspace when `root` is its file's folder or one of its
    /// folders, otherwise that folder alone.
    fn trust_scope(&self, root: &str) -> (Option<String>, Vec<String>) {
        match self.workspace_trust_scope().filter(|(_, dirs)| dirs.iter().any(|d| d == root)) {
            Some((name, dirs)) => (Some(name), dirs),
            None => (None, vec![root.to_string()]),
        }
    }

    /// Before an action that runs commands in an untrusted folder (task, test, debug):
    /// runs immediately if trusted, otherwise after the local confirmation page. `Cancel` runs nothing.
    pub(super) fn require_trust(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let root = self.active_root(cx);
        // A workspace file with tasks or debug configurations is asked about too: it may sit outside every folder.
        let workspace = self.untrusted_workspace_dir(cx).filter(|dir| Some(dir) != root.as_ref());
        self.require_trust_for(root, window, cx, move |this, window, cx| this.require_trust_for(workspace, window, cx, then));
    }

    /// `require_trust` for the folder the commands will run in, when that is not the active file's folder.
    pub(super) fn require_trust_for(
        &mut self,
        root: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(root) = root.filter(|r| !self.is_trusted_root(r, cx)) else { return then(self, window, cx) };
        // A folder of the open workspace is asked about as the workspace, once for all of its folders.
        let (workspace, roots) = self.trust_scope(&root);
        let (title, detail, button, name) = match workspace {
            Some(name) => (
                crate::i18n::t("Do you trust the authors of this workspace?"),
                crate::i18n::tf(
                    "“{}” and its folders can run programs through their tasks, debug configurations and project settings. Only continue if you trust where this workspace came from.",
                    &[&name],
                ),
                crate::i18n::t("Trust Workspace and Continue"),
                name,
            ),
            None => (
                crate::i18n::t("Do you trust the authors of this folder?"),
                crate::i18n::tf(
                    "“{}” can run programs through its tasks, debug configurations and project settings. Only continue if you trust where this folder came from.",
                    &[&ui::file_name(&root)],
                ),
                crate::i18n::t("Trust Folder and Continue"),
                ui::file_name(&root).to_string(),
            ),
        };
        let answer = window.prompt(PromptLevel::Warning, title, Some(&detail), &[button, crate::i18n::t("Cancel")], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| {
                    this.trust_roots(&roots, &name, window, cx);
                    then(this, window, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// "Workspace: Trust This Folder": trust without asking (opened from here when settings blocked it). With a
    /// workspace file open it trusts the workspace: the file's folder and every folder it lists (and the active folder,
    /// should the file not list it).
    pub(super) fn trust_this_folder(&mut self, _: &TrustWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((name, mut roots)) = self.workspace_trust_scope() {
            if let Some(root) = self.active_root(cx).filter(|r| !roots.contains(r)) {
                roots.push(root);
            }
            return self.trust_roots(&roots, &name, window, cx);
        }
        match self.active_root(cx) {
            Some(root) => self.trust_root(&root, window, cx),
            None => {
                self.notice = Some(Notice::Info(crate::i18n::t("Open a folder first.").into()));
                cx.notify();
            }
        }
    }

    /// Add a command run from the palette to "recently used".
    pub(super) fn remember_command(&mut self, label: String) {
        self.recent_commands.retain(|l| *l != label);
        self.recent_commands.insert(0, label);
        self.recent_commands.truncate(8);
        if let Some(s) = &self.persist.state {
            let _ = s.set("recent_commands", &self.recent_commands.join("\n"));
        }
    }
}

#[cfg(test)]
#[path = "workbench_connection_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "workbench_container_live_tests.rs"]
mod live_tests;
