use super::*;

#[test]
fn a_slow_job_does_not_hold_up_the_others() {
    let pool = Pool::new(4);
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<&'static str>();
    let d = done_tx.clone();
    pool.run(move || {
        let _ = release_rx.recv_timeout(std::time::Duration::from_secs(10));
        let _ = d.send("slow");
    });
    for _ in 0..3 {
        let d = done_tx.clone();
        pool.run(move || {
            let _ = d.send("fast");
        });
    }
    for _ in 0..3 {
        assert_eq!(done_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), "fast", "fast jobs finish while the slow one is stuck");
    }
    release_tx.send(()).unwrap();
    assert_eq!(done_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), "slow");
}

#[test]
fn requests_are_answered_by_id_even_when_they_run_in_the_pool() {
    let d = tmpdir("pooled");
    for i in 0..20 {
        fs::write(d.join(format!("f{i}.txt")), format!("{i}")).unwrap();
    }
    let (req_r, mut req_w) = io::pipe().unwrap();
    let (resp_r, resp_w) = io::pipe().unwrap();
    std::thread::spawn(move || serve(req_r, resp_w));
    proto::write_frame(&mut req_w, &Frame { id: 1, body: Request::Hello { client_version: proto::VERSION, session: None } }).unwrap();
    for i in 0..20u32 {
        let path = s(&d.join(format!("f{i}.txt")));
        proto::write_frame(&mut req_w, &Frame { id: 100 + i, body: Request::Read { path } }).unwrap();
    }
    let mut r = BufReader::new(resp_r);
    let mut got = std::collections::BTreeMap::new();
    while got.len() < 20 {
        let f: Frame<Response> = proto::read_frame(&mut r).unwrap().unwrap();
        if let Response::File { data, .. } = f.body {
            got.insert(f.id, String::from_utf8(data).unwrap());
        }
    }
    for i in 0..20u32 {
        assert_eq!(got[&(100 + i)], i.to_string(), "answer {i} belongs to request {i}");
    }
    proto::write_frame(&mut req_w, &Frame { id: 0, body: Request::Bye }).unwrap();
}

#[cfg(unix)]
#[test]
fn saving_never_follows_a_planted_symlink_and_handles_long_names() {
    let d = tmpdir("planted");
    let victim = d.join("victim.txt");
    fs::write(&victim, "precious").unwrap();
    // The temp file is opened with O_EXCL: an existing path (here a symlink) is refused, not followed.
    let link = d.join(".planted.tmp");
    std::os::unix::fs::symlink(&victim, &link).unwrap();
    let e = create_private(&link, true).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "precious", "the symlink target is untouched");
    // Names are not predictable from pid and counter alone.
    assert_ne!(temp_path(&d, "x"), temp_path(&d, "x"));
    // Saving a normal file next to the planted link works, and a very long name still saves.
    let target = d.join("doc.txt");
    write(&target, b"new content", None).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "new content");
    let long = d.join(format!("{}.txt", "x".repeat(240)));
    write(&long, b"ok", None).expect("long names save");
    assert_eq!(fs::read(&long).unwrap(), b"ok");
}

#[cfg(unix)]
#[test]
fn saving_over_a_private_file_keeps_it_private() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmpdir("private");
    let p = d.join(".env");
    fs::write(&p, "SECRET=1").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    write(&p, b"SECRET=2", None).unwrap();
    assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    let fresh = d.join("new.txt");
    write(&fresh, b"x", None).unwrap();
    assert_ne!(fs::metadata(&fresh).unwrap().permissions().mode() & 0o044, 0, "a new file follows the umask, not 0600");
}

#[cfg(unix)]
#[test]
fn special_files_are_refused_instead_of_blocking_the_session() {
    let d = tmpdir("fifo");
    let fifo = d.join("pipe");
    let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(matches!(read(&fifo), Err(Error::Unsupported(_))), "a FIFO would block forever");
    assert!(crate::bigfile::build(&fifo).is_err());
    assert!(matches!(read(Path::new("/dev/zero")), Err(Error::Unsupported(_))));
}

