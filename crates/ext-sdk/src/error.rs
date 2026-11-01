//! Errors. A refused call is [`Error::Denied`], and printing it says what to add to the manifest.

use std::fmt;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Why a call was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Why {
    /// The manifest does not declare the permission.
    NotDeclared,
    /// The user turned the permission off on the extension page.
    RevokedByUser,
    /// The folder is not trusted: code may read what it declared and show UI, but not send, write or run.
    UntrustedWorkspace,
    /// The registry revoked this version.
    RevokedExtension,
}

impl Why {
    pub fn as_str(self) -> &'static str {
        match self {
            Why::NotDeclared => "not-declared",
            Why::RevokedByUser => "revoked-by-user",
            Why::UntrustedWorkspace => "untrusted-workspace",
            Why::RevokedExtension => "revoked-extension",
        }
    }
}

/// A refused call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Denied {
    /// The manifest line that would allow the call, such as `net = ["api.example.com"]`. The SDK fills in the host,
    /// program or path from the call when the host only names the permission.
    pub permission: String,
    pub why: Why,
}

impl Denied {
    /// What to do about it, in one line.
    pub fn fix(&self) -> String {
        if let Some((_, reason)) = ungrantable(&self.permission) {
            return format!("none: this cannot be granted, {reason}");
        }
        match self.why {
            Why::NotDeclared => match oauth_host(&self.permission) {
                Some(("github", host)) => {
                    format!("the built-in github account signs requests only to its own API hosts; send the request to {host} with fetch (net) instead")
                }
                Some((provider, host)) => format!("add \"{host}\" to hosts in the [[oauth]] table with id = \"{provider}\""),
                None => match user_host(&self.permission) {
                    Some(host) => format!("the user has not allowed \"{host}\": they add it on the extension page with Allow Host…"),
                    None => format!("add to [permissions]:  {}", self.permission),
                },
            },
            Why::RevokedByUser => {
                format!("the user turned off `{}` on the extension page; tell them what stops working without it", self.permission)
            }
            Why::UntrustedWorkspace => "trust the folder, or set `untrusted = false` in the manifest if the extension is useless without trust".into(),
            Why::RevokedExtension => "the registry revoked this version: publish a fixed one".into(),
        }
    }

    /// Fill in the manifest line from the call when the host sent only the permission's name (`net`). Any other text
    /// from the host is kept: it knows more than the call does.
    pub(crate) fn detailed(mut self, line: &dyn Fn() -> String) -> Denied {
        if !self.permission.is_empty() && self.permission.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
            self.permission = line();
        }
        self
    }
}

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((call, _)) = ungrantable(&self.permission) {
            return write!(f, "denied: {call}\n   = fix: {}", self.fix());
        }
        let what = match self.why {
            Why::NotDeclared => "the manifest does not declare",
            Why::RevokedByUser => "the user turned off",
            Why::UntrustedWorkspace => "this folder is not trusted, which rules out",
            Why::RevokedExtension => "this version was revoked, which rules out",
        };
        write!(f, "denied: {what} {}\n   = fix: {}", self.permission, self.fix())
    }
}

/// `types.error`, plus the errors of the SDK itself (`Other`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Denied(Denied),
    NotFound(String),
    /// The user moved on, or the extension cancelled the call.
    Cancelled,
    Timeout,
    /// The call's arguments are wrong; the message says which.
    Invalid(String),
    Other(String),
}

impl Error {
    pub fn other(message: impl fmt::Display) -> Error {
        Error::Other(message.to_string())
    }

    pub fn invalid(message: impl fmt::Display) -> Error {
        Error::Invalid(message.to_string())
    }

    pub fn denied(&self) -> Option<&Denied> {
        match self {
            Error::Denied(d) => Some(d),
            _ => None,
        }
    }

