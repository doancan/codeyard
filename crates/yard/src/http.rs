//! The HTTP side: a thin layer over [`Yard`] on hyper. Nothing is logged about who asks.
//!
//! ```text
//! POST /v1/publish                 framed bundle + package          → 200 Published (JSON) | 4xx text
//! POST /v1/status                  framed bundle + status statement → 200 {id, version, status}
//! GET  /packages/<digest>.cyx      the package; counts one download for today
//! GET  /packages/<digest>.sigstore.json   its publisher signature, which clients verify themselves
//! GET  /packages/<digest>.details.json     its manifest and README, for the Market's page (not a download)
//! GET  /index.json, /index.json.sig, /root.json, /root.json.sig    the static files, for a registry without a CDN
//! GET  /healthz
//! GET  /metrics                   index version and expiry, online key expiry, counts (Prometheus text; local only:
//!                                  404 to a request that came through a proxy)
//! GET  anything else               a file of the site folder (`site` in yard.json), else 404
//! ```

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use sha2::Digest as _;

use crate::{Error, MAX_UPLOAD_BYTES, Yard};

/// How often the index is signed again with nothing changed; it expires after 72 h.
pub const RESIGN_EVERY: Duration = Duration::from_secs(24 * 3600);
/// Uploads held in memory at once (each up to 129 MiB, a per-platform package); more wait their turn.
const UPLOADS_AT_ONCE: usize = 4;
/// A slow client cannot hold a connection: headers within 30 s, an upload within 5 minutes.
const HEADER_TIMEOUT: Duration = Duration::from_secs(30);
const BODY_TIMEOUT: Duration = Duration::from_secs(300);

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Serve until the process ends: sign a fresh index now and every day, and answer requests.
pub async fn serve(yard: Arc<Yard>, listener: tokio::net::TcpListener) -> std::io::Result<()> {
    let y = yard.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(RESIGN_EVERY);
        loop {
            tick.tick().await;
            let y = y.clone();
            let left = y.online_expires().saturating_sub(now());
            if left < crate::ROTATE_WARNING_SECS {
                eprintln!("yard: warning: the online key expires in {} days; rotate it now (docs/registry.md, Rotate the online key)", left / 86400);
            }
            match tokio::task::spawn_blocking(move || y.resign(now())).await {
                Ok(Ok(v)) => eprintln!("yard: signed index version {v}"),
                Ok(Err(e)) => eprintln!("yard: cannot sign the index: {e}"),
                Err(e) => eprintln!("yard: cannot sign the index: {e}"),
            }
        }
    });
    let uploads = Arc::new(tokio::sync::Semaphore::new(UPLOADS_AT_ONCE));
    loop {
        let (stream, _) = listener.accept().await?;
        let (yard, uploads) = (yard.clone(), uploads.clone());
        tokio::spawn(async move {
            let service = service_fn(move |req| handle(yard.clone(), uploads.clone(), req));
            let _ = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_TIMEOUT)
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

async fn handle(yard: Arc<Yard>, uploads: Arc<tokio::sync::Semaphore>, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let path = req.uri().path().to_string();
    let response = match (req.method().clone(), path.as_str()) {
        (Method::GET, "/healthz") => text(StatusCode::OK, "ok"),
        // For the server's own monitoring; the proxy keeps it from the internet (deploy/yard/Caddyfile).
        // A proxy in front (Nginx Proxy Manager, Caddy, cloudflared) always says whom it forwards for; monitoring data
        // is only for the server's own services, which ask directly.
        (Method::GET, "/metrics") if forwarded(&req) => text(StatusCode::NOT_FOUND, "not found"),
        (Method::GET, "/metrics") => match tokio::task::spawn_blocking({
            let yard = yard.clone();
            move || yard.health()
        })
        .await
        {
            Ok(Ok(h)) => file(h.metrics().into_bytes(), "text/plain; version=0.0.4; charset=utf-8", "no-store"),
            Ok(Err(e)) => error(e),
            Err(e) => error(Error::Internal(e.to_string())),
        },
        (Method::POST, "/v1/publish") => match body(req, &uploads).await {
            Ok((b, _turn)) => blocking(yard, move |y| y.publish(&b, now()).map(|p| serde_json::to_vec(&p).expect("serializes"))).await,
            Err(e) => error(e),
        },
        (Method::POST, "/v1/status") => match body(req, &uploads).await {
            Ok((b, _turn)) => {
                blocking(yard, move |y| {
                    y.change_status(&b, now()).map(|(id, version, status)| {
                        serde_json::to_vec(&serde_json::json!({ "id": id, "version": version, "status": status })).expect("serializes")
                    })
                })
                .await
            }
            Err(e) => error(e),
        },
        (Method::GET, p) if p.starts_with("/packages/") && p.ends_with(".sigstore.json") => {
            let digest = p["/packages/".len()..p.len() - ".sigstore.json".len()].to_string();
            match yard.bundle(&digest).and_then(|f| std::fs::read(f).map_err(|_| Error::NotFound("no such package".into()))) {
                Ok(bytes) => file(bytes, "application/json", "public, max-age=31536000, immutable"),
                Err(e) => error(e),
            }
        }
        (Method::GET, p) if p.starts_with("/packages/") && p.ends_with(".details.json") => {
            let digest = p["/packages/".len()..p.len() - ".details.json".len()].to_string();
            match yard.details(&digest).and_then(|f| std::fs::read(f).map_err(|_| Error::NotFound("no such package".into()))) {
                Ok(bytes) => file(bytes, "application/json", "public, max-age=31536000, immutable"),
                Err(e) => error(e),
            }
        }
        (Method::GET, p) if p.starts_with("/packages/") && p.ends_with(".cyx") => {
            let digest = p["/packages/".len()..p.len() - ".cyx".len()].to_string();
            match tokio::task::spawn_blocking({
                let yard = yard.clone();
                move || yard.download(&digest, now()).and_then(|file| std::fs::read(&file).map_err(|_| Error::NotFound("no such package".into())))
            })
            .await
            {
                // Never kept by a shared cache (a CDN): each download reaches this service and is counted. The
                // client may keep it.
                Ok(Ok(bytes)) => file(bytes, "application/gzip", "private, max-age=31536000, immutable"),
                Ok(Err(e)) => error(e),
                Err(e) => error(Error::Internal(e.to_string())),
            }
        }
        (Method::GET, "/index.json" | "/index.json.sig" | "/root.json" | "/root.json.sig") => {
            let asked = req.headers().get(hyper::header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).map(str::to_string);
            match tokio::fs::read(yard.config().public.join(&path[1..])).await {
                // Clients ask daily with the ETag they have; most days the answer is 304 and nothing is sent. No cache
                // may answer without asking here: the index and its signature must come from the same moment, and a
                // revocation must reach clients on their next fetch.
                Ok(bytes) => {
                    let tag = format!("\"{}\"", crate::hex(&sha2::Sha256::digest(&bytes)));
                    let cache = "no-cache";
                    if asked.as_deref() == Some(tag.as_str()) {
                        Response::builder()
                            .status(StatusCode::NOT_MODIFIED)
                            .header("etag", &tag)
                            .header("cache-control", cache)
                            .body(Full::new(Bytes::new()))
                            .expect("valid response")
                    } else {
                        let mut r = file(bytes, "application/json", cache);
                        r.headers_mut().insert("etag", tag.parse().expect("a valid header"));
                        r
                    }
                }
                Err(_) => text(StatusCode::NOT_FOUND, "not found"),
            }
        }
        (Method::GET | Method::HEAD, p) if yard.config().site.is_some() && !p.starts_with("/v1/") && !p.starts_with("/packages/") => {
            let site = yard.config().site.clone().expect("checked");
            let page = site_file(&site, p).await;
            let mut r = match page {
                Some((bytes, kind, cache)) => file(bytes, kind, cache),
                None => text(StatusCode::NOT_FOUND, "not found"),
            };
            site_headers(&mut r);
            if req.method() == Method::HEAD {
                *r.body_mut() = Full::new(Bytes::new());
            }
            r
        }
        (Method::GET | Method::POST, _) => text(StatusCode::NOT_FOUND, "not found"),
        _ => text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
    };
    Ok(response)
}

/// The request came through a proxy: it names the client it forwards for.
fn forwarded(req: &Request<Incoming>) -> bool {
    ["forwarded", "x-forwarded-for", "x-real-ip", "cf-connecting-ip"].iter().any(|h| req.headers().contains_key(*h))
}

/// A file of the site folder for a request path: the file itself when it exists, else `index.html` for a page path
/// (anything but a missing asset: the pages are drawn by the site's script). Only plain names are looked up, so nothing
/// outside the folder (or hidden in it) can be asked for.
async fn site_file(site: &std::path::Path, path: &str) -> Option<(Vec<u8>, &'static str, &'static str)> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let plain = |s: &&str| !s.starts_with('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-@".contains(&b));
    if !parts.iter().all(plain) {
        return None;
    }
    if !parts.is_empty() {
        let mut f = site.to_path_buf();
        f.extend(&parts);
        if let Ok(bytes) = tokio::fs::read(&f).await {
            let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
            return Some((bytes, content_type(ext), if ext == "html" { "no-cache" } else { "public, max-age=3600" }));
        }
    }
    // A missing asset stays missing; anything else (`/extensions/you.night`: ids have dots) is a page.
    let asset = parts.last().and_then(|last| last.rsplit_once('.')).is_some_and(|(_, ext)| content_type(ext) != "application/octet-stream");
    if asset {
        return None;
    }
    tokio::fs::read(site.join("index.html")).await.ok().map(|bytes| (bytes, "text/html; charset=utf-8", "no-cache"))
}

fn content_type(ext: &str) -> &'static str {
    match ext {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// The site runs only its own files and reads only this address; publishers' text is never markup (site/app.js).
fn site_headers(r: &mut Response<Full<Bytes>>) {
    let h = r.headers_mut();
    for (k, v) in [
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "strict-origin-when-cross-origin"),
        ("x-frame-options", "DENY"),
    ] {
        h.insert(k, hyper::header::HeaderValue::from_static(v));
    }
}

/// Read an upload, holding one of the upload slots until the caller drops it.
async fn body(req: Request<Incoming>, uploads: &tokio::sync::Semaphore) -> Result<(Vec<u8>, tokio::sync::SemaphorePermit<'_>), Error> {
    if req.headers().get(hyper::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse::<usize>().ok()).is_some_and(|n| n > MAX_UPLOAD_BYTES) {
        return Err(Error::TooLarge(format!("the upload is larger than {} MiB", MAX_UPLOAD_BYTES >> 20)));
    }
    let turn = uploads.acquire().await.map_err(|e| Error::Internal(e.to_string()))?;
    match tokio::time::timeout(BODY_TIMEOUT, Limited::new(req.into_body(), MAX_UPLOAD_BYTES).collect()).await {
        Err(_) => Err(Error::Invalid("the upload took longer than 5 minutes".into())),
        Ok(Ok(b)) => Ok((b.to_bytes().to_vec(), turn)),
        Ok(Err(e)) if e.is::<http_body_util::LengthLimitError>() => Err(Error::TooLarge(format!("the upload is larger than {} MiB", MAX_UPLOAD_BYTES >> 20))),
        Ok(Err(e)) => Err(Error::Invalid(format!("cannot read the upload: {e}"))),
    }
}

/// Run a database-touching call off the async threads.
async fn blocking(yard: Arc<Yard>, f: impl FnOnce(&Yard) -> Result<Vec<u8>, Error> + Send + 'static) -> Response<Full<Bytes>> {
    match tokio::task::spawn_blocking(move || f(&yard)).await {
        Ok(Ok(json)) => file(json, "application/json", "no-store"),
        Ok(Err(e)) => error(e),
        Err(e) => error(Error::Internal(e.to_string())),
    }
}

fn error(e: Error) -> Response<Full<Bytes>> {
    if let Error::Internal(m) = &e {
        eprintln!("yard: internal error: {m}");
    }
    text(StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), e.public_message())
}

fn text(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(format!("{body}\n"))))
        .expect("valid response")
}

fn file(bytes: Vec<u8>, content_type: &str, cache: &str) -> Response<Full<Bytes>> {
    Response::builder().header("content-type", content_type).header("cache-control", cache).body(Full::new(Bytes::from(bytes))).expect("valid response")
}
