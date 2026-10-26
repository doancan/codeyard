//! Workbench's port forwarding: while connected over SSH, scans the ports listening on the server
//! every few seconds; ports opened after connecting are forwarded
//! automatically (ones open from the start are only listed). The PORTS bottom panel.
//!
//! The forwards themselves live in [`ForwardSet`]; this file runs the ssh work it asks for, in order, on a background
//! thread and feeds the results back.

use std::collections::BTreeSet;
use std::time::Duration;

use gpui::{AnyElement, App, Context, MouseButton, SharedString, div, prelude::*, px};
use host::forward::{LocalBind, RemoteTarget};

use super::{Conn, LOCAL_AGENT_ALIAS, Notice, Workbench, center};
use crate::port_forwards::{Effect, ForwardRequest, ForwardSet, ForwardState, OnForward, Origin, failure_text, rule_for};
use crate::ports::{self, ListeningPort, ScanMethod};
use crate::ui::{self, Palette, c, icon, layout, text};

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
            Conn::Connected { alias } if alias != LOCAL_AGENT_ALIAS => Some(alias.clone()),
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
        let Some(alias) = self.ssh_alias() else { return };
        self.ports.ignored.remove(&remote);
        let target = self.ports.listening.iter().find(|p| p.port == remote).map_or(RemoteTarget::Localhost, |p| ports::forward_target(&p.binds));
        let attrs = rule_for(&cx.global::<crate::settings::Settings>().ports_rules, remote);
        let mut r = ForwardRequest::user(&alias, remote, target, self.port_bind(cx));
        (r.label, r.protocol) = (attrs.label, attrs.protocol);
        let (_, effects) = self.ports.set.request(r);
        self.run_port_effects(effects, false, cx);
        cx.notify();
    }

    /// Stops forwarding a server port; it is not forwarded automatically again on this connection.
    pub fn stop_forward(&mut self, remote: u16, cx: &mut Context<Self>) {
        let alias = self.ssh_alias().unwrap_or_default();
        let Some(id) = self.ports.set.by_remote(&alias, remote).map(|f| f.id) else { return };
        let effects = self.ports.set.stop(id);
        self.ports.ignored.insert(remote);
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

    pub(super) fn ports_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.ssh_alias().is_none() {
            return center(p).text_color(c(p.text_muted)).child(crate::i18n::t("Port forwarding works while connected to an SSH host.")).into_any_element();
        }
        let button = |id: SharedString, label: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(22.))
                .px(px(theme::SPACE[3]))
                .flex()
                .items_center()
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .when(primary, |d| d.bg(c(p.accent)).text_color(c(p.on_accent)))
                .when(!primary, |d| d.bg(c(p.surface_hover)).text_color(c(p.text)))
                .hover(|d| d.opacity(0.9))
                .child(crate::i18n::t(label))
        };
        let row = || div().h(px(layout::ROW_H + 6.)).flex().items_center().gap(px(theme::SPACE[4])).px(px(theme::SPACE[6]));
        let mut rows: Vec<AnyElement> = Vec::new();
        for f in self.ports.set.iter() {
            let remote = f.remote;
            let (glyph, color, detail): (&str, u32, SharedString) = match &f.state {
                ForwardState::Active => ("plug", p.success, format!("localhost:{}", f.local.unwrap_or(remote)).into()), // i18n-keep: host name and port
                ForwardState::Pending => ("refresh-cw", p.text_muted, crate::i18n::t("forwarding…").into()),
                ForwardState::Interrupted => ("plug", p.text_muted, crate::i18n::t("waiting for the connection").into()),
                ForwardState::Failed(e) => ("circle-x", p.danger, failure_text(e, f.wanted_local).into()),
            };
            let mut r = row()
                .child(icon(glyph, 14., color))
                .child(div().w(px(64.)).font_family(ui::MONO_FONT).child(remote.to_string()))
                .child(div().text_color(c(p.text_muted)).child("→"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .when(f.is_active(), |d| d.font_family(ui::MONO_FONT).text_color(c(p.accent)))
                        .when(!f.is_active(), |d| d.text_color(c(color)))
                        .child(detail),
                );
            if let (true, Some(url)) = (f.is_active(), f.local.and_then(|l| f.protocol.url(l))) {
                r = r.child(button(format!("port-open-{remote}").into(), crate::i18n::tr("Open in Browser"), true).on_click(cx.listener(
                    move |_, _, _, cx| {
                        cx.open_url(&url);
                    },
                )));
            }
            if matches!(f.state, ForwardState::Failed(_)) {
                r = r.child(button(format!("port-retry-{remote}").into(), crate::i18n::tr("Retry"), false).on_click(cx.listener(move |this, _, _, cx| {
                    this.retry_forward(remote, cx);
                })));
            }
            rows.push(
                r.child(button(format!("port-stop-{remote}").into(), crate::i18n::tr("Stop"), false).on_click(cx.listener(move |this, _, _, cx| {
                    this.stop_forward(remote, cx);
                })))
                .into_any_element(),
            );
        }
        let alias = self.ssh_alias().unwrap_or_default();
        for lp in self.ports.listening.iter().filter(|lp| self.ports.set.by_remote(&alias, lp.port).is_none()) {
            let port = lp.port;
            let what = match &lp.process {
                Some(proc) => crate::i18n::tf("{} is listening on the server", &[&proc.name]),
                None => crate::i18n::t("listening on the server").to_string(),
            };
            rows.push(
                row()
                    .child(icon("server", 14., p.text_muted))
                    .child(div().w(px(64.)).font_family(ui::MONO_FONT).child(port.to_string()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(c(p.text_muted)).child(what))
                    .child(button(format!("port-fwd-{port}").into(), crate::i18n::tr("Forward Port"), false).on_click(cx.listener(move |this, _, _, cx| {
                        this.forward_port(port, cx);
                    })))
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(row().text_color(c(p.text_muted)).child(crate::i18n::t("Nothing is listening on the server yet.")).into_any_element());
        }
        div()
            .id("ports")
            .size_full()
            .overflow_y_scroll()
            .text_size(px(text::SM.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                row()
                    .child(div().text_color(c(p.text_muted)).child(crate::i18n::t("Forward a port:")))
                    .child(ui::input_box("ports-field-box", p, p.bg, false).w(px(120.)).child(self.ports.field.clone()))
                    .children(
                        self.ports.scan_error.clone().map(|e| div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(c(p.warning)).child(e)),
                    ),
            )
            .children(rows)
            .into_any_element()
    }
}
