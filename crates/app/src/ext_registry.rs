//! Installing from a registry (docs/registry.md, #67). The checks live in `ext_core::registry`; this file brings the
//! network (curl, like the app updater, so proxies and the system's certificates apply), the list of registries, and
//! one call each for the command line and the Extensions view.
//!
//! Privacy: nothing here runs unless the user asks for an install or opens the Market, or an extension from a registry
//! is installed (then that registry's index is fetched at most once a day). A user with no registry extension who never
//! opens the Market makes no request. Searching the Market asks nothing; opening an extension's page fetches its
//! details file once (cached after).

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ext_core::publisher::Pin;
use ext_core::registry::{self, Download, Fetch, Got, Registry};

/// The public registry. Its root keys come from the key ceremony (#70); until then only registries in
/// `extensions.registries` are used.
pub const PUBLIC: Option<(&str, &str, &[&str], usize)> = None;

/// Every registry this user installs from: the public one first, then their own, in settings order.
pub fn registries(settings: &crate::settings::Settings) -> Vec<Registry> {
    let mut out = Vec::new();
    if let Some((name, url, keys, threshold)) = PUBLIC {
        let keys = keys.iter().filter_map(|k| ext_core::yard::PublicKey::parse(k)).collect();
        out.push(Registry { name: name.into(), url: url.into(), keys: ext_core::yard::RootKeys { keys, threshold } });
    }
    out.extend(settings.extension_registries.iter().cloned());
    out
}

