//! The API's values in Rust, one type per WIT type, for the frames between the app and the exthost. The exthost
//! converts wasmtime's generated bindings to and from these; the broker in the app reads them. Field names follow the
//! WIT; a test checks that every WIT function has a call here.

use serde::{Deserialize, Serialize};

#[path = "value_scm.rs"]
mod scm;
pub use scm::*;
mod pulls;
pub use pulls::*;

pub type Ticket = u64;
pub type Uri = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Why {
    NotDeclared,
    RevokedByUser,
    UntrustedWorkspace,
    RevokedExtension,
}

impl Why {
    pub fn as_str(self) -> &'static str {
        match self {
            Why::NotDeclared => "not-declared",
            Why::RevokedByUser => "revoked-by-user",
            Why::UntrustedWorkspace => "untrusted-workspace",
            Why::RevokedExtension => "revoked-extension",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Denied {
    pub permission: String,
    pub why: Why,
}

/// `types.error`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Error {
    Denied(Denied),
    NotFound(String),
    Cancelled,
    Timeout,
    Invalid(String),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Denied(d) => write!(f, "denied: {} ({})", d.permission, d.why.as_str()),
            Error::NotFound(s) => write!(f, "not found: {s}"),
            Error::Cancelled => f.write_str("cancelled"),
            Error::Timeout => f.write_str("timed out"),
            Error::Invalid(s) => write!(f, "invalid: {s}"),
            Error::Other(s) => f.write_str(s),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
    Muted,
}

pub type Headers = Vec<(String, String)>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Headers,
    pub body: Vec<u8>,
    pub timeout_ms: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHead {
    pub status: u16,
    pub headers: Headers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub uri: Uri,
    pub language: String,
    pub version: u32,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStat {
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub uri: Uri,
    pub range: Range,
    pub line: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileResult {
    pub uri: Uri,
    pub contents: Result<Vec<u8>, Error>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub provider: String,
    pub label: String,
}

/// `types.completion`: the result of a ticket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Completion {
    Done,
    Failed(Error),
    Bytes(Vec<u8>),
    Text(Option<String>),
    Json(String),
    Chosen(Vec<u32>),
    Files(Vec<FileResult>),
    Uris(Vec<Uri>),
    Stat(FileStat),
    Document(Option<Document>),
    Ranges(Vec<Range>),
    Exit(Option<i32>),
    Account(Account),
}

/// `types.stream-event`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamEvent {
    HttpHead(HttpHead),
    Data(Vec<u8>),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    SearchMatch(SearchMatch),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeKind {
    Light,
    Dark,
    HighContrastLight,
    HighContrastDark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

/// `env.info`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvInfo {
    pub codeyard_version: String,
    pub api_version: String,
    pub language: String,
    pub theme: ThemeKind,
    pub platform: Platform,
    pub remote: bool,
    pub workspace_trusted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickItem {
    pub label: String,
    pub detail: Option<String>,
    pub picked: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickOptions {
    pub title: Option<String>,
    pub placeholder: Option<String>,
    pub many: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputOptions {
    pub title: Option<String>,
    pub prompt: Option<String>,
    pub value: Option<String>,
    pub placeholder: Option<String>,
    pub password: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusItem {
    pub id: String,
    pub text: String,
    pub tooltip: Option<String>,
    pub command: Option<String>,
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecorationPlace {
    Gutter,
    Line,
    After,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decoration {
    pub range: Range,
    pub place: DecorationPlace,
    pub tone: Tone,
    pub text: Option<String>,
    pub hover: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub include: Vec<String>,
    pub max: u32,
}

/// `process.command`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<Uri>,
    pub env: Vec<(String, String)>,
    pub stdin: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAction {
    pub id: String,
    pub title: String,
    pub edits: Vec<(Uri, Vec<TextEdit>)>,
    pub command: Option<String>,
    pub preferred: bool,
    pub fixes: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub group: String,
}

/// `responses.lens-command`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensCommand {
    pub title: String,
    pub command: String,
    pub args_json: String,
}

/// `responses.code-lens`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeLens {
    pub id: String,
    pub range: Range,
    pub command: Option<LensCommand>,
}

/// `responses.inlay-hint`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlayHint {
    pub position: Position,
    pub label: String,
    pub tooltip: Option<String>,
}

/// `responses.response`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    None,
    Json(String),
    /// The element tree as JSON (`ext_ui::decode_json`).
    View(String),
    Edits(Vec<TextEdit>),
    CodeActions(Vec<CodeAction>),
    Tasks(Vec<Task>),
    Ranges(Vec<Range>),
    Hover(Option<Hover>),
    Completions(CompletionList),
    CodeLenses(Vec<CodeLens>),
    InlayHints(Vec<InlayHint>),
    Scm(ScmState),
    Text(Option<String>),
    PullRequests(PullAnswer),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatOptions {
    pub tab_size: u32,
    pub insert_spaces: bool,
}

/// `host-requests.request`: what the host asks the extension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Command { command: String, args_json: String },
    Render { view: String },
    UiEvent { view: String, event_json: String },
    Format { document: Document, range: Option<Range>, options: FormatOptions },
    CodeActions { document: Document, range: Range, diagnostics: Vec<Diagnostic> },
    RunCodeAction { id: String },
    ProvideTasks,
    RunTask { id: String },
    SettingsChanged { keys: Vec<String> },
    Timer { id: u64 },
    CancelProgress { id: u64 },
    OpenUri { uri: String },
    Hover { document: Document, position: Position },
    Completion { document: Document, position: Position, trigger: Option<String> },
    CodeLens { document: Document, range: Range },
    ResolveCodeLens { document: Document, lens: CodeLens },
    InlayHints { document: Document, range: Range },
    ScmStatus { provider: String, root: Uri },
    ScmOriginal { provider: String, uri: Uri },
    ScmValidate { provider: String, root: Uri, value: String },
    PullRequests { query: PullQuery },
}

impl Request {
    /// Started by a user action, so it may use the active document (Chrome's `activeTab`) and `open-external`.
    pub fn is_user_action(&self) -> bool {
        matches!(self, Request::Command { .. } | Request::UiEvent { .. } | Request::RunCodeAction { .. } | Request::RunTask { .. } | Request::OpenUri { .. })
            || matches!(self, Request::PullRequests { query } if query.is_user_action())
    }
}

/// `responses.hover`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hover {
    /// Markdown.
    pub contents: String,
    pub range: Option<Range>,
}

/// `responses.completion-kind`, in the Language Server Protocol's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionKind {
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

impl CompletionKind {
    /// The Language Server Protocol's `CompletionItemKind` number.
    pub fn lsp(self) -> u8 {
        self as u8 + 1
    }
}

/// `responses.completion-item`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    /// Markdown.
    pub documentation: Option<String>,
    pub insert_text: Option<String>,
    pub snippet: bool,
    pub range: Option<Range>,
    pub additional_edits: Vec<TextEdit>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,
}

/// `responses.completion-list`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionList {
    pub items: Vec<CompletionItem>,
    pub incomplete: bool,
}

/// A host call that returns a ticket: the exthost forwards it to the app, which answers with a `Completion` (and
/// `StreamEvent`s before it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Start {
    ShowNotice { severity: Severity, message: String, actions: Vec<String> },
    QuickPick { items: Vec<PickItem>, options: PickOptions },
    InputBox { options: InputOptions },
    OpenExternal { url: String },
    Execute { command: String, args_json: String },
    ActiveDocument,
    Selections,
    ReadDocument { uri: Uri },
    ApplyEdits { uri: Uri, version: u32, edits: Vec<TextEdit> },
    SetSelections { uri: Uri, selections: Vec<Range> },
    ReadFile { uri: Uri },
    ReadMany { uris: Vec<Uri> },
    Stat { uri: Uri },
    FindFiles { glob: String, max: u32 },
    Search { query: SearchQuery },
    WriteFile { uri: Uri, contents: Vec<u8> },
    Delete { uri: Uri },
    Rename { from: Uri, to: Uri },
    CreateDirectory { uri: Uri },
    Fetch { request: HttpRequest },
    Session { provider: String, scopes: Vec<String> },
    AuthFetch { provider: String, request: HttpRequest },
    SecretGet { key: String },
    SecretSet { key: String, value: String },
    SecretDelete { key: String },
    Spawn { command: Command },
    ReadClipboard,
    WriteClipboard { text: String },
    SendText { text: String },
}

