//! The set of port forwards and their life cycle, without any I/O: every change returns the ssh work to do as
//! [`Effect`]s, and results come back through [`ForwardSet::on_result`]. Keeping this pure makes the races testable:
//! a second click while a forward opens, a stop while it opens, a result that arrives after a reconnect.
//!
//! Rules:
//! - one forward per (connection, remote port); asking again returns the same one;
//! - a forward is recorded the moment it is asked for (`Pending`), before ssh runs;
//! - every open carries a sequence number; a result for an older one is dropped, and if it opened a port that
//!   nobody wants any more, that port is cancelled again;
//! - when the connection drops, forwards wait (`Interrupted`) and are opened again on the same local port when it
//!   comes back.

use host::forward::{FailKind, ForwardError, LocalBind, OpenRequest, Opened, RemoteTarget};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ForwardId(pub u64);

/// Who started the forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Detected on the server and forwarded automatically.
    Auto,
    /// Forwarded by the user.
    User,
}

/// What the forwarded port speaks: decides the address "Open in Browser" uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    #[default]
    Http,
    Https,
    /// Not a web server: no browser action.
    Tcp,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Http => "http",
            Protocol::Https => "https",
            Protocol::Tcp => "tcp",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "http" => Some(Protocol::Http),
            "https" => Some(Protocol::Https),
            "tcp" => Some(Protocol::Tcp),
            _ => None,
        }
    }

    /// Browser address for a local port (`None` for plain TCP).
    pub fn url(self, local: u16) -> Option<String> {
        match self {
            Protocol::Tcp => None,
            p => Some(format!("{}://localhost:{local}", p.as_str())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardState {
    /// ssh is opening it.
    Pending,
    Active,
    Failed(ForwardError),
    /// The connection dropped; it is opened again on reconnect.
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForward {
    pub id: ForwardId,
    pub alias: String,
    pub remote: u16,
    pub target: RemoteTarget,
    /// Local port while active (kept while interrupted, so the same one is tried again).
    pub local: Option<u16>,
    pub wanted_local: Option<u16>,
    pub strict: bool,
    pub bind: LocalBind,
    /// Exact `-L` spec while active: the only way to cancel it.
    pub spec: Option<String>,
    /// Spec from before the connection dropped: cancelled before opening again, in case the shared connection
    /// survived and still holds it.
    stale: Option<String>,
    pub label: Option<String>,
    pub protocol: Protocol,
    pub origin: Origin,
    pub state: ForwardState,
    seq: u64,
}

impl PortForward {
    pub fn is_active(&self) -> bool {
        self.state == ForwardState::Active
    }
}

/// A forward to add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardRequest {
    pub alias: String,
    pub remote: u16,
    pub target: RemoteTarget,
    pub wanted_local: Option<u16>,
    pub strict: bool,
    pub bind: LocalBind,
    pub origin: Origin,
    pub label: Option<String>,
    pub protocol: Protocol,
}

impl ForwardRequest {
    /// A port forwarded by the user to the same local number when it is free.
    pub fn user(alias: &str, remote: u16, target: RemoteTarget, bind: LocalBind) -> Self {
        ForwardRequest {
            alias: alias.into(),
            remote,
            target,
            wanted_local: None,
            strict: false,
            bind,
            origin: Origin::User,
            label: None,
            protocol: Protocol::default(),
        }
    }
}

/// ssh work the caller must run; results of `Open` go back to [`ForwardSet::on_result`] with the same id and sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Open { id: ForwardId, seq: u64, req: OpenRequest },
    Cancel { alias: String, spec: String },
}

/// Read-only view of an active forward (tests).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forward {
    pub remote: u16,
    pub local: u16,
}

/// Longest label kept.
pub const MAX_LABEL: usize = 64;

/// What happens when a port is detected on the server (`ports.rules` → `onForward`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnForward {
    /// Forward it and say so.
    #[default]
    Notify,
    /// Forward it quietly.
    Silent,
    /// Forward it and open it in the browser (once per connection).
    OpenBrowser,
    /// Do not forward it automatically.
    Ignore,
}

impl OnForward {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "notify" => Some(OnForward::Notify),
            "silent" => Some(OnForward::Silent),
            "openBrowser" => Some(OnForward::OpenBrowser),
            "ignore" => Some(OnForward::Ignore),
            _ => None,
        }
    }
}

