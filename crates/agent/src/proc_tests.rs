use super::*;

#[cfg(windows)]
#[test]
#[ignore = "owned process-tree subprocess fixture"]
fn windows_fixture_tree_descendant() {
    std::thread::sleep(Duration::from_secs(30));
}

#[cfg(windows)]
#[test]
#[ignore = "owned process-tree subprocess fixture"]
#[allow(clippy::zombie_processes)] // The outer test holds a process handle and owns tree termination.
fn windows_fixture_tree_parent() {
    let descendant = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "proc::tests::windows_fixture_tree_descendant", "--ignored", "--nocapture"])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    println!("owned-descendant:{}", descendant.id());
    io::stdout().flush().unwrap();
    let mut command = String::new();
    io::stdin().read_line(&mut command).unwrap();
    // Returning deliberately leaves the descendant alive; it still belongs to the outer job.
}

#[cfg(windows)]
#[test]
fn stopping_a_windows_process_kills_its_descendants_with_a_live_or_exited_leader() {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};

    for exit_early in [false, true] {
        let (mut process, reader) = spawn(
            &std::env::current_exe().unwrap().to_string_lossy(),
            &["--exact".into(), "proc::tests::windows_fixture_tree_parent".into(), "--ignored".into(), "--nocapture".into()],
            None,
        )
        .unwrap();
        let mut lines = std::io::BufRead::lines(std::io::BufReader::new(reader));
        let descendant = lines.find_map(|line| line.unwrap().split_once("owned-descendant:").map(|(_, id)| id.trim().parse::<u32>().unwrap())).unwrap();
        // Hold the owned fixture's process handle before cancellation; its identity cannot be reused.
        // SAFETY: the id came from our child and grants only synchronization/termination access.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, descendant) };
        assert!(!raw.is_null(), "{}", io::Error::last_os_error());
        // SAFETY: the successful fresh handle transfers ownership exactly once.
        let descendant = unsafe { OwnedHandle::from_raw_handle(raw) };
        if exit_early {
            process.write(b"exit\n").unwrap();
            // SAFETY: the child handle is owned and live, even after the process exits.
            assert_eq!(unsafe { WaitForSingleObject(process.child.as_raw_handle(), 2000) }, WAIT_OBJECT_0);
        }
        drop(process);
        // SAFETY: the process handle remains live while waiting and optional failure cleanup run.
        let result = unsafe { WaitForSingleObject(descendant.as_raw_handle(), 2000) };
        if result != WAIT_OBJECT_0 {
            // SAFETY: this is the same owned fixture process handle, never a looked-up replacement pid.
            unsafe {
                TerminateProcess(descendant.as_raw_handle(), 99);
                WaitForSingleObject(descendant.as_raw_handle(), 2000);
            }
        }
        assert_eq!(result, WAIT_OBJECT_0, "descendant outlived cancellation; exited leader: {exit_early}");
    }
}

