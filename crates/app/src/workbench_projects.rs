//! Project management and favorites.
//!
//! - Project switcher (⌃R): saved projects (★) and recents, local and SSH.
//! - Saved projects live in `projects.jsonc` (settings folder; hand-editable): name, path,
//!   host, tags. Same idea as VS Code's Project Manager extension.
//! - Favorites: files/folders per project (sidebar section, ⇧⌘P list).

use gpui::{AppContext as _, Context, Window};
use serde_json::{Value, json};

use super::{InputPurpose, Notice, OverlayKind, Workbench};
use crate::i18n::{t, tf};
use crate::picker::{PickItem, Picker, Source as PickSource};
use crate::ui;
use state::RecentWorkspace;

/// A project in `projects.jsonc`.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub name: String,
    pub path: String,
    /// SSH alias (`None`: local).
    pub host: Option<String>,
    pub tags: Vec<String>,
}

/// What a row in the workspace switcher does.
#[derive(Clone, Debug)]
pub enum SwitchChoice {
    /// Saved project or recent (local or SSH).
    Open(RecentWorkspace),
    /// Connect to an SSH server (if already connected, pick a folder on that server).
    Connect(String),
    Disconnect,
    Reconnect,
    OpenFolder,
    CloneRepository,
    /// Connect by typing a server name (not in `~/.ssh/config`, or `user@host`).
    ConnectByName,
    /// Add a new server to `~/.ssh/config`.
    AddHost,
    /// Manage servers: edit, test, key login, remove.
    ManageHosts,
    /// WSL distribution or running container.
    Targets {
        wsl: bool,
    },
}

/// Summary of a connection for the switcher.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnSummary {
    Local,
    Connecting(String),
    Connected(String),
    Reconnecting(String),
    Failed(String, String),
}

/// Switcher entries (pure: `build_switcher` reads no UI state, so it is testable).
pub struct SwitcherInput<'a> {
    pub docker_enabled: bool,
    pub saved: &'a [Project],
    pub recent: &'a [RecentWorkspace],
    /// Pinned folders (`state::State::pinned_workspaces`).
    pub pinned: &'a [(Option<String>, String)],
    /// (alias, `user@host`).
    pub hosts: &'a [(String, Option<String>)],
    pub conn: ConnSummary,
    pub current_root: Option<&'a str>,
    pub now_ms: i64,
}

const PROJECTS_FILE: &str = "projects.jsonc";
const PROJECTS_HEADER: &str = "// Saved projects (⌃R opens the switcher). Edit freely: name, path, host (ssh alias or null), tags.\n";

pub fn parse_projects(text: &str) -> Vec<Project> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&theme::themes::strip_jsonc(text)) else { return Vec::new() };
    items
        .iter()
        .filter_map(|p| {
            let path = p["path"].as_str()?.to_string();
            let name = p["name"].as_str().map(str::to_string).unwrap_or_else(|| ui::file_name(&path).to_string());
            Some(Project {
                name,
                path,
                host: p["host"].as_str().map(str::to_string),
                tags: p["tags"].as_array().map(|t| t.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            })
        })
        .collect()
}

fn write_projects(projects: &[Project]) -> String {
    let items: Vec<Value> = projects.iter().map(|p| json!({ "name": p.name, "path": p.path, "host": p.host, "tags": p.tags })).collect();
    format!("{PROJECTS_HEADER}{}\n", serde_json::to_string_pretty(&Value::Array(items)).unwrap_or_else(|_| "[]".into()))
}

/// Short relative time: "now", "5m", "3h", "4d", "2mo" (same format as Timeline and the graph).
fn ago(now_ms: i64, ms: i64) -> String {
    let secs = ((now_ms - ms) / 1000).max(0);
    crate::git::short_ago(&crate::git::relative_time(secs))
}

