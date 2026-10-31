//! Accounts and secrets of extensions (docs/plugin-architecture.md §6.3, §6.4), without UI. Tokens and `secrets` live
//! in the system keychain (macOS Keychain, Windows Credential Manager, the Secret Service on Linux) under a key made of
//! the extension's publisher identity, its id and the entry's name: a sideloaded package with the same id as a registry
//! one has another identity and finds nothing.
//!
//! The browser sign-in is OAuth 2 with PKCE (RFC 7636). It returns through `codeyard://ext/<id>/callback?code&state`;
//! a callback completes only the flow whose random `state` it carries, and only when the extension in its path is the
//! one that started it. On Windows and Linux the operating system starts a second Codeyard with that address; it hands
//! the address to the running one over a loopback port (`forward_callback`) and exits.
//!
//! Requests signed with a token go through `ext_fetch` (ext-net), so the token never reaches the exthost.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use ext_api::value as v;
use ext_core::Source;
use ext_core::instances::Instance;
use ext_manifest::{Manifest, OAuthProvider};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// The address scheme the sign-in returns through.
pub const SCHEME: &str = "codeyard";
/// A sign-in that does not come back within this time is forgotten.
pub const FLOW_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// One `secrets` value. Windows Credential Manager keeps at most 2560 bytes per entry; the same limit everywhere keeps
/// extensions portable.
pub const SECRET_VALUE_BYTES: usize = 2048;
/// One `secrets` key.
pub const SECRET_KEY_LEN: usize = 128;
/// The account name shown to the user, at most.
const LABEL_LEN: usize = 64;

/// The system keychain, or a map in tests. Calls block: run them on a background thread.
pub trait Keychain: Send + Sync {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    fn write(&self, key: &str, value: &[u8]) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// A keychain in memory: tests, and a machine without a keychain service.
#[derive(Default)]
pub struct MemoryKeychain(Mutex<BTreeMap<String, Vec<u8>>>);

impl MemoryKeychain {
    #[cfg(test)]
    pub fn keys(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).keys().cloned().collect()
    }

    /// Every stored value, for tests that look for a token where it must not be.
    #[cfg(test)]
    pub fn contains_value(&self, needle: &str) -> bool {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).values().any(|v| String::from_utf8_lossy(v).contains(needle))
    }
}

impl Keychain for MemoryKeychain {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.lock().unwrap_or_else(|p| p.into_inner()).get(key).cloned())
    }
    fn write(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).insert(key.to_string(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).remove(key);
        Ok(())
    }
}

/// Who an extension is, for the keychain and the sign-in prompt: its publisher identity when a registry signed it,
/// else where it was installed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Publisher {
    /// The identity as one value; part of every keychain key.
    pub identity: String,
    /// What the prompt says.
    pub shown: String,
}

impl Publisher {
    /// `source` is the lock file's entry; `dev` is the folder of `--extension-dev` when this extension is the one in
    /// development.
    pub fn of(id: &str, source: Option<&Source>, dev: Option<&Path>) -> Publisher {
        if let Some(dir) = dev {
            let d = dir.display().to_string();
            return Publisher { identity: format!("dev\n{d}"), shown: crate::i18n::tf("{0}, in development from {1}", &[&publisher_part(id), &d]) };
        }
        match source {
            Some(Source::Registry { registry, publisher: p }) => Publisher {
                // Registry, then the signing identity in full: issuer, account, owner and repository by number, workflow.
                identity: format!(
                    "registry\n{registry}\n{}\n{}\n{}\n{}\n{}",
                    p.issuer,
                    p.subject.as_deref().unwrap_or(""),
                    p.owner_id.map(|n| n.to_string()).unwrap_or_default(),
                    p.repository_id.map(|n| n.to_string()).unwrap_or_default(),
                    p.workflow.as_deref().unwrap_or("")
                ),
                shown: crate::i18n::tf("{0}, signed by {1} in the {2} registry", &[&publisher_part(id), p, registry]),
            },
            Some(Source::File { path }) | Some(Source::Folder { path }) => Publisher {
                identity: format!("sideloaded\n{path}"),
                shown: crate::i18n::tf("{0}, installed from {1} (not verified by a registry)", &[&publisher_part(id), path]),
            },
            None => Publisher { identity: "unknown".into(), shown: crate::i18n::tf("{} (not verified by a registry)", &[&publisher_part(id)]) },
        }
    }
}

