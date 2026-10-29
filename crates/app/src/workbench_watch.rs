//! Workbench: file watching, live reload of the settings file, and git polling.

use super::*;

impl Workbench {
    /// Listen for the host's change events (called again when the host changes).
    pub(super) fn listen_fs(&mut self, cx: &mut Context<Self>) {
        self.watched.clear();
        let rx = self.remote.host.fs_events();
        self.fs_task = Some(cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            // Changes of the same path while the UI is busy merge into one entry (the watched set is small, so is the queue).
            let mut rx = ui::bridge_coalescing(rx, |q, changes| match q.back_mut() {
                Some(pending) => host::merge_fs_changes(pending, changes),
                None => q.push_back(changes),
            });
            while let Some(changes) = rx.next().await {
                if this.update(cx, |this, cx| this.on_fs_changes(changes, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Watch set: open files on this host and folders open in the tree. Recomputed
    /// every render and sent only on change, so nothing gets forgotten.
    pub(super) fn sync_watches(&mut self, cx: &App) {
        let mut paths: Vec<String> = self.tabs.editors.iter().map(|e| e.read(cx)).filter(|e| e.uses_host(&self.remote.host)).map(|e| e.path.clone()).collect();
        if let Some(server) = &self.chrome.web_preview.server {
            paths.extend(server.watched_paths());
        }
        for t in self.all_trees() {
            paths.extend(t.read(cx).expanded_dirs());
        }
        paths.extend(self.roots.extra_roots.iter().cloned());
        paths.extend(self.roots.extra_roots.iter().flat_map(|r| folder_settings_paths(r)));
        paths.extend(self.project_settings_paths(cx).into_iter().map(|(p, ..)| p));
        paths.extend(self.workspace_watch_path());
        // Commit, checkout, `git add` from a terminal: index and HEAD change.
        if let Some(g) = &self.vcs.status {
            // In a worktree/submodule `.git` is a file: watch the real directory.
            let dir = g.git_dir.clone().unwrap_or_else(|| g.abs(".git"));
            paths.push(format!("{dir}/index"));
            paths.push(format!("{dir}/HEAD"));
        }
        paths.sort();
        paths.dedup();
        if paths != self.watched {
            self.remote.host.watch(paths.clone());
            self.watched = paths;
        }
    }

    pub(super) fn on_fs_changes(&mut self, changes: host::FsChanges, cx: &mut Context<Self>) {
        let settings_files: Vec<String> = self.project_settings_paths(cx).into_iter().map(|(p, ..)| p).collect();
        if changes.iter().any(|(p, _)| settings_files.contains(p)) {
            self.load_project_settings(cx);
        }
        // Edited outside (or in an editor tab): read again; our own writes come back unchanged and are ignored.
        if self.workspace_file_changed(&changes) {
            self.reload_workspace(true, cx);
        }
        // If extra folders' settings files changed, only that folder's settings are re-read.
        for root in self.roots.extra_roots.clone() {
            if changes.iter().any(|(p, _)| folder_settings_paths(&root).contains(p)) {
                self.load_folder_settings(root, cx);
            }
        }
        if let Some(g) = &self.vcs.status {
            let prefix = format!("{}/", g.root.trim_end_matches('/'));
            let git_dir = g.git_dir.as_ref().map(|d| format!("{}/", d.trim_end_matches('/')));
            if changes.iter().any(|(p, _)| p.starts_with(&prefix) || git_dir.as_ref().is_some_and(|d| p.starts_with(d.as_str()))) {
                self.refresh_git(300, cx);
            }
        }
        for (path, kind) in changes {
            if let Some(server) = &self.chrome.web_preview.server {
                server.changed(&path);
            }
            if self.reload_in_trees(&path, cx) {
                self.roots.file_index = None;
            }
            let host = self.remote.host.clone();
            for ed in self.all_editors() {
                let matches = {
                    let e = ed.read(cx);
                    e.path == path && e.uses_host(&host)
                };
                if matches {
                    ed.update(cx, |e, cx| e.on_disk_change(kind, cx));
                }
            }
        }
        cx.notify();
    }

    // --- Settings ---

    pub(super) fn config_paths(&self) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let dir = self.cfg.dir.as_ref()?;
        Some((dir.join("settings.jsonc"), dir.join("keybindings.jsonc")))
    }

    /// Read and apply the settings and shortcut files. Warnings go to the status bar.
    pub(super) fn reload_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // If there is no settings folder (tests) only project layers are applied; if there are none either,
        // don't touch anything (don't overwrite externally supplied settings with defaults).
        let paths = self.config_paths();
        let workspace = self.workspace_settings();
        if paths.is_none() && self.cfg.project.is_empty() && workspace.is_none() {
            cx.set_global(settings::Sources::default());
            return;
        }
        let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        if let Some((spath, kpath)) = &paths {
            self.cfg.stamp =
                (mtime(spath), mtime(kpath), mtime(&spath.with_file_name(crate::ext::LOCK_FILE)), crate::ext::watch_stamp(spath.parent().unwrap_or(spath)));
        }
        let kpath = paths.as_ref().map(|(_, k)| k.clone());
        let file = paths.as_ref().and_then(|(s, _)| std::fs::read_to_string(s).ok());
        // Installed extensions first: every reader below takes its entries from them. Only the user's own file can turn
        // them off (`extensions.enabled`), not a project's.
        let ext_enabled = file.as_deref().is_none_or(|f| settings::parse_layers(f, &[]).map_or(true, |(s, _)| s.extensions_enabled));
        let ext_before = crate::ext::schema_entries();
        let mut ext_problems = crate::ext::reload_with(self.cfg.dir.as_deref(), ext_enabled);
        let ext_schemas_changed = ext_before != crate::ext::schema_entries();
        if self.cfg.dir.is_some() {
            self.reload_user_themes();
        }
        self.cfg.dirty = false;
        // The user file as extra folders resolve over it: kept from the last reading that parsed (below).
        let user_text = paths.as_ref().map(|_| file.clone().unwrap_or_else(|| "{}".into()));
        // If the user turned the gate off in their own file (even if not applied yet), every folder is trusted.
        let gate_off = file.as_deref().is_some_and(|f| settings::parse_layers(f, &[]).is_ok_and(|(s, _)| !s.workspace_trust));
        let trusted = gate_off || self.root(cx).is_none_or(|r| self.is_trusted_root(&r, cx));
        // user < workspace file < folder; the workspace file's restricted keys need its own folder to be trusted.
        let mut layers: Vec<settings::ScopedLayer> = Vec::new();
        if let Some((name, text)) = &workspace {
            let trusted = gate_off || self.workspace_settings_trusted(cx);
            layers.push(settings::ScopedLayer { name, text, warn: false, trusted, scope: settings::Scope::Workspace });
        }
        layers.extend(self.cfg.project.iter().map(|(name, text, warn)| settings::ScopedLayer {
            name,
            text,
            warn: *warn,
            trusted,
            scope: settings::Scope::Folder,
        }));
        let (new, mut warnings) = match settings::parse_scoped(file.as_deref().unwrap_or("{}"), &layers) {
            Ok((s, w, sources)) => {
                cx.set_global(sources);
                self.cfg.user_text = user_text;
                (s, w)
            }
            // Half-written file: the last valid settings stay, the error is shown.
            Err(e) => {
                let mut warnings = vec![e];
                warnings.extend(crate::apply_keymap(cx, kpath.as_deref()));
                self.notice = Some(Notice::Error(warnings.join(" · ").into()));
                cx.notify();
                return;
            }
        };
        if new.theme != THEME_AUTO && !self.themes.iter().any(|t| t.id == new.theme) {
            let ids: Vec<&str> = self.themes.iter().map(|t| t.id.as_str()).collect();
            warnings.insert(0, format!("settings.jsonc: {}", crate::i18n::tf("theme “{}” not found (available: auto, {})", &[&new.theme, &ids.join(", ")])));
        }
        // Extension language servers go in front of the user's `lsp.servers`: for the same key the user's entry wins, and
        // a change restarts servers the same way an edit of the settings file does.
        let mut new = new;
        let (servers, server_warnings) = crate::ext::server_entries();
        ext_problems.extend(server_warnings);
        new.lsp_servers.splice(0..0, servers);
        warnings.extend(ext_problems);
        let old = cx.global::<Settings>().clone();
        if !new.docker_enabled && self.any_container_connected(cx) {
            new.docker_enabled = true;
            warnings.push(crate::i18n::t("Disconnect the container before disabling the container module. Your open files are kept.").into());
        } else if !new.docker_enabled {
            self.remote.docker.cancel();
            if matches!(self.tabs.page, Some(Page::Containers)) {
                self.tabs.page = None;
                self.tabs.page_active = false;
                self.chrome.panel = Some(Panel::Explorer);
            }
            if self.chrome.panel == Some(Panel::Containers) {
                self.chrome.panel = Some(Panel::Explorer);
            }
            if matches!(&self.remote.conn, Conn::Connecting { alias, .. } if crate::docker::is_target(alias)) {
                self.cancel_pending_connection();
            }
            if matches!(
                self.overlay.as_ref().map(|o| &o.kind),
                Some(
                    OverlayKind::ContainerEngines { .. }
                        | OverlayKind::DockerContexts { .. }
                        | OverlayKind::DockerContainers { .. }
                        | OverlayKind::DockerPlan { .. }
                )
            ) {
                self.close_overlay(window, cx);
            }
        }
        // Interface scale: user settings only, the same for every window; an unchanged value does nothing. A zoom that
        // could not be saved stays until the file's own value changes.
        let mut session = cx.try_global::<settings::SessionScale>().copied().unwrap_or_default();
        new.ui_scale = session.resolve(new.ui_scale);
        cx.set_global(session);
        cx.set_global(new.clone());
        cx.set_ui_scale(new.ui_scale);
        if !new.web_preview_enabled {
            self.stop_web_preview(cx);
        } else if self.web_preview_needs_restart(&new.web_preview) {
            self.stop_web_preview(cx);
            self.open_web_preview(window, cx);
        }
        if new.docker_enabled && !old.docker_enabled {
            self.sync_container_monitor(cx);
        }
        // UI language: if it changed, the macOS menu bar and Dock menu are rebuilt (other surfaces read it on every render).
        if crate::i18n::apply_setting(&new.ui_language) {
            self.sync_dock_menu(cx);
        }
        self.refresh_speller(cx);
        if ext_schemas_changed
            || (old.schema_associations.as_slice(), old.schema_download, old.schema_validate)
                != (new.schema_associations.as_slice(), new.schema_download, new.schema_validate)
        {
            for ed in self.all_editors() {
                self.schedule_schema(&ed, Duration::ZERO, cx);
            }
        }
        crate::icon_theme::set(crate::icon_theme::IconTheme::from_id(&new.icon_theme).unwrap_or(crate::icon_theme::ALL[0]));
        if kpath.is_some() {
            warnings.extend(crate::apply_keymap(cx, kpath.as_deref()));
        }
        // If the indent setting changed (including project settings), open editors re-resolve it.
        if (old.tab_size, old.insert_spaces) != (new.tab_size, new.insert_spaces) {
            for ed in self.all_editors() {
                ed.update(cx, |e, cx| e.refresh_indent(cx));
            }
        }
        let mut colours_changed = false;
        // Theme: if there is no settings file, the old saved value (state) applies.
        if file.is_some() && !self.cfg.theme_override && new.theme != self.theme_choice {
            self.theme_choice = new.theme.clone();
            let t = self.theme_choice.clone();
            self.apply_theme(&t, window, cx);
            colours_changed = true;
        }
        if old.color_overrides != new.color_overrides {
            let choice = self.theme_choice.clone();
            self.apply_theme(&choice, window, cx);
            colours_changed = true;
        }
        // Global state: write only if it changed in this reload (an unchanged window must not
        // overwrite another window's associations with an empty list).
        if old.file_associations != new.file_associations {
            crate::editor::set_associations(new.file_associations.clone());
        }
        warnings.extend(self.reload_snippets());
        let servers = if old.lsp_enabled != new.lsp_enabled {
            crate::lsp::ServersReload::Restart
        } else {
            crate::lsp::servers_reload(&old.lsp_servers, &new.lsp_servers)
        };
        match servers {
            crate::lsp::ServersReload::Unchanged => {}
            crate::lsp::ServersReload::Settings(keys) => self.change_lsp_settings(&keys, cx),
            crate::lsp::ServersReload::Restart => self.restart_lsp(cx),
        }
        if old.files_exclude != new.files_exclude {
            self.roots.file_index = None;
            for t in self.all_trees() {
                t.update(cx, |t, cx| t.reload_all(cx));
            }
        }
        let view = crate::tree::ViewOptions { compact: new.compact_folders, sort: crate::tree::SortOrder::from_setting(&new.sort_order) };
        for t in self.all_trees() {
            t.update(cx, |t, cx| t.set_options(view, cx));
        }
        self.notice = warnings.first().map(|w| {
            let more = if warnings.len() > 1 { format!(" {}", crate::i18n::tf("(+{} more)", &[&(warnings.len() - 1)])) } else { String::new() };
            Notice::Error(format!("{w}{more}").into())
        });
        crate::system_prefs::sync(cx);
        // A broken setting is the more urgent notice.
        if colours_changed && self.notice.is_none() {
            self.warn_low_contrast(window, cx);
        }
        self.sync_settings_page(cx);
        self.sync_extensions_view();
        self.sync_running_extensions(window, cx);
        // Extra folders resolve over the user and workspace layers, which may have changed.
        self.resolve_folder_settings(cx);
        // Registries the user installed from: updates and version status, their index at most daily (none asked
        // otherwise).
        self.check_extension_updates(window, cx);
        window.refresh();
        cx.notify();
    }

    /// The watcher only sees open files and folders: `>>` in a terminal, a formatter
    /// or a generator rewriting an unopened file would leave git status stale.
    /// While the window is in front, refresh lightly every few seconds.
    pub(super) fn poll_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(4)).await;
                let ok = this
                    .update_in(cx, |this, window, cx| {
                        // Test windows are never active.
                        let active = window.is_window_active() || cfg!(test);
                        if active && this.vcs.status.is_some() && this.git_refresh_idle() {
                            this.refresh_git(0, cx);
                        }
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn git_refresh_idle(&self) -> bool {
        !self.vcs.refreshing
    }

    /// Poll once a second: settings apply right away when the file is saved.
    pub(super) fn watch_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cfg.dir.is_none() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                let ok = this
                    .update_in(cx, |this, window, cx| {
                        let Some((spath, kpath)) = this.config_paths() else { return };
                        let lpath = spath.with_file_name(crate::ext::LOCK_FILE);
                        let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                        if this.cfg.dirty
                            || (mtime(&spath), mtime(&kpath), mtime(&lpath), crate::ext::watch_stamp(spath.parent().unwrap_or(&spath))) != this.cfg.stamp
                        {
                            this.reload_config(window, cx);
                        }
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        })
        .detach();
    }
}
