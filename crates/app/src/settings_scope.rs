//! Settings layers over the user file and where each effective value comes from.

use gpui::Global;

use super::USER_ONLY_KEYS;

/// Where an effective setting comes from. Precedence: user < workspace file < folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    Workspace,
    Folder,
}

/// The layer that set each key in effect, for keys a workspace file or a folder set (the rest come from the user file or
/// the defaults). Shown on the Settings page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sources(pub std::collections::HashMap<String, Scope>);

impl Global for Sources {}

impl Sources {
    pub fn of(&self, key: &str) -> Scope {
        self.0.get(key).copied().unwrap_or(Scope::User)
    }
}

/// A settings layer over the user file: a workspace file's `settings` or a folder's settings file.
#[derive(Clone, Copy, Debug)]
pub struct ScopedLayer<'a> {
    /// File name for warnings.
    pub name: &'a str,
    pub text: &'a str,
    /// Warn about unknown keys.
    pub warn: bool,
    /// Whoever wrote this layer is trusted: only then may it set `RESTRICTED_KEYS`.
    pub trusted: bool,
    pub scope: Scope,
}

/// Whether `key` may only come from the user's own settings file. Every layer that is not the user's file (project
/// settings, a workspace file, any layer added later) is merged by `parse_scoped`, which drops these keys; a new kind of
/// layer goes through it instead of being merged on its own.
pub fn is_user_only(key: &str) -> bool {
    USER_ONLY_KEYS.contains(&key)
}
