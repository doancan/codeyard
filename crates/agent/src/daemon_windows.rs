//! The daemon's transport on Windows: a named pipe that only the current user can open.
//!
//! - **Server:** the pipe is created with a protected DACL that grants access to the user's SID alone, rejects remote
//!   clients, and is the first instance (another daemon already listening is found instead of joined).
//! - **Client:** it checks that the process serving the pipe runs as the same user, so a pipe name taken first by
//!   someone else is refused.
//! - **I/O:** both ends use overlapped I/O, so a read waiting for the peer never holds up a write on the same pipe.
//!   [`Pipe::shutdown`] cancels a pending read, which is how a client ends its connection at once.

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CreateEventW, DETACHED_PROCESS, GetCurrentProcess, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::*;

const BUFFER: u32 = 64 * 1024;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh, valid handle whose ownership moves here once.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// The SID (`S-1-5-…`) of the user `process` runs as.
fn process_user(process: HANDLE) -> io::Result<String> {
    let mut token = std::ptr::null_mut();
    // SAFETY: `process` is a live handle with query access; `token` receives a new handle, owned below.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = owned(token)?;
    let mut len = 0u32;
    // SAFETY: asking for the size only: a null buffer of length 0.
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, std::ptr::null_mut(), 0, &mut len) };
    // u64 words keep the buffer aligned for TOKEN_USER.
    let mut buffer = vec![0u64; (len as usize).div_ceil(8).max(1)];
    // SAFETY: the buffer is writable and at least `len` bytes long.
    if unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, buffer.as_mut_ptr().cast(), len, &mut len) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: GetTokenInformation(TokenUser) filled the aligned buffer with a TOKEN_USER whose SID points into it.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = std::ptr::null_mut();
    // SAFETY: `sid` is valid while `buffer` lives; `text` receives a LocalAlloc'd string, freed below.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a NUL-terminated wide string from ConvertSidToStringSidW.
    let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    // SAFETY: `len` characters were just counted in the same string.
    let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    // SAFETY: allocated by ConvertSidToStringSidW, freed once.
    unsafe { LocalFree(text.cast()) };
    Ok(sid)
}

/// The SID of the user this process runs as.
fn current_user() -> io::Result<String> {
    // SAFETY: the pseudo-handle of the current process needs no closing.
    process_user(unsafe { GetCurrentProcess() })
}

/// A security descriptor granting access to `sid` only (protected: nothing inherited), freed on drop.
struct OwnerOnly(*mut c_void);

impl OwnerOnly {
    fn new(sid: &str) -> io::Result<Self> {
        let mut sd = std::ptr::null_mut();
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
        // SAFETY: a NUL-terminated SDDL string; `sd` receives a LocalAlloc'd descriptor, freed in Drop.
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(sd))
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: self.0, bInheritHandle: 0 }
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW, freed once.
        unsafe { LocalFree(self.0) };
    }
}

/// The pipe name of the current user's terminal daemon for `scope` (the app's state folder, so separate profiles and
/// test runs get daemons of their own): `\\.\pipe\codeyard-<hash>-terminals`, the hash over the user's SID and `scope`.
pub fn pipe_name(scope: &str) -> io::Result<String> {
    let hash = proto::content_hash(format!("{}\0{scope}", current_user()?).as_bytes());
    Ok(format!(r"\\.\pipe\codeyard-{hash:016x}-terminals"))
}

/// One end of a pipe connection. Clones share it (one thread reads while another writes).
#[derive(Clone)]
pub struct Pipe {
    handle: Arc<OwnedHandle>,
    closed: Arc<AtomicBool>,
}

impl Pipe {
    fn new(handle: OwnedHandle) -> Self {
        Pipe { handle: Arc::new(handle), closed: Arc::default() }
    }

    /// One overlapped operation, waited for: `op` starts it with the given OVERLAPPED.
    fn io(&self, op: impl FnOnce(*mut OVERLAPPED) -> i32) -> io::Result<usize> {
        // SAFETY: a manual-reset, unnamed event; owned below.
        let event = owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })?;
        // SAFETY: OVERLAPPED is plain data; all zero is its initial state.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event.as_raw_handle();
        if op(&mut overlapped) == 0 {
            // SAFETY: reads this thread's last error.
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_PENDING {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
        }
        let mut done = 0u32;
        // SAFETY: waits for the operation started above to finish (or be cancelled); `overlapped` outlives it.
        if unsafe { GetOverlappedResult(self.handle.as_raw_handle(), &overlapped, &mut done, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(done as usize)
    }

    /// End the connection from this side: a pending read is cancelled and every later read is end of input, so the
    /// reader lets go and the handle closes with the last clone.
    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::Relaxed);
        // SAFETY: cancels this handle's pending I/O; harmless when there is none.
        unsafe { CancelIoEx(self.handle.as_raw_handle(), std::ptr::null()) };
    }

    /// The handle, for checks on it (tests).
    #[cfg(test)]
    pub(crate) fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
}

