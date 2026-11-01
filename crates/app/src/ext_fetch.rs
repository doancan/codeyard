//! HTTPS for extensions, without UI: `net.fetch` to the hosts in `net`, `auth.fetch` signed with an account's token
//! to that provider's hosts, and the app's own calls to token endpoints. The broker checks the host first; ext-net then
//! enforces the rest (https only, peer address, re-checked redirects, budgets). A request runs on the caller's
//! (background) thread; its head and body stream to the exthost as `http-head` and `data` events, then the ticket
//! completes with `done`. The token is added here as a header and is never part of anything sent to the exthost.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ext_api::value::{self as v, Completion, StreamEvent};
use ext_manifest::OAuthProvider;

use crate::ext_accounts::{self as acc, Provider, Token};
use crate::ext_broker::{Scope, Sink};

/// Sends one request. ext-net in the app; a recording fake in tests.
pub trait Transport: Send + Sync {
    /// Send `req`, allowed to reach `hosts` (redirects included), with `authorization` added by the app. Streams
    /// `http-head` then `data` to `sink`; returns when the body has ended.
    fn send(&self, hosts: &[String], req: &v::HttpRequest, authorization: Option<&str>, sink: &dyn Sink, cancel: &Arc<AtomicBool>) -> Result<(), v::Error>;
}

/// ext-net, on a small runtime of the calling thread.
pub struct NetTransport;

impl Transport for NetTransport {
    fn send(&self, hosts: &[String], req: &v::HttpRequest, authorization: Option<&str>, sink: &dyn Sink, cancel: &Arc<AtomicBool>) -> Result<(), v::Error> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| v::Error::Other(e.to_string()))?;
        rt.block_on(async {
            let mut limits = ext_net::Limits::default();
            if let Some(ms) = req.timeout_ms {
                limits.timeout = limits.timeout.min(Duration::from_millis(ms.max(1) as u64));
            }
            let client = ext_net::Client::new(hosts.to_vec()).map_err(net_error)?.limits(limits);
            let mut request = ext_net::Request::from_parts(&req.method, req.url.clone(), &req.headers, req.body.clone()).map_err(net_error)?;
            if let Some(a) = authorization {
                request.set_header("authorization", a).map_err(net_error)?;
            }
            // `tickets.cancel` sets the flag; ext-net stops at its next await.
            let stop = ext_net::Cancel::new();
            let (watch, flag) = (stop.clone(), cancel.clone());
            let watcher = tokio::spawn(async move {
                while !flag.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                watch.cancel();
            });
            let result = async {
                let mut res = client.send(request, &stop).await.map_err(net_error)?;
                sink.event(StreamEvent::HttpHead(v::HttpHead { status: res.status, headers: res.header_pairs() }));
                while let Some(chunk) = res.body.chunk().await.map_err(net_error)? {
                    sink.event(StreamEvent::Data(chunk.to_vec()));
                }
                Ok(())
            }
            .await;
            watcher.abort();
            result
        })
    }
}

/// ext-net's errors in the API's words.
pub fn net_error(e: ext_net::Error) -> v::Error {
    match e {
        ext_net::Error::Denied { why: ext_net::Why::UntrustedWorkspace, .. } => {
            v::Error::Denied(v::Denied { permission: "net".into(), why: v::Why::UntrustedWorkspace })
        }
        ext_net::Error::Denied { why: ext_net::Why::NotDeclared, message, .. } => v::Error::Denied(v::Denied { permission: message, why: v::Why::NotDeclared }),
        ext_net::Error::Denied { message, .. } | ext_net::Error::Invalid(message) => v::Error::Invalid(message),
        ext_net::Error::Timeout(_) => v::Error::Timeout,
        ext_net::Error::Cancelled => v::Error::Cancelled,
        ext_net::Error::Other(m) => v::Error::Other(m),
    }
}

