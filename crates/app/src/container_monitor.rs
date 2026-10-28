//! Bounded resource snapshots over the existing SSH agent, without a second SSH login.
use crate::docker::{Container, DockerContext};
use serde::Deserialize;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Usage {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "CPUPerc")]
    pub cpu: String,
    #[serde(rename = "MemPerc")]
    pub memory_percent: String,
    #[serde(rename = "MemUsage")]
    pub memory: String,
    #[serde(rename = "BlockIO")]
    pub disk_io: String,
    #[serde(rename = "NetIO")]
    pub network: String,
    #[serde(rename = "PIDs")]
    pub pids: String,
}

impl Usage {
    pub fn percent(text: &str) -> Option<f32> {
        text.trim().trim_end_matches('%').parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Disk {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Size")]
    pub size: String,
}

pub struct Snapshot {
    pub context: DockerContext,
    pub containers: Vec<Container>,
    pub usage: Vec<Usage>,
    pub disks: Option<Vec<Disk>>,
    pub warning: Option<String>,
}

#[derive(Default)]
pub struct Monitor {
    pub return_workspace: Option<ReturnWorkspace>,
    pub browser: crate::container_history::Browser,
    pub history: std::collections::HashMap<String, std::collections::VecDeque<crate::container_history::Sample>>,
    pub host: Option<String>,
    pub source: Option<Arc<dyn host::Host>>,
    pub context: Option<DockerContext>,
    pub containers: Vec<Container>,
    pub usage: Vec<Usage>,
    pub disks: Vec<Disk>,
    pub error: Option<String>,
    pub warning: Option<String>,
    pub updated: Option<Instant>,
    pub disk_updated: Option<Instant>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub generation: u64,
    pub paused: bool,
}

pub struct ReturnWorkspace {
    pub host: Arc<dyn host::Host>,
    pub alias: String,
    pub root: Option<String>,
}

impl Monitor {
    pub fn clear(&mut self) {
        if let Some(cancel) = self.browser.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        let generation = self.generation + 1;
        let request = self.browser.request + 1;
        *self = Self::default();
        self.generation = generation;
        self.browser.request = request;
    }
    pub fn apply(&mut self, result: Result<Snapshot, String>) {
        self.cancel = None;
        match result {
            Ok(snapshot) => {
                let now = Instant::now();
                if self.context.as_ref().is_some_and(|c| c.name != snapshot.context.name) {
                    self.history.clear();
                    self.disks.clear();
                    self.disk_updated = None;
                    self.browser.selected = None;
                    self.browser.overview.details = false;
                }
                self.history.retain(|id, _| snapshot.containers.iter().any(|c| &c.id == id));
                for usage in &snapshot.usage {
                    let history = self.history.entry(usage.id.clone()).or_default();
                    let sample = crate::container_history::Sample::new(now, usage, history.back());
                    history.push_back(sample);
                    while history.len() > 360 || history.front().is_some_and(|s| now.duration_since(s.at) > Duration::from_secs(900)) {
                        history.pop_front();
                    }
                }
                if self.browser.selected.as_ref().is_some_and(|id| !snapshot.containers.iter().any(|c| &c.id == id)) {
                    self.browser.selected = None;
                    self.browser.overview.details = false;
                }
                self.context = Some(snapshot.context);
                self.containers = snapshot.containers;
                self.usage = snapshot.usage;
                if let Some(disks) = snapshot.disks {
                    self.disks = disks;
                    self.disk_updated = Some(Instant::now());
                }
                self.warning = snapshot.warning;
                self.error = None;
                self.updated = Some(Instant::now());
            }
            Err(error) => {
                self.error = Some(error);
            }
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if let Some(cancel) = &self.browser.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

pub fn collect(process: host::Proc, cancel: &AtomicBool, timeout: Duration, kill: impl Fn(u32)) -> Result<String, String> {
    let start = Instant::now();
    let mut bytes = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() >= timeout {
            kill(process.id);
            return Err(if cancel.load(Ordering::Relaxed) { "Operation cancelled" } else { "Container discovery timed out" }.into());
        }
        match process.events.recv_timeout(Duration::from_millis(100)) {
            Ok(host::ProcEvent::Output(data)) => {
                if bytes.len() + data.len() > 8 * 1024 * 1024 {
                    kill(process.id);
                    return Err("Container output exceeds 8 MB".into());
                }
                bytes.extend(data);
            }
            Ok(host::ProcEvent::Exit(Some(0))) => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Ok(host::ProcEvent::Exit(_)) => return Err(String::from_utf8_lossy(&bytes).trim().to_string()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("Container process disconnected".into()),
        }
    }
}

fn run(source: &dyn host::Host, cancel: &AtomicBool, args: &[&str]) -> Result<String, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Operation cancelled".into());
    }
    let mut all = vec!["-c".into(), "exec 2>&1; exec docker \"$@\"".into(), "codeyard-containers".into()];
    all.extend(args.iter().map(|s| s.to_string()));
    let process = source.proc_spawn("sh", all, None).map_err(|e| e.to_string())?;
    collect(process, cancel, Duration::from_secs(20), |id| source.proc_kill(id))
}

pub fn execute(source: &dyn host::Host, command: &crate::docker::Command, cancel: &AtomicBool) -> Result<String, String> {
    run(source, cancel, &command.args.iter().map(String::as_str).collect::<Vec<_>>())
}

pub fn read(source: &dyn host::Host, ssh: &str, disk: bool, cancel: &AtomicBool) -> Result<Snapshot, String> {
    let context = run(source, cancel, &["context", "show"])?;
    let context = context.trim();
    // Validate before inserting server-returned data into a connection identity.
    crate::docker::target(context, "validation", None)?;
    let containers = crate::docker::json_lines(&run(source, cancel, &["--context", context, "ps", "-a", "--no-trunc", "--format", "{{json .}}"])?)?;
    let mut warning = None;
    let mut usage = Vec::new();
    if containers.iter().any(|c: &Container| c.state == "running") {
        match run(source, cancel, &["--context", context, "stats", "--no-stream", "--no-trunc", "--format", "{{json .}}"])
            .and_then(|s| crate::docker::json_lines::<Usage>(&s))
        {
            Ok(rows) => usage = rows,
            Err(e) => warning = Some(e),
        }
    }
    let disks = if disk {
        match run(source, cancel, &["--context", context, "ps", "-a", "--size", "--no-trunc", "--format", "{{json .}}"])
            .and_then(|s| crate::docker::json_lines::<Disk>(&s))
        {
            Ok(rows) => Some(rows),
            Err(e) => {
                warning = Some(e);
                None
            }
        }
    } else {
        None
    };
    Ok(Snapshot {
        context: DockerContext {
            engine: crate::container_engine::Engine::Docker,
            ssh: Some(ssh.into()),
            name: context.into(),
            endpoint: format!("SSH: {ssh}"),
        },
        containers,
        usage,
        disks,
        warning,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use host::Host;

    pub struct FixtureHost {
        inner: Arc<dyn Host>,
        pub calls: std::sync::atomic::AtomicUsize,
        pub fail_stats: AtomicBool,
    }
    impl FixtureHost {
        pub fn new() -> Arc<Self> {
            Arc::new(Self { inner: Arc::new(host::local()), calls: std::sync::atomic::AtomicUsize::new(0), fail_stats: AtomicBool::new(false) })
        }
    }
    impl Host for FixtureHost {
        fn info(&self) -> &proto::HostInfo {
            self.inner.info()
        }
        fn label(&self) -> String {
            "ssh://fixture".into()
        }
        fn is_remote(&self) -> bool {
            true
        }
        fn call(&self, r: proto::Request) -> host::Result<proto::Response> {
            self.inner.call(r)
        }
        fn search(&self, q: proto::SearchQuery) -> host::Result<host::SearchStream> {
            self.inner.search(q)
        }
        fn hash_tree(&self, r: &str, i: Vec<String>, g: bool, l: u32, o: Option<Vec<String>>) -> host::Result<host::HashStream> {
            self.inner.hash_tree(r, i, g, l, o)
        }
        fn cancel(&self, id: u32) {
            self.inner.cancel(id)
        }
        fn pty_spawn(&self, c: u16, r: u16, d: Option<String>, s: Option<String>) -> host::Result<host::Pty> {
            self.inner.pty_spawn(c, r, d, s)
        }
        fn pty_input(&self, id: u32, d: Vec<u8>) {
            self.inner.pty_input(id, d)
        }
        fn pty_resize(&self, id: u32, c: u16, r: u16) {
            self.inner.pty_resize(id, c, r)
        }
        fn pty_kill(&self, id: u32) {
            self.inner.pty_kill(id)
        }
        fn watch(&self, p: Vec<String>) {
            self.inner.watch(p)
        }
        fn fs_events(&self) -> std::sync::mpsc::Receiver<host::FsChanges> {
            self.inner.fs_events()
        }
        fn status_events(&self) -> std::sync::mpsc::Receiver<host::ConnStatus> {
            self.inner.status_events()
        }
        fn proc_input(&self, _: u32, _: Vec<u8>) {}
        fn proc_kill(&self, _: u32) {}
        fn proc_spawn(&self, program: &str, args: Vec<String>, _: Option<String>) -> host::Result<host::Proc> {
            assert_eq!(program, "sh");
            assert!(args[1].contains("exec docker"));
            self.calls.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = std::sync::mpsc::channel();
            let stats = args.iter().any(|a| a == "stats");
            let fail = stats && self.fail_stats.load(Ordering::Relaxed);
            let text = if args.last().is_some_and(|s| s == "show") {
                "default\n"
            } else if fail {
                "permission denied"
            } else if stats {
                r#"{"ID":"web-id","CPUPerc":"145.6%","MemPerc":"12.5%","MemUsage":"128MiB / 1GiB","BlockIO":"1MB / 2MB","NetIO":"3MB / 4MB","PIDs":"12"}"#
            } else if args.iter().any(|a| a == "inspect") {
                r#"[{"Mounts":[{"Source":"/data","Destination":"/app/data","Type":"bind"}],"State":{"Health":{"Status":"healthy"}}}]"#
            } else if args.iter().any(|a| a == "logs") {
                "2026-10-27T12:00:00Z ready"
            } else if args.iter().any(|a| a == "--size") {
                r#"{"ID":"web-id","Size":"12MB (virtual 200MB)"}"#
            } else {
                r#"{"ID":"web-id","Names":"web","Image":"nginx:latest","Labels":"com.docker.compose.project=shop","State":"running","Status":"Up 3 hours"}
{"ID":"db-id","Names":"database","Image":"postgres","State":"exited","Status":"Exited (0)"}"#
            };
            tx.send(host::ProcEvent::Output(text.as_bytes().to_vec())).unwrap();
            tx.send(host::ProcEvent::Exit(Some(if fail { 1 } else { 0 }))).unwrap();
            Ok(host::Proc { id: 42, events: rx })
        }
    }

    #[test]
    fn container_monitor_discovers_resources_over_existing_agent_and_keeps_inventory_on_stats_failure() {
        let source = FixtureHost::new();
        let cancel = AtomicBool::new(false);
        let snapshot = read(source.as_ref(), "linux", true, &cancel).unwrap();
        assert_eq!(snapshot.containers.len(), 2);
        assert_eq!(snapshot.usage[0].cpu, "145.6%");
        assert_eq!(snapshot.disks.unwrap()[0].size, "12MB (virtual 200MB)");
        assert_eq!(source.calls.load(Ordering::Relaxed), 4);
        source.fail_stats.store(true, Ordering::Relaxed);
        let snapshot = read(source.as_ref(), "linux", false, &cancel).unwrap();
        assert_eq!(snapshot.containers.len(), 2);
        assert!(snapshot.usage.is_empty());
        assert!(snapshot.warning.unwrap().contains("permission denied"));
        assert!(snapshot.disks.is_none());
    }
    #[test]
    fn container_monitor_bounds_and_cancels_remote_processes() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(host::ProcEvent::Output(b"permission denied".to_vec())).unwrap();
        tx.send(host::ProcEvent::Exit(Some(1))).unwrap();
        assert_eq!(collect(host::Proc { id: 7, events: rx }, &AtomicBool::new(false), Duration::from_secs(1), |_| {}).unwrap_err(), "permission denied");
        let (_tx, rx) = std::sync::mpsc::channel();
        let killed = std::cell::Cell::new(false);
        assert!(
            collect(host::Proc { id: 7, events: rx }, &AtomicBool::new(true), Duration::from_secs(1), |id| {
                assert_eq!(id, 7);
                killed.set(true);
            })
            .is_err()
        );
        assert!(killed.get());
    }
    #[test]
    fn container_monitor_keeps_multicore_cpu_and_distinguishes_disk_io() {
        let usage: Usage =
            serde_json::from_str(r#"{"ID":"web","CPUPerc":"254.8%","MemPerc":"12.5%","MemUsage":"125MiB / 1GiB","BlockIO":"1MB / 2MB"}"#).unwrap();
        assert_eq!(Usage::percent(&usage.cpu), Some(254.8));
        assert_eq!(usage.disk_io, "1MB / 2MB");
        assert!(Usage::percent("NaN%").is_none());
        assert!(Usage::percent("--").is_none());
    }
}
