//! The Windows terminal daemon with real processes: a daemon (on a thread of this test) serves a named pipe; a ConPTY
//! PowerShell is opened in a session, the client goes away as a quitting app does, and a new connection to the session
//! takes the same shell back with its token. A wrong token is refused, a session kept for 1 s ends with its shell, and
//! the pipe allows this user only.
//!
//! Every wait is bounded: reads on the test's connection time out (`READ_LIMIT`) and fail the test with what was seen.
//! ConPTY asks the terminal for its cursor position (`ESC [6n`) before it shows anything; the tests answer it, as the
//! app's terminal does.

use std::io::BufReader;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Instant;

use windows_sys::Win32::Foundation::{LocalFree, WAIT_OBJECT_0};
use windows_sys::Win32::Security::Authorization::{ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT};
use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

use super::*;

/// No single read on a test connection waits longer: the daemon or the shell is stuck then.
const READ_LIMIT: Duration = Duration::from_secs(30);

/// A daemon of its own on a pipe named after the test.
fn daemon(name: &str) -> std::path::PathBuf {
    let pipe = std::path::PathBuf::from(format!(r"\\.\pipe\codeyard-test-{}-{name}", std::process::id()));
    let p = pipe.clone();
    std::thread::spawn(move || run(&p));
    pipe
}

struct Conn {
    w: Pipe,
    r: BufReader<Pipe>,
    next: u32,
    seen: String,
}

impl Conn {
    fn open(pipe: &Path, session: &str) -> (Conn, bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let p = loop {
            match connect(pipe) {
                Ok(p) => break p,
                Err(e) => assert!(Instant::now() < deadline, "no daemon: {e}"),
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let mut reader = p.clone();
        reader.set_read_timeout(Some(READ_LIMIT));
        let mut c = Conn { r: BufReader::new(reader), w: p, next: 10, seen: String::new() };
        let hello = Request::Hello { client_version: proto::VERSION, session: Some(session.into()) };
        proto::write_frame(&mut c.w, &Frame { id: 1, body: hello }).unwrap();
        let resumed = match c.frame().body {
            Response::Hello(info) => info.resumed,
            other => panic!("{other:?}"),
        };
        (c, resumed)
    }

    /// The next frame from the daemon, within `READ_LIMIT`. Output is kept in `seen`, and ConPTY's cursor position
    /// request is answered.
    fn frame(&mut self) -> Frame<Response> {
        let frame = match proto::read_frame::<Response>(&mut self.r) {
            Ok(Some(frame)) => frame,
            Ok(None) => panic!("the daemon closed the connection; got {:?}", self.seen),
            Err(e) => panic!("no frame from the daemon: {e}; got {:?}", self.seen),
        };
        if let Response::PtyOutput { pty, data } = &frame.body {
            if data.windows(4).any(|s| s == b"\x1b[6n") {
                self.input(*pty, "\x1b[1;1R");
            }
            self.seen.push_str(&String::from_utf8_lossy(data));
        }
        frame
    }

    fn call(&mut self, body: Request) -> Response {
        self.next += 1;
        let id = self.next;
        proto::write_frame(&mut self.w, &Frame { id, body }).unwrap();
        let deadline = Instant::now() + READ_LIMIT;
        loop {
            assert!(Instant::now() < deadline, "no answer to request {id}; got {:?}", self.seen);
            let frame = self.frame();
            match frame.body {
                Response::PtyOutput { .. } => {}
                body if frame.id == id => return body,
                _ => {}
            }
        }
    }

    fn input(&mut self, pty: u32, text: &str) {
        proto::write_frame(&mut self.w, &Frame { id: 0, body: Request::PtyInput { pty, data: text.as_bytes().to_vec() } }).unwrap();
    }

    /// The shell's own pid, printed by it as `tag=<digits>`.
    fn shell_pid(&mut self, pty: u32, tag: &str) -> u32 {
        self.seen.clear();
        self.input(pty, &format!("Write-Output (\"{tag}=\" + $PID)\r"));
        let marker = format!("{tag}=");
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let found = self.seen.match_indices(&marker).find_map(|(at, _)| {
                let digits: String = self.seen[at + marker.len()..].chars().take_while(char::is_ascii_digit).collect();
                digits.parse().ok()
            });
            if let Some(pid) = found {
                return pid;
            }
            assert!(Instant::now() < deadline, "no pid in {:?}", self.seen);
            self.frame();
        }
    }

    /// Go away as a quitting app does: no `Bye`, the pipe just closes.
    fn drop_connection(self) {
        self.w.shutdown();
    }
}

fn spawn_shell(c: &mut Conn) -> u32 {
    let spawn = Request::PtySpawnEx {
        cols: 120,
        rows: 30,
        cwd: None,
        shell: Some("powershell.exe".into()),
        args: Some(vec!["-NoLogo".into(), "-NoProfile".into()]),
        env: Vec::new(),
        integration: false,
    };
    match c.call(spawn) {
        Response::PtySpawned { pty } => pty,
        other => panic!("{other:?}"),
    }
}

fn token(c: &mut Conn, pty: u32) -> (String, String) {
    match c.call(Request::PtyToken { pty }) {
        Response::PtyToken { instance, secret } => (instance, secret),
        other => panic!("{other:?}"),
    }
}

fn ends_within(pid: u32, limit: Duration) -> bool {
    // SAFETY: waits on a process this test started through the daemon; the handle is owned below.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        return true; // already gone
    }
    // SAFETY: a fresh handle, owned once.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: the owned handle is live while waiting.
    unsafe { WaitForSingleObject(process.as_raw_handle(), limit.as_millis() as u32) == WAIT_OBJECT_0 }
}

