use super::{CpuTicks, SystemSample};
use std::ffi::c_void;
use std::path::Path;

#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}
impl FileTime {
    fn ticks(&self) -> u64 {
        u64::from(self.low) | (u64::from(self.high) << 32)
    }
}

#[repr(C)]
#[derive(Default)]
struct MemoryStatus {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page: u64,
    avail_page: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended: u64,
}
#[repr(C)]
#[derive(Default)]
struct ProcessMemory {
    size: u32,
    faults: u32,
    peak_working_set: usize,
    working_set: usize,
    peak_paged: usize,
    paged: usize,
    peak_nonpaged: usize,
    nonpaged: usize,
    pagefile: usize,
    peak_pagefile: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn K32GetProcessMemoryInfo(process: *mut c_void, counters: *mut ProcessMemory, size: u32) -> i32;
    fn GetDiskFreeSpaceExW(path: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
}

pub fn sample(disk: &Path) -> SystemSample {
    use std::os::windows::ffi::OsStrExt;
    let mut result = SystemSample::default();
    let (mut idle, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default());
    let mut memory = MemoryStatus { length: std::mem::size_of::<MemoryStatus>() as u32, ..Default::default() };
    let mut app = ProcessMemory { size: std::mem::size_of::<ProcessMemory>() as u32, ..Default::default() };
    let path: Vec<u16> = disk.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0;
    // SAFETY: Windows receives valid buffers with the documented C layouts and sizes;
    // the process handle is the non-owning pseudo-handle and the path is NUL-terminated.
    unsafe {
        if GetSystemTimes(&mut idle, &mut kernel, &mut user) != 0 {
            let total = kernel.ticks().saturating_add(user.ticks());
            result.cpu = Some(CpuTicks { total, busy: total.saturating_sub(idle.ticks()) });
        }
        if GlobalMemoryStatusEx(&mut memory) != 0 {
            result.memory = Some((memory.total_phys.saturating_sub(memory.avail_phys), memory.total_phys));
        }
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut app, std::mem::size_of::<ProcessMemory>() as u32) != 0 {
            result.app_memory = Some(app.working_set as u64);
        }
        if GetDiskFreeSpaceExW(path.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) != 0 {
            result.disk_free = Some(free);
        }
    }
    result
}
