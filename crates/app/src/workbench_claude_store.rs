//! Local agent profiles and Claude Code's per-project history locations.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Provider {
    #[default]
    Claude,
    Codex,
    Antigravity,
}

impl Provider {
    pub(super) const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Antigravity];

    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Antigravity => "Antigravity CLI",
        }
    }

    pub(super) fn credentials_env(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[
                "CLAUDECODE",
                "CLAUDE_CODE_ENTRYPOINT",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_PROFILE",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
                "CLAUDE_CODE_OAUTH_SCOPES",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
            ],
            Self::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN", "CODEX_THREAD_ID", "OPENAI_BASE_URL"],
            Self::Antigravity => &[],
        }
    }

    pub(super) fn uses_local_profiles(self) -> bool {
        self != Self::Antigravity || cfg!(target_os = "macos")
    }

    pub(super) fn executable(self) -> Option<PathBuf> {
        let name = if self == Self::Antigravity { "agy" } else { self.key() };
        let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut extra: Vec<_> = dirs::home_dir().map(|home| home.join(".local").join("bin")).into_iter().collect();
        if cfg!(target_os = "macos") {
            extra.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
        }
        #[cfg(windows)]
        if self == Self::Antigravity
            && let Some(local) = dirs::data_local_dir()
        {
            extra.push(local.join("agy").join("bin"));
        }
        std::env::split_paths(&path).chain(extra).filter(|dir| dir.is_absolute()).map(|dir| dir.join(&file)).find(|path| is_executable(path))
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    metadata.is_file()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum HistoryMode {
    #[default]
    Private,
    Shared,
}

impl HistoryMode {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Shared => "shared",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Project {
    pub(super) provider: Provider,
    store: PathBuf,
    pub(super) folder: PathBuf,
    key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Session {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) path: Option<PathBuf>,
}

impl Project {
    pub(super) fn new(data: &Path, folder: &Path, provider: Provider) -> io::Result<Self> {
        let folder = folder.canonicalize()?;
        if !folder.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotADirectory, "The agent needs a project folder"));
        }
        let key = crate::update::sha256_hex(folder.as_os_str().as_encoded_bytes());
        let store = data.join("agents").join(provider.key());
        Ok(Self { provider, store, folder, key })
    }

    pub(super) fn state_key(&self) -> String {
        format!("agents.{}.project.{}", self.provider.key(), self.key)
    }

    /// The folder that holds one account's sessions and settings.
    pub(super) fn profile(&self, profile: u32) -> PathBuf {
        self.store.join("profiles").join(profile.to_string())
    }

    fn project_name(&self, mode: HistoryMode) -> String {
        // Claude ignores names longer than 64 characters and uses the working directory instead.
        format!("{}-{}", &self.key[..56], mode.key())
    }

    fn history(&self, profile: u32, mode: HistoryMode) -> PathBuf {
        match mode {
            HistoryMode::Private => self.profile(profile).join("projects").join(self.project_name(mode)),
            HistoryMode::Shared => self.store.join("shared").join(&self.key),
        }
    }

    /// Prepare only Codeyard-owned paths. The profile path has the same identity in either history mode.
    pub(super) fn prepare(&self, profile: u32, mode: HistoryMode) -> io::Result<Vec<(String, String)>> {
        let config = self.profile(profile);
        if self.provider == Provider::Codex {
            secure_dir(&config)?;
            return Ok(super::codex_store::env(&config.canonicalize()?));
        }
        if self.provider == Provider::Antigravity {
            return Ok(Vec::new());
        }
        let projects = config.join("projects");
        secure_dir(&projects)?;
        let config = config.canonicalize()?;
        let name = self.project_name(mode);
        let history = self.history(profile, mode);
        secure_dir(&history)?;
        let history = history.canonicalize()?;
        if mode == HistoryMode::Shared {
            ensure_link(&config.join("projects").join(&name), &history)?;
        }
        Ok(vec![("CLAUDE_CONFIG_DIR".into(), config.to_string_lossy().into_owned()), ("CLAUDE_CODE_PROJECT_DIR_NAME".into(), name)])
    }

    /// Protect the project's history or Antigravity's shared account while a terminal is live.
    pub(super) fn lock(&self) -> io::Result<File> {
        let locks = if self.provider == Provider::Antigravity && self.provider.uses_local_profiles() {
            dirs::cache_dir().ok_or_else(|| io::Error::other("No local cache directory")).map(|cache| cache.join("com.codeyard.app").join("antigravity"))?
        } else {
            self.store.join("locks")
        };
        secure_dir(&locks)?;
        let name = if self.provider == Provider::Antigravity { "account" } else { &self.key };
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(locks.join(name))?;
        file.try_lock()?;
        Ok(file)
    }

    pub(super) fn antigravity_accounts(&self) -> super::antigravity_accounts::Accounts {
        let data = self.store.parent().and_then(Path::parent).unwrap_or(&self.store);
        let scope = data.canonicalize().unwrap_or_else(|_| data.to_path_buf());
        super::antigravity_accounts::Accounts::new(&crate::update::sha256_hex(scope.as_os_str().as_encoded_bytes()))
    }

    pub(super) fn sessions(&self, profile: u32, mode: HistoryMode) -> io::Result<Vec<Session>> {
        if self.provider == Provider::Codex {
            return super::codex_store::sessions(&self.profile(profile), &self.folder);
        }
        if self.provider == Provider::Antigravity {
            if self.provider.uses_local_profiles() && !self.antigravity_accounts().can_list(profile)? {
                return Ok(Vec::new());
            }
            return super::antigravity_store::sessions(&self.folder);
        }
        self.sessions_in(&self.history(profile, mode))
    }

    fn legacy_history(&self, profile: u32) -> Option<PathBuf> {
        let name: String = crate::ui::local_path(&self.folder).chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' }).collect();
        (name.len() <= 200).then(|| self.profile(profile).join("projects").join(name))
    }

    pub(super) fn recoverable_sessions(&self, profile: u32, mode: HistoryMode) -> io::Result<Vec<Session>> {
        if self.provider != Provider::Claude {
            return Ok(Vec::new());
        }
        let Some(legacy) = self.legacy_history(profile) else { return Ok(Vec::new()) };
        let mut sessions = self.sessions_in(&legacy)?;
        sessions.retain(|session| !self.has_session(profile, mode, &session.id));
        Ok(sessions)
    }

    pub(super) fn recover_session(&self, profile: u32, mode: HistoryMode, id: &str) -> io::Result<()> {
        let missing = || io::Error::new(io::ErrorKind::NotFound, "Claude Code session not found");
        let legacy = self.legacy_history(profile).filter(|_| valid_session_id(id)).ok_or_else(missing)?;
        let name = format!("{id}.jsonl");
        let source = legacy.join(&name);
        if !source.symlink_metadata().is_ok_and(|metadata| metadata.is_file()) {
            return Err(missing());
        }
        let history = self.history(profile, mode);
        let dest = history.join(name);
        if dest.is_file() {
            return Ok(());
        }

        // Publish a complete copy and leave the original in the account's own history.
        let nonce = new_session_id().map_err(|_| io::Error::other("Could not create a temporary session file"))?;
        let temp = history.join(format!(".recover-{nonce}.tmp"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut output = options.open(&temp)?;
        let result = (|| {
            io::copy(&mut File::open(source)?, &mut output)?;
            output.sync_all()?;
            drop(output);
            fs::hard_link(&temp, &dest)
        })();
        let _ = fs::remove_file(&temp);
        result
    }

    fn sessions_in(&self, path: &Path) -> io::Result<Vec<Session>> {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|id| id.to_str()).filter(|id| valid_session_id(id)) else { continue };
            let modified = entry.metadata().and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            paths.push((modified, id.to_owned(), path));
        }
        paths.sort_unstable_by(|a, b| (b.0, &b.1).cmp(&(a.0, &a.1)));
        Ok(paths
            .into_iter()
            .take(100)
            .map(|(_, id, path)| Session { title: transcript_title(&path).unwrap_or_else(|| id[..8].to_owned()), id, path: None })
            .collect())
    }

    pub(super) fn transcript(&self, profile: u32, mode: HistoryMode, session: &Session) -> io::Result<PathBuf> {
        let missing = || io::Error::new(io::ErrorKind::NotFound, "The selected session has no local transcript");
        if self.provider == Provider::Claude {
            return self.session_path(profile, mode, &session.id).ok_or_else(missing);
        }
        if self.provider == Provider::Antigravity {
            return Err(missing());
        }
        let root = self.profile(profile).join("sessions").canonicalize()?;
        let path = session.path.as_ref().filter(|path| path.is_absolute()).ok_or_else(missing)?.canonicalize()?;
        let valid_name = path.file_stem().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(&format!("-{}", session.id)));
        if !valid_session_id(&session.id) || !valid_name || !path.starts_with(root) || !path.is_file() {
            return Err(missing());
        }
        Ok(path)
    }

    pub(super) fn transfer_session(&self, profile: u32, target: u32, mode: HistoryMode, session: &Session) -> io::Result<Session> {
        let _lock = self.lock()?;
        let source = self.transcript(profile, mode, session)?;
        self.prepare(target, mode)?;
        let dest = if self.provider == Provider::Claude {
            self.history(target, mode).join(format!("{}.jsonl", session.id))
        } else {
            // Codex discovers its rollouts by their dated path under the account's own home.
            let root = self.profile(profile).join("sessions").canonicalize()?;
            let suffix = source.strip_prefix(root).map_err(io::Error::other)?;
            let home = self.profile(target).canonicalize()?;
            let target_root = home.join("sessions");
            let dest = target_root.join(suffix);
            let parent = dest.parent().ok_or_else(|| io::Error::other("Invalid Codex session path"))?;
            secure_dir(parent)?;
            if !parent.canonicalize()?.starts_with(&target_root) {
                return Err(io::Error::other("The destination is outside the selected Codex account"));
            }
            dest
        };
        super::agent_transfer::copy_transcript(&source, &dest)?;
        Ok(Session { path: Some(dest), ..session.clone() })
    }

    pub(super) fn watch_paths(&self, profile: u32, mode: HistoryMode, session: Option<&str>) -> Vec<(String, PathBuf)> {
        if self.provider == Provider::Codex {
            return super::codex_store::watch_paths(&self.profile(profile));
        }
        if self.provider == Provider::Antigravity {
            return super::antigravity_store::watch_paths();
        }
        let history = self.history(profile, mode);
        let mut paths = vec![("history".into(), history.clone())];
        if let Some(session) = session {
            paths.push(("transcript".into(), history.join(format!("{session}.jsonl"))));
        }
        paths
    }

    pub(super) fn settings(&self, profile: u32, mode: HistoryMode) -> String {
        serde_json::json!({"autoMemoryDirectory": self.history(profile, mode).join("memory").to_string_lossy()}).to_string()
    }

    pub(super) fn has_session(&self, profile: u32, mode: HistoryMode, id: &str) -> bool {
        self.session_path(profile, mode, id).is_some()
    }

    pub(super) fn session_path(&self, profile: u32, mode: HistoryMode, id: &str) -> Option<PathBuf> {
        if !valid_session_id(id) {
            return None;
        }
        let path = self.history(profile, mode).join(format!("{id}.jsonl"));
        if !path.symlink_metadata().is_ok_and(|metadata| metadata.is_file()) {
            return None;
        }
        path.canonicalize().ok()
    }
}

