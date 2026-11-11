//! Looking crates up: the sparse index for versions and features, the crates.io API for search and for a crate's description.
//! Every lookup goes through one `Registry`: it answers from memory, then from the disk cache while the entry is fresh,
//! asks the server (conditionally) otherwise, and falls back to an old entry when the network is down. At most
//! `MAX_PARALLEL` requests run at once, API requests are at least a second apart (crates.io's crawler policy), and after a
//! network failure nothing is tried again for `OFFLINE_PAUSE`.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::cache::{DiskCache, Stored, now_secs};
use super::fetch::{Fetch, FetchError};
use super::index::{self, CrateInfo};

pub const INDEX_URL: &str = "https://index.crates.io";
pub const API_URL: &str = "https://crates.io/api/v1";
/// A cached index entry younger than this is used without asking the server.
pub const INDEX_FRESH: Duration = Duration::from_secs(3 * 3600);
/// A crate the index does not know is not asked for again for this long.
pub const MISSING_FRESH: Duration = Duration::from_secs(3600);
pub const META_FRESH: Duration = Duration::from_secs(7 * 24 * 3600);
pub const SEARCH_FRESH: Duration = Duration::from_secs(10 * 60);
pub const MAX_PARALLEL: usize = 4;
pub const API_SPACING: Duration = Duration::from_secs(1);
pub const OFFLINE_PAUSE: Duration = Duration::from_secs(20);
const SEARCH_PAGE: usize = 20;

/// Where lookups go. `api: None` (a mirror replaces crates.io) means no search and no descriptions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Endpoints {
    pub index: String,
    pub api: Option<String>,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self { index: INDEX_URL.into(), api: Some(API_URL.into()) }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lookup {
    /// `stale`: the entry is older than its time to live and the server could not be asked.
    Found {
        info: Arc<CrateInfo>,
        stale: bool,
    },
    NotFound,
    /// The network is unreachable and nothing is cached.
    Offline,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub name: String,
    pub version: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrateMeta {
    pub name: String,
    pub description: Option<String>,
    pub license: Option<String>,
    pub version: String,
    pub documentation: String,
    pub repository: Option<String>,
    pub homepage: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Offline,
    /// The setting is off, or the endpoint is not available for this registry.
    Unavailable,
    Other(String),
}

impl From<FetchError> for Failure {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::Offline => Self::Offline,
            FetchError::Timeout => Self::Other("the request timed out".into()),
            FetchError::Other(m) => Self::Other(m),
        }
    }
}

/// Crates.io names: ASCII letters, digits, `-` and `_`, at most 64 characters. Anything else never reaches a URL.
pub fn valid_crate_name(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn encode_query(q: &str) -> String {
    q.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// A counting semaphore: how many requests run at once.
struct Gate {
    free: Mutex<usize>,
    wake: Condvar,
}

struct Permit<'a>(&'a Gate);

impl Gate {
    fn enter(&self) -> Permit<'_> {
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        while *free == 0 {
            free = self.wake.wait(free).unwrap_or_else(|e| e.into_inner());
        }
        *free -= 1;
        Permit(self)
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.free.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.0.wake.notify_one();
    }
}

type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

struct Memory {
    /// `None`: the index has no such crate.
    info: Option<Arc<CrateInfo>>,
    fetched: u64,
    etag: Option<String>,
}

pub struct Registry {
    fetch: Arc<dyn Fetch>,
    disk: DiskCache,
    endpoints: Endpoints,
    clock: Clock,
    memory: Mutex<HashMap<String, Memory>>,
    searches: Mutex<HashMap<String, (u64, Vec<SearchHit>)>>,
    gate: Gate,
    /// When the next API request may start.
    api_next: Mutex<Instant>,
    offline_until: Mutex<Option<Instant>>,
}

