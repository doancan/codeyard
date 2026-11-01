//! The SDK on the host target, for `cargo test`: every call goes to a [`Host`] installed for the current thread, which
//! `codeyard-extension-test` provides (fake files, fixture HTTP, recorded processes, a virtual clock). The driving
//! functions below play the exthost's part: they call the same exports the WebAssembly glue does.

use std::cell::RefCell;

use crate::Extension;
use crate::error::Error;
use crate::guest;
use crate::protocol::{Completion, Request, Response, StreamEvent};
use crate::types::*;

pub use crate::protocol;

/// A call that returns a ticket: its result arrives later through [`complete`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    ShowNotice { severity: Severity, message: String, actions: Vec<String> },
    QuickPick { items: Vec<PickItem>, options: PickOptions },
    InputBox(InputOptions),
    OpenExternal(String),
    Execute { command: String, args_json: String },
    ActiveDocument,
    Selections,
    ReadDocument(Uri),
    ApplyEdits { uri: Uri, version: u32, edits: Vec<TextEdit> },
    SetSelections { uri: Uri, selections: Vec<Range> },
    ReadFile(Uri),
    ReadMany(Vec<Uri>),
    Stat(Uri),
    FindFiles { glob: String, max: u32 },
    Search(SearchQuery),
    WriteFile { uri: Uri, contents: Vec<u8> },
    DeleteFile(Uri),
    Rename { from: Uri, to: Uri },
    CreateDirectory(Uri),
    Fetch(HttpRequest),
    Session { provider: String, scopes: Vec<String> },
    AuthFetch { provider: String, request: HttpRequest },
    SecretGet(String),
    SecretSet { key: String, value: String },
    SecretDelete(String),
    Spawn(ProcessCommand),
    ReadClipboard,
    WriteClipboard(String),
    SendText(String),
}

/// A call with no result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    Log {
        level: LogLevel,
        message: String,
    },
    /// The answer to request `id`.
    Respond {
        id: u64,
        result: Result<Response, Error>,
    },
    CancelTicket(Ticket),
    ReportProgress {
        id: u64,
        message: Option<String>,
        percent: Option<u8>,
    },
    EndProgress(u64),
    RemoveStatusItem(String),
    ClearTimer(u64),
    Invalidate(String),
    SetDiagnostics {
        uri: Uri,
        diagnostics: Vec<Diagnostic>,
    },
    ClearDiagnostics(Option<Uri>),
    SetDecorations {
        uri: Uri,
        set: String,
        decorations: Vec<Decoration>,
    },
    /// `scm.refresh`: the provider's local id.
    ScmRefresh(String),
    /// `watch.stop`.
    StopWatch(u64),
}

/// What the extension calls. Implementations must not call back into the SDK from these methods: queue results and
/// deliver them with [`complete`] and [`event`] after the current call returns, as the exthost does.
pub trait Host {
    fn start(&mut self, call: Call) -> Ticket;
    fn note(&mut self, note: Note);
    fn env(&mut self) -> Env;
    fn setting(&mut self, key: &str) -> Option<String>;
    fn storage_get(&mut self, key: &str) -> Option<Vec<u8>>;
    fn storage_set(&mut self, key: &str, value: &[u8]) -> Result<(), Error>;
    fn storage_delete(&mut self, key: &str);
    fn storage_keys(&mut self) -> Vec<String>;
    fn folders(&mut self) -> Vec<Uri>;
    fn every(&mut self, seconds: u32) -> u64;
    fn start_progress(&mut self, title: &str, cancellable: bool) -> u64;
    fn set_status_item(&mut self, item: StatusItem) -> Result<(), Error>;
    /// `watch.documents` (API 1.1): the watch's id, or why it was refused. Deliver its events with [`documents`].
    fn watch_documents(&mut self, text: bool) -> Result<u64, Error> {
        let _ = text;
        Err(Error::Invalid("this host has no watches".into()))
    }
    /// `watch.files` (API 1.1). Deliver its events with [`files`].
    fn watch_files(&mut self, glob: &str) -> Result<u64, Error> {
        let _ = glob;
        Err(Error::Invalid("this host has no watches".into()))
    }
}

thread_local! {
    static HOST: RefCell<Option<Box<dyn Host>>> = const { RefCell::new(None) };
}

/// Make `host` the current thread's host, forgetting everything the previous extension registered.
pub fn install(host: Box<dyn Host>) {
    guest::reset();
    let old = HOST.with(|h| h.borrow_mut().replace(host));
    drop(old);
}

