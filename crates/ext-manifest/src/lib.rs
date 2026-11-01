//! The extension manifest, `codeyard-extension.toml`: parsing, validation with author-facing errors, the activation
//! events inferred from contributions, and the permission set with its risk. Shared by the app, `cyx` and the registry,
//! so all three accept and reject exactly the same files. No UI and no I/O.
//!
//! Design: docs/plugin-architecture.md §4–§6.

mod allow;
pub mod core_commands;
pub mod diagnostics;
pub mod limits;
mod raw;
pub mod risk;
mod validate;
pub mod when;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod reference_tests;
#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Serialize};

pub use core_commands::{CORE_COMMANDS, CoreCommand, core_command};
pub use diagnostics::{Code, Diagnostic, ErrorDoc};
pub use risk::{Clipboard, Level, Line, Permissions, Risk, is_package_program, net_allows, package_program_path, risk};

/// The file name of the manifest at the root of an extension.
pub const FILE_NAME: &str = "codeyard-extension.toml";

/// The platforms a per-platform package (one with programs inside, docs/plugin-architecture.md §7) can be built for,
/// as Rust target triples. Linux programs are static (musl), so one package serves every distribution.
pub const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
];

/// The target triple of the machine this runs on, when it is one of [`TARGETS`].
pub fn this_target() -> Option<&'static str> {
    target_for(std::env::consts::ARCH, std::env::consts::OS)
}

/// The target triple of a machine by Rust's names for its architecture and system (`std::env::consts`, which a
/// workspace host reports), when it is one of [`TARGETS`].
pub fn target_for(arch: &str, os: &str) -> Option<&'static str> {
    let t = match (arch, os) {
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("x86_64", "linux") => "x86_64-unknown-linux-musl",
        ("aarch64", "linux") => "aarch64-unknown-linux-musl",
        ("x86_64", "windows") => "x86_64-pc-windows-msvc",
        ("aarch64", "windows") => "aarch64-pc-windows-msvc",
        _ => return None,
    };
    Some(t)
}

/// Whether `triple` is one of [`TARGETS`].
pub fn is_target(triple: &str) -> bool {
    TARGETS.contains(&triple)
}

/// The file a `run` entry inside the package names in the package for `target`: `./bin/server` is `bin/server`, and
/// `bin/server.exe` on Windows.
pub fn program_file(run: &str, target: &str) -> String {
    let path = package_program_path(run);
    if target.contains("-windows-") && !path.to_ascii_lowercase().ends_with(".exe") { format!("{path}.exe") } else { path.to_string() }
}

pub use allow::glob_match;
pub use validate::{Options, check_https_url, check_package_path, is_deceptive};

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
    /// A file of this language opens, or a provider for it is asked (`[[formatters]]`, `[[diagnostics]]`, …).
    Language(String),
    /// The user lists tasks (`[[tasks]]`).
    Tasks,
    /// A trusted workspace folder has a marker of this `[[scm]]` provider (full id).
    Scm(String),
    /// The Pull Requests view asks this `[[pull-requests]]` provider (full id).
    PullRequests(String),
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

/// Where a `[[menus]]` entry appears.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuLocation {
    /// The editor's right-click menu.
    EditorContext,
    /// The right-click menu of a file or folder in the file tree.
    FileTree,
    /// The "…" menu in the title of one of the extension's views.
    ViewTitle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Menu {
    /// Full id: the extension's own command or a dependency's.
    pub command: String,
    pub location: MenuLocation,
    /// For `ViewTitle`: the view's full id.
    pub view: Option<String>,
    pub when: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusItem {
    /// Full id: `publisher.name.local`. The extension sets the text at run time with this local id.
    pub id: String,
    /// Full id of the command a click runs, unless the extension sets another.
    pub command: Option<String>,
}

/// A provider for some languages: `[[formatters]]`, `[[code-actions]]`, `[[hover]]`, `[[code-lens]]`, `[[inlay-hints]]`,
/// and with an id `[[diagnostics]]`, `[[decorations]]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// Full id, or empty for formatters, code actions, hover, code lenses and inlay hints.
    pub id: String,
    /// Language ids as Codeyard names them (`rust`, `toml`, `typescriptreact`, or a language an extension adds).
    pub languages: Vec<String>,
}

