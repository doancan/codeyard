//! Linux: a network namespace, landlock and seccomp, in that order (seccomp then refuses `unshare` itself).
//!
//! The process is still single-threaded here: `unshare(CLONE_NEWUSER)` requires that, and landlock and seccomp
//! apply to the calling thread and the threads it starts later.

use landlock::{ABI, Access, AccessFs, AccessNet, BitFlags, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope};

use super::Policy;

/// The landlock ABI these rules are written and tested against. Older kernels enforce the subset they know (ABI 1,
/// Linux 5.13, is the minimum); newer ones add nothing unless this is raised.
const ABI_TESTED: ABI = ABI::V6;

pub fn enter(policy: &Policy) -> Result<String, String> {
    let read = Policy::resolved(&policy.read)?;
    let write = Policy::resolved(&policy.write)?;
    let abi = landlock_abi();
    if abi < 1 {
        return Err("this Linux kernel has no landlock (Linux 5.13 or later with landlock enabled is required)".into());
    }
    let netns = unshare_network();
    landlock(&read, &write)?;
    seccomp()?;
    Ok(format!("landlock ABI {abi}, seccomp, {}", if netns { "network namespace" } else { "no network namespace (seccomp refuses sockets)" }))
}

/// The kernel's landlock ABI version; 0 or less without landlock.
fn landlock_abi() -> i64 {
    const LANDLOCK_CREATE_RULESET_VERSION: libc::c_ulong = 1;
    // Safety: the documented version query: no ruleset, size 0, the VERSION flag.
    unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<libc::c_void>(), 0usize, LANDLOCK_CREATE_RULESET_VERSION) }
}

/// An empty network namespace: no interfaces but a down loopback. Unprivileged processes need a user namespace for
/// it, which some systems turn off (sysctls, AppArmor on Ubuntu 24.04); then seccomp alone keeps sockets closed.
fn unshare_network() -> bool {
    // Safety: plain syscalls on this (single-threaded) process.
    unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) == 0 || libc::unshare(libc::CLONE_NEWNET) == 0 }
}

fn landlock(read: &[(std::path::PathBuf, bool)], write: &[(std::path::PathBuf, bool)]) -> Result<(), String> {
    let e = |e: landlock::RulesetError| format!("landlock: {e}");
    let reads: BitFlags<AccessFs> = AccessFs::ReadFile | AccessFs::ReadDir;
    let writes = AccessFs::from_all(ABI_TESTED) & !BitFlags::from(AccessFs::Execute);
    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(ABI_TESTED))
        .map_err(e)?
        .handle_access(AccessNet::from_all(ABI_TESTED))
        .map_err(e)?
        .scope(Scope::from_all(ABI_TESTED))
        .map_err(e)?
        .create()
        .map_err(e)?;
    let devices = [("/dev/null", AccessFs::ReadFile | AccessFs::WriteFile), ("/dev/urandom", BitFlags::from(AccessFs::ReadFile))];
    let rules = read
        .iter()
        .map(|(p, dir)| (p.as_path(), *dir, reads))
        .chain(write.iter().map(|(p, dir)| (p.as_path(), *dir, writes)))
        .chain(devices.iter().map(|(p, a)| (std::path::Path::new(p), false, *a)));
    for (path, dir, access) in rules {
        let access = if dir { access } else { access & AccessFs::from_file(ABI_TESTED) };
        let fd = PathFd::new(path).map_err(|err| format!("landlock: {}: {err}", path.display()))?;
        ruleset = ruleset.add_rule(PathBeneath::new(fd, access)).map_err(e)?;
    }
    let status = ruleset.restrict_self().map_err(e)?;
    if status.ruleset == RulesetStatus::NotEnforced {
        return Err("landlock is not enforced on this kernel".into());
    }
    Ok(())
}

// seccomp: a classic BPF program over `struct seccomp_data { int nr; u32 arch; u64 ip; u64 args[6]; }`.
const NR: u32 = 0;
const ARCH: u32 = 4;
const fn arg_low(i: u32) -> u32 {
    16 + 8 * i // the low 32 bits on little-endian targets
}

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xC000_00B7;
#[cfg(target_arch = "riscv64")]
const AUDIT_ARCH: u32 = 0xC000_00F3;

