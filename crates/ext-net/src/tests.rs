//! Every rule against a local resolver and a local TLS server. The server's certificate (testdata/, valid until 2126) is
//! issued by a test CA that only these tests trust; the production path keeps the system's roots.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::Method;
use hyper::body::Bytes;
use hyper::header::{self, HeaderValue};
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::target::{self, Target};
use crate::*;

const CA: &[u8] = include_bytes!("../testdata/ca.pem");
const LEAF: &[u8] = include_bytes!("../testdata/leaf.pem");
const LEAF_KEY: &[u8] = include_bytes!("../testdata/leaf.key");
const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn run<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

/// Host names to fixed addresses; counts lookups so tests can show nothing was resolved.
#[derive(Default)]
struct Map {
    hosts: HashMap<String, Vec<IpAddr>>,
    lookups: AtomicUsize,
}

impl Resolve for Map {
    fn resolve<'a>(&'a self, host: &'a str) -> Pin<Box<dyn Future<Output = io::Result<Vec<IpAddr>>> + Send + 'a>> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        let found = self.hosts.get(host).cloned().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown host"));
        Box::pin(async move { found })
    }
}

fn map(entries: &[(&str, &str)]) -> Arc<Map> {
    let mut hosts: HashMap<String, Vec<IpAddr>> = HashMap::new();
    for (h, ip) in entries {
        hosts.entry(h.to_string()).or_default().push(ip.parse().unwrap());
    }
    Arc::new(Map { hosts, lookups: AtomicUsize::new(0) })
}

fn test_tls() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from_pem_slice(CA).unwrap()).unwrap();
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

/// A client for `net`; the address rules apply in full.
fn client(net: &[&str], resolver: Arc<Map>) -> Client {
    Client::with_tls(net.iter().map(|s| s.to_string()).collect(), test_tls()).resolver(resolver)
}

/// The same, but the test server on 127.0.0.1 counts as public.
fn serving(net: &[&str], resolver: Arc<Map>) -> Client {
    let mut c = client(net, resolver);
    c.test_peer = Some(LOCAL);
    c
}

async fn get(c: &Client, url: &str) -> Result<Response, Error> {
    c.send(Request::get(url), &Cancel::new()).await
}

async fn text(c: &Client, req: Request) -> Result<(u16, String), Error> {
    let res = c.send(req, &Cancel::new()).await?;
    let status = res.status;
    Ok((status, String::from_utf8(res.body.bytes().await?).unwrap()))
}

fn denied(r: Result<Response, Error>) -> Why {
    match r {
        Err(Error::Denied { permission: "net", why, message }) => {
            assert!(!message.is_empty());
            why
        }
        other => panic!("expected denied, got {other:?}"),
    }
}

/// A TLS server for `*.example.test` on 127.0.0.1. Routes:
/// `/hello`, `/echo` (method, authorization and body back), `/big` (2048 bytes), `/slow` (never answers in time),
/// `/loop` (redirects to itself), `/hop/<n>` (n redirects, then hello), `/redirect/<status>?<location>`.
async fn server() -> u16 {
    let certs = vec![CertificateDer::from_pem_slice(LEAF).unwrap()];
    let key = PrivateKeyDer::from_pem_slice(LEAF_KEY).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(LOCAL, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else { return };
                let service = hyper::service::service_fn(handle);
                let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(tls), service).await;
            });
        }
    });
    port
}

async fn handle(req: hyper::Request<hyper::body::Incoming>) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or_default().to_string();
    let respond = |status: u16, location: Option<String>, body: Vec<u8>| {
        let mut res = hyper::Response::builder().status(status);
        if let Some(l) = location {
            res = res.header(header::LOCATION, l);
        }
        Ok(res.body(Full::new(Bytes::from(body))).unwrap())
    };
    match path.as_str() {
        "/hello" => respond(200, None, b"hello".to_vec()),
        "/echo" => {
            let method = req.method().to_string();
            let auth = req.headers().get(header::AUTHORIZATION).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
            let body = req.into_body().collect().await.map(|b| b.to_bytes().to_vec()).unwrap_or_default();
            respond(200, None, format!("{method} auth={auth} {}", String::from_utf8_lossy(&body)).into_bytes())
        }
        "/big" => respond(200, None, vec![b'x'; 2048]),
        "/slow" => {
            tokio::time::sleep(Duration::from_secs(30)).await;
            respond(200, None, Vec::new())
        }
        "/loop" => respond(302, Some("/loop".into()), Vec::new()),
        p if p.starts_with("/hop/") => match p["/hop/".len()..].parse::<u32>().unwrap() {
            0 => respond(200, None, b"hello".to_vec()),
            n => respond(302, Some(format!("/hop/{}", n - 1)), Vec::new()),
        },
        p if p.starts_with("/redirect/") => respond(p["/redirect/".len()..].parse().unwrap(), Some(query), Vec::new()),
        _ => respond(404, None, Vec::new()),
    }
}

