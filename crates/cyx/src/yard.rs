//! `cyx yard`: the registry operator's commands (docs/plugin-architecture.md §8). Authors never need them; they make
//! the root key ceremony and a test registry three commands each.
//!
//! ```text
//! cyx yard keygen FILE                              a new signing key, readable only by you; prints the public key
//! cyx yard sign <root|index> FILE --key KEYFILE     add your signature to FILE.sig (root holders sign one after another)
//! cyx yard verify FOLDER --root-key HEX… [--threshold N]
//! ```

use std::path::{Path, PathBuf};

use ext_core::yard::{self, Role, RootKeys, Seen, SigningKey};

pub const HELP: &str = "registry operators:
  cyx yard keygen FILE                       write a new signing key; prints its public key
  cyx yard sign <root|index> FILE --key KEY  add a signature to FILE.sig
  cyx yard verify FOLDER --root-key HEX… [--threshold N]
                                             check root.json, index.json and their signatures";

pub fn run(args: &[String]) -> Result<(), String> {
    let usage = || format!("usage:\n{HELP}");
    let (cmd, rest) = args.split_first().ok_or_else(usage)?;
    match (cmd.as_str(), rest) {
        ("keygen", [file]) => {
            let public = keygen(Path::new(file))?;
            println!("Wrote {file}\npublic key {}", public.hex());
            Ok(())
        }
        ("sign", [role, file, flag, key]) if flag == "--key" => {
            let role = match role.as_str() {
                "root" => Role::Root,
                "index" => Role::Index,
                _ => return Err(format!("cyx yard sign: the role is root or index, not '{role}'")),
            };
            let (sig, signer) = sign(role, Path::new(file), Path::new(key))?;
            println!("Signed {file} as {}; signatures are in {}", signer.hex(), sig.display());
            Ok(())
        }
        ("verify", [dir, opts @ ..]) => {
            let keys = parse_trust(opts)?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let v = verify(Path::new(dir), &keys, now)?;
            let versions: usize = v.index.extensions.values().map(|e| e.versions.len()).sum();
            println!(
                "{}: root version {}, index version {}, {} extensions, {versions} versions; index expires at {} (Unix time)",
                v.root.registry,
                v.root.version,
                v.index.version,
                v.index.extensions.len(),
                v.index.expires
            );
            if v.stale {
                return Err("error: the index has expired\n   = fix: sign a new index with the online key".into());
            }
            Ok(())
        }
        _ => Err(usage()),
    }
}

/// Write a new key to `file` (never over an existing one), readable only by its owner.
pub fn keygen(file: &Path) -> Result<yard::PublicKey, String> {
    let (key, text) = SigningKey::generate()?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(file).map_err(|e| format!("error: cannot create {}: {e}", file.display()))?;
    std::io::Write::write_all(&mut f, text.as_bytes()).map_err(|e| format!("error: cannot write {}: {e}", file.display()))?;
    Ok(key.public())
}

/// Add the key's signature over `file` to `file.sig`.
pub fn sign(role: Role, file: &Path, key: &Path) -> Result<(PathBuf, yard::PublicKey), String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("error: cannot read {}: {e}", p.display()));
    let key = SigningKey::from_file(&String::from_utf8_lossy(&read(key)?)).map_err(|e| format!("error: {}: {e}", key.display()))?;
    let bytes = read(file)?;
    let sig = sig_path(file);
    let existing = if sig.exists() { String::from_utf8_lossy(&read(&sig)?).into_owned() } else { String::new() };
    let text = yard::add_signature(&existing, &key, role, &bytes).map_err(|e| format!("error: {}: {e}", sig.display()))?;
    std::fs::write(&sig, text).map_err(|e| format!("error: cannot write {}: {e}", sig.display()))?;
    Ok((sig, key.public()))
}

pub fn verify(dir: &Path, keys: &RootKeys, now: u64) -> Result<yard::Verified, String> {
    let read = |name: &str| std::fs::read(dir.join(name)).map_err(|e| format!("error: cannot read {}: {e}", dir.join(name).display()));
    let files = [read(yard::ROOT_FILE)?, read(&sig_name(yard::ROOT_FILE))?, read(yard::INDEX_FILE)?, read(&sig_name(yard::INDEX_FILE))?];
    yard::verify(keys, [&files[0], &files[1], &files[2], &files[3]], now, Seen::default()).map_err(|e| e.to_string())
}

fn parse_trust(opts: &[String]) -> Result<RootKeys, String> {
    let mut keys = Vec::new();
    let mut threshold = None;
    let mut it = opts.iter();
    while let Some(o) = it.next() {
        let value = it.next().ok_or_else(|| format!("cyx yard verify: {o} needs a value"))?;
        match o.as_str() {
            "--root-key" => keys.push(yard::PublicKey::parse(value).ok_or_else(|| format!("cyx yard verify: '{value}' is not a public key (64 hex digits)"))?),
            "--threshold" => threshold = Some(value.parse().map_err(|_| format!("cyx yard verify: '{value}' is not a number"))?),
            _ => return Err(format!("cyx yard verify: unknown option {o}")),
        }
    }
    if keys.is_empty() {
        return Err("cyx yard verify: give the root public keys with --root-key".into());
    }
    // Without --threshold every given key must have signed.
    Ok(RootKeys { threshold: threshold.unwrap_or(keys.len()), keys })
}

fn sig_name(file: &str) -> String {
    format!("{file}{}", yard::SIG_SUFFIX)
}

fn sig_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(yard::SIG_SUFFIX);
    PathBuf::from(s)
}
