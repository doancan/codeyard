//! Port forwards over the shared ControlMaster connection (`ssh -O forward` / `-O cancel`).
//!
//! A forward is identified by its exact `-L` spec: ssh cancels only a spec it was given, so every forward keeps the
//! string it was opened with ([`Opened::spec`]) and cancels with that. Specs are built only from typed values (ports,
//! addresses, enums), never from text that came from the server or the user.

use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
use std::process::Stdio;

use crate::ssh::{Target, check_target, ssh_base, target_of};

#[cfg(windows)]
#[path = "windows_forward.rs"]
mod windows;

/// Stop all standalone forwards when the owning connection closes.
#[cfg(windows)]
pub(crate) fn close_windows_forwards(alias: &str) {
    windows::close_alias(alias);
}

/// Stop all standalone forwards on application exit.
#[cfg(windows)]
pub(crate) fn close_all_windows_forwards() {
    windows::close_all();
}

/// Whether a Windows standalone forwarding process still owns its tunnel.
#[cfg(windows)]
pub fn is_alive(alias: &str, spec: &str) -> bool {
    windows::alive(alias, spec)
}

/// Where the server side of a forward connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteTarget {
    /// Resolved on the server; ssh tries each address (right for a listener on every interface).
    Localhost,
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

impl std::fmt::Display for RemoteTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteTarget::Localhost => f.write_str("localhost"),
            RemoteTarget::V4(a) => write!(f, "{a}"),
            RemoteTarget::V6(a) => write!(f, "[{a}]"),
        }
    }
}

/// Which local addresses a forward listens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LocalBind {
    /// This computer only.
    #[default]
    Loopback,
    /// Every interface: other computers on the network can connect.
    All,
}

impl LocalBind {
    fn address(self) -> &'static str {
        match self {
            // ssh binds both loopback addresses (IPv4 and IPv6) for "localhost".
            LocalBind::Loopback => "localhost",
            LocalBind::All => "0.0.0.0",
        }
    }
}

/// A forward to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub alias: String,
    pub remote: u16,
    pub target: RemoteTarget,
    /// Preferred local port (`None`: the same number as the remote one when free).
    pub wanted_local: Option<u16>,
    /// The user picked the local port: fail rather than use another one.
    pub strict: bool,
    pub bind: LocalBind,
}

/// An open forward: the local port it listens on and the spec that cancels it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub local: u16,
    pub spec: String,
}

