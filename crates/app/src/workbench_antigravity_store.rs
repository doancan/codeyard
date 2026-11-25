//! List workspace conversations through Antigravity's Agent Client Protocol server.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use super::agent_rpc::{exchange, receive, result, send};
use super::claude_store::{Provider, Session, short_title, valid_session_id};

/// Observe the CLI's documented last-conversation cache to refresh active workspace history.
pub(super) fn watch_paths() -> Vec<(String, PathBuf)> {
    dirs::home_dir()
        .map(|home| ("antigravity-last-conversations".into(), home.join(".gemini/antigravity-cli/cache/last_conversations.json")))
        .into_iter()
        .collect()
}

/// List local workspace conversations without starting a turn or launching sign-in.
pub(super) fn sessions(folder: &Path) -> io::Result<Vec<Session>> {
    let Some(program) = Provider::Antigravity.executable() else { return Ok(Vec::new()) };
    let mut cmd = Command::new(program);
    cmd.arg("acp").current_dir(folder);
    let cwd = crate::ui::local_path(folder);
    exchange(cmd, |input, responses| {
        send(input, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}))?;
        let init = receive(responses, 1)?;
        if result(&init)?.pointer("/agentCapabilities/sessionCapabilities/list").is_none() {
            return Ok(Vec::new());
        }

        let mut rows = Vec::new();
        let mut cursor = None;
        for id in 2..=5 {
            let params = cursor.as_ref().map_or_else(|| json!({"cwd":cwd}), |cursor| json!({"cwd":cwd,"cursor":cursor}));
            send(input, &json!({"jsonrpc":"2.0","id":id,"method":"session/list","params":params}))?;
            let response = receive(responses, id)?;
            // The pre-login state is normal and interactive agy owns the authentication flow.
            if response["error"]["code"] == -32000 && response["error"]["message"] == "Authentication required" {
                return Ok(Vec::new());
            }
            let data = result(&response)?;
            let items = data["sessions"].as_array().ok_or_else(|| io::Error::other("Antigravity returned an invalid session list"))?;
            rows.extend(items.iter().filter_map(session_row).take(100 - rows.len()));
            let next = data["nextCursor"].as_str().map(str::to_owned);
            if rows.len() == 100 || next.is_none() || next == cursor {
                break;
            }
            cursor = next;
        }
        Ok(rows)
    })
}

fn session_row(item: &Value) -> Option<Session> {
    let id = item["sessionId"].as_str().filter(|id| valid_session_id(id))?.to_owned();
    let title = item["title"].as_str().and_then(short_title).unwrap_or_else(|| id.chars().take(8).collect());
    Some(Session { id, title, path: None })
}

#[cfg(test)]
#[test]
fn antigravity_acp_sessions_can_resume_with_a_valid_id() {
    let id = "a1b2c3d4-e5f6-4890-abcd-ef1234567890";
    let row = session_row(&json!({"sessionId": id, "title": "Fix the account picker"})).expect("valid session");
    assert_eq!(row, Session { id: id.into(), title: "Fix the account picker".into(), path: None });
    assert!(session_row(&json!({"sessionId": "--new-project"})).is_none());
}
