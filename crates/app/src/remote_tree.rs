//! Rows of the Remote Explorer view, built from plain data so the tree can be tested without a window:
//! SSH hosts (each with its recent folders and connection status), containers and WSL distributions (listed on
//! demand), and the forwarded ports of the current connection.

use std::collections::{HashMap, HashSet};

use state::{HostRecord, RecentWorkspace};

use super::super::ports_glue::PortRow;
use super::super::projects::{ConnSummary, ordered_recent};

/// A group of targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Ssh,
    Containers,
    Wsl,
}

impl Group {
    pub fn key(self) -> &'static str {
        match self {
            Group::Ssh => "group:ssh",
            Group::Containers => "group:containers",
            Group::Wsl => "group:wsl",
        }
    }
}

/// What is known about a host's connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostStatus {
    Connected,
    Connecting,
    Reconnecting,
    Failed(String),
    /// Connected successfully before (unix ms).
    Reachable(i64),
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteRow {
    Group {
        group: Group,
        count: usize,
        expanded: bool,
    },
    Host {
        group: Group,
        alias: String,
        detail: Option<String>,
        status: HostStatus,
        expanded: bool,
    },
    Folder {
        alias: String,
        path: String,
        current: bool,
        pinned: bool,
        last_opened: i64,
    },
    /// A host without recent folders.
    NoFolders {
        alias: String,
    },
    /// A group without targets, or why it could not be listed.
    Empty {
        group: Group,
        message: Option<String>,
    },
    Loading {
        group: Group,
    },
    PortsHeader {
        count: usize,
    },
    Port(PortRow),
}

/// Listed containers or WSL distributions: (alias such as `docker:web`, name, detail), or why they could not be listed.
pub type Targets = Result<Vec<(String, String, String)>, String>;

pub struct RemoteTreeInput<'a> {
    /// (alias, `user@host`).
    pub hosts: &'a [(String, Option<String>)],
    pub recent: &'a [RecentWorkspace],
    pub pinned: &'a [(Option<String>, String)],
    pub conn: ConnSummary,
    pub records: &'a HashMap<String, HostRecord>,
    /// Result of "Test Connection" this session: `Err` holds the reason.
    pub probes: &'a HashMap<String, Result<(), String>>,
    /// `None`: not listed yet (the group is collapsed, or listing is running).
    pub containers: Option<&'a Targets>,
    pub wsl: Option<&'a Targets>,
    pub show_wsl: bool,
    /// Keys whose open/closed state the user flipped from the default.
    pub toggled: &'a HashSet<String>,
    pub current_root: Option<&'a str>,
    /// Forwarded and listening ports while connected over SSH.
    pub ports: Option<Vec<PortRow>>,
}

pub fn host_key(alias: &str) -> String {
    format!("host:{alias}")
}

/// Open by default: the SSH group, and the host this window is connected to.
fn default_open(key: &str, conn: &ConnSummary) -> bool {
    match key {
        "group:ssh" => true,
        k => matches!(conn, ConnSummary::Connected(a) | ConnSummary::Connecting(a) | ConnSummary::Reconnecting(a) if k == host_key(a)),
    }
}

pub fn is_open(key: &str, i: &RemoteTreeInput) -> bool {
    default_open(key, &i.conn) != i.toggled.contains(key)
}

fn status(alias: &str, i: &RemoteTreeInput) -> HostStatus {
    match &i.conn {
        ConnSummary::Connected(a) if a == alias => return HostStatus::Connected,
        ConnSummary::Connecting(a) if a == alias => return HostStatus::Connecting,
        ConnSummary::Reconnecting(a) if a == alias => return HostStatus::Reconnecting,
        ConnSummary::Failed(a, why) if a == alias => return HostStatus::Failed(why.clone()),
        _ => {}
    }
    match (i.probes.get(alias), i.records.get(alias)) {
        (Some(Err(why)), _) => HostStatus::Failed(why.clone()),
        (Some(Ok(())), r) => HostStatus::Reachable(r.and_then(|r| r.last_ok_at).unwrap_or(0)),
        (None, Some(HostRecord { last_error: Some(e), .. })) => HostStatus::Failed(e.clone()),
        (None, Some(HostRecord { last_ok_at: Some(t), .. })) => HostStatus::Reachable(*t),
        _ => HostStatus::Unknown,
    }
}

