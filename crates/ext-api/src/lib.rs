//! The extension API (docs/plugin-architecture.md §5.2): the `codeyard:api` WIT package in `wit/`, which interface needs
//! which permission, which WASI imports a component may have, and the API's values in Rust for the app ↔ exthost
//! frames (`ext-proto`). GPUI-free and wasmtime-free: the app, `cyx`, the registry and the exthost all use it.
//!
//! Each permission has its own interface (`net`, `workspace`, `process`, …), so a component's import list says which
//! permissions its code can use at all. `ext_pkg::check_imports` compares that list with the manifest; the broker
//! still checks every call (hosts, globs, programs) at run time.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod value;

use ext_manifest::Permissions;

/// The WIT package name.
pub const PACKAGE: &str = "codeyard:api";
/// The API version this build implements. `0.x` while in beta: a minor may break, with a `cyx` error and a migration
/// note.
pub const VERSION: Version = Version(0, 1, 0);

/// The package's WIT files, by name.
pub const WIT: [(&str, &str); 6] = [
    ("types.wit", include_str!("../wit/types.wit")),
    ("free.wit", include_str!("../wit/free.wit")),
    ("permissions.wit", include_str!("../wit/permissions.wit")),
    ("world.wit", include_str!("../wit/world.wit")),
    ("scm.wit", include_str!("../wit/scm.wit")),
    ("pulls.wit", include_str!("../wit/pulls.wit")),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let mut it = s.split('.').map(|p| p.parse::<u32>().ok());
        let v = Version(it.next()??, it.next()??, it.next()??);
        it.next().is_none().then_some(v)
    }

    /// Whether a component built against `self` runs on a host implementing `host`: same major, and in `0.x` the
    /// same minor too (a `0.x` minor may break), and not newer.
    pub fn runs_on(self, host: Version) -> bool {
        let same_line = if self.0 == 0 { self.0 == host.0 && self.1 == host.1 } else { self.0 == host.0 };
        same_line && self <= host
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The permission an interface needs before a component may import it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Need {
    Nothing,
    Read,
    Write,
    Net,
    Auth,
    Secrets,
    Run,
    ClipboardRead,
    ClipboardWrite,
    Terminal,
}

impl Need {
    /// Whether `p` grants this at all. Which globs, hosts and programs is checked per call by the broker.
    pub fn granted(self, p: &Permissions) -> bool {
        match self {
            Need::Nothing => true,
            Need::Read => !p.read.is_empty(),
            Need::Write => !p.write.is_empty(),
            Need::Net => !p.net.is_empty(),
            Need::Auth => !p.auth.is_empty(),
            Need::Secrets => p.secrets,
            Need::Run => !p.run.is_empty(),
            Need::ClipboardRead => p.clipboard.reads(),
            Need::ClipboardWrite => p.clipboard.writes(),
            Need::Terminal => p.terminal,
        }
    }

    /// The manifest line that grants it, for error fixes.
    pub fn manifest_line(self) -> &'static str {
        match self {
            Need::Nothing => "",
            Need::Read => r#"read = ["<glob>"]"#,
            Need::Write => r#"write = ["<glob>"]"#,
            Need::Net => r#"net = ["<host>"]"#,
            Need::Auth => r#"auth = ["github"]"#,
            Need::Secrets => "secrets = true",
            Need::Run => r#"run = ["<program>"]"#,
            Need::ClipboardRead => r#"clipboard = "read""#,
            Need::ClipboardWrite => r#"clipboard = "write""#,
            Need::Terminal => "terminal = true",
        }
    }

    /// The permission's name in `[permissions]`.
    pub fn name(self) -> &'static str {
        match self {
            Need::Nothing => "",
            Need::Read => "read",
            Need::Write => "write",
            Need::Net => "net",
            Need::Auth => "auth",
            Need::Secrets => "secrets",
            Need::Run => "run",
            Need::ClipboardRead | Need::ClipboardWrite => "clipboard",
            Need::Terminal => "terminal",
        }
    }
}

/// One interface of the package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interface {
    pub name: &'static str,
    /// The API version that added it (`@since` in the WIT).
    pub since: Version,
    pub need: Need,
}

const fn i(name: &'static str, need: Need) -> Interface {
    Interface { name, since: Version(0, 1, 0), need }
}

/// Every interface of `codeyard:api`, in WIT order. A test keeps this in step with `wit/`.
pub const INTERFACES: [Interface; 26] = [
    i("types", Need::Nothing),
    // Imported by every component, through the types `guest` uses.
    i("host-requests", Need::Nothing),
    i("log", Need::Nothing),
    i("env", Need::Nothing),
    i("tickets", Need::Nothing),
    i("requests", Need::Nothing),
    i("responses", Need::Nothing),
    i("window", Need::Nothing),
    i("commands", Need::Nothing),
    i("storage", Need::Nothing),
    i("settings", Need::Nothing),
    i("timers", Need::Nothing),
    i("ui", Need::Nothing),
    i("documents", Need::Nothing),
    i("languages", Need::Nothing),
    i("workspace", Need::Read),
    i("workspace-write", Need::Write),
    i("net", Need::Net),
    i("auth", Need::Auth),
    i("secrets", Need::Secrets),
    i("process", Need::Run),
    i("clipboard-read", Need::ClipboardRead),
    i("clipboard-write", Need::ClipboardWrite),
    i("terminal", Need::Terminal),
    i("scm", Need::Nothing),
    // Types only, imported through `host-requests` and `responses`.
    i("pull-requests", Need::Nothing),
];

/// The interface the guest exports.
pub const EXPORTS: [&str; 1] = ["guest"];

pub fn interface(name: &str) -> Option<&'static Interface> {
    INTERFACES.iter().find(|i| i.name == name)
}

/// What a component import name (`codeyard:api/net@0.1.0`, `wasi:io/streams@0.2.6`) refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Import<'a> {
    /// An interface of this package at some version (none when the import is unversioned).
    Api { interface: &'a str, version: Option<Version> },
    /// A WASI interface the exthost provides.
    Wasi(&'a str),
    /// A WASI interface no extension gets: sockets and HTTP (requests go through `net`).
    ForbiddenWasi(&'a str),
    /// Anything else, including `codeyard:api` names that do not parse.
    Unknown(&'a str),
}

/// WASI interfaces the exthost provides. `wasi:filesystem` is linked (language runtimes import it) but given no
/// folders, so files are reached only through `workspace`.
const WASI_PROVIDED: [&str; 5] = ["wasi:io/", "wasi:clocks/", "wasi:random/", "wasi:cli/", "wasi:filesystem/"];
const WASI_FORBIDDEN: [&str; 2] = ["wasi:sockets/", "wasi:http/"];

pub fn classify(name: &str) -> Import<'_> {
    if let Some(rest) = name.strip_prefix(PACKAGE).and_then(|r| r.strip_prefix('/')) {
        let (iface, version) = match rest.split_once('@') {
            Some((i, v)) => match Version::parse(v) {
                Some(v) => (i, Some(v)),
                None => return Import::Unknown(name),
            },
            None => (rest, None),
        };
        return Import::Api { interface: iface, version };
    }
    if WASI_FORBIDDEN.iter().any(|p| name.starts_with(p)) {
        Import::ForbiddenWasi(name)
    } else if WASI_PROVIDED.iter().any(|p| name.starts_with(p)) {
        Import::Wasi(name)
    } else {
        Import::Unknown(name)
    }
}

#[cfg(test)]
mod tests;