#[test]
fn every_ssrf_fixture_is_refused_with_its_range() {
    let fixtures = [
        ("127.0.0.1", Blocked::Loopback),
        ("127.255.0.9", Blocked::Loopback),
        ("169.254.169.254", Blocked::Metadata),
        ("169.254.1.1", Blocked::LinkLocal),
        ("10.0.0.1", Blocked::Private),
        ("10.255.255.254", Blocked::Private),
        ("172.16.0.1", Blocked::Private),
        ("192.168.1.1", Blocked::Private),
        ("100.64.0.1", Blocked::Cgnat),
        ("100.127.255.254", Blocked::Cgnat),
        ("0.0.0.0", Blocked::Unspecified),
        ("224.0.0.1", Blocked::Multicast),
        ("255.255.255.255", Blocked::Multicast),
        ("192.0.2.1", Blocked::Documentation),
        ("198.18.0.1", Blocked::Reserved),
        ("240.0.0.1", Blocked::Reserved),
        ("::1", Blocked::Loopback),
        ("::", Blocked::Unspecified),
        ("::ffff:127.0.0.1", Blocked::Loopback),
        ("::ffff:169.254.169.254", Blocked::Metadata),
        ("::ffff:10.0.0.1", Blocked::Private),
        ("::127.0.0.1", Blocked::Loopback),
        ("::8.8.8.8", Blocked::Reserved),
        ("64:ff9b::7f00:1", Blocked::Nat64),
        ("64:ff9b::808:808", Blocked::Nat64),
        ("64:ff9b:1::1", Blocked::Nat64),
        ("fe80::1", Blocked::LinkLocal),
        ("fec0::1", Blocked::LinkLocal),
        ("fc00::1", Blocked::UniqueLocal),
        ("fd12:3456::1", Blocked::UniqueLocal),
        ("fd00:ec2::254", Blocked::Metadata),
        ("ff02::1", Blocked::Multicast),
        ("2001:db8::1", Blocked::Documentation),
        ("3fff::1", Blocked::Documentation),
        ("2002:7f00:1::", Blocked::Loopback),
        ("2002:a00:1::", Blocked::Private),
        ("2002:808:808::", Blocked::Reserved),
        ("2001::1", Blocked::Reserved),
        ("100::1", Blocked::Reserved),
    ];
    for (ip, want) in fixtures {
        assert_eq!(addr::check(ip.parse().unwrap()), Err(want), "{ip}");
        let c = client(&["api.example.test"], map(&[("api.example.test", ip)]));
        assert_eq!(run(async { denied(get(&c, "https://api.example.test/").await) }), Why::PeerAddress(want), "{ip}");
    }
    for ip in ["1.1.1.1", "8.8.8.8", "172.32.0.1", "100.128.0.1", "192.0.3.1", "2606:4700::1111", "2001:4860:4860::8888"] {
        assert_eq!(addr::check(ip.parse().unwrap()), Ok(()), "{ip}");
    }
}

#[test]
fn a_declared_host_succeeds_over_tls() {
    run(async {
        let port = server().await;
        let c = serving(&["api.example.test", "*.cdn.example.test"], map(&[("api.example.test", "127.0.0.1"), ("a.cdn.example.test", "127.0.0.1")]));
        let res = get(&c, &format!("https://API.example.test.:{port}/hello#frag")).await.unwrap();
        assert_eq!((res.status, res.url.as_str()), (200, format!("https://api.example.test:{port}/hello").as_str()));
        assert_eq!(res.body.bytes().await.unwrap(), b"hello");
        let res = get(&c, &format!("https://a.cdn.example.test:{port}/hello")).await.unwrap();
        assert_eq!(res.status, 200);
        // Of several addresses, the refused ones are skipped and the public one is used.
        let c = serving(&["api.example.test"], map(&[("api.example.test", "10.0.0.1"), ("api.example.test", "127.0.0.1")]));
        assert_eq!(get(&c, &format!("https://api.example.test:{port}/hello")).await.unwrap().status, 200);
    });
}