/// Recents and pinned: the first `RECENT_LIMIT` recents plus every pinned one
/// (a pinned folder stays even if old enough to fall off the list). Always ordered by last opened.
pub fn load_recent(state: &state::State) -> (Vec<RecentWorkspace>, Vec<(Option<String>, String)>) {
    let pinned = state.pinned_workspaces();
    let all = state.recent_workspaces(500).unwrap_or_default();
    let keep = all
        .into_iter()
        .enumerate()
        .filter(|(i, r)| *i < super::RECENT_LIMIT || pinned.iter().any(|(h, p)| *h == r.host && *p == r.path))
        .map(|(_, r)| r)
        .collect();
    (keep, pinned)
}

/// Display order: pinned (in pin order) first, then the rest by last opened. Whether each row
/// is pinned is returned alongside.
pub fn ordered_recent(recent: &[RecentWorkspace], pinned: &[(Option<String>, String)]) -> Vec<(RecentWorkspace, bool)> {
    let is_pinned = |r: &RecentWorkspace| pinned.iter().any(|(h, p)| *h == r.host && *p == r.path);
    let mut out: Vec<(RecentWorkspace, bool)> =
        pinned.iter().filter_map(|(h, p)| recent.iter().find(|r| r.host == *h && r.path == *p)).map(|r| (r.clone(), true)).collect();
    out.extend(recent.iter().filter(|r| !is_pinned(r)).map(|r| (r.clone(), false)));
    out
}

fn action_item(label: &str, detail: &str, keys: Option<&str>) -> PickItem {
    PickItem { label: label.to_string().into(), detail: Some(detail.to_string().into()), keys: keys.map(|k| k.to_string().into()) }
}

