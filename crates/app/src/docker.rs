//! Optional Docker module. Discovery is explicit, commands pin their Docker context, and
//! Dev Container compatibility is delegated to the separately installed reference CLI.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
pub struct DockerContext {
    #[serde(skip)]
    pub engine: crate::container_engine::Engine,
    #[serde(skip)]
    pub ssh: Option<String>,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "DockerEndpoint", default)]
    pub endpoint: String,
}

impl DockerContext {
    pub fn route(&self, mut command: Command) -> Command {
        command.ssh = self.ssh.clone();
        command
    }
    pub fn container_alias(&self, id: &str) -> Result<String, String> {
        if let Some(host) = &self.ssh {
            host::ssh::ssh_docker_target(host, &self.name, id).map_err(|e| e.to_string())
        } else {
            self.engine.alias(&self.name, id)
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Container {
    #[serde(rename = "Labels", default)]
    pub labels: String,
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Names")]
    pub name: String,
    #[serde(rename = "Image")]
    pub image: String,
    #[serde(rename = "State")]
    pub state: String,
    #[serde(rename = "Status", default)]
    pub status: String,
    #[serde(rename = "Ports", default)]
    pub ports: String,
}

pub fn is_target(alias: &str) -> bool {
    alias.starts_with("docker:")
        || alias.starts_with("ssh-docker:")
        || alias.starts_with("docker-context:")
        || alias.starts_with("devcontainer:")
        || alias.starts_with("container-runtime:")
        || alias.starts_with("devcontainer-runtime:")
}

pub fn target(context: &str, container: &str, user: Option<&str>) -> Result<String, String> {
    for part in [Some(context), Some(container), user].into_iter().flatten() {
        if part.is_empty() || part.starts_with('-') || !part.chars().all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c)) {
            return Err("Invalid Docker context, container or user".into());
        }
    }
    Ok(format!("docker-context:{context}/{container}{}", user.map(|u| format!("/{u}")).unwrap_or_default()))
}

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub config: PathBuf,
    pub name: String,
    pub summary: String,
    pub compose: bool,
}

impl Project {
    pub fn runtime_alias(&self, environment: &DockerContext) -> String {
        if environment.engine == crate::container_engine::Engine::Docker {
            return self.alias(&environment.name);
        }
        let base = self.alias(environment.engine.id());
        base.replacen("devcontainer:", "devcontainer-runtime:", 1)
    }

