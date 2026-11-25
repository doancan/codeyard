//! Read project conversations through the local Codex App Server.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command};
use std::sync::mpsc::Receiver;

use serde_json::{Value, json};

use super::agent_rpc::{exchange, receive, result, send};
use super::claude_store::{Provider, Session, short_title};

pub(super) fn env(home: &Path) -> Vec<(String, String)> {
    let home = crate::ui::local_path(home);
    vec![("CODEX_HOME".into(), home.clone()), ("CODEX_SQLITE_HOME".into(), home)]
}

pub(super) fn watch_paths(home: &Path) -> Vec<(String, PathBuf)> {
    let sessions = home.join("sessions");
    let index = home.join("session_index.jsonl");
    let mut dirs = vec![sessions.clone(), index];
    // Rollouts are filed by year, month, and day. Watch the recent folder hierarchy so a newly
    // created rollout refreshes the panel without observing SQLite writes made by the reader itself.
    for year in newest_directories(&sessions, 4) {
        for month in newest_directories(&year, 2) {
            dirs.extend(newest_directories(&month, 2));
            dirs.push(month);
        }
        dirs.push(year);
    }
    dirs.into_iter().map(|path| (path.to_string_lossy().into_owned(), path)).collect()
}

fn newest_directories(root: &Path, width: usize) -> Vec<PathBuf> {
    let mut dirs: Vec<_> = root
        .read_dir()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (name.len() == width && name.bytes().all(|byte| byte.is_ascii_digit()) && entry.file_type().is_ok_and(|kind| kind.is_dir())).then(|| entry.path())
        })
        .collect();
    dirs.sort_unstable_by(|a, b| b.cmp(a));
    dirs.truncate(2);
    dirs
}

/// Ask Codex to list its saved interactive sessions for one directory. No turn is started.
pub(super) fn sessions(home: &Path, folder: &Path) -> io::Result<Vec<Session>> {
    if !home.is_dir() {
        return Ok(Vec::new());
    }
    let home = home.canonicalize()?;
    let program = Provider::Codex.executable().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Codex executable not found"))?;
    let mut cmd = Command::new(program);
    cmd.args(["app-server", "--listen", "stdio://"]).current_dir(folder).envs(env(&home)).env_remove("CODEX_THREAD_ID");
    for key in Provider::Codex.credentials_env() {
        cmd.env_remove(key);
    }
    let response = exchange(cmd, |input, responses| list_threads(input, responses, folder))?;
    let result = result(&response)?;
    let data = result["data"].as_array().ok_or_else(|| io::Error::other("Codex returned an invalid session list"))?;
    Ok(session_rows(data))
}

fn list_threads(input: &mut ChildStdin, responses: &Receiver<Value>, folder: &Path) -> io::Result<Value> {
    send(
        input,
        &json!({
            "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "codeyard", "title": "Codeyard", "version": env!("CARGO_PKG_VERSION")}}
        }),
    )?;
    result(&receive(responses, 1)?)?;
    send(input, &json!({"method": "initialized"}))?;
    send(
        input,
        &json!({
            "id": 2, "method": "thread/list",
            "params": {"cwd": crate::ui::local_path(folder), "limit": 100, "sortKey": "updated_at", "modelProviders": [], "sourceKinds": ["cli", "vscode", "appServer"]}
        }),
    )?;
    receive(responses, 2)
}

fn session_rows(data: &[Value]) -> Vec<Session> {
    data.iter()
        .filter_map(|thread| {
            let id = thread["id"].as_str()?.to_owned();
            if id.is_empty() || id.starts_with('-') {
                return None;
            }
            let title = thread["name"]
                .as_str()
                .and_then(short_title)
                .or_else(|| thread["preview"].as_str().and_then(short_title))
                .unwrap_or_else(|| id.chars().take(8).collect());
            let path = thread["path"].as_str().map(PathBuf::from);
            Some(Session { id, title, path })
        })
        .collect()
}

#[cfg(test)]
#[test]
fn codex_history_uses_the_thread_name_then_the_first_prompt() {
    let rows = session_rows(
        json!([
            {"id": "id-1", "name": "My title", "preview": "First prompt"},
            {"id": "id-2", "name": null, "preview": "Another\n prompt"},
            {"id": "--bad", "preview": "Malformed ID"}
        ])
        .as_array()
        .expect("sample rows"),
    );
    assert_eq!(
        rows,
        [Session { id: "id-1".into(), title: "My title".into(), path: None }, Session { id: "id-2".into(), title: "Another prompt".into(), path: None }]
    );
}
