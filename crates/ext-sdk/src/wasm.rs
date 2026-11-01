//! The SDK inside a component: wit-bindgen's bindings for `codeyard:api`, conversions to and from the SDK's types, and
//! the guest exports. Each SDK call uses exactly one import, so a component imports only the interfaces its code
//! calls, and the import check (`cyx package`) asks only for those permissions.

use crate::error::{Denied, Error, Why};
use crate::protocol::{Completion, Request, Response, StreamEvent};
use crate::types::*;
use crate::{Extension, guest};

#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../ext-api/wit",
        world: "extension",
        pub_export_macro: true,
        default_bindings_module: "codeyard_extension::__wasm::bindings",
        runtime_path: "codeyard_extension::__wasm::wit_bindgen::rt",
    });
}

#[doc(hidden)]
pub use wit_bindgen;

use bindings::codeyard::api as w;
use bindings::exports::codeyard::api::guest as g;

#[path = "wasm_scm.rs"]
mod scm;
pub(crate) use scm::scm_refresh;

/// The exports for extension `E`; `export!` names it.
#[doc(hidden)]
pub struct Glue<E>(std::marker::PhantomData<E>);

impl<E: Extension> g::Guest for Glue<E> {
    fn activate(activation: g::Activation) -> Result<(), String> {
        guest::activate::<E>(&activation.reason)
    }

    fn deactivate() {
        guest::deactivate::<E>();
    }

    fn handle(id: u64, request: w::host_requests::Request) {
        guest::handle(id, request.into());
    }

    fn cancel(id: u64) {
        guest::cancel(id);
    }

    fn complete(ticket: u64, result: w::types::Completion) {
        guest::complete(ticket, result.into());
    }

    fn event(ticket: u64, event: w::types::StreamEvent) {
        guest::event(ticket, event.into());
    }
}

// ---- the calls the SDK makes (same names as in native.rs) --------------------------------------------------------

pub(crate) fn log(level: LogLevel, message: &str) {
    let level = match level {
        LogLevel::Trace => w::log::Level::Trace,
        LogLevel::Debug => w::log::Level::Debug,
        LogLevel::Info => w::log::Level::Info,
        LogLevel::Warn => w::log::Level::Warn,
        LogLevel::Error => w::log::Level::Error,
    };
    w::log::log(level, message);
}

pub(crate) fn env() -> Env {
    let e = w::env::info();
    Env {
        codeyard_version: e.codeyard_version,
        api_version: e.api_version,
        language: e.language,
        theme: match e.theme {
            w::env::ThemeKind::Light => ThemeKind::Light,
            w::env::ThemeKind::Dark => ThemeKind::Dark,
            w::env::ThemeKind::HighContrastLight => ThemeKind::HighContrastLight,
            w::env::ThemeKind::HighContrastDark => ThemeKind::HighContrastDark,
        },
        platform: match e.platform {
            w::env::Platform::Macos => Platform::Macos,
            w::env::Platform::Linux => Platform::Linux,
            w::env::Platform::Windows => Platform::Windows,
        },
        remote: e.remote,
        workspace_trusted: e.workspace_trusted,
    }
}

pub(crate) fn cancel_ticket(t: Ticket) {
    w::tickets::cancel(t);
}

pub(crate) fn respond(id: u64, result: Result<Response, Error>) {
    match result {
        Ok(r) => w::requests::respond(id, Ok(&r.into())),
        Err(e) => w::requests::respond(id, Err(&e.into())),
    }
}

pub(crate) fn show_notice(severity: Severity, message: &str, actions: &[String]) -> Ticket {
    let severity = match severity {
        Severity::Info => w::window::Severity::Info,
        Severity::Warning => w::window::Severity::Warning,
        Severity::Error => w::window::Severity::Error,
    };
    w::window::show_notice(severity, message, actions)
}

