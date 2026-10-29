//! Windows OpenSSH lacks ControlMaster, so each local forward owns an ssh process.
use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use super::{FailKind, ForwardError, classify};

type Key = (String, String);
enum Forward {
    Starting(Arc<AtomicBool>),
    Active(Child),
}
static FORWARDS: Mutex<Option<HashMap<Key, Forward>>> = Mutex::new(None);
const START_TIMEOUT: Duration = Duration::from_secs(60);

fn error(message: impl Into<String>) -> ForwardError {
    ForwardError { kind: FailKind::Other, message: message.into() }
}

fn command(alias: &str, spec: &str) -> Command {
    let mut cmd = super::ssh_base(alias);
    if !crate::askpass::configure(&mut cmd) {
        cmd.args(["-o", "BatchMode=yes"]);
    }
    cmd.args([
        "-N",
        "-T",
        "-v",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=2",
        "-L",
        spec,
        "--",
        alias,
    ]);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    cmd
}

/// Wait for ssh's post-authentication startup message, not merely for spawn().
/// stderr is continuously drained even after the caller stops receiving.
fn start(mut cmd: Command, local: u16, timeout: Duration, cancelled: &AtomicBool) -> Result<Child, ForwardError> {
    let mut child = cmd.spawn().map_err(|e| error(format!("could not start ssh: {e}")))?;
    let Some(mut stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error("could not read ssh startup output"));
    };
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(32);
    std::thread::spawn(move || {
        let mut buf = [0; 1024];
        while let Ok(n) = stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            let _ = tx.send(buf[..n].to_vec());
        }
    });
    let start = Instant::now();
    let mut log = Vec::new();
    loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error("SSH forwarding was cancelled"));
        }
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(chunk) => {
                log.extend(chunk);
                // Bound error output while keeping the newest startup diagnostics.
                if log.len() > 16384 {
                    log.drain(..log.len() - 8192);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {}
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                while let Ok(chunk) = rx.recv_timeout(Duration::from_millis(50)) {
                    log.extend(chunk);
                    if log.len() > 16384 {
                        log.drain(..log.len() - 8192);
                    }
                }
                return Err(classify(&String::from_utf8_lossy(&log), status.code(), local));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error(format!("could not check ssh: {e}")));
            }
            Ok(None) => {}
        }
        if log.windows(b"debug1: Entering interactive session.".len()).any(|b| b == b"debug1: Entering interactive session.") {
            return Ok(child);
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error("SSH forwarding timed out while connecting"));
        }
    }
}

pub(super) fn open(alias: &str, spec: &str, local: u16) -> Result<(), ForwardError> {
    let key = (alias.to_owned(), spec.to_owned());
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut registry = FORWARDS.lock().unwrap_or_else(|e| e.into_inner());
        let map = registry.get_or_insert_with(HashMap::new);
        if let Some(forward) = map.get_mut(&key) {
            match forward {
                Forward::Starting(_) => return Err(error("SSH forward is already opening")),
                Forward::Active(child) => {
                    if matches!(child.try_wait(), Ok(None)) {
                        return Err(error("SSH forward is already open"));
                    }
                    map.remove(&key);
                }
            }
        }
        map.insert(key.clone(), Forward::Starting(cancelled.clone()));
    }
    let result = start(command(alias, spec), local, START_TIMEOUT, &cancelled);
    let mut registry = FORWARDS.lock().unwrap_or_else(|e| e.into_inner());
    let map = registry.get_or_insert_with(HashMap::new);
    let ours = matches!(map.get(&key), Some(Forward::Starting(token)) if Arc::ptr_eq(token, &cancelled));
    match result {
        Ok(child) if ours && !cancelled.load(Ordering::Acquire) => {
            map.insert(key, Forward::Active(child));
            Ok(())
        }
        Ok(child) => {
            stop(child);
            Err(error("SSH forwarding was cancelled"))
        }
        Err(e) => {
            if ours {
                map.remove(&key);
            }
            Err(e)
        }
    }
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