const RET_KILL_PROCESS: u32 = 0x8000_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const RET_ALLOW: u32 = 0x7fff_0000;

/// Syscalls refused with `EPERM`. Many already need privileges this process lacks; they are listed so that a
/// kernel bug in one of them is not reachable from here.
const DENIED: &[libc::c_long] = &[
    // New programs and processes (`clone` without `CLONE_THREAD` is refused below).
    libc::SYS_execve,
    libc::SYS_execveat,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_fork,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_vfork,
    // Network and IPC endpoints.
    libc::SYS_socket,
    libc::SYS_socketpair,
    // Other processes.
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_process_madvise,
    libc::SYS_pidfd_getfd,
    libc::SYS_kcmp,
    // Namespaces, mounts and file handles that bypass paths.
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_chroot,
    libc::SYS_open_tree,
    libc::SYS_move_mount,
    libc::SYS_fsopen,
    libc::SYS_fsconfig,
    libc::SYS_fsmount,
    libc::SYS_fspick,
    libc::SYS_mount_setattr,
    libc::SYS_open_by_handle_at,
    libc::SYS_name_to_handle_at,
    // Large kernel surfaces this process never needs.
    libc::SYS_bpf,
    libc::SYS_perf_event_open,
    libc::SYS_userfaultfd,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_kexec_load,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_acct,
    libc::SYS_swapon,
    libc::SYS_swapoff,
    libc::SYS_reboot,
    libc::SYS_syslog,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_iopl,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_ioperm,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_modify_ldt,
];

/// `ioctl` requests refused on any descriptor: they push input into a terminal (stderr may be one under `cyx dev`).
const DENIED_IOCTLS: &[u32] = &[libc::TIOCSTI as u32, libc::TIOCLINUX as u32];

fn stmt(code: u16, k: u32) -> libc::sock_filter {
    libc::sock_filter { code, jt: 0, jf: 0, k }
}

fn jump(code: u16, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter { code, jt, jf, k }
}

