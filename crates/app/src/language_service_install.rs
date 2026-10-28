//! Transactional language-service installation on the document's host, outside workspace folders.
//! Call from a worker. No lifecycle scripts, global packages or workspace configuration are used.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use host::{Host, ProcEvent};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Value, json};

use crate::language_services::{Catalog, PACKAGE_JSON, PACKAGE_LOCK, Service, supported_node};

const LOG_LIMIT: usize = 64 * 1024;
const ARCHIVE_LIMIT: u64 = 128 * 1024 * 1024;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Checking,
    RuntimeDownload,
    RuntimeVerify,
    Packages,
    Verifying,
    Publishing,
    Ready,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub stage: Stage,
    pub log: String,
}

#[derive(Default)]
pub struct Progress {
    pub cancel: AtomicBool,
    snapshot: Mutex<Snapshot>,
}

impl Progress {
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn stage(&self, stage: Stage) {
        self.snapshot.lock().unwrap_or_else(|e| e.into_inner()).stage = stage;
    }

    fn log(&self, bytes: &[u8]) {
        let mut state = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        state.log.push_str(&String::from_utf8_lossy(bytes));
        if state.log.len() > LOG_LIMIT {
            let cut = (state.log.len() - LOG_LIMIT..=state.log.len()).find(|i| state.log.is_char_boundary(*i)).unwrap_or(state.log.len());
            state.log.drain(..cut);
        }
    }

