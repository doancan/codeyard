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
    let secrets_file = n.rsplit_once('.').is_some_and(|(stem, _)| matches!(stem, "secret" | "secrets"));
    n == ".env"
        || n.starts_with(".env.")
        || n.ends_with(".env")
        || ["key", "pem", "p12", "pfx", "jks", "keystore", "ppk", "p8"].iter().any(|x| n.rsplit_once('.').is_some_and(|(_, e)| e == *x))
        || ssh_key
        || secrets_file
        || matches!(n.as_str(), ".npmrc" | ".netrc" | ".pypirc" | ".git-credentials" | "credentials.json")
}

/// What a file's contents give away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leak {
    PrivateKey,
    /// An access token of this kind (`GitHub`, `AWS`, `Slack`).
    Token(&'static str),
}

/// Files whose contents are never scanned: build outputs (they carry strings from dependencies and from the
/// extension's own messages) and documents (they talk about keys).
fn is_unscanned(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(n.as_str(), "extension.wasm" | "extension.js.map")
        || n.ends_with(".md")
        || ["readme", "license", "licence", "copying", "changelog", "notice"].iter().any(|d| n.starts_with(d))
}

/// What the contents of the file called `name` give away, whatever its name says: a private key block or an
/// obvious access token. Build outputs and documents are not looked at.
pub fn leak_in(name: &str, data: &[u8]) -> Option<Leak> {
    if is_unscanned(name) {
        return None;
    }
    let text = String::from_utf8_lossy(data);
    if has_private_key_block(&text) {
        return Some(Leak::PrivateKey);
    }
    // Tokens are looked for in text only: binary data produces matches by chance.
    if std::str::from_utf8(data).is_ok() {
        return token_kind(&text).map(Leak::Token);
    }
    None
}

/// Whether the text holds a whole PEM or PGP private key block (a BEGIN line, a base64 body, an END line) or a
/// PuTTY key file. A line that only mentions the BEGIN line is not one.
fn has_private_key_block(text: &str) -> bool {
    const BEGIN: &str = "-----BEGIN ";
    const DASHES: &str = "-----";
    if text.starts_with("PuTTY-User-Key-File-") && text.contains("Private-Lines:") {
        return true;
    }
    let mut rest = text;
    while let Some(at) = rest.find(BEGIN) {
        rest = &rest[at + BEGIN.len()..];
        let Some(label_end) = rest.find(DASHES) else { break };
        let label = &rest[..label_end];
        if label.contains('\n') || !label.contains("PRIVATE KEY") {
            continue;
        }
        let body = &rest[label_end + DASHES.len()..];
        let Some(end) = body.find(&format!("-----END {label}-----")) else { continue };
        let mut payload = 0;
        let mut plain = true;
        for line in body[..end].lines().map(str::trim).filter(|l| !l.is_empty()) {
            // Encrypted PEM and PGP carry `Key: value` header lines before the body.
            if line.contains(':') {
                continue;
            }
            plain &= line.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_'));
            payload += line.len();
        }
        if plain && payload >= MIN_KEY_BODY {
            return true;
        }
    }
    false
}

/// A real key body is hundreds of characters; this only separates one from the placeholders in prose and tests.
const MIN_KEY_BODY: usize = 32;