pub(crate) fn quick_pick(items: &[PickItem], o: &PickOptions) -> Ticket {
    let items: Vec<_> = items.iter().map(|i| w::window::PickItem { label: i.label.clone(), detail: i.detail.clone(), picked: i.picked }).collect();
    w::window::quick_pick(&items, &w::window::PickOptions { title: o.title.clone(), placeholder: o.placeholder.clone(), many: o.many })
}

pub(crate) fn input_box(o: &InputOptions) -> Ticket {
    w::window::input_box(&w::window::InputOptions {
        title: o.title.clone(),
        prompt: o.prompt.clone(),
        value: o.value.clone(),
        placeholder: o.placeholder.clone(),
        password: o.password,
    })
}

pub(crate) fn start_progress(title: &str, cancellable: bool) -> u64 {
    w::window::start_progress(title, cancellable)
}

pub(crate) fn report_progress(id: u64, message: Option<&str>, percent: Option<u8>) {
    w::window::report_progress(id, message, percent);
}

pub(crate) fn end_progress(id: u64) {
    w::window::end_progress(id);
}

pub(crate) fn set_status_item(i: &StatusItem) -> Result<(), Error> {
    let item = w::window::StatusItem { id: i.id.clone(), text: i.text.clone(), tooltip: i.tooltip.clone(), command: i.command.clone(), tone: i.tone.into() };
    w::window::set_status_item(&item).map_err(Into::into)
}

pub(crate) fn remove_status_item(id: &str) {
    w::window::remove_status_item(id);
}

pub(crate) fn open_external(url: &str) -> Ticket {
    w::window::open_external(url)
}

pub(crate) fn execute(command: &str, args_json: &str) -> Ticket {
    w::commands::execute(command, args_json)
}

pub(crate) fn storage_get(key: &str) -> Option<Vec<u8>> {
    w::storage::get(key)
}

pub(crate) fn storage_set(key: &str, value: &[u8]) -> Result<(), Error> {
    w::storage::set(key, value).map_err(Into::into)
}

pub(crate) fn storage_delete(key: &str) {
    w::storage::delete(key);
}

pub(crate) fn storage_keys() -> Vec<String> {
    w::storage::keys()
}

pub(crate) fn setting(key: &str) -> Option<String> {
    w::settings::get(key)
}

pub(crate) fn every(seconds: u32) -> u64 {
    w::timers::every(seconds)
}

pub(crate) fn clear_timer(id: u64) {
    w::timers::clear(id);
}

pub(crate) fn invalidate(view: &str) {
    w::ui::invalidate(view);
}

pub(crate) fn active_document() -> Ticket {
    w::documents::active()
}

pub(crate) fn selections() -> Ticket {
    w::documents::selections()
}

pub(crate) fn read_document(uri: &str) -> Ticket {
    w::documents::read(uri)
}

pub(crate) fn apply_edits(uri: &str, version: u32, edits: &[TextEdit]) -> Ticket {
    let edits: Vec<_> = edits.iter().cloned().map(Into::into).collect();
    w::documents::apply_edits(uri, version, &edits)
}

pub(crate) fn set_selections(uri: &str, selections: &[Range]) -> Ticket {
    let selections: Vec<_> = selections.iter().map(|&r| r.into()).collect();
    w::documents::set_selections(uri, &selections)
}

pub(crate) fn set_diagnostics(uri: &str, diagnostics: &[Diagnostic]) {
    let d: Vec<_> = diagnostics.iter().cloned().map(Into::into).collect();
    w::languages::set_diagnostics(uri, &d);
}

pub(crate) fn clear_diagnostics(uri: Option<&str>) {
    w::languages::clear_diagnostics(uri);
}

pub(crate) fn set_decorations(uri: &str, set: &str, decorations: &[Decoration]) {
    let d: Vec<_> = decorations
        .iter()
        .map(|d| w::languages::Decoration {
            range: d.range.into(),
            place: match d.place {
                DecorationPlace::Gutter => w::languages::DecorationPlace::Gutter,
                DecorationPlace::Line => w::languages::DecorationPlace::Line,
                DecorationPlace::After => w::languages::DecorationPlace::After,
            },
            tone: d.tone.into(),
            text: d.text.clone(),
            hover: d.hover.clone(),
        })
        .collect();
    w::languages::set_decorations(uri, set, &d);
}

