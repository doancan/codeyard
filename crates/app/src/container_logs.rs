//! Presentation-only log parsing; original timestamps and messages remain available for copying.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Info,
    Debug,
    Plain,
}
pub struct Line {
    pub number: usize,
    pub timestamp: Option<String>,
    pub message: String,
    pub level: Level,
    pub full: String,
}
pub struct View {
    pub query: String,
    pub field: Option<gpui::Entity<crate::input::TextField>>,
    pub subscription: Option<gpui::Subscription>,
    pub problems_only: bool,
    pub follow: bool,
    pub wrap: bool,
    pub scroll: gpui::ScrollHandle,
}
impl Default for View {
    fn default() -> Self {
        Self { query: String::new(), field: None, subscription: None, problems_only: false, follow: true, wrap: true, scroll: gpui::ScrollHandle::new() }
    }
}
pub fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            if c == '\u{1b}' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if ch == '\t' || ch == '\n' || !ch.is_control() {
            out.push(ch);
        }
    }
    out
}
fn level(text: &str) -> Level {
    match text.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_ascii_lowercase().as_str() {
        "error" | "err" | "fatal" | "critical" | "panic" | "50" | "60" => Level::Error,
        "warn" | "warning" | "40" => Level::Warning,
        "info" | "information" | "notice" | "30" => Level::Info,
        "debug" | "trace" | "10" | "20" => Level::Debug,
        _ => Level::Plain,
    }
}
pub fn parse(text: &str) -> Vec<Line> {
    clean(text)
        .lines()
        .enumerate()
        .map(|(i, raw)| {
            let (timestamp, message) = match raw.split_once(' ') {
                Some((time, message)) if timestamp(time) => (Some(time.to_string()), message.to_string()),
                _ => (None, raw.to_string()),
            };
            let severity = serde_json::from_str::<serde_json::Value>(&message)
                .ok()
                .and_then(|v| {
                    v.get("level")
                        .or_else(|| v.get("severity"))
                        .or_else(|| v.get("log.level"))
                        .or_else(|| v.pointer("/log/level"))
                        .map(|v| level(v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()).as_str()))
                })
                .unwrap_or_else(|| {
                    let first = message.split_whitespace().next().unwrap_or_default();
                    level(first.strip_prefix("level=").unwrap_or(first).split(':').next().unwrap_or_default())
                });
            Line { number: i + 1, timestamp, message, level: severity, full: raw.to_string() }
        })
        .collect()
}
fn timestamp(time: &str) -> bool {
    time.len() >= 20
        && time.ends_with('Z')
        && time.as_bytes()[..19].iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            10 => *b == b'T',
            13 | 16 => *b == b':',
            _ => b.is_ascii_digit(),
        })
}
pub fn matcher(query: &str) -> Option<regex::Regex> {
    if query.is_empty() {
        return None;
    }
    regex::RegexBuilder::new(&regex::escape(query)).case_insensitive(true).size_limit(1024 * 1024).build().ok()
}
pub fn visible<'a>(lines: &'a [Line], query: &str, problems: bool) -> Vec<&'a Line> {
    let regex = matcher(query);
    lines
        .iter()
        .filter(|line| {
            (!problems || matches!(line.level, Level::Error | Level::Warning)) && (query.is_empty() || regex.as_ref().is_some_and(|r| r.is_match(&line.full)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn container_logs_preserve_unicode_and_classify_structured_and_plain_messages() {
        let rows = parse("2026-10-27T12:34:56.123Z \u{1b}[31mERROR: disk dolu şğ\u{1b}[0m\n{\"level\":40,\"msg\":\"slow\"}\nNo error occurred\n[DEBUG] ready");
        assert_eq!(rows[0].timestamp.as_deref(), Some("2026-10-27T12:34:56.123Z"));
        assert!(rows[0].message.contains("şğ"));
        assert!(rows[0].level == Level::Error);
        assert!(rows[1].level == Level::Warning);
        assert!(rows[2].level == Level::Plain);
        assert!(rows[3].level == Level::Debug);
        assert_eq!(visible(&rows, "DOLU", true).len(), 1);
        assert_eq!(visible(&rows, "", true).len(), 2);
        assert_eq!(clean("\u{1b}]8;;https://example.com\u{1b}\\link\u{1b}]8;;\u{1b}\\"), "link");
    }
}
