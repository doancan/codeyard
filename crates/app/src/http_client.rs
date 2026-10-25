//! `.http` / `.rest` files (VS Code REST Client and JetBrains format): parse requests, resolve variables, build the `curl`
//! arguments, decode the response. Independent of the UI; the `Host::exec("curl", …)` call that runs the request lives in Workbench.
//!
//! ```text
//! @host = https://api.example.com
//! ### login
//! # @name login
//! POST {{host}}/login
//! Content-Type: application/json
//!
//! {"user": "ada"}
//!
//! ###
//! GET {{host}}/me
//! Authorization: Bearer {{login.response.body.$.token}}
//! ```

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Text(String),
    /// `< ./file.json`: request body from a file.
    File(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub name: Option<String>,
    pub title: String,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Body>,
    /// 0-based number of the request line and the last line of the block.
    pub line: usize,
    pub end_line: usize,
    /// First line of the block (after the previous `###` separator).
    pub block_start: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub requests: Vec<Request>,
    /// `@name = value` definitions (anywhere in the file, in order; a later one with the same name overrides).
    pub vars: Vec<(String, String)>,
}

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT"];

pub fn is_http_file(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    ext == "http" || ext == "rest"
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    (t.starts_with('#') && !t.starts_with("###")) || t.starts_with("//")
}

fn parse_request_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let (first, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(a, b)| (a, b.trim()));
    let strip_version = |u: &str| -> String {
        match u.rsplit_once(char::is_whitespace) {
            Some((url, v)) if v.starts_with("HTTP/") => url.trim().to_string(),
            _ => u.trim().to_string(),
        }
    };
    if METHODS.contains(&first.to_ascii_uppercase().as_str()) && !rest.is_empty() {
        return Some((first.to_ascii_uppercase(), strip_version(rest)));
    }
    // Address only: counts as GET.
    (first.starts_with("http://") || first.starts_with("https://") || first.starts_with("{{")).then(|| ("GET".to_string(), strip_version(line)))
}

pub fn parse(text: &str) -> Parsed {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Parsed::default();
    let mut block_start = 0usize;
    let mut title = String::new();
    let mut i = 0;
    while i <= lines.len() {
        // End of the block: a separator line or end of file.
        let at_sep = i == lines.len() || lines[i].trim_start().starts_with("###");
        if at_sep {
            if let Some(req) = parse_block(&lines, block_start, i, &title, &mut out.vars) {
                out.requests.push(req);
            }
            title = lines.get(i).map(|l| l.trim_start().trim_start_matches('#').trim().to_string()).unwrap_or_default();
            block_start = i + 1;
        }
        i += 1;
    }
    out
}

fn parse_block(lines: &[&str], start: usize, end: usize, title: &str, vars: &mut Vec<(String, String)>) -> Option<Request> {
    let mut name = None;
    let mut ix = start;
    let mut request: Option<(String, String, usize)> = None;
    while ix < end {
        let raw = lines[ix];
        let t = raw.trim();
        if t.is_empty() {
            ix += 1;
            continue;
        }
        if is_comment(raw) {
            // `# @name name` / `// @name name`
            let body = t.trim_start_matches(['#', '/']).trim();
            if let Some(n) = body.strip_prefix("@name ") {
                name = Some(n.trim().to_string()).filter(|n| !n.is_empty());
            }
            ix += 1;
            continue;
        }
        // Variable definition: `@name = value`.
        if let Some(def) = t.strip_prefix('@')
            && let Some((k, v)) = def.split_once('=')
            && !k.trim().is_empty()
            && !k.trim().contains(char::is_whitespace)
        {
            vars.push((k.trim().to_string(), v.trim().to_string()));
            ix += 1;
            continue;
        }
        if let Some((m, u)) = parse_request_line(raw) {
            request = Some((m, u, ix));
        }
        break;
    }
    let (method, mut url, line) = request?;
    ix = line + 1;
    // Multi-line address: indented lines starting with `?a=1` / `&b=2` are appended.
    while ix < end && matches!(lines[ix].trim_start().chars().next(), Some('?' | '&')) {
        url.push_str(lines[ix].trim());
        ix += 1;
    }
    let mut headers = Vec::new();
    while ix < end {
        let raw = lines[ix];
        if raw.trim().is_empty() {
            ix += 1;
            break;
        }
        if !is_comment(raw)
            && let Some((k, v)) = raw.split_once(':')
            && !k.trim().is_empty()
            && !k.contains(char::is_whitespace)
        {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
        ix += 1;
    }
    // Body: the remaining lines (trailing blanks dropped); if it is entirely `< path`, from the file.
    let mut body_lines: Vec<&str> = lines[ix.min(end)..end].to_vec();
    while body_lines.last().is_some_and(|l| l.trim().is_empty()) {
        body_lines.pop();
    }
    while body_lines.first().is_some_and(|l| l.trim().is_empty()) {
        body_lines.remove(0);
    }
    let body = if body_lines.is_empty() {
        None
    } else if body_lines.len() == 1 && body_lines[0].trim_start().starts_with("< ") {
        Some(Body::File(body_lines[0].trim_start()[2..].trim().to_string()))
    } else {
        Some(Body::Text(body_lines.join("\n")))
    };
    Some(Request { name, title: title.to_string(), method, url, headers, body, line, end_line: end.saturating_sub(1).max(line), block_start: start })
}

/// The request the cursor (line) belongs to: if inside a block, that one; on a `### title` line, the request just below it;
/// on lines before the first request, the first request.
pub fn request_at(parsed: &Parsed, line: usize) -> Option<usize> {
    parsed
        .requests
        .iter()
        .position(|r| line + 1 >= r.block_start && line <= r.end_line)
        .or_else(|| parsed.requests.iter().rposition(|r| r.block_start <= line))
        .or(if parsed.requests.is_empty() { None } else { Some(0) })
}

// --- Variables ---

/// The response of a sent request (for `{{name.response.body.$.x}}` references).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stored {
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Resolution context: the sources of dynamic variables are passed as functions so they can be tested.
pub struct Context<'a> {
    pub vars: &'a [(String, String)],
    pub responses: &'a HashMap<String, Stored>,
    pub now: i64,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    pub uuid: &'a dyn Fn() -> String,
    pub random: &'a dyn Fn(i64, i64) -> i64,
}