pub(crate) fn folders() -> Vec<Uri> {
    w::workspace::folders()
}

pub(crate) fn read_file(uri: &str) -> Ticket {
    w::workspace::read_file(uri)
}

pub(crate) fn read_many(uris: &[&str]) -> Ticket {
    w::workspace::read_many(&uris.iter().map(|u| u.to_string()).collect::<Vec<_>>())
}

pub(crate) fn stat(uri: &str) -> Ticket {
    w::workspace::stat(uri)
}

pub(crate) fn search(q: &SearchQuery) -> Ticket {
    w::workspace::search(&w::workspace::SearchQuery {
        pattern: q.pattern.clone(),
        regex: q.regex,
        case_sensitive: q.case_sensitive,
        whole_word: q.whole_word,
        include: q.include.clone(),
        max: q.max,
    })
}

pub(crate) fn find_files(glob: &str, max: u32) -> Ticket {
    w::workspace::find_files(glob, max)
}

pub(crate) fn write_file(uri: &str, contents: &[u8]) -> Ticket {
    w::workspace_write::write_file(uri, contents)
}

pub(crate) fn delete_file(uri: &str) -> Ticket {
    w::workspace_write::delete(uri)
}

pub(crate) fn rename(from: &str, to: &str) -> Ticket {
    w::workspace_write::rename(from, to)
}

pub(crate) fn create_directory(uri: &str) -> Ticket {
    w::workspace_write::create_directory(uri)
}

fn http(r: &HttpRequest) -> w::types::HttpRequest {
    w::types::HttpRequest { method: r.method.clone(), url: r.url.clone(), headers: r.headers.clone(), body: r.body.clone(), timeout_ms: r.timeout_ms }
}

pub(crate) fn fetch(request: &HttpRequest) -> Ticket {
    w::net::fetch(&http(request))
}

pub(crate) fn session(provider: &str, scopes: &[String]) -> Ticket {
    w::auth::session(provider, scopes)
}

pub(crate) fn auth_fetch(provider: &str, request: &HttpRequest) -> Ticket {
    w::auth::fetch(provider, &http(request))
}

pub(crate) fn secret_get(key: &str) -> Ticket {
    w::secrets::get(key)
}

pub(crate) fn secret_set(key: &str, value: &str) -> Ticket {
    w::secrets::set(key, value)
}

pub(crate) fn secret_delete(key: &str) -> Ticket {
    w::secrets::delete(key)
}

pub(crate) fn spawn(c: &ProcessCommand) -> Ticket {
    w::process::spawn(&w::process::Command { program: c.program.clone(), args: c.args.clone(), cwd: c.cwd.clone(), env: c.env.clone(), stdin: c.stdin.clone() })
}

pub(crate) fn read_clipboard() -> Ticket {
    w::clipboard_read::read_text()
}

pub(crate) fn write_clipboard(text: &str) -> Ticket {
    w::clipboard_write::write_text(text)
}

pub(crate) fn send_text(text: &str) -> Ticket {
    w::terminal::send_text(text)
}

// ---- conversions -------------------------------------------------------------------------------------------------

impl From<Position> for w::types::Position {
    fn from(p: Position) -> Self {
        w::types::Position { line: p.line, character: p.character }
    }
}

impl From<w::types::Position> for Position {
    fn from(p: w::types::Position) -> Self {
        Position { line: p.line, character: p.character }
    }
}

impl From<Range> for w::types::Range {
    fn from(r: Range) -> Self {
        w::types::Range { start: r.start.into(), end: r.end.into() }
    }
}