impl Start {
    /// The WIT function, as `interface/function`.
    pub fn wit_name(&self) -> &'static str {
        match self {
            Start::ShowNotice { .. } => "window/show-notice",
            Start::QuickPick { .. } => "window/quick-pick",
            Start::InputBox { .. } => "window/input-box",
            Start::OpenExternal { .. } => "window/open-external",
            Start::Execute { .. } => "commands/execute",
            Start::ActiveDocument => "documents/active",
            Start::Selections => "documents/selections",
            Start::ReadDocument { .. } => "documents/read",
            Start::ApplyEdits { .. } => "documents/apply-edits",
            Start::SetSelections { .. } => "documents/set-selections",
            Start::ReadFile { .. } => "workspace/read-file",
            Start::ReadMany { .. } => "workspace/read-many",
            Start::Stat { .. } => "workspace/stat",
            Start::FindFiles { .. } => "workspace/find-files",
            Start::Search { .. } => "workspace/search",
            Start::WriteFile { .. } => "workspace-write/write-file",
            Start::Delete { .. } => "workspace-write/delete",
            Start::Rename { .. } => "workspace-write/rename",
            Start::CreateDirectory { .. } => "workspace-write/create-directory",
            Start::Fetch { .. } => "net/fetch",
            Start::Session { .. } => "auth/session",
            Start::AuthFetch { .. } => "auth/fetch",
            Start::SecretGet { .. } => "secrets/get",
            Start::SecretSet { .. } => "secrets/set",
            Start::SecretDelete { .. } => "secrets/delete",
            Start::Spawn { .. } => "process/spawn",
            Start::ReadClipboard => "clipboard-read/read-text",
            Start::WriteClipboard { .. } => "clipboard-write/write-text",
            Start::SendText { .. } => "terminal/send-text",
        }
    }
}

