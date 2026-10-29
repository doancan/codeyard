use super::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

pub(crate) struct Fixture(PathBuf);
impl Fixture {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!("codeyard-managed-install-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub(crate) fn path(&self, tail: &str) -> String {
        self.0.join(tail).to_string_lossy().into_owned().replace('\\', "/")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Outcome {
    Success,
    Failure,
    Cancel,
    Disconnect,
}

pub(crate) struct FakeHost {
    inner: host::RemoteHost,
    info: proto::HostInfo,
    node: String,
    npm: String,
    outcome: Outcome,
    commands: Mutex<Vec<(String, Vec<String>, String)>>,
    pending: Mutex<Option<mpsc::Sender<ProcEvent>>>,
    started: AtomicBool,
    pub(crate) kill_count: AtomicU32,
}

impl FakeHost {
    pub(crate) fn install_starts(&self) -> usize {
        self.commands.lock().unwrap().iter().filter(|(_, args, _)| args.iter().any(|arg| arg == "ci")).count()
    }
    pub(crate) fn waiting_for_cancel(&self) -> bool {
        self.pending.lock().unwrap().is_some()
    }

    pub(crate) fn new(dir: &Fixture, outcome: Outcome) -> Arc<Self> {
        let inner = host::local();
        let mut info = inner.info().clone();
        info.home = dir.path("");
        let node = dir.path("node");
        let npm = dir.path("npm-cli.js");
        std::fs::write(&node, "fixture runtime").unwrap();
        std::fs::write(&npm, "fixture npm").unwrap();
        Arc::new(Self {
            inner,
            info,
            node,
            npm,
            outcome,
            commands: Mutex::default(),
            pending: Mutex::default(),
            started: AtomicBool::new(false),
            kill_count: AtomicU32::new(0),
        })
    }
}

impl Host for FakeHost {
    fn info(&self) -> &proto::HostInfo {
        &self.info
    }
    fn label(&self) -> String {
        "Owned installer fixture".into()
    }
    fn is_remote(&self) -> bool {
        true // Its owned home must be independent of the process's local profile override.
    }
    fn call(&self, req: proto::Request) -> host::Result<proto::Response> {
        if let proto::Request::ProcKill { proc } = req {
            self.proc_kill(proc);
            return Ok(proto::Response::Done);
        }
        self.inner.call(req)
    }
    fn proc_spawn(&self, program: &str, args: Vec<String>, cwd: Option<String>) -> host::Result<host::Proc> {
        let cwd = cwd.unwrap();
        self.commands.lock().unwrap().push((program.into(), args.clone(), cwd.clone()));
        let (tx, rx) = mpsc::channel();
        if program == "chmod" {
            assert!(self.inner.exec(program, &args.iter().map(String::as_str).collect::<Vec<_>>(), Some(&cwd), None)?.ok());
        } else if args == ["-e", NODE_PROBE] {
            assert_eq!(args, ["-e", NODE_PROBE]);
            tx.send(ProcEvent::Output(json!({"node":self.node,"version":"v22.23.4","npm":self.npm}).to_string().into_bytes())).unwrap();
        } else {
            assert_eq!(program, self.node);
            assert!(args.iter().any(|arg| arg == "ci"));
            for service in [Service::TypeScript, Service::Html, Service::Css] {
                let file = PathBuf::from(join(&cwd, service.entry()));
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file, "fixture entry").unwrap();
            }
            // Publication must not happen until verification has finished.
            if let Ok(active) = std::fs::read_to_string(PathBuf::from(&cwd).parent().unwrap().join("active.json")) {
                assert!(!active.contains(cwd.rsplit(".staging-").next().unwrap()));
            }
            self.started.store(true, Ordering::Release);
            match self.outcome {
                Outcome::Failure => {
                    tx.send(ProcEvent::Output("offline registry fixture".as_bytes().to_vec())).unwrap();
                    tx.send(ProcEvent::Exit(Some(1))).unwrap();
                    return Ok(host::Proc { id: 9, events: rx });
                }
                Outcome::Cancel => {
                    *self.pending.lock().unwrap() = Some(tx);
                    return Ok(host::Proc { id: 9, events: rx });
                }
                Outcome::Disconnect => {
                    tx.send(ProcEvent::Exit(None)).unwrap();
                    return Ok(host::Proc { id: 9, events: rx });
                }
                Outcome::Success => {}
            }
        }
        tx.send(ProcEvent::Exit(Some(0))).unwrap();
        Ok(host::Proc { id: 9, events: rx })
    }
    fn proc_kill(&self, proc: u32) {
        assert_eq!(proc, 9);
        self.kill_count.fetch_add(1, Ordering::Relaxed);
        if let Some(tx) = self.pending.lock().unwrap().take() {
            tx.send(ProcEvent::Exit(None)).unwrap(); // Killing removes the process before its output reader reports exit.
        }
    }
    fn proc_input(&self, _: u32, _: Vec<u8>) {
        panic!("installer does not send process input")
    }
    fn search(&self, _: proto::SearchQuery) -> host::Result<host::SearchStream> {
        panic!("not used")
    }
    fn hash_tree(&self, _: &str, _: Vec<String>, _: bool, _: u32, _: Option<Vec<String>>) -> host::Result<host::HashStream> {
        panic!("not used")
    }
    fn cancel(&self, _: u32) {
        panic!("not used")
    }
    fn pty_spawn(&self, _: u16, _: u16, _: Option<String>, _: Option<String>) -> host::Result<host::Pty> {
        panic!("installation never opens a shell")
    }
    fn pty_input(&self, _: u32, _: Vec<u8>) {
        panic!("not used")
    }
    fn pty_resize(&self, _: u32, _: u16, _: u16) {
        panic!("not used")
    }
    fn pty_kill(&self, _: u32) {
        panic!("not used")
    }
    fn watch(&self, _: Vec<String>) {
        panic!("not used")
    }
    fn fs_events(&self) -> mpsc::Receiver<host::FsChanges> {
        panic!("not used")
    }
    fn status_events(&self) -> mpsc::Receiver<host::ConnStatus> {
        panic!("not used")
    }
}

#[cfg(unix)]
#[test]
fn managed_install_real_process_cancellation_requires_the_kill_acknowledgement() {
    let fixture = Fixture::new();
    let host: Arc<dyn Host> = Arc::new(host::local());
    let progress = Arc::new(Progress::default());
    let worker = {
        let (host, progress, cwd) = (host.clone(), progress.clone(), fixture.path(""));
        std::thread::spawn(move || {
            run(host.as_ref(), "/bin/sh", vec!["-c".into(), "printf 'managed-cancel-ready:%s\\n' $$; exec /bin/sleep 30".into()], &cwd, &progress)
        })
    };
    let start = Instant::now();
    let pid = loop {
        if let Some(pid) = progress.snapshot().log.lines().find_map(|line| line.strip_prefix("managed-cancel-ready:").and_then(|pid| pid.parse::<u32>().ok())) {
            break pid;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    };
    progress.cancel.store(true, Ordering::Release);
    assert_eq!(worker.join().unwrap(), Err(Error::Cancelled));
    let alive = std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "the acknowledged cancellation must have reaped its process");
}

#[test]
fn managed_install_destination_respects_local_profiles_and_keeps_remote_paths_on_the_remote_host() {
    assert_eq!(destination_for("/home/owner", false, Some("/owned/profile")).unwrap(), "/owned/profile/language-services");
    assert_eq!(destination_for("/home/owner", true, Some("/owned/profile")).unwrap(), "/home/owner/.codeyard/language-services");
    assert_eq!(destination_for("C:/Users/owner", false, None).unwrap(), "C:/Users/owner/.codeyard/language-services");
    assert_eq!(destination_for("relative", true, None), Err(Error::InvalidInstallation));
}

#[test]
fn managed_install_publishes_a_complete_generation_without_using_workspace_or_global_config() {
    let dir = Fixture::new();
    let host = FakeHost::new(&dir, Outcome::Success);
    let root = dir.path("managed");
    let progress = Progress::default();
    assert_eq!(discover(host.as_ref(), &root).unwrap(), None);
    let installed = install(host.as_ref(), &root, &progress).unwrap();
    assert_eq!(progress.snapshot().stage, Stage::Ready);
    assert_eq!(discover(host.as_ref(), &root).unwrap(), Some(installed.clone()));
    assert!(!std::fs::read_dir(&root).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".staging-")));
    let commands = host.commands.lock().unwrap();
    let (_, args, cwd) = commands.last().unwrap();
    for flag in ["--ignore-scripts", "--no-audit", "--no-fund", "--registry=https://registry.npmjs.org"] {
        assert!(args.iter().any(|arg| arg == flag));
    }
    for name in ["userconfig", "globalconfig", "cache"] {
        assert!(args.iter().any(|arg| arg.starts_with(&format!("--{name}={cwd}/"))));
    }
    assert!(commands.iter().all(|(_, _, cwd)| cwd.starts_with(&root)));
    assert!(installed.command(Service::TypeScript).1[0].starts_with(&installed.directory));
}

