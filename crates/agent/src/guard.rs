//! Process guard: the processes a host starts outlive nothing that started them. On Unix a task leads its own process
//! group and a terminal its own session, so when the owner dies by a signal (SIGKILL, a crash) nothing would stop them.
//! The owner therefore starts a small guard process (`--process-guard`, the same binary) and tells it, over the guard's
//! standard input, which groups it owns. The kernel closes that pipe however the owner ends, so end of input means "the
//! owner is gone": the guard kills every task group at once, hangs up every terminal group and kills what is left of
//! them after a short grace period, then exits.
//!
//! Lines on the pipe: `t <pgid>` (a task, process or language server), `p <pgid>` (a terminal's shell), `- <pgid>`
//! (reaped or stopped by the owner: forget it). The owner forgets a group before it reaps the leader, so the group id
//! cannot have been reused when the guard acts on it. Groups are only ever process group ids of the owner's own
//! unreaped children; the guard never acts on a single pid.
//!
//! Windows needs no guard: tasks run in a job object that kills its processes when its last handle closes, which the
//! system does when the owner dies; a terminal's pseudo console closes the same way and ends its console processes.

#[cfg(unix)]
pub use imp::*;

#[cfg(unix)]
mod imp {
    use std::collections::HashMap;
    use std::io::{self, BufRead, Write};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use crate::Locked;

    /// How long a terminal's shell gets after the hangup to save its history and end its jobs.
    pub const PTY_GRACE: Duration = Duration::from_secs(2);

    /// The write end of the guard's pipe, while there is a guard.
    static PIPE: Mutex<Option<ChildStdin>> = Mutex::new(None);

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Kind {
        Task,
        Pty,
    }

    /// Start `program --process-guard` (an executable that calls [`run`] for that argument) and send it every group this
    /// process starts from now on. The guard leads its own process group, so a signal to the owner's group (Ctrl+C in
    /// the terminal that started it) does not take it down too. Keep the returned child: dropping it does not stop the
    /// guard, but waiting on it would block until the owner exits.
    pub fn spawn(program: &std::path::Path) -> io::Result<Child> {
        let mut cmd = Command::new(program);
        cmd.arg("--process-guard").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd.spawn()?;
        // The pipe is created close-on-exec, so no task inherits it: only this process holds the write end.
        *PIPE.locked() = child.stdin.take();
        Ok(child)
    }

    fn send(line: &str) {
        let mut pipe = PIPE.locked();
        let Some(w) = pipe.as_mut() else { return };
        if w.write_all(line.as_bytes()).and_then(|_| w.flush()).is_err() {
            // The guard is gone (killed on its own): nothing more to tell it.
            *pipe = None;
        }
    }

    /// A task's process group (its leader's pid) was started.
    pub(crate) fn task_started(pgid: u32) {
        send(&format!("t {pgid}\n"));
    }

    /// A terminal's session (its shell's pid, which is also its process group) was started.
    pub(crate) fn pty_started(pgid: u32) {
        send(&format!("p {pgid}\n"));
    }

    /// The owner is about to reap the group's leader, or has stopped the group itself.
    pub(crate) fn forget(pgid: u32) {
        send(&format!("- {pgid}\n"));
    }

    /// The guard's loop: read the owner's lines until end of input, then end what is still registered.
    pub fn run(input: impl io::Read) {
        // A hangup or interrupt meant for the owner's terminal is not for the guard.
        // SAFETY: setting a signal to SIG_IGN has no preconditions.
        unsafe {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            libc::signal(libc::SIGINT, libc::SIG_IGN);
        }
        let groups = read_groups(io::BufReader::new(input));
        end(&groups, PTY_GRACE);
    }

    fn read_groups(input: impl BufRead) -> HashMap<i32, Kind> {
        let mut groups = HashMap::new();
        for line in input.split(b'\n') {
            let Ok(line) = line else { break };
            let Some((op, id)) = std::str::from_utf8(&line).ok().and_then(|l| l.split_once(' ')) else { continue };
            // Only positive ids above 1: `kill(-1)` and `kill(0)` would reach far more than one group.
            let Some(id) = id.trim().parse::<i32>().ok().filter(|id| *id > 1) else { continue };
            match op {
                "t" => {
                    groups.insert(id, Kind::Task);
                }
                "p" => {
                    groups.insert(id, Kind::Pty);
                }
                "-" => {
                    groups.remove(&id);
                }
                _ => {}
            }
        }
        groups
    }

    fn alive(pgid: i32) -> bool {
        // SAFETY: signal 0 only checks that the group exists and may be signalled.
        unsafe { libc::kill(-pgid, 0) == 0 }
    }

    fn signal(pgid: i32, sig: libc::c_int) {
        // SAFETY: kill has no memory-safety preconditions; a negative pid names the group.
        unsafe { libc::kill(-pgid, sig) };
    }

    /// Tasks: SIGKILL now. Terminals: SIGHUP (and SIGCONT, so a stopped job sees it), then SIGKILL after `grace` for
    /// whatever is still there.
    fn end(groups: &HashMap<i32, Kind>, grace: Duration) {
        let mut ptys = Vec::new();
        for (&pgid, &kind) in groups {
            match kind {
                Kind::Task => signal(pgid, libc::SIGKILL),
                Kind::Pty => {
                    signal(pgid, libc::SIGHUP);
                    signal(pgid, libc::SIGCONT);
                    ptys.push(pgid);
                }
            }
        }
        let deadline = Instant::now() + grace;
        while !ptys.is_empty() && Instant::now() < deadline {
            ptys.retain(|g| alive(*g));
            std::thread::sleep(Duration::from_millis(50));
        }
        for pgid in ptys {
            signal(pgid, libc::SIGKILL);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn lines_register_and_forget_groups_and_ignore_anything_else() {
            let input = b"t 100\np 200\nt 300\n- 100\nx 400\nt -5\nt 0\nt 1\nt nope\np\n";
            let groups = read_groups(&input[..]);
            assert_eq!(groups, HashMap::from([(200, Kind::Pty), (300, Kind::Task)]));
        }
    }
}
