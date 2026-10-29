//! From `raw` to `Manifest`: every check, every error with its place and fix.

use std::collections::HashSet;
use std::ops::Range;

use crate::raw::{RawManifest, RawOAuth, RawPermissions, S};
use crate::risk::{self, Clipboard, Permissions};
use crate::{
    Activation, BUILTIN_PROVIDERS, CodeEntry, CodeLanguage, Command, Contributions, ExtId, FileEntry, Keybinding, Language, LanguageServer, Manifest,
    OAuthProvider, SchemaAssociation, Setting, SettingKind, Snippets, Version, View, ViewLocation, check_id_part, limits, when,
};
use crate::{Code as C, Diagnostic};

struct Ctx {
    errors: Vec<Diagnostic>,
    options: Options,
}

impl Ctx {
    fn err(&mut self, code: C, span: Range<usize>, message: impl Into<String>) -> &mut Diagnostic {
        self.errors.push(Diagnostic::new(code, message).at(Some(span)));
        self.errors.last_mut().expect("just pushed")
    }

    /// A required text field: not empty, at most `max` characters, nothing that can disguise text on the consent sheet.
    fn text(&mut self, what: &str, v: &S<String>, max: usize) -> String {
        let s = v.get_ref().trim();
        if s.is_empty() {
            self.err(C::BadText, v.span(), format!("{what} is empty"));
        } else if s.chars().count() > max {
            self.err(C::BadText, v.span(), format!("{what} is longer than {max} characters"));
        } else if let Some(c) = s.chars().find(|&c| is_deceptive(c)) {
            self.err(C::BadText, v.span(), format!("{what} contains the invisible or control character U+{:04X}", c as u32));
        }
        s.to_string()
    }

    /// A path inside the package: relative, `/`-separated, no `..`, no empty parts.
    fn path(&mut self, what: &str, v: &S<String>) -> String {
        let p = v.get_ref();
        if let Err(e) = check_package_path(p) {
            self.err(C::BadPath, v.span(), format!("{what} '{p}' {e}")).fix = Some("use a path relative to the extension folder, like themes/dark.json".into());
        }
        p.clone()
    }

    fn when(&mut self, what: &str, v: &Option<S<String>>) -> Option<String> {
        let v = v.as_ref()?;
        if let Err(e) = when::parse(v.get_ref()) {
            self.err(C::BadWhen, v.span(), format!("{what}: {e}"));
        }
        Some(v.get_ref().clone())
    }

    fn count<T>(&mut self, what: &str, items: &[S<T>]) {
        if items.len() > limits::CONTRIBUTIONS_PER_KIND
            && let Some(first) = items.get(limits::CONTRIBUTIONS_PER_KIND)
        {
            self.err(C::BadContribution, first.span(), format!("more than {} {what}", limits::CONTRIBUTIONS_PER_KIND));
        }
    }
}

/// Characters that change how text looks without being seen: controls, bidirectional overrides and zero-width
/// characters. Rejected everywhere the consent sheet or a file name could show them.
pub fn is_deceptive(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{061C}' | '\u{FEFF}' | '\u{00AD}')
}

/// Windows device names, refused as a file name with any extension (`con.json` opens the console there).
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7",
    "lpt8", "lpt9",
];

/// A path inside the package: relative, `/`-separated, the same file on every platform.
pub fn check_package_path(p: &str) -> Result<(), String> {
    if p.is_empty() {
        return Err("is empty".into());
    }
    if p.len() > limits::ITEM_LEN {
        return Err(format!("is longer than {} characters", limits::ITEM_LEN));
    }
    if p.contains('\\') {
        return Err("uses '\\'; use '/'".into());
    }
    if p.starts_with('/') || p.starts_with('~') || p.contains(':') {
        return Err("is absolute or names a drive or stream".into());
    }
    if p.chars().any(is_deceptive) || p.contains(['*', '?', '"', '<', '>', '|']) {
        return Err("contains a character file names cannot have on every platform".into());
    }
    for part in p.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err("must not contain '.', '..' or empty parts".into());
        }
        if part.ends_with(['.', ' ']) {
            return Err(format!("has a part ending in '.' or a space ('{part}')"));
        }
        let stem = part.split('.').next().unwrap_or(part).to_ascii_lowercase();
        if RESERVED.contains(&stem.as_str()) {
            return Err(format!("uses the reserved Windows name '{part}'"));
        }
    }
    Ok(())
}

