//! `~/.ssh/config` editing: add and remove servers. Only the relevant block is touched (comments, `Include`,
//! `Match` and other entries stay as they are), `config.codeyard-bak` is saved before each change and writes
//! are atomic (temp file + rename; permissions kept, 0600 for a new file).

use std::path::{Path, PathBuf};

use crate::ssh::parse_config;

/// A server added by the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewHost {
    pub alias: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

/// The config file in use (`CODEYARD_SSH_CONFIG` if set, otherwise `~/.ssh/config`).
pub fn config_path() -> Option<PathBuf> {
    match std::env::var_os("CODEYARD_SSH_CONFIG") {
        Some(p) => Some(PathBuf::from(p)),
        None => dirs::home_dir().map(|h| h.join(".ssh").join("config")),
    }
}

/// Reject option injection (`-oProxyCommand=…`) and config corruption: no whitespace, `#`, quotes or wildcards.
fn token_ok(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, '#' | '"' | '\'' | '*' | '?' | '!' | '=' | '\\' | ',' | '%' | '$' | '`' | ';' | '&' | '|' | '<' | '>' | '(' | ')')
        })
}

pub fn validate(h: &NewHost) -> Result<(), String> {
    if !token_ok(&h.alias) {
        return Err(format!("“{}” is not a valid name (no spaces or special characters, and it cannot start with “-”)", h.alias));
    }
    let host_ok = !h.hostname.is_empty()
        && !h.hostname.starts_with('-')
        && h.hostname.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'));
    if !host_ok {
        return Err(format!("“{}” is not a valid host name or address", h.hostname));
    }
    if let Some(u) = &h.user
        && (!token_ok(u) || u.contains('@') || u.contains(':'))
    {
        return Err(format!("“{u}” is not a valid user name"));
    }
    if h.port == Some(0) {
        return Err("The port must be between 1 and 65535".into());
    }
    if let Some(f) = &h.identity_file
        && (f.is_empty() || f.chars().any(|c| c.is_control() || c == '"'))
    {
        return Err("The identity file path is not valid".into());
    }
    Ok(())
}

/// Create the file empty if missing (dir 0700, file 0600) so an editor can open it.
pub fn ensure_exists(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    backup_and_write(path, "")
}

/// `user@host[:port] [as alias]` (or just `host`): the one line the user types.
pub fn parse_target(input: &str) -> Result<NewHost, String> {
    let input = input.trim();
    let (target, alias) = match input.split_once(" as ") {
        Some((t, a)) => (t.trim(), Some(a.trim().to_string())),
        None => (input, None),
    };
    if target.is_empty() || target.contains(char::is_whitespace) {
        return Err("Type user@host, or user@host:2222 as name".into());
    }
    let (user, rest) = match target.rsplit_once('@') {
        Some((u, r)) => (Some(u.to_string()), r),
        None => (None, target),
    };
    // Port only with a single `:` and no brackets (IPv6 looks like `[::1]:22`).
    let (hostname, port) = match (rest.strip_prefix('[').and_then(|r| r.split_once("]:")), rest.matches(':').count()) {
        (Some((h, p)), _) => (format!("[{h}]"), Some(p)),
        (None, 1) => match rest.split_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (rest.to_string(), None),
        },
        _ => (rest.to_string(), None),
    };
    let port = match port {
        Some(p) => Some(p.parse::<u16>().map_err(|_| format!("“{p}” is not a valid port"))?),
        None => None,
    };
    let alias = alias.filter(|a| !a.is_empty()).unwrap_or_else(|| hostname.trim_matches(['[', ']']).to_string());
    let host = NewHost { alias, hostname: hostname.trim_matches(['[', ']']).to_string(), user, port, identity_file: None };
    validate(&host)?;
    Ok(host)
}

