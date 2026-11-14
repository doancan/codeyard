//! The open-file limit ssh is started with.

use std::process::Command;

/// Open files the shared ssh connection may hold. A GUI app on macOS starts with a soft limit of 256, and ssh keeps two
/// listeners for every `localhost` forward plus a descriptor for each connection through it, so a few dozen forwards
/// (dev servers, test runners and language servers open ports all the time) made every later forward fail with
/// "Port forwarding failed". Only ssh gets the higher limit: the app and the programs its terminals start keep theirs
/// (a program that still uses `select()` breaks above 1024 descriptors).
#[cfg(unix)]
const SSH_FD_LIMIT: u64 = 8192;

/// The soft limit ssh is started with: [`SSH_FD_LIMIT`] capped by the hard limit; `None` when the current one is enough.
#[cfg(unix)]
fn ssh_fd_limit() -> Option<libc::rlim_t> {
    static LIMIT: std::sync::OnceLock<Option<libc::rlim_t>> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        let mut now = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: `now` is a valid out-pointer for the duration of the call.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut now) } != 0 {
            return None;
        }
        let want = now.rlim_max.min(SSH_FD_LIMIT as libc::rlim_t);
        (want > now.rlim_cur).then_some(want)
    })
}

/// Starts `cmd` with the higher descriptor limit (the child only; this process keeps its own).
#[cfg(unix)]
pub(crate) fn lift_fd_limit(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    let Some(want) = ssh_fd_limit() else { return };
    // SAFETY: the closure only calls `getrlimit`/`setrlimit`, which are async-signal-safe, and allocates nothing.
    unsafe {
        cmd.pre_exec(move || {
            let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 {
                lim.rlim_cur = want.min(lim.rlim_max);
                // A refused raise (a stricter system cap) is not worth failing the connection for.
                let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn ssh_starts_with_a_higher_descriptor_limit_than_the_app_keeps() {
        let soft = |lift: bool| {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "ulimit -n"]);
            if lift {
                lift_fd_limit(&mut cmd);
            }
            String::from_utf8(cmd.output().unwrap().stdout).unwrap().trim().parse::<u64>().unwrap()
        };
        let (plain, lifted) = (soft(false), soft(true));
        assert!(lifted >= plain, "{lifted} < {plain}");
        if ssh_fd_limit().is_some() {
            assert!(lifted > plain && lifted <= SSH_FD_LIMIT, "{plain} -> {lifted}");
        }
    }
}
