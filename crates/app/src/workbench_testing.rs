//! Workbench: accessors and shortcuts used only by tests (not part of production code).

use super::*;

impl Workbench {
    /// Recents in display order: (host, path, pinned).
    pub fn recent_for_test(&self) -> Vec<(Option<String>, String, bool)> {
        super::projects::ordered_recent(&self.persist.recent, &self.persist.pinned).into_iter().map(|(r, pinned)| (r.host, r.path, pinned)).collect()
    }

    /// Write a folder into history (without connecting).
    pub fn record_recent_for_test(&self, host: Option<&str>, path: &str) {
        if let Some(s) = &self.persist.state {
            let _ = s.record_workspace(host, path);
        }
    }

    pub fn toggle_pin_recent_for_test(&mut self, host: Option<&str>, path: &str, cx: &mut Context<Self>) {
        let r = RecentWorkspace { host: host.map(str::to_string), path: path.into(), last_opened_at: 0 };
        self.toggle_pin_recent(r, cx);
    }

    pub fn forget_recent_for_test(&mut self, host: Option<&str>, path: &str, cx: &mut Context<Self>) {
        let r = RecentWorkspace { host: host.map(str::to_string), path: path.into(), last_opened_at: 0 };
        self.forget_recent(r, cx);
    }

    pub fn reconnect_lost_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reconnect_lost(window, cx);
    }

    pub fn is_trusted_root_for_test(&self, root: &str, cx: &App) -> bool {
        self.is_trusted_root(root, cx)
    }

    pub fn apply_workspace_edit_for_test(&mut self, edit: &serde_json::Value, cx: &mut Context<Self>) -> bool {
        self.apply_workspace_edit(edit, cx)
    }

    /// Shows a response the way a finished request does.
    pub fn show_response_for_test(&mut self, r: crate::http_client::Response, window: &mut Window, cx: &mut Context<Self>) {
        let prepared = crate::http_client::Prepared { method: "GET".into(), url: "http://a.test/".into(), headers: Vec::new(), body: None };
        self.show_response(None, &prepared, r, window, cx);
    }

    /// The file the response tab is backed by, once the tab is open.
    pub fn response_tab_for_test(&self) -> Option<String> {
        self.lsp.http_tab.clone()
    }

    pub fn discard_response_on_quit_for_test(&mut self) {
        self.discard_response_on_quit();
    }

    pub fn bottom_panel_open_for_test(&self) -> bool {
        self.chrome.panel_open
    }

    pub fn set_data_dir(&mut self, dir: std::path::PathBuf) {
        self.persist.data_dir = Some(dir);
    }

    // --- Session and backups (hot exit) ---

    pub fn theme_picker_open(&self) -> bool {
        matches!(self.overlay, Some(Overlay { kind: OverlayKind::Theme { .. }, .. }))
    }

    pub fn palette_open(&self) -> bool {
        matches!(self.overlay, Some(Overlay { kind: OverlayKind::Palette { .. }, .. }))
    }

    pub fn active_path(&self, cx: &App) -> Option<String> {
        self.active_editor().map(|e| e.read(cx).path.clone())
    }

    pub fn active_cursor(&self, cx: &App) -> Option<(usize, usize)> {
        self.active_editor().map(|e| e.read(cx).cursor_position())
    }

    pub fn tree_event_for_test(&mut self, ev: crate::tree::TreeEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The real handler, so tests exercise the path the app runs rather than a copy.
        self.on_tree_event(&ev, window, cx);
    }

    pub fn set_notice_for_test(&mut self, text: &str, error: bool, cx: &mut Context<Self>) {
        self.notice = Some(if error { Notice::Error(text.to_string().into()) } else { Notice::Info(text.to_string().into()) });
        cx.notify();
    }

    pub fn menu_labels(&self, cx: &App) -> Option<Vec<String>> {
        self.menu.as_ref().map(|m| m.menu.read(cx).labels())
    }

    pub fn open_manage_menu_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_manage_menu(window, cx);
    }

    pub fn menu_anchor_for_test(&self, cx: &App) -> Option<gpui::Corner> {
        self.menu.as_ref().map(|m| m.menu.read(cx).anchor())
    }

    pub fn menu_position_for_test(&self) -> Option<gpui::Point<gpui::Pixels>> {
        self.menu.as_ref().map(|m| m.position)
    }

    pub fn menu_separators_for_test(&self, cx: &App) -> Option<Vec<bool>> {
        self.menu.as_ref().map(|m| m.menu.read(cx).separators())
    }

    pub fn manage_menu_origin_for_test(viewport_height: gpui::Pixels, status_bar: bool) -> gpui::Point<gpui::Pixels> {
        Self::manage_menu_origin(viewport_height, status_bar)
    }

    pub fn open_editor_menu_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_editor_menu(gpui::point(px(300.), px(200.)), window, cx);
    }

    pub fn close_menu_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(window, cx);
    }

    pub fn open_tab_menu_for_test(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab_menu(ix, gpui::point(px(120.), px(40.)), window, cx);
    }

    pub fn select_menu_for_test(&mut self, label: &str, cx: &mut Context<Self>) {
        let menu = self.menu.as_ref().expect("menu is not open").menu.clone();
        menu.update(cx, |m, cx| m.select_for_test(label, cx));
    }

    pub fn terminal_cwds_for_test(&self, cx: &App) -> Vec<Option<String>> {
        self.terminals.iter().map(|t| t.read(cx).cwd_for_test()).collect()
    }

    pub fn current_theme_id(&self) -> &str {
        &self.theme_choice
    }

    /// Open a permanent tab inside the open folder (without changing the root).
    pub fn open_in_root(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(path.to_string(), window, cx);
    }

    pub fn preview_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.remote.host.clone();
        self.open_file_as(host, path.to_string(), true, window, cx);
    }

    pub fn tab_flags(&self, cx: &App) -> Vec<(String, bool, bool)> {
        self.tabs
            .editors
            .iter()
            .map(|e| {
                let e = e.read(cx);
                (ui::file_name(&e.path).to_string(), e.preview, e.pinned)
            })
            .collect()
    }

    /// Tab menu commands (without opening the menu with the mouse in a test).
    pub fn tab_command(&mut self, ix: usize, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        let cmd = match cmd {
            "pin" => MenuCmd::TogglePin,
            "close-others" => MenuCmd::CloseOthers,
            "close-right" => MenuCmd::CloseRight,
            "close-saved" => MenuCmd::CloseSaved,
            _ => MenuCmd::CloseTab,
        };
        let path = self.tabs.editors[ix].read(cx).path.clone();
        self.run_menu_cmd(cmd, MenuTarget { path, is_dir: false, tab: Some(ix) }, window, cx);
    }

    pub fn open_paths(&self, cx: &App) -> Vec<String> {
        self.tabs.editors.iter().map(|e| e.read(cx).path.clone()).collect()
    }

    pub fn active_editor_entity(&self) -> Option<Entity<EditorView>> {
        self.active_editor().cloned()
    }

    pub fn open_error_text(&self) -> Option<String> {
        self.tabs.open_error.as_ref().map(|(_, m)| m.to_string())
    }

    pub fn open_hex_of_for_test(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_hex_of(path, window, cx);
    }

    pub fn tree_names(&self, cx: &App) -> Vec<String> {
        self.roots.primary.read(cx).row_names()
    }

    /// "local", "connecting", "connected", "reconnecting" or "failed: <message>".
    pub fn conn_for_test(&self) -> String {
        match &self.remote.conn {
            Conn::Local => "local".into(),
            Conn::Connecting { .. } => "connecting".into(),
            Conn::Connected { .. } => "connected".into(),
            Conn::Reconnecting { .. } => "reconnecting".into(),
            Conn::Failed { message, .. } => format!("failed: {message}"),
        }
    }

    pub fn sidebar_width_for_test(&self) -> f32 {
        self.chrome.sidebar_w
    }

    pub fn set_sidebar_width_for_test(&mut self, w: f32) {
        self.chrome.sidebar_w = w;
    }

    pub fn zen_for_test(&self) -> bool {
        self.chrome.zen.is_some()
    }

    pub fn panel_for_test(&self) -> Option<&'static str> {
        self.chrome.panel.map(|p| match p {
            Panel::Explorer => "explorer",
            Panel::Search => "search",
            Panel::Scm => "scm",
            Panel::Debug => "debug",
            Panel::Testing => "testing",
            Panel::Remote => "remote",
        })
    }

    pub fn search_summary_for_test(&self, cx: &App) -> Option<(usize, usize)> {
        self.search.read(cx).summary_for_test()
    }

    pub fn search_results_for_test(&self, cx: &App) -> Vec<(String, usize)> {
        self.search.read(cx).results_for_test()
    }

    pub fn search_state_for_test(&self, cx: &App) -> (String, bool, bool) {
        self.search.read(cx).state_for_test(cx)
    }

    pub fn tree_selected_for_test(&self, cx: &App) -> Option<String> {
        self.roots.primary.read(cx).selected_path()
    }

    pub fn tree_guide_for_test(&self, cx: &App) -> Option<(usize, usize, usize)> {
        self.roots.primary.read(cx).guide_for_test()
    }

    pub fn tree_reveal_dir_for_test(&mut self, path: &str, cx: &mut Context<Self>) {
        self.roots.primary.update(cx, |t, cx| t.reveal_dir(path, cx));
    }

    pub fn reload_config_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_config(window, cx);
    }

    pub fn tree_clear_selection_for_test(&mut self, cx: &mut Context<Self>) {
        self.roots.primary.update(cx, |t, cx| t.clear_selection(cx));
    }

    pub fn tree_target_for_test(&self, cx: &App) -> Option<String> {
        self.roots.primary.read(cx).target_dir()
    }

    pub fn tree_expand_for_test(&mut self, path: &str, recursive: bool, cx: &mut Context<Self>) {
        self.roots.primary.update(cx, |t, cx| if recursive { t.expand_recursive(path, cx) } else { t.collapse_recursive(path, cx) });
    }

    pub fn saved_tree_for_test(&self) -> Option<String> {
        self.persist.state.as_ref()?.get(&format!("tree:{}", self.persist.workspace_id?))
    }

    pub fn section_for_test(&mut self, id: &'static str, open: bool, height: Option<f32>) {
        self.sections.set_open(id, open);
        if let Some(h) = height {
            self.sections.set_height(id, h);
        }
        self.sections_text_for_test();
    }

    pub fn sections_text_for_test(&self) -> String {
        self.sections.encode()
    }

    pub fn tree_options_for_test(&mut self, compact: bool, sort: &str, cx: &mut Context<Self>) {
        let opts = crate::tree::ViewOptions { compact, sort: crate::tree::SortOrder::from_setting(sort) };
        self.roots.primary.update(cx, |t, cx| t.set_options(opts, cx));
    }

    pub fn favorite_list(&self) -> Vec<String> {
        self.roots.favorites.clone()
    }

    pub fn root_for_test(&self, cx: &App) -> Option<String> {
        self.root(cx)
    }

    pub fn tree_selected(&self, cx: &App) -> Option<String> {
        self.roots.primary.read(cx).selected_path()
    }

    pub fn active_selection(&self, cx: &App) -> Option<String> {
        self.active_editor().and_then(|e| e.read(cx).selected_text())
    }

    pub fn search_panel(&self) -> Entity<SearchPanel> {
        self.search.clone()
    }

    /// Run a command by its palette name (for tests).
    pub fn command_labels_for_test(&self) -> Vec<String> {
        self.commands().into_iter().map(|c| c.label).collect()
    }

    /// Welcome tips (for the translation table test).
    pub fn tips_for_test() -> &'static [&'static str] {
        welcome_glue::TIPS
    }

    pub fn tip_for_test(day: u64) -> &'static str {
        welcome_glue::tip_of_the_day(day)
    }

    pub fn run_command_for_test(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let run = self.commands().into_iter().find(|c| c.label == label).unwrap_or_else(|| panic!("no such command: {label}")).run;
        self.run_command(run, window, cx);
    }

    pub fn focus_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.roots.primary.focus_handle(cx));
    }

    /// (open editor count, startup error)
    pub fn open_state(&self) -> (usize, Option<String>) {
        (self.tabs.editors.len(), self.tabs.open_error.as_ref().map(|(_, m)| m.to_string()))
    }

    pub fn overlay_labels(&self, cx: &App) -> Option<Vec<String>> {
        self.overlay.as_ref().map(|o| o.picker.read(cx).labels())
    }

    pub fn remote_folder_open(&self) -> bool {
        matches!(self.overlay, Some(Overlay { kind: OverlayKind::RemoteFolder { .. }, .. }))
    }

    pub fn overlay_query(&self, cx: &App) -> Option<String> {
        self.overlay.as_ref().map(|o| o.picker.read(cx).query().to_string())
    }

    pub fn set_overlay_query_for_test(&mut self, query: String, cx: &mut Context<Self>) {
        if let Some(o) = &self.overlay {
            o.picker.update(cx, |p, cx| p.set_query(query, cx));
        }
    }

    pub fn overlay_details(&self, cx: &App) -> Option<Vec<String>> {
        self.overlay.as_ref().map(|o| o.picker.read(cx).details())
    }

    /// Tab drag and drop: groups left/right, tabs by name.
    pub fn drop_tab_for_test(&mut self, from_left: bool, name: &str, to_left: bool, before: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let group = |this: &Self, left: bool| -> Vec<Entity<EditorView>> {
            if left == this.tabs.focused_left { this.tabs.editors.clone() } else { this.tabs.other.as_ref().map(|g| g.editors.clone()).unwrap_or_default() }
        };
        let find = |v: &[Entity<EditorView>], n: &str| v.iter().find(|e| ui::file_name(&e.read(cx).path) == n).cloned();
        let ed = find(&group(self, from_left), name).expect("no such tab");
        let before = before.and_then(|b| find(&group(self, to_left), b));
        let (from_focused, to_focused) = (from_left == self.tabs.focused_left, to_left == self.tabs.focused_left);
        self.drop_tab(ed, from_focused, to_focused, before, window, cx);
    }

    pub fn groups(&self, cx: &App) -> (Vec<String>, Option<Vec<String>>, bool) {
        let names = |v: &[Entity<EditorView>]| v.iter().map(|e| ui::file_name(&e.read(cx).path).to_string()).collect::<Vec<_>>();
        let (focused, other) = (names(&self.tabs.editors), self.tabs.other.as_ref().map(|g| names(&g.editors)));
        if self.tabs.focused_left { (focused, other, true) } else { (other.unwrap_or_default(), Some(focused), false) }
    }

    /// Open without changing the root (for tests; same path as ⌘P / the tree).
    pub fn open_file_in_root(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(path, window, cx);
    }

    /// File transfer (without the confirmation dialog): upload items to a folder on the server.
    pub fn upload_for_test(&mut self, alias: &str, items: Vec<std::path::PathBuf>, remote_dir: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        let job = super::transfer::Job::Upload { alias: alias.into(), items, remote_dir: remote_dir.to_string_lossy().into_owned() };
        self.begin(job, window, cx);
    }

    pub fn download_for_test(
        &mut self,
        alias: &str,
        remote: &std::path::Path,
        is_dir: bool,
        local_dir: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let job = super::transfer::Job::Download { alias: alias.into(), remote: remote.to_string_lossy().into_owned(), is_dir, local_dir: local_dir.into() };
        self.begin(job, window, cx);
    }

    pub fn drop_external_for_test(&mut self, items: Vec<std::path::PathBuf>, dir: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_external(items, dir.to_string_lossy().into_owned(), window, cx);
    }

    /// `test`: probe the connection; otherwise remove it (a confirmation dialog appears).
    pub fn run_host_op_for_test(&mut self, test: bool, alias: String, window: &mut Window, cx: &mut Context<Self>) {
        self.run_host_op(if test { super::hosts::HostOp::Test } else { super::hosts::HostOp::Remove }, alias, window, cx);
    }

    pub fn finish_add_ssh_host_for_test(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_add_ssh_host(text, window, cx);
    }

    /// Rebuild spell checking from the settings (no settings file in tests).
    pub fn refresh_speller_for_test(&mut self, cx: &mut Context<Self>) {
        self.refresh_speller(cx);
    }

    pub fn spell_choice_for_test(&mut self, editor: &Entity<EditorView>, range: std::ops::Range<usize>, word: &str, add: bool, cx: &mut Context<Self>) {
        let choice = if add { super::spell_glue::SpellChoice::AddToDictionary } else { super::spell_glue::SpellChoice::IgnoreOnce };
        self.apply_spell_choice(editor.downgrade(), range, word.to_string(), choice, cx);
    }

    /// Pull request action (the list comes from the network; run directly here): 0 fetch, 1 open, 2 link, 3 branch name.
    pub fn pull_action_for_test(&mut self, remote: &str, number: u64, head: &str, action: u8, cx: &mut Context<Self>) {
        let pull = crate::github::Pull {
            number,
            title: "t".into(),
            author: "a".into(),
            head: head.into(),
            base: "main".into(),
            draft: false,
            updated: String::new(),
            url: format!("https://github.com/o/r/pull/{number}"),
            review: None,
            checks: None,
        };
        let action = [
            super::pulls_glue::PullAction::Checkout,
            super::pulls_glue::PullAction::Open,
            super::pulls_glue::PullAction::CopyLink,
            super::pulls_glue::PullAction::CopyBranch,
        ][action as usize];
        self.run_pull_action(remote.to_string(), pull, action, cx);
    }

    pub fn open_large_for_test(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_large(path.to_string(), window, cx);
    }

    pub fn follow_path_for_test(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_path(path.to_string(), window, cx);
    }

    pub fn filter_log_for_test(&mut self, path: &str, pattern: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.run_filter_log(path.to_string(), pattern.to_string(), window, cx);
    }

    /// Key-based login setup (without the confirmation dialog): takes a test directory and the `authorized_keys` path on the server.
    pub fn key_setup_for_test(&mut self, alias: &str, ssh_dir: std::path::PathBuf, authorized_keys: &str, cx: &mut Context<Self>) {
        self.run_key_setup(alias.to_string(), Some(ssh_dir), Some(authorized_keys.to_string()), cx);
    }

    pub fn finish_edit_host_field_for_test(&mut self, alias: &str, field: host::ssh_config::Field, value: &str, cx: &mut Context<Self>) {
        self.finish_edit_host_field(alias.to_string(), field, value.to_string(), cx);
    }

    pub fn start_download_for_test(&mut self, remote: String, window: &mut Window, cx: &mut Context<Self>) {
        self.start_download(remote, false, window, cx);
    }

    pub fn http_busy_for_test(&self) -> bool {
        self.lsp.http_busy
    }

    /// The host gate shared by `.http` requests and schema downloads.
    pub fn net_gate_for_test(&mut self, cx: &gpui::App) -> Arc<schema::gate::HostGate> {
        self.schema_service(cx).store.gate().clone()
    }

    /// Ask about the hosts that background schema downloads are waiting for (normally done after an analysis).
    pub fn ask_schema_hosts_for_test(&mut self, cx: &mut Context<Self>) {
        let svc = self.schema_service(cx);
        self.ask_schema_hosts(&svc, cx);
    }

    pub fn close_tabs_for_test(&mut self, indices: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tabs(indices, window, cx);
    }

    pub fn start_upload_for_test(&mut self, dir: String, window: &mut Window, cx: &mut Context<Self>) {
        self.start_upload(dir, window, cx);
    }

    pub fn set_spell_root_for_test(&mut self, root: std::path::PathBuf) {
        self.lsp.spell_root = Some(root);
    }

    // --- Updates ---

    pub fn set_update_backend_for_test(&mut self, backend: Arc<dyn crate::update_backend::UpdateBackend>) {
        self.update.backend = backend;
    }

    /// The updater's state as one line: `idle`, `checking`, `up-to-date`, `available 0.2.0`, `downloading 0.2.0`, `installing 0.2.0`,
    /// `ready 0.2.0` or `error <kind>`.
    pub fn update_state_for_test(&self) -> String {
        use update_glue::UpdateState as S;
        match &self.update.state {
            S::Idle => "idle".into(),
            S::Checking => "checking".into(),
            S::UpToDate => "up-to-date".into(),
            S::Available(r) => format!("available {}", r.version),
            S::Downloading { release, .. } => format!("downloading {}", release.version),
            S::Installing { release } => format!("installing {}", release.version),
            S::Ready { version, .. } => format!("ready {version}"),
            S::Error(f) => format!("error {:?}", f.kind).to_lowercase(),
        }
    }

    /// What the update dialog shows right now (`None`: closed).
    pub fn update_dialog_for_test(&self, cx: &App) -> Option<DialogView> {
        use update_glue::Body;
        let m = self.dialog_model(cx)?;
        Some(DialogView {
            title: m.title.to_string(),
            subtitle: m.subtitle.as_ref().map(|s| s.to_string()),
            text: m.text(),
            progress: match &m.body {
                Body::Progress { fraction, .. } => Some(*fraction),
                Body::Working(_) => Some(None),
                _ => None,
            },
            buttons: m.buttons.iter().map(|b| b.label.to_string()).collect(),
            primary: m.buttons.iter().find(|b| b.primary).map(|b| b.label.to_string()),
        })
    }

    /// Press the dialog button with this label (as a click would).
    pub fn press_update_button_for_test(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let action =
            self.dialog_model(cx).and_then(|m| m.buttons.into_iter().find(|b| b.label == label)).unwrap_or_else(|| panic!("no “{label}” button")).action;
        self.update_action(action, window, cx);
    }

    /// Show the dialog in a window of its own, as on macOS (windows made by tests draw the card over the editor otherwise).
    pub fn use_update_window_for_test(&mut self) {
        self.update.native = true;
    }

    /// The platform's question "may the dialog's window close?", asked from outside any update like the real one.
    pub fn dialog_may_close_for_test(host: gpui::WindowHandle<Workbench>, cx: &mut App) -> bool {
        update_window_glue::dialog_may_close(host, cx)
    }

    /// The dialog's own window, while it is open.
    pub fn update_window_for_test(&self, cx: &App) -> Option<gpui::AnyWindowHandle> {
        self.update.window.filter(|w| update_glue::window_is_open(*w, cx)).map(Into::into)
    }

    /// Esc, as the dialog's own window sends it.
    pub fn update_escape_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_cmd(update_glue::UpdateCmd::Dismiss, window, cx);
    }

    /// The status bar item: (label, tooltip).
    pub fn update_status_for_test(&self) -> Option<(String, String)> {
        self.update_status_item().map(|i| (i.label.to_string(), i.tip.to_string()))
    }

    pub fn open_update_dialog_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_update_dialog(window, cx);
    }

    pub fn check_if_due_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.check_if_due(window, cx);
    }

    pub fn background_check_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_check(false, window, cx);
    }

    /// The first-run question; `false`: something was in the way.
    pub fn ask_update_policy_for_test(&mut self, window_active: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.ask_update_policy(window_active, window, cx)
    }

    pub fn schedule_update_checks_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.schedule_update_checks(window, cx);
    }

    /// What quitting does to a running update: the flag that says its thread is done.
    pub fn cancel_update_for_quit_for_test(&mut self) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        self.cancel_update_for_quit()
    }

    /// Forget everything the updater knows (a new session of the same window).
    pub fn reset_update_for_test(&mut self) {
        self.update.run = None;
        self.update.state = update_glue::UpdateState::Idle;
        self.update.dialog = update_glue::Dialog::Closed;
    }

    /// Label of the dialog button that has the keyboard focus.
    pub fn focused_update_button_for_test(&self, window: &Window, cx: &App) -> Option<String> {
        let model = self.dialog_model(cx)?;
        let ix = self.update.focus.iter().position(|h| h.is_focused(window))?;
        model.buttons.get(ix).map(|b| b.label.to_string())
    }

    /// How many blocks the notes are drawn as, and the text of the last one.
    pub fn update_notes_for_test(&self) -> (usize, String) {
        let last = self.update.notes.last().map(|b| crate::markdown::plain_text(std::slice::from_ref(b))).unwrap_or_default();
        (self.update.notes.len(), last)
    }

    pub fn notes_focused_for_test(&self, window: &Window) -> bool {
        self.update.notes_focus.is_focused(window)
    }

    /// How far the release notes are scrolled (0 at the top, negative below it) and how far they can be.
    pub fn notes_scroll_for_test(&self) -> (f32, f32) {
        (f32::from(self.update.notes_scroll.offset().y), f32::from(self.update.notes_scroll.max_offset().height))
    }

    pub fn open_palette_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette("", window, cx);
    }

    pub fn state_value_for_test(&self, key: &str) -> Option<String> {
        self.persist.state.as_ref().and_then(|s| s.get(key))
    }

    pub fn set_state_value_for_test(&self, key: &str, value: &str) {
        self.persist.state.as_ref().expect("a state database").set(key, value).expect("state write");
    }

    /// Save As without the prompt: the write is only spawned, so a test can type before it completes.
    pub fn save_as_for_test(&mut self, from: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as_to(from, name, window, cx);
    }

    pub fn has_state_for_test(&self) -> bool {
        self.persist.state.is_some()
    }

    pub fn notice_for_test(&self) -> Option<(bool, String)> {
        match &self.notice {
            Some(Notice::Info(m)) => Some((false, m.to_string())),
            Some(Notice::Error(m)) => Some((true, m.to_string())),
            None => None,
        }
    }
}

/// What a test sees of the update dialog.
#[derive(Debug, PartialEq)]
pub struct DialogView {
    pub title: String,
    pub subtitle: Option<String>,
    /// Title and body text, one line each.
    pub text: String,
    /// `Some(None)`: working without a known end.
    pub progress: Option<Option<f32>>,
    pub buttons: Vec<String>,
    pub primary: Option<String>,
}
