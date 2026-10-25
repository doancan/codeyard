//! "Data: Decode Selection": a JWT (header and payload, never verified) or Unix timestamps (seconds or milliseconds)
//! as readable lines. The selection is looked at and nothing else: no signature check, no key lookup, no request, and
//! the text is not written to the log.

use serde_json::Value;

use crate::datalens::{DataError, emit_json};
use crate::i18n::{t, tf};

/// Claims that hold a time (seconds since 1970) and get a readable date next to them.
const TIME_CLAIMS: &[&str] = &["exp", "iat", "nbf", "auth_time"];

/// A timestamp from here on is read as milliseconds: 10^11 seconds is the year 5138, 10^11 ms is March 1973.
const MS_FROM: f64 = 1.0e11;
/// Beyond this a number is not offered as a date at all (year 5138 in milliseconds is 10^14).
const LIMIT: f64 = 1.0e14;

pub fn decode(selection: &str) -> Result<Vec<String>, DataError> {
    let text = selection.trim().trim_matches(['"', '\'']).trim();
    let text = text.strip_prefix("Bearer ").or_else(|| text.strip_prefix("bearer ")).unwrap_or(text).trim();
    if text.is_empty() {
        return Err(DataError { message: t("Select a JWT or a Unix timestamp first").to_string(), at: None });
    }
    if let Some(lines) = jwt(text) {
        return Ok(lines);
    }
    timestamps(text)
}

/// `header.payload.signature`, each part base64url; the signature may be empty (`alg: none`).
fn jwt(text: &str) -> Option<Vec<String>> {
    let mut parts = text.split('.');
    let (head, body, sig) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || head.is_empty() || body.is_empty() {
        return None;
    }
    let url_safe = |s: &str| s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'='));
    if !(url_safe(head) && url_safe(body) && url_safe(sig)) {
        return None;
    }
    let part = |s: &str| -> Option<Value> { serde_json::from_str(&crate::texttools::base64_decode(s)?).ok() };
    let (header, payload) = (part(head)?, part(body)?);
    if !header.is_object() {
        return None;
    }
    let mut lines = vec![t("Signature not verified: the token is only decoded, not checked").to_string(), String::new(), t("Header").to_string()];
    lines.extend(json_lines(&header));
    lines.push(String::new());
    lines.push(t("Payload").to_string());
    lines.extend(json_lines(&payload));
    if let Value::Object(claims) = &payload {
        for name in TIME_CLAIMS {
            if let Some(secs) = claims.get(*name).and_then(Value::as_f64) {
                lines.push(String::new());
                lines.push(tf("{} is a date:", &[name]));
                lines.extend(when(secs.floor() as i64, 0).into_iter().map(|l| format!("  {l}")));
            }
        }
    }
    if sig.is_empty() {
        lines.push(String::new());
        lines.push(t("The token has no signature.").to_string());
    }
    Some(lines)
}

fn json_lines(v: &Value) -> Vec<String> {
    match emit_json(v, 2) {
        Ok(text) => text.lines().map(|l| format!("  {l}")).collect(),
        Err(_) => Vec::new(),
    }
}

/// One or more numbers, separated by spaces, commas or semicolons.
fn timestamps(text: &str) -> Result<Vec<String>, DataError> {
    let tokens: Vec<&str> = text.split(|c: char| c.is_whitespace() || c == ',' || c == ';').filter(|s| !s.is_empty()).collect();
    if tokens.len() > 20 {
        return Err(DataError { message: t("Select at most 20 timestamps").to_string(), at: None });
    }
    let mut lines = Vec::new();
    for token in tokens {
        let Some(value) = token.parse::<f64>().ok().filter(|v| v.is_finite() && token.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'.'))) else {
            return Err(DataError { message: t("The selection is neither a JWT nor a Unix timestamp").to_string(), at: None });
        };
        if value.abs() >= LIMIT {
            return Err(DataError { message: tf("{} is too large to be a time in seconds or milliseconds", &[&token]), at: None });
        }
        let (millis, unit) = if value.abs() >= MS_FROM { (value, t("milliseconds")) } else { (value * 1000.0, t("seconds")) };
        let millis = millis.round() as i64;
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!("{token} ({unit})"));
        lines.extend(when(millis.div_euclid(1000), millis.rem_euclid(1000) as u32).into_iter().map(|l| format!("  {l}")));
    }
    Ok(lines)
}

/// UTC and local rows for a time.
fn when(secs: i64, millis: u32) -> Vec<String> {
    let frac = if millis > 0 { format!(".{millis:03}") } else { String::new() };
    let mut rows = vec![format!("UTC    {}{frac}", civil(secs))];
    match local_offset(secs) {
        Some(off) => {
            let sign = if off < 0 { '-' } else { '+' };
            let (h, m) = (off.abs() / 3600, off.abs() / 60 % 60);
            rows.push(format!("{}  {}{frac} {sign}{h:02}:{m:02}", t("Local"), civil(secs + off)));
        }
        None => rows.push(t("Local time is not available on this system").to_string()),
    }
    rows
}

/// `2026-10-24 13:05:09` for seconds since the epoch (Howard Hinnant's days-to-date algorithm, valid for negative times too).
fn civil(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", if m <= 2 { y + 1 } else { y }, rem / 3600, rem / 60 % 60, rem % 60)
}

/// Seconds east of UTC at that moment in the user's time zone (daylight saving included).
#[cfg(unix)]
fn local_offset(secs: i64) -> Option<i64> {
    let at = libc::time_t::try_from(secs).ok()?;
    // SAFETY: `localtime_r` writes only into the `tm` we pass and reads `at`; both live for the call.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&at, &mut tm).is_null() { None } else { Some(tm.tm_gmtoff as i64) }
    }
}

/// No portable way without another dependency: the UTC row is shown alone.
#[cfg(not(unix))]
fn local_offset(_: i64) -> Option<i64> {
    None
}

#[cfg(test)]
pub fn civil_for_test(secs: i64) -> String {
    civil(secs)
}
