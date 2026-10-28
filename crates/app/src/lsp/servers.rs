//! Language server table: which server serves which file, install hints, user overrides.

use std::sync::Arc;

use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Key in the settings and the LSP languageId ("rust", "python"...).
    pub language: String,
    pub command: String,
    pub args: Vec<String>,
    /// The `settings` tree the server reads through `workspace/configuration` (`Null`: none configured).
    pub settings: Arc<Value>,
    /// Sent unchanged as `initializationOptions` with `initialize`.
    pub init_options: Option<Value>,
    /// Only default commands may resolve to an explicitly installed managed service.
    pub managed: Option<crate::language_services::Service>,
}

#[cfg(test)]
impl ServerConfig {
    /// A server with no configuration of its own.
    pub fn plain(language: &str, command: &str, args: Vec<String>) -> Self {
        ServerConfig { language: language.into(), command: command.into(), args, settings: Arc::new(Value::Null), init_options: None, managed: None }
    }
}

/// One `lsp.servers` entry. `command: None` keeps the default server of the key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerSpec {
    pub command: Option<String>,
    pub args: Vec<String>,
    /// Dotted keys already expanded (`expand_dotted`); `Null` when none is configured. Shared, never copied: a tree can be
    /// megabytes.
    pub settings: Arc<Value>,
    pub init_options: Option<Value>,
}

impl From<(String, Vec<String>)> for ServerSpec {
    fn from((command, args): (String, Vec<String>)) -> Self {
        ServerSpec { command: Some(command), args, ..ServerSpec::default() }
    }
}

impl ServerSpec {
    /// Whether a reload from `self` to `new` needs the server process restarted: everything but `settings` is only read
    /// at start; `settings` reach a running server through `workspace/didChangeConfiguration`.
    fn restart_needed(&self, new: &ServerSpec) -> bool {
        (&self.command, &self.args, &self.init_options) != (&new.command, &new.args, &new.init_options)
    }
}

/// File -> (languageId, server key). One server can carry several languages.
/// Keys without an entry in `DEFAULT_SERVERS` (json, yaml, toml) are opt-in through `lsp.servers`.
pub fn language_of(path: &str) -> Option<(&'static str, &'static str)> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if matches!(name, "Gemfile" | "Rakefile" | "Podfile" | "Vagrantfile" | "Guardfile") {
        return Some(("ruby", "ruby"));
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    Some(match ext.as_str() {
        "rs" => ("rust", "rust"),
        "c" | "h" => ("c", "c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => ("cpp", "c"),
        "go" => ("go", "go"),
        "py" | "pyi" => ("python", "python"),
        "ts" | "mts" | "cts" => ("typescript", "typescript"),
        "tsx" => ("typescriptreact", "typescript"),
        "js" | "mjs" | "cjs" => ("javascript", "typescript"),
        "jsx" => ("javascriptreact", "typescript"),
        "java" => ("java", "java"),
        "cs" => ("csharp", "csharp"),
        "php" | "phtml" => ("php", "php"),
        "rb" | "rake" | "gemspec" | "ru" => ("ruby", "ruby"),
        "kt" | "kts" => ("kotlin", "kotlin"),
        "swift" => ("swift", "swift"),
        "lua" => ("lua", "lua"),
        "sh" | "bash" => ("shellscript", "bash"),
        "html" | "htm" => ("html", "html"),
        "css" => ("css", "css"),
        "scss" => ("scss", "css"),
        "less" => ("less", "css"),
        // No default server: the built-in schema engine already checks JSON and YAML, and the usual servers
        // (vscode-json-language-server, yaml-language-server, taplo) download schemas on their own, outside
        // `schema.download` and the confirmation of remote requests. Opt in with `lsp.servers`.
        "json" => ("json", "json"),
        "jsonc" => ("jsonc", "json"),
        "yaml" | "yml" => ("yaml", "yaml"),
        "toml" => ("toml", "toml"),
        // A language an installed extension adds, for files no built-in language claims: its id is also its key.
        _ => return crate::ext::language_for(path).map(|l| (l.id, l.id)),
    })
}

/// The server key of a built-in language id (`typescriptreact` → `typescript`); `None` for ids the table does not know,
/// whose key is the id itself.
pub fn key_for_language(lang: &str) -> Option<&'static str> {
    const SAMPLES: &[&str] = &[
        "x.rs", "x.c", "x.cpp", "x.go", "x.py", "x.ts", "x.tsx", "x.js", "x.jsx", "x.java", "x.cs", "x.php", "x.rb", "x.kt", "x.swift", "x.lua", "x.sh",
        "x.html", "x.css", "x.scss", "x.less", "x.json", "x.jsonc", "x.yaml", "x.toml",
    ];
    SAMPLES.iter().filter_map(|f| language_of(f)).find(|(id, _)| *id == lang).map(|(_, key)| key)
}