impl From<w::types::Range> for Range {
    fn from(r: w::types::Range) -> Self {
        Range { start: r.start.into(), end: r.end.into() }
    }
}

impl From<TextEdit> for w::types::TextEdit {
    fn from(e: TextEdit) -> Self {
        w::types::TextEdit { range: e.range.into(), new_text: e.new_text }
    }
}

impl From<Tone> for w::types::Tone {
    fn from(t: Tone) -> Self {
        match t {
            Tone::Neutral => w::types::Tone::Neutral,
            Tone::Accent => w::types::Tone::Accent,
            Tone::Success => w::types::Tone::Success,
            Tone::Warning => w::types::Tone::Warning,
            Tone::Danger => w::types::Tone::Danger,
            Tone::Muted => w::types::Tone::Muted,
        }
    }
}

impl From<Diagnostic> for w::types::Diagnostic {
    fn from(d: Diagnostic) -> Self {
        let severity = match d.severity {
            DiagnosticSeverity::Error => w::types::DiagnosticSeverity::Error,
            DiagnosticSeverity::Warning => w::types::DiagnosticSeverity::Warning,
            DiagnosticSeverity::Information => w::types::DiagnosticSeverity::Information,
            DiagnosticSeverity::Hint => w::types::DiagnosticSeverity::Hint,
        };
        w::types::Diagnostic { range: d.range.into(), severity, message: d.message, code: d.code }
    }
}

impl From<w::types::Diagnostic> for Diagnostic {
    fn from(d: w::types::Diagnostic) -> Self {
        let severity = match d.severity {
            w::types::DiagnosticSeverity::Error => DiagnosticSeverity::Error,
            w::types::DiagnosticSeverity::Warning => DiagnosticSeverity::Warning,
            w::types::DiagnosticSeverity::Information => DiagnosticSeverity::Information,
            w::types::DiagnosticSeverity::Hint => DiagnosticSeverity::Hint,
        };
        Diagnostic { range: d.range.into(), severity, message: d.message, code: d.code }
    }
}

impl From<w::types::Document> for Document {
    fn from(d: w::types::Document) -> Self {
        Document { uri: d.uri, language: d.language, version: d.version, text: d.text }
    }
}

impl From<Error> for w::types::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Denied(d) => w::types::Error::Denied(w::types::Denied {
                permission: d.permission,
                why: match d.why {
                    Why::NotDeclared => w::types::Why::NotDeclared,
                    Why::RevokedByUser => w::types::Why::RevokedByUser,
                    Why::UntrustedWorkspace => w::types::Why::UntrustedWorkspace,
                    Why::RevokedExtension => w::types::Why::RevokedExtension,
                },
            }),
            Error::NotFound(s) => w::types::Error::NotFound(s),
            Error::Cancelled => w::types::Error::Cancelled,
            Error::Timeout => w::types::Error::Timeout,
            Error::Invalid(s) => w::types::Error::Invalid(s),
            Error::Other(s) => w::types::Error::Other(s),
        }
    }
}

impl From<w::types::Error> for Error {
    fn from(e: w::types::Error) -> Self {
        match e {
            w::types::Error::Denied(d) => Error::Denied(Denied {
                permission: d.permission,
                why: match d.why {
                    w::types::Why::NotDeclared => Why::NotDeclared,
                    w::types::Why::RevokedByUser => Why::RevokedByUser,
                    w::types::Why::UntrustedWorkspace => Why::UntrustedWorkspace,
                    w::types::Why::RevokedExtension => Why::RevokedExtension,
                },
            }),
            w::types::Error::NotFound(s) => Error::NotFound(s),
            w::types::Error::Cancelled => Error::Cancelled,
            w::types::Error::Timeout => Error::Timeout,
            w::types::Error::Invalid(s) => Error::Invalid(s),
            w::types::Error::Other(s) => Error::Other(s),
        }
    }
}

