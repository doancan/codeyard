//! Daemon sessions that outlive this process: the terminals of a session kept for the next start keep running when
//! the process ends (no `Bye` on drop), and after a restart the saved ones are taken back. Output of terminals this
//! client has not taken back yet (what the daemon buffered while no one was attached, replayed when the session is
//! resumed) is kept per terminal, bounded, until `adopt` hands it over or the terminal is never claimed. Taking a
//! terminal back needs its token (`PtyAttach`); a refused token's buffered output is dropped unread.

use super::*;

/// Buffered output per terminal not taken back yet (the daemon keeps as much), and how many such terminals.
const ORPHAN_BYTES: usize = 256 * 1024;
const ORPHAN_TERMINALS: usize = 16;

impl Shared {
    /// An event of a terminal nobody listens to (yet).
    pub(super) fn orphan(&self, pty: u32, event: PtyEvent) {
        let mut orphans = self.orphans.locked();
        if !orphans.contains_key(&pty) && orphans.len() >= ORPHAN_TERMINALS {
            return;
        }
        let events = orphans.entry(pty).or_default();
        events.push(event);
        let mut bytes: usize = events.iter().map(|e| if let PtyEvent::Output(d) = e { d.len() } else { 0 }).sum();
        while bytes > ORPHAN_BYTES && events.len() > 1 {
            if let PtyEvent::Output(d) = events.remove(0) {
                bytes -= d.len();
            }
        }
    }
}

impl RemoteHost {
    /// Take terminal `pty` of this session back with its token (see `Host::pty_adopt`).
    pub(super) fn adopt(&self, pty: u32, instance: String, secret: String) -> Result<(Pty, u64)> {
        let (tx, rx) = mpsc::channel();
        {
            // Registered before asking, so nothing that arrives meanwhile is lost; what came before goes first.
            let mut table = self.shared.ptys.locked();
            // Already someone's: one owner per terminal.
            if table.contains_key(&pty) {
                return Err(HostError::Op(proto::Error::AlreadyExists(format!("terminal {pty}"))));
            }
            for event in self.shared.orphans.locked().remove(&pty).unwrap_or_default() {
                let _ = tx.send(event);
            }
            table.insert(pty, tx);
        }
        match self.pty_attach(pty, instance, secret) {
            Ok((_, dropped)) => Ok((Pty { id: pty, events: rx }, dropped)),
            Err(e) => {
                self.shared.ptys.locked().remove(&pty);
                Err(e)
            }
        }
    }

    /// End the session now (`Bye`): the daemon closes its terminals at once instead of keeping them for a restart.
    pub fn end_session(&self) {
        self.shared.keep_session.store(false, Ordering::Relaxed);
        self.shared.closing.store(true, Ordering::Relaxed);
        if let Ok(w) = self.shared.writer()
            && let Ok(bytes) = Shared::encode(0, &Request::Bye)
        {
            let _ = w.send_within(bytes, std::time::Duration::from_secs(1));
        }
    }

