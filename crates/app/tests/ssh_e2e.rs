//! The real remote path: system `ssh` + a temporary `sshd` running on 127.0.0.1
//! on this machine with user privileges. Connect → upload agent (SHA-256) → daemon → file, command,
//! terminal → port forwarding (over ControlMaster) → cancel.
//! Skipped (loudly) if sshd or the agent binary is missing; CODEYARD_REQUIRE_SSHD=1 makes that a failure.
#![cfg(unix)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use host::Host;

fn keygen(path: &Path) {
    assert!(Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(path).status().unwrap().success());
}

fn agent_dir() -> Option<PathBuf> {
    // target/<profile>/codeyard → codeyard-agent in the same directory (built in the workspace test).
    let dir = Path::new(env!("CARGO_BIN_EXE_codeyard")).parent()?.to_path_buf();
    dir.join("codeyard-agent").is_file().then_some(dir)
}

#[test]
fn connect_upload_use_and_forward_over_real_ssh() {
    let sshd = Path::new("/usr/sbin/sshd");
    let Some(agents) = agent_dir() else {
        assert!(std::env::var_os("CODEYARD_REQUIRE_SSHD").is_none(), "codeyard-agent not built and CODEYARD_REQUIRE_SSHD is set");
        eprintln!("SKIPPED: codeyard-agent not built (cargo build -p agent; set CODEYARD_REQUIRE_SSHD=1 to make this an error)");
        return;
    };
    if !sshd.exists() {
        assert!(std::env::var_os("CODEYARD_REQUIRE_SSHD").is_none(), "no /usr/sbin/sshd and CODEYARD_REQUIRE_SSHD is set");
        eprintln!("SKIPPED: no /usr/sbin/sshd (set CODEYARD_REQUIRE_SSHD=1 to make this an error)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("codeyard-sshe2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    keygen(&dir.join("host_key"));
    keygen(&dir.join("user_key"));
    std::fs::copy(dir.join("user_key.pub"), dir.join("authorized_keys")).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let d = dir.display();
    std::fs::write(
        dir.join("sshd_config"),
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/host_key\nAuthorizedKeysFile {d}/authorized_keys\n\
             PasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nStrictModes no\nPidFile {d}/sshd.pid\nAllowTcpForwarding yes\n"
        ),
    )
    .unwrap();
    let user = std::env::var("USER").unwrap();
    std::fs::write(
        dir.join("ssh_config"),
        format!(
            "Host codeyard-e2e\n  HostName 127.0.0.1\n  Port {port}\n  User {user}\n  IdentityFile {d}/user_key\n  IdentitiesOnly yes\n\
               IdentityAgent none\n  UserKnownHostsFile {d}/known_hosts\n  StrictHostKeyChecking accept-new\n"
        ),
    )
    .unwrap();
    let mut server = Command::new(sshd).arg("-D").arg("-f").arg(dir.join("sshd_config")).stderr(Stdio::null()).spawn().unwrap();
    let start = std::time::Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(start.elapsed().as_secs() < 5, "sshd did not start");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // SAFETY: no other thread in this test process reads the environment (single test).
    unsafe {
        std::env::set_var("CODEYARD_SSH_CONFIG", dir.join("ssh_config"));
        std::env::set_var("CODEYARD_REMOTE_DIR", dir.join("remote"));
        std::env::set_var("CODEYARD_AGENT_DIR", &agents);
    }
    let cleanup = |server: &mut std::process::Child| {
        host::ssh::close_master("codeyard-e2e");
        let _ = Command::new("pkill").arg("-f").arg(format!("{}/remote/agent", dir.display())).status();
        let _ = server.kill();
        let _ = server.wait();
    };

    let result = std::panic::catch_unwind(|| {
        let mut steps = Vec::new();
        let host = host::ssh::connect("codeyard-e2e", &host::ssh::AgentBundle::discover(), |s| steps.push(s)).expect("should connect");
        assert!(steps.contains(&host::ssh::Step::Uploading), "the agent is uploaded on first connect: {steps:?}");
        assert!(host.is_remote());
        assert_eq!(host.info().os, std::env::consts::OS);

        // File, command, terminal: all on the agent over ssh.
        std::fs::write(dir.join("merhaba.txt"), "ğüş\n").unwrap();
        let (data, _) = host.read(&dir.join("merhaba.txt").to_string_lossy()).unwrap();
        assert_eq!(data, "ğüş\n".as_bytes());
        let out = host.exec("echo", &["uzak"], None, None).unwrap();
        assert_eq!(out.stdout_str().trim(), "uzak");
        let pty = host.pty_spawn(80, 24, None, Some("/bin/sh".into())).unwrap();
        host.pty_input(pty.id, b"echo terminal-$((40+2))\n".to_vec());
        let mut text = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !text.contains("terminal-42") {
            assert!(std::time::Instant::now() < deadline, "terminal: {text}");
            if let Ok(host::PtyEvent::Output(d)) = pty.events.recv_timeout(std::time::Duration::from_millis(200)) {
                text.push_str(&String::from_utf8_lossy(&d));
            }
        }

        // A development server "on the server": reachable through the local port.
        let app = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_port = app.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in app.incoming().flatten() {
                let _ = s.write_all(b"merhaba-port");
            }
        });
        let local = host::ssh::free_local_port(remote_port); // same machine: it is taken, so another port is returned
        assert_ne!(local, remote_port);
        host::ssh::forward_port("codeyard-e2e", local, remote_port, false).expect("forward");
        let mut got = String::new();
        std::net::TcpStream::connect(("127.0.0.1", local)).unwrap().read_to_string(&mut got).unwrap();
        assert_eq!(got, "merhaba-port");
        host::ssh::forward_port("codeyard-e2e", local, remote_port, true).expect("iptal");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(std::net::TcpStream::connect(("127.0.0.1", local)).is_err(), "port is closed after cancel");

        // Forwards with a kept spec: an exact IPv4 target, a busy preferred port, a strict request and cancel by spec.
        use host::forward::{FailKind, LocalBind, OpenRequest, RemoteTarget};
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy_port = busy.local_addr().unwrap().port();
        let req = |strict| OpenRequest {
            alias: "codeyard-e2e".into(),
            remote: remote_port,
            target: RemoteTarget::V4(std::net::Ipv4Addr::LOCALHOST),
            wanted_local: Some(busy_port),
            strict,
            bind: LocalBind::Loopback,
        };
        let err = host::forward::open(&req(true)).expect_err("a strict busy port fails");
        assert_eq!(err.kind, FailKind::AddrInUse, "{err:?}");
        let opened = host::forward::open(&req(false)).expect("falls back to another port");
        assert_ne!(opened.local, busy_port);
        let mut got = String::new();
        std::net::TcpStream::connect(("127.0.0.1", opened.local)).unwrap().read_to_string(&mut got).unwrap();
        assert_eq!(got, "merhaba-port");
        host::forward::cancel("codeyard-e2e", &opened.spec).expect("cancel with the kept spec");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(std::net::TcpStream::connect(("127.0.0.1", opened.local)).is_err(), "closed after cancel by spec");
        assert!(host::forward::check_master("codeyard-e2e"));

        // Second connection: the agent is already uploaded (hash matches), no upload.
        drop(host);
        let mut steps = Vec::new();
        let again = host::ssh::connect("codeyard-e2e", &host::ssh::AgentBundle::discover(), |s| steps.push(s)).unwrap();
        assert!(!steps.contains(&host::ssh::Step::Uploading), "{steps:?}");
        drop(again);
    });
    cleanup(&mut server);
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
