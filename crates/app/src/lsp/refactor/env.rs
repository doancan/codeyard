//! The plan's view of the world on a real host: what a path is (a link counts as itself), its bytes, and where it really is.
//! The open documents are a snapshot taken on the UI thread, because the plan is made on a background thread.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use host::{Host, HostError};

use super::plan::{Env, Kind, OpenDoc};

pub struct HostEnv {
    host: Arc<dyn Host>,
    open: HashMap<String, OpenDoc>,
    /// Resolved paths already asked for (`prepare` fills it with one request for many paths).
    resolved: Mutex<HashMap<String, Option<String>>>,
}

impl HostEnv {
    pub fn new(host: Arc<dyn Host>, open: HashMap<String, OpenDoc>) -> Self {
        HostEnv { host, open, resolved: Mutex::new(HashMap::new()) }
    }
}

/// `real` as the rest of the app writes paths: `/` separators and no `\\?\` prefix of the Windows API.
fn tidy(real: &str) -> String {
    let real = real.strip_prefix(r"\\?\UNC\").map(|rest| format!(r"\\{rest}")).unwrap_or_else(|| real.strip_prefix(r"\\?\").unwrap_or(real).to_string());
    real.replace('\\', "/")
}

impl Env for HostEnv {
    fn open_doc(&self, path: &str) -> Option<&OpenDoc> {
        self.open.get(path)
    }

    fn kind(&self, path: &str) -> Kind {
        match self.host.lstat(path) {
            Ok(meta) => match meta.kind {
                proto::Kind::File | proto::Kind::Symlink => Kind::File,
                proto::Kind::Dir => Kind::Dir,
                proto::Kind::Other => Kind::Other,
            },
            Err(HostError::Op(proto::Error::NotFound(_))) => Kind::Missing,
            // Unknown (the connection dropped, no permission): never treated as free to create over.
            Err(_) => Kind::Other,
        }
    }

    fn read(&self, path: &str) -> Result<(Vec<u8>, u64), String> {
        self.host.read(path).map_err(|e| e.to_string())
    }

    fn prepare(&self, paths: &[String]) {
        let Ok(answers) = self.host.real_paths(paths.to_vec()) else { return };
        let mut cache = self.resolved.lock().unwrap_or_else(|e| e.into_inner());
        for (path, answer) in paths.iter().zip(answers) {
            cache.insert(path.clone(), answer.ok().map(|real| tidy(&real)));
        }
    }

    fn resolve(&self, path: &str) -> String {
        // The nearest part that exists is resolved; the part below it does not exist yet, so it has no links.
        let mut tail: Vec<&str> = Vec::new();
        let mut current = path;
        loop {
            let known = self.resolved.lock().unwrap_or_else(|e| e.into_inner()).get(current).cloned();
            let answer = match known {
                Some(answer) => answer,
                None => {
                    let answer = self.host.real_paths(vec![current.to_string()]).ok().and_then(|mut a| a.pop()).and_then(|r| r.ok()).map(|real| tidy(&real));
                    self.resolved.lock().unwrap_or_else(|e| e.into_inner()).insert(current.to_string(), answer.clone());
                    answer
                }
            };
            if let Some(real) = answer {
                let mut out = real;
                for part in tail.iter().rev() {
                    out = format!("{}/{part}", out.trim_end_matches('/'));
                }
                return out;
            }
            match current.trim_end_matches(['/', '\\']).rsplit_once(['/', '\\']) {
                Some((parent, name)) if !parent.is_empty() => {
                    tail.push(name);
                    current = parent;
                }
                // Nothing on the way exists (or the path is relative): the path as given.
                _ => return super::plan::clean(path),
            }
        }
    }
}

#[cfg(test)]
#[path = "env_tests.rs"]
mod tests;
