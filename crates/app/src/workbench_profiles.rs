//! Settings profiles in the workbench: which profile is active, switching (settings, keybindings, snippets and theme change
//! at once, without a restart), and the commands *Profiles: Create / Switch / Rename / Delete / Duplicate*. The files and
//! the checks are in `profiles.rs`; this file is the windows and prompts around them.

use gpui::{AppContext as _, Context, Window};

use super::super::{InputPurpose, Notice, NoticeSource, OverlayKind, Workbench};
use crate::picker::{PickItem, Picker, Source as PickSource};
use crate::settings;

#[path = "profiles.rs"]
pub mod store;

gpui::actions!(profiles, [CreateProfile, SwitchProfile, RenameProfile, DeleteProfile, DuplicateProfile]);

/// What a name typed into the prompt is for.
#[derive(Clone)]
pub(in crate::workbench) enum ProfileInput {
    Create,
    /// A copy of the active profile.
    Duplicate,
    Rename {
        from: String,
    },
}

/// What picking a profile from the list does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(in crate::workbench) enum ProfileAction {
    Switch,
    Rename,
    Delete,
}

impl Workbench {
    /// The profile `active-profile` names, or the default one (and a message when a named profile cannot be used).
    /// Read at the start of every settings reload, so a switch made by another window or by hand is followed.
    pub(in crate::workbench) fn sync_profile(&mut self) -> Option<String> {
        let dir = self.cfg.dir.clone()?;
        let active = store::read_active(&dir);
        let warning = match &active.fell_back {
            // Gone, or not a profile name: forget it, so the next window does not report it again.
            Some(name) if !active.transient => {
                let _ = store::set_active(&dir, None);
                Some(crate::i18n::tf("The profile “{}” is missing or cannot be used. The default profile is active.", &[name]))
            }
            // The folder is there but cannot be read now: the pointer stays, so the profile returns when it can be read; said once.
            Some(name) if self.cfg.profile_warned.as_ref() != Some(name) => {
                self.cfg.profile_warned = Some(name.clone());
                Some(crate::i18n::tf("The profile “{}” cannot be read now. The default profile is active until it can.", &[name]))
            }
            Some(_) => None,
            None => {
                self.cfg.profile_warned = None;
                None
            }
        };
        self.cfg.profile = active.name;
        warning
    }

    /// The active profile's folder was deleted while this window uses it.
    pub(in crate::workbench) fn profile_folder_missing(&self) -> bool {
        match (self.cfg.dir.as_deref(), self.cfg.profile.as_deref()) {
            (Some(dir), Some(name)) => !store::profile_dir(dir, name).is_dir(),
            _ => false,
        }
    }

    /// Whether the active-profile file now names another profile than the one in use.
    pub(in crate::workbench) fn profile_pointer_moved(&self) -> bool {
        self.cfg.dir.as_deref().is_some_and(|dir| store::read_active(dir).name != self.cfg.profile)
    }

    /// The folder of this profile's `snippets`.
    pub(in crate::workbench) fn snippets_dir(&self) -> Option<std::path::PathBuf> {
        if self.profile_folder_missing() {
            return None;
        }
        let root = self.cfg.dir.clone().or_else(settings::config_dir)?;
        Some(store::files_dir(&root, self.cfg.profile.as_deref()).join("snippets"))
    }

    /// The profile's name for the status bar and messages: `None` for the default profile.
    pub fn active_profile(&self) -> Option<&str> {
        self.cfg.profile.as_deref()
    }

    fn profile_notice(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.notify(NoticeSource::Settings, notice);
        cx.notify();
    }

    fn profile_error(&mut self, e: &store::ProfileError, cx: &mut Context<Self>) {
        self.profile_notice(Notice::Error(e.message().into()), cx);
    }

    /// The config folder, or a message that profiles need one.
    fn profile_root(&mut self, cx: &mut Context<Self>) -> Option<std::path::PathBuf> {
        let dir = self.cfg.dir.clone();
        if dir.is_none() {
            self.profile_notice(Notice::Info(crate::i18n::t("no configuration directory").into()), cx);
        }
        dir
    }

    /// Makes `name` (`None`: the default profile) the active one and applies it.
    fn switch_profile_to(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.profile_root(cx) else { return };
        if name == self.cfg.profile {
            let said = match &name {
                Some(n) => crate::i18n::tf("“{}” is already the active profile.", &[n]),
                None => crate::i18n::t("The default profile is already active.").to_string(),
            };
            return self.profile_notice(Notice::Info(said.into()), cx);
        }
        // The files the profile starts with are there before they are read, so the settings of the profile left behind
        // (its theme in particular) do not stay.
        let settings_file = store::files_dir(&dir, name.as_deref()).join("settings.jsonc");
        if let Err(e) = settings::ensure_file(&settings_file, settings::TEMPLATE).and_then(|_| store::set_active(&dir, name.as_deref())) {
            let message = crate::i18n::tf("Could not switch profile: {}", &[&e]);
            return self.profile_notice(Notice::Error(message.into()), cx);
        }
        self.reload_config(window, cx);
        if self.notice().is_none() {
            let said = match &self.cfg.profile {
                Some(n) => crate::i18n::tf("Profile “{}” is active.", &[n]),
                None => crate::i18n::t("The default profile is active.").to_string(),
            };
            self.notify(NoticeSource::Settings, Notice::Info(said.into()));
        }
        cx.notify();
    }

    // --- Commands ---

