//! Removes what must not leave the machine from text that ends up in the log, a crash report or the diagnostics report:
//! the home folder, credentials and the user part of `user@host`. Pure; the work is linear in the input length.
//!
//! What stays on purpose: host names (`ssh deploy@build.example.com` becomes `ssh ***@build.example.com`, because the
//! host often is what a bug is about), commit ids (40 or 64 hex digits) and ordinary words.
//!
//! Paths are matched whatever way they were written: `/` or `\` or runs of them (`C:\\Users\\x` from a `{:?}` or JSON
//! string), the encoded forms `%2F` and `%5C`, any letter case, and Turkish letters composed or decomposed.

use regex::{Captures, Regex};
use std::sync::LazyLock;

const REDACTED: &str = "[redacted]";
/// One path separator: a slash, a backslash or their percent-encoded forms, one or more times.
const SEP: &str = r"(?:[/\\]|%2[fF]|%5[cC])+";

/// `scheme://user:password@host`: the user and password go, the host stays.
static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9+.\-]*://)[^\s/@]+@").expect("valid regex"));
/// `Authorization: Basic abc`, `Cookie: ...`, `Set-Cookie: ...`, `X-Api-Key: ...`, also as a JSON key (`"Cookie": "sid=abc"`): the value.
static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?im)(["']?\b(?:proxy-)?authorization|["']?\b(?:set-)?cookie|["']?\bx-api-key)(["']?)[ \t]*:[ \t]*("[^"\r\n]*"|'[^'\r\n]*'|[^\r\n]+)"#)
        .expect("valid regex")
});
/// `token=abc`, `"password": "abc"`, `GITHUB_TOKEN: abc`, `session_id=abc`.
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b([a-z0-9_.\-]*(?:token|secret|passw(?:or)?d|pwd|api[_\-]?key|access[_\-]?key|private[_\-]?key|credential|cookie|authorization|session[_\-]?id)[a-z0-9_.\-]*["']?[ \t]*[=:][ \t]*)("[^"\r\n]*"|'[^'\r\n]*'|[^\s,;&"']+)"#,
    )
    .expect("valid regex")
});
/// `aws_key=<40 characters>`: a key or secret whose value is base64 text, `/` and `+` included.
static KEYED_AWS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\b([a-z0-9_.\-]*(?:key|secret)[a-z0-9_.\-]*["']?[ \t]*[=:][ \t]*["']?)([A-Za-z0-9/+]{40})\b"#).expect("valid regex"));
/// `Bearer abc` or `Basic abc` outside a header; `token_shaped` decides whether the word after it is a credential.
static BEARER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(bearer|basic)[ \t]+([A-Za-z0-9._~+/=\-]{8,})").expect("valid regex"));
/// Tokens that announce themselves by their prefix.
static PREFIXED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9_\-]{16,}|xox[abprs]-[A-Za-z0-9\-]{10,}|sk-[A-Za-z0-9_\-]{20,}|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}(?:\.[A-Za-z0-9_\-]*)?)",
    )
    .expect("valid regex")
});
/// An AWS secret access key: 40 characters of base64, so `/` and `+` can be in it. The characters around it are kept.
static AWS_SECRET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|[^A-Za-z0-9/+=_\-])([A-Za-z0-9/+]{40})($|[^A-Za-z0-9/+=_\-])").expect("valid regex"));
/// A long run of token characters.
static LONG_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9_\-+=]{32,}").expect("valid regex"));
/// `anything@host`: email addresses and ssh logins. The host part is kept.
static LOGIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{N}._%+\-]+@([\p{L}\p{N}][\p{L}\p{N}.\-]*)").expect("valid regex"));
/// The home folder of any account, in case the text names someone else's (a CI log, a shared machine). Only where a path
/// STARTS (the beginning of the text, after a space, quote, bracket, `=` or `file://`, optionally with a drive or `\\?\`):
/// `src/users/list.rs` and `crates/app/src/home/view.rs` are project paths, not home folders.
static OTHER_HOME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"(?im)(^|[\s"'(\[{{<>=,;]|file:{SEP})((?:{SEP}\?{SEP})?(?:[a-z]:)?){SEP}(?:users|home){SEP}[^/\\\s:*?<>|"'%]+"#))
        .expect("valid regex")
});