impl From<w::types::Completion> for Completion {
    fn from(c: w::types::Completion) -> Self {
        use w::types::Completion as C;
        match c {
            C::Done => Completion::Done,
            C::Failed(e) => Completion::Failed(e.into()),
            C::Bytes(b) => Completion::Bytes(b),
            C::Text(t) => Completion::Text(t),
            C::Json(j) => Completion::Json(j),
            C::Chosen(c) => Completion::Chosen(c),
            C::Files(f) => Completion::Files(f.into_iter().map(|f| (f.uri, f.contents.map_err(Into::into))).collect()),
            C::Uris(u) => Completion::Uris(u),
            C::Stat(s) => Completion::Stat(FileStat { is_dir: s.is_dir, size: s.size, modified: s.modified }),
            C::Document(d) => Completion::Document(d.map(Into::into)),
            C::Ranges(r) => Completion::Ranges(r.into_iter().map(Into::into).collect()),
            C::Exit(code) => Completion::Exit(code),
            C::Account(a) => Completion::Account(Account { provider: a.provider, label: a.label }),
        }
    }
}

impl From<w::types::StreamEvent> for StreamEvent {
    fn from(e: w::types::StreamEvent) -> Self {
        use w::types::StreamEvent as E;
        match e {
            E::HttpHead(h) => StreamEvent::HttpHead { status: h.status, headers: h.headers },
            E::Data(d) => StreamEvent::Data(d),
            E::Stdout(d) => StreamEvent::Stdout(d),
            E::Stderr(d) => StreamEvent::Stderr(d),
            E::SearchMatch(m) => StreamEvent::SearchMatch { uri: m.uri, range: m.range.into(), line: m.line },
        }
    }
}

impl From<w::host_requests::Request> for Request {
    fn from(r: w::host_requests::Request) -> Self {
        use w::host_requests::Request as R;
        match r {
            R::Command((command, args_json)) => Request::Command { command, args_json },
            R::Render(v) => Request::Render(v),
            R::UiEvent((view, event_json)) => Request::UiEvent { view, event_json },
            R::Format((document, range, o)) => Request::Format {
                document: document.into(),
                range: range.map(Into::into),
                options: FormatOptions { tab_size: o.tab_size, insert_spaces: o.insert_spaces },
            },
            R::CodeActions((document, range, diagnostics)) => {
                Request::CodeActions { document: document.into(), range: range.into(), diagnostics: diagnostics.into_iter().map(Into::into).collect() }
            }
            R::RunCodeAction(id) => Request::RunCodeAction(id),
            R::ProvideTasks => Request::ProvideTasks,
            R::RunTask(id) => Request::RunTask(id),
            R::SettingsChanged(keys) => Request::SettingsChanged(keys),
            R::Timer(id) => Request::Timer(id),
            R::CancelProgress(id) => Request::CancelProgress(id),
            R::OpenUri(u) => Request::OpenUri(u),
            R::Hover((document, position)) => Request::Hover { document: document.into(), position: position.into() },
            R::Completion((document, position, trigger)) => Request::Completion { document: document.into(), position: position.into(), trigger },
            R::CodeLens((document, range)) => Request::CodeLens { document: document.into(), range: range.into() },
            R::ResolveCodeLens((document, lens)) => Request::ResolveCodeLens { document: document.into(), lens: lens.into() },
            R::InlayHints((document, range)) => Request::InlayHints { document: document.into(), range: range.into() },
            R::ScmStatus((provider, root)) => Request::ScmStatus { provider, root },
            R::ScmOriginal((provider, uri)) => Request::ScmOriginal { provider, uri },
            R::ScmValidate((provider, root, value)) => Request::ScmValidate { provider, root, value },
            R::PullRequests(q) => Request::PullRequests(crate::pulls::wasm::query(q)),
        }
    }
}

impl From<w::responses::CodeLens> for CodeLens {
    fn from(l: w::responses::CodeLens) -> Self {
        let command =
            l.command.map(|c| LensCommand { title: c.title, command: c.command, args: serde_json::from_str(&c.args_json).unwrap_or(serde_json::Value::Null) });
        CodeLens { id: l.id, range: l.range.into(), command }
    }
}