pub fn render_block(h: &NewHost) -> String {
    let mut out = format!("# Added by Codeyard\nHost {}\n    HostName {}\n", h.alias, h.hostname);
    if let Some(u) = &h.user {
        out += &format!("    User {u}\n");
    }
    if let Some(p) = h.port {
        out += &format!("    Port {p}\n");
    }
    if let Some(f) = &h.identity_file {
        out += &format!("    IdentityFile {}\n    IdentitiesOnly yes\n", quote_value(f));
    }
    out
}

fn backup_and_write(path: &Path, new_text: &str) -> Result<(), String> {
    // For a symlinked config (dotfiles repo) modify the target, not the link; a rename would otherwise
    // replace the link with a regular file.
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = real.as_path();
    let dir = path.parent().ok_or("invalid config path")?;
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let existing = std::fs::read(path).ok();
    let existing_mode = std::fs::metadata(path).ok();
    // Keep CRLF if the file uses it (Windows editors).
    let crlf = existing.as_deref().is_some_and(|b| b.windows(2).any(|w| w == b"\r\n"));
    let body: std::borrow::Cow<str> = if crlf { new_text.replace("\r\n", "\n").replace('\n', "\r\n").into() } else { new_text.into() };
    if existing.is_some() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".codeyard-bak");
        std::fs::copy(path, &bak).map_err(|e| format!("could not back up {}: {e}", path.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".codeyard-tmp{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, body.as_bytes()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = existing_mode.map_or(0o600, |m| m.permissions().mode() & 0o777);
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = existing_mode;
    std::fs::rename(&tmp, path)
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Add a new `Host` block. Errors if the name exists (never overwrites).
pub fn add_host(path: &Path, h: &NewHost) -> Result<(), String> {
    validate(h)?;
    // Line endings are normalised to LF; the file's own style (CRLF) is restored on write.
    let text = std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n");
    if parse_config(&text).iter().any(|x| x.alias.eq_ignore_ascii_case(&h.alias)) {
        return Err(format!("A host named “{}” already exists in {}", h.alias, path.display()));
    }
    let mut out = text;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
    out += &render_block(h);
    backup_and_write(path, &out)
}

/// Remove `alias`. A single-name `Host` block (and the "Added by Codeyard" comment right above it) is deleted;
/// on a multi-name `Host a b` line only that name goes. `Ok(false)`: not found.
pub fn remove_host(path: &Path, alias: &str) -> Result<bool, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    let host_tokens = |l: &str| -> Option<Vec<String>> {
        let t = l.trim();
        let (k, v) = t.split_once(|c: char| c.is_whitespace() || c == '=')?;
        k.eq_ignore_ascii_case("host").then(|| v.trim_start_matches(|c: char| c.is_whitespace() || c == '=').split_whitespace().map(String::from).collect())
    };
    // Split on the separator rather than slicing at byte offsets: the line is arbitrary user text.
    let is_boundary = |l: &str| {
        l.trim_start()
            .split_once(|c: char| c.is_whitespace() || c == '=')
            .is_some_and(|(k, _)| k.eq_ignore_ascii_case("host") || k.eq_ignore_ascii_case("match"))
    };
    let Some((at, tokens)) = lines.iter().enumerate().find_map(|(i, l)| host_tokens(l).filter(|t| t.iter().any(|a| a == alias)).map(|t| (i, t))) else {
        return Ok(false);
    };
    let mut out: Vec<String> = Vec::new();
    if tokens.len() == 1 {
        let end = (at + 1..lines.len()).find(|&i| is_boundary(lines[i])).unwrap_or(lines.len());
        let mut start = at;
        if start > 0 && lines[start - 1].trim().starts_with("# Added by Codeyard") {
            start -= 1;
        }
        // Keep the blank line before the deleted block; trailing blank lines aren't part of it.
        let mut keep_end = end;
        while keep_end > at + 1 && lines[keep_end - 1].trim().is_empty() {
            keep_end -= 1;
        }
        out.extend(lines[..start].iter().map(|s| s.to_string()));
        out.extend(lines[keep_end..].iter().map(|s| s.to_string()));
    } else {
        let rest: Vec<&String> = tokens.iter().filter(|a| *a != alias).collect();
        let indent: String = lines[at].chars().take_while(|c| c.is_whitespace()).collect();
        out.extend(lines[..at].iter().map(|s| s.to_string()));
        out.push(format!("{indent}Host {}", rest.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")));
        out.extend(lines[at + 1..].iter().map(|s| s.to_string()));
    }
    let mut new_text = out.join("\n");
    if !new_text.is_empty() {
        new_text.push('\n');
    }
    backup_and_write(path, &new_text)?;
    Ok(true)
}

/// Editable `Host` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    HostName,
    User,
    Port,
    IdentityFile,
    ProxyJump,
}

impl Field {
    pub const ALL: [Field; 5] = [Field::HostName, Field::User, Field::Port, Field::IdentityFile, Field::ProxyJump];

    /// Directive name in the config.
    pub fn key(self) -> &'static str {
        match self {
            Field::HostName => "HostName",
            Field::User => "User",
            Field::Port => "Port",
            Field::IdentityFile => "IdentityFile",
            Field::ProxyJump => "ProxyJump",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Field::HostName => "address or name to connect to, e.g. 10.0.0.5",
            Field::User => "user name on the server",
            Field::Port => "port number (default 22)",
            Field::IdentityFile => "private key file, e.g. ~/.ssh/id_work",
            Field::ProxyJump => "jump host(s), e.g. bastion or me@bastion:2222",
        }
    }

    fn check(self, value: &str) -> Result<(), String> {
        let ok = match self {
            Field::HostName => validate(&NewHost { alias: "x".into(), hostname: value.into(), user: None, port: None, identity_file: None }).is_ok(),
            Field::User => validate(&NewHost { alias: "x".into(), hostname: "h".into(), user: Some(value.into()), port: None, identity_file: None }).is_ok(),
            Field::Port => value.parse::<u16>().is_ok_and(|p| p > 0),
            Field::IdentityFile => {
                validate(&NewHost { alias: "x".into(), hostname: "h".into(), user: None, port: None, identity_file: Some(value.into()) }).is_ok()
            }
            // Comma-separated `[user@]host[:port]`; can't start with `-` (option injection).
            Field::ProxyJump => {
                !value.is_empty()
                    && value.split(',').all(|hop| {
                        let hop = hop.trim();
                        let (user, rest) = hop.rsplit_once('@').map_or((None, hop), |(u, r)| (Some(u), r));
                        let (host, port) = rest.rsplit_once(':').filter(|(h, _)| !h.ends_with(':')).map_or((rest, None), |(h, p)| (h, Some(p)));
                        user.is_none_or(token_ok)
                            && !host.is_empty()
                            && !host.starts_with('-')
                            && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '[' | ']' | ':'))
                            && port.is_none_or(|p| p.parse::<u16>().is_ok_and(|n| n > 0))
                    })
            }
        };
        if ok { Ok(()) } else { Err(format!("“{value}” is not a valid {} value", self.key())) }
    }
}

