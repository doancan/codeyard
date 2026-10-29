//! Permissions and the one function that judges them. `risk` decides whether `why` is required, whether the consent
//! sheet shows a red banner, whether the registry asks for a human review and what `cyx publish --dry-run` prints.
//! Authors never see the levels; they see the sheet.

use serde::{Deserialize, Serialize};

/// Programs that run whatever script or command line they are given. Allowed in `run` only when the script ships inside
/// the package (checked by `ext-pkg`), so a language server wrapper is a normal high-risk extension, not a way around
/// review. Matched on the file name without `.exe`/`.cmd`/`.bat`/`.com`, ignoring case.
pub const INTERPRETERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "csh",
    "tcsh",
    "nu",
    "elvish",
    "xonsh",
    "busybox",
    "cmd",
    "powershell",
    "pwsh",
    "wsl",
    "env",
    "xargs",
    "nohup",
    "sudo",
    "doas",
    "npx",
    "pnpx",
    "bunx",
    "deno",
    "bun",
    "py",
    "rscript",
    "osascript",
    "wscript",
    "cscript",
    "mshta",
    "rundll32",
    "regsvr32",
    "tclsh",
    "wish",
    "awk",
    "gawk",
    "expect",
];
/// Interpreter names that also come with a version suffix: `python3.12`, `node22`, `pypy3`, `pythonw`.
pub const INTERPRETER_FAMILIES: &[&str] = &["python", "pypy", "node", "ruby", "perl", "php", "lua", "luajit", "julia", "java", "dotnet"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Clipboard {
    #[default]
    None,
    Read,
    Write,
    ReadWrite,
}

impl Clipboard {
    pub fn reads(self) -> bool {
        matches!(self, Clipboard::Read | Clipboard::ReadWrite)
    }
    pub fn writes(self) -> bool {
        matches!(self, Clipboard::Write | Clipboard::ReadWrite)
    }
}

/// What the manifest declares in `[permissions]`, validated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    /// Workspace globs.
    pub read: Vec<String>,
    pub write: Vec<String>,
    /// Hosts, optionally `*.example.com`.
    pub net: Vec<String>,
    /// Account providers (`github`, or an OAuth provider declared by the extension).
    pub auth: Vec<String>,
    pub secrets: bool,
    /// Programs, by name or as a path inside the package (`bin/server`).
    pub run: Vec<String>,
    pub clipboard: Clipboard,
    pub terminal: bool,
    /// Public core commands (`core:…`).
    pub commands: Vec<String>,
    /// The author's one-sentence reason, quoted on the consent sheet.
    pub why: Option<String>,
}

impl Permissions {
    pub fn is_empty(&self) -> bool {
        self.read.is_empty()
            && self.write.is_empty()
            && self.net.is_empty()
            && self.auth.is_empty()
            && !self.secrets
            && self.run.is_empty()
            && self.clipboard == Clipboard::None
            && !self.terminal
            && self.commands.is_empty()
    }

    /// Interpreters in `run`; each needs its script inside the package.
    pub fn interpreters(&self) -> impl Iterator<Item = &str> {
        self.run.iter().map(String::as_str).filter(|p| is_interpreter(p))
    }

    /// One line per permission, in the words of the consent sheet: what the user is asked about, in order. The app
    /// translates each kind; `describe` gives the English, for `cyx` and the registry.
    pub fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        let list = |v: &[String]| v.join(", ");
        if !self.read.is_empty() {
            out.push(if self.read.iter().any(|g| is_wide_glob(g)) { Line::ReadAll } else { Line::Read(list(&self.read)) });
            if self.read.iter().any(|g| is_sensitive_glob(g)) {
                out.push(Line::ReadSecrets);
            }
        }
        if !self.write.is_empty() {
            out.push(if self.write.iter().any(|g| is_wide_glob(g)) { Line::ChangeAll } else { Line::Change(list(&self.write)) });
        }
        if !self.net.is_empty() {
            out.push(if self.net.iter().any(|h| is_wide_host(h)) { Line::ConnectAny } else { Line::Connect(list(&self.net)) });
        }
        out.extend(self.auth.iter().map(|a| Line::Account(provider_name(a))));
        if self.secrets {
            out.push(Line::Secrets);
        }
        out.extend(self.run.iter().map(|p| Line::Run(p.clone())));
        if self.clipboard != Clipboard::None {
            out.push(Line::Clipboard(self.clipboard));
        }
        if self.terminal {
            out.push(Line::Terminal);
        }
        if !self.commands.is_empty() {
            out.push(Line::Commands(list(&self.commands)));
        }
        out
    }

    /// `lines` in English.
    pub fn describe(&self) -> Vec<String> {
        self.lines().iter().map(Line::english).collect()
    }

    /// What `self` asks for that `old` did not: the update card shows only this, and an empty result updates without
    /// asking. Entries compare as written, so a narrower glob or host than before still counts as new. `why` is not
    /// a permission.
    pub fn added_since(&self, old: &Permissions) -> Permissions {
        let new = |mine: &[String], theirs: &[String]| mine.iter().filter(|x| !theirs.contains(x)).cloned().collect::<Vec<_>>();
        let clipboard = match (self.clipboard.reads() && !old.clipboard.reads(), self.clipboard.writes() && !old.clipboard.writes()) {
            (true, true) => Clipboard::ReadWrite,
            (true, false) => Clipboard::Read,
            (false, true) => Clipboard::Write,
            (false, false) => Clipboard::None,
        };
        Permissions {
            read: new(&self.read, &old.read),
            write: new(&self.write, &old.write),
            net: new(&self.net, &old.net),
            auth: new(&self.auth, &old.auth),
            secrets: self.secrets && !old.secrets,
            run: new(&self.run, &old.run),
            clipboard,
            terminal: self.terminal && !old.terminal,
            commands: new(&self.commands, &old.commands),
            why: None,
        }
    }
}