    fn check(&self) -> Result<(), Error> {
        if self.cancel.load(Ordering::Acquire) { Err(Error::Cancelled) } else { Ok(()) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Cancelled,
    /// A lost host or missing stop acknowledgement leaves staging for inspection; never delete beneath a live writer.
    Uncertain,
    InvalidCatalog,
    UnsupportedRuntime,
    Integrity,
    InvalidInstallation,
    Failed(String),
}

fn failed(error: impl std::fmt::Display) -> Error {
    Error::Failed(error.to_string())
}

fn host_error(error: host::HostError) -> Error {
    match error {
        host::HostError::Disconnected(_) | host::HostError::Uncertain(_) => Error::Uncertain,
        error => failed(error),
    }
}

fn join(root: &str, tail: &str) -> String {
    format!("{}/{tail}", root.trim_end_matches(['/', '\\']))
}

fn absolute(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 32768
        && !path.chars().any(char::is_control)
        && (path.starts_with('/') || path.starts_with("\\\\") || path.as_bytes().get(1) == Some(&b':') && matches!(path.as_bytes().get(2), Some(b'/' | b'\\')))
}

pub fn destination(host: &dyn Host) -> Result<String, Error> {
    if !absolute(&host.info().home) {
        return Err(Error::InvalidInstallation);
    }
    Ok(join(&host.info().home, ".codeyard/language-services"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub directory: String,
    pub node: String,
    pub node_version: String,
    pub fingerprint: String,
}

impl Installed {
    pub fn command(&self, service: Service) -> (String, Vec<String>) {
        (self.node.clone(), vec![join(&self.directory, service.entry()), "--stdio".into()])
    }

    fn record(&self, generation: &str) -> Value {
        json!({"version":1,"generation":generation,"node":self.node,"node_version":self.node_version,"fingerprint":self.fingerprint})
    }
}

/// Reads only our bounded active record. Entry scripts are fixed by the bundled catalogue.
pub fn discover(host: &dyn Host, root: &str) -> Result<Option<Installed>, Error> {
    let active = join(root, "active.json");
    match host.stat(&active) {
        Err(host::HostError::Op(proto::Error::NotFound(_))) => return Ok(None),
        Err(error) => return Err(host_error(error)),
        Ok(meta) if meta.size > 16384 => return Err(Error::InvalidInstallation),
        Ok(_) => {}
    }
    let bytes = host.read(&active).map_err(host_error)?.0;
    if bytes.len() > 16384 {
        return Err(Error::InvalidInstallation);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::InvalidInstallation)?;
    let text = |key| value.get(key).and_then(Value::as_str).ok_or(Error::InvalidInstallation);
    let catalog = Catalog::bundled().map_err(|_| Error::InvalidCatalog)?;
    let generation = text("generation")?;
    let prefix = format!("bundle-{}-", catalog.fingerprint);
    if value.get("version").and_then(Value::as_u64) != Some(1)
        || text("fingerprint")? != catalog.fingerprint
        || !generation.strip_prefix(&prefix).is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        || !absolute(text("node")?)
        || !supported_node(text("node_version")?)
    {
        return Err(Error::InvalidInstallation);
    }
    let installed = Installed {
        directory: join(root, generation),
        node: text("node")?.into(),
        node_version: text("node_version")?.into(),
        fingerprint: catalog.fingerprint,
    };
    verify_entries(host, &installed)?;
    Ok(Some(installed))
}

fn run(host: &dyn Host, program: &str, args: Vec<String>, cwd: &str, progress: &Progress) -> Result<Vec<u8>, Error> {
    progress.check()?;
    let proc = host.proc_spawn(program, args, Some(cwd.into())).map_err(host_error)?;
    let start = Instant::now();
    let mut stopping = None;
    let mut output = Vec::new();
    loop {
        if stopping.is_none() && (progress.cancel.load(Ordering::Acquire) || start.elapsed() > PROCESS_TIMEOUT) {
            host.proc_kill(proc.id);
            stopping = Some(Instant::now());
        }
        if stopping.is_some_and(|at| at.elapsed() > STOP_TIMEOUT) {
            return Err(Error::Uncertain);
        }
        match proc.events.recv_timeout(Duration::from_millis(50)) {
            Ok(ProcEvent::Output(bytes)) => {
                progress.log(&bytes);
                let keep = bytes.len().min(LOG_LIMIT.saturating_sub(output.len()));
                output.extend_from_slice(&bytes[..keep]);
            }
            Ok(ProcEvent::Exit(None)) | Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Error::Uncertain),
            Ok(ProcEvent::Exit(Some(code))) => {
                progress.check()?;
                if stopping.is_some() {
                    return Err(failed("The installation process timed out."));
                }
                return if code == 0 { Ok(output) } else { Err(failed(format!("{program} exited with code {code}"))) };
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn verify_entries(host: &dyn Host, installed: &Installed) -> Result<(), Error> {
    for path in std::iter::once(installed.node.clone()).chain([Service::TypeScript, Service::Html, Service::Css].map(|s| join(&installed.directory, s.entry())))
    {
        let meta = host.stat(&path).map_err(host_error)?;
        if meta.kind != proto::Kind::File || meta.size == 0 {
            return Err(Error::InvalidInstallation);
        }
    }
    Ok(())
}

// This fixed script finds npm beside the selected Node binary, without searching workspace modules.
const NODE_PROBE: &str = "const p=require('node:path'),f=require('node:fs'),d=p.dirname(process.execPath);const npm=[p.join(d,'node_modules/npm/bin/npm-cli.js'),p.join(d,'../lib/node_modules/npm/bin/npm-cli.js')].find(x=>f.existsSync(x));console.log(JSON.stringify({node:process.execPath,version:process.version,npm:npm?f.realpathSync(npm):null}))";

fn runtime(host: &dyn Host, staging: &str, progress: &Progress) -> Result<(String, String, String), Error> {
    let probe = run(host, "node", vec!["-e".into(), NODE_PROBE.into()], staging, progress);
    match probe {
        Ok(bytes) => {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                && let (Some(node), Some(version), Some(npm)) = (value["node"].as_str(), value["version"].as_str(), value["npm"].as_str())
                && absolute(node)
                && absolute(npm)
                && supported_node(version)
            {
                return Ok((node.into(), version.into(), npm.into()));
            }
        }
        Err(error @ (Error::Cancelled | Error::Uncertain)) => return Err(error),
        Err(_) => {}
    }
    progress.check()?;
    let os = match host.info().os.as_str() {
        "macos" => "darwin",
        "windows" => "win",
        "linux" => "linux",
        _ => return Err(Error::UnsupportedRuntime),
    };
    let arch = match host.info().arch.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        _ => return Err(Error::UnsupportedRuntime),
    };
    let suffix = if os == "win" { "zip" } else { "tar.gz" };
    let name = format!("node-v22.23.4-{os}-{arch}.{suffix}");
    let catalog: Value = serde_json::from_str(include_str!("../resources/language-services/node-runtimes.json")).map_err(|_| Error::InvalidCatalog)?;
    let asset = catalog["assets"].as_array().and_then(|assets| assets.iter().find(|a| a["name"] == name)).ok_or(Error::UnsupportedRuntime)?;
    let (Some(url), Some(expected)) = (asset["url"].as_str(), asset["sha256"].as_str()) else { return Err(Error::InvalidCatalog) };
    if url != format!("https://nodejs.org/dist/v22.23.4/{name}") || expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidCatalog);
    }
    let archive = join(staging, &name);
    progress.stage(Stage::RuntimeDownload);
    let args = [
        "--disable",
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--stderr",
        "-",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "15",
        "--max-time",
        "600",
        "--speed-limit",
        "1024",
        "--speed-time",
        "60",
        "--max-filesize",
        "134217728",
        "--output",
        &archive,
        "--",
        url,
    ];
    run(host, if os == "win" { "curl.exe" } else { "curl" }, args.map(String::from).to_vec(), staging, progress)?;
    if host.stat(&archive).map_err(host_error)?.size > ARCHIVE_LIMIT {
        return Err(Error::Integrity);
    }
    progress.stage(Stage::RuntimeVerify);
    let (hasher, args) = match os {
        "darwin" => ("shasum", vec!["-a", "256", "--", &archive]),
        "win" => ("certutil.exe", vec!["-hashfile", &archive, "SHA256"]),
        _ => ("sha256sum", vec!["--", &archive]),
    };
    let hash = run(host, hasher, args.into_iter().map(String::from).collect(), staging, progress)?;
    if !String::from_utf8_lossy(&hash).split_whitespace().any(|word| word.eq_ignore_ascii_case(expected)) {
        return Err(Error::Integrity);
    }
    run(host, "tar", vec!["-xf".into(), archive, "-C".into(), staging.into()], staging, progress)?;
    let runtime = join(staging, &format!("node-v22.23.4-{os}-{arch}"));
    let (node, npm) = if os == "win" {
        (join(&runtime, "node.exe"), join(&runtime, "node_modules/npm/bin/npm-cli.js"))
    } else {
        (join(&runtime, "bin/node"), join(&runtime, "lib/node_modules/npm/bin/npm-cli.js"))
    };
    let version = run(host, &node, vec!["--version".into()], staging, progress)?;
    let version = String::from_utf8_lossy(&version).trim().to_string();
    if !supported_node(&version) {
        return Err(Error::UnsupportedRuntime);
    }
    Ok((node, version, npm))
}

/// A successful generation becomes active in one atomic file write. Earlier generations are retained.
/// Failed or cancelled work removes only its own staging; uncertain writers are left untouched.
pub fn install(host: &dyn Host, root: &str, progress: &Progress) -> Result<Installed, Error> {
    progress.check()?;
    let catalog = Catalog::bundled().map_err(|_| Error::InvalidCatalog)?;
    if !absolute(root) {
        return Err(Error::InvalidInstallation);
    }
    let mut nonce = [0; 16];
    SystemRandom::new().fill(&mut nonce).map_err(failed)?;
    let nonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let staging = join(root, &format!(".staging-{nonce}"));
    host.create_dir(&staging).map_err(host_error)?;
    let result = (|| {
        if host.info().os != "windows" {
            run(host, "chmod", vec!["700".into(), staging.clone()], root, progress)?;
        }
        let (node, node_version, npm) = runtime(host, &staging, progress)?;
        for (name, content) in [("package.json", PACKAGE_JSON), ("package-lock.json", PACKAGE_LOCK), ("user.npmrc", ""), ("global.npmrc", "")] {
            host.write(&join(&staging, name), content.as_bytes().to_vec(), None).map_err(host_error)?;
        }
        progress.stage(Stage::Packages);
        // Forward npm's diagnostic stream into the bounded process output channel.
        let wrapper = "process.stderr.write=process.stdout.write.bind(process.stdout);require(process.argv[1])";
        let args = vec![
            "-e".into(),
            wrapper.into(),
            npm,
            "ci".into(),
            "--ignore-scripts".into(),
            "--no-audit".into(),
            "--no-fund".into(),
            "--registry=https://registry.npmjs.org".into(),
            "--fetch-retries=0".into(),
            "--fetch-timeout=30000".into(),
            format!("--userconfig={}", join(&staging, "user.npmrc")),
            format!("--globalconfig={}", join(&staging, "global.npmrc")),
            format!("--cache={}", join(&staging, "cache")),
        ];
        run(host, &node, args, &staging, progress)?;
        progress.stage(Stage::Verifying);
        let installed = Installed { directory: staging.clone(), node, node_version, fingerprint: catalog.fingerprint.clone() };
        verify_entries(host, &installed)?;
        progress.check()?;
        progress.stage(Stage::Publishing);
        let generation = format!("bundle-{}-{nonce}", catalog.fingerprint);
        let directory = join(root, &generation);
        let node = installed.node.strip_prefix(&format!("{staging}/")).map(|rel| join(&directory, rel)).unwrap_or(installed.node);
        host.rename(&staging, &directory).map_err(host_error)?;
        let installed = Installed { directory, node, ..installed };
        host.write(&join(root, "active.json"), installed.record(&generation).to_string().into_bytes(), None).map_err(host_error)?;
        progress.stage(Stage::Ready);
        Ok(installed)
    })();
    if !matches!(result, Err(Error::Uncertain)) && host.stat(&staging).is_ok() {
        let _ = host.delete(&staging);
    }
    result
}

#[cfg(test)]
#[path = "language_service_install_tests.rs"]
pub(crate) mod tests;
