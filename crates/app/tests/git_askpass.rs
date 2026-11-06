//! Real `git` over HTTP → `GIT_ASKPASS` → the app's askpass client → bridge → handler chain: a server that wants a login asks for
//! the user name (shown as typed) and the password (masked), a cancelled question fails the command, and a background fetch
//! never opens a dialog. The server is a small Python program that checks Basic credentials and hands the request to
//! `git http-backend`. It runs in a separate process because the bridge is installed process-wide.

#![cfg(unix)]

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use host::askpass::{AskKind, AskRequest};

const USER: &str = "ayse";
const PASSWORD: &str = "s3cret";

const SERVER: &str = r#"
import base64, http.server, os, subprocess, sys
ROOT, USER, PASSWORD, BACKEND = sys.argv[1:5]
GOOD = "Basic " + base64.b64encode(f"{USER}:{PASSWORD}".encode()).decode()

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def serve(self):
        if self.headers.get("Authorization", "") != GOOD:
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="git"')
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        path, _, query = self.path.partition("?")
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        env = {"GIT_PROJECT_ROOT": ROOT, "GIT_HTTP_EXPORT_ALL": "1", "PATH_INFO": path, "QUERY_STRING": query,
               "REQUEST_METHOD": self.command, "CONTENT_TYPE": self.headers.get("Content-Type", ""),
               "CONTENT_LENGTH": str(len(body)), "REMOTE_USER": USER, "PATH": os.environ["PATH"]}
        out = subprocess.run([BACKEND], input=body, env=env, capture_output=True).stdout
        head, _, rest = out.partition(b"\r\n\r\n")
        status, headers = 200, []
        for line in head.split(b"\r\n"):
            key, _, value = line.partition(b": ")
            if key.lower() == b"status":
                status = int(value.split()[0])
            else:
                headers.append((key.decode(), value.decode()))
        self.send_response(status)
        for key, value in headers:
            self.send_header(key, value)
        self.send_header("Content-Length", str(len(rest)))
        self.end_headers()
        self.wfile.write(rest)

    do_GET = do_POST = serve

server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
"#;

struct Fixture {
    dir: PathBuf,
    server: Child,
    url: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git").args(args).current_dir(dir).status().is_ok_and(|s| s.success())
}

fn start() -> Option<Fixture> {
    let exec_path = Command::new("git").arg("--exec-path").output().ok()?;
    let backend = PathBuf::from(String::from_utf8_lossy(&exec_path.stdout).trim()).join("git-http-backend");
    if !backend.exists() || Command::new("python3").arg("--version").output().is_err() {
        return None;
    }
    let dir = std::fs::canonicalize({
        let dir = std::env::temp_dir().join(format!("codeyard-git-askpass-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("srv")).ok()?;
        dir
    })
    .ok()?;
    let work = dir.join("work");
    let bare = dir.join("srv/repo.git");
    std::fs::create_dir_all(&work).ok()?;
    let ok = git(&dir, &["init", "-q", "--bare", bare.to_str()?])
        && git(&work, &["init", "-q", "-b", "main"])
        && std::fs::write(work.join("f.txt"), "x\n").is_ok()
        && git(&work, &["add", "-A"])
        && git(&work, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "first"])
        && git(&work, &["push", "-q", bare.to_str()?, "main"]);
    if !ok {
        return None;
    }
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER).ok()?;
    let mut server = Command::new("python3")
        .arg(&script)
        .args([dir.join("srv").to_str()?, USER, PASSWORD, backend.to_str()?])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut port = String::new();
    BufReader::new(server.stdout.take()?).read_line(&mut port).ok()?;
    let url = format!("http://127.0.0.1:{}/repo.git", port.trim());
    Some(Fixture { dir, server, url })
}

#[derive(Default)]
struct Script {
    answer: bool,
    asked: Vec<AskRequest>,
}

#[test]
fn a_server_login_is_asked_in_the_window_and_the_answers_reach_git() {
    let Some(fx) = start() else {
        eprintln!("SKIPPED: python3 or git http-backend is missing");
        return;
    };
    // The client sits under a path with a space and a parenthesis, which git runs directly (no shell: quoting the path would break it).
    let spaced = fx.dir.join("Ali Veli (work)");
    std::fs::create_dir_all(&spaced).unwrap();
    let askpass_exe = spaced.join("codeyard");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_codeyard"), &askpass_exe).unwrap();
    // The login must not reach the real keychain: no system or user git configuration (osxkeychain would store it).
    let home = fx.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: a single test; no other thread in the process is reading environment variables yet.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        std::env::set_var("CODEYARD_ASKPASS_EXE", &askpass_exe);
    }
    let script = Arc::new(Mutex::new(Script { answer: true, ..Script::default() }));
    let s = script.clone();
    host::askpass::install(Arc::new(move |req: AskRequest| {
        let mut s = s.lock().unwrap();
        s.asked.push(req.clone());
        s.answer.then(|| match req.kind {
            AskKind::Text => USER.to_string(),
            _ => PASSWORD.to_string(),
        })
    }))
    .expect("askpass bridge");
    let host = host::local();
    let ls_remote = |h: &dyn host::Host| h.exec("git", &["ls-remote", &fx.url], None, None).expect("git ran");

    let out = ls_remote(&host);
    assert_eq!(out.code, Some(0), "stderr: {}", out.stderr_str());
    assert!(out.stdout_str().contains("refs/heads/main"), "{}", out.stdout_str());
    let asked = script.lock().unwrap().asked.clone();
    assert_eq!(
        asked.iter().map(|a| a.kind).collect::<Vec<_>>(),
        [AskKind::Text, AskKind::Secret],
        "the name first, shown as typed; then the password: {asked:?}"
    );
    assert!(asked[0].prompt.starts_with("Username for 'http://127.0.0.1:"), "{:?}", asked[0].prompt);

    // Cancelling the question fails the command: git is not left waiting.
    {
        let mut s = script.lock().unwrap();
        s.answer = false;
        s.asked.clear();
    }
    let out = ls_remote(&host);
    assert_ne!(out.code, Some(0), "a cancelled login must fail");
    assert_eq!(script.lock().unwrap().asked.len(), 1, "git asked for the name and stopped when it was declined");

    // While a background operation runs, the question is declined without reaching the window.
    {
        let mut s = script.lock().unwrap();
        s.answer = true;
        s.asked.clear();
    }
    let quiet = host::askpass::Quiet::begin();
    let out = ls_remote(&host);
    assert_ne!(out.code, Some(0), "a background fetch must not log in");
    assert!(script.lock().unwrap().asked.is_empty(), "the user was not asked");
    drop(quiet);
    assert_eq!(ls_remote(&host).code, Some(0), "asked again once the background operation is over");
}
