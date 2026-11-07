//! The console hosts and shells of this process's terminals end with it.
//!
//! A pseudo console runs in a `conhost.exe` (or a side-by-side `OpenConsole.exe`) that `CreatePseudoConsole` starts as a
//! child of this process. When this process ends without `ClosePseudoConsole` (the app quits with terminals open, crashes,
//! or a test binary exits with terminals it never closed), older Windows releases (Windows 10, Windows Server 2022) leave
//! that console host running, and it spins on the broken pipe at a full core each. Every console host and shell a
//! terminal starts is therefore put into one job that closes with this process and kills what is in it. Terminals meant
//! to outlive the app run in the terminal daemon, a process of its own, so they end with the daemon instead.

use std::collections::HashSet;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

fn owned(handle: HANDLE) -> Option<OwnedHandle> {
    // SAFETY: a fresh, successful Win32 handle; ownership moves here once.
    (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then(|| unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// The job, made once and never closed: the handle closes when this process ends, however it ends.
fn job() -> Option<&'static OwnedHandle> {
    static JOB: OnceLock<Option<OwnedHandle>> = OnceLock::new();
    JOB.get_or_init(|| {
        // SAFETY: null attributes and name: an unnamed job whose handle children do not inherit.
        let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: a live job handle; the pointer and size describe `limits`, which outlives the call.
        let set = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        };
        (set != 0).then_some(job)
    })
    .as_ref()
}

/// The console hosts this process started that are still running.
pub(super) fn console_hosts() -> HashSet<u32> {
    let mut found = HashSet::new();
    // SAFETY: GetCurrentProcessId has no preconditions.
    let me = unsafe { GetCurrentProcessId() };
    // SAFETY: a snapshot only reads the process list; the handle is owned below.
    let Some(snapshot) = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else { return found };
    let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    // SAFETY: `entry` is initialised with its size and writable; the snapshot stays open while it is walked.
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]).to_ascii_lowercase();
        if entry.th32ParentProcessID == me && (name == "conhost.exe" || name == "openconsole.exe") {
            found.insert(entry.th32ProcessID);
        }
        // SAFETY: as for Process32FirstW.
        more = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    found
}

/// Put process `pid` into the job, so it ends with this process. Best effort: without the job (a Windows that refuses it)
/// the terminal works as before.
pub(super) fn adopt(pid: u32) -> bool {
    let Some(job) = job() else { return false };
    // SAFETY: opening a process for these rights does not change it; the handle is owned below.
    let Some(process) = owned(unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) }) else { return false };
    // SAFETY: both handles are live for the call.
    unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) != 0 }
}

/// Adopt the console hosts started since `before` was taken (the one `openpty` just made, and any another thread made
/// meanwhile: all of them are this process's).
pub(super) fn adopt_new_console_hosts(before: &HashSet<u32>) {
    for pid in console_hosts().difference(before) {
        adopt(*pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_our_job(pid: u32) -> bool {
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
        // SAFETY: a query-only handle, owned below.
        let process = owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }).expect("open the process");
        let mut result = 0;
        // SAFETY: live handles and a writable result.
        assert_ne!(unsafe { IsProcessInJob(process.as_raw_handle(), job().unwrap().as_raw_handle(), &mut result) }, 0);
        result != 0
    }

    /// A terminal's console host and shell are in the job, so neither can outlive this process.
    #[test]
    fn a_terminals_console_host_and_shell_end_with_this_process() {
        let before = console_hosts();
        let (pty, _reader) = super::super::spawn_with(super::super::Spawn {
            cols: 80,
            rows: 24,
            shell: Some("cmd.exe".into()),
            args: Some(vec!["/q".into(), "/k".into()]),
            ..Default::default()
        })
        .expect("spawn a terminal");
        let hosts: Vec<u32> = console_hosts().difference(&before).copied().collect();
        assert!(!hosts.is_empty(), "the pseudo console runs in a console host this process started");
        for host in &hosts {
            assert!(in_our_job(*host), "console host {host} is in the job");
        }
        let shell = pty.child.lock().unwrap().process_id().expect("the shell's id");
        assert!(in_our_job(shell), "the shell is in the job");
    }

    /// Run by the next test in a process of its own: open a terminal, say which processes it runs in, and exit without
    /// closing it (as an app that crashes, or a test binary that never closed its terminals).
    #[test]
    #[ignore = "a helper the next test runs in its own process"]
    fn helper_exits_with_an_open_terminal() {
        let before = console_hosts();
        let (pty, _reader) = super::super::spawn_with(super::super::Spawn {
            cols: 80,
            rows: 24,
            shell: Some("cmd.exe".into()),
            args: Some(vec!["/q".into(), "/k".into()]),
            ..Default::default()
        })
        .expect("spawn a terminal");
        let shell = pty.child.lock().unwrap().process_id().expect("the shell's id");
        let hosts: Vec<String> = console_hosts().difference(&before).map(u32::to_string).collect();
        println!("PIDS {shell} {}", hosts.join(" "));
        std::process::exit(0);
    }

    /// Windows 11 ends a console host whose owner is gone by itself; Windows Server 2022 (the CI runner) does not, and
    /// there this fails without the job.
    #[test]
    fn a_process_that_exits_with_an_open_terminal_takes_its_console_host_and_shell_along() {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::{PROCESS_SYNCHRONIZE, WaitForSingleObject};
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["pty::job::tests::helper_exits_with_an_open_terminal", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().find_map(|l| l.split_once("PIDS ").map(|(_, pids)| pids)).unwrap_or_else(|| panic!("the helper's processes: {text}"));
        let pids: Vec<u32> = line.split_whitespace().map(|p| p.parse().unwrap()).collect();
        assert!(pids.len() >= 2, "a shell and its console host: {line}");
        for pid in pids {
            // SAFETY: a wait-only handle, owned below; a process that is already gone cannot be opened (also an answer).
            let Some(process) = owned(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) }) else { continue };
            // SAFETY: a live handle; waits at most 10 s.
            let ended = unsafe { WaitForSingleObject(process.as_raw_handle(), 10_000) } == WAIT_OBJECT_0;
            assert!(ended, "process {pid} of the exited helper's terminal is still running");
        }
    }
}