fn publisher_part(id: &str) -> String {
    id.split('.').next().unwrap_or(id).to_string()
}

/// What a keychain entry holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry<'a> {
    Account(&'a str),
    Secret(&'a str),
}

/// The keychain key of one entry: `codeyard-ext:<hash of the publisher identity>/<extension id>/<kind>/<name>`.
pub fn entry_key(publisher: &Publisher, extension: &str, entry: &Entry) -> String {
    let h = Sha256::digest(publisher.identity.as_bytes());
    let hash: String = h[..12].iter().map(|b| format!("{b:02x}")).collect();
    let (kind, name) = match entry {
        Entry::Account(p) => ("account", *p),
        Entry::Secret(k) => ("secret", *k),
    };
    format!("codeyard-ext:{hash}/{extension}/{kind}/{name}")
}

/// A signed-in account as the keychain keeps it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    #[serde(rename = "a")]
    pub access: String,
    #[serde(rename = "r", default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
    /// Unix seconds.
    #[serde(rename = "e", default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
    /// The account's name, shown to the user and to the extension.
    #[serde(rename = "l")]
    pub label: String,
    #[serde(rename = "s", default)]
    pub scopes: Vec<String>,
}

/// Never print a token.
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token").field("label", &self.label).field("expires", &self.expires).finish_non_exhaustive()
    }
}

impl Token {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Option<Token> {
        serde_json::from_slice(bytes).ok()
    }

    /// Expired, or expiring within a minute.
    pub fn stale(&self, now: u64) -> bool {
        self.expires.is_some_and(|e| now + 60 >= e)
    }
}

/// The keychain operations on accounts and secrets, for one extension of one publisher.
pub struct Vault<'a> {
    pub keychain: &'a dyn Keychain,
    pub publisher: &'a Publisher,
    pub extension: &'a str,
}

impl Vault<'_> {
    fn key(&self, e: Entry) -> String {
        entry_key(self.publisher, self.extension, &e)
    }

    pub fn account(&self, provider: &str) -> Result<Option<Token>, String> {
        Ok(self.keychain.read(&self.key(Entry::Account(provider)))?.as_deref().and_then(Token::decode))
    }

    pub fn save_account(&self, provider: &str, token: &Token) -> Result<(), String> {
        self.keychain.write(&self.key(Entry::Account(provider)), &token.encode())
    }

    /// Sign out: the token leaves the keychain.
    pub fn sign_out(&self, provider: &str) -> Result<(), String> {
        let key = self.key(Entry::Account(provider));
        // Some keychains report an error for a missing entry; nothing to delete is not a failure.
        if self.keychain.read(&key)?.is_none() {
            return Ok(());
        }
        self.keychain.delete(&key)
    }

    pub fn secret(&self, key: &str) -> Result<Option<String>, v::Error> {
        check_secret_key(key)?;
        let bytes = self.keychain.read(&self.key(Entry::Secret(key))).map_err(v::Error::Other)?;
        Ok(bytes.and_then(|b| String::from_utf8(b).ok()))
    }

    pub fn set_secret(&self, key: &str, value: &str) -> Result<(), v::Error> {
        check_secret_key(key)?;
        if value.len() > SECRET_VALUE_BYTES {
            return Err(v::Error::Invalid(format!("a secret holds at most {SECRET_VALUE_BYTES} bytes; this one has {}", value.len())));
        }
        self.keychain.write(&self.key(Entry::Secret(key)), value.as_bytes()).map_err(v::Error::Other)
    }

    pub fn delete_secret(&self, key: &str) -> Result<(), v::Error> {
        check_secret_key(key)?;
        let k = self.key(Entry::Secret(key));
        if self.keychain.read(&k).map_err(v::Error::Other)?.is_none() {
            return Ok(());
        }
        self.keychain.delete(&k).map_err(v::Error::Other)
    }
}

