//! An owned job contains a suspended process before its first instruction runs.
//! Closing the non-inherited handle also terminates its descendants.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::process::Child;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{GetProcessIdOfThread, OpenThread, ResumeThread, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME};

pub(super) struct Job {
    _handle: OwnedHandle,
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: each caller passes a fresh successful Win32 handle; ownership transfers once.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

impl Job {
    pub(super) fn attach(child: &Child) -> io::Result<Self> {
        // SAFETY: null attributes/name create an unnamed, non-inheritable job.
        let handle = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: both handles are live; the limits pointer has the exact declared size and lifetime.
        if unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the process was created suspended and is still owned by the caller.
        if unsafe { AssignProcessToJobObject(handle.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let job = Self { _handle: handle };
        resume(child.id())?;
        Ok(job)
    }
}

fn resume(process: u32) -> io::Result<()> {
    // SAFETY: snapshot creation does not mutate any process; the fresh handle is owned below.
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    let mut entry = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
    // SAFETY: entry is initialized and writable; snapshot stays open throughout iteration.
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
    for _ in 0..65536 {
        if found == 0 {
            break;
        }
        if entry.th32OwnerProcessID == process {
            // Hold and check the live handle: a snapshot's numeric thread id alone is insufficient.
            // SAFETY: opening a thread does not modify it; identity is checked before resuming it.
            let thread = owned(unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION, 0, entry.th32ThreadID) })?;
            // SAFETY: thread stays owned through this identity check and ResumeThread.
            if unsafe { GetProcessIdOfThread(thread.as_raw_handle()) } != process {
                // SAFETY: the initialized entry and owned snapshot remain valid for the next record.
                found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
                continue;
            }
            // SAFETY: the thread handle is live and belongs to the caller's suspended process.
            match unsafe { ResumeThread(thread.as_raw_handle()) } {
                1 => return Ok(()),
                u32::MAX => return Err(io::Error::last_os_error()),
                0 => {} // A running injected/helper thread did not own the creation suspension.
                _ => return Err(io::Error::other("the process has additional thread suspension owners")),
            }
        }
        // SAFETY: the same initialized entry and live snapshot are reused for the next record.
        found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
    }
    Err(io::Error::other("the suspended process's primary thread was not found"))
}