impl Provider {
    pub fn handles(&self, language: &str) -> bool {
        self.languages.iter().any(|l| l == language)
    }
}

/// `[[completion]]`: completions for some languages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionProvider {
    pub languages: Vec<String>,
    /// Characters that ask for completions when typed (`.`, `:`), besides typing a word.
    pub trigger_characters: Vec<char>,
}

impl CompletionProvider {
    pub fn handles(&self, language: &str) -> bool {
        self.languages.iter().any(|l| l == language)
    }
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
    pub menus: Vec<Menu>,
    pub status_items: Vec<StatusItem>,
    pub formatters: Vec<Provider>,
    pub code_actions: Vec<Provider>,
    pub diagnostics: Vec<Provider>,
    pub decorations: Vec<Provider>,
    /// Ids (full) of the extension's task providers.
    pub tasks: Vec<String>,
    #[serde(default)]
    pub hover: Vec<Provider>,
    #[serde(default)]
    pub completion: Vec<CompletionProvider>,
    #[serde(default)]
    pub code_lenses: Vec<Provider>,
    #[serde(default)]
    pub inlay_hints: Vec<Provider>,
    /// Source control providers (`[[scm]]`).
    #[serde(default)]
    pub scm: Vec<ScmProvider>,
    #[serde(default)]
    pub pull_requests: Vec<PullRequestProvider>,
}

/// `[[scm]]`: a source control provider. It claims a workspace folder that has one of `markers` at its top (a `.hg` or
/// `.jj` folder, a file): Source Control and the gutter's change marks ask it there, and git stays for the others.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmProvider {
    /// Full id: `publisher.name.local`.
    pub id: String,
    /// Shown in Source Control: `Mercurial`, `Jujutsu`.
    pub label: String,
    /// Names relative to the folder, such as `.hg`.
    pub markers: Vec<String>,
}

impl Contributions {
    /// Contributions that need extension code to do anything.
    pub fn needs_code(&self) -> bool {
        !self.commands.is_empty()
            || !self.views.is_empty()
            || !self.status_items.is_empty()
            || !self.formatters.is_empty()
            || !self.code_actions.is_empty()
            || !self.diagnostics.is_empty()
            || !self.decorations.is_empty()
            || !self.tasks.is_empty()
            || !self.hover.is_empty()
            || !self.completion.is_empty()
            || !self.code_lenses.is_empty()
            || !self.inlay_hints.is_empty()
            || !self.scm.is_empty()
            || !self.pull_requests.is_empty()
    }

    /// Languages a provider of this extension handles, each once.
    pub fn provider_languages(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let completion = self.completion.iter().map(|c| &c.languages);
        let all = self.formatters.iter().chain(&self.code_actions).chain(&self.diagnostics).chain(&self.decorations).chain(&self.hover);
        let all = all.chain(&self.code_lenses).chain(&self.inlay_hints);
        for languages in all.map(|p| &p.languages).chain(completion) {
            for l in languages {
                if !out.contains(l) {
                    out.push(l.clone());
                }
            }
        }
        out
    }
}

/// `[[pull-requests]]`: pull (merge) requests of the repository remotes on `hosts`, merged into the Pull Requests view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestProvider {
    /// Full id: `publisher.name.local`.
    pub id: String,
    /// Shown next to each request, such as `Gitea`.
    pub label: String,
    /// Remote hosts it claims, in the form of `net` entries (`gitea.example.com`, `*.example.com`).
    pub hosts: Vec<String>,
    /// The `auth` provider its requests are signed with, passed to it with each remote.
    pub auth: Option<String>,
}

impl PullRequestProvider {
    /// Whether a remote on `host` is this provider's.
    pub fn claims(&self, host: &str) -> bool {
        net_allows(&self.hosts, &host.to_ascii_lowercase())
    }
}

