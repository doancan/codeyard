//! The identity token Fulcio turns into a signing certificate, from wherever `cyx publish` runs:
//!
//! 1. `SIGSTORE_ID_TOKEN`, when something already got one (Sigstore's convention);
//! 2. GitHub Actions: the workflow's own token (`permissions: id-token: write`), for audience `sigstore`;
//! 3. a laptop: a browser login at Sigstore's sign-in service (OAuth with PKCE, answered on a local port).
//!
//! The token is used once and never written anywhere.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use sha2::{Digest as _, Sha256};

use crate::net;

/// Sigstore's public sign-in service.
pub const DEFAULT_ISSUER: &str = "https://oauth2.sigstore.dev/auth";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

pub fn token(issuer: &str) -> Result<String, String> {
    if let Ok(t) = std::env::var("SIGSTORE_ID_TOKEN")
        && !t.trim().is_empty()
    {
        return Ok(t.trim().to_string());
    }
    if let (Ok(url), Ok(bearer)) = (std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL"), std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN")) {
        return github_actions(&url, &bearer);
    }
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        return Err("error: this workflow cannot get an identity token\n   = fix: add `permissions: id-token: write` to the job".into());
    }
    browser(issuer)
}

fn github_actions(url: &str, bearer: &str) -> Result<String, String> {
    let sep = if url.contains('?') { '&' } else { '?' };
    let res = net::request("GET", &format!("{url}{sep}audience=sigstore"), &[("authorization", &format!("bearer {bearer}"))], Vec::new())?;
    if !res.ok() {
        return Err(format!("error: GitHub refused an identity token ({}): {}", res.status, res.text()));
    }
    let v: serde_json::Value = serde_json::from_slice(&res.body).map_err(|e| format!("error: GitHub's token answer: {e}"))?;
    v["value"].as_str().map(str::to_string).ok_or_else(|| "error: GitHub's token answer has no value".into())
}

/// What Fulcio needs signed as proof that the key belongs to the token's holder: the email for an email identity,
/// the subject otherwise.
pub fn subject(token: &str) -> Result<String, String> {
    let payload = token.split('.').nth(1).ok_or("error: the identity token is not a JWT")?;
    let bytes = B64URL.decode(payload.trim_end_matches('=')).map_err(|_| "error: the identity token is not a JWT")?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "error: the identity token is not a JWT")?;
    claims["email"].as_str().or(claims["sub"].as_str()).map(str::to_string).ok_or_else(|| "error: the identity token names no subject".into())
}

/// The OAuth 2 authorization-code flow with PKCE: open the browser at the sign-in page, wait for its redirect on a
/// local port, trade the code for an ID token.
fn browser(issuer: &str) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("error: cannot listen for the sign-in: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://localhost:{port}/auth/callback");
    let (verifier, challenge) = pkce();
    let state = random_token();
    let url = format!(
        "{issuer}/auth?response_type=code&client_id=sigstore&scope=openid%20email&redirect_uri={}&code_challenge={challenge}&code_challenge_method=S256&state={state}&nonce={}",
        encode(&redirect),
        random_token()
    );
    eprintln!("Sign in to sign this release. Opening your browser; if it does not open, go to:\n\n  {url}\n");
    open_browser(&url);
    let code = wait_for_code(&listener, &state)?;
    trade(issuer, &code, &redirect, &verifier)
}

/// Exchange the authorization code for an ID token.
pub fn trade(issuer: &str, code: &str, redirect: &str, verifier: &str) -> Result<String, String> {
    let form = format!("grant_type=authorization_code&client_id=sigstore&code={}&redirect_uri={}&code_verifier={verifier}", encode(code), encode(redirect));
    let res = net::request("POST", &format!("{issuer}/token"), &[("content-type", "application/x-www-form-urlencoded")], form.into_bytes())?;
    if !res.ok() {
        return Err(format!("error: the sign-in was refused ({}): {}", res.status, res.text()));
    }
    let v: serde_json::Value = serde_json::from_slice(&res.body).map_err(|e| format!("error: the sign-in answer: {e}"))?;
    v["id_token"].as_str().map(str::to_string).ok_or_else(|| "error: the sign-in answer has no id_token".into())
}