#[test]
fn delete_refuses_home_and_its_parents() {
    let d = tmpdir("delguard");
    let home = d.join("users").join("me");
    fs::create_dir_all(home.join("project")).unwrap();
    assert!(matches!(delete_guarded(&home, &home), Err(Error::PermissionDenied(_))));
    assert!(matches!(delete_guarded(home.parent().unwrap(), &home), Err(Error::PermissionDenied(_))), "/Users itself");
    assert!(matches!(delete_guarded(Path::new("/"), &home), Err(Error::PermissionDenied(_))));
    assert!(home.join("project").exists());
    delete_guarded(&home.join("project"), &home).expect("inside home is fine");
    assert!(!home.join("project").exists());
}

#[cfg(any(unix, windows))]
fn directory_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        // Directory junctions work without Developer Mode or elevation.
        let output = std::process::Command::new("cmd.exe").args(["/D", "/C", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
}

#[cfg(any(unix, windows))]
#[test]
fn deleting_a_link_to_home_preserves_the_home_directory() {
    let d = tmpdir("delete-home-link");
    let home = d.join("home");
    fs::create_dir(&home).unwrap();
    fs::write(home.join("keep.txt"), "keep").unwrap();
    let link = d.join("link");
    directory_link(&home, &link);
    delete_guarded(&link, &home).unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
    assert_eq!(fs::read_to_string(home.join("keep.txt")).unwrap(), "keep");
}

#[cfg(any(unix, windows))]
#[test]
fn deleting_a_dangling_directory_link_does_not_require_its_target() {
    let d = tmpdir("delete-dangling-link");
    let target = d.join("target");
    fs::create_dir(&target).unwrap();
    let link = d.join("link");
    directory_link(&target, &link);
    fs::remove_dir(&target).unwrap();
    delete_guarded(&link, &d.join("home")).unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
}

#[cfg(any(unix, windows))]
#[test]
fn deleting_a_directory_link_preserves_its_target_contents() {
    let d = tmpdir("delete-directory-link");
    let target = d.join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep.txt"), "keep").unwrap();
    let link = d.join("link");
    directory_link(&target, &link);
    delete_guarded(&link, &d.join("home")).unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
    assert_eq!(fs::read_to_string(target.join("keep.txt")).unwrap(), "keep");
}

#[cfg(any(unix, windows))]
#[test]
fn configured_home_link_and_its_linked_parent_remain_protected() {
    let d = tmpdir("delete-configured-home-link");
    let target = d.join("target");
    fs::create_dir_all(target.join("home")).unwrap();
    let link = d.join("link");
    directory_link(&target, &link);
    assert!(matches!(delete_guarded(&link, &link), Err(Error::PermissionDenied(_))));
    assert!(matches!(delete_guarded(&link, &link.join("home")), Err(Error::PermissionDenied(_))));
    assert!(fs::symlink_metadata(&link).is_ok());
    assert!(target.join("home").exists());
}

/// A writer that remembers what was written.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);
impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn an_oversized_answer_becomes_an_error_and_the_link_survives() {
    let link = Link::default();
    let sink = Sink::default();
    link.attach(Box::new(sink.clone()), None).unwrap();
    let huge = Response::File { data: vec![0u8; proto::MAX_FRAME + 1], hash: 0 };
    link.send(7, huge).unwrap();
    // The client got an error for id 7 instead of silence...
    let bytes = sink.0.lock().unwrap().clone();
    let frame: Frame<Response> = proto::read_frame(&mut &bytes[..]).unwrap().expect("a frame");
    assert_eq!(frame.id, 7);
    assert!(matches!(frame.body, Response::Error(Error::Other(ref m)) if m.contains("too large")), "{:?}", frame.body);
    // ...and the connection still works.
    sink.0.lock().unwrap().clear();
    link.send(8, Response::Done).unwrap();
    let bytes = sink.0.lock().unwrap().clone();
    let frame: Frame<Response> = proto::read_frame(&mut &bytes[..]).unwrap().expect("a frame");
    assert_eq!((frame.id, matches!(frame.body, Response::Done)), (8, true));
}

#[test]
fn a_failed_attach_keeps_the_backlog_for_the_next_one() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }
    let link = Link::default();
    link.send(0, Response::PtyOutput { pty: 1, data: b"hello".to_vec() }).unwrap();
    link.send(0, Response::PtyExit { pty: 1, code: Some(3) }).unwrap();
    assert!(link.attach(Box::new(Broken), None).is_err());
    let sink = Sink::default();
    link.attach(Box::new(sink.clone()), None).unwrap();
    let bytes = sink.0.lock().unwrap().clone();
    let mut cur = &bytes[..];
    let a: Frame<Response> = proto::read_frame(&mut cur).unwrap().unwrap();
    let b: Frame<Response> = proto::read_frame(&mut cur).unwrap().unwrap();
    assert!(matches!(a.body, Response::PtyOutput { pty: 1, ref data } if data == b"hello"));
    assert!(matches!(b.body, Response::PtyExit { pty: 1, code: Some(3) }), "the exit was not lost");
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-agent-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(windows)]
#[test]
fn case_only_rename_preserves_contents_and_refuses_other_files() {
    let dir = tmpdir("case-rename-şablon boşluk");
    let from = dir.join("readme.txt");
    let to = dir.join("README.txt");
    fs::write(&from, "Türkçe: ıİşğüöç").unwrap();
    rename(&from, &to).unwrap();
    assert_eq!(fs::read_to_string(&to).unwrap(), "Türkçe: ıİşğüöç");
    assert!(fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name() == "README.txt"));
    fs::write(dir.join("other.txt"), "other").unwrap();
    assert!(matches!(rename(&to, &dir.join("other.txt")), Err(Error::AlreadyExists(_))));
    assert_eq!(fs::read_to_string(dir.join("other.txt")).unwrap(), "other");
}

