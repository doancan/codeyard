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