/// The peer has gone (or this side shut the connection).
fn ended(error: &io::Error) -> bool {
    let code = error.raw_os_error().map(|c| c as u32);
    matches!(code, Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED | ERROR_OPERATION_ABORTED))
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.closed.load(Ordering::Relaxed) {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize) as u32;
        let handle = self.handle.as_raw_handle();
        // SAFETY: `buf` is writable for `len` bytes and outlives the waited-for operation.
        match self.io(|ov| unsafe { ReadFile(handle, buf.as_mut_ptr(), len, std::ptr::null_mut(), ov) }) {
            Err(e) if ended(&e) => Ok(0),
            other => other,
        }
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let len = buf.len().min(u32::MAX as usize) as u32;
        let handle = self.handle.as_raw_handle();
        // SAFETY: `buf` is readable for `len` bytes and outlives the waited-for operation.
        match self.io(|ov| unsafe { WriteFile(handle, buf.as_ptr(), len, std::ptr::null_mut(), ov) }) {
            Err(e) if ended(&e) => Err(io::ErrorKind::BrokenPipe.into()),
            other => other,
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A server instance of the pipe (the first one fails with `AccessDenied` when the pipe already exists).
fn instance(name: &[u16], first: bool, security: &OwnerOnly) -> io::Result<OwnedHandle> {
    let attributes = security.attributes();
    let mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    // SAFETY: a NUL-terminated name and live security attributes for the duration of the call.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER,
            BUFFER,
            0,
            &attributes,
        )
    };
    owned(handle)
}

/// Daemon: listen on the pipe `name` (see [`pipe_name`]). Returns at once if a daemon already listens there.
pub fn run(name: &Path) -> io::Result<()> {
    let name = wide(&name.to_string_lossy());
    let security = OwnerOnly::new(&current_user()?)?;
    let mut next = match instance(&name, true, &security) {
        Ok(handle) => handle,
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => return Ok(()),
        Err(e) => return Err(e),
    };
    let registry = Arc::new(Registry::default());
    start_reaper(registry.clone(), || {});
    loop {
        let pipe = Pipe::new(next);
        let handle = pipe.handle.as_raw_handle();
        // SAFETY: waits for a client on this instance with the overlapped structure `io` provides.
        match pipe.io(|ov| unsafe { ConnectNamedPipe(handle, ov) }) {
            Ok(_) => {}
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) => {}
            Err(_) => {
                next = instance(&name, false, &security)?;
                continue;
            }
        }
        next = instance(&name, false, &security)?;
        let registry = registry.clone();
        std::thread::spawn(move || {
            let _ = registry.serve_connection(pipe.clone(), pipe);
        });
    }
}

/// A connection to the daemon on `name`, refused when the process serving it is not this user's.
pub fn connect(name: &Path) -> io::Result<Pipe> {
    let wide_name = wide(&name.to_string_lossy());
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let handle = loop {
        // SAFETY: a NUL-terminated name; the new handle is owned below.
        let handle = unsafe {
            CreateFileW(wide_name.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED, std::ptr::null_mut())
        };
        match owned(handle) {
            Ok(handle) => break handle,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && std::time::Instant::now() < deadline => {
                // SAFETY: a NUL-terminated name; waits at most a second for a free instance.
                unsafe { WaitNamedPipeW(wide_name.as_ptr(), 1000) };
            }
            Err(e) => return Err(e),
        }
    };
    let mut server = 0u32;
    // SAFETY: a live pipe handle; `server` receives the serving process's id.
    if unsafe { GetNamedPipeServerProcessId(handle.as_raw_handle(), &mut server) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: opens the serving process for querying its token only; owned below.
    let process = owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, server) })?;
    if process_user(process.as_raw_handle())? != current_user()? {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the terminal daemon's pipe is served by another user"));
    }
    Ok(Pipe::new(handle))
}

/// A connection to the daemon on `name`, started first (this executable with `--daemon`, detached from this process,
/// its console and, where allowed, its job, so it outlives it) if none answers.
pub fn connect_or_start(name: &Path) -> io::Result<Pipe> {
    if let Ok(pipe) = connect(name) {
        return Ok(pipe);
    }
    let exe = std::env::current_exe()?;
    let spawn = |flags: u32| {
        Command::new(&exe).arg("--daemon").arg(name).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(flags).spawn()
    };
    let detached = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // A job that does not allow breaking away refuses the first form; the daemon then stays in that job.
    spawn(detached | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| spawn(detached))?;
    let mut last = None;
    for _ in 0..100 {
        match connect(name) {
            Ok(pipe) => return Ok(pipe),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Err(last.unwrap_or_else(|| io::Error::other("daemon did not start")))
}

#[cfg(test)]
#[path = "daemon_windows_tests.rs"]
mod tests;