    pub fn runtime_command(&self, environment: &DockerContext, rebuild: bool) -> Command {
        let mut command = self.command(&environment.name, rebuild);
        if environment.engine != crate::container_engine::Engine::Docker {
            command.context = None;
            command.args.extend([
                "--docker-path".into(),
                host::find_program(environment.engine.program())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| environment.engine.program().into()),
            ]);
            if self.compose && environment.engine == crate::container_engine::Engine::Podman {
                command.args.extend([
                    "--docker-compose-path".into(),
                    host::find_program("podman-compose").map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "podman-compose".into()),
                ]);
            }
        }
        command
    }
    pub fn alias(&self, context: &str) -> String {
        let encode = |p: &Path| p.to_string_lossy().as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>();
        format!("devcontainer:{context}/{}/{}", encode(&self.root), encode(&self.config))
    }
    pub fn discover(root: &Path) -> Result<Self, String> {
        let root = PathBuf::from(crate::ui::local_path(&std::fs::canonicalize(root).map_err(|e| e.to_string())?));
        let config = [root.join(".devcontainer/devcontainer.json"), root.join(".devcontainer.json")]
            .into_iter()
            .find(|p| p.is_file())
            .ok_or_else(|| "No .devcontainer/devcontainer.json or .devcontainer.json found".to_string())?;
        if std::fs::metadata(&config).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
            return Err("Dev Container configuration exceeds 1 MB".into());
        }
        let data = std::fs::read(&config).map_err(|e| e.to_string())?;
        let source = String::from_utf8(data).map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_str(&theme::themes::strip_jsonc(&source)).map_err(|e| e.to_string())?;
        if !value.is_object() {
            return Err("Dev Container configuration must be an object".into());
        }
        let mode = if let Some(image) = value["image"].as_str() {
            crate::i18n::tf("Image: {}", &[&image])
        } else if !value["dockerComposeFile"].is_null() {
            format!("Compose: {} · {}", value["dockerComposeFile"], value["service"].as_str().unwrap_or("?")) // i18n-keep: Compose is the configuration format.
        } else if !value["build"].is_null() || !value["dockerFile"].is_null() {
            "Dockerfile".to_string()
        } else {
            return Err("Configuration needs image, build or dockerComposeFile".into());
        };
        let hooks: Vec<_> = ["initializeCommand", "onCreateCommand", "updateContentCommand", "postCreateCommand", "postStartCommand", "postAttachCommand"]
            .into_iter()
            .filter(|k| !value[*k].is_null())
            .collect();
        let summary = [
            mode,
            crate::i18n::tf("User: {}", &[&value["remoteUser"].as_str().or(value["containerUser"].as_str()).unwrap_or(crate::i18n::t("image default"))]),
            crate::i18n::tf("Ports: {}", &[&value.get("forwardPorts").unwrap_or(&Value::Array(Vec::new()))]),
            crate::i18n::tf("Mounts: {}", &[&value.get("mounts").and_then(Value::as_array).map_or(0, Vec::len)]),
            crate::i18n::tf("Lifecycle: {}", &[&hooks.join(", ")]),
        ]
        .join("\n");
        let name = value["name"].as_str().unwrap_or("Dev Container").to_string(); // i18n-keep: Dev Container is the specification's name.
        let compose = !value["dockerComposeFile"].is_null();
        Ok(Self { root, config, name, summary, compose })
    }

    pub fn command(&self, context: &str, rebuild: bool) -> Command {
        let mut args = vec![
            "up".into(),
            "--workspace-folder".into(),
            self.root.to_string_lossy().into_owned(),
            "--config".into(),
            self.config.to_string_lossy().into_owned(),
            "--log-format".into(),
            "json".into(),
        ];
        if rebuild {
            args.push("--remove-existing-container".into());
        }
        Command {
            program: "devcontainer".into(),
            args,
            context: Some(context.into()),
            ssh: None,
            cwd: Some(self.root.clone()),
            timeout: Duration::from_secs(1800),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub context: Option<String>,
    pub ssh: Option<String>,
    pub cwd: Option<PathBuf>,
    pub timeout: Duration,
}

impl Command {
    pub fn docker(context: Option<&str>, args: &[&str]) -> Self {
        let mut all = Vec::new();
        if let Some(context) = context {
            all.extend(["--context".into(), context.into()]);
        }
        all.extend(args.iter().map(|s| s.to_string()));
        Self { program: "docker".into(), args: all, context: None, ssh: None, cwd: None, timeout: Duration::from_secs(30) }
    }
}

pub fn json_lines<T: for<'a> Deserialize<'a>>(text: &str) -> Result<Vec<T>, String> {
    text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).map_err(|e| e.to_string())).collect()
}

#[derive(Debug)]
pub struct Opened {
    pub container: String,
    pub folder: String,
}

pub fn opened(text: &str) -> Result<Opened, String> {
    let value = text
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v.get("outcome").is_some())
        .ok_or_else(|| "Dev Container CLI did not return a result".to_string())?;
    if value["outcome"] != "success" {
        return Err(value["description"].as_str().unwrap_or("Dev Container creation failed").to_string());
    }
    let container = value["containerId"].as_str().ok_or("Missing containerId")?.to_string();
    let folder = value["remoteWorkspaceFolder"]
        .as_str()
        .filter(|s| s.starts_with('/') && !s.contains(['\0', '\r', '\n']))
        .ok_or("Missing remoteWorkspaceFolder")?
        .to_string();
    Ok(Opened { container, folder })
}