#[test]
fn bodies_stream_both_ways() {
    run(async {
        let port = server().await;
        let c = serving(&["api.example.test"], map(&[("api.example.test", "127.0.0.1")]));
        let url = format!("https://api.example.test:{port}/echo");
        let full = Request { method: Method::POST, url: url.clone(), headers: HeaderMap::new(), body: Body::Full(Bytes::from_static(b"one")) };
        assert_eq!(text(&c, full).await.unwrap(), (200, "POST auth= one".into()));
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            for part in ["a", "b", "c"] {
                tx.send(Bytes::from(part)).await.unwrap();
            }
        });
        let streamed = Request { method: Method::PUT, url: url.clone(), headers: HeaderMap::new(), body: Body::Stream(rx) };
        assert_eq!(text(&c, streamed).await.unwrap(), (200, "PUT auth= abc".into()));
        let mut res = get(&c, &format!("https://api.example.test:{port}/big")).await.unwrap();
        let mut n = 0;
        while let Some(chunk) = res.body.chunk().await.unwrap() {
            n += chunk.len();
        }
        assert_eq!(n, 2048);
    });
}

#[test]
fn redirects_are_checked_again() {
    run(async {
        let port = server().await;
        let resolver = map(&[
            ("api.example.test", "127.0.0.1"),
            ("other.example.test", "127.0.0.1"),
            ("evil.example.test", "127.0.0.1"),
            ("internal.example.test", "10.0.0.5"),
            ("metadata.example.test", "169.254.169.254"),
        ]);
        let c = serving(&["api.example.test", "other.example.test", "internal.example.test", "metadata.example.test"], resolver);
        let start = format!("https://api.example.test:{port}");
        // A chain that ends in a private range.
        let into_private = format!("{start}/redirect/302?{start}/redirect/301?https://internal.example.test:{port}/hello");
        let err = get(&c, &into_private).await.unwrap_err();
        assert_eq!(err.why(), Some(Why::PeerAddress(Blocked::Private)), "{err}");
        let to_metadata = format!("{start}/redirect/307?https://metadata.example.test/latest/meta-data/");
        assert_eq!(denied(get(&c, &to_metadata).await), Why::PeerAddress(Blocked::Metadata));
        // An undeclared host, even one with a public address.
        let err = get(&c, &format!("{start}/redirect/302?https://evil.example.test:{port}/hello")).await.unwrap_err();
        assert_eq!(err.why(), Some(Why::NotDeclared));
        assert!(err.to_string().contains("redirected to evil.example.test") && err.to_string().contains("add \"evil.example.test\""), "{err}");
        // Out of https, or to an address.
        assert_eq!(denied(get(&c, &format!("{start}/redirect/302?http://api.example.test/")).await), Why::NotHttps);
        assert_eq!(denied(get(&c, &format!("{start}/redirect/302?https://127.0.0.1:{port}/hello")).await), Why::IpLiteral);
        assert_eq!(denied(get(&c, &format!("{start}/redirect/302?https://[::1]:{port}/hello")).await), Why::IpLiteral);
        // Five are followed, the sixth is refused.
        let res = get(&c, &format!("{start}/hop/5")).await.unwrap();
        assert_eq!((res.status, res.url), (200, format!("{start}/hop/0")));
        let err = get(&c, &format!("{start}/hop/6")).await.unwrap_err();
        assert!(matches!(&err, Error::Other(m) if m.contains("more than 5 redirects")), "{err}");
        let err = get(&c, &format!("{start}/loop")).await.unwrap_err();
        assert!(err.to_string().contains("more than 5 redirects"), "{err}");
        // A 304 or a 3xx without a location is the answer itself.
        assert_eq!(get(&c, &format!("{start}/redirect/304?")).await.unwrap().status, 304);
    });
}

