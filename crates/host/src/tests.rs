use super::*;

/// The heartbeat primitive: a live agent answers `Ping`, a silent one does not (and its waiter is removed again).
#[test]
fn ping_tells_a_live_agent_from_a_silent_one() {
    let live = piped_agent();
    assert!(live.shared.ping(std::time::Duration::from_secs(5)));
    assert!(live.shared.waiters.lock().unwrap().is_empty());

    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    std::thread::spawn(move || {
        let mut r = BufReader::new(req_r);
        let mut w = resp_w;
        while let Ok(Some(Frame { id, body })) = proto::read_frame::<Request>(&mut r) {
            if let Request::Hello { client_version, .. } = body {
                let info =
                    proto::HostInfo { version: client_version, os: "t".into(), arch: "t".into(), home: "/".into(), hostname: "t".into(), resumed: false };
                let _ = proto::write_frame(&mut w, &Frame { id, body: Response::Hello(info) });
            }
        }
    });
    let silent = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "silent").unwrap();
    let started = std::time::Instant::now();
    assert!(!silent.shared.ping(std::time::Duration::from_millis(300)));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(silent.shared.waiters.lock().unwrap().is_empty(), "no waiter is leaked by a missed ping");
}

/// An agent that answers the handshake and then goes silent: the call returns an error instead of hanging forever,
/// and the connection is still usable afterwards.
#[test]
fn a_silent_agent_makes_a_call_fail_instead_of_hanging() {
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    std::thread::spawn(move || {
        let mut r = BufReader::new(req_r);
        let mut w = resp_w;
        while let Ok(Some(Frame { id, body })) = proto::read_frame::<Request>(&mut r) {
            // Answers Hello and Stat of "/ok"; swallows everything else.
            match body {
                Request::Hello { client_version, .. } => {
                    let _ = proto::write_frame(
                        &mut w,
                        &Frame {
                            id,
                            body: Response::Hello(proto::HostInfo {
                                version: client_version,
                                os: "test".into(),
                                arch: "test".into(),
                                home: "/".into(),
                                hostname: "t".into(),
                                resumed: false,
                            }),
                        },
                    );
                }
                Request::Stat { path } if path == "/ok" => {
                    let meta = proto::Meta { kind: proto::Kind::File, size: 1, mtime_ms: 0, readonly: false, file_id: 0, created_ms: 0 };
                    let _ = proto::write_frame(&mut w, &Frame { id, body: Response::Stat(meta) });
                }
                _ => {}
            }
        }
    });
    let host = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "silent").unwrap();
    host.set_call_timeout_secs(1);
    let started = std::time::Instant::now();
    let err = host.call(Request::ReadDir { path: "/hang".into() }).unwrap_err();
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    assert!(err.to_string().contains("did not answer"), "{err}");
    assert!(host.shared.waiters.lock().unwrap().is_empty(), "the abandoned waiter was removed");
    assert!(matches!(host.call(Request::Stat { path: "/ok".into() }), Ok(Response::Stat(_))), "the connection still works");
}

