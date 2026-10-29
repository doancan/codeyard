//! The extension manifest, `codeyard-extension.toml`: parsing, validation with author-facing errors, the activation
//! events inferred from contributions, and the permission set with its risk. Shared by the app, `cyx` and the registry,
//! so all three accept and reject exactly the same files. No UI and no I/O.
//!
//! Design: docs/plugin-architecture.md §4–§6.

pub mod diagnostics;
pub mod limits;
mod raw;
pub mod risk;
mod validate;
pub mod when;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Serialize};

pub use diagnostics::{Code, Diagnostic};
pub use risk::{Clipboard, Level, Line, Permissions, Risk, net_allows, risk};

/// The file name of the manifest at the root of an extension.
pub const FILE_NAME: &str = "codeyard-extension.toml";

pub use validate::{Options, check_package_path, is_deceptive};

/// Parse and validate a manifest, as install and publish do. Every problem is reported, not just the first.
pub fn parse(text: &str) -> Result<Manifest, Vec<Diagnostic>> {
    validate::parse(text, Options::default())
}

/// Parse with `options`; `cyx dev` uses this to warn about a missing `why` instead of failing (see `Manifest::missing_why`).
pub fn parse_with(text: &str, options: Options) -> Result<Manifest, Vec<Diagnostic>> {
    validate::parse(text, options)
}

/// `publisher.name`. Serialized as its text and checked again when read back.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ExtId(String);

impl TryFrom<String> for ExtId {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        ExtId::parse(&s)
    }
}

impl From<ExtId> for String {
    fn from(id: ExtId) -> String {
        id.0
    }
}

impl ExtId {
    /// Check the form: two parts, each lowercase `a-z 0-9 -`, starting with a letter, no `--`, no trailing `-`.
    pub fn parse(s: &str) -> Result<ExtId, String> {
        if s.len() > limits::ID_LEN {
            return Err(format!("is longer than {} characters", limits::ID_LEN));
        }
        let mut parts = s.split('.');
        let (Some(publisher), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err("must be <publisher>.<name>, with exactly one dot".into());
        };
        for (what, part) in [("publisher", publisher), ("name", name)] {
            check_id_part(part).map_err(|e| format!("{what} '{part}' {e}"))?;
        }
        Ok(ExtId(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn publisher(&self) -> &str {
        self.0.split('.').next().unwrap_or_default()
    }

    /// The full id of one of this extension's contributions: `publisher.name.local`.
    pub fn qualify(&self, local: &str) -> String {
        format!("{}.{local}", self.0)
    }
}

impl fmt::Display for ExtId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Lowercase `a-z 0-9 -`, starting with a letter, no `--`, no trailing `-`, at most `limits::ID_PART_LEN`.
pub fn check_id_part(part: &str) -> Result<(), String> {
    if part.is_empty() {
        return Err("is empty".into());
    }
    if part.len() > limits::ID_PART_LEN {
        return Err(format!("is longer than {} characters", limits::ID_PART_LEN));
    }
    if !part.starts_with(|c: char| c.is_ascii_lowercase()) {
        return Err("must start with a lowercase letter".into());
    }
    if let Some(c) = part.chars().find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')) {
        return Err(format!("contains '{c}'; use lowercase a-z, 0-9 and -"));
    }
    if part.contains("--") || part.ends_with('-') {
        return Err("must not contain '--' or end with '-'".into());
    }
    Ok(())
}

/// `major.minor.patch` with an optional `-pre` part; build metadata (`+…`) is not allowed. Serialized as its text.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(s: &str) -> Result<Version, String> {
        Self::parse_inner(s, false)
    }

    /// A minimum version in `dependencies`: `1`, `1.2` or `1.2.3` (missing parts are 0).
    pub fn parse_minimum(s: &str) -> Result<Version, String> {
        Self::parse_inner(s, true)
    }

    fn parse_inner(s: &str, partial: bool) -> Result<Version, String> {
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        if s.contains('+') {
            return Err("build metadata (+…) is not allowed".into());
        }
        let nums: Vec<&str> = core.split('.').collect();
        if nums.len() > 3 || (!partial && nums.len() != 3) || nums.is_empty() {
            return Err(if partial { "must be 1, 1.2 or 1.2.3".into() } else { "must be major.minor.patch, like 0.1.0".into() });
        }
        let mut n = [0u64; 3];
        for (i, part) in nums.iter().enumerate() {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) || (part.len() > 1 && part.starts_with('0')) {
                return Err(format!("'{part}' is not a number without leading zeros"));
            }
            n[i] = part.parse().map_err(|_| format!("'{part}' is too large"))?;
        }
        if let Some(p) = pre
            && (p.is_empty() || !p.split('.').all(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')))
        {
            return Err(format!("pre-release '{p}' must be dot-separated a-z, 0-9 and -"));
        }
        if let Some(p) = pre
            && p.split('.').any(|id| id.len() > 1 && id.starts_with('0') && id.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(format!("pre-release '{p}' has a number with a leading zero"));
        }
        Ok(Version { major: n[0], minor: n[1], patch: n[2], pre: pre.map(str::to_string) })
    }
}

impl TryFrom<String> for Version {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Version::parse(&s)
    }
}