/// What a refused `auth.fetch` to a host outside the account's hosts carries as its permission:
/// `[[oauth]] gitlab hosts = ["evil.example.org"]`. It has a `=`, so the SDKs keep it, and [`oauth_host_fix`] reads it.
pub fn oauth_host_line(provider: &str, host: &str) -> String {
    format!("[[oauth]] {provider} hosts = [{host:?}]")
}

/// The provider and host of an [`oauth_host_line`].
pub fn parse_oauth_host_line(permission: &str) -> Option<(&str, &str)> {
    let (provider, rest) = permission.strip_prefix("[[oauth]] ")?.split_once(" hosts = [\"")?;
    Some((provider, rest.strip_suffix("\"]")?))
}

/// What a refused call carries as its permission when no manifest entry could allow it: a file outside the workspace,
/// or a URL that names an IP address. `what` is the call (`read file:///etc/passwd`), `reason` says why it cannot be
/// granted. It has no `= [`, so the SDKs keep it, and they print the reason instead of a line to add
/// ([`parse_ungrantable_line`]).
pub fn ungrantable_line(what: &str, reason: &str) -> String {
    format!("{what} {UNGRANTABLE}{reason})")
}

const UNGRANTABLE: &str = "(cannot be granted: ";

/// The call and the reason of an [`ungrantable_line`].
pub fn parse_ungrantable_line(permission: &str) -> Option<(&str, &str)> {
    let (what, rest) = permission.split_once(&format!(" {UNGRANTABLE}"))?;
    Some((what, rest.strip_suffix(')')?))
}

/// An IP address rather than a host name: `127.0.0.1`, `[::1]`, and the other spellings resolvers take for an IPv4
/// address (`127.1`, `2130706433`, `0x7f.1`): a name whose labels are all numbers, or whose last one is (no top-level
/// domain is numeric). `net` lists host names only.
pub fn is_ip_address(host: &str) -> bool {
    let number =
        |l: &str| !l.is_empty() && (l.bytes().all(|b| b.is_ascii_digit()) || l.strip_prefix("0x").is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit())));
    host.starts_with('[')
        || host.parse::<std::net::IpAddr>().is_ok()
        || host.split('.').all(number)
        || host.rsplit('.').next().is_some_and(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()))
}

/// The fix for an [`oauth_host_line`]: the host goes into `hosts` of that `[[oauth]]` table. GitHub's account is built
/// in, so its hosts are fixed.
pub fn oauth_host_fix(permission: &str) -> Option<String> {
    let (provider, host) = parse_oauth_host_line(permission)?;
    Some(if BUILTIN_PROVIDERS.contains(&provider) {
        format!("the built-in {provider} account signs requests only to its own API hosts; send the request to {host} with fetch (net) instead")
    } else {
        format!("add \"{host}\" to hosts in the [[oauth]] table with id = \"{provider}\"")
    })
}

/// Account providers built into Codeyard; `[permissions] auth` may name them without an `[[oauth]]` table.
pub const BUILTIN_PROVIDERS: &[&str] = &["github"];

/// The hosts the built-in `github` account signs requests to.
pub const GITHUB_HOSTS: &[&str] = &["api.github.com", "uploads.github.com"];

/// The hosts `auth.fetch` signs requests to for `provider`: GitHub's API hosts, or the `[[oauth]]` table's `hosts`.
pub fn account_hosts(m: &Manifest, provider: &str) -> Option<Vec<String>> {
    match m.oauth_provider(provider) {
        Some(o) => Some(o.hosts.clone()),
        None if provider == "github" => Some(GITHUB_HOSTS.iter().map(|h| h.to_string()).collect()),
        None => None,
    }
}