/// Workspace switcher rows, in order: connection action, saved projects (★), recents,
/// SSH servers (with live status), actions. All in one list, filtered as you type.
pub fn build_switcher(i: &SwitcherInput) -> Vec<(PickItem, SwitchChoice)> {
    let mut rows: Vec<(PickItem, SwitchChoice)> = Vec::new();
    // 1) Action for the current connection.
    match &i.conn {
        ConnSummary::Connected(alias) => rows.push((
            action_item(&tf("Disconnect from {}", &[alias]), t("Back to this computer; files with unsaved changes must be saved or closed first"), None),
            SwitchChoice::Disconnect,
        )),
        ConnSummary::Failed(alias, why) => rows.push((action_item(&tf("Reconnect to {}", &[alias]), why, None), SwitchChoice::Reconnect)),
        _ => {}
    }
    // 2) Saved projects, 3) recents.
    for p in i.saved.iter().filter(|p| i.docker_enabled || !p.host.as_deref().is_some_and(crate::docker::is_target)) {
        let place = p.host.as_ref().map_or(p.path.clone(), |h| format!("{h}:{}", p.path));
        let tags = if p.tags.is_empty() { String::new() } else { format!(" · {}", p.tags.join(", ")) };
        rows.push((
            PickItem { label: format!("★ {}", p.name).into(), detail: Some(format!("{place}{tags}").into()), keys: None },
            SwitchChoice::Open(RecentWorkspace { host: p.host.clone(), path: p.path.clone(), last_opened_at: 0 }),
        ));
    }
    for (r, pinned) in ordered_recent(i.recent, i.pinned) {
        if !i.docker_enabled && r.host.as_deref().is_some_and(crate::docker::is_target) {
            continue;
        }
        let r = &r;
        if i.saved.iter().any(|p| p.path == r.path && p.host == r.host) {
            continue;
        }
        let place = r.host.as_ref().map_or(r.path.clone(), |h| format!("{h}:{}", r.path));
        let here = if i.current_root == Some(r.path.as_str()) { format!(" · {}", t("current")) } else { String::new() };
        let pin = if pinned { format!(" · {}", t("pinned")) } else { String::new() };
        rows.push((
            PickItem {
                label: ui::file_name(&r.path).to_string().into(),
                detail: Some(format!("{place} · {}{here}{pin}", ago(i.now_ms, r.last_opened_at)).into()),
                keys: None,
            },
            SwitchChoice::Open(r.clone()),
        ));
    }
    // 4) SSH servers, with their status.
    for (alias, detail) in i.hosts {
        let state = match &i.conn {
            ConnSummary::Connected(a) if a == alias => Some(("● ".to_string(), t("connected").to_string())),
            ConnSummary::Connecting(a) if a == alias => Some(("… ".to_string(), t("connecting").to_string())),
            ConnSummary::Reconnecting(a) if a == alias => Some(("… ".to_string(), t("reconnecting").to_string())),
            ConnSummary::Failed(a, why) if a == alias => Some(("✖ ".to_string(), why.clone())),
            _ => None,
        };
        let (mark, status) = state.unwrap_or_default();
        let detail =
            [Some(t("SSH host").to_string()), detail.clone(), (!status.is_empty()).then_some(status)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        rows.push((PickItem { label: format!("{mark}{alias}").into(), detail: Some(detail.into()), keys: None }, SwitchChoice::Connect(alias.clone())));
    }
    // 5) Actions.
    rows.push((action_item(t("Open Folder…"), t("On this computer"), Some(if cfg!(target_os = "macos") { "⌘O" } else { "Ctrl+O" })), SwitchChoice::OpenFolder));
    rows.push((action_item(t("Connect to SSH host…"), t("Type a name from ~/.ssh/config or user@host"), None), SwitchChoice::ConnectByName));
    rows.push((action_item(t("Add SSH host…"), t("Save user@host to ~/.ssh/config and connect"), None), SwitchChoice::AddHost));
    rows.push((action_item(t("Manage SSH hosts…"), t("Edit, test, set up key login, remove"), None), SwitchChoice::ManageHosts));
    rows.push((action_item(t("Clone Git Repository…"), t("Into a folder on this computer or the connected host"), None), SwitchChoice::CloneRepository));
    if i.docker_enabled {
        rows.push((action_item(t("Connect to a running container…"), t("Docker, then open a folder inside it"), None), SwitchChoice::Targets { wsl: false }));
    }
    if cfg!(windows) {
        rows.push((action_item(t("Connect to WSL…"), t("Open a folder in a Linux distribution"), None), SwitchChoice::Targets { wsl: true }));
    }
    rows
}

impl Workbench {
    fn projects_path(&self) -> Option<std::path::PathBuf> {
        self.cfg.dir.as_ref().map(|d| d.join(PROJECTS_FILE))
    }

    pub fn saved_projects(&self) -> Vec<Project> {
        self.projects_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse_projects(&t)).unwrap_or_default()
    }

    /// Workspace switcher: ⌃R, the status bar connection indicator, the welcome screen and the menus open it; the Remote
    /// Explorer view shows the same hosts and folders as a tree.
    pub(super) fn show_project_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        // The server list is refreshed on every open (the config file may have changed).
        self.remote.ssh_hosts = host::ssh::list_hosts();
        let hosts = self.ssh_host_rows();
        let conn = self.conn_summary();
        let saved = self.saved_projects();
        let current = self.root(cx);
        let rows = build_switcher(&SwitcherInput {
            docker_enabled: cx.global::<crate::settings::Settings>().docker_enabled,
            saved: &saved,
            recent: &self.persist.recent,
            pinned: &self.persist.pinned,
            hosts: &hosts,
            conn,
            current_root: current.as_deref(),
            now_ms: state::now_ms(),
        });
        let (items, choices): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
        let picker =
            cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], t("Open a project, connect to a host, or switch workspace"), "", None, cx));
        self.show_overlay(picker, OverlayKind::Projects(choices), window, cx);
    }

    pub(super) fn pick_switch(&mut self, choice: SwitchChoice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            SwitchChoice::Open(r) => self.open_recent(r, window, cx),
            // Already connected to this server: pick a folder on it instead of reconnecting.
            SwitchChoice::Connect(alias) if matches!(&self.remote.conn, super::Conn::Connected { alias: a } if *a == alias) && self.remote.host.is_remote() => {
                self.open_remote_folder(window, cx)
            }
            SwitchChoice::Connect(alias) => self.connect(alias, window, cx),
            SwitchChoice::Disconnect => self.disconnect(cx),
            SwitchChoice::Reconnect => self.reconnect_lost(window, cx),
            SwitchChoice::OpenFolder => self.open_folder(&super::OpenFolder, window, cx),
            SwitchChoice::CloneRepository => self.clone_repository(&super::CloneRepository, window, cx),
            SwitchChoice::ConnectByName => {
                self.show_input(InputPurpose::ConnectHost, t("Connect"), t("Host name (from ~/.ssh/config) or user@host"), "", window, cx)
            }
            SwitchChoice::AddHost => self.add_ssh_host(&super::AddSshHost, window, cx),
            SwitchChoice::ManageHosts => self.show_host_menu(window, cx),
            SwitchChoice::Targets { wsl } => self.show_remote_targets(wsl, window, cx),
        }
    }

    /// "Projects: Add Folder to Projects…": save the open folder under a name.
    pub(super) fn start_save_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else {
            self.notice = Some(Notice::Info(t("Open a folder first, then save it as a project.").into()));
            cx.notify();
            return;
        };
        let name = ui::file_name(&root).to_string();
        self.show_input(InputPurpose::SaveProject { path: root }, t("Save project as"), t("Project name"), &name, window, cx);
    }

    pub(super) fn save_project(&mut self, path: String, name: String, cx: &mut Context<Self>) {
        let Some(file) = self.projects_path() else { return };
        let host = match &self.remote.conn {
            super::Conn::Connected { alias } if self.remote.host.is_remote() => Some(alias.clone()),
            _ => None,
        };
        let mut projects = self.saved_projects();
        projects.retain(|p| !(p.path == path && p.host == host));
        projects.push(Project { name: name.clone(), path, host, tags: Vec::new() });
        let result = std::fs::create_dir_all(file.parent().unwrap_or(&file)).and_then(|_| crate::fsutil::write_atomic(&file, write_projects(&projects)));
        self.notice = Some(match result {
            Ok(()) => Notice::Info(tf("Saved project “{}” (⌃R to switch)", &[&name]).into()),
            Err(e) => Notice::Error(tf("Could not write {0}: {1}", &[&file.display(), &e]).into()),
        });
        cx.notify();
    }

    pub(super) fn edit_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.projects_path() else { return };
        if !file.exists() {
            let _ = std::fs::create_dir_all(file.parent().unwrap_or(&file));
            let _ = crate::fsutil::write_atomic(&file, write_projects(&[]));
        }
        let host = self.remote.local_host.clone();
        self.open_file_on(host, file.to_string_lossy().into_owned(), window, cx);
    }

    /// Remove from recents (picker).
    pub(super) fn show_forget_recent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self
            .persist
            .recent
            .iter()
            .map(|r| PickItem {
                label: ui::file_name(&r.path).to_string().into(),
                detail: Some(r.host.as_ref().map_or(r.path.clone(), |h| format!("{h}:{}", r.path)).into()),
                keys: None,
            })
            .collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], t("Remove from recent"), "", None, cx));
        self.show_overlay(picker, OverlayKind::ForgetRecent(self.persist.recent.clone()), window, cx);
    }

    pub(super) fn forget_recent(&mut self, r: RecentWorkspace, cx: &mut Context<Self>) {
        if let Some(s) = &self.persist.state {
            let _ = s.forget_workspace(r.host.as_deref(), &r.path);
        }
        self.reload_recent(cx);
    }

    /// Re-read recents and pinned from state; refresh menus and the screen.
    pub(super) fn reload_recent(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.persist.state {
            (self.persist.recent, self.persist.pinned) = load_recent(s);
            self.sync_dock_menu(cx);
        }
        cx.notify();
    }

    /// Pin on Welcome: pin the folder to the top of the list or unpin it.
    pub(super) fn toggle_pin_recent(&mut self, r: RecentWorkspace, cx: &mut Context<Self>) {
        let Some(s) = &self.persist.state else { return };
        let pinned = !self.persist.pinned.iter().any(|(h, p)| *h == r.host && *p == r.path);
        let _ = s.set_workspace_pinned(r.host.as_deref(), &r.path, pinned);
        self.reload_recent(cx);
    }

    /// Ask about a recent that fails to open: "remove from list?" (folder deleted, server gone…).
    pub(super) fn ask_forget_unreachable(&mut self, r: RecentWorkspace, why: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = ui::file_name(&r.path).to_string();
        let place = r.host.as_ref().map_or(r.path.clone(), |h| format!("{h}:{}", r.path));
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &tf("“{}” can't be opened", &[&name]),
            Some(&format!("{place}\n{why}\n\n{}", t("Remove it from Recent?"))),
            &[t("Remove from Recent"), t("Keep")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| this.forget_recent(r, cx)).ok();
            }
        })
        .detach();
    }

    // --- Favorites ---

    pub(super) fn load_favorites(&mut self) {
        self.roots.favorites = match (&self.persist.state, self.persist.workspace_id) {
            (Some(s), Some(ws)) => s.favorites(ws).unwrap_or_default(),
            _ => Vec::new(),
        };
    }

    /// Add/remove a file or folder from favorites (tree menu, active file).
    pub fn toggle_favorite(&mut self, path: &str, cx: &mut Context<Self>) {
        let (Some(s), Some(ws)) = (&self.persist.state, self.persist.workspace_id) else { return };
        let Some(rel) = self.rel(path, cx) else { return };
        let added = s.toggle_favorite(ws, &rel).unwrap_or(false);
        self.load_favorites();
        self.notice = Some(Notice::Info(if added { tf("★ Added to favorites: {}", &[&rel]) } else { tf("Removed from favorites: {}", &[&rel]) }.into()));
        cx.notify();
    }

    /// File/folder was renamed or deleted (`to` = `None`): favorites follow
    /// (including favorites under a folder).
    pub(super) fn favorites_moved(&mut self, from: &str, to: Option<&str>, cx: &mut Context<Self>) {
        let (Some(s), Some(ws)) = (&self.persist.state, self.persist.workspace_id) else { return };
        let (Some(from_rel), to_rel) = (self.rel(from, cx), to.and_then(|t| self.rel(t, cx))) else { return };
        let under = format!("{from_rel}/");
        for fav in self.roots.favorites.clone() {
            let new = if fav == from_rel {
                to_rel.clone()
            } else if let Some(rest) = fav.strip_prefix(&under) {
                to_rel.as_ref().map(|t| format!("{t}/{rest}"))
            } else {
                continue;
            };
            let _ = s.move_favorite(ws, &fav, new.as_deref());
        }
        self.load_favorites();
    }

    pub(super) fn toggle_active_favorite(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.active_editor().map(|e| e.read(cx).path.clone()) {
            self.toggle_favorite(&path, cx);
        }
    }

    pub(super) fn show_favorites(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.roots.favorites.is_empty() {
            self.notice = Some(Notice::Info(t("No favorites yet: right-click a file → Add to Favorites, or “Favorites: Toggle Active File”.").into()));
            cx.notify();
            return;
        }
        let items = self
            .roots
            .favorites
            .iter()
            .map(|rel| PickItem { label: format!("★ {}", ui::file_name(rel)).into(), detail: Some(rel.clone().into()), keys: None })
            .collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], t("Favorites"), "", None, cx));
        self.show_overlay(picker, OverlayKind::Favorites(self.roots.favorites.clone()), window, cx);
    }

    /// Favorite picked: open it if a file, reveal in the tree if a folder.
    pub(super) fn open_favorite(&mut self, rel: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else { return };
        let path = ui::join(&root, &rel);
        let host = self.remote.host.clone();
        let p = path.clone();
        let task = cx.background_executor().spawn(async move { host.stat(&p).map(|m| m.kind == proto::Kind::Dir) });
        cx.spawn_in(window, async move |this, cx| {
            let is_dir = task.await;
            this.update_in(cx, |this, window, cx| match is_dir {
                Ok(true) => {
                    this.chrome.panel = Some(super::Panel::Explorer);
                    this.roots.primary.update(cx, |t, cx| t.reveal_dir(&path, cx));
                    cx.notify();
                }
                Ok(false) => this.open_file(path, window, cx),
                Err(e) => {
                    this.notice = Some(Notice::Error(format!("{rel}: {e}").into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

// --- Line bookmarks ---

impl Workbench {
    /// Place saved bookmarks when an editor opens.
    pub(super) fn load_bookmarks(&self, ed: &gpui::Entity<crate::editor::EditorView>, cx: &mut Context<Self>) {
        let (Some(s), Some(ws)) = (&self.persist.state, self.persist.workspace_id) else { return };
        let Some(rel) = self.rel(&ed.read(cx).path, cx) else { return };
        let lines: Vec<usize> = s.bookmarks(ws).unwrap_or_default().into_iter().filter(|b| b.rel_path == rel).map(|b| b.line).collect();
        if !lines.is_empty() {
            ed.update(cx, |e, cx| e.set_bookmark_lines(&lines, cx));
        }
    }

    /// Save bookmarks (on change, and on session save: lines shifted by edits).
    pub(super) fn save_bookmarks(&self, ed: &gpui::Entity<crate::editor::EditorView>, cx: &gpui::App) {
        let (Some(s), Some(ws)) = (&self.persist.state, self.persist.workspace_id) else { return };
        let e = ed.read(cx);
        let Some(rel) = self.rel(&e.path, cx) else { return };
        let lines: Vec<(usize, Option<String>)> = e.bookmark_lines().into_iter().map(|l| (l, None)).collect();
        let _ = s.save_file_bookmarks(ws, &rel, &lines);
    }

    /// "Bookmarks: List…": all bookmarks in the project (current lines in open files).
    pub(super) fn show_bookmarks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(s), Some(ws), Some(root)) = (&self.persist.state, self.persist.workspace_id, self.root(cx)) else { return };
        for ed in self.all_editors() {
            self.save_bookmarks(&ed, cx);
        }
        let marks = s.bookmarks(ws).unwrap_or_default();
        if marks.is_empty() {
            self.notice = Some(Notice::Info(t("No bookmarks yet: ⌥⌘K toggles one on the cursor's line.").into()));
            cx.notify();
            return;
        }
        let locs = marks.into_iter().map(|b| (ui::join(&root, &b.rel_path), (b.line as u32, 0))).collect();
        self.show_locations(locs, "bookmarks", window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_file_round_trip() {
        let p = vec![
            Project { name: "Kod".into(), path: "/a/kod".into(), host: None, tags: vec!["rust".into()] },
            Project { name: "Sunucu".into(), path: "/srv/app".into(), host: Some("node-dev".into()), tags: vec![] },
        ];
        let text = write_projects(&p);
        assert!(text.starts_with("//"), "comment line");
        assert_eq!(parse_projects(&text), p);
        assert_eq!(parse_projects(r#"[{ "path": "/x/y" }]"#)[0].name, "y", "folder name when there is no name");
    }

    fn recent(host: Option<&str>, path: &str, ago_secs: i64) -> RecentWorkspace {
        RecentWorkspace { host: host.map(str::to_string), path: path.into(), last_opened_at: 1_000_000_000 - ago_secs * 1000 }
    }

    fn labels(rows: &[(PickItem, SwitchChoice)]) -> Vec<String> {
        rows.iter().map(|(i, _)| i.label.to_string()).collect()
    }

    fn input<'a>(saved: &'a [Project], recent_list: &'a [RecentWorkspace], hosts: &'a [(String, Option<String>)], conn: ConnSummary) -> SwitcherInput<'a> {
        SwitcherInput { docker_enabled: true, saved, recent: recent_list, pinned: &[], hosts, conn, current_root: Some("/a/kod"), now_ms: 1_000_000_000 }
    }

    #[test]
    fn the_switcher_lists_projects_recent_hosts_and_actions_in_one_list_in_that_order() {
        let saved = vec![Project { name: "Kod".into(), path: "/a/kod".into(), host: None, tags: vec!["rust".into()] }];
        let recents = vec![recent(None, "/a/kod", 60), recent(Some("devbox"), "/srv/app", 7200), recent(None, "/b/notes", 86_400 * 4)];
        let hosts = vec![("devbox".to_string(), Some("me@10.0.0.5".to_string())), ("build".to_string(), None)];
        let rows = build_switcher(&input(&saved, &recents, &hosts, ConnSummary::Local));
        let l = labels(&rows);
        assert_eq!(&l[..4], ["★ Kod", "app", "notes", "devbox"], "{l:?}");
        assert!(l.contains(&"build".to_string()));
        assert_eq!(
            &l[l.len() - 6..],
            ["Open Folder…", "Connect to SSH host…", "Add SSH host…", "Manage SSH hosts…", "Clone Git Repository…", "Connect to a running container…"]
                .map(String::from)
                .as_slice(),
            "{l:?}"
        );
        // A saved project is not repeated in recents; a remote recent comes with its server; time is written short.
        assert_eq!(l.iter().filter(|x| x.contains("kod")).count(), 0, "{l:?}");
        let detail = |label: &str| rows.iter().find(|(i, _)| i.label == label).and_then(|(i, _)| i.detail.clone()).map(|d| d.to_string()).unwrap_or_default();
        assert!(detail("app").starts_with("devbox:/srv/app") && detail("app").contains("2h"), "{}", detail("app"));
        assert!(detail("notes").contains("4d"), "{}", detail("notes"));
        assert!(detail("devbox").contains("SSH host") && detail("devbox").contains("me@10.0.0.5"), "{}", detail("devbox"));
    }

    #[test]
    fn the_switcher_shows_connection_state_and_the_right_action() {
        let hosts = vec![("devbox".to_string(), Some("me@10.0.0.5".to_string())), ("build".to_string(), None)];
        // Connected: the disconnect row is on top, the server row has ●.
        let rows = build_switcher(&input(&[], &[], &hosts, ConnSummary::Connected("devbox".into())));
        assert_eq!(labels(&rows)[0], "Disconnect from devbox");
        assert!(matches!(rows[0].1, SwitchChoice::Disconnect));
        assert!(labels(&rows).contains(&"● devbox".to_string()), "{:?}", labels(&rows));
        assert!(!labels(&rows).iter().any(|l| l.starts_with("Reconnect")));
        // Dropped: "Reconnect" with the reason on top, the server row has ✖ and the reason.
        let rows = build_switcher(&input(&[], &[], &hosts, ConnSummary::Failed("devbox".into(), "Connection lost: broken pipe".into())));
        assert_eq!(labels(&rows)[0], "Reconnect to devbox");
        assert!(rows[0].0.detail.as_ref().unwrap().contains("broken pipe"));
        let host_row = rows.iter().find(|(i, _)| i.label == "✖ devbox").expect("failed host row");
        assert!(host_row.0.detail.as_ref().unwrap().contains("broken pipe"));
        // Local: no connection action.
        let rows = build_switcher(&input(&[], &[], &hosts, ConnSummary::Local));
        assert!(!labels(&rows).iter().any(|l| l.starts_with("Disconnect") || l.starts_with("Reconnect")));
        // Connecting / reconnecting: the state is shown, no extra action.
        let rows = build_switcher(&input(&[], &[], &hosts, ConnSummary::Reconnecting("build".into())));
        let r = rows.iter().find(|(i, _)| i.label == "… build").expect("reconnecting row");
        assert!(r.0.detail.as_ref().unwrap().contains("reconnecting"));
    }

    #[test]
    fn every_row_has_a_choice_and_every_host_connects() {
        let hosts = vec![("a".to_string(), None), ("b".to_string(), None)];
        let rows = build_switcher(&input(&[], &[recent(None, "/x", 5)], &hosts, ConnSummary::Local));
        assert_eq!(rows.iter().filter(|(_, c)| matches!(c, SwitchChoice::Connect(a) if a == "a" || a == "b")).count(), 2);
        assert!(rows.iter().all(|(i, _)| !i.label.is_empty()));
    }

    #[test]
    fn docker_module_off_hides_container_actions_projects_and_recents() {
        let recents =
            vec![recent(Some("docker:web"), "/app", 1), recent(Some("devcontainer:default/abc/def"), "/workspace", 2), recent(Some("devbox"), "/srv", 3)];
        let saved = vec![Project { name: "Container".into(), path: "/app".into(), host: Some("docker-context:remote/abc".into()), tags: vec![] }];
        let mut input = input(&saved, &recents, &[], ConnSummary::Local);
        input.docker_enabled = false;
        let rows = build_switcher(&input);
        assert!(!rows.iter().any(|(_, c)| matches!(c, SwitchChoice::Targets { wsl: false })));
        assert!(!rows.iter().any(|(_, c)| matches!(c, SwitchChoice::Open(r) if r.host.as_deref().is_some_and(crate::docker::is_target))));
        assert!(rows.iter().any(|(_, c)| matches!(c, SwitchChoice::Open(r) if r.host.as_deref() == Some("devbox"))));
    }

    #[test]
    fn pinned_recents_come_first_in_pin_order_and_the_rest_keep_recency() {
        let recents = vec![recent(None, "/c", 10), recent(None, "/a", 20), recent(Some("devbox"), "/srv", 30), recent(None, "/b", 40)];
        let pinned = vec![(Some("devbox".to_string()), "/srv".to_string()), (None, "/b".to_string())];
        let ordered = ordered_recent(&recents, &pinned);
        let flat: Vec<(String, bool)> = ordered.iter().map(|(r, p)| (r.path.clone(), *p)).collect();
        assert_eq!(flat, vec![("/srv".into(), true), ("/b".into(), true), ("/c".into(), false), ("/a".into(), false)]);
        // A pinned entry that is missing from the list (deleted) produces no ghost row; with no pins the order is unchanged.
        let ghost = vec![(None, "/gone".to_string())];
        assert_eq!(ordered_recent(&recents, &ghost).len(), recents.len());
        assert!(ordered_recent(&recents, &[]).iter().all(|(_, p)| !p));
        assert_eq!(ordered_recent(&recents, &[]).iter().map(|(r, _)| r.path.as_str()).collect::<Vec<_>>(), vec!["/c", "/a", "/srv", "/b"]);
        // The same path on two hosts is a different entry.
        let both = vec![recent(None, "/x", 1), recent(Some("h"), "/x", 2)];
        let ordered = ordered_recent(&both, &[(Some("h".to_string()), "/x".to_string())]);
        assert_eq!((ordered[0].0.host.as_deref(), ordered[0].1), (Some("h"), true));
        assert_eq!((ordered[1].0.host.as_deref(), ordered[1].1), (None, false));
    }

    #[test]
    fn the_switcher_marks_pinned_recents_and_lists_them_first() {
        let recents = vec![recent(None, "/a/one", 10), recent(None, "/a/two", 20)];
        let pinned = vec![(None, "/a/two".to_string())];
        let mut i = input(&[], &recents, &[], ConnSummary::Local);
        i.pinned = &pinned;
        let rows = build_switcher(&i);
        assert_eq!(labels(&rows)[..2], ["two", "one"]);
        assert!(rows[0].0.detail.as_ref().unwrap().contains("pinned"));
        assert!(!rows[1].0.detail.as_ref().unwrap().contains("pinned"));
    }
}
