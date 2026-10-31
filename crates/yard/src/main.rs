//! `yard`: run the registry's publish service, or act as its security role.
//!
//! ```text
//! yard init FOLDER [--registry NAME] [--root-keys HEX,… --threshold N] [--fulcio PEM --rekor PEM]
//!                                              a new registry: keys, root.json, yard.json
//! yard serve CONFIG                            answer publishers; sign the index now and daily
//! yard rotate CONFIG                           the next online key and root.next.json, for the root key holders
//! yard promote CONFIG                          check their signatures and put the new root and key in place
//! yard resign CONFIG [--expired]               sign a fresh index once (the daily timer); --expired: one that has
//!                                              already expired, to see clients refuse it (staging and rehearsals)
//! yard status CONFIG [--hours N] [--key-days N] the index's and online key's expiry; fails within N h (24) / days (14)
//! yard revoke CONFIG ID VERSION REASON…        stop a version on every client, with the reason users see
//! yard immediate CONFIG ID VERSION             let clients update to an advisory fix at once
//! yard review list CONFIG                      first native programs waiting for a reviewer
//! yard review approve CONFIG ID VERSION        list one (and trust its publisher's later native programs)
//! yard review reject CONFIG ID VERSION REASON… refuse one; its files are deleted
//! ```

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use ext_manifest::{ExtId, Version};
use yard::{Config, Yard, http};

const HELP: &str = "yard: the Codeyard extension registry

usage:
  yard init FOLDER [--registry NAME] [--root-keys HEX,HEX,… --threshold N] [--fulcio PEM --rekor PEM]
                                           create a registry (keys, root.json, yard.json); with the
                                           production root keys the root waits for their signatures;
                                           --fulcio/--rekor pin another Sigstore (a rehearsal's stand-in)
  yard serve CONFIG                        run the publish service
  yard rotate CONFIG                       start an online key rotation (root.next.json to sign)
  yard promote CONFIG                      finish it once the root key holders signed
  yard resign CONFIG [--expired]           sign a fresh index once (--expired: one already past its
                                           expiry, to check that clients refuse to install from it)
  yard status CONFIG [--hours N] [--key-days N]
                                           index and online key expiry and counts; fails when the
                                           index expires within N hours (24) or the key within N days (14)
  yard revoke CONFIG ID VERSION REASON…    revoke a version everywhere
  yard immediate CONFIG ID VERSION         let clients update to an advisory fix at once
  yard review list CONFIG                  first native programs waiting for a reviewer
  yard review approve CONFIG ID VERSION    list it; the publisher's later native programs are not held
  yard review reject CONFIG ID VERSION REASON…
                                           refuse it; its files are deleted, its version stays used

Code and native programs are taken from the publishers listed in yard.json's \"invited\" only.

Docs: docs/registry.md";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn open(config: &str) -> Result<Yard, String> {
    Yard::open(Config::load(Path::new(config))?)
}

fn id_version(id: &str, version: &str) -> Result<(ExtId, Version), String> {
    Ok((ExtId::parse(id).map_err(|e| format!("id {e}"))?, Version::parse(version).map_err(|e| format!("version {e}"))?))
}