/// Compile every pattern now, so that no later caller (the panic hook) has to.
pub fn warm_up() {
    redact("", None);
}

/// Redact, then (when `max_chars` is set) cut: a secret cut in two by the limit would leave its first half behind.
pub fn redact_and_cut(text: &str, home: Option<&str>, max_chars: usize) -> String {
    // Redacting a huge text is linear but not free: very long input is first cut at a word boundary, where no token spans the cut.
    let limit = max_chars.saturating_mul(50);
    let input = if text.chars().count() > limit {
        let cut: String = text.chars().take(limit).collect();
        cut.rsplit_once(char::is_whitespace).map_or(cut.clone(), |(head, _)| head.to_string())
    } else {
        text.to_string()
    };
    let out = redact(&input, home);
    if out.chars().count() <= max_chars { out } else { out.chars().take(max_chars).collect::<String>() + "…" }
}

/// `text` with the home folder shown as `~` and credentials removed. `home` is the current user's home folder.
pub fn redact(text: &str, home: Option<&str>) -> String {
    let mut out = HEADER.replace_all(text, "${1}${2}: [redacted]").into_owned();
    out = URL_USERINFO.replace_all(&out, "${1}***@").into_owned();
    out = ASSIGNMENT
        .replace_all(&out, |c: &Captures| if line_number_after_colon(&c[1], &c[2]) { c[0].to_string() } else { format!("{}{REDACTED}", &c[1]) })
        .into_owned();
    out = KEYED_AWS.replace_all(&out, |c: &Captures| if looks_like_aws_secret(&c[2]) { format!("{}{REDACTED}", &c[1]) } else { c[0].to_string() }).into_owned();
    out = BEARER.replace_all(&out, |c: &Captures| if token_shaped(&c[2]) { format!("{} {REDACTED}", &c[1]) } else { c[0].to_string() }).into_owned();
    out = PREFIXED.replace_all(&out, REDACTED).into_owned();
    out = AWS_SECRET
        .replace_all(&out, |c: &Captures| if looks_like_aws_secret(&c[2]) { format!("{}{REDACTED}{}", &c[1], &c[3]) } else { c[0].to_string() })
        .into_owned();
    out = LONG_RUN.replace_all(&out, |c: &Captures| if looks_secret(&c[0]) { REDACTED.to_string() } else { c[0].to_string() }).into_owned();
    out = LOGIN.replace_all(&out, "***@$1").into_owned();
    // The home folder after the rules above, so the text they replaced cannot hide a path from it.
    if let Some(home) = home.map(|h| h.trim_end_matches(['/', '\\'])).filter(|h| h.len() > 1) {
        out = replace_home(&out, home);
    }
    OTHER_HOME.replace_all(&out, "${1}~").into_owned()
}

/// `tokenizer.rs:12`: a file name that holds "token" and a line (and column) number after a colon with no space is a source location, not a
/// credential. `token: 12` and `token=12` still are.
fn line_number_after_colon(key_and_separator: &str, value: &str) -> bool {
    key_and_separator.ends_with(':') && !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit() || b == b':')
}

/// The word after `Bearer` / `Basic` is a credential when it has a digit or a symbol, or is long mixed-case text. Prose
/// ("a basic settings page", "bearer authentication") has none of these.
fn token_shaped(word: &str) -> bool {
    let symbol = word.bytes().any(|b| b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'~' | b'+' | b'/' | b'=' | b'-'));
    let mixed = word.len() >= 10 && word.bytes().any(|b| b.is_ascii_uppercase()) && word.bytes().any(|b| b.is_ascii_lowercase());
    symbol || mixed || word.len() >= 24
}

fn looks_like_aws_secret(run: &str) -> bool {
    run.bytes().any(|b| b.is_ascii_digit())
        && run.bytes().any(|b| b.is_ascii_alphabetic())
        && run.bytes().any(|b| matches!(b, b'/' | b'+'))
        && !run.contains("//")
}

/// A long run is a secret when it mixes letters and digits. Git commit ids (SHA-1, SHA-256) do too, but a bug report wants
/// them; a long identifier (`snake_case_names_of_a_test`) has no digits.
fn looks_secret(run: &str) -> bool {
    let commit_id = matches!(run.len(), 40 | 64) && run.bytes().all(|b| b.is_ascii_hexdigit());
    !commit_id && run.bytes().any(|b| b.is_ascii_digit()) && run.bytes().any(|b| b.is_ascii_alphabetic())
}

