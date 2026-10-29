//! Request URLs: `https://` only, a host name (never an IP address), no user info. Redirect locations are resolved
//! against the URL that sent them and then go through the same check.

use std::net::Ipv4Addr;

use hyper::Uri;

use crate::{Error, Why};

/// A checked request URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// Lowercase, without a trailing dot.
    pub host: String,
    pub port: u16,
    /// Path and query, at least `/`. The fragment is dropped, as browsers do.
    pub path: String,
}

impl Target {
    /// `host` or `host:port` when the port is not 443: the `Host` header.
    pub fn authority(&self) -> String {
        if self.port == 443 { self.host.clone() } else { format!("{}:{}", self.host, self.port) }
    }

    pub fn url(&self) -> String {
        format!("https://{}{}", self.authority(), self.path)
    }

    /// Same host and port, so credentials may follow a redirect.
    pub fn same_origin(&self, other: &Target) -> bool {
        self.host == other.host && self.port == other.port
    }
}

pub(crate) fn parse(url: &str) -> Result<Target, Error> {
    let url = url.trim();
    let url = url.split_once('#').map_or(url, |(u, _)| u);
    let shown = shorten(url);
    let invalid = |why: &str| Error::Invalid(format!("{shown}: {why}"));
    let Some(scheme_end) = url.find("://") else { return Err(invalid("is not a URL; write it as https://host/path")) };
    if !url[..scheme_end].eq_ignore_ascii_case("https") {
        return Err(Error::denied(Why::NotHttps, format!("{shown}: only https:// URLs are allowed; use https://")));
    }
    let uri: Uri = url.parse().map_err(|e| invalid(&format!("is not a valid URL ({e})")))?;
    let Some(authority) = uri.authority() else { return Err(invalid("has no host")) };
    if authority.as_str().contains('@') {
        return Err(invalid("must not contain a user name or password; send credentials in a header"));
    }
    let raw = authority.host();
    if raw.starts_with('[') {
        return Err(ip_literal(&shown));
    }
    let host = raw.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host).to_string();
    if host.is_empty() {
        return Err(invalid("has no host"));
    }
    // What `net` can list. A name in another script is written in its `xn--` form.
    if !host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.') || host.contains("..") {
        return Err(invalid("has an invalid host name; use letters, digits, '-' and dots (xn-- for other scripts)"));
    }
    if is_numeric_host(&host) {
        return Err(ip_literal(&shown));
    }
    let port = match authority.port_u16() {
        Some(0) => return Err(invalid("has port 0")),
        Some(p) => p,
        None if authority.as_str().ends_with(':') => return Err(invalid("has an empty port")),
        None => 443,
    };
    let mut path = match uri.path() {
        p if p.starts_with('/') => p.to_string(),
        p => format!("/{p}"),
    };
    if let Some(q) = uri.query() {
        path = format!("{path}?{q}");
    }
    Ok(Target { host, port, path })
}

/// `127.0.0.1` and the other spellings resolvers accept for an IPv4 address: `127.1`, `2130706433`, `0x7f.1`. A name
/// whose labels are all numbers is one of them, and so is a name ending in a number (no top-level domain is numeric).
fn is_numeric_host(host: &str) -> bool {
    let number =
        |l: &str| !l.is_empty() && (l.bytes().all(|b| b.is_ascii_digit()) || l.strip_prefix("0x").is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit())));
    host.parse::<Ipv4Addr>().is_ok() || host.split('.').all(number) || host.rsplit('.').next().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
}

fn ip_literal(shown: &str) -> Error {
    Error::denied(Why::IpLiteral, format!("{shown}: names an IP address; extensions connect only to host names listed in `net`, so use the host name"))
}

/// The URL a `Location` header points to, relative to `base`. The result still goes through `parse`.
pub(crate) fn join(base: &Target, location: &str) -> String {
    let loc = location.trim();
    let has_scheme = loc.find(':').is_some_and(|i| {
        let s = &loc[..i];
        !s.is_empty() && s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    });
    if has_scheme {
        return loc.to_string();
    }
    if let Some(rest) = loc.strip_prefix("//") {
        return format!("https://{rest}");
    }
    let origin = format!("https://{}", base.authority());
    if loc.starts_with('/') {
        return format!("{origin}{loc}");
    }
    let path = base.path.split(['?', '#']).next().unwrap_or("/");
    if loc.is_empty() || loc.starts_with('#') {
        return format!("{origin}{}", base.path);
    }
    if loc.starts_with('?') {
        return format!("{origin}{path}{loc}");
    }
    let dir = path.rfind('/').map_or("/", |i| &path[..=i]);
    format!("{origin}{dir}{loc}")
}

/// URLs in messages are cut, so a hostile page cannot fill the log with one.
fn shorten(url: &str) -> String {
    const MAX: usize = 200;
    if url.len() <= MAX {
        return url.to_string();
    }
    let mut end = MAX;
    while !url.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &url[..end])
}
