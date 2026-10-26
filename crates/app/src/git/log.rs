//! File history and blame, and the relative-time text they show.

use host::Host;

use super::git_ok;

/// Turns English `relative_time` text such as "5 minutes ago" into the full phrase in the current language ("5 dakika önce");
/// unrecognized text is left as is. (`relative_time` itself is an English intermediate form: `short_ago` parses it.)
pub fn ago_phrase(text: &str) -> String {
    use crate::i18n::{t, tn};
    let text = text.trim();
    if text == "just now" {
        return t("just now").into();
    }
    let mut parts = text.split_whitespace();
    let (Some(n), Some(unit)) = (parts.next(), parts.next()) else { return text.to_string() };
    let Ok(n) = n.parse::<usize>() else { return text.to_string() };
    match unit.trim_end_matches(',').trim_end_matches('s') {
        "minute" => tn("{} minute ago", "{} minutes ago", n, &[]),
        "hour" => tn("{} hour ago", "{} hours ago", n, &[]),
        "day" => tn("{} day ago", "{} days ago", n, &[]),
        "week" => tn("{} week ago", "{} weeks ago", n, &[]),
        "month" => tn("{} month ago", "{} months ago", n, &[]),
        "year" => tn("{} year ago", "{} years ago", n, &[]),
        _ => text.to_string(),
    }
}

/// A commit in a file's history.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub hash: String,
    pub short: String,
    pub author: String,
    /// "3 days ago"
    pub when: String,
    pub subject: String,
    /// The path at that commit (may be the old name after a rename).
    pub path: String,
}

const LOG_FORMAT: &str = "--format=%x1e%H%x1f%h%x1f%an%x1f%ar%x1f%s";

/// Output of `git log --follow --name-only` (records separated by \x1e, fields by \x1f).
pub fn parse_log(out: &str) -> Vec<LogEntry> {
    out.split('\x1e')
        .filter_map(|rec| {
            let (head, names) = rec.split_once('\n').unwrap_or((rec, ""));
            let f: Vec<&str> = head.split('\x1f').collect();
            let [hash, short, author, when, subject] = f[..] else { return None };
            let path = names.lines().map(str::trim).find(|l| !l.is_empty())?.to_string();
            Some(LogEntry { hash: hash.into(), short: short.into(), author: author.into(), when: when.into(), subject: subject.into(), path })
        })
        .collect()
}

/// A file's history (follows renames), newest first.
pub fn file_log(host: &dyn Host, root: &str, rel: &str, limit: usize) -> Result<Vec<LogEntry>, String> {
    let n = format!("-n{limit}");
    git_ok(host, root, &["log", "--follow", &n, LOG_FORMAT, "--name-only", "--", rel]).map(|o| parse_log(&o))
}

/// The commit that last changed the line. `contents`: unsaved text (so lines do not shift).
/// For a line not yet committed, `hash` is all zeros.
pub fn blame_line(host: &dyn Host, root: &str, rel: &str, line: usize, contents: Option<String>) -> Result<LogEntry, String> {
    let range = format!("-L{},{}", line + 1, line + 1);
    let mut args = vec!["blame", "--no-textconv", "--porcelain", range.as_str()];
    if contents.is_some() {
        args.extend(["--contents", "-"]);
    }
    args.extend(["--", rel]);
    let out = host.exec("git", &args, Some(root), contents.map(String::into_bytes)).map_err(|e| e.to_string())?;
    if !out.ok() {
        return Err(out.stderr_str());
    }
    parse_blame(&out.stdout_str(), rel).ok_or_else(|| crate::i18n::t("git blame gave no answer").into())
}