/// One line of the consent sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    ReadAll,
    /// The globs, comma-separated.
    Read(String),
    /// Added after a `read` line that can match credential files.
    ReadSecrets,
    ChangeAll,
    Change(String),
    ConnectAny,
    Connect(String),
    /// The account provider's display name.
    Account(String),
    Secrets,
    Run(String),
    Clipboard(Clipboard),
    Terminal,
    Commands(String),
}

impl Line {
    pub fn english(&self) -> String {
        match self {
            Line::ReadAll => "Read all files in the workspace".into(),
            Line::Read(g) => format!("Read {g}"),
            Line::ReadSecrets => "Read files that can hold passwords and keys".into(),
            Line::ChangeAll => "Change all files in the workspace".into(),
            Line::Change(g) => format!("Change {g}"),
            Line::ConnectAny => "Connect to any website".into(),
            Line::Connect(h) => format!("Connect to {h}"),
            Line::Account(a) => format!("Act as your {a} account"),
            Line::Secrets => "Store secrets in your keychain".into(),
            Line::Run(p) => format!("Run {p} with any arguments"),
            Line::Clipboard(Clipboard::Read) => "Read the clipboard".into(),
            Line::Clipboard(Clipboard::Write) => "Write to the clipboard".into(),
            Line::Clipboard(_) => "Read and write the clipboard".into(),
            Line::Terminal => "Type into terminals".into(),
            Line::Commands(c) => format!("Run Codeyard commands: {c}"),
        }
    }
}

fn provider_name(id: &str) -> String {
    match id {
        "github" => "GitHub".into(),
        "gitlab" => "GitLab".into(),
        "vercel" => "Vercel".into(),
        other => other.to_string(),
    }
}

pub fn is_interpreter(program: &str) -> bool {
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program).to_ascii_lowercase();
    let base = [".exe", ".cmd", ".bat", ".com"].iter().find_map(|x| base.strip_suffix(x)).unwrap_or(&base);
    INTERPRETERS.contains(&base)
        || INTERPRETER_FAMILIES.iter().any(|f| base.strip_prefix(f).is_some_and(|v| v.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | 'w'))))
}

fn segments(glob: &str) -> Vec<&str> {
    glob.split('/').filter(|s| !s.is_empty() && *s != ".").collect()
}

fn has_wildcard(s: &str) -> bool {
    s.contains(['*', '?', '[', '{'])
}

/// A glob that reaches more than named files in one folder: anything with `**`, a wildcard in a folder part, or a
/// wildcard file part other than `name*.ext`. `src/*.rs` and `.github/workflows/*.yml` are narrow; `src/**`, `*/x`,
/// `**/*.rs`, `?*` and `*` are wide.
pub fn is_wide_glob(glob: &str) -> bool {
    let parts = segments(glob);
    let Some((last, dirs)) = parts.split_last() else { return true };
    if parts.iter().any(|p| p.contains("**")) || dirs.iter().any(|d| has_wildcard(d)) {
        return true;
    }
    if !has_wildcard(last) {
        return false;
    }
    if last.contains(['?', '[', '{']) || last.matches('*').count() != 1 {
        return true;
    }
    let after = last.split_once('*').map_or("", |(_, a)| a);
    !(after.len() > 1 && after.starts_with('.'))
}

/// Folders and files that hold credentials, or that run code when a tool opens them. Reading them counts as reading
/// private data; writing them counts as running code. Matched on any path part, ignoring case.
const SENSITIVE_DIRS: &[&str] =
    &[".git", ".ssh", ".gnupg", ".aws", ".azure", ".kube", ".docker", ".vscode", ".idea", ".cargo", ".husky", ".github", ".config", ".devcontainer"];