/// A host call with no answer, forwarded to the app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Notify {
    Log { level: LogLevel, message: String },
    Cancel { ticket: Ticket },
    Respond { id: u64, result: Result<Response, Error> },
    StartProgress { id: u64, title: String, cancellable: bool },
    ReportProgress { id: u64, message: Option<String>, percent: Option<u8> },
    EndProgress { id: u64 },
    SetStatusItem { item: StatusItem },
    RemoveStatusItem { id: String },
    Every { id: u64, seconds: u32 },
    ClearTimer { id: u64 },
    Invalidate { view: String },
    SetDiagnostics { uri: Uri, diagnostics: Vec<Diagnostic> },
    ClearDiagnostics { uri: Option<Uri> },
    SetDecorations { uri: Uri, set: String, decorations: Vec<Decoration> },
    ScmRefresh { provider: String },
}

impl Notify {
    pub fn wit_name(&self) -> &'static str {
        match self {
            Notify::Log { .. } => "log/log",
            Notify::Cancel { .. } => "tickets/cancel",
            Notify::Respond { .. } => "requests/respond",
            Notify::StartProgress { .. } => "window/start-progress",
            Notify::ReportProgress { .. } => "window/report-progress",
            Notify::EndProgress { .. } => "window/end-progress",
            Notify::SetStatusItem { .. } => "window/set-status-item",
            Notify::RemoveStatusItem { .. } => "window/remove-status-item",
            Notify::Every { .. } => "timers/every",
            Notify::ClearTimer { .. } => "timers/clear",
            Notify::Invalidate { .. } => "ui/invalidate",
            Notify::SetDiagnostics { .. } => "languages/set-diagnostics",
            Notify::ClearDiagnostics { .. } => "languages/clear-diagnostics",
            Notify::SetDecorations { .. } => "languages/set-decorations",
            Notify::ScmRefresh { .. } => "scm/refresh",
        }
    }
}

/// WIT functions the exthost answers itself, without a frame: from what the app sent at start (`env`, `settings`,
/// the workspace folders) or from the extension's own data folder (`storage`).
pub const LOCAL: [&str; 7] = ["env/info", "settings/get", "workspace/folders", "storage/get", "storage/set", "storage/delete", "storage/keys"];
