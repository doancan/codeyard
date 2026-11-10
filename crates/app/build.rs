//! Record the SHA-256 of the remote agents published with a Windows or macOS build. The agents themselves are not shipped:
//! a server downloads them from this version's GitHub release (or the app does, and copies them over ssh), and they run only
//! when their hash is the one recorded here. Build scripts never download or compile tools.
// Included by `tests/agent_build.rs`, where `main` is not called.
#![cfg_attr(test, allow(dead_code))]

use std::path::{Path, PathBuf};

const LINUX: [(&str, u16); 2] = [("x86_64-unknown-linux-musl", 62), ("aarch64-unknown-linux-musl", 183)];

fn validate_linux(bytes: &[u8], machine: u16) -> Result<(), String> {
    if bytes.len() < 64 || &bytes[..6] != b"\x7fELF\x02\x01" {
        return Err("expected a 64-bit little-endian ELF executable".into());
    }
    if u16::from_le_bytes([bytes[18], bytes[19]]) != machine {
        return Err("ELF CPU architecture does not match its filename".into());
    }
    Ok(())
}

// scripts/check-windows.ps1 compiles this file alone with `rustc --test`, without its build dependencies (like
// embed_resource below): the tests check what is recorded, not the hash function, so they use a stand-in.
#[cfg(not(test))]
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:064x}", bytes.len())
}

fn sources(dir: &Path, required: bool) -> Result<String, String> {
    // Watch the bundle as a directory so optional agents appearing/disappearing are detected.
    // Watching nonexistent optional files would force Cargo to rebuild the app on every invocation.
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries = Vec::new();
    for (triple, machine) in LINUX {
        let path = dir.join(format!("codeyard-agent-{triple}"));
        match std::fs::read(&path) {
            Ok(bytes) => {
                validate_linux(&bytes, machine).map_err(|e| format!("{}: {e}", path.display()))?;
                entries.push((triple, sha256_hex(&bytes)));
            }
            Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}. Run scripts/build-windows.ps1 to build a complete Windows app.", path.display())),
        }
    }
    // Release CI also supplies macOS agents built from the same checkout.
    for triple in ["aarch64-apple-darwin", "x86_64-apple-darwin"] {
        let path = dir.join(format!("codeyard-agent-{triple}"));
        if path.is_file() {
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            entries.push((triple, sha256_hex(&bytes)));
        }
    }
    let mut source = String::from("pub static AGENTS: &[host::ssh::PublishedAgent] = &[\n");
    for (triple, sha256) in entries {
        source.push_str(&format!("host::ssh::PublishedAgent {{ triple: {triple:?}, sha256: {sha256:?} }},\n"));
    }
    source.push_str("];");
    Ok(source)
}

fn main() {
    #[cfg(not(test))]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/windows/codeyard.rc");
        println!("cargo:rerun-if-changed=assets/windows/codeyard.ico");
        embed_resource::compile("assets/windows/codeyard.rc", embed_resource::NONE).manifest_required().expect("embed Windows application icon");
    }
    println!("cargo:rerun-if-env-changed=AGENTS_DIR");
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let source = if os == "windows" || os == "macos" {
        let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("../..");
        let dir = std::env::var_os("AGENTS_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target/agents"));
        // Windows release builds always need them. On macOS release builds also serve the memory budgets (memcheck.sh), so
        // the release workflow checks the finished binary instead (`--check-agent-bundle`).
        let required = os == "windows" && std::env::var("PROFILE").as_deref() == Ok("release");
        let source = sources(&dir, required).unwrap_or_else(|e| panic!("Cannot record the SSH agents: {e}"));
        if os == "windows" && !required && !LINUX.iter().all(|(triple, _)| dir.join(format!("codeyard-agent-{triple}")).is_file()) {
            println!("cargo:warning=SSH agents are incomplete; use scripts/build-windows.ps1 for a Windows app that can connect to servers.");
        }
        source
    } else {
        "pub static AGENTS: &[host::ssh::PublishedAgent] = &[];".into()
    };
    std::fs::write(out.join("bundled_agents.rs"), source).expect("write the published agent registry");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_windows_executables_and_mislabeled_linux_architectures() {
        assert!(validate_linux(b"MZ", 62).is_err());
        let mut elf = [0_u8; 64];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[18] = 62;
        assert!(validate_linux(&elf, 62).is_ok());
        assert!(validate_linux(&elf, 183).is_err());
        elf[4] = 1;
        assert!(validate_linux(&elf, 62).is_err());
    }

    #[test]
    fn production_builds_reject_missing_agent_assets() {
        let missing = std::env::temp_dir().join(format!("codeyard-no-agent-assets-{}", std::process::id()));
        assert!(sources(&missing, true).is_err());
        assert!(sources(&missing, false).unwrap().contains("= &[\n];"));
    }

    #[test]
    fn only_the_hash_is_recorded_never_the_bytes() {
        let dir = std::env::temp_dir().join(format!("codeyard-agent-hashes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (triple, machine) in LINUX {
            let mut elf = vec![0_u8; 64];
            elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
            elf[18..20].copy_from_slice(&machine.to_le_bytes());
            std::fs::write(dir.join(format!("codeyard-agent-{triple}")), &elf).unwrap();
        }
        let source = sources(&dir, true).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!source.contains("include_bytes"), "{source}");
        assert_eq!(source.matches("sha256: \"").count(), 2, "{source}");
    }
}