/// `a-z A-Z 0-9 . _ -`, 1 to 128 characters: the key ends up in a keychain item's name.
pub fn check_secret_key(key: &str) -> Result<(), v::Error> {
    let ok = !key.is_empty() && key.len() <= SECRET_KEY_LEN && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok { Ok(()) } else { Err(v::Error::Invalid(format!("secret keys are 1 to {SECRET_KEY_LEN} characters of a-z, A-Z, 0-9, '.', '_' and '-'"))) }
}

/// An account provider as the app uses it: built in (GitHub) or the extension's `[[oauth]]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    pub id: String,
    pub name: String,
    /// The hosts `auth.fetch` signs requests to.
    pub hosts: Vec<String>,
    /// `None` for GitHub, which uses the sign-in Codeyard already has (the GitHub CLI).
    pub oauth: Option<OAuthProvider>,
}

pub const GITHUB_HOSTS: &[&str] = &["api.github.com", "uploads.github.com"];

/// The provider `id` for this extension, if its manifest can use it: built in, an `[[oauth]]` table, or a
/// self-managed instance of one the user added (`gitlab@git.example.com`, #184).
pub fn provider(m: &Manifest, instances: &[Instance], id: &str) -> Option<Provider> {
    if id == "github" {
        return Some(Provider { id: id.into(), name: "GitHub".into(), hosts: GITHUB_HOSTS.iter().map(|h| h.to_string()).collect(), oauth: None });
    }
    let p = match ext_manifest::split_account(id) {
        (base, Some(host)) => instances.iter().find(|i| i.provider == base && i.host == host)?.oauth(m)?,
        (_, None) => m.oauth_provider(id)?.clone(),
    };
    Some(Provider { id: p.id.clone(), name: p.name.clone(), hosts: p.hosts.clone(), oauth: Some(p) })
}

/// The `[permissions] auth` entry an account id is granted by: an instance's is its provider's.
pub fn granted_as(id: &str) -> &str {
    ext_manifest::split_account(id).0
}

/// The scopes to ask for: what `auth.session` names, each declared in `[[oauth]] scopes`; all declared ones when it
/// names none.
pub fn scopes_for(p: &OAuthProvider, requested: &[String]) -> Result<Vec<String>, v::Error> {
    if requested.is_empty() {
        return Ok(p.scopes.clone());
    }
    match requested.iter().find(|s| !p.scopes.contains(s)) {
        Some(s) => Err(v::Error::Invalid(format!("scope '{s}' is not in [[oauth]] scopes of provider '{}'; add it there first", p.id))),
        None => Ok(requested.to_vec()),
    }
}

/// The verifier the app keeps and the challenge the browser carries (RFC 7636, S256).
#[derive(Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Pkce {
        Pkce::from_verifier(random_token())
    }

    pub fn from_verifier(verifier: String) -> Pkce {
        let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
        Pkce { verifier, challenge }
    }
}

/// 32 random bytes from the system's generator, base64url: a PKCE verifier or a `state`.
pub fn random_token() -> String {
    use ring::rand::SecureRandom as _;
    let mut b = [0u8; 32];
    ring::rand::SystemRandom::new().fill(&mut b).expect("the system random generator works");
    b64url(&b)
}