/// The kind of the first access token in `text`: GitHub (`ghp_`, `github_pat_`), AWS (`AKIA`) or Slack (`xoxb-`).
fn token_kind(text: &str) -> Option<&'static str> {
    let tokens = [("GitHub", "ghp_", 36, 36), ("GitHub", "github_pat_", 22, usize::MAX), ("AWS", "AKIA", 16, 16), ("Slack", "xoxb-", 20, usize::MAX)];
    for (kind, prefix, min, max) in tokens {
        for (at, _) in text.match_indices(prefix) {
            // Part of a longer word (`MAKIA...`) is not a token.
            let before = text[..at].chars().next_back();
            if before.is_some_and(|c| c.is_ascii_alphanumeric()) {
                continue;
            }
            let body = text[at + prefix.len()..]
                .bytes()
                .take_while(|b| match prefix {
                    "AKIA" => b.is_ascii_uppercase() || b.is_ascii_digit(),
                    "xoxb-" => b.is_ascii_alphanumeric() || *b == b'-',
                    _ => b.is_ascii_alphanumeric() || *b == b'_',
                })
                .count();
            if body >= min && body <= max {
                return Some(kind);
            }
        }
    }
    None
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
/// (`docs/internal`, `/notes.md`). A trailing `/` matches folders only. The file only lists what to leave out: a
/// `!pattern` line is an error, not a quiet no-op.
#[derive(Clone, Debug, Default)]
pub struct Ignore {
    patterns: Vec<Pattern>,
}

#[derive(Clone, Debug)]
struct Pattern {
    glob: String,
    anchored: bool,
    dir_only: bool,
    /// 1-based line in the file, and the line as written, for messages.
    line: usize,
    text: String,
}

/// The `.cyxignore` line that leaves a path out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IgnoreHit {
    pub line: usize,
    pub pattern: String,
}

impl Ignore {
    pub fn parse(text: &str) -> Result<Ignore, String> {
        let mut patterns = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let l = raw.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            if l.starts_with('!') {
                return Err(format!(
                    "{IGNORE_FILE} line {}: '{l}' is a negation, which is not supported: the file only lists what to leave out\n   = fix: remove the line, or narrow the pattern that leaves the file out",
                    i + 1
                ));
            }
            let dir_only = l.ends_with('/');
            let trimmed = l.trim_end_matches('/');
            let glob = trimmed.trim_start_matches('/').to_string();
            if glob.is_empty() {
                continue;
            }
            patterns.push(Pattern { glob, anchored: trimmed.contains('/'), dir_only, line: i + 1, text: l.to_string() });
        }
        Ok(Ignore { patterns })
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    fn matches(&self, rel: &str, is_dir: bool) -> bool {
        self.hit(rel, is_dir).is_some()
    }

    fn hit(&self, rel: &str, is_dir: bool) -> Option<&Pattern> {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        self.patterns.iter().find(|p| (is_dir || !p.dir_only) && glob(&p.glob, if p.anchored { rel } else { name }))
    }

    /// The line that leaves the file `rel` out of a package: one that names it, or a folder above it.
    pub fn explain(&self, rel: &str) -> Option<IgnoreHit> {
        let mut end = 0;
        for part in rel.split('/') {
            end += part.len();
            let is_dir = end < rel.len();
            if let Some(p) = self.hit(&rel[..end], is_dir) {
                return Some(IgnoreHit { line: p.line, pattern: p.text.clone() });
            }
            end += 1;
        }
        None
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
    }

    const KEY_BODY: &str = "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZWQyNTUxOQ";

    #[test]
    fn more_secret_names_are_caught() {
        for name in ["putty.ppk", "AuthKey.p8", "secrets.json", "secret.txt", "Secrets.YAML", "secret.json"] {
            assert_eq!(keep(name, false), Rule::Secret, "{name}");
        }
        for name in ["secretary.txt", "secret", "my-secrets-list.md", "secrets"] {
            assert_ne!(keep(name, false), Rule::Secret, "{name}");
        }
    }

    #[test]
    fn a_whole_private_key_block_is_found_whatever_the_name() {
        let pem = format!("x\n-----BEGIN OPENSSH PRIVATE KEY-----\n{KEY_BODY}\n-----END OPENSSH PRIVATE KEY-----\n");
        let pgp = format!("-----BEGIN PGP PRIVATE KEY BLOCK-----\nVersion: GnuPG\n\n{KEY_BODY}\n=abcd\n-----END PGP PRIVATE KEY BLOCK-----");
        let ppk = "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none\nPrivate-Lines: 1\nPrivate-MAC: 00";
        for (what, data) in [("pem", pem.as_str()), ("pgp", pgp.as_str()), ("ppk", ppk)] {
            assert_eq!(leak_in("notes.txt", data.as_bytes()), Some(Leak::PrivateKey), "{what}");
        }
    }

