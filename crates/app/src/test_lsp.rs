//! A fake language server for tests: a Python script driven by a JSON scenario that records every message it receives,
//! so a test can check what Codeyard sent and make the server answer the way the test needs.
//!
//! Scenario keys (all optional):
//! - `capabilities`: the `initialize` result's capabilities.
//! - `responses`: method → `{ "result": … }`, `{ "error": { "code": …, "message": … } }` or `"silent"` (never answers).
//!   Requests without an entry get error -32601, except `initialize` and `shutdown`.
//! - `on_open`: messages the server sends after each `textDocument/didOpen`, notifications or requests (give those an
//!   `id`). The string `"$uri"` anywhere inside is replaced with the opened document's URI.
//! - `after`: method → messages the server sends each time it receives that method (after answering it), e.g. a
//!   `workspace/configuration` request after `workspace/didChangeConfiguration`.
//!
//! Use it as a server: `Settings { lsp_servers: vec![("rust".into(), Some(fake.command().into()))], .. }`.

use std::path::{Path, PathBuf};

use serde_json::Value;

const SCRIPT: &str = r#"import json, sys
scenario_path, log_path, starts_path = sys.argv[1:4]
with open(scenario_path, encoding='utf-8') as f:
    scenario = json.load(f)
responses = scenario.get('responses', {})
with open(starts_path, 'a') as f:
    f.write('start\n')

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
    n = int(headers[b'content-length'])
    return json.loads(sys.stdin.buffer.read(n))

def send(m):
    b = json.dumps(dict(m, jsonrpc='2.0')).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b)
    sys.stdout.buffer.flush()

def record(m):
    with open(log_path, 'a', encoding='utf-8') as f:
        f.write(json.dumps(m) + '\n')

def subst(v, uri):
    if v == '$uri':
        return uri
    if isinstance(v, dict):
        return {k: subst(x, uri) for k, x in v.items()}
    if isinstance(v, list):
        return [subst(x, uri) for x in v]
    return v

while True:
    m = read()
    record(m)
    method, mid = m.get('method'), m.get('id')
    if method is None:
        continue  # a reply to one of our own requests: recorded, nothing to answer
    if method == 'exit':
        sys.exit(0)
    if mid is not None:
        if method in responses:
            r = responses[method]
            if r != 'silent':
                send(dict(r, id=mid))
        elif method == 'initialize':
            send({'id': mid, 'result': {'capabilities': scenario.get('capabilities', {})}})
        elif method == 'shutdown':
            send({'id': mid, 'result': None})
        else:
            send({'id': mid, 'error': {'code': -32601, 'message': 'method not found: ' + method}})
    if method == 'textDocument/didOpen':
        uri = m['params']['textDocument']['uri']
        for out in scenario.get('on_open', []):
            send(subst(out, uri))
    for out in scenario.get('after', {}).get(method, []):
        send(out)
"#;

pub struct FakeServer {
    script: PathBuf,
    scenario: PathBuf,
    log: PathBuf,
    starts: PathBuf,
}

fn python() -> &'static str {
    if cfg!(windows) { "python" } else { "python3" }
}

impl FakeServer {
    /// Writes the server into `dir`. `None` (with the skip reported) when Python 3 is not installed.
    pub fn new(dir: &Path, scenario: &Value) -> Option<Self> {
        let ok = std::process::Command::new(python()).arg("-c").arg("import sys; sys.exit(sys.version_info[0] != 3)").status().is_ok_and(|s| s.success());
        if !ok {
            crate::test_sshd::skip_missing("python3");
            return None;
        }
        std::fs::create_dir_all(dir).unwrap();
        let fake =
            FakeServer { script: dir.join("fake-ls.py"), scenario: dir.join("scenario.json"), log: dir.join("messages.jsonl"), starts: dir.join("starts.log") };
        std::fs::write(&fake.script, SCRIPT).unwrap();
        std::fs::write(&fake.scenario, scenario.to_string()).unwrap();
        let _ = std::fs::remove_file(&fake.log);
        let _ = std::fs::remove_file(&fake.starts);
        Some(fake)
    }

    /// The `lsp.servers` entry that starts this server.
    pub fn command(&self) -> (String, Vec<String>) {
        let s = |p: &PathBuf| p.to_string_lossy().into_owned();
        (python().into(), vec![s(&self.script), s(&self.scenario), s(&self.log), s(&self.starts)])
    }

