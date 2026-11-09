//! Starting a terminal's shell, or taking one back from the terminal daemon after a restart, and the loop that feeds the
//! shell's output into the view.

use super::*;

/// The most output one turn of the pump parses before the window gets to draw.
const TURN_BYTES: usize = 256 * 1024;
/// The least time between two turns while output keeps coming (a little under a 60 Hz frame).
const FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(12);

impl TerminalView {
    /// A new shell on the terminal daemon (`host`), whose token is kept so a restart can take it back.
    pub fn kept(host: Arc<dyn Host>, cwd: Option<String>, launch: Launch, cx: &mut Context<Self>) -> Self {
        Self::start(host, cwd, launch, true, cx)
    }

    /// A new shell running `launch`; `keep`: it runs on the terminal daemon, so ask for its token once it is up.
    pub(super) fn start(host: Arc<dyn Host>, cwd: Option<String>, launch: Launch, keep: bool, cx: &mut Context<Self>) -> Self {
        let Launch { shell, args, env, color } = launch;
        let mut view = Self::blank(host.clone(), cwd.clone(), color, cx);
        view.launched = (shell.clone(), args.clone());
        view.has_shell = true;
        view.starting = true;
        // Only to end a shell that comes up after its view is gone; never keeps the connection open itself.
        let cleanup_host = Arc::downgrade(&host);
        let (cols, rows) = (view.size.cols as u16, view.size.rows as u16);
        let integration = cx.global::<crate::settings::Settings>().terminal_shell_integration;
        let spawn = cx.background_executor().spawn(async move {
            match (&args, env.is_empty(), integration) {
                (None, true, false) => host.pty_spawn(cols, rows, cwd, shell),
                _ => host.pty_spawn_ex(host::PtySpec { cols, rows, cwd, shell, args, env, integration }),
            }
        });
        cx.spawn(async move |this, cx| {
            let pty = match spawn.await {
                Ok(p) => p,
                Err(e) => {
                    this.update(cx, |t, cx| {
                        t.starting = false;
                        t.error = Some(crate::i18n::tf("Could not start a shell: {}", &[&e]).into());
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let id = pty.id;
            let ok = this.update(cx, |t, cx| {
                if t.closed.get() {
                    // Closed while the shell was starting: the late shell must not keep running.
                    t.host.pty_kill(id);
                    return false;
                }
                t.pty = Some(id);
                t.starting = false;
                // Report it if the view got its size in the meantime.
                t.host.pty_resize(id, t.size.cols as u16, t.size.rows as u16);
                let pending = std::mem::take(&mut t.pending_input);
                if !pending.is_empty() {
                    t.host.pty_input(id, pending);
                }
                cx.notify();
                true
            });
            match ok {
                Ok(true) => {}
                Ok(false) => return,
                Err(_) => {
                    // The view is gone: end the late shell if its connection is still there.
                    if let Some(host) = cleanup_host.upgrade() {
                        host.pty_kill(id);
                    }
                    return;
                }
            }
            if keep {
                Self::fetch_token(this.clone(), id, cx);
            }
            Self::pump(this, pty, cx).await;
        })
        .detach();
        view
    }

    /// Feed the terminal's events into the view until the shell exits or the view is gone.
    pub(super) async fn pump(this: gpui::WeakEntity<Self>, pty: host::Pty, cx: &mut gpui::AsyncApp) {
        // Output pump: wait, then parse whatever piled up in one go (no frame per chunk).
        use futures::StreamExt;
        // If the UI lags (e.g. `yes` output), pending output must not grow without bound: whatever exceeds 32 MB is dropped and a note
        // is left (the terminal's scrollback is only 2,000 lines anyway). The exit event is never dropped.
        let backlog = Arc::new(OutputBacklog::default());
        let gate = backlog.clone();
        let mut events = crate::ui::bridge_with(pty.events, move |ev| match ev {
            PtyEvent::Output(d) => gate.admit(d.len()),
            PtyEvent::Exit(_) => true,
        });
        // How long until a synchronized update that has not ended is applied anyway (`Terminal::release_expired_sync`).
        let mut sync_left: Option<std::time::Duration> = None;
        loop {
            let first = match sync_left {
                None => events.next().await,
                Some(left) => match futures::future::select(events.next(), cx.background_executor().timer(left)).await {
                    futures::future::Either::Left((ev, _)) => ev,
                    futures::future::Either::Right(_) => {
                        let Ok(rest) = this.update(cx, |t, cx| {
                            let rest = t.release_expired_sync();
                            cx.notify();
                            rest
                        }) else {
                            return;
                        };
                        sync_left = rest;
                        continue;
                    }
                },
            };
            let Some(first) = first else { break };
            let turn = std::time::Instant::now();
            // One turn takes what piled up, up to a byte budget: a flood is parsed in slices with the window drawn in between.
            let (mut batch, mut bytes) = (Vec::new(), 0usize);
            let mut next = Some(first);
            while let Some(ev) = next.take() {
                bytes += match &ev {
                    PtyEvent::Output(d) => d.len(),
                    PtyEvent::Exit(_) => 0,
                };
                batch.push(ev);
                if bytes < TURN_BYTES && batch.len() < 256 {
                    next = events.try_recv().ok();
                }
            }
            let mut exited = false;
            let alive = this
                .update(cx, |t, cx| {
                    // The output of the turn is parsed as one piece: the work around parsing (selection, marks, find) runs once.
                    let mut output = Vec::with_capacity(bytes);
                    for ev in batch {
                        match ev {
                            PtyEvent::Output(data) => {
                                backlog.consumed(data.len());
                                output.extend_from_slice(&data);
                            }
                            PtyEvent::Exit(code) => {
                                t.feed(&std::mem::take(&mut output));
                                t.on_exit(code, cx);
                                exited = true;
                            }
                        }
                    }
                    t.feed(&output);
                    if backlog.take_dropped() {
                        t.feed(b"\x18\r\n\x1b[2m[output was dropped: the terminal could not keep up]\x1b[0m\r\n");
                    }
                    t.drain_events(cx);
                    cx.notify();
                    t.release_expired_sync()
                })
                .map(|left| sync_left = left)
                .is_ok();
            if !alive || exited {
                return;
            }
            // Drawing follows every turn. Output arriving faster than the display shows is gathered for the next turn, so a
            // busy program costs one frame per interval rather than one per chunk.
            // (Tests run on a clock that only moves when told to: a wait here would never end.)
            if let Some(rest) = FRAME_INTERVAL.checked_sub(turn.elapsed()).filter(|_| !cfg!(test)) {
                cx.background_executor().timer(rest).await;
            }
        }
        this.update(cx, |t, cx| t.on_exit(None, cx)).ok();
    }

    /// Ask the daemon for the token of terminal `id` (off the UI thread) and keep it on the view.
    fn fetch_token(this: gpui::WeakEntity<Self>, id: u32, cx: &mut gpui::AsyncApp) {
        let Ok(host) = this.read_with(cx, |t, _| t.host.clone()) else { return };
        cx.spawn(async move |cx| {
            // Without its token a terminal cannot be taken back after a restart: a busy connection gets a few more tries.
            for wait in [0, 1, 3, 10] {
                if wait > 0 {
                    cx.background_executor().timer(std::time::Duration::from_secs(wait)).await;
                }
                let host = host.clone();
                if let Ok((instance, secret)) = cx.background_executor().spawn(async move { host.pty_token(id) }).await {
                    this.update(cx, |t, cx| {
                        t.token = Some(crate::terminal_session::SavedPty { pty: id, instance, secret });
                        cx.emit(TerminalEvent::Kept);
                    })
                    .ok();
                    return;
                }
                if this.read_with(cx, |t, _| t.has_exited()).unwrap_or(true) {
                    return;
                }
            }
        })
        .detach();
    }

    /// A terminal taken back from the terminal daemon: `pty` already receives its events (output buffered while no
    /// window was attached comes first). A dimmed line says it was reconnected, and how much output was lost meanwhile.
    pub fn reattached(host: Arc<dyn Host>, pty: host::Pty, saved: &crate::terminal_session::SavedTerminal, dropped: u64, cx: &mut Context<Self>) -> Self {
        let mut view = Self::blank(host, saved.cwd.clone(), saved.color.clone(), cx);
        view.launched = (saved.shell.clone(), saved.args.clone());
        view.has_shell = true;
        view.pty = Some(pty.id);
        view.token = saved.token.clone();
        if let Some(title) = &saved.title {
            view.set_fixed_title(title.clone());
        }
        let mut notice = crate::i18n::t("Reconnected to terminal session").to_string();
        if dropped > 0 {
            notice.push_str(&format!(" ({})", crate::i18n::tf("{} bytes of output were lost while no window was attached", &[&dropped])));
        }
        view.feed(format!("\x1b[2m{notice}\x1b[0m\r\n").as_bytes());
        view.host.pty_resize(pty.id, view.size.cols as u16, view.size.rows as u16);
        cx.spawn(async move |this, cx| Self::pump(this, pty, cx).await).detach();
        view
    }

    /// The token to take this terminal back after a restart, when the terminal daemon keeps it.
    pub fn token(&self) -> Option<&crate::terminal_session::SavedPty> {
        self.token.as_ref()
    }
}