fn json_path(body: &str, path: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|_| crate::i18n::t("the response body is not JSON").to_string())?;
    let mut cur = &v;
    let path = path.strip_prefix('$').unwrap_or(path);
    let mut rest = path;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(['.', '[']).unwrap_or(r.len());
            let key = &r[..end];
            cur = cur.get(key).ok_or_else(|| crate::i18n::tf("no “{}” in the response", &[&key]))?;
            rest = &r[end..];
        } else if let Some(r) = rest.strip_prefix('[') {
            let end = r.find(']').ok_or(crate::i18n::t("unclosed [ in the path"))?;
            let idx: usize = r[..end].parse().map_err(|_| crate::i18n::t("bad array index").to_string())?;
            cur = cur.get(idx).ok_or_else(|| crate::i18n::tf("no item {} in the response", &[&idx]))?;
            rest = &r[end + 1..];
        } else {
            return Err(crate::i18n::tf("cannot read “{}” in the path", &[&rest]));
        }
    }
    Ok(match cur {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// ISO-8601 UTC (`2026-10-24T12:00:00.000Z`) / RFC 1123 (`Sat, 24 Oct 2026 12:00:00 GMT`).
pub fn format_datetime(secs: i64, kind: &str) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, mi, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // Calendar from day count (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    if kind == "rfc1123" {
        const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("{}, {d:02} {} {y} {h:02}:{mi:02}:{s:02} GMT", DAYS[days.rem_euclid(7) as usize], MONTHS[(m - 1) as usize])
    } else {
        format!("{y}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.000Z")
    }
}

fn resolve_one(expr: &str, ctx: &Context, depth: usize) -> Result<String, String> {
    let expr = expr.trim();
    if let Some(dynamic) = expr.strip_prefix('$') {
        let mut parts = dynamic.split_whitespace();
        return match parts.next() {
            Some("guid") => Ok((ctx.uuid)()),
            Some("timestamp") => Ok(ctx.now.to_string()),
            Some("randomInt") => {
                let a: i64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                let b: i64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(1000);
                Ok((ctx.random)(a.min(b), a.max(b)).to_string())
            }
            Some("datetime") | Some("localDatetime") => Ok(format_datetime(ctx.now, if parts.next() == Some("rfc1123") { "rfc1123" } else { "iso8601" })),
            Some("processEnv") => {
                let name = parts.next().ok_or(crate::i18n::t("{{$processEnv}} needs a name"))?;
                (ctx.env)(name).ok_or_else(|| crate::i18n::tf("the environment variable {} is not set", &[&name]))
            }
            Some(other) => Err(crate::i18n::tf("unknown variable {}", &[&format!("{{{{${other}}}}}")])),
            None => Err(crate::i18n::t("empty variable").into()),
        };
    }
    // `name.response.body.$.path` / `name.response.headers.Header`
    if let Some((name, rest)) = expr.split_once(".response.") {
        let stored = ctx.responses.get(name).ok_or_else(|| crate::i18n::tf("send the request named “{}” first", &[&name]))?;
        if let Some(path) = rest.strip_prefix("body.") {
            return json_path(&stored.body, path);
        }
        if rest == "body" || rest == "body.*" {
            return Ok(stored.body.clone());
        }
        if let Some(h) = rest.strip_prefix("headers.") {
            return stored
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(h))
                .map(|(_, v)| v.clone())
                .ok_or_else(|| crate::i18n::tf("the response of “{}” has no header {}", &[&name, &h]));
        }
        return Err(crate::i18n::tf("cannot read “{}” of the response", &[&rest]));
    }
    match ctx.vars.iter().rev().find(|(k, _)| k == expr) {
        Some((_, v)) if depth < 6 => substitute(v, ctx, depth + 1),
        Some(_) => Err(crate::i18n::tf("the variable {} refers to itself", &[&expr])),
        None => Err(crate::i18n::tf("undefined variable {}", &[&expr])),
    }
}