/// Remove the current thread's host.
pub fn uninstall() {
    guest::reset();
    let old = HOST.with(|h| h.borrow_mut().take());
    drop(old);
}

fn with<R>(f: impl FnOnce(&mut dyn Host) -> R) -> R {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let host = h.as_deref_mut().expect(
            "codeyard-extension: no host on this thread. In tests, start the extension with codeyard_extension_test::MockHost; \
             on the host target the SDK has no Codeyard to talk to",
        );
        f(host)
    })
}

fn note(n: Note) {
    // Notes after the host is gone (a task dropped at the end of a test) are dropped too.
    HOST.with(|h| {
        if let Some(host) = h.borrow_mut().as_deref_mut() {
            host.note(n);
        }
    });
}

fn start(c: Call) -> Ticket {
    with(|h| h.start(c))
}

// ---- driving the extension, as the exthost does ------------------------------------------------------------------

pub fn activate<E: Extension>(reason: &str) -> Result<(), String> {
    guest::activate::<E>(reason)
}

pub fn deactivate<E: Extension>() {
    guest::deactivate::<E>();
}

/// Forget what the extension registered and its running tasks, keeping the host (a hot reload).
pub fn forget() {
    guest::reset();
}

pub fn handle(id: u64, request: Request) {
    guest::handle(id, request);
}

pub fn cancel(id: u64) {
    guest::cancel(id);
}

pub fn complete(ticket: Ticket, c: Completion) {
    guest::complete(ticket, c);
}

pub fn event(ticket: Ticket, e: StreamEvent) {
    guest::event(ticket, e);
}

/// Deliver events of a [`Host::watch_documents`] watch.
pub fn documents(watch: u64, events: Vec<DocumentEvent>) {
    guest::documents(watch, events);
}

/// Deliver events of a [`Host::watch_files`] watch.
pub fn files(watch: u64, events: Vec<FileEvent>) {
    guest::files(watch, events);
}

// ---- the calls the SDK makes (same names as in wasm.rs) ------------------------------------------------------------

pub(crate) fn log(level: LogLevel, message: &str) {
    note(Note::Log { level, message: message.into() });
}

pub(crate) fn env() -> Env {
    with(|h| h.env())
}

pub(crate) fn cancel_ticket(t: Ticket) {
    note(Note::CancelTicket(t));
}

pub(crate) fn respond(id: u64, result: Result<Response, Error>) {
    note(Note::Respond { id, result });
}

pub(crate) fn show_notice(severity: Severity, message: &str, actions: &[String]) -> Ticket {
    start(Call::ShowNotice { severity, message: message.into(), actions: actions.to_vec() })
}

pub(crate) fn quick_pick(items: &[PickItem], options: &PickOptions) -> Ticket {
    start(Call::QuickPick { items: items.to_vec(), options: options.clone() })
}

pub(crate) fn input_box(options: &InputOptions) -> Ticket {
    start(Call::InputBox(options.clone()))
}

pub(crate) fn start_progress(title: &str, cancellable: bool) -> u64 {
    with(|h| h.start_progress(title, cancellable))
}

pub(crate) fn report_progress(id: u64, message: Option<&str>, percent: Option<u8>) {
    note(Note::ReportProgress { id, message: message.map(Into::into), percent });
}

pub(crate) fn end_progress(id: u64) {
    note(Note::EndProgress(id));
}

pub(crate) fn set_status_item(item: &StatusItem) -> Result<(), Error> {
    with(|h| h.set_status_item(item.clone()))
}

pub(crate) fn remove_status_item(id: &str) {
    note(Note::RemoveStatusItem(id.into()));
}

pub(crate) fn open_external(url: &str) -> Ticket {
    start(Call::OpenExternal(url.into()))
}

pub(crate) fn execute(command: &str, args_json: &str) -> Ticket {
    start(Call::Execute { command: command.into(), args_json: args_json.into() })
}

pub(crate) fn storage_get(key: &str) -> Option<Vec<u8>> {
    with(|h| h.storage_get(key))
}

pub(crate) fn storage_set(key: &str, value: &[u8]) -> Result<(), Error> {
    with(|h| h.storage_set(key, value))
}

pub(crate) fn storage_delete(key: &str) {
    with(|h| h.storage_delete(key));
}

pub(crate) fn storage_keys() -> Vec<String> {
    with(|h| h.storage_keys())
}

pub(crate) fn setting(key: &str) -> Option<String> {
    with(|h| h.setting(key))
}

pub(crate) fn every(seconds: u32) -> u64 {
    with(|h| h.every(seconds))
}