pub fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | ((*b as u32) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// `application/x-www-form-urlencoded` and query values.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                out.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

pub fn form(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&")
}

/// Where the browser comes back to for `extension`.
pub fn redirect_uri(extension: &str) -> String {
    format!("{SCHEME}://ext/{extension}/callback")
}

/// The sign-in page's address: the authorization code flow with PKCE.
pub fn authorize_url(p: &OAuthProvider, extension: &str, scopes: &[String], state: &str, challenge: &str) -> String {
    let sep = if p.authorize_url.contains('?') { '&' } else { '?' };
    let query = form(&[
        ("response_type", "code"),
        ("client_id", &p.client_id),
        ("redirect_uri", &redirect_uri(extension)),
        ("scope", &scopes.join(" ")),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ]);
    format!("{}{sep}{query}", p.authorize_url)
}

/// A sign-in waiting for the browser.
pub struct Flow {
    pub extension: String,
    pub provider: String,
    pub state: String,
    pub pkce: Pkce,
    pub scopes: Vec<String>,
    /// The `auth.session` ticket it completes.
    pub ticket: u64,
    pub started: Instant,
}

/// What came back in `codeyard://ext/<id>/callback?…`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Callback {
    pub extension: String,
    pub state: String,
    /// The code, or the provider's error.
    pub result: Result<String, String>,
}

/// Parse a callback address; anything else is `None`.
pub fn parse_callback(url: &str) -> Option<Callback> {
    let rest = url.strip_prefix(&format!("{SCHEME}://ext/"))?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let extension = path.strip_suffix("/callback")?.trim_end_matches('/');
    ext_manifest::ExtId::parse(extension).ok()?;
    let query = query.split('#').next().unwrap_or_default();
    let param = |name: &str| query.split('&').find_map(|kv| kv.strip_prefix(name)?.strip_prefix('=')).and_then(decode);
    let state = param("state")?;
    let result = match param("code") {
        Some(code) => Ok(code),
        None => Err(param("error_description").or_else(|| param("error")).unwrap_or_else(|| "no code".into())),
    };
    Some(Callback { extension: extension.to_string(), state, result })
}

/// Why a callback completed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// No sign-in in this window has that state (another window's, an old one, or a forged one).
    Unknown,
    /// The state belongs to another extension's sign-in: that flow stays open, this answer is dropped.
    OtherExtension { owner: String },
}

/// Sign-ins in progress in one window.
#[derive(Default)]
pub struct Flows(Vec<Flow>);

impl Flows {
    pub fn start(&mut self, flow: Flow) {
        self.0.retain(|f| f.started.elapsed() < FLOW_TIMEOUT && !(f.extension == flow.extension && f.provider == flow.provider));
        self.0.push(flow);
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(test)]
    pub fn state_of(&self, extension: &str) -> Option<String> {
        self.0.iter().find(|f| f.extension == extension).map(|f| f.state.clone())
    }

    /// The flow `cb` completes, taken out of the list. Its state must match one in full and the extension in the
    /// address must be the one that started it.
    pub fn take(&mut self, cb: &Callback) -> Result<Flow, Refused> {
        self.0.retain(|f| f.started.elapsed() < FLOW_TIMEOUT);
        let Some(i) = self.0.iter().position(|f| same(&f.state, &cb.state)) else { return Err(Refused::Unknown) };
        if self.0[i].extension != cb.extension {
            return Err(Refused::OtherExtension { owner: self.0[i].extension.clone() });
        }
        Ok(self.0.remove(i))
    }
}

/// Compare without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// A token endpoint's answer (RFC 6749 §5): JSON, or a form for providers that answer that way.
pub fn parse_token_response(status: u16, body: &[u8], now: u64) -> Result<Token, String> {
    let text = String::from_utf8_lossy(body);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| {
        serde_json::Value::Object(
            text.split('&').filter_map(|kv| kv.split_once('=')).filter_map(|(k, v)| Some((decode(k)?, serde_json::Value::String(decode(v)?)))).collect(),
        )
    });
    let field = |k: &str| json.get(k).and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string())));
    if let Some(e) = field("error") {
        let why = field("error_description").map(|d| format!("{e}: {d}")).unwrap_or(e);
        return Err(crate::i18n::tf("the sign-in was refused ({})", &[&clean(&why, 200)]));
    }
    if !(200..300).contains(&status) {
        return Err(crate::i18n::tf("the token endpoint answered {}", &[&status]));
    }
    let access = field("access_token").filter(|t| !t.is_empty()).ok_or_else(|| crate::i18n::t("the token endpoint sent no access_token").to_string())?;
    if let Some(kind) = field("token_type")
        && !kind.eq_ignore_ascii_case("bearer")
    {
        return Err(crate::i18n::tf("the token endpoint sent a {} token; only bearer tokens can sign requests", &[&clean(&kind, 40)]));
    }
    let expires = field("expires_in").and_then(|s| s.parse::<u64>().ok()).map(|s| now + s);
    let scopes = field("scope").map(|s| s.split([' ', ',']).filter(|s| !s.is_empty()).map(str::to_string).collect()).unwrap_or_default();
    Ok(Token { access, refresh: field("refresh_token"), expires, label: String::new(), scopes })
}

