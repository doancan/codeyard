//! Optional static preview, loopback by default. Files come from the active Host, including SSH.
use crate::web_preview_config::Config;
use host::Host;
use std::collections::{HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

const MAX_REQUEST: usize = 16 * 1024;
const MAX_WATCHED: usize = 512;
static WORKERS: AtomicUsize = AtomicUsize::new(0);
pub const LIVE_SCRIPT: &str = include_str!("web_preview_live.js");

pub enum Event {
    Watch,
    Error(String),
}

#[derive(Default)]
struct Changes {
    revision: u64,
    paths: HashSet<String>,
    history: VecDeque<(u64, String)>,
}

struct Shared {
    host: Arc<dyn Host>,
    root: String,
    authority: String,
    config: Config,
    token: String,
    stopped: AtomicBool,
    changes: Mutex<Changes>,
    events: mpsc::SyncSender<Event>,
}

pub struct Server {
    shared: Arc<Shared>,
    addr: SocketAddr,
    listen_addr: SocketAddr,
    listener: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    pub fn start(host: Arc<dyn Host>, root: String, config: Config) -> Result<(Self, mpsc::Receiver<Event>), String> {
        use ring::rand::{SecureRandom, SystemRandom};
        let mut bytes = [0u8; 24];
        SystemRandom::new().fill(&mut bytes).map_err(|_| "Could not create preview session".to_string())?;
        let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let bind = SocketAddr::new(config.host, config.port);
        let listener = TcpListener::bind(bind).map_err(|e| format!("{bind}: {e}"))?;
        let listen_addr = listener.local_addr().map_err(|e| e.to_string())?;
        // Wildcard addresses listen on all interfaces; navigation and accept-loop wakeup use loopback.
        let mut addr = listen_addr;
        if addr.ip().is_unspecified() {
            addr.set_ip(if addr.is_ipv4() { std::net::Ipv4Addr::LOCALHOST.into() } else { std::net::Ipv6Addr::LOCALHOST.into() });
        }
        let (tx, rx) = mpsc::sync_channel(1024);
        let shared = Arc::new(Shared {
            host,
            root: if root == "/" || (root.len() == 3 && root.ends_with(":/")) { root } else { root.trim_end_matches('/').to_string() },
            authority: addr.to_string(),
            config,
            token,
            stopped: AtomicBool::new(false),
            changes: Mutex::new(Changes::default()),
            events: tx,
        });
        let state = shared.clone();
        let thread = std::thread::Builder::new()
            .name("web-preview".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if state.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = stream else { break };
                    if WORKERS.fetch_add(1, Ordering::AcqRel) >= 8 {
                        WORKERS.fetch_sub(1, Ordering::AcqRel);
                        let _ = stream.shutdown(Shutdown::Both);
                        continue;
                    }
                    let state = state.clone();
                    if std::thread::Builder::new()
                        .name("preview-request".into())
                        .spawn(move || {
                            struct Guard;
                            impl Drop for Guard {
                                fn drop(&mut self) {
                                    WORKERS.fetch_sub(1, Ordering::AcqRel);
                                }
                            }
                            let _guard = Guard;
                            serve(stream, &state);
                        })
                        .is_err()
                    {
                        WORKERS.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok((Self { shared, addr, listen_addr, listener: Some(thread) }, rx))
    }
    pub fn root(&self) -> &str {
        &self.shared.root
    }
    pub fn port(&self) -> u16 {
        self.addr.port()
    }
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }
    pub fn config(&self) -> &Config {
        &self.shared.config
    }
    pub fn uses_host(&self, host: &Arc<dyn Host>) -> bool {
        Arc::ptr_eq(&self.shared.host, host)
    }
    pub fn url(&self, relative: &str) -> String {
        format!("http://{}/{}?__codeyard_token={}", self.shared.authority, encode_path(relative), self.shared.token)
    }
    pub fn watched_paths(&self) -> Vec<String> {
        self.shared.changes.lock().unwrap_or_else(|e| e.into_inner()).paths.iter().map(|p| crate::ui::join(self.root(), p)).collect()
    }
    pub fn changed(&self, path: &str) {
        let Some(relative) = path.strip_prefix(&format!("{}/", self.root().trim_end_matches('/'))) else { return };
        let mut changes = self.shared.changes.lock().unwrap_or_else(|e| e.into_inner());
        if changes.paths.contains(relative) {
            changes.revision += 1;
            let revision = changes.revision;
            changes.history.push_back((revision, format!("/{}", encode_path(relative))));
            while changes.history.len() > 128 {
                changes.history.pop_front();
            }
        }
    }
    pub fn stop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.listener.take() {
            // Wake the accept loop; it performs no host I/O and exits before handling this socket.
            let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(100));
            let _ = thread.join();
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn is_html(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm"))
}

fn encode_path(path: &str) -> String {
    path.bytes().map(|b| if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") }).collect()
}

fn decode_path(path: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut source = path.bytes();
    while let Some(b) = source.next() {
        bytes.push(if b == b'%' {
            let a = char::from(source.next()?).to_digit(16)?;
            let b = char::from(source.next()?).to_digit(16)?;
            (a * 16 + b) as u8
        } else {
            b
        });
    }
    let decoded = String::from_utf8(bytes).ok()?;
    if !decoded.starts_with('/') || decoded.chars().any(|c| c.is_control() || matches!(c, '\\' | ':')) {
        return None;
    }
    let relative = decoded.trim_start_matches('/');
    if relative.split('/').any(|part| part.starts_with('.') || part.ends_with('.') || part.ends_with(' ')) {
        return None;
    }
    let mut relative = relative.split('/').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/");
    if !relative.is_empty() && decoded.ends_with('/') {
        relative.push('/');
    }
    Some(relative)
}

fn redirect_location(relative: &str, query: &str) -> String {
    let path = format!("/{}", encode_path(relative));
    if query.is_empty() { path } else { format!("{path}?{query}") }
}

fn mime(path: &str) -> Option<&'static str> {
    Some(match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => return None,
    })
}

fn reply(stream: &mut impl Write, status: u16, mime: &str, body: &[u8], head: bool, extra: &str) {
    let reason = match status {
        200 => "OK",
        302 => "Found",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Service Unavailable",
    };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    let _ = stream.write_all(headers.as_bytes());
    if !head {
        let _ = stream.write_all(body);
    }
}

fn reply_changes(stream: &mut impl Write, state: &Shared, since: Option<u64>, head: bool) {
    let changes = state.changes.lock().unwrap_or_else(|e| e.into_inner());
    let paths: Vec<_> = changes.history.iter().filter(|(rev, _)| since.is_some_and(|s| *rev > s)).map(|(_, p)| p).collect();
    let css_only =
        !paths.is_empty() && paths.iter().all(|p| p.ends_with(".css")) && since.is_some_and(|s| changes.history.front().is_none_or(|(rev, _)| s + 1 >= *rev));
    let body = serde_json::json!({"revision": changes.revision, "cssOnly": css_only, "paths": paths}).to_string();
    drop(changes);
    reply(stream, 200, "application/json", body.as_bytes(), head, "");
}

fn serve(mut stream: TcpStream, state: &Shared) {
    let Ok(local_addr) = stream.local_addr() else { return };
    let expected_authority = local_addr.to_string();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut bytes = Vec::new();
    let mut buf = [0u8; 1024];
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
        if bytes.len() >= MAX_REQUEST || std::time::Instant::now() >= deadline || state.stopped.load(Ordering::Acquire) {
            return;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
        }
    }
    let Ok(request) = std::str::from_utf8(&bytes) else { return };
    let mut lines = request.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("");
    let target = first.next().unwrap_or("");
    let head = method == "HEAD";
    let headers: Vec<_> = lines.take_while(|s| !s.is_empty()).filter_map(|s| s.split_once(':')).collect();
    let authority: Vec<_> = headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("host")).collect();
    let error = |stream: &mut TcpStream, status| {
        let path: String = target.split('?').next().unwrap_or("").chars().filter(|c| !c.is_control()).take(256).collect();
        let _ = state.events.try_send(Event::Error(format!("HTTP {status}: {path}")));
        reply(stream, status, "text/plain; charset=utf-8", b"Preview request rejected or file unavailable.", head, "");
    };
    if authority.len() != 1 || authority[0].1.trim() != expected_authority {
        return error(&mut stream, 403);
    }
    if method != "GET" && !head {
        return error(&mut stream, 405);
    }
    if target.chars().any(char::is_control) {
        return error(&mut stream, 400);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let Some(mut relative) = decode_path(path) else { return error(&mut stream, 403) };
    // Cookies are not scoped to ports: separate names let multiple workspaces coexist.
    let cookie = format!("codeyard_preview_{}={}", &state.token[..16], state.token);
    let authenticated = headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("cookie")).any(|(_, v)| v.split(';').any(|c| c.trim() == cookie));
    if query.split('&').any(|p| p == format!("__codeyard_token={}", state.token)) {
        let query = query.split('&').filter(|p| p.split('=').next() != Some("__codeyard_token")).collect::<Vec<_>>().join("&");
        let location = redirect_location(&relative, &query);
        return reply(
            &mut stream,
            302,
            "text/plain",
            b"",
            head,
            &format!("Location: {location}\r\nSet-Cookie: {cookie}; HttpOnly; SameSite=Strict; Path=/\r\n"),
        );
    }
    if !authenticated {
        return error(&mut stream, 403);
    }
    if headers.iter().any(|(k, v)| {
        (k.eq_ignore_ascii_case("sec-fetch-site") && !matches!(v.trim(), "same-origin" | "none"))
            || (k.eq_ignore_ascii_case("origin") && v.trim() != format!("http://{expected_authority}"))
    }) {
        return error(&mut stream, 403);
    }
    if relative == "__codeyard_live.js" {
        if !state.config.live_reload {
            return error(&mut stream, 404);
        }
        return reply(&mut stream, 200, "text/javascript; charset=utf-8", LIVE_SCRIPT.as_bytes(), head, "");
    }
    if relative == "__codeyard_changes" {
        if !state.config.live_reload {
            return error(&mut stream, 404);
        }
        let since = query.strip_prefix("since=").and_then(|s| s.parse::<u64>().ok());
        return reply_changes(&mut stream, state, since, head);
    }
    if relative.is_empty() || relative.ends_with('/') {
        relative.push_str("index.html");
    }
    let mut body = match state.host.read_preview(&state.root, &relative) {
        Ok((body, _)) => body,
        Err(host::HostError::Op(proto::Error::IsDirectory(_))) if !path.ends_with('/') => {
            let location = redirect_location(&format!("{relative}/"), query);
            return reply(&mut stream, 302, "text/plain", b"", head, &format!("Location: {location}\r\n"));
        }
        Err(e) => {
            let status = match &e {
                host::HostError::Op(proto::Error::PermissionDenied(_) | proto::Error::Unsupported(_)) => 403,
                host::HostError::Op(proto::Error::TooLarge { .. }) => 413,
                _ => 404,
            };
            let _ = state.events.try_send(Event::Error(e.to_string()));
            return error(&mut stream, status);
        }
    };
    let Some(content_type) = mime(&relative) else { return error(&mut stream, 403) };
    if state.stopped.load(Ordering::Acquire) {
        return;
    }
    if state.config.live_reload {
        let mut changes = state.changes.lock().unwrap_or_else(|e| e.into_inner());
        if changes.paths.len() < MAX_WATCHED && changes.paths.insert(relative.clone()) {
            let _ = state.events.try_send(Event::Watch);
        }
    }
    if state.config.live_reload && is_html(&relative) {
        // External same-origin script also works with a typical script-src 'self' policy.
        body.extend_from_slice(format!("\n<script src=\"/__codeyard_live.js\" data-interval=\"{}\"></script>\n", state.config.reload_interval).as_bytes());
    }
    reply(&mut stream, 200, content_type, &body, head, "");
}

#[cfg(test)]
#[path = "web_preview_tests.rs"]
pub(crate) mod tests;
