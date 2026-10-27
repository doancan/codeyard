//! Test helper for scripts a test writes and then runs.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

/// Makes the script at `path` executable in a way that running it cannot fail with "Text file busy" (ETXTBSY).
///
/// Tests run on many threads. While a test holds a file open for writing, another thread may fork to start a program, and
/// the child keeps that descriptor until it execs; running the script in that window fails. So the content is written
/// again by a short-lived `sh` into a new file, made executable there and moved over the path. This process never had
/// the new file open, so no fork of it can hold it.
pub fn make_executable(path: &Path) {
    let content = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut child = Command::new("/bin/sh")
        .args(["-c", r#"cat > "$1.x" && chmod 755 "$1.x" && mv -f "$1.x" "$1""#, "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("sh");
    child.stdin.take().expect("stdin").write_all(&content).expect("write the script");
    assert!(child.wait().expect("sh").success(), "could not make {} executable", path.display());
}