/// Wait for the browser's redirect to `/auth/callback?code=…&state=…` and answer it with a page saying it is done.
pub fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, String> {
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    loop {
        let (stream, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err("error: no sign-in within 5 minutes".into());
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        stream.set_nonblocking(false).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).map_err(|e| e.to_string())?;
        let target = line.split(' ').nth(1).unwrap_or_default();
        let Some(query) = target.strip_prefix("/auth/callback?") else {
            reply(&stream, "404 Not Found", "Not found.");
            continue;
        };
        let param = |name: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{name}="))).map(decode);
        if param("state").as_deref() != Some(state) {
            reply(&stream, "400 Bad Request", "This sign-in did not come from cyx; nothing was signed.");
            return Err("error: the sign-in answer did not match (state); nothing was signed".into());
        }
        return match param("code") {
            Some(code) => {
                reply(&stream, "200 OK", "Signed in. You can close this tab and go back to the terminal.");
                Ok(code)
            }
            None => {
                let why = param("error_description").or_else(|| param("error")).unwrap_or_else(|| "no code".into());
                reply(&stream, "400 Bad Request", "The sign-in failed; see the terminal.");
                Err(format!("error: the sign-in failed: {why}"))
            }
        };
    }
}

fn reply(mut stream: &std::net::TcpStream, status: &str, text: &str) {
    let body = format!("<!doctype html><meta charset=utf-8><title>cyx</title><p style=\"font:16px system-ui;margin:3em\">{text}</p>");
    let _ = write!(stream, "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
}

fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(windows) {
        ("rundll32", vec!["url.dll,FileProtocolHandler", url])
    } else {
        ("xdg-open", vec![url])
    };
    let _ = std::process::Command::new(opener.0).args(opener.1).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

/// (verifier, S256 challenge).
pub fn pkce() -> (String, String) {
    let verifier = random_token();
    let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).expect("system randomness");
    B64URL.encode(bytes)
}

/// Percent-encoding for a query value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                Ok(v) => {
                    out.push(v);
                    i += 3;
                    continue;
                }
                Err(_) => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subject_is_the_email_or_sub() {
        let jwt = |claims: &str| format!("h.{}.s", B64URL.encode(claims));
        assert_eq!(subject(&jwt(r#"{"sub":"repo:you/night:ref:refs/tags/v1","email":null}"#)).unwrap(), "repo:you/night:ref:refs/tags/v1");
        assert_eq!(subject(&jwt(r#"{"sub":"123","email":"me@example.com"}"#)).unwrap(), "me@example.com");
        assert!(subject("not a jwt").is_err());
    }

    #[test]
    fn pkce_challenge_is_s256_of_the_verifier() {
        let (v, c) = pkce();
        assert_eq!(c, B64URL.encode(Sha256::digest(v.as_bytes())));
        assert!(v.len() >= 43);
    }

    #[test]
    fn query_values_round_trip() {
        let s = "http://localhost:1234/auth/callback?x=a b&ü";
        assert_eq!(decode(&encode(s)), s);
        assert_eq!(decode("a%2"), "a%2");
        assert_eq!(decode("%"), "%");
    }

    #[test]
    fn the_callback_checks_state_and_returns_the_code() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // Sent in this order: the stray request first, then the callback.
        let send = move |path: &'static str, after_ms: u64| {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(after_ms));
                let mut s = std::net::TcpStream::connect(addr).unwrap();
                write!(s, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
                let mut out = String::new();
                let _ = std::io::Read::read_to_string(&mut s, &mut out);
                out
            })
        };
        let favicon = send("/favicon.ico", 20);
        let page = send("/auth/callback?code=abc%2Fd&state=S1", 200);
        let code = wait_for_code(&listener, "S1").unwrap();
        assert_eq!(code, "abc/d");
        assert!(favicon.join().unwrap().starts_with("HTTP/1.1 404"));
        assert!(page.join().unwrap().contains("You can close this tab"));
        let forged = send("/auth/callback?code=abc&state=OTHER", 20);
        assert!(wait_for_code(&listener, "S1").unwrap_err().contains("did not match"));
        forged.join().unwrap();
    }
}
