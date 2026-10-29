//! Windows: the app starts `codeyard-exthost` in a less-privileged AppContainer (LPAC) inside a Job Object
//! (docs/plugin-architecture.md §3). An AppContainer cannot be entered from inside a running process, so this is the
//! parent's half of the sandbox; the exthost only checks that it is in one.
//!
//! - LPAC without capabilities: no `internetClient` (no network), and no access to anything that does not name this
//!   container or "all restricted application packages" in its ACL (system DLLs do; the user's files do not).
//! - The container may read the exthost binary, the package and the compiled code, and read and write the data
//!   folder: `spawn` adds those grants to the files' ACLs.
//! - No child processes (`PROCESS_CREATION_CHILD_PROCESS_RESTRICTED`, and the Job Object allows one process).
//! - The Job Object caps committed memory (`limits::EXTHOST_MEMORY_BYTES`) and kills the process when the app's
//!   handle closes, so an exthost never outlives the app.
//!
//! Off until it passes on physical hardware (§15 decision 1): `scripts/check-exthost-sandbox-windows.ps1` runs the
//! escape probes through `spawn` and prints a record for the issue; then `VERIFIED` becomes true.

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GENERIC_ALL, HANDLE, HANDLE_FLAG_INHERIT, LocalFree, SetHandleInformation, WAIT_OBJECT_0};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName};
use windows_sys::Win32::Security::{
    ACE_FLAGS, ACL, DACL_SECURITY_INFORMATION, FreeSid, NO_INHERITANCE, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
    PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

// winbase.h; windows-sys keeps them under `WindowsProgramming`.
const PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT: u32 = 1;
const PROCESS_CREATION_CHILD_PROCESS_RESTRICTED: u32 = 1;

/// Whether AppContainer has passed the hardware check. While false, the exthost refuses to run installed code on
/// Windows and `--check-sandbox` answers `unavailable`.
pub const VERIFIED: bool = false;

/// What one extension's exthost may touch, and its memory cap.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Names the container: one per extension, so extensions cannot read each other's data.
    pub extension: String,
    pub package_dir: PathBuf,
    pub compiled: PathBuf,
    pub data_dir: PathBuf,
    /// Committed memory of the process; going over fails the allocation and the exthost dies.
    pub memory_bytes: u64,
}

/// A running exthost. Dropping it closes the Job Object, which kills the process.
pub struct Child {
    pub stdin: Option<File>,
    pub stdout: File,
    pub stderr: File,
    process: OwnedHandle,
    _job: OwnedHandle,
}

impl Child {
    /// Wait for the process to exit; its exit code.
    pub fn wait(&mut self) -> io::Result<u32> {
        let h = self.process.as_raw_handle() as HANDLE;
        // Safety: a live process handle owned by `self`.
        unsafe {
            if WaitForSingleObject(h, INFINITE) != WAIT_OBJECT_0 {
                return Err(io::Error::last_os_error());
            }
            let mut code = 0u32;
            if GetExitCodeProcess(h, &mut code) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(code)
        }
    }

