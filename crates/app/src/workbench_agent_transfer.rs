//! Local handoff of an agent's saved conversation.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

use super::claude_store::{Provider, new_session_id};

/// Copy the latest linear history to another account and preserve divergent copies.
pub(super) fn copy_transcript(source: &Path, dest: &Path) -> io::Result<()> {
    let exists = match dest.symlink_metadata() {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => return Err(io::Error::other("The destination is not a regular session file")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e),
    };
    if exists {
        if source == dest.canonicalize()? {
            return Ok(());
        }
        let source_len = source.metadata()?.len();
        let dest_len = dest.metadata()?.len();
        if !same_prefix(source, dest, source_len.min(dest_len))? {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "The destination account has a different version of this session"));
        }
        if dest_len >= source_len {
            return Ok(());
        }
    }

    let nonce = new_session_id().map_err(|_| io::Error::other("Could not create a temporary session file"))?;
    let temp = dest.with_file_name(format!(".handoff-{nonce}.tmp"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut output = options.open(&temp)?;
    let result = (|| {
        io::copy(&mut File::open(source)?, &mut output)?;
        output.sync_all()?;
        drop(output);
        if exists { fs::rename(&temp, dest) } else { fs::hard_link(&temp, dest) }
    })();
    let _ = fs::remove_file(&temp);
    result
}

fn same_prefix(source: &Path, dest: &Path, mut len: u64) -> io::Result<bool> {
    let mut source = File::open(source)?;
    let mut dest = File::open(dest)?;
    let (mut a, mut b) = ([0u8; 8192], [0u8; 8192]);
    while len > 0 {
        let size = len.min(a.len() as u64) as usize;
        source.read_exact(&mut a[..size])?;
        dest.read_exact(&mut b[..size])?;
        if a[..size] != b[..size] {
            return Ok(false);
        }
        len -= size as u64;
    }
    Ok(true)
}

/// Extract recent visible messages for an explicitly requested clipboard handoff.
pub(super) fn context(path: &Path, provider: Provider, title: &str) -> io::Result<String> {
    const BYTES: u64 = 1024 * 1024;
    let mut file = File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file.take(BYTES));
    if start > 0 {
        reader.skip_until(b'\n')?;
    }
    let mut messages = VecDeque::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some((role, text)) = visible_message(&entry, provider) {
            keep_recent(&mut messages, (role, text));
        }
    }
    format_context(provider, title, messages)
}

fn keep_recent<T>(messages: &mut VecDeque<T>, message: T) {
    if messages.len() == 12 {
        messages.pop_front();
    }
    messages.push_back(message);
}

fn format_context(provider: Provider, title: &str, messages: impl IntoIterator<Item = (&'static str, String)>) -> io::Result<String> {
    let mut messages = messages.into_iter().peekable();
    if messages.peek().is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "The local transcript has no supported messages"));
    }
    let mut copy = format!("# {} · {title}\n", provider.label());
    for (role, text) in messages {
        use std::fmt::Write as _;
        let _ = write!(copy, "\n## {role}\n\n{text}\n");
    }
    Ok(copy)
}

fn visible_message(entry: &Value, provider: Provider) -> Option<(&'static str, String)> {
    let (role, content) = match provider {
        Provider::Claude if (entry["type"] == "user" || entry["type"] == "assistant") && entry["isMeta"] != true => {
            (entry["type"].as_str()?, &entry["message"]["content"])
        }
        Provider::Codex if entry["type"] == "response_item" && entry["payload"]["type"] == "message" => {
            (entry["payload"]["role"].as_str()?, &entry["payload"]["content"])
        }
        _ => return None,
    };
    let user = role == "user";
    let role = match role {
        "user" => crate::i18n::t("User message"),
        "assistant" => crate::i18n::t("Assistant response"),
        _ => return None,
    };
    let text = if let Some(text) = content.as_str() {
        text.to_owned()
    } else {
        content
            .as_array()?
            .iter()
            .filter_map(|block| match block["type"].as_str()? {
                "text" | "input_text" | "output_text" => block["text"].as_str(),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let text = text.trim();
    let internal =
        ["<environment_context>", "<system-reminder>", "<local-command-caveat>", "<local-command-stdout>", "<command-name>", "# AGENTS.md instructions"];
    if text.is_empty() || (user && internal.iter().any(|marker| text.starts_with(marker))) {
        return None;
    }
    Some((role, text.chars().take(2400).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_extends_linear_history_and_preserves_conflicting_versions() {
        let folder = crate::qa_workbench_tests::fresh("handoff-history");
        let (a, b) = (folder.join("a.jsonl"), folder.join("b.jsonl"));
        fs::write(&a, "original\n").expect("first account");
        copy_transcript(&a, &b).expect("copy into second account");
        fs::write(&b, "original\ncontinued\n").expect("append on the second account");
        copy_transcript(&b, &a).expect("bring back the longer conversation");
        assert_eq!(fs::read_to_string(&a).expect("first account"), "original\ncontinued\n");
        fs::write(&b, "original\nother branch\n").expect("divergent update");
        assert!(copy_transcript(&b, &a).is_err());
        assert_eq!(fs::read_to_string(&a).expect("kept its history"), "original\ncontinued\n");
    }

    #[test]
    fn context_copies_visible_text_and_omits_internal_records() {
        let folder = crate::qa_workbench_tests::fresh("handoff-text");
        let file = folder.join("test.jsonl");
        fs::write(&file, concat!(
            "{\"type\":\"user\",\"message\":{\"content\":\"<button>Fix the issue</button>\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"thinking\",\"thinking\":\"hidden\"},{\"type\":\"text\",\"text\":\"Handled\"}]}}\n",
            "{\"type\":\"user\",\"isMeta\":true,\"message\":{\"content\":\"internal\"}}\n",
        )).expect("fixture");
        let copy = context(&file, Provider::Claude, "Original").expect("plain text");
        assert!(copy.contains("<button>Fix the issue</button>") && copy.contains("Handled"));
        assert!(!copy.contains("hidden") && !copy.contains("internal"));
    }
}
