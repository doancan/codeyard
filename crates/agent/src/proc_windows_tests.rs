//! Windows process trees of tasks, with real processes: a task's leader starts a grandchild that holds the task's
//! output pipe; Stop ends the grandchild whether the leader still runs or already exited, and so does the death of the
//! process that owns the task (the job object is closed by the system, which kills what is in it). The fixtures are
//! this test binary run again (`--ignored`), so no other program is needed.

use std::io::{BufRead, BufReader};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};

use super::*;

/// Set for the leader fixture: exit right after starting the grandchild (which keeps the output pipe open).
const LEADER_EXITS: &str = "CODEYARD_TEST_LEADER_EXITS";

/// This test binary, running the ignored fixture `name`.
fn fixture(name: &str) -> proto::TaskCommand {
    proto::TaskCommand::Process {
        program: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
        args: vec!["--exact".into(), format!("proc::windows_tests::{name}"), "--ignored".into(), "--nocapture".into()],
    }
}

#[test]
#[ignore = "task process-tree fixture"]
fn fixture_grandchild() {
    std::thread::sleep(Duration::from_secs(60));
}

/// The task's leader: starts the grandchild (it inherits the task's output pipe), says its id, then waits or exits.
#[test]
#[ignore = "task process-tree fixture"]
#[allow(clippy::zombie_processes)] // The grandchild is meant to outlive this leader; the job owns it.
fn fixture_leader() {
    let grandchild = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "proc::windows_tests::fixture_grandchild", "--ignored", "--nocapture"])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    println!("grandchild:{}", grandchild.id());
    io::stdout().flush().unwrap();
    if std::env::var_os(LEADER_EXITS).is_none() {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// An owner of a task (as the app is): runs the leader as a task, passes its lines on, and waits to be killed.
#[test]
#[ignore = "task process-tree fixture"]
fn fixture_owner() {
    let (_task, out) = spawn_task(&fixture("fixture_leader"), None, &[]).unwrap();
    for line in BufReader::new(out).lines() {
        let Ok(line) = line else { break };
        println!("{line}");
        io::stdout().flush().unwrap();
        if line.starts_with("grandchild:") {
            break;
        }
    }
    std::thread::sleep(Duration::from_secs(60));
}

/// The grandchild's id from a fixture's output.
fn grandchild_of(out: impl Read) -> u32 {
    BufReader::new(out)
        .lines()
        .find_map(|line| line.ok()?.split_once("grandchild:").map(|(_, id)| id.trim().parse::<u32>().unwrap()))
        .expect("the leader names its grandchild")
}

/// A handle to a process this test started (held before anything is stopped, so its identity cannot be reused).
fn open(pid: u32) -> OwnedHandle {
    // SAFETY: the id is of a fixture this test started; the access asked for is only to wait for it and end it.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
    assert!(!raw.is_null(), "{}", io::Error::last_os_error());
    // SAFETY: a fresh successful handle; ownership moves here once.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

/// Whether the process ends within `limit`; a survivor is ended here so a failing test leaves nothing behind.
fn ends_within(process: &OwnedHandle, limit: Duration) -> bool {
    // SAFETY: the handle is owned and live while waiting.
    let result = unsafe { WaitForSingleObject(process.as_raw_handle(), limit.as_millis() as u32) };
    if result == WAIT_TIMEOUT {
        // SAFETY: the same owned fixture handle, never a looked-up pid.
        unsafe {
            TerminateProcess(process.as_raw_handle(), 99);
            WaitForSingleObject(process.as_raw_handle(), 2000);
        }
    }
    result == WAIT_OBJECT_0
}

/// Stop (`ProcKill`, here dropping the task) ends the grandchild: with the leader running, and after the leader exited
/// while the grandchild still holds the task's output pipe.
#[test]
fn windows_stopping_a_task_ends_its_whole_tree_with_a_live_or_exited_leader() {
    for leader_exits in [false, true] {
        let env = if leader_exits { vec![(LEADER_EXITS.to_string(), Some("1".to_string()))] } else { Vec::new() };
        let (mut task, out) = spawn_task(&fixture("fixture_leader"), None, &env).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(grandchild_of(out));
        });
        let grandchild = open(rx.recv_timeout(Duration::from_secs(30)).expect("the grandchild's id"));
        if leader_exits {
            let deadline = Instant::now() + Duration::from_secs(10);
            while task.try_wait().unwrap().is_none() {
                assert!(Instant::now() < deadline, "the leader did not exit");
                std::thread::sleep(Duration::from_millis(20));
            }
            // SAFETY: the grandchild handle is owned; a zero wait only polls.
            assert_eq!(unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 0) }, WAIT_TIMEOUT, "the grandchild outlives its leader");
        }
        drop(task);
        assert!(ends_within(&grandchild, Duration::from_secs(5)), "the grandchild outlived Stop (leader exited first: {leader_exits})");
    }
}

/// The owner of a task is killed (`TerminateProcess`, as a crash or Task Manager does): nothing runs its destructors,
/// but the system closes its job handle, and kill-on-close ends the task's leader and grandchild.
#[test]
fn windows_killing_the_owner_ends_its_task_trees() {
    let exe = std::env::current_exe().unwrap();
    let mut owner = Command::new(exe)
        .args(["--exact", "proc::windows_tests::fixture_owner", "--ignored", "--nocapture"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let out = owner.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(grandchild_of(out));
    });
    let grandchild = open(rx.recv_timeout(Duration::from_secs(30)).expect("the grandchild's id"));
    owner.kill().unwrap();
    owner.wait().unwrap();
    assert!(ends_within(&grandchild, Duration::from_secs(5)), "the grandchild outlived its killed owner");
}