/// Closing a host tells the agent (`Bye`): its session ends at once.
// Unix only: it opens a pty running `/bin/sh`. The ConPTY side has its own tests below.
#[cfg(unix)]
#[test]
fn dropping_a_host_ends_the_agent_session_immediately() {
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = ended.clone();
    std::thread::spawn(move || {
        let _ = agent::serve(req_r, resp_w);
        flag.store(true, Ordering::Relaxed);
    });
    let host = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "bye").unwrap();
    let pty = host.pty_spawn(80, 24, None, Some("/bin/sh".into())).unwrap();
    drop(pty);
    drop(host);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ended.load(Ordering::Relaxed) {
        assert!(std::time::Instant::now() < deadline, "the agent session did not end after the host was dropped");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Two pipes and a thread instead of a real process: end-to-end protocol test.
fn piped_agent() -> RemoteHost {
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    std::thread::spawn(move || agent::serve(req_r, resp_w));
    RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "test").unwrap()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("codeyard-host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn remote_and_local_behave_the_same() {
    let dir = tmp("same");
    let path = dir.join("a.rs").to_string_lossy().into_owned();

    let remote = piped_agent();
    let local = local();
    assert_eq!(remote.info().version, proto::VERSION);
    assert!(!local.is_remote());

    let h1 = remote.write(&path, b"fn main() {}\n".to_vec(), None).unwrap();
    let (data, h2) = local.read(&path).unwrap();
    assert_eq!((data.as_slice(), h1), (&b"fn main() {}\n"[..], h2));

    // Modify locally, then try writing remotely with the old hash: conflict.
    local.write(&path, b"// degisti\n".to_vec(), Some(h2)).unwrap();
    let err = remote.write(&path, b"x".to_vec(), Some(h1)).unwrap_err();
    assert!(matches!(err, HostError::Op(proto::Error::Conflict { .. })), "{err}");

    let names: Vec<_> = remote.read_dir(&dir.to_string_lossy()).unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["a.rs"]);
}

#[test]
fn concurrent_calls_are_matched_by_id() {
    let dir = tmp("concurrent");
    for i in 0..20 {
        std::fs::write(dir.join(format!("{i}.txt")), i.to_string()).unwrap();
    }
    let host = Arc::new(piped_agent());
    let handles: Vec<_> = (0..20)
        .map(|i| {
            let host = host.clone();
            let p = dir.join(format!("{i}.txt")).to_string_lossy().into_owned();
            std::thread::spawn(move || host.read(&p).unwrap().0)
        })
        .collect();
    for (i, h) in handles.into_iter().enumerate() {
        assert_eq!(h.join().unwrap(), i.to_string().into_bytes(), "replies must not get mixed up");
    }
}

