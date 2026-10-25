//! SSH: the system `ssh` binary. ~/.ssh/config, ProxyJump, ssh-agent and
//! hardware keys work unchanged; no crypto of our own.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::{Connector, HostError, RemoteHost, Result, spawn_transport};

pub const APP_DIR: &str = "codeyard";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
}

/// Concrete Hosts in `~/.ssh/config` (no wildcard patterns).
pub fn list_hosts() -> Vec<SshHost> {
    // Same config as used for connecting (`CODEYARD_SSH_CONFIG` if set, else ~/.ssh/config).
    let Some(path) = crate::ssh_config::config_path() else { return Vec::new() };
    std::fs::read_to_string(path).map(|s| parse_config(&s)).unwrap_or_default()
}

/// Known limitation: `Include` and `Match` blocks are not followed.
pub fn parse_config(text: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.to_ascii_lowercase(), v.trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim()),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                current.clear();
                for alias in value.split_whitespace() {
                    if alias.contains(['*', '?', '!']) || hosts.iter().any(|h| h.alias == alias) {
                        continue;
                    }
                    current.push(hosts.len());
                    hosts.push(SshHost { alias: alias.into(), hostname: None, user: None });
                }
            }
            "match" => current.clear(),
            "hostname" => current.iter().for_each(|&i| hosts[i].hostname = Some(value.into())),
            "user" => current.iter().for_each(|&i| hosts[i].user = Some(value.into())),
            _ => {}
        }
    }
    hosts
}

/// This machine's target triple (picks the right binary from the agent bundle).
pub fn local_triple() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("x86_64", "linux") => "x86_64-unknown-linux-musl",
        ("aarch64", "linux") => "aarch64-unknown-linux-musl",
        _ => "unknown",
    }
}

/// Target triple from `uname -sm` output.
pub fn triple_for_uname(uname: &str) -> Option<&'static str> {
    let mut parts = uname.split_whitespace();
    match (parts.next()?, parts.next()?) {
        ("Linux", "x86_64" | "amd64") => Some("x86_64-unknown-linux-musl"),
        ("Linux", "aarch64" | "arm64") => Some("aarch64-unknown-linux-musl"),
        ("Darwin", "arm64") => Some("aarch64-apple-darwin"),
        ("Darwin", "x86_64") => Some("x86_64-apple-darwin"),
        _ => None,
    }
}

/// Agent dirs next to the executable. In the macOS bundle (`Codeyard.app/Contents/MacOS/codeyard`) agents
/// live in `Contents/Resources/agents`: code signing treats everything in `MacOS/` as executable and Linux
/// binaries can't be signed there, while `Resources/` is sealed as plain data.
fn bundle_dirs_for(exe: &Path) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else { return Vec::new() };
    let mut dirs = vec![dir.join("agents")];
    if dir.file_name().is_some_and(|n| n == "MacOS")
        && let Some(contents) = dir.parent()
    {
        dirs.push(contents.join("Resources").join("agents"));
    }
    dirs.push(dir.to_path_buf());
    dirs
}

/// Agent binaries bundled with the app.
pub struct AgentBundle {
    dirs: Vec<PathBuf>,
}

impl AgentBundle {
    /// Search order: $CODEYARD_AGENT_DIR, `<exe>/agents/`, `<exe>/../Resources/agents/` (macOS bundle), `<exe>/`, and
    /// `target/<triple>/<profile>/` in development (see `find`).
    pub fn discover() -> Self {
        let mut dirs = Vec::new();
        if let Some(d) = std::env::var_os("CODEYARD_AGENT_DIR") {
            dirs.push(PathBuf::from(d));
        }
        if let Ok(exe) = std::env::current_exe() {
            dirs.extend(bundle_dirs_for(&exe));
        }
        AgentBundle { dirs }
    }

    pub fn find(&self, triple: &str) -> Option<PathBuf> {
        let named = format!("codeyard-agent-{triple}");
        let mut candidates = Vec::new();
        for d in &self.dirs {
            candidates.push(d.join(&named));
            if triple == local_triple() {
                candidates.push(d.join("codeyard-agent"));
            }
        }
        // target/<triple>/{release-agent,release,debug}/codeyard-agent
        if let Ok(exe) = std::env::current_exe()
            && let Some(target) = exe.parent().and_then(Path::parent)
        {
            for profile in ["release-agent", "release", "debug"] {
                candidates.push(target.join(triple).join(profile).join("codeyard-agent"));
            }
        }
        candidates.into_iter().find(|p| p.is_file())
    }
}