/// Current values of a `Host` block (`None` if unset).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<String>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
}

impl Details {
    pub fn get(&self, f: Field) -> Option<&str> {
        match f {
            Field::HostName => self.hostname.as_deref(),
            Field::User => self.user.as_deref(),
            Field::Port => self.port.as_deref(),
            Field::IdentityFile => self.identity_file.as_deref(),
            Field::ProxyJump => self.proxy_jump.as_deref(),
        }
    }
}

fn directive(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    let (k, v) = t.split_once(|c: char| c.is_whitespace() || c == '=')?;
    let v = v.trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim();
    // `IdentityFile "~/.ssh/id work"`: the unquoted form as a single value.
    let v = v.strip_prefix('"').and_then(|r| r.strip_suffix('"')).unwrap_or(v);
    Some((k.to_ascii_lowercase(), v.to_string()))
}

/// Block of `alias`: (Host line number, exclusive end line). `Err` on a multi-name line.
fn block_of(lines: &[&str], alias: &str) -> Result<(usize, usize), String> {
    let at = lines.iter().position(|l| directive(l).is_some_and(|(k, v)| k == "host" && v.split_whitespace().any(|a| a == alias))).ok_or_else(|| {
        // The Host may come from another file (`Include`), which we don't edit.
        if lines.iter().any(|l| directive(l).is_some_and(|(k, _)| k == "include")) {
            format!("“{alias}” is not in the SSH config file itself; it may come from an Include file: edit that file")
        } else {
            format!("“{alias}” is not in the SSH config")
        }
    })?;
    if directive(lines[at]).is_some_and(|(_, v)| v.split_whitespace().count() > 1) {
        return Err(format!("“{alias}” shares its Host line with other names; edit the file directly"));
    }
    let end = (at + 1..lines.len()).find(|&i| directive(lines[i]).is_some_and(|(k, _)| k == "host" || k == "match")).unwrap_or(lines.len());
    Ok((at, end))
}