#[test]
fn search_streams_while_other_calls_proceed() {
    let dir = tmp("search");
    std::fs::write(dir.join("a.txt"), "iğne\n").unwrap();
    std::fs::write(dir.join("b.txt"), "saman İĞNE\n").unwrap();
    let host = local();
    let stream = host
        .search(SearchQuery {
            root: dir.to_string_lossy().into_owned(),
            pattern: "iğne".into(),
            regex: false,
            case_sensitive: false,
            whole_word: false,
            include: vec![],
            exclude: vec![],
            max_results: 100,
        })
        .unwrap();
    // Another call runs while a search is in progress.
    assert!(host.stat(&dir.to_string_lossy()).is_ok());
    let mut files = Vec::new();
    for r in stream.rx.iter() {
        match r {
            Response::SearchChunk(fm) => files.extend(fm.into_iter().map(|f| f.path)),
            Response::SearchDone { matches, .. } => {
                assert_eq!(matches, 2);
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    files.sort();
    assert_eq!(files, ["a.txt", "b.txt"]);
}

#[test]
fn hash_tree_streams_records_then_done_and_hashes_on_request() {
    let dir = tmp("hashtree");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/a.txt"), "iğne").unwrap();
    let root = dir.to_string_lossy().into_owned();
    let host = local();
    let collect = |only: Option<Vec<String>>| {
        let stream = host.hash_tree(&root, vec![], true, 100, only).unwrap();
        let mut records = Vec::new();
        loop {
            match stream.next().expect("the stream must end with Done") {
                HashEvent::Records(r) => records.extend(r),
                HashEvent::Done { entries, .. } => return (records, entries),
                HashEvent::Failed(e) => panic!("{e}"),
            }
        }
    };
    let (listing, entries) = collect(None);
    assert_eq!((listing.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), entries), (vec!["sub", "sub/a.txt"], 2));
    assert!(listing.iter().all(|r| r.hash.is_none()));
    let (hashed, _) = collect(Some(vec!["sub/a.txt".into()]));
    assert_eq!(hashed[0].hash, Some(proto::content_hash("iğne".as_bytes())));
    let stream = host.hash_tree("/definitely/not/here", vec![], true, 10, None).unwrap();
    assert!(matches!(stream.next(), Some(HashEvent::Failed(proto::Error::NotFound(_)))));
}

#[cfg(unix)]
#[test]
fn pty_roundtrip_through_host() {
    let host = local();
    let pty = host.pty_spawn(80, 24, None, Some("/bin/sh".into())).unwrap();
    host.pty_input(pty.id, b"printf 'merhaba\\n'; exit 0\n".to_vec());
    let mut out = Vec::new();
    loop {
        match pty.events.recv_timeout(std::time::Duration::from_secs(10)).expect("output expected") {
            PtyEvent::Output(d) => out.extend(d),
            PtyEvent::Exit(code) => {
                assert_eq!(code, Some(0));
                break;
            }
        }
    }
    assert!(String::from_utf8_lossy(&out).contains("merhaba"));
}

#[test]
fn watch_reports_external_changes() {
    let dir = tmp("watch");
    let f = dir.join("a.txt");
    std::fs::write(&f, "1").unwrap();
    let host = local();
    let events = host.fs_events();
    let key = f.to_string_lossy().into_owned();
    host.watch(vec![key.clone()]);
    std::thread::sleep(std::time::Duration::from_millis(100));
    std::fs::write(&f, "dışarıdan").unwrap();
    let batch = events.recv_timeout(std::time::Duration::from_secs(5)).expect("change event");
    assert_eq!(batch, vec![(key, ChangeKind::Modified)]);
}

/// Server-side writer: writes fail once the switch is off (network drop).
#[cfg(unix)]
struct Killable {
    inner: std::io::PipeWriter,
    dead: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(unix)]
impl Write for Killable {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.dead.load(Ordering::Relaxed) {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.inner.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(unix)]
#[test]
fn reconnects_and_terminal_continues() {
    let registry = Arc::new(agent::daemon::Registry::default());
    let switch: Arc<Mutex<Arc<std::sync::atomic::AtomicBool>>> = Arc::default();
    let attempts = Arc::new(AtomicU32::new(0));
    let (reg, sw, att) = (registry.clone(), switch.clone(), attempts.clone());
    let connector: Connector = Arc::new(move || {
        att.fetch_add(1, Ordering::Relaxed);
        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, resp_w) = std::io::pipe().unwrap();
        let dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
        *sw.lock().unwrap() = dead.clone();
        let reg = reg.clone();
        std::thread::spawn(move || reg.serve_connection(req_r, Killable { inner: resp_w, dead }));
        Ok(Transport::pipes(Box::new(resp_r), Box::new(req_w)))
    });
    let host = RemoteHost::connect(connector, "pencere-1".into(), "test").unwrap();
    let status = host.status_events();
    let pty = host.pty_spawn(80, 24, None, Some("/bin/sh".into())).unwrap();
    // Piped process like a language server: can't survive a drop, gets terminated.
    let lsp = host.proc_spawn("cat", Vec::new(), None).unwrap();

    // Network drops; then a command that produces output (the write error triggers the drop).
    switch.lock().unwrap().store(true, Ordering::Relaxed);
    host.pty_input(pty.id, b"echo koptuktan-sonra\n".to_vec());

    let wait = std::time::Duration::from_secs(15);
    assert!(matches!(status.recv_timeout(wait).unwrap(), ConnStatus::Reconnecting { .. }));
    assert!(matches!(host.stat("/"), Err(HostError::Disconnected(_))), "call must not hang while disconnected");
    assert_eq!(status.recv_timeout(wait).unwrap(), ConnStatus::Connected { resumed: true });
    assert!(attempts.load(Ordering::Relaxed) >= 2);

    // Same terminal channel: output produced while disconnected and the new command arrive.
    host.pty_input(pty.id, b"echo yeniden-bagli\n".to_vec());
    let mut out = String::new();
    let deadline = std::time::Instant::now() + wait;
    while !(out.contains("koptuktan-sonra") && out.contains("yeniden-bagli")) {
        assert!(std::time::Instant::now() < deadline, "terminal output: {out}");
        if let Ok(PtyEvent::Output(d)) = pty.events.recv_timeout(std::time::Duration::from_millis(200)) {
            out.push_str(&String::from_utf8_lossy(&d));
        }
    }
    assert!(host.stat("/").is_ok(), "connection works again");

    // The piped process was reported as exited (the client restarts it); the new one runs.
    let mut exited = false;
    while let Ok(ev) = lsp.events.recv_timeout(std::time::Duration::from_millis(500)) {
        if ev == PtyEvent::Exit(None) {
            exited = true;
        }
    }
    assert!(exited, "dropped process must get Exit(None)");
    let again = host.proc_spawn("cat", Vec::new(), None).unwrap();
    host.proc_input(again.id, b"tekrar\n".to_vec());
    assert_eq!(again.events.recv_timeout(wait).unwrap(), PtyEvent::Output(b"tekrar\n".to_vec()));
}

#[test]
fn dead_agent_reports_disconnected() {
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    let t = std::thread::spawn(move || {
        // Only answer the handshake, then die.
        let mut r = BufReader::new(req_r);
        let mut w = resp_w;
        let f: Frame<Request> = proto::read_frame(&mut r).unwrap().unwrap();
        proto::write_frame(&mut w, &Frame { id: f.id, body: agent::handle(f.body) }).unwrap();
    });
    let host = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "test").unwrap();
    t.join().unwrap();
    let err = host.read_dir("/").unwrap_err();
    assert!(matches!(err, HostError::Disconnected(_)), "{err}");
    // Later calls must fail the same way right away, not hang.
    assert!(matches!(host.stat("/"), Err(HostError::Disconnected(_))));
}

#[test]
#[cfg(unix)]
fn exec_and_piped_process() {
    let h = piped_agent();
    let dir = tmp("exec");
    // stdin + cwd; the program name is looked up with the login shell.
    let out = h.exec("cat", &[], Some(&dir.to_string_lossy()), Some("merhaba\n".into())).unwrap();
    assert!(out.ok());
    assert_eq!(out.stdout_str(), "merhaba\n");
    let out = h.exec("pwd", &[], Some(&dir.to_string_lossy()), None).unwrap();
    assert!(out.stdout_str().trim().ends_with(dir.file_name().unwrap().to_str().unwrap()));
    let out = h.exec("codeyard-boyle-bir-program-yok", &[], None, None).unwrap();
    assert_eq!(out.code, Some(127), "shell could not find it: {}", out.stderr_str());
    let out = h.exec("/yok/program", &[], None, None);
    assert!(matches!(out, Err(HostError::Op(proto::Error::NotFound(_)))));

    // Long-running process: write, read, close.
    let p = h.proc_spawn("cat", Vec::new(), None).unwrap();
    h.proc_input(p.id, b"ping\n".to_vec());
    match p.events.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
        ProcEvent::Output(d) => assert_eq!(d, b"ping\n"),
        other => panic!("{other:?}"),
    }
    h.proc_kill(p.id);
    loop {
        match p.events.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
            ProcEvent::Exit(_) => break,
            ProcEvent::Output(_) => {}
        }
    }
}

