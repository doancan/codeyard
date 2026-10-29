//! Building a code extension's component: plain cargo for `wasm32-wasip2` (TypeScript and JavaScript: typescript.rs),
//! the result written to `extension.wasm` in the extension's folder, where packaging, `cyx test` and `cyx dev` find it.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ext_manifest::{CodeLanguage, Manifest};

pub const TARGET: &str = "wasm32-wasip2";

/// The manifest of the extension in `dir`, read leniently (`cyx check` reports its problems).
pub fn manifest(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join(ext_manifest::FILE_NAME);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("error: cannot read {}: {e}", path.display()))?;
    ext_manifest::parse_with(&text, ext_manifest::Options { require_why: false })
        .map_err(|_| "error: the manifest has problems; `cyx check` lists them".to_string())
}

/// The cargo to run: `CARGO` when cyx itself runs under cargo, else `cargo` on PATH.
fn cargo() -> PathBuf {
    std::env::var_os("CARGO").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cargo"))
}

/// Whether the Rust toolchain on PATH can build for `wasm32-wasip2`: its standard library for the target is installed.
pub fn target_installed() -> bool {
    let rustc = std::env::var_os("RUSTC").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("rustc"));
    Command::new(rustc)
        .args(["--print", "target-libdir", "--target", TARGET])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .is_some_and(|d| {
            d.join("libstd.rlib").is_file()
                || std::fs::read_dir(&d).is_ok_and(|mut it| it.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("libstd-"))))
        })
}

/// Build the component of the extension in `dir` if it has code; returns where `extension.wasm` was written.
pub fn build(dir: &Path) -> Result<Option<PathBuf>, String> {
    build_in(dir, None)
}

/// `build` with cargo's output in `target_dir` instead of the crate's own `target/`.
pub fn build_in(dir: &Path, target_dir: Option<&Path>) -> Result<Option<PathBuf>, String> {
    // An invalid manifest is not built; `cyx check` (run next by every caller) reports it in full.
    let Ok(m) = manifest(dir) else { return Ok(None) };
    let Some(code) = &m.code else { return Ok(None) };
    if code.language != CodeLanguage::Rust {
        return crate::typescript::build(dir, &m, &code.entry).map(Some);
    }
    if !target_installed() {
        return Err(format!(
            "error: the Rust target {TARGET} is not installed\n   = fix: rustup target add {TARGET}   (or install the target's standard library with your Rust distribution)"
        ));
    }
    let crate_manifest = dir.join(&code.entry);
    println!("Building {} for {TARGET}…", m.id);
    let mut cmd = Command::new(cargo());
    cmd.args(["build", "--release", "--target", TARGET, "--message-format=json-render-diagnostics", "--manifest-path"]).arg(&crate_manifest);
    if let Some(t) = target_dir {
        cmd.arg("--target-dir").arg(t);
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("error: cannot run cargo: {e}\n   = fix: install Rust from https://rustup.rs"))?;
    let stdout = child.stdout.take().ok_or("error: cargo has no output")?;
    let mut wasm = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(path) = artifact(&line) {
            wasm = Some(path);
        }
    }
    let status = child.wait().map_err(|e| format!("error: cargo: {e}"))?;
    if !status.success() {
        return Err("error: the build failed (cargo's errors are above)".into());
    }
    let wasm = wasm.ok_or_else(|| {
        format!("error: the build made no .wasm library\n   = fix: set `crate-type = [\"cdylib\"]` under [lib] in {}", crate_manifest.display())
    })?;
    let out = dir.join(ext_pkg::COMPONENT);
    std::fs::copy(&wasm, &out).map_err(|e| format!("error: cannot copy {} to {}: {e}", wasm.display(), out.display()))?;
    Ok(Some(out))
}

/// The `.wasm` of a cdylib in one line of cargo's JSON messages.
fn artifact(line: &str) -> Option<PathBuf> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v["reason"] != "compiler-artifact" || !v["target"]["crate_types"].as_array()?.iter().any(|t| t == "cdylib") {
        return None;
    }
    v["filenames"].as_array()?.iter().filter_map(|f| f.as_str()).find(|f| f.ends_with(".wasm")).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::artifact;

    #[test]
    fn finds_the_cdylib() {
        let line = r#"{"reason":"compiler-artifact","target":{"crate_types":["cdylib"]},"filenames":["/t/wasm32-wasip2/release/hello.wasm"]}"#;
        assert_eq!(artifact(line).unwrap().to_str(), Some("/t/wasm32-wasip2/release/hello.wasm"));
        assert!(artifact(r#"{"reason":"compiler-artifact","target":{"crate_types":["lib"]},"filenames":["/t/x.rlib"]}"#).is_none());
        assert!(artifact("not json").is_none());
    }
}