    /// Hello with `session` on `t`, then read; `true` with it when the daemon still had that session (`resumed`), whose
    /// terminals are then taken back with `pty_adopt`. Never reconnects: a lost connection is final, as another window
    /// or process may have taken the session over. `close` ends the connection when the host is dropped, so the daemon
    /// sees it go at once. The session is kept for the next start unless `end_session` is called.
    pub fn attach_session(t: Transport, close: Box<dyn FnOnce() + Send>, session: &str, label: impl Into<String>) -> Result<(Self, bool)> {
        let (info, reader, writer, child, stderr) = handshake(t, Some(session))?;
        let shared = Arc::new(Shared { next_id: AtomicU32::new(2), ..Default::default() });
        shared.keep_session.store(true, Ordering::Relaxed);
        *shared.closer.locked() = Some(close);
        *shared.session.locked() = Some(session.to_string());
        shared.set_writer(writer).map_err(|e| HostError::Disconnected(e.to_string()))?;
        *shared.child.locked() = child;
        *shared.stderr.locked() = stderr;
        let reader_shared = shared.clone();
        let session = session.to_string();
        std::thread::Builder::new()
            .name("host-reader".into())
            .spawn(move || reader_loop(reader_shared, reader, None, Some(session)))
            .map_err(|e| HostError::Disconnected(e.to_string()))?;
        let resumed = info.resumed;
        Ok((RemoteHost { shared, info, label: label.into(), remote: false }, resumed))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    fn transport(sock: &std::path::Path) -> (Transport, Box<dyn FnOnce() + Send>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match UnixStream::connect(sock) {
                Ok(s) => break s,
                Err(e) => assert!(Instant::now() < deadline, "no daemon: {e}"),
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let closer = stream.try_clone().unwrap();
        (Transport::pipes(Box::new(stream.try_clone().unwrap()), Box::new(stream)), Box::new(move || drop(closer.shutdown(std::net::Shutdown::Both))))
    }

    fn read_until(pty: &Pty, want: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        while !seen.contains(want) {
            assert!(Instant::now() < deadline, "waited for {want:?}, got {seen:?}");
            if let Ok(PtyEvent::Output(d)) = pty.events.recv_timeout(Duration::from_millis(200)) {
                seen.push_str(&String::from_utf8_lossy(&d));
            }
        }
    }

    #[test]
    fn a_dropped_client_leaves_its_terminal_and_a_new_one_takes_it_back_with_buffered_output() {
        let dir = std::env::temp_dir().join(format!("codeyard-host-reattach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sock = dir.join("t.sock");
        let s2 = sock.clone();
        std::thread::spawn(move || agent::daemon::run(&s2));

        let (t, close) = transport(&sock);
        let (first, resumed) = RemoteHost::attach_session(t, close, "host-window", "Local").unwrap();
        assert!(!resumed, "a new session has nothing to take back");
        let spec = PtySpec { cols: 80, rows: 24, shell: Some("/bin/sh".into()), ..Default::default() };
        let pty = first.pty_spawn_ex(spec).unwrap();
        let (instance, secret) = first.pty_token(pty.id).unwrap();
        first.pty_input(pty.id, b"sleep 0.4; echo written-while-away\n".to_vec());
        std::thread::sleep(Duration::from_millis(100));
        drop(first); // no Bye: the session and its shell stay
        std::thread::sleep(Duration::from_millis(800));

        let (t, close) = transport(&sock);
        let (second, resumed) = RemoteHost::attach_session(t, close, "host-window", "Local").unwrap();
        assert!(resumed);
        assert!(second.pty_attach(pty.id, instance.clone(), "0".repeat(32)).is_err(), "a wrong secret is refused");
        let (back, dropped) = second.pty_adopt(pty.id, instance, secret).unwrap();
        assert_eq!(dropped, 0, "nothing lost");
        let back = &back;
        read_until(back, "written-while-away");
        second.pty_input(pty.id, b"echo still-here\n".to_vec());
        read_until(back, "still-here");
        assert_eq!(second.pty_list().unwrap(), vec![pty.id]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A terminal not named when the session is resumed (one shown elsewhere in the window) is taken back later with
    /// `pty_adopt`: what the daemon replayed for it meanwhile is kept for it, and a wrong token gets nothing.
    #[test]
    fn a_terminal_adopted_after_the_resume_still_gets_its_buffered_output() {
        let dir = std::env::temp_dir().join(format!("codeyard-host-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sock = dir.join("t.sock");
        let s2 = sock.clone();
        std::thread::spawn(move || agent::daemon::run(&s2));

        let (t, close) = transport(&sock);
        let (first, _) = RemoteHost::attach_session(t, close, "adopt-window", "Local").unwrap();
        let spec = PtySpec { cols: 80, rows: 24, shell: Some("/bin/sh".into()), ..Default::default() };
        let pty = first.pty_spawn_ex(spec).unwrap();
        let (instance, secret) = first.pty_token(pty.id).unwrap();
        first.pty_input(pty.id, b"sleep 0.3; echo replayed-for-later\n".to_vec());
        std::thread::sleep(Duration::from_millis(100));
        drop(first);
        std::thread::sleep(Duration::from_millis(700));

        let (t, close) = transport(&sock);
        let (second, resumed) = RemoteHost::attach_session(t, close, "adopt-window", "Local").unwrap();
        assert!(resumed);
        assert_eq!(second.session().as_deref(), Some("adopt-window"));
        std::thread::sleep(Duration::from_millis(300)); // the replay arrives before anyone adopts the terminal
        let (back, dropped) = second.pty_adopt(pty.id, instance.clone(), secret).unwrap();
        assert_eq!(dropped, 0);
        read_until(&back, "replayed-for-later");
        second.pty_input(pty.id, b"echo adopted\n".to_vec());
        read_until(&back, "adopted");
        assert!(second.pty_adopt(pty.id, instance, "f".repeat(32)).is_err(), "taken already");
        second.pty_input(pty.id, b"echo still-mine\n".to_vec());
        read_until(&back, "still-mine");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
