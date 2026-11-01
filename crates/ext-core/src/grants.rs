//! The user's per-permission switches (docs/plugin-architecture.md §6.3): what they turned off on an extension's page,
//! kept in `<data>/extensions/permissions.json` next to the store. A record belongs to one extension id *and* one
//! publisher identity: a package with the same id from another publisher, or a sideloaded one, starts with no
//! decision carried over from it (`revoked` of a record is ignored when its identity is not the installed one's).
//!
//! `trust::effective(manifest, revoked, trust)` applies the switches; the app reads them before every call, so turning
//! one off takes effect at once.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ext_manifest::{Clipboard, ExtId, Permissions};
use serde::{Deserialize, Serialize};

use crate::{Error, Source, Store, write_atomic};

const FORMAT: u32 = 1;

/// One extension's switches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// `identity(source)` of the package the switches were set for.
    pub publisher: String,
    /// The declared entries the user turned off (only `read`, `net` … entries; `why` is unused).
    pub revoked: Permissions,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    format: u32,
    extensions: BTreeMap<String, Record>,
}

/// The publisher identity the switches are keyed by: the registry and its pinned publisher, or `sideload` for a file
/// or folder the user installed.
pub fn identity(source: &Source) -> String {
    match source {
        Source::Registry { registry, publisher } => format!("{registry}: {publisher}"),
        Source::File { .. } | Source::Folder { .. } => "sideload".into(),
    }
}

/// One switch on the extension page: a declared permission entry (`net`, `api.example.com`), with what it reads as
/// in a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// `read`, `write`, `net`, `net-from-setting`, `auth`, `secrets`, `run`, `clipboard`, `terminal` or `commands`.
    pub kind: &'static str,
    /// The glob, host, provider, program, command or setting key; `read`/`write` for `clipboard`; empty for the yes/no
    /// ones.
    pub value: String,
}

impl Entry {
    /// The manifest line that grants this entry: `net = ["api.example.com"]`.
    pub fn manifest_line(&self) -> String {
        match self.kind {
            "secrets" | "terminal" => format!("{} = true", self.kind),
            "clipboard" | "net-from-setting" => format!("{} = \"{}\"", self.kind, self.value),
            k => format!("{k} = [{:?}]", self.value),
        }
    }
}

/// Every switch the declared set offers, in manifest order.
pub fn entries(declared: &Permissions) -> Vec<Entry> {
    let list = |kind: &'static str, v: &[String]| v.iter().map(move |x| Entry { kind, value: x.clone() }).collect::<Vec<_>>();
    let mut out = Vec::new();
    out.extend(list("read", &declared.read));
    out.extend(list("write", &declared.write));
    out.extend(list("net", &declared.net));
    out.extend(declared.net_from_setting.iter().map(|k| Entry { kind: "net-from-setting", value: k.clone() }));
    out.extend(list("auth", &declared.auth));
    if declared.secrets {
        out.push(Entry { kind: "secrets", value: String::new() });
    }
    out.extend(list("run", &declared.run));
    if declared.clipboard.reads() {
        out.push(Entry { kind: "clipboard", value: "read".into() });
    }
    if declared.clipboard.writes() {
        out.push(Entry { kind: "clipboard", value: "write".into() });
    }
    if declared.terminal {
        out.push(Entry { kind: "terminal", value: String::new() });
    }
    out.extend(list("commands", &declared.commands));
    out
}

fn clipboard(read: bool, write: bool) -> Clipboard {
    match (read, write) {
        (true, true) => Clipboard::ReadWrite,
        (true, false) => Clipboard::Read,
        (false, true) => Clipboard::Write,
        (false, false) => Clipboard::None,
    }
}

