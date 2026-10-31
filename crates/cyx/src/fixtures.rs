//! HTTP fixtures for `cyx test` (docs/extensions/testing.md): the answers to `net.fetch` and `auth.fetch`, so a provider
//! or command that fetches runs its success path in the real extension host. Requests are checked first, as the app
//! checks them (the manifest's `net`, the account's hosts); only an allowed request gets its fixture.
//!
//! ```json
//! { "http": [
//!     { "method": "GET", "url": "https://api.example.com/items?page=1", "status": 200, "body": [{ "id": 1 }] },
//!     { "method": "*", "url": "https://api.example.com/fail", "status": 503, "body": "down", "headers": { "retry-after": "60" } }
//! ] }
//! ```

use std::path::{Path, PathBuf};

/// Where `cyx test` looks when `--fixtures` is not given.
pub const DEFAULT: &str = "test/fixtures.json";

/// One answer: `method` (`*` for any) and the full `url` must match the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Http {
    pub method: String,
    pub url: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fixtures {
    pub http: Vec<Http>,
    /// The file they came from, for messages.
    pub source: Option<PathBuf>,
}

impl Fixtures {
    /// `explicit` (it must exist), else `test/fixtures.json` in `dir` when there is one, else none.
    pub fn load(dir: &Path, explicit: Option<&Path>) -> Result<Fixtures, String> {
        let path = match explicit {
            Some(p) => p.to_path_buf(),
            None => match dir.join(DEFAULT) {
                p if p.is_file() => p,
                _ => return Ok(Fixtures::default()),
            },
        };
        let text = std::fs::read_to_string(&path).map_err(|e| format!("error: cannot read {}: {e}", path.display()))?;
        let mut f =
            Fixtures::parse(&text).map_err(|e| format!("error: {}: {e}\n   = fix: see \"HTTP fixtures\" in docs/extensions/testing.md", path.display()))?;
        f.source = Some(path);
        Ok(f)
    }

    /// The JSON format above. A `body` that is a string is sent as it is; any other JSON value is sent as JSON, with
    /// `content-type: application/json` unless the fixture sets one.
    pub fn parse(text: &str) -> Result<Fixtures, String> {
        let raw: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let top = raw.as_object().ok_or("the file must be a JSON object: { \"http\": [ … ] }")?;
        if let Some(k) = top.keys().find(|k| *k != "http") {
            return Err(format!("unknown field '{k}' (expected http)"));
        }
        let list = match top.get("http") {
            None => &Vec::new(),
            Some(serde_json::Value::Array(a)) => a,
            Some(_) => return Err("http must be a list".into()),
        };
        let mut http = Vec::new();
        for (i, r) in list.iter().enumerate() {
            let r = r.as_object().ok_or_else(|| format!("http[{i}] must be an object"))?;
            if let Some(k) = r.keys().find(|k| !["method", "url", "status", "headers", "body"].contains(&k.as_str())) {
                return Err(format!("http[{i}]: unknown field '{k}' (expected method, url, status, headers, body)"));
            }
            let text = |k: &str| r.get(k).map(|v| v.as_str().map(str::to_string).ok_or_else(|| format!("http[{i}]: {k} must be a string"))).transpose();
            let method = text("method")?.unwrap_or_else(|| "*".into());
            let url = text("url")?.ok_or_else(|| format!("http[{i}] has no url"))?;
            if !url.starts_with("https://") {
                return Err(format!("http[{i}]: url '{url}' must start with https:// (Codeyard fetches nothing else)"));
            }
            let status = match r.get("status") {
                None => 200,
                Some(v) => v.as_u64().filter(|s| (100..=599).contains(s)).ok_or_else(|| format!("http[{i}]: status {v} is not an HTTP status"))? as u16,
            };
            let mut headers = Vec::new();
            if let Some(h) = r.get("headers") {
                let h = h.as_object().ok_or_else(|| format!("http[{i}]: headers must be an object of strings"))?;
                for (k, v) in h {
                    let v = v.as_str().ok_or_else(|| format!("http[{i}]: header {k} must be a string"))?;
                    headers.push((k.to_ascii_lowercase(), v.to_string()));
                }
            }
            let body = match r.get("body") {
                None | Some(serde_json::Value::Null) => Vec::new(),
                Some(serde_json::Value::String(s)) => s.clone().into_bytes(),
                Some(v) => {
                    if !headers.iter().any(|(k, _)| k == "content-type") {
                        headers.push(("content-type".into(), "application/json; charset=utf-8".into()));
                    }
                    v.to_string().into_bytes()
                }
            };
            http.push(Http { method, url, status, headers, body });
        }
        Ok(Fixtures { http, source: None })
    }

    /// The fixture for a request.
    pub fn find(&self, method: &str, url: &str) -> Option<&Http> {
        self.http.iter().find(|f| (f.method == "*" || f.method.eq_ignore_ascii_case(method)) && f.url == url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bodies_as_text_or_json() {
        let f = Fixtures::parse(
            r#"{ "http": [
                { "method": "GET", "url": "https://api.example.com/a", "body": [1, 2] },
                { "url": "https://api.example.com/b", "status": 404, "body": "gone", "headers": { "X-Why": "moved" } }
            ] }"#,
        )
        .unwrap();
        let a = f.find("get", "https://api.example.com/a").unwrap();
        assert_eq!((a.status, a.body.as_slice()), (200, b"[1,2]".as_slice()));
        assert_eq!(a.headers, [("content-type".to_string(), "application/json; charset=utf-8".to_string())]);
        let b = f.find("POST", "https://api.example.com/b").unwrap();
        assert_eq!((b.status, b.body.as_slice()), (404, b"gone".as_slice()));
        assert_eq!(b.headers, [("x-why".to_string(), "moved".to_string())]);
        assert!(f.find("GET", "https://api.example.com/a?x").is_none(), "the URL matches in full");
        assert!(f.find("POST", "https://api.example.com/a").is_none());
    }

    #[test]
    fn refuses_what_codeyard_would_never_fetch() {
        assert!(Fixtures::parse(r#"{ "http": [{ "url": "http://a.example.com/" }] }"#).unwrap_err().contains("https://"));
        assert!(Fixtures::parse(r#"{ "http": [{ "url": "https://a.example.com/", "status": 42 }] }"#).unwrap_err().contains("status"));
        assert!(Fixtures::parse(r#"{ "htp": [] }"#).is_err(), "unknown fields are typos");
    }

    #[test]
    fn loads_the_default_file_when_there_is_one() {
        let dir = std::env::temp_dir().join(format!("cyx-fixtures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("test")).unwrap();
        assert_eq!(Fixtures::load(&dir, None).unwrap(), Fixtures::default());
        std::fs::write(dir.join(DEFAULT), r#"{ "http": [{ "url": "https://a.example.com/" }] }"#).unwrap();
        assert_eq!(Fixtures::load(&dir, None).unwrap().http.len(), 1);
        assert!(Fixtures::load(&dir, Some(&dir.join("missing.json"))).unwrap_err().contains("cannot read"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
