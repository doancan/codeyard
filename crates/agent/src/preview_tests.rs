use super::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("codeyard-preview-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("root")).unwrap();
        fs::write(path.join("root/index.html"), "inside").unwrap();
        fs::write(path.join("outside.html"), "outside").unwrap();
        Self(path)
    }
    fn root(&self) -> PathBuf {
        self.0.join("root")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn preview_read_is_bounded_and_validates_agent_inputs() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.root().join("assets/css")).unwrap();
    fs::write(fixture.root().join("assets/css/style.css"), "body {}").unwrap();
    assert_eq!(read(&fixture.root(), "assets/css/style.css").unwrap(), Response::File { data: b"body {}".to_vec(), hash: proto::content_hash(b"body {}") });
    assert!(matches!(read(&fixture.root(), "assets"), Err(Error::IsDirectory(_))));
    assert!(matches!(read(&fixture.root(), "missing.html"), Err(Error::NotFound(_))));
    for path in [
        "",
        "/index.html",
        "../outside.html",
        "assets/../../outside.html",
        ".env",
        "assets//style.css",
        "a\\b",
        "index.html:stream",
        "index.html.",
        "index.html ",
        "\0",
    ] {
        assert!(matches!(read(&fixture.root(), path), Err(Error::PermissionDenied(_))), "{path:?}");
    }
    assert!(matches!(read(Path::new("relative"), "index.html"), Err(Error::PermissionDenied(_))));
    for path in ["CON.html", "nul", "COM1.txt", "LPT1.css", "COM¹.txt", "NUL .html"] {
        assert!(matches!(read(&fixture.root(), path), Err(Error::PermissionDenied(_))), "{path}");
    }
    let huge = fs::File::create(fixture.root().join("huge.html")).unwrap();
    huge.set_len(proto::MAX_FILE + 1).unwrap();
    assert!(matches!(read(&fixture.root(), "huge.html"), Err(Error::TooLarge { .. })));
}

#[cfg(unix)]
#[test]
fn preview_rejects_symlinks_at_root_parent_component_and_leaf() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    symlink(&fixture.0, fixture.root().join("linked")).unwrap();
    symlink(fixture.0.join("outside.html"), fixture.root().join("direct.html")).unwrap();
    symlink(fixture.root(), fixture.0.join("linked-root")).unwrap();
    for path in ["linked/outside.html", "direct.html"] {
        assert!(matches!(read(&fixture.root(), path), Err(Error::PermissionDenied(_))), "{path}");
    }
    assert!(matches!(read(&fixture.0.join("linked-root"), "index.html"), Err(Error::PermissionDenied(_))));
}

#[cfg(unix)]
#[test]
fn preview_reads_the_opened_file_after_a_symlink_swap() {
    let fixture = Fixture::new();
    let path = fixture.root().join("index.html");
    let opened = open(&fixture.root(), "index.html").unwrap();
    fs::rename(&path, fixture.root().join("original.html")).unwrap();
    std::os::unix::fs::symlink(fixture.0.join("outside.html"), &path).unwrap();
    let Response::File { data, .. } = read_opened(opened, &path).unwrap() else { panic!("expected the original file") };
    assert_eq!(data, b"inside", "the open handle must not follow a replacement symlink");
    assert!(matches!(read(&fixture.root(), "index.html"), Err(Error::PermissionDenied(_))));
}

#[cfg(unix)]
#[test]
fn preview_parent_directory_swaps_never_read_outside_contents() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root().join("assets")).unwrap();
    fs::write(fixture.root().join("assets/outside.html"), "inside").unwrap();
    let root = fixture.root();
    let swap_root = root.clone();
    let outside = fixture.0.clone();
    let swapper = std::thread::spawn(move || {
        for _ in 0..200 {
            fs::rename(swap_root.join("assets"), swap_root.join("parked")).unwrap();
            std::os::unix::fs::symlink(&outside, swap_root.join("assets")).unwrap();
            std::thread::yield_now();
            fs::remove_file(swap_root.join("assets")).unwrap();
            fs::rename(swap_root.join("parked"), swap_root.join("assets")).unwrap();
        }
    });
    for _ in 0..200 {
        if let Ok(Response::File { data, .. }) = read(&root, "assets/outside.html") {
            assert_eq!(data, b"inside", "a replaced parent must not escape the opened root");
        }
    }
    swapper.join().unwrap();
    assert!(matches!(read(&root, "assets/outside.html"), Ok(Response::File { .. })));
}

#[cfg(unix)]
#[test]
fn preview_rejects_fifo_without_waiting_for_a_writer() {
    let fixture = Fixture::new();
    let path = fixture.root().join("pipe.html");
    let status = std::process::Command::new("mkfifo").arg(&path).status().unwrap();
    assert!(status.success());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = read(&fixture.root(), "pipe.html");
        tx.send(result).unwrap();
    });
    assert!(matches!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), Err(Error::PermissionDenied(_))));
}

#[cfg(windows)]
#[test]
fn preview_rejects_directory_junctions() {
    let fixture = Fixture::new();
    let link = fixture.root().join("linked");
    let result = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&fixture.0).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(matches!(read(&fixture.root(), "linked/outside.html"), Err(Error::PermissionDenied(_))));
    fs::remove_dir(link).unwrap();
}
