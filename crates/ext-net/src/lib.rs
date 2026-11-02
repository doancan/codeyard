//! Network for extensions: HTTPS to the hosts the manifest lists in `net`, and nothing else. The app's broker makes
//! every extension request through a [`Client`]; the extension never opens a socket itself.
//!
//! The rules (docs/plugin-architecture.md §6.2):
//!
//! - `https://` only, to a host name that `net` allows (`ext_manifest::net_allows`). IP addresses in URLs are refused.
//! - Every connection's peer address must be public unicast ([`addr::check`]): no loopback, private, CGNAT, link-local,
//!   cloud metadata, NAT64, IPv4-mapped or other special ranges. The check is on the address actually connected to,
//!   so a DNS answer that changes between lookups does not get around it.
//! - At most [`limits::NET_REDIRECTS`] redirects, each checked against `net` and the address rules like the first URL.
//! - No proxy: environment variables such as `HTTPS_PROXY` are not read. Requests leave from the UI machine (§7).
//! - In a folder that is not trusted, no request is made at all.
//!
//! Errors follow the API's shape: [`Error::Denied`] carries the permission and why, the rest are `invalid`, `timeout`,
//! `cancelled` and `other`. Every message says what to change. No UI here; the app wires it to the broker.

pub mod addr;
mod body;
mod target;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;

use std::fmt;
use std::future::{Future, poll_fn};
use std::io;
use std::net::IpAddr;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::Poll;
use std::time::Duration;

use ext_manifest::limits;
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Method, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time::Instant;

pub use addr::Blocked;
pub use body::{Body, ResponseBody};
use body::{Outgoing, TooLarge};
use target::Target;

/// Why a request was denied. `as_str` is the `why` of the API's `denied { permission, why }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The host is not in `net`, or a redirect pointed to one that is not.
    NotDeclared,
    /// The folder is not trusted; extensions cannot send anything until it is.
    UntrustedWorkspace,
    /// The URL is not `https://`.
    NotHttps,
    /// The URL names an IP address instead of a host.
    IpLiteral,
    /// The host resolved to an address extensions may not reach.
    PeerAddress(Blocked),
}

impl Why {
    pub fn as_str(self) -> &'static str {
        match self {
            Why::NotDeclared => "not-declared",
            Why::UntrustedWorkspace => "untrusted-workspace",
            Why::NotHttps => "not-https",
            Why::IpLiteral => "ip-literal",
            Why::PeerAddress(_) => "peer-address",
        }
    }
}

/// A failed request. `code` is the API's error name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Refused by a rule; nothing was sent to the refused host.
    Denied { permission: &'static str, why: Why, message: String },
    /// The request itself is malformed (URL, headers, body).
    Invalid(String),
    /// The request did not finish within [`Limits::timeout`].
    Timeout(String),
    /// The caller cancelled it.
    Cancelled,
    /// Resolving, connecting, TLS, the server, a budget, or too many redirects.
    Other(String),
}

impl Error {
    pub(crate) fn denied(why: Why, message: String) -> Error {
        Error::Denied { permission: "net", why, message }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Error::Denied { .. } => "denied",
            Error::Invalid(_) => "invalid",
            Error::Timeout(_) => "timeout",
            Error::Cancelled => "cancelled",
            Error::Other(_) => "other",
        }
    }

    pub fn why(&self) -> Option<Why> {
        match self {
            Error::Denied { why, .. } => Some(*why),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Denied { message, .. } | Error::Invalid(message) | Error::Timeout(message) | Error::Other(message) => f.write_str(message),
            Error::Cancelled => f.write_str("the request was cancelled"),
        }
    }
}

impl std::error::Error for Error {}

/// Budgets of one request. The defaults are the platform's (`ext_manifest::limits`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// From the first connection to the last byte of the response body, redirects included.
    pub timeout: Duration,
    pub request_bytes: u64,
    pub response_bytes: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { timeout: Duration::from_secs(limits::NET_TIMEOUT_SECS), request_bytes: limits::NET_REQUEST_BYTES, response_bytes: limits::NET_RESPONSE_BYTES }
    }
}

/// Stops a request from another task: the guest cancelled it, or the extension stopped. Clones share the state.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<CancelState>);

#[derive(Debug, Default)]
struct CancelState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        loop {
            let mut notified = pin!(self.0.notify.notified());
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// Turns a host name into addresses. The system resolver in the app; tests map declared hosts to the addresses they
/// want refused.
pub trait Resolve: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str) -> Pin<Box<dyn Future<Output = io::Result<Vec<IpAddr>>> + Send + 'a>>;
}