impl From<Version> for String {
    fn from(v: Version) -> String {
        v.to_string()
    }
}

impl Ord for Version {
    /// Semver precedence: a pre-release sorts before its release; pre-release identifiers compare numerically when both
    /// are numbers, otherwise as text, and a shorter list sorts first.
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => {
                let (mut a, mut b) = (a.split('.'), b.split('.'));
                loop {
                    match (a.next(), b.next()) {
                        (None, None) => return Ordering::Equal,
                        (None, Some(_)) => return Ordering::Less,
                        (Some(_), None) => return Ordering::Greater,
                        (Some(x), Some(y)) => {
                            let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                                (Ok(x), Ok(y)) => x.cmp(&y),
                                (Ok(_), Err(_)) => Ordering::Less,
                                (Err(_), Ok(_)) => Ordering::Greater,
                                (Err(_), Err(_)) => x.cmp(y),
                            };
                            if o != Ordering::Equal {
                                return o;
                            }
                        }
                    }
                }
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

/// The language of `[code] entry`, from its file name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodeLanguage {
    Rust,
    TypeScript,
    JavaScript,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeEntry {
    pub entry: String,
    pub language: CodeLanguage,
}

/// When a code extension starts. Inferred from contributions; `WorkspaceContains` is the only form written by hand.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Activation {
    Command(String),
    View(String),
    WorkspaceContains(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub label: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippets {
    pub language: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keybinding {
    pub key: String,
    /// Full id: `publisher.name.local`, another extension's full id, or `core:…`.
    pub command: String,
    pub when: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Language {
    pub id: String,
    pub name: Option<String>,
    /// Without the dot: `toml`.
    pub extensions: Vec<String>,
    pub filenames: Vec<String>,
    pub grammar: Option<String>,
    pub line_comment: Option<String>,
    pub block_comment: Option<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaAssociation {
    pub file_match: Vec<String>,
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingKind {
    String,
    Number,
    Boolean,
    Array,
    Object,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    /// Local key; users see and set it as `ext.<publisher.name>.<key>`.
    pub key: String,
    pub kind: SettingKind,
    pub default: Option<toml::Value>,
    pub description: String,
    pub choices: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageServer {
    pub command: String,
    pub args: Vec<String>,
    pub languages: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// Full id: `publisher.name.local`.
    pub id: String,
    pub title: String,
    pub category: Option<String>,
    pub enablement: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewLocation {
    Sidebar,
    Bottom,
    Editor,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// Full id: `publisher.name.local`.
    pub id: String,
    pub title: String,
    pub location: ViewLocation,
    pub when: Option<String>,
}

/// Everything an extension adds to Codeyard.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Contributions {
    pub themes: Vec<FileEntry>,
    pub snippets: Vec<Snippets>,
    pub keymap_presets: Vec<FileEntry>,
    pub keybindings: Vec<Keybinding>,
    pub languages: Vec<Language>,
    pub json_schemas: Vec<SchemaAssociation>,
    pub yaml_schemas: Vec<SchemaAssociation>,
    pub settings: Vec<Setting>,
    pub language_servers: Vec<LanguageServer>,
    pub commands: Vec<Command>,
    pub views: Vec<View>,
}

impl Contributions {
    /// Contributions that need extension code to do anything.
    pub fn needs_code(&self) -> bool {
        !self.commands.is_empty() || !self.views.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: ExtId,
    pub version: Version,
    pub name: String,
    pub license: String,
    pub description: Option<String>,
    pub repository: Option<String>,
    pub categories: Vec<String>,
    pub icon: Option<String>,
    pub code: Option<CodeEntry>,
    pub permissions: Permissions,
    /// Inferred from commands and views plus `activation.workspace-contains`; empty for declarative extensions.
    /// (Language, URI-handler and account activation arrive with those contribution points in Phase 2.)
    pub activation: Vec<Activation>,
    pub dependencies: Vec<(ExtId, Version)>,
    /// Runs in untrusted folders (with no outbound permissions there). Default true.
    pub untrusted: bool,
    pub contributions: Contributions,
}

impl Manifest {
    /// No code: the extension only adds data (themes, snippets, languages, …) and never starts a process of its own.
    pub fn is_declarative(&self) -> bool {
        self.code.is_none()
    }

    pub fn risk(&self) -> Risk {
        risk(&self.permissions)
    }

    /// The permission set is high risk and has no `why`: an error at install and publish, a warning in `cyx dev`.
    pub fn missing_why(&self) -> bool {
        self.risk().needs_why() && self.permissions.why.is_none()
    }
}