/// Longest control socket we ask ssh to bind, including the `.` + 16 random characters it appends while binding
/// (it renames the socket into place afterwards). `sun_path` holds 108 bytes on Linux and 104 on macOS, both
/// counting the NUL, so 90 leaves a margin for platforms we have not looked at.
#[cfg(unix)]
const MAX_SOCKET: usize = 90;
#[cfg(unix)]
const SSH_BIND_SUFFIX: usize = 17;

/// Where the control sockets live: `<home>/.cache/codeyard`, or a short private directory when that would push the
/// socket past `MAX_SOCKET`. `None` means no safe directory exists: the caller then runs without connection sharing.
#[cfg(unix)]
fn control_dir() -> Option<PathBuf> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    control_dir_in(&std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir), Path::new("/tmp"), uid)
}

/// Windows OpenSSH has no ControlMaster; the directory only holds the dev agent's socket.
#[cfg(not(unix))]
fn control_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(".cache").join(APP_DIR);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// `short_base` is `/tmp` and not `$TMPDIR`, which on macOS is itself ~50 bytes (`/var/folders/..`).
#[cfg(unix)]
fn control_dir_in(home: &Path, short_base: &Path, uid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let preferred = home.join(".cache").join(APP_DIR);
    if preferred.as_os_str().as_bytes().len() <= MAX_DIR && private_dir(&preferred, None).is_ok() {
        return Some(preferred);
    }
    // The shared base is world-writable, so another user may have squatted the name: `private_dir` refuses
    // anything that is not a real directory owned by us.
    let short = short_base.join(format!("cy-{uid}"));
    private_dir(&short, Some(uid)).ok().map(|()| short)
}

/// Longest directory that still leaves room for `/cm-<16 hex>` and ssh's bind suffix.
#[cfg(unix)]
const MAX_DIR: usize = MAX_SOCKET - SSH_BIND_SUFFIX - SOCKET_NAME_LEN - 1;
#[cfg(unix)]
const SOCKET_NAME_LEN: usize = "cm-".len() + 16;

/// Create `dir` (0700) if needed and make sure it is a real directory only we can enter. With `owner`, an existing
/// entry must also belong to that uid: it may sit in a shared sticky directory like `/tmp`, where a symlink or a
/// directory planted by another user would otherwise receive our sockets. We own what passes the check and
/// the sticky bit stops others from renaming it, so tightening the mode afterwards is not racy.
#[cfg(unix)]
fn private_dir(dir: &Path, owner: Option<u32>) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let check = |want_private: bool| -> std::io::Result<()> {
        // symlink_metadata: a symlink must not be followed to wherever it points.
        let md = std::fs::symlink_metadata(dir)?;
        if !md.is_dir() {
            return Err(Error::new(ErrorKind::PermissionDenied, "not a directory"));
        }
        if owner.is_some_and(|uid| md.uid() != uid) {
            return Err(Error::new(ErrorKind::PermissionDenied, "owned by another user"));
        }
        if want_private && md.mode() & 0o077 != 0 {
            return Err(Error::new(ErrorKind::PermissionDenied, "accessible to others"));
        }
        Ok(())
    };
    check(false)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    check(true)
}

/// The control socket for `alias`: `<dir>/cm-<16 hex>`. ssh's own `%C` is 40 hex characters and cannot be shortened,
/// so we hash the alias ourselves; the alias fixes host, user and port through ssh config. 64 bits
/// make a collision between the handful of aliases a person has negligible, and one would only reuse the wrong
/// connection, never expose a socket.
#[cfg(unix)]
fn control_path(alias: &str) -> Option<PathBuf> {
    Some(control_path_in(&control_dir()?, alias))
}

