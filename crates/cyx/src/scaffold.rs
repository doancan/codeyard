//! `cyx new`: a working extension of each kind. Every template passes `cyx check` as written, the code ones once built
//! (tests make sure; examples/extensions holds them, in Rust and TypeScript), so the first `cyx dev` shows something.

use std::path::Path;

use crate::Kind;

fn header(publisher: &str, name: &str, title: &str, description: &str) -> String {
    format!(
        "[extension]\nid          = \"{publisher}.{name}\"\nversion     = \"0.1.0\"\nname        = \"{title}\"\nlicense     = \"MIT\"\ndescription = \"{description}\"\n"
    )
}

/// "my-theme" → "My Theme".
fn title(name: &str) -> String {
    name.split('-').filter(|w| !w.is_empty()).map(|w| w[..1].to_uppercase() + &w[1..]).collect::<Vec<_>>().join(" ")
}

const THEME: &str = r##"{
  // Codeyard's own format: "appearance" plus the colour roles you want to change; every other role keeps the default.
  // A VS Code colour theme (with "colors" and "tokenColors") works here too.
  "name": "NAME",
  "appearance": "dark",
  "color": {
    "bg": "#1b1f27",
    "surface": "#20252e",
    "accent": "#7aa2f7",
    "text": "#c8d0e0",
    "syntax-keyword": "#bb9af7",
    "syntax-string": "#9ece6a",
    "syntax-function": "#7aa2f7",
    "syntax-comment": "#5c6370"
  }
}
"##;

const SNIPPETS: &str = r#"{
  // Snippet format: $1, ${2:default} are tab stops; $0 is where the cursor ends.
  "Print": {
    "prefix": "pr",
    "body": ["println!(\"{}\", $1);", "$0"],
    "description": "Print a value"
  }
}
"#;

const KEYMAP: &str = r#"[
  // keybindings.jsonc format. A preset may change built-in keys: users choose it with "keymap.preset".
  { "key": "ctrl-x ctrl-s", "command": "editor.save" },
  { "key": "ctrl-x ctrl-f", "command": "workbench.quickOpen" }
]
"#;

/// Publishing on a tag push (`.github/workflows/publish.yml`): the identity the registry pins is this repository and
/// this file, so it keeps one name. The job can sign releases, so a moved tag must not change the code it runs: the
/// checkout action is pinned to a commit, and the publish action to the release's commit. Until the first release that can
/// publish API 1.1 code is tagged there is no such commit, so the publish action follows `main` (as the SDK does,
/// `sdk_git_ref`). `docs/releasing.md` says how a release builds re-pin it.
pub const PUBLISH_WORKFLOW: &str = r#"# Publishes this extension to the Codeyard registry when you push a tag: git tag v0.2.0 && git push --tags
# No secrets: the job's identity token signs the release (keyless), so only this repository can publish it.
name: Publish
on:
  push:
    tags: ["v*"]
permissions:
  contents: read
  id-token: write
jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - uses: doancan/codeyard/actions/publish@a6e55db0a4a7b401d0ea569a10291eae0cee977e # v0.7.0
"#;

const README: &str = "# NAME\n\nA Codeyard extension. Run it with `cyx dev`, check it with `cyx check`, publish it with `cyx publish` or by pushing a tag (`git tag v0.1.0 && git push --tags`).\n";

const CODE_README: &str = "# NAME\n\nA Codeyard extension in Rust. You need Rust and its WebAssembly target: `rustup target add wasm32-wasip2`.\n\n```sh\ncargo test     # unit tests, against a mock Codeyard\ncyx test       # the built extension, in the real extension host\ncyx dev        # run it in Codeyard\ncyx package    # build extension.wasm and write the .cyx\n```\n\nPublish it with `cyx publish` or by pushing a tag (`git tag v0.1.0 && git push --tags`).\n";

/// The Rust crate of a code extension. Its own `[workspace]`, so it builds the same inside another project's folder.
const CARGO_TOML: &str = r#"[package]
name = "NAME"
version = "0.1.0"
edition = "2024"
publish = false

# A WebAssembly component: `cyx package` builds it with `cargo build --release --target wasm32-wasip2`.
[lib]
crate-type = ["cdylib"]

[dependencies]
SDK

[dev-dependencies]
TEST

[profile.release]
opt-level = "s"
lto = true
strip = "debuginfo"

[workspace]
"#;

