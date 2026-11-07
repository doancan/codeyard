//! Real `ssh` → `SSH_ASKPASS` → the app's askpass client → bridge → handler chain: key passphrase, unknown
//! host key, wrong password, giving up and rejecting. It has its own `sshd` and a passphrase-protected key; it runs
//! in a separate process because the bridge is installed process-wide.
//!
//! (Password and key passphrase are asked the same way: `AskKind::Secret`. A non-root `sshd` can't verify
//! the system password, so the server side uses a passphrase-protected key instead of a password.)

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use host::askpass::{AskKind, AskRequest};

const ALIAS: &str = "askpass-e2e";
const PASSPHRASE: &str = "s3cret";

struct Fixture {
    dir: PathBuf,
    child: Child,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        host::ssh::close_master(ALIAS);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn keygen(path: &Path, passphrase: &str) -> bool {
    Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", passphrase, "-f"]).arg(path).status().is_ok_and(|s| s.success())
}

fn start() -> Option<Fixture> {
    let sshd = Path::new("/usr/sbin/sshd");
    if !sshd.exists() {
        return None;
    }
    let dir = std::env::temp_dir().join(format!("codeyard-askpass-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    let dir = std::fs::canonicalize(&dir).ok()?;
    if !keygen(&dir.join("host_key"), "") || !keygen(&dir.join("user_key"), PASSPHRASE) {
        return None;
    }
    std::fs::copy(dir.join("user_key.pub"), dir.join("authorized_keys")).ok()?;
    let port = std::net::TcpListener::bind("127.0.0.1:0").ok()?.local_addr().ok()?.port();
    let d = dir.display();
    std::fs::write(
        dir.join("sshd_config"),
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/host_key\nAuthorizedKeysFile {d}/authorized_keys\n\
             PasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nStrictModes no\nPidFile {d}/sshd.pid\n"
        ),
    )
    .ok()?;
    let user = std::env::var("USER").ok()?;
    std::fs::write(
        dir.join("ssh_config"),
        format!(
            "Host {ALIAS}\n  HostName 127.0.0.1\n  Port {port}\n  User {user}\n  IdentityFile {d}/user_key\n  IdentitiesOnly yes\n  IdentityAgent none\n  \
             PasswordAuthentication no\n  UserKnownHostsFile {d}/known_hosts\n  StrictHostKeyChecking ask\n  ControlMaster no\n"
        ),
    )
    .ok()?;
    let child = Command::new(sshd).arg("-D").arg("-f").arg(dir.join("sshd_config")).stderr(Stdio::null()).spawn().ok()?;
    let start = std::time::Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        if start.elapsed().as_secs() > 5 {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Some(Fixture { dir, child })
}

/// The handler's behavior (changes in later steps of the test) and the log of incoming questions.
#[derive(Default)]
struct Script {
    host_key: Option<String>,
    secret: Option<String>,
    asked: Vec<AskRequest>,
}

#[test]
fn ssh_questions_reach_the_window_and_answers_reach_ssh() {
    let Some(fx) = start() else {
        assert!(std::env::var_os("CODEYARD_REQUIRE_SSHD").is_none(), "no /usr/sbin/sshd or ssh-keygen and CODEYARD_REQUIRE_SSHD is set");
        eprintln!("SKIPPED: no /usr/sbin/sshd or ssh-keygen (set CODEYARD_REQUIRE_SSHD=1 to make this an error)");
        return;
    };
    // Long on purpose: a deep home (CI runner, container, long user name) overflowed the control socket's
    // Unix path limit and every connection failed. Keep it past the point where the app must use its short dir.
    let home = fx.dir.join("a-deliberately-long-home-directory-for-the-control-socket-path").join("home");
    assert!(home.as_os_str().len() > 90, "{home:?} must stay long");
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: a single test; no other thread in the process is reading environment variables yet.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("CODEYARD_SSH_CONFIG", fx.dir.join("ssh_config"));
        std::env::set_var("CODEYARD_ASKPASS_EXE", env!("CARGO_BIN_EXE_codeyard"));
    }
    let script = Arc::new(Mutex::new(Script::default()));
    let s = script.clone();
    host::askpass::install(Arc::new(move |req: AskRequest| {
        let mut s = s.lock().unwrap();
        s.asked.push(req.clone());
        match req.kind {
            AskKind::HostKey => s.host_key.clone(),
            AskKind::Secret => s.secret.clone(),
            AskKind::Confirm | AskKind::Text => None,
        }
    }))
    .expect("askpass bridge");
    let bundle = host::ssh::AgentBundle::discover();
    let reset = |host_key: Option<&str>, secret: Option<&str>| {
        host::ssh::close_master(ALIAS);
        let mut s = script.lock().unwrap();
        *s = Script { host_key: host_key.map(String::from), secret: secret.map(String::from), asked: Vec::new() };
    };
    let known_hosts = fx.dir.join("known_hosts");

    // 1. Unknown host key and the right passphrase: both questions are asked, in that order, and the connection works.
    reset(Some("yes"), Some(PASSPHRASE));
    let probe = host::ssh::probe(ALIAS, &bundle).expect("connects after the questions are answered");
    assert!(!probe.uname.is_empty());
    {
        let s = script.lock().unwrap();
        assert_eq!(s.asked.len(), 2, "{:?}", s.asked);
        assert_eq!(s.asked[0].kind, AskKind::HostKey);
        assert!(s.asked[0].prompt.contains("authenticity of host"), "{}", s.asked[0].prompt);
        assert_eq!(s.asked[1].kind, AskKind::Secret);
        assert!(s.asked[1].prompt.contains("passphrase for key") && s.asked[1].prompt.contains("user_key"), "{}", s.asked[1].prompt);
    }
    assert!(std::fs::read_to_string(&known_hosts).unwrap().contains("127.0.0.1"), "the accepted host key is remembered");

    // 2. Known host now: only the passphrase is asked.
    reset(None, Some(PASSPHRASE));
    host::ssh::probe(ALIAS, &bundle).expect("second connection");
    assert_eq!(script.lock().unwrap().asked.iter().filter(|a| a.kind == AskKind::HostKey).count(), 0);

    // 3. Wrong passphrase: ssh gives up with a clear reason after a few tries.
    reset(None, Some("not-the-passphrase"));
    let err = host::ssh::probe(ALIAS, &bundle).unwrap_err().to_string();
    assert!(err.contains("Permission denied") || err.contains("passphrase") || err.contains("publickey"), "{err}");
    let tries = script.lock().unwrap().asked.iter().filter(|a| a.kind == AskKind::Secret).count();
    assert!((1..=3).contains(&tries), "ssh asks at most three times, asked {tries}");

    // 4. The user cancels the passphrase prompt: a failure, not a hang.
    reset(None, None);
    let started = std::time::Instant::now();
    assert!(host::ssh::probe(ALIAS, &bundle).is_err());
    assert!(started.elapsed().as_secs() < 20, "cancelling must not hang");

    // 5. The user rejects an unknown host key.
    let _ = std::fs::remove_file(&known_hosts);
    reset(Some("no"), Some(PASSPHRASE));
    let err = host::ssh::probe(ALIAS, &bundle).unwrap_err().to_string();
    assert!(err.contains("Host key verification failed"), "{err}");
    assert!(!known_hosts.exists() || !std::fs::read_to_string(&known_hosts).unwrap().contains("127.0.0.1"), "a rejected key is not remembered");
    assert_eq!(script.lock().unwrap().asked.iter().filter(|a| a.kind == AskKind::Secret).count(), 0, "no passphrase is asked of a host that was refused");

    // 6. Key login setup through the same questions: the key lands in authorized_keys.
    let ssh_dir = fx.dir.join("dot-ssh");
    let (line, _, generated) = host::ssh_config::ensure_key(&ssh_dir).unwrap();
    assert!(generated);
    let authorized = fx.dir.join("installed_keys");
    reset(Some("yes"), Some(PASSPHRASE));
    assert!(host::ssh::install_public_key(ALIAS, &line, authorized.to_str()).expect("installed"));
    assert_eq!(std::fs::read_to_string(&authorized).unwrap().trim(), line);
    {
        let s = script.lock().unwrap();
        assert!(s.asked.iter().any(|a| a.kind == AskKind::Secret), "the passphrase was asked: {:?}", s.asked);
    }
    reset(None, Some(PASSPHRASE));
    assert!(!host::ssh::install_public_key(ALIAS, &line, authorized.to_str()).unwrap(), "second time: already there");
}
