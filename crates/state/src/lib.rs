//! Local state: recently opened folders, hosts, session. SQLite, WAL.
//! `schema.sql` at the crate root is the single source of the schema.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

pub use rusqlite::Error;

const SCHEMA: &str = include_str!("../schema.sql");

/// schema.sql = version 1. Every later change is appended here and runs once, in order.
const MIGRATIONS: &[(i64, &str)] = &[
    (
        2,
        "create table if not exists kv (
           key        text primary key,
           value      text not null,
           updated_at integer not null
         );",
    ),
    // One backup per file per workspace.
    // (With duplicate rows the index can't be built and the app would hang on every launch: the newest row is kept.)
    (
        3,
        "delete from backups where id not in (select max(id) from backups group by workspace_id, coalesce(rel_path, '#' || id));
         create unique index if not exists backups_ws_rel on backups (workspace_id, rel_path);",
    ),
    // Favorite files/folders and line bookmarks (per project, path relative to the root).
    (
        4,
        "create table if not exists favorites (
           workspace_id integer not null references workspaces(id) on delete cascade,
           rel_path     text not null,
           created_at   integer not null,
           primary key (workspace_id, rel_path)
         );
         create table if not exists bookmarks (
           workspace_id integer not null references workspaces(id) on delete cascade,
           rel_path     text not null,
           line         integer not null,
           label        text,
           created_at   integer not null,
           primary key (workspace_id, rel_path, line)
         );",
    ),
    // "Remove from recents" hides the record instead of deleting it: unsaved-change backups, tabs and favorites must not go away.
    (5, "alter table workspaces add column hidden integer not null default 0;"),
    // Ports the user forwarded in a project (restored when the project opens again) with their label and protocol.
    (
        6,
        "create table if not exists port_forwards (
           workspace_id integer not null references workspaces(id) on delete cascade,
           remote_port  integer not null check (remote_port between 1 and 65535),
           local_port   integer check (local_port is null or local_port between 1 and 65535),
           label        text,
           protocol     text not null default 'http' check (protocol in ('http', 'https', 'tcp')),
           updated_at   integer not null,
           primary key (workspace_id, remote_port)
         );",
    ),
];
const SCHEMA_VERSION: i64 = 6;

const DAY_MS: i64 = 86_400_000;
const FILE_STATE_MAX_AGE_DAYS: i64 = 90;
const FILE_STATE_PER_WORKSPACE: i64 = 500;
const HISTORY_MAX_AGE_DAYS: i64 = 180;
const PORT_FORWARD_MAX_AGE_DAYS: i64 = 180;
/// Most saved forwards per project (the oldest go first).
const PORT_FORWARDS_PER_WORKSPACE: i64 = 200;
/// Longest saved forward label.
const PORT_LABEL_MAX: usize = 64;

/// A tab in the session.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub rel_path: String,
    pub active: bool,
    pub pinned: bool,
    /// Editor group: 0 left, 1 right (split layout).
    pub group: u8,
}

/// Line bookmark (0-based line).
#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub rel_path: String,
    pub line: usize,
    pub label: Option<String>,
}

/// A file's view: cursor and scroll (line, 0-based).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub line: usize,
    pub column: usize,
    pub scroll_line: usize,
}

/// Backup of an unsaved change.
#[derive(Debug, Clone, PartialEq)]
pub struct Backup {
    pub rel_path: String,
    pub blob_path: String,
    /// Hash of the file on disk when the backup was taken: if it changed, it's a conflict.
    pub base_hash: u64,
}

/// Key under which pinned folders are stored (one `host<TAB>path` per line).
const PINNED_KEY: &str = "pinned_recent";

/// A port forward saved for a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedForward {
    pub remote: u16,
    /// Local port chosen by the user (`None`: same number as the remote one when free).
    pub local: Option<u16>,
    pub label: Option<String>,
    /// `http`, `https` or `tcp`.
    pub protocol: String,
}

/// What is known about an SSH host from earlier connections.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostRecord {
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecentWorkspace {
    /// `None`: local.
    pub host: Option<String>,
    pub path: String,
    pub last_opened_at: i64,
}

pub struct State {
    db: Connection,
}

