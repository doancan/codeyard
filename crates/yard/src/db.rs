//! The registry's state in SQLite: publisher names, extensions, versions and daily download counts. JSON columns
//! hold the types the index already defines, so the index is these rows, sorted.

use std::collections::BTreeMap;
use std::path::Path;

use ext_core::yard::{Entry, Publisher, Release, Status};
use ext_manifest::{ExtId, Manifest, Version};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS prefixes (
    prefix TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    registered INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS extensions (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    categories TEXT NOT NULL,
    publisher TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS versions (
    id TEXT NOT NULL REFERENCES extensions(id),
    version TEXT NOT NULL,
    release TEXT NOT NULL,
    digest TEXT NOT NULL UNIQUE,
    PRIMARY KEY (id, version)
);
CREATE TABLE IF NOT EXISTS downloads (
    digest TEXT NOT NULL,
    day INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (digest, day)
);
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS target_packages (
    digest TEXT PRIMARY KEY,
    id TEXT NOT NULL,
    version TEXT NOT NULL,
    target TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS reviews (
    id TEXT NOT NULL,
    version TEXT NOT NULL,
    owner TEXT NOT NULL,
    submitted INTEGER NOT NULL,
    state TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (id, version)
);
CREATE TABLE IF NOT EXISTS reviewed_owners (
    owner TEXT PRIMARY KEY,
    approved INTEGER NOT NULL
);
";

/// Where a version stands with the reviewers: only versions with no review row, or an approved one, are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Review {
    Pending,
    Approved,
    Rejected,
}

impl Review {
    fn as_str(self) -> &'static str {
        match self {
            Review::Pending => "pending",
            Review::Approved => "approved",
            Review::Rejected => "rejected",
        }
    }

    fn parse(s: &str) -> Review {
        match s {
            "approved" => Review::Approved,
            "rejected" => Review::Rejected,
            _ => Review::Pending,
        }
    }
}

/// One version in the review queue: (id, version, owner, submitted).
pub type Queued = (ExtId, Version, String, u64);

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // `yard revoke` and `yard immediate` write while `yard serve` runs: wait for its transaction instead of failing.
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    // WAL also lets Litestream copy the database continuously for backups.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("serializes")
}

fn from_json<T: serde::de::DeserializeOwned>(col: usize, text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|e| rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, Box::new(e)))
}

pub fn prefix_owner(tx: &Transaction, prefix: &str) -> rusqlite::Result<Option<String>> {
    tx.query_row("SELECT owner FROM prefixes WHERE prefix = ?1", [prefix], |r| r.get(0)).optional()
}

pub fn prefixes(tx: &Transaction) -> rusqlite::Result<Vec<(String, String)>> {
    let mut st = tx.prepare("SELECT prefix, owner FROM prefixes")?;
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}

pub fn add_prefix(tx: &Transaction, prefix: &str, owner: &str, now: u64) -> rusqlite::Result<()> {
    tx.execute("INSERT INTO prefixes (prefix, owner, registered) VALUES (?1, ?2, ?3)", params![prefix, owner, now as i64]).map(drop)
}

pub fn extension_publisher(tx: &Transaction, id: &ExtId) -> rusqlite::Result<Option<Publisher>> {
    tx.query_row("SELECT publisher FROM extensions WHERE id = ?1", [id.as_str()], |r| from_json(0, &r.get::<_, String>(0)?)).optional()
}

/// (id, name) of every extension another owner publishes.
pub fn extensions_not_owned_by(tx: &Transaction, owner: &str) -> rusqlite::Result<Vec<(String, String)>> {
    let mut st = tx.prepare("SELECT id, name FROM extensions WHERE owner != ?1")?;
    st.query_map([owner], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}

/// Insert a new extension or refresh the text a new version brings (name, description, categories).
pub fn upsert_extension(tx: &Transaction, m: &Manifest, publisher: &Publisher) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO extensions (id, owner, name, description, categories, publisher) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET name = ?3, description = ?4, categories = ?5",
        params![m.id.as_str(), crate::names::owner_key(publisher), m.name, m.description.clone().unwrap_or_default(), json(&m.categories), json(publisher)],
    )
    .map(drop)
}

