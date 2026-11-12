//! What the status bar says about rust-analyzer: its health, whether it is still loading, and the check it runs on
//! save. Plain data fed by `ServerEvent`s; the words are chosen here so a test can read them.

use std::collections::BTreeMap;

use crate::i18n::{t, tf};
use crate::lsp::{FlycheckStep, Health, ServerStatus};

/// One running check (`cargo check` or `clippy`) of one workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Check {
    title: String,
    /// The target being compiled (`demo (lib)`).
    target: Option<String>,
}

/// Everything known about one rust-analyzer process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerInfo {
    status: Option<ServerStatus>,
    checks: BTreeMap<String, Check>,
}

/// Which start of a language server is the current one for each slot. Events carry the start that sent them, so what an
/// old server says after a restart (its exit, a check it never ended) is dropped, whatever happened to it.
#[derive(Debug, Default)]
pub struct Starts {
    current: std::collections::HashMap<String, u64>,
    last: u64,
}

impl Starts {
    /// A new start for `key`: from now on only its events count.
    pub fn begin(&mut self, key: &str) -> u64 {
        self.last += 1;
        self.current.insert(key.to_string(), self.last);
        self.last
    }

    pub fn is_current(&self, key: &str, start: u64) -> bool {
        self.current.get(key) == Some(&start)
    }

    /// No start of `key` counts any more (the server was stopped on purpose).
    pub fn forget(&mut self, key: &str) {
        self.current.remove(key);
    }

    pub fn clear(&mut self) {
        self.current.clear();
    }
}

/// What a status bar item shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warning,
    Error,
}

impl ServerInfo {
    pub fn apply_status(&mut self, status: ServerStatus) {
        self.status = Some(status);
    }

    pub fn apply_flycheck(&mut self, token: String, step: FlycheckStep) {
        match step {
            FlycheckStep::Begin(title) => {
                self.checks.insert(token, Check { title, target: None });
            }
            FlycheckStep::Report(target) => {
                // A report that arrives after the end (or without a begin) is stale.
                if let Some(check) = self.checks.get_mut(&token) {
                    check.target = Some(target).filter(|t| !t.is_empty());
                }
            }
            FlycheckStep::End => {
                self.checks.remove(&token);
            }
        }
    }

    /// Whether the server has said anything yet; the status bar shows nothing for a server that never does.
    pub fn is_known(&self) -> bool {
        self.status.is_some() || !self.checks.is_empty()
    }

    pub fn health(&self) -> Health {
        self.status.as_ref().map_or(Health::Ok, |s| s.health)
    }

    pub fn loading(&self) -> bool {
        self.status.as_ref().is_some_and(|s| !s.quiescent)
    }

    pub fn checking(&self) -> bool {
        !self.checks.is_empty()
    }

    pub fn busy(&self) -> bool {
        self.loading() || self.checking()
    }

    pub fn message(&self) -> Option<&str> {
        self.status.as_ref().and_then(|s| s.message.as_deref())
    }

    pub fn tone(&self) -> Tone {
        match self.health() {
            Health::Ok => Tone::Ok,
            Health::Warning => Tone::Warning,
            Health::Error => Tone::Error,
        }
    }

    /// The running checks as one line: `cargo check: demo (lib)`.
    pub fn check_line(&self) -> Option<String> {
        let mut lines = self.checks.values().map(|c| match &c.target {
            Some(target) => format!("{}: {target}", c.title),
            None => c.title.clone(),
        });
        let first = lines.next()?;
        let more = lines.count();
        Some(if more == 0 { first } else { tf("{0} (+{1} more)", &[&first, &more]) })
    }

    /// The word on the status bar. A problem outranks activity.
    pub fn label(&self) -> String {
        match (self.health(), self.loading(), self.checking()) {
            (Health::Error, ..) => t("rust-analyzer: error").to_string(),
            (Health::Warning, ..) => t("rust-analyzer: warning").to_string(),
            (_, true, _) => t("rust-analyzer: loading").to_string(),
            (_, _, true) => t("rust-analyzer: checking").to_string(),
            _ => t("rust-analyzer: ready").to_string(),
        }
    }

    /// What a screen reader and the tooltip say: the state, the server's message and the running check.
    pub fn description(&self) -> String {
        let mut parts = vec![self.label()];
        parts.extend(self.message().map(str::to_string));
        parts.extend(self.check_line());
        parts.join(". ")
    }

    /// The details view: state, message, checks, what to do about it and the server's own report (`analyzerStatus`).
    pub fn details_markdown(&self, server: &str, analyzer_status: Option<&str>) -> String {
        let health = match self.health() {
            Health::Ok => t("OK"),
            Health::Warning => t("Warning"),
            Health::Error => t("Error"),
        };
        let activity = match (self.loading(), self.check_line()) {
            (true, _) => t("Loading the workspace or indexing").to_string(),
            (false, Some(check)) => check,
            (false, None) => t("Idle").to_string(),
        };
        let mut out = format!(
            "# {}\n\n- **{}** {server}\n- **{}** {health}\n- **{}** {activity}\n",
            t("rust-analyzer status"),
            t("Server:"),
            t("Health:"),
            t("Activity:")
        );
        if let Some(message) = self.message() {
            out.push_str(&format!("\n## {}\n\n{}\n", t("Message"), quote_block(message)));
        }
        out.push_str(&format!("\n## {}\n\n", t("What you can do")));
        for line in [
            t("Rust: Reload Workspace"),
            t("Rust: Rebuild Proc Macros"),
            t("Rust: Run Flycheck"),
            t("Rust: Cancel Flycheck"),
            t("Rust: Clear Flycheck"),
            t("Rust: Restart rust-analyzer"),
        ] {
            out.push_str(&format!("- {line}\n"));
        }
        if let Some(report) = analyzer_status.map(str::trim).filter(|r| !r.is_empty()) {
            out.push_str(&format!("\n## {}\n\n{}", t("Server report"), super::views::fenced(report)));
        }
        out
    }
}