/// What an extension's file call reaches is checked on these: a link inside a folder can lead anywhere.
#[cfg(unix)]
#[test]
fn real_paths_follow_every_link_and_resolve_missing_parts() {
    use std::os::unix::fs::symlink;
    let d = fs::canonicalize(tmpdir("real-paths şü")).unwrap();
    fs::create_dir_all(d.join("ws/src")).unwrap();
    fs::write(d.join("ws/.env"), "SECRET=1").unwrap();
    fs::write(d.join("outside.txt"), "x").unwrap();
    symlink("/etc", d.join("ws/src/link-out")).unwrap();
    symlink("../.env", d.join("ws/src/settings.txt")).unwrap();
    symlink(d.join("nowhere/new.txt"), d.join("ws/src/dangling")).unwrap();
    symlink(d.join("outside.txt"), d.join("ws/src/out-file")).unwrap();
    symlink("loop", d.join("ws/src/loop")).unwrap();
    let ask = |rel: &[&str]| {
        let paths = rel.iter().map(|r| s(&d.join(r))).collect();
        let Response::RealPaths(list) = handle(Request::RealPaths { paths }) else { panic!() };
        list
    };
    let etc = s(&fs::canonicalize("/etc/passwd").unwrap());
    let got = ask(&["ws/src/link-out/passwd", "ws/src/settings.txt", "ws/src/dangling", "ws/src/out-file", "ws/src/new/dir/file.txt", "ws/src"]);
    assert_eq!(got[0], Ok(etc));
    assert_eq!(got[1], Ok(s(&d.join("ws/.env"))));
    assert_eq!(got[2], Ok(s(&d.join("nowhere/new.txt"))), "a dangling link leads where a write would create the file");
    assert_eq!(got[3], Ok(s(&d.join("outside.txt"))));
    assert_eq!(got[4], Ok(s(&d.join("ws/src/new/dir/file.txt"))), "parts that do not exist yet are kept");
    assert_eq!(got[5], Ok(s(&d.join("ws/src"))));
    assert!(ask(&["ws/src/loop"])[0].is_err(), "a link loop is an error, not a path");
}

#[test]
fn write_then_read_roundtrip() {
    let d = tmpdir("rt");
    let p = d.join("a.txt");
    let Response::Written { hash } = handle(Request::Write { path: s(&p), data: b"merhaba\n".to_vec(), expect_hash: None }) else { panic!() };
    let Response::File { data, hash: h2 } = handle(Request::Read { path: s(&p) }) else { panic!() };
    assert_eq!(data, b"merhaba\n");
    assert_eq!(hash, h2);
    // No temp file should remain.
    assert_eq!(fs::read_dir(&d).unwrap().count(), 1);
}

