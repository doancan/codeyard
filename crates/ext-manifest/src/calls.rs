//! Rules on what one call carries, besides its permission: a request URL, a status item, the size of an answer. The
//! app, the extension host, the mock hosts (`ext-test`, and the TypeScript copy checked against
//! `sdk/typescript/test/rule-cases.json`) and `cyx test` all apply these, so a call refused in Codeyard is refused in a
//! test too, with the same words.

use std::net::Ipv4Addr;

use crate::limits;
use crate::validate::is_deceptive;

/// Why a request URL is refused. `NotHttps` and `IpLiteral` are the `why` of a `denied` error in ext-net; the app
/// reports all three as `invalid`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlProblem {
    NotHttps,
    IpLiteral,
    Invalid,
}

/// A refused request URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadUrl {
    pub problem: UrlProblem,
    pub message: String,
}

/// The host and port of a request URL that passed [`check_url`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlTarget {
    /// Lowercase, without a trailing dot.
    pub host: String,
    pub port: u16,
}

/// A URL `net.fetch` or `auth.fetch` may request: `https://` only, a host name (never an IP address), no user name or
/// password, a port that is a number from 1. The fragment is ignored, as browsers do. What `net` allows is checked
/// after this, against [`UrlTarget::host`].
pub fn check_url(url: &str) -> Result<UrlTarget, BadUrl> {
    let url = url.trim();
    let url = url.split_once('#').map_or(url, |(u, _)| u);
    let shown = shorten(url);
    let bad = |problem: UrlProblem, why: &str| BadUrl { problem, message: format!("{shown}: {why}") };
    let invalid = |why: &str| bad(UrlProblem::Invalid, why);
    let Some(scheme_end) = url.find("://") else { return Err(invalid("is not a URL; write it as https://host/path")) };
    if !url[..scheme_end].eq_ignore_ascii_case("https") {
        return Err(bad(UrlProblem::NotHttps, "only https:// URLs are allowed; use https://"));
    }
    let authority = url[scheme_end + 3..].split(['/', '?']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(invalid("must not contain a user name or password; send credentials in a header"));
    }
    let ip_literal = || bad(UrlProblem::IpLiteral, "names an IP address; extensions connect only to host names listed in `net`, so use the host name");
    if authority.starts_with('[') {
        return Err(ip_literal());
    }
    let (raw, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
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
        return Err(ip_literal());
    }
    let port = match port {
        None => 443,
        Some("") => return Err(invalid("has an empty port")),
        Some(p) if !p.bytes().all(|b| b.is_ascii_digit()) => return Err(invalid("has an invalid port")),
        Some(p) => match p.parse::<u16>() {
            Ok(0) => return Err(invalid("has port 0")),
            Ok(p) => p,
            Err(_) => return Err(invalid("has an invalid port")),
        },
    };
    Ok(UrlTarget { host, port })
}

/// `127.0.0.1` and the other spellings resolvers accept for an IPv4 address: `127.1`, `2130706433`, `0x7f.1`. A name
/// whose labels are all numbers is one of them, and so is a name ending in a number (no top-level domain is numeric).
fn is_numeric_host(host: &str) -> bool {
    let number =
        |l: &str| !l.is_empty() && (l.bytes().all(|b| b.is_ascii_digit()) || l.strip_prefix("0x").is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit())));
    host.parse::<Ipv4Addr>().is_ok() || host.split('.').all(number) || host.rsplit('.').next().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
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

/// `window.set-status-item`: the item's local `id` must be declared in `[[status-bar]]` (`declared` says whether it
/// is; none skips the check, as for a mock without a manifest), its text 1 to [`limits::STATUS_TEXT_LEN`] visible
/// characters once trimmed, its tooltip at most [`limits::ITEM_LEN`]; neither may hold control, bidirectional or
/// zero-width characters. Returns the text as Codeyard shows it.
pub fn check_status_item(declared: Option<&dyn Fn(&str) -> bool>, id: &str, text: &str, tooltip: Option<&str>) -> Result<String, String> {
    if id.is_empty() {
        return Err("a status item needs an id: the local id of a [[status-bar]] entry".into());
    }
    if declared.is_some_and(|d| !d(id)) {
        return Err(format!("status item \"{id}\" is not declared; add [[status-bar]] with id = \"{id}\" to the manifest"));
    }
    let shown = text.trim();
    if shown.is_empty() || shown.chars().count() > limits::STATUS_TEXT_LEN || shown.chars().any(is_deceptive) {
        return Err(format!(
            "status item \"{id}\": its text must be 1 to {} visible characters, without control, bidirectional or zero-width characters",
            limits::STATUS_TEXT_LEN
        ));
    }
    if let Some(t) = tooltip
        && (t.chars().count() > limits::ITEM_LEN || t.chars().any(is_deceptive))
    {
        return Err(format!(
            "status item \"{id}\": its tooltip must be at most {} characters, without control, bidirectional or zero-width characters",
            limits::ITEM_LEN
        ));
    }
    Ok(shown.to_string())
}

