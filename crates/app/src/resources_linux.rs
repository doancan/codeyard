use super::{CpuTicks, SystemSample};
use std::path::Path;

pub fn sample(disk: &Path) -> SystemSample {
    let mut result = SystemSample::default();
    if let Ok(stat) = std::fs::read_to_string("/proc/stat") {
        let fields: Vec<u64> = stat.lines().next().unwrap_or_default().split_whitespace().skip(1).take(8).filter_map(|v| v.parse().ok()).collect();
        if fields.len() >= 4 {
            let total = fields.iter().sum::<u64>();
            let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
            result.cpu = Some(CpuTicks { total, busy: total.saturating_sub(idle) });
        }
    }
    if let Ok(info) = std::fs::read_to_string("/proc/meminfo") {
        let kb = |key: &str| info.lines().find_map(|l| l.strip_prefix(key)?.split_whitespace().next()?.parse::<u64>().ok()).map(|n| n.saturating_mul(1024));
        if let (Some(total), Some(available)) = (kb("MemTotal:"), kb("MemAvailable:")) {
            result.memory = Some((total.saturating_sub(available), total));
        }
    }
    if let Ok(stat) = std::fs::read_to_string("/proc/self/statm") {
        // SAFETY: sysconf has no pointer arguments.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
        result.app_memory = stat.split_whitespace().nth(1).and_then(|n| n.parse::<u64>().ok()).map(|n| n.saturating_mul(page));
    }
    let mut load = [0.; 3];
    // SAFETY: the output buffer has exactly three entries.
    if unsafe { libc::getloadavg(load.as_mut_ptr(), 3) } == 3 {
        result.load = Some(load);
    }
    result.disk_free = super::unix_disk_free(disk);
    result
}