#[test]
fn a_write_against_a_version_does_not_recreate_a_deleted_file() {
    let d = tmpdir("deleted-expect");
    let p = d.join("gone.txt");
    fs::write(&p, "v1").unwrap();
    let base = content_hash(b"v1");
    fs::remove_file(&p).unwrap();
    let r = handle(Request::Write { path: s(&p), data: b"v2".to_vec(), expect_hash: Some(base) });
    assert!(matches!(r, Response::Error(Error::Conflict { .. })), "{r:?}");
    assert!(!p.exists(), "not recreated");
    // Without an expected version (a forced save, a new file) it is still written.
    let r = handle(Request::Write { path: s(&p), data: b"v2".to_vec(), expect_hash: None });
    assert!(matches!(r, Response::Written { .. }), "{r:?}");
    assert_eq!(fs::read_to_string(&p).unwrap(), "v2");
}

#[test]
fn conflict_when_disk_changed() {
    let d = tmpdir("conflict");
    let p = d.join("a.txt");
    fs::write(&p, "v1").unwrap();
    let base = content_hash(b"v1");
    fs::write(&p, "dışarıdan değişti").unwrap();
    let r = handle(Request::Write { path: s(&p), data: b"benim".to_vec(), expect_hash: Some(base) });
    assert!(matches!(r, Response::Error(Error::Conflict { .. })), "{r:?}");
    assert_eq!(fs::read_to_string(&p).unwrap(), "dışarıdan değişti");
}

#[cfg(unix)]
#[test]
fn keeps_permissions_and_symlink() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let d = tmpdir("perm");
    let real = d.join("script.sh");
    fs::write(&real, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o750)).unwrap();
    let link = d.join("link.sh");
    symlink(&real, &link).unwrap();

    let r = handle(Request::Write { path: s(&link), data: b"#!/bin/sh\necho 1\n".to_vec(), expect_hash: None });
    assert!(matches!(r, Response::Written { .. }), "{r:?}");
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link must be kept");
    assert_eq!(fs::read_to_string(&real).unwrap(), "#!/bin/sh\necho 1\n");
    assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o750);
}

#[cfg(unix)]
#[test]
fn hard_link_written_in_place() {
    let d = tmpdir("hardlink");
    let a = d.join("a");
    let b = d.join("b");
    fs::write(&a, "1").unwrap();
    fs::hard_link(&a, &b).unwrap();
    handle(Request::Write { path: s(&a), data: b"2".to_vec(), expect_hash: None });
    assert_eq!(fs::read_to_string(&b).unwrap(), "2", "the hard link must not break");
}

/// A write that dies part way (disk full, I/O error) must not leave the file empty or half new: the original comes back,
/// through every hard link.
#[cfg(unix)]
#[test]
fn failed_in_place_write_restores_the_original() {
    let d = tmpdir("inplace-fail");
    let a = d.join("a");
    let b = d.join("b");
    let original = b"the original, longer than the replacement fragment".to_vec();
    fs::write(&a, &original).unwrap();
    fs::hard_link(&a, &b).unwrap();
    let mut f = fs::OpenOptions::new().write(true).open(&a).unwrap();
    let err = overwrite(&mut f, b"new contents that never finish", Some(&original), |f, d| {
        f.write_all(&d[..10])?;
        Err(io::Error::other("no space left on device"))
    })
    .unwrap_err();
    assert!(err.to_string().contains("no space"), "{err}");
    assert_eq!(fs::read(&a).unwrap(), original);
    assert_eq!(fs::read(&b).unwrap(), original);
}

/// In place, a shorter file replaces a longer one completely (the length is cut after the write, not before).
#[cfg(unix)]
#[test]
fn in_place_write_shrinks_the_file() {
    let d = tmpdir("inplace-shrink");
    let a = d.join("a");
    let b = d.join("b");
    fs::write(&a, "a long first version").unwrap();
    fs::hard_link(&a, &b).unwrap();
    let r = handle(Request::Write { path: s(&a), data: b"short".to_vec(), expect_hash: None });
    assert!(matches!(r, Response::Written { .. }), "{r:?}");
    assert_eq!(fs::read(&b).unwrap(), b"short");
}

