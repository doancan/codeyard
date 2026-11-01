//! Which files of an extension's folder go into its package (docs/extensions/developer-guide.md, "What goes into a
//! package"). A package holds what Codeyard reads at run time: the manifest, `extension.wasm` and its source map, the
//! files the manifest names, README, LICENSE, CHANGELOG and any other data the extension reads from its package.
//! Secrets, version control, dependencies, build input and tests stay out, and `.cyxignore` leaves out more.

/// Folder and file names left out wherever they are: build output, dependencies, OS litter. Names starting with `.`
/// (version control, editor settings, `.env`) are left out too.
pub const EXCLUDED: &[&str] = &[".git", ".github", ".hg", ".svn", "target", "node_modules", ".DS_Store", "Thumbs.db", "desktop.ini"];

/// Folders left out at the top of an extension only: its tests (`test/` in TypeScript, `tests/` in Rust), which run
/// on the author's machine and in `cyx test`, never in Codeyard, and the code's source (`src/`), which is compiled
/// into `extension.wasm`.
pub const EXCLUDED_AT_ROOT: &[&str] = &["test", "tests", "src"];

/// Build input left out wherever it is: the code reaches Codeyard as `extension.wasm`. `package.json` and its lock
/// file would also publish the author's local paths (`file:/Users/…`).
pub const BUILD_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "rust-toolchain",
    "rust-toolchain.toml",
    "package.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "tsconfig.json",
];

/// Source file extensions left out wherever they are (Rust and TypeScript; `.d.ts` ends in `.ts`).
pub const SOURCE_EXTENSIONS: &[&str] = &["rs", "ts", "tsx", "mts", "cts"];

/// The file in an extension's folder that lists more paths to leave out.
pub const IGNORE_FILE: &str = ".cyxignore";

/// What happens to one file or folder of an extension's folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    Keep,
    /// Left out without a word: litter, dependencies, build input, tests, `.cyxignore` entries.
    Skip,
    /// Left out, and the author is told: its name says it holds a secret (`.env`, `*.key`, `id_ed25519`).
    Secret,
}

/// Whether a file name says it holds a secret: `.env` files, private keys and certificates with their keys, SSH
/// keys and the files package managers keep tokens in. A file that only looks like one can be renamed.
pub fn is_secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let ssh_key = n.starts_with("id_") && !n.contains('.');
    n == ".env"
        || n.starts_with(".env.")
        || n.ends_with(".env")
        || ["key", "pem", "p12", "pfx", "jks", "keystore"].iter().any(|x| n.rsplit_once('.').is_some_and(|(_, e)| e == *x))
        || ssh_key
        || matches!(n.as_str(), ".npmrc" | ".netrc" | ".pypirc" | ".git-credentials" | "credentials.json")
}

/// The marker every PEM private key carries (`-----BEGIN RSA PRIVATE KEY-----`, `… OPENSSH PRIVATE KEY-----`).
const PRIVATE_KEY_MARKER: &[u8] = b"PRIVATE KEY-----";

/// Whether a file's contents hold a PEM private key, whatever its name.
pub fn holds_private_key(data: &[u8]) -> bool {
    data.windows(PRIVATE_KEY_MARKER.len()).any(|w| w == PRIVATE_KEY_MARKER)
}

/// The rule for `rel` (a path under the folder, with `/`), a folder when `is_dir`. `top` is false inside
/// `native/<triple>/`, whose files are programs: the top-of-folder rules do not apply there.
pub fn rule(rel: &str, is_dir: bool, top: bool, ignore: &Ignore) -> Rule {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let at_root = !rel.contains('/');
    if !is_dir && is_secret_name(name) {
        return Rule::Secret;
    }
    let skip = name.starts_with('.')
        || EXCLUDED.contains(&name)
        || (top && at_root && is_dir && EXCLUDED_AT_ROOT.contains(&name))
        || (!is_dir && (BUILD_FILES.contains(&name) || has_extension(name, SOURCE_EXTENSIONS) || has_extension(name, &["cyx"])))
        || ignore.matches(rel, is_dir);
    if skip { Rule::Skip } else { Rule::Keep }
}

fn has_extension(name: &str, extensions: &[&str]) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, x)| !stem.is_empty() && extensions.iter().any(|e| x.eq_ignore_ascii_case(e)))
}

/// Why a path the manifest names is not in the package, when a rule left it out.
pub fn left_out_because(rel: &str) -> Option<&'static str> {
    let parts: Vec<&str> = rel.split('/').collect();
    let name = parts.last().copied().unwrap_or(rel);
    if is_secret_name(name) {
        return Some("its name says it holds a secret, and packages never hold secrets");
    }
    if parts.iter().any(|p| p.starts_with('.') || EXCLUDED.contains(p)) {
        return Some("hidden files, version control, dependencies and build output are never packaged");
    }
    if parts.len() > 1 && EXCLUDED_AT_ROOT.contains(&parts[0]) {
        return Some("src/, test/ and tests/ at the top of the extension are never packaged");
    }
    if BUILD_FILES.contains(&name) || has_extension(name, SOURCE_EXTENSIONS) {
        return Some("source code and build files are never packaged");
    }
    None
}

