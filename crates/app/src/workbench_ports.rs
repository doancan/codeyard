//! Workbench's port forwarding: while connected over SSH, scans the ports listening on the server
//! every few seconds; ports opened after connecting are forwarded
//! automatically (ones open from the start are only listed). The PORTS bottom panel.
//!
//! The forwards themselves live in [`ForwardSet`]; this file runs the ssh work it asks for, in order, on a background
//! thread and feeds the results back.

use std::collections::BTreeSet;
use std::time::Duration;

use gpui::{App, Context, SharedString, Window};
use host::forward::{LocalBind, RemoteTarget};

use super::{Conn, InputPurpose, LOCAL_AGENT_ALIAS, Notice, Workbench};
use crate::port_forwards::{Effect, ForwardRequest, ForwardSet, ForwardState, OnForward, Origin, Protocol, failure_text, rule_for};
use crate::ports::{self, ListeningPort, ScanMethod};

#[path = "workbench_ports_view.rs"]
mod view;
pub use view::{PortMenuCmd, PortRow, RowState, port_rows};

#[cfg(test)]
pub use crate::port_forwards::Forward;

/// Port forwarding state in the Workbench: the forwards, the ports listening on the server, the ports from the first
/// scan (not forwarded automatically), ports stopped on purpose, the scan and the manual entry field.
pub struct PortsState {
    pub set: ForwardSet,
    pub listening: Vec<ListeningPort>,
    pub baseline: Option<BTreeSet<u16>>,
    /// Stopped by the user: not forwarded automatically again on this connection.
    pub ignored: BTreeSet<u16>,
    /// Ports whose rule says to open the browser, waiting for their forward (opened once per connection).
    pub open_when_ready: BTreeSet<u16>,
    pub method: Option<ScanMethod>,
    /// Why the last scan failed (the list shown is the one before).
    pub scan_error: Option<SharedString>,
    pub task: Option<gpui::Task<()>>,
    pub field: gpui::Entity<crate::input::TextField>,
}

impl PortsState {
    pub fn new(field: gpui::Entity<crate::input::TextField>) -> Self {
        Self {
            set: ForwardSet::default(),
            listening: Vec::new(),
            baseline: None,
            ignored: BTreeSet::new(),
            open_when_ready: BTreeSet::new(),
            method: None,
            scan_error: None,
            task: None,
            field,
        }
    }
}

/// Scan interval while connected; after failed scans it backs off up to `SCAN_MAX`.
const SCAN_EVERY: Duration = Duration::from_secs(4);
const SCAN_MAX: Duration = Duration::from_secs(30);

impl Workbench {
    /// The connection port forwarding can use (ssh alias).
    pub(super) fn ssh_alias(&self) -> Option<String> {
        match &self.remote.conn {
            // Containers and WSL have no ssh connection to forward ports over.
            Conn::Connected { alias } if alias != LOCAL_AGENT_ALIAS && matches!(host::ssh::target_of(alias), host::ssh::Target::Ssh(_)) => Some(alias.clone()),
            _ => None,
        }
    }