const GITIGNORE: &str = "target/\nextension.wasm\n*.cyx\n";
const COMMAND_RS: &str = include_str!("../templates/command.rs");
const VIEW_RS: &str = include_str!("../templates/view.rs");

const TS_README: &str = "# NAME\n\nA Codeyard extension in TypeScript. You need Node.js 22.18 or newer; `cyx` installs its build tools (esbuild, ComponentizeJS) itself.\n\n```sh\nnpm install    # the SDK, its types and TypeScript\nnpm test       # unit tests, against a mock Codeyard\nnpm run check  # type-check\ncyx test       # the built extension, in the real extension host\ncyx dev        # run it in Codeyard\ncyx package    # build extension.wasm and write the .cyx\n```\n\nPublish it with `cyx publish` or by pushing a tag (`git tag v0.1.0 && git push --tags`).\n";

/// The npm project of a TypeScript extension. `npm test` runs the tests with Node's type stripping: no compile step.
const PACKAGE_JSON: &str = r#"{
  "name": "NAME",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "node --test \"test/*.test.ts\"",
    "check": "tsc"
  },
  "devDependencies": {
    "@codeyard/extension": "SDK",
    "@types/node": "24.19.2",
    "typescript": "6.0.3"
  }
}
"#;

const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "es2022",
    "module": "nodenext",
    "lib": ["es2023", "dom", "dom.iterable"],
    "types": ["node"],
    "strict": true,
    "noEmit": true,
    "allowImportingTsExtensions": true,
    "verbatimModuleSyntax": true,
    "erasableSyntaxOnly": true,
    "skipLibCheck": true
  },
  "include": ["src", "test"]
}
"#;

const TS_GITIGNORE: &str = "node_modules/\nextension.wasm\nextension.js.map\n*.cyx\n";
const COMMAND_TS: &str = include_str!("../templates/command.ts");
const COMMAND_TEST_TS: &str = include_str!("../templates/command.test.ts");
const VIEW_TS: &str = include_str!("../templates/view.ts");
const VIEW_TEST_TS: &str = include_str!("../templates/view.test.ts");
const PULLS_RS: &str = include_str!("../templates/pulls.rs");
const PULLS_TS: &str = include_str!("../templates/pulls.ts");
const PULLS_TEST_TS: &str = include_str!("../templates/pulls.test.ts");
const PULLS_FIXTURES: &str = include_str!("../templates/pulls.fixtures.json");

/// The pull request provider's README: what to register before the first sign-in.
const PULLS_README: &str = "\n## Before you run it\n\nThe provider speaks the Gitea and Forgejo API on `forge.example.com`. Put your forge's host in `codeyard-extension.toml` (the `[[oauth]]` URLs and `hosts`, and `hosts` of `[[pull-requests]]`), then register an OAuth application there: a public client (no secret: Codeyard signs in with PKCE) with the redirect URI `codeyard://ext/ID/callback`. Put its client id in `client-id`; `cyx check` warns while it is still `your-client-id`, and `cyx publish` refuses it.\n\nUsers with a server of their own add it on the extension's page in Codeyard (Accounts › Add Instance…), with an OAuth application they register there: `instances = true` in `[[oauth]]` allows it.\n\n`cyx test` lists the pull requests of a remote on the first host, `https://forge.example.com/example/repo.git`, and answers its requests from `test/fixtures.json`. Tests stay out of the package.\n";

/// The language of a code extension's template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
}

impl Lang {
    pub fn parse(s: &str) -> Option<Lang> {
        match s {
            "rust" | "rs" => Some(Lang::Rust),
            "ts" | "typescript" => Some(Lang::TypeScript),
            _ => None,
        }
    }
}

/// Where a TypeScript extension gets `@codeyard/extension`: npm, or the SDK folder of a Codeyard checkout. `cyx`
/// builds with the copy it ships either way; npm's is for `npm test` and the editor's types.
fn npm_sdk(sdk: Option<&str>) -> String {
    match sdk {
        Some(root) => format!("file:{}/sdk/typescript", root.trim_end_matches('/')),
        None => NPM_SDK_VERSION.to_string(),
    }
}

/// The `@codeyard/extension` version a new TypeScript extension asks npm for.
pub const NPM_SDK_VERSION: &str = "1.0.0";

