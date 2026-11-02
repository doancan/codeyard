//! The resident-memory cap of exthosts on macOS and Linux (docs/extensions-spike.md decision 5). `RLIMIT_AS` cannot
//! cap them (wasmtime reserves hundreds of GB of address space), so the app reads each running exthost's resident size
//! every few seconds and stops one that goes over `limits::EXTHOST_MEMORY_BYTES`. Windows caps the process with a Job
//! Object commit limit instead, and nothing here runs there.
//!
//! No UI here: reading the size and deciding. The window stops the process (`workbench_ext_prompts.rs`).

use std::time::Duration;

/// How often the window reads the sizes while any exthost runs.
pub const EVERY: Duration = Duration::from_secs(3);

/// Whether this system's exthosts are capped here (not on Windows, where the Job Object does it).
pub const CHECKED_HERE: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// What process `pid` holds in memory, in bytes: on macOS its physical footprint (what Activity Monitor shows as
/// Memory), on Linux its resident set. `None` when the process is gone or the system does not say.
pub fn resident_bytes(pid: u32) -> Option<u64> {
    imp::resident_bytes(pid)
}

#[cfg(target_os = "macos")]
mod imp {
    pub fn resident_bytes(pid: u32) -> Option<u64> {
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a valid, writable rusage_info_v2, which is what RUSAGE_INFO_V2 fills.
        let r = unsafe { libc::proc_pid_rusage(pid as libc::c_int, libc::RUSAGE_INFO_V2, (&mut info as *mut libc::rusage_info_v2).cast()) };
        (r == 0).then_some(info.ri_phys_footprint)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub fn resident_bytes(pid: u32) -> Option<u64> {
        let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        super::from_statm(&statm, page_size())
    }

    fn page_size() -> u64 {
        // SAFETY: sysconf has no preconditions.
        let p = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if p > 0 { p as u64 } else { 4096 }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    pub fn resident_bytes(_pid: u32) -> Option<u64> {
        None
    }
}

/// `/proc/<pid>/statm`: sizes in pages; the second is the resident set.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn from_statm(statm: &str, page: u64) -> Option<u64> {
    statm.split_whitespace().nth(1)?.parse::<u64>().ok().map(|pages| pages * page)
}

/// The exthosts over `cap`: (extension, bytes), from (extension, resident bytes) pairs.
pub fn over(sizes: &[(String, Option<u64>)], cap: u64) -> Vec<(String, u64)> {
    sizes.iter().filter_map(|(id, b)| b.filter(|b| *b > cap).map(|b| (id.clone(), b))).collect()
}

/// The reason an exthost was stopped, for its log, the notice and the failed calls.
pub fn reason(bytes: u64, cap: u64) -> String {
    crate::i18n::tf("it used {0} MB of memory, over the limit of {1} MB, and was stopped", &[&mb(bytes), &mb(cap)])
}

fn mb(b: u64) -> u64 {
    b / (1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statm_gives_the_resident_set() {
        assert_eq!(from_statm("123456 2048 300 10 0 900 0\n", 4096), Some(2048 * 4096));
        assert_eq!(from_statm("", 4096), None);
    }

    #[test]
    fn only_processes_over_the_cap_are_named() {
        let sizes = vec![("you.a".to_string(), Some(600u64 << 20)), ("you.b".to_string(), Some(20 << 20)), ("you.c".to_string(), None)];
        assert_eq!(over(&sizes, 512 << 20), vec![("you.a".to_string(), 600 << 20)]);
        assert_eq!(reason(600 << 20, 512 << 20), "it used 600 MB of memory, over the limit of 512 MB, and was stopped");
    }

    #[test]
    fn this_process_has_a_resident_size() {
        if CHECKED_HERE {
            let b = resident_bytes(std::process::id()).expect("readable");
            assert!(b > 1 << 20, "{b}");
        }
        assert_eq!(resident_bytes(u32::MAX - 1), None, "a process that does not exist");
    }
}