/// Fault injection, slow consumer of a process (this is what a language server or debug adapter is): nothing may be
/// dropped and nothing may be reordered, and the connection reader must keep serving every other call, because it is
/// shared by all calls, terminals, searches and the heartbeat. Blocking it to bound the queue would stall all of those.
#[test]
#[cfg(unix)]
fn a_stalled_process_consumer_loses_nothing_and_does_not_block_other_calls() {
    const TOTAL: usize = 4 * 1024 * 1024;
    let h = piped_agent();
    let p = h.proc_spawn("cat", Vec::new(), None).unwrap();
    let pattern = |i: usize| (i % 251) as u8;
    let mut sent = 0;
    while sent < TOTAL {
        let n = (64 * 1024).min(TOTAL - sent);
        h.proc_input(p.id, (sent..sent + n).map(pattern).collect());
        sent += n;
    }
    // Give the echo time to pile up unread, then prove other calls are served meanwhile.
    std::thread::sleep(std::time::Duration::from_millis(800));
    let t = std::time::Instant::now();
    assert!(h.stat("/").is_ok());
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "a stat took {:?} behind a stalled process consumer", t.elapsed());
    // The slow consumer finally reads: every byte, in order.
    let mut got = 0usize;
    while got < TOTAL {
        match p.events.recv_timeout(std::time::Duration::from_secs(20)).expect("output keeps coming") {
            ProcEvent::Output(d) => {
                assert!(d.iter().enumerate().all(|(i, b)| *b == pattern(got + i)), "bytes reordered or damaged at {got}");
                got += d.len();
            }
            ProcEvent::Exit(c) => panic!("cat exited early: {c:?}"),
        }
    }
    assert_eq!(got, TOTAL);
    h.proc_kill(p.id);
}

