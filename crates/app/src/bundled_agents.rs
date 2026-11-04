//! The remote agents of this build: on Windows and macOS only their SHA-256 travels inside the executable (see build.rs);
//! the bytes come from this version's GitHub release, fetched by the server or, without internet access there, by the app.
include!(concat!(env!("OUT_DIR"), "/bundled_agents.rs"));

/// This version's release download folder (`<repository>/releases/download/v<version>`), where the agents are published.
pub fn release_url() -> String {
    format!("{}/releases/download/v{}", env!("CARGO_PKG_REPOSITORY"), env!("CARGO_PKG_VERSION"))
}

pub fn discover() -> host::ssh::AgentBundle {
    host::ssh::AgentBundle::discover().with_published(release_url(), AGENTS)
}

/// Packaging smoke test: checks only the recorded hashes, never the filesystem or the network.
pub fn check() -> Result<(), String> {
    for triple in ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"] {
        let agent = AGENTS.iter().find(|a| a.triple == triple).ok_or_else(|| format!("missing SSH agent hash: {triple}"))?;
        if agent.sha256.len() != 64 || !agent.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            return Err(format!("invalid SSH agent hash: {triple}"));
        }
    }
    Ok(())
}

#[cfg(all(test, any(windows, target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn published_agents_are_complete_and_available_to_connections() {
        // Ordinary debug cargo builds without assets are supported for UI-only development.
        if AGENTS.is_empty() && cfg!(debug_assertions) {
            return;
        }
        check().unwrap();
        let bundle = discover();
        for agent in AGENTS {
            assert!(bundle.supports(agent.triple));
            assert_eq!(bundle.source(agent.triple).unwrap().unwrap().sha256(), agent.sha256);
        }
    }

    #[test]
    fn agents_come_from_this_versions_release() {
        assert_eq!(release_url(), format!("https://github.com/doancan/codeyard/releases/download/v{}", env!("CARGO_PKG_VERSION")));
    }
}
