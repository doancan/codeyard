//! Files on the clipboard of a Linux desktop. gpui has no access to the clipboard's file formats, so the tools of the session
//! are asked for them: `wl-paste` (Wayland) and `xclip` (X11). Without one of them installed there are simply no files. Every
//! call has a time limit, so a clipboard owner that does not answer can never hold the window up.
//!
//! A file manager offers a copy as `x-special/gnome-copied-files` (GNOME, Cinnamon, MATE, Nemo: "copy" or "cut", then one URI
//! per line) and as `text/uri-list` (KDE, Thunar, PCManFM and others: one URI per line, `#` for comments).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long one tool may take.
const LIMIT: Duration = Duration::from_millis(400);
/// More than this is not a list of files.
const MAX_BYTES: u64 = 1 << 20;
/// In the order they are tried.
const KINDS: [&str; 2] = ["x-special/gnome-copied-files", "text/uri-list"];

/// The absolute paths in `text`, offered as `kind`; anything that is not a `file:` URI is left out. A "cut" is read as a
/// copy: the files of another program are never moved away.
pub fn parse(kind: &str, text: &str) -> Vec<PathBuf> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).peekable();
    if kind == KINDS[0] && lines.peek().is_some_and(|l| matches!(*l, "copy" | "cut")) {
        lines.next();
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for line in lines {
        // `//server/share` is what a URI with a host other than this computer comes out as.
        if let Some(path) = schema::file_uri::to_path(line).filter(|p| p.starts_with('/') && !p.starts_with("//")) {
            let path = PathBuf::from(path);
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// Run `argv` and return its stdout, or `None` when it cannot start, fails, answers too much or is not done within `limit`.
pub fn fetch(argv: &[&str], limit: Duration) -> Option<String> {
    let (program, args) = argv.split_first()?;
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        out.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok().map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > limit => {
                // The reader ends by itself when the pipe closes.
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => return None,
        }
    };
    let bytes = reader.join().ok()??;
    (status.success() && bytes.len() as u64 <= MAX_BYTES).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

#[derive(Clone, Copy)]
enum Tool {
    WlPaste,
    Xclip,
}

impl Tool {
    fn argv(self, kind: &str) -> Vec<&str> {
        match self {
            Tool::WlPaste => vec!["wl-paste", "--no-newline", "--type", kind],
            Tool::Xclip => vec!["xclip", "-selection", "clipboard", "-t", kind, "-o"],
        }
    }
}

/// The tools of the running session, the one of its own display server first.
fn tools() -> Vec<Tool> {
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    let mut tools = Vec::new();
    if set("WAYLAND_DISPLAY") {
        tools.push(Tool::WlPaste);
    }
    if set("DISPLAY") {
        tools.push(Tool::Xclip);
    }
    tools
}

/// A number for what the clipboard holds: the same files give the same number, other ones another, none gives 0. There is no
/// change counter to ask, so a copy of the very same files again looks like no change.
fn stamp_of(text: &str) -> isize {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    (hasher.finish() as isize) | 1
}

/// The files on the clipboard and a number that tells one list from another.
pub fn read() -> (isize, Vec<PathBuf>) {
    for tool in tools() {
        for kind in KINDS {
            let Some(text) = fetch(&tool.argv(kind), LIMIT) else { continue };
            let files = parse(kind, &text);
            if !files.is_empty() {
                return (stamp_of(&text), files);
            }
        }
    }
    (0, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_managers_copy_is_read_in_either_format() {
        let gnome = "copy\nfile:///home/me/Belgeler/rapor%20%C4%9F%C3%BC%C5%9F.txt\nfile://localhost/tmp/b.txt\n";
        assert_eq!(parse(KINDS[0], gnome), [PathBuf::from("/home/me/Belgeler/rapor ğüş.txt"), PathBuf::from("/tmp/b.txt")]);
        assert_eq!(parse(KINDS[0], "cut\r\nfile:///tmp/a\r\n"), [PathBuf::from("/tmp/a")], "a cut is only a copy here");
        let list = "# from the file manager\r\nfile:///tmp/a%23b\r\n\r\nfile:///tmp/a%23b\r\nfile:///tmp/dir/\r\n";
        assert_eq!(parse(KINDS[1], list), [PathBuf::from("/tmp/a#b"), PathBuf::from("/tmp/dir/")], "comments and duplicates left out");
        // The word "copy" is only a header in the GNOME format.
        assert_eq!(parse(KINDS[1], "copy\nfile:///tmp/a\n"), [PathBuf::from("/tmp/a")]);
    }

    #[test]
    fn what_is_not_a_local_file_is_left_out() {
        for text in ["", "copy\n", "https://example.com/a.txt\n", "file://server/share/a.txt\n", "not a uri\n", "file://bad%zz\n", "/tmp/plain/path\n"] {
            assert!(parse(KINDS[1], text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn a_tool_answers_within_its_time_or_not_at_all() {
        let limit = Duration::from_secs(5);
        assert_eq!(fetch(&["sh", "-c", "printf 'file:///tmp/a%%20b\\n'"], limit).as_deref(), Some("file:///tmp/a%20b\n"));
        assert_eq!(fetch(&["sh", "-c", "echo nope; exit 1"], limit), None, "a tool that fails has no files");
        assert_eq!(fetch(&["no-such-clipboard-tool-xyz"], limit), None, "a tool that is not installed");
        assert_eq!(fetch(&[], limit), None);
        let t = Instant::now();
        assert_eq!(fetch(&["sh", "-c", "sleep 30"], Duration::from_millis(150)), None);
        assert!(t.elapsed() < Duration::from_secs(5), "a silent clipboard owner is given up on: {:?}", t.elapsed());
        let big = fetch(&["sh", "-c", "head -c 2000000 /dev/zero | tr '\\0' 'a'"], limit);
        assert_eq!(big, None, "more than {MAX_BYTES} bytes is not a list of files");
    }

    #[test]
    fn the_stamp_tells_lists_apart() {
        assert_eq!(stamp_of("file:///a\n"), stamp_of("file:///a\n"));
        assert_ne!(stamp_of("file:///a\n"), stamp_of("file:///b\n"));
        assert_ne!(stamp_of(""), 0, "0 is kept for no files");
    }
}