#[cfg(windows)]
#[test]
fn windows_local_exec_and_piped_process() {
    let h = piped_agent();
    let dir = tmp("exec-şablon boşluk");
    let out = h.exec("git", &["--version"], Some(&dir.to_string_lossy()), None).unwrap();
    assert!(out.ok(), "{}", out.stderr_str());
    assert!(out.stdout_str().starts_with("git version"));
    assert!(matches!(h.exec("codeyard-no-such-program", &[], None, None), Err(HostError::Op(proto::Error::NotFound(_)))));
    let p = h.proc_spawn("cmd.exe", vec!["/d".into(), "/q".into(), "/c".into(), "more".into()], Some(dir.to_string_lossy().into_owned())).unwrap();
    h.proc_input(p.id, b"pipe-marker\r\n".to_vec());
    let mut output = Vec::new();
    while !String::from_utf8_lossy(&output).contains("pipe-marker") {
        match p.events.recv_timeout(std::time::Duration::from_secs(5)).expect("piped output") {
            ProcEvent::Output(data) => output.extend(data),
            other => panic!("{other:?}"),
        }
    }
    h.proc_kill(p.id);
    while !matches!(p.events.recv_timeout(std::time::Duration::from_secs(5)).expect("process exit"), ProcEvent::Exit(_)) {}
}

