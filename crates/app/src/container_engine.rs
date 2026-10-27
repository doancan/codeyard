//! Container runtimes are separate adapters: an OCI image does not imply Docker CLI compatibility.
use crate::docker::{Command, Container};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Engine {
    #[default]
    Docker,
    Apple,
    Wslc,
    Podman,
}

#[derive(Clone, Debug)]
pub struct Choice {
    pub engine: Engine,
    pub installed: bool,
}

impl Engine {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "docker" => Some(Self::Docker),
            "apple" => Some(Self::Apple),
            "wslc" => Some(Self::Wslc),
            "podman" => Some(Self::Podman),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Apple => "apple",
            Self::Wslc => "wslc",
            Self::Podman => "podman",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Docker => "Docker",
            Self::Apple => "Apple Container",
            Self::Wslc => "WSL Containers",
            Self::Podman => "Podman",
        }
    }
    pub fn program(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Apple => "container",
            Self::Wslc => "wslc.exe",
            Self::Podman => "podman",
        }
    }
    pub fn setup_url(self) -> &'static str {
        match self {
            Self::Docker => "https://docs.docker.com/get-started/get-docker/",
            Self::Apple => "https://github.com/apple/container#requirements",
            Self::Wslc => "https://learn.microsoft.com/windows/wsl/wsl-container",
            Self::Podman => "https://podman.io/docs/installation",
        }
    }
    pub fn candidates(os: &str) -> Vec<Self> {
        match os {
            "macos" => vec![Self::Apple, Self::Docker, Self::Podman],
            "windows" => vec![Self::Wslc, Self::Docker, Self::Podman],
            _ => vec![Self::Podman, Self::Docker],
        }
    }
    pub fn discover() -> Vec<Choice> {
        Self::detect(std::env::consts::OS, |engine| host::find_program(engine.program()).is_some())
    }
    pub fn detect(os: &str, mut installed: impl FnMut(Engine) -> bool) -> Vec<Choice> {
        Self::candidates(os).into_iter().map(|engine| Choice { engine, installed: installed(engine) }).collect()
    }
    pub fn command(self, context: &str, args: &[&str]) -> Command {
        if self == Self::Docker {
            return Command::docker(Some(context), args);
        }
        let mut command = Command::docker(None, args);
        command.program = self.program().into();
        command
    }
    pub fn list(self, context: &str) -> Command {
        match self {
            Self::Docker => self.command(context, &["ps", "-a", "--no-trunc", "--format", "{{json .}}"]),
            Self::Apple => self.command(context, &["list", "--all", "--format", "json"]),
            Self::Wslc => self.command(context, &["container", "list", "--all", "--no-trunc", "--format", "json"]),
            Self::Podman => self.command(context, &["ps", "--all", "--no-trunc", "--format", "json"]),
        }
    }
    pub fn logs(self, context: &str, id: &str) -> Command {
        if self == Self::Apple { self.command(context, &["logs", "-n", "200", id]) } else { self.command(context, &["logs", "--tail", "200", id]) }
    }
    pub fn alias(self, context: &str, id: &str) -> Result<String, String> {
        crate::docker::target("default", id, None)?;
        if self == Self::Docker { crate::docker::target(context, id, None) } else { Ok(format!("container-runtime:{}/{id}", self.id())) }
    }
    pub fn containers(self, text: &str) -> Result<Vec<Container>, String> {
        if self == Self::Docker || self == Self::Wslc {
            let mut containers: Vec<Container> = crate::docker::json_lines(text)?;
            for container in &mut containers {
                container.state = container.state.to_lowercase();
                if container.state == "stopped" {
                    container.state = "exited".into();
                }
            }
            return Ok(containers);
        }
        let rows: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let rows = rows.as_array().ok_or("Expected a JSON container array")?;
        rows.iter()
            .map(|row| {
                let string = |value: &Value| value.as_str().unwrap_or_default().to_string();
                let (id, name, image, state, status, ports) = if self == Self::Apple {
                    let id =
                        row.pointer("/configuration/id").or_else(|| row.get("id")).and_then(Value::as_str).ok_or("Missing Apple container id")?.to_string();
                    let image = string(&row["configuration"]["image"]["reference"]);
                    let state = row["status"]["state"].as_str().or(row["status"].as_str()).ok_or("Missing Apple container state")?.to_lowercase();
                    (id.clone(), id, image, state.clone(), state, String::new())
                } else {
                    let id = row.get("Id").or_else(|| row.get("ID")).and_then(Value::as_str).ok_or("Missing Podman container id")?.to_string();
                    let name = row["Names"].as_array().and_then(|n| n.first()).and_then(Value::as_str).or(row["Names"].as_str()).unwrap_or(&id).to_string();
                    let state = string(&row["State"]).to_lowercase();
                    let ports = row.get("Ports").filter(|p| !p.is_null()).map(Value::to_string).unwrap_or_default();
                    (id, name, string(&row["Image"]), state, string(&row["Status"]), ports)
                };
                let state = if state == "stopped" { "exited".into() } else { state };
                Ok(Container { id, name, image, state, status, ports, labels: String::new() })
            })
            .collect()
    }
}