/// The filter, as instructions. Everything not listed is allowed: wasmtime and the standard library need the rest.
fn program() -> Vec<libc::sock_filter> {
    const LD: u16 = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    const JEQ: u16 = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    const JSET: u16 = (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16;
    const RET: u16 = (libc::BPF_RET | libc::BPF_K) as u16;
    let eperm = RET_ERRNO | libc::EPERM as u32;
    let mut p = vec![
        // Another architecture's syscall numbers mean other syscalls: kill.
        stmt(LD, ARCH),
        jump(JEQ, AUDIT_ARCH, 1, 0),
        stmt(RET, RET_KILL_PROCESS),
        stmt(LD, NR),
    ];
    // x32 syscalls are the x86-64 numbers with bit 30 set; none is needed, and they would bypass the list.
    #[cfg(target_arch = "x86_64")]
    p.extend([jump((libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K) as u16, 0x4000_0000, 0, 1), stmt(RET, eperm)]);
    for &nr in DENIED {
        p.extend([jump(JEQ, nr as u32, 0, 1), stmt(RET, eperm)]);
    }
    // clone3 passes its flags in memory, out of the filter's reach: ENOSYS makes the C library fall back to clone.
    p.extend([jump(JEQ, libc::SYS_clone3 as u32, 0, 1), stmt(RET, RET_ERRNO | libc::ENOSYS as u32)]);
    // ioctl: the request is the second argument.
    p.extend([jump(JEQ, libc::SYS_ioctl as u32, 0, DENIED_IOCTLS.len() as u8 + 3), stmt(LD, arg_low(1))]);
    for (i, &req) in DENIED_IOCTLS.iter().enumerate() {
        p.push(jump(JEQ, req, (DENIED_IOCTLS.len() - i) as u8, 0));
    }
    p.extend([stmt(RET, RET_ALLOW), stmt(RET, eperm)]);
    // clone: threads only.
    p.extend([jump(JEQ, libc::SYS_clone as u32, 0, 3), stmt(LD, arg_low(0)), jump(JSET, libc::CLONE_THREAD as u32, 1, 0), stmt(RET, eperm)]);
    p.push(stmt(RET, RET_ALLOW));
    p
}

fn seccomp() -> Result<(), String> {
    let filter = program();
    let prog = libc::sock_fprog { len: filter.len() as u16, filter: filter.as_ptr() as *mut libc::sock_filter };
    // Safety: `prog` points at `filter`, which outlives the call; the kernel copies it. TSYNC applies it to every
    // thread (there is only one).
    let r = unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(format!("prctl(NO_NEW_PRIVS): {}", std::io::Error::last_os_error()));
        }
        libc::syscall(libc::SYS_seccomp, libc::SECCOMP_SET_MODE_FILTER, libc::SECCOMP_FILTER_FLAG_TSYNC, &prog as *const libc::sock_fprog)
    };
    if r != 0 {
        return Err(format!("seccomp: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the filter the way the kernel does, for the instructions it uses.
    fn verdict(arch: u32, nr: libc::c_long, args: [u64; 6]) -> u32 {
        let p = program();
        let word = |off: u32| -> u32 {
            match off {
                ARCH => arch,
                NR => nr as u32,
                o => args[((o - 16) / 8) as usize] as u32,
            }
        };
        let (mut pc, mut acc) = (0usize, 0u32);
        loop {
            let ins = p[pc];
            let class = ins.code & 0x07;
            if class == libc::BPF_RET as u16 {
                return ins.k;
            } else if class == libc::BPF_LD as u16 {
                acc = word(ins.k);
                pc += 1;
            } else {
                let taken = match ins.code & 0xf0 {
                    op if op == libc::BPF_JEQ as u16 => acc == ins.k,
                    op if op == libc::BPF_JGE as u16 => acc >= ins.k,
                    op if op == libc::BPF_JSET as u16 => acc & ins.k != 0,
                    op => panic!("unexpected jump {op:#x}"),
                };
                pc += 1 + if taken { ins.jt } else { ins.jf } as usize;
            }
            assert!(pc < p.len(), "the filter runs off its end");
        }
    }

    fn call(nr: libc::c_long, args: [u64; 6]) -> u32 {
        verdict(AUDIT_ARCH, nr, args)
    }

    #[test]
    fn the_filter_refuses_processes_sockets_and_terminal_input() {
        let eperm = RET_ERRNO | libc::EPERM as u32;
        assert_eq!(call(libc::SYS_execve, [0; 6]), eperm);
        assert_eq!(call(libc::SYS_socket, [libc::AF_INET as u64, 0, 0, 0, 0, 0]), eperm);
        assert_eq!(call(libc::SYS_socket, [libc::AF_UNIX as u64, 0, 0, 0, 0, 0]), eperm);
        assert_eq!(call(libc::SYS_ptrace, [0; 6]), eperm);
        assert_eq!(call(libc::SYS_unshare, [0; 6]), eperm);
        // A thread may start; a process may not.
        let thread = (libc::CLONE_VM | libc::CLONE_FS | libc::CLONE_FILES | libc::CLONE_SIGHAND | libc::CLONE_THREAD) as u64;
        assert_eq!(call(libc::SYS_clone, [thread, 0, 0, 0, 0, 0]), RET_ALLOW);
        assert_eq!(call(libc::SYS_clone, [(libc::CLONE_VM | libc::CLONE_VFORK) as u64, 0, 0, 0, 0, 0]), eperm);
        assert_eq!(call(libc::SYS_clone3, [0; 6]), RET_ERRNO | libc::ENOSYS as u32);
        assert_eq!(call(libc::SYS_ioctl, [2, libc::TIOCSTI as _, 0, 0, 0, 0]), eperm);
        assert_eq!(call(libc::SYS_ioctl, [2, libc::TIOCLINUX as _, 0, 0, 0, 0]), eperm);
        assert_eq!(call(libc::SYS_ioctl, [0, libc::FIONREAD as _, 0, 0, 0, 0]), RET_ALLOW);
        assert_eq!(call(libc::SYS_read, [0; 6]), RET_ALLOW);
        assert_eq!(call(libc::SYS_mprotect, [0; 6]), RET_ALLOW);
        assert_eq!(call(libc::SYS_openat, [0; 6]), RET_ALLOW);
        assert_eq!(verdict(0x4000_0003, libc::SYS_read, [0; 6]), RET_KILL_PROCESS);
        #[cfg(target_arch = "x86_64")]
        assert_eq!(call(libc::SYS_socket | 0x4000_0000, [0; 6]), eperm);
    }
}