/// The lowercase host of an `https://` URL, without port; ext-net checks the rest of the URL.
pub fn https_host(url: &str) -> Result<String, v::Error> {
    let rest = url.trim().strip_prefix("https://").ok_or_else(|| v::Error::Invalid(format!("{url}: only https:// URLs can be fetched")))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(v::Error::Invalid(format!("{url}: a user name or password in the URL is not allowed")));
    }
    let host = match authority.rsplit_once(':') {
        Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) && !authority.starts_with('[') => h,
        _ => authority,
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return Err(v::Error::Invalid(format!("{url}: no host")));
    }
    Ok(host)
}

fn untrusted(permission: &str) -> v::Error {
    v::Error::Denied(v::Denied { permission: permission.into(), why: v::Why::UntrustedWorkspace })
}

fn done(r: Result<(), v::Error>) -> Completion {
    match r {
        Ok(()) => Completion::Done,
        Err(e) => Completion::Failed(e),
    }
}

/// `net.fetch`: to a host in `net`, refused in an untrusted folder.
pub fn net_fetch(t: &dyn Transport, scope: &Scope, req: &v::HttpRequest, sink: &dyn Sink, cancel: &Arc<AtomicBool>) -> Completion {
    if !scope.trusted {
        return Completion::Failed(untrusted("net"));
    }
    let host = match https_host(&req.url) {
        Ok(h) => h,
        Err(e) => return Completion::Failed(e),
    };
    if !ext_manifest::net_allows(&scope.permissions.net, &host) {
        return Completion::Failed(v::Error::Denied(not_in_net(scope, &host)));
    }
    done(t.send(&scope.permissions.net, req, None, sink, cancel))
}

/// The denial for a host `net` does not list: for an IP address, which `net` cannot list, why not; with
/// `net-from-setting`, the user's fix (they allow the host on the extension page); else the line to add.
pub fn not_in_net(scope: &Scope, host: &str) -> v::Denied {
    let permission = if ext_manifest::is_ip_address(host) {
        ext_manifest::ungrantable_line(&format!("net {host}"), "it is an IP address, and net lists host names only; connect to the server by its host name")
    } else if scope.permissions.net_from_setting.is_some() {
        ext_manifest::user_host_line(host)
    } else {
        format!("net = [\"{host}\"]")
    };
    v::Denied { permission, why: v::Why::NotDeclared }
}

/// The checks of `auth.fetch` that need no token: the permission, the folder's trust, and that the URL's host is one
/// the provider declares. A request that fails them is not signed and not sent.
pub fn may_sign(scope: &Scope, provider: &Provider, req: &v::HttpRequest) -> Result<(), v::Error> {
    // A self-managed instance (`gitlab@git.example.com`) is granted by its provider's entry.
    let granted = acc::granted_as(&provider.id);
    if !scope.trusted {
        return Err(untrusted(&format!("auth = [\"{granted}\"]")));
    }
    if !scope.permissions.auth.iter().any(|a| a == granted) {
        return Err(v::Error::Denied(v::Denied { permission: format!("auth = [\"{granted}\"]"), why: v::Why::NotDeclared }));
    }
    let host = https_host(&req.url)?;
    if !ext_manifest::net_allows(&provider.hosts, &host) {
        // `[[oauth]] gitlab hosts = ["evil.example.org"]`: the SDKs keep a line with `=` and name the real fix.
        return Err(v::Error::Denied(v::Denied { permission: ext_manifest::oauth_host_line(&provider.id, &host), why: v::Why::NotDeclared }));
    }
    if req.headers.iter().any(|(n, _)| n.trim().eq_ignore_ascii_case("authorization")) {
        return Err(v::Error::Invalid("the authorization header is added by Codeyard; remove it from the request".into()));
    }
    Ok(())
}

/// `auth.fetch`: checked, then sent with `Authorization: Bearer <token>` to the provider's hosts only.
pub fn auth_fetch(
    t: &dyn Transport,
    scope: &Scope,
    provider: &Provider,
    token: &Token,
    req: &v::HttpRequest,
    sink: &dyn Sink,
    cancel: &Arc<AtomicBool>,
) -> Completion {
    if let Err(e) = may_sign(scope, provider, req) {
        return Completion::Failed(e);
    }
    done(t.send(&provider.hosts, req, Some(&format!("Bearer {}", token.access)), sink, cancel))
}

