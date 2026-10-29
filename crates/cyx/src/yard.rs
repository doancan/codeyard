//! `cyx yard`: the registry operator's commands (docs/plugin-architecture.md §8). Authors never need them; they make
//! the root key ceremony and a test registry three commands each.
//!
//! ```text
//! cyx yard keygen FILE                              a new signing key, readable only by you; prints the public key
//! cyx yard sign <root|index> FILE --key KEYFILE     add your signature to FILE.sig (root holders sign one after another)
//! cyx yard verify FOLDER --root-key HEX… [--threshold N]
//! cyx yard fixture FOLDER --extensions N --url URL      a signed index of N made-up extensions, for the Market
//! ```

use std::path::{Path, PathBuf};

use ext_core::yard::{self, Role, RootKeys, Seen, SigningKey};

pub const HELP: &str = "registry operators:
  cyx yard keygen FILE                       write a new signing key; prints its public key
  cyx yard sign <root|index> FILE --key KEY  add a signature to FILE.sig
  cyx yard verify FOLDER --root-key HEX… [--threshold N]
                                             check root.json, index.json and their signatures
  cyx yard fixture FOLDER --extensions N --url URL
                                             a signed index of N made-up extensions (no packages) for
                                             trying the Market and its performance run; prints the
                                             extensions.registries entry that trusts it";

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
        ("fixture", [dir, flag, n, url_flag, url]) if flag == "--extensions" && url_flag == "--url" => {
            let n: usize = n.parse().map_err(|_| format!("cyx yard fixture: '{n}' is not a number"))?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            println!("{}", fixture(Path::new(dir), n, url, now)?);
            Ok(())
        }
        _ => Err(usage()),
    }
}

/// Write `root.json`, `index.json` and their signatures for `n` made-up extensions (the same ones for the same `n`)
/// into `dir`, with fresh keys that are not kept. Returns the `extensions.registries` entry for `url`.
pub fn fixture(dir: &Path, n: usize, url: &str, now: u64) -> Result<String, String> {
    const WORDS: [&str; 24] = [
        "night", "owl", "rust", "theme", "snippet", "keymap", "dark", "light", "go", "python", "solar", "forest", "ocean", "mono", "retro", "neon", "paper",
        "vim", "emacs", "toml", "yaml", "docker", "sql", "markdown",
    ];
    const CATEGORIES: [&str; 5] = ["themes", "snippets", "keymaps", "languages", "other"];
    let write = |name: &str, bytes: &[u8]| std::fs::write(dir.join(name), bytes).map_err(|e| format!("error: cannot write {}: {e}", dir.join(name).display()));
    std::fs::create_dir_all(dir).map_err(|e| format!("error: cannot create {}: {e}", dir.display()))?;
    let (root_key, _) = SigningKey::generate()?;
    let (online, _) = SigningKey::generate()?;
    let root = yard::Root {
        format: yard::FORMAT,
        version: 1,
        registry: "fixture.test".into(),
        online: yard::OnlineKey { key: online.public(), expires: now + 90 * 86400 },
        fulcio: String::new(),
        rekor: String::new(),
    };
    // A small linear congruential generator: the same index for the same `n`, without a dependency.
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = |m: usize| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as usize % m
    };
    let mut extensions = std::collections::BTreeMap::new();
    for i in 0..n {
        let w: Vec<&str> = (0..5).map(|_| WORDS[next(WORDS.len())]).collect();
        let id = ext_manifest::ExtId::parse(&format!("p{}.{}-{}-{i}", i % 997, w[0], w[1])).map_err(|e| e.to_string())?;
        let versions = (0..1 + next(3))
            .map(|v| yard::Release {
                version: ext_manifest::Version::parse(&format!("1.{v}.0")).expect("a version"),
                digest: format!("{:064x}", i * 4 + v),
                sha256: format!("{:064x}", i * 4 + v),
                size: 1000,
                permissions: format!("{:064x}", 0),
                code: false,
                published: now - 86400 * (30 - v as u64),
                status: yard::Status::Ok,
                downloads: next(5000) as u64,
                immediate: false,
                details: None,
            })
            .collect();
        let name = format!("{} {}", capitalize(w[0]), capitalize(w[1]));
        let description = format!("A {} {} for {} and {}.", w[2], w[3], w[4], w[0]);
        let entry = yard::Entry {
            name,
            description,
            categories: vec![CATEGORIES[next(CATEGORIES.len())].to_string()],
            publisher: yard::Publisher {
                issuer: "https://token.actions.githubusercontent.com".into(),
                subject: None,
                owner_id: Some(i as u64),
                repository_id: Some(i as u64),
                workflow: Some(".github/workflows/publish.yml".into()),
            },
            versions,
        };
        extensions.insert(id, entry);
    }
    let index = yard::Index { format: yard::FORMAT, registry: root.registry.clone(), version: 1, expires: now + yard::INDEX_LIFETIME_SECS, extensions };
    let root_bytes = serde_json::to_vec(&root).map_err(|e| e.to_string())?;
    let index_bytes = serde_json::to_vec(&index).map_err(|e| e.to_string())?;
    write(yard::ROOT_FILE, &root_bytes)?;
    write(&sig_name(yard::ROOT_FILE), yard::add_signature("", &root_key, Role::Root, &root_bytes)?.as_bytes())?;
    write(yard::INDEX_FILE, &index_bytes)?;
    write(&sig_name(yard::INDEX_FILE), yard::add_signature("", &online, Role::Index, &index_bytes)?.as_bytes())?;
    Ok(serde_json::json!({ "name": "fixture", "url": url, "root-keys": [root_key.public().hex()], "threshold": 1 }).to_string())
}

fn capitalize(w: &str) -> String {
    let mut c = w.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
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