#[cfg(windows)]
#[test]
fn windows_conpty_roundtrip_resize_and_exit() {
    let h = piped_agent();
    let dir = tmp("terminal-şablon boşluk");
    let p = h.pty_spawn(80, 24, Some(dir.to_string_lossy().into_owned()), Some("cmd.exe".into())).unwrap();
    h.pty_resize(p.id, 120, 35);
    let mut output = Vec::new();
    let mut sent = false;
    loop {
        match p
            .events
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|e| panic!("ConPTY output/exit: {e}: {}", String::from_utf8_lossy(&output)))
        {
            PtyEvent::Output(data) => {
                output.extend(data);
                // ConPTY asks the terminal emulator for its initial cursor position.
                if !sent && output.windows(4).any(|s| s == b"\x1b[6n") {
                    h.pty_input(p.id, b"\x1b[1;1R".to_vec());
                    h.pty_input(p.id, "chcp 65001 >nul\r\necho __CY_%TERM_PROGRAM%__\r\necho Türkçe: ıİşğüöç\r\necho %CD%\r\nexit 7\r\n".as_bytes().to_vec());
                    sent = true;
                }
            }
            PtyEvent::Exit(code) => {
                assert_eq!(code, Some(7), "{}", String::from_utf8_lossy(&output));
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("__CY_codeyard__"), "{text}");
    assert!(text.contains("Türkçe: ıİşğüöç"), "{text}");
    assert!(text.contains(&*dir.to_string_lossy()), "{text}");
}

#[test]
fn fs_changes_of_a_stalled_consumer_merge_per_path() {
    use ChangeKind::*;
    let mut pending: FsChanges = Vec::new();
    merge_fs_changes(&mut pending, vec![("a".into(), Created), ("b".into(), Modified), ("c".into(), Removed)]);
    merge_fs_changes(&mut pending, vec![("a".into(), Modified), ("b".into(), Removed), ("c".into(), Created), ("d".into(), Created)]);
    assert_eq!(pending, vec![("a".into(), Created), ("b".into(), Removed), ("c".into(), Modified), ("d".into(), Created)]);

    // Fault injection: the consumer never reads while the agent keeps reporting its 2048 watched paths every second
    // for hours. The backlog is the watched set, not the number of reports.
    let mut stalled: FsChanges = Vec::new();
    for round in 0..20_000u32 {
        let kind = if round % 2 == 0 { Modified } else { Removed };
        merge_fs_changes(&mut stalled, (0..64).map(|i| (format!("/w/{i}"), kind)).collect());
    }
    assert_eq!(stalled.len(), 64);
    assert!(stalled.iter().all(|(_, k)| *k == Removed), "the last report wins");
}

/// A writer that blocks in `write` until the gate opens, then forwards to `inner`; `fail` makes later writes error.
struct Gated {
    inner: Box<dyn Write + Send>,
    gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
    fail: Arc<std::sync::atomic::AtomicBool>,
}

impl Write for Gated {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let (open, cv) = &*self.gate;
        let mut open = open.locked();
        while !*open {
            open = cv.wait(open).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(open);
        if self.fail.load(Ordering::Relaxed) {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.inner.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

type Gate = Arc<(Mutex<bool>, std::sync::Condvar)>;

fn open_gate(gate: &Gate) {
    *gate.0.locked() = true;
    gate.1.notify_all();
}

/// A host over a real agent whose request pipe goes through a gate (open for the handshake, closed afterwards).
fn gated_agent() -> (RemoteHost, Gate, Arc<std::sync::atomic::AtomicBool>) {
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    std::thread::spawn(move || agent::serve(req_r, resp_w));
    let gate: Gate = Arc::new((Mutex::new(true), std::sync::Condvar::new()));
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let w = Gated { inner: Box::new(req_w), gate: gate.clone(), fail: fail.clone() };
    let host = RemoteHost::from_streams(Box::new(resp_r), Box::new(w), "gated").unwrap();
    *gate.0.locked() = false;
    (host, gate, fail)
}

/// The UI thread only queues: with a 30 MB write stuck in the pipe, notifications return at once, and everything goes out
/// in order once the peer reads again.
#[test]
fn notify_does_not_wait_for_a_stuck_write() {
    let dir = tmp("notify-stuck");
    let path = dir.join("big.bin").to_string_lossy().into_owned();
    let (host, gate, _) = gated_agent();
    let host = Arc::new(host);
    let big = {
        let host = host.clone();
        std::thread::spawn(move || host.write(&path, vec![7u8; 30 * 1024 * 1024], None))
    };
    std::thread::sleep(std::time::Duration::from_millis(200));

    let started = std::time::Instant::now();
    host.pty_resize(1, 80, 24);
    host.proc_input(1, b"didChange".to_vec());
    host.cancel(99);
    assert!(started.elapsed() < std::time::Duration::from_millis(50), "{:?}", started.elapsed());
    // The heartbeat probe must not wait on the stuck write either.
    let started = std::time::Instant::now();
    assert!(!host.shared.ping(std::time::Duration::from_millis(100)));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));

    open_gate(&gate);
    assert!(big.join().unwrap().is_ok());
    assert!(host.stat(&dir.to_string_lossy()).is_ok(), "the connection works after the stall");
}

/// Frames reach the peer in the order they were queued, notifications and requests alike.
#[test]
fn queued_frames_keep_their_order() {
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    let (seen_tx, seen_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut r = BufReader::new(req_r);
        let mut w = resp_w;
        while let Ok(Some(Frame { id, body })) = proto::read_frame::<Request>(&mut r) {
            match body {
                Request::Hello { client_version, .. } => {
                    let info =
                        proto::HostInfo { version: client_version, os: "t".into(), arch: "t".into(), home: "/".into(), hostname: "t".into(), resumed: false };
                    let _ = proto::write_frame(&mut w, &Frame { id, body: Response::Hello(info) });
                }
                Request::PtyInput { data, .. } => {
                    let _ = seen_tx.send(data);
                }
                Request::Stat { .. } => {
                    let _ = seen_tx.send(b"stat".to_vec());
                    let meta = proto::Meta { kind: proto::Kind::File, size: 0, mtime_ms: 0, readonly: false, file_id: 0, created_ms: 0 };
                    let _ = proto::write_frame(&mut w, &Frame { id, body: Response::Stat(meta) });
                }
                _ => {}
            }
        }
    });
    let host = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "order").unwrap();
    for i in 0..200u32 {
        host.pty_input(1, i.to_le_bytes().to_vec());
    }
    host.stat("/x").unwrap();
    for i in 200..210u32 {
        host.pty_input(1, i.to_le_bytes().to_vec());
    }
    let wait = std::time::Duration::from_secs(5);
    for i in 0..200u32 {
        assert_eq!(seen_rx.recv_timeout(wait).unwrap(), i.to_le_bytes().to_vec());
    }
    assert_eq!(seen_rx.recv_timeout(wait).unwrap(), b"stat".to_vec());
    for i in 200..210u32 {
        assert_eq!(seen_rx.recv_timeout(wait).unwrap(), i.to_le_bytes().to_vec());
    }
}

