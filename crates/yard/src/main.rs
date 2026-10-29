//! `yard`: run the registry's publish service, or act as its security role.
//!
//! ```text
//! yard init FOLDER [--registry NAME]           a new registry: keys, root.json, yard.json
//! yard serve CONFIG                            answer publishers; sign the index now and daily
//! yard resign CONFIG                           sign a fresh index once (for a scheduler instead of serve's timer)
//! yard revoke CONFIG ID VERSION REASON…        stop a version on every client, with the reason users see
//! yard immediate CONFIG ID VERSION             let clients update to an advisory fix at once
//! ```

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use ext_manifest::{ExtId, Version};
use yard::{Config, Yard, http};

const HELP: &str = "yard: the Codeyard extension registry

usage:
  yard init FOLDER [--registry NAME]       create a registry (keys, root.json, yard.json)
  yard serve CONFIG                        run the publish service
  yard resign CONFIG                       sign a fresh index once
  yard revoke CONFIG ID VERSION REASON…    revoke a version everywhere
  yard immediate CONFIG ID VERSION         let clients update to an advisory fix at once

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
            let registry = match rest {
                [] => "localhost",
                ["--registry", name] => name,
                _ => return Err(HELP.into()),
            };
            let cfg = yard::init(Path::new(dir), registry, yard::SIGSTORE_FULCIO, yard::SIGSTORE_REKOR, http::now())?;
            println!(
                "Created {}\nThe root is signed by one development key (keys/root.key); its public key is in root-key.txt.\n\
                 For production, have the root key holders sign root.json with `cyx yard sign root` instead.\n\nNext:\n  yard serve {}",
                cfg.display(),
                cfg.display()
            );
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
