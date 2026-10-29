//! Exclusive publication of complete new files, with no destination placeholder.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use proto::{Error, content_hash};

pub(super) fn write(path: &Path, data: &[u8]) -> Result<u64, Error> {
    let dir = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|error| super::map_err(dir, error))?;
    let short: String = path.file_name().map(|name| name.to_string_lossy().chars().take(60).collect()).unwrap_or_default();
    let mut attempts = 0;
    let (stage, mut file) = loop {
        let stage = super::temp_path(dir, &short);
        match super::create_private(&stage, true) {
            Ok(file) => break (stage, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempts < 8 => attempts += 1,
            Err(error) => return Err(super::map_err(path, error)),
        }
    };
    let written = file.write_all(data).and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| super::fileops::rename_exclusive(&stage, path));
    if let Err(error) = result {
        // Only a stage successfully created by this operation is eligible for cleanup.
        let _ = fs::remove_file(&stage);
        return Err(super::map_err(path, error));
    }
    if let Ok(parent) = fs::File::open(dir) {
        let _ = parent.sync_all();
    }
    Ok(content_hash(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    fn scratch(name: &str) -> std::path::PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("codeyard-write-new-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_new_publishes_complete_contents_and_refuses_existing_files_and_directories() {
        let dir = scratch("publication");
        let target = dir.join("nested/file.txt");
        assert_eq!(write(&target, b"complete contents"), Ok(content_hash(b"complete contents")));
        assert!(matches!(write(&target, b"replacement"), Err(Error::AlreadyExists(_))));
        assert_eq!(fs::read(&target).unwrap(), b"complete contents");
        let occupied = dir.join("directory");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("keep.txt"), b"keep").unwrap();
        assert!(write(&occupied, b"replacement").is_err());
        assert_eq!(fs::read(occupied.join("keep.txt")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1, "no staging file remains after refusal");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
    }

    #[test]
    fn simultaneous_write_new_requests_have_exactly_one_winner() {
        let dir = scratch("race");
        let target = dir.join("winner.txt");
        let barrier = Arc::new(Barrier::new(24));
        let jobs: Vec<_> = (0..24)
            .map(|index| {
                let barrier = barrier.clone();
                let target = target.clone();
                std::thread::spawn(move || {
                    let data = format!("complete document {index}").into_bytes();
                    barrier.wait();
                    (write(&target, &data), data)
                })
            })
            .collect();
        let results: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
        let winners: Vec<_> = results.iter().filter(|(result, _)| result.is_ok()).collect();
        assert_eq!(winners.len(), 1);
        assert_eq!(fs::read(&target).unwrap(), winners[0].1);
        assert_eq!(results.iter().filter(|(result, _)| matches!(result, Err(Error::AlreadyExists(_)))).count(), 23);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "losers clean only their own staging files");
    }

    #[cfg(unix)]
    #[test]
    fn write_new_refuses_existing_and_dangling_symlinks_without_following_them() {
        let dir = scratch("symlinks");
        let original = dir.join("original.txt");
        fs::write(&original, b"keep").unwrap();
        for (name, target) in [("existing", original.clone()), ("dangling", dir.join("missing.txt"))] {
            let link = dir.join(name);
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(matches!(write(&link, b"replacement"), Err(Error::AlreadyExists(_))));
            assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        }
        assert_eq!(fs::read(&original).unwrap(), b"keep");
        assert!(!dir.join("missing.txt").exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
    }
}
