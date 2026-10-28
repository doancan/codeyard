//! Workbench: theme selection, persistence and the picker.

use super::*;

/// A rejected theme file as a translated, one-line notice text (the theme crate's own `Display` is English).
pub(super) fn theme_load_error(path: &std::path::Path, error: &themes::ThemeError) -> String {
    use crate::i18n::tf;
    use themes::ThemeError::*;
    let reason = match error {
        Read(e) => e.clone(),
        Json(e) => tf("invalid JSON: {}", &[e]),
        NotAnObject => tf("a theme must be a JSON object", &[]),
        UnknownFormat => tf("unrecognized theme format (expected “color” or “colors”)", &[]),
        ColorNotAnObject => tf("“color” must be an object", &[]),
        ColorNotAString { role } => tf("{}: the color must be a string", &[role]),
        InvalidColor { role, value } => tf("{}: invalid color {}", &[role, value]),
        UnknownRole(role) => tf("unknown role: {}", &[role]),
    };
    format!("{}: {reason}", path.display())
}

/// What "Auto" picks while the system asks for more contrast.
const HIGH_CONTRAST_DARK: &str = "high-contrast-dark";
const HIGH_CONTRAST_LIGHT: &str = "high-contrast-light";

impl Workbench {
    /// Re-read user themes (after an import).
    /// User themes folder: under the given settings folder (temporary in tests), else the default.
    pub(super) fn theme_dir(&self) -> Option<std::path::PathBuf> {
        self.cfg.dir.as_ref().map(|d| d.join("themes")).or_else(user_theme_dir)
    }

    pub(super) fn reload_user_themes(&mut self) {
        let mut all = themes::builtin();
        if let Some(dir) = self.theme_dir() {
            all.extend(themes::load_dir(&dir).0);
        }
        self.themes = all;
    }

    /// Write the theme choice to the settings file (comments preserved).
    pub(super) fn persist_theme(&mut self, id: &str) {
        match self.config_paths() {
            Some((spath, _)) => {
                let result = settings::ensure_file(&spath, settings::TEMPLATE)
                    .and_then(|_| std::fs::read_to_string(&spath))
                    .and_then(|text| crate::fsutil::write_atomic(&spath, settings::set_string_in(&text, "theme", id)));
                if let Err(e) = result {
                    self.notice = Some(Notice::Error(crate::i18n::tf("Could not save the theme: {}", &[&e]).into()));
                }
            }
            None => {
                if let Some(s) = &self.persist.state {
                    let _ = s.set("theme", id);
                }
            }
        }
    }

    // --- Themes ---

    pub(super) fn resolve_theme(&self, choice: &str, window: &Window, cx: &App) -> &Theme {
        let id = if choice == THEME_AUTO {
            let light = matches!(window.appearance(), gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight);
            // The system's Increase contrast (macOS) or a high-contrast theme (Windows) picks the high-contrast pair.
            match (light, crate::system_prefs::increase_contrast(cx)) {
                (true, false) => themes::DEFAULT_LIGHT,
                (false, false) => themes::DEFAULT_DARK,
                (true, true) => HIGH_CONTRAST_LIGHT,
                (false, true) => HIGH_CONTRAST_DARK,
            }
        } else {
            choice
        };
        self.themes
            .iter()
            .find(|t| t.id == id)
            .or_else(|| self.themes.iter().find(|t| t.id == themes::DEFAULT_DARK))
            .expect("embedded themes are always present")
    }

    pub(super) fn apply_theme(&mut self, choice: &str, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.resolve_theme(choice, window, cx);
        let (mut palette, id, name) = (t.palette, t.id.clone(), t.name.clone());
        // `workbench.colorCustomizations`: first those that apply to every theme, then those specific to this theme.
        let overrides = &cx.global::<Settings>().color_overrides;
        let mine = |scope: &Option<String>| scope.as_ref().is_some_and(|s| s.eq_ignore_ascii_case(&id) || s.eq_ignore_ascii_case(&name));
        for (_, role, color) in overrides.iter().filter(|(s, ..)| s.is_none()) {
            palette.set(role, *color);
        }
        for (_, role, color) in overrides.iter().filter(|(s, ..)| mine(s)) {
            palette.set(role, *color);
        }
        cx.set_global(ActiveTheme(palette));
        window.refresh();
        cx.notify();
    }