    pub fn kill(&mut self) -> io::Result<()> {
        // Safety: as above.
        if unsafe { TerminateProcess(self.process.as_raw_handle() as HANDLE, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// The container's name: `codeyard.exthost.<hash of the extension id>` (names are limited to 64 characters).
pub fn container_name(extension: &str) -> String {
    // FNV-1a: stable across builds, no dependency.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in extension.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    format!("codeyard.exthost.{h:016x}")
}

/// Start `exe args…` in the extension's AppContainer and a Job Object, with piped stdin, stdout and stderr, the
/// inherited environment plus `env`, and the data folder as its working directory.
pub fn spawn(exe: &Path, args: &[OsString], env: &[(&str, &str)], policy: &Policy) -> io::Result<Child> {
    std::fs::create_dir_all(&policy.data_dir)?;
    let sid = Sid::for_container(&container_name(&policy.extension))?;
    let read = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
    grant(exe, sid.0, read, NO_INHERITANCE)?;
    grant(&policy.package_dir, sid.0, read, SUB_CONTAINERS_AND_OBJECTS_INHERIT)?;
    grant(&policy.compiled, sid.0, FILE_GENERIC_READ, NO_INHERITANCE)?;
    grant(&policy.data_dir, sid.0, GENERIC_ALL, SUB_CONTAINERS_AND_OBJECTS_INHERIT)?;

    let (child_in, parent_in) = pipe(true)?;
    let (parent_out, child_out) = pipe(false)?;
    let (parent_err, child_err) = pipe(false)?;
    let job = job(policy.memory_bytes)?;

    let mut caps = SECURITY_CAPABILITIES { AppContainerSid: sid.0, Capabilities: null_mut(), CapabilityCount: 0, Reserved: 0 };
    let lpac: u32 = PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;
    let no_children: u32 = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
    let handles = [raw(&child_in), raw(&child_out), raw(&child_err)];
    let mut attrs = Attributes::new(4)?;
    attrs.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, (&raw mut caps).cast(), size_of::<SECURITY_CAPABILITIES>())?;
    attrs.set(PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY, (&raw const lpac).cast(), size_of::<u32>())?;
    attrs.set(PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, (&raw const no_children).cast(), size_of::<u32>())?;
    attrs.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, handles.as_ptr().cast(), size_of_val(&handles))?;

    let mut si = STARTUPINFOEXW::default();
    si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = handles[0];
    si.StartupInfo.hStdOutput = handles[1];
    si.StartupInfo.hStdError = handles[2];
    si.lpAttributeList = attrs.ptr();
    let exe_w = wide(exe.as_os_str());
    let mut cmdline = wide(&command_line(exe.as_os_str(), args));
    let env_block = environment(env);
    let cwd = wide(policy.data_dir.as_os_str());
    let mut pi = PROCESS_INFORMATION::default();
    let flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT;
    // Safety: every pointer refers to a local that outlives the call; the attribute list holds pointers to `caps`,
    // `lpac`, `no_children` and `handles`, all still alive.
    let ok = unsafe {
        CreateProcessW(exe_w.as_ptr(), cmdline.as_mut_ptr(), null(), null(), 1, flags, env_block.as_ptr().cast(), cwd.as_ptr(), &si.StartupInfo, &mut pi)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // Safety: CreateProcessW returned these handles and nothing else owns them.
    let (process, thread) = unsafe { (OwnedHandle::from_raw_handle(pi.hProcess), OwnedHandle::from_raw_handle(pi.hThread)) };
    // Safety: live handles; the process is suspended until it is in the job.
    unsafe {
        if AssignProcessToJobObject(raw(&job), raw(&process)) == 0 || ResumeThread(raw(&thread)) == u32::MAX {
            let e = io::Error::last_os_error();
            TerminateProcess(raw(&process), 1);
            return Err(e);
        }
    }
    drop((child_in, child_out, child_err, thread));
    Ok(Child { stdin: Some(parent_in.into()), stdout: parent_out.into(), stderr: parent_err.into(), process, _job: job })
}

fn raw(h: &OwnedHandle) -> HANDLE {
    h.as_raw_handle() as HANDLE
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}

/// A pipe as (read end, write end). The child's end is inheritable; `child_reads` says which end that is.
fn pipe(child_reads: bool) -> io::Result<(OwnedHandle, OwnedHandle)> {
    let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: null_mut(), bInheritHandle: 1 };
    let (mut r, mut w): (HANDLE, HANDLE) = (null_mut(), null_mut());
    // Safety: out-pointers to locals; the handles are owned right after.
    unsafe {
        if CreatePipe(&mut r, &mut w, &sa, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        let (r, w) = (OwnedHandle::from_raw_handle(r), OwnedHandle::from_raw_handle(w));
        let parent = if child_reads { &w } else { &r };
        if SetHandleInformation(raw(parent), HANDLE_FLAG_INHERIT, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((r, w))
    }
}

fn job(memory_bytes: u64) -> io::Result<OwnedHandle> {
    // Safety: plain calls; the handle is owned right after creation.
    unsafe {
        let h = CreateJobObjectW(null(), null());
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = OwnedHandle::from_raw_handle(h);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        info.BasicLimitInformation.ActiveProcessLimit = 1;
        info.ProcessMemoryLimit = usize::try_from(memory_bytes).unwrap_or(usize::MAX);
        let len = size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        if SetInformationJobObject(raw(&job), JobObjectExtendedLimitInformation, (&raw const info).cast(), len) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
}

/// An AppContainer SID, created on first use.
struct Sid(PSID);

impl Sid {
    fn for_container(name: &str) -> io::Result<Sid> {
        let name_w = wide(OsStr::new(name));
        let mut sid: PSID = null_mut();
        // Safety: NUL-terminated strings, an out-pointer; the SID is freed in Drop.
        let hr = unsafe { CreateAppContainerProfile(name_w.as_ptr(), name_w.as_ptr(), name_w.as_ptr(), null(), 0, &mut sid) };
        let already = 0x8007_0000u32 as i32 | ERROR_ALREADY_EXISTS as i32;
        if hr == already {
            // Safety: as above.
            let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name_w.as_ptr(), &mut sid) };
            if hr < 0 {
                return Err(io::Error::other(format!("DeriveAppContainerSidFromAppContainerName: {hr:#x}")));
            }
        } else if hr < 0 {
            return Err(io::Error::other(format!("CreateAppContainerProfile: {hr:#x}")));
        }
        Ok(Sid(sid))
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        // Safety: the SID came from one of the two calls above.
        unsafe { FreeSid(self.0) };
    }
}

/// Add an allow entry for `sid` to `path`'s ACL.
fn grant(path: &Path, sid: PSID, access: u32, inherit: ACE_FLAGS) -> io::Result<()> {
    let path_w = wide(path.as_os_str());
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    let err = |what: &str, code: u32| io::Error::other(format!("{what} {}: {}", path.display(), io::Error::from_raw_os_error(code as i32)));
    // Safety: NUL-terminated path, out-pointers to locals; both buffers are freed with LocalFree.
    unsafe {
        let r = GetNamedSecurityInfoW(path_w.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), &mut dacl, null_mut(), &mut sd);
        if r != 0 {
            return Err(err("reading the ACL of", r));
        }
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: access,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: inherit,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: sid.cast(),
            },
        };
        let mut new: *mut ACL = null_mut();
        let r = SetEntriesInAclW(1, &entry, dacl, &mut new);
        LocalFree(sd);
        if r != 0 {
            return Err(err("building the ACL of", r));
        }
        let r = SetNamedSecurityInfoW(path_w.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), new, null());
        LocalFree(new.cast());
        if r != 0 {
            return Err(err("writing the ACL of", r));
        }
    }
    Ok(())
}