#[test]
fn redirects_change_method_and_drop_credentials_like_browsers() {
    run(async {
        let port = server().await;
        let c = serving(&["api.example.test", "other.example.test"], map(&[("api.example.test", "127.0.0.1"), ("other.example.test", "127.0.0.1")]));
        let start = format!("https://api.example.test:{port}");
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer t"));
        let post = |url: String, body: Body| Request { method: Method::POST, url, headers: headers.clone(), body };
        let same = post(format!("{start}/redirect/307?/echo"), Body::Full(Bytes::from_static(b"again")));
        assert_eq!(text(&c, same).await.unwrap().1, "POST auth=Bearer t again");
        let see_other = post(format!("{start}/redirect/303?/echo"), Body::Full(Bytes::from_static(b"x")));
        assert_eq!(text(&c, see_other).await.unwrap().1, "GET auth=Bearer t ");
        let elsewhere = post(format!("{start}/redirect/308?https://other.example.test:{port}/echo"), Body::Full(Bytes::from_static(b"y")));
        assert_eq!(text(&c, elsewhere).await.unwrap().1, "POST auth= y");
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        let err = text(&c, post(format!("{start}/redirect/307?/echo"), Body::Stream(rx))).await.unwrap_err();
        assert!(err.to_string().contains("streamed body"), "{err}");
    });
}

