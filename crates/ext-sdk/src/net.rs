//! HTTPS requests to the hosts in `net` (and, signed, to an `auth` provider's hosts). Requests leave from the machine
//! Codeyard runs on; the extension never opens a socket.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};
use crate::protocol::{Completion, StreamEvent};
use crate::runtime::Pending;
use crate::types::HttpRequest;

/// A request to send with [`Context::fetch`](crate::Context::fetch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request(pub(crate) HttpRequest);

impl Request {
    pub fn new(method: &str, url: impl Into<String>) -> Request {
        Request(HttpRequest { method: method.to_string(), url: url.into(), headers: Vec::new(), body: Vec::new(), timeout_ms: None })
    }

    pub fn get(url: impl Into<String>) -> Request {
        Request::new("GET", url)
    }

    pub fn post(url: impl Into<String>) -> Request {
        Request::new("POST", url)
    }

    pub fn header(mut self, name: &str, value: &str) -> Request {
        self.0.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Request {
        self.0.body = body.into();
        self
    }

    /// A JSON body, with its content type.
    pub fn json(self, value: &impl Serialize) -> Result<Request> {
        Ok(self.header("content-type", "application/json").body(serde_json::to_vec(value)?))
    }

    pub fn timeout_ms(mut self, ms: u32) -> Request {
        self.0.timeout_ms = Some(ms);
        self
    }

    pub fn url(&self) -> &str {
        &self.0.url
    }

    pub fn method(&self) -> &str {
        &self.0.method
    }

    /// The host part of the URL, for the `net = [...]` line a refusal suggests.
    pub(crate) fn host(&self) -> String {
        host_of(&self.0.url)
    }
}

pub(crate) fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}

/// A response, with its whole body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> Result<String> {
        Ok(String::from_utf8(self.body.clone())?)
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.body)?)
    }

    /// `Err` for a status outside 200–299, with the start of the body.
    pub fn error_for_status(self) -> Result<Response> {
        if self.ok() {
            return Ok(self);
        }
        let start: String = String::from_utf8_lossy(&self.body).chars().take(200).collect();
        Err(Error::Other(format!("HTTP {}: {start}", self.status)))
    }
}

/// Wait for a fetch ticket: the head, then the body as `data` events, then `done`.
pub(crate) async fn collect(ticket: u64, host: String, what: &str) -> Result<Response> {
    let mut pending = Pending::new(ticket);
    let mut response = Response { status: 0, headers: Vec::new(), body: Vec::new() };
    while let Some(e) = pending.next_event().await {
        match e {
            StreamEvent::HttpHead { status, headers } => {
                response.status = status;
                response.headers = headers;
            }
            StreamEvent::Data(d) => response.body.extend_from_slice(&d),
            _ => {}
        }
    }
    match pending.await {
        Completion::Done | Completion::Bytes(_) => Ok(response),
        Completion::Failed(e) => Err(e.detailed(&|| format!(r#"{what} = ["{host}"]"#))),
        c => Err(crate::context::unexpected(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::host_of;

    #[test]
    fn hosts() {
        assert_eq!(host_of("https://api.example.com/v1?x=1"), "api.example.com");
        assert_eq!(host_of("https://user@API.example.com:8443"), "api.example.com");
        assert_eq!(host_of("example.com/x"), "example.com");
    }
}