/// Default server per key: (key, command, arguments, install hint shown when it is missing).
/// Commands are looked up on the host's PATH; macOS ships a `sourcekit-lsp` shim next to `xcrun`.
pub const DEFAULT_SERVERS: &[(&str, &str, &[&str], &str)] = &[
    ("rust", "rust-analyzer", &[], "rustup component add rust-analyzer"),
    ("c", "clangd", &[], "install clangd (Xcode tools, apt install clangd)"),
    ("go", "gopls", &[], "go install golang.org/x/tools/gopls@latest"),
    ("python", "pyright-langserver", &["--stdio"], "npm i -g pyright"),
    ("typescript", "typescript-language-server", &["--stdio"], "npm i -g typescript-language-server typescript"),
    ("java", "jdtls", &[], "brew install jdtls, or unpack Eclipse JDT LS and put jdtls on PATH"),
    ("csharp", "csharp-ls", &[], "dotnet tool install -g csharp-ls"),
    ("php", "intelephense", &["--stdio"], "npm i -g intelephense"),
    ("ruby", "ruby-lsp", &[], "gem install ruby-lsp"),
    ("kotlin", "kotlin-language-server", &[], "brew install kotlin-language-server"),
    ("swift", "sourcekit-lsp", &[], "install Xcode or a Swift toolchain (it contains sourcekit-lsp)"),
    ("lua", "lua-language-server", &[], "brew install lua-language-server"),
    ("bash", "bash-language-server", &["start"], "npm i -g bash-language-server"),
    ("html", "vscode-html-language-server", &["--stdio"], "npm i -g vscode-langservers-extracted"),
    ("css", "vscode-css-language-server", &["--stdio"], "npm i -g vscode-langservers-extracted"),
];

