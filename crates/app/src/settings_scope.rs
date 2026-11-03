//! Settings layers over the user file and where each effective value comes from.

use gpui::Global;

/// A project's settings file could choose a program by itself: these keys come only from user
/// settings or a trusted folder.
pub const RESTRICTED_KEYS: &[&str] = &[
    "terminal.shell",
    "terminal.defaultProfile",
    "terminal.profiles.macos",
    "terminal.profiles.linux",
    "terminal.profiles.windows",
    "lsp.servers",
    "json.schemas",
    "yaml.schemas",
];

/// Not read from any layer but the user's own file, even in a trusted folder: the gate is global, so one
/// repo touching it would open it for every folder. `is_user_only` applies it to every non-user layer.
pub const USER_ONLY_KEYS: &[&str] = &[
    "modules.containers.enabled",
    "modules.webPreview.enabled",
    "webPreview.port",
    "webPreview.host",
    "webPreview.liveReload",
    "webPreview.reloadInterval",
    "webPreview.root",
    "security.workspaceTrust.enabled",
    "security.workspaceTrust.startupPrompt",
    "http.confirmRequests",
    "ui.language",
    // The interface scale applies to every window, so a repository must not zoom the user's whole application.
    "ui.scale",
    "update.check",
    // A repository must not switch extensions on or off for the user, or swap their keys for a preset.
    "extensions.enabled",
    "keymap.preset",
    // A repository must not turn updates (and the fixes they bring) off, nor install them for the user.
    "extensions.autoUpdate",
    // A repository must not add a registry to install from.
    "extensions.registries",
    // --- git-sync ---
    // A repository must not turn the question before a sync off, or make every commit push by itself.
    "git.confirmSync",
    "git.postCommitCommand",
    // A repository must not make forwarded ports reachable from the network.
    "ports.localBind",
    // A repository must not let programs fill the clipboard, or turn off the checks on what is pasted back: together
    // they would put a command on the clipboard that a later paste runs.
    "terminal.allowClipboardWrite",
    "terminal.sanitizePaste",
    "terminal.warnMultilinePaste",
];

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