/// Why a forward could not be opened or closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailKind {
    /// The local port is taken.
    AddrInUse,
    /// Ports below 1024 need privileges on this computer.
    Privileged,
    /// The shared connection is gone (reconnect first).
    MasterGone,
    /// The server refused the forward.
    Rejected,
    /// Not possible for this target (Windows, containers, WSL).
    Unsupported,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardError {
    pub kind: FailKind,
    pub message: String,
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ForwardError {}

impl From<ForwardError> for crate::HostError {
    fn from(e: ForwardError) -> Self {
        crate::HostError::Op(proto::Error::Other(e.message))
    }
}

/// Longest error text kept from ssh (it is shown in the UI).
const MAX_MESSAGE: usize = 300;
/// OS-assigned ports tried after the preferred one is taken.
const RETRIES: usize = 4;

/// `-L` argument: `bind:local:target:remote`.
pub fn spec(bind: LocalBind, local: u16, target: &RemoteTarget, remote: u16) -> String {
    format!("{}:{local}:{target}:{remote}", bind.address())
}

/// Sorts ssh's error output into a [`FailKind`]; the message is cut to one readable line without control characters.
pub fn classify(stderr: &str, status: Option<i32>, local: u16) -> ForwardError {
    let text = stderr.trim();
    let lower = text.to_ascii_lowercase();
    let kind = if lower.contains("address already in use") || lower.contains("cannot listen") {
        FailKind::AddrInUse
    } else if lower.contains("permission denied") && local < 1024 {
        FailKind::Privileged
    } else if lower.contains("control socket")
        || lower.contains("no such file or directory")
        || lower.contains("connection refused")
        || lower.contains("no controlpersist")
    {
        FailKind::MasterGone
    } else if lower.contains("forwarding request failed") || lower.contains("administratively prohibited") {
        FailKind::Rejected
    } else {
        FailKind::Other
    };
    let mut message: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if message.trim().is_empty() {
        message = match status {
            Some(code) => format!("ssh exited with code {code}"),
            None => "ssh was stopped by a signal".into(),
        };
    }
    if let Some((cut, _)) = message.char_indices().nth(MAX_MESSAGE) {
        message.truncate(cut);
        message.push('…');
    }
    ForwardError { kind, message: message.trim().to_string() }
}

fn unsupported(message: &str) -> ForwardError {
    ForwardError { kind: FailKind::Unsupported, message: message.into() }
}

/// Checks shared by every forward: ControlMaster exists here, the target is an SSH host, the name is safe.
fn guard(alias: &str, reverse: bool) -> Result<(), ForwardError> {
    if cfg!(windows) && reverse {
        return Err(unsupported("In-app reverse SSH forwarding requires ControlMaster, which Windows OpenSSH does not support. Use ssh -R in a terminal."));
    }
    if !matches!(target_of(alias), Target::Ssh(_)) {
        return Err(unsupported("Port forwarding needs an SSH host: containers and WSL share the local network"));
    }
    check_target(alias).map_err(|e| ForwardError { kind: FailKind::Other, message: e.to_string() })
}

/// `ssh -O forward|cancel -L|-R spec -- alias` on the shared connection.
fn mux(alias: &str, flag: &str, spec: &str, cancel: bool, local: u16) -> Result<(), ForwardError> {
    guard(alias, flag == "-R")?;
    let mut command = ssh_base(alias);
    command.args(["-o", "BatchMode=yes", "-O", if cancel { "cancel" } else { "forward" }, flag, spec, "--", alias]).stdin(Stdio::null());
    let out = output_within(&mut command, MUX_TIMEOUT).map_err(|e| ForwardError { kind: FailKind::Other, message: format!("could not run ssh: {e}") })?;
    if out.status.success() { Ok(()) } else { Err(classify(&String::from_utf8_lossy(&out.stderr), out.status.code(), local)) }
}

/// Longest a request to the shared connection may take. A connection that has stopped answering (its process is blocked) would
/// otherwise hold the caller, and the forward it was asked to close, for ever.
const MUX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// `command.output()`, but the process is killed and an error returned when it has not finished after `limit`.
fn output_within(command: &mut std::process::Command, limit: std::time::Duration) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    // The output is read on threads: a child that fills a pipe nobody reads would never finish.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let (stdout, stderr) = (drain(child.stdout.take().map(|p| Box::new(p) as _)), drain(child.stderr.take().map(|p| Box::new(p) as _)));
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, format!("ssh did not answer within {} s", limit.as_secs())));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let join = |t: std::thread::JoinHandle<Vec<u8>>| t.join().unwrap_or_default();
    Ok(std::process::Output { status, stdout: join(stdout), stderr: join(stderr) })
}

/// Opens `-L spec` (or cancels it).
pub(crate) fn local_spec(alias: &str, spec: &str, cancel: bool) -> Result<(), ForwardError> {
    #[cfg(windows)]
    {
        guard(alias, false)?;
        if cancel { windows::cancel(alias, spec) } else { windows::open(alias, spec, local_of(spec)) }
    }
    #[cfg(not(windows))]
    mux(alias, "-L", spec, cancel, local_of(spec))
}

/// The local port of a `-L` spec (`bind:local:target:remote`); 0 when it does not parse. Only used to name the cause of
/// an ssh failure.
fn local_of(spec: &str) -> u16 {
    spec.split(':').nth(1).and_then(|p| p.parse().ok()).unwrap_or(0)
}

/// Opens `-R spec` (or cancels it).
pub(crate) fn remote_spec(alias: &str, spec: &str, cancel: bool) -> Result<(), ForwardError> {
    mux(alias, "-R", spec, cancel, 0)
}

/// Can `port` be bound on the addresses `bind` uses? Only a hint: ssh's own bind decides.
pub(crate) fn bindable(bind: LocalBind, port: u16) -> bool {
    match bind {
        LocalBind::Loopback => {
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok() && (TcpListener::bind((Ipv6Addr::LOCALHOST, port)).is_ok() || !ipv6_available())
        }
        LocalBind::All => TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok(),
    }
}

fn ipv6_available() -> bool {
    TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).is_ok()
}

/// A port the OS currently considers free on loopback.
pub(crate) fn os_port() -> Option<u16> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok().and_then(|l| l.local_addr().ok()).map(|a| a.port())
}

/// Local ports to try, in order: the preferred one (when it can be bound without privileges), then OS-assigned ones.
fn candidates(req: &OpenRequest) -> Vec<u16> {
    let wanted = req.wanted_local.unwrap_or(req.remote);
    if req.strict {
        return vec![wanted];
    }
    let mut out = Vec::with_capacity(RETRIES + 1);
    if wanted >= 1024 && bindable(req.bind, wanted) {
        out.push(wanted);
    }
    out.extend(std::iter::from_fn(os_port).take(RETRIES));
    out.dedup();
    out
}