/// Programs found through the login shell get their arguments unchanged whatever that shell is (fish has no `$@`; a
/// csh-style shell falls back to `/bin/sh`).
#[cfg(unix)]
#[test]
fn programs_start_through_any_login_shell() {
    let mut shells: Vec<String> = ["/bin/sh", "/bin/bash", "/bin/zsh", "/bin/dash", "/bin/tcsh", "/bin/csh"]
        .into_iter()
        .filter(|s| std::path::Path::new(s).exists())
        .map(str::to_string)
        .collect();
    shells.extend(std::env::var("CODEYARD_FISH").ok());
    for shell in shells {
        let out = login_command(&shell, "printf").args(["<%s>", "a b", "it's", "$HOME"]).env("HOME", std::env::temp_dir()).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "<a b><it's><$HOME>", "{shell}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

#[cfg(unix)]
#[test]
fn stopping_a_process_stops_what_it_started() {
    let (p, mut out) = spawn("/bin/sh", &["-c".into(), "sleep 600 & echo $!; wait".into()], None).unwrap();
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    while out.read(&mut b).unwrap() == 1 && b[0] != b'\n' {
        line.push(b[0]);
    }
    let child: i32 = String::from_utf8(line).unwrap().trim().parse().unwrap();
    drop(p);
    let started = Instant::now();
    // SAFETY: signal 0 only checks that the process exists.
    while unsafe { libc::kill(child, 0) } == 0 {
        assert!(started.elapsed() < Duration::from_secs(10), "the grandchild {child} outlived its parent");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
fn fixture(name: &str, stdin: Option<Vec<u8>>, timeout: Duration) -> io::Result<Output> {
    exec_with_timeout(
        &std::env::current_exe()?.to_string_lossy(),
        &["--exact".into(), format!("proc::tests::{name}"), "--ignored".into(), "--nocapture".into()],
        None,
        stdin,
        timeout,
    )
}

#[cfg(windows)]
#[test]
#[ignore = "subprocess fixture"]
fn windows_fixture_stdio() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    print!("{input}");
    eprintln!("stderr-marker");
}

#[cfg(windows)]
#[test]
#[ignore = "subprocess fixture"]
fn windows_fixture_sleep() {
    std::thread::sleep(Duration::from_secs(3));
}

#[cfg(windows)]
#[test]
#[ignore = "subprocess fixture"]
fn windows_fixture_flood() {
    let data = vec![b'x'; MAX_OUTPUT as usize + 65536];
    let writer = std::thread::spawn(move || io::stderr().write_all(&data).unwrap());
    io::stdout().write_all(&vec![b'y'; MAX_OUTPUT as usize + 65536]).unwrap();
    writer.join().unwrap();
}

#[cfg(windows)]
#[test]
#[ignore = "subprocess fixture"]
#[allow(clippy::zombie_processes)] // This bounded fixture deliberately tests pipes inherited after its parent exits.
fn windows_fixture_background() {
    let _child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "proc::tests::windows_fixture_sleep", "--ignored", "--nocapture"])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    println!("parent-exited");
}

#[cfg(windows)]
#[test]
fn windows_exec_roundtrips_unicode_stdin_and_stderr() {
    let out = fixture("windows_fixture_stdio", Some("Türkçe: ıİşğüöç\nsecond line\n".as_bytes().to_vec()), Duration::from_secs(5)).unwrap();
    assert_eq!(out.code, Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Türkçe: ıİşğüöç\nsecond line"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("stderr-marker"));
}

#[cfg(windows)]
#[test]
fn windows_exec_enforces_timeout() {
    let start = Instant::now();
    let err = fixture("windows_fixture_sleep", None, Duration::from_millis(200)).err().expect("timeout");
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[cfg(windows)]
#[test]
fn windows_exec_drains_both_streams_past_the_output_limit() {
    let out = fixture("windows_fixture_flood", None, Duration::from_secs(30)).unwrap();
    assert_eq!(out.code, Some(0));
    assert_eq!(out.stdout.len(), MAX_OUTPUT as usize);
    assert_eq!(out.stderr.len(), MAX_OUTPUT as usize);
}

#[cfg(windows)]
#[test]
fn windows_exec_does_not_wait_for_inherited_background_pipes() {
    let start = Instant::now();
    let out = fixture("windows_fixture_background", None, Duration::from_secs(5)).unwrap();
    assert_eq!(out.code, Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("parent-exited"));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[cfg(windows)]
#[test]
fn windows_batch_launcher_preserves_spaces_unicode_and_literal_arguments() {
    let dir = std::env::temp_dir().join(format!("codeyard-cmd-şablon boşluk-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("language-server.cmd");
    std::fs::write(&script, "@echo off\r\nchcp 65001 >nul\r\necho \"%~1\"\r\ncd\r\n").unwrap();
    let out = exec(&script.with_extension("").to_string_lossy(), &["Türkçe & literal".into()], Some(&dir.to_string_lossy()), None).unwrap();
    assert_eq!(out.code, Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("\"Türkçe & literal\""), "{text}");
    assert!(text.contains(&*dir.to_string_lossy()), "{text}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn writing_to_a_process_that_never_reads_does_not_block() {
    // `sleep` never reads stdin: its pipe fills up. The old code blocked the dispatch thread (and the proc table lock)
    // in `write_all`; now the input is queued and the call returns, finally with WouldBlock.
    let (mut p, _out) = spawn("sleep", &["30".to_string()], None).unwrap();
    let chunk = vec![b'x'; 64 * 1024];
    let started = std::time::Instant::now();
    let mut refused = None;
    for _ in 0..20_000 {
        if let Err(e) = p.write(&chunk) {
            refused = Some(e.kind());
            break;
        }
    }
    assert!(started.elapsed() < Duration::from_secs(10), "write blocked for {:?}", started.elapsed());
    assert_eq!(refused, Some(io::ErrorKind::WouldBlock), "a full queue is reported, not waited for");
    // Killing still works immediately (the writer thread is stuck, the process is not).
    let t = std::time::Instant::now();
    drop(p);
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_missing_working_directory_is_an_error_not_a_silent_fallback() {
    let Err(e) = exec("pwd", &[], Some("/definitely/not/a/dir"), None) else { panic!("must fail") };
    assert!(e.to_string().contains("does not exist"), "{e}");
    assert!(spawn("cat", &[], Some("/definitely/not/a/dir")).is_err());
}

/// The command finished but a process it left in the background (a daemon started by the login shell,
/// git's background maintenance) still holds stderr open: `exec` must not wait for it.
#[test]
#[cfg(unix)]
fn exec_returns_when_the_command_exits_even_if_a_background_child_holds_its_pipes() {
    let t = Instant::now();
    let out = exec("/bin/sh", &["-c".into(), "sleep 20 >/dev/null & echo tamam; echo uyarı >&2".into()], None, None).unwrap();
    let took = t.elapsed();
    assert_eq!((out.code, out.stdout.as_slice(), out.stderr.as_slice()), (Some(0), "tamam\n".as_bytes(), "uyarı\n".as_bytes()));
    assert!(took < Duration::from_secs(3), "{took:?}: waiting for the background process");
    // The same goes for whoever holds stdout.
    let t = Instant::now();
    let out = exec("/bin/sh", &["-c".into(), "sleep 20 2>/dev/null & echo tamam".into()], None, None).unwrap();
    assert_eq!(out.stdout, b"tamam\n");
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
}

/// A command that never finishes: killed when the time runs out, returns an error.
#[test]
#[cfg(unix)]
fn a_command_that_never_ends_is_killed_at_the_timeout() {
    let t = Instant::now();
    let err = exec_with_timeout("/bin/sleep", &["30".into()], None, None, Duration::from_millis(300)).err().expect("timeout");
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

/// A command given stdin (git commit -F -) goes through the same path.
#[test]
#[cfg(unix)]
fn stdin_is_written_and_closed() {
    let out = exec("/bin/cat", &[], None, Some(b"ileti\nikinci satir\n".to_vec())).unwrap();
    assert_eq!(out.stdout, b"ileti\nikinci satir\n");
}

/// Lots of output (exceeding the pipe's 64 KB) is read completely.
#[test]
#[cfg(unix)]
fn large_output_is_complete() {
    let out = exec("/bin/sh", &["-c".into(), "i=0; while [ $i -lt 20000 ]; do echo satır-$i; i=$((i+1)); done; echo son >&2".into()], None, None).unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 20000);
    assert_eq!(text.lines().last(), Some("satır-19999"));
    assert_eq!(out.stderr, b"son\n");
}

#[cfg(unix)]
#[test]
fn stopping_a_process_cleans_its_group_after_the_leader_has_already_exited() {
    let (p, mut out) = spawn("/bin/sh", &["-c".into(), "sleep 30 & echo $!; exit 0".into()], None).unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(&mut out), &mut line).unwrap();
    let child: i32 = line.trim().parse().unwrap();
    // Observe the owned leader's exit without reaping it or releasing its process id.
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
    // SAFETY: info is writable, the id is our unreaped child, and WNOWAIT leaves ownership with Proc.
    assert_eq!(unsafe { libc::waitid(libc::P_PID, p.child.id(), info.as_mut_ptr(), libc::WEXITED | libc::WNOWAIT) }, 0);
    drop(p);
    let start = Instant::now();
    // SAFETY: signal zero only checks the known child; no unrelated processes are targeted.
    while unsafe { libc::kill(child, 0) } == 0 {
        if start.elapsed() > Duration::from_secs(2) {
            // SAFETY: the fixture child is still in its 30-second sleep; clean up before failing.
            unsafe { libc::kill(child, libc::SIGKILL) };
            panic!("the descendant outlived its already-exited group leader");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `cmd /C line` runs the line as cmd reads it: the quotes in it reach the program unchanged.
#[cfg(windows)]
#[test]
fn cmd_runs_its_line_as_written() {
    let out = command("cmd", &["/C".into(), r#"echo "a b"& echo done"#.into()], None).unwrap().output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.lines().map(str::trim).collect::<Vec<_>>(), [r#""a b""#, "done"]);
}