pub fn details(path: &Path, alias: &str) -> Option<Details> {
    let text = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let (at, end) = block_of(&lines, alias).ok()?;
    let mut d = Details::default();
    for l in &lines[at + 1..end] {
        if let Some((k, v)) = directive(l) {
            let slot = match k.as_str() {
                "hostname" => &mut d.hostname,
                "user" => &mut d.user,
                "port" => &mut d.port,
                "identityfile" => &mut d.identity_file,
                "proxyjump" => &mut d.proxy_jump,
                _ => continue,
            };
            slot.get_or_insert(v);
        }
    }
    Some(d)
}

/// ssh config needs quotes around values with whitespace (`IdentityFile "~/.ssh/id work"`).
fn quote_value(v: &str) -> String {
    if v.contains(char::is_whitespace) { format!("\"{v}\"") } else { v.to_string() }
}

/// Change, add (`Some`) or remove (`None`) a directive in the block. Other lines, comments and other
/// blocks are untouched; a backup is taken first.
pub fn set_field(path: &Path, alias: &str, field: Field, value: Option<&str>) -> Result<(), String> {
    let value = value.map(str::trim).filter(|v| !v.is_empty());
    if let Some(v) = value {
        field.check(v)?;
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    let (at, end) = block_of(&lines, alias)?;
    let key = field.key().to_ascii_lowercase();
    let is_field = |l: &str| directive(l).is_some_and(|(k, _)| k == key);
    let mut out: Vec<String> = lines[..=at].iter().map(|s| s.to_string()).collect();
    let mut placed = false;
    let indent_of = |l: &str| l.chars().take_while(|c| c.is_whitespace()).collect::<String>();
    let mut indent = "    ".to_string();
    for l in &lines[at + 1..end] {
        if directive(l).is_some() {
            indent = indent_of(l);
        }
        if is_field(l) {
            // A repeated directive: update the first, remove the rest.
            if let (Some(v), false) = (value, placed) {
                out.push(format!("{}{} {}", indent_of(l), field.key(), quote_value(v)));
                placed = true;
            }
            continue;
        }
        out.push(l.to_string());
    }
    if let (Some(v), false) = (value, placed) {
        // After the block's last non-blank line (trailing blank lines stay after the block).
        let mut insert_at = out.len();
        while insert_at > at + 1 && out[insert_at - 1].trim().is_empty() {
            insert_at -= 1;
        }
        out.insert(insert_at, format!("{indent}{} {}", field.key(), quote_value(v)));
    }
    out.extend(lines[end..].iter().map(|s| s.to_string()));
    let mut new_text = out.join("\n");
    new_text.push('\n');
    if new_text == text {
        return Ok(());
    }
    backup_and_write(path, &new_text)
}

// --- Key login ---

/// Public key in `ssh_dir` (ed25519, ecdsa, rsa in that order); if none, generate a new ed25519 key (no passphrase,
/// via `ssh-keygen`). Returns (public key line, file path, whether newly generated).
pub fn ensure_key(ssh_dir: &Path) -> Result<(String, PathBuf, bool), String> {
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let public = ssh_dir.join(format!("{name}.pub"));
        if let Ok(text) = std::fs::read_to_string(&public) {
            let line = text.lines().next().unwrap_or_default().trim().to_string();
            if valid_public_key(&line) {
                return Ok((line, public, false));
            }
        }
    }
    std::fs::create_dir_all(ssh_dir).map_err(|e| format!("{}: {e}", ssh_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(ssh_dir, std::fs::Permissions::from_mode(0o700));
    }
    let private = ssh_dir.join("id_ed25519");
    if private.exists() {
        // Private key exists without a public one: derive it.
        let out = std::process::Command::new("ssh-keygen")
            .arg("-y")
            .arg("-f")
            .arg(&private)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("ssh-keygen: {e}"))?;
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || !valid_public_key(&line) {
            return Err(format!("{} exists but its public key could not be read (is it passphrase-protected?)", private.display()));
        }
        let public = ssh_dir.join("id_ed25519.pub");
        std::fs::write(&public, format!("{line}\n")).map_err(|e| format!("{}: {e}", public.display()))?;
        return Ok((line, public, false));
    }
    let out = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "codeyard", "-f"])
        .arg(&private)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("ssh-keygen is not available: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let public = ssh_dir.join("id_ed25519.pub");
    let line = std::fs::read_to_string(&public).map_err(|e| format!("{}: {e}", public.display()))?.lines().next().unwrap_or_default().trim().to_string();
    if valid_public_key(&line) { Ok((line, public, true)) } else { Err("ssh-keygen produced no usable key".into()) }
}