impl From<CodeLens> for w::responses::CodeLens {
    fn from(l: CodeLens) -> Self {
        let command = l.command.map(|c| w::responses::LensCommand { title: c.title, command: c.command, args_json: c.args.to_string() });
        w::responses::CodeLens { id: l.id, range: l.range.into(), command }
    }
}

impl From<Response> for w::responses::Response {
    fn from(r: Response) -> Self {
        use w::responses::Response as R;
        match r {
            Response::None => R::None,
            Response::Json(j) => R::Json(j),
            Response::View(v) => R::View(v),
            Response::Edits(e) => R::Edits(e.into_iter().map(Into::into).collect()),
            Response::CodeActions(a) => R::CodeActions(
                a.into_iter()
                    .map(|a| w::responses::CodeAction {
                        id: a.id,
                        title: a.title,
                        edits: a.edits.into_iter().map(|(uri, e)| (uri, e.into_iter().map(Into::into).collect())).collect(),
                        command: a.command,
                        preferred: a.preferred,
                        fixes: a.fixes.into_iter().map(Into::into).collect(),
                    })
                    .collect(),
            ),
            Response::Hover(h) => R::Hover(h.map(|h| w::responses::Hover { contents: h.contents, range: h.range.map(Into::into) })),
            Response::Completions(l) => R::Completions(completion_list(l)),
            Response::CodeLenses(l) => R::CodeLenses(l.into_iter().map(Into::into).collect()),
            Response::InlayHints(h) => {
                R::InlayHints(h.into_iter().map(|h| w::responses::InlayHint { position: h.position.into(), label: h.label, tooltip: h.tooltip }).collect())
            }
            Response::Scm(s) => R::Scm(s.into()),
            Response::Text(t) => R::Text(t),
            Response::PullRequests(a) => R::PullRequests(crate::pulls::wasm::answer(a)),
        }
    }
}

fn completion_list(l: CompletionList) -> w::responses::CompletionList {
    use w::responses::CompletionKind as K;
    let kind = |k: CompletionKind| match k {
        CompletionKind::Text => K::Text,
        CompletionKind::Method => K::Method,
        CompletionKind::Function => K::Function,
        CompletionKind::Constructor => K::Constructor,
        CompletionKind::Field => K::Field,
        CompletionKind::Variable => K::Variable,
        CompletionKind::Class => K::Class,
        CompletionKind::Interface => K::Interface,
        CompletionKind::Module => K::Module,
        CompletionKind::Property => K::Property,
        CompletionKind::Unit => K::Unit,
        CompletionKind::Value => K::Value,
        CompletionKind::Enum => K::Enum,
        CompletionKind::Keyword => K::Keyword,
        CompletionKind::Snippet => K::Snippet,
        CompletionKind::Color => K::Color,
        CompletionKind::File => K::File,
        CompletionKind::Reference => K::Reference,
        CompletionKind::Folder => K::Folder,
        CompletionKind::EnumMember => K::EnumMember,
        CompletionKind::Constant => K::Constant,
        CompletionKind::Struct => K::Struct,
        CompletionKind::Event => K::Event,
        CompletionKind::Operator => K::Operator,
        CompletionKind::TypeParameter => K::TypeParameter,
    };
    let items = l
        .items
        .into_iter()
        .map(|i| w::responses::CompletionItem {
            label: i.label,
            kind: kind(i.kind),
            detail: i.detail,
            documentation: i.documentation,
            insert_text: i.insert_text,
            snippet: i.snippet,
            range: i.range.map(Into::into),
            additional_edits: i.additional_edits.into_iter().map(Into::into).collect(),
            filter_text: i.filter_text,
            sort_text: i.sort_text,
        })
        .collect();
    w::responses::CompletionList { items, incomplete: l.incomplete }
}