/// One `ports.rules` entry: a port or a range and what to use for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortRule {
    pub from: u16,
    pub to: u16,
    pub label: Option<String>,
    pub on_forward: Option<OnForward>,
    pub protocol: Option<Protocol>,
}

/// The rule values that apply to one port.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortAttrs {
    pub label: Option<String>,
    pub on_forward: OnForward,
    pub protocol: Protocol,
}

/// Values for `port`: the narrowest matching rule wins (a single port beats a range), the first one on a tie.
pub fn rule_for(rules: &[PortRule], port: u16) -> PortAttrs {
    let best = rules.iter().filter(|r| (r.from..=r.to).contains(&port)).min_by_key(|r| r.to - r.from);
    best.map_or_else(PortAttrs::default, |r| PortAttrs {
        label: r.label.clone(),
        on_forward: r.on_forward.unwrap_or_default(),
        protocol: r.protocol.unwrap_or_default(),
    })
}

/// `ports.rules` from the settings file: `{ "3000": { "label": "web", "onForward": "openBrowser", "protocol": "http" },
/// "9000-9100": { "onForward": "ignore" } }`. A bad entry is skipped with a warning; the others still apply.
pub fn parse_rules(map: &serde_json::Map<String, serde_json::Value>, warn: &mut Vec<String>) -> Vec<PortRule> {
    let port = |s: &str| s.trim().parse::<u16>().ok().filter(|p| *p > 0);
    let mut out = Vec::new();
    for (key, value) in map {
        let range = match key.split_once('-') {
            Some((a, b)) => port(a).zip(port(b)).filter(|(a, b)| a <= b),
            None => port(key).map(|p| (p, p)),
        };
        let Some((from, to)) = range else {
            warn.push(crate::i18n::tf("ports.rules.{}: expected a port or a range such as 3000-3010", &[key]));
            continue;
        };
        let Some(obj) = value.as_object() else {
            warn.push(crate::i18n::tf("ports.rules.{}: expected an object with label, onForward or protocol", &[key]));
            continue;
        };
        let mut rule = PortRule { from, to, label: None, on_forward: None, protocol: None };
        for (field, v) in obj {
            let ok = match (field.as_str(), v.as_str()) {
                ("label", Some(l)) => {
                    rule.label = clean_label(l);
                    true
                }
                ("onForward", Some(o)) => OnForward::parse(o).map(|o| rule.on_forward = Some(o)).is_some(),
                ("protocol", Some(p)) => Protocol::parse(p).map(|p| rule.protocol = Some(p)).is_some(),
                _ => false,
            };
            if !ok {
                warn.push(crate::i18n::tf("ports.rules.{}.{}: unexpected value", &[key, field]));
            }
        }
        out.push(rule);
    }
    out
}