pub fn version_exists(tx: &Transaction, id: &ExtId, version: &Version) -> rusqlite::Result<bool> {
    tx.query_row("SELECT 1 FROM versions WHERE id = ?1 AND version = ?2", [id.as_str(), &version.to_string()], |_| Ok(())).optional().map(|r| r.is_some())
}

pub fn add_version(tx: &Transaction, id: &ExtId, r: &Release) -> rusqlite::Result<()> {
    tx.execute("INSERT INTO versions (id, version, release, digest) VALUES (?1, ?2, ?3, ?4)", params![id.as_str(), r.version.to_string(), json(r), r.digest])
        .map(drop)
}

fn release(tx: &Transaction, id: &ExtId, version: &Version) -> rusqlite::Result<Option<Release>> {
    tx.query_row("SELECT release FROM versions WHERE id = ?1 AND version = ?2", [id.as_str(), &version.to_string()], |r| from_json(0, &r.get::<_, String>(0)?))
        .optional()
}

pub fn put_release(tx: &Transaction, id: &ExtId, r: &Release) -> rusqlite::Result<()> {
    tx.execute("UPDATE versions SET release = ?3 WHERE id = ?1 AND version = ?2", params![id.as_str(), r.version.to_string(), json(r)]).map(drop)
}

/// Every version whose release has no details digest yet.
pub fn releases_without_details(tx: &Transaction) -> rusqlite::Result<Vec<(ExtId, Release)>> {
    let mut st = tx.prepare("SELECT id, release FROM versions")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, from_json::<Release>(1, &r.get::<_, String>(1)?)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, release) = row?;
        if release.details.is_none() {
            let id = ExtId::parse(&id).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into()))?;
            out.push((id, release));
        }
    }
    Ok(out)
}

pub fn status(tx: &Transaction, id: &ExtId, version: &Version) -> rusqlite::Result<Option<Status>> {
    Ok(release(tx, id, version)?.map(|r| r.status))
}

pub fn set_status(tx: &Transaction, id: &ExtId, version: &Version, status: &Status) -> rusqlite::Result<()> {
    let Some(mut r) = release(tx, id, version)? else { return Ok(()) };
    r.status = status.clone();
    put_release(tx, id, &r)
}

pub fn set_immediate(tx: &Transaction, id: &ExtId, version: &Version) -> rusqlite::Result<()> {
    let Some(mut r) = release(tx, id, version)? else { return Ok(()) };
    r.immediate = true;
    put_release(tx, id, &r)
}

/// Count a download; false when no listed version has this digest (as its package or one platform's).
pub fn count_download(conn: &Connection, digest: &str, day: u64) -> rusqlite::Result<bool> {
    let owner = conn
        .query_row("SELECT id, version FROM versions WHERE digest = ?1 UNION SELECT id, version FROM target_packages WHERE digest = ?1", [digest], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .optional()?;
    let Some((id, version)) = owner else { return Ok(false) };
    let review: Option<String> = conn.query_row("SELECT state FROM reviews WHERE id = ?1 AND version = ?2", [&id, &version], |r| r.get(0)).optional()?;
    if review.is_some_and(|s| Review::parse(&s) != Review::Approved) {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO downloads (digest, day, count) VALUES (?1, ?2, 1) ON CONFLICT(digest, day) DO UPDATE SET count = count + 1",
        params![digest, day as i64],
    )?;
    Ok(true)
}

/// The counter every signed index carries; it only goes up.
pub fn next_index_version(tx: &Transaction) -> rusqlite::Result<u64> {
    tx.execute("INSERT INTO meta (key, value) VALUES ('index_version', 1) ON CONFLICT(key) DO UPDATE SET value = value + 1", [])?;
    tx.query_row("SELECT value FROM meta WHERE key = 'index_version'", [], |r| r.get::<_, i64>(0)).map(|v| v as u64)
}

/// Every extension as the index lists it, versions oldest first, with their download totals.
pub fn entries(tx: &Transaction) -> rusqlite::Result<BTreeMap<ExtId, Entry>> {
    let mut downloads = BTreeMap::<String, u64>::new();
    let mut st = tx.prepare("SELECT digest, SUM(count) FROM downloads GROUP BY digest")?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (d, n) = row?;
        downloads.insert(d, n as u64);
    }
    let mut out = BTreeMap::new();
    let mut st = tx.prepare("SELECT id, name, description, categories, publisher FROM extensions")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            from_json::<Vec<String>>(3, &r.get::<_, String>(3)?)?,
            from_json::<Publisher>(4, &r.get::<_, String>(4)?)?,
        ))
    })?;
    for row in rows {
        let (id, name, description, categories, publisher) = row?;
        let id = ExtId::parse(&id).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into()))?;
        out.insert(id, Entry { name, description, categories, publisher, versions: Vec::new() });
    }
    // Versions waiting for a reviewer, or rejected, are not listed.
    let mut st = tx.prepare(
        "SELECT v.id, v.release FROM versions v LEFT JOIN reviews r ON r.id = v.id AND r.version = v.version
         WHERE r.state IS NULL OR r.state = 'approved'",
    )?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, from_json::<Release>(1, &r.get::<_, String>(1)?)?)))? {
        let (id, mut release) = row?;
        release.downloads = downloads.get(&release.digest).copied().unwrap_or(0);
        release.downloads +=
            release.targets.values().filter(|t| t.digest != release.digest).map(|t| downloads.get(&t.digest).copied().unwrap_or(0)).sum::<u64>();
        if let Some(e) = ExtId::parse(&id).ok().and_then(|id| out.get_mut(&id)) {
            e.versions.push(release);
        }
    }
    // An extension whose only version waits for review is not listed yet.
    out.retain(|_, e| !e.versions.is_empty());
    for e in out.values_mut() {
        e.versions.sort_by(|a, b| a.version.cmp(&b.version));
    }
    Ok(out)
}

