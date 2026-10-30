//! Native programs from packages (docs/plugin-architecture.md §7, #146). An extension whose `run` names a program
//! inside its package (`./bin/server`) is built per platform; the app keeps each platform's package in the store,
//! checked against the digests in the lock, and copies the program's bytes to the workspace's host before it runs. The
//! host downloads nothing.
//!
//! The copy uses the agent install's path (`host::ssh::connect_session`): the bytes go to a temporary file, the host
//! computes their SHA-256 (`sha256sum`, or `shasum -a 256` on macOS) and the file is moved into place only when it
//! matches the hash in the package's `CONTENTS.sha256`. Before every run the pushed copy's hash is checked again, so a
//! changed copy is replaced, never run. Local and SSH hosts take the same path; Windows hosts (local only) do the same
//! steps with files.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ext_manifest::ExtId;
use host::Host;
use sha2::{Digest as _, Sha256};

/// Bytes per write: well under the protocol's frame limit, so a program of any allowed size fits.
const CHUNK: usize = 8 << 20;

/// One native program, read from a package in the store.
pub struct Program {
    /// Its file name, the last part of its path in the package (`server`, `server.exe`).
    pub name: String,
    pub bytes: Vec<u8>,
    /// SHA-256 the package lists for it (lowercase hex).
    pub sha256: String,
}

/// Finds an extension's program for a platform: `(target triple, run entry)`. Runs on a background thread and may
/// download that platform's package.
#[derive(Clone)]
pub struct Programs(pub Arc<dyn Fn(&str, &str) -> Result<Program, String> + Send + Sync>);

impl std::fmt::Debug for Programs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Programs")
    }
}

/// The platform of the workspace's host, as a target triple packages are built for.
pub fn host_target(host: &dyn Host) -> Option<&'static str> {
    let info = host.info();
    ext_manifest::target_for(&info.arch, &info.os)
}

/// `run` entry `run` of a package unpacked at `dir`, built for `target`: its bytes and the hash its `CONTENTS.sha256`
/// lists. A development folder (`cyx dev`) has no hash list; its `native/<target>/` file is hashed as it is.
pub fn read_program(dir: &Path, target: &str, run: &str) -> Result<Program, String> {
    let file = ext_manifest::program_file(run, target);
    let name = file.rsplit('/').next().unwrap_or(&file).to_string();
    let listed = std::fs::read_to_string(dir.join(ext_pkg::CONTENTS)).ok();
    let (path, sha256) = match listed {
        Some(list) => {
            let sha = list.lines().find_map(|l| l.split_once("  ").filter(|(_, p)| *p == file).map(|(h, _)| h.to_string()));
            (join(dir, &file), sha.ok_or_else(|| crate::i18n::tf("{} is not in the package", &[&file]))?)
        }
        None => (join(&dir.join(ext_pkg::NATIVE_DIR).join(target), &file), String::new()),
    };
    let bytes = std::fs::read(&path).map_err(|e| crate::i18n::tf("Cannot read {0}: {1}", &[&path.display(), &e]))?;
    let sha256 = if sha256.is_empty() { hex(&Sha256::digest(&bytes)) } else { sha256 };
    Ok(Program { name, bytes, sha256 })
}

fn join(dir: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(dir.to_path_buf(), |p, part| p.join(part))
}

/// The package folder of installed extension `id` for `target`: `dir` (the one that runs) when it is built for it, else
/// that platform's package from the store, fetched from the registry the first time.
pub fn package_dir(cfg_dir: Option<&Path>, registries: &[ext_core::registry::Registry], id: &ExtId, dir: &Path, target: &str) -> Result<PathBuf, String> {
    let own = std::fs::read_to_string(dir.join(ext_pkg::TARGET)).ok();
    // A development folder holds every platform under `native/`.
    if own.as_deref().map(str::trim_end) == Some(target) || (own.is_none() && dir.join(ext_pkg::NATIVE_DIR).join(target).is_dir()) {
        return Ok(dir.to_path_buf());
    }
    let cfg = cfg_dir.ok_or_else(|| crate::i18n::tf("{0} has no package for {1}", &[id, &target]))?;
    if let Some(d) = crate::ext::store(cfg).target_dir(id, target) {
        return Ok(d);
    }
    crate::ext_registry::fetch_target(cfg, registries, id, target)
}

/// Where `extension`'s program goes on the host: `~/.cache/codeyard/programs/<id>/<hash>/`, beside the agent's
/// default folder, by its hash, so versions never overwrite each other.
pub fn pushed_path(host: &dyn Host, extension: &str, p: &Program) -> String {
    let home = host.info().home.trim_end_matches(['/', '\\']).to_string();
    format!("{home}/.cache/{}/programs/{extension}/{}/{}", host::ssh::APP_DIR, &p.sha256[..16.min(p.sha256.len())], p.name)
}

