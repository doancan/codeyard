use super::*;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("codeyard-fileops-{}-{}", std::process::id(), NEXT_COPY.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
    fn no_staging(&self) {
        assert!(fs::read_dir(&self.0).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".codeyard-copy-")));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn copy_preserves_binary_contents_unicode_names_and_empty_directories() {
    let f = Fixture::new();
    let source = f.path("source with ' quotes; $ and İstanbul 🚀");
    fs::create_dir(&source).unwrap();
    fs::create_dir(source.join("empty")).unwrap();
    fs::write(source.join("file.bin"), [0, 255, 128, 3]).unwrap();
    copy(&source, &f.path("result")).unwrap();
    assert_eq!(fs::read(f.path("result/file.bin")).unwrap(), [0, 255, 128, 3]);
    assert!(f.path("result/empty").is_dir());
    assert!(source.join("file.bin").exists());
    f.no_staging();
}

#[test]
fn copy_refuses_existing_file_or_directory_and_leaves_them_intact() {
    let f = Fixture::new();
    fs::write(f.path("source"), "new").unwrap();
    fs::write(f.path("target"), "original").unwrap();
    assert!(matches!(copy(&f.path("source"), &f.path("target")), Err(Error::AlreadyExists(_))));
    assert_eq!(fs::read_to_string(f.path("target")).unwrap(), "original");
    fs::create_dir(f.path("directory")).unwrap();
    assert!(matches!(copy(&f.path("source"), &f.path("directory")), Err(Error::AlreadyExists(_))));
    f.no_staging();
}

#[test]
fn exclusive_publication_has_one_winner_and_never_overwrites_it() {
    let f = Fixture::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
    let handles: Vec<_> = (0..12)
        .map(|i| {
            let source = f.path(&format!("source-{i}"));
            let target = f.path("target");
            fs::write(&source, i.to_string()).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                (i, rename_exclusive(&source, &target))
            })
        })
        .collect();
    let mut winner = None;
    for handle in handles {
        let (i, result) = handle.join().unwrap();
        match result {
            Ok(()) => {
                assert!(winner.replace(i).is_none());
            }
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::AlreadyExists),
        }
    }
    assert_eq!(fs::read_to_string(f.path("target")).unwrap(), winner.unwrap().to_string());
}

#[test]
fn copy_does_not_load_large_files_into_the_protocol() {
    use std::io::{Seek, SeekFrom};
    let f = Fixture::new();
    let mut source = fs::File::create(f.path("large")).unwrap();
    source.set_len(proto::MAX_FILE + 4096).unwrap();
    source.seek(SeekFrom::End(-4)).unwrap();
    source.write_all(b"tail").unwrap();
    drop(source);
    copy(&f.path("large"), &f.path("copy")).unwrap();
    assert_eq!(fs::metadata(f.path("copy")).unwrap().len(), proto::MAX_FILE + 4096);
    let mut copied = fs::File::open(f.path("copy")).unwrap();
    copied.seek(SeekFrom::End(-4)).unwrap();
    let mut tail = String::new();
    std::io::Read::read_to_string(&mut copied, &mut tail).unwrap();
    assert_eq!(tail, "tail");
}

#[cfg(unix)]
#[test]
fn copy_preserves_dangling_and_directory_links_without_following_them() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::write(f.path("source/run"), "program").unwrap();
    fs::set_permissions(f.path("source/run"), fs::Permissions::from_mode(0o751)).unwrap();
    symlink("missing", f.path("source/dangling")).unwrap();
    symlink("..", f.path("source/loop")).unwrap();
    copy(&f.path("source"), &f.path("copy")).unwrap();
    assert_eq!(fs::read_link(f.path("copy/dangling")).unwrap(), Path::new("missing"));
    assert_eq!(fs::read_link(f.path("copy/loop")).unwrap(), Path::new(".."));
    assert_eq!(fs::metadata(f.path("copy/run")).unwrap().permissions().mode() & 0o777, 0o751);
    symlink("not-there", f.path("occupied")).unwrap();
    assert!(matches!(copy(&f.path("source/run"), &f.path("occupied")), Err(Error::AlreadyExists(_))));
    f.no_staging();
}

#[cfg(unix)]
#[test]
fn copy_rejects_special_files_and_cleans_partial_directories() {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    fs::write(f.path("source/file"), "data").unwrap();
    let fifo = std::ffi::CString::new(f.path("source/fifo").as_os_str().as_bytes()).unwrap();
    // SAFETY: CString is valid; the FIFO is confined to the disposable fixture.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(copy(&f.path("source"), &f.path("copy")).is_err());
    assert!(!f.path("copy").exists());
    assert_eq!(fs::read_to_string(f.path("source/file")).unwrap(), "data");
    f.no_staging();
}

#[cfg(unix)]
#[test]
fn copy_rejects_a_destination_below_a_symlink_into_the_source() {
    let f = Fixture::new();
    fs::create_dir(f.path("source")).unwrap();
    std::os::unix::fs::symlink(f.path("source"), f.path("alias")).unwrap();
    assert!(matches!(copy(&f.path("source"), &f.path("alias/copy")), Err(Error::PermissionDenied(_))));
    assert!(!f.path("source/copy").exists());
    f.no_staging();
}

#[test]
fn copy_limits_deep_directory_trees_without_leaving_partial_results() {
    let f = Fixture::new();
    let source = f.path("source");
    fs::create_dir(&source).unwrap();
    let mut path = source.clone();
    for _ in 0..MAX_DEPTH + 2 {
        path.push("d");
        fs::create_dir(&path).unwrap();
    }
    assert!(copy(&source, &f.path("copy")).is_err());
    assert!(!f.path("copy").exists());
    f.no_staging();
}

#[test]
fn cancelled_copy_acknowledges_after_removing_its_staged_entry() {
    let f = Fixture::new();
    fs::write(f.path("source"), "keep").unwrap();
    let cancel = AtomicBool::new(true);
    assert!(matches!(copy_cancellable(&f.path("source"), &f.path("target"), &cancel), Err(Error::Cancelled)));
    assert!(!f.path("target").exists());
    assert_eq!(fs::read_to_string(f.path("source")).unwrap(), "keep");
    f.no_staging();
}
