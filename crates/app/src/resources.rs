//! On-demand, local resource samples. No subprocesses, network requests or directory watches.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default)]
pub struct CpuTicks {
    pub busy: u64,
    pub total: u64,
}

impl CpuTicks {
    pub fn usage_since(self, old: Self) -> Option<f32> {
        let total = self.total.checked_sub(old.total)?;
        let busy = self.busy.checked_sub(old.busy)?;
        (total > 0 && busy <= total).then(|| busy as f32 / total as f32 * 100.)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SystemSample {
    pub cpu: Option<CpuTicks>,
    pub cores: usize,
    pub load: Option<[f64; 3]>,
    pub memory: Option<(u64, u64)>, // used, total bytes
    pub app_memory: Option<u64>,
    pub disk_free: Option<u64>,
}

pub fn sample(disk: &Path) -> SystemSample {
    let mut sample = platform::sample(disk);
    sample.cores = std::thread::available_parallelism().map_or(1, usize::from);
    sample
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // libc counters differ in width across Unix targets.
fn unix_disk_free(disk: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(disk.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: statvfs initializes the buffer on success; path is NUL-terminated.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: the successful call initialized every field.
    let stat = unsafe { stat.assume_init() };
    Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

#[derive(Clone, Debug, Default)]
pub struct ProjectSize {
    pub bytes: u64,
    pub files: u64,
    pub partial: bool,
}

/// Apparent bytes of regular files, including hidden/build files. Never follow symlinks.
/// Stop at 100k entries or 1.5 seconds, and label that result as partial.
pub fn project_size(root: PathBuf, cancel: &AtomicBool) -> ProjectSize {
    let mut result = ProjectSize::default();
    let mut pending = vec![root];
    let started = Instant::now();
    let mut visited = 0;
    while let Some(dir) = pending.pop() {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => {
                result.partial = true;
                continue;
            }
        };
        for entry in entries {
            if cancel.load(Ordering::Relaxed) || visited >= 100_000 || started.elapsed() >= Duration::from_millis(1500) {
                result.partial = true;
                return result;
            }
            visited += 1;
            let meta = entry.and_then(|e| std::fs::symlink_metadata(e.path()).map(|m| (e.path(), m)));
            match meta {
                Ok((path, meta)) if meta.is_dir() => pending.push(path),
                Ok((_, meta)) if meta.is_file() => {
                    result.files += 1;
                    result.bytes = result.bytes.saturating_add(meta.len());
                }
                Ok(_) => {}
                Err(_) => result.partial = true,
            }
        }
    }
    result
}

#[cfg(target_os = "macos")]
#[path = "resources_macos.rs"]
mod platform;
#[cfg(target_os = "linux")]
#[path = "resources_linux.rs"]
mod platform;
#[cfg(windows)]
#[path = "resources_windows.rs"]
mod platform;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    pub fn sample(_: &std::path::Path) -> super::SystemSample {
        super::SystemSample::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_usage_requires_two_valid_samples() {
        let old = CpuTicks { busy: 100, total: 200 };
        assert_eq!(CpuTicks { busy: 125, total: 300 }.usage_since(old), Some(25.));
        assert_eq!(old.usage_since(old), None);
        assert_eq!(CpuTicks { busy: 1, total: 2 }.usage_since(old), None);
        assert_eq!(CpuTicks { busy: 400, total: 300 }.usage_since(old), None);
    }

    #[test]
    fn project_size_counts_hidden_files_and_never_follows_symlinks() {
        let root = crate::qa_workbench_tests::fresh("resource-size");
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), b"abc").unwrap();
        std::fs::write(root.join("file"), b"12345").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        let size = project_size(root.clone(), &AtomicBool::new(false));
        assert_eq!((size.bytes, size.files, size.partial), (8, 2, false));
        assert!(project_size(root, &AtomicBool::new(true)).partial);
    }

    #[test]
    fn native_sample_has_real_memory_and_cpu_counters() {
        let sample = sample(&std::env::temp_dir());
        assert!(sample.cores >= 1);
        assert!(sample.cpu.is_some());
        let (used, total) = sample.memory.expect("native physical memory");
        assert!(used <= total && total > 0);
        assert!(sample.app_memory.is_some_and(|n| n > 0));
        assert!(sample.disk_free.is_some_and(|n| n > 0));
    }
}