/// The error for an answer (a command's result, a view's tree, …) of `bytes`, if it is larger than
/// [`limits::CALL_BYTES`]: the app never receives it, the request fails with this instead.
pub fn result_too_large(bytes: usize) -> Option<String> {
    const MIB: usize = 1 << 20;
    if bytes <= limits::CALL_BYTES {
        return None;
    }
    // In tenths of a MiB, rounded, and never shown as the limit itself.
    let tenths = ((bytes * 10 + MIB / 2) / MIB).max(limits::CALL_BYTES * 10 / MIB + 1);
    let size = if tenths.is_multiple_of(10) { format!("{}", tenths / 10) } else { format!("{}.{}", tenths / 10, tenths % 10) };
    Some(format!("the result is {size} MiB; at most {} MiB", limits::CALL_BYTES / MIB))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(check_url("https://API.Example.com./x?y#z"), Ok(UrlTarget { host: "api.example.com".into(), port: 443 }));
        assert_eq!(check_url("https://api.example.com:8443/").map(|t| t.port), Ok(8443));
        for (url, problem) in [
            ("http://api.example.com/", UrlProblem::NotHttps),
            ("HTTP://api.example.com/", UrlProblem::NotHttps),
            ("file:///etc/passwd", UrlProblem::NotHttps),
            ("https://127.0.0.1/", UrlProblem::IpLiteral),
            ("https://[::1]/", UrlProblem::IpLiteral),
            ("https://2130706433/", UrlProblem::IpLiteral),
            ("https://0x7f.1/", UrlProblem::IpLiteral),
            ("https://user:pw@api.example.com/", UrlProblem::Invalid),
            ("https://user@api.example.com/", UrlProblem::Invalid),
            ("https://", UrlProblem::Invalid),
            ("https://api.example.com:0/", UrlProblem::Invalid),
            ("https://api.example.com:/", UrlProblem::Invalid),
            ("https://api.example.com:99999/", UrlProblem::Invalid),
            ("https://api.example.com:+80/", UrlProblem::Invalid),
            ("https://bad_host.example.com/", UrlProblem::Invalid),
            ("https://ıstanbul.example.com/", UrlProblem::Invalid),
            ("api.example.com/x", UrlProblem::Invalid),
            ("javascript:alert(1)", UrlProblem::Invalid),
        ] {
            assert_eq!(check_url(url).map_err(|e| e.problem), Err(problem), "{url}");
        }
        assert!(check_url(&format!("http://{}", "a".repeat(1000))).unwrap_err().message.len() < 300, "long URLs are cut in messages");
    }

    #[test]
    fn status_items() {
        let declared = |id: &str| id == "deploys";
        let d: Option<&dyn Fn(&str) -> bool> = Some(&declared);
        assert_eq!(check_status_item(d, "deploys", "  Ready ✓ ", None), Ok("Ready ✓".into()));
        assert!(check_status_item(d, "other", "Ready", None).unwrap_err().contains("not declared"));
        assert!(check_status_item(None, "other", "Ready", None).is_ok(), "no manifest, no declared check");
        assert!(check_status_item(d, "", "Ready", None).unwrap_err().contains("needs an id"));
        assert!(check_status_item(d, "deploys", "   ", None).is_err());
        assert!(check_status_item(d, "deploys", &"y".repeat(65), None).unwrap_err().contains("1 to 64"));
        assert!(check_status_item(d, "deploys", &"ğ".repeat(64), None).is_ok(), "characters, not bytes");
        assert!(check_status_item(d, "deploys", "a\u{202e}b", None).is_err());
        assert!(check_status_item(d, "deploys", "ok", Some(&"t".repeat(257))).unwrap_err().contains("tooltip"));
    }

    #[test]
    fn result_sizes() {
        assert_eq!(result_too_large(limits::CALL_BYTES), None);
        assert_eq!(result_too_large(20 << 20).as_deref(), Some("the result is 20 MiB; at most 8 MiB"));
        assert_eq!(result_too_large((20 << 20) + 2).as_deref(), Some("the result is 20 MiB; at most 8 MiB"));
        assert_eq!(result_too_large(limits::CALL_BYTES + 1).as_deref(), Some("the result is 8.1 MiB; at most 8 MiB"));
        assert_eq!(result_too_large(12_900_000).as_deref(), Some("the result is 12.3 MiB; at most 8 MiB"));
    }
}