/// On lock contention (SQLITE_BUSY / SQLITE_LOCKED) retry for at most ~5 s.
fn retry_locked<T>(mut f: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    use rusqlite::ErrorCode::{DatabaseBusy, DatabaseLocked};
    let mut tries = 0;
    loop {
        match f() {
            Err(Error::SqliteFailure(e, _)) if matches!(e.code, DatabaseBusy | DatabaseLocked) && tries < 100 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// The file is not a usable database (overwritten, truncated, damaged pages). Retrying cannot help; only moving it away can.
pub fn is_corrupt(e: &Error) -> bool {
    use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
    matches!(e, Error::SqliteFailure(f, _) if matches!(f.code, NotADatabase | DatabaseCorrupt))
}

/// `host<TAB>path` lines; malformed lines are skipped.
fn parse_pinned(text: &str) -> Vec<(Option<String>, String)> {
    text.lines()
        .filter_map(|l| {
            let (host, path) = l.split_once('\t')?;
            (!path.is_empty()).then(|| ((!host.is_empty()).then(|| host.to_string()), path.to_string()))
        })
        .collect()
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl State {
    /// Open the file (create if missing, mode 0600), apply the schema, prune.
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let db = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Self::init(db)
    }

    pub fn in_memory() -> Result<Self, Error> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut db: Connection) -> Result<Self, Error> {
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL and NORMAL: committed writes survive an app crash; only a power loss can drop the last transactions.
        // (`save_view` writes on every cursor move; FULL would mean an fsync on every write.) These PRAGMAs are issued outside a transaction.
        // When two windows open for the first time at once, switching to WAL can return "database is locked" without honoring `busy_timeout`:
        // retry at short intervals.
        retry_locked(|| db.query_row("pragma journal_mode = wal", [], |_| Ok(())))?;
        db.execute_batch("pragma synchronous = normal;")?;
        // Version read, first-time setup and migrations run in one IMMEDIATE transaction: if two windows (or processes) first open
        // at the same time the second waits and sees the finished schema; an interrupted setup can't leave "version 1 but no tables".
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut version: i64 = tx.query_row("pragma user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(Error::ToSqlConversionFailure(
                format!("this state database was written by a newer Codeyard (schema {version}, this version knows {SCHEMA_VERSION}); update Codeyard or move the file away")
                    .into(),
            ));
        }
        if version == 0 {
            tx.execute_batch(SCHEMA)?;
            tx.execute_batch("pragma user_version = 1")?;
            version = 1;
        }
        for (v, sql) in MIGRATIONS.iter().filter(|(v, _)| *v > version) {
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("pragma user_version = {v}"))?;
        }
        tx.commit()?;
        debug_assert_eq!(db.query_row("pragma user_version", [], |r| r.get::<_, i64>(0))?, SCHEMA_VERSION);
        db.execute_batch("pragma foreign_keys = on;")?;
        let state = State { db };
        state.prune(now_ms())?;
        Ok(state)
    }

    /// Retention policy (see schema.sql): once at startup.
    pub fn prune(&self, now: i64) -> Result<(), Error> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("delete from file_state where updated_at < ?1", [now - FILE_STATE_MAX_AGE_DAYS * DAY_MS])?;
        tx.execute(
            "delete from file_state where id in (
               select id from (
                 select id, row_number() over (partition by workspace_id order by updated_at desc) as rn
                 from file_state
               ) where rn > ?1
             )",
            [FILE_STATE_PER_WORKSPACE],
        )?;
        tx.execute("delete from history where used_at < ?1", [now - HISTORY_MAX_AGE_DAYS * DAY_MS])?;
        tx.execute("delete from port_forwards where updated_at < ?1", [now - PORT_FORWARD_MAX_AGE_DAYS * DAY_MS])?;
        tx.commit()
    }

    fn host_id(&self, alias: &str, now: i64) -> Result<i64, Error> {
        self.db.execute(
            "insert into hosts (alias, created_at, updated_at) values (?1, ?2, ?2)
             on conflict (alias) do update set updated_at = excluded.updated_at",
            params![alias, now],
        )?;
        self.db.query_row("select id from hosts where alias = ?1", [alias], |r| r.get(0))
    }

    /// Folder opened: move it to the front of the recents list.
    pub fn record_workspace(&self, host: Option<&str>, path: &str) -> Result<i64, Error> {
        let now = now_ms();
        // IMMEDIATE: if two windows save at once the second waits up to `busy_timeout`; a deferred read later upgraded
        // to a write would fail instantly with SQLITE_BUSY in WAL.
        let tx = rusqlite::Transaction::new_unchecked(&self.db, rusqlite::TransactionBehavior::Immediate)?;
        let host_id = host.map(|h| self.host_id(h, now)).transpose()?;
        let existing: Option<i64> = tx
            .query_row("select id from workspaces where coalesce(host_id, 0) = coalesce(?1, 0) and path = ?2", params![host_id, path], |r| r.get(0))
            .optional()?;
        let id = match existing {
            Some(id) => {
                tx.execute("update workspaces set last_opened_at = ?1, updated_at = ?1, hidden = 0 where id = ?2", params![now, id])?;
                id
            }
            None => {
                tx.execute(
                    "insert into workspaces (host_id, path, last_opened_at, created_at, updated_at)
                     values (?1, ?2, ?3, ?3, ?3)",
                    params![host_id, path, now],
                )?;
                tx.last_insert_rowid()
            }
        };
        tx.commit()?;
        Ok(id)
    }

    pub fn recent_workspaces(&self, limit: usize) -> Result<Vec<RecentWorkspace>, Error> {
        let mut stmt = self.db.prepare(
            "select h.alias, w.path, w.last_opened_at
             from workspaces w left join hosts h on h.id = w.host_id
             where w.hidden = 0
             order by w.last_opened_at desc limit ?1",
        )?;
        stmt.query_map([limit as i64], |r| Ok(RecentWorkspace { host: r.get(0)?, path: r.get(1)?, last_opened_at: r.get(2)? }))?.collect()
    }

    /// Remove from recents (the project switcher's "Remove"). The row is hidden, not deleted: that folder's unsaved-change
    /// backups, tabs and favorites (via cascade) must not go away; they all return when the folder is reopened. If it was pinned, the pin
    /// is removed. An unknown host alias doesn't match a local folder.
    pub fn forget_workspace(&self, host: Option<&str>, path: &str) -> Result<(), Error> {
        match host {
            Some(alias) => self
                .db
                .execute("update workspaces set hidden = 1 where path = ?2 and host_id = (select id from hosts where alias = ?1)", params![alias, path])?,
            None => self.db.execute("update workspaces set hidden = 1 where path = ?1 and host_id is null", [path])?,
        };
        self.set_workspace_pinned(host, path, false)
    }

    /// Pinned folders (host alias or `None`, path), in pin order. Stays at the top of the list.
    pub fn pinned_workspaces(&self) -> Vec<(Option<String>, String)> {
        parse_pinned(&self.get(PINNED_KEY).unwrap_or_default())
    }

    /// Pin or unpin a folder (passing the same value again changes nothing). Read-modify-write in a single
    /// IMMEDIATE transaction: so two windows don't overwrite each other's pins; a read failure (SQLITE_BUSY, corrupt file) must not
    /// be treated as an empty list and wipe all pins, it is returned as an error.
    pub fn set_workspace_pinned(&self, host: Option<&str>, path: &str, pinned: bool) -> Result<(), Error> {
        // The list is one `host<TAB>path` per line: a name with a break in it would read back as two other pins (or none).
        if pinned && (path.contains(['\n', '\r']) || host.is_some_and(|h| h.contains(['\n', '\r', '\t']))) {
            return Err(Error::InvalidParameterName("a folder name with a line break cannot be pinned".into()));
        }
        let tx = rusqlite::Transaction::new_unchecked(&self.db, rusqlite::TransactionBehavior::Immediate)?;
        let text: String = tx.query_row("select value from kv where key = ?1", [PINNED_KEY], |r| r.get(0)).optional()?.unwrap_or_default();
        let mut list = parse_pinned(&text);
        let key = (host.map(str::to_string), path.to_string());
        list.retain(|k| *k != key);
        if pinned {
            list.push(key);
        }
        let text: Vec<String> = list.iter().map(|(h, p)| format!("{}\t{p}", h.as_deref().unwrap_or(""))).collect();
        tx.execute(
            "insert into kv (key, value, updated_at) values (?1, ?2, ?3)
             on conflict (key) do update set value = excluded.value, updated_at = excluded.updated_at",
            params![PINNED_KEY, text.join("\n"), now_ms()],
        )?;
        tx.commit()
    }

    // --- Favorites and bookmarks ---

    pub fn favorites(&self, workspace: i64) -> Result<Vec<String>, Error> {
        let mut stmt = self.db.prepare("select rel_path from favorites where workspace_id = ?1 order by created_at")?;
        stmt.query_map([workspace], |r| r.get(0))?.collect()
    }

    /// Remove if it's a favorite, add if not; returns the new state.
    pub fn toggle_favorite(&self, workspace: i64, rel_path: &str) -> Result<bool, Error> {
        let removed = self.db.execute("delete from favorites where workspace_id = ?1 and rel_path = ?2", params![workspace, rel_path])?;
        if removed > 0 {
            return Ok(false);
        }
        self.db.execute("insert into favorites (workspace_id, rel_path, created_at) values (?1, ?2, ?3)", params![workspace, rel_path, now_ms()])?;
        Ok(true)
    }

    /// Rename/delete: the favorite's path must change or drop too.
    pub fn move_favorite(&self, workspace: i64, from: &str, to: Option<&str>) -> Result<(), Error> {
        match to {
            Some(to) => {
                self.db.execute("update or ignore favorites set rel_path = ?3 where workspace_id = ?1 and rel_path = ?2", params![workspace, from, to])?
            }
            None => self.db.execute("delete from favorites where workspace_id = ?1 and rel_path = ?2", params![workspace, from])?,
        };
        Ok(())
    }

    pub fn bookmarks(&self, workspace: i64) -> Result<Vec<Bookmark>, Error> {
        let mut stmt = self.db.prepare("select rel_path, line, label from bookmarks where workspace_id = ?1 order by rel_path, line")?;
        stmt.query_map([workspace], |r| Ok(Bookmark { rel_path: r.get(0)?, line: r.get::<_, i64>(1)? as usize, label: r.get(2)? }))?.collect()
    }

    /// Replace a file's bookmarks (lines shifted by edits are written in bulk).
    pub fn save_file_bookmarks(&self, workspace: i64, rel_path: &str, marks: &[(usize, Option<String>)]) -> Result<(), Error> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("delete from bookmarks where workspace_id = ?1 and rel_path = ?2", params![workspace, rel_path])?;
        for (line, label) in marks {
            tx.execute(
                "insert or ignore into bookmarks (workspace_id, rel_path, line, label, created_at) values (?1, ?2, ?3, ?4, ?5)",
                params![workspace, rel_path, *line as i64, label, now_ms()],
            )?;
        }
        tx.commit()
    }

    /// Settings store (e.g. theme).
    pub fn get(&self, key: &str) -> Option<String> {
        self.db.query_row("select value from kv where key = ?1", [key], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn set(&self, key: &str, value: &str) -> Result<(), Error> {
        self.db.execute(
            "insert into kv (key, value, updated_at) values (?1, ?2, ?3)
             on conflict (key) do update set value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now_ms()],
        )?;
        Ok(())
    }

    /// Replace a workspace's tabs (order is preserved).
    pub fn save_tabs(&self, workspace: i64, tabs: &[Tab]) -> Result<(), Error> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("delete from open_editors where workspace_id = ?1", [workspace])?;
        for (i, t) in tabs.iter().enumerate() {
            tx.execute(
                "insert into open_editors (workspace_id, group_index, position, rel_path, is_active, is_pinned)
                 values (?1, ?6, ?2, ?3, ?4, ?5)",
                params![workspace, i as i64, t.rel_path, t.active as i64, t.pinned as i64, t.group as i64],
            )?;
        }
        tx.commit()
    }

    pub fn tabs(&self, workspace: i64) -> Result<Vec<Tab>, Error> {
        let mut stmt =
            self.db.prepare("select rel_path, is_active, is_pinned, group_index from open_editors where workspace_id = ?1 order by group_index, position")?;
        stmt.query_map([workspace], |r| {
            Ok(Tab { rel_path: r.get(0)?, active: r.get::<_, i64>(1)? != 0, pinned: r.get::<_, i64>(2)? != 0, group: r.get::<_, i64>(3)?.clamp(0, 1) as u8 })
        })?
        .collect()
    }

    pub fn save_view(&self, workspace: i64, rel_path: &str, v: View) -> Result<(), Error> {
        self.db.execute(
            "insert into file_state (workspace_id, rel_path, cursors, scroll_top, updated_at) values (?1, ?2, ?3, ?4, ?5)
             on conflict (workspace_id, rel_path) do update set cursors = excluded.cursors,
               scroll_top = excluded.scroll_top, updated_at = excluded.updated_at",
            params![workspace, rel_path, format!("[[{},{}]]", v.line, v.column), v.scroll_line as i64, now_ms()],
        )?;
        Ok(())
    }

    pub fn view(&self, workspace: i64, rel_path: &str) -> Option<View> {
        let (cursors, scroll): (String, i64) = self
            .db
            .query_row("select cursors, scroll_top from file_state where workspace_id = ?1 and rel_path = ?2", params![workspace, rel_path], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .ok()?;
        let nums: Vec<usize> = cursors.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse().ok()).collect();
        Some(View { line: *nums.first()?, column: *nums.get(1)?, scroll_line: scroll.max(0) as usize })
    }

    pub fn save_backup(&self, workspace: i64, b: &Backup) -> Result<(), Error> {
        self.db.execute(
            "insert into backups (workspace_id, rel_path, blob_path, base_hash, updated_at) values (?1, ?2, ?3, ?4, ?5)
             on conflict (workspace_id, rel_path) do update set blob_path = excluded.blob_path,
               base_hash = excluded.base_hash, updated_at = excluded.updated_at",
            params![workspace, b.rel_path, b.blob_path, b.base_hash.to_string(), now_ms()],
        )?;
        Ok(())
    }

    pub fn backups(&self, workspace: i64) -> Result<Vec<Backup>, Error> {
        let mut stmt = self.db.prepare("select rel_path, blob_path, base_hash from backups where workspace_id = ?1 and rel_path is not null")?;
        stmt.query_map([workspace], |r| Ok(Backup { rel_path: r.get(0)?, blob_path: r.get(1)?, base_hash: r.get::<_, String>(2)?.parse().unwrap_or(0) }))?
            .collect()
    }

    /// Returns the deleted record's file path (the caller deletes the file).
    pub fn remove_backup(&self, workspace: i64, rel_path: &str) -> Result<Option<String>, Error> {
        let blob: Option<String> = self
            .db
            .query_row("select blob_path from backups where workspace_id = ?1 and rel_path = ?2", params![workspace, rel_path], |r| r.get(0))
            .optional()?;
        self.db.execute("delete from backups where workspace_id = ?1 and rel_path = ?2", params![workspace, rel_path])?;
        Ok(blob)
    }

    pub fn record_host_ok(&self, alias: &str, agent_version: u32, arch: &str) -> Result<(), Error> {
        let now = now_ms();
        self.host_id(alias, now)?;
        self.db.execute(
            "update hosts set agent_version = ?2, agent_arch = ?3, last_ok_at = ?4, last_error = null
             where alias = ?1",
            params![alias, agent_version.to_string(), arch_column(arch), now],
        )?;
        Ok(())
    }

    pub fn record_host_error(&self, alias: &str, error: &str) -> Result<(), Error> {
        let now = now_ms();
        self.host_id(alias, now)?;
        self.db.execute("update hosts set last_error = ?2 where alias = ?1", params![alias, error])?;
        Ok(())
    }

    /// Last successful connection and last error per host alias.
    pub fn host_records(&self) -> std::collections::HashMap<String, HostRecord> {
        let read = || -> Result<std::collections::HashMap<String, HostRecord>, Error> {
            let mut stmt = self.db.prepare("select alias, last_ok_at, last_error from hosts")?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, HostRecord { last_ok_at: r.get(1)?, last_error: r.get(2)? })))?.collect()
        };
        read().unwrap_or_default()
    }

    /// Forwards saved for a project, by remote port. Rows that do not fit the types are skipped.
    pub fn port_forwards(&self, workspace: i64) -> Vec<SavedForward> {
        let read = || -> Result<Vec<SavedForward>, Error> {
            let mut stmt =
                self.db.prepare("select remote_port, local_port, label, protocol from port_forwards where workspace_id = ?1 order by remote_port")?;
            stmt.query_map([workspace], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?)))?
                .collect::<Result<Vec<_>, _>>()
                .map(|rows| {
                    rows.into_iter()
                        .filter_map(|(remote, local, label, protocol)| {
                            let local = match local {
                                Some(l) => Some(u16::try_from(l).ok().filter(|p| *p > 0)?),
                                None => None,
                            };
                            Some(SavedForward { remote: u16::try_from(remote).ok().filter(|p| *p > 0)?, local, label, protocol })
                        })
                        .collect()
                })
        };
        read().unwrap_or_default()
    }

    /// Saves (or replaces) a project's forward for `f.remote`; the label is capped and the oldest rows beyond the
    /// per-project limit are dropped.
    pub fn save_port_forward(&self, workspace: i64, f: &SavedForward) -> Result<(), Error> {
        let label: Option<String> = f.label.as_deref().map(|l| l.chars().filter(|c| !c.is_control()).take(PORT_LABEL_MAX).collect());
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "insert into port_forwards (workspace_id, remote_port, local_port, label, protocol, updated_at) values (?1, ?2, ?3, ?4, ?5, ?6)
             on conflict (workspace_id, remote_port) do update set
               local_port = excluded.local_port, label = excluded.label, protocol = excluded.protocol, updated_at = excluded.updated_at",
            params![workspace, f.remote, f.local, label, f.protocol, now_ms()],
        )?;
        tx.execute(
            "delete from port_forwards where workspace_id = ?1 and remote_port not in (
               select remote_port from port_forwards where workspace_id = ?1 order by updated_at desc limit ?2
             )",
            params![workspace, PORT_FORWARDS_PER_WORKSPACE],
        )?;
        tx.commit()
    }

    pub fn delete_port_forward(&self, workspace: i64, remote: u16) -> Result<(), Error> {
        self.db.execute("delete from port_forwards where workspace_id = ?1 and remote_port = ?2", params![workspace, remote])?;
        Ok(())
    }
}

