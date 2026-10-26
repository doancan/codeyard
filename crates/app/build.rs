//! Embed remote agents in Windows executables. Build scripts never download or compile tools.
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
                entries.push((triple, path));
            }
            Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}. Run scripts/build-windows.ps1 to build a complete Windows app.", path.display())),
        }
    }
    // Release CI also supplies macOS agents built from the same checkout.
    for triple in ["aarch64-apple-darwin", "x86_64-apple-darwin"] {
        let path = dir.join(format!("codeyard-agent-{triple}"));
        if path.is_file() {
            entries.push((triple, path));
        }
    }
    let mut source = String::from("pub static AGENTS: &[host::ssh::EmbeddedAgent] = &[\n");
    for (triple, path) in entries {
        let path = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
        source.push_str(&format!("host::ssh::EmbeddedAgent {{ triple: {triple:?}, bytes: include_bytes!({:?}) }},\n", path.to_string_lossy()));
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
    let source = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("../..");
        let dir = std::env::var_os("AGENTS_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target/agents"));
        let required = std::env::var("PROFILE").as_deref() == Ok("release");
        let source = sources(&dir, required).unwrap_or_else(|e| panic!("Cannot embed SSH agents: {e}"));
        if !required && !LINUX.iter().all(|(triple, _)| dir.join(format!("codeyard-agent-{triple}")).is_file()) {
            println!("cargo:warning=SSH agents are incomplete; use scripts/build-windows.ps1 for a standalone Windows app.");
        }
        source
    } else {
        "pub static AGENTS: &[host::ssh::EmbeddedAgent] = &[];".into()
    };
    std::fs::write(out.join("bundled_agents.rs"), source).expect("write embedded agent registry");
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
}
