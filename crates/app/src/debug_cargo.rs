//! Resolve automatically detected Rust launch programs from Cargo's build artifacts.

use std::collections::BTreeSet;

use crate::cargo_project::CargoTarget;
use serde_json::Value;

use super::StartError;

pub(super) fn build(host: &dyn host::Host, target: &CargoTarget, root: &str) -> Result<String, StartError> {
    let args = target.build_args();
    let command = if target.kind.is_test() { "cargo test" } else { "cargo build" };
    let output = host
        .exec("cargo", &args.iter().map(String::as_str).collect::<Vec<_>>(), Some(root), None)
        .map_err(|e| (crate::i18n::tf("Could not run “{}”: {}", &[&command, &e.to_string()]), String::new()))?;
    let (executables, diagnostics) = artifacts(&output.stdout_str(), target);
    let diagnostics = format!("{diagnostics}{}", output.stderr_str());
    if !output.ok() || !crate::cargo_project::build_finished(&output.stdout_str()) {
        return Err((crate::i18n::tf("Build failed: {}", &[&command]), diagnostics));
    }
    match (executables.len(), executables.into_iter().next()) {
        (1, Some(path)) => Ok(path),
        (count, _) => {
            let reason = crate::i18n::tf("Cargo reported {0} executables for target {1}", &[&count, &target.name]);
            Err((crate::i18n::tf("Could not start debugging: {}", &[&reason]), diagnostics))
        }
    }
}

fn artifacts(output: &str, target: &CargoTarget) -> (BTreeSet<String>, String) {
    let mut executables = BTreeSet::new();
    for line in output.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else { continue };
        if target.accepts_artifact(&message)
            && let Some(path) = message["executable"].as_str().filter(|path| !path.is_empty())
        {
            executables.insert(path.to_string());
        }
    }
    (executables, crate::cargo_project::diagnostics(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cargo_artifacts_preserve_custom_and_remote_executable_paths() {
        for path in ["/srv/custom target/debug/demo", "C:\\build outputs\\debug\\demo.exe"] {
            let messages = [
                json!({"reason": "compiler-artifact", "target": {"name": "dependency", "kind": ["bin"]}, "executable": "/wrong/dependency"}),
                json!({"reason": "compiler-artifact", "target": {"name": "demo", "kind": ["lib"]}, "executable": null}),
                json!({"reason": "compiler-artifact", "target": {"name": "demo", "kind": ["bin"]}, "executable": path}),
            ];
            let output = messages.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
            assert_eq!(artifacts(&output, &CargoTarget::binary("demo".into())).0.into_iter().collect::<Vec<_>>(), [path], "executable {path}");
        }
    }

    #[test]
    fn cargo_diagnostics_remain_readable_for_the_problems_panel() {
        let rendered = "error[E0308]: mismatched types\n --> src/main.rs:2:5\n";
        let message = json!({"reason": "compiler-message", "message": {"rendered": rendered}});
        let (executables, diagnostics) = artifacts(&format!("{message}\n"), &CargoTarget::binary("demo".into()));
        assert!(executables.is_empty());
        assert_eq!(diagnostics, rendered);
    }
}