/// Collects a response for the app's own requests (token endpoints, account names).
#[derive(Default)]
struct Collect {
    status: std::sync::Mutex<u16>,
    body: std::sync::Mutex<Vec<u8>>,
}

const COLLECT_BYTES: usize = 1 << 20;

impl Sink for Collect {
    fn event(&self, e: StreamEvent) {
        match e {
            StreamEvent::HttpHead(h) => *self.status.lock().unwrap_or_else(|p| p.into_inner()) = h.status,
            StreamEvent::Data(d) => {
                let mut b = self.body.lock().unwrap_or_else(|p| p.into_inner());
                if b.len() + d.len() <= COLLECT_BYTES {
                    b.extend_from_slice(&d);
                }
            }
            _ => {}
        }
    }
}

/// One request of the app's own, whole: status and body.
pub fn fetch_all(t: &dyn Transport, hosts: &[String], req: &v::HttpRequest, authorization: Option<&str>) -> Result<(u16, Vec<u8>), String> {
    let c = Collect::default();
    t.send(hosts, req, authorization, &c, &Arc::new(AtomicBool::new(false))).map_err(|e| e.to_string())?;
    let status = *c.status.lock().unwrap_or_else(|p| p.into_inner());
    Ok((status, c.body.into_inner().unwrap_or_else(|p| p.into_inner())))
}

fn post_form(t: &dyn Transport, url: &str, body: String) -> Result<(u16, Vec<u8>), String> {
    let host = ext_manifest::check_https_url(url)?;
    let req = v::HttpRequest {
        method: "POST".into(),
        url: url.to_string(),
        headers: vec![("content-type".into(), "application/x-www-form-urlencoded".into()), ("accept".into(), "application/json".into())],
        body: body.into_bytes(),
        timeout_ms: Some(30_000),
    };
    fetch_all(t, &[host], &req, None)
}

/// Trade the authorization code for a token (RFC 6749 §4.1.3 with the PKCE verifier).
pub fn exchange_code(t: &dyn Transport, p: &OAuthProvider, extension: &str, code: &str, verifier: &str, now: u64) -> Result<Token, String> {
    let redirect = acc::redirect_uri(extension);
    let body = acc::form(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", &redirect),
        ("client_id", &p.client_id),
        ("code_verifier", verifier),
    ]);
    let (status, bytes) = post_form(t, &p.token_url, body)?;
    acc::parse_token_response(status, &bytes, now)
}

/// A new access token from the refresh token; the label stays, and the refresh token too when the provider sends
/// none.
pub fn refresh(t: &dyn Transport, p: &OAuthProvider, old: &Token, now: u64) -> Result<Token, String> {
    let r = old.refresh.as_deref().ok_or_else(|| crate::i18n::t("the sign-in expired; sign in again").to_string())?;
    let body = acc::form(&[("grant_type", "refresh_token"), ("refresh_token", r), ("client_id", &p.client_id)]);
    let (status, bytes) = post_form(t, &p.token_url, body)?;
    let mut new = acc::parse_token_response(status, &bytes, now)?;
    new.label = old.label.clone();
    new.refresh = new.refresh.or_else(|| old.refresh.clone());
    if new.scopes.is_empty() {
        new.scopes = old.scopes.clone();
    }
    Ok(new)
}

/// The account's name from `url` (an account URL, or GitHub's `/user`), asked with the token.
pub fn fetch_label(t: &dyn Transport, url: &str, token: &str) -> Option<String> {
    let host = ext_manifest::check_https_url(url).ok()?;
    let req = v::HttpRequest {
        method: "GET".into(),
        url: url.to_string(),
        headers: vec![("accept".into(), "application/json".into())],
        body: Vec::new(),
        timeout_ms: Some(30_000),
    };
    let (status, body) = fetch_all(t, &[host], &req, Some(&format!("Bearer {token}"))).ok()?;
    if (200..300).contains(&status) { acc::account_label(&body) } else { None }
}

#[cfg(test)]
#[path = "ext_fetch_tests.rs"]
pub(crate) mod tests;
