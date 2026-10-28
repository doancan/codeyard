//! Workbench: context menus and menu commands.

use super::*;

impl Workbench {
    /// Run the action immediately (from buttons; regardless of where focus is).
    pub(super) fn dispatch_now(&mut self, action: &dyn gpui::Action, window: &mut Window, cx: &mut Context<Self>) {
        window.dispatch_action(action.boxed_clone(), cx);
    }

    // --- File operations (tree context menu) ---

    pub(super) fn open_context_menu(&mut self, target: MenuTarget, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let is_root = self.all_roots(cx).contains(&target.path);
        let local_mac = !self.remote.host.is_remote() && cfg!(target_os = "macos");
        let remote = matches!(self.remote.conn, Conn::Connected { .. }) && self.remote.host.is_remote();
        let key = |a: &dyn gpui::Action| Command { label: String::new(), run: Run::Action(a.boxed_clone()) }.shortcut(cx);
        let mut items = vec![MenuItem::new(crate::i18n::t("New File…")), MenuItem::new(crate::i18n::t("New Folder…"))];
        let mut commands = vec![MenuCmd::NewFile, MenuCmd::NewFolder];
        items.push(MenuItem::new(crate::i18n::t("Cut")).shortcut(key(&crate::tree::CutEntry)).enabled(!is_root).separator());
        commands.push(MenuCmd::CutEntry);
        items.push(MenuItem::new(crate::i18n::t("Copy")).shortcut(key(&crate::tree::CopyEntry)).enabled(!is_root));
        commands.push(MenuCmd::CopyEntry);
        items.push(MenuItem::new(crate::i18n::t("Paste")).shortcut(key(&crate::tree::PasteEntry)).enabled(self.roots.file_clip.is_some()));
        commands.push(MenuCmd::PasteEntry);
        items.push(MenuItem::new(crate::i18n::t("Rename…")).shortcut(key(&crate::tree::RenameEntry)).enabled(!is_root).separator());
        commands.push(MenuCmd::Rename);
        items.push(MenuItem::new(crate::i18n::t("Delete")).shortcut(key(&crate::tree::DeleteEntry)).enabled(!is_root));
        commands.push(MenuCmd::Delete);
        items.push(MenuItem::new(crate::i18n::t("Duplicate")).enabled(!is_root));
        commands.push(MenuCmd::Duplicate);
        if target.is_dir {
            items.push(MenuItem::new(crate::i18n::t("Expand All Inside")).separator());
            commands.push(MenuCmd::ExpandAllInside);
            items.push(MenuItem::new(crate::i18n::t("Collapse All Inside")));
            commands.push(MenuCmd::CollapseAllInside);
            items.push(MenuItem::new(crate::i18n::t("Find in Folder…")));
            commands.push(MenuCmd::FindInFolder);
            // --- compare ---
            match &self.roots.compare_folder_a {
                Some(a) if a.path != target.path => {
                    items.push(MenuItem::new(crate::i18n::t("Compare Folder with Selected")).separator());
                    commands.push(MenuCmd::CompareFolderWithSelected);
                    items.push(MenuItem::new(crate::i18n::t("Select Folder for Compare")));
                }
                _ => items.push(MenuItem::new(crate::i18n::t("Select Folder for Compare")).separator()),
            }
            commands.push(MenuCmd::SelectFolderForCompare);
        }
        if !target.is_dir {
            items.push(MenuItem::new(crate::i18n::t("Open to the Side")).separator());
            commands.push(MenuCmd::OpenSide);
            items.push(MenuItem::new(crate::i18n::t("Follow (Tail)")));
            commands.push(MenuCmd::Follow);
            match &self.roots.compare_a {
                Some(a) if *a != target.path => {
                    items.push(MenuItem::new(crate::i18n::t("Compare with Selected")));
                    commands.push(MenuCmd::CompareWithSelected);
                }
                _ => {}
            }
            items.push(MenuItem::new(crate::i18n::t("Select for Compare")));
            commands.push(MenuCmd::SelectForCompare);
        }
        if remote {
            items.push(MenuItem::new(crate::i18n::t("Download…")).separator());
            commands.push(MenuCmd::Download);
            if target.is_dir {
                items.push(MenuItem::new(crate::i18n::t("Upload Files Here…")));
                commands.push(MenuCmd::Upload);
            }
        }
        items.push(MenuItem::new(crate::i18n::t("Open in Integrated Terminal")).separator());
        commands.push(MenuCmd::OpenTerminal);
        items.push(MenuItem::new(crate::i18n::t("Copy Path")).separator());
        commands.push(MenuCmd::CopyPath);
        items.push(MenuItem::new(crate::i18n::t("Copy Relative Path")).enabled(!is_root));
        commands.push(MenuCmd::CopyRelativePath);
        let fav = self.rel(&target.path, cx).is_some_and(|r| self.roots.favorites.contains(&r));
        items.push(MenuItem::new(if fav { crate::i18n::t("Remove from Favorites") } else { crate::i18n::t("Add to Favorites") }).enabled(!is_root).separator());
        commands.push(MenuCmd::Favorite);
        // --- git-refs ---
        if !is_root && self.git_status().is_some() {
            items.push(MenuItem::new(crate::i18n::t("Add to .gitignore")).separator());
            commands.push(MenuCmd::GitIgnore);
        }
        if local_mac {
            items.push(MenuItem::new(crate::i18n::t("Reveal in Finder")).separator());
            commands.push(MenuCmd::Reveal);
        }
        self.show_menu(items, commands, Vec::new(), target, position, window, cx);
    }