/// Options for `parse_with`.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// A high-risk permission set without `why` is an error. `cyx dev` turns this off to warn instead (it calls
    /// `missing_why`), so an author can try things before writing the sentence; install and publish keep it on.
    pub require_why: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { require_why: true }
    }
}

pub fn parse(text: &str, options: Options) -> Result<Manifest, Vec<Diagnostic>> {
    if text.len() > limits::MANIFEST_BYTES {
        return Err(vec![Diagnostic::new(C::TooLarge, format!("the manifest is larger than {} KiB", limits::MANIFEST_BYTES / 1024))]);
    }
    let raw: RawManifest = match toml::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            let message = e.message().trim().to_string();
            let fix = if message.contains("missing field `extension`") {
                Some("start the file with [extension] and its id, version, name and license")
            } else if message.contains("unknown field") {
                Some("check the spelling against the manifest reference")
            } else {
                None
            };
            let d = Diagnostic::new(C::Syntax, message).at(e.span());
            return Err(vec![match fix {
                Some(f) => d.fix(f),
                None => d,
            }]);
        }
    };
    let mut cx = Ctx { errors: Vec::new(), options };
    let m = build(&mut cx, raw);
    if cx.errors.is_empty() { Ok(m) } else { Err(cx.errors) }
}

fn build(cx: &mut Ctx, raw: RawManifest) -> Manifest {
    let ext = raw.extension.get_ref();
    let id = match ExtId::parse(ext.id.get_ref()) {
        Ok(id) => id,
        Err(e) => {
            cx.err(C::BadId, ext.id.span(), format!("extension.id {e}")).fix = Some("use <publisher>.<name>, like dogan.deploy-status".into());
            ExtId("invalid.invalid".into())
        }
    };
    let version = Version::parse(ext.version.get_ref()).unwrap_or_else(|e| {
        cx.err(C::BadVersion, ext.version.span(), format!("extension.version {e}"));
        Version { major: 0, minor: 0, patch: 0, pre: None }
    });
    let name = cx.text("extension.name", &ext.name, limits::NAME_LEN);
    let license = cx.text("extension.license", &ext.license, limits::ITEM_LEN);
    let description = ext.description.as_ref().map(|d| cx.text("extension.description", d, limits::DESCRIPTION_LEN));
    let repository = ext.repository.as_ref().map(|r| {
        let s = cx.text("extension.repository", r, limits::ITEM_LEN);
        if !s.starts_with("https://") {
            cx.err(C::BadText, r.span(), "extension.repository must be an https:// URL");
        }
        s
    });
    if let Some(extra) = ext.categories.get(limits::CATEGORIES) {
        cx.err(C::BadText, extra.span(), format!("more than {} categories", limits::CATEGORIES));
    }
    let categories = ext.categories.iter().map(|c| cx.text("a category", c, limits::NAME_LEN)).collect();
    let icon = ext.icon.as_ref().map(|i| cx.path("extension.icon", i));

    let code = raw.code.as_ref().map(|c| {
        let entry = c.get_ref().entry.get_ref().clone();
        let span = c.get_ref().entry.span();
        let language = if entry == "Cargo.toml" || entry.ends_with("/Cargo.toml") {
            CodeLanguage::Rust
        } else if entry.ends_with(".ts") {
            CodeLanguage::TypeScript
        } else if entry.ends_with(".js") || entry.ends_with(".mjs") {
            CodeLanguage::JavaScript
        } else {
            cx.err(C::BadContribution, span.clone(), format!("code.entry '{entry}' is not a Cargo.toml, .ts or .js file")).fix =
                Some("point code.entry at Cargo.toml for Rust or at the main .ts/.js file".into());
            CodeLanguage::Rust
        };
        cx.path("code.entry", &c.get_ref().entry);
        CodeEntry { entry, language }
    });

    let permissions = raw.permissions.as_ref().map(|p| permissions(cx, p.get_ref())).unwrap_or_default();

    let mut dependencies = Vec::new();
    if raw.dependencies.len() > limits::DEPENDENCIES {
        cx.errors.push(Diagnostic::new(C::BadDependency, format!("more than {} dependencies", limits::DEPENDENCIES)));
    }
    for (dep, min) in &raw.dependencies {
        match (ExtId::parse(dep.get_ref()), Version::parse_minimum(min.get_ref())) {
            (Ok(d), _) if d == id => {
                cx.err(C::BadDependency, dep.span(), "an extension cannot depend on itself");
            }
            (Ok(d), Ok(v)) => dependencies.push((d, v)),
            (Err(e), _) => {
                cx.err(C::BadDependency, dep.span(), format!("dependency '{}' {e}", dep.get_ref()));
            }
            (_, Err(e)) => {
                cx.err(C::BadDependency, min.span(), format!("dependency version '{}' {e}", min.get_ref()));
            }
        }
    }

    let contributions = contributions(cx, &raw, &id, &permissions);
    let oauth = oauth(cx, &raw.oauth, raw.permissions.as_ref().map(|p| p.get_ref()), &permissions);

    let mut activation: Vec<Activation> = Vec::new();
    if code.is_some() {
        activation.extend(contributions.commands.iter().map(|c| Activation::Command(c.id.clone())));
        activation.extend(contributions.views.iter().map(|v| Activation::View(v.id.clone())));
    }
    if let (Some(a), None) = (&raw.activation, &raw.code)
        && let Some(first) = a.workspace_contains.first()
    {
        cx.err(C::BadContribution, first.span(), "[activation] only applies to extensions with [code]").fix =
            Some("remove [activation]: themes, snippets, languages and other no-code contributions are always on".into());
    }
    for g in raw.activation.iter().flat_map(|a| &a.workspace_contains) {
        if let Err(e) = check_workspace_glob(g.get_ref()) {
            cx.err(C::BadPath, g.span(), format!("activation.workspace-contains '{}' {e}", g.get_ref()));
        }
        activation.push(Activation::WorkspaceContains(g.get_ref().clone()));
    }
    if let Some(c) = &raw.code
        && activation.is_empty()
    {
        cx.err(C::NeverActivates, c.span(), "this extension has code but nothing ever starts it").fix =
            Some("add a [[commands]] or [[views]] entry, or [activation] workspace-contains = [\"some-file\"]".into());
    }
    if code.is_none() && contributions.needs_code() {
        let span = raw.commands.first().map(|c| c.span()).or_else(|| raw.views.first().map(|v| v.span()));
        cx.errors.push(
            Diagnostic::new(C::NeedsCode, "commands and views need extension code, but there is no [code] table")
                .at(span)
                .fix("add [code] entry = \"src/main.ts\" (or \"Cargo.toml\"), or run `cyx new command` to start from a template"),
        );
    }

    Manifest {
        id,
        version,
        name,
        license,
        description,
        repository,
        categories,
        icon,
        code,
        permissions,
        activation,
        dependencies,
        untrusted: raw.untrusted.unwrap_or(true),
        contributions,
        oauth,
    }
}