    pub(in crate::workbench) fn create_profile(&mut self, _: &CreateProfile, window: &mut Window, cx: &mut Context<Self>) {
        if self.profile_root(cx).is_none() {
            return;
        }
        self.show_input(
            InputPurpose::Profile(ProfileInput::Create),
            crate::i18n::t("Create profile"),
            crate::i18n::t("Name of the new profile (empty settings, keybindings and snippets)"),
            "",
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn duplicate_profile(&mut self, _: &DuplicateProfile, window: &mut Window, cx: &mut Context<Self>) {
        if self.profile_root(cx).is_none() {
            return;
        }
        let from = self.cfg.profile.clone().unwrap_or_else(|| store::DEFAULT_NAME.to_string());
        let initial = crate::i18n::tf("{} copy", &[&from]);
        self.show_input(
            InputPurpose::Profile(ProfileInput::Duplicate),
            crate::i18n::t("Duplicate profile"),
            crate::i18n::t("Name of the copy (it starts with the settings, keybindings and snippets in use)"),
            &initial,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn switch_profile(&mut self, _: &SwitchProfile, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_profile(ProfileAction::Switch, window, cx);
    }

    pub(in crate::workbench) fn rename_profile(&mut self, _: &RenameProfile, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_profile(ProfileAction::Rename, window, cx);
    }

    pub(in crate::workbench) fn delete_profile(&mut self, _: &DeleteProfile, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_profile(ProfileAction::Delete, window, cx);
    }

    /// The list of profiles to choose from. Only Switch lists the default profile: it cannot be renamed or deleted.
    fn pick_profile(&mut self, action: ProfileAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.profile_root(cx) else { return };
        let mut names: Vec<Option<String>> = store::list(&dir).into_iter().map(Some).collect();
        if action == ProfileAction::Switch {
            names.insert(0, None);
        }
        if names.is_empty() {
            return self.profile_notice(Notice::Info(crate::i18n::t("There are no profiles yet. Create one with “Profiles: Create…”.").into()), cx);
        }
        let items = names
            .iter()
            .map(|name| PickItem {
                label: name.clone().unwrap_or_else(|| crate::i18n::t("Default").to_string()).into(),
                detail: (*name == self.cfg.profile).then(|| crate::i18n::t("active").into()),
                keys: None,
            })
            .collect();
        let placeholder = match action {
            ProfileAction::Switch => crate::i18n::t("Switch to a profile"),
            ProfileAction::Rename => crate::i18n::t("Rename a profile"),
            ProfileAction::Delete => crate::i18n::t("Delete a profile"),
        };
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], placeholder, "", None, cx));
        self.show_overlay(picker, OverlayKind::Profile { action, names }, window, cx);
    }

    /// A profile was chosen from the list (`None`: the default profile).
    pub(in crate::workbench) fn profile_picked(&mut self, action: ProfileAction, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        match (action, name) {
            (ProfileAction::Switch, name) => self.switch_profile_to(name, window, cx),
            (ProfileAction::Rename, Some(from)) => {
                let initial = from.clone();
                self.show_input(
                    InputPurpose::Profile(ProfileInput::Rename { from }),
                    crate::i18n::t("Rename profile"),
                    crate::i18n::t("New name of the profile"),
                    &initial,
                    window,
                    cx,
                );
            }
            (ProfileAction::Delete, Some(name)) => self.ask_delete_profile(name, window, cx),
            // The default profile is not in the lists of Rename and Delete.
            (_, None) => {}
        }
    }

    /// The name typed into the prompt.
    pub(in crate::workbench) fn finish_profile_input(&mut self, purpose: ProfileInput, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.profile_root(cx) else { return };
        match purpose {
            ProfileInput::Create => match store::create(&dir, &name) {
                Ok(made) => self.switch_profile_to(Some(made), window, cx),
                Err(e) => self.profile_error(&e, cx),
            },
            // The copy is made from what is in use now, including a settings file just edited.
            ProfileInput::Duplicate => match store::duplicate(&dir, self.cfg.profile.as_deref(), &name) {
                Ok(made) => self.switch_profile_to(Some(made), window, cx),
                Err(e) => self.profile_error(&e, cx),
            },
            ProfileInput::Rename { from } => match store::rename(&dir, &from, &name) {
                Ok(to) => {
                    if self.cfg.profile.as_deref() == Some(from.as_str()) {
                        if let Err(e) = store::set_active(&dir, Some(&to)) {
                            let message = crate::i18n::tf("Could not switch profile: {}", &[&e]);
                            return self.profile_notice(Notice::Error(message.into()), cx);
                        }
                        self.reload_config(window, cx);
                    }
                    self.profile_notice(Notice::Info(crate::i18n::tf("Profile “{0}” is now “{1}”.", &[&from, &to]).into()), cx);
                }
                Err(e) => self.profile_error(&e, cx),
            },
        }
    }

    /// Deleting asks first. When the profile is the active one the default profile takes over before its folder goes.
    fn ask_delete_profile(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.cfg.profile.as_deref() == Some(name.as_str());
        let answer = crate::questions::delete_profile(&name, active).ask(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Some(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                let Some(dir) = this.cfg.dir.clone() else { return };
                if this.cfg.profile.as_deref() == Some(name.as_str()) {
                    this.switch_profile_to(None, window, cx);
                    if this.cfg.profile.is_some() {
                        return;
                    }
                }
                match store::delete(&dir, &name) {
                    Ok(()) => this.profile_notice(Notice::Info(crate::i18n::tf("Profile “{}” deleted.", &[&name]).into()), cx),
                    Err(e) => this.profile_error(&e, cx),
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
impl Workbench {
    pub fn config_paths_for_test(&self) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        self.config_paths()
    }

    pub fn write_setting_for_test(&mut self, key: &str, value: serde_json::Value, window: &mut Window, cx: &mut Context<Self>) {
        self.write_setting(key, value, window, cx);
    }
}

#[cfg(test)]
#[path = "workbench_profiles_tests.rs"]
mod tests;