fn default_server(key: &str) -> Option<(&'static str, &'static [&'static str])> {
    DEFAULT_SERVERS.iter().find(|d| d.0 == key).map(|d| (d.1, d.2))
}

/// Title of a list of locations (`what` names the kind: the languages differ in plural forms, so each kind has its own key).
pub fn locations_title(what: &str, n: usize) -> String {
    use crate::i18n::tn;
    match what {
        "references" => tn("{} reference", "{} references", n, &[]),
        "bookmarks" => tn("{} bookmark", "{} bookmarks", n, &[]),
        "implementations" => tn("{} implementation", "{} implementations", n, &[]),
        "definition" => tn("{} definition", "{} definitions", n, &[]),
        "type definition" => tn("{} type definition", "{} type definitions", n, &[]),
        "incoming calls" => tn("{} incoming call", "{} incoming calls", n, &[]),
        "outgoing calls" => tn("{} outgoing call", "{} outgoing calls", n, &[]),
        "supertypes" => tn("{} supertype", "{} supertypes", n, &[]),
        "subtypes" => tn("{} subtype", "{} subtypes", n, &[]),
        "parent modules" => tn("{} parent module", "{} parent modules", n, &[]),
        _ => format!("{n} {what}"),
    }
}

/// Message when a location request found nothing (`what` as in `locations_title`).
pub fn none_found(what: &str) -> String {
    use crate::i18n::t;
    match what {
        "references" => t("No references found"),
        "implementations" => t("No implementations found"),
        "definition" => t("No definition found"),
        "type definition" => t("No type definition found"),
        "incoming calls" => t("No incoming calls found"),
        "outgoing calls" => t("No outgoing calls found"),
        "supertypes" => t("No supertypes found"),
        "subtypes" => t("No subtypes found"),
        _ => return format!("No {what} found"),
    }
    .to_string()
}

/// Message when the server lacks a hierarchy capability.
pub fn unsupported(what: &str) -> String {
    use crate::i18n::t;
    match what {
        "incoming calls" => t("This language server does not support incoming calls"),
        "outgoing calls" => t("This language server does not support outgoing calls"),
        "supertypes" => t("This language server does not support supertypes"),
        "subtypes" => t("This language server does not support subtypes"),
        _ => return format!("This language server does not support {what}"),
    }
    .to_string()
}

/// The install hint in the user's language (most hints are shell commands and stay as they are).
pub fn install_hint_text(hint: &str) -> String {
    use crate::i18n::t;
    match hint {
        "install clangd (Xcode tools, apt install clangd)" => t("install clangd (Xcode tools, apt install clangd)"),
        "brew install jdtls, or unpack Eclipse JDT LS and put jdtls on PATH" => t("brew install jdtls, or unpack Eclipse JDT LS and put jdtls on PATH"),
        "install Xcode or a Swift toolchain (it contains sourcekit-lsp)" => t("install Xcode or a Swift toolchain (it contains sourcekit-lsp)"),
        other => other,
    }
    .to_string()
}

/// Install command of the default server (shown in the missing-server notice).
pub fn install_hint(key: &str) -> Option<&'static str> {
    DEFAULT_SERVERS.iter().find(|d| d.0 == key).map(|d| d.3)
}

/// Server definitions from the settings: language → entry; `None` disables that language. A later entry wins.
pub type ServerOverrides = [(String, Option<ServerSpec>)];

/// The entry that applies to `key`: `None` when there is none, `Some(None)` when the language is turned off.
fn override_for<'a>(key: &str, overrides: &'a ServerOverrides) -> Option<Option<&'a ServerSpec>> {
    overrides.iter().rev().find(|(k, _)| k == key).map(|(_, o)| o.as_ref())
}

/// Configuration for a server key: settings override the defaults. An entry without `command` configures the default
/// server; a key with neither a default nor a `command` has no server.
pub fn server_for(key: &str, overrides: &ServerOverrides) -> Option<ServerConfig> {
    let spec = match override_for(key, overrides) {
        Some(None) => return None,
        Some(Some(spec)) => Some(spec),
        None => None,
    };
    let (command, args) = match spec.and_then(|s| s.command.as_ref()) {
        Some(command) => (command.clone(), spec.map(|s| s.args.clone()).unwrap_or_default()),
        None => {
            let (command, args) = default_server(key)?;
            // `args` without `command` would be arguments for a program the user did not name: the default's own apply.
            (command.to_string(), args.iter().map(|s| s.to_string()).collect())
        }
    };
    let managed = if spec.is_some_and(|s| s.command.is_some()) { None } else { crate::language_services::Service::for_language(key) };
    let settings = spec.map_or_else(|| Arc::new(Value::Null), |s| s.settings.clone());
    let settings = managed.map_or_else(|| settings.clone(), |service| Arc::new(service.configuration(&settings)));
    Some(ServerConfig { language: key.into(), command, args, settings, init_options: spec.and_then(|s| s.init_options.clone()), managed })
}

/// What a settings reload means for the running language servers.
#[derive(Debug, PartialEq)]
pub enum ServersReload {
    Unchanged,
    /// Only `settings` changed for these keys: tell their servers, do not restart them.
    Settings(Vec<String>),
    /// A command, its arguments or its initialization options changed, or an entry was added, removed or turned off.
    Restart,
}