/// Resolve the `{{…}}` expressions in the text.
pub fn substitute(text: &str, ctx: &Context, depth: usize) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        out.push_str(&resolve_one(&after[..end], ctx, depth)?);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

// --- Preparation and curl ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

fn header_ok(s: &str) -> bool {
    !s.chars().any(|c| c == '\r' || c == '\n' || c == '\0')
}

/// Resolve variables, validate (address is http/https, no line breaks in headers), read the body.
pub fn prepare(req: &Request, ctx: &Context, read_file: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Prepared, String> {
    let url = substitute(&req.url, ctx, 0)?;
    if !(url.starts_with("http://") || url.starts_with("https://")) || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(crate::i18n::tf("“{}” is not a valid http(s) address", &[&url]));
    }
    let mut headers = Vec::new();
    for (k, v) in &req.headers {
        let (k, mut v) = (substitute(k, ctx, 0)?, substitute(v, ctx, 0)?);
        if !header_ok(&k) || !header_ok(&v) || k.contains([' ', ':']) {
            return Err(crate::i18n::tf("the header “{}” is not valid", &[&k]));
        }
        // `Authorization: Basic user:password` → base64 (leave it alone if already encoded).
        if k.eq_ignore_ascii_case("authorization")
            && let Some(cred) = v.strip_prefix("Basic ")
            && cred.contains(':')
        {
            v = format!("Basic {}", crate::texttools::base64_encode(cred));
        }
        headers.push((k, v));
    }
    let body = match &req.body {
        None => None,
        Some(Body::Text(t)) => Some(substitute(t, ctx, 0)?.into_bytes()),
        Some(Body::File(p)) => Some(read_file(&substitute(p, ctx, 0)?)?),
    };
    Ok(Prepared { method: req.method.clone(), url, headers, body })
}

/// Separator appended to the end of the output with `-w`.
pub const MARKER: &str = "<<codeyard-http>>";

/// Larger responses aren't downloaded (if the content length is known curl cuts it off at the start).
pub const MAX_RESPONSE_BYTES: u64 = 20 * 1024 * 1024;

pub fn curl_args(p: &Prepared) -> Vec<String> {
    // No `-L`: a redirect would re-send the method, body and custom headers to a host nobody approved (loopback, link-local metadata).
    // The 3xx is shown with its Location instead and the user sends the next request deliberately.
    let mut a: Vec<String> = ["-sS", "-i", "--max-time", "60", "--compressed"].iter().map(|s| s.to_string()).collect();
    a.push("--max-filesize".into());
    a.push(MAX_RESPONSE_BYTES.to_string());
    if p.method == "HEAD" {
        a.push("-I".into());
    } else {
        a.push("-X".into());
        a.push(p.method.clone());
    }
    for (k, v) in &p.headers {
        a.push("-H".into());
        a.push(format!("{k}: {v}"));
    }
    if p.body.is_some() {
        a.push("--data-binary".into());
        a.push("@-".into());
    }
    a.push("-w".into());
    a.push(format!("\\n{MARKER}%{{http_code}} %{{time_total}} %{{size_download}}"));
    a.push("--".into());
    a.push(p.url.clone());
    a
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub millis: u64,
    pub size: u64,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// Text to show and (if any) a decoding note. If not UTF-8, the declared `charset` (Latin-1 / Turkish Latin-5) is used, or, for a text
    /// type, a conversion that turns broken bytes into “�”; binary content is `None` (a hex dump is shown).
    pub fn decoded(&self) -> Option<(String, Option<String>)> {
        if let Ok(t) = std::str::from_utf8(&self.body) {
            return Some((t.to_string(), None));
        }
        let ct = self.header("content-type").unwrap_or("").to_ascii_lowercase();
        let charset = ct.split(';').skip(1).find_map(|p| p.trim().strip_prefix("charset=")).map(|c| c.trim_matches('"').trim().to_string());
        match charset.as_deref() {
            Some("iso-8859-1" | "latin1" | "windows-1252" | "us-ascii") => {
                let t: String = self.body.iter().map(|&b| b as char).collect();
                return Some((t, Some(crate::i18n::tf("decoded as {}", &[&charset.unwrap_or_default()]))));
            }
            Some("iso-8859-9" | "windows-1254" | "latin5") => {
                let t: String = self
                    .body
                    .iter()
                    .map(|&b| match b {
                        0xD0 => 'Ğ',
                        0xDD => 'İ',
                        0xDE => 'Ş',
                        0xF0 => 'ğ',
                        0xFD => 'ı',
                        0xFE => 'ş',
                        b => b as char,
                    })
                    .collect();
                return Some((t, Some(crate::i18n::tf("decoded as {}", &[&charset.unwrap_or_default()]))));
            }
            _ => {}
        }
        let text_like = ["text/", "json", "xml", "javascript", "yaml", "html", "css", "markdown", "csv", "x-www-form-urlencoded"].iter().any(|k| ct.contains(k))
            // If no type is declared, treat it as text if there is no NUL in the first 8 KB.
            || (ct.is_empty() && !self.body.iter().take(8192).any(|&b| b == 0));
        text_like.then(|| (String::from_utf8_lossy(&self.body).into_owned(), Some("not valid UTF-8: unreadable bytes are shown as �".to_string())))
    }

    /// File extension (for syntax highlighting of the response tab).
    pub fn extension(&self) -> &'static str {
        let ct = self.header("content-type").unwrap_or("").to_ascii_lowercase();
        match () {
            _ if ct.contains("json") => "json",
            _ if ct.contains("html") => "html",
            _ if ct.contains("xml") => "xml",
            _ if ct.contains("css") => "css",
            _ if ct.contains("javascript") => "js",
            _ if ct.contains("yaml") => "yaml",
            _ if ct.contains("markdown") => "md",
            _ => "txt",
        }
    }
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