/// One-line OpenSSH public key (`ssh-ed25519 AAAA… comment`).
pub fn valid_public_key(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let kind = parts.next().unwrap_or_default();
    let blob = parts.next().unwrap_or_default();
    (kind.starts_with("ssh-") || kind.starts_with("ecdsa-") || kind.starts_with("sk-"))
        && blob.len() > 20
        && blob.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
        && !line.contains(['\n', '\r', '\0', '\''])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-sshcfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("config")
    }

    #[test]
    fn parses_user_host_port_and_alias() {
        let h = parse_target("deploy@10.0.0.5:2222 as prod").unwrap();
        assert_eq!((h.alias.as_str(), h.hostname.as_str(), h.user.as_deref(), h.port), ("prod", "10.0.0.5", Some("deploy"), Some(2222)));
        let h = parse_target("example.com").unwrap();
        assert_eq!((h.alias.as_str(), h.user, h.port), ("example.com", None, None));
        let h = parse_target("me@[::1]:2200").unwrap();
        assert_eq!((h.hostname.as_str(), h.port), ("::1", Some(2200)));
        assert_eq!(parse_target("me@fe80::1").unwrap().port, None, "bare IPv6 has no port");
        for bad in ["", "two words", "me@host:99999", "me@host:abc", "-oProxyCommand=x", "me@-host", "host as -x", "host as a b", "a;b", "me@ho$t"] {
            assert!(parse_target(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn add_appends_one_block_and_keeps_everything_else() {
        let p = tmp("add");
        std::fs::write(&p, "# my hosts\nHost old\n    HostName old.example.com\n").unwrap();
        let h = NewHost {
            alias: "prod".into(),
            hostname: "10.0.0.5".into(),
            user: Some("deploy".into()),
            port: Some(2222),
            identity_file: Some("~/.ssh/id_prod".into()),
        };
        add_host(&p, &h).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("# my hosts\nHost old\n    HostName old.example.com\n\n# Added by Codeyard\nHost prod\n"), "{text}");
        assert!(text.contains("    Port 2222\n    IdentityFile ~/.ssh/id_prod\n    IdentitiesOnly yes\n"));
        let hosts = parse_config(&text);
        assert_eq!(hosts.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(), ["old", "prod"]);
        assert!(std::fs::read_to_string(p.with_file_name("config.codeyard-bak")).unwrap().contains("Host old"), "backup of the previous file");
        // Duplicate refused, file untouched.
        assert!(add_host(&p, &h).unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), text);
    }

    #[test]
    fn add_creates_missing_file_and_handles_missing_trailing_newline() {
        let p = tmp("create");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
        let h = parse_target("a@b").unwrap();
        add_host(&p, &h).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().starts_with("# Added by Codeyard\nHost b\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let p2 = tmp("nonl");
        std::fs::write(&p2, "Host x\n    HostName x.example").unwrap();
        add_host(&p2, &parse_target("c@d").unwrap()).unwrap();
        assert_eq!(parse_config(&std::fs::read_to_string(&p2).unwrap()).len(), 2);
    }

    const CFG: &str = "# my hosts\nHost web\n    HostName web.example.com\n    User deploy\n    # keep me\n    Port 2200\n\nHost db\n  HostName 10.0.0.9\n\nHost a b\n    HostName ab.example\n\nHost *\n    ServerAliveInterval 30\n";

    #[test]
    fn details_and_set_field_edit_only_their_own_block() {
        let p = tmp("setfield");
        std::fs::write(&p, CFG).unwrap();
        let d = details(&p, "web").unwrap();
        assert_eq!(
            (d.hostname.as_deref(), d.user.as_deref(), d.port.as_deref(), d.identity_file.as_deref()),
            (Some("web.example.com"), Some("deploy"), Some("2200"), None)
        );
        assert_eq!(details(&p, "db").unwrap().hostname.as_deref(), Some("10.0.0.9"));
        assert!(details(&p, "nope").is_none());
        // Replace keeps the indentation and everything else.
        set_field(&p, "web", Field::Port, Some("2222")).unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("    # keep me\n    Port 2222\n\nHost db\n  HostName 10.0.0.9\n"), "{t}");
        // Add goes after the block's last directive, in that block's indentation (db uses two spaces).
        set_field(&p, "db", Field::User, Some("root")).unwrap();
        set_field(&p, "db", Field::IdentityFile, Some("~/.ssh/id_db")).unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("Host db\n  HostName 10.0.0.9\n  User root\n  IdentityFile ~/.ssh/id_db\n\nHost a b"), "{t}");
        // Remove.
        set_field(&p, "web", Field::User, None).unwrap();
        set_field(&p, "web", Field::Port, Some("  ")).unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("Host web\n    HostName web.example.com\n    # keep me\n\nHost db"), "{t}");
        assert!(t.contains("Host *\n    ServerAliveInterval 30\n"));
        assert!(
            std::fs::read_to_string(p.with_file_name("config.codeyard-bak")).unwrap().contains("Port 2222"),
            "the file before the last change is backed up"
        );
        // Shared Host lines and unknown names are refused; nothing is written.
        let before = std::fs::read_to_string(&p).unwrap();
        assert!(set_field(&p, "a", Field::User, Some("x")).unwrap_err().contains("shares its Host line"));
        assert!(set_field(&p, "ghost", Field::User, Some("x")).unwrap_err().contains("not in the SSH config"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
        // Idempotent: the same value does not rewrite or back up again.
        set_field(&p, "db", Field::User, Some("root")).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
    }

    #[test]
    fn field_values_are_validated_against_injection() {
        let p = tmp("validate");
        std::fs::write(&p, CFG).unwrap();
        for (f, bad) in [
            (Field::Port, "0"),
            (Field::Port, "99999"),
            (Field::Port, "22; rm"),
            (Field::HostName, "-oProxyCommand=x"),
            (Field::HostName, "a b"),
            (Field::User, "-x"),
            (Field::User, "a@b"),
            (Field::ProxyJump, "-oProxyCommand=x"),
            (Field::ProxyJump, "a,,b"),
            (Field::ProxyJump, "host:abc"),
            (Field::IdentityFile, "a\nb"),
            (Field::IdentityFile, "\"quoted\""),
        ] {
            assert!(set_field(&p, "web", f, Some(bad)).is_err(), "{f:?} {bad:?}");
        }
        for (f, good) in [
            (Field::Port, "2222"),
            (Field::ProxyJump, "bastion"),
            (Field::ProxyJump, "me@bastion:2222,hop2"),
            (Field::IdentityFile, "~/.ssh/id work"),
            (Field::HostName, "10.0.0.5"),
            (Field::User, "ada"),
        ] {
            assert!(set_field(&p, "web", f, Some(good)).is_ok(), "{f:?} {good:?}");
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap().matches("ProxyJump").count(), 1, "one directive per field");
    }

    #[test]
    fn keys_are_found_derived_or_generated() {
        let d = tmp("keys").parent().unwrap().to_path_buf();
        let ssh = d.join("dotssh");
        // Generated when nothing exists: ed25519, private key 0600, directory 0700.
        let (line, path, generated) = ensure_key(&ssh).unwrap();
        assert!(generated && valid_public_key(&line) && line.starts_with("ssh-ed25519 ") && path.ends_with("id_ed25519.pub"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(ssh.join("id_ed25519")).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&ssh).unwrap().permissions().mode() & 0o777, 0o700);
        }
        // An existing key is reused untouched.
        let (again, _, generated) = ensure_key(&ssh).unwrap();
        assert!(!generated && again == line);
        // Missing public key: derived from the private one.
        std::fs::remove_file(ssh.join("id_ed25519.pub")).unwrap();
        let (derived, _, generated) = ensure_key(&ssh).unwrap();
        assert!(!generated && derived.split_whitespace().nth(1) == line.split_whitespace().nth(1));
        assert!(valid_public_key("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl me@x"));
        assert!(!valid_public_key("ssh-ed25519 short") && !valid_public_key("") && !valid_public_key("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC' ; rm"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_stays_a_symlink_and_the_target_is_edited() {
        let real = tmp("symlink");
        std::fs::write(&real, "Host web\n    HostName web.example\n").unwrap();
        let link = real.with_file_name("config-link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        set_field(&link, "web", Field::User, Some("ada")).unwrap();
        add_host(&link, &parse_target("x@y as extra").unwrap()).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link itself is not replaced");
        let t = std::fs::read_to_string(&real).unwrap();
        assert!(t.contains("User ada") && t.contains("Host extra"), "{t}");
        assert!(real.with_file_name("config.codeyard-bak").exists(), "the backup sits next to the real file");
    }

    #[test]
    fn crlf_files_stay_crlf_and_tab_indented_uppercase_blocks_are_understood() {
        let p = tmp("crlf");
        std::fs::write(&p, "HOST web\r\n\tHOSTNAME web.example\r\n\tPort = 2200\r\n\r\nHost db\r\n\tHostName db.example\r\n").unwrap();
        set_field(&p, "web", Field::Port, Some("2222")).unwrap();
        add_host(&p, &parse_target("a@b as new").unwrap()).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        let t = String::from_utf8(bytes.clone()).unwrap();
        assert!(t.contains("\tPort 2222\r\n"), "{t:?}");
        assert!(!t.replace("\r\n", "").contains('\n'), "no bare LF was introduced: {t:?}");
        assert_eq!(details(&p, "web").unwrap().port.as_deref(), Some("2222"));
        assert_eq!(details(&p, "web").unwrap().hostname.as_deref(), Some("web.example"), "keywords are case-insensitive");
        assert!(remove_host(&p, "db").unwrap());
        assert!(!String::from_utf8(std::fs::read(&p).unwrap()).unwrap().replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn values_with_spaces_are_quoted_and_read_back() {
        let p = tmp("quote");
        std::fs::write(&p, "Host web\n    HostName w.example\n").unwrap();
        set_field(&p, "web", Field::IdentityFile, Some("~/My Keys/id work")).unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("IdentityFile \"~/My Keys/id work\"\n"), "{t}");
        assert_eq!(details(&p, "web").unwrap().identity_file.as_deref(), Some("~/My Keys/id work"));
        // Changing it again replaces the quoted line instead of adding a second one.
        set_field(&p, "web", Field::IdentityFile, Some("~/.ssh/id_plain")).unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert_eq!(t.matches("IdentityFile").count(), 1);
        assert!(t.contains("IdentityFile ~/.ssh/id_plain\n"));
        let h = NewHost { alias: "k".into(), hostname: "h".into(), user: None, port: None, identity_file: Some("/a b/key".into()) };
        assert!(render_block(&h).contains("IdentityFile \"/a b/key\"\n"));
    }

    #[test]
    fn names_from_an_included_file_are_explained_and_duplicates_ignore_case() {
        let p = tmp("include");
        std::fs::write(&p, "Include ~/.ssh/work.conf\nHost Local\n    HostName l\n").unwrap();
        let e = set_field(&p, "work-box", Field::User, Some("x")).unwrap_err();
        assert!(e.contains("Include"), "{e}");
        assert!(
            set_field(&p, "ghost-without-include", Field::User, Some("x")).unwrap_err().contains("Include"),
            "any missing name hints at Include when the file has one"
        );
        assert!(add_host(&p, &parse_target("a@b as local").unwrap()).unwrap_err().contains("already exists"), "Host names match without regard to case");
        let q = tmp("noinclude");
        std::fs::write(&q, "Host a\n").unwrap();
        assert!(!set_field(&q, "zzz", Field::User, Some("x")).unwrap_err().contains("Include"));
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_config_reports_the_path_and_changes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("readonly").parent().unwrap().join("ro");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config");
        std::fs::write(&p, "Host a\n    HostName a\n").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        // A read-only directory blocks the temporary file (the usual case for a root-owned ~/.ssh).
        let err = set_field(&p, "a", Field::User, Some("x"));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        if let Err(e) = err {
            assert!(e.contains("ro") || e.contains("config"), "{e}");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "Host a\n    HostName a\n", "the file is untouched");
        } else {
            // Running as root: permissions do not apply, the edit simply succeeds.
            assert!(std::fs::read_to_string(&p).unwrap().contains("User x"));
        }
    }

    #[test]
    fn empty_and_missing_files_are_handled() {
        let p = tmp("empty");
        std::fs::write(&p, "").unwrap();
        assert!(set_field(&p, "a", Field::User, Some("x")).is_err());
        assert!(!remove_host(&p, "a").unwrap());
        add_host(&p, &parse_target("u@h as first").unwrap()).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().starts_with("# Added by Codeyard\nHost first\n"));
        let missing = p.with_file_name("nope");
        assert!(set_field(&missing, "a", Field::User, Some("x")).unwrap_err().contains("nope"));
        assert!(remove_host(&missing, "a").unwrap_err().contains("nope"));
        assert!(details(&missing, "a").is_none());
    }

    #[test]
    fn remove_deletes_only_that_block() {
        let p = tmp("remove");
        std::fs::write(
            &p,
            "Host keep\n    HostName keep.example\n\n# Added by Codeyard\nHost gone\n    HostName gone.example\n    User x\n\nHost *\n    ServerAliveInterval 30\n\nHost a b\n    HostName ab.example\n",
        )
        .unwrap();
        assert!(remove_host(&p, "gone").unwrap());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("gone") && !text.contains("Added by Codeyard"), "{text}");
        assert!(text.contains("Host keep\n    HostName keep.example\n") && text.contains("Host *\n    ServerAliveInterval 30\n"), "{text}");
        // Multi-alias line: only that name goes.
        assert!(remove_host(&p, "a").unwrap());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("Host b\n    HostName ab.example"), "{text}");
        assert!(!remove_host(&p, "nope").unwrap());
    }

    #[test]
    fn remove_survives_multibyte_text_around_the_boundary_keywords() {
        let p = tmp("remove-utf8");
        // "Hosé" has a char boundary at byte 3 and 5, not 4: byte slicing at 4 would panic.
        std::fs::write(&p, "Host gone\n    HostName g.example\nHosé x\nMatch\tall\n    User y\n").unwrap();
        assert!(remove_host(&p, "gone").unwrap());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "Match\tall\n    User y\n", "the unknown `Hosé` line belongs to the block; `Match` ends it");
    }
}
