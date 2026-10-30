//! #146 AC1: a native program from a package runs on the workspace's host from the pushed copy, after its hash is
//! checked there; locally and over SSH (this machine presented as a remote host, as the other remote tests do).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use ext_api::value::{self as v, Completion, Start, StreamEvent};
use ext_manifest::Permissions;
use host::Host;

use super::*;
use crate::ext_broker::{Scope, Sink};
use crate::ext_programs;

/// This machine with its home in a test folder, presented as local or as an SSH server. Records the scripts it runs.
struct Fixture {
    local: Arc<dyn Host>,
    info: proto::HostInfo,
    remote: bool,
    execs: Mutex<Vec<String>>,
}

impl Fixture {
    fn new(home: &Path, remote: bool) -> Arc<Fixture> {
        let local: Arc<dyn Host> = Arc::new(host::local());
        let mut info = local.info().clone();
        info.home = home.to_string_lossy().replace('\\', "/");
        Arc::new(Fixture { local, info, remote, execs: Mutex::default() })
    }

    fn scripts(&self) -> Vec<String> {
        self.execs.lock().unwrap().clone()
    }
}

impl Host for Fixture {
    fn info(&self) -> &proto::HostInfo {
        &self.info
    }
    fn label(&self) -> String {
        if self.remote { "ssh://fixture".into() } else { "Local".into() }
    }
    fn is_remote(&self) -> bool {
        self.remote
    }
    fn call(&self, request: proto::Request) -> host::Result<proto::Response> {
        if let proto::Request::Exec { args, .. } = &request {
            self.execs.lock().unwrap().push(args.get(1).cloned().unwrap_or_default());
        }
        self.local.call(request)
    }
    fn search(&self, q: proto::SearchQuery) -> host::Result<host::SearchStream> {
        self.local.search(q)
    }
    fn hash_tree(&self, r: &str, i: Vec<String>, g: bool, l: u32, o: Option<Vec<String>>) -> host::Result<host::HashStream> {
        self.local.hash_tree(r, i, g, l, o)
    }
    fn cancel(&self, id: u32) {
        self.local.cancel(id)
    }
    fn pty_spawn(&self, c: u16, r: u16, d: Option<String>, s: Option<String>) -> host::Result<host::Pty> {
        self.local.pty_spawn(c, r, d, s)
    }
    fn pty_input(&self, p: u32, data: Vec<u8>) {
        self.local.pty_input(p, data)
    }
    fn pty_resize(&self, p: u32, c: u16, r: u16) {
        self.local.pty_resize(p, c, r)
    }
    fn pty_kill(&self, p: u32) {
        self.local.pty_kill(p)
    }
    fn watch(&self, paths: Vec<String>) {
        self.local.watch(paths)
    }
    fn fs_events(&self) -> std::sync::mpsc::Receiver<host::FsChanges> {
        self.local.fs_events()
    }
    fn status_events(&self) -> std::sync::mpsc::Receiver<host::ConnStatus> {
        self.local.status_events()
    }
    fn proc_spawn(&self, p: &str, a: Vec<String>, c: Option<String>) -> host::Result<host::Proc> {
        self.local.proc_spawn(p, a, c)
    }
    fn proc_input(&self, p: u32, data: Vec<u8>) {
        self.local.proc_input(p, data)
    }
    fn proc_kill(&self, p: u32) {
        self.local.proc_kill(p)
    }
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("ext-programs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("home")).unwrap();
        std::fs::create_dir_all(d.join("work")).unwrap();
        Dir(std::fs::canonicalize(&d).unwrap())
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Collect(Mutex<Vec<u8>>);

impl Sink for Collect {
    fn event(&self, e: StreamEvent) {
        if let StreamEvent::Stdout(b) = e {
            self.0.lock().unwrap().extend(b);
        }
    }
}

/// The package for this machine's platform, unpacked as the store holds it.
fn unpacked(d: &Dir, target: &str) -> PathBuf {
    let pkg = ext_pkg::read(&ext_core::testing::native_package("you.hello", "1.0.0", target)).unwrap();
    let dir = d.0.join("store");
    for e in pkg.files.values() {
        let path = dir.join(&e.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &e.data).unwrap();
    }
    dir
}

fn scope(work: &str, store: PathBuf) -> Scope {
    Scope {
        extension: "you.hello".into(),
        permissions: Permissions { run: vec!["./bin/hello".into()], ..Default::default() },
        roots: vec![work.to_string()],
        trusted: true,
        dependencies: vec![],
        revoked: Permissions::default(),
        programs: Some(ext_programs::Programs(Arc::new(move |target: &str, run: &str| ext_programs::read_program(&store, target, run)))),
    }
}

fn spawn(host: &dyn Host, scope: &Scope, program: &str) -> (Completion, String) {
    let sink = Collect::default();
    let command = v::Command { program: program.into(), args: vec!["there".into()], cwd: None, env: vec![], stdin: None };
    let c = crate::ext_broker::run(host, scope, &Start::Spawn { command }, &sink, &AtomicBool::new(false)).expect("handled by the broker");
    (c, String::from_utf8_lossy(&sink.0.into_inner().unwrap()).into_owned())
}

#[cfg(unix)]
#[test]
fn a_package_program_runs_locally_and_over_ssh_from_the_checked_copy() {
    let Some(target) = ext_manifest::this_target() else { return };
    for remote in [false, true] {
        let d = Dir::new(if remote { "ssh" } else { "local" });
        let host = Fixture::new(&d.0.join("home"), remote);
        let s = scope(&d.0.join("work").to_string_lossy(), unpacked(&d, target));
        let (c, out) = spawn(host.as_ref(), &s, "./bin/hello");
        assert_eq!(c, Completion::Exit(Some(0)), "{out}");
        assert_eq!(out.trim(), format!("hello from {target}: there"), "remote: {remote}");
        // Pushed under the host's home by its hash: a temporary file, the hash on the host, then the move.
        let p = ext_programs::read_program(&d.0.join("store"), target, "./bin/hello").unwrap();
        let pushed = ext_programs::pushed_path(host.as_ref(), "you.hello", &p);
        assert!(pushed.starts_with(&format!("{}/.cache/codeyard/programs/you.hello/", host.info().home)), "{pushed}");
        assert_eq!(std::fs::read(&pushed).unwrap(), p.bytes);
        let scripts = host.scripts();
        assert!(scripts.iter().any(|s| s.contains("cat >")) && scripts.iter().any(|s| s.contains("sha256sum") && s.contains("mv -f")), "{scripts:?}");
        // A second run checks the hash of the copy there and pushes nothing.
        let before = host.scripts().len();
        assert_eq!(spawn(host.as_ref(), &s, "bin/hello").0, Completion::Exit(Some(0)), "`bin/hello` is the same entry");
        let again: Vec<String> = host.scripts()[before..].to_vec();
        assert!(again.iter().all(|s| !s.contains("cat >")), "{again:?}");
        // A copy changed on the host is replaced before it runs, never run.
        std::fs::write(&pushed, "#!/bin/sh\necho changed\n").unwrap();
        let (c, out) = spawn(host.as_ref(), &s, "./bin/hello");
        assert_eq!((c, out.trim()), (Completion::Exit(Some(0)), format!("hello from {target}: there").as_str()));
        // Not declared: refused before anything is pushed.
        assert!(matches!(spawn(host.as_ref(), &s, "./bin/other").0, Completion::Failed(v::Error::Denied(_))));
    }
}

/// A program whose bytes do not match the hash its package lists is refused on the host, and nothing is left there.
#[cfg(unix)]
#[test]
fn a_hash_mismatch_refuses_the_program() {
    let d = Dir::new("mismatch");
    for remote in [false, true] {
        let host = Fixture::new(&d.0.join("home"), remote);
        let p = ext_programs::Program { name: "hello".into(), bytes: b"#!/bin/sh\necho evil\n".to_vec(), sha256: "ab".repeat(32) };
        let e = ext_programs::push(host.as_ref(), "you.hello", &p).unwrap_err();
        assert!(e.contains("does not match"), "{e}");
        let pushed = ext_programs::pushed_path(host.as_ref(), "you.hello", &p);
        assert!(!Path::new(&pushed).exists());
        let dir = Path::new(&pushed).parent().unwrap();
        assert_eq!(std::fs::read_dir(dir).map(|r| r.count()).unwrap_or(0), 0, "no temporary file is left");
    }
    // The store's copy was changed after install: its bytes no longer match the hash list, so the broker refuses.
    let Some(target) = ext_manifest::this_target() else { return };
    let store = unpacked(&d, target);
    std::fs::write(store.join("bin/hello"), "#!/bin/sh\necho evil\n").unwrap();
    let host = Fixture::new(&d.0.join("home"), true);
    let (c, out) = spawn(host.as_ref(), &scope(&d.0.join("work").to_string_lossy(), store), "./bin/hello");
    assert!(matches!(&c, Completion::Failed(v::Error::Other(m)) if m.contains("does not match")), "{c:?} {out}");
    assert!(!out.contains("evil"));
}

/// Another platform's package is used for a host on that platform; one the extension was not built for is an error.
#[test]
fn the_package_follows_the_hosts_platform() {
    let d = Dir::new("platform");
    let linux = "x86_64-unknown-linux-musl";
    let store = unpacked(&d, linux);
    let p = ext_programs::read_program(&store, linux, "./bin/hello").unwrap();
    assert_eq!(p.name, "hello");
    assert_eq!(p.sha256.len(), 64);
    assert!(ext_programs::read_program(&store, "x86_64-pc-windows-msvc", "./bin/hello").is_err(), "no bin/hello.exe in a Linux package");
    let mut info = host::local().info().clone();
    info.os = "linux".into();
    info.arch = "x86_64".into();
    assert_eq!(ext_manifest::target_for(&info.arch, &info.os), Some(linux));
    // A package directory serves its own platform; `package_dir` finds it without the store.
    let id = ext_manifest::ExtId::parse("you.hello").unwrap();
    assert_eq!(ext_programs::package_dir(None, &[], &id, &store, linux).unwrap(), store);
    assert!(ext_programs::package_dir(None, &[], &id, &store, "aarch64-apple-darwin").is_err());
}