#[cfg(unix)]
fn control_path_in(dir: &Path, alias: &str) -> PathBuf {
    let hash: String = Sha256::digest(alias.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    dir.join(format!("cm-{hash}"))
}

/// ssh expands `%` tokens and splits option values at whitespace, so a path goes in escaped and quoted.
#[cfg(unix)]
fn control_path_option(path: &Path) -> String {
    let p = path.to_string_lossy().replace('%', "%%").replace('\\', "\\\\").replace('"', "\\\"");
    format!("ControlPath=\"{p}\"")
}

/// All `ssh` calls share one ControlMaster connection: auth happens once,
/// later calls take milliseconds.
/// Common options: a separate ssh config (`CODEYARD_SSH_CONFIG`, for tests and
/// non-standard setups) and the shared ControlMaster socket.
fn ssh_base(alias: &str) -> Command {
    let mut cmd = Command::new("ssh");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
        // Native Windows OpenSSH does not support Unix ControlMaster sockets.
        cmd.args(["-o", "ControlMaster=no", "-o", "ControlPath=none", "-o", "ControlPersist=no"]);
    }
    if let Some(cfg) = std::env::var_os("CODEYARD_SSH_CONFIG") {
        cmd.arg("-F").arg(cfg);
    }
    #[cfg(unix)]
    {
        match control_path(alias) {
            Some(path) => cmd.arg("-o").arg(control_path_option(&path)),
            // No safe directory: connect without sharing (ssh keeps the first value it sees for an option).
            None => cmd.args(["-o", "ControlMaster=no", "-o", "ControlPath=none"]),
        };
    }
    #[cfg(not(unix))]
    {
        let _ = alias;
    }
    cmd
}

/// Aliases with a ControlMaster opened by this process; all closed on exit.
static MASTERS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Connection target: an SSH alias, a running Docker container (`docker:name`) or a WSL
/// distro (`wsl:name`). Scripts run on the target via `sh -c`; the agent speaks the same protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target<'a> {
    Ssh(&'a str),
    Docker(&'a str),
    Wsl(&'a str),
}

pub fn target_of(alias: &str) -> Target<'_> {
    match (alias.strip_prefix("docker:"), alias.strip_prefix("wsl:")) {
        (Some(c), _) if !c.is_empty() => Target::Docker(c),
        (_, Some(d)) if !d.is_empty() => Target::Wsl(d),
        _ => Target::Ssh(alias),
    }
}

/// `Command` that runs `<command>` on the target; the caller appends the script as one argument.
pub(crate) fn target_command(alias: &str) -> Command {
    match target_of(alias) {
        Target::Ssh(a) => ssh_command(a),
        Target::Docker(c) => {
            let mut cmd = Command::new("docker");
            cmd.args(["exec", "-i", c, "sh", "-c"]);
            cmd
        }
        Target::Wsl(d) => {
            let mut cmd = Command::new("wsl.exe");
            cmd.args(["-d", d, "--", "sh", "-c"]);
            cmd
        }
    }
}

/// `docker ps` output (`name\timage` lines) → (name, image).
pub fn parse_containers(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let (name, image) = l.split_once('\t')?;
            (!name.trim().is_empty()).then(|| (name.trim().to_string(), image.trim().to_string()))
        })
        .collect()
}