    pub(crate) fn detailed(self, line: &dyn Fn() -> String) -> Error {
        match self {
            Error::Denied(d) => Error::Denied(d.detailed(line)),
            e => e,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Denied(d) => d.fmt(f),
            Error::NotFound(s) => write!(f, "not found: {s}"),
            Error::Cancelled => f.write_str("cancelled"),
            Error::Timeout => f.write_str("timed out"),
            Error::Invalid(s) => write!(f, "invalid: {s}"),
            Error::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(s: String) -> Error {
        Error::Other(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Error {
        Error::Other(s.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::Invalid(format!("JSON: {e}"))
    }
}

impl From<std::string::FromUtf8Error> for Error {
    fn from(e: std::string::FromUtf8Error) -> Error {
        Error::Invalid(format!("not UTF-8: {e}"))
    }
}

impl From<fmt::Error> for Error {
    fn from(e: fmt::Error) -> Error {
        Error::Other(e.to_string())
    }
}

/// The provider and host of a refused `auth.fetch` to a host outside the account's: `[[oauth]] gitlab hosts = ["h"]`.
fn oauth_host(permission: &str) -> Option<(&str, &str)> {
    let (provider, rest) = permission.strip_prefix("[[oauth]] ")?.split_once(" hosts = [\"")?;
    Some((provider, rest.strip_suffix("\"]")?))
}

/// The call and the reason of a refusal no manifest entry can lift (a file outside the workspace, an IP address):
/// `read file:///etc/passwd (cannot be granted: <reason>)`, as `ext_manifest::ungrantable_line` writes it.
fn ungrantable(permission: &str) -> Option<(&str, &str)> {
    let (call, rest) = permission.split_once(" (cannot be granted: ")?;
    Some((call, rest.strip_suffix(')')?))
}

/// The host of a refused request from an extension with `net-from-setting` to a server the user has not allowed:
/// `net-from-setting host = "git.example.com"`.
fn user_host(permission: &str) -> Option<&str> {
    permission.strip_prefix("net-from-setting host = \"")?.strip_suffix('"')
}

/// The manifest line for a permission when the call gives nothing more specific.
pub(crate) fn line_for(permission: &str) -> String {
    match permission {
        "read" => r#"read = ["<glob>"]"#.into(),
        "write" => r#"write = ["<glob>"]"#.into(),
        "net" => r#"net = ["<host>"]"#.into(),
        "auth" => r#"auth = ["<provider>"]"#.into(),
        "run" => r#"run = ["<program>"]"#.into(),
        "secrets" | "terminal" => format!("{permission} = true"),
        other => other.into(),
    }
}

/// `read = ["**/vercel.json"]` for a file uri: the narrowest glob the SDK can name without knowing the workspace.
pub(crate) fn path_line(permission: &str, uri: &str) -> String {
    let name = uri.rsplit('/').next().unwrap_or(uri);
    format!(r#"{permission} = ["**/{name}"]"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_prints_the_fix() {
        let e = Error::Denied(Denied { permission: "net".into(), why: Why::NotDeclared }).detailed(&|| r#"net = ["api.example.com"]"#.into());
        let s = e.to_string();
        assert!(s.contains(r#"= fix: add to [permissions]:  net = ["api.example.com"]"#), "{s}");
        // A line the host already made is kept.
        let e = Error::Denied(Denied { permission: r#"run = ["git"]"#.into(), why: Why::NotDeclared }).detailed(&|| "x".into());
        assert!(e.to_string().contains(r#"run = ["git"]"#));
    }

    /// #183: a host outside the account's is fixed in the `[[oauth]]` table, not with the `auth` line.
    #[test]
    fn a_host_outside_the_account_names_the_oauth_table() {
        let host = r#"[[oauth]] gitlab hosts = ["evil.example.org"]"#;
        let e = Error::Denied(Denied { permission: host.into(), why: Why::NotDeclared }).detailed(&|| r#"auth = ["gitlab"]"#.into());
        let s = e.to_string();
        assert!(s.contains(host), "{s}");
        assert!(s.contains(r#"= fix: add "evil.example.org" to hosts in the [[oauth]] table with id = "gitlab""#), "{s}");
        assert!(!s.contains("auth = "), "{s}");
        let gh = Denied { permission: r#"[[oauth]] github hosts = ["example.com"]"#.into(), why: Why::NotDeclared };
        assert!(gh.fix().contains("with fetch (net) instead"), "{}", gh.fix());
        // Text without `=` that is more than a permission's name is kept too.
        let e = Error::Denied(Denied { permission: "a path inside the workspace".into(), why: Why::NotDeclared }).detailed(&|| "x".into());
        assert!(e.to_string().contains("a path inside the workspace"));
    }

    /// A file outside the workspace or an IP address: no manifest line allows it, so none is suggested.
    #[test]
    fn a_denial_no_entry_can_lift_says_so() {
        let d = Denied { permission: "read file:///etc/passwd (cannot be granted: the file is outside the workspace)".into(), why: Why::NotDeclared };
        let s = Error::Denied(d.clone()).detailed(&|| r#"read = ["**/passwd"]"#.into()).to_string();
        assert_eq!(s, "denied: read file:///etc/passwd\n   = fix: none: this cannot be granted, the file is outside the workspace");
        assert!(!d.fix().contains("[permissions]"), "{}", d.fix());
    }

    #[test]
    fn path_lines() {
        assert_eq!(path_line("read", "file:///work/apps/vercel.json"), r#"read = ["**/vercel.json"]"#);
        assert_eq!(line_for("secrets"), "secrets = true");
    }
}