/// Compares the `lsp.servers` of two settings, key by key (the effective entry of each key, so a duplicate that
/// changes nothing changes nothing).
pub fn servers_reload(old: &ServerOverrides, new: &ServerOverrides) -> ServersReload {
    let mut keys: Vec<&str> = old.iter().chain(new).map(|(k, _)| k.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    let mut settings = Vec::new();
    for key in keys {
        match (override_for(key, old), override_for(key, new)) {
            (a, b) if a == b => {}
            (Some(Some(a)), Some(Some(b))) if !a.restart_needed(b) => settings.push(key.to_string()),
            _ => return ServersReload::Restart,
        }
    }
    if settings.is_empty() { ServersReload::Unchanged } else { ServersReload::Settings(settings) }
}

/// Expands dotted keys into nested objects (`"a.b": 1` → `{ "a": { "b": 1 } }`), in document order: when two forms set
/// the same key, objects are merged and otherwise the later one wins. Keys with an empty segment (`"a..b"`, `".a"`) are
/// kept as they are.
pub fn expand_dotted(v: &Value) -> Value {
    let Value::Object(m) = v else { return v.clone() };
    let mut out = Map::new();
    for (k, v) in m {
        let v = expand_dotted(v);
        let path: Vec<&str> = if k.split('.').any(str::is_empty) { vec![k.as_str()] } else { k.split('.').collect() };
        insert_path(&mut out, &path, v);
    }
    Value::Object(out)
}

fn insert_path(obj: &mut Map<String, Value>, path: &[&str], v: Value) {
    let (first, rest) = path.split_first().expect("a key has at least one segment");
    if rest.is_empty() {
        match (obj.get_mut(*first), v) {
            (Some(Value::Object(old)), Value::Object(new)) => {
                for (k, v) in new {
                    insert_path(old, &[k.as_str()], v);
                }
            }
            (_, v) => {
                obj.insert(first.to_string(), v);
            }
        }
        return;
    }
    let slot = obj.entry(first.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !slot.is_object() {
        *slot = Value::Object(Map::new());
    }
    insert_path(slot.as_object_mut().expect("just made an object"), rest, v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn managed_services_apply_only_to_enabled_default_commands() {
        for (key, service) in [
            ("typescript", crate::language_services::Service::TypeScript),
            ("html", crate::language_services::Service::Html),
            ("css", crate::language_services::Service::Css),
        ] {
            assert_eq!(server_for(key, &[]).unwrap().managed, Some(service));
            assert!(server_for(key, &[(key.into(), None)]).is_none());
            let custom = vec![(key.into(), Some(("custom-server".into(), vec!["--custom".into()]).into()))];
            let config = server_for(key, &custom).unwrap();
            assert_eq!(config.managed, None);
            assert_eq!(config.command, "custom-server");
            assert_eq!(config.args, ["--custom"]);
        }
        assert_eq!(server_for("rust", &[]).unwrap().managed, None);
    }

    #[test]
    fn server_overrides() {
        assert_eq!(server_for("rust", &[]).unwrap().command, "rust-analyzer");
        let o = vec![("python".to_string(), Some(("pylsp".to_string(), vec![]).into())), ("go".to_string(), None)];
        assert_eq!(server_for("python", &o).unwrap().command, "pylsp");
        assert!(server_for("go", &o).is_none(), "null disables");
        assert_eq!(language_of("/a/b.tsx"), Some(("typescriptreact", "typescript")));
    }

    /// Every default server is reachable from at least one file name, carries an install hint and runs without a shell.
    #[test]
    fn every_default_server_has_files_a_hint_and_a_plain_command() {
        let samples: &[(&str, &str, &str)] = &[
            ("a.rs", "rust", "rust"),
            ("a.c", "c", "c"),
            ("A.java", "java", "java"),
            ("A.cs", "csharp", "csharp"),
            ("a.php", "php", "php"),
            ("a.rb", "ruby", "ruby"),
            ("Gemfile", "ruby", "ruby"),
            ("a.kt", "kotlin", "kotlin"),
            ("build.gradle.kts", "kotlin", "kotlin"),
            ("a.swift", "swift", "swift"),
            ("a.lua", "lua", "lua"),
            ("a.sh", "shellscript", "bash"),
            ("a.html", "html", "html"),
            ("a.css", "css", "css"),
            ("a.scss", "scss", "css"),
            ("a.less", "less", "css"),
        ];
        for (file, id, key) in samples {
            assert_eq!(language_of(&format!("/p/{file}")), Some((*id, *key)), "{file}");
        }
        let mut keys = std::collections::HashSet::new();
        for (key, command, args, hint) in DEFAULT_SERVERS {
            assert!(keys.insert(*key), "duplicate default for {key}");
            let cfg = server_for(key, &[]).unwrap_or_else(|| panic!("no default for {key}"));
            assert_eq!((cfg.command.as_str(), cfg.args.len()), (*command, args.len()));
            assert_eq!(cfg.language, *key);
            assert_eq!(install_hint(key), Some(*hint));
            assert!(!command.contains(' ') && !command.contains('/'), "{key}: the command is looked up on PATH, no shell syntax");
            assert!(hint.len() > 8);
        }
        // Every key a file can map to is either a default or one of the documented opt-in keys.
        for ext in [
            "rs", "c", "cpp", "go", "py", "ts", "tsx", "js", "jsx", "java", "cs", "php", "rb", "kt", "swift", "lua", "sh", "html", "css", "json", "yaml",
            "toml",
        ] {
            let (_, key) = language_of(&format!("/p/x.{ext}")).unwrap();
            assert!(default_server(key).is_some() || ["json", "yaml", "toml"].contains(&key), "{ext} -> {key}");
        }
    }

    /// JSON, YAML and TOML are checked by the built-in schema engine (no server, no duplicate diagnostics, no downloads
    /// outside `schema.download`): they have no default server, but `lsp.servers` can still switch one on.
    #[test]
    fn json_yaml_and_toml_servers_are_opt_in() {
        for (file, key) in [("a.json", "json"), ("a.jsonc", "json"), ("a.yaml", "yaml"), ("a.yml", "yaml"), ("Cargo.toml", "toml")] {
            let (_, k) = language_of(file).unwrap();
            assert_eq!(k, key);
            assert!(server_for(k, &[]).is_none(), "{key} must have no default server");
            assert!(install_hint(k).is_none());
        }
        let o = vec![("toml".to_string(), Some(("taplo".to_string(), vec!["lsp".to_string(), "stdio".to_string()]).into()))];
        let cfg = server_for("toml", &o).unwrap();
        assert_eq!((cfg.command.as_str(), cfg.args.as_slice()), ("taplo", &["lsp".to_string(), "stdio".to_string()][..]));
        // Defaults can be turned off, and a later entry wins over an earlier one.
        let off = vec![("java".to_string(), None)];
        assert!(server_for("java", &off).is_none());
        let twice = vec![("php".to_string(), None), ("php".to_string(), Some(("phpactor".to_string(), vec!["language-server".to_string()]).into()))];
        assert_eq!(server_for("php", &twice).unwrap().command, "phpactor");
    }

    fn spec(command: Option<&str>, settings: Value, init: Option<Value>) -> Option<ServerSpec> {
        Some(ServerSpec { command: command.map(str::to_string), args: Vec::new(), settings: Arc::new(settings), init_options: init })
    }

    /// An entry with only `settings` keeps the default server and carries the settings; `command` replaces it.
    #[test]
    fn server_for_falls_back_to_the_default_command_and_keeps_settings() {
        let tree = json!({ "rust-analyzer": { "check": { "command": "clippy" } } });
        let o = vec![("rust".to_string(), spec(None, tree.clone(), Some(json!({ "cargo": { "features": ["x"] } }))))];
        let cfg = server_for("rust", &o).unwrap();
        assert_eq!((cfg.command.as_str(), cfg.args.len()), ("rust-analyzer", 0));
        assert_eq!(*cfg.settings, tree);
        assert_eq!(cfg.init_options, Some(json!({ "cargo": { "features": ["x"] } })));
        // A default server's own arguments stay when only settings are given.
        let o = vec![("python".to_string(), spec(None, json!({ "python": {} }), None))];
        assert_eq!(server_for("python", &o).unwrap().args, vec!["--stdio".to_string()]);
        let o = vec![("go".to_string(), spec(Some("my-gopls"), json!({ "gopls": {} }), None))];
        let cfg = server_for("go", &o).unwrap();
        assert_eq!((cfg.command.as_str(), &*cfg.settings), ("my-gopls", &json!({ "gopls": {} })));
        // Without an entry: no settings, no options.
        let cfg = server_for("rust", &[]).unwrap();
        assert_eq!((&*cfg.settings, cfg.init_options), (&Value::Null, None));
    }

    /// `json` has no default server: settings alone start nothing.
    #[test]
    fn a_key_with_no_default_and_no_command_has_no_server() {
        let o = vec![("json".to_string(), spec(None, json!({ "json": { "format": {} } }), None))];
        assert!(server_for("json", &o).is_none());
    }

    #[test]
    fn a_reload_restarts_only_for_what_a_running_server_cannot_take() {
        let base = vec![("rust".to_string(), spec(None, json!({ "a": 1 }), None)), ("go".to_string(), spec(Some("gopls"), Value::Null, None))];
        assert_eq!(servers_reload(&base, &base.clone()), ServersReload::Unchanged);
        // Only settings: no restart, just the key.
        let mut only_settings = base.clone();
        only_settings[0].1 = spec(None, json!({ "a": 2 }), None);
        assert_eq!(servers_reload(&base, &only_settings), ServersReload::Settings(vec!["rust".into()]));
        // Arguments, options or the command: restart.
        let mut args = base.clone();
        args[1].1.as_mut().unwrap().args = vec!["-rpc.trace".into()];
        assert_eq!(servers_reload(&base, &args), ServersReload::Restart);
        let mut init = base.clone();
        init[0].1 = spec(None, json!({ "a": 1 }), Some(json!({})));
        assert_eq!(servers_reload(&base, &init), ServersReload::Restart);
        // Added, removed, turned off.
        assert_eq!(servers_reload(&base, &base[..1]), ServersReload::Restart);
        assert_eq!(servers_reload(&base[..1], &base), ServersReload::Restart);
        let mut off = base.clone();
        off[1].1 = None;
        assert_eq!(servers_reload(&base, &off), ServersReload::Restart);
        // A duplicate that leaves the effective entry as it was changes nothing.
        let mut dup = base.clone();
        dup.insert(0, ("go".to_string(), None));
        assert_eq!(servers_reload(&base, &dup), ServersReload::Unchanged);
    }

    #[test]
    fn dotted_keys_expand_and_merge_in_document_order() {
        let v: Value = serde_json::from_str(
            r#"{ "rust-analyzer": { "check": { "command": "check", "extraArgs": [] } }, "rust-analyzer.check.command": "clippy",
                 "rust-analyzer.cargo.features": ["x"], "a.b": 1, "a": { "c": 2 }, "x": 1, "x.y": 2, "..odd": 3,
                 "deep": { "p.q": { "r.s": true } } }"#,
        )
        .unwrap();
        let e = expand_dotted(&v);
        assert_eq!(e["rust-analyzer"], json!({ "check": { "command": "clippy", "extraArgs": [] }, "cargo": { "features": ["x"] } }));
        assert_eq!(e["a"], json!({ "b": 1, "c": 2 }), "nested after dotted merges");
        assert_eq!(e["x"], json!({ "y": 2 }), "a later dotted key replaces a value that is not an object");
        assert_eq!(e["..odd"], json!(3));
        assert_eq!(e["deep"], json!({ "p": { "q": { "r": { "s": true } } } }), "inside nested objects too");
        // Later nested form wins over an earlier dotted one, at the leaf.
        let v: Value = serde_json::from_str(r#"{ "a.b": 1, "a": { "b": 2 } }"#).unwrap();
        assert_eq!(expand_dotted(&v), json!({ "a": { "b": 2 } }));
        assert_eq!(expand_dotted(&json!([1, { "a.b": 1 }])), json!([1, { "a.b": 1 }]), "arrays are values, not settings");
    }
}