/// Running Docker containers (error message if docker is missing or not running).
pub fn list_containers() -> std::result::Result<Vec<(String, String)>, String> {
    let out = Command::new("docker")
        .args(["ps", "--format", "{{.Names}}\t{{.Image}}"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("docker is not available: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(parse_containers(&String::from_utf8_lossy(&out.stdout)))
}

/// `wsl.exe -l -q` prints UTF-16LE (BOM and NULs are stripped).
pub fn parse_wsl_list(raw: &[u8]) -> Vec<String> {
    let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    let text = if raw.len() >= 2 && raw.len().is_multiple_of(2) && raw.iter().skip(1).step_by(2).filter(|&&b| b == 0).count() * 2 >= raw.len() / 2 {
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(raw).into_owned()
    };
    text.trim_start_matches('\u{feff}').lines().map(|l| l.trim().trim_matches('\0').to_string()).filter(|l| !l.is_empty()).collect()
}

pub fn list_wsl() -> std::result::Result<Vec<String>, String> {
    let out = Command::new("wsl.exe").args(["-l", "-q"]).stdin(Stdio::null()).output().map_err(|e| format!("WSL is not available: {e}"))?;
    if !out.status.success() {
        // wsl.exe writes error text in UTF-16 too.
        return Err(parse_wsl_list(&out.stdout).join(" "));
    }
    Ok(parse_wsl_list(&out.stdout).into_iter().filter(|d| !d.starts_with("docker-desktop")).collect())
}

fn ssh_command(alias: &str) -> Command {
    MASTERS.lock().unwrap_or_else(|e| e.into_inner()).insert(alias.to_string());
    let mut cmd = ssh_base(alias);
    // Password / passphrase / host key are asked via the window prompt;
    // with no bridge (CLI, tests) don't ask, fail at once.
    if !crate::askpass::configure(&mut cmd) {
        cmd.args(["-o", "BatchMode=yes"]);
    }
    // ServerAliveCountMax=2: a dead connection is noticed in ~30 s and reconnecting starts.
    cmd.args(["-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=2"]);
    #[cfg(unix)]
    cmd.args(["-o", "ControlMaster=auto", "-o", "ControlPersist=600"]);
    // `--` first: a host name that starts with `-` is a host name, never an ssh option (`-oProxyCommand=…` runs a program).
    cmd.arg("--").arg(alias);
    cmd
}

/// Refuses names that would be read as an option by ssh/docker, or that cannot be one line of a command.
/// Every path to a target goes through here first; `--` in the command line is the second lock.
fn check_target(alias: &str) -> Result<()> {
    let name = match target_of(alias) {
        Target::Ssh(n) | Target::Docker(n) | Target::Wsl(n) => n,
    };
    if name.is_empty() || name.starts_with('-') || name.contains(['\0', '\n', '\r']) {
        return Err(HostError::Disconnected(format!("“{}” is not a valid host name", alias.escape_debug())));
    }
    Ok(())
}

pub(crate) fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Overall timeout for container/WSL commands (ssh has its own `ConnectTimeout`), agent upload included.
const RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

fn run(alias: &str, script: &str, stdin: Option<&[u8]>) -> Result<String> {
    use std::io::{Read, Write};
    check_target(alias)?;
    let mut cmd = target_command(alias);
    // ssh passes the script to the server's login shell (fish, csh…); `sh -c` gives the same POSIX behaviour everywhere.
    // Container and WSL commands already start with `sh -c`.
    let script = if matches!(target_of(alias), Target::Ssh(_)) { format!("sh -c {}", sh_quote(script)) } else { script.to_string() };
    cmd.arg(script).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd.spawn().map_err(|e| HostError::Disconnected(format!("could not start {program}: {e}")))?;
    // Upload on its own thread: if the target exits early (EPIPE), the real cause is read from stderr.
    let writer = stdin.zip(child.stdin.take()).map(|(data, mut w)| {
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = w.write_all(&data);
        })
    });
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out_t = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err_t = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > RUN_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HostError::Disconnected(format!("{program} did not answer within {} s", RUN_TIMEOUT.as_secs())));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(15)),
            Err(e) => return Err(HostError::Disconnected(e.to_string())),
        }
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    let (stdout, stderr) = (out_t.join().unwrap_or_default(), err_t.join().unwrap_or_default());
    if !status.success() {
        let err = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(HostError::Disconnected(if err.is_empty() { format!("{program} exited with {status}") } else { err }));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

/// Connection steps, used by the UI for progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Probing,
    Uploading,
    Starting,
}

/// Result of "test connection": server responds, its architecture, whether the bundle has a matching agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub uname: String,
    pub triple: Option<&'static str>,
    pub agent_bundled: bool,
    pub millis: u128,
}

/// Connect (without installing the agent), verify identity and read the architecture. Errors are ssh's own message.
pub fn probe(alias: &str, bundle: &AgentBundle) -> Result<Probe> {
    let start = std::time::Instant::now();
    let out = run(alias, "uname -sm", None)?;
    let uname = out.lines().next().unwrap_or_default().trim().to_string();
    let triple = triple_for_uname(&uname);
    let agent_bundled = triple.is_some_and(|t| bundle.find(t).is_some());
    Ok(Probe { uname, triple, agent_bundled, millis: start.elapsed().as_millis() })
}

