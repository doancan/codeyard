//! Remote agent bytes travel inside the Windows executable and its signed update.
include!(concat!(env!("OUT_DIR"), "/bundled_agents.rs"));

pub fn discover() -> host::ssh::AgentBundle {
    host::ssh::AgentBundle::discover().with_embedded(AGENTS)
}

/// Packaging smoke test: checks only embedded bytes, never the filesystem or the network.
pub fn check() -> Result<(), String> {
    for (triple, machine) in [("x86_64-unknown-linux-musl", 62_u16), ("aarch64-unknown-linux-musl", 183_u16)] {
        let agent = AGENTS.iter().find(|a| a.triple == triple).ok_or_else(|| format!("missing embedded SSH agent: {triple}"))?;
        if agent.bytes.len() < 64 || &agent.bytes[..6] != b"\x7fELF\x02\x01" || u16::from_le_bytes([agent.bytes[18], agent.bytes[19]]) != machine {
            return Err(format!("invalid embedded SSH agent: {triple}"));
        }
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn windows_embedded_agents_are_complete_and_available_to_connections() {
        // Ordinary debug cargo builds without assets are supported for UI-only development.
        if AGENTS.is_empty() && cfg!(debug_assertions) {
            return;
        }
        check().unwrap();
        let bundle = discover();
        for agent in AGENTS {
            assert!(bundle.supports(agent.triple));
            assert!(!bundle.binary(agent.triple).unwrap().unwrap().is_empty());
        }
    }
}
