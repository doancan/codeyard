//! Resolve automatically detected Rust launch programs from Cargo's build artifacts.

use std::collections::BTreeSet;

use serde_json::Value;

use super::StartError;

pub(super) fn build(host: &dyn host::Host, bin: &str, root: &str) -> Result<String, StartError> {
    let output = host
        .exec("cargo", &["build", "--message-format=json", "--bin", bin], Some(root), None)
        .map_err(|e| (crate::i18n::tf("Could not run “{}”: {}", &[&"cargo build", &e.to_string()]), String::new()))?;
    let (executables, diagnostics) = artifacts(&output.stdout_str(), bin);
    let diagnostics = format!("{diagnostics}{}", output.stderr_str());
    if !output.ok() {
        return Err((crate::i18n::tf("Build failed: {}", &[&"cargo build"]), diagnostics));
    }
    match (executables.len(), executables.into_iter().next()) {
        (1, Some(path)) => Ok(path),
        (count, _) => {
            let reason = format!("Cargo reported {count} executables for binary {bin}");
            Err((crate::i18n::tf("Could not start debugging: {}", &[&reason]), diagnostics))
        }
    }
}

fn artifacts(output: &str, bin: &str) -> (BTreeSet<String>, String) {
    let mut executables = BTreeSet::new();
    let mut diagnostics = String::new();
    for line in output.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            diagnostics.push_str(line);
            diagnostics.push('\n');
            continue;
        };
        if message["reason"] == "compiler-message" {
            if let Some(rendered) = message["message"]["rendered"].as_str() {
                diagnostics.push_str(rendered);
            }
        } else if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == bin
            && message["target"]["kind"].as_array().is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            && let Some(path) = message["executable"].as_str().filter(|path| !path.is_empty())
        {
            executables.insert(path.to_string());
        }
    }
    (executables, diagnostics)
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
            assert_eq!(artifacts(&output, "demo").0.into_iter().collect::<Vec<_>>(), [path], "executable {path}");
        }
    }

    #[test]
    fn cargo_diagnostics_remain_readable_for_the_problems_panel() {
        let rendered = "error[E0308]: mismatched types\n --> src/main.rs:2:5\n";
        let message = json!({"reason": "compiler-message", "message": {"rendered": rendered}});
        let (executables, diagnostics) = artifacts(&format!("{message}\n"), "demo");
        assert!(executables.is_empty());
        assert_eq!(diagnostics, rendered);
    }
}