/// Where a generated code extension gets the SDK: the Codeyard repository (the SDK is pre-release and not on
/// crates.io yet), or a local checkout of it.
fn sdk_dependency(package: &str, folder: &str, sdk: Option<&str>) -> String {
    match sdk {
        Some(root) => format!("{package} = {{ path = \"{}/crates/{folder}\" }}", root.trim_end_matches('/')),
        None => format!("{}\n{package} = {{ git = \"{SDK_REPOSITORY}\", {} }}", sdk_comment(BUILD), sdk_git_ref(BUILD)),
    }
}

/// This cyx's version: the release whose SDK its templates are made for.
const BUILD: &str = env!("CARGO_PKG_VERSION");

/// The git reference a template takes the SDK at: the tag of `build`'s release, so the extension keeps building
/// against the SDK it was written for. A preview build, before the first release with the SDK (its API's release in
/// `ext_api::RELEASES`), has no such tag yet and follows `main`.
pub fn sdk_git_ref(build: &str) -> String {
    match ext_api::codeyard_for(ext_api::VERSION).filter(|r| ext_api::is_preview_of(build, r)) {
        Some(_) => "branch = \"main\"".into(),
        None => format!("tag = \"v{build}\""),
    }
}

fn sdk_comment(build: &str) -> String {
    match ext_api::codeyard_for(ext_api::VERSION).filter(|r| ext_api::is_preview_of(build, r)) {
        Some(r) => format!("# Pre-release: the SDK from the Codeyard repository's main branch until v{r}, its first release, is tagged."),
        None => format!("# Pre-release: the SDK of Codeyard {build} from its repository until the SDK is on crates.io."),
    }
}

pub const SDK_REPOSITORY: &str = "https://github.com/doancan/codeyard";

/// "my-tools" → "MyTools".
fn type_name(name: &str) -> String {
    title(name).replace(' ', "")
}

/// Write the template for `kind` into a new folder `dir`.
#[cfg(test)]
pub fn create(dir: &Path, kind: Kind, name: &str, publisher: &str) -> Result<(), String> {
    create_with(dir, kind, name, publisher, None)
}

/// Like `create`; a code extension takes the SDK from the Codeyard checkout `sdk` when given.
#[cfg(test)]
pub fn create_with(dir: &Path, kind: Kind, name: &str, publisher: &str, sdk: Option<&str>) -> Result<(), String> {
    create_in(dir, kind, name, publisher, sdk, Lang::Rust)
}