/// The account's name from an account URL's JSON, the first of the usual fields; cleaned for display.
pub fn account_label(body: &[u8]) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    let user = json.get("user").filter(|u| u.is_object()).unwrap_or(&json);
    ["login", "preferred_username", "username", "email", "name", "sub", "id"].iter().find_map(|k| {
        let v = user.get(*k)?;
        let s = v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string()))?;
        let s = clean(&s, LABEL_LEN);
        (!s.is_empty()).then_some(s)
    })
}

/// Text from a server, made safe to show: no control, bidirectional or zero-width characters, at most `max` characters.
pub fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() && !ext_manifest::is_deceptive(*c)).take(max).collect::<String>().trim().to_string()
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The file in the state folder that tells a second Codeyard where to hand a callback.
pub fn callback_port_file(state_dir: &Path) -> PathBuf {
    state_dir.join("ext-callback.port")
}

/// In the Codeyard the operating system started with a `codeyard://` address: hand it to the running one.
pub fn forward_callback(url: &str, port_file: &Path) -> Result<(), String> {
    if parse_callback(url).is_none() {
        return Err(format!("{url} is not a sign-in answer for an extension"));
    }
    let port: u16 = std::fs::read_to_string(port_file).ok().and_then(|s| s.trim().parse().ok()).ok_or("Codeyard is not waiting for a sign-in")?;
    let mut s = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(5)).map_err(|e| e.to_string())?;
    s.write_all(format!("{url}\n").as_bytes()).map_err(|e| e.to_string())
}

/// Listen on a loopback port for addresses from `forward_callback`, and write the port to `port_file`. Each one that
/// parses as a callback arrives on the receiver.
pub fn listen_for_callbacks(port_file: &Path) -> Result<mpsc::Receiver<String>, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    if let Some(dir) = port_file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(port_file, port.to_string()).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("ext-callback".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut line = String::new();
                let _ = BufReader::new(std::io::Read::take(&stream, 8192)).read_line(&mut line);
                let url = line.trim().to_string();
                if parse_callback(&url).is_some() && tx.send(url).is_err() {
                    return;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(rx)
}

/// Linux: tell the desktop that `codeyard://` addresses open this Codeyard (a `.desktop` file in the user's
/// applications folder and `xdg-mime`). macOS reads the scheme from Info.plist and Windows from the installer.
#[cfg(target_os = "linux")]
pub fn register_scheme() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".local/share"))).ok_or("no home folder")?;
    let dir = data.join("applications");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = "codeyard-url-handler.desktop";
    let exec = exe.display().to_string().replace('\\', "\\\\").replace('"', "\\\"");
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=Codeyard\nNoDisplay=true\nExec=\"{exec}\" %u\nMimeType=x-scheme-handler/{SCHEME};\n" // i18n-keep: desktop file
    );
    std::fs::write(dir.join(file), text).map_err(|e| e.to_string())?;
    let ok = std::process::Command::new("xdg-mime").args(["default", file, &format!("x-scheme-handler/{SCHEME}")]).status().is_ok_and(|s| s.success());
    if ok { Ok(()) } else { Err("xdg-mime failed".into()) }
}

#[cfg(test)]
#[path = "ext_accounts_tests.rs"]
mod tests;
