-- Local state database: ~/.local/state/<app>/state.db (macOS: ~/Library/Application Support/<app>/)
-- SQLite (rusqlite, bundled), WAL mode. Local machine only; the remote agent keeps no database.
-- Single user, single process: access control = file permissions (0600). No RLS, none needed.
-- Time: INTEGER unix epoch milliseconds, UTC. id: INTEGER (no uuid, single machine).

-- PRAGMAs (journal_mode = WAL, synchronous = NORMAL, foreign_keys = ON) and user_version are set in `State::init`:
-- this file is tables only and runs inside a single IMMEDIATE transaction (journal_mode can't change inside a transaction).

-- SSH hosts are read from ~/.ssh/config; only app-specific extras are kept here.
create table hosts (
  id            integer primary key,
  alias         text not null unique,          -- Host name in ~/.ssh/config
  agent_version text,                          -- agent version installed on the remote
  agent_arch    text check (agent_arch in ('x86_64-linux','aarch64-linux','x86_64-darwin','aarch64-darwin')),
  last_ok_at    integer,
  last_error    text,
  created_at    integer not null,
  updated_at    integer not null
);

-- Opened folders. host_id null = local.
create table workspaces (
  id             integer primary key,
  host_id        integer references hosts(id) on delete cascade,
  path           text not null,
  last_opened_at integer not null,
  layout         text not null default '{}',   -- sidebar/panel sizes, split layout (JSON)
  created_at     integer not null,
  updated_at     integer not null
);
-- The same host+path is a single record. NULLs count as distinct in unique, hence the expression index.
create unique index workspaces_host_path on workspaces (coalesce(host_id, 0), path);
create index workspaces_recent on workspaces (last_opened_at desc);

-- Session restore: which tabs were open, in which order.
create table open_editors (
  id           integer primary key,
  workspace_id integer not null references workspaces(id) on delete cascade,
  group_index  integer not null default 0,     -- split group
  position     integer not null,
  rel_path     text not null,
  is_active    integer not null default 0 check (is_active in (0,1)),
  is_pinned    integer not null default 0 check (is_pinned in (0,1)),
  unique (workspace_id, group_index, position)
);
create index open_editors_ws on open_editors (workspace_id);

-- Per-file view state: cursor, scroll, folds. Remembered even after the tab closes.
create table file_state (
  id           integer primary key,
  workspace_id integer not null references workspaces(id) on delete cascade,
  rel_path     text not null,
  cursors      text not null,                  -- JSON [[line,col,anchorLine,anchorCol],...]
  scroll_top   integer not null default 0,
  folds        text not null default '[]',
  updated_at   integer not null,
  unique (workspace_id, rel_path)
);
create index file_state_lru on file_state (workspace_id, updated_at);

-- Backups of unsaved changes (hot exit). Contents live in a separate file on disk;
-- only the index is here. base_hash: hash of the file on disk when the backup was taken.
create table backups (
  id           integer primary key,
  workspace_id integer not null references workspaces(id) on delete cascade,
  rel_path     text,                           -- null = unnamed new file
  blob_path    text not null unique,
  base_hash    text,
  updated_at   integer not null
);
create index backups_ws on backups (workspace_id);

-- Palette / search history (for ordering by recent use).
create table history (
  id      integer primary key,
  kind    text not null check (kind in ('command','search','goto_file')),
  value   text not null,
  used_at integer not null,
  uses    integer not null default 1,
  unique (kind, value)
);
create index history_recent on history (kind, used_at desc);

-- Retention policy (at app startup, single transaction; state::prune):
-- Not a trigger: file_state is written on every cursor move, and instead of adding a
-- DELETE cost to every write, pruning happens once at startup.
--   1) File states older than 90 days and history records older than 180 days are deleted.
--   2) Only the newest 500 file states per workspace are kept.
-- :now = unix ms
--
-- delete from file_state where updated_at < :now - 90 * 86400000;
-- delete from file_state where id in (
--   select id from (
--     select id, row_number() over (partition by workspace_id order by updated_at desc) as rn
--     from file_state
--   ) where rn > 500
-- );
-- delete from history where used_at < :now - 180 * 86400000;
-- delete from port_forwards where updated_at < :now - 180 * 86400000;   (version 6)

-- Version 2 (crates/state/src/lib.rs MIGRATIONS): key-value store for settings.
-- create table kv (key text primary key, value text not null, updated_at integer not null);
-- Version 3: create unique index backups_ws_rel on backups (workspace_id, rel_path);
-- Version 5: workspaces.hidden: "remove from recents" hides the row instead of deleting it (so backups and tabs don't go away via cascade).
-- Version 6: port_forwards: ports the user forwarded per workspace (remote port, chosen local port, label, protocol),
--   at most 200 per workspace; restored when the workspace opens again.
