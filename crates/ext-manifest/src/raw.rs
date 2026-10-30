//! `codeyard-extension.toml` exactly as written. Unknown fields are errors, so a typo is reported instead of ignored.
//! Fields an error can point at are `Spanned`.

use std::collections::BTreeMap;

use serde::Deserialize;
use toml::Spanned;

pub type S<T> = Spanned<T>;

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawManifest {
    pub extension: S<RawExtension>,
    pub code: Option<S<RawCode>>,
    #[serde(default)]
    pub permissions: Option<S<RawPermissions>>,
    #[serde(default)]
    pub activation: Option<RawActivation>,
    #[serde(default)]
    pub dependencies: BTreeMap<S<String>, S<String>>,
    pub untrusted: Option<bool>,

    #[serde(default)]
    pub themes: Vec<S<RawFileEntry>>,
    #[serde(default)]
    pub snippets: Vec<S<RawSnippets>>,
    #[serde(default)]
    pub keymap_preset: Vec<S<RawFileEntry>>,
    #[serde(default)]
    pub keybindings: Vec<S<RawKeybinding>>,
    #[serde(default)]
    pub languages: Vec<S<RawLanguage>>,
    #[serde(default)]
    pub json_schemas: Vec<S<RawSchema>>,
    #[serde(default)]
    pub yaml_schemas: Vec<S<RawSchema>>,
    #[serde(default)]
    pub settings: BTreeMap<S<String>, S<RawSetting>>,
    #[serde(default)]
    pub language_servers: Vec<S<RawLanguageServer>>,
    #[serde(default)]
    pub commands: Vec<S<RawCommand>>,
    #[serde(default)]
    pub views: Vec<S<RawView>>,
    #[serde(default)]
    pub oauth: Vec<S<RawOAuth>>,
    #[serde(default)]
    pub menus: Vec<S<RawMenu>>,
    #[serde(default)]
    pub status_bar: Vec<S<RawStatusItem>>,
    #[serde(default)]
    pub formatters: Vec<S<RawLanguages>>,
    #[serde(default)]
    pub code_actions: Vec<S<RawLanguages>>,
    #[serde(default)]
    pub diagnostics: Vec<S<RawNamedLanguages>>,
    #[serde(default)]
    pub decorations: Vec<S<RawNamedLanguages>>,
    #[serde(default)]
    pub tasks: Vec<S<RawTasks>>,
    #[serde(default)]
    pub code_lens: Vec<S<RawLanguages>>,
    #[serde(default)]
    pub inlay_hints: Vec<S<RawLanguages>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawExtension {
    pub id: S<String>,
    pub version: S<String>,
    pub name: S<String>,
    pub license: S<String>,
    pub description: Option<S<String>>,
    pub repository: Option<S<String>>,
    #[serde(default)]
    pub categories: Vec<S<String>>,
    pub icon: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawCode {
    pub entry: S<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawPermissions {
    #[serde(default)]
    pub read: Vec<S<String>>,
    #[serde(default)]
    pub write: Vec<S<String>>,
    #[serde(default)]
    pub net: Vec<S<String>>,
    #[serde(default)]
    pub auth: Vec<S<String>>,
    #[serde(default)]
    pub secrets: bool,
    #[serde(default)]
    pub run: Vec<S<String>>,
    pub clipboard: Option<S<String>>,
    #[serde(default)]
    pub terminal: bool,
    #[serde(default)]
    pub commands: Vec<S<String>>,
    pub why: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawActivation {
    #[serde(default)]
    pub workspace_contains: Vec<S<String>>,
}

/// `[[themes]]`, `[[keymap-preset]]`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawFileEntry {
    pub label: S<String>,
    pub path: S<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawSnippets {
    pub language: S<String>,
    pub path: S<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawKeybinding {
    pub key: S<String>,
    pub command: S<String>,
    pub when: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawLanguage {
    pub id: S<String>,
    pub name: Option<S<String>>,
    #[serde(default)]
    pub extensions: Vec<S<String>>,
    #[serde(default)]
    pub filenames: Vec<S<String>>,
    /// A built-in grammar to highlight with, by its language id (`rust`, `toml`, …).
    pub grammar: Option<S<String>>,
    pub line_comment: Option<S<String>>,
    pub block_comment: Option<S<[String; 2]>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawSchema {
    pub file_match: Vec<S<String>>,
    pub path: S<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawSetting {
    #[serde(rename = "type")]
    pub kind: S<String>,
    pub default: Option<toml::Value>,
    pub description: S<String>,
    #[serde(default, rename = "enum")]
    pub choices: Vec<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawLanguageServer {
    pub command: S<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub languages: Vec<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawCommand {
    pub id: S<String>,
    pub title: S<String>,
    pub category: Option<S<String>>,
    pub enablement: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawView {
    pub id: S<String>,
    pub title: S<String>,
    pub location: S<String>,
    pub when: Option<S<String>>,
}

/// `[[oauth]]`: an OAuth 2 provider the extension signs in to (authorization code with PKCE).
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawOAuth {
    pub id: S<String>,
    pub name: S<String>,
    pub authorize_url: S<String>,
    pub token_url: S<String>,
    pub client_id: S<String>,
    #[serde(default)]
    pub scopes: Vec<S<String>>,
    pub hosts: Vec<S<String>>,
    pub account_url: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawMenu {
    pub command: S<String>,
    pub location: S<String>,
    /// For `view-title`: the view (local id).
    pub view: Option<S<String>>,
    pub when: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawStatusItem {
    pub id: S<String>,
    pub command: Option<S<String>>,
}

/// `[[formatters]]`, `[[code-actions]]`, `[[code-lens]]`, `[[inlay-hints]]`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawLanguages {
    pub languages: Vec<S<String>>,
}

/// `[[diagnostics]]`, `[[decorations]]`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawNamedLanguages {
    pub id: S<String>,
    pub languages: Vec<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawTasks {
    pub id: S<String>,
}
