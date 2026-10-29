//! Debug Adapter Protocol client (VS Code's debugger protocol). The process starts through `host`
//! (locally, or on the server over SSH); framing is the same as LSP
//! (`Content-Length`). This file knows only the protocol; the UI lives in `workbench_debug.rs`.
//!
//! Flow: `initialize` → `launch` (its response arrives after `configurationDone`, so don't wait for it) →
//! `initialized` event → breakpoints → `configurationDone` → `launch` response.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use host::{Host, ProcEvent};
use serde_json::{Value, json};

use crate::lsp::{Decoder, encode};

#[derive(Debug, Clone, PartialEq)]
pub enum DapEvent {
    /// The adapter is ready for configuration (breakpoints are sent now).
    Initialized,
    Stopped {
        thread_id: i64,
        reason: String,
        text: Option<String>,
    },
    Continued,
    /// Output of the debuggee or the adapter: (category, text).
    Output {
        category: String,
        text: String,
    },
    /// The session ended (the program exited or the adapter closed).
    Terminated,
    /// The program ended with an exit code.
    Exited(i32),
    /// The adapter process died.
    AdapterExited(Option<i32>),
    /// Answer to `setBreakpoints` for `path`: the 1-based lines in the order sent and the result for each.
    BreakpointsSet {
        path: String,
        sent: Vec<u32>,
        results: Vec<BpResult>,
    },
    /// Answer to `setFunctionBreakpoints` (same order as the enabled function breakpoints sent).
    FunctionBreakpointsSet(Vec<BpResult>),
    /// The adapter changed a breakpoint by itself (verified later, moved, removed).
    BreakpointChanged(BpResult),
    /// A thread started or exited.
    Thread {
        thread_id: i64,
        started: bool,
    },
    /// The adapter announced new capabilities (merged into `Session::capabilities`).
    Capabilities,
    /// The adapter asks to run the program in a terminal (`runInTerminal`; already answered).
    RunInTerminal(TerminalRequest),
}

/// What `runInTerminal` asks for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TerminalRequest {
    pub title: Option<String>,
    pub cwd: String,
    pub args: Vec<String>,
    /// `None` value: remove the variable.
    pub env: Vec<(String, Option<String>)>,
}

impl TerminalRequest {
    fn parse(a: &Value) -> Option<Self> {
        let args: Vec<String> = a["args"].as_array()?.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
        if args.is_empty() {
            return None;
        }
        let env = a["env"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string))).collect()).unwrap_or_default();
        Some(TerminalRequest { title: a["title"].as_str().map(str::to_string), cwd: a["cwd"].as_str().unwrap_or_default().to_string(), args, env })
    }
}

pub use crate::debug_model::BpResult;

/// A breakpoint on a function name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FunctionBreakpoint {
    pub name: String,
    pub condition: Option<String>,
}

impl FunctionBreakpoint {
    fn to_json(&self) -> Value {
        let mut b = json!({ "name": self.name });
        if let Some(c) = self.condition.as_ref().filter(|c| !c.trim().is_empty()) {
            b["condition"] = json!(c);
        }
        b
    }
}

/// Results of a `setBreakpoints` / `setFunctionBreakpoints` answer.
pub fn parse_bp_results(body: &Value) -> Vec<BpResult> {
    body["breakpoints"].as_array().map(|a| a.iter().map(parse_bp_result).collect()).unwrap_or_default()
}

fn parse_bp_result(b: &Value) -> BpResult {
    BpResult {
        id: b["id"].as_i64(),
        verified: b["verified"].as_bool().unwrap_or(false),
        message: b["message"].as_str().map(clip_message),
        line: b["line"].as_u64().map(|l| l as u32),
    }
}

/// A breakpoint: line (1-based) and an optional condition / log message / hit count.
/// A breakpoint with a log message (logpoint) doesn't stop; it writes the message to the console (`{expression}` is expanded).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Breakpoint {
    pub line: u32,
    pub condition: Option<String>,
    pub log_message: Option<String>,
    pub hit_condition: Option<String>,
}

impl Breakpoint {
    /// A plain breakpoint on a line (the editor builds them from its own marks, so only tests need this).
    #[cfg(test)]
    pub fn at(line: u32) -> Self {
        Breakpoint { line, ..Default::default() }
    }

    fn to_json(&self) -> Value {
        let mut b = json!({ "line": self.line });
        for (key, v) in [("condition", &self.condition), ("logMessage", &self.log_message), ("hitCondition", &self.hit_condition)] {
            if let Some(v) = v.as_ref().filter(|v| !v.trim().is_empty()) {
                b[key] = json!(v);
            }
        }
        b
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<String>,
    /// 1-based.
    pub line: u32,
    pub column: u32,
    /// The adapter can restart the program from this frame.
    pub can_restart: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    pub name: String,
    pub reference: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub ty: Option<String>,
    /// > 0: has children (requested with `variables`).
    pub reference: i64,
    /// Expression that evaluates to this value (for watches and copying).
    pub evaluate_name: Option<String>,
    /// Number of indexed children (array elements), when the adapter says: those are loaded a page at a time.
    pub indexed: usize,
}

type Pending = Arc<Mutex<HashMap<i64, mpsc::Sender<Result<Value, String>>>>>;

/// Where messages go: the adapter process's stdin or a TCP connection.
enum Writer {
    Proc { host: Arc<dyn Host>, id: u32 },
    Tcp(Mutex<TcpStream>),
}

impl Writer {
    fn send(&self, msg: &Value) {
        if std::env::var_os("CODEYARD_DAP_TRACE").is_some() {
            eprintln!("dap-> {}", msg.to_string().chars().take(400).collect::<String>());
        }
        let bytes = encode(msg);
        match self {
            Writer::Proc { host, id } => host.proc_input(*id, bytes),
            Writer::Tcp(s) => {
                let _ = s.lock().unwrap().write_all(&bytes);
            }
        }
    }
}

/// How the adapter talks: stdin/stdout (lldb-dap, debugpy), or TCP: if the arguments contain
/// `${port}` the adapter listens on that port and we connect (js-debug); with `${clientAddr}` we
/// listen and the adapter connects to us (delve: `dlv dap --client-addr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    Connect,
    Accept,
}

pub fn transport_of(args: &[String]) -> Transport {
    if args.iter().any(|a| a.contains("${port}")) {
        Transport::Connect
    } else if args.iter().any(|a| a.contains("${clientAddr}")) {
        Transport::Accept
    } else {
        Transport::Stdio
    }
}

fn substitute(args: &[String], port: u16) -> Vec<String> {
    args.iter().map(|a| a.replace("${port}", &port.to_string()).replace("${clientAddr}", &format!("127.0.0.1:{port}"))).collect()
}

/// Longest error text (characters) taken from an adapter: it ends up in a toast or the console.
const MAX_MESSAGE_CHARS: usize = 2000;

fn clip_message(text: &str) -> String {
    crate::lsp::clip_text(text, MAX_MESSAGE_CHARS)
}