pub fn cache_root(cfg_dir: &Path) -> PathBuf {
    crate::ext::data_root(cfg_dir).join("registries")
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `curl`, as the updater runs it: https only (http only to this machine), bounded size and time.
struct Curl;

impl Fetch for Curl {
    fn get(&self, url: &str, etag: Option<&str>, limit: usize) -> Result<Got, String> {
        let local = url.starts_with("http://localhost") || url.starts_with("http://127.0.0.1");
        let dir = tempdir()?;
        let (headers, body) = (dir.join("headers"), dir.join("body"));
        let mut c = Command::new("curl");
        c.args(["-q", "--silent", "--show-error", "--location", "--max-redirs", "5", "--connect-timeout", "15", "--max-time", "120", "--tlsv1.2"]);
        c.args(["--proto", if local { "=https,http" } else { "=https" }, "--proto-redir", if local { "=https,http" } else { "=https" }]);
        c.args(["--max-filesize", &limit.to_string(), "--dump-header"]).arg(&headers).arg("--output").arg(&body);
        if let Some(tag) = etag {
            c.args(["--header", &format!("If-None-Match: {tag}")]);
        }
        c.args(["--url", url]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let out = c.output().map_err(|e| format!("cannot run curl: {e}"))?;
        let result = (|| {
            if !out.status.success() {
                return Err(format!("{url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
            let head = std::fs::read_to_string(&headers).unwrap_or_default();
            // After redirects the last response counts.
            let last = head.rsplit("HTTP/").next().unwrap_or_default();
            let status: u16 = last.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let etag = last.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("etag")).map(|(_, v)| v.trim().to_string()));
            match status {
                304 => Ok(Got::NotModified),
                200..=299 => {
                    let mut bytes = Vec::new();
                    std::fs::File::open(&body).and_then(|f| f.take(limit as u64 + 1).read_to_end(&mut bytes)).map_err(|e| e.to_string())?;
                    if bytes.len() > limit {
                        return Err(format!("{url}: larger than {limit} bytes"));
                    }
                    Ok(Got::Body { bytes, etag })
                }
                s => Err(format!("{url}: the server answered {s}")),
            }
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }
}

fn tempdir() -> Result<PathBuf, String> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("codeyard-fetch-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    Ok(d)
}

/// Tests answer from folders instead of the network (`ext_core::testing::DirFetch`), one per test registry, so tests
/// running side by side never see each other's; in tests nothing reaches the network.
#[cfg(test)]
static TEST_FETCH: std::sync::Mutex<Vec<std::sync::Arc<dyn Fetch + Send + Sync>>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub fn add_fetch_for_test(f: std::sync::Arc<dyn Fetch + Send + Sync>) {
    TEST_FETCH.lock().unwrap_or_else(|e| e.into_inner()).push(f);
}

/// Addresses asked for that no test registry serves (an image in a README, say): tests check theirs are not here.
#[cfg(test)]
pub static UNROUTED_FOR_TEST: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
struct TestRoute;

#[cfg(test)]
impl Fetch for TestRoute {
    fn get(&self, url: &str, etag: Option<&str>, limit: usize) -> Result<Got, String> {
        let all = TEST_FETCH.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for f in all {
            match f.get(url, etag, limit) {
                Err(e) if e.ends_with(ext_core::testing::NOT_THIS_REGISTRY) => continue,
                r => return r,
            }
        }
        UNROUTED_FOR_TEST.lock().unwrap_or_else(|e| e.into_inner()).push(url.to_string());
        Err(format!("{url}: no test registry answers it"))
    }
}

fn with_fetch<T>(f: impl FnOnce(&dyn Fetch) -> T) -> T {
    #[cfg(test)]
    return f(&TestRoute);
    #[cfg(not(test))]
    f(&Curl)
}

/// A package found in a registry and checked, with what the consent sheet must say about it.
pub struct Found {
    pub registry: Registry,
    pub download: Download,
    pub pin: Pin,
    /// The index is past its 72 h: the user confirms before installing.
    pub stale: bool,
}

/// Find `spec` (`publisher.name[@version]`) in the user's registries, download and check it. The daily cached index
/// is used first; a registry is asked again only if it does not list the id yet.
pub fn find(cfg_dir: &Path, registries: &[Registry], spec: &str, now: u64) -> Result<Found, String> {
    let (id, version) = registry::parse_spec(spec)?;
    if registries.is_empty() {
        return Err(crate::i18n::t("No extension registry is set up yet. Add one to extensions.registries in your settings, or install a .cyx file.").into());
    }
    let pre = version.as_ref().is_some_and(|v| v.pre.is_some());
    let locked = crate::ext::store(cfg_dir).locked().unwrap_or_default();
    let mut errors = Vec::new();
    for reg in registries {
        let dir = registry::cache_dir(&cache_root(cfg_dir), reg);
        let listed = |force: bool| with_fetch(|f| registry::index(reg, &dir, f, now, force)).map(|v| v.index.extensions.contains_key(&id).then_some(v));
        let before = registry::fetched_at(&dir);
        let v = match listed(false) {
            Ok(Some(v)) => v,
            // Not in the copy from earlier today (it may be newer than that): ask the registry this once.
            Ok(None) if before.is_some() && registry::fetched_at(&dir) == before => match listed(true) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(e) => {
                    errors.push(format!("{}: {e}", reg.name));
                    continue;
                }
            },
            Ok(None) => continue,
            Err(e) => {
                errors.push(format!("{}: {e}", reg.name));
                continue;
            }
        };
        let stale = v.stale;
        let download = with_fetch(|f| registry::download(reg, &v, &id, version.as_ref(), pre, f, now)).map_err(|e| e.to_string())?;
        let pin = registry::pin(locked.get(&id), &reg.name, &download.signed.publisher);
        return Ok(Found { registry: reg.clone(), download, pin, stale });
    }
    Err(match errors.is_empty() {
        true => crate::i18n::tf("{} is not in any of your registries", &[&id]),
        false => errors.join("\n"),
    })
}

/// Every registry's index for the Market: the cached copy when fetched within the day, else fetched now. A registry
/// that cannot be read is reported and the others are listed.
pub fn market(cfg_dir: &Path, registries: &[Registry], now: u64) -> (Vec<(Registry, ext_core::yard::Verified)>, Vec<String>) {
    let (mut listed, mut errors) = (Vec::new(), Vec::new());
    for reg in registries {
        let dir = registry::cache_dir(&cache_root(cfg_dir), reg);
        match with_fetch(|f| registry::index(reg, &dir, f, now, false)) {
            Ok(v) => listed.push((reg.clone(), v)),
            Err(e) => errors.push(format!("{}: {e}", reg.name)),
        }
    }
    (listed, errors)
}

/// The Market page of one version of `id` in `reg` (manifest and README), checked against its index.
pub fn page(cfg_dir: &Path, reg: &Registry, id: &ext_manifest::ExtId, release: &ext_core::yard::Release) -> Result<registry::Page, String> {
    let dir = registry::cache_dir(&cache_root(cfg_dir), reg);
    with_fetch(|f| registry::page(reg, &dir, id, release, f)).map_err(|e| e.to_string())
}

/// Install what [`find`] returned (after the user's consent), pinning its publisher.
pub fn install(cfg_dir: &Path, found: &Found) -> Result<ext_core::Installed, String> {
    if found.download.manifest.code.is_some() {
        return Err(crate::i18n::tf("{} contains code, which this version does not run", &[&found.download.manifest.id]));
    }
    let source = ext_core::Source::Registry { registry: found.registry.name.clone(), publisher: found.download.signed.publisher.clone() };
    crate::ext::store(cfg_dir).install(&found.download.bytes, source).map_err(|e| e.to_string())
}

/// `codeyard --install-extension publisher.name[@version]`: running it is the consent, but a changed publisher or a
/// stale index needs the Extensions view, where the user sees why.
pub fn install_from_command_line(cfg_dir: &Path, registries: &[Registry], spec: &str) -> Result<ext_manifest::Manifest, String> {
    let found = find(cfg_dir, registries, spec, now())?;
    if let Pin::Changed { was } = &found.pin {
        return Err(crate::i18n::tf(
            "{0} is now published by {1}, not {2} as before; review it with Extensions: Install from Registry…",
            &[&found.download.manifest.id, &found.download.signed.publisher, was],
        ));
    }
    if found.stale {
        return Err(crate::i18n::tf("the {} registry's list is more than 3 days old; install from the Extensions view to confirm", &[&found.registry.name]));
    }
    install(cfg_dir, &found).map(|i| i.manifest)
}

/// Keep the indexes of the registries the user installed from fresh (daily). Nothing is asked of a registry that no
/// installed extension came from. Errors wait for the next day.
pub fn refresh_installed(cfg_dir: &Path, registries: &[Registry], now: u64) {
    let locked = crate::ext::store(cfg_dir).locked().unwrap_or_default();
    for reg in registries {
        if locked.values().any(|l| l.source.registry().is_some_and(|(r, _)| r == reg.name)) {
            let dir = registry::cache_dir(&cache_root(cfg_dir), reg);
            let _ = with_fetch(|f| registry::index(reg, &dir, f, now, false));
        }
    }
}

#[cfg(test)]
mod curl_tests {
    use std::io::{BufRead as _, BufReader, Write as _};

    use super::*;

    /// A one-file HTTP server: `/index.json` with an ETag (304 when it matches), a redirect to it, 404 otherwise.
    fn serve() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split(' ').nth(1).unwrap_or_default().to_string();
                let mut tag = None;
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("if-none-match:") {
                        tag = Some(v.trim().to_string());
                    }
                }
                let mut s = stream;
                let _ = match (path.as_str(), tag.as_deref()) {
                    ("/moved", _) => write!(s, "HTTP/1.1 301 Moved\r\nLocation: /index.json\r\nContent-Length: 0\r\n\r\n"),
                    ("/index.json", Some("\"v1\"")) => write!(s, "HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\n\r\n"),
                    ("/index.json", _) => write!(s, "HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nContent-Length: 5\r\n\r\nhello"),
                    _ => write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"),
                };
            }
        });
        base
    }

    #[test]
    fn curl_reads_bodies_etags_and_not_modified() {
        if Command::new("curl").arg("--version").output().is_err() {
            return crate::test_sshd::skip_missing("curl");
        }
        let base = serve();
        let Got::Body { bytes, etag } = Curl.get(&format!("{base}/index.json"), None, 100).unwrap() else { panic!("a body") };
        assert_eq!((bytes.as_slice(), etag.as_deref()), (&b"hello"[..], Some("\"v1\"")));
        assert!(matches!(Curl.get(&format!("{base}/index.json"), Some("\"v1\""), 100).unwrap(), Got::NotModified));
        // After a redirect the last response's headers count.
        assert!(matches!(Curl.get(&format!("{base}/moved"), None, 100).unwrap(), Got::Body { etag: Some(_), .. }));
        assert!(Curl.get(&format!("{base}/missing"), None, 100).unwrap_err().contains("404"));
        assert!(Curl.get(&format!("{base}/index.json"), None, 3).is_err(), "over the size limit");
        // Plain http is for this machine only.
        assert!(Curl.get("http://example.com/index.json", None, 100).is_err());
    }
}
