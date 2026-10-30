//! Run-time prompts of code extensions (docs/plugin-architecture.md §6.3). Undeclared access is denied, never asked;
//! the app asks only for declared access that reaches further than the manifest shows: the first clipboard read of an
//! extension per session, and its first process on a remote host per session. (The first account use asks too, in
//! `workbench_ext_accounts.rs`.)
//!
//! No UI here: which calls ask, what the user answered, and the calls waiting for an answer. The window shows the
//! dialog (`workbench_ext_prompts.rs`).

use std::collections::HashMap;

use ext_api::value::{self as v, Start};

use crate::ext_broker::Scope;

/// What an extension wants that asks first.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Its first `clipboard-read` this session.
    ClipboardRead,
    /// Its first process on this remote host this session (the host's label: `ssh://node-dev`).
    RemoteProcess { host: String },
}

impl Kind {
    /// The permission a denial names. A kind, not a manifest line: the extension declared it, so adding it again is
    /// no fix (`ext_inspector::fix_line`).
    pub fn permission(&self) -> &'static str {
        match self {
            Kind::ClipboardRead => "clipboard",
            Kind::RemoteProcess { .. } => "run",
        }
    }

    /// The completion of a call the user denied.
    pub fn denied(&self) -> v::Completion {
        v::Completion::Failed(v::Error::Denied(v::Denied { permission: self.permission().into(), why: v::Why::RevokedByUser }))
    }
}

/// Whether `call` asks first. `remote` is the window's host label when it is remote. Calls the manifest or the
/// broker refuses anyway never ask: they are denied as before.
pub fn kind_of(call: &Start, scope: &Scope, remote: Option<&str>) -> Option<Kind> {
    match call {
        Start::ReadClipboard if scope.permissions.clipboard.reads() => Some(Kind::ClipboardRead),
        Start::Spawn { command } => {
            let host = remote?;
            scope.may_run(&command.program).ok()?;
            Some(Kind::RemoteProcess { host: host.to_string() })
        }
        _ => None,
    }
}

/// What to do with a call that may ask.
#[derive(Debug, PartialEq, Eq)]
pub enum Step<T> {
    /// Allowed this session: go on with it.
    Go(T),
    /// Denied this session: refuse it without asking.
    Deny(T),
    /// The first such call: show the dialog. The call waits for the answer.
    Ask,
    /// A dialog for the same thing is open: the call waits for its answer.
    Wait,
}

/// The answers given this session, and the calls waiting for one. `T` is a waiting call.
pub struct Prompts<T> {
    answers: HashMap<(String, Kind), bool>,
    waiting: HashMap<(String, Kind), Vec<T>>,
}

impl<T> Default for Prompts<T> {
    fn default() -> Self {
        Prompts { answers: HashMap::new(), waiting: HashMap::new() }
    }
}

impl<T> Prompts<T> {
    /// `call` of `extension` needs `kind`: go on, refuse, or wait (and ask, when nothing is asking yet).
    pub fn check(&mut self, extension: &str, kind: &Kind, call: T) -> Step<T> {
        let key = (extension.to_string(), kind.clone());
        match self.answers.get(&key) {
            Some(true) => return Step::Go(call),
            Some(false) => return Step::Deny(call),
            None => {}
        }
        let list = self.waiting.entry(key).or_default();
        list.push(call);
        if list.len() == 1 { Step::Ask } else { Step::Wait }
    }

    /// The user answered: remembered for the session. The calls that waited, in the order they came.
    pub fn answer(&mut self, extension: &str, kind: &Kind, allowed: bool) -> Vec<T> {
        let key = (extension.to_string(), kind.clone());
        self.answers.insert(key.clone(), allowed);
        self.waiting.remove(&key).unwrap_or_default()
    }

    /// What the user answered for `kind` this session.
    #[cfg(test)]
    pub fn answered(&self, extension: &str, kind: &Kind) -> Option<bool> {
        self.answers.get(&(extension.to_string(), kind.clone())).copied()
    }
}

/// The dialog's title and text: who asks and what for. `name` is the extension's display name, `publisher` its
/// publisher identity as the install sheet shows it.
pub fn dialog(kind: &Kind, name: &str, id: &str, publisher: &str, program: Option<&str>) -> (String, String) {
    let who = crate::i18n::tf("{0} ({1}) is published by {2}.", &[&name, &id, &publisher]);
    let (title, what) = match kind {
        Kind::ClipboardRead => (
            crate::i18n::tf("Let {} read the clipboard?", &[&name]),
            crate::i18n::t("It wants to read the text you copied, here or in other apps.").to_string(),
        ),
        Kind::RemoteProcess { host } => (
            crate::i18n::tf("Let {0} run programs on {1}?", &[&name, host]),
            crate::i18n::tf("It wants to run {0} on the remote host {1}, with your account there.", &[&program.unwrap_or_default(), host]),
        ),
    };
    let until = crate::i18n::t("Your answer holds until Codeyard quits.");
    (title, format!("{who}\n\n{what} {until}"))
}

#[cfg(test)]
#[path = "ext_prompts_tests.rs"]
mod tests;
