//! What an extension may do in a given folder: its declared permissions, minus what the user turned off, intersected
//! with Workspace Trust. The broker (Phase 2) asks this before every call; the Extensions view shows the same answer.
//!
//! In an untrusted folder (docs/plugin-architecture.md §6.3) declarative contributions apply; code may read what it
//! declared and show UI, but cannot send, write or run, and extension language servers and debug adapters do not start.
//! An extension with `untrusted = false` does not run there at all.

use ext_manifest::{Clipboard, Manifest, Permissions};

/// Whether the folder the window shows is trusted (`is_trusted_root` in the app).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    Untrusted,
}

/// What an extension gets in a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effective {
    /// The extension's code may start at all.
    pub code_runs: bool,
    /// Its language servers and debug adapters may start.
    pub programs_start: bool,
    pub permissions: Permissions,
}

/// `declared` minus `revoked` (per-permission switches the user turned off), then masked by `trust`.
pub fn effective(m: &Manifest, revoked: &Permissions, trust: Trust) -> Effective {
    let declared = &m.permissions;
    let keep = |list: &[String], off: &[String]| -> Vec<String> { list.iter().filter(|x| !off.contains(x)).cloned().collect() };
    let mut p = Permissions {
        read: keep(&declared.read, &revoked.read),
        write: keep(&declared.write, &revoked.write),
        net: keep(&declared.net, &revoked.net),
        auth: keep(&declared.auth, &revoked.auth),
        secrets: declared.secrets && !revoked.secrets,
        run: keep(&declared.run, &revoked.run),
        clipboard: match (declared.clipboard, revoked.clipboard) {
            (c, Clipboard::None) => c,
            (_, Clipboard::ReadWrite) => Clipboard::None,
            (Clipboard::ReadWrite, Clipboard::Read) => Clipboard::Write,
            (Clipboard::ReadWrite, Clipboard::Write) => Clipboard::Read,
            (c, off) if c == off => Clipboard::None,
            (c, _) => c,
        },
        terminal: declared.terminal && !revoked.terminal,
        commands: keep(&declared.commands, &revoked.commands),
        why: declared.why.clone(),
    };
    let mut code_runs = m.code.is_some();
    let mut programs_start = true;
    if trust == Trust::Untrusted {
        if !m.untrusted {
            code_runs = false;
        }
        programs_start = false;
        // Reading and UI only: nothing that sends, writes, runs or acts as the user.
        p.write.clear();
        p.net.clear();
        p.auth.clear();
        p.secrets = false;
        p.run.clear();
        p.terminal = false;
        p.commands.clear();
        p.clipboard = match p.clipboard {
            Clipboard::ReadWrite | Clipboard::Read => Clipboard::Read,
            _ => Clipboard::None,
        };
    }
    if !code_runs {
        p = Permissions { why: p.why, ..Default::default() };
    }
    Effective { code_runs, programs_start, permissions: p }
}