    /// Runs the ssh work of `effects` in order on one background thread, then feeds the open results back.
    /// `quiet`: no success notice (forwards restored after a reconnect).
    pub(super) fn run_port_effects(&mut self, effects: Vec<Effect>, quiet: bool, cx: &mut Context<Self>) {
        if effects.is_empty() {
            return;
        }
        let work = cx.background_executor().spawn(async move {
            effects
                .into_iter()
                .filter_map(|e| match e {
                    Effect::Cancel { alias, spec } => {
                        let _ = host::forward::cancel(&alias, &spec); // already gone is fine
                        None
                    }
                    Effect::Open { id, seq, req } => Some((id, seq, req.alias.clone(), host::forward::open(&req))),
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let results = work.await;
            this.update(cx, |this, cx| {
                let mut more = Vec::new();
                let mut opened = Vec::new();
                for (id, seq, alias, result) in results {
                    if let Ok(o) = &result {
                        more.extend(this.ports.set.orphan_cancel(id, &alias, o));
                    }
                    let failed = result.as_ref().err().cloned();
                    more.extend(this.ports.set.on_result(id, seq, result));
                    let Some(f) = this.ports.set.get(id) else { continue };
                    match (&f.state, failed) {
                        (ForwardState::Active, _) => {
                            let local = f.local.unwrap_or(f.remote);
                            if this.ports.open_when_ready.remove(&f.remote)
                                && let Some(url) = f.protocol.url(local)
                            {
                                cx.open_url(&url);
                            }
                            opened.push((f.remote, local));
                        }
                        (ForwardState::Failed(_), Some(e)) => {
                            let why = failure_text(&e, f.wanted_local);
                            this.notice = Some(Notice::Error(crate::i18n::tf("Could not forward port {}: {}", &[&f.remote, &why]).into()));
                        }
                        _ => {}
                    }
                }
                if !quiet {
                    match opened.as_slice() {
                        [] => {}
                        [(remote, local)] if remote == local => {
                            this.notice = Some(Notice::Info(crate::i18n::tf("Port {} is forwarded: localhost:{}", &[remote, local]).into()))
                        }
                        [(remote, local)] => {
                            this.notice =
                                Some(Notice::Info(crate::i18n::tf("Port {} is forwarded to localhost:{} ({} was busy here)", &[remote, local, remote]).into()))
                        }
                        many => this.notice = Some(Notice::Info(crate::i18n::tf("{} ports are forwarded", &[&many.len()]).into())),
                    }
                }
                this.run_port_effects(more, true, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A connection was attached: forwards of another connection go, this connection's waiting ones come back; the
    /// scan starts.
    pub(super) fn attach_ports(&mut self, alias: &str, cx: &mut Context<Self>) {
        let effects = self.ports.set.on_attach(alias);
        self.run_port_effects(effects, true, cx);
        self.ports.ignored.clear();
        self.ports.open_when_ready.clear();
        self.start_ports(cx);
    }

    /// The connection dropped (it may come back): forwards wait, the scan stops.
    pub(super) fn ports_interrupted(&mut self) {
        // Standalone Windows forwards are independent of the main SSH session.
        // Stop only this window's forwards before marking them interrupted.
        #[cfg(windows)]
        for f in self.ports.set.iter().filter(|f| f.is_active()) {
            if let Some(spec) = &f.spec {
                let _ = host::forward::cancel(&f.alias, spec);
            }
        }
        self.ports.set.on_disconnected();
        self.ports.task = None;
    }

    /// The connection is back: waiting forwards open again and the scan restarts.
    pub(super) fn ports_reconnected(&mut self, cx: &mut Context<Self>) {
        let effects = self.ports.set.on_reconnected();
        self.run_port_effects(effects, true, cx);
        self.start_ports(cx);
    }

    /// Leaving the connection while it may stay open for other windows (disconnect, closing the window): cancel this
    /// window's forwards on a background thread, so closing never waits for ssh. Quitting closes the connections anyway.
    pub(super) fn cancel_all_forwards(&mut self) {
        let effects = self.ports.set.take_all_cancels();
        if effects.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            for e in effects {
                if let Effect::Cancel { alias, spec } = e {
                    let _ = host::forward::cancel(&alias, &spec);
                }
            }
        });
    }

    /// Start the scan (after connecting or reconnecting).
    pub(super) fn start_ports(&mut self, cx: &mut Context<Self>) {
        if self.ssh_alias().is_none() {
            self.stop_ports();
            return;
        }
        self.ports.method = None; // the server may differ: probe again
        self.ports.task = Some(cx.spawn(async move |this, cx| {
            let mut wait = SCAN_EVERY;
            loop {
                let Ok((host, mut method)) = this.update(cx, |this, _| (this.remote.host.clone(), this.ports.method)) else { return };
                let (found, method) = cx
                    .background_executor()
                    .spawn(async move {
                        let found = ports::detect(host.as_ref(), &mut method);
                        (found, method)
                    })
                    .await;
                wait = if found.is_ok() { SCAN_EVERY } else { (wait * 2).min(SCAN_MAX) };
                if this
                    .update(cx, |this, cx| {
                        #[cfg(windows)]
                        {
                            let exited: Vec<_> = this
                                .ports
                                .set
                                .iter()
                                .filter(|f| f.is_active())
                                .filter_map(|f| {
                                    let spec = f.spec.as_ref()?;
                                    (!host::forward::is_alive(&f.alias, spec)).then(|| (f.id, spec.clone()))
                                })
                                .collect();
                            for (id, spec) in exited {
                                this.ports.set.process_exited(id, &spec);
                            }
                        }
                        this.ports.method = method;
                        this.on_ports(found, cx)
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(wait).await;
            }
        }));
    }

    pub(super) fn stop_ports(&mut self) {
        self.ports.task = None;
        self.ports.listening.clear();
        self.ports.baseline = None;
        self.ports.scan_error = None;
    }

    fn on_ports(&mut self, found: Result<Vec<ListeningPort>, String>, cx: &mut Context<Self>) {
        let found = match found {
            Ok(found) => {
                self.ports.scan_error = None;
                found
            }
            Err(e) => {
                // Keep the last list: a failed scan is not "nothing is listening".
                self.ports.scan_error = Some(crate::i18n::tf("Could not list the server's ports: {}", &[&e]).into());
                cx.notify();
                return;
            }
        };
        let numbers: BTreeSet<u16> = found.iter().map(|p| p.port).collect();
        let auto = cx.global::<crate::settings::Settings>().ports_auto_forward;
        match &self.ports.baseline {
            // First scan: ports already open (database, ssh) are the baseline and never auto-forwarded.
            None => self.ports.baseline = Some(numbers.clone()),
            Some(base) if auto => {
                let alias = self.ssh_alias().unwrap_or_default();
                // Our own local forward ports: if the server is this machine they'd look like new ports and get
                // forwarded again in a chain.
                let ours = self.ports.set.used_locals();
                let new: Vec<&ListeningPort> = found
                    .iter()
                    .filter(|p| !base.contains(&p.port) && !self.ports.ignored.contains(&p.port) && ports::worth_forwarding(p.port))
                    .filter(|p| !self.ports.listening.iter().any(|seen| seen.port == p.port))
                    .filter(|p| self.ports.set.by_remote(&alias, p.port).is_none() && !ours.contains(&p.port))
                    .collect();
                let rules = cx.global::<crate::settings::Settings>().ports_rules.clone();
                let (mut loud, mut quiet) = (Vec::new(), Vec::new());
                for p in new {
                    let attrs = rule_for(&rules, p.port);
                    if attrs.on_forward == OnForward::Ignore {
                        continue;
                    }
                    let mut r = ForwardRequest::user(&alias, p.port, ports::forward_target(&p.binds), self.port_bind(cx));
                    (r.origin, r.label, r.protocol) = (Origin::Auto, attrs.label, attrs.protocol);
                    let effects = self.ports.set.request(r).1;
                    match attrs.on_forward {
                        OnForward::Silent => quiet.extend(effects),
                        OnForward::OpenBrowser => {
                            self.ports.open_when_ready.insert(p.port);
                            quiet.extend(effects);
                        }
                        _ => loud.extend(effects),
                    }
                }
                self.run_port_effects(loud, false, cx);
                self.run_port_effects(quiet, true, cx);
            }
            Some(_) => {}
        }
        if found != self.ports.listening {
            self.ports.listening = found;
            cx.notify();
        }
    }

    /// Local addresses new forwards listen on (`ports.localBind`).
    pub(super) fn port_bind(&self, cx: &App) -> LocalBind {
        cx.global::<crate::settings::Settings>().ports_local_bind
    }

    /// Forwards a server port (typed, clicked or detected); asking again for a forwarded port does nothing.
    pub fn forward_port(&mut self, remote: u16, cx: &mut Context<Self>) {
        self.forward_port_to(remote, None, cx);
    }

    /// Forwards a server port to exactly `local` (`None`: the same number when free, else another).
    fn forward_port_to(&mut self, remote: u16, local: Option<u16>, cx: &mut Context<Self>) {
        let Some(alias) = self.ssh_alias() else { return };
        self.ports.ignored.remove(&remote);
        let target = self.ports.listening.iter().find(|p| p.port == remote).map_or(RemoteTarget::Localhost, |p| ports::forward_target(&p.binds));
        let attrs = rule_for(&cx.global::<crate::settings::Settings>().ports_rules, remote);
        let mut r = ForwardRequest::user(&alias, remote, target, self.port_bind(cx));
        (r.label, r.protocol, r.wanted_local, r.strict) = (attrs.label, attrs.protocol, local, local.is_some());
        let (_, effects) = self.ports.set.request(r);
        self.run_port_effects(effects, false, cx);
        self.save_forward(remote);
        cx.notify();
    }

    /// Stops forwarding a server port; it is not forwarded automatically again on this connection.
    pub fn stop_forward(&mut self, remote: u16, cx: &mut Context<Self>) {
        let alias = self.ssh_alias().unwrap_or_default();
        let Some(id) = self.ports.set.by_remote(&alias, remote).map(|f| f.id) else { return };
        let effects = self.ports.set.stop(id);
        self.ports.ignored.insert(remote);
        if let (Some(s), Some(ws)) = (self.persist.state.as_ref(), self.persist.workspace_id) {
            let _ = s.delete_port_forward(ws, remote);
        }
        self.run_port_effects(effects, true, cx);
        cx.notify();
    }

    fn retry_forward(&mut self, remote: u16, cx: &mut Context<Self>) {
        let alias = self.ssh_alias().unwrap_or_default();
        let Some(id) = self.ports.set.by_remote(&alias, remote).map(|f| f.id) else { return };
        let effects = self.ports.set.retry(id);
        self.run_port_effects(effects, false, cx);
        cx.notify();
    }

    /// Number of active forwards (status bar, panel tab).
    pub(super) fn active_forwards(&self) -> usize {
        self.ports.set.iter().filter(|f| f.is_active()).count()
    }

    /// Whether the first scan is done (baseline known; ports opened afterwards are forwarded).
    #[cfg(test)]
    pub fn ports_ready(&self) -> bool {
        self.ssh_alias().is_some() && self.ports.baseline.is_some()
    }

    #[cfg(test)]
    pub fn forwards(&self) -> Vec<Forward> {
        self.ports.set.active()
    }

    #[cfg(test)]
    pub fn forward_label_for_test(&self, remote: u16) -> Option<String> {
        self.ports.set.get(self.forward_id(remote)?)?.label.clone()
    }

    #[cfg(test)]
    pub fn set_forward_label_for_test(&mut self, remote: u16, label: &str, cx: &mut Context<Self>) {
        self.finish_port_label(remote, label.into(), cx);
    }

    fn forward_id(&self, remote: u16) -> Option<crate::port_forwards::ForwardId> {
        self.ports.set.by_remote(&self.ssh_alias()?, remote).map(|f| f.id)
    }

    /// Writes a forward the user touched to the project's saved forwards (it comes back when the project opens again).
    fn save_forward(&self, remote: u16) {
        let (Some(state), Some(ws), Some(id)) = (self.persist.state.as_ref(), self.persist.workspace_id, self.forward_id(remote)) else { return };
        let Some(f) = self.ports.set.get(id) else { return };
        let saved = state::SavedForward { remote, local: f.wanted_local.filter(|_| f.strict), label: f.label.clone(), protocol: f.protocol.as_str().into() };
        let _ = state.save_port_forward(ws, &saved);
    }

    /// The project opened over SSH: forward the ports saved for it (quietly; failures show on their rows).
    pub(super) fn restore_forwards(&mut self, cx: &mut Context<Self>) {
        let (Some(alias), Some(ws)) = (self.ssh_alias(), self.persist.workspace_id) else { return };
        let Some(saved) = self.persist.state.as_ref().map(|s| s.port_forwards(ws)) else { return };
        let bind = self.port_bind(cx);
        let mut effects = Vec::new();
        for f in saved {
            if self.ports.set.by_remote(&alias, f.remote).is_some() {
                continue;
            }
            let target = self.ports.listening.iter().find(|p| p.port == f.remote).map_or(RemoteTarget::Localhost, |p| ports::forward_target(&p.binds));
            let mut r = ForwardRequest::user(&alias, f.remote, target, bind);
            (r.wanted_local, r.strict, r.label) = (f.local, f.local.is_some(), f.label);
            r.protocol = Protocol::parse(&f.protocol).unwrap_or_default();
            effects.extend(self.ports.set.request(r).1);
        }
        self.run_port_effects(effects, true, cx);
    }

    /// The "Forward a port" field: `3000`, `localhost:3000`, or `3000:8080` for server port 3000 on local port 8080.
    pub(super) fn submit_port_field(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        match parse_forward_input(text) {
            Ok((remote, local)) => {
                self.forward_port_to(remote, local, cx);
                true
            }
            Err(()) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("“{}” is not a port number", &[&text.trim()]).into()));
                cx.notify();
                false
            }
        }
    }

    pub(super) fn run_port_menu(&mut self, cmd: PortMenuCmd, remote: u16, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.forward_id(remote) else {
            if cmd == PortMenuCmd::Forward {
                self.forward_port(remote, cx);
            }
            return;
        };
        let Some(f) = self.ports.set.get(id).cloned() else { return };
        let address = f.local.map(|l| format!("localhost:{l}"));
        match cmd {
            PortMenuCmd::OpenBrowser => {
                if let Some(url) = f.local.and_then(|l| f.protocol.url(l)) {
                    cx.open_url(&url);
                }
            }
            PortMenuCmd::CopyAddress => {
                if let Some(a) = address {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(a));
                }
            }
            PortMenuCmd::SetLabel => self.show_input(
                InputPurpose::PortLabel { remote },
                crate::i18n::t("Set"),
                crate::i18n::t("label, e.g. web (empty: none)"),
                f.label.as_deref().unwrap_or(""),
                window,
                cx,
            ),
            PortMenuCmd::ChangeLocal => {
                let current = f.local.map(|l| l.to_string()).unwrap_or_default();
                self.show_input(InputPurpose::PortLocal { remote }, crate::i18n::t("Change"), crate::i18n::t("local port, e.g. 8080"), &current, window, cx)
            }
            PortMenuCmd::Protocol(p) => {
                self.ports.set.set_protocol(id, p);
                self.save_forward(remote);
            }
            PortMenuCmd::Retry => self.retry_forward(remote, cx),
            PortMenuCmd::Stop => self.stop_forward(remote, cx),
            PortMenuCmd::Forward => {}
        }
        cx.notify();
    }

    pub(super) fn finish_port_label(&mut self, remote: u16, label: String, cx: &mut Context<Self>) {
        if let Some(id) = self.forward_id(remote) {
            self.ports.set.set_label(id, &label);
            self.save_forward(remote);
            cx.notify();
        }
    }

    pub(super) fn finish_port_local(&mut self, remote: u16, text: String, cx: &mut Context<Self>) {
        let Some(id) = self.forward_id(remote) else { return };
        match text.trim().parse::<u16>() {
            Ok(local) if local > 0 && !self.ports.set.iter().any(|f| f.id != id && f.local == Some(local)) => {
                let effects = self.ports.set.change_local(id, local);
                self.run_port_effects(effects, false, cx);
                self.save_forward(remote);
            }
            _ => self.notice = Some(Notice::Error(crate::i18n::tf("“{}” is not a free local port", &[&text.trim()]).into())),
        }
        cx.notify();
    }
}

/// `3000`, `localhost:3000`, `127.0.0.1:3000` → (3000, None); `3000:8080` → server port 3000 on local port 8080.
pub fn parse_forward_input(text: &str) -> Result<(u16, Option<u16>), ()> {
    let t = text.trim();
    let t = ["localhost:", "127.0.0.1:"].iter().find_map(|p| t.strip_prefix(p)).unwrap_or(t);
    let port = |s: &str| s.trim().parse::<u16>().ok().filter(|p| *p > 0).ok_or(());
    match t.split_once(':') {
        Some((remote, local)) => Ok((port(remote)?, Some(port(local)?))),
        None => Ok((port(t)?, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forward_field_reads_a_port_or_a_pair() {
        assert_eq!(parse_forward_input(" 3000 "), Ok((3000, None)));
        assert_eq!(parse_forward_input("localhost:5173"), Ok((5173, None)));
        assert_eq!(parse_forward_input("3000:8080"), Ok((3000, Some(8080))));
        for bad in ["", "0", "70000", "web", "3000:", ":80", "1:2:3", "localhost:"] {
            assert!(parse_forward_input(bad).is_err(), "{bad:?}");
        }
    }
}