/// A value satisfying the `hosts.agent_arch` check constraint (unknown: null).
fn arch_column(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64-linux" | "x86_64 linux" => Some("x86_64-linux"),
        "aarch64-linux" => Some("aarch64-linux"),
        "x86_64-darwin" | "x86_64-macos" => Some("x86_64-darwin"),
        "aarch64-darwin" | "aarch64-macos" => Some("aarch64-darwin"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("state.db")
    }

    #[test]
    fn forgetting_a_workspace_hides_it_but_keeps_unsaved_work_and_tabs() {
        let s = State::in_memory().unwrap();
        let ws = s.record_workspace(None, "/p").unwrap();
        s.save_tabs(ws, &[Tab { rel_path: "a.rs".into(), active: true, pinned: false, group: 0 }]).unwrap();
        s.save_backup(ws, &Backup { rel_path: "a.rs".into(), blob_path: "/blobs/1".into(), base_hash: 7 }).unwrap();
        s.forget_workspace(None, "/p").unwrap();
        assert!(s.recent_workspaces(10).unwrap().is_empty());
        // Reopening brings the same workspace back with its tabs and its backup.
        assert_eq!(s.record_workspace(None, "/p").unwrap(), ws);
        assert_eq!(s.recent_workspaces(10).unwrap().len(), 1);
        assert_eq!(s.tabs(ws).unwrap().len(), 1);
        assert_eq!(s.backups(ws).unwrap().len(), 1, "the unsaved edit survived \"remove from recent\"");
    }

    #[test]
    fn forgetting_with_an_unknown_host_never_touches_the_local_folder() {
        let s = State::in_memory().unwrap();
        s.record_workspace(None, "/a").unwrap();
        s.forget_workspace(Some("no-such-host"), "/a").unwrap();
        assert_eq!(s.recent_workspaces(10).unwrap().len(), 1, "host `no-such-host` is not the local machine");
        s.record_workspace(Some("srv"), "/a").unwrap();
        s.forget_workspace(Some("srv"), "/a").unwrap();
        let r = s.recent_workspaces(10).unwrap();
        assert_eq!((r.len(), r[0].host.clone()), (1, None), "only the remote one went");
    }

    #[test]
    fn a_database_from_a_newer_version_is_refused_not_used() {
        let path = temp_db("newer");
        drop(State::open(&path).unwrap());
        Connection::open(&path).unwrap().execute_batch(&format!("pragma user_version = {}", SCHEMA_VERSION + 1)).unwrap();
        let Err(e) = State::open(&path) else { panic!("must refuse") };
        assert!(e.to_string().contains("newer Codeyard"), "{e}");
    }

    #[test]
    fn two_windows_share_one_file_without_busy_errors() {
        let path = temp_db("two");
        let (a, b) = (State::open(&path).unwrap(), State::open(&path).unwrap());
        let mut handles = Vec::new();
        for (n, st) in [(0, a), (1, b)] {
            handles.push(std::thread::spawn(move || {
                for i in 0..40 {
                    st.record_workspace(None, &format!("/w{n}-{i}")).unwrap();
                    st.set_workspace_pinned(None, &format!("/w{n}-{i}"), true).unwrap();
                    st.save_view(1, "f.rs", View { line: i, column: 0, scroll_line: 0 }).ok();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let c = State::open(&path).unwrap();
        assert_eq!(c.recent_workspaces(500).unwrap().len(), 80);
        assert_eq!(c.pinned_workspaces().len(), 80, "no window overwrote the other's pins");
    }

    #[test]
    fn first_start_in_two_processes_does_not_fail_on_the_schema() {
        let path = temp_db("race");
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let p = path.clone();
                std::thread::spawn(move || State::open(&p).map(|_| ()).map_err(|e| e.to_string()))
            })
            .collect();
        for h in handles {
            h.join().unwrap().expect("every opener gets a ready database");
        }
    }

    #[test]
    fn migration_3_survives_duplicate_backups_from_an_old_database() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch("pragma user_version = 2").unwrap();
        db.execute_batch(
            "insert into workspaces (id, path, last_opened_at, created_at, updated_at) values (1, '/p', 1, 1, 1);
             insert into backups (workspace_id, rel_path, blob_path, updated_at) values (1, 'a.rs', 'b1', 1), (1, 'a.rs', 'b2', 2), (1, null, 'n1', 1), (1, null, 'n2', 1);",
        )
        .unwrap();
        let s = State::init(db).unwrap();
        let n: i64 = s.db.query_row("select count(*) from backups where rel_path = 'a.rs'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "the duplicate was resolved, the app opens");
        let unnamed: i64 = s.db.query_row("select count(*) from backups where rel_path is null", [], |r| r.get(0)).unwrap();
        assert_eq!(unnamed, 2, "unnamed new-file backups are all kept");
    }

    #[test]
    fn recent_workspaces_dedupe_and_order() {
        let s = State::in_memory().unwrap();
        s.record_workspace(None, "/a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        s.record_workspace(Some("node-dev"), "/a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        s.record_workspace(None, "/a").unwrap(); // again: not a new record, moved to the front
        let r = s.recent_workspaces(10).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].host.as_deref(), r[0].path.as_str()), (None, "/a"));
        assert_eq!(r[1].host.as_deref(), Some("node-dev"));
    }

    #[test]
    fn port_forwards_are_saved_per_project_replaced_and_checked() {
        let s = State::in_memory().unwrap();
        let ws = s.record_workspace(Some("box"), "/srv/app").unwrap();
        let other = s.record_workspace(None, "/tmp/x").unwrap();
        let web = SavedForward { remote: 3000, local: None, label: Some("web\u{1b}[2J".into()), protocol: "http".into() };
        s.save_port_forward(ws, &web).unwrap();
        s.save_port_forward(ws, &SavedForward { remote: 5432, local: Some(15432), label: None, protocol: "tcp".into() }).unwrap();
        s.save_port_forward(ws, &SavedForward { protocol: "https".into(), ..web.clone() }).unwrap();
        let saved = s.port_forwards(ws);
        assert_eq!(saved.len(), 2, "saving the same port replaces it");
        assert_eq!((saved[0].label.as_deref(), saved[0].protocol.as_str()), (Some("web[2J"), "https"), "control characters are dropped");
        assert_eq!(saved[1].local, Some(15432));
        assert!(s.port_forwards(other).is_empty(), "per project");
        assert!(s.save_port_forward(ws, &SavedForward { protocol: "gopher".into(), ..web.clone() }).is_err(), "the protocol is checked");
        s.delete_port_forward(ws, 3000).unwrap();
        assert_eq!(s.port_forwards(ws).len(), 1);
        for p in 1..=(PORT_FORWARDS_PER_WORKSPACE as u16 + 20) {
            s.save_port_forward(ws, &SavedForward { remote: 10_000 + p, local: None, label: None, protocol: "http".into() }).unwrap();
        }
        assert_eq!(s.port_forwards(ws).len(), PORT_FORWARDS_PER_WORKSPACE as usize, "capped per project");
        s.db.execute("delete from workspaces where id = ?1", [ws]).unwrap();
        assert!(s.port_forwards(ws).is_empty(), "removed with the project");
    }

    #[test]
    fn host_records_report_the_last_success_and_error() {
        let s = State::in_memory().unwrap();
        s.record_host_ok("box", 10, "x86_64-linux").unwrap();
        s.record_host_error("down", "Connection refused").unwrap();
        let r = s.host_records();
        assert!(r["box"].last_ok_at.is_some() && r["box"].last_error.is_none());
        assert_eq!(r["down"].last_error.as_deref(), Some("Connection refused"));
    }

    #[test]
    fn prune_keeps_newest_500_and_drops_old() {
        let s = State::in_memory().unwrap();
        let ws = s.record_workspace(None, "/p").unwrap();
        let now = now_ms();
        let tx = s.db.unchecked_transaction().unwrap();
        for i in 0..600 {
            tx.execute("insert into file_state (workspace_id, rel_path, cursors, updated_at) values (?1, ?2, '[]', ?3)", params![ws, format!("f{i}"), now - i])
                .unwrap();
        }
        tx.execute("insert into file_state (workspace_id, rel_path, cursors, updated_at) values (?1, 'eski', '[]', ?2)", params![ws, now - 91 * DAY_MS])
            .unwrap();
        tx.commit().unwrap();

        s.prune(now).unwrap();
        let count: i64 = s.db.query_row("select count(*) from file_state", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 500);
        let oldest_kept: String = s.db.query_row("select rel_path from file_state order by updated_at asc limit 1", [], |r| r.get(0)).unwrap();
        assert_eq!(oldest_kept, "f499");
    }

    #[test]
    fn kv_and_migration_from_v1() {
        // A database left at version 1: the kv table must arrive via migration.
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch("pragma user_version = 1").unwrap();
        let s = State::init(db).unwrap();
        assert_eq!(s.db.query_row("pragma user_version", [], |r| r.get::<_, i64>(0)).unwrap(), SCHEMA_VERSION);
        assert_eq!(s.get("theme"), None);
        s.set("theme", "fjord").unwrap();
        s.set("theme", "ember").unwrap();
        assert_eq!(s.get("theme").as_deref(), Some("ember"));
    }

    #[test]
    fn session_tabs_views_and_backups() {
        let s = State::in_memory().unwrap();
        let ws = s.record_workspace(None, "/p").unwrap();
        let tabs = vec![
            Tab { rel_path: "a.rs".into(), active: false, pinned: true, group: 0 },
            Tab { rel_path: "src/b.rs".into(), active: true, pinned: false, group: 0 },
        ];
        s.save_tabs(ws, &tabs).unwrap();
        assert_eq!(s.tabs(ws).unwrap(), tabs);
        s.save_tabs(ws, &tabs[1..]).unwrap();
        assert_eq!(s.tabs(ws).unwrap().len(), 1, "replaces, does not append");

        s.save_view(ws, "a.rs", View { line: 12, column: 4, scroll_line: 3 }).unwrap();
        s.save_view(ws, "a.rs", View { line: 13, column: 0, scroll_line: 5 }).unwrap();
        assert_eq!(s.view(ws, "a.rs"), Some(View { line: 13, column: 0, scroll_line: 5 }));
        assert_eq!(s.view(ws, "yok.rs"), None);

        let b = Backup { rel_path: "a.rs".into(), blob_path: "/tmp/x".into(), base_hash: u64::MAX };
        s.save_backup(ws, &b).unwrap();
        s.save_backup(ws, &Backup { blob_path: "/tmp/y".into(), ..b.clone() }).unwrap();
        let all = s.backups(ws).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].blob_path.as_str(), all[0].base_hash), ("/tmp/y", u64::MAX), "u64 hash is lossless");
        assert_eq!(s.remove_backup(ws, "a.rs").unwrap().as_deref(), Some("/tmp/y"));
        assert!(s.backups(ws).unwrap().is_empty());
    }

    #[test]
    fn host_status_roundtrip() {
        let s = State::in_memory().unwrap();
        s.record_host_error("node-dev", "Permission denied").unwrap();
        s.record_host_ok("node-dev", 1, "x86_64-linux").unwrap();
        let (err, arch): (Option<String>, Option<String>) =
            s.db.query_row("select last_error, agent_arch from hosts where alias = 'node-dev'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((err, arch.as_deref()), (None, Some("x86_64-linux")));
    }

    #[test]
    fn favorites_bookmarks_and_groups() {
        let s = State::in_memory().unwrap();
        let ws = s.record_workspace(None, "/p").unwrap();
        assert!(s.toggle_favorite(ws, "src/a.rs").unwrap());
        assert!(s.toggle_favorite(ws, "README.md").unwrap());
        assert!(!s.toggle_favorite(ws, "README.md").unwrap(), "the second call removes it");
        s.move_favorite(ws, "src/a.rs", Some("src/b.rs")).unwrap();
        assert_eq!(s.favorites(ws).unwrap(), vec!["src/b.rs".to_string()]);
        s.save_file_bookmarks(ws, "src/b.rs", &[(3, None), (9, Some("burası".into()))]).unwrap();
        assert_eq!(s.bookmarks(ws).unwrap().len(), 2);
        s.save_file_bookmarks(ws, "src/b.rs", &[(4, None)]).unwrap();
        assert_eq!(s.bookmarks(ws).unwrap(), vec![Bookmark { rel_path: "src/b.rs".into(), line: 4, label: None }]);
        let tabs =
            vec![Tab { rel_path: "a".into(), active: false, pinned: false, group: 0 }, Tab { rel_path: "b".into(), active: true, pinned: false, group: 1 }];
        s.save_tabs(ws, &tabs).unwrap();
        assert_eq!(s.tabs(ws).unwrap(), tabs);
        s.forget_workspace(None, "/p").unwrap();
        assert!(s.recent_workspaces(10).unwrap().is_empty());
    }

    #[test]
    fn pinned_workspaces_round_trip_keep_order_and_follow_forgetting() {
        let s = State::in_memory().unwrap();
        assert!(s.pinned_workspaces().is_empty());
        s.record_workspace(None, "/a").unwrap();
        s.record_workspace(Some("node-dev"), "/srv/app").unwrap();
        s.set_workspace_pinned(Some("node-dev"), "/srv/app", true).unwrap();
        s.set_workspace_pinned(None, "/a", true).unwrap();
        assert_eq!(s.pinned_workspaces(), vec![(Some("node-dev".to_string()), "/srv/app".to_string()), (None, "/a".to_string())], "pin order is kept");
        // Passing the same value again doesn't duplicate.
        s.set_workspace_pinned(None, "/a", true).unwrap();
        assert_eq!(s.pinned_workspaces().len(), 2);
        s.set_workspace_pinned(Some("node-dev"), "/srv/app", false).unwrap();
        assert_eq!(s.pinned_workspaces(), vec![(None, "/a".to_string())]);
        // Removing a pinned folder from recents also drops its pin.
        s.forget_workspace(None, "/a").unwrap();
        assert!(s.pinned_workspaces().is_empty());
        assert!(s.recent_workspaces(10).unwrap().iter().all(|r| r.path != "/a"));
    }

    #[test]
    fn a_folder_name_with_a_line_break_is_not_pinned_as_somebody_else() {
        let s = State::in_memory().unwrap();
        s.set_workspace_pinned(None, "/keep", true).unwrap();
        // Written as is this would read back as pins `("" , "/a")` and `("b", "c")`.
        assert!(s.set_workspace_pinned(None, "/a\nb\tc", true).is_err());
        assert!(s.set_workspace_pinned(Some("h\nx"), "/p", true).is_err());
        assert_eq!(s.pinned_workspaces(), vec![(None, "/keep".to_string())]);
        // Unpinning such a name is harmless and must not fail.
        s.set_workspace_pinned(None, "/a\nb", false).unwrap();
    }

    #[test]
    fn pinned_workspaces_ignore_a_damaged_value() {
        let s = State::in_memory().unwrap();
        s.set("pinned_recent", "no tab here\n\t\nhost\t\n\t/only/path\n").unwrap();
        assert_eq!(s.pinned_workspaces(), vec![(None, "/only/path".to_string())]);
    }
}