pub struct Session {
    writer: Arc<Writer>,
    /// Adapter process (to shut it down); absent on attached child sessions.
    proc: Option<(Arc<dyn Host>, u32)>,
    next_seq: AtomicI64,
    pending: Pending,
    alive: Arc<AtomicBool>,
    initialized: Arc<(Mutex<bool>, Condvar)>,
    pub capabilities: Mutex<Value>,
    /// Address of the TCP adapter: `startDebugging` child sessions connect here.
    addr: Option<SocketAddr>,
    events: mpsc::Sender<DapEvent>,
    /// Adapters like js-debug run the real program in a child session; all later
    /// requests (step, stack, breakpoints) go to the child.
    child: Mutex<Option<Arc<Session>>>,
    adapter_id: Mutex<String>,
    breakpoints: Mutex<BTreeMap<String, Vec<Breakpoint>>>,
    functions: Mutex<Vec<FunctionBreakpoint>>,
    /// Project root: lldb compares source paths without resolving symlinks (the build uses `/tmp/x`,
    /// the editor knows `/private/tmp/x`); the root-relative path is sent too and matches by that suffix.
    root: Mutex<Option<String>>,
    /// Exception filters picked by the user (`None`: never chose, use the adapter's defaults).
    exception_pref: Mutex<Option<Vec<String>>>,
    /// For `attach` the program isn't killed on close (we just detach).
    attached: AtomicBool,
    /// The client offered `runInTerminal` (the configuration asked for a terminal); a request is refused otherwise.
    terminal: AtomicBool,
    /// In a direct connection where we didn't start the process, a dropped connection also ends the session.
    direct: bool,
    me: Weak<Session>,
    /// SSH port forwarding (removed on close): (alias, kind, server port, local port).
    forward: Mutex<Option<Forward>>,
}

type Forward = (String, Transport, u16, u16);

fn release_forward(f: &Option<Forward>) {
    if let Some((alias, transport, remote, local)) = f {
        let _ = match transport {
            Transport::Connect => host::ssh::forward_port(alias, *local, *remote, true),
            _ => host::ssh::forward_port_reverse(alias, *remote, *local, true),
        };
    }
}

/// A random port on the server for the adapter (40000–59999; if taken, the adapter reports a clear error).
fn remote_port() -> u16 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0) as u64;
    40000 + ((n ^ (std::process::id() as u64).wrapping_mul(7919)) % 20000) as u16
}

/// A free local port (listen, then release immediately; the adapter takes it later).
fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

impl Session {
    fn build(writer: Writer, proc: Option<(Arc<dyn Host>, u32)>, addr: Option<SocketAddr>, events: mpsc::Sender<DapEvent>) -> Arc<Session> {
        Arc::new_cyclic(|me| Session {
            writer: Arc::new(writer),
            proc,
            next_seq: AtomicI64::new(1),
            pending: Arc::default(),
            alive: Arc::new(AtomicBool::new(true)),
            initialized: Arc::new((Mutex::new(false), Condvar::new())),
            capabilities: Mutex::new(Value::Null),
            addr,
            events,
            child: Mutex::new(None),
            adapter_id: Mutex::new(String::new()),
            breakpoints: Mutex::new(BTreeMap::new()),
            functions: Mutex::new(Vec::new()),
            root: Mutex::new(None),
            exception_pref: Mutex::new(None),
            attached: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
            direct: false,
            me: me.clone(),
            forward: Mutex::new(None),
        })
    }