#[test]
fn managed_install_reuses_a_verified_previous_runtime_and_retains_both_generations() {
    let fixture = Fixture::new();
    let host = FakeHost::new(&fixture, Outcome::Success);
    let root = fixture.path("managed");
    let first = install(host.as_ref(), &root, &Progress::default()).unwrap();
    let second = install(host.as_ref(), &root, &Progress::default()).unwrap();
    assert_ne!(first.directory, second.directory);
    assert_eq!(first.node, second.node);
    assert!(PathBuf::from(&first.directory).exists());
    assert_eq!(discover(host.as_ref(), &root).unwrap(), Some(second));
    assert_eq!(
        host.commands.lock().unwrap().iter().filter(|(program, _, _)| program == "node").count(),
        1,
        "a verified runtime should be reused before probing PATH again"
    );
}

#[test]
fn managed_install_failure_keeps_the_previous_active_record_and_removes_its_partial_files() {
    let dir = Fixture::new();
    let host = FakeHost::new(&dir, Outcome::Failure);
    let root = dir.path("managed");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(join(&root, "active.json"), "previous active generation").unwrap();
    let progress = Progress::default();
    assert!(matches!(install(host.as_ref(), &root, &progress), Err(Error::Failed(_))));
    assert!(progress.snapshot().log.contains("offline registry fixture"));
    assert_eq!(std::fs::read_to_string(join(&root, "active.json")).unwrap(), "previous active generation");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn managed_install_cancellation_waits_for_process_exit_before_cleaning_staging() {
    let dir = Fixture::new();
    let host = FakeHost::new(&dir, Outcome::Cancel);
    let root = dir.path("managed");
    let progress = Arc::new(Progress::default());
    let worker = {
        let host = host.clone();
        let progress = progress.clone();
        let root = root.clone();
        std::thread::spawn(move || install(host.as_ref(), &root, &progress))
    };
    let start = Instant::now();
    while !host.started.load(Ordering::Acquire) {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    progress.cancel.store(true, Ordering::Release);
    assert_eq!(worker.join().unwrap(), Err(Error::Cancelled));
    assert_eq!(host.kill_count.load(Ordering::Relaxed), 1);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn managed_install_disconnect_keeps_staging_and_never_publishes() {
    let dir = Fixture::new();
    let host = FakeHost::new(&dir, Outcome::Disconnect);
    let root = dir.path("managed");
    assert_eq!(install(host.as_ref(), &root, &Progress::default()), Err(Error::Uncertain));
    assert!(!PathBuf::from(join(&root, "active.json")).exists());
    let entries: Vec<_> = std::fs::read_dir(&root).unwrap().map(|entry| entry.unwrap().file_name()).collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].to_string_lossy().starts_with(".staging-"));
}

#[test]
fn managed_install_rejects_unsafe_records_and_bounds_unicode_logs() {
    let dir = Fixture::new();
    let host = FakeHost::new(&dir, Outcome::Success);
    let root = dir.path("managed");
    std::fs::create_dir(&root).unwrap();
    for text in ["{", &"x".repeat(16385), "{\"version\":1,\"generation\":\"../victim\"}"] {
        std::fs::write(join(&root, "active.json"), text).unwrap();
        assert_eq!(discover(host.as_ref(), &root), Err(Error::InvalidInstallation));
    }
    let progress = Progress::default();
    progress.log("ş".repeat(LOG_LIMIT).as_bytes());
    assert!(progress.snapshot().log.len() <= LOG_LIMIT);
    assert!(std::str::from_utf8(progress.snapshot().log.as_bytes()).is_ok());
    progress.cancel.store(true, Ordering::Release);
    assert_eq!(install(host.as_ref(), &root, &progress), Err(Error::Cancelled));
    assert!(host.commands.lock().unwrap().is_empty());
}