    /// After a theme is chosen or the colour settings change: say so when the colours in use (theme plus
    /// `workbench.colorCustomizations`) fall below WCAG AA. Built-in themes pass on their own; a theme you wrote or a
    /// customization may not.
    pub(super) fn warn_low_contrast(&mut self, window: &Window, cx: &App) {
        let failures = themes::palette_contrast_failures(&cx.global::<ActiveTheme>().0);
        let Some((fg, bg, ratio, min)) = failures.first() else { return };
        let name = self.resolve_theme(&self.theme_choice, window, cx).name.clone();
        let msg = crate::i18n::tn(
            "Theme “{1}”: {0} colour pair is below WCAG AA contrast ({2} on {3}: {4}:1, needs {5}:1)",
            "Theme “{1}”: {0} colour pairs are below WCAG AA contrast, for example {2} on {3}: {4}:1, needs {5}:1",
            failures.len(),
            &[&name, fg, bg, &format!("{ratio:.1}"), &format!("{min:.1}")],
        );
        self.notice = Some(Notice::Info(msg.into()));
    }

    /// `--theme`: for this session only, without writing to the saved preference.
    pub fn use_theme(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.themes.iter().any(|t| t.id == id) || id == THEME_AUTO {
            self.cfg.theme_override = true;
            self.theme_choice = id.to_string();
            self.apply_theme(id, window, cx);
            self.warn_low_contrast(window, cx);
        } else {
            let known: Vec<&str> = self.themes.iter().map(|t| t.id.as_str()).collect();
            self.notice = Some(Notice::Error(crate::i18n::tf("Unknown theme '{0}'. Available: {1}", &[&id, &known.join(", ")]).into()));
        }
    }

    pub(super) fn select_theme(&mut self, _: &SelectTheme, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.is_some() {
            return;
        }
        let mut ids = vec![THEME_AUTO.to_string()];
        let mut items =
            vec![PickItem { label: crate::i18n::t("Auto (follow system)").into(), detail: Some("Codeyard Dark / Codeyard Light".into()), keys: None }];
        for t in &self.themes {
            ids.push(t.id.clone());
            let kind = if t.dark { crate::i18n::t("dark") } else { crate::i18n::t("light") };
            let detail = match &t.source {
                Source::Builtin => kind.to_string(),
                Source::User(_) => crate::i18n::tf("{} · user", &[&kind]),
                Source::VsCode(_) => crate::i18n::tf("{} · VS Code import", &[&kind]),
            };
            items.push(PickItem { label: t.name.clone().into(), detail: Some(detail.into()), keys: None });
        }
        let initial = ids.iter().position(|i| *i == self.theme_choice).unwrap_or(0);
        let picker = cx.new(|cx| {
            Picker::new(
                vec![PickSource::List { prefix: "", items }],
                crate::i18n::t("Select a color theme"),
                "",
                Some(Pick::Item { source: 0, index: initial }),
                cx,
            )
        });
        let kind = OverlayKind::Theme { ids, original: self.theme_choice.clone() };
        self.show_overlay(picker, kind, window, cx);
    }

    /// "Preferences: File Icon Theme": picker with live preview.
    pub(super) fn select_icon_theme(&mut self, _: &SelectIconTheme, window: &mut Window, cx: &mut Context<Self>) {
        use crate::icon_theme::{self, ALL};
        if self.overlay.is_some() {
            return;
        }
        let original = icon_theme::current();
        let items = ALL.iter().map(|t| PickItem { label: t.label().into(), detail: Some(t.about().into()), keys: None }).collect();
        let initial = ALL.iter().position(|t| *t == original).unwrap_or(0);
        let picker = cx.new(|cx| {
            Picker::new(
                vec![PickSource::List { prefix: "", items }],
                crate::i18n::t("Select a file icon theme"),
                "",
                Some(Pick::Item { source: 0, index: initial }),
                cx,
            )
        });
        self.show_overlay(picker, OverlayKind::IconTheme { original }, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_theme_files_are_reported_in_the_ui_language() {
        let path = std::path::Path::new("themes/x.json");
        let bad = themes::ThemeError::InvalidColor { role: "bg".into(), value: "#zzz".into() };
        let _g = crate::i18n::LangGuard::set("tr");
        assert_eq!(theme_load_error(path, &bad), "themes/x.json: bg: geçersiz renk #zzz");
        assert_eq!(theme_load_error(path, &themes::ThemeError::UnknownRole("bgg".into())), "themes/x.json: bilinmeyen rol: bgg");
    }
}
