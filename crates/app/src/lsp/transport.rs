//! JSON-RPC over the agent's piped process: framing, request/response matching,
//! replies to the server's own requests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use host::{Host, ProcEvent};
use serde_json::{Value, json};

use super::{Diagnostic, ServerConfig, parse_diagnostics, path_to_uri};

// --- Framing ---

pub fn encode(msg: &Value) -> Vec<u8> {
    let body = msg.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Upper bound for a single LSP/DAP message (64 MB; the largest real responses are a few MB).
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

/// Extracts complete messages from output that arrives in pieces.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, data: &[u8]) -> Vec<Value> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        while let Some(head_end) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&self.buf[..head_end]).to_string();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length:").or_else(|| l.strip_prefix("content-length:")))
                .and_then(|v| v.trim().parse::<usize>().ok());
            let Some(len) = len else {
                // Malformed header: skip (the server may have logged to stdout).
                self.buf.drain(..head_end + 4);
                continue;
            };
            // Oversized (or overflowing) length: a broken or hostile server. Waiting for the body would bloat memory and
            // `start + len` could overflow; since the stream is untrustworthy, the accumulated data is discarded.
            if len > MAX_MESSAGE {
                self.buf.clear();
                break;
            }
            let start = head_end + 4;
            if self.buf.len() < start + len {
                break;
            }
            if let Ok(v) = serde_json::from_slice(&self.buf[start..start + len]) {
                out.push(v);
            }
            self.buf.drain(..start + len);
        }
        // Without this, unbounded data before the header ends (a server dumping endless text to stdout) could bloat memory: whatever is left after the loop
        // is either headerless or a partial body, and the body limit is already `MAX_MESSAGE`.
        if self.buf.len() > MAX_MESSAGE + 64 * 1024 {
            self.buf.clear();
        }
        out
    }
}

// --- Client ---

#[derive(Debug, Clone)]
pub enum ServerEvent {
    Diagnostics {
        path: String,
        diagnostics: Vec<Diagnostic>,
    },
    /// Ongoing work ("indexing 45%"); `None`: finished.
    Progress(Option<String>),
    /// Message from the server to the user (error/warning).
    Message(String),
    /// `window/logMessage`: for the Output panel (type: 1 error … 4 log).
    Log {
        level: u8,
        text: String,
    },
    /// The server wants an edit applied (after a code action command):
    /// once applied, `Client::respond(id, …)`.
    ApplyEdit {
        id: Value,
        edit: Value,
    },
    /// `window/showMessageRequest`: the user picks one of the options (or dismisses it);
    /// the reply is `Client::respond(id, {"title": …} | null)`.
    Question {
        id: Value,
        message: String,
        actions: Vec<String>,
    },
    Exited(Option<i32>),
}

/// Queue discipline for events the UI has not taken yet. Only state that a later message fully replaces is folded together:
/// the diagnostics of a file (`publishDiagnostics` always carries the whole list) and the progress line. Everything else
/// (questions, edits the server asks for, messages, log lines, exit) stays queued in order and is never dropped.
pub fn merge_server_event(queue: &mut std::collections::VecDeque<ServerEvent>, ev: ServerEvent) {
    let slot = match &ev {
        ServerEvent::Diagnostics { path, .. } => queue.iter_mut().find(|e| matches!(e, ServerEvent::Diagnostics { path: p, .. } if p == path)),
        ServerEvent::Progress(_) => queue.iter_mut().find(|e| matches!(e, ServerEvent::Progress(_))),
        _ => None,
    };
    match slot {
        Some(slot) => *slot = ev,
        None => queue.push_back(ev),
    }
}

type Pending = Arc<Mutex<HashMap<i64, mpsc::Sender<Result<Value, String>>>>>;

pub struct Client {
    pub config: ServerConfig,
    host: Arc<dyn Host>,
    proc: u32,
    next_id: AtomicI64,
    pending: Pending,
    alive: Arc<AtomicBool>,
    pub capabilities: Value,
}

impl Client {
    /// Start the server and handshake. Error if the command is not found or fails to start.
    pub fn start(host: Arc<dyn Host>, config: ServerConfig, root: &str, events: mpsc::Sender<ServerEvent>) -> Result<Arc<Client>, String> {
        Self::start_with(host, config, root, events, Duration::from_secs(60))
    }