/// Parse `curl -i … -w` output: skips intermediate header blocks (`100 Continue`), takes the last one.
pub fn parse_output(stdout: &[u8]) -> Result<Response, String> {
    let marker_at = stdout.windows(MARKER.len()).rposition(|w| w == MARKER.as_bytes()).ok_or("curl returned no response")?;
    let meta = String::from_utf8_lossy(&stdout[marker_at + MARKER.len()..]).trim().to_string();
    let mut parts = meta.split_whitespace();
    let code: u16 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let secs: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.);
    let size: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    // The "\n" at the start of the `-w` format is not part of the body.
    let mut data = &stdout[..marker_at.saturating_sub(usize::from(marker_at > 0 && stdout[marker_at - 1] == b'\n'))];
    let mut headers_block: &[u8] = &[];
    while data.starts_with(b"HTTP/") {
        let (end, sep) = match (find(data, b"\r\n\r\n", 0), find(data, b"\n\n", 0)) {
            (Some(a), Some(b)) if b < a => (b, 2),
            (Some(a), _) => (a, 4),
            (None, Some(b)) => (b, 2),
            (None, None) => (data.len(), 0),
        };
        headers_block = &data[..end];
        data = &data[(end + sep).min(data.len())..];
        // If the next block is also headers, this was an intermediate block (100 Continue).
    }
    if code == 0 || headers_block.is_empty() {
        return Err(crate::i18n::t("curl returned no response headers").into());
    }
    let text = String::from_utf8_lossy(headers_block);
    let mut lines = text.lines();
    let status_line = lines.next().unwrap_or_default();
    let reason = status_line.splitn(3, ' ').nth(2).unwrap_or("").trim().to_string();
    let headers = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))).collect();
    Ok(Response { status: code, reason, headers, body: data.to_vec(), millis: (secs * 1000.).round() as u64, size })
}

/// Turns a curl failure into a single line saying what happened and what to do. `place`: the machine the request ran on.
pub fn explain_curl(code: Option<i32>, stderr: &str, place: &str, url: &str) -> String {
    let host = url.split_once("://").map_or(url, |(_, r)| r).split(['/', '?', '#']).next().unwrap_or(url).rsplit('@').next().unwrap_or(url).to_string();
    let local_hint = if host.starts_with("localhost") || host.starts_with("127.") || host.starts_with("[::1]") {
        format!(" {}", crate::i18n::tf("“localhost” means {0}: for a service on your own computer set http.runOn to {1}.", &[&place, &"\"local\""]))
    } else {
        String::new()
    };
    let detail = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    match code {
        Some(3) => crate::i18n::tf("The address is malformed: {}", &[&url]),
        Some(5 | 6) => crate::i18n::tf("Could not resolve “{}”: check the spelling and the network (the request ran on {}).", &[&host, &place]),
        Some(7) => format!(
            "{}{local_hint}",
            crate::i18n::tf("Could not connect to {}: nothing is listening there, or a firewall blocks it (the request ran on {}).", &[&host, &place])
        ),
        Some(28) => crate::i18n::t("The server did not answer within 60 seconds.").to_string(),
        Some(35 | 51 | 58 | 59 | 60 | 77 | 83 | 90) => {
            crate::i18n::tf("The TLS/SSL handshake with {} failed (certificate or protocol problem): {}", &[&host, &detail])
        }
        Some(52) => crate::i18n::tf("{} accepted the connection but sent no reply.", &[&host]),
        Some(55 | 56) => crate::i18n::tf("The connection to {} dropped while the data was being exchanged.", &[&host]),
        Some(63) => crate::i18n::tf("The response is larger than {} and was not downloaded.", &[&human_size(MAX_RESPONSE_BYTES)]),
        Some(23) => crate::i18n::t("The response could not be saved (curl write error).").to_string(),
        Some(c) if !detail.is_empty() => crate::i18n::tf("{} (curl exit {})", &[&detail, &c]),
        _ => crate::i18n::t("The request failed.").to_string(),
    }
}