    /// The tab's right-click menu.
    pub(super) fn open_tab_menu(&mut self, ix: usize, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.tabs.editors.get(ix) else { return };
        let (path, pinned) = {
            let e = ed.read(cx);
            (e.path.clone(), e.pinned)
        };
        let local_mac = ed.read(cx).uses_host(&self.remote.local_host) && cfg!(target_os = "macos");
        let key = |a: &dyn gpui::Action| Command { label: String::new(), run: Run::Action(a.boxed_clone()) }.shortcut(cx);
        let n = self.tabs.editors.len();
        let mut items = vec![
            MenuItem::new(crate::i18n::t("Close")).shortcut(key(&CloseEditor)),
            MenuItem::new(crate::i18n::t("Close Others")).enabled(n > 1),
            MenuItem::new(crate::i18n::t("Close to the Right")).enabled(ix + 1 < n),
            MenuItem::new(crate::i18n::t("Close Saved")),
            MenuItem::new(if pinned { crate::i18n::t("Unpin") } else { crate::i18n::t("Pin") }).separator(),
            MenuItem::new(crate::i18n::t("Follow (Tail)")).separator(),
            MenuItem::new(crate::i18n::t("Copy Path")),
            MenuItem::new(crate::i18n::t("Copy Relative Path")),
            MenuItem::new(if self.rel(&path, cx).is_some_and(|r| self.roots.favorites.contains(&r)) {
                crate::i18n::t("Remove from Favorites")
            } else {
                crate::i18n::t("Add to Favorites")
            })
            .separator(),
        ];
        let mut commands = vec![
            MenuCmd::CloseTab,
            MenuCmd::CloseOthers,
            MenuCmd::CloseRight,
            MenuCmd::CloseSaved,
            MenuCmd::TogglePin,
            MenuCmd::Follow,
            MenuCmd::CopyPath,
            MenuCmd::CopyRelativePath,
            MenuCmd::Favorite,
        ];
        // --- git-refs ---
        if self.git_status().is_some() {
            items.push(MenuItem::new(crate::i18n::t("Open Changes")).separator());
            commands.push(MenuCmd::GitOpenChanges);
            items.push(MenuItem::new(crate::i18n::t("Open File (HEAD)")));
            commands.push(MenuCmd::GitOpenHead);
        }
        if local_mac {
            items.push(MenuItem::new(crate::i18n::t("Reveal in Finder")).separator());
            commands.push(MenuCmd::Reveal);
        }
        let target = MenuTarget { path, is_dir: false, tab: Some(ix) };
        self.show_menu(items, commands, Vec::new(), target, position, window, cx);
    }