/// Like `create_with`, with the code in `lang`.
pub fn create_in(dir: &Path, kind: Kind, name: &str, publisher: &str, sdk: Option<&str>, lang: Lang) -> Result<(), String> {
    let t = title(name);
    let rust = lang == Lang::Rust;
    let entry = if rust { "Cargo.toml" } else { "src/extension.ts" };
    let code = |rs: &str, ts: &str, test: &str| {
        if !rust {
            return vec![
                ("package.json".to_string(), PACKAGE_JSON.replace("NAME", name).replace("SDK", &npm_sdk(sdk))),
                ("tsconfig.json".to_string(), TSCONFIG.to_string()),
                ("src/extension.ts".to_string(), ts.replace("TITLE", &t)),
                ("test/extension.test.ts".to_string(), test.to_string()),
                (".gitignore".to_string(), TS_GITIGNORE.to_string()),
            ];
        }
        let cargo = CARGO_TOML
            .replace("NAME", name)
            .replace("SDK", &sdk_dependency("codeyard-extension", "ext-sdk", sdk))
            .replace("TEST", &sdk_dependency("codeyard-extension-test", "ext-test", sdk));
        vec![
            ("Cargo.toml".to_string(), cargo),
            ("src/lib.rs".to_string(), rs.replace("TITLE", &t).replace("TYPE", &type_name(name))),
            (".gitignore".to_string(), GITIGNORE.to_string()),
        ]
    };
    let (manifest, files): (String, Vec<(String, String)>) = match kind {
        Kind::Theme => (
            format!("{}\n[[themes]]\nlabel = \"{t}\"\npath  = \"themes/{name}.json\"\n", header(publisher, name, &t, "A colour theme.")),
            vec![(format!("themes/{name}.json"), THEME.replace("NAME", &t))],
        ),
        Kind::Snippets => (
            format!("{}\n[[snippets]]\nlanguage = \"rust\"\npath     = \"snippets/rust.json\"\n", header(publisher, name, &t, "Snippets.")),
            vec![("snippets/rust.json".into(), SNIPPETS.to_string())],
        ),
        Kind::Keymap => (
            format!(
                "{}\n[[keymap-preset]]\nlabel = \"{t}\"\npath  = \"keymap.json\"\n\n# A single key binding for everyone who installs it (never a key a built-in uses):\n[[keybindings]]\nkey     = \"cmd-alt-shift-z\"\ncommand = \"core:workbench.toggleZenMode\"\n",
                header(publisher, name, &t, "A keymap preset.")
            ),
            vec![("keymap.json".into(), KEYMAP.to_string())],
        ),
        Kind::Language => (
            format!(
                "{}\n# Files no built-in language claims. `grammar` borrows a built-in grammar for highlighting.\n[[languages]]\nid           = \"{name}\"\nextensions   = [\"{name}\"]\ngrammar      = \"toml\"\nline-comment = \"#\"\n",
                header(publisher, name, &t, "Language support.")
            ),
            vec![],
        ),
        Kind::LanguageServer => (
            format!(
                "{}\n# A language server is a program on PATH where the files are (also over SSH). It starts in trusted folders only.\n[permissions]\nrun = [\"taplo\"]\nwhy = \"Starts the taplo language server for TOML files.\"\n\n[[language-servers]]\ncommand   = \"taplo\"\nargs      = [\"lsp\", \"stdio\"]\nlanguages = [\"toml\"]\n",
                header(publisher, name, &t, "A language server.")
            ),
            vec![],
        ),
        Kind::Command => (
            format!(
                "{}\n[code]\nentry = \"{entry}\"\n\n[[commands]]\nid    = \"hello\"\ntitle = \"{t}: Say Hello\"\n",
                header(publisher, name, &t, "A command.")
            ),
            code(COMMAND_RS, COMMAND_TS, COMMAND_TEST_TS),
        ),
        Kind::PullRequests => (
            format!(
                "{}\n[code]\nentry = \"{entry}\"\n\n# Codeyard signs every request to the forge with the user's account; the extension never sees the token.\n[permissions]\nauth = [\"forge\"]\nwhy  = \"Lists the pull requests of your repositories on the forge.\"\n\n# The forge's OAuth application (README.md): a public client with the redirect URI codeyard://ext/{publisher}.{name}/callback.\n[[oauth]]\nid            = \"forge\"\nname          = \"Forge\"\nauthorize-url = \"https://forge.example.com/login/oauth/authorize\"\ntoken-url     = \"https://forge.example.com/login/oauth/access_token\"\nclient-id     = \"your-client-id\"\nscopes        = [\"read:repository\"]\nhosts         = [\"forge.example.com\"]\naccount-url   = \"https://forge.example.com/api/v1/user\"\n# Users add servers of their own on the extension's page (Accounts › Add Instance…): these URLs on another host.\ninstances     = true\n\n# The remotes this provider answers for (origin = git@forge.example.com:owner/repo.git), and the account they use.\n[[pull-requests]]\nid    = \"pulls\"\nlabel = \"{t}\"\nhosts = [\"forge.example.com\"]\nauth  = \"forge\"\n",
                header(publisher, name, &t, "Pull requests of a forge in the Pull Requests view.")
            ),
            {
                let mut files = code(PULLS_RS, PULLS_TS, PULLS_TEST_TS);
                files.push(("test/fixtures.json".to_string(), PULLS_FIXTURES.to_string()));
                files
            },
        ),
        Kind::View => (
            format!(
                "{}\n[code]\nentry = \"{entry}\"\n\n[[views]]\nid       = \"main\"\ntitle    = \"{t}\"\nlocation = \"sidebar\"\n\n[[commands]]\nid    = \"reset\"\ntitle = \"{t}: Reset\"\n",
                header(publisher, name, &t, "A view.")
            ),
            code(VIEW_RS, VIEW_TS, VIEW_TEST_TS),
        ),
    };
    let readme = match (kind.is_code(), rust) {
        (false, _) => README.to_string(),
        (true, true) => CODE_README.to_string(),
        (true, false) => TS_README.to_string(),
    };
    let readme = if kind == Kind::PullRequests { readme + &PULLS_README.replace("ID", &format!("{publisher}.{name}")) } else { readme };
    std::fs::create_dir_all(dir).map_err(|e| format!("error: cannot create {}: {e}", dir.display()))?;
    let mut all = vec![
        (ext_manifest::FILE_NAME.to_string(), manifest),
        ("README.md".to_string(), readme.replace("NAME", &t)),
        (".github/workflows/publish.yml".to_string(), PUBLISH_WORKFLOW.to_string()),
    ];
    all.extend(files);
    for (rel, text) in all {
        let path = dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("error: cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("error: cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}
