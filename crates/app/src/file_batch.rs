//! Ordered, cancellable filesystem batches. The agent owns exclusive copy/move semantics.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use host::{Host, HostError};

#[derive(Clone)]
pub struct Clipboard {
    pub host: Arc<dyn Host>,
    pub paths: Vec<String>,
    pub cut: bool,
}

#[derive(Clone)]
pub enum Operation {
    Copy(String),
    Move(String),
    Delete { trash: bool },
}

#[derive(Default)]
pub struct Progress {
    pub cancel: AtomicBool,
    pub completed: AtomicUsize,
    pub total: usize,
}

pub struct Completed {
    pub source: String,
    pub target: Option<String>,
    pub edits: Vec<serde_json::Value>,
    pub is_file: bool,
}

#[derive(Default)]
pub struct Outcome {
    pub completed: Vec<Completed>,
    pub failed: Vec<(String, String)>,
    pub remaining: Vec<String>,
    pub warnings: Vec<String>,
}

struct Failure {
    message: String,
    stop: bool,
    cancelled: bool,
}

impl From<HostError> for Failure {
    fn from(error: HostError) -> Self {
        Self {
            stop: matches!(error, HostError::Disconnected(_) | HostError::Uncertain(_)),
            cancelled: matches!(error, HostError::Op(proto::Error::Cancelled)),
            message: error.to_string(),
        }
    }
}

impl Failure {
    fn message(message: String) -> Self {
        Self { message, stop: false, cancelled: false }
    }
}

/// Preserve visible order; select a parent only once even when its children are marked.
/// Reject roots and traversal before handing any path to a destructive operation.
pub fn top_level(paths: Vec<String>, roots: &[String]) -> Vec<String> {
    let valid = |p: &str| {
        crate::ui::is_absolute_path(p)
            && !p.contains('\0')
            && !p.split('/').any(|s| s == ".." || s == ".")
            && !p.trim_end_matches('/').is_empty()
            && !p.trim_end_matches('/').ends_with(':')
            && !roots.iter().any(|r| r.trim_end_matches('/') == p.trim_end_matches('/') || r.starts_with(&format!("{}/", p.trim_end_matches('/'))))
    };
    let paths: Vec<_> = paths.into_iter().filter(|p| valid(p)).map(|p| p.trim_end_matches('/').to_string()).collect();
    let all: HashSet<_> = paths.iter().cloned().collect();
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|p| {
            if !seen.insert(p.clone()) {
                return false;
            }
            let mut parent = p.as_str();
            while let Some((up, _)) = parent.rsplit_once('/') {
                if all.contains(up) {
                    return false;
                }
                parent = up;
            }
            true
        })
        .collect()
}

pub fn run(host: &dyn Host, paths: &[String], op: &Operation, progress: &Progress, clients: &[Arc<crate::lsp::Client>]) -> Outcome {
    let mut out = Outcome::default();
    for (i, source) in paths.iter().enumerate() {
        if progress.cancel.load(Ordering::Acquire) {
            out.remaining.extend_from_slice(&paths[i..]);
            break;
        }
        let mut edits = Vec::new();
        let result = match op {
            Operation::Copy(dir) => copy(host, source, dir, &progress.cancel).map(Some),
            Operation::Move(dir) => {
                let target = crate::ui::join(dir, crate::ui::file_name(source));
                if dir == source || dir.starts_with(&format!("{source}/")) {
                    Err(Failure::message(crate::i18n::t("Can't move a folder into itself.").to_string()))
                } else if &target == source {
                    out.remaining.push(source.clone());
                    progress.completed.fetch_add(1, Ordering::Release);
                    continue;
                } else {
                    if host.stat(&target).is_err() {
                        let params = serde_json::json!({"files": [{"oldUri": crate::lsp::path_to_uri(source), "newUri": crate::lsp::path_to_uri(&target)}]});
                        for client in clients {
                            match client.request("workspace/willRenameFiles", params.clone(), std::time::Duration::from_secs(5)) {
                                Ok(v) if !v.is_null() => edits.push(crate::lsp::remap_edit_uris(&v, source, &target)),
                                Ok(_) => {}
                                Err(e) => out.warnings.push(format!("willRenameFiles ({}): {e}", client.config.language)),
                            }
                        }
                    }
                    if progress.cancel.load(Ordering::Acquire) {
                        out.remaining.extend_from_slice(&paths[i..]);
                        break;
                    }
                    host.rename(source, &target).map(|()| Some(target)).map_err(Failure::from)
                }
            }
            Operation::Delete { trash: true } => {
                crate::trash::move_to_trash(std::path::Path::new(source)).map(|_| None).map_err(|e| Failure::message(e.to_string()))
            }
            Operation::Delete { trash: false } => host.delete(source).map(|()| None).map_err(Failure::from),
        };
        match result {
            Ok(target) => {
                let is_file = matches!(op, Operation::Copy(_)) && target.as_ref().is_some_and(|t| host.stat(t).is_ok_and(|m| m.kind == proto::Kind::File));
                out.completed.push(Completed { source: source.clone(), target, edits, is_file });
            }
            Err(error) if error.cancelled => {
                out.remaining.extend_from_slice(&paths[i..]);
                break;
            }
            Err(error) => {
                out.failed.push((source.clone(), error.message));
                if error.stop {
                    out.remaining.extend_from_slice(&paths[i + 1..]);
                    break;
                }
            }
        }
        progress.completed.fetch_add(1, Ordering::Release);
    }
    out
}

fn copy(host: &dyn Host, source: &str, dir: &str, cancel: &AtomicBool) -> Result<String, Failure> {
    if dir == source || dir.starts_with(&format!("{source}/")) {
        return Err(Failure::message(crate::i18n::t("Can't copy a folder into itself.").into()));
    }
    let name = crate::ui::file_name(source);
    for target in std::iter::once(crate::ui::join(dir, name)).chain((0..1000).map(|n| crate::ui::join(dir, &crate::workbench::copy_name(name, n)))) {
        match host.copy_path_cancellable(source, &target, cancel) {
            Ok(()) => return Ok(target),
            Err(HostError::Op(proto::Error::AlreadyExists(_))) => continue,
            Err(e) => return Err(Failure::from(e)),
        }
    }
    Err(Failure::message(crate::i18n::t("too many copies").to_string()))
}

#[cfg(test)]
#[path = "file_batch_tests.rs"]
mod tests;