fn secure_dir(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn ensure_link(link: &Path, target: &Path) -> io::Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::symlink as create;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_dir as create;

    match create(target, link) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if fs::read_link(link).is_ok_and(|path| path == target) {
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

pub(super) fn valid_session_id(id: &str) -> bool {
    id.len() == 36 && id.bytes().enumerate().all(|(i, byte)| if matches!(i, 8 | 13 | 18 | 23) { byte == b'-' } else { byte.is_ascii_hexdigit() })
}

pub(super) fn new_session_id() -> Result<String, ring::error::Unspecified> {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut id = String::with_capacity(36);
    for (i, byte) in bytes.into_iter().enumerate() {
        use std::fmt::Write as _;
        if matches!(i, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        let _ = write!(id, "{byte:02x}");
    }
    Ok(id)
}

fn transcript_title(path: &Path) -> Option<String> {
    let mut first_prompt = None;
    for line in BufReader::new(File::open(path).ok()?.take(64 * 1024)).lines().map_while(Result::ok) {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if let Some(title) = entry["customTitle"].as_str().and_then(short_title) {
            return Some(title);
        }
        if first_prompt.is_none() && entry["type"] == "user" && entry["isMeta"] != true {
            let content = &entry["message"]["content"];
            let text =
                content.as_str().or_else(|| content.as_array()?.iter().find_map(|block| (block["type"] == "text").then(|| block["text"].as_str()).flatten()));
            first_prompt = text.filter(|text| !text.starts_with('<')).and_then(short_title);
        }
    }
    first_prompt
}

pub(super) fn short_title(text: &str) -> Option<String> {
    let title: String = text.split_whitespace().filter(|part| !part.chars().any(char::is_control)).collect::<Vec<_>>().join(" ").chars().take(100).collect();
    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_handoff_copies_the_rollout_into_the_target_profile_only() {
        let data = crate::qa_workbench_tests::fresh("codex-profile-handoff");
        let project = Project::new(&data, &data, Provider::Codex).expect("project");
        project.prepare(0, HistoryMode::Private).expect("source profile");
        let id = new_session_id().expect("thread");
        let date = "sessions/2026/11/25";
        let path = project.profile(0).join(date).join(format!("rollout-test-{id}.jsonl"));
        secure_dir(path.parent().expect("parent")).expect("rollout directory");
        fs::write(&path, "test history").expect("source transcript");
        let session = Session { id, title: "Local thread".into(), path: Some(path.clone()) };
        let result = project.transfer_session(0, 1, HistoryMode::Private, &session).expect("copied thread");
        let dest = result.path.expect("destination");
        assert!(dest.starts_with(project.profile(1).canonicalize().expect("target profile")));
        assert_eq!(fs::read_to_string(dest).expect("target transcript"), "test history");
        assert!(path.is_file());
        assert!(project.transcript(1, HistoryMode::Private, &session).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn changing_the_history_mode_keeps_auth_paths_and_isolates_private_sessions() {
        let data = crate::qa_workbench_tests::fresh("claude-storage");
        let project = Project::new(&data, &data, Provider::Claude).expect("project folder");
        let private_a = project.prepare(0, HistoryMode::Private).expect("private profile A");
        let shared_a = project.prepare(0, HistoryMode::Shared).expect("shared profile A");
        let shared_b = project.prepare(1, HistoryMode::Shared).expect("shared profile B");
        for env in [&private_a, &shared_a, &shared_b] {
            let name = &env[1].1;
            assert!(name.len() <= 64 && !name.is_empty());
            assert!(name.bytes().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_')));
        }
        assert_eq!(private_a[0], shared_a[0]);
        assert_ne!(shared_a[0], shared_b[0]);
        let id = new_session_id().expect("a session UUID");
        let a_history = Path::new(&shared_a[0].1).join("projects").join(&shared_a[1].1);
        let b_history = Path::new(&shared_b[0].1).join("projects").join(&shared_b[1].1);
        fs::write(a_history.join(format!("{id}.jsonl")), "{\"type\":\"user\",\"message\":{\"content\":\"Test project history\"}}\n").expect("test transcript");
        assert!(b_history.join(format!("{id}.jsonl")).exists());
        assert!(project.sessions(1, HistoryMode::Private).expect("private sessions").is_empty());
        assert_eq!(project.sessions(1, HistoryMode::Shared).expect("shared sessions"), [Session { id, title: "Test project history".into(), path: None }]);
        let legacy = project.legacy_history(0).expect("the previous Claude project folder");
        secure_dir(&legacy).expect("legacy project");
        let lost = new_session_id().expect("a second UUID");
        let old = legacy.join(format!("{lost}.jsonl"));
        let transcript = "{\"type\":\"user\",\"message\":{\"content\":\"Recover me\"}}\n";
        fs::write(&old, transcript).expect("legacy transcript");
        assert_eq!(
            project.recoverable_sessions(0, HistoryMode::Shared).expect("recoverable"),
            [Session { id: lost.clone(), title: "Recover me".into(), path: None }]
        );
        assert!(project.recoverable_sessions(1, HistoryMode::Shared).expect("other account").is_empty());
        project.recover_session(0, HistoryMode::Shared, &lost).expect("recover one conversation");
        assert!(project.has_session(1, HistoryMode::Shared, &lost));
        assert_eq!(fs::read_to_string(old).expect("kept the source"), transcript);
        let lock = project.lock().expect("lock the project");
        assert!(project.lock().is_err());
        drop(lock);
        assert!(project.lock().is_ok());
    }
}
