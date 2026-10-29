use std::sync::Mutex;

use ext_manifest::Permissions;

use super::*;
use crate::ext_accounts::{MemoryKeychain, Publisher, Vault};

/// What reached the fake server.
#[derive(Clone, Debug)]
pub struct Sent {
    pub hosts: Vec<String>,
    pub method: String,
    pub url: String,
    pub authorization: Option<String>,
    pub body: Vec<u8>,
}

/// A server in memory: answers by URL prefix, records every request, and keeps to `hosts` as ext-net does.
#[derive(Default)]
pub struct Fake {
    pub routes: Mutex<Vec<(String, u16, Vec<u8>)>>,
    pub sent: Mutex<Vec<Sent>>,
}

impl Fake {
    pub fn route(self, prefix: &str, status: u16, body: &str) -> Fake {
        self.routes.lock().unwrap().push((prefix.to_string(), status, body.as_bytes().to_vec()));
        self
    }

    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }
}

impl Transport for Fake {
    fn send(&self, hosts: &[String], req: &v::HttpRequest, authorization: Option<&str>, sink: &dyn Sink, cancel: &Arc<AtomicBool>) -> Result<(), v::Error> {
        let host = https_host(&req.url)?;
        if !ext_manifest::net_allows(hosts, &host) {
            return Err(v::Error::Denied(v::Denied { permission: format!("{host} is not in net"), why: v::Why::NotDeclared }));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(v::Error::Cancelled);
        }
        self.sent.lock().unwrap().push(Sent {
            hosts: hosts.to_vec(),
            method: req.method.clone(),
            url: req.url.clone(),
            authorization: authorization.map(str::to_string),
            body: req.body.clone(),
        });
        let routes = self.routes.lock().unwrap();
        let (status, body) = routes.iter().find(|(p, _, _)| req.url.starts_with(p.as_str())).map_or((404, Vec::new()), |(_, s, b)| (*s, b.clone()));
        sink.event(StreamEvent::HttpHead(v::HttpHead { status, headers: vec![("content-type".into(), "application/json".into())] }));
        for chunk in body.chunks(4) {
            sink.event(StreamEvent::Data(chunk.to_vec()));
        }
        Ok(())
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<StreamEvent>>);

impl Sink for Events {
    fn event(&self, e: StreamEvent) {
        self.0.lock().unwrap().push(e);
    }
}

impl Events {
    fn body(&self) -> String {
        let all = self.0.lock().unwrap();
        String::from_utf8(all.iter().filter_map(|e| if let StreamEvent::Data(d) = e { Some(d.clone()) } else { None }).flatten().collect()).unwrap()
    }