    /// The editor's right-click menu (VS Code order: navigation, replace, clipboard, palette).
    pub(super) fn open_editor_menu(&mut self, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_editor().is_none() {
            return;
        }
        use crate::editor as e;
        let entries: Vec<(&str, Box<dyn gpui::Action>, bool)> = vec![
            // (label, action, separator before)
            (crate::i18n::t("Go to Definition"), Box::new(e::GoToDefinition), false),
            (crate::i18n::t("Go to Type Definition"), Box::new(e::GoToTypeDefinition), false),
            (crate::i18n::t("Go to Implementations"), Box::new(e::GoToImplementation), false),
            (crate::i18n::t("Find All References"), Box::new(FindReferences), false),
            (crate::i18n::t("Go to Symbol in Editor…"), Box::new(GoToSymbol), false),
            (crate::i18n::t("Rename Symbol"), Box::new(RenameSymbol), true),
            (crate::i18n::t("Change All Occurrences"), Box::new(e::SelectAllOccurrences), false),
            (crate::i18n::t("Format Document"), Box::new(FormatDocument), false),
            (crate::i18n::t("Quick Fix…"), Box::new(CodeActions), false),
            (crate::i18n::t("Cut"), Box::new(e::Cut), true),
            (crate::i18n::t("Copy"), Box::new(e::Copy), false),
            (crate::i18n::t("Paste"), Box::new(e::Paste), false),
            (crate::i18n::t("Toggle Line Comment"), Box::new(e::ToggleComment), true),
            (crate::i18n::t("Toggle Bookmark"), Box::new(e::ToggleBookmark), false),
            (crate::i18n::t("View File History"), Box::new(GitFileHistory), true),
            (crate::i18n::t("Blame Line"), Box::new(GitBlameLine), false),
            // --- git-refs ---
            (crate::i18n::t("Open Changes"), Box::new(GitOpenChanges), false),
            (crate::i18n::t("Open File (HEAD)"), Box::new(GitOpenFileHead), false),
            (crate::i18n::t("Compare with Branch…"), Box::new(GitCompareBranch), false),
            (crate::i18n::t("Compare with Commit…"), Box::new(GitCompareCommit), false),
            (crate::i18n::t("Command Palette…"), Box::new(CommandPalette), true),
        ];
        self.show_action_menu(entries, position, window, cx);
    }

    /// Context menu from an action list: (label, action, separator before). Shortcuts
    /// are read from the user's key bindings; the chosen action goes to the focused item.
    pub(super) fn show_action_menu(
        &mut self,
        entries: Vec<(&str, Box<dyn gpui::Action>, bool)>,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = |a: &dyn gpui::Action| Command { label: String::new(), run: Run::Action(a.boxed_clone()) }.shortcut(cx);
        let mut items = Vec::new();
        let mut commands = Vec::new();
        let mut actions = Vec::new();
        for (i, (label, action, sep)) in entries.into_iter().enumerate() {
            let item = MenuItem::new(label).shortcut(key(action.as_ref()));
            items.push(if sep { item.separator() } else { item });
            commands.push(MenuCmd::Dispatch(i));
            actions.push(action);
        }
        let target = MenuTarget { path: String::new(), is_dir: false, tab: None };
        self.show_menu(items, commands, actions, target, position, window, cx);
    }

    /// Like `show_action_menu`, but `position` is where the menu's `anchor` corner sits: a menu that opens upward next to a
    /// button asks for `Corner::BottomLeft`, so it grows away from the button instead of being pushed over it.
    pub(super) fn show_action_menu_at(
        &mut self,
        entries: Vec<(&str, Box<dyn gpui::Action>, bool)>,
        position: gpui::Point<gpui::Pixels>,
        anchor: gpui::Corner,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_action_menu(entries, position, window, cx);
        if let Some(state) = &self.menu {
            state.menu.update(cx, |menu, _| menu.set_anchor(anchor));
        }
    }