/// A label as stored: one printable line, capped; empty means none.
pub fn clean_label(raw: &str) -> Option<String> {
    let s: String = raw.chars().filter(|c| !c.is_control()).take(MAX_LABEL).collect();
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

#[derive(Debug, Default)]
pub struct ForwardSet {
    items: Vec<PortForward>,
    next_id: u64,
    next_seq: u64,
}

impl ForwardSet {
    pub fn iter(&self) -> impl Iterator<Item = &PortForward> {
        self.items.iter()
    }

    pub fn get(&self, id: ForwardId) -> Option<&PortForward> {
        self.items.iter().find(|f| f.id == id)
    }

    fn get_mut(&mut self, id: ForwardId) -> Option<&mut PortForward> {
        self.items.iter_mut().find(|f| f.id == id)
    }

    pub fn by_remote(&self, alias: &str, remote: u16) -> Option<&PortForward> {
        self.items.iter().find(|f| f.alias == alias && f.remote == remote)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[cfg(test)]
    pub fn active(&self) -> Vec<Forward> {
        self.items.iter().filter(|f| f.is_active()).filter_map(|f| Some(Forward { remote: f.remote, local: f.local? })).collect()
    }

    /// Local ports held or about to be held by these forwards (a scan of a server that is this computer must not
    /// forward them again in a chain).
    pub fn used_locals(&self) -> std::collections::BTreeSet<u16> {
        self.items.iter().filter_map(|f| f.local.or(f.wanted_local)).collect()
    }

    /// Marks the forward `Pending` and returns the open to run (after cancelling a spec left from before a drop).
    fn open(&mut self, id: ForwardId) -> Vec<Effect> {
        self.next_seq += 1;
        let seq = self.next_seq;
        let Some(f) = self.get_mut(id) else { return Vec::new() };
        f.seq = seq;
        f.state = ForwardState::Pending;
        let mut out: Vec<Effect> = f.stale.take().map(|spec| Effect::Cancel { alias: f.alias.clone(), spec }).into_iter().collect();
        let req = OpenRequest { alias: f.alias.clone(), remote: f.remote, target: f.target, wanted_local: f.wanted_local, strict: f.strict, bind: f.bind };
        out.push(Effect::Open { id, seq, req });
        out
    }

    /// Adds a forward, or returns the existing one for this port (opened again when it had failed or was waiting).
    pub fn request(&mut self, r: ForwardRequest) -> (ForwardId, Vec<Effect>) {
        if let Some(f) = self.items.iter().find(|f| f.alias == r.alias && f.remote == r.remote) {
            let id = f.id;
            return match f.state {
                ForwardState::Pending | ForwardState::Active => (id, Vec::new()),
                ForwardState::Failed(_) | ForwardState::Interrupted => (id, self.open(id)),
            };
        }
        self.next_id += 1;
        let id = ForwardId(self.next_id);
        self.items.push(PortForward {
            id,
            alias: r.alias,
            remote: r.remote,
            target: r.target,
            local: None,
            wanted_local: r.wanted_local,
            strict: r.strict,
            bind: r.bind,
            spec: None,
            stale: None,
            label: r.label.as_deref().and_then(clean_label),
            protocol: r.protocol,
            origin: r.origin,
            state: ForwardState::Pending,
            seq: 0,
        });
        (id, self.open(id))
    }

    /// Result of an `Open` effect. A stale or unwanted success is cancelled right away.
    pub fn on_result(&mut self, id: ForwardId, seq: u64, result: Result<Opened, ForwardError>) -> Vec<Effect> {
        // A stale open with the same spec as the live forward is that forward: cancelling it would close the live one.
        let alias = self.get(id).filter(|f| result.as_ref().map_or(true, |o| f.spec.as_deref() != Some(o.spec.as_str()))).map(|f| f.alias.clone());
        let current = self.get_mut(id).filter(|f| f.seq == seq && f.state == ForwardState::Pending);
        match (current, result) {
            (Some(f), Ok(opened)) => {
                f.local = Some(opened.local);
                f.spec = Some(opened.spec);
                f.state = ForwardState::Active;
                Vec::new()
            }
            (Some(f), Err(e)) => {
                f.state = ForwardState::Failed(e);
                Vec::new()
            }
            // Stopped, replaced or interrupted meanwhile: give the port back. A removed forward is cancelled by the
            // caller, which knows the alias it opened on (see `orphan_cancel`).
            (None, Ok(opened)) => alias.map(|alias| Effect::Cancel { alias, spec: opened.spec }).into_iter().collect(),
            (None, Err(_)) => Vec::new(),
        }
    }

    /// Cancel for a successful open whose forward no longer exists (the caller knows the alias it opened on).
    pub fn orphan_cancel(&self, id: ForwardId, alias: &str, opened: &Opened) -> Option<Effect> {
        self.get(id).is_none().then(|| Effect::Cancel { alias: alias.into(), spec: opened.spec.clone() })
    }

    /// Removes a forward; an active one is cancelled, an opening one is cancelled when its result arrives.
    pub fn stop(&mut self, id: ForwardId) -> Vec<Effect> {
        let Some(i) = self.items.iter().position(|f| f.id == id) else { return Vec::new() };
        let f = self.items.remove(i);
        match (f.state, f.spec) {
            (ForwardState::Active, Some(spec)) => vec![Effect::Cancel { alias: f.alias, spec }],
            _ => Vec::new(),
        }
    }

    /// Opens a failed or waiting forward again.
    pub fn retry(&mut self, id: ForwardId) -> Vec<Effect> {
        match self.get(id).map(|f| &f.state) {
            Some(ForwardState::Failed(_) | ForwardState::Interrupted) => self.open(id),
            _ => Vec::new(),
        }
    }

    pub fn set_label(&mut self, id: ForwardId, label: &str) {
        if let Some(f) = self.get_mut(id) {
            f.label = clean_label(label);
        }
    }

    pub fn set_protocol(&mut self, id: ForwardId, protocol: Protocol) {
        if let Some(f) = self.get_mut(id) {
            f.protocol = protocol;
        }
    }

    /// Moves a forward to the local port the user chose: the old one is cancelled, the new one must be exactly `port`.
    pub fn change_local(&mut self, id: ForwardId, port: u16) -> Vec<Effect> {
        let Some(f) = self.get_mut(id) else { return Vec::new() };
        let mut out = Vec::new();
        if let (ForwardState::Active, Some(spec)) = (&f.state, f.spec.take()) {
            out.push(Effect::Cancel { alias: f.alias.clone(), spec });
        }
        f.local = None;
        f.wanted_local = Some(port);
        f.strict = true;
        out.extend(self.open(id));
        out
    }

    /// A connection was attached: forwards of another connection are cancelled and dropped; this connection's
    /// waiting forwards open again.
    pub fn on_attach(&mut self, alias: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        let (keep, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.items).into_iter().partition(|f| f.alias == alias);
        for f in gone {
            if let (ForwardState::Active, Some(spec)) = (f.state, f.spec) {
                out.push(Effect::Cancel { alias: f.alias, spec });
            }
        }
        self.items = keep;
        out.extend(self.on_reconnected());
        out
    }

    /// The connection dropped: its forwards are gone with it and wait for a reconnect; opens in flight are dropped.
    pub fn on_disconnected(&mut self) {
        self.next_seq += 1;
        for f in &mut self.items {
            if matches!(f.state, ForwardState::Active | ForwardState::Pending) {
                f.state = ForwardState::Interrupted;
                f.stale = f.spec.take().or(f.stale.take());
                f.seq = 0;
            }
        }
    }

    /// An independently managed SSH forward exited while the main connection stayed up.
    #[cfg(windows)]
    pub fn process_exited(&mut self, id: ForwardId, spec: &str) {
        if let Some(f) = self.get_mut(id)
            && f.state == ForwardState::Active
            && f.spec.as_deref() == Some(spec)
        {
            f.spec = None;
            f.state = ForwardState::Failed(ForwardError { kind: host::forward::FailKind::Other, message: "SSH forwarding process exited".into() });
        }
    }

    /// The connection is back: waiting forwards open again, on the local port they had when possible.
    pub fn on_reconnected(&mut self) -> Vec<Effect> {
        let waiting: Vec<ForwardId> = self.items.iter().filter(|f| f.state == ForwardState::Interrupted).map(|f| f.id).collect();
        let mut out = Vec::new();
        for id in waiting {
            if let Some(f) = self.get_mut(id) {
                f.wanted_local = f.local.or(f.wanted_local);
            }
            out.extend(self.open(id));
        }
        out
    }

    /// Specs to cancel when this window leaves the connection while it stays open (it may be shared with others).
    pub fn take_all_cancels(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.items)
            .into_iter()
            .filter_map(|f| match (f.state, f.spec) {
                (ForwardState::Active, Some(spec)) => Some(Effect::Cancel { alias: f.alias, spec }),
                _ => None,
            })
            .collect()
    }
}

/// Failure text to show for a forward error.
pub fn failure_text(e: &ForwardError, local: Option<u16>) -> String {
    match e.kind {
        FailKind::AddrInUse => match local {
            Some(p) => crate::i18n::tf("Local port {} is busy", &[&p]),
            None => crate::i18n::t("The local port is busy").to_string(),
        },
        FailKind::Privileged => crate::i18n::t("Ports below 1024 need extra privileges on this computer").to_string(),
        FailKind::MasterGone => crate::i18n::t("The SSH connection is gone: reconnect to forward again").to_string(),
        _ => e.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(remote: u16) -> ForwardRequest {
        ForwardRequest::user("box", remote, RemoteTarget::Localhost, LocalBind::Loopback)
    }

    fn opened(local: u16) -> Result<Opened, ForwardError> {
        Ok(Opened { local, spec: format!("localhost:{local}:localhost:{local}") })
    }

    fn open_of(effects: &[Effect]) -> (ForwardId, u64) {
        match effects {
            [.., Effect::Open { id, seq, .. }] => (*id, *seq),
            other => panic!("expected an open: {other:?}"),
        }
    }

    #[test]
    fn asking_twice_for_a_port_opens_it_once() {
        let mut set = ForwardSet::default();
        let (a, e1) = set.request(req(3000));
        let (b, e2) = set.request(req(3000));
        assert_eq!(a, b);
        assert_eq!(e1.len(), 1);
        assert!(e2.is_empty(), "the second click while opening does nothing");
        assert_eq!(set.get(a).unwrap().state, ForwardState::Pending, "recorded before ssh runs");
        assert!(set.used_locals().is_empty() || set.used_locals().contains(&3000));
    }

    #[test]
    fn a_result_activates_and_stop_cancels_the_exact_spec() {
        let mut set = ForwardSet::default();
        let (id, e) = set.request(req(3000));
        let (_, seq) = open_of(&e);
        assert!(set.on_result(id, seq, opened(3001)).is_empty());
        assert_eq!(set.active(), vec![Forward { remote: 3000, local: 3001 }]);
        assert_eq!(set.stop(id), vec![Effect::Cancel { alias: "box".into(), spec: "localhost:3001:localhost:3001".into() }]);
        assert!(set.is_empty());
    }

    #[test]
    fn a_port_opened_after_its_stop_is_given_back() {
        let mut set = ForwardSet::default();
        let (id, e) = set.request(req(3000));
        let (_, seq) = open_of(&e);
        assert!(set.stop(id).is_empty(), "nothing to cancel yet");
        let late = opened(3000).unwrap();
        assert!(set.on_result(id, seq, Ok(late.clone())).is_empty(), "the set no longer knows the alias");
        assert_eq!(set.orphan_cancel(id, "box", &late), Some(Effect::Cancel { alias: "box".into(), spec: late.spec }));
    }

    #[test]
    fn a_stale_result_after_a_reconnect_is_cancelled_not_trusted() {
        let mut set = ForwardSet::default();
        let (id, e) = set.request(req(3000));
        let (_, old_seq) = open_of(&e);
        set.on_disconnected();
        let reopen = set.on_reconnected();
        let (_, new_seq) = open_of(&reopen);
        assert_ne!(old_seq, new_seq);
        let back = set.on_result(id, old_seq, opened(3000));
        assert!(matches!(back.as_slice(), [Effect::Cancel { .. }]), "the old open is undone: {back:?}");
        assert_eq!(set.get(id).unwrap().state, ForwardState::Pending, "still waiting for the new open");
        set.on_result(id, new_seq, opened(3000));
        assert!(set.get(id).unwrap().is_active());
        // The other order: the new open wins first with the same spec; the late old result must not close it.
        set.on_disconnected();
        let (_, newer) = open_of(&set.on_reconnected());
        set.on_result(id, newer, opened(3000));
        assert!(set.on_result(id, new_seq, opened(3000)).is_empty(), "same spec as the live forward: left alone");
        assert!(set.get(id).unwrap().is_active());
    }

    #[test]
    fn a_reconnect_reopens_on_the_same_local_port_and_reports_failures_per_forward() {
        let mut set = ForwardSet::default();
        let (id, e) = set.request(req(5173));
        set.on_result(id, open_of(&e).1, opened(5174));
        set.on_disconnected();
        assert_eq!(set.get(id).unwrap().state, ForwardState::Interrupted);
        assert!(set.active().is_empty());
        let reopen = set.on_reconnected();
        assert_eq!(reopen[0], Effect::Cancel { alias: "box".into(), spec: "localhost:5174:localhost:5174".into() }, "the old spec goes first");
        let Effect::Open { seq, req, .. } = &reopen[1] else { panic!("{reopen:?}") };
        assert_eq!(req.wanted_local, Some(5174), "prefers the port it had");
        let busy = ForwardError { kind: FailKind::AddrInUse, message: "busy".into() };
        set.on_result(id, *seq, Err(busy.clone()));
        assert_eq!(set.get(id).unwrap().state, ForwardState::Failed(busy));
        assert_eq!(set.retry(id).len(), 1);
    }

    #[test]
    fn changing_the_local_port_cancels_the_old_one_and_is_strict() {
        let mut set = ForwardSet::default();
        let (id, e) = set.request(req(8080));
        set.on_result(id, open_of(&e).1, opened(8080));
        let effects = set.change_local(id, 9090);
        assert!(matches!(&effects[0], Effect::Cancel { spec, .. } if spec.contains(":8080:")));
        let Effect::Open { req, .. } = &effects[1] else { panic!("{effects:?}") };
        assert_eq!((req.wanted_local, req.strict), (Some(9090), true));
    }

    #[test]
    fn attaching_another_connection_cancels_the_old_forwards() {
        let mut set = ForwardSet::default();
        let (id, e) = set.request(req(3000));
        set.on_result(id, open_of(&e).1, opened(3000));
        let effects = set.on_attach("other");
        assert_eq!(effects, vec![Effect::Cancel { alias: "box".into(), spec: "localhost:3000:localhost:3000".into() }]);
        assert!(set.is_empty());
        // Same connection again: waiting forwards come back.
        let (id, e) = set.request(req(4000));
        set.on_result(id, open_of(&e).1, opened(4000));
        set.on_disconnected();
        assert!(matches!(set.on_attach("box").as_slice(), [Effect::Cancel { .. }, Effect::Open { .. }]));
    }

    #[test]
    fn random_event_orders_never_leave_a_pending_forward_without_an_open_in_flight() {
        // Small deterministic generator: no new dependency.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..200 {
            let mut set = ForwardSet::default();
            let mut inflight: Vec<(ForwardId, u64)> = Vec::new();
            let track = |effects: Vec<Effect>, inflight: &mut Vec<(ForwardId, u64)>| {
                for e in effects {
                    if let Effect::Open { id, seq, .. } = e {
                        inflight.push((id, seq));
                    }
                }
            };
            for _ in 0..60 {
                match next(7) {
                    0 | 1 => {
                        let (_, e) = set.request(req(3000 + next(4) as u16));
                        track(e, &mut inflight);
                    }
                    2 if !inflight.is_empty() => {
                        let (id, seq) = inflight.remove(next(inflight.len() as u64) as usize);
                        let r = if next(3) == 0 { Err(ForwardError { kind: FailKind::Other, message: "x".into() }) } else { opened(4000 + next(9) as u16) };
                        let _ = set.on_result(id, seq, r);
                    }
                    3 => {
                        let pick = next(4) as usize;
                        let chosen = set.iter().map(|f| f.id).nth(pick);
                        if let Some(id) = chosen {
                            let _ = set.stop(id);
                        }
                    }
                    4 => set.on_disconnected(),
                    5 => track(set.on_reconnected(), &mut inflight),
                    _ => {
                        let first = set.iter().map(|f| f.id).next();
                        if let Some(id) = first {
                            track(set.retry(id), &mut inflight);
                        }
                    }
                }
                for f in set.iter().filter(|f| f.state == ForwardState::Pending) {
                    assert!(inflight.iter().any(|(id, seq)| *id == f.id && *seq == f.seq), "pending without an open: {f:?}");
                }
                for f in set.iter() {
                    assert_eq!(f.is_active(), f.spec.is_some(), "a spec exactly while active: {f:?}");
                }
            }
        }
    }

    #[test]
    fn port_rules_parse_with_warnings_and_the_narrowest_wins() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{ "3000": { "label": "web", "onForward": "openBrowser" }, "3000-3999": { "protocol": "https", "label": "range" },
                 "9000-9100": { "onForward": "ignore" }, "abc": {}, "10-5": {}, "0": {}, "70000": {}, "4000": 3,
                 "4001": { "onForward": "sometimes", "protocol": "gopher", "label": 5, "extra": true } }"#,
        )
        .unwrap();
        let mut warn = Vec::new();
        let rules = parse_rules(json.as_object().unwrap(), &mut warn);
        assert_eq!(rules.len(), 4, "{rules:?}");
        assert_eq!(warn.len(), 9, "every bad key or value is reported: {warn:?}");
        let web = rule_for(&rules, 3000);
        assert_eq!((web.label.as_deref(), web.on_forward, web.protocol), (Some("web"), OnForward::OpenBrowser, Protocol::Http), "a single port beats a range");
        assert_eq!(rule_for(&rules, 3500).protocol, Protocol::Https);
        assert_eq!(rule_for(&rules, 9050).on_forward, OnForward::Ignore);
        assert_eq!(rule_for(&rules, 80), PortAttrs::default());
    }

    #[test]
    fn labels_are_one_clean_short_line() {
        assert_eq!(clean_label("  web\x1b[2J\n "), Some("web[2J".into()));
        assert_eq!(clean_label("\n\t"), None);
        assert_eq!(clean_label(&"x".repeat(500)).unwrap().len(), MAX_LABEL);
        assert_eq!(Protocol::Https.url(8443).as_deref(), Some("https://localhost:8443"));
        assert_eq!(Protocol::Tcp.url(5432), None);
    }
}