/// The patterns of a `.cyxignore`: one per line, `#` starts a comment. `*` matches any characters but `/`. A pattern
/// without a `/` matches a name at any depth (`*.psd`, `drafts`); one with a `/` matches the path from the top
/// (`docs/internal`, `/notes.md`). A trailing `/` matches folders only.
#[derive(Clone, Debug, Default)]
pub struct Ignore {
    patterns: Vec<Pattern>,
}

#[derive(Clone, Debug)]
struct Pattern {
    glob: String,
    anchored: bool,
    dir_only: bool,
}

impl Ignore {
    pub fn parse(text: &str) -> Ignore {
        let patterns = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let dir_only = l.ends_with('/');
                let l = l.trim_end_matches('/');
                let anchored = l.contains('/');
                Pattern { glob: l.trim_start_matches('/').to_string(), anchored, dir_only }
            })
            .filter(|p| !p.glob.is_empty())
            .collect();
        Ignore { patterns }
    }

    fn matches(&self, rel: &str, is_dir: bool) -> bool {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        self.patterns.iter().any(|p| (is_dir || !p.dir_only) && glob(&p.glob, if p.anchored { rel } else { name }))
    }
}

/// `*` matches any run of characters except `/`; everything else matches itself.
fn glob(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, rest)) => {
            let Some(text) = text.strip_prefix(head) else { return false };
            (0..=text.len()).take_while(|&i| !text[..i].contains('/')).filter(|&i| text.is_char_boundary(i)).any(|i| glob(rest, &text[i..]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keep(rel: &str, is_dir: bool) -> Rule {
        rule(rel, is_dir, true, &Ignore::default())
    }

    #[test]
    fn secrets_are_told_apart_from_other_files() {
        for name in [".env", ".env.local", "prod.env", "signing.key", "server.pem", "cert.p12", "id_ed25519", "id_rsa", ".npmrc", "credentials.json"] {
            assert_eq!(keep(name, false), Rule::Secret, "{name}");
            assert_eq!(keep(&format!("deep/{name}"), false), Rule::Secret, "{name} in a folder");
        }
        for name in ["id_ed25519.pub", "id_map.json", "keymap.json", "monkey.json", "README.md", "environment.json"] {
            assert_ne!(keep(name, false), Rule::Secret, "{name}");
        }
        assert!(holds_private_key(b"x\n-----BEGIN OPENSSH PRIVATE KEY-----\nabc"));
        assert!(!holds_private_key(b"-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn build_input_tests_and_hidden_files_stay_out() {
        for (rel, dir) in [
            ("src", true),
            ("test", true),
            ("tests", true),
            ("Cargo.lock", false),
            ("Cargo.toml", false),
            ("package.json", false),
            ("package-lock.json", false),
            ("tsconfig.json", false),
            ("lib/helper.ts", false),
            ("types.d.ts", false),
            ("main.rs", false),
            (".gitignore", false),
            (".vscode", true),
            ("node_modules", true),
            ("target", true),
            ("old.cyx", false),
        ] {
            assert_eq!(keep(rel, dir), Rule::Skip, "{rel}");
        }
        for (rel, dir) in [
            ("README.md", false),
            ("LICENSE", false),
            ("CHANGELOG.md", false),
            ("extension.wasm", false),
            ("extension.js.map", false),
            ("themes", true),
            ("themes/src", true),
            ("themes/test", true),
            ("icon.png", false),
        ] {
            assert_eq!(keep(rel, dir), Rule::Keep, "{rel}");
        }
        // Inside `native/<triple>/` a folder named `src` holds programs.
        assert_eq!(rule("src", true, false, &Ignore::default()), Rule::Keep);
    }

    #[test]
    fn cyxignore_leaves_out_what_it_names() {
        let ignore = Ignore::parse("# drafts\n*.psd\ndrafts/\n/notes.md\ndocs/internal\n\n");
        let r = |rel: &str, dir: bool| rule(rel, dir, true, &ignore);
        assert_eq!(r("art/logo.psd", false), Rule::Skip);
        assert_eq!(r("drafts", true), Rule::Skip);
        assert_eq!(r("themes/drafts", true), Rule::Skip);
        assert_eq!(r("drafts", false), Rule::Keep, "a trailing / matches folders only");
        assert_eq!(r("notes.md", false), Rule::Skip);
        assert_eq!(r("themes/notes.md", false), Rule::Keep, "a pattern with / is anchored");
        assert_eq!(r("docs/internal", true), Rule::Skip);
        assert_eq!(r("docs/guide.md", false), Rule::Keep);
        assert!(glob("a*c", "abbc") && glob("*", "x") && !glob("*", "a/b") && !glob("a*", "b"));
    }

    #[test]
    fn a_left_out_path_says_why() {
        assert!(left_out_because("src/theme.json").is_some_and(|w| w.contains("src/")));
        assert!(left_out_because(".themes/a.json").is_some());
        assert!(left_out_because("signing.key").is_some_and(|w| w.contains("secret")));
        assert_eq!(left_out_because("themes/a.json"), None);
    }
}