    /// The gear button's menu, grouped like VS Code's Manage menu: commands, then preferences, then About. The `bool` puts a
    /// separator in front of the entry.
    pub(super) fn manage_menu_entries() -> Vec<(&'static str, Box<dyn gpui::Action>, bool)> {
        use crate::i18n::t;
        vec![
            (t("Command Palette…"), Box::new(CommandPalette), false),
            (t("Settings"), Box::new(OpenSettingsPage), true),
            (t("Keyboard Shortcuts"), Box::new(OpenKeybindings), false),
            (t("Color Theme…"), Box::new(SelectTheme), false),
            (t("File Icon Theme…"), Box::new(SelectIconTheme), false),
            (t("About Codeyard"), Box::new(ShowAbout), true),
        ]
    }

    /// Where the Manage menu's bottom-left corner goes: right of the activity bar, level with the bottom of the gear button
    /// (the button is the last item of the bar, which ends where the status bar starts).
    pub(super) fn manage_menu_origin(viewport_height: gpui::Pixels, status_bar: bool) -> gpui::Point<gpui::Pixels> {
        let status = if status_bar { layout::STATUSBAR_H } else { 0. };
        gpui::point(px(layout::ACTIVITYBAR_W + 2.), viewport_height - px(status))
    }

    /// The gear button: opens the Manage menu above-right of it.
    pub(super) fn open_manage_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let status_bar = self.chrome.zen.is_none() && cx.global::<Settings>().status_bar;
        let origin = Self::manage_menu_origin(window.viewport_size().height, status_bar);
        self.show_action_menu_at(Self::manage_menu_entries(), origin, gpui::Corner::BottomLeft, window, cx);
    }

    pub(super) fn show_menu(
        &mut self,
        items: Vec<MenuItem>,
        commands: Vec<MenuCmd>,
        actions: Vec<Box<dyn gpui::Action>>,
        target: MenuTarget,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = cx.new(|cx| ContextMenu::new(items, cx));
        let sub = cx.subscribe_in(&menu, window, |this, _, event: &MenuEvent, window, cx| {
            if let MenuEvent::Horizontal(forward) = event {
                this.move_application_menu(*forward, window, cx);
                return;
            }
            let Some(state) = this.close_menu(window, cx) else { return };
            if let MenuEvent::Select(i) = event {
                match state.commands[*i] {
                    // Focus returned from the menu (editor): the action goes there.
                    MenuCmd::Dispatch(a) => window.dispatch_action(state.actions[a].boxed_clone(), cx),
                    MenuCmd::AppSubmenu(index) => {
                        if let Some(mut path) = state.app_path {
                            path.push(index);
                            this.open_application_menu(path, state.position, window, cx);
                        }
                    }
                    cmd => this.run_menu_cmd(cmd, state.target, window, cx),
                }
            }
        });
        let return_focus = window.focused(cx);
        window.focus(&menu.focus_handle(cx));
        self.scm.update(cx, |scm, cx| scm.set_menu_open(true, cx));
        self.menu = Some(MenuState { menu, position, target, commands, actions, return_focus, app_path: None, _sub: sub });
        cx.notify();
    }

    pub(super) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<MenuState> {
        let state = self.menu.take()?;
        self.scm.update(cx, |scm, cx| scm.set_menu_open(false, cx));
        match &state.return_focus {
            Some(f) => window.focus(f),
            None => window.focus(&self.focus_handle),
        }
        cx.notify();
        Some(state)
    }

    pub(super) fn run_menu_cmd(&mut self, cmd: MenuCmd, target: MenuTarget, window: &mut Window, cx: &mut Context<Self>) {
        // If it isn't a folder, the new item goes into the same folder.
        let dir = if target.is_dir { target.path.clone() } else { parent_of(&target.path) };
        match cmd {
            MenuCmd::NewFile => self.start_create(dir, false, window, cx),
            MenuCmd::NewFolder => self.start_create(dir, true, window, cx),
            MenuCmd::Rename => self.start_rename(target.path, window, cx),
            MenuCmd::Delete => self.confirm_delete(target.path, target.is_dir, window, cx),
            MenuCmd::CopyPath => cx.write_to_clipboard(gpui::ClipboardItem::new_string(target.path)),
            MenuCmd::CopyRelativePath => {
                let rel = self.rel_in_roots(&target.path, cx).unwrap_or(target.path);
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(rel));
            }
            MenuCmd::CutEntry => self.roots.file_clip = Some((target.path, true)),
            MenuCmd::CopyEntry => self.roots.file_clip = Some((target.path, false)),
            MenuCmd::PasteEntry => self.paste_entry(dir, window, cx),
            // sent in show_menu (the action list is there).
            MenuCmd::Dispatch(_) | MenuCmd::AppSubmenu(_) => {}
            MenuCmd::Graph(option) => self.scm.update(cx, |scm, cx| scm.apply_graph_option(option, window, cx)),
            MenuCmd::Port(c) => self.run_port_menu(c, target.path.parse().unwrap_or(0), window, cx),
            MenuCmd::Remote(c) => self.run_remote_menu(c, target.path, window, cx),
            MenuCmd::Debug(c) => self.run_debug_menu(c, target, window, cx),
            MenuCmd::Container(action) => self.run_container_action(action, &target.path, window, cx),
            MenuCmd::ContainerTool(command) => self.run_container_tool(command, window, cx),
            // Without touching the copy clipboard: "name copy.ext" in the same folder.
            MenuCmd::Duplicate => {
                let previous = self.roots.file_clip.replace((target.path.clone(), false));
                self.paste_entry(parent_of(&target.path), window, cx);
                self.roots.file_clip = previous;
            }
            MenuCmd::OpenTerminal => self.new_terminal_in(Some(dir), window, cx),
            MenuCmd::SelectForCompare => {
                self.notice = Some(Notice::Info(
                    crate::i18n::tf("Selected {} for compare. Right-click another file and choose Compare.", &[&ui::file_name(&target.path)]).into(),
                ));
                self.roots.compare_a = Some(target.path);
                cx.notify();
            }
            MenuCmd::CompareWithSelected => {
                if let Some(a) = self.roots.compare_a.take() {
                    self.compare_paths(a, target.path, window, cx);
                }
            }
            // --- compare ---
            MenuCmd::SelectFolderForCompare => self.select_folder_for_compare(target.path, cx),
            MenuCmd::CompareFolderWithSelected => self.compare_folder_with_selected(target.path, window, cx),
            MenuCmd::Follow => self.follow_path(target.path, window, cx),
            MenuCmd::OpenSide => {
                self.open_file(target.path, window, cx);
                self.split_editor(&SplitEditor, window, cx);
            }
            MenuCmd::ExpandAllInside => self.tree_for(&target.path, cx).update(cx, |t, cx| t.expand_recursive(&target.path, cx)),
            MenuCmd::CollapseAllInside => self.tree_for(&target.path, cx).update(cx, |t, cx| t.collapse_recursive(&target.path, cx)),
            MenuCmd::FindInFolder => {
                let rel = self.rel(&target.path, cx).filter(|r| !r.is_empty());
                self.chrome.panel = Some(Panel::Search);
                let glob = rel.map_or("**".to_string(), |r| format!("{r}/**"));
                self.search.update(cx, |s, cx| s.limit_to(&glob, window, cx));
                cx.notify();
            }
            MenuCmd::CopyHash => cx.write_to_clipboard(gpui::ClipboardItem::new_string(target.path)),
            MenuCmd::CopySubject => {
                let subject = self.scm.read(cx).commit_by_hash(&target.path).map(|c| c.subject).unwrap_or_default();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(subject));
            }
            MenuCmd::CommitFiles => {
                if let Some(c) = self.scm.read(cx).commit_by_hash(&target.path) {
                    self.show_commit_files(c, window, cx);
                }
            }
            MenuCmd::CommitBranch => self.show_input(
                InputPurpose::CommitBranch { hash: target.path },
                crate::i18n::t("Create branch"),
                crate::i18n::t("Name of the new branch (starts at this commit)"),
                "",
                window,
                cx,
            ),
            MenuCmd::CommitTag => self.show_input(
                InputPurpose::CommitTag { hash: target.path },
                crate::i18n::t("Create tag"),
                crate::i18n::t("Tag name, then an optional message for an annotated tag"),
                "",
                window,
                cx,
            ),
            MenuCmd::CommitCheckout | MenuCmd::CommitCherryPick | MenuCmd::CommitRevert | MenuCmd::CommitReset(_) => {
                use crate::graph::CommitOp;
                let op = match cmd {
                    MenuCmd::CommitCheckout => CommitOp::Checkout,
                    MenuCmd::CommitCherryPick => CommitOp::CherryPick,
                    MenuCmd::CommitRevert => CommitOp::Revert,
                    MenuCmd::CommitReset(m) => CommitOp::Reset(m),
                    _ => return,
                };
                self.scm.update(cx, |s, cx| s.commit_op(target.path, op, window, cx));
            }
            // --- git-refs ---
            MenuCmd::GitIgnore => self.git_ignore_path(target.path, target.is_dir, cx),
            MenuCmd::GitOpenChanges => self.git_open_changes(target.path, window, cx),
            MenuCmd::GitOpenHead => self.git_open_head_file(target.path, window, cx),
            MenuCmd::ScmFile(action) => self.run_scm_file_action(action, target.path, window, cx),
            MenuCmd::Download => self.start_download(target.path.clone(), target.is_dir, window, cx),
            MenuCmd::Upload => self.start_upload(target.path.clone(), window, cx),
            MenuCmd::Reveal => {
                let _ = std::process::Command::new("open").arg("-R").arg(&target.path).spawn();
            }
            MenuCmd::CloseTab => {
                if let Some(ix) = target.tab {
                    self.close_editor_at(ix, window, cx);
                }
            }
            MenuCmd::CloseOthers => {
                let keep = target.tab;
                let ix: Vec<usize> = (0..self.tabs.editors.len()).filter(|i| Some(*i) != keep).collect();
                self.close_tabs(ix, window, cx);
            }
            MenuCmd::CloseRight => {
                let from = target.tab.map_or(self.tabs.editors.len(), |t| t + 1);
                self.close_tabs((from..self.tabs.editors.len()).collect(), window, cx);
            }
            MenuCmd::CloseSaved => {
                let ix: Vec<usize> = (0..self.tabs.editors.len()).filter(|i| !self.tabs.editors[*i].read(cx).is_dirty()).collect();
                self.close_tabs(ix, window, cx);
            }
            MenuCmd::Favorite => self.toggle_favorite(&target.path, cx),
            MenuCmd::TogglePin => {
                if let Some(ix) = target.tab {
                    self.toggle_pin(ix, cx);
                }
            }
        }
    }

    /// Bulk close: pinned and dirty tabs stay (so changes aren't lost silently).
    pub(super) fn close_tabs(&mut self, mut indices: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) {
        indices.sort_unstable();
        let mut kept_dirty = 0;
        for ix in indices.into_iter().rev() {
            let e = self.tabs.editors[ix].read(cx);
            if e.pinned {
                continue;
            }
            if e.is_dirty() && !self.also_in_other(&self.tabs.editors[ix]) {
                kept_dirty += 1;
                continue;
            }
            self.remove_editor(ix, window, cx);
        }
        if kept_dirty > 0 {
            self.notice = Some(Notice::Info(
                crate::i18n::tn("{} tab with unsaved changes stayed open.", "{} tabs with unsaved changes stayed open.", kept_dirty, &[]).into(),
            ));
        }
        cx.notify();
    }

    /// Pin/unpin: pinned tabs go on the left, the order among them is preserved.
    pub(super) fn toggle_pin(&mut self, ix: usize, cx: &mut Context<Self>) {
        let active = self.tabs.active.and_then(|a| self.tabs.editors.get(a)).cloned();
        let ed = self.tabs.editors[ix].clone();
        ed.update(cx, |e, _| {
            e.pinned = !e.pinned;
            e.preview = false;
        });
        let (mut pinned, rest): (Vec<_>, Vec<_>) = self.tabs.editors.drain(..).partition(|e| e.read(cx).pinned);
        pinned.extend(rest);
        self.tabs.editors = pinned;
        self.tabs.active = active.and_then(|a| self.tabs.editors.iter().position(|e| *e == a));
        self.persist_session(cx);
        cx.notify();
    }
}
