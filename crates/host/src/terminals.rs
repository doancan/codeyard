//! A connection to a terminal daemon's session that outlives this process: the terminals it opens keep running when
//! the process ends (no `Bye` on drop), and after a restart `attach_session` takes the saved ones back. The ids of the
//! terminals to take back are registered before the first frame is read, so their output buffered while no one was
//! attached reaches them, not the floor. Taking a terminal back still needs its token (`Host::pty_attach`); a caller
//! that gets a refusal drops what arrived for that id.

use super::*;

impl RemoteHost {
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

    /// Hello with `session` on `t`, then read. When the daemon still has that session (`resumed`), a `Pty` per id in
    /// `ptys` receives that terminal's events from the first frame on. Never reconnects: a lost connection is final, as
    /// another window or process may have taken the session over.
    /// `close` ends the connection when the host is dropped, so the daemon sees it go at once.
    pub fn attach_session(
        t: Transport,
        close: Box<dyn FnOnce() + Send>,
        session: &str,
        ptys: &[u32],
        label: impl Into<String>,
    ) -> Result<(Self, bool, Vec<Pty>)> {
        let (info, reader, writer, child, stderr) = handshake(t, Some(session))?;
        let shared = Arc::new(Shared { next_id: AtomicU32::new(2), ..Default::default() });
        shared.keep_session.store(true, Ordering::Relaxed);
        *shared.closer.locked() = Some(close);
        shared.set_writer(writer).map_err(|e| HostError::Disconnected(e.to_string()))?;
        *shared.child.locked() = child;
        *shared.stderr.locked() = stderr;
        let mut taken = Vec::new();
        if info.resumed {
            let mut table = shared.ptys.locked();
            for &id in ptys {
                let (tx, rx) = mpsc::channel();
                table.insert(id, tx);
                taken.push(Pty { id, events: rx });
            }
        }
        let reader_shared = shared.clone();
        let session = session.to_string();
        std::thread::Builder::new()
            .name("host-reader".into())
            .spawn(move || reader_loop(reader_shared, reader, None, Some(session)))
            .map_err(|e| HostError::Disconnected(e.to_string()))?;
        let resumed = info.resumed;
        Ok((RemoteHost { shared, info, label: label.into(), remote: false }, resumed, taken))
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
        let (first, resumed, none) = RemoteHost::attach_session(t, close, "host-window", &[7], "Local").unwrap();
        assert!(!resumed && none.is_empty(), "a new session has nothing to take back");
        let spec = PtySpec { cols: 80, rows: 24, shell: Some("/bin/sh".into()), ..Default::default() };
        let pty = first.pty_spawn_ex(spec).unwrap();
        let (instance, secret) = first.pty_token(pty.id).unwrap();
        first.pty_input(pty.id, b"sleep 0.4; echo written-while-away\n".to_vec());
        std::thread::sleep(Duration::from_millis(100));
        drop(first); // no Bye: the session and its shell stay
        std::thread::sleep(Duration::from_millis(800));

        let (t, close) = transport(&sock);
        let (second, resumed, taken) = RemoteHost::attach_session(t, close, "host-window", &[pty.id], "Local").unwrap();
        assert!(resumed);
        assert_eq!(second.pty_attach(pty.id, instance.clone(), secret).unwrap().1, 0, "nothing lost");
        assert!(second.pty_attach(pty.id, instance, "0".repeat(32)).is_err(), "a wrong secret is refused");
        let back = &taken[0];
        read_until(back, "written-while-away");
        second.pty_input(pty.id, b"echo still-here\n".to_vec());
        read_until(back, "still-here");
        assert_eq!(second.pty_list().unwrap(), vec![pty.id]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