/// An `https://` URL with a host name (the rules of `net`), no user info and no fragment. Returns the host.
pub fn check_https_url(u: &str) -> Result<String, String> {
    let Some(rest) = u.strip_prefix("https://") else { return Err("must start with https://".into()) };
    if u.len() > limits::ITEM_LEN {
        return Err(format!("is longer than {} characters", limits::ITEM_LEN));
    }
    if u.contains('#') || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("must not contain spaces, control characters or a #fragment".into());
    }
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err("must not contain a user name or password".into());
    }
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if let Some(port) = port
        && port.parse::<u16>().is_err()
    {
        return Err(format!("has an invalid port '{port}'"));
    }
    if host.starts_with("*.") {
        return Err("must name one host, without *".into());
    }
    check_host(host).map_err(|e| format!("host '{host}' {e}"))?;
    Ok(host.to_string())
}

/// `[[oauth]]` tables: each well formed and named in `[permissions] auth`; every `auth` entry is either built in or
/// declared here.
fn oauth(cx: &mut Ctx, raw: &[S<RawOAuth>], perms_raw: Option<&RawPermissions>, perms: &Permissions) -> Vec<OAuthProvider> {
    cx.count("oauth providers", raw);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in raw {
        let span = r.span();
        let r = r.get_ref();
        let id = r.id.get_ref().clone();
        if let Err(e) = check_id_part(&id) {
            cx.err(C::BadContribution, r.id.span(), format!("oauth id '{id}' {e}"));
        } else if BUILTIN_PROVIDERS.contains(&id.as_str()) {
            cx.err(C::BadContribution, r.id.span(), format!("oauth id '{id}' is built into Codeyard")).fix =
                Some(format!("remove this [[oauth]] table; auth = [\"{id}\"] is enough"));
        } else if !seen.insert(id.clone()) {
            cx.err(C::DuplicateId, r.id.span(), format!("oauth id '{id}' is used twice"));
        } else if !perms.auth.contains(&id) {
            cx.err(C::NotDeclared, span.clone(), format!("oauth provider '{id}' is not in [permissions] auth")).fix =
                Some(format!("add to [permissions]:  auth = [\"{id}\"]"));
        }
        let name = cx.text("oauth name", &r.name, limits::NAME_LEN);
        let mut url = |what: &str, v: &S<String>| {
            if let Err(e) = check_https_url(v.get_ref()) {
                cx.err(C::BadContribution, v.span(), format!("oauth {what} '{}' {e}", v.get_ref()));
            }
            v.get_ref().clone()
        };
        let authorize_url = url("authorize-url", &r.authorize_url);
        let token_url = url("token-url", &r.token_url);
        let account_url = r.account_url.as_ref().map(|a| url("account-url", a));
        let client_id = cx.text("oauth client-id", &r.client_id, limits::ITEM_LEN);
        if client_id.chars().any(|c| c.is_whitespace() || matches!(c, '&' | '#' | '"')) {
            cx.err(C::BadContribution, r.client_id.span(), "oauth client-id must not contain spaces, &, # or quotes");
        }
        let scopes = list(cx, "oauth scopes", &r.scopes, |s| {
            let ok = !s.is_empty() && s.len() <= limits::ITEM_LEN && s.chars().all(|c| c.is_ascii_graphic() && c != '"' && c != '\\');
            if ok { Ok(()) } else { Err("must be one scope: printable characters without spaces or quotes".into()) }
        });
        if r.hosts.is_empty() {
            cx.err(C::BadContribution, span.clone(), format!("oauth provider '{id}' lists no hosts")).fix =
                Some("add  hosts = [\"api.example.com\"]: the hosts whose requests Codeyard signs".into());
        }
        let hosts = list(cx, "oauth hosts", &r.hosts, check_host);
        out.push(OAuthProvider { id, name, authorize_url, token_url, client_id, scopes, hosts, account_url });
    }
    if let Some(p) = perms_raw {
        for a in &p.auth {
            let a_id = a.get_ref();
            if check_id_part(a_id).is_ok() && !BUILTIN_PROVIDERS.contains(&a_id.as_str()) && !raw.iter().any(|r| r.get_ref().id.get_ref() == a_id) {
                cx.err(C::NotDeclared, a.span(), format!("permissions.auth '{a_id}' is not a built-in provider and has no [[oauth]] table")).fix =
                    Some(format!("use auth = [\"github\"], or add [[oauth]] with id = \"{a_id}\", its authorize-url, token-url, client-id and hosts"));
            }
        }
    }
    out
}