/// The conflict check hashes the file on disk, so it obeys the same size limit as `Read`: a target that grew past it
/// is refused instead of being read into memory.
#[test]
fn conflict_check_refuses_a_target_past_the_size_limit() {
    let d = tmpdir("conflict-big");
    let f = d.join("big.bin");
    fs::write(&f, "small").unwrap();
    let hash = content_hash(b"small");
    fs::OpenOptions::new().write(true).open(&f).unwrap().set_len(proto::MAX_FILE + 1).unwrap();
    let r = handle(Request::Write { path: s(&f), data: b"x".to_vec(), expect_hash: Some(hash) });
    assert!(matches!(r, Response::Error(Error::TooLarge { size, .. }) if size == proto::MAX_FILE + 1), "{r:?}");
    assert_eq!(fs::metadata(&f).unwrap().len(), proto::MAX_FILE + 1, "nothing was written");
}

#[test]
fn read_dir_and_errors() {
    let d = tmpdir("dir");
    fs::create_dir(d.join("src")).unwrap();
    fs::write(d.join("Cargo.toml"), "").unwrap();
    let Response::Entries(mut es) = handle(Request::ReadDir { path: s(&d) }) else { panic!() };
    es.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(es.iter().map(|e| (e.name.as_str(), e.is_dir())).collect::<Vec<_>>(), [("Cargo.toml", false), ("src", true)]);
    assert!(matches!(handle(Request::Read { path: s(&d.join("yok")) }), Response::Error(Error::NotFound(_))));
    assert!(matches!(handle(Request::Read { path: s(&d) }), Response::Error(Error::IsDirectory(_))));
}

#[test]
fn list_files_respects_gitignore_and_skips_git() {
    let d = tmpdir("list");
    fs::create_dir_all(d.join("src/deep")).unwrap();
    fs::create_dir_all(d.join(".git")).unwrap();
    fs::create_dir_all(d.join("target")).unwrap();
    fs::create_dir_all(d.join("node_modules/x")).unwrap();
    fs::write(d.join(".gitignore"), "target/\n*.log\n").unwrap();
    for f in ["src/main.rs", "src/deep/a.rs", ".env", "build.log", "target/out", ".git/HEAD", "node_modules/x/i.js"] {
        fs::write(d.join(f), "").unwrap();
    }
    let Response::Files { paths, truncated } = handle(Request::ListFiles { root: s(&d), limit: 100 }) else { panic!() };
    assert!(!truncated);
    assert_eq!(paths, [".env", ".gitignore", "src/deep/a.rs", "src/main.rs"]);
    let Response::Files { paths, truncated } = handle(Request::ListFiles { root: s(&d), limit: 2 }) else { panic!() };
    assert!(truncated);
    assert_eq!(paths.len(), 2);
}

#[test]
fn only_one_concurrent_directory_creator_can_succeed() {
    let root = tmpdir("concurrent-mkdir");
    for attempt in 0..8 {
        let path = root.join(format!("parent-{attempt}/new-directory"));
        let barrier = Arc::new(std::sync::Barrier::new(24));
        let threads: Vec<_> = (0..24)
            .map(|_| {
                let path = s(&path);
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    handle(Request::CreateDir { path })
                })
            })
            .collect();
        let replies: Vec<_> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
        assert_eq!(replies.iter().filter(|reply| matches!(reply, Response::Done)).count(), 1, "exactly one caller owns the new directory: {replies:?}");
        assert_eq!(replies.iter().filter(|reply| matches!(reply, Response::Error(Error::AlreadyExists(_)))).count(), 23, "{replies:?}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_operations() {
    let d = tmpdir("ops");
    let f = d.join("yeni.rs");
    assert!(matches!(handle(Request::CreateFile { path: s(&f) }), Response::Done));
    fs::write(&f, "içerik").unwrap();
    assert!(matches!(handle(Request::CreateFile { path: s(&f) }), Response::Error(Error::AlreadyExists(_))), "must not overwrite an existing file");
    assert_eq!(fs::read_to_string(&f).unwrap(), "içerik");

    let sub = d.join("alt");
    assert!(matches!(handle(Request::CreateDir { path: s(&sub) }), Response::Done));
    let g = sub.join("taşındı.rs");
    assert!(matches!(handle(Request::Rename { from: s(&f), to: s(&g) }), Response::Done));
    assert!(!f.exists() && g.exists());

    fs::write(&f, "başka").unwrap();
    assert!(matches!(handle(Request::Rename { from: s(&f), to: s(&g) }), Response::Error(Error::AlreadyExists(_))));
    assert_eq!(fs::read_to_string(&g).unwrap(), "içerik", "must not overwrite the target");

    assert!(matches!(handle(Request::Delete { path: s(&sub) }), Response::Done));
    assert!(!sub.exists());
}

