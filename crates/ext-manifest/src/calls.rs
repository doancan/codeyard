//! Rules on what one call carries, besides its permission: a request URL, a status item, the size of an answer. The
//! app, the extension host, the mock hosts (`ext-test`, and the TypeScript copy checked against
//! `sdk/typescript/test/rule-cases.json`) and `cyx test` all apply these, so a call refused in Codeyard is refused in a
//! test too, with the same words.

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
    /// For `IpLiteral`, the address as written (`127.0.0.1`, `[::1]`), lowercase; else empty.
    pub host: String,
}

impl BadUrl {
    /// How Codeyard's broker refuses it: an IP address is `denied` with [`ip_address_line`] (no `net` entry can allow
    /// it), the rest `invalid` with the message. `Err` is the denial's permission, `Ok` the invalid message.
    pub fn refusal(&self) -> Result<String, String> {
        if self.problem == UrlProblem::IpLiteral { Err(ip_address_line(&self.host)) } else { Ok(self.message.clone()) }
    }
}

/// The `permission` of the denial for a request to an IP address: `net` lists host names only, so nothing can grant
/// it (`crate::ungrantable_line`).
pub fn ip_address_line(host: &str) -> String {
    crate::ungrantable_line(&format!("net {host}"), "it is an IP address, and net lists host names only; connect to the server by its host name")
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
    let bad = |problem: UrlProblem, why: &str| BadUrl { problem, message: format!("{shown}: {why}"), host: String::new() };
    let invalid = |why: &str| bad(UrlProblem::Invalid, why);
    let Some(scheme_end) = url.find("://") else { return Err(invalid("is not a URL; write it as https://host/path")) };
    if !url[..scheme_end].eq_ignore_ascii_case("https") {
        return Err(bad(UrlProblem::NotHttps, "only https:// URLs are allowed; use https://"));
    }
    let authority = url[scheme_end + 3..].split(['/', '?']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(invalid("must not contain a user name or password; send credentials in a header"));
    }
    let ip_literal = |host: &str| BadUrl {
        host: host.to_ascii_lowercase(),
        ..bad(UrlProblem::IpLiteral, "names an IP address; extensions connect only to host names listed in `net`, so use the host name")
    };
    if authority.starts_with('[') {
        return Err(ip_literal(authority.split_inclusive(']').next().unwrap_or(authority)));
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
    // One trailing dot is the root (`example.com.`); `example.com..` is no name.
    if !host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.') || host.contains("..") || host.ends_with('.') {
        return Err(invalid("has an invalid host name; use letters, digits, '-' and dots (xn-- for other scripts)"));
    }
    if crate::is_ip_address(&host) {
        return Err(ip_literal(&host));
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

/// How `languages.set-diagnostics` treats a message: trimmed, `None` when it is empty or has a character that could
/// disguise what it says (`is_deceptive`: bidirectional overrides, zero-width characters; line breaks and tabs, which
/// compilers' messages have, are fine), cut to [`limits::DIAGNOSTIC_MESSAGE_LEN`] characters with a `…`. The `bool` says
/// it was cut.
pub fn shown_diagnostic_message(message: &str) -> Option<(String, bool)> {
    let message = message.trim();
    if message.is_empty() || message.chars().any(|c| c != '\n' && c != '\t' && is_deceptive(c)) {
        return None;
    }
    Some(match message.char_indices().nth(limits::DIAGNOSTIC_MESSAGE_LEN) {
        Some((cut, _)) => (format!("{}…", &message[..cut]), true),
        None => (message.to_string(), false),
    })
}

/// Whether an extension may set diagnostics on a file, or why not (the app logs it as a warning and shows nothing):
/// `declared` is the `languages` of each `[[diagnostics]]` entry (an empty list means every language; `None` skips the
/// check, as for a mock without a manifest), `language` the file's language id, `reachable` whether the file is in the
/// workspace or open in an editor, `other_files` how many other files already have diagnostics from the extension.
pub fn check_diagnostics_target(declared: Option<&[Vec<String>]>, language: &str, reachable: bool, other_files: usize) -> Result<(), String> {
    if let Some(declared) = declared {
        if declared.is_empty() {
            return Err("the manifest declares no [[diagnostics]]".into());
        }
        if !declared.iter().any(|languages| languages.is_empty() || languages.iter().any(|l| l == language)) {
            return Err(format!("[[diagnostics]] does not list its language, {language}"));
        }
    }
    if !reachable {
        return Err("it is neither in the workspace nor open in an editor".into());
    }
    if other_files >= limits::DIAGNOSTIC_FILES {
        return Err(format!("it has diagnostics on {other_files} files already, the most one extension may; clear some first"));
    }
    Ok(())
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
            ("https://a.example.com../", UrlProblem::Invalid),
            ("https://ıstanbul.example.com/", UrlProblem::Invalid),
            ("api.example.com/x", UrlProblem::Invalid),
            ("javascript:alert(1)", UrlProblem::Invalid),
        ] {
            assert_eq!(check_url(url).map_err(|e| e.problem), Err(problem), "{url}");
        }
        assert!(check_url(&format!("http://{}", "a".repeat(1000))).unwrap_err().message.len() < 300, "long URLs are cut in messages");
        // An IP address cannot be granted: the denial names it as written.
        let line = check_url("https://[::1]:8443/x").unwrap_err().refusal().unwrap_err();
        assert_eq!(crate::parse_ungrantable_line(&line).map(|(what, _)| what), Some("net [::1]"));
        assert_eq!(check_url("https://10.1/").unwrap_err().refusal(), Err(ip_address_line("10.1")));
        assert!(check_url("http://a.example.com/").unwrap_err().refusal().is_ok(), "the rest is invalid");
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
    fn diagnostics() {
        assert_eq!(shown_diagnostic_message("  borrow of moved value\n  --> a.rs:1\t "), Some(("borrow of moved value\n  --> a.rs:1".into(), false)));
        assert_eq!(shown_diagnostic_message("   "), None);
        assert_eq!(shown_diagnostic_message("a\u{202e}b"), None);
        assert_eq!(shown_diagnostic_message("a\u{200b}b"), None);
        let (cut, was_cut) = shown_diagnostic_message(&"ğ".repeat(limits::DIAGNOSTIC_MESSAGE_LEN + 1)).unwrap();
        assert!(was_cut && cut.chars().count() == limits::DIAGNOSTIC_MESSAGE_LEN + 1 && cut.ends_with('…'));
        assert_eq!(shown_diagnostic_message(&"x".repeat(limits::DIAGNOSTIC_MESSAGE_LEN)).map(|m| m.1), Some(false));

        let rust = vec![vec!["rust".to_string()]];
        assert_eq!(check_diagnostics_target(Some(&rust), "rust", true, 0), Ok(()));
        assert_eq!(check_diagnostics_target(Some(&[vec![]]), "toml", true, 0), Ok(()), "no languages: every language");
        assert_eq!(check_diagnostics_target(None, "toml", true, 0), Ok(()), "no manifest: nothing declared to check");
        assert!(check_diagnostics_target(Some(&[]), "rust", true, 0).unwrap_err().contains("declares no [[diagnostics]]"));
        assert!(check_diagnostics_target(Some(&rust), "toml", true, 0).unwrap_err().contains("does not list its language, toml"));
        assert!(check_diagnostics_target(Some(&rust), "rust", false, 0).unwrap_err().contains("neither in the workspace nor open"));
        assert!(check_diagnostics_target(Some(&rust), "rust", true, limits::DIAGNOSTIC_FILES).unwrap_err().contains("1000 files already"));
        assert_eq!(check_diagnostics_target(Some(&rust), "rust", true, limits::DIAGNOSTIC_FILES - 1), Ok(()));
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
