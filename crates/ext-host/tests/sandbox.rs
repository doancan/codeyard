//! The OS sandbox from outside: `codeyard-exthost --sandbox-selftest` enters it with an extension's policy and tries
//! to escape (src/selftest.rs); every escape must fail with the OS's own refusal, and the extension's own paths must
//! still work. On Windows the same probes run in an AppContainer, by hand on physical hardware
//! (scripts/check-exthost-sandbox-windows.ps1).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

const EXE: &str = env!("CARGO_BIN_EXE_codeyard-exthost");

struct Dirs {
    base: PathBuf,
    package: PathBuf,
    compiled: PathBuf,
    data: PathBuf,
    outside: PathBuf,
}

fn dirs(name: &str) -> Dirs {
    let base = std::env::temp_dir().join(format!("exthost-sandbox-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let d = Dirs { package: base.join("package"), compiled: base.join("compiled.cwasm"), data: base.join("data"), outside: base.join("outside"), base };
    for p in [&d.package, &d.data, &d.outside] {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(d.package.join("extension.toml"), b"id = \"you.probe\"").unwrap();
    std::fs::write(&d.compiled, b"code").unwrap();
    std::fs::write(d.outside.join("secret"), b"secret").unwrap();
    d
}

/// probe → (outcome, detail): `ok`, `denied` or `failed`.
fn outcomes(stdout: &[u8]) -> BTreeMap<String, (String, String)> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|l| {
            let (probe, rest) = l.split_once(": ")?;
            let (outcome, detail) = rest.split_once(' ').unwrap_or((rest, ""));
            Some((probe.to_string(), (outcome.to_string(), detail.to_string())))
        })
        .collect()
}

#[cfg(unix)]
fn selftest(d: &Dirs) -> BTreeMap<String, (String, String)> {
    let args = [&d.package, &d.compiled, &d.data, &d.outside];
    let out = Command::new(EXE).arg("--sandbox-selftest").args(args).env("CODEYARD_EXTHOST_SELFTEST", "1").output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    eprintln!("{stderr}");
    outcomes(&out.stdout)
}

fn expect(results: &BTreeMap<String, (String, String)>, probe: &str, want: &str) {
    let (got, detail) = results.get(probe).unwrap_or_else(|| panic!("no result for {probe}: {results:?}"));
    assert_eq!(got, want, "{probe}: {got} {detail}");
}

#[cfg(unix)]
#[test]
fn every_escape_is_refused_by_the_os() {
    let d = dirs("escape");
    // Something real to reach outside: a file, a listening Unix socket and the home folder.
    let _listener = std::os::unix::net::UnixListener::bind(d.outside.join("socket")).unwrap();
    assert!(std::fs::read_dir(std::env::var("HOME").unwrap()).is_ok());
    let r = selftest(&d);
    eprintln!("{r:#?}");
    for probe in ["sandbox", "read-package", "read-compiled", "write-data"] {
        expect(&r, probe, "ok");
    }
    for probe in ["read-outside", "list-outside", "write-outside", "list-home", "connect-tcp", "bind-tcp", "connect-unix", "exec"] {
        expect(&r, probe, "denied");
    }
    // ~/.ssh may not exist on the machine; it must not be readable either way.
    assert_ne!(r["read-ssh"].0, "ok");
    assert!(!d.outside.join("escaped").exists());
    std::fs::remove_dir_all(&d.base).unwrap();
}

#[cfg(unix)]
#[test]
fn the_sandbox_check_answers_in_one_line() {
    let out = Command::new(EXE).arg("--check-sandbox").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // macOS and Linux CI must provide the sandbox: an unavailable one fails here instead of skipping the escapes.
    assert!(out.status.success(), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(stdout.starts_with("ok: "), "{stdout}");
    eprintln!("{stdout}");
}

#[test]
fn the_selftest_is_a_test_hook_only() {
    let d = dirs("hook");
    let out = Command::new(EXE).arg("--sandbox-selftest").args([&d.package, &d.compiled, &d.data, &d.outside]).output().unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    std::fs::remove_dir_all(&d.base).unwrap();
}

/// Run by scripts/check-exthost-sandbox-windows.ps1 on physical hardware (§15 decision 1): the probes inside an
/// AppContainer started the way the app starts the exthost.
#[cfg(windows)]
#[test]
#[ignore = "needs physical hardware: scripts/check-exthost-sandbox-windows.ps1"]
fn appcontainer_refuses_every_escape() {
    use std::io::Read;
    let d = dirs("appcontainer");
    let policy = ext_proto::appcontainer::Policy {
        extension: "you.probe".into(),
        package_dir: d.package.clone(),
        compiled: d.compiled.clone(),
        data_dir: d.data.clone(),
        memory_bytes: ext_manifest::limits::EXTHOST_MEMORY_BYTES,
    };
    let args = ["--sandbox-selftest".into(), d.package.clone().into_os_string(), d.compiled.clone().into(), d.data.clone().into(), d.outside.clone().into()];
    let mut child = ext_proto::appcontainer::spawn(std::path::Path::new(EXE), &args, &[("CODEYARD_EXTHOST_SELFTEST", "1")], &policy).unwrap();
    drop(child.stdin.take());
    let mut stdout = Vec::new();
    child.stdout.read_to_end(&mut stdout).unwrap();
    assert_eq!(child.wait().unwrap(), 0);
    let r = outcomes(&stdout);
    for (probe, (outcome, detail)) in &r {
        println!("probe {probe}: {outcome} {detail}");
    }
    for probe in ["sandbox", "read-package", "read-compiled", "write-data"] {
        expect(&r, probe, "ok");
    }
    for probe in ["read-outside", "list-outside", "write-outside", "list-home", "connect-tcp", "exec"] {
        expect(&r, probe, "denied");
    }
    assert_ne!(r["read-ssh"].0, "ok");
}