    #[test]
    fn talk_about_keys_is_not_a_key() {
        let begin = "-----BEGIN RSA PRIVATE KEY-----";
        for text in [
            format!("starts with {begin}"),
            format!("{begin} then prose, no end"),
            format!("{begin}\nnot a body at all, just words\n-----END RSA PRIVATE KEY-----"),
            format!("{begin}\nabc\n-----END RSA PRIVATE KEY-----"),
            "-----BEGIN CERTIFICATE-----".to_string(),
        ] {
            assert_eq!(leak_in("main.js", text.as_bytes()), None, "{text}");
        }
        // Build outputs and documents are never looked at, even with a whole block in them.
        let whole = format!("{begin}\n{KEY_BODY}\n-----END RSA PRIVATE KEY-----");
        for name in ["extension.wasm", "extension.js.map", "README.md", "LICENSE", "CHANGELOG", "guide.MD"] {
            assert_eq!(leak_in(name, whole.as_bytes()), None, "{name}");
        }
    }

    #[test]
    fn obvious_tokens_are_found_in_text_only() {
        // Built here so no whole token sits in the source for secret scanners to flag.
        let slack = format!("SLACK={}-1234567890-abcdefghijklmnop", ["xox", "b"].concat());
        let ghp = format!("token = \"ghp_{}\"", "a1B2".repeat(9));
        for (text, kind) in [
            (ghp.as_str(), "GitHub"),
            ("github_pat_11ABCDEFG0abcdefghijkl_mnopqrstuvwxyz", "GitHub"),
            ("key=AKIAIOSFODNN7EXAMPLE", "AWS"),
            (slack.as_str(), "Slack"),
        ] {
            assert_eq!(leak_in("config.txt", text.as_bytes()), Some(Leak::Token(kind)), "{text}");
        }
        for text in ["ghp_short", "MAKIAIOSFODNN7EXAMPLE", "AKIAlowercaseisnotakey12", "github_pat_x"] {
            assert_eq!(leak_in("config.txt", text.as_bytes()), None, "{text}");
        }
        let mut binary = vec![0xff, 0xfe];
        binary.extend_from_slice(b"AKIAIOSFODNN7EXAMPLE");
        assert_eq!(leak_in("blob.bin", &binary), None);
        assert_eq!(leak_in("README.md", b"AKIAIOSFODNN7EXAMPLE"), None);
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
        let ignore = Ignore::parse("# drafts\n*.psd\ndrafts/\n/notes.md\ndocs/internal\n\n").unwrap();
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
    fn a_negation_is_refused_and_a_hit_names_its_line() {
        let e = Ignore::parse("*.psd\n  !keep.psd\n").unwrap_err();
        assert!(e.contains(".cyxignore line 2") && e.contains("'!keep.psd'") && e.contains("negation"), "{e}");
        let ignore = Ignore::parse("# c\n\ndrafts/\n*.psd\n").unwrap();
        assert_eq!(ignore.explain("drafts/a/b.json"), Some(IgnoreHit { line: 3, pattern: "drafts/".into() }));
        assert_eq!(ignore.explain("art/x.psd"), Some(IgnoreHit { line: 4, pattern: "*.psd".into() }));
        assert_eq!(ignore.explain("themes/a.json"), None);
        assert_eq!(ignore.explain("drafts"), None, "a trailing / matches folders only, and `drafts` here is a file");
    }

    #[test]
    fn a_left_out_path_says_why() {
        assert!(left_out_because("src/theme.json").is_some_and(|w| w.contains("src/")));
        assert!(left_out_because(".themes/a.json").is_some());
        assert!(left_out_because("signing.key").is_some_and(|w| w.contains("secret")));
        assert_eq!(left_out_because("themes/a.json"), None);
    }
}