    fn start_with(
        host: Arc<dyn Host>,
        config: ServerConfig,
        root: &str,
        events: mpsc::Sender<ServerEvent>,
        init_timeout: Duration,
    ) -> Result<Arc<Client>, String> {
        let proc = host.proc_spawn(&config.command, config.args.clone(), Some(root.to_string())).map_err(|e| e.to_string())?;
        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let (writer_host, id) = (host.clone(), proc.id);
        // Reader: responses go to whoever is waiting, notifications to the event channel; it answers
        // the server's own requests (configuration, registration) itself.
        let reader = {
            let (pending, alive) = (pending.clone(), alive.clone());
            std::thread::Builder::new().name(format!("lsp-{}", config.language)).spawn(move || {
                let mut dec = Decoder::default();
                while let Ok(ev) = proc.events.recv() {
                    match ev {
                        ProcEvent::Output(data) => {
                            for msg in dec.push(&data) {
                                route(msg, &pending, &events, &writer_host, id);
                            }
                        }
                        ProcEvent::Exit(code) => {
                            alive.store(false, Ordering::Relaxed);
                            for (_, tx) in pending.lock().unwrap().drain() {
                                let _ = tx.send(Err(match code {
                                    Some(127) => "not found".to_string(),
                                    Some(c) => format!("exited with code {c}"),
                                    None => "stopped".to_string(),
                                }));
                            }
                            let _ = events.send(ServerEvent::Exited(code));
                            break;
                        }
                    }
                }
            })
        };
        if let Err(e) = reader {
            host.proc_kill(id);
            return Err(e.to_string());
        }
        let mut client = Client { config, host, proc: id, next_id: AtomicI64::new(1), pending, alive, capabilities: Value::Null };
        // A server that never answers (or answers with an error) must not be left running: the caller only sees the Err and
        // marks the slot failed, and a later restart would spawn a second process next to this one.
        let init = match client.request("initialize", initialize_params(root), init_timeout) {
            Ok(init) => init,
            Err(e) => {
                client.host.proc_kill(client.proc);
                return Err(e);
            }
        };
        client.capabilities = init["capabilities"].clone();
        client.notify("initialized", json!({}));
        client.notify("workspace/didChangeConfiguration", json!({ "settings": {} }));
        Ok(Arc::new(client))
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    fn send(&self, msg: Value) {
        self.host.proc_input(self.proc, encode(&msg));
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Send a request and wait for the response (called on a background thread).
    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        if !self.is_alive() {
            return Err(crate::i18n::t("language server is not running").into());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let result = rx.recv_timeout(timeout).map_err(|_| {
            self.pending.lock().unwrap().remove(&id);
            // Tell the server we gave up too (so it drops the expensive work).
            self.notify("$/cancelRequest", json!({ "id": id }));
            crate::i18n::tf("{0}: no answer from {1}", &[&method, &self.config.command])
        });
        result?
    }

    /// Reply to a request from the server (e.g. `workspace/applyEdit`).
    pub fn respond(&self, id: Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    /// Shut down politely (short wait), then kill the process.
    pub fn shutdown(&self) {
        if self.is_alive() {
            let _ = self.request("shutdown", Value::Null, Duration::from_millis(500));
            self.notify("exit", Value::Null);
        }
        self.host.proc_kill(self.proc);
    }

    /// Whether the server announced this character as a completion trigger.
    pub fn is_trigger(&self, ch: char) -> bool {
        self.capabilities["completionProvider"]["triggerCharacters"].as_array().is_some_and(|a| a.iter().any(|t| t.as_str().is_some_and(|t| t.starts_with(ch))))
    }

    /// Whether the server announced this capability (`inlayHintProvider`, `foldingRangeProvider` …).
    pub fn supports(&self, capability: &str) -> bool {
        !matches!(&self.capabilities[capability], Value::Null | Value::Bool(false))
    }

    pub fn wants_save(&self) -> bool {
        let sync = &self.capabilities["textDocumentSync"];
        sync.is_object() && (sync["save"].is_object() || sync["save"].as_bool() == Some(true))
    }
}

/// The most the UI is shown of one server-supplied message, option title and option count. A server can send megabytes
/// in a single `showMessage`; the status bar, the toast and the output panel all lay the text out in full.
const MAX_MESSAGE_CHARS: usize = 2000;
const MAX_ACTION_CHARS: usize = 200;
const MAX_ACTIONS: usize = 20;

/// Cuts `text` to `max` characters (never inside one) and marks the cut with an ellipsis.
pub fn clip_text(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

fn route(msg: Value, pending: &Pending, events: &mpsc::Sender<ServerEvent>, host: &Arc<dyn Host>, proc: u32) {
    let method = msg["method"].as_str();
    if std::env::var_os("CODEYARD_LSP_TRACE").is_some() {
        eprintln!("lsp<- {} {} bytes", method.unwrap_or("(response)"), msg.to_string().len());
    }
    match (msg.get("id"), method) {
        // Response.
        (Some(id), None) => {
            let Some(id) = id.as_i64() else { return };
            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                let result = match msg.get("error") {
                    Some(e) => Err(clip_text(e["message"].as_str().unwrap_or("error"), MAX_MESSAGE_CHARS)),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(result);
            }
        }
        // Request from the server: some servers wait if it is not answered.
        (Some(id), Some(method)) => {
            let result = match method {
                "workspace/configuration" => {
                    let n = msg["params"]["items"].as_array().map_or(0, |a| a.len());
                    json!(vec![Value::Null; n])
                }
                "window/workDoneProgress/create" | "client/registerCapability" | "client/unregisterCapability" => Value::Null,
                "workspace/workspaceFolders" => Value::Null,
                "window/showMessageRequest" => {
                    let p = &msg["params"];
                    let actions = p["actions"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x["title"].as_str().map(|t| clip_text(t, MAX_ACTION_CHARS))).take(MAX_ACTIONS).collect())
                        .unwrap_or_default();
                    let message = clip_text(p["message"].as_str().unwrap_or_default(), MAX_MESSAGE_CHARS);
                    let _ = events.send(ServerEvent::Question { id: id.clone(), message, actions });
                    return; // the UI answers once the user chooses
                }
                "workspace/applyEdit" => {
                    let _ = events.send(ServerEvent::ApplyEdit { id: id.clone(), edit: msg["params"]["edit"].clone() });
                    return; // the UI answers once the edit is applied
                }
                _ => {
                    let reply = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "not supported" } });
                    host.proc_input(proc, encode(&reply));
                    return;
                }
            };
            host.proc_input(proc, encode(&json!({ "jsonrpc": "2.0", "id": id, "result": result })));
        }
        (None, Some("textDocument/publishDiagnostics")) => {
            if let Some((path, diagnostics)) = parse_diagnostics(&msg["params"]) {
                let _ = events.send(ServerEvent::Diagnostics { path, diagnostics });
            }
        }
        (None, Some("$/progress")) => {
            let v = &msg["params"]["value"];
            let text = match v["kind"].as_str() {
                Some("end") => None,
                _ => {
                    let title = clip_text(v["title"].as_str().or(v["message"].as_str()).unwrap_or("working"), MAX_ACTION_CHARS);
                    Some(match v["percentage"].as_u64() {
                        Some(p) => format!("{title} {p}%"),
                        None => title,
                    })
                }
            };
            let _ = events.send(ServerEvent::Progress(text));
        }
        // 1 error, 2 warning; information messages are noise.
        (None, Some("window/logMessage")) => {
            let p = &msg["params"];
            let _ = events.send(ServerEvent::Log { level: p["type"].as_u64().unwrap_or(4) as u8, text: p["message"].as_str().unwrap_or_default().to_string() });
        }
        (None, Some("window/showMessage")) if msg["params"]["type"].as_u64().is_some_and(|t| t <= 2) => {
            let _ = events.send(ServerEvent::Message(clip_text(msg["params"]["message"].as_str().unwrap_or_default(), MAX_MESSAGE_CHARS)));
        }
        _ => {}
    }
}

