//! Shell-integration sequences in terminal output: OSC 7 (the shell's folder), OSC 133 (prompt, command start and
//! end with its exit code) and the notifications programs send (OSC 9 and OSC 777;notify). The scanner only watches the bytes, which all still go to the terminal parser; it reports
//! where in the stream each sequence ended, so the caller can note the cursor's line at that moment. Sequences may be
//! split across chunks. Payloads are capped, and anything else (other OSC numbers, broken sequences) is ignored.

/// Longest OSC payload looked at.
const MAX_PAYLOAD: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OscEvent {
    /// OSC 7: the shell's folder (decoded; only for this host or no host).
    Cwd(String),
    /// OSC 133;A: a prompt starts.
    PromptStart,
    /// OSC 133;C: a command starts running.
    CommandStart,
    /// OSC 133;D[;code]: it finished.
    CommandEnd(Option<i32>),
    /// OSC 9;text (iTerm2, Windows Terminal) or OSC 777;notify;title;body (urxvt, kitty): a program wants the user,
    /// like Claude Code or Codex waiting for an answer. Control characters are removed.
    Notify { title: Option<String>, body: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Esc,
    /// Inside `ESC ]`.
    Osc,
    /// `ESC` inside an OSC (maybe the start of `ESC \`).
    OscEsc,
}

#[derive(Debug, Default)]
pub struct Scanner {
    state: State,
    payload: Vec<u8>,
    overflow: bool,
}

impl Scanner {
    /// Events in `data`, each with the index just past the sequence that produced it.
    pub fn scan(&mut self, data: &[u8]) -> Vec<(usize, OscEvent)> {
        let mut out = Vec::new();
        // Fast path: nothing in progress and no ESC or BEL that could start or end one.
        if self.state == State::Ground && memchr(0x1b, data).is_none() {
            return out;
        }
        for (i, &b) in data.iter().enumerate() {
            self.state = match (self.state, b) {
                (State::Ground, 0x1b) => State::Esc,
                (State::Ground, _) => State::Ground,
                (State::Esc, b']') => {
                    self.payload.clear();
                    self.overflow = false;
                    State::Osc
                }
                (State::Esc, 0x1b) => State::Esc,
                (State::Esc, _) => State::Ground,
                (State::Osc, 0x07) => {
                    out.extend(self.finish().map(|e| (i + 1, e)));
                    State::Ground
                }
                (State::Osc, 0x1b) => State::OscEsc,
                (State::Osc, b) => {
                    self.push(b);
                    State::Osc
                }
                (State::OscEsc, b'\\') => {
                    out.extend(self.finish().map(|e| (i + 1, e)));
                    State::Ground
                }
                // A new ESC ] inside a broken OSC: start over.
                (State::OscEsc, b']') => {
                    self.payload.clear();
                    self.overflow = false;
                    State::Osc
                }
                (State::OscEsc, _) => State::Ground,
            };
        }
        out
    }

    fn push(&mut self, b: u8) {
        if self.payload.len() < MAX_PAYLOAD {
            self.payload.push(b);
        } else {
            self.overflow = true;
        }
    }

    fn finish(&mut self) -> Option<OscEvent> {
        if self.overflow {
            return None;
        }
        let text = std::str::from_utf8(&self.payload).ok()?;
        let (code, rest) = text.split_once(';').unwrap_or((text, ""));
        match code {
            "7" => parse_cwd(rest).map(OscEvent::Cwd),
            "133" => match rest.split(';').next() {
                Some("A") => Some(OscEvent::PromptStart),
                Some("C") => Some(OscEvent::CommandStart),
                Some("D") => Some(OscEvent::CommandEnd(rest.split(';').nth(1).and_then(|c| c.trim().parse().ok()))),
                _ => None,
            },
            "9" => parse_osc9(rest),
            "777" => {
                let mut parts = rest.splitn(3, ';');
                if parts.next() != Some("notify") {
                    return None;
                }
                let title = clean(parts.next().unwrap_or(""));
                let body = clean(parts.next().unwrap_or(""));
                (!title.is_empty() || !body.is_empty()).then(|| OscEvent::Notify { title: (!title.is_empty()).then_some(title), body })
            }
            _ => None,
        }
    }
}

/// Longest notification text kept.
const MAX_NOTIFY: usize = 300;

/// OSC 9 is also ConEmu's command family (`9;4;state;percent` is the progress bar Windows Terminal shows): a number
/// followed by `;` or nothing is one of those, not a message.
fn parse_osc9(rest: &str) -> Option<OscEvent> {
    let (head, _) = rest.split_once(';').unwrap_or((rest, ""));
    if !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let body = clean(rest);
    (!body.is_empty()).then_some(OscEvent::Notify { title: None, body })
}

/// Control characters become spaces, runs of white space one space, and the text is capped.
fn clean(text: &str) -> String {
    let words: Vec<&str> = text.split(|c: char| c.is_control() || c.is_whitespace()).filter(|w| !w.is_empty()).collect();
    words.join(" ").chars().take(MAX_NOTIFY).collect()
}

fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|b| *b == needle)
}

