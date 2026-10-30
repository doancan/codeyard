//! What the host and the SDK pass each other: requests in, answers out, ticket results and streamed data. Authors never
//! use these; the mock host (`codeyard-extension-test`) does.

use crate::error::Error;
use crate::scm::ScmState;
use crate::types::*;

/// The result of a ticket (`types.completion`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Completion {
    Done,
    Failed(Error),
    Bytes(Vec<u8>),
    Text(Option<String>),
    Json(String),
    /// Indices into the offered list; empty when dismissed.
    Chosen(Vec<u32>),
    Files(Vec<(Uri, Result<Vec<u8>, Error>)>),
    Uris(Vec<Uri>),
    Stat(FileStat),
    Document(Option<Document>),
    Ranges(Vec<Range>),
    /// The exit code, none when killed by a signal.
    Exit(Option<i32>),
    Account(Account),
}

/// Streamed data for a ticket, before its completion (`types.stream-event`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    HttpHead { status: u16, headers: Vec<(String, String)> },
    Data(Vec<u8>),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    SearchMatch { uri: Uri, range: Range, line: String },
}

/// What the host asks the extension (`host-requests.request`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// A command from the manifest: its local id and JSON arguments.
    Command {
        command: String,
        args_json: String,
    },
    /// Draw a view (by local id).
    Render(String),
    /// Something happened in a view: its id and the event, JSON-encoded.
    UiEvent {
        view: String,
        event_json: String,
    },
    Format {
        document: Document,
        range: Option<Range>,
        options: FormatOptions,
    },
    CodeActions {
        document: Document,
        range: Range,
        diagnostics: Vec<Diagnostic>,
    },
    RunCodeAction(String),
    ProvideTasks,
    RunTask(String),
    SettingsChanged(Vec<String>),
    Timer(u64),
    CancelProgress(u64),
    OpenUri(String),
    Hover {
        document: Document,
        position: Position,
    },
    /// The trigger character typed, when one of the manifest's `trigger-characters` asked.
    Completion {
        document: Document,
        position: Position,
        trigger: Option<String>,
    },
    /// Code lenses for the lines in view (the range).
    CodeLens {
        document: Document,
        range: Range,
    },
    /// A lens without a command is on screen.
    ResolveCodeLens {
        document: Document,
        lens: CodeLens,
    },
    /// Inlay hints for the lines in view (the range).
    InlayHints {
        document: Document,
        range: Range,
    },
    ScmStatus {
        provider: String,
        root: Uri,
    },
    ScmOriginal {
        provider: String,
        uri: Uri,
    },
    ScmValidate {
        provider: String,
        root: Uri,
        value: String,
    },
    PullRequests(crate::pulls::PullQuery),
}

/// An answer to a request (`responses.response`), as far as the SDK makes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    None,
    Json(String),
    /// A view's element tree, JSON-encoded.
    View(String),
    Edits(Vec<TextEdit>),
    CodeActions(Vec<CodeAction>),
    Hover(Option<Hover>),
    Completions(CompletionList),
    CodeLenses(Vec<CodeLens>),
    InlayHints(Vec<InlayHint>),
    /// A folder's source control state.
    Scm(ScmState),
    /// A file's original text, or a validation message.
    Text(Option<String>),
    PullRequests(crate::pulls::PullAnswer),
}