pub fn parse_blame(out: &str, rel: &str) -> Option<LogEntry> {
    let mut lines = out.lines();
    let hash = lines.next()?.split_whitespace().next()?.to_string();
    let (mut author, mut time, mut subject) = (String::new(), 0i64, String::new());
    for l in lines {
        if let Some(v) = l.strip_prefix("author ") {
            author = v.to_string();
        } else if let Some(v) = l.strip_prefix("author-time ") {
            time = v.parse().unwrap_or(0);
        } else if let Some(v) = l.strip_prefix("summary ") {
            subject = v.to_string();
        }
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    Some(LogEntry { short: hash.chars().take(7).collect(), hash, author, when: relative_time(now - time), subject, path: rel.to_string() })
}

/// "4 days ago" → "4d", "25 minutes ago" → "25m", "just now" → "now"; unrecognized text is left as is.
/// Units: s, m, h, d, w, mo, y ("1 year, 2 months ago" takes only the first part: "1y").
pub fn short_ago(text: &str) -> String {
    use crate::i18n::tf;
    let t = text.trim();
    if t == "just now" || t == "now" {
        return crate::i18n::t("now").into();
    }
    let mut parts = t.split_whitespace();
    let (Some(n), Some(unit)) = (parts.next(), parts.next()) else { return t.to_string() };
    let Ok(n) = n.parse::<u64>() else { return t.to_string() };
    match unit.trim_end_matches(',').trim_end_matches('s') {
        "second" => tf("{}s", &[&n]),
        "minute" => tf("{}m", &[&n]),
        "hour" => tf("{}h", &[&n]),
        "day" => tf("{}d", &[&n]),
        "week" => tf("{}w", &[&n]),
        "month" => tf("{}mo", &[&n]),
        "year" => tf("{}y", &[&n]),
        _ => t.to_string(),
    }
}

/// "just now", "5 minutes ago", "3 days ago", "2 years ago".
pub fn relative_time(secs: i64) -> String {
    let units = [(31_536_000, "year"), (2_592_000, "month"), (604_800, "week"), (86_400, "day"), (3_600, "hour"), (60, "minute")];
    match units.iter().find(|(u, _)| secs >= *u) {
        Some((u, name)) => {
            let n = secs / u;
            format!("{n} {name}{} ago", if n == 1 { "" } else { "s" })
        }
        None => "just now".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{repo, show};

    #[test]
    fn parses_log_and_blame() {
        let out =
            "\x1eaaaa111\x1faaaa1\x1fAyşe\x1f2 days ago\x1fyeni ad\n\nsrc/yeni.rs\n\x1ebbbb222\x1fbbbb2\x1fAli\x1f3 weeks ago\x1filk | boru\n\nsrc/eski.rs\n";
        let log = parse_log(out);
        assert_eq!(log.len(), 2);
        assert_eq!((log[0].author.as_str(), log[0].path.as_str()), ("Ayşe", "src/yeni.rs"));
        assert_eq!(log[1].path, "src/eski.rs", "path before the rename");
        assert!(parse_log("").is_empty());
        let blame =
            "0123456789abcdef0123456789abcdef01234567 5 5 1\nauthor Ali Veli\nauthor-mail <a@b>\nauthor-time 0\nsummary düzeltme\nfilename x.rs\n\tsatır\n";
        let b = parse_blame(blame, "x.rs").unwrap();
        assert_eq!((b.short.as_str(), b.author.as_str(), b.subject.as_str()), ("0123456", "Ali Veli", "düzeltme"));
        assert_eq!(relative_time(30), "just now");
        assert_eq!(relative_time(3600), "1 hour ago");
        for (long, short) in [
            ("4 days ago", "4d"),
            ("25 minutes ago", "25m"),
            ("1 minute ago", "1m"),
            ("2 hours ago", "2h"),
            ("3 weeks ago", "3w"),
            ("5 months ago", "5mo"),
            ("2 years ago", "2y"),
            ("1 year, 2 months ago", "1y"),
            ("12 seconds ago", "12s"),
            ("just now", "now"),
            ("", ""),
            ("yesterday", "yesterday"),
            ("3 fortnights ago", "3 fortnights ago"),
        ] {
            assert_eq!(short_ago(long), short, "{long:?}");
        }
        assert_eq!(short_ago(&relative_time(86_400 * 4)), "4d");
        assert_eq!(relative_time(86_400 * 3), "3 days ago");
    }

    /// `diff.<driver>.textconv` from the repository's config runs a program and replaces file content in `show` and `blame`;
    /// the editor needs the raw bytes (hunk staging writes them back to the index), so it is never applied.
    #[cfg(unix)]
    #[test]
    fn show_and_blame_ignore_textconv_drivers() {
        let Some((dir, run)) = repo("textconv") else { return };
        let marker = dir.join("..").join(format!("codeyard-textconv-marker-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let conv = dir.join("conv.sh");
        std::fs::write(&conv, format!("#!/bin/sh\ntouch '{}'\necho CONVERTED\n", marker.display())).unwrap();
        crate::test_files::make_executable(&conv);
        std::fs::write(dir.join(".gitattributes"), "*.txt diff=evil\n").unwrap();
        if run(&["config", "diff.evil.textconv", &conv.to_string_lossy()]).is_none() {
            return;
        }
        let host = host::local();
        let d = dir.to_string_lossy().into_owned();
        assert_eq!(show(&host, &d, "HEAD", "f.txt").as_deref(), Some("base\n"), "raw content, not the converter's output");
        let entry = blame_line(&host, &d, "f.txt", 0, None).unwrap();
        assert!(!entry.hash.is_empty());
        assert!(!marker.exists(), "a textconv program from the repository was executed");
        let _ = std::fs::remove_file(&marker);
    }
}
