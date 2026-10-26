//! Workbench: back/forward navigation, new file, save as.

use super::*;

impl Workbench {
    /// The active editor's position.
    pub(super) fn current_nav(&self, cx: &App) -> Option<NavPos> {
        let e = self.active_editor()?.read(cx);
        let (line, col) = e.cursor_position();
        Some((e.path.clone(), line as u32, col.saturating_sub(1) as u32))
    }

    /// Push the current position onto the back stack before a jump.
    pub(super) fn record_nav(&mut self, cx: &App) {
        if self.nav.navigating {
            return;
        }
        let Some(pos) = self.current_nav(cx) else { return };
        if self.nav.back.last() != Some(&pos) {
            self.nav.back.push(pos);
            if self.nav.back.len() > 50 {
                self.nav.back.remove(0);
            }
        }
        self.nav.forward.clear();
    }

    /// Go to a position (without writing to history).
    pub(super) fn navigate_to(&mut self, (path, line, col): NavPos, window: &mut Window, cx: &mut Context<Self>) {
        self.nav.navigating = true;
        self.tabs.pending_reveal = Some((path.clone(), line, col, 0));
        match self.tabs.editors.iter().position(|e| e.read(cx).path == path) {
            Some(ix) => self.activate(ix, window, cx),
            None => self.open_file(path, window, cx),
        }
        self.nav.navigating = false;
    }

    pub(super) fn go_back(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.nav.back.pop() else { return };
        if let Some(cur) = self.current_nav(cx) {
            self.nav.forward.push(cur);
        }
        self.navigate_to(target, window, cx);
    }

    pub(super) fn go_forward(&mut self, _: &GoForward, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.nav.forward.pop() else { return };
        if let Some(cur) = self.current_nav(cx) {
            self.nav.back.push(cur);
        }
        self.navigate_to(target, window, cx);
    }

    pub(super) fn reopen_closed_editor(&mut self, _: &ReopenClosedEditor, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.nav.closed.pop() else { return };
        self.record_nav(cx);
        self.navigate_to(target, window, cx);
    }

    /// ⌘N: new file in the active file's folder (or the open folder if there is none).
    pub(super) fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a folder first (⌘O), then create files in it.").into()));
            cx.notify();
            return;
        };
        let roots = self.all_roots(cx);
        let base = self
            .target_dir_any(cx)
            .or_else(|| self.active_editor().map(|e| parent_of(&e.read(cx).path)))
            .filter(|d| roots.iter().any(|r| d.starts_with(r.as_str())))
            .or_else(|| self.active_root(cx))
            .unwrap_or(root);
        self.show_input(
            InputPurpose::NewFile { base },
            crate::i18n::t("Create file"),
            crate::i18n::t("File name (a path like src/new.rs works too)"),
            "",
            window,
            cx,
        );
    }

    /// ⇧⌘S: save the active file's contents to another path.
    pub(super) fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor() else { return };
        let from = ed.read(cx).path.clone();
        let initial = self.rel_in_roots(&from, cx).unwrap_or_else(|| from.clone());
        self.show_input(
            InputPurpose::SaveAs { from },
            crate::i18n::t("Save as"),
            crate::i18n::t("Path (relative to the folder, or absolute)"),
            &initial,
            window,
            cx,
        );
    }

    /// Write if the target doesn't exist (never overwrites an existing file), then the tab switches to the new file;
    /// the old file stays on disk as it was.
    pub(super) fn save_as_to(&mut self, from: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.tabs.editors.iter().find(|e| e.read(cx).path == from).cloned() else { return };
        let root = self.root(cx).unwrap_or_else(|| parent_of(&from));
        let target = if name.starts_with('/') { name } else { ui::join(&root, &name) };
        if target == from {
            ed.update(cx, |e, cx| e.save_now(cx));
            return;
        }
        let data = match ed.read(cx).encoded_bytes() {
            Ok(d) => d,
            Err(msg) => {
                self.notice = Some(Notice::Error(msg.into()));
                cx.notify();
                return;
            }
        };
        let written_version = ed.read(cx).version();
        let host = self.remote.host.clone();
        let t = target.clone();
        let task = cx.background_executor().spawn(async move {
            if host.stat(&t).is_ok() {
                return Err(crate::i18n::tf("{} already exists. Choose another name.", &[&t]));
            }
            host.write(&t, data, None).map(|_| ()).map_err(|e| crate::i18n::tf("Could not save: {}", &[&e]))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        this.roots.primary.update(cx, |t, cx| t.reload(&parent_of(&target), cx));
                        this.roots.file_index = None;
                        // The old tab's unsaved changes live on in the new file, so close it: unless more was typed while the write was in
                        // flight, which only the old tab has.
                        let kept_typing = ed.read(cx).version() != written_version;
                        if !kept_typing && let Some(ix) = this.tabs.editors.iter().position(|e| *e == ed) {
                            this.remove_editor(ix, window, cx);
                        }
                        this.open_file(target.clone(), window, cx);
                        // After the open, which has its own things to say: this is the message that must stay on screen.
                        if kept_typing {
                            let note = crate::i18n::tf(
                                "Saved as {}. You kept typing while it was saving, so the original tab stays open with your latest changes.",
                                &[&target],
                            );
                            this.notice = Some(Notice::Info(note.into()));
                        }
                    }
                    Err(e) => {
                        this.notice = Some(Notice::Error(e.into()));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Over SSH: save the active file's contents to a file on this computer (the tab stays remote).
    pub(super) fn save_local_copy(&mut self, _: &SaveLocalCopy, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor() else { return };
        if !self.remote.host.is_remote() {
            self.notice = Some(Notice::Info(crate::i18n::t("This file is already on this computer. Use Save As instead.").into()));
            cx.notify();
            return;
        }
        let from = ed.read(cx).path.clone();
        let home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default();
        let initial = ui::join(&ui::join(&home, "Downloads"), ui::file_name(&from));
        self.show_input(
            InputPurpose::SaveLocalCopy { from },
            crate::i18n::t("Save copy"),
            crate::i18n::t("Path on this computer (~ is your home folder)"),
            &initial,
            window,
            cx,
        );
    }

    /// Write if the local target doesn't exist (never overwrites an existing file). A relative path is resolved against the home directory.
    pub(super) fn save_local_copy_to(&mut self, from: String, name: String, cx: &mut Context<Self>) {
        let Some(ed) = self.tabs.editors.iter().find(|e| e.read(cx).path == from).cloned() else { return };
        let target = match local_copy_target(&name, dirs::home_dir().as_deref()) {
            Ok(t) => t,
            Err(msg) => {
                self.notice = Some(Notice::Error(msg.into()));
                cx.notify();
                return;
            }
        };
        let data = match ed.read(cx).encoded_bytes() {
            Ok(d) => d,
            Err(msg) => {
                self.notice = Some(Notice::Error(msg.into()));
                cx.notify();
                return;
            }
        };
        let local = self.remote.local_host.clone();
        let t = target.clone();
        let task = cx.background_executor().spawn(async move {
            if local.stat(&t).is_ok() {
                return Err(crate::i18n::tf("{} already exists. Choose another name.", &[&t]));
            }
            local.write(&t, data, None).map(|_| ()).map_err(|e| crate::i18n::tf("Could not save: {}", &[&e]))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.notice = Some(match result {
                    Ok(()) => Notice::Info(crate::i18n::tf("Saved a copy to {}", &[&target]).into()),
                    Err(e) => Notice::Error(e.into()),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