/// The operating system's resolver (`getaddrinfo`), as every other program on the machine sees it.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve<'a>(&'a self, host: &'a str) -> Pin<Box<dyn Future<Output = io::Result<Vec<IpAddr>>> + Send + 'a>> {
        Box::pin(async move { Ok(tokio::net::lookup_host((host, 443)).await?.map(|a| a.ip()).collect()) })
    }
}

/// One request from an extension.
#[derive(Debug)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Body,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Request {
        Request { method: Method::GET, url: url.into(), headers: HeaderMap::new(), body: Body::Empty }
    }

    /// A request as the API carries it (`http-request`): method and header names as text, the body in one piece.
    /// Malformed names or values are `invalid`; the rules on headers apply when it is sent.
    pub fn from_parts(method: &str, url: impl Into<String>, headers: &[(String, String)], body: Vec<u8>) -> Result<Request, Error> {
        let method =
            Method::from_bytes(method.trim().to_ascii_uppercase().as_bytes()).map_err(|_| Error::Invalid(format!("'{method}' is not an HTTP method")))?;
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            let n = header::HeaderName::from_bytes(name.trim().as_bytes()).map_err(|_| Error::Invalid(format!("'{name}' is not a header name")))?;
            let v = HeaderValue::from_str(value).map_err(|_| Error::Invalid(format!("the value of header '{name}' has characters a header cannot carry")))?;
            map.append(n, v);
        }
        let body = if body.is_empty() { Body::Empty } else { Body::Full(body.into()) };
        Ok(Request { method, url: url.into(), headers: map, body })
    }

    /// Set a header, replacing what the caller set (the app adds `authorization` this way).
    pub fn set_header(&mut self, name: &'static str, value: &str) -> Result<(), Error> {
        let mut v = HeaderValue::from_str(value).map_err(|_| Error::Invalid(format!("the value of header '{name}' has characters a header cannot carry")))?;
        // Kept out of `Debug` output and logs.
        v.set_sensitive(true);
        self.headers.insert(header::HeaderName::from_static(name), v);
        Ok(())
    }

    /// Whether the caller set header `name` (lowercase).
    pub fn has_header(&self, name: &str) -> bool {
        self.headers.contains_key(name)
    }
}

impl Response {
    /// The headers as text pairs, as the API carries them; values that are not text are skipped.
    pub fn header_pairs(&self) -> Vec<(String, String)> {
        self.headers.iter().filter_map(|(n, v)| v.to_str().ok().map(|v| (n.as_str().to_string(), v.to_string()))).collect()
    }
}

/// The final response, after redirects. The body is read separately so it can stream.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    /// The URL that answered, after redirects.
    pub url: String,
    pub body: ResponseBody,
}

/// Headers the extension may not set: the connection is the client's business, and `host` must match the URL.
const FORBIDDEN_HEADERS: &[&str] = &["host", "connection", "keep-alive", "upgrade", "te", "trailer", "transfer-encoding", "content-length"];

/// Headers dropped when a redirect leaves the origin, as browsers do.
const CREDENTIAL_HEADERS: &[header::HeaderName] = &[header::AUTHORIZATION, header::COOKIE];

/// Makes requests for one extension, under its `net` permission and the folder's trust.
#[derive(Clone)]
pub struct Client {
    net: Vec<String>,
    untrusted: bool,
    limits: Limits,
    resolver: Arc<dyn Resolve>,
    tls: Arc<rustls::ClientConfig>,
    /// Tests run their TLS server on 127.0.0.1; this one address is let through so the success path can be tested.
    #[cfg(test)]
    test_peer: Option<IpAddr>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").field("net", &self.net).field("untrusted", &self.untrusted).field("limits", &self.limits).finish_non_exhaustive()
    }
}

impl Client {
    /// A client for the hosts in `net` (the extension's `Permissions::net`). TLS trusts the system's root
    /// certificates, loaded once per process.
    pub fn new(net: Vec<String>) -> Result<Client, Error> {
        Ok(Client::with_tls(net, native_tls()?))
    }

    pub(crate) fn with_tls(net: Vec<String>, tls: Arc<rustls::ClientConfig>) -> Client {
        Client {
            net,
            untrusted: false,
            limits: Limits::default(),
            resolver: Arc::new(SystemResolver),
            tls,
            #[cfg(test)]
            test_peer: None,
        }
    }

    /// `true` while the workspace is not trusted: every request is denied with `untrusted-workspace`.
    pub fn untrusted(mut self, untrusted: bool) -> Client {
        self.untrusted = untrusted;
        self
    }

    pub fn limits(mut self, limits: Limits) -> Client {
        self.limits = limits;
        self
    }

    pub fn resolver(mut self, resolver: Arc<dyn Resolve>) -> Client {
        self.resolver = resolver;
        self
    }