/// A piece of a path, with letters folded so that case and the composed/decomposed forms of Turkish letters do not matter.
#[derive(PartialEq)]
enum Piece {
    Separator,
    Letter(char),
}

fn is_combining(c: char) -> bool {
    ('\u{300}'..='\u{36f}').contains(&c)
}

fn fold(c: char) -> char {
    let lower = c.to_lowercase().next().unwrap_or(c);
    match lower {
        'ğ' => 'g',
        'ü' => 'u',
        'ş' => 's',
        'ö' => 'o',
        'ç' => 'c',
        'ı' => 'i',
        other => other,
    }
}

/// The separator at `at` (a slash, a backslash, `%2F`, `%5C`): its length in characters.
fn separator_len(text: &[char], at: usize) -> usize {
    match text.get(at) {
        Some('/' | '\\') => 1,
        Some('%') if matches!((text.get(at + 1), text.get(at + 2)), (Some('2'), Some('f' | 'F')) | (Some('5'), Some('c' | 'C'))) => 3,
        _ => 0,
    }
}

fn pieces(home: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    for c in home.chars().filter(|c| !is_combining(*c)) {
        match (c == '/' || c == '\\', out.last()) {
            (true, Some(Piece::Separator)) => {}
            (true, _) => out.push(Piece::Separator),
            (false, _) => out.push(Piece::Letter(fold(c))),
        }
    }
    out
}

/// Where the home folder ends when it starts at `from`, if it does.
fn match_home(text: &[char], from: usize, home: &[Piece]) -> Option<usize> {
    let mut at = from;
    for piece in home {
        while text.get(at).is_some_and(|c| is_combining(*c)) {
            at += 1;
        }
        match piece {
            Piece::Separator => {
                let first = separator_len(text, at);
                if first == 0 {
                    return None;
                }
                at += first;
                loop {
                    match separator_len(text, at) {
                        0 => break,
                        n => at += n,
                    }
                }
            }
            Piece::Letter(':') if matches!((text.get(at), text.get(at + 1), text.get(at + 2)), (Some('%'), Some('3'), Some('a' | 'A'))) => at += 3,
            Piece::Letter(letter) => {
                if fold(*text.get(at)?) != *letter {
                    return None;
                }
                at += 1;
            }
        }
    }
    while text.get(at).is_some_and(|c| is_combining(*c)) {
        at += 1;
    }
    ends_name(text.get(at).copied()).then_some(at)
}

/// Every occurrence of `home` becomes `~`, whichever separators the text uses (see the module note). A longer name that merely
/// starts with it (`/Users/ad` in `/Users/adam`) is left to the generic rule.
fn replace_home(text: &str, home: &str) -> String {
    let home = pieces(home);
    let starts_with_separator = home.first() == Some(&Piece::Separator);
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        match match_home(&chars, i, &home) {
            Some(end) => {
                drop_drive_letter(&mut out);
                out.push('~');
                i = end;
            }
            None => {
                // A failed match at the start of a separator run fails from every later position of the run too (the home folder
                // consumes the whole run): skip it, or a long run of slashes would take quadratic time.
                let run = if starts_with_separator { separator_run(&chars, i) } else { 0 };
                let step = run.max(1);
                out.extend(&chars[i..i + step]);
                i += step;
            }
        }
    }
    out
}

/// The length of the run of separators that starts at `at` (0 when there is none).
fn separator_run(text: &[char], at: usize) -> usize {
    let mut end = at;
    loop {
        match separator_len(text, end) {
            0 => return end - at,
            n => end += n,
        }
    }
}

/// `C:` in front of a home folder that is replaced belongs to the path: `C:\\Users\\x` becomes `~`, not `C:~`.
fn drop_drive_letter(out: &mut String) {
    let mut tail = out.chars().rev();
    if tail.next() == Some(':') && tail.next().is_some_and(|c| c.is_ascii_alphabetic()) && tail.next().is_none_or(|c| !c.is_alphanumeric()) {
        out.pop();
        out.pop();
    }
}

/// The character after a home folder: the path goes on, or the text does.
fn ends_name(next: Option<char>) -> bool {
    next.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '~')))
}

#[cfg(test)]
#[path = "diagnostics_redact_tests.rs"]
mod tests;
