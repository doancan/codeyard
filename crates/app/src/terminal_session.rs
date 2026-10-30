//! What is saved of a window's terminals so a restart can bring them back: their order, folders, titles, colors and the
//! program they ran, and optionally a bounded copy of their history. A terminal the terminal daemon still runs is taken
//! back with its saved token (the window's daemon session plus the terminal's own token, never a process id); any other
//! is restored as a new shell: nothing saved is ever sent to a terminal as input, and saved history is plain text with
//! every control sequence removed, so showing it cannot run anything or change the terminal's state. Environment
//! variables are never saved (they may hold secrets).

use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;
/// Terminals saved per window (the panel opens at most this many anyway).
pub const MAX_TERMINALS: usize = 8;
const MAX_TITLE: usize = 256;
/// History kept per terminal, at most, whatever the setting says.
pub const MAX_HISTORY_LINES: usize = 10_000;
const MAX_HISTORY_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedTerminal {
    /// Folder it was started in.
    pub cwd: Option<String>,
    /// Program and arguments (`None`: the user's shell / a login shell).
    pub shell: Option<String>,
    pub args: Option<Vec<String>>,
    pub color: Option<String>,
    /// A title the user gave it (`None`: the shell sets it).
    pub title: Option<String>,
    /// The last lines of its history, as plain text (only with `terminal.persistentScrollback`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<String>,
    /// The token to take it back from the terminal daemon, when the daemon kept it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<SavedPty>,
}

/// A terminal's token on the terminal daemon: its id in the window's session, the daemon's instance id and the
/// terminal's secret. All three must match for the daemon to hand the terminal back.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedPty {
    pub pty: u32,
    pub instance: String,
    pub secret: String,
}

impl SavedTerminal {
    /// A terminal as it is saved; `history` is its text, kept (cleaned) only when `history_lines` is above 0.
    pub fn of(
        cwd: Option<String>,
        launched: (Option<String>, Option<Vec<String>>),
        color: Option<String>,
        title: Option<String>,
        history: &str,
        history_lines: usize,
    ) -> Self {
        let history = (history_lines > 0).then(|| sanitize_history(history, history_lines)).flatten();
        Self { cwd, shell: launched.0, args: launched.1, color, title, history, token: None }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedTerminals {
    pub version: u32,
    pub active: usize,
    pub terminals: Vec<SavedTerminal>,
    /// The window's session on the terminal daemon (a random secret), while its terminals run there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// Plain text only: escape sequences and control characters (but newlines and tabs) are dropped, at most `lines` lines
/// from the end and `MAX_HISTORY_BYTES` bytes.
pub fn sanitize_history(text: &str, lines: usize) -> Option<String> {
    let lines = lines.min(MAX_HISTORY_LINES);
    if lines == 0 {
        return None;
    }
    let mut clean = String::with_capacity(text.len().min(MAX_HISTORY_BYTES));
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // ESC: skip the whole sequence (CSI up to its final byte, OSC up to BEL or ST, others one character).
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' | '\t' => clean.push(ch),
            c if c.is_control() => {}
            c => clean.push(c),
        }
    }
    let trimmed = clean.trim_end_matches(['\n', ' ']);
    let mut tail: Vec<&str> = trimmed.lines().rev().take(lines).collect();
    tail.reverse();
    let mut out = tail.join("\n");
    if out.len() > MAX_HISTORY_BYTES {
        let mut cut = out.len() - MAX_HISTORY_BYTES;
        while !out.is_char_boundary(cut) {
            cut += 1;
        }
        out = out[cut..].to_string();
    }
    (!out.trim().is_empty()).then_some(out)
}

impl SavedTerminals {
    pub fn to_json(&self) -> Option<String> {
        serde_json::to_string(&Self { version: VERSION, ..self.clone() }).ok()
    }

    /// A saved value, made safe to restore: an unknown version or broken JSON gives nothing; too many terminals, long
    /// titles and long histories are cut; history is cleaned again (the file may have been edited by hand).
    pub fn parse(text: &str, history_lines: usize) -> Option<Self> {
        let mut saved: Self = serde_json::from_str(text).ok()?;
        if saved.version != VERSION {
            return None;
        }
        saved.terminals.truncate(MAX_TERMINALS);
        for t in &mut saved.terminals {
            if let Some(title) = &mut t.title {
                if title.len() > MAX_TITLE {
                    let mut end = MAX_TITLE;
                    while !title.is_char_boundary(end) {
                        end -= 1;
                    }
                    title.truncate(end);
                }
                *title = title.chars().filter(|c| !c.is_control()).collect();
            }
            t.history = t.history.as_deref().and_then(|h| sanitize_history(h, history_lines));
        }
        saved.active = saved.active.min(saved.terminals.len().saturating_sub(1));
        // A token is only ever compared by the daemon, but a hand-edited file must not make it hold anything large.
        let sane = |s: &str| s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        if saved.session.as_deref().is_some_and(|s| !sane(s)) {
            saved.session = None;
        }
        for t in &mut saved.terminals {
            if t.token.as_ref().is_some_and(|k| !sane(&k.instance) || !sane(&k.secret)) || saved.session.is_none() {
                t.token = None;
            }
        }
        Some(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_plain_text_only_and_the_last_lines() {
        let raw = "one\n\u{1b}[31mred\u{1b}[0m two\n\u{1b}]0;title\u{7}three\u{1b}]8;;http://x\u{1b}\\link\u{1b}]8;;\u{1b}\\\nfour\u{7}\r\n";
        assert_eq!(sanitize_history(raw, 10).as_deref(), Some("one\nred two\nthreelink\nfour"));
        assert_eq!(sanitize_history(raw, 2).as_deref(), Some("threelink\nfour"));
        assert_eq!(sanitize_history(raw, 0), None, "off");
        assert_eq!(sanitize_history("\u{1b}[2J\u{1b}[H", 10), None, "nothing visible");
        let big = "x".repeat(MAX_HISTORY_BYTES * 2);
        assert!(sanitize_history(&big, 10).unwrap().len() <= MAX_HISTORY_BYTES);
    }

    #[test]
    fn a_damaged_or_foreign_value_restores_nothing_or_what_is_safe() {
        assert_eq!(SavedTerminals::parse("{", 100), None);
        assert_eq!(SavedTerminals::parse(r#"{"version":9,"active":0,"terminals":[]}"#, 100), None);
        let many: Vec<SavedTerminal> = (0..20).map(|i| SavedTerminal { title: Some(format!("t{i}\u{7}")), ..Default::default() }).collect();
        let text = SavedTerminals { version: 1, active: 15, terminals: many, session: None }.to_json().unwrap();
        let saved = SavedTerminals::parse(&text, 100).unwrap();
        assert_eq!(saved.terminals.len(), MAX_TERMINALS);
        assert_eq!(saved.active, MAX_TERMINALS - 1);
        assert_eq!(saved.terminals[3].title.as_deref(), Some("t3"), "control characters leave titles");
    }

    #[test]
    fn history_is_dropped_when_the_setting_is_off() {
        let t = SavedTerminal { history: Some("secret output".into()), ..Default::default() };
        let text = SavedTerminals { version: 1, active: 0, terminals: vec![t], session: None }.to_json().unwrap();
        assert_eq!(SavedTerminals::parse(&text, 0).unwrap().terminals[0].history, None);
    }
}
