//! Workbench: the window's title, as the title bar draws it and as the Dock menu lists the windows.

use super::*;

impl Workbench {
    /// "● file — folder" (window title and drawn title bar).
    pub(super) fn title_text(&self, cx: &App) -> Option<String> {
        let root = self.roots.primary.read(cx).root().map(|r| ui::file_name(r).to_string());
        let file = self.tab_editor().map(|e| {
            let e = e.read(cx);
            format!("{}{}", if e.is_dirty() { "● " } else { "" }, e.name)
        });
        let parts: Vec<String> = [file, root].into_iter().flatten().collect();
        (!parts.is_empty()).then(|| parts.join(" — "))
    }

    /// Title layout: `● file — folder — ssh-host` (● unsaved).
    pub(super) fn update_title(&mut self, window: &mut Window, cx: &mut App) {
        let remote = self.remote.host.is_remote().then(|| self.remote.host.label());
        let parts: Vec<String> = [self.title_text(cx), remote].into_iter().flatten().collect();
        let title = if parts.is_empty() { "Codeyard".to_string() } else { parts.join(" — ") };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
            // The Dock menu lists the open windows by title.
            #[cfg(target_os = "macos")]
            crate::menus::refresh_dock_windows(cx);
        }
    }

    /// The window's title as last set (the Dock menu's window rows).
    #[cfg(target_os = "macos")]
    pub(crate) fn window_title(&self) -> &str {
        &self.title
    }
}
