//! The API's values as authors use them. One type per `codeyard:api` type; the WebAssembly glue (`wasm.rs`) converts
//! them to and from the generated bindings, and the mock host (`ext-test`) uses them directly.

/// A host call that finishes later (`types.ticket`).
pub type Ticket = u64;

/// A file or folder. Opaque: the same code works on local and SSH folders.
pub type Uri = String;

/// Zero-based line and UTF-16 column, as in JavaScript strings and the Language Server Protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Position {
        Position { line, character }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Range {
        Range { start, end }
    }

    /// One line, from column `start` to `end`.
    pub fn line(line: u32, start: u32, end: u32) -> Range {
        Range { start: Position::new(line, start), end: Position::new(line, end) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

impl TextEdit {
    pub fn replace(range: Range, new_text: impl Into<String>) -> TextEdit {
        TextEdit { range, new_text: new_text.into() }
    }
}

/// Colour roles: extensions pick a tone, the theme picks the colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
    Muted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub code: Option<String>,
}

impl Diagnostic {
    pub fn new(range: Range, severity: DiagnosticSeverity, message: impl Into<String>) -> Diagnostic {
        Diagnostic { range, severity, message: message.into(), code: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub uri: Uri,
    pub language: String,
    /// Increases with every change; edits made against an older version are refused.
    pub version: u32,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStat {
    pub is_dir: bool,
    pub size: u64,
    /// Unix milliseconds.
    pub modified: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub provider: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickItem {
    pub label: String,
    pub detail: Option<String>,
    pub picked: bool,
}

impl<S: Into<String>> From<S> for PickItem {
    fn from(label: S) -> PickItem {
        PickItem { label: label.into(), ..PickItem::default() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickOptions {
    pub title: Option<String>,
    pub placeholder: Option<String>,
    pub many: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputOptions {
    pub title: Option<String>,
    pub prompt: Option<String>,
    pub value: Option<String>,
    pub placeholder: Option<String>,
    pub password: bool,
}

impl InputOptions {
    pub fn prompt(prompt: impl Into<String>) -> InputOptions {
        InputOptions { prompt: Some(prompt.into()), ..InputOptions::default() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusItem {
    /// Local to the extension.
    pub id: String,
    pub text: String,
    pub tooltip: Option<String>,
    /// A command id to run on click.
    pub command: Option<String>,
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationPlace {
    Gutter,
    Line,
    /// Text after the range's end, like an inline hint.
    After,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoration {
    pub range: Range,
    pub place: DecorationPlace,
    pub tone: Tone,
    pub text: Option<String>,
    pub hover: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatOptions {
    pub tab_size: u32,
    pub insert_spaces: bool,
}

/// A code action a provider offers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeAction {
    /// Passed back to `on_code_action` when chosen and `command` is set.
    pub id: String,
    pub title: String,
    /// Edits per document, applied when chosen.
    pub edits: Vec<(Uri, Vec<TextEdit>)>,
    pub command: Option<String>,
    pub preferred: bool,
    /// The diagnostics this action fixes.
    pub fixes: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    Light,
    Dark,
    HighContrastLight,
    HighContrastDark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

/// What the extension runs in (`env.info`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Env {
    pub codeyard_version: String,
    pub api_version: String,
    /// The interface language, such as `en` or `tr`.
    pub language: String,
    pub theme: ThemeKind,
    pub platform: Platform,
    /// The workspace is on an SSH host.
    pub remote: bool,
    pub workspace_trusted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// An HTTPS request (`net.fetch`). Build one with [`Request::get`](crate::net::Request::get) and friends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout_ms: Option<u32>,
}

/// A program to run (`process.spawn`). Build one with [`Command::new`](crate::process::Command::new).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessCommand {
    /// A name from `run`, or a path inside the package.
    pub program: String,
    pub args: Vec<String>,
    /// Defaults to the first workspace folder.
    pub cwd: Option<Uri>,
    pub env: Vec<(String, String)>,
    pub stdin: Option<Vec<u8>>,
}