fn check_workspace_glob(g: &str) -> Result<(), String> {
    if g.is_empty() || g.len() > limits::ITEM_LEN {
        return Err(format!("must be 1 to {} characters", limits::ITEM_LEN));
    }
    if g.starts_with('/') || g.starts_with('~') || g.contains('\\') || g.contains(':') {
        return Err("must be relative to the workspace and '/'-separated".into());
    }
    if g.contains(['{', '}']) {
        return Err("must not use {a,b} alternatives; list each glob separately".into());
    }
    if g.chars().any(is_deceptive) {
        return Err("contains an invisible or control character".into());
    }
    let body = g.strip_prefix("./").unwrap_or(g);
    if body.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
        return Err("must not contain '.', '..' or empty parts".into());
    }
    Ok(())
}

/// `core:` followed by a command id as the command palette lists it: ASCII letters, digits and `-`, in dot-separated
/// parts (`core:workbench.openSettings`).
fn check_core_command(c: &str) -> Result<(), String> {
    let ok = c.strip_prefix("core:").is_some_and(|id| {
        !id.is_empty()
            && id.len() <= limits::ID_LEN
            && id.split('.').all(|p| p.starts_with(|c: char| c.is_ascii_alphabetic()) && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    });
    if ok { Ok(()) } else { Err("must name a core command, like core:workbench.openSettings".into()) }
}

fn check_host(h: &str) -> Result<(), String> {
    let name = h.strip_prefix("*.").unwrap_or(h);
    if h == "*" {
        return Err("allows every host; list the hosts you call".into());
    }
    if h.contains("://") || h.contains('/') || h.contains(':') {
        return Err("must be a host name only, without scheme, port or path".into());
    }
    if name.parse::<std::net::Ipv4Addr>().is_ok() || name.split('.').next_back().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit())) {
        return Err("must be a name, not an IP address".into());
    }
    if name.len() > 253 || !name.contains('.') {
        return Err("must be a full host name, like api.example.com".into());
    }
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-') {
            return Err(format!("has an invalid label '{label}'"));
        }
        if !label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err("must be lowercase a-z, 0-9, - and dots".into());
        }
    }
    Ok(())
}