/// Append the public key to the server's `authorized_keys` (no-op if present); the password prompt goes through the
/// askpass bridge. `authorized_keys`: path on the server; `None`: `~/.ssh/authorized_keys` (dir set to 0700, file to
/// 0600). Returns `true` if added, `false` if already there.
pub fn install_public_key(alias: &str, public_key: &str, authorized_keys: Option<&str>) -> Result<bool> {
    if !crate::ssh_config::valid_public_key(public_key) {
        return Err(HostError::Disconnected("that is not a valid public key".into()));
    }
    let file = match authorized_keys {
        Some(p) => sh_quote(p),
        None => r#""$HOME/.ssh/authorized_keys""#.to_string(),
    };
    // The key goes via stdin, not the script, so the shell can't interpret it. The same key (even with another comment) is
    // not added twice; a file without a trailing newline gets one first so the keys don't run together.
    let script = format!(
        r#"set -e; umask 077; f={file}; d=$(dirname "$f"); mkdir -p "$d"; {chmod_dir}touch "$f"; chmod 600 "$f";
k=$(cat); b=$(printf '%s' "$k" | awk '{{print $2}}');
if grep -qF -- "$b" "$f"; then echo present;
else if [ -s "$f" ] && [ "$(tail -c1 "$f" | wc -l | tr -d ' ')" = 0 ]; then printf '\n' >> "$f"; fi; printf '%s\n' "$k" >> "$f"; echo added; fi"#,
        chmod_dir = if authorized_keys.is_none() { r#"chmod 700 "$d"; "# } else { "" },
    );
    let out = run(alias, &script, Some(public_key.as_bytes()))?;
    Ok(out.trim() == "added")
}

/// Connect: identify the server, install the agent if missing (SHA-256 verified), start it.
pub fn connect(alias: &str, bundle: &AgentBundle, mut progress: impl FnMut(Step)) -> Result<RemoteHost> {
    progress(Step::Probing);
    let probe = run(alias, r#"uname -sm; printf '%s\n' "$HOME""#, None)?;
    let mut lines = probe.lines();
    let uname = lines.next().unwrap_or_default();
    let home = lines.next().unwrap_or_default().trim().to_string();
    let triple = triple_for_uname(uname).ok_or_else(|| HostError::Disconnected(format!("unsupported server: {uname}")))?;
    let local = bundle.find(triple).ok_or_else(|| HostError::Disconnected(format!("no agent binary bundled for {triple} (codeyard-agent-{triple})")))?;
    let binary = std::fs::read(&local).map_err(|e| HostError::Disconnected(format!("{}: {e}", local.display())))?;
    let sha: String = Sha256::digest(&binary).iter().map(|b| format!("{b:02x}")).collect();

    // CODEYARD_REMOTE_DIR: agent location on the server (default ~/.cache/codeyard).
    let dir = std::env::var("CODEYARD_REMOTE_DIR").unwrap_or_else(|_| format!("{home}/.cache/{APP_DIR}"));
    let remote = format!("{dir}/agent-v{}-{}", proto::VERSION, &sha[..12]);
    let installed = run(alias, &format!("test -x {} && echo ok || true", sh_quote(&remote)), None)?;
    if installed.trim() != "ok" {
        progress(Step::Uploading);
        // Write to a temp file, verify the hash on the server, move into place only on a match.
        let script = format!(
            r#"set -e; mkdir -p {dir}; t={remote}.tmp.$$; cat > "$t"; chmod 755 "$t";
h=$( (sha256sum "$t" 2>/dev/null || shasum -a 256 "$t") | cut -d' ' -f1);
if [ "$h" = {sha} ]; then mv "$t" {remote}; else rm -f "$t"; echo "agent hash mismatch: $h" >&2; exit 3; fi"#,
            dir = sh_quote(&dir),
            remote = sh_quote(&remote),
            sha = sh_quote(&sha),
        );
        run(alias, &script, Some(&binary))?;
    }

    progress(Step::Starting);
    // Each ssh connection bridges to the daemon, so sessions (terminals) outlive an ssh drop.
    let socket = format!("{remote}.sock");
    let alias_owned = alias.to_string();
    let connector: Connector = Arc::new(move || {
        let mut cmd = target_command(&alias_owned);
        cmd.arg(format!("{} --attach {}", sh_quote(&remote), sh_quote(&socket)));
        spawn_transport(cmd)
    });
    let label = match target_of(alias) {
        Target::Ssh(a) => format!("ssh://{a}"),
        Target::Docker(c) => format!("docker://{c}"),
        Target::Wsl(d) => format!("wsl://{d}"),
    };
    RemoteHost::connect(connector, new_session_id(alias), label)
}

/// Forward a local port to a server port (or remove it with `cancel`). Reuses the open
/// ControlMaster connection: no new authentication.
pub fn forward_port(alias: &str, local: u16, remote: u16, cancel: bool) -> Result<()> {
    if cfg!(windows) {
        return Err(HostError::Op(proto::Error::Other(
            "In-app SSH port forwarding requires ControlMaster, which Windows OpenSSH does not support. Use ssh -L in a terminal.".into(),
        )));
    }
    if !matches!(target_of(alias), Target::Ssh(_)) {
        return Err(HostError::Op(proto::Error::Other("Port forwarding needs an SSH host: containers and WSL share the local network".into())));
    }
    check_target(alias)?;
    let out = ssh_base(alias)
        .args(["-o", "BatchMode=yes", "-O", if cancel { "cancel" } else { "forward" }, "-L"])
        .arg(format!("{local}:localhost:{remote}"))
        .arg("--")
        .arg(alias)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| HostError::Disconnected(format!("could not start ssh: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(HostError::Op(proto::Error::Other(if err.is_empty() { format!("ssh exited with {}", out.status) } else { err })))
    }
}

/// Forward a server port to a local port (`ssh -R`): a program on the server connecting to `localhost:remote`
/// reaches `local` here (or remove it with `cancel`).
/// Debug adapters (delve) connect by dialing the client.
pub fn forward_port_reverse(alias: &str, remote: u16, local: u16, cancel: bool) -> Result<()> {
    if cfg!(windows) {
        return Err(HostError::Op(proto::Error::Other(
            "In-app reverse SSH forwarding requires ControlMaster, which Windows OpenSSH does not support. Use ssh -R in a terminal.".into(),
        )));
    }
    if !matches!(target_of(alias), Target::Ssh(_)) {
        return Err(HostError::Op(proto::Error::Other("Port forwarding needs an SSH host: containers and WSL share the local network".into())));
    }
    check_target(alias)?;
    let out = ssh_base(alias)
        .args(["-o", "BatchMode=yes", "-O", if cancel { "cancel" } else { "forward" }, "-R"])
        .arg(format!("{remote}:localhost:{local}"))
        .arg("--")
        .arg(alias)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| HostError::Disconnected(format!("could not start ssh: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(HostError::Op(proto::Error::Other(if err.is_empty() { format!("ssh exited with {}", out.status) } else { err })))
    }
}

/// Close the shared ControlMaster connection (ends its forwards too).
pub fn close_master(alias: &str) {
    if cfg!(windows) {
        return;
    }
    if !matches!(target_of(alias), Target::Ssh(_)) {
        return;
    }
    let _ = ssh_base(alias).args(["-o", "BatchMode=yes", "-O", "exit", "--"]).arg(alias).stdin(Stdio::null()).output();
}

/// On exit: close every ControlMaster this process opened. `ControlPersist`
/// would keep them alive after quit, and macOS shows that as "running in the background".
pub fn close_all_masters() {
    let aliases = std::mem::take(&mut *MASTERS.lock().unwrap_or_else(|e| e.into_inner()));
    for alias in aliases {
        close_master(&alias);
    }
}

/// Free port on this machine: the requested one if free, else one from the OS.
pub fn free_local_port(wanted: u16) -> u16 {
    let try_bind = |p: u16| std::net::TcpListener::bind(("127.0.0.1", p)).ok().and_then(|l| l.local_addr().ok()).map(|a| a.port());
    try_bind(wanted).filter(|_| wanted >= 1024).or_else(|| try_bind(0)).unwrap_or(wanted)
}

/// Per-window session ID (finds the terminals on the server).
pub fn new_session_id(salt: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{:016x}", proto::content_hash(format!("{salt}-{}-{nanos}", std::process::id()).as_bytes()))
}

/// Dev only: exercise the remote code path without ssh, with the real daemon and socket.
/// Killing the bridge process (`--attach`) behaves like an ssh drop.
pub fn spawn_local_agent(bundle: &AgentBundle) -> Result<RemoteHost> {
    let bin = bundle.find(local_triple()).ok_or_else(|| HostError::Disconnected("local codeyard-agent not found (cargo build -p agent)".into()))?;
    let socket = control_dir().unwrap_or_else(std::env::temp_dir).join(format!("dev-agent-v{}.sock", proto::VERSION));
    let connector: Connector = Arc::new(move || {
        let mut cmd = Command::new(&bin);
        cmd.arg("--attach").arg(&socket);
        spawn_transport(cmd)
    });
    RemoteHost::connect(connector, new_session_id("dev"), "agent (stdio)")
}

#[cfg(test)]
mod tests {
    #[test]
    fn agents_are_looked_up_next_to_the_binary_and_in_the_macos_bundle() {
        let plain = bundle_dirs_for(Path::new("/opt/codeyard/codeyard"));
        assert_eq!(plain, [PathBuf::from("/opt/codeyard/agents"), PathBuf::from("/opt/codeyard")]);
        let app = bundle_dirs_for(Path::new("/Applications/Codeyard.app/Contents/MacOS/codeyard"));
        assert_eq!(
            app,
            [
                PathBuf::from("/Applications/Codeyard.app/Contents/MacOS/agents"),
                PathBuf::from("/Applications/Codeyard.app/Contents/Resources/agents"),
                PathBuf::from("/Applications/Codeyard.app/Contents/MacOS"),
            ],
            "the bundle's Resources folder is searched too, and the old MacOS/agents location still works"
        );
        assert!(bundle_dirs_for(Path::new("/")).is_empty());
    }

    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_ssh_avoids_unix_multiplexing_and_reports_forwarding_limits() {
        let cmd = ssh_command("example");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.iter().any(|a| a == "ControlMaster=no"));
        assert!(args.iter().any(|a| a == "ControlPath=none"));
        assert!(!args.iter().any(|a| a == "ControlMaster=auto" || a == "ControlPersist=600"));
        assert!(forward_port("example", 3000, 3000, false).unwrap_err().to_string().contains("Windows OpenSSH"));
        assert!(forward_port_reverse("example", 3000, 3000, false).is_err());
    }

    #[test]
    fn a_host_name_can_never_be_an_ssh_option() {
        let args = |a: &str| ssh_command(a).get_args().map(|x| x.to_string_lossy().into_owned()).collect::<Vec<_>>();
        let a = args("devbox");
        assert_eq!(&a[a.len() - 2..], ["--", "devbox"], "`--` comes right before the host");
        for bad in ["-oProxyCommand=touch /tmp/x", "-F/etc/passwd", "", "a\nb", "docker:-v", "wsl:--x", "x\0y"] {
            let err = check_target(bad).unwrap_err().to_string();
            assert!(err.contains("not a valid host name"), "{bad:?}: {err}");
            assert!(run(bad, "true", None).is_err());
        }
        assert!(forward_port("-oProxyCommand=x", 1, 2, false).is_err());
        for ok in ["devbox", "user@10.0.0.5", "docker:web", "wsl:Ubuntu", "my-host.example.com"] {
            assert!(check_target(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn parses_ssh_config() {
        let cfg = "\
Host *\n  ServerAliveInterval 30\n\
Host node-dev dev2\n  HostName 10.0.0.5\n  User deploy\n\
# yorum\n\
Host=web\n  HostName=web.example.com\n\
Host prod-*\n  User root\n\
Match host foo\n  User nope\n";
        let hosts = parse_config(cfg);
        assert_eq!(hosts.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(), ["node-dev", "dev2", "web"]);
        assert_eq!(hosts[0].user.as_deref(), Some("deploy"));
        assert_eq!(hosts[1].hostname.as_deref(), Some("10.0.0.5"));
        assert_eq!(hosts[2].hostname.as_deref(), Some("web.example.com"));
    }

    /// Scratch directory under the system temp dir, removed on drop.
    #[cfg(unix)]
    struct Scratch(PathBuf);
    #[cfg(unix)]
    impl Scratch {
        fn new(tag: &str) -> Self {
            // /tmp, not temp_dir(): macOS's is long enough to trigger the fallback by itself.
            let dir = Path::new("/tmp").join(format!("cy-ssh-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }
    #[cfg(unix)]
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// What ssh really binds: the path plus `.` and 16 random characters; macOS allows 103 bytes, Linux 107.
    #[cfg(unix)]
    fn bound_len(path: &Path) -> usize {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().len() + SSH_BIND_SUFFIX
    }

    #[cfg(unix)]
    #[test]
    fn the_control_socket_path_fits_for_any_home_length() {
        let scratch = Scratch::new("len");
        let short_base = scratch.0.join("tmp");
        std::fs::create_dir_all(&short_base).unwrap();
        let me = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&short_base).unwrap());
        let aliases = ["web", "prod-eu-west-1.internal.example.com", "çok-uzun-sunucu-adı-🚀-".repeat(12).leak(), "user@host:2222"];
        for len in 1..=200 {
            // A home of exactly `len` bytes below the scratch dir (a single component grows without a limit).
            let home = scratch.0.join("h".repeat(len));
            let dir = control_dir_in(&home, &short_base, me).unwrap_or_else(|| panic!("no dir for home of {len} bytes"));
            for alias in aliases {
                let path = control_path_in(&dir, alias);
                assert!(bound_len(&path) <= MAX_SOCKET, "{len}: {path:?} binds {} bytes", bound_len(&path));
            }
            assert!(dir == home.join(".cache").join(APP_DIR) || dir == short_base.join(format!("cy-{me}")), "{dir:?}");
            let _ = std::fs::remove_dir_all(&home);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_short_home_keeps_the_socket_in_its_cache() {
        let scratch = Scratch::new("short");
        let home = scratch.0.join("h");
        let dir = control_dir_in(&home, &scratch.0, 0).unwrap();
        assert_eq!(dir, home.join(".cache").join(APP_DIR));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn the_socket_name_is_stable_and_tells_hosts_apart() {
        let dir = Path::new("/d");
        let a = control_path_in(dir, "web");
        assert_eq!(a, control_path_in(dir, "web"));
        let others = ["web2", "Web", "deploy@web", "web:2222", "deploy@web:2222", ""];
        let mut seen = std::collections::BTreeSet::from([a]);
        for o in others {
            assert!(seen.insert(control_path_in(dir, o)), "{o:?} collides");
        }
        assert_eq!(control_path_in(dir, "web").file_name().unwrap().len(), SOCKET_NAME_LEN);
    }

    #[cfg(unix)]
    #[test]
    fn the_short_fallback_dir_is_private_and_refuses_impostors() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
        let scratch = Scratch::new("fallback");
        let base = scratch.0.join("tmp");
        std::fs::create_dir_all(&base).unwrap();
        let home = scratch.0.join("h".repeat(150));
        let me = std::fs::metadata(&base).unwrap().uid();

        // Created 0700 even when the umask is loose, and reused on the next call.
        let dir = control_dir_in(&home, &base, me).unwrap();
        assert_eq!(dir, base.join(format!("cy-{me}")));
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        assert_eq!(control_dir_in(&home, &base, me), Some(dir.clone()));

        // A directory that is too open is tightened (it is ours).
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(control_dir_in(&home, &base, me), Some(dir.clone()));
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);

        // A directory that belongs to someone else (here: we pretend to be another uid) is refused.
        assert!(private_dir(&dir, Some(me.wrapping_add(1))).is_err(), "owned by another uid");

        // A symlink planted at the name is never followed, even when it points at a directory we own.
        std::fs::remove_dir(&dir).unwrap();
        let target = scratch.0.join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        symlink(&target, &dir).unwrap();
        assert_eq!(control_dir_in(&home, &base, me), None);
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0, "nothing was put behind the link");

        // A plain file in the way is refused as well.
        std::fs::remove_file(&dir).unwrap();
        std::fs::write(&dir, b"").unwrap();
        assert_eq!(control_dir_in(&home, &base, me), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_control_path_option_survives_ssh_parsing() {
        assert_eq!(control_path_option(Path::new("/a b/100%/cm-1")), r#"ControlPath="/a b/100%%/cm-1""#);
    }

    #[test]
    fn maps_uname() {
        assert_eq!(triple_for_uname("Linux x86_64\n"), Some("x86_64-unknown-linux-musl"));
        assert_eq!(triple_for_uname("Darwin arm64"), Some("aarch64-apple-darwin"));
        assert_eq!(triple_for_uname("FreeBSD amd64"), None);
    }

    #[test]
    fn quotes_for_shell() {
        assert_eq!(sh_quote("/home/a b/x'y"), r"'/home/a b/x'\''y'");
    }

    #[test]
    fn targets_containers_and_wsl_lists() {
        assert_eq!(target_of("prod"), Target::Ssh("prod"));
        assert_eq!(target_of("docker:web-1"), Target::Docker("web-1"));
        assert_eq!(target_of("wsl:Ubuntu"), Target::Wsl("Ubuntu"));
        assert_eq!(target_of("docker:"), Target::Ssh("docker:"), "empty name: treated as an ssh alias");
        assert_eq!(
            parse_containers("web\tnginx:1\n\ndb\tpostgres\nbad line\n"),
            vec![("web".to_string(), "nginx:1".to_string()), ("db".to_string(), "postgres".to_string())]
        );
        let utf16: Vec<u8> = "\u{feff}Ubuntu\r\nDebian\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(parse_wsl_list(&utf16), vec!["Ubuntu".to_string(), "Debian".to_string()]);
        assert_eq!(parse_wsl_list(b"Alpine\nArch\n"), vec!["Alpine".to_string(), "Arch".to_string()], "UTF-8 de kabul");
        assert!(forward_port("docker:x", 1, 2, false).is_err());
    }
}