/// `[[oauth]]`: an OAuth 2 provider declared by the extension. Codeyard runs the browser sign-in (authorization code
/// with PKCE, returning through `codeyard://ext/<id>/callback`), keeps the token in the keychain and signs requests to
/// `hosts`. The extension never sees the token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthProvider {
    /// The name `[permissions] auth` and `auth.session` use.
    pub id: String,
    /// Shown to the user: "Act as your <name> account".
    pub name: String,
    /// `https://` only.
    pub authorize_url: String,
    /// `https://` only: the code and refresh tokens are traded here.
    pub token_url: String,
    pub client_id: String,
    /// Asked for at sign-in; `auth.session` may ask for fewer, never others.
    pub scopes: Vec<String>,
    /// The hosts `auth.fetch` signs requests to, in the form of `net` entries.
    pub hosts: Vec<String>,
    /// An `https://` URL that answers JSON about the signed-in account; its name is shown to the user.
    pub account_url: Option<String>,
    /// The user may add self-managed instances (`instances = true`, #184): the same endpoints on another host, with the
    /// user's own OAuth application. The URLs are then all on [`OAuthProvider::home`], which `hosts` lists.
    #[serde(default)]
    pub instances: bool,
}

impl OAuthProvider {
    /// The host the authorize URL is on: for a provider with `instances`, the one an instance replaces.
    pub fn home(&self) -> Option<String> {
        check_https_url(&self.authorize_url).ok()
    }

    /// This provider on the self-managed `host`, signed in with the user's `client_id`: the URLs on `host` instead of
    /// [`home`](Self::home), `hosts` only that host, and the id `<id>@<host>` (an account of its own). `None` when the
    /// manifest does not offer instances.
    pub fn instance(&self, host: &str, client_id: &str) -> Option<OAuthProvider> {
        let home = self.home().filter(|_| self.instances)?;
        let on = |url: &str| url.replacen(&format!("https://{home}"), &format!("https://{host}"), 1);
        Some(OAuthProvider {
            id: instance_account(&self.id, host),
            name: format!("{} ({host})", self.name),
            authorize_url: on(&self.authorize_url),
            token_url: on(&self.token_url),
            client_id: client_id.to_string(),
            scopes: self.scopes.clone(),
            hosts: vec![host.to_string()],
            account_url: self.account_url.as_deref().map(on),
            instances: false,
        })
    }
}

/// The account id of a self-managed instance: `gitlab@git.example.com`. It is what remotes on that host carry and what
/// the extension passes to `auth.session` and `auth.fetch`.
pub fn instance_account(provider: &str, host: &str) -> String {
    format!("{provider}@{host}")
}

/// `gitlab@git.example.com` → (`gitlab`, Some(`git.example.com`)); a plain provider id has no host.
pub fn split_account(account: &str) -> (&str, Option<&str>) {
    match account.split_once('@') {
        Some((p, h)) => (p, Some(h)),
        None => (account, None),
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
    /// Inferred from commands, views, providers and tasks plus `activation.workspace-contains`; empty for declarative
    /// extensions. (URI-handler and account activation arrive with those contribution points.)
    pub activation: Vec<Activation>,
    pub dependencies: Vec<(ExtId, Version)>,
    /// Runs in untrusted folders (with no outbound permissions there). Default true.
    pub untrusted: bool,
    pub contributions: Contributions,
    /// `[[oauth]]` providers, each named in `[permissions] auth`.
    #[serde(default)]
    pub oauth: Vec<OAuthProvider>,
}

impl Manifest {
    /// No code: the extension only adds data (themes, snippets, languages, …) and never starts a process of its own.
    pub fn is_declarative(&self) -> bool {
        self.code.is_none()
    }

    /// The `[[oauth]]` provider named `id`.
    pub fn oauth_provider(&self, id: &str) -> Option<&OAuthProvider> {
        self.oauth.iter().find(|p| p.id == id)
    }

    pub fn risk(&self) -> Risk {
        risk(&self.permissions)
    }

    /// It ships native programs inside its package (`run = ["./bin/server"]`): it is built and published per platform,
    /// and installing it asks the user to type its id (docs/plugin-architecture.md §6.3).
    pub fn has_native_programs(&self) -> bool {
        self.permissions.package_programs().next().is_some()
    }

    /// The permission set is high risk and has no `why`: an error at install and publish, a warning in `cyx dev`.
    pub fn missing_why(&self) -> bool {
        self.risk().needs_why() && self.permissions.why.is_none()
    }
}