fn check_program(p: &str) -> Result<(), String> {
    if p.is_empty() || p.len() > limits::ITEM_LEN {
        return Err(format!("must be 1 to {} characters", limits::ITEM_LEN));
    }
    if p.contains(char::is_whitespace) {
        return Err("must be one program name; arguments are not part of it".into());
    }
    if p.starts_with('/') || p.contains('\\') || p.as_bytes().get(1) == Some(&b':') || p.starts_with('~') {
        return Err("must be a program name found on PATH, or a path inside the package".into());
    }
    if p.contains('/') {
        check_package_path(p.trim_start_matches("./"))?;
    }
    Ok(())
}

fn list(cx: &mut Ctx, what: &str, items: &[S<String>], check: impl Fn(&str) -> Result<(), String>) -> Vec<String> {
    if let Some(extra) = items.get(limits::PERMISSION_ENTRIES) {
        cx.err(C::BadPermission, extra.span(), format!("permissions.{what} has more than {} entries", limits::PERMISSION_ENTRIES));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for v in items {
        if let Err(e) = check(v.get_ref()) {
            cx.err(C::BadPermission, v.span(), format!("permissions.{what} '{}' {e}", v.get_ref()));
        }
        if seen.insert(v.get_ref().clone()) {
            out.push(v.get_ref().clone());
        }
    }
    out
}

fn permissions(cx: &mut Ctx, p: &RawPermissions) -> Permissions {
    let read = list(cx, "read", &p.read, check_workspace_glob);
    let write = list(cx, "write", &p.write, check_workspace_glob);
    let net = list(cx, "net", &p.net, check_host);
    let auth = list(cx, "auth", &p.auth, check_id_part);
    let run = list(cx, "run", &p.run, check_program);
    let commands = list(cx, "commands", &p.commands, check_core_command);
    let clipboard = match p.clipboard.as_ref().map(|c| (c.get_ref().as_str(), c.span())) {
        None => Clipboard::None,
        Some(("read", _)) => Clipboard::Read,
        Some(("write", _)) => Clipboard::Write,
        Some(("read-write", _)) => Clipboard::ReadWrite,
        Some((other, span)) => {
            cx.err(C::BadPermission, span, format!("permissions.clipboard '{other}' must be \"read\", \"write\" or \"read-write\""));
            Clipboard::None
        }
    };
    let why = p.why.as_ref().map(|w| {
        let s = cx.text("permissions.why", w, limits::WHY_LEN);
        if s.contains("://") || s.contains("www.") {
            cx.err(C::BadText, w.span(), "permissions.why must not contain links");
        }
        s
    });
    let perms = Permissions { read, write, net, auth, secrets: p.secrets, run, clipboard, terminal: p.terminal, commands, why };
    let r = risk::risk(&perms);
    if r.needs_why() && perms.why.is_none() && cx.options.require_why {
        let reason = if r.runs_code {
            "it can run code as you".to_string()
        } else {
            format!("it can read private data ({}) and send it ({})", r.reads.join(", "), r.sends.join(", "))
        };
        cx.errors.push(
            Diagnostic::new(C::WhyRequired, format!("these permissions are high risk because {reason}, so users need to know why"))
                .fix("add to [permissions]:  why = \"One honest sentence about what you do with them.\""),
        );
        // Point at the table so the error has a line.
        if let Some(last) = cx.errors.last_mut() {
            last.span = p.run.first().or(p.net.first()).or(p.read.first()).map(|s| s.span());
        }
    }
    perms
}

fn local_id(cx: &mut Ctx, what: &str, v: &S<String>, seen: &mut HashSet<String>) -> String {
    let s = v.get_ref();
    if let Err(e) = check_id_part(s) {
        cx.err(C::BadContribution, v.span(), format!("{what} id '{s}' {e}")).fix =
            Some("use a short local id like refresh; Codeyard adds your extension id in front".into());
    } else if !seen.insert(s.clone()) {
        cx.err(C::DuplicateId, v.span(), format!("{what} id '{s}' is used twice"));
    }
    s.clone()
}

fn contributions(cx: &mut Ctx, raw: &RawManifest, id: &ExtId, perms: &Permissions) -> Contributions {
    let mut out = Contributions::default();
    let file_entries = |cx: &mut Ctx, what: &str, items: &[S<crate::raw::RawFileEntry>]| -> Vec<FileEntry> {
        cx.count(what, items);
        items
            .iter()
            .map(|e| {
                let e = e.get_ref();
                FileEntry { label: cx.text(&format!("{what} label"), &e.label, limits::NAME_LEN), path: cx.path(&format!("{what} path"), &e.path) }
            })
            .collect()
    };
    out.themes = file_entries(cx, "themes", &raw.themes);
    out.keymap_presets = file_entries(cx, "keymap-preset", &raw.keymap_preset);
    for e in out.themes.iter().zip(&raw.themes) {
        if !e.0.path.ends_with(".json") {
            cx.err(C::BadContribution, e.1.get_ref().path.span(), format!("theme '{}' must be a .json file", e.0.path));
        }
    }

    cx.count("snippets", &raw.snippets);
    out.snippets = raw
        .snippets
        .iter()
        .map(|s| {
            let s = s.get_ref();
            Snippets { language: cx.text("snippets language", &s.language, limits::ID_PART_LEN), path: cx.path("snippets path", &s.path) }
        })
        .collect();

    let mut command_ids = HashSet::new();
    cx.count("commands", &raw.commands);
    for c in &raw.commands {
        let c = c.get_ref();
        let local = local_id(cx, "command", &c.id, &mut command_ids);
        let title = cx.text("command title", &c.title, limits::NAME_LEN);
        let category = c.category.as_ref().map(|v| cx.text("command category", v, limits::NAME_LEN));
        let enablement = cx.when("command enablement", &c.enablement);
        out.commands.push(Command { id: id.qualify(&local), title, category, enablement });
    }

    let mut view_ids = HashSet::new();
    cx.count("views", &raw.views);
    for v in &raw.views {
        let v = v.get_ref();
        let local = local_id(cx, "view", &v.id, &mut view_ids);
        let title = cx.text("view title", &v.title, limits::NAME_LEN);
        let location = match v.location.get_ref().as_str() {
            "sidebar" => ViewLocation::Sidebar,
            "bottom" => ViewLocation::Bottom,
            "editor" => ViewLocation::Editor,
            other => {
                cx.err(C::BadContribution, v.location.span(), format!("view location '{other}' must be \"sidebar\", \"bottom\" or \"editor\""));
                ViewLocation::Sidebar
            }
        };
        let when = cx.when("view when", &v.when);
        out.views.push(View { id: id.qualify(&local), title, location, when });
    }

    cx.count("keybindings", &raw.keybindings);
    let deps: HashSet<&str> = raw.dependencies.keys().map(|k| k.get_ref().as_str()).collect();
    for k in &raw.keybindings {
        let k = k.get_ref();
        let key = k.key.get_ref().trim().to_string();
        if key.is_empty() || key.len() > 64 || key.split(' ').any(|stroke| stroke.is_empty() || stroke.split('-').any(str::is_empty)) {
            cx.err(C::BadContribution, k.key.span(), format!("key '{key}' must look like \"cmd-shift-r\" or \"ctrl-k ctrl-r\""));
        }
        let cmd = k.command.get_ref();
        // A binding only acts when the user presses the key, so binding a core command needs no permission. What it may
        // not do is take a key a built-in binding uses; the app refuses that when it applies the keymap.
        let command = if cmd.starts_with("core:") {
            if let Err(e) = check_core_command(cmd) {
                cx.err(C::BadContribution, k.command.span(), format!("keybinding command '{cmd}' {e}"));
            }
            cmd.clone()
        } else if command_ids.contains(cmd) {
            id.qualify(cmd)
        } else if cmd.rsplit_once('.').is_some_and(|(owner, _)| deps.contains(owner)) {
            cmd.clone()
        } else {
            cx.err(
                C::BadContribution,
                k.command.span(),
                format!("keybinding command '{cmd}' is not one of your [[commands]], a dependency's command or a core: command"),
            );
            cmd.clone()
        };
        let when = cx.when("keybinding when", &k.when);
        out.keybindings.push(Keybinding { key, command, when });
    }

    cx.count("languages", &raw.languages);
    let mut language_ids = HashSet::new();
    for l in &raw.languages {
        let l = l.get_ref();
        let lid = l.id.get_ref().clone();
        if let Err(e) = check_id_part(&lid) {
            cx.err(C::BadContribution, l.id.span(), format!("language id '{lid}' {e}"));
        } else if !language_ids.insert(lid.clone()) {
            cx.err(C::DuplicateId, l.id.span(), format!("language id '{lid}' is used twice"));
        }
        if l.extensions.is_empty() && l.filenames.is_empty() {
            cx.err(C::BadContribution, l.id.span(), format!("language '{lid}' matches no files")).fix =
                Some("add extensions = [\"ext\"] or filenames = [\"Name\"]".into());
        }
        let extensions = l
            .extensions
            .iter()
            .map(|e| {
                let s = e.get_ref().trim_start_matches('.').to_string();
                if s.is_empty() || s.contains('/') || s.contains(char::is_whitespace) {
                    cx.err(C::BadContribution, e.span(), format!("file extension '{}' must be like \"toml\"", e.get_ref()));
                }
                s
            })
            .collect();
        let filenames = l
            .filenames
            .iter()
            .map(|f| {
                if f.get_ref().is_empty() || f.get_ref().contains('/') {
                    cx.err(C::BadContribution, f.span(), format!("file name '{}' must be a plain name like \"Justfile\"", f.get_ref()));
                }
                f.get_ref().clone()
            })
            .collect();
        out.languages.push(Language {
            id: lid,
            name: l.name.as_ref().map(|n| cx.text("language name", n, limits::NAME_LEN)),
            extensions,
            filenames,
            grammar: l.grammar.as_ref().map(|g| g.get_ref().clone()),
            line_comment: l.line_comment.as_ref().map(|c| c.get_ref().clone()),
            block_comment: l.block_comment.as_ref().map(|b| (b.get_ref()[0].clone(), b.get_ref()[1].clone())),
        });
    }

    for (what, items, target) in [("json-schemas", &raw.json_schemas, 0), ("yaml-schemas", &raw.yaml_schemas, 1)] {
        cx.count(what, items);
        let list: Vec<SchemaAssociation> = items
            .iter()
            .map(|s| {
                let s = s.get_ref();
                if s.file_match.is_empty() {
                    cx.err(C::BadContribution, s.path.span(), format!("{what} entry needs at least one file-match glob"));
                }
                let file_match = s
                    .file_match
                    .iter()
                    .map(|g| {
                        if let Err(e) = check_workspace_glob(g.get_ref()) {
                            cx.err(C::BadPath, g.span(), format!("{what} file-match '{}' {e}", g.get_ref()));
                        }
                        g.get_ref().clone()
                    })
                    .collect();
                SchemaAssociation { file_match, path: cx.path(&format!("{what} path"), &s.path) }
            })
            .collect();
        if target == 0 {
            out.json_schemas = list;
        } else {
            out.yaml_schemas = list;
        }
    }

    for (key, s) in &raw.settings {
        let s = s.get_ref();
        let mut ignored = HashSet::new();
        let key_s = local_id(cx, "setting", key, &mut ignored);
        let kind = match s.kind.get_ref().as_str() {
            "string" => SettingKind::String,
            "number" => SettingKind::Number,
            "boolean" => SettingKind::Boolean,
            "array" => SettingKind::Array,
            "object" => SettingKind::Object,
            other => {
                cx.err(C::BadContribution, s.kind.span(), format!("setting type '{other}' must be string, number, boolean, array or object"));
                SettingKind::String
            }
        };
        if let Some(d) = &s.default {
            let ok = matches!(
                (kind, d),
                (SettingKind::String, toml::Value::String(_))
                    | (SettingKind::Number, toml::Value::Integer(_) | toml::Value::Float(_))
                    | (SettingKind::Boolean, toml::Value::Boolean(_))
                    | (SettingKind::Array, toml::Value::Array(_))
                    | (SettingKind::Object, toml::Value::Table(_))
            );
            if !ok {
                cx.err(C::BadContribution, s.kind.span(), format!("setting '{key_s}' default does not match its type {}", s.kind.get_ref()));
            }
        }
        if !s.choices.is_empty() && kind != SettingKind::String {
            cx.err(C::BadContribution, s.kind.span(), format!("setting '{key_s}' has enum values, so its type must be string"));
        }
        if let (Some(toml::Value::String(d)), false) = (&s.default, s.choices.is_empty())
            && !s.choices.iter().any(|c| c.get_ref() == d)
        {
            cx.err(C::BadContribution, s.kind.span(), format!("setting '{key_s}' default '{d}' is not one of its enum values"));
        }
        out.settings.push(Setting {
            key: key_s,
            kind,
            default: s.default.clone(),
            description: cx.text("setting description", &s.description, limits::DESCRIPTION_LEN),
            choices: s.choices.iter().map(|c| c.get_ref().clone()).collect(),
        });
    }

    let need_run = |cx: &mut Ctx, what: &str, command: &S<String>| {
        let c = command.get_ref();
        if let Err(e) = check_program(c) {
            cx.err(C::BadContribution, command.span(), format!("{what} command '{c}' {e}"));
        } else if !perms.run.iter().any(|r| r == c) {
            cx.err(C::NotDeclared, command.span(), format!("run access to \"{c}\" is not declared")).fix =
                Some(format!("add to [permissions]:  run = [\"{c}\"]"));
        }
        c.clone()
    };
    cx.count("language-servers", &raw.language_servers);
    for s in &raw.language_servers {
        let s = s.get_ref();
        let command = need_run(cx, "language server", &s.command);
        if s.languages.is_empty() {
            cx.err(C::BadContribution, s.command.span(), "a language server needs at least one language");
        }
        out.language_servers.push(LanguageServer { command, args: s.args.clone(), languages: s.languages.iter().map(|l| l.get_ref().clone()).collect() });
    }
    out
}
