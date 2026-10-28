//! Permissions and the one function that judges them. `risk` decides whether `why` is required, whether the consent
//! sheet shows a red banner, whether the registry asks for a human review and what `cyx publish --dry-run` prints.
//! Authors never see the levels; they see the sheet.

/// Programs that run whatever script they are given. Allowed in `run` only when the script ships inside the package
/// (checked by `ext-pkg`), so a language server wrapper is a normal high-risk extension, not a way around review.
pub const INTERPRETERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "csh",
    "tcsh",
    "cmd",
    "cmd.exe",
    "powershell",
    "powershell.exe",
    "pwsh",
    "pwsh.exe",
    "node",
    "deno",
    "bun",
    "python",
    "python3",
    "ruby",
    "perl",
    "php",
    "lua",
    "osascript",
    "env",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

    /// One line per permission, in the words of the consent sheet.
    pub fn describe(&self) -> Vec<String> {
        let mut out = Vec::new();
        let list = |v: &[String]| v.join(", ");
        if !self.read.is_empty() {
            out.push(if self.read.iter().any(|g| is_wide_glob(g)) { "Read all files in the workspace".into() } else { format!("Read {}", list(&self.read)) });
        }
        if !self.write.is_empty() {
            out.push(if self.write.iter().any(|g| is_wide_glob(g)) {
                "Change all files in the workspace".into()
            } else {
                format!("Change {}", list(&self.write))
            });
        }
        if !self.net.is_empty() {
            out.push(if self.net.iter().any(|h| is_wide_host(h)) { "Connect to any website".into() } else { format!("Connect to {}", list(&self.net)) });
        }
        for a in &self.auth {
            out.push(format!("Act as your {} account", provider_name(a)));
        }
        if self.secrets {
            out.push("Store secrets in your keychain".into());
        }
        for p in &self.run {
            out.push(format!("Run {p} with any arguments"));
        }
        match self.clipboard {
            Clipboard::None => {}
            Clipboard::Read => out.push("Read the clipboard".into()),
            Clipboard::Write => out.push("Write to the clipboard".into()),
            Clipboard::ReadWrite => out.push("Read and write the clipboard".into()),
        }
        if self.terminal {
            out.push("Type into terminals".into());
        }
        if !self.commands.is_empty() {
            out.push(format!("Run Codeyard commands: {}", list(&self.commands)));
        }
        out
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
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program);
    INTERPRETERS.iter().any(|i| i.eq_ignore_ascii_case(base)) || base.starts_with("python3.")
}

/// A glob that matches the whole workspace (or most of it) rather than named files.
pub fn is_wide_glob(glob: &str) -> bool {
    let g = glob.trim_start_matches("./");
    matches!(g, "*" | "**" | "**/*" | "**/**") || g.starts_with("**/*.") || (g.starts_with("**/") && g.ends_with("/**"))
}

/// `*.com` and friends: a wildcard directly under a public suffix is any host. Without a public-suffix list, a wildcard
/// with fewer than two labels after it, or over a known shared suffix, counts as wide.
pub fn is_wide_host(host: &str) -> bool {
    let Some(rest) = host.strip_prefix("*.") else { return host == "*" };
    rest.split('.').count() < 2 || SHARED_SUFFIXES.contains(&rest)
}

const SHARED_SUFFIXES: &[&str] = &[
    "co.uk",
    "com.tr",
    "com.au",
    "co.jp",
    "github.io",
    "vercel.app",
    "netlify.app",
    "pages.dev",
    "herokuapp.com",
    "azurewebsites.net",
    "cloudfront.net",
    "appspot.com",
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
    /// `run` is declared: the extension can run code as the user.
    pub runs_code: bool,
}

impl Risk {
    /// `why` is required for high-risk permission sets.
    pub fn needs_why(&self) -> bool {
        self.level == Level::High
    }
}

/// The one judgement of a permission set. Reading private data together with any way to send it is High; any `run` is
/// High ("can run code as you").
pub fn risk(p: &Permissions) -> Risk {
    let mut reads = Vec::new();
    if p.read.iter().any(|g| is_wide_glob(g)) {
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
    let runs_code = !p.run.is_empty();
    let level = if runs_code || (!reads.is_empty() && !sends.is_empty()) {
        Level::High
    } else if !reads.is_empty() || !sends.is_empty() || !p.read.is_empty() || p.clipboard.writes() || !p.commands.is_empty() {
        Level::Medium
    } else {
        Level::Low
    };
    Risk { level, reads, sends, runs_code }
}