/// `file://host/path` (path percent-encoded or not) → an absolute path; `None` for other schemes, relative paths or
/// control characters. The host is not checked here (any host name the shell knows itself by).
pub fn parse_cwd(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = path.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8(out).ok()?;
    (path.starts_with('/') && !path.chars().any(char::is_control) && path.len() <= MAX_PAYLOAD).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequences_are_found_with_either_terminator() {
        let mut s = Scanner::default();
        let data = b"$ \x1b]133;A\x07prompt\x1b]133;C\x1b\\out\x1b]133;D;1\x07\x1b]7;file://box/home/a%20b\x07";
        let ev = s.scan(data);
        assert_eq!(
            ev.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>(),
            vec![OscEvent::PromptStart, OscEvent::CommandStart, OscEvent::CommandEnd(Some(1)), OscEvent::Cwd("/home/a b".into())]
        );
        assert_eq!(&data[..ev[0].0], b"$ \x1b]133;A\x07", "the index is just past the sequence");
    }

    #[test]
    fn a_sequence_split_anywhere_is_still_found() {
        let data = b"ab\x1b]133;D;0\x07cd\x1b]7;file:///tmp\x1b\\";
        for cut in 0..data.len() {
            let mut s = Scanner::default();
            let mut ev = s.scan(&data[..cut]);
            ev.extend(s.scan(&data[cut..]).into_iter().map(|(i, e)| (i + cut, e)));
            assert_eq!(ev, vec![(12, OscEvent::CommandEnd(Some(0))), (data.len(), OscEvent::Cwd("/tmp".into()))], "cut at {cut}");
        }
    }

    #[test]
    fn other_and_broken_sequences_are_ignored() {
        let mut s = Scanner::default();
        let huge = format!("\x1b]7;file:///{}\x07", "x".repeat(10_000));
        assert!(s.scan(huge.as_bytes()).is_empty(), "oversized payloads");
        assert!(s.scan(b"\x1b]0;title\x07\x1b]52;c;aGk=\x07\x1b[31m\x1b]133;Z\x07").is_empty());
        assert!(s.scan(b"\x1b]133;D;notanumber\x07").first().is_some_and(|(_, e)| *e == OscEvent::CommandEnd(None)));
        assert_eq!(s.scan(b"\x1b]7;file://h/ok\x1bX\x1b]133;A\x07"), vec![(25, OscEvent::PromptStart)], "an aborted OSC does not hide the next one");
        assert_eq!(parse_cwd("http://x/y"), None);
        assert_eq!(parse_cwd("file://host"), None);
        assert_eq!(parse_cwd("file:///a%0Ab"), None, "no control characters");
        assert_eq!(parse_cwd("file:///a%zzb").as_deref(), Some("/a%zzb"), "a bad escape stays as it is");
    }

    #[test]
    fn programs_ask_for_the_user_with_osc_9_and_777() {
        let mut s = Scanner::default();
        let notify = |title: Option<&str>, body: &str| OscEvent::Notify { title: title.map(Into::into), body: body.into() };
        let ev: Vec<OscEvent> = s
            .scan(b"\x1b]9;\n\nClaude needs your permission\x07\x1b]777;notify;Codex;Approve\tthe edit?\x1b\\\x1b]9;\x07")
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        assert_eq!(ev, [notify(None, "Claude needs your permission"), notify(Some("Codex"), "Approve the edit?")], "an empty OSC 9 says nothing");
        assert!(s.scan(b"\x1b]9;4;1;50\x07\x1b]9;9;C:\\\x07\x1b]777;preexec\x07").is_empty(), "progress, ConEmu commands");
    }
}