/// Both streams are drained concurrently. Output and queue sizes are bounded; cancellation
/// terminates the process tree. Docker resources are deliberately left for inspection.
pub fn run(command: &Command, cancel: &AtomicBool, mut log: impl FnMut(String)) -> Result<String, String> {
    use std::process::Stdio;
    if cancel.load(Ordering::Relaxed) {
        return Err("Operation cancelled".into());
    }
    let mut cmd = if let Some(ssh) = &command.ssh {
        let quote = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
        let script = std::iter::once(&command.program).chain(&command.args).map(|s| quote(s)).collect::<Vec<_>>().join(" ");
        host::ssh::script_command(ssh, &format!("exec {script}")).map_err(|e| e.to_string())?
    } else {
        host::process_command(&command.program, &command.args, command.cwd.as_ref().and_then(|p| p.to_str())).map_err(|e| e.to_string())?
    };
    if let Some(context) = &command.context {
        cmd.env("DOCKER_CONTEXT", context).env_remove("DOCKER_HOST");
    } else if command.program == "devcontainer" && command.args.iter().any(|a| a == "--docker-path") {
        cmd.env_remove("DOCKER_CONTEXT").env_remove("DOCKER_HOST");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("{}: {e}", command.program))?;
    let (tx, rx) = mpsc::sync_channel(64);
    fn pump(mut read: impl Read + Send + 'static, stdout: bool, tx: mpsc::SyncSender<(bool, Vec<u8>)>) {
        std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(n) = read.read(&mut bytes) {
                if n == 0 || tx.send((stdout, bytes[..n].to_vec())).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(stdout) = child.stdout.take() {
        pump(stdout, true, tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        pump(stderr, false, tx.clone());
    }
    drop(tx);
    let start = Instant::now();
    let mut stdout = Vec::new();
    let mut tail = String::new();
    let mut pending = Vec::new();
    let mut exited = None;
    let mut exit_at = None;
    loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() > command.timeout {
            terminate(&mut child);
            return Err(if cancel.load(Ordering::Relaxed) { "Operation cancelled" } else { "Operation timed out" }.into());
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok((is_stdout, bytes)) => {
                if is_stdout {
                    if stdout.len() + bytes.len() > 8 * 1024 * 1024 {
                        terminate(&mut child);
                        return Err("Command output exceeds 8 MB".into());
                    }
                    stdout.extend_from_slice(&bytes);
                } else {
                    pending.extend_from_slice(&bytes);
                    while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                        let line = String::from_utf8_lossy(&pending[..end]).into_owned();
                        pending.drain(..=end);
                        tail = line.clone();
                        log(line);
                    }
                    if pending.len() > 16384 {
                        log(String::from_utf8_lossy(&pending).into_owned());
                        pending.clear();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if exited.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {}
        }
        if exited.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    exited = Some(status);
                    exit_at = Some(Instant::now());
                }
                Ok(None) => {}
                Err(e) => {
                    terminate(&mut child);
                    return Err(e.to_string());
                }
            }
        }
        if exit_at.is_some_and(|at| at.elapsed() > Duration::from_secs(1)) {
            break;
        }
    }
    if !pending.is_empty() {
        tail = String::from_utf8_lossy(&pending).into_owned();
        log(tail.clone());
    }
    if exited.is_some_and(|s| s.success()) {
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    } else {
        if tail.is_empty() {
            tail = String::from_utf8_lossy(&stdout).lines().last().unwrap_or_default().chars().take(4096).collect();
        }
        Err(format!("{} failed: {tail}", command.program))
    }
}

fn terminate(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill").args(["/PID", &child.id().to_string(), "/T", "/F"]).creation_flags(0x08000000).output();
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[derive(Default)]
pub struct State {
    pub monitor: crate::container_monitor::Monitor,
    pub cancel: Option<Arc<AtomicBool>>,
    pub generation: u64,
    pub project: Option<(Project, DockerContext)>,
}

impl State {
    pub fn cancel(&mut self) {
        self.monitor.clear();
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.generation += 1;
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}