/// Points that must be confirmed with the user before sending: reading environment variables and body files outside the request file's folder.
/// A `.http` file may come from someone else; it must not silently load `$HOME/.ssh/id_rsa`.
pub fn sensitive_parts(req: &Request) -> Vec<String> {
    let mut out = Vec::new();
    let mut texts: Vec<&str> = vec![&req.url];
    for (k, v) in &req.headers {
        texts.push(k);
        texts.push(v);
    }
    match &req.body {
        Some(Body::Text(t)) => texts.push(t),
        Some(Body::File(p)) => texts.push(p),
        None => {}
    }
    if texts.iter().any(|t| t.contains("$processEnv")) {
        out.push(crate::i18n::t("reads environment variables from this computer ({{$processEnv}})").to_string());
    }
    if let Some(Body::File(p)) = &req.body {
        if p.contains("{{") {
            out.push(crate::i18n::tf("uploads a file whose name comes from a variable ({})", &[&p]));
        } else if p.starts_with('/') || p.starts_with('~') || p.split(['/', '\\']).any(|c| c == "..") {
            out.push(crate::i18n::tf("uploads {}, which is outside this file's folder", &[&p]));
        }
    }
    out
}

/// The `Location` of a redirect answer; such a response is not followed automatically (see `curl_args`).
pub fn redirect_target(r: &Response) -> Option<&str> {
    (matches!(r.status, 301 | 302 | 303 | 307 | 308)).then(|| r.header("location")).flatten().filter(|l| !l.is_empty())
}

pub fn status_line(r: &Response) -> String {
    let reason = if r.reason.is_empty() { default_reason(r.status).to_string() } else { r.reason.clone() };
    format!("{} {}", r.status, reason).trim().to_string()
}

fn default_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

