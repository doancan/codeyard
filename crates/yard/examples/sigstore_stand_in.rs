//! A stand-in Sigstore for rehearsing the registry without the network (`deploy/rehearse.sh`). Never deployed: it is
//! built from the test authority in `ext_core::testing`, which no release compiles.
//!
//! ```text
//! sigstore_stand_in serve DIR              write DIR/fulcio.pem and DIR/rekor.pem (for `yard init --fulcio --rekor`),
//!                                          serve Fulcio and Rekor on a local port, write its URL to DIR/url
//! sigstore_stand_in token OWNER REPO       an identity token for the GitHub publisher with these numeric ids
//!                                          (for SIGSTORE_ID_TOKEN; only the stand-in accepts it)
//! ```

use std::sync::Arc;

use ext_core::testing::{Authority, github, identity_token, serve_sigstore};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["serve", dir] => {
            let dir = std::path::Path::new(dir);
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let authority = Arc::new(Authority::new());
            std::fs::write(dir.join("fulcio.pem"), authority.fulcio_pem()).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("rekor.pem"), authority.rekor_pem()).map_err(|e| e.to_string())?;
            let url = serve_sigstore(authority);
            std::fs::write(dir.join("url"), format!("{url}\n")).map_err(|e| e.to_string())?;
            println!("stand-in Sigstore on {url}");
            loop {
                std::thread::park();
            }
        }
        ["token", owner, repo] => {
            let n = |s: &str| s.parse::<u64>().map_err(|_| format!("'{s}' is not a number"));
            println!("{}", identity_token("rehearsal", &github(n(owner)?, n(repo)?)));
            Ok(())
        }
        _ => Err("usage: sigstore_stand_in serve DIR | token OWNER_ID REPO_ID".into()),
    }
}