/// Opens a forward. A busy local port is retried with another one unless the request is `strict`; the result carries the
/// port in use and the spec that cancels it.
pub fn open(req: &OpenRequest) -> Result<Opened, ForwardError> {
    guard(&req.alias, false)?;
    let mut last = ForwardError { kind: FailKind::AddrInUse, message: "no free local port".into() };
    for local in candidates(req) {
        // ssh reports success when any one address of "localhost" binds, so a port another program holds on IPv4 would
        // look forwarded while connections reach that program: check before asking ssh.
        if !bindable(req.bind, local) {
            last = ForwardError { kind: FailKind::AddrInUse, message: format!("local port {local} is in use") };
            if req.strict {
                return Err(last);
            }
            continue;
        }
        let spec = spec(req.bind, local, &req.target, req.remote);
        #[cfg(windows)]
        let result = windows::open(&req.alias, &spec, local);
        #[cfg(not(windows))]
        let result = mux(&req.alias, "-L", &spec, false, local);
        match result {
            Ok(()) => return Ok(Opened { local, spec }),
            Err(mut e) => {
                // Through the shared connection ssh only reports "forwarding request failed"; a port nobody else can
                // bind now is the usual reason.
                if matches!(e.kind, FailKind::Rejected | FailKind::Other) && !bindable(req.bind, local) {
                    e.kind = FailKind::AddrInUse;
                }
                // "Refused" from a connection that is not there any more is a dead connection, not a refusal: the caller
                // reconnects instead of asking the user to try again.
                if e.kind == FailKind::Rejected && !check_master(&req.alias) {
                    e.kind = FailKind::MasterGone;
                }
                if e.kind != FailKind::AddrInUse || req.strict {
                    return Err(e);
                }
                last = e;
            }
        }
    }
    Err(last)
}

/// Tries of `ssh -O cancel` for one forward, and the wait before the second (it doubles for the third).
const CANCEL_TRIES: u32 = 3;
const CANCEL_WAIT: std::time::Duration = std::time::Duration::from_millis(150);

/// Closes a forward opened by [`open`], with the spec it returned. A cancel that fails for a reason that can pass (the shared
/// connection is busy, ssh could not be started for a moment) is tried again: one that is left undone keeps the local port
/// listening with nothing behind it. A connection that is gone took its forwards with it, so that is not tried again.
pub fn cancel(alias: &str, spec: &str) -> Result<(), ForwardError> {
    cancel_with(CANCEL_WAIT, || local_spec(alias, spec, true))
}

fn cancel_with(wait: std::time::Duration, mut run: impl FnMut() -> Result<(), ForwardError>) -> Result<(), ForwardError> {
    let mut result = run();
    for attempt in 1..CANCEL_TRIES {
        match &result {
            Ok(()) => break,
            Err(e) if matches!(e.kind, FailKind::MasterGone | FailKind::Unsupported) => break,
            Err(_) => {
                std::thread::sleep(wait * 2u32.pow(attempt - 1));
                result = run();
            }
        }
    }
    result
}