/// Make sure the host has `p` at its [`pushed_path`] with the right hash, pushing it if not; the absolute path to run.
/// A hash that does not match after the copy is an error, and nothing is left at the path.
pub fn push(host: &dyn Host, extension: &str, p: &Program) -> Result<String, String> {
    let path = pushed_path(host, extension, p);
    if p.sha256.len() != 64 || !p.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(crate::i18n::tf("{} has no valid hash in its package", &[&p.name]));
    }
    if host.info().os == "windows" {
        return push_files(host, &path, p);
    }
    let fail = |e: host::HostError| e.to_string();
    // The hash of what is there now: a match runs it, anything else (missing, changed) is replaced.
    let check = r#"f="$1"; if [ -f "$f" ] && [ -x "$f" ]; then a=$( (sha256sum "$f" 2>/dev/null || shasum -a 256 "$f") | cut -d' ' -f1); [ "$a" = "$2" ] && { echo ok; exit 0; }; fi; echo missing"#;
    let out = host.exec("sh", &["-c", check, "sh", &path, &p.sha256], None, None).map_err(fail)?;
    if out.ok() && out.stdout_str().trim() == "ok" {
        return Ok(path);
    }
    // Into a temporary file beside the target, in pieces that fit one message each; then the hash on the host, and the
    // move only on a match.
    let tmp = format!("{path}.tmp.{}", host::random_token());
    let mut first = true;
    for chunk in p.bytes.chunks(CHUNK).chain(p.bytes.is_empty().then_some(&[][..])) {
        let script = if first { r#"set -e; mkdir -p "$(dirname "$1")"; cat > "$1""# } else { r#"cat >> "$1""# };
        first = false;
        let out = host.exec("sh", &["-c", script, "sh", &tmp], None, Some(chunk.to_vec())).map_err(fail)?;
        if !out.ok() {
            let _ = host.exec("rm", &["-f", &tmp], None, None);
            return Err(crate::i18n::tf("Cannot copy {0} to {1}: {2}", &[&p.name, &host.label(), &out.stderr_str().trim()]));
        }
    }
    let finish = r#"t="$1"; chmod 755 "$t" || exit 1; h=$( (sha256sum "$t" 2>/dev/null || shasum -a 256 "$t") | cut -d' ' -f1);
if [ "$h" = "$3" ]; then mv -f "$t" "$2"; else rm -f "$t"; echo "program hash mismatch: $h" >&2; exit 3; fi"#;
    let out = host.exec("sh", &["-c", finish, "sh", &tmp, &path, &p.sha256], None, None).map_err(fail)?;
    if out.code == Some(3) {
        return Err(crate::i18n::tf("{0} on {1} does not match the hash in its package; it was not run", &[&p.name, &host.label()]));
    }
    if !out.ok() {
        return Err(crate::i18n::tf("Cannot copy {0} to {1}: {2}", &[&p.name, &host.label(), &out.stderr_str().trim()]));
    }
    Ok(path)
}

/// The same steps on a Windows host, which has no `sh`: Windows hosts are this machine, so they are files here.
fn push_files(host: &dyn Host, path: &str, p: &Program) -> Result<String, String> {
    if host.is_remote() {
        return Err(crate::i18n::tf("{} cannot receive native programs", &[&host.label()]));
    }
    let target = PathBuf::from(path);
    if std::fs::read(&target).is_ok_and(|b| hex(&Sha256::digest(&b)) == p.sha256) {
        return Ok(path.to_string());
    }
    let io = |e: std::io::Error| crate::i18n::tf("Cannot copy {0} to {1}: {2}", &[&p.name, &host.label(), &e]);
    let dir = target.parent().ok_or_else(|| io(std::io::Error::other("no folder")))?;
    std::fs::create_dir_all(dir).map_err(io)?;
    let tmp = dir.join(format!("{}.tmp.{}", p.name, host::random_token()));
    std::fs::write(&tmp, &p.bytes).map_err(io)?;
    let written = std::fs::read(&tmp).map(|b| hex(&Sha256::digest(&b))).unwrap_or_default();
    if written != p.sha256 {
        let _ = std::fs::remove_file(&tmp);
        return Err(crate::i18n::tf("{0} on {1} does not match the hash in its package; it was not run", &[&p.name, &host.label()]));
    }
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&tmp, &target).map_err(io)?;
    Ok(path.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
#[path = "ext_programs_tests.rs"]
mod tests;