pub fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.)
    } else {
        format!("{:.1} MB", n as f64 / 1024. / 1024.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "# Notes\n@host = https://api.example.com\n@token = abc\n\n### Log in\n# @name login\nPOST {{host}}/login?x=1\n  &y=2\nContent-Type: application/json\n\n{\n  \"user\": \"ada\"\n}\n\n###\nGET {{host}}/me HTTP/1.1\nAuthorization: Bearer {{login.response.body.$.token}}\n// a comment\nX-Id: {{$guid}}\n\n### upload\nPUT https://x.test/f\nContent-Type: text/plain\n\n< ./body.txt\n\n### just a url\nhttps://plain.test/\n";

    #[test]
    fn parses_blocks_names_variables_headers_and_bodies() {
        let p = parse(FILE);
        assert_eq!(p.vars, [("host".into(), "https://api.example.com".into()), ("token".into(), "abc".into())]);
        assert_eq!(p.requests.len(), 4);
        let r = &p.requests[0];
        assert_eq!((r.method.as_str(), r.url.as_str(), r.name.as_deref(), r.title.as_str()), ("POST", "{{host}}/login?x=1&y=2", Some("login"), "Log in"));
        assert_eq!(r.headers, [("Content-Type".to_string(), "application/json".to_string())]);
        assert_eq!(r.body, Some(Body::Text("{\n  \"user\": \"ada\"\n}".into())));
        assert_eq!(r.line, 6);
        let me = &p.requests[1];
        assert_eq!((me.method.as_str(), me.url.as_str()), ("GET", "{{host}}/me"), "the HTTP version is dropped");
        assert_eq!(me.headers.len(), 2, "comment lines are not headers");
        assert!(me.body.is_none());
        assert_eq!(p.requests[2].body, Some(Body::File("./body.txt".into())));
        assert_eq!((p.requests[3].method.as_str(), p.requests[3].url.as_str()), ("GET", "https://plain.test/"));
    }

    #[test]
    fn the_request_under_the_cursor() {
        let p = parse(FILE);
        let l = |needle: &str| FILE.lines().position(|x| x.contains(needle)).unwrap();
        assert_eq!(request_at(&p, l("\"user\"")), Some(0), "inside the body");
        assert_eq!(request_at(&p, l("Authorization")), Some(1));
        assert_eq!(request_at(&p, l("### upload")), Some(2), "the title line belongs to the request below it");
        assert_eq!(request_at(&p, 0), Some(0), "before the first request: the first one");
        assert_eq!(parse("just text\nno requests\n").requests.len(), 0);
        assert_eq!(request_at(&parse(""), 0), None);
    }

    fn ctx<'a>(vars: &'a [(String, String)], responses: &'a HashMap<String, Stored>) -> Context<'a> {
        Context {
            vars,
            responses,
            now: 1_792_845_015,
            env: &|n| (n == "HOME_X").then(|| "/home/x".to_string()),
            uuid: &|| "00000000-0000-4000-8000-000000000000".into(),
            random: &|a, _| a,
        }
    }

    #[test]
    fn variables_dynamic_values_and_chained_responses() {
        let vars = vec![("host".to_string(), "https://h".to_string()), ("base".to_string(), "{{host}}/v1".to_string())];
        let mut responses = HashMap::new();
        responses
            .insert("login".to_string(), Stored { headers: vec![("X-Req".into(), "r1".into())], body: r#"{"token":"t0k","items":[{"id":7}],"n":3}"#.into() });
        let c = ctx(&vars, &responses);
        let sub = |t: &str| substitute(t, &c, 0);
        assert_eq!(sub("{{base}}/users").unwrap(), "https://h/v1/users", "variables can use variables");
        assert_eq!(sub("{{$guid}} {{$timestamp}} {{$randomInt 5 9}}").unwrap(), "00000000-0000-4000-8000-000000000000 1792845015 5");
        assert_eq!(sub("{{$datetime iso8601}}").unwrap(), "2026-10-24T12:30:15.000Z");
        assert_eq!(sub("{{$datetime rfc1123}}").unwrap(), "Sat, 24 Oct 2026 12:30:15 GMT");
        assert_eq!(sub("{{$processEnv HOME_X}}").unwrap(), "/home/x");
        assert_eq!(sub("{{login.response.body.$.token}}").unwrap(), "t0k");
        assert_eq!(sub("{{login.response.body.$.items[0].id}}").unwrap(), "7");
        assert_eq!(sub("{{login.response.body.$.n}}").unwrap(), "3");
        assert_eq!(sub("{{login.response.headers.x-req}}").unwrap(), "r1");
        assert_eq!(sub("plain {{ unclosed").unwrap(), "plain {{ unclosed");
        for (bad, why) in [
            ("{{nope}}", "undefined variable nope"),
            ("{{other.response.body.$.x}}", "send the request named “other” first"),
            ("{{login.response.body.$.missing}}", "no “missing”"),
            ("{{$processEnv NOPE}}", "not set"),
            ("{{$wat}}", "unknown variable"),
        ] {
            let e = sub(bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        let looping = vec![("a".to_string(), "{{a}}".to_string())];
        assert!(substitute("{{a}}", &ctx(&looping, &responses), 0).unwrap_err().contains("itself"));
    }

    #[test]
    fn preparing_validates_and_builds_curl_arguments_without_a_shell() {
        let p = parse(FILE);
        let vars = p.vars.clone();
        let mut responses = HashMap::new();
        responses.insert("login".to_string(), Stored { headers: vec![], body: r#"{"token":"T"}"#.into() });
        let c = ctx(&vars, &responses);
        let read = |path: &str| if path == "./body.txt" { Ok(b"file body".to_vec()) } else { Err(format!("no file {path}")) };
        let login = prepare(&p.requests[0], &c, &read).unwrap();
        assert_eq!(login.url, "https://api.example.com/login?x=1&y=2");
        assert_eq!(login.body.as_deref(), Some(&b"{\n  \"user\": \"ada\"\n}"[..]));
        let a = curl_args(&login);
        assert_eq!(&a[a.len() - 2..], ["--", "https://api.example.com/login?x=1&y=2"], "the address is last, after `--`");
        assert!(
            a.windows(2).any(|w| w == ["-X", "POST"])
                && a.windows(2).any(|w| w == ["-H", "Content-Type: application/json"])
                && a.windows(2).any(|w| w == ["--data-binary", "@-"])
        );
        let me = prepare(&p.requests[1], &c, &read).unwrap();
        assert!(me.headers.contains(&("Authorization".to_string(), "Bearer T".to_string())));
        assert!(!curl_args(&me).iter().any(|x| x == "--data-binary"), "no body, no data flag");
        assert_eq!(prepare(&p.requests[2], &c, &read).unwrap().body.as_deref(), Some(&b"file body"[..]));
        let head = Prepared { method: "HEAD".into(), url: "https://h/".into(), headers: vec![], body: None };
        assert!(curl_args(&head).iter().any(|x| x == "-I") && !curl_args(&head).iter().any(|x| x == "-X"));
        // Basic auth with plain credentials is encoded; invalid addresses and header injection are refused.
        let basic = Request { headers: vec![("Authorization".into(), "Basic ada:secret".into())], ..p.requests[3].clone() };
        assert_eq!(prepare(&basic, &c, &read).unwrap().headers[0].1, "Basic YWRhOnNlY3JldA==");
        let preencoded = Request { headers: vec![("Authorization".into(), "Basic YWRhOnNlY3JldA==".into())], ..p.requests[3].clone() };
        assert_eq!(prepare(&preencoded, &c, &read).unwrap().headers[0].1, "Basic YWRhOnNlY3JldA==");
        let bad_url = Request { url: "file:///etc/passwd".into(), ..p.requests[3].clone() };
        assert!(prepare(&bad_url, &c, &read).unwrap_err().contains("not a valid http(s) address"));
        let spaced = Request { url: "https://a b/".into(), ..p.requests[3].clone() };
        assert!(prepare(&spaced, &c, &read).is_err());
        let inject = Request { headers: vec![("X".into(), "a\r\nEvil: 1".into())], ..p.requests[3].clone() };
        assert!(prepare(&inject, &c, &read).unwrap_err().contains("not valid"));
        let missing = Request { body: Some(Body::File("./nope".into())), ..p.requests[3].clone() };
        assert!(prepare(&missing, &c, &read).unwrap_err().contains("no file"));
    }

    #[test]
    fn redirects_are_not_followed_and_their_target_is_reported() {
        let p = Prepared { method: "POST".into(), url: "http://a.test/".into(), headers: vec![("X-Key".into(), "k".into())], body: Some(b"x".to_vec()) };
        let args = curl_args(&p);
        assert!(!args.iter().any(|a| a == "-L" || a == "--location" || a.starts_with("--max-redirs")), "{args:?}");
        let r = parse_output(&out("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:9/admin\r\n\r\n", "", "307 0.1 0")).unwrap();
        assert_eq!(redirect_target(&r), Some("http://127.0.0.1:9/admin"));
        assert_eq!(redirect_target(&resp(None, b"")), None);
    }

    /// Real curl against two local listeners: the first answers 307 to the second, which must never see a connection.
    #[test]
    fn a_307_to_another_host_is_shown_not_followed() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::process::{Command, Stdio};
        if Command::new("curl").arg("--version").output().is_err() {
            return;
        }
        let (first, second) = (TcpListener::bind("127.0.0.1:0").unwrap(), TcpListener::bind("127.0.0.1:0").unwrap());
        second.set_nonblocking(true).unwrap();
        let target = format!("http://127.0.0.1:{}/admin", second.local_addr().unwrap().port());
        let reply = format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let url = format!("http://127.0.0.1:{}/", first.local_addr().unwrap().port());
        let server = std::thread::spawn(move || {
            let (mut s, _) = first.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            s.write_all(reply.as_bytes()).unwrap();
        });
        let p = Prepared { method: "POST".into(), url, headers: vec![("X-Key".into(), "k".into())], body: Some(b"secret".to_vec()) };
        let mut child = Command::new("curl").args(curl_args(&p)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(b"secret").unwrap();
        let out = child.wait_with_output().unwrap();
        server.join().unwrap();
        let r = parse_output(&out.stdout).unwrap();
        assert_eq!((r.status, redirect_target(&r)), (307, Some(target.as_str())));
        assert!(second.accept().is_err(), "the redirect target received a connection");
    }

    fn out(headers: &str, body: &str, meta: &str) -> Vec<u8> {
        format!("{headers}{body}\n{MARKER}{meta}").into_bytes()
    }

    #[test]
    fn responses_are_decoded_including_redirect_and_continue_blocks() {
        let raw = out("HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nX-A: 1\r\n\r\n", "{\"ok\":true}", "200 0.125 11");
        let r = parse_output(&raw).unwrap();
        assert_eq!((r.status, r.reason.as_str(), r.millis, r.size), (200, "OK", 125, 11));
        assert_eq!(r.decoded().map(|d| d.0).as_deref(), Some("{\"ok\":true}"));
        assert_eq!((r.header("content-type"), r.extension()), (Some("application/json; charset=utf-8"), "json"));
        assert_eq!(status_line(&r), "200 OK");
        // Redirect then final answer: only the last block counts.
        let raw = out("HTTP/1.1 301 Moved Permanently\r\nLocation: /b\r\n\r\nHTTP/2 404\r\ncontent-type: text/html\r\n\r\n", "<h1>no</h1>", "404 0.5 11");
        let r = parse_output(&raw).unwrap();
        assert_eq!((r.status, status_line(&r).as_str(), r.extension()), (404, "404 Not Found", "html"), "HTTP/2 has no reason phrase: a default is used");
        let raw = out("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\n\r\n", "", "201 0.01 0");
        let r = parse_output(&raw).unwrap();
        assert_eq!((r.status, r.body.len()), (201, 0));
        // A body that itself contains blank lines and the word HTTP/ is kept whole.
        let raw = out("HTTP/1.1 200 OK\r\n\r\n", "line1\n\nHTTP/1.1 not a header\n\nline4", "200 0.1 30");
        assert_eq!(parse_output(&raw).unwrap().decoded().map(|d| d.0).as_deref(), Some("line1\n\nHTTP/1.1 not a header\n\nline4"));
        // Binary bodies survive as bytes.
        let mut raw = b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\n\r\n".to_vec();
        raw.extend_from_slice(&[0x89, 0x50, 0xff, 0x00]);
        raw.extend_from_slice(format!("\n{MARKER}200 0.1 4").as_bytes());
        let r = parse_output(&raw).unwrap();
        assert_eq!(r.body, [0x89, 0x50, 0xff, 0x00]);
        assert!(r.decoded().is_none());
        assert!(parse_output(b"curl: (7) Failed").is_err());
        assert!(parse_output(format!("garbage\n{MARKER}000 0 0").as_bytes()).is_err());
        assert_eq!(human_size(10), "10 B");
        assert_eq!(human_size(2048), "2.0 KB");
    }

    fn resp(ct: Option<&str>, body: &[u8]) -> Response {
        Response {
            status: 200,
            reason: "OK".into(),
            headers: ct.map(|c| vec![("Content-Type".to_string(), c.to_string())]).unwrap_or_default(),
            body: body.to_vec(),
            millis: 1,
            size: body.len() as u64,
        }
    }

    #[test]
    fn bodies_are_decoded_by_charset_and_binary_stays_binary() {
        assert_eq!(resp(Some("text/plain"), "héllo".as_bytes()).decoded(), Some(("héllo".into(), None)));
        // Latin-1 and Turkish Latin-5 (İ ı Ş ş Ğ ğ differ from Latin-1).
        let (t, note) = resp(Some("text/html; charset=ISO-8859-1"), b"caf\xe9").decoded().unwrap();
        assert_eq!((t.as_str(), note.is_some()), ("café", true));
        let (t, _) = resp(Some("text/plain; charset=iso-8859-9"), b"\xdd\xfd\xde\xfe\xd0\xf0").decoded().unwrap();
        assert_eq!(t, "İıŞşĞğ");
        // Declared UTF-8 but broken, or no charset on a text type: replacement characters and a note.
        let (t, note) = resp(Some("application/json"), b"{\"a\":\"\xff\"}").decoded().unwrap();
        assert!(t.contains('\u{fffd}') && note.unwrap().contains("not valid UTF-8"));
        // Binary types and NUL-carrying unlabeled bodies are never forced into text.
        assert!(resp(Some("image/png"), &[0x89, 0x50, 0xff]).decoded().is_none());
        assert!(resp(Some("application/octet-stream"), &[0xff, 0xfe]).decoded().is_none());
        assert!(resp(None, &[b'a', 0, 0xff]).decoded().is_none());
        assert!(resp(None, b"plain \xff text").decoded().is_some(), "no content type and no NUL: text");
    }

    #[test]
    fn curl_failures_name_the_cause_and_where_the_request_ran() {
        let e = explain_curl(Some(7), "curl: (7) Failed to connect", "build-server", "http://localhost:8080/x");
        assert!(e.contains("localhost:8080") && e.contains("build-server") && e.contains("http.runOn"), "{e}");
        let e = explain_curl(Some(7), "", "this computer", "https://api.example.com/x");
        assert!(e.contains("api.example.com") && !e.contains("http.runOn"), "only localhost addresses get the runOn hint: {e}");
        assert!(explain_curl(Some(6), "", "srv", "https://nope.invalid/").contains("Could not resolve “nope.invalid”"));
        assert!(explain_curl(Some(28), "", "srv", "http://a/").contains("60 seconds"));
        assert!(explain_curl(Some(60), "curl: (60) SSL certificate problem: self-signed certificate", "srv", "https://a/").contains("self-signed"));
        assert!(explain_curl(Some(63), "", "srv", "http://a/").contains("20.0 MB"));
        assert!(explain_curl(Some(52), "", "srv", "http://a/").contains("no reply"));
        assert!(explain_curl(Some(99), "curl: (99) odd\n", "srv", "http://a/").contains("odd (curl exit 99)"));
        assert_eq!(explain_curl(None, "", "srv", "http://a/"), "The request failed.");
        // Credentials in the address are not echoed back.
        assert!(!explain_curl(Some(6), "", "srv", "https://user:secret@h.test/").contains("secret"));
    }

    #[test]
    fn sensitive_requests_are_recognised() {
        let p = parse(
            "GET https://a/\n\n###\nGET https://a/{{$processEnv HOME}}\n\n###\nPOST https://a/\n\n< ../secret.json\n\n###\nPOST https://a/\n\n< /etc/passwd\n\n###\nPOST https://a/\n\n< {{f}}\n\n###\nPOST https://a/\n\n< ./ok.json\n\n###\nPOST https://a/\nX-Key: {{$processEnv KEY}}\n\nbody\n",
        );
        let r: Vec<Vec<String>> = p.requests.iter().map(sensitive_parts).collect();
        assert!(r[0].is_empty());
        assert!(r[1][0].contains("environment variables"));
        assert!(r[2][0].contains("outside this file's folder") && r[2][0].contains("../secret.json"));
        assert!(r[3][0].contains("/etc/passwd"));
        assert!(r[4][0].contains("comes from a variable"));
        assert!(r[5].is_empty(), "files next to the .http file are fine");
        assert!(r[6][0].contains("environment variables"), "headers count too");
    }

    #[test]
    fn curl_refuses_oversized_responses() {
        let a = curl_args(&Prepared { method: "GET".into(), url: "https://h/".into(), headers: vec![], body: None });
        assert!(a.windows(2).any(|w| w[0] == "--max-filesize" && w[1] == MAX_RESPONSE_BYTES.to_string()));
    }

    #[test]
    fn datetime_formatting() {
        assert_eq!(format_datetime(0, "iso8601"), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_datetime(951_868_800, "iso8601"), "2000-03-01T00:00:00.000Z");
        assert_eq!(format_datetime(0, "rfc1123"), "Thu, 01 Jan 1970 00:00:00 GMT");
    }

    #[test]
    fn http_files_are_recognized() {
        assert!(is_http_file("/a/api.http") && is_http_file("x.REST") && !is_http_file("x.json"));
    }
}
