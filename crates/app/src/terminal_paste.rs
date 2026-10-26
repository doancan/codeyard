//! What a paste into the terminal sends. Pasted text is untrusted: a web page can put text on the clipboard that looks
//! like one command and carries another. So:
//! - the bracketed-paste markers (`ESC [200~`, `ESC [201~`, also as 8-bit CSI) are always removed from the text, or a
//!   paste could end the bracketed mode early and have the shell run what follows;
//! - other control characters (escape sequences, backspaces, C1 controls) are removed when the shell does not use
//!   bracketed paste, or always if the user asks; tabs and line breaks stay;
//! - line breaks become carriage returns, as typed keys would be;
//! - a very large paste is cut.

/// `terminal.sanitizePaste`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sanitize {
    /// Remove control characters unless the shell takes the paste as bracketed (it then reads it as text).
    #[default]
    Auto,
    Always,
    /// Only the bracketed-paste markers are removed.
    Off,
}

impl Sanitize {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Sanitize::Auto),
            "always" => Some(Sanitize::Always),
            "off" => Some(Sanitize::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Sanitize::Auto => "auto",
            Sanitize::Always => "always",
            Sanitize::Off => "off",
        }
    }
}

/// `terminal.warnMultilinePaste`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WarnMultiline {
    /// Ask when the shell would run each line as it arrives (no bracketed paste).
    #[default]
    Unbracketed,
    Always,
    Never,
}

impl WarnMultiline {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "unbracketed" => Some(WarnMultiline::Unbracketed),
            "always" => Some(WarnMultiline::Always),
            "never" => Some(WarnMultiline::Never),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WarnMultiline::Unbracketed => "unbracketed",
            WarnMultiline::Always => "always",
            WarnMultiline::Never => "never",
        }
    }
}

/// Largest paste sent (bytes).
pub const MAX_PASTE: usize = 8 * 1024 * 1024;

/// A paste ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub data: Vec<u8>,
    /// Lines with content.
    pub lines: usize,
    /// Characters removed.
    pub stripped: usize,
    pub truncated: bool,
}

const MARKERS: [&str; 4] = ["\x1b[200~", "\x1b[201~", "\u{9b}200~", "\u{9b}201~"];

pub fn prepare(text: &str, bracketed: bool, policy: Sanitize) -> Prepared {
    let mut stripped = 0;
    let mut text = text.to_string();
    // Removing one marker can join the halves of another (`ESC[20` + marker + `1~`): repeat until none is left.
    loop {
        let before = text.len();
        for m in MARKERS {
            stripped += text.matches(m).count() * m.chars().count();
            text = text.replace(m, "");
        }
        if text.len() == before {
            break;
        }
    }
    let strip_controls = match policy {
        Sanitize::Always => true,
        Sanitize::Auto => !bracketed,
        Sanitize::Off => false,
    };
    let mut out = String::with_capacity(text.len());
    let mut truncated = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let c = match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                '\r'
            }
            '\n' => '\r',
            c => c,
        };
        let control = c.is_control() && !matches!(c, '\t' | '\r');
        if control && strip_controls {
            stripped += 1;
            continue;
        }
        if out.len() + c.len_utf8() > MAX_PASTE {
            truncated = true;
            break;
        }
        out.push(c);
    }
    let lines = out.split('\r').filter(|l| !l.is_empty()).count();
    let data = if bracketed { format!("\x1b[200~{out}\x1b[201~") } else { out };
    Prepared { data: data.into_bytes(), lines, stripped, truncated }
}

/// Should the user confirm before this paste is sent?
pub fn needs_confirm(p: &Prepared, bracketed: bool, warn: WarnMultiline) -> bool {
    p.lines > 1
        && match warn {
            WarnMultiline::Always => true,
            WarnMultiline::Unbracketed => !bracketed,
            WarnMultiline::Never => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(text: &str, bracketed: bool, policy: Sanitize) -> String {
        String::from_utf8(prepare(text, bracketed, policy).data).unwrap()
    }

    #[test]
    fn markers_inside_the_text_never_reach_the_shell() {
        let evil = "echo safe\x1b[201~rm -rf ~\x1b[200~";
        assert_eq!(sent(evil, true, Sanitize::Off), "\x1b[200~echo saferm -rf ~\x1b[201~", "only our own markers wrap it");
        assert_eq!(sent("a\u{9b}201~b", true, Sanitize::Off), "\x1b[200~ab\x1b[201~", "the 8-bit form too");
        assert_eq!(sent("x\x1b[20\x1b[201~1~y", true, Sanitize::Off), "\x1b[200~xy\x1b[201~", "a marker split by another one");
    }

    #[test]
    fn control_characters_follow_the_policy_and_line_breaks_become_returns() {
        let text = "ls\x1b[31m\x08\x7f\u{85}\tdone\r\nnext\n";
        assert_eq!(sent(text, false, Sanitize::Auto), "ls[31m\tdone\rnext\r", "without bracketed paste they are removed");
        assert_eq!(
            sent(text, true, Sanitize::Auto),
            "\x1b[200~ls\x1b[31m\x08\x7f\u{85}\tdone\rnext\r\x1b[201~",
            "inside a bracketed paste the shell reads them as text"
        );
        assert_eq!(sent(text, true, Sanitize::Always), "\x1b[200~ls[31m\tdone\rnext\r\x1b[201~");
        let p = prepare(text, false, Sanitize::Auto);
        assert_eq!((p.lines, p.stripped), (2, 4));
    }

    #[test]
    fn multi_line_pastes_ask_first_by_the_setting() {
        let one = prepare("make test\n", false, Sanitize::Auto);
        let two = prepare("cd /\nrm -rf x\n", false, Sanitize::Auto);
        assert!(!needs_confirm(&one, false, WarnMultiline::Unbracketed), "a single command does not ask");
        assert!(needs_confirm(&two, false, WarnMultiline::Unbracketed));
        assert!(!needs_confirm(&two, true, WarnMultiline::Unbracketed), "a bracketed paste waits for Enter anyway");
        assert!(needs_confirm(&two, true, WarnMultiline::Always));
        assert!(!needs_confirm(&two, false, WarnMultiline::Never));
    }

    #[test]
    fn huge_pastes_are_cut_on_a_character_boundary() {
        let big = "ğ".repeat(MAX_PASTE);
        let p = prepare(&big, false, Sanitize::Auto);
        assert!(p.truncated && p.data.len() <= MAX_PASTE && String::from_utf8(p.data).is_ok());
    }
}