pub fn release_of(tx: &Transaction, id: &ExtId, version: &Version) -> rusqlite::Result<Option<Release>> {
    release(tx, id, version)
}

/// Record one platform's package of a version, so its downloads count.
pub fn add_target_package(tx: &Transaction, id: &ExtId, version: &Version, target: &str, digest: &str) -> rusqlite::Result<()> {
    tx.execute("INSERT INTO target_packages (digest, id, version, target) VALUES (?1, ?2, ?3, ?4)", params![digest, id.as_str(), version.to_string(), target])
        .map(drop)
}

/// A publisher (by owner key) whose first native program a reviewer approved.
pub fn owner_reviewed(tx: &Transaction, owner: &str) -> rusqlite::Result<bool> {
    tx.query_row("SELECT 1 FROM reviewed_owners WHERE owner = ?1", [owner], |_| Ok(())).optional().map(|r| r.is_some())
}

pub fn set_owner_reviewed(tx: &Transaction, owner: &str, now: u64) -> rusqlite::Result<()> {
    tx.execute("INSERT OR IGNORE INTO reviewed_owners (owner, approved) VALUES (?1, ?2)", params![owner, now as i64]).map(drop)
}

pub fn review(tx: &Transaction, id: &ExtId, version: &Version) -> rusqlite::Result<Option<Review>> {
    tx.query_row("SELECT state FROM reviews WHERE id = ?1 AND version = ?2", [id.as_str(), &version.to_string()], |r| r.get::<_, String>(0))
        .optional()
        .map(|s| s.map(|s| Review::parse(&s)))
}

pub fn queue_review(tx: &Transaction, id: &ExtId, version: &Version, owner: &str, now: u64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO reviews (id, version, owner, submitted, state) VALUES (?1, ?2, ?3, ?4, 'pending')",
        params![id.as_str(), version.to_string(), owner, now as i64],
    )
    .map(drop)
}

pub fn set_review(tx: &Transaction, id: &ExtId, version: &Version, state: Review, note: &str) -> rusqlite::Result<()> {
    tx.execute("UPDATE reviews SET state = ?3, note = ?4 WHERE id = ?1 AND version = ?2", params![id.as_str(), version.to_string(), state.as_str(), note])
        .map(drop)
}

/// Versions waiting for a reviewer, oldest first.
pub fn pending_reviews(tx: &Transaction) -> rusqlite::Result<Vec<Queued>> {
    let mut st = tx.prepare("SELECT id, version, owner, submitted FROM reviews WHERE state = 'pending' ORDER BY submitted, id, version")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?;
    let bad = |e: String| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into());
    let mut out = Vec::new();
    for row in rows {
        let (id, version, owner, at) = row?;
        out.push((ExtId::parse(&id).map_err(bad)?, Version::parse(&version).map_err(bad)?, owner, at as u64));
    }
    Ok(out)
}