/// A process attribute list.
struct Attributes(Vec<usize>);

impl Attributes {
    fn new(count: u32) -> io::Result<Attributes> {
        let mut size = 0usize;
        // Safety: the documented size query (it fails with ERROR_INSUFFICIENT_BUFFER and sets `size`), then the
        // initialisation of a buffer of that size.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), count, 0, &mut size);
            let mut buf = vec![0usize; size.div_ceil(size_of::<usize>())];
            if InitializeProcThreadAttributeList(buf.as_mut_ptr().cast(), count, 0, &mut size) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Attributes(buf))
        }
    }

    fn ptr(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }

    fn set(&mut self, attribute: u32, value: *const c_void, size: usize) -> io::Result<()> {
        // Safety: an initialised list; the caller keeps `value` alive until the process is created.
        if unsafe { UpdateProcThreadAttribute(self.ptr(), 0, attribute as usize, value, size, null_mut(), null()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // Safety: initialised in `new`.
        unsafe { DeleteProcThreadAttributeList(self.ptr()) };
    }
}

/// The command line, quoted the way the C runtime splits it (the same rules as `std::process::Command`).
fn command_line(exe: &OsStr, args: &[OsString]) -> OsString {
    let mut out = String::new();
    for (i, a) in std::iter::once(exe).chain(args.iter().map(OsString::as_os_str)).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let a = a.to_string_lossy();
        if !a.is_empty() && !a.contains([' ', '\t', '"']) {
            out.push_str(&a);
            continue;
        }
        out.push('"');
        let mut backslashes = 0;
        for c in a.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    backslashes = 0;
                }
                _ => {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    backslashes = 0;
                }
            }
            if c != '\\' {
                out.push(c);
            }
        }
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
        out.push('"');
    }
    out.into()
}

/// The inherited environment plus `extra`, as a sorted UTF-16 block.
fn environment(extra: &[(&str, &str)]) -> Vec<u16> {
    let mut vars: std::collections::BTreeMap<String, OsString> = std::env::vars_os()
        .map(|(k, v)| {
            let mut kv = k.clone();
            kv.push("=");
            kv.push(v);
            (k.to_string_lossy().to_uppercase(), kv)
        })
        .collect();
    for (k, v) in extra {
        vars.insert(k.to_uppercase(), format!("{k}={v}").into());
    }
    let mut block: Vec<u16> = vars.values().flat_map(|kv| kv.encode_wide().chain([0])).collect();
    block.push(0);
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_quoted_for_the_c_runtime() {
        let line = command_line(OsStr::new(r"C:\a b\x.exe"), &["plain".into(), "two words".into(), r#"q"uote"#.into(), r"C:\dir\".into(), "".into()]);
        assert_eq!(line, OsString::from(r#""C:\a b\x.exe" plain "two words" "q\"uote" C:\dir\ """#));
        assert_eq!(command_line(OsStr::new("x"), &[r"a b\".into()]), OsString::from(r#"x "a b\\""#));
    }

    #[test]
    fn each_extension_gets_its_own_short_container() {
        let a = container_name("you.probe");
        assert_eq!(a, container_name("you.probe"));
        assert_ne!(a, container_name("you.other"));
        assert!(a.len() <= 64);
    }
}