impl Registry {
    pub fn new(fetch: Arc<dyn Fetch>, disk: DiskCache, endpoints: Endpoints) -> Arc<Self> {
        Self::with_clock(fetch, disk, endpoints, Arc::new(now_secs))
    }

    pub fn with_clock(fetch: Arc<dyn Fetch>, disk: DiskCache, endpoints: Endpoints, clock: Clock) -> Arc<Self> {
        Arc::new(Self {
            fetch,
            disk,
            endpoints,
            clock,
            memory: Mutex::default(),
            searches: Mutex::default(),
            gate: Gate { free: Mutex::new(MAX_PARALLEL), wake: Condvar::new() },
            api_next: Mutex::new(Instant::now()),
            offline_until: Mutex::new(None),
        })
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One request, counted against the parallel limit and the offline pause.
    fn request(&self, url: &str, etag: Option<&str>, api: bool) -> Result<super::fetch::Response, FetchError> {
        if Self::lock(&self.offline_until).is_some_and(|t| Instant::now() < t) {
            return Err(FetchError::Offline);
        }
        let _permit = self.gate.enter();
        if api {
            let mut next = Self::lock(&self.api_next);
            let wait = next.saturating_duration_since(Instant::now());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
            *next = Instant::now() + API_SPACING;
        }
        let result = self.fetch.get(url, etag);
        if matches!(result, Err(FetchError::Offline | FetchError::Timeout)) {
            *Self::lock(&self.offline_until) = Some(Instant::now() + OFFLINE_PAUSE);
        }
        result
    }

    fn entry(&self, key: &str) -> Option<Memory> {
        if let Some(m) = Self::lock(&self.memory).get(key) {
            return Some(Memory { info: m.info.clone(), fetched: m.fetched, etag: m.etag.clone() });
        }
        let stored = self.disk.read("index", key)?;
        let info = if stored.payload["missing"].as_bool() == Some(true) { None } else { Some(Arc::new(index::from_json(&stored.payload)?)) };
        let m = Memory { info, fetched: stored.fetched, etag: stored.etag };
        Self::lock(&self.memory).insert(key.to_string(), Memory { info: m.info.clone(), fetched: m.fetched, etag: m.etag.clone() });
        Some(m)
    }

    fn remember(&self, key: &str, info: Option<Arc<CrateInfo>>, etag: Option<String>) {
        let fetched = self.now();
        let payload = info.as_deref().map_or_else(|| json!({ "missing": true }), index::to_json);
        self.disk.write("index", key, &Stored { etag: etag.clone(), fetched, payload });
        Self::lock(&self.memory).insert(key.to_string(), Memory { info, fetched, etag });
    }

    /// Versions and features of `name`. Blocks on the network when the entry is missing or old: call it off the UI thread.
    pub fn crate_info(&self, name: &str) -> Lookup {
        self.crate_info_spelled(name, true)
    }

    /// `try_variant`: on a 404, ask once more with `-` and `_` swapped.
    fn crate_info_spelled(&self, name: &str, try_variant: bool) -> Lookup {
        if !valid_crate_name(name) {
            return Lookup::NotFound;
        }
        let key = name.to_ascii_lowercase();
        let old = self.entry(&key);
        if let Some(m) = &old {
            let fresh = if m.info.is_some() { INDEX_FRESH } else { MISSING_FRESH };
            if self.now().saturating_sub(m.fetched) <= fresh.as_secs() {
                return Self::found(m);
            }
        }
        let etag = old.as_ref().and_then(|m| m.etag.clone()).filter(|_| old.as_ref().is_some_and(|m| m.info.is_some()));
        let url = format!("{}/{}", self.endpoints.index.trim_end_matches('/'), index::index_path(&key));
        match self.request(&url, etag.as_deref(), false) {
            Ok(r) if r.status == 200 => {
                let info = Arc::new(index::parse_index(&key, &String::from_utf8_lossy(&r.body)));
                self.remember(&key, Some(info.clone()), r.etag);
                Lookup::Found { info, stale: false }
            }
            Ok(r) if r.status == 304 => match old {
                Some(m) => {
                    self.remember(&key, m.info.clone(), m.etag.clone());
                    Self::found(&Memory { fetched: self.now(), ..m })
                }
                None => Lookup::Failed("the server answered 304 without a cached copy".into()),
            },
            Ok(r) if matches!(r.status, 404 | 410) => match Self::spelling_variant(&key).filter(|_| try_variant && old.is_none()) {
                Some(other) => self.variant(&key, &other),
                _ => {
                    self.remember(&key, None, None);
                    Lookup::NotFound
                }
            },
            Ok(r) => self.fallback(old, Failure::Other(format!("the index answered {}", r.status))),
            Err(e) => self.fallback(old, e.into()),
        }
    }

    fn found(m: &Memory) -> Lookup {
        match &m.info {
            Some(info) => Lookup::Found { info: info.clone(), stale: false },
            None => Lookup::NotFound,
        }
    }

    /// The old entry when the server could not be reached (marked stale), else the reason.
    fn fallback(&self, old: Option<Memory>, why: Failure) -> Lookup {
        match old.and_then(|m| m.info) {
            Some(info) => Lookup::Found { info, stale: true },
            None => match why {
                Failure::Offline => Lookup::Offline,
                Failure::Unavailable => Lookup::Failed("lookups are off".into()),
                Failure::Other(m) => Lookup::Failed(m),
            },
        }
    }

    /// Cargo treats `-` and `_` alike when it looks a crate up; the index file has the published spelling.
    fn spelling_variant(key: &str) -> Option<String> {
        let other: String = key
            .chars()
            .map(|c| match c {
                '-' => '_',
                '_' => '-',
                c => c,
            })
            .collect();
        (other != key).then_some(other)
    }

    fn variant(&self, key: &str, other: &str) -> Lookup {
        match self.crate_info_spelled(other, false) {
            Lookup::NotFound => {
                self.remember(key, None, None);
                Lookup::NotFound
            }
            found => {
                // The spelling that was asked answers from now on without the second request.
                if let Lookup::Found { info, .. } = &found {
                    self.remember(key, Some(info.clone()), None);
                }
                found
            }
        }
    }

    /// Look up many crates with at most `MAX_PARALLEL` requests at a time; the answers come in the order asked.
    pub fn lookup_many(&self, names: &[String]) -> Vec<(String, Lookup)> {
        let next = Mutex::new(0usize);
        let answers: Mutex<Vec<Option<Lookup>>> = Mutex::new(vec![None; names.len()]);
        std::thread::scope(|scope| {
            for _ in 0..MAX_PARALLEL.min(names.len()) {
                scope.spawn(|| {
                    loop {
                        let i = {
                            let mut n = Self::lock(&next);
                            let i = *n;
                            *n += 1;
                            i
                        };
                        let Some(name) = names.get(i) else { break };
                        let found = self.crate_info(name);
                        Self::lock(&answers)[i] = Some(found);
                    }
                });
            }
        });
        let answers = answers.into_inner().unwrap_or_else(|e| e.into_inner());
        names.iter().cloned().zip(answers.into_iter().map(|a| a.unwrap_or(Lookup::Failed("not looked up".into())))).collect()
    }

    fn api(&self) -> Result<&str, Failure> {
        self.endpoints.api.as_deref().map(|a| a.trim_end_matches('/')).ok_or(Failure::Unavailable)
    }

    /// Crates matching `query` (relevance order). Blocks: call it off the UI thread.
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>, Failure> {
        let query = query.trim();
        if query.is_empty() || query.len() > 64 {
            return Ok(Vec::new());
        }
        let key = query.to_lowercase();
        if let Some((at, hits)) = Self::lock(&self.searches).get(&key)
            && self.now().saturating_sub(*at) <= SEARCH_FRESH.as_secs()
        {
            return Ok(hits.clone());
        }
        let url = format!("{}/crates?q={}&per_page={SEARCH_PAGE}", self.api()?, encode_query(query));
        let r = self.request(&url, None, true)?;
        if r.status != 200 {
            return Err(Failure::Other(format!("crates.io answered {}", r.status)));
        }
        let hits = parse_search(&String::from_utf8_lossy(&r.body));
        Self::lock(&self.searches).insert(key, (self.now(), hits.clone()));
        Ok(hits)
    }

    /// The description, licence and links of a crate, cached for a week. Blocks.
    pub fn meta(&self, name: &str) -> Result<CrateMeta, Failure> {
        if !valid_crate_name(name) {
            return Err(Failure::Other("not a crate name".into()));
        }
        let key = name.to_ascii_lowercase();
        if let Some(s) = self.disk.read("meta", &key)
            && self.now().saturating_sub(s.fetched) <= META_FRESH.as_secs()
            && let Some(meta) = meta_from_json(&s.payload)
        {
            return Ok(meta);
        }
        let api = self.api()?.to_string();
        let r = self.request(&format!("{api}/crates/{key}?include="), None, true)?;
        if r.status != 200 {
            return Err(Failure::Other(format!("crates.io answered {}", r.status)));
        }
        let body: Value = serde_json::from_slice(&r.body).map_err(|e| Failure::Other(e.to_string()))?;
        let version = body["crate"]["max_stable_version"].as_str().or_else(|| body["crate"]["default_version"].as_str()).unwrap_or_default().to_string();
        // The licence belongs to a version, so it is one more request; without it the rest still shows.
        let license = (!version.is_empty())
            .then(|| self.request(&format!("{api}/crates/{key}/{version}"), None, true).ok())
            .flatten()
            .filter(|r| r.status == 200)
            .and_then(|r| serde_json::from_slice::<Value>(&r.body).ok())
            .and_then(|v| v["version"]["license"].as_str().map(str::to_string));
        let meta = meta_from_crate(&body["crate"], license).ok_or_else(|| Failure::Other("unexpected answer".into()))?;
        self.disk.write("meta", &key, &Stored { etag: None, fetched: self.now(), payload: meta_to_json(&meta) });
        Ok(meta)
    }
}

pub fn parse_search(body: &str) -> Vec<SearchHit> {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    v["crates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let name = c["name"].as_str().filter(|n| valid_crate_name(n))?.to_string();
            let version = c["max_stable_version"].as_str().or_else(|| c["max_version"].as_str()).or_else(|| c["newest_version"].as_str())?.to_string();
            Some(SearchHit { name, version, description: c["description"].as_str().unwrap_or_default().trim().to_string() })
        })
        .collect()
}

fn meta_from_crate(c: &Value, license: Option<String>) -> Option<CrateMeta> {
    let name = c["name"].as_str()?.to_string();
    let version = c["max_stable_version"].as_str().or_else(|| c["default_version"].as_str()).unwrap_or_default().to_string();
    let text = |k: &str| c[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    Some(CrateMeta {
        documentation: text("documentation").unwrap_or_else(|| format!("https://docs.rs/{name}")),
        description: text("description"),
        repository: text("repository"),
        homepage: text("homepage"),
        license,
        version,
        name,
    })
}

fn meta_to_json(m: &CrateMeta) -> Value {
    json!({ "name": m.name, "description": m.description, "license": m.license, "version": m.version,
            "documentation": m.documentation, "repository": m.repository, "homepage": m.homepage })
}

fn meta_from_json(v: &Value) -> Option<CrateMeta> {
    let text = |k: &str| v[k].as_str().map(str::to_string);
    Some(CrateMeta {
        name: text("name")?,
        description: text("description"),
        license: text("license"),
        version: text("version")?,
        documentation: text("documentation")?,
        repository: text("repository"),
        homepage: text("homepage"),
    })
}
