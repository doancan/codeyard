//! Throwaway `sshd` for tests: on this machine, as the current user, on 127.0.0.1.
//! The `codeyard-e2e` alias is in a separate ssh config; the agent is uploaded to a temp dir.
//! Drop shuts down the ssh master, the remote daemon and sshd.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub const ALIAS: &str = "codeyard-e2e";

pub struct Sshd {
    pub dir: PathBuf,
    pub port: u16,
    child: Child,
    /// Environment variables are process-wide: SSH tests run sequentially.
    _turn: std::sync::MutexGuard<'static, ()>,
}

static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn keygen(path: &Path) -> bool {
    Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(path).status().is_ok_and(|s| s.success())
}

/// Env switch for CI jobs that ship sshd: a skip below becomes a failure, so a broken runner cannot turn the SSH tests into no-ops.
const REQUIRE: &str = "CODEYARD_REQUIRE_SSHD";

/// Slow runners (instrumented coverage builds) multiply every `wait_secs` limit with `CODEYARD_TEST_TIMEOUT_SCALE` (default 1).
pub fn timeout_scale() -> u64 {
    std::env::var("CODEYARD_TEST_TIMEOUT_SCALE").ok().and_then(|v| v.parse().ok()).filter(|&n| n >= 1).unwrap_or(1)
}

/// A test that cannot run says so on stderr (visible with `--nocapture` and in failures); with `CODEYARD_REQUIRE_SSHD=1` it fails.
fn skip<T>(test: &str, why: &str) -> Option<T> {
    assert!(std::env::var_os(REQUIRE).is_none(), "{test}: cannot run ({why}) and {REQUIRE} is set");
    eprintln!("SKIPPED {test}: {why} (set {REQUIRE}=1 to make this an error)");
    None
}

/// Call right before an early `return` in a test that needs an external tool (python3, node, clang, lldb-dap, debugpy, go, dlv, clangd).
/// Prints a visible SKIPPED line; `CODEYARD_REQUIRE_TOOLS=1` (or a list such as `python3,clang`) turns the skip into a failure.
pub fn skip_missing(tools: &str) {
    let test = std::thread::current().name().unwrap_or("?").to_string();
    if let Some(req) = std::env::var("CODEYARD_REQUIRE_TOOLS").ok().filter(|r| !r.is_empty()) {
        let wanted = req == "1" || tools.split(',').any(|t| req.split(',').any(|r| r == t));
        assert!(!wanted, "{test}: needs {tools}, which is unavailable, and CODEYARD_REQUIRE_TOOLS={req}");
    }
    eprintln!("SKIPPED {test}: needs {tools}, which is unavailable (set CODEYARD_REQUIRE_TOOLS=1 to make this an error)");
}

/// `None`: no sshd or agent binary, or the setup failed (the test is skipped, loudly; see `skip`).
pub fn start(name: &str) -> Option<Sshd> {
    match try_start(name) {
        Ok(s) => Some(s),
        Err(why) => skip(name, &why),
    }
}