#[test]
fn windows_a_restarted_client_takes_its_conpty_shell_back_with_its_token_only() {
    let pipe = daemon("reattach");
    let (mut c1, resumed) = Conn::open(&pipe, "x-windows-window");
    assert!(!resumed);
    let pty = spawn_shell(&mut c1);
    let before = c1.shell_pid(pty, "before");
    let (instance, secret) = token(&mut c1, pty);
    c1.drop_connection();
    std::thread::sleep(Duration::from_millis(500));

    let (mut c2, resumed) = Conn::open(&pipe, "x-windows-window");
    assert!(resumed, "the session outlived its client");
    let wrong = c2.call(Request::PtyAttach { pty, instance: instance.clone(), secret: "0".repeat(32) });
    assert!(matches!(wrong, Response::Error(proto::Error::PermissionDenied(_))), "{wrong:?}");
    match c2.call(Request::PtyAttach { pty, instance, secret }) {
        Response::PtyAttached { pty: p, sequence, .. } => assert!(p == pty && sequence > 0),
        other => panic!("{other:?}"),
    }
    assert_eq!(c2.shell_pid(pty, "after"), before, "the same shell process");
    // Leaving on purpose ends the session and its shell.
    proto::write_frame(&mut c2.w, &Frame { id: 0, body: Request::Bye }).unwrap();
    assert!(ends_within(before, Duration::from_secs(10)), "the shell outlived a deliberate close");
}

/// A session its client asked to keep for 1 s ends, with its shell, once that time has passed after the client went.
#[test]
fn windows_a_kept_session_ends_with_its_shell_after_its_timeout() {
    let pipe = daemon("timeout");
    let (mut c, _) = Conn::open(&pipe, "x-windows-timeout");
    let pty = spawn_shell(&mut c);
    let pid = c.shell_pid(pty, "short");
    proto::write_frame(&mut c.w, &Frame { id: 0, body: Request::KeepFor { secs: 1 } }).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    c.drop_connection();
    // The daemon looks every 10 s at most.
    assert!(ends_within(pid, Duration::from_secs(20)), "the shell of an expired session still runs");
}

/// The pipe is created with a protected DACL that grants access to this user's SID alone.
#[test]
fn windows_the_daemon_pipe_is_for_this_user_only() {
    let pipe = daemon("acl");
    let (c, _) = Conn::open(&pipe, "x-windows-acl");
    let mut sd = std::ptr::null_mut();
    // SAFETY: queries the DACL of our live pipe handle; `sd` receives a LocalAlloc'd descriptor, freed below.
    let status = unsafe {
        GetSecurityInfo(
            c.w.raw(),
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    assert_eq!(status, 0, "GetSecurityInfo");
    let mut text = std::ptr::null_mut();
    // SAFETY: `sd` is the descriptor just returned; `text` receives a LocalAlloc'd string, freed below.
    let ok = unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, std::ptr::null_mut()) };
    assert_ne!(ok, 0, "{}", io::Error::last_os_error());
    // SAFETY: a NUL-terminated wide string from the call above.
    let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    // SAFETY: `len` characters counted in the same string.
    let sddl = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    // SAFETY: both were allocated by the calls above, freed once.
    unsafe {
        LocalFree(text.cast());
        LocalFree(sd);
    }
    let user = current_user().unwrap();
    assert!(sddl.starts_with("D:P"), "protected, nothing inherited: {sddl}");
    assert_eq!(sddl.matches("(A;").count(), 1, "one access entry: {sddl}");
    assert!(sddl.contains(&user), "for this user: {sddl}");
    for others in [";WD)", ";AU)", ";BU)", ";AN)", ";SY)", ";BA)"] {
        assert!(!sddl.contains(others), "{others} has access: {sddl}");
    }
    c.drop_connection();
}
