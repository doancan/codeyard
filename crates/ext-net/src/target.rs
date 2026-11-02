//! Request URLs: `https://` only, a host name (never an IP address), no user info, as `ext_manifest::calls::check_url`
//! decides for every copy of the rule. Redirect locations are resolved against the URL that sent them and then go
//! through the same check.

use hyper::Uri;

use ext_manifest::calls::UrlProblem;

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
    // The rules every copy shares (the mock hosts and `cyx test` too): scheme, user info, host name, port.
    let checked = ext_manifest::calls::check_url(url).map_err(|e| match e.problem {
        UrlProblem::NotHttps => Error::denied(Why::NotHttps, e.message),
        UrlProblem::IpLiteral => Error::denied(Why::IpLiteral, e.message),
        UrlProblem::Invalid => Error::Invalid(e.message),
    })?;
    let url = url.trim();
    let url = url.split_once('#').map_or(url, |(u, _)| u);
    let uri: Uri = url.parse().map_err(|e| Error::Invalid(format!("{}: is not a valid URL ({e})", shorten(url))))?;
    let mut path = match uri.path() {
        p if p.starts_with('/') => p.to_string(),
        p => format!("/{p}"),
    };
    if let Some(q) = uri.query() {
        path = format!("{path}?{q}");
    }
    Ok(Target { host: checked.host, port: checked.port, path })
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
