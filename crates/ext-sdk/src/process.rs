//! Programs listed in `run`, on the workspace's host. Output streams as it comes; the process ends with its exit code.

use crate::error::{Error, Result};
use crate::protocol::{Completion, StreamEvent};
use crate::runtime::Pending;
use crate::types::{ProcessCommand, Uri};

/// A program to run with [`Context::spawn`](crate::Context::spawn) or [`Context::run`](crate::Context::run).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command(pub(crate) ProcessCommand);

impl Command {
    /// A name from `run`, or a path inside the package.
    pub fn new(program: impl Into<String>) -> Command {
        Command(ProcessCommand { program: program.into(), ..ProcessCommand::default() })
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Command {
        self.0.args.push(arg.into());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, args: I) -> Command {
        self.0.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Defaults to the first workspace folder.
    pub fn cwd(mut self, uri: impl Into<Uri>) -> Command {
        self.0.cwd = Some(uri.into());
        self
    }

    /// Added to a minimal environment; the user's environment is not passed.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Command {
        self.0.env.push((key.into(), value.into()));
        self
    }

    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Command {
        self.0.stdin = Some(bytes.into());
        self
    }

    pub fn program(&self) -> &str {
        &self.0.program
    }
}

/// A chunk of output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chunk {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

/// A running process. Dropping it before it ends kills it.
pub struct Process {
    pending: Option<Pending>,
    program: String,
}

impl Process {
    pub(crate) fn new(ticket: u64, program: String) -> Process {
        Process { pending: Some(Pending::new(ticket)), program }
    }

    /// The next chunk of output, or `None` once the process has ended.
    pub async fn next(&mut self) -> Option<Chunk> {
        let pending = self.pending.as_mut()?;
        loop {
            match pending.next_event().await? {
                StreamEvent::Stdout(b) => return Some(Chunk::Stdout(b)),
                StreamEvent::Stderr(b) => return Some(Chunk::Stderr(b)),
                _ => {}
            }
        }
    }

    /// Wait for it to end, dropping output not yet read. The exit code is none when a signal killed it.
    pub async fn wait(mut self) -> Result<Option<i32>> {
        let Some(pending) = self.pending.take() else { return Err(Error::Other("the process was already waited for".into())) };
        let program = self.program.clone();
        match pending.await {
            Completion::Exit(code) => Ok(code),
            Completion::Failed(e) => Err(e.detailed(&|| format!(r#"run = ["{program}"]"#))),
            c => Err(crate::context::unexpected(c)),
        }
    }
}

/// What [`Context::run`](crate::Context::run) collects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// None when a signal killed it.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}