/// Whether `revoked` turns `e` off.
pub fn is_revoked(revoked: &Permissions, e: &Entry) -> bool {
    let has = |v: &[String]| v.contains(&e.value);
    match e.kind {
        "read" => has(&revoked.read),
        "write" => has(&revoked.write),
        "net" => has(&revoked.net),
        "net-from-setting" => revoked.net_from_setting.is_some(),
        "auth" => has(&revoked.auth),
        "secrets" => revoked.secrets,
        "run" => has(&revoked.run),
        "clipboard" if e.value == "read" => revoked.clipboard.reads(),
        "clipboard" => revoked.clipboard.writes(),
        "terminal" => revoked.terminal,
        "commands" => has(&revoked.commands),
        _ => false,
    }
}

/// `revoked` with `e` turned off (`on == false`) or on again.
pub fn switch(revoked: &Permissions, e: &Entry, on: bool) -> Permissions {
    let mut r = revoked.clone();
    let set = |v: &mut Vec<String>| {
        v.retain(|x| x != &e.value);
        if !on {
            v.push(e.value.clone());
        }
    };
    match e.kind {
        "read" => set(&mut r.read),
        "write" => set(&mut r.write),
        "net" => set(&mut r.net),
        "net-from-setting" => r.net_from_setting = (!on).then(|| e.value.clone()),
        "auth" => set(&mut r.auth),
        "secrets" => r.secrets = !on,
        "run" => set(&mut r.run),
        "clipboard" if e.value == "read" => r.clipboard = clipboard(!on, r.clipboard.writes()),
        "clipboard" => r.clipboard = clipboard(r.clipboard.reads(), !on),
        "terminal" => r.terminal = !on,
        "commands" => set(&mut r.commands),
        _ => {}
    }
    r
}

impl Store {
    /// `<data>/extensions/permissions.json`.
    pub fn permissions_path(&self) -> PathBuf {
        self.root.join("permissions.json")
    }

    fn read_permissions(&self) -> File {
        std::fs::read(self.permissions_path()).ok().and_then(|b| serde_json::from_slice::<File>(&b).ok()).filter(|f| f.format == FORMAT).unwrap_or_default()
    }

    /// What the user turned off for `id` as installed from `source`; nothing for a package from another identity.
    pub fn revoked_permissions(&self, id: &ExtId, source: &Source) -> Permissions {
        let who = identity(source);
        self.read_permissions().extensions.remove(id.as_str()).filter(|r| r.publisher == who).map(|r| r.revoked).unwrap_or_default()
    }

    /// The switches of every installed extension whose record matches its installed identity, by id (the app keeps
    /// this in memory and reads it again after a change).
    pub fn all_revoked_permissions(&self) -> BTreeMap<String, Permissions> {
        let Ok((lock, _)) = self.read_lock() else { return BTreeMap::new() };
        let records = self.read_permissions().extensions;
        lock.extensions
            .iter()
            .filter_map(|(id, l)| {
                let r = records.get(id.as_str())?;
                (r.publisher == identity(&l.source)).then(|| (id.to_string(), r.revoked.clone()))
            })
            .collect()
    }

    /// Record the switches of `id` from `source`; an empty set removes its record.
    pub fn set_revoked_permissions(&self, id: &ExtId, source: &Source, revoked: &Permissions) -> Result<(), Error> {
        let _guard = self.guard()?;
        let mut file = self.read_permissions();
        file.format = FORMAT;
        if revoked.is_empty() {
            file.extensions.remove(id.as_str());
        } else {
            let revoked = Permissions { why: None, ..revoked.clone() };
            file.extensions.insert(id.to_string(), Record { publisher: identity(source), revoked });
        }
        write_atomic(&self.permissions_path(), &serde_json::to_vec_pretty(&file).expect("permissions serialize"))
    }

    /// Forget the switches of an uninstalled extension.
    pub(crate) fn forget_permissions(&self, id: &ExtId) {
        let mut file = self.read_permissions();
        if file.extensions.remove(id.as_str()).is_some() {
            let _ = write_atomic(&self.permissions_path(), &serde_json::to_vec_pretty(&file).expect("permissions serialize"));
        }
    }
}

#[cfg(test)]
#[path = "grants_tests.rs"]
mod tests;
