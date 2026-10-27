use super::{CpuTicks, SystemSample};
use std::path::Path;

unsafe extern "C" {
    fn mach_port_deallocate(task: libc::mach_port_t, name: libc::mach_port_t) -> libc::kern_return_t;
}

// libc moved these declarations to mach2; the underlying Darwin APIs remain supported.
#[allow(deprecated)]
pub fn sample(disk: &Path) -> SystemSample {
    let mut result = SystemSample::default();
    // SAFETY: every Mach/sysctl output points to a correctly sized, initialized buffer;
    // return codes and lengths are checked before using the samples.
    unsafe {
        let host = libc::mach_host_self();
        let mut cpu: libc::host_cpu_load_info = std::mem::zeroed();
        let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
        if libc::host_statistics(host, libc::HOST_CPU_LOAD_INFO, (&mut cpu as *mut libc::host_cpu_load_info).cast(), &mut count) == libc::KERN_SUCCESS {
            let total = cpu.cpu_ticks.iter().map(|&n| u64::from(n)).sum::<u64>();
            result.cpu = Some(CpuTicks { total, busy: total - u64::from(cpu.cpu_ticks[libc::CPU_STATE_IDLE as usize]) });
        }
        let mut total = 0u64;
        let mut size = std::mem::size_of_val(&total);
        let mut vm: libc::vm_statistics64 = std::mem::zeroed();
        count = libc::HOST_VM_INFO64_COUNT;
        if libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut total as *mut u64).cast(), &mut size, std::ptr::null_mut(), 0) == 0
            && libc::host_statistics64(host, libc::HOST_VM_INFO64, (&mut vm as *mut libc::vm_statistics64).cast(), &mut count) == libc::KERN_SUCCESS
        {
            // Cached file pages are reclaimable; wired, anonymous and compressed pages are in use.
            let pages = u64::from(vm.active_count) + u64::from(vm.inactive_count) + u64::from(vm.wire_count) + u64::from(vm.compressor_page_count);
            let used =
                pages.saturating_sub(u64::from(vm.external_page_count) + u64::from(vm.purgeable_count)) * libc::sysconf(libc::_SC_PAGESIZE).max(1) as u64;
            result.memory = Some((used.min(total), total));
        }
        let mut info: libc::mach_task_basic_info = std::mem::zeroed();
        count = libc::MACH_TASK_BASIC_INFO_COUNT;
        if libc::task_info(libc::mach_task_self(), libc::MACH_TASK_BASIC_INFO, (&mut info as *mut libc::mach_task_basic_info).cast(), &mut count)
            == libc::KERN_SUCCESS
        {
            result.app_memory = Some(info.resident_size);
        }
        let mut load = [0.; 3];
        if libc::getloadavg(load.as_mut_ptr(), 3) == 3 {
            result.load = Some(load);
        }
        // mach_host_self creates a send right for each call.
        mach_port_deallocate(libc::mach_task_self(), host);
    }
    result.disk_free = super::unix_disk_free(disk);
    result
}