const SENSITIVE_FILES: &[&str] = &[
    ".envrc",
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    ".pypirc",
    ".netrc",
    ".git-credentials",
    ".gitconfig",
    ".gitattributes",
    ".pgpass",
    "credentials",
    "id_rsa",
    "id_ecdsa",
    "id_ed25519",
    "makefile",
    "justfile",
    "build.rs",
    "cargo.toml",
    "package.json",
    "setup.py",
    "pyproject.toml",
    "gradlew",
    "build.gradle",
    "pom.xml",
    "dockerfile",
    "docker-compose.yml",
    "flake.nix",
    ".pre-commit-config.yaml",
    ".bashrc",
    ".zshrc",
    ".profile",
];
const SENSITIVE_EXTENSIONS: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".keystore", ".jks", ".kdbx", ".ovpn", ".tfstate", ".tfvars"];

/// A glob that names or can match a credential file, or a file that runs code when a tool opens it.
pub fn is_sensitive_glob(glob: &str) -> bool {
    let g = glob.to_ascii_lowercase();
    segments(&g).iter().any(|p| {
        SENSITIVE_DIRS.contains(p)
            || SENSITIVE_FILES.contains(p)
            || p.starts_with(".env")
            || SENSITIVE_EXTENSIONS.iter().any(|x| p.ends_with(x))
            || (p.starts_with('.') && has_wildcard(p))
    })
}

/// `*.com` and friends: a wildcard directly under a public suffix is any host. Without a public-suffix list, a wildcard
/// with fewer than two labels after it, or over a known shared suffix, counts as wide.
pub fn is_wide_host(host: &str) -> bool {
    let Some(rest) = host.strip_prefix("*.") else { return host == "*" };
    rest.split('.').count() < 2 || SHARED_SUFFIXES.contains(&rest)
}

/// Suffixes under which anyone can get a host name. A full public-suffix list would be exact; this list covers the common
/// ones, and only the sheet's wording depends on it (any `net` already counts as sending).
const SHARED_SUFFIXES: &[&str] = &[
    "co.uk",
    "org.uk",
    "com.tr",
    "com.au",
    "co.jp",
    "co.in",
    "com.br",
    "com.cn",
    "co.nz",
    "co.za",
    "com.mx",
    "github.io",
    "githubusercontent.com",
    "gitlab.io",
    "vercel.app",
    "netlify.app",
    "pages.dev",
    "workers.dev",
    "herokuapp.com",
    "azurewebsites.net",
    "cloudfront.net",
    "appspot.com",
    "web.app",
    "firebaseapp.com",
    "s3.amazonaws.com",
    "amazonaws.com",
    "blogspot.com",
    "ngrok.io",
    "ngrok-free.app",
    "onrender.com",
    "fly.dev",
    "railway.app",
    "glitch.me",
    "repl.co",
    "trycloudflare.com",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Nothing that reads private data or sends anything.
    Low,
    /// Reads named files, or sends, but not both.
    Medium,
    /// Can read private data and send it somewhere, or can run code as the user.
    High,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Risk {
    pub level: Level,
    /// The permissions that read private data (wide `read`, clipboard, secrets, accounts).
    pub reads: Vec<&'static str>,
    /// The permissions that send data out (`net`, `run`, `write`, terminal).
    pub sends: Vec<&'static str>,
    /// The extension can run code as the user: `run`, `terminal`, or writing files that other tools run.
    pub runs_code: bool,
}

impl Risk {
    /// `why` is required for high-risk permission sets.
    pub fn needs_why(&self) -> bool {
        self.level == Level::High
    }

    /// The red line of the consent sheet for a High-risk set, in English (the app translates the same words).
    pub fn headline(&self) -> Option<&'static str> {
        match (self.level, self.runs_code) {
            (Level::High, true) => Some("High risk: this extension can run programs as you."),
            (Level::High, false) => Some("High risk: this extension can read private data and send it elsewhere."),
            _ => None,
        }
    }
}

/// The one judgement of a permission set. Reading private data together with any way to send it is High; anything that
/// can run code as the user is High.
pub fn risk(p: &Permissions) -> Risk {
    let mut reads = Vec::new();
    if p.read.iter().any(|g| is_wide_glob(g) || is_sensitive_glob(g)) {
        reads.push("read");
    }
    if p.clipboard.reads() {
        reads.push("clipboard");
    }
    if p.secrets {
        reads.push("secrets");
    }
    if !p.auth.is_empty() {
        reads.push("auth");
    }
    let mut sends = Vec::new();
    if !p.net.is_empty() {
        sends.push("net");
    }
    if !p.run.is_empty() {
        sends.push("run");
    }
    if !p.write.is_empty() {
        sends.push("write");
    }
    if p.terminal {
        sends.push("terminal");
    }
    // Typing into a terminal runs a command; writing build files, hooks or editor tasks runs code at the next build,
    // commit or open.
    let runs_code = !p.run.is_empty() || p.terminal || p.write.iter().any(|g| is_wide_glob(g) || is_sensitive_glob(g));
    let level = if runs_code || (!reads.is_empty() && !sends.is_empty()) {
        Level::High
    } else if !reads.is_empty() || !sends.is_empty() || !p.read.is_empty() || p.clipboard.writes() || !p.commands.is_empty() {
        Level::Medium
    } else {
        Level::Low
    };
    Risk { level, reads, sends, runs_code }
}
