//! HTTP(S) for `cyx publish`: one blocking request at a time, TLS through rustls with the system's roots. Plain
//! `http://` is allowed only for localhost (a local registry or the tests' stand-ins).

use std::time::Duration;

use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::Bytes;

const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_RESPONSE: usize = 16 << 20;

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).trim().to_string()
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

fn local(url: &str) -> bool {
    let rest = url.strip_prefix("http://").unwrap_or_default();
    let host = rest.split(['/', ':']).next().unwrap_or_default();
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

pub fn request(method: &str, url: &str, headers: &[(&str, &str)], body: Vec<u8>) -> Result<Response, String> {
    if !url.starts_with("https://") && !local(url) {
        return Err(format!("{url}: only https:// addresses are allowed (http:// only for localhost)"));
    }
    let mut req = hyper::Request::builder().method(method).uri(url).header("user-agent", concat!("cyx/", env!("CARGO_PKG_VERSION")));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = req.body(Full::new(Bytes::from(body))).map_err(|e| format!("{url}: {e}"))?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_native_roots(rustls::crypto::ring::default_provider())
            .map_err(|e| format!("cannot load the system's certificates: {e}"))?
            .https_or_http()
            .enable_http1()
            .build();
        let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new()).build(https);
        let send = async {
            let res = client.request(req).await.map_err(|e| format!("{url}: {e}"))?;
            let status = res.status().as_u16();
            let body = Limited::new(res.into_body(), MAX_RESPONSE).collect().await.map_err(|e| format!("{url}: {e}"))?.to_bytes().to_vec();
            Ok(Response { status, body })
        };
        tokio::time::timeout(TIMEOUT, send).await.map_err(|_| format!("{url}: no answer in {} s", TIMEOUT.as_secs()))?
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn plain_http_only_for_this_machine() {
        assert!(super::local("http://127.0.0.1:8080/v1/publish"));
        assert!(super::local("http://localhost/x"));
        assert!(!super::local("http://localhost.evil.test/x"));
        assert!(!super::local("http://yard.codeyard.dev/x"));
        assert!(super::request("GET", "http://example.com/", &[], Vec::new()).unwrap_err().contains("only https://"));
    }
}
