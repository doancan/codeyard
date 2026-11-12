//! Cancelling and interrupting uploads: what is left behind, and what the remote side's words are worth. They drive a local POSIX
//! shell as the "server", so they are Unix only.

#![cfg(unix)]

use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-transfer-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A finalization that is cancelled still tells what the remote side said before it stopped.
#[cfg(unix)]
#[test]
fn an_interrupted_finalization_keeps_what_the_remote_side_said() {
    let child = spawn(local_shell("cat > /dev/null; echo 'No space left on device' >&2; exit 1"), Stdio::piped(), Stdio::null()).unwrap();
    let error = finish_folder_commit(child, &AtomicBool::new(true)).unwrap_err().to_string();
    assert!(error.contains("finalization was interrupted"), "{error}");
    assert!(error.contains("No space left on device"), "{error}");
}

/// A cancelled folder upload must not remove the staging folder while the remote side may still be writing into it: the
/// remote shell gets to finish (it cleans up after itself when its input ends) before anything is deleted or killed. The
/// fake server keeps a writer running that would recreate the staging folder after a hasty `rm -rf`; load only made the old
/// ordering fail now and then, this makes it fail every time.
#[cfg(unix)]
#[test]
fn cancelling_a_folder_upload_waits_for_the_remote_side_before_cleaning_up() {
    let root = dir("folder-cancel-writer");
    let source = root.join("project");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("payload.bin"), vec![1u8; CHUNK * 8]).unwrap();
    let destination = root.join("server");
    std::fs::create_dir(&destination).unwrap();
    let fake = format!(
        r#"t=$(mktemp -d {dest}/.codeyard-upload-XXXXXXXX); mkdir "$t/tree"; printf '%s\0' "$t"
( n=0; while [ "$n" -lt 150 ]; do mkdir -p "$t/tree/d$n"; n=$((n + 1)); sleep 0.01; done ) &
writer=$!
trap 'kill "$writer" 2>/dev/null; rm -rf -- "$t"' EXIT
cat > /dev/null"#,
        dest = crate::ssh::sh_quote(&destination.to_string_lossy())
    );
    let server = |script: &str| if script.starts_with("set -e; if [ -e ") { local_shell(&fake) } else { local_shell(script) };
    let cancel = AtomicBool::new(false);
    let result = upload(&server, &source, &destination.to_string_lossy(), &cancel, |_| cancel.store(true, Ordering::Relaxed));
    assert!(matches!(result, Err(HostError::Op(proto::Error::Cancelled))), "{result:?}");
    // The writer of the fake server runs for at most 1.5 s: give a leftover time to show up.
    std::thread::sleep(std::time::Duration::from_millis(1700));
    let left: Vec<_> = std::fs::read_dir(&destination).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(left.is_empty(), "the cancelled upload left {left:?}");
}