/// Is the shared connection to `alias` still up (`ssh -O check`)?
pub fn check_master(alias: &str) -> bool {
    guard(alias, false).is_ok()
        && ssh_base(alias)
            .args(["-o", "BatchMode=yes", "-O", "check", "--", alias])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_command_that_does_not_finish_is_killed_and_one_that_does_keeps_its_output() {
        let mut slow = std::process::Command::new("sleep");
        slow.arg("30");
        let started = std::time::Instant::now();
        let err = output_within(&mut slow, std::time::Duration::from_millis(200)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let mut echo = std::process::Command::new("sh");
        echo.args(["-c", "printf out; printf err >&2; exit 3"]);
        let out = output_within(&mut echo, std::time::Duration::from_secs(10)).unwrap();
        assert_eq!((out.status.code(), out.stdout.as_slice(), out.stderr.as_slice()), (Some(3), b"out".as_slice(), b"err".as_slice()));
        // More output than a pipe holds: the child is not left waiting for a reader.
        let mut flood = std::process::Command::new("sh");
        flood.args(["-c", "head -c 300000 /dev/zero"]);
        assert_eq!(output_within(&mut flood, std::time::Duration::from_secs(10)).unwrap().stdout.len(), 300_000);
    }

    #[test]
    fn a_cancel_that_fails_for_a_reason_that_can_pass_is_tried_again() {
        let err = |kind| ForwardError { kind, message: "x".into() };
        let none = std::time::Duration::ZERO;
        let mut calls = 0;
        let result = cancel_with(none, || {
            calls += 1;
            if calls < 3 { Err(err(FailKind::Other)) } else { Ok(()) }
        });
        assert_eq!((result, calls), (Ok(()), 3), "the third try works");
        let mut calls = 0;
        let result = cancel_with(none, || {
            calls += 1;
            Err(err(FailKind::Rejected))
        });
        assert_eq!((result.map_err(|e| e.kind), calls), (Err(FailKind::Rejected), 3), "three tries, then the last error is reported");
        for kind in [FailKind::MasterGone, FailKind::Unsupported] {
            let mut calls = 0;
            let _ = cancel_with(none, || {
                calls += 1;
                Err(err(kind))
            });
            assert_eq!(calls, 1, "{kind:?}: nothing to cancel, or nothing that trying again would change");
        }
    }

    #[test]
    fn specs_are_built_from_typed_values() {
        assert_eq!(spec(LocalBind::Loopback, 3000, &RemoteTarget::Localhost, 8080), "localhost:3000:localhost:8080");
        assert_eq!(spec(LocalBind::All, 3000, &RemoteTarget::V4(Ipv4Addr::LOCALHOST), 80), "0.0.0.0:3000:127.0.0.1:80");
        assert_eq!(spec(LocalBind::Loopback, 5173, &RemoteTarget::V6(Ipv6Addr::LOCALHOST), 5173), "localhost:5173:[::1]:5173");
    }

    #[test]
    fn ssh_errors_are_sorted_into_kinds() {
        let k = |s: &str, local: u16| classify(s, Some(255), local).kind;
        assert_eq!(
            k("bind [127.0.0.1]:3000: Address already in use\nchannel_setup_fwd_listener_tcpip: cannot listen to port: 3000", 3000),
            FailKind::AddrInUse
        );
        assert_eq!(k("bind [127.0.0.1]:80: Permission denied", 80), FailKind::Privileged);
        assert_eq!(k("Control socket connect(/tmp/cm): No such file or directory", 3000), FailKind::MasterGone);
        assert_eq!(k("mux_client_forward: forwarding request failed: Port forwarding failed", 3000), FailKind::Rejected);
        assert_eq!(k("channel 2: open failed: administratively prohibited: open failed", 3000), FailKind::Rejected);
        assert_eq!(k("something new", 3000), FailKind::Other);
    }

    #[test]
    fn the_local_port_is_read_from_a_spec() {
        assert_eq!(local_of("localhost:3000:localhost:8080"), 3000);
        assert_eq!(local_of("0.0.0.0:80:[::1]:80"), 80);
        assert_eq!(local_of("garbage"), 0);
    }

    #[test]
    fn error_text_is_one_short_clean_line() {
        assert_eq!(classify("", Some(255), 1).message, "ssh exited with code 255");
        assert_eq!(classify("  \n", None, 1).message, "ssh was stopped by a signal");
        let hostile = format!("bad\x1b[2J\x07line\n{}", "x".repeat(5000));
        let m = classify(&hostile, Some(1), 1).message;
        assert!(!m.chars().any(char::is_control), "{m:?}");
        assert!(m.chars().count() <= MAX_MESSAGE + 1 && m.ends_with('…'));
    }

    #[test]
    fn a_strict_request_tries_only_its_own_port_and_others_fall_back() {
        let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = busy.local_addr().unwrap().port();
        let req = |strict| OpenRequest {
            alias: "h".into(),
            remote: port,
            target: RemoteTarget::Localhost,
            wanted_local: Some(port),
            strict,
            bind: LocalBind::Loopback,
        };
        assert_eq!(candidates(&req(true)), vec![port]);
        let loose = candidates(&req(false));
        assert!(!loose.contains(&port), "a busy preferred port is skipped: {loose:?}");
        assert!(!loose.is_empty() && loose.len() <= RETRIES);
        let privileged = OpenRequest { wanted_local: Some(80), ..req(false) };
        assert!(!candidates(&privileged).contains(&80), "privileged ports are never tried implicitly");
    }

    #[test]
    fn containers_and_wsl_cannot_forward() {
        let req = OpenRequest {
            alias: "docker:box".into(),
            remote: 8080,
            target: RemoteTarget::Localhost,
            wanted_local: None,
            strict: false,
            bind: LocalBind::Loopback,
        };
        assert_eq!(open(&req).unwrap_err().kind, FailKind::Unsupported);
        assert_eq!(cancel("wsl:Ubuntu", "127.0.0.1:1:localhost:1").unwrap_err().kind, FailKind::Unsupported);
    }
}