/// A block of plain text as a Markdown quote, one `>` per line.
fn quote_block(text: &str) -> String {
    text.lines().map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(health: Health, quiescent: bool, message: Option<&str>) -> ServerStatus {
        ServerStatus { health, quiescent, message: message.map(str::to_string) }
    }

    #[test]
    fn only_the_current_start_of_a_slot_counts() {
        let mut starts = Starts::default();
        let old = starts.begin("rust@/p");
        assert!(starts.is_current("rust@/p", old));
        let new = starts.begin("rust@/p");
        assert!(!starts.is_current("rust@/p", old), "the old server's events are dropped after a restart");
        assert!(starts.is_current("rust@/p", new));
        assert!(!starts.is_current("rust@/other", new));
        starts.forget("rust@/p");
        assert!(!starts.is_current("rust@/p", new), "stopped on purpose: nothing counts until the next start");
        starts.clear();
    }

    #[test]
    fn a_server_that_said_nothing_is_not_shown() {
        let info = ServerInfo::default();
        assert!(!info.is_known());
        assert_eq!(info.label(), "rust-analyzer: ready");
    }

    #[test]
    fn loading_then_ready_then_checking() {
        let mut info = ServerInfo::default();
        info.apply_status(status(Health::Ok, false, None));
        assert_eq!((info.label().as_str(), info.busy()), ("rust-analyzer: loading", true));
        info.apply_status(status(Health::Ok, true, None));
        assert_eq!((info.label().as_str(), info.busy()), ("rust-analyzer: ready", false));
        info.apply_flycheck("f0".into(), FlycheckStep::Begin("cargo check".into()));
        assert_eq!(info.label(), "rust-analyzer: checking");
        assert_eq!(info.check_line().as_deref(), Some("cargo check"));
        info.apply_flycheck("f0".into(), FlycheckStep::Report("demo (lib)".into()));
        assert_eq!(info.check_line().as_deref(), Some("cargo check: demo (lib)"));
        info.apply_flycheck("f0".into(), FlycheckStep::End);
        assert_eq!((info.label().as_str(), info.busy(), info.check_line()), ("rust-analyzer: ready", false, None));
    }

    #[test]
    fn a_report_after_the_end_does_not_bring_the_check_back() {
        let mut info = ServerInfo::default();
        info.apply_flycheck("f0".into(), FlycheckStep::Report("late".into()));
        assert!(!info.checking());
        info.apply_flycheck("f0".into(), FlycheckStep::Begin("clippy".into()));
        info.apply_flycheck("f0".into(), FlycheckStep::End);
        info.apply_flycheck("f0".into(), FlycheckStep::Report("late".into()));
        assert!(!info.checking());
    }

    #[test]
    fn two_workspaces_checking_are_counted() {
        let mut info = ServerInfo::default();
        info.apply_flycheck("f0".into(), FlycheckStep::Begin("cargo check".into()));
        info.apply_flycheck("f1".into(), FlycheckStep::Begin("cargo check".into()));
        assert_eq!(info.check_line().as_deref(), Some("cargo check (+1 more)"));
        info.apply_flycheck("f0".into(), FlycheckStep::End);
        assert_eq!(info.check_line().as_deref(), Some("cargo check"));
    }

    #[test]
    fn a_problem_outranks_activity_and_the_message_is_read_aloud() {
        let mut info = ServerInfo::default();
        info.apply_status(status(Health::Error, false, Some("Failed to discover workspace.")));
        assert_eq!((info.label().as_str(), info.tone()), ("rust-analyzer: error", Tone::Error));
        assert_eq!(info.description(), "rust-analyzer: error. Failed to discover workspace.");
        info.apply_status(status(Health::Warning, true, Some("Proc macros are disabled.")));
        assert_eq!((info.label().as_str(), info.tone()), ("rust-analyzer: warning", Tone::Warning));
    }

    #[test]
    fn the_details_view_holds_state_message_and_the_servers_report() {
        let mut info = ServerInfo::default();
        info.apply_status(status(Health::Warning, true, Some("line one\nline two")));
        let text = info.details_markdown("rust@/work/demo", Some("Workspaces:\nLoaded 27 packages\n"));
        assert!(text.starts_with("# rust-analyzer status\n"));
        assert!(text.contains("- **Server:** rust@/work/demo\n- **Health:** Warning\n- **Activity:** Idle\n"), "{text}");
        assert!(text.contains("> line one\n> line two\n"), "{text}");
        assert!(text.contains("- Rust: Reload Workspace\n"));
        assert!(text.contains("## Server report\n\n```text\nWorkspaces:\nLoaded 27 packages\n```"), "{text}");
        let plain = ServerInfo::default().details_markdown("rust", None);
        assert!(!plain.contains("Server report") && !plain.contains("## Message"));
    }
}