fn try_start(name: &str) -> Result<Sshd, String> {
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let sshd = Path::new("/usr/sbin/sshd");
    // target/<profile>/deps/codeyard-… → target/<profile>/codeyard-agent
    let agents = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.parent()?.to_path_buf())).ok_or("no test executable path")?;
    if !sshd.exists() {
        return Err("/usr/sbin/sshd is missing".into());
    }
    if !agents.join("codeyard-agent").is_file() {
        return Err(format!("{} is missing (build it: cargo build -p agent)", agents.join("codeyard-agent").display()));
    }
    let io = |what: &'static str| move |e: std::io::Error| format!("{what}: {e}");
    let dir = std::env::temp_dir().join(format!("codeyard-sshd-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(io("create temp dir"))?;
    let dir = std::fs::canonicalize(&dir).map_err(io("canonicalize temp dir"))?;
    if !keygen(&dir.join("host_key")) || !keygen(&dir.join("user_key")) {
        return Err("ssh-keygen failed".into());
    }
    std::fs::copy(dir.join("user_key.pub"), dir.join("authorized_keys")).map_err(io("copy authorized_keys"))?;
    let port = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map_err(io("pick a port"))?.port();
    let d = dir.display();
    std::fs::write(
        dir.join("sshd_config"),
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/host_key\nAuthorizedKeysFile {d}/authorized_keys\n\
             PasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nStrictModes no\nPidFile {d}/sshd.pid\nAllowTcpForwarding yes\n"
        ),
    )
    .map_err(io("write sshd_config"))?;
    let user = std::env::var("USER").map_err(|_| "USER is not set")?;
    std::fs::write(
        dir.join("ssh_config"),
        format!(
            "Host {ALIAS}\n  HostName 127.0.0.1\n  Port {port}\n  User {user}\n  IdentityFile {d}/user_key\n  IdentitiesOnly yes\n\
               IdentityAgent none\n  UserKnownHostsFile {d}/known_hosts\n  StrictHostKeyChecking accept-new\n"
        ),
    )
    .map_err(io("write ssh_config"))?;
    let child = Command::new(sshd).arg("-D").arg("-f").arg(dir.join("sshd_config")).stderr(Stdio::null()).spawn().map_err(io("spawn sshd"))?;
    let start = std::time::Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        if start.elapsed().as_secs() > 5 {
            return Err("sshd did not listen within 5 s".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // SAFETY: only ssh commands read it; there is only one sshd test at a time.
    unsafe {
        std::env::set_var("CODEYARD_SSH_CONFIG", dir.join("ssh_config"));
        std::env::set_var("CODEYARD_REMOTE_DIR", dir.join("remote"));
        std::env::set_var("CODEYARD_AGENT_DIR", &agents);
    }
    Ok(Sshd { dir, port, child, _turn: turn })
}

impl Sshd {
    /// Kill the server (cuts the connection and new ones; also closes the remote agent's bridge).
    pub fn stop_server(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = Command::new("pkill").arg("-f").arg(format!("{}/remote/agent.*--attach", self.dir.display())).status();
        host::ssh::close_master(ALIAS);
    }

    /// Second alias to the same server without the key (to provoke an auth failure).
    pub fn add_alias_without_key(&self, alias: &str) {
        use std::io::Write;
        let user = std::env::var("USER").unwrap_or_default();
        let mut f = std::fs::OpenOptions::new().append(true).open(self.dir.join("ssh_config")).expect("ssh_config");
        let _ = write!(
            f,
            "\nHost {alias}\n  HostName 127.0.0.1\n  Port {}\n  User {user}\n  IdentityFile /nonexistent/key\n  IdentitiesOnly yes\n  IdentityAgent none\n  \
             PasswordAuthentication no\n  BatchMode yes\n  UserKnownHostsFile {}/known_hosts\n  StrictHostKeyChecking accept-new\n",
            self.port,
            self.dir.display()
        );
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        host::ssh::close_master(ALIAS);
        let _ = Command::new("pkill").arg("-f").arg(format!("{}/remote/agent", self.dir.display())).status();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Fake `docker` (put on PATH): `docker exec -i NAME sh -c SCRIPT` → runs the script on this machine.
/// Tests the container flow (agent upload, `--attach`, remote files) without real docker.
pub struct FakeDocker {
    dir: PathBuf,
    old_path: Option<std::ffi::OsString>,
    _turn: std::sync::MutexGuard<'static, ()>,
}

pub fn start_fake_docker(name: &str) -> Option<FakeDocker> {
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let agents = std::env::current_exe().ok()?.parent()?.parent()?.to_path_buf();
    if !agents.join("codeyard-agent").is_file() {
        return skip(name, "codeyard-agent is missing (build it: cargo build -p agent)");
    }
    let dir = std::env::temp_dir().join(format!("codeyard-fakedocker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).ok()?;
    let dir = std::fs::canonicalize(&dir).ok()?;
    let shim = dir.join("bin/docker");
    std::fs::write(&shim, "#!/bin/sh\nif [ \"$1\" = exec ]; then shift; shift; shift; exec \"$@\"; fi\nif [ \"$1\" = ps ]; then printf 'fake-box\\tdebian:12\\n'; exit 0; fi\nexit 1\n").ok()?;
    {
        #[cfg(unix)]
        crate::test_files::make_executable(&shim);
    }
    let old_path = std::env::var_os("PATH");
    let new_path = format!("{}:{}", dir.join("bin").display(), old_path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    // SAFETY: one remote test at a time (TURN).
    unsafe {
        std::env::set_var("PATH", new_path);
        std::env::set_var("CODEYARD_REMOTE_DIR", dir.join("remote"));
        std::env::set_var("CODEYARD_AGENT_DIR", &agents);
    }
    Some(FakeDocker { dir, old_path, _turn: turn })
}

impl Drop for FakeDocker {
    fn drop(&mut self) {
        let _ = Command::new("pkill").arg("-f").arg(format!("{}/remote/agent", self.dir.display())).status();
        // SAFETY: see start_fake_docker.
        unsafe {
            match &self.old_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
