//! The network boundary of dependency lookups: one trait, the system `curl` behind it (as the updater does), and a fake in
//! the tests. No shell, no `~/.curlrc`, strict timeouts, a size cap, no redirects, HTTPS only.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// What a lookup sends in the `User-Agent` header: the app and its version, nothing about the machine or the person.
pub fn user_agent() -> String {
    // crates.io asks clients to say who they are; the project's repository is the contact, not a person.
    format!("codeyard/{} ({})", env!("CARGO_PKG_VERSION"), env!("CARGO_PKG_REPOSITORY"))
}

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(12);
/// An index file larger than this is refused (the biggest, `windows-sys`, is a few megabytes).
pub const MAX_BODY: u64 = 16 * 1024 * 1024;
const HEADER_ALLOWANCE: u64 = 64 * 1024;
const TOO_LARGE: &str = "the answer was too large";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub etag: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// No route, DNS failure, refused or dropped connection: the machine is offline or the host is down.
    Offline,
    Timeout,
    Other(String),
}

pub trait Fetch: Send + Sync {
    /// `GET url`; `etag` makes it conditional (`If-None-Match`), and a `304` comes back with an empty body.
    fn get(&self, url: &str, etag: Option<&str>) -> Result<Response, FetchError>;
}

pub struct CurlFetch {
    curl: PathBuf,
    /// Tests talk to a server on 127.0.0.1; the app never does.
    allow_http: bool,
    /// The most bytes of answer read; the child is killed past it, so a server that never stops cannot hold a lookup until the
    /// total timeout.
    max_body: u64,
}

impl CurlFetch {
    pub fn new() -> Self {
        Self { curl: crate::update::system_tool("curl"), allow_http: false, max_body: MAX_BODY }
    }

    #[cfg(test)]
    pub fn for_test() -> Self {
        Self { allow_http: true, max_body: 256 * 1024, ..Self::new() }
    }

    fn command(&self, url: &str, etag: Option<&str>) -> Command {
        let mut c = Command::new(&self.curl);
        // `-q` first: ignore `~/.curlrc`. `--include` puts the status line and headers before the body.
        c.args(["-q", "--silent", "--show-error", "--include", "--compressed", "--max-redirs", "0", "--tlsv1.2"]);
        c.args(["--connect-timeout", &CONNECT_TIMEOUT.as_secs().to_string(), "--max-time", &TOTAL_TIMEOUT.as_secs().to_string()]);
        c.args(["--max-filesize", &self.max_body.to_string(), "--proto", if self.allow_http { "=https,http" } else { "=https" }]);
        c.args(["--user-agent", &user_agent(), "--header", "Accept: application/json, text/plain"]);
        if let Some(tag) = etag.filter(|t| t.bytes().all(|b| (0x20..0x7f).contains(&b))) {
            c.args(["--header", &format!("If-None-Match: {tag}")]);
        }
        c.arg("--").arg(url);
        c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        c
    }
}

impl Fetch for CurlFetch {
    fn get(&self, url: &str, etag: Option<&str>) -> Result<Response, FetchError> {
        let mut child = self.command(url, etag).spawn().map_err(|e| FetchError::Other(format!("curl: {e}")))?;
        // Both pipes are drained while the child runs, so a full pipe never stalls it.
        let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(e) = err.as_mut() {
                e.take(4096).read_to_string(&mut text).ok();
            }
            text
        });
        let mut raw = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        while let Some(Ok(n)) = out.as_mut().map(|o| o.read(&mut chunk)) {
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..n]);
            // Headers come first, so a little more than the body's cap is allowed before it counts as too large.
            if raw.len() as u64 > self.max_body + HEADER_ALLOWANCE {
                child.kill().ok();
                child.wait().ok();
                errors.join().ok();
                // Not `Offline`: the server is there, it sends too much, and that must not pause every other lookup.
                return Err(FetchError::Other(TOO_LARGE.into()));
            }
        }
        let status = child.wait().map_err(|e| FetchError::Other(format!("curl: {e}")))?;
        let stderr = errors.join().unwrap_or_default();
        if !status.success() {
            return Err(curl_error(status.code(), &stderr));
        }
        parse_response(&raw).ok_or_else(|| FetchError::Other("the answer was not an HTTP response".into()))
    }
}

/// What a curl exit status means for a lookup.
fn curl_error(code: Option<i32>, stderr: &str) -> FetchError {
    match code {
        // 6 could not resolve host, 7 could not connect, 52 empty reply, 56 receive error.
        Some(6 | 7 | 52 | 56) => FetchError::Offline,
        Some(28) => FetchError::Timeout,
        Some(63) => FetchError::Other(TOO_LARGE.into()),
        _ => FetchError::Other(
            stderr.lines().next().map(str::trim).filter(|l| !l.is_empty()).map_or_else(|| format!("curl exited with {code:?}"), str::to_string),
        ),
    }
}

/// Split `--include` output into the status, the `ETag` and the body. Interim blocks (`100 Continue`, a proxy's
/// `200 Connection established`) come first; the last header block is the answer.
fn parse_response(raw: &[u8]) -> Option<Response> {
    let mut rest = raw;
    loop {
        if !rest.starts_with(b"HTTP/") {
            return None;
        }
        let end = find(rest, b"\r\n\r\n").map(|i| (i, 4)).or_else(|| find(rest, b"\n\n").map(|i| (i, 2)))?;
        let head = String::from_utf8_lossy(&rest[..end.0]).into_owned();
        rest = &rest[end.0 + end.1..];
        let status: u16 = head.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
        if (100..200).contains(&status) || (status == 200 && head.contains("Connection established") && rest.starts_with(b"HTTP/")) {
            continue;
        }
        let etag = head.lines().skip(1).find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.trim().eq_ignore_ascii_case("etag").then(|| value.trim().to_string())
        });
        return Some(Response { status, etag, body: rest.to_vec() });
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_etag_and_body() {
        let raw = b"HTTP/2 200 \r\netag: \"abc\"\r\ncache-control: public\r\n\r\n{\"a\":1}\n";
        let r = parse_response(raw).unwrap();
        assert_eq!((r.status, r.etag.as_deref(), r.body.as_slice()), (200, Some("\"abc\""), &b"{\"a\":1}\n"[..]));
    }

    #[test]
    fn skips_interim_blocks_and_keeps_an_empty_304() {
        let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 304 Not Modified\r\nETag: W/\"x\"\r\n\r\n";
        let r = parse_response(raw).unwrap();
        assert_eq!((r.status, r.etag.as_deref(), r.body.len()), (304, Some("W/\"x\""), 0));
    }

    #[test]
    fn refuses_text_that_is_not_http() {
        assert_eq!(parse_response(b"<html>"), None);
    }

    #[test]
    fn maps_curl_exit_codes() {
        assert_eq!(curl_error(Some(6), "Could not resolve host"), FetchError::Offline);
        assert_eq!(curl_error(Some(7), ""), FetchError::Offline);
        assert_eq!(curl_error(Some(28), ""), FetchError::Timeout);
        assert_eq!(curl_error(Some(60), "curl: (60) SSL certificate problem\nmore"), FetchError::Other("curl: (60) SSL certificate problem".into()));
    }

    #[test]
    fn the_user_agent_names_only_the_app() {
        let ua = user_agent();
        assert!(ua.starts_with("codeyard/") && !ua.contains('@'), "{ua}");
    }
}
