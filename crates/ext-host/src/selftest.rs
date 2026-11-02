//! `codeyard-exthost --sandbox-selftest <package dir> <compiled> <data dir> <outside dir>`: enters the sandbox with
//! the policy an extension would get, then tries what an escaped extension would try and prints one line per
//! attempt. tests/sandbox.rs and scripts/check-exthost-sandbox-windows.ps1 read them. A test hook, not a feature: it
//! runs only with `CODEYARD_EXTHOST_SELFTEST=1`.
//!
//! Lines: `<probe>: ok`, `<probe>: denied (<error>)` (the OS refused: `PermissionDenied`), or
//! `<probe>: failed (<error>)` (any other error, which proves nothing either way).

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::sandbox::{self, Policy};

pub const ENV: &str = "CODEYARD_EXTHOST_SELFTEST";

pub fn run(args: &[String]) -> Result<(), String> {
    if std::env::var_os(ENV).is_none_or(|v| v != "1") {
        return Err(format!("--sandbox-selftest is a test hook; it runs only with {ENV}=1"));
    }
    let [package, compiled, data, outside] = args else {
        return Err("usage: codeyard-exthost --sandbox-selftest <package dir> <compiled> <data dir> <outside dir>".into());
    };
    let (package, compiled, data, outside) = (Path::new(package), Path::new(compiled), Path::new(data), Path::new(outside));
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    let what = sandbox::enter_unverified(&Policy { read: vec![package.into(), compiled.into()], write: vec![data.into()] })?;
    let mut out = std::io::stdout().lock();
    let mut line = |probe: &str, r: std::io::Result<()>| {
        let _ = match r {
            Ok(()) => writeln!(out, "{probe}: ok"),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => writeln!(out, "{probe}: denied ({e})"),
            Err(e) => writeln!(out, "{probe}: failed ({e})"),
        };
    };
    line("sandbox", Ok(()));
    eprintln!("selftest: {what}");

    // Must work.
    line("read-package", std::fs::read_dir(package).map(drop));
    line("read-compiled", std::fs::read(compiled).map(drop));
    line(
        "write-data",
        (|| {
            let f = data.join("selftest.tmp");
            std::fs::write(&f, b"x")?;
            std::fs::rename(&f, data.join("selftest"))?;
            std::fs::remove_file(data.join("selftest"))
        })(),
    );
    // Must be refused.
    line("read-outside", std::fs::read(outside.join("secret")).map(drop));
    line("list-outside", std::fs::read_dir(outside).map(drop));
    line("write-outside", std::fs::write(outside.join("escaped"), b"x"));
    line("list-home", std::fs::read_dir(&home).map(drop));
    line("read-ssh", std::fs::read_dir(home.join(".ssh")).map(drop));
    let addr = std::net::SocketAddr::from(([1, 1, 1, 1], 443));
    line("connect-tcp", std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(3)).map(drop));
    line("bind-tcp", std::net::TcpListener::bind("127.0.0.1:0").map(drop));
    #[cfg(unix)]
    line("connect-unix", std::os::unix::net::UnixStream::connect(outside.join("socket")).map(drop));
    let shell = if cfg!(windows) { r"C:\Windows\System32\cmd.exe" } else { "/bin/sh" };
    let flag = if cfg!(windows) { "/c" } else { "-c" };
    line("exec", std::process::Command::new(shell).args([flag, "exit 0"]).status().map(drop));
    Ok(())
}