pub(super) fn cancel(alias: &str, spec: &str) -> Result<(), ForwardError> {
    let child = FORWARDS.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(&(alias.to_owned(), spec.to_owned())));
    if let Some(forward) = child {
        match forward {
            Forward::Starting(token) => token.store(true, Ordering::Release),
            Forward::Active(mut child) => {
                let _ = child.kill();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
        }
    }
    Ok(())
}

pub(super) fn alive(alias: &str, spec: &str) -> bool {
    let mut registry = FORWARDS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(map) = registry.as_mut() else { return false };
    let key = (alias.to_owned(), spec.to_owned());
    let alive = map.get_mut(&key).is_some_and(|forward| match forward {
        Forward::Starting(_) => true,
        Forward::Active(child) => matches!(child.try_wait(), Ok(None)),
    });
    if !alive {
        map.remove(&key);
    }
    alive
}

pub(super) fn close_alias(alias: &str) {
    let children: Vec<Forward> = {
        let mut registry = FORWARDS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(map) = registry.as_mut() else { return };
        let keys: Vec<_> = map.keys().filter(|(a, _)| a == alias).cloned().collect();
        keys.iter().filter_map(|key| map.remove(key)).collect()
    };
    for forward in children {
        stop_forward(forward);
    }
}

pub(super) fn close_all() {
    let children = FORWARDS.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
    for (_, forward) in children {
        stop_forward(forward);
    }
}

fn stop_forward(forward: Forward) {
    match forward {
        Forward::Starting(token) => token.store(true, Ordering::Release),
        Forward::Active(child) => stop(child),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(code: &str) -> Command {
        let mut cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", code]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        cmd
    }

    #[test]
    fn startup_waits_for_readiness_and_stops_the_child() {
        let mut child = start(
            shell("[Console]::Error.WriteLine('debug1: Entering interactive session.'); Start-Sleep -Seconds 30"),
            3000,
            Duration::from_secs(10),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(child.try_wait().unwrap().is_none());
        stop(child);
    }

    #[test]
    fn startup_reports_exit_and_times_out() {
        let fail =
            start(shell("[Console]::Error.WriteLine('Address already in use'); exit 1"), 3000, Duration::from_secs(10), &AtomicBool::new(false)).unwrap_err();
        assert_eq!(fail.kind, FailKind::AddrInUse);
        let timeout = start(shell("Start-Sleep -Seconds 30"), 3000, Duration::from_millis(200), &AtomicBool::new(false)).unwrap_err();
        assert!(timeout.message.contains("timed out"));
    }

    #[test]
    fn ssh_command_uses_no_multiplexing_and_requests_failure_on_bind_errors() {
        let cmd = command("myhost", "localhost:1234:localhost:4321");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.iter().any(|a| a == "ExitOnForwardFailure=yes"));
        assert!(args.ends_with(&["-L", "localhost:1234:localhost:4321", "--", "myhost"].map(str::to_string)));
    }

    #[test]
    fn cancellation_during_start_cannot_leave_a_process_running() {
        let token = Arc::new(AtomicBool::new(false));
        let key = ("test-cancel-pending".to_owned(), "local:1:remote:2".to_owned());
        FORWARDS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key.clone(), Forward::Starting(token.clone()));
        let thread = std::thread::spawn(move || start(shell("Start-Sleep -Seconds 30"), 3000, Duration::from_secs(10), &token));
        cancel(&key.0, &key.1).unwrap();
        assert!(thread.join().unwrap().unwrap_err().message.contains("cancelled"));
        assert!(!alive(&key.0, &key.1));
    }

    #[test]
    fn cancellation_removes_active_process() {
        let child = start(
            shell("[Console]::Error.WriteLine('debug1: Entering interactive session.'); Start-Sleep -Seconds 30"),
            3000,
            Duration::from_secs(10),
            &AtomicBool::new(false),
        )
        .unwrap();
        let key = ("test-cancel-active".to_owned(), "local:3:remote:4".to_owned());
        FORWARDS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key.clone(), Forward::Active(child));
        assert!(alive(&key.0, &key.1));
        cancel(&key.0, &key.1).unwrap();
        assert!(!alive(&key.0, &key.1));
    }
}