    /// Everything the exthost would get, as it goes over the wire.
    fn frames(&self) -> String {
        self.0.lock().unwrap().iter().map(|e| format!("{e:?}")).collect()
    }
}

fn scope(p: Permissions, trusted: bool) -> Scope {
    Scope { extension: "you.deploy".into(), permissions: p, roots: Vec::new(), trusted, dependencies: Vec::new() }
}

fn get(url: &str) -> v::HttpRequest {
    v::HttpRequest { method: "GET".into(), url: url.into(), headers: Vec::new(), body: Vec::new(), timeout_ms: None }
}

fn no_cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn why(c: &Completion) -> Option<v::Why> {
    match c {
        Completion::Failed(v::Error::Denied(d)) => Some(d.why),
        _ => None,
    }
}

#[test]
fn net_fetch_streams_from_declared_hosts_only() {
    let t = Fake::default().route("https://api.vercel.com/", 200, "{\"ok\":true}");
    let s = scope(Permissions { net: vec!["api.vercel.com".into()], ..Default::default() }, true);
    let ev = Events::default();
    assert_eq!(net_fetch(&t, &s, &get("https://api.vercel.com/v6/deployments"), &ev, &no_cancel()), Completion::Done);
    assert!(matches!(ev.0.lock().unwrap()[0], StreamEvent::HttpHead(v::HttpHead { status: 200, .. })), "the head comes first");
    assert_eq!(ev.body(), "{\"ok\":true}");
    assert_eq!(t.sent()[0].authorization, None, "net.fetch is never signed");
    // Another host is denied with the line to add; nothing is sent.
    let c = net_fetch(&t, &s, &get("https://evil.example.com/"), &Events::default(), &no_cancel());
    assert!(matches!(&c, Completion::Failed(v::Error::Denied(d)) if d.permission == "net = [\"evil.example.com\"]" && d.why == v::Why::NotDeclared), "{c:?}");
    assert!(matches!(net_fetch(&t, &s, &get("http://api.vercel.com/"), &Events::default(), &no_cancel()), Completion::Failed(v::Error::Invalid(_))));
    // An untrusted folder sends nothing at all.
    let u = scope(Permissions { net: vec!["api.vercel.com".into()], ..Default::default() }, false);
    assert_eq!(why(&net_fetch(&t, &u, &get("https://api.vercel.com/"), &Events::default(), &no_cancel())), Some(v::Why::UntrustedWorkspace));
    // A cancelled ticket stops it.
    let cancelled = Arc::new(AtomicBool::new(true));
    assert_eq!(net_fetch(&t, &s, &get("https://api.vercel.com/"), &Events::default(), &cancelled), Completion::Failed(v::Error::Cancelled));
    assert_eq!(t.sent().len(), 1);
}

#[test]
fn auth_fetch_signs_only_the_providers_hosts_and_never_returns_the_token() {
    let m = ext_manifest::parse(
        "[extension]\nid = \"you.deploy\"\nversion = \"1.0.0\"\nname = \"D\"\nlicense = \"MIT\"\n[permissions]\nauth = [\"github\"]\nnet = [\"example.com\"]\nwhy = \"x\"\n",
    )
    .unwrap();
    let github = crate::ext_accounts::provider(&m, "github").unwrap();
    let t = Fake::default().route("https://api.github.com/user", 200, "{\"login\":\"ada\"}");
    let s = scope(m.permissions.clone(), true);
    let token = Token { access: "gho_SECRET_1234".into(), refresh: None, expires: None, label: "ada".into(), scopes: Vec::new() };
    let ev = Events::default();
    assert_eq!(auth_fetch(&t, &s, &github, &token, &get("https://api.github.com/user"), &ev, &no_cancel()), Completion::Done);
    assert_eq!(t.sent()[0].authorization.as_deref(), Some("Bearer gho_SECRET_1234"), "the server gets the token");
    assert_eq!(t.sent()[0].hosts, ["api.github.com", "uploads.github.com"], "redirects stay on GitHub's hosts");
    assert_eq!(ev.body(), "{\"login\":\"ada\"}");
    assert!(!ev.frames().contains("gho_SECRET_1234"), "the extension gets the answer, not the token");
    // AC2: a host outside the provider's, even one in `net`, is not signed and not sent.
    for url in ["https://example.com/", "https://api.github.com.evil.example/", "https://evil.example/?u=https://api.github.com/"] {
        let c = auth_fetch(&t, &s, &github, &token, &get(url), &Events::default(), &no_cancel());
        assert_eq!(why(&c), Some(v::Why::NotDeclared), "{url}: {c:?}");
    }
    assert_eq!(t.sent().len(), 1);
    // The extension cannot set its own authorization, or use an account it did not declare.
    let mut forged = get("https://api.github.com/user");
    forged.headers.push(("Authorization".into(), "Bearer mine".into()));
    assert!(matches!(auth_fetch(&t, &s, &github, &token, &forged, &Events::default(), &no_cancel()), Completion::Failed(v::Error::Invalid(_))));
    let none = scope(Permissions { net: vec!["api.github.com".into()], ..Default::default() }, true);
    assert_eq!(why(&auth_fetch(&t, &none, &github, &token, &get("https://api.github.com/user"), &Events::default(), &no_cancel())), Some(v::Why::NotDeclared));
    let untrusted = scope(m.permissions.clone(), false);
    let c = auth_fetch(&t, &untrusted, &github, &token, &get("https://api.github.com/user"), &Events::default(), &no_cancel());
    assert_eq!(why(&c), Some(v::Why::UntrustedWorkspace));
    assert_eq!(t.sent().len(), 1);
}

fn acme() -> OAuthProvider {
    OAuthProvider {
        id: "acme".into(),
        name: "Acme".into(),
        authorize_url: "https://acme.example.com/oauth/authorize".into(),
        token_url: "https://acme.example.com/oauth/token".into(),
        client_id: "cy-123".into(),
        scopes: vec!["read".into()],
        hosts: vec!["api.acme.example.com".into()],
        account_url: Some("https://api.acme.example.com/me".into()),
    }
}

#[test]
fn the_code_is_traded_with_the_verifier_and_refreshed_later() {
    let t = Fake::default()
        .route("https://acme.example.com/oauth/token", 200, r#"{"access_token":"at1","token_type":"bearer","expires_in":60,"refresh_token":"rt1"}"#)
        .route("https://api.acme.example.com/me", 200, r#"{"preferred_username":"ada"}"#);
    let tok = exchange_code(&t, &acme(), "you.deploy", "the-code", "the-verifier", 1000).unwrap();
    assert_eq!((tok.access.as_str(), tok.expires), ("at1", Some(1060)));
    let sent = &t.sent()[0];
    assert_eq!((sent.method.as_str(), sent.hosts.as_slice()), ("POST", &["acme.example.com".to_string()][..]));
    let body = String::from_utf8(sent.body.clone()).unwrap();
    assert_eq!(
        body,
        "grant_type=authorization_code&code=the-code&redirect_uri=codeyard%3A%2F%2Fext%2Fyou.deploy%2Fcallback&client_id=cy-123&code_verifier=the-verifier"
    );
    assert_eq!(fetch_label(&t, "https://api.acme.example.com/me", &tok.access).as_deref(), Some("ada"));
    assert_eq!(t.sent()[1].authorization.as_deref(), Some("Bearer at1"));
    // Refresh keeps the name and the refresh token when the answer has none.
    let old = Token { label: "ada".into(), ..tok };
    let t2 = Fake::default().route("https://acme.example.com/oauth/token", 200, r#"{"access_token":"at2","expires_in":60}"#);
    let new = refresh(&t2, &acme(), &old, 2000).unwrap();
    assert_eq!((new.access.as_str(), new.refresh.as_deref(), new.label.as_str(), new.expires), ("at2", Some("rt1"), "ada", Some(2060)));
    assert!(String::from_utf8(t2.sent()[0].body.clone()).unwrap().starts_with("grant_type=refresh_token&refresh_token=rt1&client_id=cy-123"));
    // A refused code is an error in plain words.
    let t3 = Fake::default().route("https://acme.example.com/oauth/token", 400, r#"{"error":"invalid_grant"}"#);
    assert!(exchange_code(&t3, &acme(), "you.deploy", "x", "y", 0).unwrap_err().contains("invalid_grant"));
}

#[test]
fn https_hosts_are_parsed_strictly() {
    assert_eq!(https_host("https://API.GitHub.com:443/user?x=1").unwrap(), "api.github.com");
    assert_eq!(https_host("https://api.github.com./user").unwrap(), "api.github.com");
    assert!(https_host("https://user@api.github.com/").is_err());
    assert!(https_host("ftp://api.github.com/").is_err());
    assert!(https_host("https:///x").is_err());
}

#[test]
fn a_vault_and_a_transport_together_keep_the_token_on_the_app_side() {
    // The pieces the window joins: the token comes out of the keychain into the header, nowhere else.
    let kc = MemoryKeychain::default();
    let publisher = Publisher::of("you.deploy", Some(&ext_core::Source::File { path: "/x.cyx".into() }), None);
    let vault = Vault { keychain: &kc, publisher: &publisher, extension: "you.deploy" };
    vault.save_account("github", &Token { access: "gho_VAULT".into(), refresh: None, expires: None, label: "ada".into(), scopes: Vec::new() }).unwrap();
    let m = ext_manifest::parse("[extension]\nid = \"you.deploy\"\nversion = \"1.0.0\"\nname = \"D\"\nlicense = \"MIT\"\n[permissions]\nauth = [\"github\"]\n")
        .unwrap();
    let github = crate::ext_accounts::provider(&m, "github").unwrap();
    let t = Fake::default().route("https://api.github.com/", 200, "[]");
    let ev = Events::default();
    let tok = vault.account("github").unwrap().unwrap();
    assert_eq!(
        auth_fetch(&t, &scope(m.permissions.clone(), true), &github, &tok, &get("https://api.github.com/user/repos"), &ev, &no_cancel()),
        Completion::Done
    );
    assert!(!ev.frames().contains("gho_VAULT"));
    assert_eq!(t.sent()[0].authorization.as_deref(), Some("Bearer gho_VAULT"));
}