/// A write error reaches the caller that waited for it as `Disconnected`, and later calls fail the same way.
#[test]
fn a_write_error_surfaces_as_disconnected() {
    let (host, gate, fail) = gated_agent();
    fail.store(true, Ordering::Relaxed);
    open_gate(&gate);
    let err = host.stat("/").unwrap_err();
    assert!(matches!(err, HostError::Disconnected(_)), "{err}");
    assert!(matches!(host.stat("/"), Err(HostError::Disconnected(_))));
    host.pty_input(1, b"x".to_vec()); // does not panic or block
}

/// Input is never dropped quietly: when the peer has not read for far longer than the queue holds, the connection is
/// given up on and callers see `Disconnected`.
#[test]
fn a_peer_that_never_reads_fails_the_host_instead_of_dropping_input() {
    let (host, gate, fail) = gated_agent();
    for _ in 0..40 {
        host.proc_input(1, vec![0u8; 2 * 1024 * 1024]);
    }
    assert!(host.shared.broken().is_some());
    assert!(matches!(host.stat("/"), Err(HostError::Disconnected(_))));
    // Let the stuck writer thread go (and fail) instead of leaving it parked.
    fail.store(true, Ordering::Relaxed);
    open_gate(&gate);
}

/// Dropping the host ends the writer thread: the stream it owns is dropped with it.
#[test]
fn the_writer_thread_ends_with_the_host() {
    struct Sink(Arc<std::sync::atomic::AtomicBool>);
    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Drop for Sink {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let w = outbox::Writer::start(BufWriter::new(Box::new(Sink(released.clone())))).unwrap();
    w.send(vec![1, 2, 3]).unwrap();
    assert!(!released.load(Ordering::Relaxed));
    drop(w);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !released.load(Ordering::Relaxed) {
        assert!(std::time::Instant::now() < deadline, "writer thread still alive");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
