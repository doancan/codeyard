//! `cyx new`: a working extension of each kind. Every template passes `cyx check` as written (a test makes sure), so
//! the first `cyx dev` shows something.

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
  // VS Code snippet format. $1, ${2:default} are tab stops; $0 is where the cursor ends.
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
/// this file, so it keeps one name. Both actions are pinned to a commit: the job can sign releases, so a moved tag must
/// not change the code it runs. `docs/releasing.md` says when to move the publish action's pin.
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
      - uses: doancan/codeyard/actions/publish@efba4a1e224e81f266bbb7973bdc3ec751b0d49b # main, 2026-10-29
"#;

const README: &str = "# NAME\n\nA Codeyard extension. Run it with `cyx dev`, check it with `cyx check`, publish it with `cyx publish` or by pushing a tag (`git tag v0.1.0 && git push --tags`).\n";

/// Write the template for `kind` into a new folder `dir`.
pub fn create(dir: &Path, kind: Kind, name: &str, publisher: &str) -> Result<(), String> {
    let t = title(name);
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
    };
    std::fs::create_dir_all(dir).map_err(|e| format!("error: cannot create {}: {e}", dir.display()))?;
    let mut all = vec![
        (ext_manifest::FILE_NAME.to_string(), manifest),
        ("README.md".to_string(), README.replace("NAME", &t)),
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
