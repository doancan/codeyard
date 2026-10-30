//! Small, frequently used commands: compare with clipboard / saved version, run the selection
//! in the terminal, jump to last edit, scratch file.

use gpui::{AppContext as _, Context, Focusable as _, Window};
use std::io::Write as _;

use super::{
    CompareWithClipboard, CompareWithSaved, ConfigureSnippets, GoToLastEdit, InsertSnippet, NewScratchFile, NewTerminal, Notice, OpenThemesFolder,
    RunSelectedText, SaveThemeAs, Workbench,
};
use crate::ui;

impl Workbench {
    pub(super) fn open_compare(&mut self, title: String, label: &'static str, left: &str, right: &str, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| crate::diff::DiffView::new(title, false, left, right, cx).with_label(label));
        window.focus(&view.focus_handle(cx));
        self.tabs.diff = Some(view);
        self.tabs.diff_active = true;
        self.tabs.page_active = false;
        self.tabs.large_active = false;
        cx.notify();
    }

    /// Text of the active file (including unsaved changes), line endings LF.
    fn active_text(&self, cx: &gpui::App) -> Option<(String, String)> {
        let ed = self.active_editor()?.read(cx);
        Some((ed.path.clone(), ed.text_lines().join("\n")))
    }

    pub(super) fn compare_with_clipboard(&mut self, _: &CompareWithClipboard, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, current)) = self.active_text(cx) else { return };
        let Some(clip) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            self.notice = Some(Notice::Info(crate::i18n::t("The clipboard has no text.").into()));
            cx.notify();
            return;
        };
        let title = crate::i18n::tf("{} ↔ Clipboard", &[&ui::file_name(&path)]);
        self.open_compare(title, crate::i18n::t("Compare"), &clip.replace("\r\n", "\n"), &current, window, cx);
    }

    /// Unsaved changes: compare with the on-disk version.
    pub(super) fn compare_with_saved(&mut self, _: &CompareWithSaved, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, current)) = self.active_text(cx) else { return };
        let host = self.remote.host.clone();
        let p = path.clone();
        let task = cx.background_executor().spawn(async move { host.read(&p).map(|(d, _)| String::from_utf8_lossy(&d).replace("\r\n", "\n")) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(saved) if saved == current => {
                    this.notice = Some(Notice::Info(crate::i18n::t("No unsaved changes: the file matches what is on disk.").into()));
                    cx.notify();
                }
                Ok(saved) => {
                    let title = crate::i18n::tf("{0} (saved) ↔ {1} (editor)", &[&ui::file_name(&path), &ui::file_name(&path)]);
                    this.open_compare(title, crate::i18n::t("Compare"), &saved, &current, window, cx);
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not read {}: {}", &[&path, &e]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Run the selection (or the cursor's line if none) in the active terminal.
    pub(super) fn run_selected_text(&mut self, _: &RunSelectedText, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let text = {
            let e = ed.read(cx);
            match e.selected_text() {
                Some(t) => t,
                None => e.text_lines().get(e.cursor_position().0.saturating_sub(1)).cloned().unwrap_or_default(),
            }
        };
        let text = text.trim_end_matches('\n').to_string();
        if text.trim().is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("Nothing to run: select some text or put the cursor on a line.").into()));
            cx.notify();
            return;
        }
        let payload = format!("{text}\n");
        let existed = self.active_terminal().is_some();
        if !existed {
            self.new_terminal(&NewTerminal, window, cx);
        }
        self.chrome.panel_open = true;
        let delay = if existed { 0 } else { 600 };
        cx.spawn_in(window, async move |this, cx| {
            if delay > 0 {
                cx.background_executor().timer(std::time::Duration::from_millis(delay)).await;
            }
            this.update(cx, |this, cx| {
                if let Some(t) = this.active_terminal() {
                    t.update(cx, |t, _| t.send(&payload));
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn go_to_last_edit(&mut self, _: &GoToLastEdit, window: &mut Window, cx: &mut Context<Self>) {
        match self.nav.last_edit.clone() {
            Some(pos) => {
                self.record_nav(cx);
                self.navigate_to(pos, window, cx);
            }
            None => {
                self.notice = Some(Notice::Info(crate::i18n::t("No edits yet in this session.").into()));
                cx.notify();
            }
        }
    }

    /// On each `Changed` event, if the text version changed, record the cursor as the "last edit".
    pub(super) fn note_edit(&mut self, ed: &gpui::Entity<crate::editor::EditorView>, cx: &gpui::App) {
        let e = ed.read(cx);
        let (path, v) = (e.path.clone(), e.version());
        if self.nav.edit_versions.insert(path.clone(), v).is_some_and(|prev| prev != v) {
            let (line, col) = e.cursor_position();
            self.nav.last_edit = Some((path, line as u32, col as u32));
        }
    }

    /// "Theme: Save Current Colors as User Theme…": asks for a name.
    pub(super) fn start_save_theme(&mut self, _: &SaveThemeAs, window: &mut Window, cx: &mut Context<Self>) {
        self.show_input(
            super::InputPurpose::SaveTheme,
            crate::i18n::t("Save theme"),
            crate::i18n::t("Name of the new theme (it joins the theme list)"),
            "My Theme",
            window,
            cx,
        );
    }

    /// Write the active colors (including color customizations) as `themes/<name>.json` and select it.
    pub(super) fn save_theme_as(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        let slug: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let Some(dir) = self.theme_dir().filter(|_| !slug.is_empty()) else {
            self.notice = Some(Notice::Error(crate::i18n::t("Give the theme a name.").into()));
            cx.notify();
            return;
        };
        let palette = *ui::palette(cx);
        let dark = !ui::luminance_is_light(palette.bg);
        let colors: serde_json::Map<String, serde_json::Value> =
            theme::ROLES.iter().filter_map(|r| palette.get(r).map(|v| (r.to_string(), serde_json::Value::String(format!("#{v:06x}"))))).collect();
        let json = serde_json::json!({ "name": name, "appearance": if dark { "dark" } else { "light" }, "color": colors });
        let path = dir.join(format!("{slug}.json"));
        let result = std::fs::create_dir_all(&dir).and_then(|_| crate::fsutil::write_atomic(&path, serde_json::to_string_pretty(&json).unwrap_or_default()));
        match result {
            Ok(()) => {
                self.reload_user_themes();
                let id = format!("user:{slug}");
                if self.themes.iter().any(|t| t.id == id) {
                    self.theme_choice = id.clone();
                    self.persist_theme(&id);
                    self.apply_theme(&id, window, cx);
                }
                self.notice = Some(Notice::Info(crate::i18n::tf("Saved {} (edit the colors in that file)", &[&path.display()]).into()));
            }
            Err(e) => self.notice = Some(Notice::Error(crate::i18n::tf("Could not write {}: {}", &[&path.display(), &e]).into())),
        }
        cx.notify();
    }

    pub(super) fn open_themes_folder(&mut self, _: &OpenThemesFolder, _: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.theme_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(all(target_os = "macos", not(test)))]
        let _ = std::process::Command::new("open").arg(&dir).spawn();
        self.notice = Some(Notice::Info(crate::i18n::tf("Themes live in {}: drop VS Code or native theme .json files there", &[&dir.display()]).into()));
        cx.notify();
    }

    /// Read the user's snippets (`<settings folder>/snippets/*.json`); warnings are returned.
    pub(super) fn reload_snippets(&mut self) -> Vec<String> {
        let Some(dir) = self.cfg.dir.as_ref().map(|d| d.join("snippets")) else { return Vec::new() };
        let (mut all, mut warnings) = (Vec::new(), Vec::new());
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let lang = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let (list, w) = crate::snippets::parse_user_file(&lang, &text);
            all.extend(list);
            warnings.extend(w);
        }
        // Extensions' snippets after the user's own: for the same prefix, yours wins.
        for ext in &crate::ext::current().active {
            for s in &ext.manifest.contributions.snippets {
                let label = format!("{}: {}", ext.id(), s.path);
                match std::fs::read_to_string(ext.file(&s.path)) {
                    Ok(text) => {
                        let (list, w) = crate::snippets::parse_file(&s.language, &text, Some(ext.id()), &label);
                        all.extend(list);
                        warnings.extend(w);
                    }
                    Err(e) => warnings.push(format!("{label}: {e}")),
                }
            }
        }
        crate::snippets::set_user(all);
        warnings
    }

    /// A snippet file was saved: reread it.
    pub(super) fn reload_snippets_if_edited(&mut self, ed: &gpui::Entity<crate::editor::EditorView>, cx: &mut Context<Self>) {
        let Some(dir) = self.cfg.dir.as_ref().map(|d| d.join("snippets")) else { return };
        if !std::path::Path::new(&ed.read(cx).path).starts_with(&dir) {
            return;
        }
        let warnings = self.reload_snippets();
        self.notice = Some(match warnings.first() {
            Some(w) => Notice::Error(w.clone().into()),
            None => Notice::Info(crate::i18n::t("Snippets reloaded").into()),
        });
        cx.notify();
    }

    pub(super) fn insert_snippet(&mut self, _: &InsertSnippet, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_editor().map(|e| e.read(cx).path.clone()) else { return };
        if self.overlay.is_some() {
            return;
        }
        let list = crate::snippets::for_path(&path);
        let items = list
            .iter()
            .map(|s| crate::picker::PickItem {
                label: s.prefix.clone().into(),
                detail: Some(
                    match (&s.origin, s.user) {
                        (Some(ext), _) => format!("{ext} · {}", s.description),
                        (None, true) => format!("{} · {}", crate::i18n::t("user"), s.description),
                        (None, false) => s.description.clone(),
                    }
                    .into(),
                ),
                keys: None,
            })
            .collect();
        let picker = cx.new(|cx| {
            crate::picker::Picker::new(
                vec![crate::picker::Source::List { prefix: "", items }],
                crate::i18n::t("Insert a snippet (or type its prefix in the editor and press Tab)"),
                "",
                None,
                cx,
            )
        });
        self.show_overlay(picker, super::OverlayKind::Snippets(list), window, cx);
    }

    /// Open the user snippet file for the active file's language (from the template if missing).
    pub(super) fn configure_snippets(&mut self, _: &ConfigureSnippets, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.cfg.dir.clone().or_else(crate::settings::config_dir).map(|d| d.join("snippets")) else { return };
        let lang = self.active_editor().map(|e| crate::snippets::lang_of_path(&e.read(cx).path)).filter(|l| *l != "plaintext").unwrap_or("global");
        let path = dir.join(format!("{lang}.json"));
        let result = std::fs::create_dir_all(&dir).and_then(|_| if path.exists() { Ok(()) } else { std::fs::write(&path, crate::snippets::template(lang)) });
        match result {
            Ok(()) => {
                let host = self.remote.local_host.clone();
                self.open_file_on(host, path.to_string_lossy().into_owned(), window, cx);
            }
            Err(e) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("Could not create {}: {}", &[&path.display(), &e]).into()));
                cx.notify();
            }
        }
    }

    /// Scratch file: under `scratches/` in the settings folder, usable without an open folder.
    pub(super) fn new_scratch_file(&mut self, _: &NewScratchFile, window: &mut Window, cx: &mut Context<Self>) {
        self.show_input(
            super::InputPurpose::Scratch,
            crate::i18n::t("Create scratch file"),
            crate::i18n::t("Name with an extension: the extension picks the language (scratch.md, notes.json…)"),
            "scratch.md",
            window,
            cx,
        );
    }

    pub(super) fn create_scratch(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.create_scratch_with(name, "", window, cx);
    }

    /// Open the first 512 KB of the active file as a hex dump in a scratch tab.
    pub(super) fn open_as_hex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let path = ed.read(cx).path.clone();
        self.open_hex_of(path, window, cx);
    }

    /// Open the hex dump of the given file in a scratch tab (also the button on the binary-file page).
    pub(super) fn open_hex_of(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.remote.host.clone();
        let task = cx.background_executor().spawn(async move { host.read(&path).map(|(d, _)| (path, d)) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((path, data)) => {
                    let name = format!("{}.hex.txt", ui::file_name(&path));
                    this.create_scratch_with(name, &crate::hexdump::dump(&data), window, cx);
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not read the file: {}", &[&e]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Where scratch files live (on this computer, whatever folder is open).
    pub(super) fn scratch_dir(&self) -> std::path::PathBuf {
        self.cfg.dir.clone().or_else(crate::settings::config_dir).unwrap_or_else(std::env::temp_dir).join("scratches")
    }

    /// Scratch file with the given content (e.g. search results). Returns the path it opens under; the tab itself appears once loaded.
    pub(super) fn create_scratch_with(&mut self, name: String, content: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<String> {
        let path = self.write_scratch(name, content, cx)?;
        let host = self.remote.local_host.clone();
        self.open_file_on(host, path.clone(), window, cx);
        Some(path)
    }

    /// A new scratch file (owner-only) holding `content`; `None` with a notice when it cannot be written.
    pub(super) fn write_scratch(&mut self, name: String, content: &str, cx: &mut Context<Self>) -> Option<String> {
        let name = ui::file_name(name.trim()).to_string();
        if name.is_empty() {
            return None;
        }
        let dir = self.scratch_dir();
        // Scratches hold HTTP responses (tokens, personal data) as well as notes: owner-only.
        let result = crate::fsutil::create_private_dir(&dir).map(|_| {
            // If the name exists: "scratch 2.md", "scratch 3.md"…
            let (stem, ext) = name.rsplit_once('.').map_or((name.as_str(), String::new()), |(s, e)| (s, format!(".{e}")));
            let mut path = dir.join(&name);
            let mut n = 2;
            while path.exists() {
                path = dir.join(format!("{stem} {n}{ext}"));
                n += 1;
            }
            path
        });
        match result.and_then(|p| crate::fsutil::create_private(&p)?.write_all(content.as_bytes()).map(|_| p)) {
            Ok(path) => Some(ui::local_path(&path)),
            Err(e) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("Could not create the scratch file: {}", &[&e]).into()));
                cx.notify();
                None
            }
        }
    }

    #[cfg(test)]
    pub fn close_diff_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_diff(window, cx);
    }

    #[cfg(test)]
    pub fn save_theme_for_test(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.save_theme_as(name.to_string(), window, cx);
    }

    #[cfg(test)]
    pub fn outline_for_test(&self) -> Vec<(String, usize)> {
        self.outline.symbols.iter().map(|s| (s.name.clone(), s.line)).collect()
    }

    #[cfg(test)]
    pub fn last_edit_for_test(&self) -> Option<(String, u32, u32)> {
        self.nav.last_edit.clone()
    }

    #[cfg(test)]
    pub fn create_scratch_for_test(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.create_scratch(name.to_string(), window, cx);
    }
}
