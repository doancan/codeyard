//! Bounded JSON-RPC exchanges with a short-lived local agent subprocess.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::Value;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn exchange<T>(mut cmd: Command, request: impl FnOnce(&mut ChildStdin, &Receiver<Value>) -> io::Result<T>) -> io::Result<T> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    std::thread::scope(|scope| {
        let result = match (child.stdin.take(), child.stdout.take()) {
            (Some(mut input), Some(output)) => {
                let (tx, rx) = mpsc::channel();
                scope.spawn(move || {
                    for line in BufReader::new(output.take(8 * 1024 * 1024)).lines().map_while(Result::ok) {
                        if let Ok(message) = serde_json::from_str::<Value>(&line)
                            && message.get("id").is_some()
                            && tx.send(message).is_err()
                        {
                            break;
                        }
                    }
                });
                request(&mut input, &rx)
            }
            _ => Err(io::Error::other("The agent did not open its local protocol streams")),
        };
        let _ = child.kill();
        let _ = child.wait();
        result
    })
}

pub(super) fn send(input: &mut ChildStdin, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *input, value)?;
    input.write_all(b"\n")?;
    input.flush()
}

pub(super) fn receive(responses: &Receiver<Value>, id: u32) -> io::Result<Value> {
    let end = Instant::now() + RESPONSE_TIMEOUT;
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        let response = responses.recv_timeout(left).map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => io::Error::new(io::ErrorKind::TimedOut, "The local agent did not respond"),
            mpsc::RecvTimeoutError::Disconnected => io::Error::other("The local agent closed the connection"),
        })?;
        if response["id"] == id {
            return Ok(response);
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "The local agent did not respond"))
}

pub(super) fn result(response: &Value) -> io::Result<&Value> {
    response.get("result").ok_or_else(|| io::Error::other(response["error"]["message"].as_str().unwrap_or("The local agent rejected the request")))
}