    /// Send `req`, following redirects. The response's body is still to be read.
    pub async fn send(&self, req: Request, cancel: &Cancel) -> Result<Response, Error> {
        if self.untrusted {
            return Err(Error::denied(
                Why::UntrustedWorkspace,
                "this folder is not trusted, so extensions cannot send requests; trust the folder to allow it".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let deadline = Instant::now() + self.limits.timeout;
        let Request { mut method, url, mut headers, body } = req;
        if method == Method::CONNECT {
            return Err(Error::Invalid("CONNECT is not allowed; send a normal request to the host".into()));
        }
        for name in headers.keys() {
            let n = name.as_str();
            if FORBIDDEN_HEADERS.contains(&n) || n.starts_with("proxy-") {
                return Err(Error::Invalid(format!("the header '{n}' is set by Codeyard; remove it from the request")));
            }
        }
        if let Body::Full(b) = &body
            && b.len() as u64 > self.limits.request_bytes
        {
            return Err(too_large(self.limits.request_bytes));
        }
        if !headers.contains_key(header::USER_AGENT) {
            headers.insert(header::USER_AGENT, HeaderValue::from_static(concat!("Codeyard/", env!("CARGO_PKG_VERSION"))));
        }
        let mut target = target::parse(&url)?;
        self.allow(&target.host, None)?;
        // `Some` while the body can still be sent: a stream only once.
        let mut body = Some(body);
        let mut redirects = 0;
        loop {
            let outgoing = match body.take() {
                Some(Body::Full(b)) => {
                    body = Some(Body::Full(b.clone()));
                    Outgoing::Full(Some(b))
                }
                Some(Body::Stream(rx)) => Outgoing::Stream { rx, sent: 0, limit: self.limits.request_bytes },
                Some(Body::Empty) | None => {
                    body = Some(Body::Empty);
                    Outgoing::Empty
                }
            };
            let url = target.url();
            let res = guard(self.exchange(&target, &method, &headers, outgoing), deadline, cancel, &url).await?;
            let status = res.status();
            let location = res.headers().get(header::LOCATION).cloned();
            let (Some(location), true) = (location, status.is_redirection() && status != StatusCode::NOT_MODIFIED) else {
                let (parts, incoming) = res.into_parts();
                let body = ResponseBody { incoming, received: 0, limit: self.limits.response_bytes, deadline, cancel: cancel.clone(), url: url.clone() };
                return Ok(Response { status: status.as_u16(), headers: parts.headers, url, body });
            };
            drop(res);
            if redirects == limits::NET_REDIRECTS {
                return Err(Error::Other(format!("{url}: more than {} redirects; call the final URL directly", limits::NET_REDIRECTS)));
            }
            redirects += 1;
            let location = location.to_str().map_err(|_| Error::Other(format!("{url}: the redirect location is not text")))?;
            let next = target::parse(&target::join(&target, location)).map_err(|e| redirected(&url, e))?;
            self.allow(&next.host, Some(&url))?;
            let see_other = status == StatusCode::SEE_OTHER && method != Method::HEAD;
            let post_to_get = matches!(status, StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND) && method == Method::POST;
            if see_other || post_to_get {
                method = Method::GET;
                body = Some(Body::Empty);
                for h in [header::CONTENT_TYPE, header::CONTENT_ENCODING, header::CONTENT_LANGUAGE] {
                    headers.remove(h);
                }
            } else if body.is_none() {
                return Err(Error::Other(format!(
                    "{url}: the server redirected a streamed body ({status}), which cannot be sent twice; send the body \
                     as bytes, or call the final URL directly"
                )));
            }
            if !next.same_origin(&target) {
                for h in CREDENTIAL_HEADERS {
                    headers.remove(h);
                }
            }
            target = next;
        }
    }

    /// `host` is in `net`; `from` is the URL that redirected here.
    fn allow(&self, host: &str, from: Option<&str>) -> Result<(), Error> {
        if ext_manifest::net_allows(&self.net, host) {
            return Ok(());
        }
        let fix = format!("add \"{host}\" to `net` in [permissions] of codeyard-extension.toml");
        Err(Error::denied(
            Why::NotDeclared,
            match from {
                Some(from) => format!("{from} redirected to {host}, which is not in the extension's `net` permission; {fix}"),
                None => format!("{host} is not in the extension's `net` permission; {fix}"),
            },
        ))
    }

    fn check_peer(&self, host: &str, ip: IpAddr) -> Result<(), Error> {
        #[cfg(test)]
        if self.test_peer == Some(ip) {
            return Ok(());
        }
        addr::check(ip).map_err(|blocked| {
            Error::denied(
                Why::PeerAddress(blocked),
                format!(
                    "{host} resolved to {ip}, a {blocked} address; extensions may connect only to public addresses, so \
                     use a host that is reachable from the internet"
                ),
            )
        })
    }

    /// One request on a new connection: resolve, check, connect, check the peer, TLS, HTTP/1.1.
    async fn exchange(&self, target: &Target, method: &Method, headers: &HeaderMap, body: Outgoing) -> Result<hyper::Response<Incoming>, Error> {
        let host = &target.host;
        let ips = self.resolver.resolve(host).await.map_err(|e| Error::Other(format!("cannot find {host} ({e}); check the host name")))?;
        let mut refused = None;
        let mut failed = None;
        let mut stream = None;
        for ip in ips {
            if let Err(e) = self.check_peer(host, ip) {
                refused.get_or_insert(e);
                continue;
            }
            match TcpStream::connect((ip, target.port)).await {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => failed = Some(e),
            }
        }
        let Some(stream) = stream else {
            return Err(match (failed, refused) {
                (Some(e), _) => Error::Other(format!("cannot connect to {}: {e}", target.authority())),
                (None, Some(denied)) => denied,
                (None, None) => Error::Other(format!("cannot find {host} (no addresses); check the host name")),
            });
        };
        // The address the socket really reached, which is what the rules are about.
        let peer = stream.peer_addr().map_err(|e| Error::Other(format!("cannot connect to {}: {e}", target.authority())))?;
        self.check_peer(host, peer.ip())?;
        let _ = stream.set_nodelay(true);
        let name = ServerName::try_from(host.clone()).map_err(|_| Error::Invalid(format!("{host} is not a valid host name")))?;
        let tls = tokio_rustls::TlsConnector::from(self.tls.clone())
            .connect(name, stream)
            .await
            .map_err(|e| Error::Other(format!("secure connection to {host} failed: {e}")))?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls)).await.map_err(|e| Error::Other(format!("{}: {e}", target.url())))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let mut req = hyper::Request::builder().method(method.clone()).uri(target.path.as_str());
        if let Some(h) = req.headers_mut() {
            h.clone_from(headers);
            let authority = HeaderValue::from_str(&target.authority()).map_err(|_| Error::Invalid(format!("{host} is not a valid host name")))?;
            h.insert(header::HOST, authority);
        }
        let req = req.body(body).map_err(|e| Error::Invalid(format!("{}: {e}", target.url())))?;
        sender.send_request(req).await.map_err(|e| {
            let mut source: Option<&dyn std::error::Error> = Some(&e);
            while let Some(s) = source {
                if let Some(TooLarge(limit)) = s.downcast_ref::<TooLarge>() {
                    return too_large(*limit);
                }
                source = s.source();
            }
            Error::Other(format!("{}: {e}", target.url()))
        })
    }
}

fn too_large(limit: u64) -> Error {
    Error::Other(format!("the request body is larger than {limit} bytes, the limit for one request; send it in parts"))
}

/// An error in the URL a redirect points to, told as such.
fn redirected(from: &str, e: Error) -> Error {
    match e {
        Error::Denied { permission, why, message } => Error::Denied { permission, why, message: format!("{from} redirected: {message}") },
        Error::Invalid(m) | Error::Other(m) => Error::Other(format!("{from} redirected: {m}")),
        e => e,
    }
}

/// `fut`, unless the deadline passes or the request is cancelled first.
pub(crate) async fn guard<T>(fut: impl Future<Output = Result<T, Error>>, deadline: Instant, cancel: &Cancel, url: &str) -> Result<T, Error> {
    let mut fut = pin!(fut);
    let mut sleep = pin!(tokio::time::sleep_until(deadline));
    let mut cancelled = pin!(cancel.cancelled());
    poll_fn(|cx| {
        if cancelled.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(Error::Cancelled));
        }
        if let Poll::Ready(r) = fut.as_mut().poll(cx) {
            return Poll::Ready(r);
        }
        if sleep.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(Error::Timeout(format!(
                "{url}: no complete answer within the time limit for one request; ask for less, or stream it in parts"
            ))));
        }
        Poll::Pending
    })
    .await
}

/// rustls with the system's root certificates, HTTP/1.1 only (the ALPN offer matches what hyper speaks).
fn native_tls() -> Result<Arc<rustls::ClientConfig>, Error> {
    use hyper_rustls::ConfigBuilderExt as _;
    static TLS: OnceLock<Result<Arc<rustls::ClientConfig>, String>> = OnceLock::new();
    TLS.get_or_init(|| {
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_native_roots()
            .map_err(|e| format!("cannot load the system's certificates: {e}"))?
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    })
    .clone()
    .map_err(Error::Other)
}