fn run(args: &[String]) -> Result<(), String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["init", dir, rest @ ..] => {
            let (mut registry, mut keys, mut threshold) = ("localhost", None, None);
            let (mut fulcio, mut rekor) = (yard::SIGSTORE_FULCIO.to_string(), yard::SIGSTORE_REKOR.to_string());
            let read = |path: &str| std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"));
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                let value = *it.next().ok_or(HELP)?;
                match *flag {
                    "--registry" => registry = value,
                    "--root-keys" => {
                        let parsed: Option<Vec<_>> = value.split(',').map(|k| ext_core::yard::PublicKey::parse(k.trim())).collect();
                        keys = Some(parsed.ok_or("--root-keys: each key is 64 hex digits, separated by commas")?);
                    }
                    "--threshold" => threshold = Some(value.parse::<usize>().map_err(|_| format!("--threshold: '{value}' is not a number"))?),
                    "--fulcio" => fulcio = read(value)?,
                    "--rekor" => rekor = read(value)?,
                    _ => return Err(HELP.into()),
                }
            }
            let trust = keys.map(|keys| ext_core::yard::RootKeys { threshold: threshold.unwrap_or(keys.len()), keys });
            let production = trust.is_some();
            let cfg = yard::init(Path::new(dir), registry, &fulcio, &rekor, trust, http::now())?;
            let root = cfg.with_file_name("root.json");
            if production {
                println!(
                    "Created {}\nroot.json has no signatures yet. Each root key holder signs it on their own computer:\n  \
                     cyx yard sign root root.json --key KEYFILE   (or --key - to paste the key)\nthen copy root.json.sig back to {}.sig.\n\nNext:\n  yard serve {}",
                    cfg.display(),
                    root.display(),
                    cfg.display()
                );
            } else {
                println!(
                    "Created {}\nThe root is signed by one development key (keys/root.key); its public key is in root-key.txt.\n\
                     For production, use --root-keys with the keys from the root key ceremony.\n\nNext:\n  yard serve {}",
                    cfg.display(),
                    cfg.display()
                );
            }
            Ok(())
        }
        ["rotate", config] => {
            let next = yard::rotate(Path::new(config), http::now())?;
            println!(
                "Wrote {0} and the next online key beside the current one.\nEach root key holder signs it on their own computer:\n  \
                 cyx yard sign root {1} --key KEYFILE   (or --key - to paste the key)\nWith enough signatures in {0}.sig:\n  yard promote {config}",
                next.display(),
                next.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            );
            Ok(())
        }
        ["promote", config] => {
            let v = yard::promote(Path::new(config), http::now())?;
            println!("Root version {v} is live with the new online key, and a fresh index is signed. Restart `yard serve` so it signs with it.");
            Ok(())
        }
        ["serve", config] => {
            let yard = Arc::new(open(config)?);
            let listen = yard.config().listen.clone();
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind(&listen).await.map_err(|e| format!("cannot listen on {listen}: {e}"))?;
                eprintln!("yard: serving {} on http://{listen}", yard.config().public.display());
                http::serve(yard, listener).await.map_err(|e| e.to_string())
            })
        }
        ["resign", config] => {
            let v = open(config)?.resign(http::now()).map_err(|e| e.to_string())?;
            println!("Signed index version {v}");
            Ok(())
        }
        ["resign", config, "--expired"] => {
            // Signed as of a moment long enough ago that it has already expired; the next resign replaces it.
            let at = http::now().saturating_sub(ext_core::yard::INDEX_LIFETIME_SECS + 3600);
            let v = open(config)?.resign(at).map_err(|e| e.to_string())?;
            println!("Signed index version {v}, already expired: clients refuse to install from it until the next `yard resign`");
            Ok(())
        }
        ["status", config, opts @ ..] => {
            let (mut hours, mut key_days) = (24, 14);
            let mut it = opts.iter();
            while let Some(flag) = it.next() {
                let value = it.next().ok_or(HELP)?;
                let n = value.parse::<u64>().map_err(|_| format!("{flag}: '{value}' is not a number"))?;
                match *flag {
                    "--hours" => hours = n,
                    "--key-days" => key_days = n,
                    _ => return Err(HELP.into()),
                }
            }
            let h = open(config)?.health().map_err(|e| e.to_string())?;
            let now = http::now();
            println!(
                "index version {}, expires in {} h; online key expires in {} days; {} extensions, {} versions ({} revoked); {} waiting for review",
                h.index_version,
                h.index_expires.saturating_sub(now) / 3600,
                h.online_expires.saturating_sub(now) / 86400,
                h.extensions,
                h.versions,
                h.revoked,
                h.waiting_for_review
            );
            match h.problem(now, hours, key_days) {
                Some(p) => Err(format!("yard: {p}")),
                None => Ok(()),
            }
        }
        ["revoke", config, id, version, reason @ ..] if !reason.is_empty() => {
            let (id, version) = id_version(id, version)?;
            open(config)?.revoke(&id, &version, &reason.join(" "), http::now()).map_err(|e| e.to_string())?;
            println!("Revoked {id} {version}; clients stop it on their next index fetch");
            Ok(())
        }
        ["immediate", config, id, version] => {
            let (id, version) = id_version(id, version)?;
            open(config)?.clear_for_immediate_update(&id, &version, http::now()).map_err(|e| e.to_string())?;
            println!("Clients may update to {id} {version} at once");
            Ok(())
        }
        ["review", "list", config] => {
            let queue = open(config)?.review_queue().map_err(|e| e.to_string())?;
            if queue.is_empty() {
                println!("Nothing waits for review.");
            }
            for w in queue {
                println!("{} {}  published by {}  (submitted at {}, Unix time)", w.id, w.version, w.publisher, w.submitted);
                for line in &w.permissions {
                    println!("    • {line}");
                }
                for (target, path) in &w.packages {
                    println!("    {target}: {}", path.display());
                }
            }
            Ok(())
        }
        ["review", "approve", config, id, version] => {
            let (id, version) = id_version(id, version)?;
            open(config)?.approve(&id, &version, http::now()).map_err(|e| e.to_string())?;
            println!("Approved {id} {version}: it is listed now, and its publisher's later native programs are listed without review");
            Ok(())
        }
        ["review", "reject", config, id, version, reason @ ..] if !reason.is_empty() => {
            let (id, version) = id_version(id, version)?;
            open(config)?.reject(&id, &version, &reason.join(" ")).map_err(|e| e.to_string())?;
            println!("Rejected {id} {version}; it will not be listed and its files are deleted");
            Ok(())
        }
        ["-h" | "--help" | "help"] => {
            println!("{HELP}");
            Ok(())
        }
        ["-V" | "--version"] => {
            println!("yard {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err(HELP.into()),
    }
}
