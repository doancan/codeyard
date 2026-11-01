//! `codeyard-extension.toml` exactly as written. Unknown fields are errors, so a typo is reported instead of ignored.
//! Fields an error can point at are `Spanned`.

use std::collections::BTreeMap;

use serde::Deserialize;
use toml::Spanned;

pub type S<T> = Spanned<T>;

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawManifest {
    /// Who the extension is: id, version, name, license.
    pub extension: S<RawExtension>,
    /// Where the extension's code is. Leave it out for themes, snippets and other no-code packs.
    pub code: Option<S<RawCode>>,
    /// What the code may reach: files, hosts, programs, accounts. Anything not listed is refused.
    #[serde(default)]
    pub permissions: Option<S<RawPermissions>>,
    /// Rarely needed: Codeyard starts the code when one of its contributions is first used.
    #[serde(default)]
    pub activation: Option<RawActivation>,
    /// Other extensions this one needs, by full id, each with a minimum version: `"other.ext" = "1.2"`.
    #[serde(default)]
    pub dependencies: BTreeMap<S<String>, S<String>>,
    /// `false` keeps the extension off in untrusted folders. By default it runs there, but cannot send, write or run anything.
    pub untrusted: Option<bool>,

    /// A colour theme: a Codeyard or VS Code colour theme JSON file.
    #[serde(default)]
    pub themes: Vec<S<RawFileEntry>>,
    /// Snippets for one language, in the VS Code snippet format.
    #[serde(default)]
    pub snippets: Vec<S<RawSnippets>>,
    /// A whole keymap the user can pick with `"keymap.preset": "<extension id>/<label>"`.
    #[serde(default)]
    pub keymap_preset: Vec<S<RawFileEntry>>,
    /// A key binding for one of your commands, a dependency's command or a `core:` command. A key a built-in shortcut uses is left out.
    #[serde(default)]
    pub keybindings: Vec<S<RawKeybinding>>,
    /// A language for files no built-in language claims.
    #[serde(default)]
    pub languages: Vec<S<RawLanguage>>,
    /// A JSON schema for the JSON files that match.
    #[serde(default)]
    pub json_schemas: Vec<S<RawSchema>>,
    /// A JSON schema for the YAML files that match.
    #[serde(default)]
    pub yaml_schemas: Vec<S<RawSchema>>,
    /// A setting, as `[settings.<key>]`. The user sets it as `<extension id>.<key>`; code reads it by its key.
    #[serde(default)]
    pub settings: BTreeMap<S<String>, S<RawSetting>>,
    /// A language server: a program that speaks the Language Server Protocol. It runs where the files are, so it works over SSH.
    #[serde(default)]
    pub language_servers: Vec<S<RawLanguageServer>>,
    /// A command in the palette. Needs `[code]`; the code registers its handler with the same id.
    #[serde(default)]
    pub commands: Vec<S<RawCommand>>,
    /// A view drawn from built-in elements (reference/ui.md). Needs `[code]`.
    #[serde(default)]
    pub views: Vec<S<RawView>>,
    /// An OAuth 2 provider the extension signs in to, named in `[permissions] auth`.
    #[serde(default)]
    pub oauth: Vec<S<RawOAuth>>,
    /// An entry in a context menu or a view's title bar that runs a command.
    #[serde(default)]
    pub menus: Vec<S<RawMenu>>,
    /// A status bar item; the code sets its text with this id.
    #[serde(default)]
    pub status_bar: Vec<S<RawStatusItem>>,
    /// Formats documents of these languages (`request.format`).
    #[serde(default)]
    pub formatters: Vec<S<RawLanguages>>,
    /// Code actions for these languages, shown in Quick Fix (`request.code-actions`).
    #[serde(default)]
    pub code_actions: Vec<S<RawLanguages>>,
    /// A set of diagnostics the code pushes for files of these languages.
    #[serde(default)]
    pub diagnostics: Vec<S<RawNamedLanguages>>,
    /// A set of decorations (gutter, line, after the text) the code pushes for files of these languages.
    #[serde(default)]
    pub decorations: Vec<S<RawNamedLanguages>>,
    /// Tasks the code lists and runs (`request.provide-tasks`, `request.run-task`).
    #[serde(default)]
    pub tasks: Vec<S<RawTasks>>,
    /// Hover text for these languages (`request.hover`).
    #[serde(default)]
    pub hover: Vec<S<RawLanguages>>,
    /// Completion items for these languages (`request.completion`).
    #[serde(default)]
    pub completion: Vec<S<RawCompletion>>,
    /// Code lenses for the lines in view (`request.code-lens`).
    #[serde(default)]
    pub code_lens: Vec<S<RawLanguages>>,
    /// Inlay hints for the lines in view (`request.inlay-hints`).
    #[serde(default)]
    pub inlay_hints: Vec<S<RawLanguages>>,
    /// A source control provider for workspace folders that contain one of its markers.
    #[serde(default)]
    pub scm: Vec<S<RawScm>>,
    /// A pull (merge) request provider for the repository remotes on its hosts.
    #[serde(default)]
    pub pull_requests: Vec<S<RawPullRequests>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawExtension {
    /// `<publisher>.<name>`, lowercase `a-z 0-9 -`. The publisher is claimed on first publish.
    pub id: S<String>,
    /// `major.minor.patch`, with an optional pre-release suffix (`1.0.0-beta.1`).
    pub version: S<String>,
    /// The display name.
    pub name: S<String>,
    /// An SPDX expression, such as `MIT` or `MIT OR Apache-2.0`.
    pub license: S<String>,
    /// One or two sentences for the Extensions view.
    pub description: Option<S<String>>,
    /// An `https://` URL of the source.
    pub repository: Option<S<String>>,
    /// Words that group the extension in the Extensions view.
    #[serde(default)]
    pub categories: Vec<S<String>>,
    /// A path to an image in the package.
    pub icon: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawCode {
    /// `Cargo.toml` for Rust, a `.ts` or `.js` file for TypeScript or JavaScript.
    pub entry: S<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawPermissions {
    /// Workspace globs the code may read. `.env*` and `.git/` must be named explicitly.
    #[serde(default)]
    pub read: Vec<S<String>>,
    /// Workspace globs the code may write.
    #[serde(default)]
    pub write: Vec<S<String>>,
    /// Hosts the code may reach over HTTPS: `api.example.com`, or `*.example.com` for its subdomains.
    #[serde(default)]
    pub net: Vec<S<String>>,
    /// A string setting (the key of a `[[settings]]` entry) that names a server of the user's, such as a company
    /// GitHub Enterprise: the user allows that host on the extension page, and the code may then reach it over HTTPS.
    pub net_from_setting: Option<S<String>>,
    /// Accounts the code may sign requests with: `github`, or the id of an `[[oauth]]` table. It never sees the token.
    #[serde(default)]
    pub auth: Vec<S<String>>,
    /// Keychain entries of the extension's own.
    #[serde(default)]
    pub secrets: bool,
    /// Programs the code may run, with any arguments, on the machine the workspace is on.
    #[serde(default)]
    pub run: Vec<S<String>>,
    /// `"read"`, `"write"` or `"read-write"`.
    pub clipboard: Option<S<String>>,
    /// Send text to the integrated terminal.
    #[serde(default)]
    pub terminal: bool,
    /// `core:` commands the code runs (reference/core-commands.md).
    #[serde(default)]
    pub commands: Vec<S<String>>,
    /// One sentence for the install sheet. Required when the permissions are high risk.
    pub why: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawActivation {
    /// Start the code in folders that contain a file matching one of these globs (files `.gitignore` ignores do not count).
    #[serde(default)]
    pub workspace_contains: Vec<S<String>>,
}

/// `[[themes]]`, `[[keymap-preset]]`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawFileEntry {
    /// The name the user picks it by.
    pub label: S<String>,
    /// The file in the package.
    pub path: S<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawSnippets {
    /// The language id the snippets apply to.
    pub language: S<String>,
    /// The snippet file in the package.
    pub path: S<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawKeybinding {
    /// Key strokes such as `"cmd-shift-r"` or `"ctrl-k ctrl-r"`.
    pub key: S<String>,
    /// The local id of one of your commands, a dependency's full id, or a `core:` command.
    pub command: S<String>,
    /// A `when` expression; the binding applies only while it holds.
    pub when: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawLanguage {
    /// The language id, lowercase `a-z 0-9 -`.
    pub id: S<String>,
    /// The display name.
    pub name: Option<S<String>>,
    /// File extensions without the dot (`"toml"`). A language needs these or `filenames`.
    #[serde(default)]
    pub extensions: Vec<S<String>>,
    /// Whole file names (`"Justfile"`).
    #[serde(default)]
    pub filenames: Vec<S<String>>,
    /// A built-in grammar to highlight with, by its language id (`rust`, `toml`, …).
    pub grammar: Option<S<String>>,
    /// The line comment token (`"#"`), for Toggle Comment.
    pub line_comment: Option<S<String>>,
    /// The start and end of a block comment (`["/*", "*/"]`).
    pub block_comment: Option<S<[String; 2]>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawSchema {
    /// Workspace globs of the files the schema applies to.
    pub file_match: Vec<S<String>>,
    /// The schema file in the package.
    pub path: S<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawSetting {
    /// `string`, `number`, `boolean`, `array` or `object`.
    #[serde(rename = "type")]
    pub kind: S<String>,
    /// The value when the user has not set one; it must match `type`.
    pub default: Option<toml::Value>,
    /// What the setting does, shown in Settings.
    pub description: S<String>,
    /// The allowed values of a string setting.
    #[serde(default, rename = "enum")]
    pub choices: Vec<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawLanguageServer {
    /// The program, also listed in `[permissions] run`: a name on `PATH` or one inside the package.
    pub command: S<String>,
    /// Arguments passed to the program.
    #[serde(default)]
    pub args: Vec<String>,
    /// Language ids it serves.
    pub languages: Vec<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawCommand {
    /// The local id: `refresh` becomes `<extension id>.refresh`.
    pub id: S<String>,
    /// The palette title, such as `"Deploy: Refresh"`.
    pub title: S<String>,
    /// Groups the command in the palette.
    pub category: Option<S<String>>,
    /// A `when` expression; the command is greyed out while it does not hold.
    pub enablement: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawView {
    /// The local id.
    pub id: S<String>,
    /// The view's title.
    pub title: S<String>,
    /// `sidebar`, `bottom` or `editor`.
    pub location: S<String>,
    /// A `when` expression; the view is offered only while it holds.
    pub when: Option<S<String>>,
}

/// `[[oauth]]`: an OAuth 2 provider the extension signs in to (authorization code with PKCE).
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawOAuth {
    /// The account id, named in `[permissions] auth`.
    pub id: S<String>,
    /// The provider's name, shown to the user.
    pub name: S<String>,
    /// The `https://` authorization endpoint.
    pub authorize_url: S<String>,
    /// The `https://` token endpoint.
    pub token_url: S<String>,
    /// The public client id. There is no client secret: PKCE replaces it.
    pub client_id: S<String>,
    /// The scopes to ask for; `auth.session` may ask for fewer.
    #[serde(default)]
    pub scopes: Vec<S<String>>,
    /// The hosts `auth.fetch` signs requests to; any other host is refused.
    pub hosts: Vec<S<String>>,
    /// An `https://` URL that answers JSON with the account's name, shown on the extension's page.
    pub account_url: Option<S<String>>,
    /// The user may add self-managed instances of this provider (#184).
    pub instances: Option<S<bool>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawMenu {
    /// One of your commands, a dependency's full id, or a `core:` command that takes no argument.
    pub command: S<String>,
    /// `editor-context`, `file-tree` or `view-title`.
    pub location: S<String>,
    /// For `view-title`: the view (local id).
    pub view: Option<S<String>>,
    /// A `when` expression; the entry is hidden while it does not hold.
    pub when: Option<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawStatusItem {
    /// The local id the code sets the item's text with. An undeclared id is not shown.
    pub id: S<String>,
    /// The command a click runs, unless the item names another at run time.
    pub command: Option<S<String>>,
}

/// `[[formatters]]`, `[[code-actions]]`, `[[hover]]`, `[[code-lens]]`, `[[inlay-hints]]`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawLanguages {
    /// Language ids it serves.
    pub languages: Vec<S<String>>,
}

/// `[[diagnostics]]`, `[[decorations]]`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawNamedLanguages {
    /// The local id the code pushes them under.
    pub id: S<String>,
    /// Language ids it serves.
    pub languages: Vec<S<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawTasks {
    /// The local id.
    pub id: S<String>,
}

/// `[[completion]]`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawCompletion {
    /// Language ids it serves.
    pub languages: Vec<S<String>>,
    /// Punctuation characters that ask too, besides typing a word (`["."]`).
    #[serde(default)]
    pub trigger_characters: Vec<S<String>>,
}

/// `[[scm]]`: a source control provider for folders that contain one of `markers`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawScm {
    /// The local id; the code answers for it.
    pub id: S<String>,
    /// The name shown in Source Control.
    pub label: S<String>,
    /// Files or folders at the top of a workspace folder that it claims, such as `".hg"`.
    pub markers: Vec<S<String>>,
}

/// `[[pull-requests]]`: a pull (merge) request provider for the remotes on `hosts`.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RawPullRequests {
    /// The local id; the code answers for it.
    pub id: S<String>,
    /// Shown next to each request.
    pub label: S<String>,
    /// The hosts of the remotes it claims; `*.example.com` works as in `net`.
    pub hosts: S<Vec<S<String>>>,
    /// An account from `[permissions] auth`, passed to the code as `remote.account`.
    pub auth: Option<S<String>>,
}