pub(crate) fn clear_timer(id: u64) {
    note(Note::ClearTimer(id));
}

pub(crate) fn watch_documents(text: bool) -> Result<u64, Error> {
    with(|h| h.watch_documents(text))
}

pub(crate) fn watch_files(glob: &str) -> Result<u64, Error> {
    with(|h| h.watch_files(glob))
}

pub(crate) fn stop_watch(id: u64) {
    note(Note::StopWatch(id));
}

pub(crate) fn invalidate(view: &str) {
    note(Note::Invalidate(view.into()));
}

pub(crate) fn scm_refresh(provider: &str) {
    note(Note::ScmRefresh(provider.into()));
}

pub(crate) fn active_document() -> Ticket {
    start(Call::ActiveDocument)
}

pub(crate) fn selections() -> Ticket {
    start(Call::Selections)
}

pub(crate) fn read_document(uri: &str) -> Ticket {
    start(Call::ReadDocument(uri.into()))
}

pub(crate) fn apply_edits(uri: &str, version: u32, edits: &[TextEdit]) -> Ticket {
    start(Call::ApplyEdits { uri: uri.into(), version, edits: edits.to_vec() })
}

pub(crate) fn set_selections(uri: &str, selections: &[Range]) -> Ticket {
    start(Call::SetSelections { uri: uri.into(), selections: selections.to_vec() })
}

pub(crate) fn set_diagnostics(uri: &str, diagnostics: &[Diagnostic]) {
    note(Note::SetDiagnostics { uri: uri.into(), diagnostics: diagnostics.to_vec() });
}

pub(crate) fn clear_diagnostics(uri: Option<&str>) {
    note(Note::ClearDiagnostics(uri.map(Into::into)));
}

pub(crate) fn set_decorations(uri: &str, set: &str, decorations: &[Decoration]) {
    note(Note::SetDecorations { uri: uri.into(), set: set.into(), decorations: decorations.to_vec() });
}

pub(crate) fn folders() -> Vec<Uri> {
    with(|h| h.folders())
}

pub(crate) fn read_file(uri: &str) -> Ticket {
    start(Call::ReadFile(uri.into()))
}

pub(crate) fn read_many(uris: &[&str]) -> Ticket {
    start(Call::ReadMany(uris.iter().map(|u| u.to_string()).collect()))
}

pub(crate) fn stat(uri: &str) -> Ticket {
    start(Call::Stat(uri.into()))
}

pub(crate) fn search(query: &SearchQuery) -> Ticket {
    start(Call::Search(query.clone()))
}

pub(crate) fn find_files(glob: &str, max: u32) -> Ticket {
    start(Call::FindFiles { glob: glob.into(), max })
}

pub(crate) fn write_file(uri: &str, contents: &[u8]) -> Ticket {
    start(Call::WriteFile { uri: uri.into(), contents: contents.to_vec() })
}

pub(crate) fn delete_file(uri: &str) -> Ticket {
    start(Call::DeleteFile(uri.into()))
}

pub(crate) fn rename(from: &str, to: &str) -> Ticket {
    start(Call::Rename { from: from.into(), to: to.into() })
}

pub(crate) fn create_directory(uri: &str) -> Ticket {
    start(Call::CreateDirectory(uri.into()))
}

pub(crate) fn fetch(request: &HttpRequest) -> Ticket {
    start(Call::Fetch(request.clone()))
}

pub(crate) fn session(provider: &str, scopes: &[String]) -> Ticket {
    start(Call::Session { provider: provider.into(), scopes: scopes.to_vec() })
}

pub(crate) fn auth_fetch(provider: &str, request: &HttpRequest) -> Ticket {
    start(Call::AuthFetch { provider: provider.into(), request: request.clone() })
}

pub(crate) fn secret_get(key: &str) -> Ticket {
    start(Call::SecretGet(key.into()))
}

pub(crate) fn secret_set(key: &str, value: &str) -> Ticket {
    start(Call::SecretSet { key: key.into(), value: value.into() })
}

pub(crate) fn secret_delete(key: &str) -> Ticket {
    start(Call::SecretDelete(key.into()))
}

pub(crate) fn spawn(command: &ProcessCommand) -> Ticket {
    start(Call::Spawn(command.clone()))
}

pub(crate) fn read_clipboard() -> Ticket {
    start(Call::ReadClipboard)
}

pub(crate) fn write_clipboard(text: &str) -> Ticket {
    start(Call::WriteClipboard(text.into()))
}

pub(crate) fn send_text(text: &str) -> Ticket {
    start(Call::SendText(text.into()))
}
