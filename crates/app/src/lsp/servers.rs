//! Language server table: which server serves which file, install hints, user overrides.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Key in the settings and the LSP languageId ("rust", "python"...).
    pub language: String,
    pub command: String,
    pub args: Vec<String>,
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
        _ => return None,
    })
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

/// Server definitions from the settings: language → (command, arguments); `None` disables that language.
pub type ServerOverrides = [(String, Option<(String, Vec<String>)>)];

/// Configuration for a server key: settings override the defaults.
pub fn server_for(key: &str, overrides: &ServerOverrides) -> Option<ServerConfig> {
    if let Some((_, o)) = overrides.iter().rev().find(|(k, _)| k == key) {
        return o.as_ref().map(|(command, args)| ServerConfig { language: key.into(), command: command.clone(), args: args.clone() });
    }
    let (command, args) = default_server(key)?;
    Some(ServerConfig { language: key.into(), command: command.into(), args: args.iter().map(|s| s.to_string()).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_overrides() {
        assert_eq!(server_for("rust", &[]).unwrap().command, "rust-analyzer");
        let o = vec![("python".to_string(), Some(("pylsp".to_string(), vec![]))), ("go".to_string(), None)];
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
        let o = vec![("toml".to_string(), Some(("taplo".to_string(), vec!["lsp".to_string(), "stdio".to_string()])))];
        let cfg = server_for("toml", &o).unwrap();
        assert_eq!((cfg.command.as_str(), cfg.args.as_slice()), ("taplo", &["lsp".to_string(), "stdio".to_string()][..]));
        // Defaults can be turned off, and a later entry wins over an earlier one.
        let off = vec![("java".to_string(), None)];
        assert!(server_for("java", &off).is_none());
        let twice = vec![("php".to_string(), None), ("php".to_string(), Some(("phpactor".to_string(), vec!["language-server".to_string()])))];
        assert_eq!(server_for("php", &twice).unwrap().command, "phpactor");
    }
}