#[test]
fn refuses_to_delete_home_or_root() {
    assert!(matches!(handle(Request::Delete { path: "~".into() }), Response::Error(Error::PermissionDenied(_))));
    assert!(matches!(handle(Request::Delete { path: "/".into() }), Response::Error(Error::PermissionDenied(_))));
}

/// End to end: a framed session over a pipe.
struct Client {
    w: io::PipeWriter,
    r: BufReader<io::PipeReader>,
}

impl Client {
    fn new() -> Self {
        let (req_r, req_w) = io::pipe().unwrap();
        let (resp_r, resp_w) = io::pipe().unwrap();
        std::thread::spawn(move || serve(req_r, resp_w));
        Client { w: req_w, r: BufReader::new(resp_r) }
    }
    fn send(&mut self, id: u32, body: Request) {
        proto::write_frame(&mut self.w, &Frame { id, body }).unwrap();
    }
    fn recv(&mut self) -> Frame<Response> {
        proto::read_frame(&mut self.r).unwrap().unwrap()
    }
}

#[test]
fn session_streams_search_results() {
    let d = tmpdir("session-search");
    fs::write(d.join("a.txt"), "iğne\nsaman\niğne iğne\n").unwrap();
    let mut c = Client::new();
    c.send(
        5,
        Request::Search(proto::SearchQuery {
            root: s(&d),
            pattern: "iğne".into(),
            regex: false,
            case_sensitive: true,
            whole_word: false,
            include: vec![],
            exclude: vec![],
            max_results: 100,
        }),
    );
    let mut chunks = Vec::new();
    loop {
        let f = c.recv();
        assert_eq!(f.id, 5);
        match f.body {
            Response::SearchChunk(fm) => chunks.extend(fm),
            Response::SearchDone { matches, .. } => {
                assert_eq!(matches, 3);
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(chunks[0].matches.iter().map(|m| m.line).collect::<Vec<_>>(), [1, 3]);
}

#[cfg(unix)]
#[test]
fn session_runs_a_shell_in_a_pty() {
    let mut c = Client::new();
    c.send(1, Request::PtySpawn { cols: 80, rows: 24, cwd: None, shell: Some("/bin/sh".into()) });
    let Response::PtySpawned { pty } = c.recv().body else { panic!("spawn") };
    // id 0: no response expected.
    c.send(0, Request::PtyInput { pty, data: b"echo kod-$((40+2)); exit 3\n".to_vec() });
    let mut output = Vec::new();
    let code = loop {
        let f = c.recv();
        assert_eq!(f.id, 0, "terminal messages are events");
        match f.body {
            Response::PtyOutput { data, .. } => output.extend(data),
            Response::PtyExit { code, .. } => break code,
            other => panic!("{other:?}"),
        }
    };
    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("kod-42"), "{text}");
    assert_eq!(code, Some(3));
}

/// Output of a task until its `ProcExit`: (text, exit code).
#[cfg(unix)]
fn task_output(c: &mut Client, proc: u32) -> (String, Option<i32>) {
    let mut text = Vec::new();
    loop {
        match c.recv().body {
            Response::ProcOutput { proc: p, data } if p == proc => text.extend(data),
            Response::ProcExit { proc: p, code } if p == proc => return (String::from_utf8_lossy(&text).into_owned(), code),
            other => panic!("{other:?}"),
        }
    }
}

/// Process tasks get their arguments literally; variables are set and removed in order; stderr arrives in the same
/// stream; the exit code is reported.
#[cfg(unix)]
#[test]
fn a_process_task_keeps_its_arguments_and_environment_exactly() {
    let mut c = Client::new();
    let dir = std::env::temp_dir().join(format!("codeyard-task-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = "printf '%s|' \"$@\"; printf 'A=%s B=%s' \"$TASK_A\" \"${TASK_B-unset}\"; echo err >&2; exit 3";
    let args = vec!["-c".into(), script.into(), "sh".into(), "two words".into(), "'q' \"d\" $HOME; *".into(), String::new()];
    c.send(
        1,
        Request::TaskSpawn {
            command: proto::TaskCommand::Process { program: "/bin/sh".into(), args },
            cwd: Some(dir.to_string_lossy().into_owned()),
            env: vec![("TASK_A".into(), Some("é ü".into())), ("TASK_B".into(), Some("x".into())), ("TASK_B".into(), None)],
        },
    );
    let Response::ProcSpawned { proc } = c.recv().body else { panic!("spawn") };
    let (text, code) = task_output(&mut c, proc);
    assert_eq!(code, Some(3));
    assert!(text.starts_with("two words|'q' \"d\" $HOME; *||A=é ü B=unset"), "{text:?}");
    assert!(text.contains("err"), "stderr is part of the output: {text:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_shell_task_runs_its_line_in_the_given_shell_and_a_missing_folder_is_an_error() {
    let mut c = Client::new();
    let shell = proto::TaskCommand::Shell { shell: Some("/bin/sh".into()), args: Some(vec!["-c".into()]), line: "echo $((6 * 7))".into() };
    c.send(1, Request::TaskSpawn { command: shell.clone(), cwd: None, env: Vec::new() });
    let Response::ProcSpawned { proc } = c.recv().body else { panic!("spawn") };
    assert_eq!(task_output(&mut c, proc), ("42\n".into(), Some(0)));
    c.send(2, Request::TaskSpawn { command: shell, cwd: Some("/nonexistent/codeyard".into()), env: Vec::new() });
    assert!(matches!(c.recv().body, Response::Error(_)));
}

/// Stopping a task stops what it started, also after the leader has exited and only a descendant holds the output.
#[cfg(unix)]
#[test]
fn killing_a_task_stops_its_descendants() {
    let mut c = Client::new();
    let dir = std::env::temp_dir().join(format!("codeyard-task-tree-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pidfile = dir.join("child.pid");
    let line = format!("sleep 300 & echo $! > '{}'; echo started; wait", pidfile.display());
    c.send(
        1,
        Request::TaskSpawn {
            command: proto::TaskCommand::Shell { shell: Some("/bin/sh".into()), args: Some(vec!["-c".into()]), line },
            cwd: None,
            env: Vec::new(),
        },
    );
    let Response::ProcSpawned { proc } = c.recv().body else { panic!("spawn") };
    let Response::ProcOutput { .. } = c.recv().body else { panic!("output") };
    let child: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    // SAFETY: signal 0 only checks that the process exists.
    assert_eq!(unsafe { libc::kill(child, 0) }, 0, "the descendant runs");
    c.send(2, Request::ProcKill { proc });
    let mut exited = None;
    while exited.is_none() {
        match c.recv().body {
            Response::ProcExit { proc: p, code } if p == proc => exited = Some(code),
            Response::Done | Response::ProcOutput { .. } => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(exited, Some(None), "a killed task has no exit code");
    let gone = (0..200).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(10));
        // SAFETY: as above.
        (unsafe { libc::kill(child, 0) }) != 0
    });
    assert!(gone, "the descendant was stopped with the task");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A server that stops reading must not silently lose later input: once the queue overflows the process is ended and the
/// client gets its `ProcExit`.
#[cfg(unix)]
#[test]
fn a_process_that_never_reads_its_input_is_ended_when_the_queue_overflows() {
    let mut c = Client::new();
    c.send(1, Request::ProcSpawn { program: "sleep".into(), args: vec!["60".into()], cwd: None });
    let Response::ProcSpawned { proc } = c.recv().body else { panic!("spawn") };
    let chunk = vec![b'x'; 64 * 1024];
    for _ in 0..600 {
        c.send(0, Request::ProcInput { proc, data: chunk.clone() });
    }
    loop {
        match c.recv().body {
            Response::ProcExit { proc: p, .. } => {
                assert_eq!(p, proc);
                break;
            }
            Response::ProcOutput { .. } => {}
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn hello_rejects_other_version() {
    assert!(matches!(handle(Request::Hello { client_version: 999, session: None }), Response::Error(Error::VersionMismatch { .. })));
    assert!(matches!(handle(Request::Hello { client_version: proto::VERSION, session: None }), Response::Hello(_)));
}