/// Recent folders of one host: pinned first, then by last opened.
fn folders(alias: &str, i: &RemoteTreeInput, out: &mut Vec<RemoteRow>) {
    let ordered = ordered_recent(i.recent, i.pinned);
    let mut any = false;
    for (r, pinned) in ordered.into_iter().filter(|(r, _)| r.host.as_deref() == Some(alias)) {
        any = true;
        let current = matches!(&i.conn, ConnSummary::Connected(a) if a == alias) && i.current_root == Some(r.path.as_str());
        out.push(RemoteRow::Folder { alias: alias.into(), path: r.path, current, pinned, last_opened: r.last_opened_at });
    }
    if !any {
        out.push(RemoteRow::NoFolders { alias: alias.into() });
    }
}

fn group(g: Group, hosts: Vec<(String, Option<String>)>, listed: Option<&Targets>, i: &RemoteTreeInput, out: &mut Vec<RemoteRow>) {
    let open = is_open(g.key(), i);
    out.push(RemoteRow::Group { group: g, count: hosts.len(), expanded: open });
    if !open {
        return;
    }
    match listed {
        _ if g == Group::Ssh => {}
        None => return out.push(RemoteRow::Loading { group: g }),
        Some(Err(why)) => return out.push(RemoteRow::Empty { group: g, message: Some(why.clone()) }),
        Some(Ok(_)) => {}
    }
    if hosts.is_empty() {
        out.push(RemoteRow::Empty { group: g, message: None });
    }
    for (alias, detail) in hosts {
        let expanded = is_open(&host_key(&alias), i);
        out.push(RemoteRow::Host { group: g, alias: alias.clone(), detail, status: status(&alias, i), expanded });
        if expanded {
            folders(&alias, i, out);
        }
    }
}