    /// Every message received so far, in order (a line still being written is skipped).
    pub fn messages(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// The messages with this method (requests and notifications from Codeyard).
    pub fn received(&self, method: &str) -> Vec<Value> {
        self.messages().into_iter().filter(|m| m["method"] == method).collect()
    }

    /// Codeyard's replies to the server's own requests.
    pub fn replies(&self) -> Vec<Value> {
        self.messages().into_iter().filter(|m| m.get("method").is_none()).collect()
    }

    /// How many times the server was started.
    pub fn starts(&self) -> usize {
        std::fs::read_to_string(&self.starts).map(|s| s.lines().count()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    use gpui::TestAppContext;
    use serde_json::json;

    use super::*;
    use crate::qa_integration_tests::{setup_with, wait_ticking};
    use crate::settings::Settings;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-test-lsp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(d).unwrap()
    }

    fn spawn(fake: &FakeServer) -> Child {
        let (cmd, args) = fake.command();
        Command::new(cmd).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap()
    }

    fn frame(v: &Value) -> Vec<u8> {
        let b = v.to_string();
        format!("Content-Length: {}\r\n\r\n{b}", b.len()).into_bytes()
    }

    fn read_frame(out: &mut BufReader<std::process::ChildStdout>) -> Value {
        let mut len = 0;
        loop {
            let mut line = String::new();
            out.read_line(&mut line).unwrap();
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                len = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; len];
        out.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let start = std::time::Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(10), "timeout: {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Answers by scenario, -32601 for unknown requests, nothing for `silent`; records requests whole even when their
    /// bytes arrive in pieces, and records replies to its own requests.
    #[test]
    fn fake_server_answers_and_records_by_scenario() {
        let d = dir("protocol");
        let scenario =
            json!({ "responses": { "custom/ok": { "result": 42 }, "custom/err": { "error": { "code": -1, "message": "no" } }, "custom/quiet": "silent" } });
        let Some(fake) = FakeServer::new(&d, &scenario) else { return };
        let mut child = spawn(&fake);
        let mut input = child.stdin.take().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());

        // A request split in the middle of its header and its body.
        let bytes = frame(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "rootUri": "file:///p" } }));
        let (a, b) = bytes.split_at(9);
        input.write_all(a).unwrap();
        input.flush().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        input.write_all(b).unwrap();
        assert_eq!(read_frame(&mut out)["id"], 1);

        input.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": 2, "method": "custom/quiet" }))).unwrap();
        input.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": 3, "method": "custom/ok" }))).unwrap();
        let ok = read_frame(&mut out);
        assert_eq!((ok["id"].clone(), ok["result"].clone()), (json!(3), json!(42)), "the silent request (id 2) got no answer");
        input.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": 4, "method": "custom/err" }))).unwrap();
        assert_eq!(read_frame(&mut out)["error"]["message"], "no");
        input.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": 5, "method": "nobody/knows" }))).unwrap();
        assert_eq!(read_frame(&mut out)["error"]["code"], -32601);
        // A reply to a request of the server's own.
        input.write_all(&frame(&json!({ "jsonrpc": "2.0", "id": 900, "result": [null] }))).unwrap();
        input.write_all(&frame(&json!({ "jsonrpc": "2.0", "method": "exit" }))).unwrap();
        drop(input);
        assert!(child.wait().unwrap().success());

        let init = fake.received("initialize");
        assert_eq!(init.len(), 1, "recorded once, whole");
        assert_eq!(init[0]["params"]["rootUri"], "file:///p");
        assert_eq!(fake.received("custom/quiet").len(), 1);
        assert_eq!(fake.replies(), vec![json!({ "jsonrpc": "2.0", "id": 900, "result": [null] })]);
        assert_eq!(fake.starts(), 1);
    }

    /// Through the real workbench: the server is started for the file, sees `initialize` and `didOpen`, and what it
    /// publishes on open (with `$uri` filled in) reaches the Problems panel.
    #[gpui::test]
    fn workbench_talks_to_the_fake_server(cx: &mut TestAppContext) {
        let d = dir("workbench");
        std::fs::write(d.join("lib.rs"), "fn main() {}\n").unwrap();
        let scenario = json!({ "on_open": [{ "method": "textDocument/publishDiagnostics", "params": { "uri": "$uri", "diagnostics": [
            { "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 7 } }, "severity": 1, "message": "fake problem" }
        ] } }] });
        let Some(fake) = FakeServer::new(&d.join("server"), &scenario) else { return };
        let (wb, cx) = setup_with(cx, Settings { lsp_servers: vec![("rust".into(), Some(fake.command().into()))], ..Settings::default() });
        let file = d.join("lib.rs").to_string_lossy().into_owned();
        wb.update_in(cx, |wb, window, cx| wb.open_path(&file, window, cx));
        wait_ticking(cx, "the fake server's diagnostic in Problems", |cx| wb.read_with(cx, |wb, _| wb.problems().iter().any(|(p, n)| p == &file && *n == 1)));

        assert_eq!(fake.starts(), 1);
        let init = fake.received("initialize");
        assert_eq!(init.len(), 1);
        assert!(init[0]["params"]["rootUri"].as_str().is_some_and(|u| u.starts_with("file://")), "{}", init[0]);
        let opened = fake.received("textDocument/didOpen");
        assert_eq!(opened.len(), 1);
        assert!(opened[0]["params"]["textDocument"]["uri"].as_str().is_some_and(|u| u.ends_with("/lib.rs")), "{}", opened[0]);
        assert_eq!(opened[0]["params"]["textDocument"]["languageId"], "rust");
        wait_until("initialized", || !fake.received("initialized").is_empty());
    }
}