    /// Start the adapter. Error if the command isn't found; the handshake is done by `begin`.
    pub fn start(host: Arc<dyn Host>, command: &str, args: Vec<String>, cwd: Option<String>, events: mpsc::Sender<DapEvent>) -> Result<Arc<Session>, String> {
        let transport = transport_of(&args);
        // TCP: pick the port now, start the process, then connect / accept the connection. Over SSH the adapter
        // runs on the server: to connect, the local port is forwarded to the server's (`ssh -L`), and the reverse
        // for adapters that connect to us (`ssh -R`); both go through the open ControlMaster.
        let remote = host.is_remote();
        let alias = host.label().trim_start_matches("ssh://").to_string();
        let (adapter_port, port, listener) = match transport {
            Transport::Stdio => (0, 0, None),
            Transport::Connect => {
                let local = free_port()?;
                (if remote { remote_port() } else { local }, local, None)
            }
            Transport::Accept => {
                let l = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
                l.set_nonblocking(true).map_err(|e| e.to_string())?;
                let local = l.local_addr().map_err(|e| e.to_string())?.port();
                (if remote { remote_port() } else { local }, local, Some(l))
            }
        };
        let forward = (remote && transport != Transport::Stdio).then(|| (alias.clone(), transport, adapter_port, port));
        if let Some((alias, transport, remote_port, local)) = &forward {
            let done = match transport {
                Transport::Connect => host::ssh::forward_port(alias, *local, *remote_port, false),
                _ => host::ssh::forward_port_reverse(alias, *remote_port, *local, false),
            };
            done.map_err(|e| crate::i18n::tf("Could not forward the debug port over SSH: {}", &[&e]))?;
        }
        let args = if transport == Transport::Stdio { args } else { substitute(&args, adapter_port) };
        let proc = host.proc_spawn(command, args, cwd).map_err(|e| e.to_string())?;
        let id = proc.id;
        let slot: Arc<Mutex<Weak<Session>>> = Arc::default();
        let exited = Arc::new(AtomicBool::new(false));
        let tail: Arc<Mutex<String>> = Arc::default();
        let session_for_stdio = Arc::new(Mutex::new(Decoder::default()));
        {
            let (slot, exited, tail, dec) = (slot.clone(), exited.clone(), tail.clone(), session_for_stdio.clone());
            std::thread::Builder::new()
                .name("dap-proc".into())
                .spawn(move || {
                    while let Ok(ev) = proc.events.recv() {
                        match ev {
                            ProcEvent::Output(data) => {
                                if transport == Transport::Stdio {
                                    let msgs = dec.lock().unwrap().push(&data);
                                    if let Some(s) = slot.lock().unwrap().upgrade() {
                                        for m in msgs {
                                            s.route(m);
                                        }
                                    }
                                } else {
                                    // Output of the TCP adapter itself (build error, crash): keep the tail for the error message.
                                    let mut t = tail.lock().unwrap();
                                    t.push_str(&String::from_utf8_lossy(&data));
                                    let extra = t.len().saturating_sub(2000);
                                    if extra > 0 {
                                        let cut = (extra..t.len()).find(|&i| t.is_char_boundary(i)).unwrap_or(t.len());
                                        t.drain(..cut);
                                    }
                                }
                            }
                            ProcEvent::Exit(code) => {
                                exited.store(true, Ordering::Relaxed);
                                if let Some(s) = slot.lock().unwrap().upgrade() {
                                    s.on_exit(code);
                                }
                                break;
                            }
                        }
                    }
                })
                .map_err(|e| {
                    host.proc_kill(id);
                    e.to_string()
                })?;
        }
        let session = match transport {
            Transport::Stdio => Session::build(Writer::Proc { host: host.clone(), id }, Some((host.clone(), id)), None, events),
            Transport::Connect | Transport::Accept => {
                let started = std::time::Instant::now();
                let fail = |why: &str| {
                    host.proc_kill(id);
                    release_forward(&forward);
                    let out = tail.lock().unwrap().trim().to_string();
                    Err(if out.is_empty() { why.to_string() } else { format!("{why}: {out}") })
                };
                let stream = loop {
                    let attempt = match &listener {
                        Some(l) => l.accept().map(|(s, _)| s),
                        None => TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(250)),
                    };
                    match attempt {
                        // SSH `-L` accepts the local port before the remote service is ready, then closes right away:
                        // if the connection didn't stay open for a short while the service isn't listening yet, retry.
                        Ok(s) if forward.is_some() && transport == Transport::Connect => {
                            let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                            let closed = matches!(s.peek(&mut [0u8; 1]), Ok(0))
                                || matches!(s.peek(&mut [0u8; 1]), Err(e) if !matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut));
                            if !closed {
                                let _ = s.set_read_timeout(None);
                                break s;
                            }
                            if exited.load(Ordering::Relaxed) {
                                return fail(crate::i18n::t("The debug adapter exited before it started listening"));
                            }
                            if started.elapsed() > Duration::from_secs(30) {
                                return fail(crate::i18n::t("The debug adapter did not start listening"));
                            }
                            std::thread::sleep(Duration::from_millis(150));
                        }
                        Ok(s) => break s,
                        Err(_) if exited.load(Ordering::Relaxed) => return fail(crate::i18n::t("The debug adapter exited before connecting")),
                        Err(_) if started.elapsed() > Duration::from_secs(30) => return fail(crate::i18n::t("The debug adapter did not accept a connection")),
                        Err(_) => std::thread::sleep(Duration::from_millis(100)),
                    }
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_nodelay(true);
                let reader = match stream.try_clone() {
                    Ok(r) => r,
                    Err(e) => return fail(&e.to_string()),
                };
                let s = Session::build(Writer::Tcp(Mutex::new(stream)), Some((host.clone(), id)), Some(SocketAddr::from(([127, 0, 0, 1], port))), events);
                *s.forward.lock().unwrap() = forward;
                *slot.lock().unwrap() = Arc::downgrade(&s);
                s.spawn_tcp_reader(reader);
                return Ok(s);
            }
        };
        *slot.lock().unwrap() = Arc::downgrade(&session);
        Ok(session)
    }

    /// Connect directly to an already running DAP server (starts no process): `debugpy --listen`,
    /// `dlv --headless`… Over SSH the server port is forwarded to a local port.
    pub fn attach_socket(host: Arc<dyn Host>, remote_host: &str, port: u16, events: mpsc::Sender<DapEvent>) -> Result<Arc<Session>, String> {
        let remote = host.is_remote();
        let alias = host.label().trim_start_matches("ssh://").to_string();
        let local = if remote { free_port()? } else { port };
        let target: SocketAddr = if remote {
            host::ssh::forward_port(&alias, local, port, false).map_err(|e| crate::i18n::tf("Could not forward the debug port over SSH: {}", &[&e]))?;
            SocketAddr::from(([127, 0, 0, 1], local))
        } else {
            use std::net::ToSocketAddrs;
            (remote_host, port)
                .to_socket_addrs()
                .map_err(|e| format!("{remote_host}:{port}: {e}"))?
                .next()
                .ok_or_else(|| crate::i18n::tf("{}:{}: no address", &[&remote_host, &port]))?
        };
        let started = std::time::Instant::now();
        let stream = loop {
            if let Ok(s) = TcpStream::connect_timeout(&target, Duration::from_millis(500)) {
                if !remote {
                    break s;
                }
                // SSH `-L` accepts a remote port that isn't ready and then closes: the connection must stay open.
                let _ = s.set_read_timeout(Some(Duration::from_millis(400)));
                let closed = matches!(s.peek(&mut [0u8; 1]), Ok(0))
                    || matches!(s.peek(&mut [0u8; 1]), Err(e) if !matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut));
                let _ = s.set_read_timeout(None);
                if !closed {
                    break s;
                }
            }
            if started.elapsed() > Duration::from_secs(10) {
                if remote {
                    let _ = host::ssh::forward_port(&alias, local, port, true);
                }
                return Err(crate::i18n::tf(
                    "Nothing is listening on {0}:{1} (start the program with its debug server first, e.g. `python -m debugpy --listen {1} --wait-for-client app.py`).",
                    &[&remote_host, &port],
                ));
            }
            std::thread::sleep(Duration::from_millis(150));
        };
        let _ = stream.set_nodelay(true);
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        let s = Arc::new_cyclic(|me| Session {
            writer: Arc::new(Writer::Tcp(Mutex::new(stream))),
            proc: None,
            next_seq: AtomicI64::new(1),
            pending: Arc::default(),
            alive: Arc::new(AtomicBool::new(true)),
            initialized: Arc::new((Mutex::new(false), Condvar::new())),
            capabilities: Mutex::new(Value::Null),
            addr: Some(target),
            events,
            child: Mutex::new(None),
            adapter_id: Mutex::new(String::new()),
            breakpoints: Mutex::new(BTreeMap::new()),
            functions: Mutex::new(Vec::new()),
            root: Mutex::new(None),
            exception_pref: Mutex::new(None),
            attached: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
            direct: true,
            forward: Mutex::new(None),
            me: me.clone(),
        });
        *s.forward.lock().unwrap() = remote.then_some((alias, Transport::Connect, port, local));
        s.spawn_tcp_reader(reader);
        Ok(s)
    }

    /// Child session: a second connection to the same TCP adapter (starts no process).
    fn connect_child(&self) -> Result<Arc<Session>, String> {
        let addr = self.addr.ok_or("this adapter cannot open child sessions")?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|e| e.to_string())?;
        let _ = stream.set_nodelay(true);
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        let child = Session::build(Writer::Tcp(Mutex::new(stream)), None, Some(addr), self.events.clone());
        child.spawn_tcp_reader(reader);
        Ok(child)
    }

    fn spawn_tcp_reader(&self, mut stream: TcpStream) {
        let me = self.me.clone();
        let _ = std::thread::Builder::new().name("dap-tcp".into()).spawn(move || {
            let (mut dec, mut buf) = (Decoder::default(), [0u8; 16 * 1024]);
            loop {
                match stream.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        let Some(s) = me.upgrade() else { break };
                        for m in dec.push(&buf[..n]) {
                            s.route(m);
                        }
                    }
                    _ => {
                        if let Some(s) = me.upgrade() {
                            s.on_exit(None);
                        }
                        break;
                    }
                }
            }
        });
    }

    fn on_exit(&self, code: Option<i32>) {
        if !self.alive.swap(false, Ordering::Relaxed) {
            return;
        }
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(crate::i18n::t("the debug adapter stopped").into()));
        }
        // Wake any pending `wait_initialized`.
        *self.initialized.0.lock().unwrap() = true;
        self.initialized.1.notify_all();
        // A child session's connection closing isn't the end of the whole session; the parent reports that.
        if self.proc.is_some() || self.direct {
            let _ = self.events.send(DapEvent::AdapterExited(code));
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    fn child(&self) -> Option<Arc<Session>> {
        self.child.lock().unwrap().clone().filter(|c| c.is_alive())
    }

    /// Send on this connection without routing to the child session (for the handshake).
    fn raw_async(&self, command: &str, arguments: Value) -> mpsc::Receiver<Result<Value, String>> {
        let (tx, rx) = mpsc::channel();
        if !self.is_alive() {
            let _ = tx.send(Err(crate::i18n::t("the debug adapter is not running").into()));
            return rx;
        }
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().unwrap().insert(seq, tx);
        self.writer.send(&json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments }));
        rx
    }

    /// Send the request and return the response channel immediately (for late responses like `launch`).
    /// Goes to the child session if one was opened.
    pub fn request_async(&self, command: &str, arguments: Value) -> mpsc::Receiver<Result<Value, String>> {
        match self.child() {
            Some(c) => c.raw_async(command, arguments),
            None => self.raw_async(command, arguments),
        }
    }

    /// Send a request and wait for the response (called on a background thread).
    pub fn request(&self, command: &str, arguments: Value, timeout: Duration) -> Result<Value, String> {
        self.request_async(command, arguments).recv_timeout(timeout).map_err(|_| crate::i18n::tf("{}: no answer from the debug adapter", &[&command]))?
    }

    fn raw_request(&self, command: &str, arguments: Value, timeout: Duration) -> Result<Value, String> {
        self.raw_async(command, arguments).recv_timeout(timeout).map_err(|_| crate::i18n::tf("{}: no answer from the debug adapter", &[&command]))?
    }

    fn wait_initialized(&self, timeout: Duration) -> Result<(), String> {
        let (lock, cv) = &*self.initialized;
        let guard = lock.lock().unwrap();
        let (guard, res) = cv.wait_timeout_while(guard, timeout, |ready| !*ready).unwrap();
        if res.timed_out() || !*guard {
            return Err(crate::i18n::t("the debug adapter did not report that it is ready").into());
        }
        if !self.is_alive() {
            return Err(crate::i18n::t("the debug adapter stopped while starting").into());
        }
        Ok(())
    }

    /// Handshake and start: `launch_args` go straight to the adapter (`program`, `args`, `cwd`…).
    /// Breakpoints (absolute path → 1-based lines) are sent before `launch`.
    pub fn begin(
        &self,
        adapter_id: &str,
        request: &str,
        launch_args: Value,
        breakpoints: &BTreeMap<String, Vec<Breakpoint>>,
        functions: &[FunctionBreakpoint],
    ) -> Result<(), String> {
        *self.adapter_id.lock().unwrap() = adapter_id.to_string();
        *self.breakpoints.lock().unwrap() = breakpoints.clone();
        *self.functions.lock().unwrap() = functions.to_vec();
        self.attached.store(request == "attach", Ordering::Relaxed);
        // Only a configuration that asks for a terminal gets one: some adapters change how they start the program
        // as soon as the client says it can.
        let terminal = matches!(launch_args["console"].as_str(), Some("integratedTerminal" | "externalTerminal"));
        self.terminal.store(terminal, Ordering::Relaxed);
        let init = self.raw_request(
            "initialize",
            json!({
                "clientID": "codeyard", "clientName": "Codeyard", "adapterID": adapter_id,
                "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path",
                "supportsVariableType": true, "supportsVariablePaging": true, "supportsRunInTerminalRequest": terminal,
                "supportsStartDebuggingRequest": true,
            }),
            Duration::from_secs(30),
        )?;
        *self.capabilities.lock().unwrap() = init;
        // The `launch` / `attach` response comes after `configurationDone`: carry on without waiting.
        let launched = self.raw_async(request, launch_args);
        self.wait_initialized(Duration::from_secs(60))?;
        for (path, list) in breakpoints {
            for (i, args) in self.breakpoint_requests(path, list).into_iter().enumerate() {
                let answer = self.raw_request("setBreakpoints", args, Duration::from_secs(15))?;
                // The first request carries the editor's path; a second one (lldb's relative path) is not reported.
                if i == 0 {
                    let sent = list.iter().map(|b| b.line).collect();
                    let _ = self.events.send(DapEvent::BreakpointsSet { path: path.clone(), sent, results: parse_bp_results(&answer) });
                }
            }
        }
        if !functions.is_empty() && self.supports("supportsFunctionBreakpoints") {
            let answer = self.raw_request("setFunctionBreakpoints", Self::function_args(functions), Duration::from_secs(15))?;
            let _ = self.events.send(DapEvent::FunctionBreakpointsSet(parse_bp_results(&answer)));
        }
        // Exception breakpoints: the preference (else the adapter's default), intersected with what the adapter offers.
        let available = self.exception_filters();
        if !available.is_empty() {
            let pref = self.exception_pref.lock().unwrap().clone();
            let chosen: Vec<String> = match pref {
                Some(p) => p.into_iter().filter(|f| available.iter().any(|(id, ..)| id == f)).collect(),
                None => available.iter().filter(|(.., d)| *d).map(|(id, ..)| id.clone()).collect(),
            };
            self.raw_request("setExceptionBreakpoints", json!({ "filters": chosen }), Duration::from_secs(15))?;
        }
        let supports_done = self.capabilities.lock().unwrap()["supportsConfigurationDoneRequest"].as_bool().unwrap_or(true);
        if supports_done {
            self.raw_request("configurationDone", json!({}), Duration::from_secs(30))?;
        }
        launched.recv_timeout(Duration::from_secs(120)).map_err(|_| crate::i18n::tf("{}: the program did not start", &[&request]))??;
        Ok(())
    }

    /// Exception filters the adapter offers: (id, name, enabled by default).
    pub fn exception_filters(&self) -> Vec<(String, String, bool)> {
        self.capabilities.lock().unwrap()["exceptionBreakpointFilters"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| {
                        Some((
                            f["filter"].as_str()?.to_string(),
                            f["label"].as_str().unwrap_or_else(|| f["filter"].as_str().unwrap_or("")).to_string(),
                            f["default"].as_bool().unwrap_or(false),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The user's preference before start (`begin` intersects it with what the adapter has and sends it).
    pub fn set_exception_pref(&self, pref: Option<Vec<String>>) {
        *self.exception_pref.lock().unwrap() = pref;
    }

    /// Change while running: no response awaited (reaches the adapter before the next `continue`).
    pub fn set_exception_filters(&self, filters: &[String]) {
        *self.exception_pref.lock().unwrap() = Some(filters.to_vec());
        let _ = self.request_async("setExceptionBreakpoints", json!({ "filters": filters }));
    }

    pub fn set_root(&self, root: &str) {
        *self.root.lock().unwrap() = Some(root.trim_end_matches('/').to_string());
    }

    /// `setBreakpoints` requests for a file: the absolute path, plus the root-relative path for lldb.
    fn breakpoint_requests(&self, path: &str, bps: &[Breakpoint]) -> Vec<Value> {
        let mut out = vec![Self::breakpoints_args(path, bps)];
        let lldb = self.adapter_id.lock().unwrap().as_str() == "lldb-dap";
        let rel = self.root.lock().unwrap().as_deref().and_then(|r| path.strip_prefix(&format!("{r}/")).map(str::to_string));
        if let (true, Some(rel)) = (lldb, rel) {
            out.push(Self::breakpoints_args(&rel, bps));
        }
        out
    }

    /// The adapter asked for `startDebugging` (js-debug runs the real program in a child session):
    /// open a new connection to the same adapter, start it with the same breakpoints; later requests go to it.
    fn adopt_child(&self, request: String, configuration: Value) {
        let child = match self.connect_child() {
            Ok(c) => c,
            Err(e) => {
                let _ = self.events.send(DapEvent::Output {
                    category: "stderr".into(),
                    text: format!("{}\n", crate::i18n::tf("Could not open the debug session: {}", &[&e])),
                });
                return;
            }
        };
        *self.child.lock().unwrap() = Some(child.clone());
        let (id, bps, functions) = (self.adapter_id.lock().unwrap().clone(), self.breakpoints.lock().unwrap().clone(), self.functions.lock().unwrap().clone());
        if let Err(e) = child.begin(&id, &request, configuration, &bps, &functions) {
            let _ = self
                .events
                .send(DapEvent::Output { category: "stderr".into(), text: format!("{}\n", crate::i18n::tf("Could not start the debug session: {}", &[&e])) });
            let _ = self.events.send(DapEvent::Terminated);
        }
    }

    /// Send without awaiting a response: the request reaches the adapter before commands that follow it,
    /// like `continue` (sending from a background thread doesn't guarantee the order).
    /// The answer to the first request is reported as `BreakpointsSet` from a short-lived thread.
    pub fn set_breakpoints_async(&self, path: &str, bps: &[Breakpoint]) {
        self.remember(path, bps);
        let mut answers = self.breakpoint_requests(path, bps).into_iter().map(|args| self.request_async("setBreakpoints", args)).collect::<Vec<_>>();
        if answers.is_empty() {
            return;
        }
        let first = answers.swap_remove(0);
        let (events, path, sent) = (self.events.clone(), path.to_string(), bps.iter().map(|b| b.line).collect::<Vec<_>>());
        let _ = std::thread::Builder::new().name("dap-bp".into()).spawn(move || {
            if let Ok(Ok(answer)) = first.recv_timeout(Duration::from_secs(15)) {
                let _ = events.send(DapEvent::BreakpointsSet { path, sent, results: parse_bp_results(&answer) });
            }
        });
    }

    /// Replace the function breakpoints while running (answer reported as `FunctionBreakpointsSet`).
    pub fn set_function_breakpoints_async(&self, functions: &[FunctionBreakpoint]) {
        *self.functions.lock().unwrap() = functions.to_vec();
        if !self.supports("supportsFunctionBreakpoints") {
            return;
        }
        let answer = self.request_async("setFunctionBreakpoints", Self::function_args(functions));
        let events = self.events.clone();
        let _ = std::thread::Builder::new().name("dap-fbp".into()).spawn(move || {
            if let Ok(Ok(a)) = answer.recv_timeout(Duration::from_secs(15)) {
                let _ = events.send(DapEvent::FunctionBreakpointsSet(parse_bp_results(&a)));
            }
        });
    }

    fn function_args(functions: &[FunctionBreakpoint]) -> Value {
        json!({ "breakpoints": functions.iter().map(FunctionBreakpoint::to_json).collect::<Vec<_>>() })
    }

    /// A capability flag of the adapter (of the child session when there is one).
    pub fn supports(&self, flag: &str) -> bool {
        match self.child() {
            Some(c) => c.supports(flag),
            None => self.capabilities.lock().unwrap()[flag].as_bool().unwrap_or(false),
        }
    }

    fn remember(&self, path: &str, bps: &[Breakpoint]) {
        let mut all = self.breakpoints.lock().unwrap();
        if bps.is_empty() {
            all.remove(path);
        } else {
            all.insert(path.to_string(), bps.to_vec());
        }
    }

    fn breakpoints_args(path: &str, bps: &[Breakpoint]) -> Value {
        // `lines` is for old adapters (deprecated but harmless).
        json!({ "source": { "path": path }, "breakpoints": bps.iter().map(Breakpoint::to_json).collect::<Vec<_>>(), "lines": bps.iter().map(|b| b.line).collect::<Vec<_>>() })
    }

    /// Threads of the program: (id, name).
    pub fn threads(&self) -> Vec<(i64, String)> {
        self.request("threads", json!({}), Duration::from_secs(10))
            .ok()
            .and_then(|r| {
                r["threads"].as_array().map(|a| a.iter().map(|t| (t["id"].as_i64().unwrap_or(0), t["name"].as_str().unwrap_or("thread").to_string())).collect())
            })
            .unwrap_or_default()
    }

    /// Data breakpoint info: (dataId, description). `reference`: the scope or object containing the variable.
    pub fn data_breakpoint_info(&self, reference: i64, name: &str) -> Result<(String, String), String> {
        let r = self.request("dataBreakpointInfo", json!({ "variablesReference": reference, "name": name }), Duration::from_secs(10))?;
        match r["dataId"].as_str() {
            Some(id) => Ok((id.to_string(), r["description"].as_str().unwrap_or(name).to_string())),
            None => Err(clip_message(r["description"].as_str().unwrap_or(crate::i18n::t("this value cannot be watched")))),
        }
    }

    /// Replace data breakpoints (the whole list): the program stops when the value changes.
    pub fn set_data_breakpoints(&self, data_ids: &[String]) -> Result<(), String> {
        let list: Vec<Value> = data_ids.iter().map(|id| json!({ "dataId": id, "accessType": "write" })).collect();
        let r = self.request("setDataBreakpoints", json!({ "breakpoints": list }), Duration::from_secs(10))?;
        if let Some(bad) = r["breakpoints"].as_array().and_then(|a| a.iter().find(|b| b["verified"].as_bool() == Some(false))) {
            return Err(clip_message(bad["message"].as_str().unwrap_or(crate::i18n::t("the debugger could not set the data breakpoint"))));
        }
        Ok(())
    }

    /// Without awaiting a response (order is kept: reaches the adapter before the next `continue`).
    pub fn set_data_breakpoints_async(&self, data_ids: &[String]) {
        let list: Vec<Value> = data_ids.iter().map(|id| json!({ "dataId": id, "accessType": "write" })).collect();
        let _ = self.request_async("setDataBreakpoints", json!({ "breakpoints": list }));
    }

    pub fn supports_data_breakpoints(&self) -> bool {
        self.capabilities.lock().unwrap()["supportsDataBreakpoints"].as_bool().unwrap_or(false)
    }

    #[cfg(test)]
    pub fn stack_trace(&self, thread_id: i64) -> Result<Vec<Frame>, String> {
        Ok(self.stack_page(thread_id, 0, 50)?.0)
    }

    /// A page of a thread's stack and the total depth when the adapter knows it.
    pub fn stack_page(&self, thread_id: i64, start: usize, levels: usize) -> Result<(Vec<Frame>, Option<usize>), String> {
        let r = self.request("stackTrace", json!({ "threadId": thread_id, "startFrame": start, "levels": levels }), Duration::from_secs(15))?;
        Ok((parse_frames(&r), r["totalFrames"].as_u64().map(|n| n as usize)))
    }

    /// Restart the program from a frame (`restartFrame`; the adapter then reports a stop).
    pub fn restart_frame(&self, frame_id: i64) {
        let _ = self.request_async("restartFrame", json!({ "frameId": frame_id }));
    }

    /// Change a variable in a scope or object; returns the value as the adapter shows it now.
    pub fn set_variable(&self, container: i64, name: &str, value: &str) -> Result<String, String> {
        let r = self.request("setVariable", json!({ "variablesReference": container, "name": name, "value": value }), Duration::from_secs(15))?;
        Ok(r["value"].as_str().unwrap_or(value).to_string())
    }

    /// Assign to an expression in a frame (adapters without `setVariable`).
    pub fn set_expression(&self, frame_id: Option<i64>, expression: &str, value: &str) -> Result<String, String> {
        let mut args = json!({ "expression": expression, "value": value });
        if let Some(f) = frame_id {
            args["frameId"] = json!(f);
        }
        let r = self.request("setExpression", args, Duration::from_secs(15))?;
        Ok(r["value"].as_str().unwrap_or(value).to_string())
    }

    /// Completions for the console input (`column` is the 1-based cursor column).
    pub fn completions(&self, frame_id: Option<i64>, text: &str, column: usize) -> Result<Vec<String>, String> {
        let mut args = json!({ "text": text, "column": column });
        if let Some(f) = frame_id {
            args["frameId"] = json!(f);
        }
        let r = self.request("completions", args, Duration::from_secs(5))?;
        Ok(r["targets"]
            .as_array()
            .map(|a| a.iter().filter_map(|t| t["text"].as_str().or(t["label"].as_str()).map(str::to_string)).collect())
            .unwrap_or_default())
    }

    pub fn scopes(&self, frame_id: i64) -> Result<Vec<Scope>, String> {
        Ok(parse_scopes(&self.request("scopes", json!({ "frameId": frame_id }), Duration::from_secs(15))?))
    }

    pub fn variables(&self, reference: i64) -> Result<Vec<Variable>, String> {
        Ok(parse_variables(&self.request("variables", json!({ "variablesReference": reference }), Duration::from_secs(15))?))
    }

    /// Only the named children (`filter: named`), for values whose elements are paged.
    pub fn named_variables(&self, reference: i64) -> Result<Vec<Variable>, String> {
        Ok(parse_variables(&self.request("variables", json!({ "variablesReference": reference, "filter": "named" }), Duration::from_secs(15))?))
    }

    /// A page of indexed children (array elements `start..start + count`).
    pub fn indexed_variables(&self, reference: i64, start: usize, count: usize) -> Result<Vec<Variable>, String> {
        let args = json!({ "variablesReference": reference, "filter": "indexed", "start": start, "count": count });
        Ok(parse_variables(&self.request("variables", args, Duration::from_secs(15))?))
    }

    /// Evaluate an expression (`context`: "repl", "watch", "hover"): (result, child reference).
    pub fn evaluate(&self, frame_id: Option<i64>, expression: &str, context: &str) -> Result<(String, i64), String> {
        let mut args = json!({ "expression": expression, "context": context });
        if let Some(f) = frame_id {
            args["frameId"] = json!(f);
        }
        let r = self.request("evaluate", args, Duration::from_secs(15))?;
        Ok((r["result"].as_str().unwrap_or_default().to_string(), r["variablesReference"].as_i64().unwrap_or(0)))
    }

    /// `continue`, `next`, `stepIn`, `stepOut`, `pause`.
    pub fn control(&self, command: &str, thread_id: i64) {
        let _ = self.request_async(command, json!({ "threadId": thread_id }));
    }

    /// Close the session: the program ends too; the adapter is killed if it doesn't exit shortly.
    pub fn stop(&self) {
        if let Some(c) = self.child.lock().unwrap().take() {
            c.stop();
        }
        if self.is_alive() {
            // `attach`: just detach, the program keeps running; `launch`: the program ends too.
            let _ = self.raw_request("disconnect", json!({ "terminateDebuggee": !self.attached.load(Ordering::Relaxed) }), Duration::from_secs(2));
        }
        if let Writer::Tcp(s) = &*self.writer {
            let _ = s.lock().unwrap().shutdown(std::net::Shutdown::Both);
        }
        if let Some((host, id)) = &self.proc {
            host.proc_kill(*id);
        }
        release_forward(&self.forward.lock().unwrap().take());
    }

    fn route(&self, msg: Value) {
        if std::env::var_os("CODEYARD_DAP_TRACE").is_some() {
            eprintln!("dap<- {}", msg.to_string().chars().take(400).collect::<String>());
        }
        match msg["type"].as_str() {
            Some("response") => {
                let Some(seq) = msg["request_seq"].as_i64() else { return };
                if let Some(tx) = self.pending.lock().unwrap().remove(&seq) {
                    let result = if msg["success"].as_bool().unwrap_or(false) {
                        Ok(msg.get("body").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(clip_message(msg["message"].as_str().or(msg["body"]["error"]["format"].as_str()).unwrap_or(crate::i18n::t("request failed"))))
                    };
                    let _ = tx.send(result);
                }
            }
            Some("event") => {
                let b = &msg["body"];
                let ev = match msg["event"].as_str() {
                    Some("initialized") => {
                        *self.initialized.0.lock().unwrap() = true;
                        self.initialized.1.notify_all();
                        DapEvent::Initialized
                    }
                    Some("stopped") => DapEvent::Stopped {
                        thread_id: b["threadId"].as_i64().unwrap_or(0),
                        reason: b["reason"].as_str().unwrap_or("stopped").to_string(),
                        text: b["text"].as_str().or(b["description"].as_str()).map(str::to_string),
                    },
                    Some("continued") => DapEvent::Continued,
                    Some("output") => DapEvent::Output {
                        category: b["category"].as_str().unwrap_or("console").to_string(),
                        text: b["output"].as_str().unwrap_or_default().to_string(),
                    },
                    Some("terminated") => DapEvent::Terminated,
                    Some("exited") => DapEvent::Exited(b["exitCode"].as_i64().unwrap_or(0) as i32),
                    Some("breakpoint") => DapEvent::BreakpointChanged(parse_bp_result(&b["breakpoint"])),
                    Some("thread") => DapEvent::Thread { thread_id: b["threadId"].as_i64().unwrap_or(0), started: b["reason"].as_str() == Some("started") },
                    Some("capabilities") => {
                        if let (Some(new), Value::Object(caps)) = (b["capabilities"].as_object(), &mut *self.capabilities.lock().unwrap()) {
                            caps.extend(new.clone());
                        }
                        DapEvent::Capabilities
                    }
                    _ => return,
                };
                let _ = self.events.send(ev);
            }
            // Request from the adapter: `startDebugging` and `runInTerminal` are supported; others don't go unanswered.
            Some("request") => {
                let command = msg["command"].as_str().unwrap_or_default();
                // Only when it was offered: whatever is on the other end of the adapter's connection must not be able to
                // run a program by asking (a TCP adapter's port is reachable by other users of the machine).
                let offered = self.terminal.load(Ordering::Relaxed);
                let terminal = (command == "runInTerminal" && offered).then(|| TerminalRequest::parse(&msg["arguments"])).flatten();
                let ok = (command == "startDebugging" && self.addr.is_some()) || terminal.is_some();
                if let Some(t) = terminal {
                    let _ = self.events.send(DapEvent::RunInTerminal(t));
                }
                self.writer.send(&json!({
                    "seq": 0, "type": "response", "request_seq": msg["seq"], "success": ok, "command": command,
                    "message": if ok { Value::Null } else { json!("not supported") },
                }));
                if ok && command == "startDebugging" {
                    let (request, configuration) =
                        (msg["arguments"]["request"].as_str().unwrap_or("launch").to_string(), msg["arguments"]["configuration"].clone());
                    if let Some(me) = self.me.upgrade() {
                        let _ = std::thread::Builder::new().name("dap-child".into()).spawn(move || me.adopt_child(request, configuration));
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn parse_frames(body: &Value) -> Vec<Frame> {
    body["stackFrames"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| Frame {
                    id: f["id"].as_i64().unwrap_or(0),
                    name: f["name"].as_str().unwrap_or("?").to_string(),
                    // Frames without source give a fake path ("/usr/lib/dyld`_dyld_start") or keep the content
                    // in the adapter (sourceReference > 0): neither is an openable file.
                    path: f["source"]["path"]
                        .as_str()
                        .filter(|p| {
                            (p.starts_with('/') || p.as_bytes().get(1) == Some(&b':'))
                                && !p.contains('`')
                                && f["source"]["sourceReference"].as_i64().unwrap_or(0) == 0
                        })
                        .map(str::to_string),
                    line: f["line"].as_u64().unwrap_or(0) as u32,
                    column: f["column"].as_u64().unwrap_or(1) as u32,
                    can_restart: f["canRestart"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_scopes(body: &Value) -> Vec<Scope> {
    body["scopes"]
        .as_array()
        .map(|a| {
            a.iter().map(|s| Scope { name: s["name"].as_str().unwrap_or("?").to_string(), reference: s["variablesReference"].as_i64().unwrap_or(0) }).collect()
        })
        .unwrap_or_default()
}

pub fn parse_variables(body: &Value) -> Vec<Variable> {
    body["variables"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| Variable {
                    name: v["name"].as_str().unwrap_or("?").to_string(),
                    value: v["value"].as_str().unwrap_or_default().to_string(),
                    ty: v["type"].as_str().map(str::to_string),
                    reference: v["variablesReference"].as_i64().unwrap_or(0),
                    evaluate_name: v["evaluateName"].as_str().map(str::to_string),
                    indexed: v["indexedVariables"].as_u64().unwrap_or(0) as usize,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn parses_stack_scopes_and_variables() {
        let frames = parse_frames(&json!({ "stackFrames": [
            { "id": 1000, "name": "main", "source": { "path": "/p/main.c" }, "line": 7, "column": 5 },
            { "id": 1001, "name": "_start", "line": 0, "column": 0 },
            { "id": 1002, "name": "_dyld_start", "source": { "path": "/usr/lib/dyld`_dyld_start" }, "line": 1, "column": 0 },
        ] }));
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[2].path, None, "lldb's fake path doesn't count as a file");
        assert_eq!((frames[0].id, frames[0].name.as_str(), frames[0].path.as_deref(), frames[0].line), (1000, "main", Some("/p/main.c"), 7));
        assert_eq!(frames[1].path, None, "frame without source");
        let scopes = parse_scopes(&json!({ "scopes": [{ "name": "Locals", "variablesReference": 3 }] }));
        assert_eq!(scopes, vec![Scope { name: "Locals".into(), reference: 3 }]);
        let vars = parse_variables(&json!({ "variables": [
            { "name": "x", "value": "42", "type": "int", "variablesReference": 0 },
            { "name": "p", "value": "{...}", "variablesReference": 9 },
        ] }));
        assert_eq!(vars[0], Variable { name: "x".into(), value: "42".into(), ty: Some("int".into()), reference: 0, ..Default::default() });
        assert_eq!((vars[1].reference, vars[1].ty.clone()), (9, None));
        assert!(parse_frames(&json!({})).is_empty() && parse_variables(&Value::Null).is_empty());
    }

    /// The launch configuration's `debugAdapter` object that runs the fake adapter, logging to `log`.
    pub(crate) fn fake_adapter_json(script: &std::path::Path, log: &std::path::Path) -> String {
        use crate::test_paths::AppPath;
        let (command, args) = crate::qa_integration_tests::script_command(script, vec![log.app()]);
        json!({ "command": command, "args": args }).to_string()
    }

    /// Deterministic fake adapter (tests): the `launch` response comes after `configurationDone`;
    /// stops at the first breakpoint (line 3 if none), `next` advances one line, `continue`
    /// goes to the next breakpoint or ends the program. `argv[1]` is the log every command is written to.
    pub(crate) fn write_fake_adapter(dir: &std::path::Path) -> std::path::PathBuf {
        let script = dir.join("fake-dap.py");
        std::fs::write(
            &script,
            r#"#!/usr/bin/env python3
import sys, json
log = open(sys.argv[1], 'a')
def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            break
        k, v = line.split(b':', 1)
        headers[k.strip().lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers[b'content-length'])))
seq = [0]
def send(m):
    seq[0] += 1
    m['seq'] = seq[0]
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b)
    sys.stdout.buffer.flush()
def ok(req, body=None):
    send({'type': 'response', 'request_seq': req['seq'], 'success': True, 'command': req['command'], 'body': body or {}})
def event(name, body=None):
    send({'type': 'event', 'event': name, 'body': body or {}})
bps = {}
logs = {}
values = {}
def extras():
    return bool(launch and launch['arguments'].get('extras'))
launch = None
cur = 3
def stop(reason):
    event('stopped', {'reason': reason, 'threadId': 1, 'allThreadsStopped': True})
while True:
    m = read()
    if m.get('type') == 'response':
        continue
    cmd = m.get('command')
    log.write(cmd + ' ' + json.dumps(m.get('arguments', {}), sort_keys=True) + '\n'); log.flush()
    a = m.get('arguments', {})
    if cmd == 'initialize':
        ok(m, {'supportsConfigurationDoneRequest': True, 'supportsDataBreakpoints': True, 'supportsFunctionBreakpoints': True,
               'supportsSetVariable': True, 'supportsCompletionsRequest': True, 'supportsRestartFrame': True, 'exceptionBreakpointFilters': [{'filter': 'raised', 'label': 'Raised Exceptions'}, {'filter': 'uncaught', 'label': 'Uncaught Exceptions', 'default': True}]})
        event('initialized')
    elif cmd in ('launch', 'attach'):
        launch = m
        if a.get('fail'):
            send({'type': 'response', 'request_seq': m['seq'], 'success': False, 'command': 'launch', 'message': 'cannot run ' + a['program']})
            launch = None
    elif cmd == 'setBreakpoints':
        # A breakpoint with condition "skip" doesn't stop; a logpoint doesn't stop, it writes its message.
        entries = a.get('breakpoints', [])
        bps[a['source']['path']] = sorted(b['line'] for b in entries if b.get('condition') != 'skip' and 'logMessage' not in b)
        for b in entries:
            if 'logMessage' in b:
                logs.setdefault(a['source']['path'], {})[b['line']] = b['logMessage']
        # Lines past 10 have no code: not verified, with a reason.
        ok(m, {'breakpoints': [{'id': l, 'verified': l <= 10, 'line': l, 'message': None if l <= 10 else 'no code on this line'} for l in a.get('lines', [])]})
    elif cmd == 'setFunctionBreakpoints':
        ok(m, {'breakpoints': [{'verified': b['name'] != 'missing'} for b in a.get('breakpoints', [])]})
    elif cmd == 'configurationDone':
        ok(m)
        # `sneak`: ask for a terminal although the client did not offer one (a hostile peer would).
        if launch and (launch['arguments'].get('console') == 'integratedTerminal' or launch['arguments'].get('sneak')):
            send({'type': 'request', 'command': 'runInTerminal', 'arguments': {'kind': 'integrated', 'title': 'Fake program', 'cwd': launch['arguments'].get('cwd', '/'),
                  'args': ['sh', '-c', 'echo ran-in-terminal > ' + launch['arguments']['marker']], 'env': {'FAKE_ENV': 'yes'}}})
        if launch and launch['arguments'].get('noDebug'):
            ok(launch)
            event('output', {'category': 'stdout', 'output': 'ran without debugging\n'})
            event('exited', {'exitCode': 0})
            event('terminated')
        elif launch:
            ok(launch)
            event('output', {'category': 'stdout', 'output': 'hello from the program\n'})
            src = launch['arguments'].get('source')
            lines = bps.get(src, [])
            cur = lines[0] if lines else 3
            stop('breakpoint')
    elif cmd == 'setExceptionBreakpoints':
        ok(m, {})
    elif cmd == 'dataBreakpointInfo':
        ok(m, {'dataId': 'var:' + a['name'], 'description': a['name'] + ' (4 bytes)', 'accessTypes': ['write']})
    elif cmd == 'setDataBreakpoints':
        ok(m, {'breakpoints': [{'verified': True} for _ in a.get('breakpoints', [])]})
    elif cmd == 'threads':
        ok(m, {'threads': [{'id': 1, 'name': 'main'}, {'id': 2, 'name': 'worker'}]})
    elif cmd == 'stackTrace' and extras():
        # A deep stack, paged: 45 frames in all.
        src = launch['arguments'].get('source')
        start, levels = a.get('startFrame', 0), a.get('levels', 20)
        frames = [{'id': 200 + i, 'name': 'main' if i == 0 else 'frame%d' % i, 'source': {'path': src} if i == 0 else {}, 'line': cur if i == 0 else 0, 'column': 1, 'canRestart': i == 0}
                  for i in range(start, min(45, start + levels))]
        ok(m, {'stackFrames': frames, 'totalFrames': 45})
    elif cmd == 'stackTrace':
        src = launch['arguments'].get('source') if launch else None
        ok(m, {'stackFrames': [
            {'id': 100, 'name': 'main', 'source': {'path': src}, 'line': cur, 'column': 1},
            {'id': 101, 'name': 'caller', 'line': 0, 'column': 0}]})
    elif cmd == 'scopes':
        ok(m, {'scopes': [{'name': 'Locals', 'variablesReference': 10}]})
    elif cmd == 'variables' and a['variablesReference'] == 12:
        # 250 elements, asked for a page at a time, plus one named field.
        if a.get('filter') == 'named':
            ok(m, {'variables': [{'name': 'length', 'value': '250', 'variablesReference': 0}]})
        else:
            start, count = a.get('start', 0), a.get('count', 250)
            ok(m, {'variables': [{'name': '[%d]' % i, 'value': str(i * i), 'variablesReference': 0} for i in range(start, min(250, start + count))]})
    elif cmd == 'variables':
        if a['variablesReference'] == 10:
            vs = [{'name': 'x', 'value': values.get('x', str(cur * 10)), 'type': 'int', 'variablesReference': 0, 'evaluateName': 'x'},
                  {'name': 'point', 'value': '{x, y}', 'type': 'Point', 'variablesReference': 11}]
            if extras():
                vs.append({'name': 'big', 'value': 'array(250)', 'variablesReference': 12, 'indexedVariables': 250, 'evaluateName': 'big'})
            ok(m, {'variables': vs})
        else:
            ok(m, {'variables': [{'name': 'x', 'value': '1', 'variablesReference': 0}, {'name': 'y', 'value': '2', 'variablesReference': 0}]})
    elif cmd in ('next', 'stepIn', 'stepOut'):
        ok(m)
        cur += 1
        stop('step')
    elif cmd == 'continue':
        ok(m, {'allThreadsContinued': True})
        src = launch['arguments'].get('source') if launch else None
        later = [l for l in bps.get(src, []) if l > cur]
        for l, msg in sorted(logs.get(src, {}).items()):
            if l > cur and (not later or l < later[0]):
                event('output', {'category': 'console', 'output': 'LOG ' + msg + '\n'})
        if later:
            cur = later[0]
            stop('breakpoint')
        else:
            event('exited', {'exitCode': 0})
            event('terminated')
    elif cmd == 'setVariable':
        values[a['name']] = a['value']
        ok(m, {'value': a['value']})
    elif cmd == 'completions':
        word = a['text'][:a['column'] - 1].split(' ')[-1]
        ok(m, {'targets': [{'label': t} for t in ['point', 'print', 'x'] if t.startswith(word)]})
    elif cmd == 'restartFrame':
        ok(m)
        cur = 3
        stop('restart')
    elif cmd == 'evaluate':
        ok(m, {'result': 'eval:' + a['expression'], 'variablesReference': 0})
    elif cmd == 'disconnect':
        ok(m)
        sys.exit(0)
    else:
        send({'type': 'response', 'request_seq': m['seq'], 'success': False, 'command': cmd, 'message': 'unsupported'})
"#,
        )
        .unwrap();
        #[cfg(unix)]
        crate::test_files::make_executable(&script);
        script
    }

    fn wait_event(rx: &mpsc::Receiver<DapEvent>, want: impl Fn(&DapEvent) -> bool) -> DapEvent {
        let start = std::time::Instant::now();
        loop {
            let left = Duration::from_secs(10).saturating_sub(start.elapsed());
            let ev = rx.recv_timeout(left).expect("expected DAP event did not arrive");
            if want(&ev) {
                return ev;
            }
        }
    }

    #[test]
    fn session_runs_to_a_breakpoint_steps_and_finishes() {
        if std::process::Command::new("python3").arg("-V").output().is_err() {
            crate::test_sshd::skip_missing("python3");
            return;
        }
        let dir = std::env::temp_dir().join(format!("codeyard-dap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log.txt");
        let _ = std::fs::remove_file(&log);
        let script = write_fake_adapter(&dir);
        let (tx, rx) = mpsc::channel();
        let host: Arc<dyn Host> = Arc::new(host::local());
        let (command, args) = crate::qa_integration_tests::script_command(&script, vec![log.to_string_lossy().into_owned()]);
        let session = Session::start(host, &command, args, None, tx).unwrap();
        let src = "/p/main.c".to_string();
        let mut bps = BTreeMap::new();
        bps.insert(src.clone(), vec![Breakpoint::at(5), Breakpoint::at(9)]);
        session.begin("fake", "launch", json!({ "program": "/p/a.out", "source": src }), &bps, &[]).unwrap();

        // Output event and first stop: line 5.
        wait_event(&rx, |e| matches!(e, DapEvent::Output { text, .. } if text.contains("hello")));
        let DapEvent::Stopped { thread_id, reason, .. } = wait_event(&rx, |e| matches!(e, DapEvent::Stopped { .. })) else { unreachable!() };
        assert_eq!((thread_id, reason.as_str()), (1, "breakpoint"));
        let frames = session.stack_trace(1).unwrap();
        assert_eq!((frames[0].line, frames[0].path.as_deref()), (5, Some("/p/main.c")));
        let scopes = session.scopes(frames[0].id).unwrap();
        let vars = session.variables(scopes[0].reference).unwrap();
        assert_eq!((vars[0].name.as_str(), vars[0].value.as_str()), ("x", "50"));
        let point = session.variables(vars[1].reference).unwrap();
        assert_eq!(point.iter().map(|v| v.value.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(session.evaluate(Some(100), "x + 1", "repl").unwrap().0, "eval:x + 1");

        // Step: line 6; continue: second breakpoint at line 9; continue: the program ends.
        session.control("next", 1);
        wait_event(&rx, |e| matches!(e, DapEvent::Stopped { reason, .. } if reason == "step"));
        assert_eq!(session.stack_trace(1).unwrap()[0].line, 6);
        session.control("continue", 1);
        wait_event(&rx, |e| matches!(e, DapEvent::Stopped { reason, .. } if reason == "breakpoint"));
        assert_eq!(session.stack_trace(1).unwrap()[0].line, 9);
        session.control("continue", 1);
        wait_event(&rx, |e| matches!(e, DapEvent::Exited(0)));
        wait_event(&rx, |e| matches!(e, DapEvent::Terminated));
        session.stop();
        wait_event(&rx, |e| matches!(e, DapEvent::AdapterExited(_)));

        // Order: initialize → launch → setBreakpoints → setExceptionBreakpoints → configurationDone.
        let order: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().map(|l| l.split(' ').next().unwrap().to_string()).collect();
        assert_eq!(&order[..5], ["initialize", "launch", "setBreakpoints", "setExceptionBreakpoints", "configurationDone"], "{order:?}");
    }

    #[test]
    fn launch_failure_is_reported_with_the_adapters_message() {
        if std::process::Command::new("python3").arg("-V").output().is_err() {
            crate::test_sshd::skip_missing("python3");
            return;
        }
        let dir = std::env::temp_dir().join(format!("codeyard-dap-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = write_fake_adapter(&dir);
        let (tx, _rx) = mpsc::channel();
        let host: Arc<dyn Host> = Arc::new(host::local());
        let (command, args) = crate::qa_integration_tests::script_command(&script, vec![dir.join("log.txt").to_string_lossy().into_owned()]);
        let session = Session::start(host, &command, args, None, tx).unwrap();
        let err = session.begin("fake", "launch", json!({ "program": "/nope", "fail": true }), &BTreeMap::new(), &[]).unwrap_err();
        assert!(err.contains("cannot run /nope"), "{err}");
        session.stop();
    }

    #[test]
    fn a_missing_adapter_is_an_error_not_a_hang() {
        let (tx, _rx) = mpsc::channel();
        let host: Arc<dyn Host> = Arc::new(host::local());
        let r = Session::start(host, "/definitely/not/an/adapter", Vec::new(), None, tx);
        if let Ok(session) = r {
            // The process may start and die right away: `begin` must fail without waiting for a timeout.
            let started = std::time::Instant::now();
            assert!(session.begin("x", "launch", json!({}), &BTreeMap::new(), &[]).is_err());
            assert!(started.elapsed() < Duration::from_secs(20));
        }
    }
}