/// Semantic token types announced to the server (the server supplies its own table via `legend`).
pub const SEMANTIC_TYPES: [&str; 14] = [
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "function",
    "method",
    "macro",
];

fn initialize_params(root: &str) -> Value {
    let uri = path_to_uri(root);
    let name = root.rsplit('/').next().unwrap_or(root);
    json!({
        "processId": Value::Null,
        "clientInfo": { "name": "codeyard", "version": env!("CARGO_PKG_VERSION") },
        "rootUri": uri,
        "rootPath": root,
        "workspaceFolders": [{ "uri": uri, "name": name }],
        "capabilities": {
            "general": { "positionEncodings": ["utf-16"] },
            "window": { "workDoneProgress": true },
            "workspace": {
                "configuration": true,
                "workspaceFolders": true,
                "didChangeConfiguration": { "dynamicRegistration": false },
                "fileOperations": { "willRename": true },
                "applyEdit": true,
                "workspaceEdit": { "documentChanges": true },
                "didChangeWatchedFiles": { "dynamicRegistration": false },
                "symbol": { "dynamicRegistration": false },
            },
            "textDocument": {
                "synchronization": { "didSave": true, "dynamicRegistration": false },
                "publishDiagnostics": { "relatedInformation": false },
                "hover": { "contentFormat": ["markdown", "plaintext"] },
                "definition": { "linkSupport": true },
                "references": {},
                "rename": { "prepareSupport": true },
                "inlayHint": {},
                "foldingRange": { "lineFoldingOnly": true },
                "documentLink": {},
                "callHierarchy": {},
                "documentHighlight": {},
                "codeLens": {},
                "semanticTokens": {
                    "requests": { "full": true },
                    "tokenTypes": SEMANTIC_TYPES,
                    "tokenModifiers": [],
                    "formats": ["relative"],
                },
                "typeHierarchy": {},
                "formatting": {},
                "rangeFormatting": {},
                "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                "signatureHelp": { "signatureInformation": { "parameterInformation": { "labelOffsetSupport": true } } },
                "codeAction": {
                    "codeActionLiteralSupport": { "codeActionKind": { "valueSet": [
                        "", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"
                    ] } },
                    "resolveSupport": { "properties": ["edit"] },
                    "dataSupport": true,
                },
                "completion": {
                    "completionItem": {
                        "snippetSupport": true,
                        "insertReplaceSupport": false,
                        "documentationFormat": ["plaintext", "markdown"],
                        "resolveSupport": { "properties": ["documentation", "detail", "additionalTextEdits"] },
                    },
                    "contextSupport": true,
                },
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::{hover_text, parse_locations, server_for};

    #[test]
    fn frames_split_across_chunks() {
        let a = encode(&json!({"id": 1, "result": "ğ"}));
        let b = encode(&json!({"method": "x"}));
        let mut all = a.clone();
        all.extend_from_slice(&b);
        let mut d = Decoder::default();
        let (p1, p2) = all.split_at(7);
        assert!(d.push(p1).is_empty());
        let msgs = d.push(p2);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["result"], "ğ", "Content-Length counts bytes");
    }

    #[test]
    fn a_hostile_content_length_is_dropped_instead_of_buffered_or_overflowing() {
        let mut d = Decoder::default();
        // usize::MAX would overflow `start + len`; 1 GB would be buffered forever.
        assert!(d.push(format!("Content-Length: {}\r\n\r\n{{}}", usize::MAX).as_bytes()).is_empty());
        assert!(d.push(b"Content-Length: 1073741824\r\n\r\nabc").is_empty());
        // The decoder recovers: the next well-formed message is understood.
        let ok = encode(&json!({"id": 9}));
        let msgs = d.push(&ok);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["id"], 9);
        // Endless output without a header terminator does not grow without bound.
        let mut d = Decoder::default();
        for _ in 0..6 {
            d.push(&vec![b'x'; 12 * 1024 * 1024]);
        }
        assert!(d.buf.len() <= MAX_MESSAGE + 12 * 1024 * 1024, "{}", d.buf.len());
    }

    fn diag(path: &str, n: usize) -> ServerEvent {
        ServerEvent::Diagnostics {
            path: path.into(),
            diagnostics: vec![Diagnostic { start: (0, 0), end: (0, 1), severity: 1, message: "m".into(), source: None }; n],
        }
    }

    /// A UI that stalls keeps the latest diagnostics per file and the latest progress line; every other event stays, in order.
    #[test]
    fn a_stalled_ui_holds_one_diagnostics_list_per_file_and_never_loses_other_events() {
        let mut q = std::collections::VecDeque::new();
        merge_server_event(&mut q, ServerEvent::Question { id: json!(1), message: "reload?".into(), actions: vec!["Yes".into()] });
        for i in 0..50_000 {
            merge_server_event(&mut q, diag(if i % 2 == 0 { "/a.rs" } else { "/b.rs" }, i % 7));
            merge_server_event(&mut q, ServerEvent::Progress(Some(format!("indexing {i}"))));
            if i % 10_000 == 0 {
                merge_server_event(&mut q, ServerEvent::Message(format!("warning {i}")));
            }
        }
        merge_server_event(&mut q, ServerEvent::Progress(None));
        merge_server_event(&mut q, ServerEvent::Exited(Some(1)));
        // 1 question + 2 files + 1 progress + 5 messages + exit; nothing but diagnostics and progress was folded.
        assert_eq!(q.len(), 10, "{} events queued", q.len());
        assert!(matches!(q.front(), Some(ServerEvent::Question { .. })), "ordering of unfoldable events is kept");
        assert!(matches!(q.back(), Some(ServerEvent::Exited(Some(1)))));
        let messages = q.iter().filter(|e| matches!(e, ServerEvent::Message(_))).count();
        assert_eq!(messages, 5, "messages are never merged");
        let progress: Vec<_> = q.iter().filter_map(|e| if let ServerEvent::Progress(p) = e { Some(p.clone()) } else { None }).collect();
        assert_eq!(progress, vec![None], "the final progress state wins");
        fn latest(q: &std::collections::VecDeque<ServerEvent>, path: &str) -> Option<usize> {
            q.iter().find_map(|e| match e {
                ServerEvent::Diagnostics { path: p, diagnostics } if p == path => Some(diagnostics.len()),
                _ => None,
            })
        }
        assert_eq!(latest(&q, "/a.rs"), Some(49_998 % 7));
        assert_eq!(latest(&q, "/b.rs"), Some(49_999 % 7));
        // A later empty list clears the file (the UI removes the entry), so it must replace, not be skipped.
        merge_server_event(&mut q, diag("/a.rs", 0));
        assert_eq!(latest(&q, "/a.rs"), Some(0));
    }

    /// The local host with a count of `proc_kill` calls.
    struct KillCounter {
        inner: host::RemoteHost,
        kills: std::sync::atomic::AtomicUsize,
    }

    impl Host for KillCounter {
        fn info(&self) -> &proto::HostInfo {
            self.inner.info()
        }
        fn label(&self) -> String {
            self.inner.label()
        }
        fn is_remote(&self) -> bool {
            false
        }
        fn call(&self, req: proto::Request) -> host::Result<proto::Response> {
            self.inner.call(req)
        }
        fn search(&self, query: proto::SearchQuery) -> host::Result<host::SearchStream> {
            self.inner.search(query)
        }
        fn hash_tree(&self, root: &str, ignore: Vec<String>, gitignore: bool, limit: u32, only: Option<Vec<String>>) -> host::Result<host::HashStream> {
            self.inner.hash_tree(root, ignore, gitignore, limit, only)
        }
        fn cancel(&self, id: u32) {
            self.inner.cancel(id)
        }
        fn pty_spawn(&self, cols: u16, rows: u16, cwd: Option<String>, shell: Option<String>) -> host::Result<host::Pty> {
            self.inner.pty_spawn(cols, rows, cwd, shell)
        }
        fn pty_input(&self, pty: u32, data: Vec<u8>) {
            self.inner.pty_input(pty, data)
        }
        fn pty_resize(&self, pty: u32, cols: u16, rows: u16) {
            self.inner.pty_resize(pty, cols, rows)
        }
        fn pty_kill(&self, pty: u32) {
            self.inner.pty_kill(pty)
        }
        fn watch(&self, paths: Vec<String>) {
            self.inner.watch(paths)
        }
        fn fs_events(&self) -> mpsc::Receiver<host::FsChanges> {
            self.inner.fs_events()
        }
        fn status_events(&self) -> mpsc::Receiver<host::ConnStatus> {
            self.inner.status_events()
        }
        fn proc_spawn(&self, program: &str, args: Vec<String>, cwd: Option<String>) -> host::Result<host::Proc> {
            self.inner.proc_spawn(program, args, cwd)
        }
        fn proc_input(&self, proc: u32, data: Vec<u8>) {
            self.inner.proc_input(proc, data)
        }
        fn proc_kill(&self, proc: u32) {
            self.kills.fetch_add(1, Ordering::SeqCst);
            self.inner.proc_kill(proc)
        }
    }

    #[test]
    fn a_server_that_never_answers_initialize_is_killed() {
        let host = Arc::new(KillCounter { inner: host::local(), kills: Default::default() });
        // `sleep` never reads the request, let alone answers it.
        let cfg = ServerConfig { language: "x".into(), command: "sleep".into(), args: vec!["30".into()] };
        let (tx, _rx) = mpsc::channel();
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let err = Client::start_with(host.clone(), cfg, &dir, tx, Duration::from_millis(300)).err().expect("no answer is an error");
        assert!(err.contains("no answer"), "{err}");
        assert_eq!(host.kills.load(Ordering::SeqCst), 1, "the silent server must not be left running");
    }

    #[test]
    fn server_messages_reach_the_ui_clipped() {
        let host: Arc<dyn Host> = Arc::new(host::local());
        let (tx, rx) = mpsc::channel();
        let pending: Pending = Arc::default();
        let big = "ğ".repeat(5 * 1024 * 1024 / 2);
        route(json!({ "method": "window/showMessage", "params": { "type": 1, "message": big } }), &pending, &tx, &host, 0);
        let Ok(ServerEvent::Message(m)) = rx.try_recv() else { panic!("no message") };
        assert!(m.chars().count() <= MAX_MESSAGE_CHARS + 1 && m.ends_with('…'), "{} chars", m.chars().count());

        let actions: Vec<Value> = (0..500).map(|_| json!({ "title": "x".repeat(10_000) })).collect();
        route(json!({ "id": 3, "method": "window/showMessageRequest", "params": { "message": big, "actions": actions } }), &pending, &tx, &host, 0);
        let Ok(ServerEvent::Question { message, actions, .. }) = rx.try_recv() else { panic!("no question") };
        assert!(message.chars().count() <= MAX_MESSAGE_CHARS + 1);
        assert_eq!(actions.len(), MAX_ACTIONS);
        assert!(actions.iter().all(|a| a.chars().count() <= MAX_ACTION_CHARS + 1));

        let (rtx, rrx) = mpsc::channel();
        pending.lock().unwrap().insert(7, rtx);
        route(json!({ "id": 7, "error": { "message": big } }), &pending, &tx, &host, 0);
        assert!(rrx.try_recv().unwrap().unwrap_err().chars().count() <= MAX_MESSAGE_CHARS + 1);
        assert_eq!(clip_text("kısa", 10), "kısa");
        assert_eq!(clip_text("şğüiö", 3), "şğü…");
    }

    /// Real language server (if clangd is available): handshake, diagnostics, hover, definition.
    #[test]
    fn real_clangd_round_trip() {
        if std::process::Command::new("clangd").arg("--version").output().is_err() {
            eprintln!("clangd not found, skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("codeyard-lsp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap(); // servers return the canonical path (the app opens it that way too)
        let src = "int topla(int a, int b) { return a + b; }\nint main(void) { return topla(1, yok); }\n";
        let file = dir.join("main.c");
        std::fs::write(&file, src).unwrap();
        let (root, path) = (dir.to_string_lossy().into_owned(), file.to_string_lossy().into_owned());
        let host: Arc<dyn Host> = Arc::new(host::local());
        let (tx, rx) = mpsc::channel();
        let cfg = server_for("c", &[]).unwrap();
        let client = Client::start(host, cfg, &root, tx).expect("clangd should start");
        assert!(client.is_alive());
        client.notify("textDocument/didOpen", json!({ "textDocument": { "uri": path_to_uri(&path), "languageId": "c", "version": 1, "text": src } }));
        let diags = loop {
            match rx.recv_timeout(Duration::from_secs(20)).expect("a diagnostic should arrive") {
                ServerEvent::Diagnostics { path: p, diagnostics } if p == path && !diagnostics.is_empty() => break diagnostics,
                _ => {}
            }
        };
        assert_eq!(diags[0].severity, 1);
        assert!(diags[0].message.contains("yok"), "{:?}", diags[0]);
        assert_eq!(diags[0].start.0, 1, "on the second line");

        let at = json!({ "textDocument": { "uri": path_to_uri(&path) }, "position": { "line": 1, "character": 26 } });
        let hover = client.request("textDocument/hover", at.clone(), Duration::from_secs(10)).unwrap();
        assert!(hover_text(&hover).is_some_and(|t| t.contains("topla")), "{hover}");
        let def = client.request("textDocument/definition", at, Duration::from_secs(10)).unwrap();
        assert_eq!(parse_locations(&def), vec![(path.clone(), (0, 4))]);
        client.shutdown();
    }
}
