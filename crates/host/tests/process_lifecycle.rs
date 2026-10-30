#![cfg(unix)]

use host::{Host, ProcEvent};
use std::time::{Duration, Instant};

#[test]
fn cancellation_remains_available_after_the_process_closes_stdout() {
    let host = host::local();
    let process = host.proc_spawn("/bin/sh", vec!["-c".into(), "printf 'ready\\n'; exec 1>&-; exec /bin/sleep 5".into()], None).unwrap();
    let mut output = Vec::new();
    while !output.ends_with(b"ready\n") {
        match process.events.recv_timeout(Duration::from_secs(2)).unwrap() {
            ProcEvent::Output(bytes) => output.extend(bytes),
            other => panic!("unexpected event: {other:?}"),
        }
    }
    // Give the output reader time to observe EOF; the child deliberately keeps running.
    std::thread::sleep(Duration::from_millis(100));
    let start = Instant::now();
    host.done(proto::Request::ProcKill { proc: process.id }).unwrap();
    assert!(start.elapsed() < Duration::from_secs(2), "cancellation waited for natural exit: {:?}", start.elapsed());
}

/// A task through the local host: typed argv, environment, merged output and exit code; a kill ends with `Exit(None)`.
#[test]
fn tasks_run_through_the_host_and_stop_on_request() {
    let host = host::local();
    let command = proto::TaskCommand::Process {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "echo \"$1 $X\"; echo e >&2; exit 4".into(), "sh".into(), "a b".into()],
    };
    let task = host.task_spawn(command, None, vec![("X".into(), Some("y".into()))]).unwrap();
    let mut output = Vec::new();
    let code = loop {
        match task.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            ProcEvent::Output(bytes) => output.extend(bytes),
            ProcEvent::Exit(code) => break code,
        }
    };
    assert_eq!((String::from_utf8_lossy(&output).as_ref(), code), ("a b y\ne\n", Some(4)));

    let command = proto::TaskCommand::Shell { shell: Some("/bin/sh".into()), args: Some(vec!["-c".into()]), line: "echo up; exec sleep 30".into() };
    let task = host.task_spawn(command, None, Vec::new()).unwrap();
    assert!(matches!(task.events.recv_timeout(Duration::from_secs(5)).unwrap(), ProcEvent::Output(_)));
    host.proc_kill(task.id);
    let end = loop {
        if let ProcEvent::Exit(code) = task.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            break code;
        }
    };
    assert_eq!(end, None);
}
