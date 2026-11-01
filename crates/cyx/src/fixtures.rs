//! Fixtures for `cyx test` (docs/extensions/testing.md): the answers to `net.fetch` and `auth.fetch`, and the workspace
//! files the extension reads, so a provider or command that fetches or reads runs its success path in the real extension
//! host. Calls are checked first, as the app checks them (the manifest's `net` and `read`, the account's hosts); only an
//! allowed call gets its fixture.
//!
//! ```json
//! { "http": [
//!     { "method": "GET", "url": "https://api.example.com/items?page=1", "status": 200, "body": [{ "id": 1 }] },
//!     { "method": "*", "url": "https://api.example.com/fail", "status": 503, "body": "down", "headers": { "retry-after": "60" } }
//!   ],
//!   "files": { "vercel.json": "{}", ".git/config": "[remote \"origin\"]\n\turl = https://github.com/you/app.git\n" },
//!   "commands": { "collect": "TODO: write the docs" } }
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
    /// Workspace files: the path relative to the folder, and the text. The first one is also the sample document the
    /// providers are asked about.
    pub files: Vec<(String, String)>,
    /// The arguments `cyx test` runs a command with (its local id → JSON); a command not listed gets none.
    pub commands: Vec<(String, String)>,
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
        let mut f = Fixtures::parse(&text)
            .map_err(|e| format!("error: {}: {e}\n   = fix: see \"Fixtures for cyx test\" in docs/extensions/testing.md", path.display()))?;
        f.source = Some(path);
        Ok(f)
    }

    /// The JSON format above. A `body` that is a string is sent as it is; any other JSON value is sent as JSON, with
    /// `content-type: application/json` unless the fixture sets one.
    pub fn parse(text: &str) -> Result<Fixtures, String> {
        let raw: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let top = raw.as_object().ok_or("the file must be a JSON object: { \"http\": [ … ] }")?;
        if let Some(k) = top.keys().find(|k| !["http", "files", "commands"].contains(&k.as_str())) {
            return Err(format!("unknown field '{k}' (expected http, files, commands)"));
        }
        let commands = match top.get("commands") {
            None => Vec::new(),
            Some(serde_json::Value::Object(map)) => map.iter().map(|(id, args)| (id.clone(), args.to_string())).collect(),
            Some(_) => return Err("commands must be an object: { \"command-id\": its arguments as JSON }".into()),
        };
        let mut files = Vec::new();
        match top.get("files") {
            None => {}
            Some(serde_json::Value::Object(map)) => {
                for (path, text) in map {
                    let text = text.as_str().ok_or_else(|| format!("files: \"{path}\" must be the file's text, a string"))?;
                    // The same rule as a `read` glob without wildcards: relative, '/'-separated, no '.' or '..' parts.
                    if path.contains(['*', '?']) {
                        return Err(format!("files: \"{path}\" is a path, not a glob"));
                    }
                    ext_manifest::glob::check(path).map_err(|e| format!("files: \"{path}\" {e}"))?;
                    files.push((path.trim_start_matches("./").to_string(), text.to_string()));
                }
            }
            Some(_) => return Err("files must be an object: { \"path/in/the/folder\": \"its text\" }".into()),
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
            // Codeyard refuses such a request before it is sent, so its fixture could never answer.
            if let Err(e) = ext_manifest::calls::check_url(&url) {
                return Err(format!("http[{i}]: Codeyard never fetches it: {}", e.message));
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
        Ok(Fixtures { http, files, commands, source: None })
    }

    /// The text of the workspace file at `rel` (relative to the folder).
    pub fn file(&self, rel: &str) -> Option<&str> {
        self.files.iter().find(|(p, _)| p == rel).map(|(_, t)| t.as_str())
    }

    /// The arguments for command `id`, as JSON (empty: none).
    pub fn command_args(&self, id: &str) -> String {
        self.commands.iter().find(|(c, _)| c == id).map(|(_, a)| a.clone()).unwrap_or_default()
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
    fn files_are_text_at_paths_in_the_folder() {
        let f = Fixtures::parse(r#"{ "files": { ".git/config": "[core]", "./src/main.rs": "fn main() {}" } }"#).unwrap();
        assert_eq!(f.file(".git/config"), Some("[core]"));
        assert_eq!(f.file("src/main.rs"), Some("fn main() {}"));
        assert!(Fixtures::parse(r#"{ "files": { "../etc/passwd": "x" } }"#).unwrap_err().contains(".."));
        assert!(Fixtures::parse(r#"{ "files": { "/etc/passwd": "x" } }"#).is_err());
        assert!(Fixtures::parse(r#"{ "files": { "src/*.rs": "x" } }"#).unwrap_err().contains("not a glob"));
        assert!(Fixtures::parse(r#"{ "files": { "a.bin": 1 } }"#).unwrap_err().contains("a string"));
        assert!(Fixtures::parse(r#"{ "files": [] }"#).is_err());
        let f = Fixtures::parse(r#"{ "commands": { "collect": "TODO: docs", "open": { "line": 3 } } }"#).unwrap();
        assert_eq!(f.command_args("collect"), r#""TODO: docs""#);
        assert_eq!(f.command_args("open"), r#"{"line":3}"#);
        assert_eq!(f.command_args("other"), "");
    }

    #[test]
    fn refuses_what_codeyard_would_never_fetch() {
        assert!(Fixtures::parse(r#"{ "http": [{ "url": "http://a.example.com/" }] }"#).unwrap_err().contains("only https://"));
        assert!(Fixtures::parse(r#"{ "http": [{ "url": "https://me:pw@a.example.com/" }] }"#).unwrap_err().contains("user name or password"));
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
