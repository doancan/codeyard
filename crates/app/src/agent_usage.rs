//! Tokens a Claude Code account spent recently, counted from the session files the tool keeps for the account
//! (`<profile>/projects/**/*.jsonl`, one JSON object per line; an assistant line carries `message.usage`).
//!
//! The tools give no number for "how much of the plan is left", so this is not a percentage: it is how much one account was used
//! compared with another, which is what choosing the account to continue on needs. Only the account's own (private) history
//! counts: shared history is one folder for every account and cannot be told apart, and links are never followed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// The window the count covers: the shortest quota window the tools use.
pub const WINDOW_SECS: i64 = 5 * 60 * 60;

/// Most files looked into: a profile with years of history reads only its newest sessions.
const MAX_FILES: usize = 200;

/// Deepest folder level read below `projects` (`projects/<name>/<session>.jsonl`, with room for a layout change).
const MAX_DEPTH: usize = 3;

/// Tokens spent in the last [`WINDOW_SECS`] before `now`: input, output and cache writes (cache reads are not counted: they cost
/// next to nothing against a quota).
pub fn claude_tokens(profile_dir: &Path, now: i64) -> u64 {
    let since = now - WINDOW_SECS;
    let mut files = Vec::new();
    collect(&profile_dir.join("projects"), since, 0, &mut files);
    // Newest first, so the cap drops the oldest.
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    files.truncate(MAX_FILES);
    // A message is written once per streamed update with a growing count: the largest one per message is its total.
    let mut per_message: HashMap<String, u64> = HashMap::new();
    for (_, path) in files {
        let Ok(file) = std::fs::File::open(&path) else { continue };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if !line.contains("\"usage\"") {
                continue;
            }
            let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if entry["type"] != "assistant" || !entry["timestamp"].as_str().and_then(crate::github::parse_iso).is_some_and(|at| at >= since && at <= now) {
                continue;
            }
            let usage = &entry["message"]["usage"];
            let tokens: u64 = ["input_tokens", "output_tokens", "cache_creation_input_tokens"].iter().filter_map(|key| usage[*key].as_u64()).sum();
            let id = entry["message"]["id"].as_str().map_or_else(|| format!("{}:{}", path.display(), entry["uuid"]), str::to_string);
            let seen = per_message.entry(id).or_default();
            *seen = (*seen).max(tokens);
        }
    }
    per_message.values().sum()
}

/// Files below `dir` changed since `since`, newest information first is the caller's business.
fn collect(dir: &Path, since: i64, depth: usize, out: &mut Vec<(i64, std::path::PathBuf)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        // A link (shared history) is another account's too, and may loop.
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            collect(&path, since, depth + 1, out);
        } else if path.extension().is_some_and(|ext| ext == "jsonl")
            && let Some(modified) = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            && modified.as_secs() as i64 >= since
        {
            out.push((modified.as_secs() as i64, path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = crate::qa_workbench_tests::fresh(name);
        std::fs::create_dir_all(dir.join("projects/p")).unwrap();
        dir
    }

    fn line(id: &str, at: &str, input: u64, output: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","uuid":"u-{id}-{input}","message":{{"id":"{id}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_creation_input_tokens":10,"cache_read_input_tokens":99999}}}}}}"#
        )
    }

    /// 2026-11-25T12:00:00Z
    const NOW: i64 = 1_795_608_000;

    #[test]
    fn recent_assistant_messages_are_counted_without_cache_reads() {
        let d = dir("usage-count");
        let text = [line("a", "2026-11-25T11:00:00Z", 100, 50), line("b", "2026-11-25T11:30:00Z", 200, 25)].join("\n");
        std::fs::write(d.join("projects/p/s.jsonl"), text).unwrap();
        assert_eq!(claude_tokens(&d, NOW), (100 + 50 + 10) + (200 + 25 + 10));
    }

    #[test]
    fn a_message_written_again_with_a_growing_count_counts_once_at_its_largest() {
        let d = dir("usage-stream");
        let text = [line("a", "2026-11-25T11:00:00Z", 100, 5), line("a", "2026-11-25T11:00:01Z", 100, 60)].join("\n");
        std::fs::write(d.join("projects/p/s.jsonl"), text).unwrap();
        assert_eq!(claude_tokens(&d, NOW), 100 + 60 + 10);
    }

    #[test]
    fn messages_older_than_the_window_and_other_lines_are_left_out() {
        let d = dir("usage-old");
        let text = [
            line("old", "2026-11-25T06:00:00Z", 1000, 1000),
            r#"{"type":"user","timestamp":"2026-11-25T11:00:00Z","message":{"usage":{"input_tokens":7}}}"#.to_string(),
            "not json but mentions \"usage\"".to_string(),
            line("new", "2026-11-25T11:59:00Z", 1, 2),
        ]
        .join("\n");
        std::fs::write(d.join("projects/p/s.jsonl"), text).unwrap();
        assert_eq!(claude_tokens(&d, NOW), 1 + 2 + 10);
    }

    #[test]
    fn a_profile_without_sessions_has_used_nothing() {
        assert_eq!(claude_tokens(&crate::qa_workbench_tests::fresh("usage-none"), NOW), 0);
    }

    #[cfg(unix)]
    #[test]
    fn linked_history_is_not_followed() {
        let d = dir("usage-link");
        let shared = crate::qa_workbench_tests::fresh("usage-shared");
        std::fs::write(shared.join("s.jsonl"), line("shared", "2026-11-25T11:00:00Z", 500, 500)).unwrap();
        std::os::unix::fs::symlink(&shared, d.join("projects/linked")).unwrap();
        assert_eq!(claude_tokens(&d, NOW), 0, "the shared folder belongs to no one account");
    }
}