pub fn build_remote_tree(i: &RemoteTreeInput) -> Vec<RemoteRow> {
    let mut out = Vec::new();
    group(Group::Ssh, i.hosts.to_vec(), None, i, &mut out);
    let listed =
        |t: Option<&Targets>| t.and_then(|r| r.as_ref().ok()).map(|v| v.iter().map(|(a, _, d)| (a.clone(), Some(d.clone()))).collect()).unwrap_or_default();
    group(Group::Containers, listed(i.containers), i.containers, i, &mut out);
    if i.show_wsl {
        group(Group::Wsl, listed(i.wsl), i.wsl, i, &mut out);
    }
    if let Some(ports) = &i.ports {
        out.push(RemoteRow::PortsHeader { count: ports.len() });
        out.extend(ports.iter().cloned().map(RemoteRow::Port));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recent(host: &str, path: &str, at: i64) -> RecentWorkspace {
        RecentWorkspace { host: Some(host.into()), path: path.into(), last_opened_at: at }
    }

    struct Fixture {
        hosts: Vec<(String, Option<String>)>,
        recent: Vec<RecentWorkspace>,
        pinned: Vec<(Option<String>, String)>,
        records: HashMap<String, HostRecord>,
        probes: HashMap<String, Result<(), String>>,
        toggled: HashSet<String>,
    }

    fn fixture() -> Fixture {
        Fixture {
            hosts: vec![("box".into(), Some("me@box.lan".into())), ("db".into(), None), ("old".into(), None)],
            recent: vec![
                recent("box", "/srv/app", 30),
                recent("box", "/srv/api", 20),
                recent("db", "/var/db", 10),
                RecentWorkspace { host: None, path: "/tmp".into(), last_opened_at: 5 },
            ],
            pinned: vec![(Some("box".into()), "/srv/api".into())],
            records: HashMap::from([("old".into(), HostRecord { last_ok_at: None, last_error: Some("Connection refused".into()) })]),
            probes: HashMap::new(),
            toggled: HashSet::new(),
        }
    }

    fn input<'a>(f: &'a Fixture, conn: ConnSummary, containers: Option<&'a Targets>) -> RemoteTreeInput<'a> {
        RemoteTreeInput {
            hosts: &f.hosts,
            recent: &f.recent,
            pinned: &f.pinned,
            conn,
            records: &f.records,
            probes: &f.probes,
            containers,
            wsl: None,
            show_wsl: false,
            toggled: &f.toggled,
            current_root: Some("/srv/app"),
            ports: None,
        }
    }

    #[test]
    fn the_connected_host_is_open_with_pinned_folders_first_and_the_current_one_marked() {
        let f = fixture();
        let rows = build_remote_tree(&input(&f, ConnSummary::Connected("box".into()), None));
        assert_eq!(rows[0], RemoteRow::Group { group: Group::Ssh, count: 3, expanded: true });
        assert!(matches!(&rows[1], RemoteRow::Host { alias, status: HostStatus::Connected, expanded: true, .. } if alias == "box"));
        assert!(matches!(&rows[2], RemoteRow::Folder { path, pinned: true, current: false, .. } if path == "/srv/api"));
        assert!(matches!(&rows[3], RemoteRow::Folder { path, pinned: false, current: true, .. } if path == "/srv/app"));
        assert!(matches!(&rows[4], RemoteRow::Host { alias, expanded: false, .. } if alias == "db"), "other hosts stay closed");
        assert!(matches!(&rows[5], RemoteRow::Host { alias, status: HostStatus::Failed(why), .. } if alias == "old" && why.contains("refused")));
        assert_eq!(rows[6], RemoteRow::Group { group: Group::Containers, count: 0, expanded: false });
        assert_eq!(rows.len(), 7, "local folders never appear");
    }

    #[test]
    fn toggling_flips_the_default_and_a_host_without_folders_says_so() {
        let mut f = fixture();
        f.toggled.extend([host_key("old"), "group:ssh".into()]);
        let rows = build_remote_tree(&input(&f, ConnSummary::Local, None));
        assert_eq!(rows[0], RemoteRow::Group { group: Group::Ssh, count: 3, expanded: false });
        f.toggled.remove("group:ssh");
        let rows = build_remote_tree(&input(&f, ConnSummary::Local, None));
        let at = rows.iter().position(|r| matches!(r, RemoteRow::Host { alias, .. } if alias == "old")).unwrap();
        assert_eq!(rows[at + 1], RemoteRow::NoFolders { alias: "old".into() });
    }

    #[test]
    fn a_live_state_or_a_test_result_beats_the_saved_record() {
        let mut f = fixture();
        f.probes.insert("old".into(), Ok(()));
        f.probes.insert("db".into(), Err("Permission denied".into()));
        let rows = build_remote_tree(&input(&f, ConnSummary::Failed("box".into(), "timed out".into()), None));
        let status = |alias: &str| {
            rows.iter()
                .find_map(|r| match r {
                    RemoteRow::Host { alias: a, status, .. } if a == alias => Some(status.clone()),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(status("box"), HostStatus::Failed("timed out".into()));
        assert_eq!(status("db"), HostStatus::Failed("Permission denied".into()));
        assert!(matches!(status("old"), HostStatus::Reachable(_)));
    }

    #[test]
    fn containers_show_loading_an_error_or_their_targets_when_opened() {
        let mut f = fixture();
        f.toggled.insert("group:containers".into());
        let rows = |c: Option<&Targets>, f: &Fixture| build_remote_tree(&input(f, ConnSummary::Local, c));
        assert!(rows(None, &f).contains(&RemoteRow::Loading { group: Group::Containers }));
        let err: Targets = Err("docker: command not found".into());
        assert!(rows(Some(&err), &f).contains(&RemoteRow::Empty { group: Group::Containers, message: Some("docker: command not found".into()) }));
        let none: Targets = Ok(Vec::new());
        assert!(rows(Some(&none), &f).contains(&RemoteRow::Empty { group: Group::Containers, message: None }));
        let some: Targets = Ok(vec![("docker:web".into(), "web".into(), "nginx:1.27".into())]);
        assert!(rows(Some(&some), &f).iter().any(|r| matches!(r, RemoteRow::Host { group: Group::Containers, alias, .. } if alias == "docker:web")));
    }
}