#[test]
fn urls_must_be_https_to_a_declared_name() {
    run(async {
        let resolver = map(&[("api.example.test", "1.1.1.1")]);
        let c = client(&["api.example.test"], resolver.clone());
        for url in ["https://127.0.0.1/", "https://[::1]/", "https://[::ffff:7f00:1]/", "https://2130706433/", "https://127.1/", "https://0x7f.0x1/"] {
            assert_eq!(denied(get(&c, url).await), Why::IpLiteral, "{url}");
        }
        for url in ["http://api.example.test/", "ftp://api.example.test/", "HTTP://api.example.test/"] {
            assert_eq!(denied(get(&c, url).await), Why::NotHttps, "{url}");
        }
        let err = get(&c, "https://evil.example.test/").await.unwrap_err();
        assert_eq!(err.why(), Some(Why::NotDeclared));
        assert!(err.to_string().contains("add \"evil.example.test\" to `net`"), "{err}");
        assert_eq!(get(&c, "https://example.test/").await.unwrap_err().why(), Some(Why::NotDeclared));
        for url in ["api.example.test/x", "https://user:pw@api.example.test/", "https://", "https://api.example.test:0/"] {
            assert_eq!(get(&c, url).await.unwrap_err().code(), "invalid", "{url}");
        }
        // Nothing above reached the resolver.
        assert_eq!(resolver.lookups.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn an_untrusted_folder_sends_nothing() {
    run(async {
        let resolver = map(&[("api.example.test", "1.1.1.1")]);
        let c = client(&["api.example.test"], resolver.clone()).untrusted(true);
        let err = get(&c, "https://api.example.test/").await.unwrap_err();
        assert_eq!((err.code(), err.why()), ("denied", Some(Why::UntrustedWorkspace)));
        assert!(err.to_string().contains("trust the folder"), "{err}");
        assert_eq!(resolver.lookups.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn headers_that_belong_to_the_connection_are_refused() {
    run(async {
        let c = client(&["api.example.test"], map(&[]));
        for name in ["host", "connection", "transfer-encoding", "proxy-authorization", "upgrade", "content-length"] {
            let mut req = Request::get("https://api.example.test/");
            req.headers.insert(header::HeaderName::from_static(name), HeaderValue::from_static("x"));
            let err = c.send(req, &Cancel::new()).await.unwrap_err();
            assert!(matches!(&err, Error::Invalid(m) if m.contains(name)), "{name}: {err}");
        }
        let mut req = Request::get("https://api.example.test/");
        req.method = Method::CONNECT;
        assert_eq!(c.send(req, &Cancel::new()).await.unwrap_err().code(), "invalid");
    });
}

#[test]
fn budgets_timeout_and_cancellation() {
    run(async {
        let port = server().await;
        let small = Limits { timeout: Duration::from_millis(300), request_bytes: 4, response_bytes: 1000 };
        let c = serving(&["api.example.test"], map(&[("api.example.test", "127.0.0.1")])).limits(small);
        let start = format!("https://api.example.test:{port}");
        let err = get(&c, &format!("{start}/big")).await.unwrap().body.bytes().await.unwrap_err();
        assert!(matches!(&err, Error::Other(m) if m.contains("larger than 1000 bytes")), "{err}");
        let full = Request { method: Method::POST, url: format!("{start}/echo"), headers: HeaderMap::new(), body: Body::Full(Bytes::from_static(b"12345")) };
        assert!(c.send(full, &Cancel::new()).await.unwrap_err().to_string().contains("larger than 4 bytes"));
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            for _ in 0..3 {
                let _ = tx.send(Bytes::from_static(b"abc")).await;
            }
        });
        let streamed = Request { method: Method::POST, url: format!("{start}/echo"), headers: HeaderMap::new(), body: Body::Stream(rx) };
        let err = c.send(streamed, &Cancel::new()).await.unwrap_err();
        assert!(err.to_string().contains("larger than 4 bytes"), "{err}");
        assert_eq!(get(&c, &format!("{start}/slow")).await.unwrap_err().code(), "timeout");

        let c = serving(&["api.example.test"], map(&[("api.example.test", "127.0.0.1")]));
        let cancel = Cancel::new();
        let later = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            later.cancel();
        });
        assert_eq!(c.send(Request::get(format!("{start}/slow")), &cancel).await.unwrap_err(), Error::Cancelled);
        assert_eq!(c.send(Request::get(format!("{start}/hello")), &cancel).await.unwrap_err(), Error::Cancelled);
        // Cancelling while the body is read.
        let cancel = Cancel::new();
        let mut res = c.send(Request::get(format!("{start}/hello")), &cancel).await.unwrap();
        cancel.cancel();
        assert_eq!(res.body.chunk().await.unwrap_err(), Error::Cancelled);
    });
}

#[test]
fn tls_is_verified() {
    run(async {
        let port = server().await;
        // Declared, public by the test's rules, but the certificate is for *.example.test.
        let c = serving(&["api.example.org"], map(&[("api.example.org", "127.0.0.1")]));
        let err = get(&c, &format!("https://api.example.org:{port}/hello")).await.unwrap_err();
        assert!(matches!(&err, Error::Other(m) if m.contains("secure connection to api.example.org failed")), "{err}");
        // No test CA: the system's roots do not know the test server.
        let mut c = Client::with_tls(vec!["api.example.test".into()], native_tls().unwrap()).resolver(map(&[("api.example.test", "127.0.0.1")]));
        c.test_peer = Some(LOCAL);
        let err = get(&c, &format!("https://api.example.test:{port}/hello")).await.unwrap_err();
        assert!(err.to_string().contains("secure connection"), "{err}");
    });
}

#[test]
fn redirect_locations_resolve_like_browsers() {
    let base = Target { host: "api.x.test".into(), port: 8443, path: "/a/b/c?q=1".into() };
    let cases = [
        ("https://other.test/p", "https://other.test/p"),
        ("//other.test/p", "https://other.test/p"),
        ("/root", "https://api.x.test:8443/root"),
        ("d", "https://api.x.test:8443/a/b/d"),
        ("../d", "https://api.x.test:8443/a/b/../d"),
        ("?z=2", "https://api.x.test:8443/a/b/c?z=2"),
        ("", "https://api.x.test:8443/a/b/c?q=1"),
        ("http://api.x.test/", "http://api.x.test/"),
        ("javascript:alert(1)", "javascript:alert(1)"),
    ];
    for (loc, want) in cases {
        assert_eq!(target::join(&base, loc), want, "{loc}");
    }
    let t = target::parse("https://Api.X.Test./p?q#f").unwrap();
    assert_eq!((t.host.as_str(), t.port, t.path.as_str(), t.url()), ("api.x.test", 443, "/p?q", "https://api.x.test/p?q".to_string()));
    assert_eq!(target::parse("https://api.x.test").unwrap().path, "/");
    assert_eq!(target::parse("javascript:alert(1)").unwrap_err().code(), "invalid");
}

#[test]
fn a_request_from_api_parts_carries_the_header_the_app_sets() {
    run(async {
        let port = server().await;
        let c = serving(&["api.example.test"], map(&[("api.example.test", "127.0.0.1")]));
        let url = format!("https://api.example.test:{port}/echo");
        let headers = vec![("Accept".to_string(), "application/json".to_string()), ("authorization".to_string(), "Bearer guest".to_string())];
        let mut req = Request::from_parts("post", url, &headers, b"body".to_vec()).unwrap();
        assert!(req.has_header("authorization"));
        // The app's value replaces the caller's.
        req.set_header("authorization", "Bearer app").unwrap();
        assert!(!format!("{req:?}").contains("Bearer app"), "the token stays out of Debug output");
        assert_eq!(text(&c, req).await.unwrap().1, "POST auth=Bearer app body");
        assert!(matches!(Request::from_parts("G ET", "https://x.test/", &[], Vec::new()), Err(Error::Invalid(_))));
        let bad = vec![("x-a".to_string(), "line\nbreak".to_string())];
        assert!(matches!(Request::from_parts("GET", "https://x.test/", &bad, Vec::new()), Err(Error::Invalid(_))));
    });
}
