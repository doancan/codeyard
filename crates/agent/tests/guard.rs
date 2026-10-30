//! The process guard with real processes: an owner (this test binary, run again as a helper) starts a guard, a task whose
//! shell leaves a grandchild behind and a terminal whose shell ignores the hangup; then the owner is killed with SIGKILL.
//! Both descendants must be gone within a bounded time, and a task the owner had already reaped must not be touched.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const OWNER_DIR: &str = "CODEYARD_GUARD_TEST_OWNER";

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn read_pid(path: &Path, deadline: Instant) -> i32 {
    loop {
        if let Some(pid) = std::fs::read_to_string(path).ok().and_then(|s| s.trim().parse().ok()) {
            return pid;
        }
        assert!(Instant::now() < deadline, "{} was not written", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_gone(pid: i32, within: Duration, what: &str) {
    let deadline = Instant::now() + within;
    while alive(pid) {
        assert!(Instant::now() < deadline, "{what} (pid {pid}) is still running {within:?} after its owner was killed");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The owner: runs only when the test below starts this binary again with `OWNER_DIR` set.
#[test]
#[allow(clippy::zombie_processes)] // The guard is meant to outlive this owner, which is killed and never returns.
fn guard_owner_helper() {
    let Some(dir) = std::env::var_os(OWNER_DIR).map(PathBuf::from) else { return };
    let _guard = agent::guard::spawn(Path::new(env!("CARGO_BIN_EXE_codeyard-agent"))).unwrap();
    let sh = |line: String| proto::TaskCommand::Shell { shell: Some("/bin/sh".into()), args: Some(vec!["-c".into()]), line };
    // A task whose leader starts a grandchild in its group and waits for it.
    let task_line = format!("sleep 300 & echo $! > {}; wait", dir.join("task-grandchild").display());
    let (_task, _out) = agent::proc::spawn_task(&sh(task_line), None, &[]).unwrap();
    // A task that ends by itself and is reaped: the guard forgets it.
    let (mut done, _done_out) = agent::proc::spawn_task(&sh(format!("echo $$ > {}", dir.join("done").display())), None, &[]).unwrap();
    assert_eq!(done.wait(), Some(0));
    // A terminal whose shell and its child ignore the hangup: only the guard's SIGKILL after the grace period ends them.
    let pty_line = format!("trap '' HUP; sleep 300 & echo $! > {}; wait", dir.join("pty-child").display());
    let spawn = agent::pty::Spawn {
        cols: 80,
        rows: 24,
        cwd: None,
        shell: Some("/bin/sh".into()),
        args: Some(vec!["-c".into(), pty_line]),
        env: Vec::new(),
        integration: false,
    };
    let (_pty, _pty_out) = agent::pty::spawn_with(spawn).unwrap();
    std::fs::write(dir.join("ready"), "").unwrap();
    std::thread::sleep(Duration::from_secs(300));
}

#[test]
fn a_killed_owner_leaves_no_task_or_terminal_behind() {
    if std::env::var_os(OWNER_DIR).is_some() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("codeyard-guard-kill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut owner = Command::new(std::env::current_exe().unwrap())
        .args(["guard_owner_helper", "--exact", "--nocapture", "--test-threads=1"])
        .env(OWNER_DIR, &dir)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !dir.join("ready").exists() {
        if let Some(status) = owner.try_wait().unwrap() {
            panic!("the owner ended early: {status}");
        }
        assert!(Instant::now() < deadline, "the owner did not get ready");
        std::thread::sleep(Duration::from_millis(20));
    }
    let task_child = read_pid(&dir.join("task-grandchild"), deadline);
    let pty_child = read_pid(&dir.join("pty-child"), deadline);
    assert!(alive(task_child) && alive(pty_child), "the descendants run while their owner lives");

    owner.kill().unwrap(); // SIGKILL: no destructor, no exit handler runs in the owner.
    owner.wait().unwrap();
    wait_gone(task_child, Duration::from_secs(5), "the task's grandchild");
    wait_gone(pty_child, agent::guard::PTY_GRACE + Duration::from_secs(5), "the terminal's child");
    let _ = std::fs::remove_dir_all(&dir);
}
