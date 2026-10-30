//! wasmtime's generated types ↔ `ext_api::value`, the types the frames carry. Field for field; the WIT is the same.

use ext_api::value as v;

use crate::bindings::codeyard::api as g;
use crate::bindings::exports::codeyard::api::guest as gx;

// --- guest → app

impl From<g::types::Position> for v::Position {
    fn from(p: g::types::Position) -> Self {
        v::Position { line: p.line, character: p.character }
    }
}

impl From<g::types::Range> for v::Range {
    fn from(r: g::types::Range) -> Self {
        v::Range { start: r.start.into(), end: r.end.into() }
    }
}

impl From<g::types::TextEdit> for v::TextEdit {
    fn from(e: g::types::TextEdit) -> Self {
        v::TextEdit { range: e.range.into(), new_text: e.new_text }
    }
}

impl From<g::types::Tone> for v::Tone {
    fn from(t: g::types::Tone) -> Self {
        use g::types::Tone as T;
        match t {
            T::Neutral => v::Tone::Neutral,
            T::Accent => v::Tone::Accent,
            T::Success => v::Tone::Success,
            T::Warning => v::Tone::Warning,
            T::Danger => v::Tone::Danger,
            T::Muted => v::Tone::Muted,
        }
    }
}

impl From<g::types::Why> for v::Why {
    fn from(w: g::types::Why) -> Self {
        use g::types::Why as W;
        match w {
            W::NotDeclared => v::Why::NotDeclared,
            W::RevokedByUser => v::Why::RevokedByUser,
            W::UntrustedWorkspace => v::Why::UntrustedWorkspace,
            W::RevokedExtension => v::Why::RevokedExtension,
        }
    }
}

impl From<g::types::Error> for v::Error {
    fn from(e: g::types::Error) -> Self {
        use g::types::Error as E;
        match e {
            E::Denied(d) => v::Error::Denied(v::Denied { permission: d.permission, why: d.why.into() }),
            E::NotFound(s) => v::Error::NotFound(s),
            E::Cancelled => v::Error::Cancelled,
            E::Timeout => v::Error::Timeout,
            E::Invalid(s) => v::Error::Invalid(s),
            E::Other(s) => v::Error::Other(s),
        }
    }
}

impl From<g::types::DiagnosticSeverity> for v::DiagnosticSeverity {
    fn from(s: g::types::DiagnosticSeverity) -> Self {
        use g::types::DiagnosticSeverity as S;
        match s {
            S::Error => v::DiagnosticSeverity::Error,
            S::Warning => v::DiagnosticSeverity::Warning,
            S::Information => v::DiagnosticSeverity::Information,
            S::Hint => v::DiagnosticSeverity::Hint,
        }
    }
}

impl From<g::types::Diagnostic> for v::Diagnostic {
    fn from(d: g::types::Diagnostic) -> Self {
        v::Diagnostic { range: d.range.into(), severity: d.severity.into(), message: d.message, code: d.code }
    }
}

impl From<g::types::HttpRequest> for v::HttpRequest {
    fn from(r: g::types::HttpRequest) -> Self {
        v::HttpRequest { method: r.method, url: r.url, headers: r.headers, body: r.body, timeout_ms: r.timeout_ms }
    }
}

impl From<g::log::Level> for v::LogLevel {
    fn from(l: g::log::Level) -> Self {
        use g::log::Level as L;
        match l {
            L::Trace => v::LogLevel::Trace,
            L::Debug => v::LogLevel::Debug,
            L::Info => v::LogLevel::Info,
            L::Warn => v::LogLevel::Warn,
            L::Error => v::LogLevel::Error,
        }
    }
}

impl From<g::window::Severity> for v::Severity {
    fn from(s: g::window::Severity) -> Self {
        use g::window::Severity as S;
        match s {
            S::Info => v::Severity::Info,
            S::Warning => v::Severity::Warning,
            S::Error => v::Severity::Error,
        }
    }
}

impl From<g::window::PickItem> for v::PickItem {
    fn from(i: g::window::PickItem) -> Self {
        v::PickItem { label: i.label, detail: i.detail, picked: i.picked }
    }
}

impl From<g::window::PickOptions> for v::PickOptions {
    fn from(o: g::window::PickOptions) -> Self {
        v::PickOptions { title: o.title, placeholder: o.placeholder, many: o.many }
    }
}

impl From<g::window::InputOptions> for v::InputOptions {
    fn from(o: g::window::InputOptions) -> Self {
        v::InputOptions { title: o.title, prompt: o.prompt, value: o.value, placeholder: o.placeholder, password: o.password }
    }
}

impl From<g::window::StatusItem> for v::StatusItem {
    fn from(i: g::window::StatusItem) -> Self {
        v::StatusItem { id: i.id, text: i.text, tooltip: i.tooltip, command: i.command, tone: i.tone.into() }
    }
}

impl From<g::languages::Decoration> for v::Decoration {
    fn from(d: g::languages::Decoration) -> Self {
        use g::languages::DecorationPlace as P;
        let place = match d.place {
            P::Gutter => v::DecorationPlace::Gutter,
            P::Line => v::DecorationPlace::Line,
            P::After => v::DecorationPlace::After,
        };
        v::Decoration { range: d.range.into(), place, tone: d.tone.into(), text: d.text, hover: d.hover }
    }
}

impl From<g::workspace::SearchQuery> for v::SearchQuery {
    fn from(q: g::workspace::SearchQuery) -> Self {
        v::SearchQuery { pattern: q.pattern, regex: q.regex, case_sensitive: q.case_sensitive, whole_word: q.whole_word, include: q.include, max: q.max }
    }
}

impl From<g::process::Command> for v::Command {
    fn from(c: g::process::Command) -> Self {
        v::Command { program: c.program, args: c.args, cwd: c.cwd, env: c.env, stdin: c.stdin }
    }
}

impl From<g::responses::Response> for v::Response {
    fn from(r: g::responses::Response) -> Self {
        use g::responses::Response as R;
        match r {
            R::None => v::Response::None,
            R::Json(s) => v::Response::Json(s),
            R::View(s) => v::Response::View(s),
            R::Edits(e) => v::Response::Edits(e.into_iter().map(Into::into).collect()),
            R::CodeActions(a) => v::Response::CodeActions(
                a.into_iter()
                    .map(|a| v::CodeAction {
                        id: a.id,
                        title: a.title,
                        edits: a.edits.into_iter().map(|(u, e)| (u, e.into_iter().map(Into::into).collect())).collect(),
                        command: a.command,
                        preferred: a.preferred,
                        fixes: a.fixes.into_iter().map(Into::into).collect(),
                    })
                    .collect(),
            ),
            R::Tasks(t) => v::Response::Tasks(t.into_iter().map(|t| v::Task { id: t.id, label: t.label, detail: t.detail, group: t.group }).collect()),
            R::Ranges(r) => v::Response::Ranges(r.into_iter().map(Into::into).collect()),
            R::Hover(h) => v::Response::Hover(h.map(|h| v::Hover { contents: h.contents, range: h.range.map(Into::into) })),
            R::Completions(l) => v::Response::Completions(completion_list(l)),
            R::CodeLenses(l) => v::Response::CodeLenses(l.into_iter().map(Into::into).collect()),
            R::InlayHints(h) => {
                v::Response::InlayHints(h.into_iter().map(|h| v::InlayHint { position: h.position.into(), label: h.label, tooltip: h.tooltip }).collect())
            }
        }
    }
}

impl From<g::responses::CodeLens> for v::CodeLens {
    fn from(l: g::responses::CodeLens) -> Self {
        let command = l.command.map(|c| v::LensCommand { title: c.title, command: c.command, args_json: c.args_json });
        v::CodeLens { id: l.id, range: l.range.into(), command }
    }
}

// --- app → guest

impl From<v::CodeLens> for g::responses::CodeLens {
    fn from(l: v::CodeLens) -> Self {
        let command = l.command.map(|c| g::responses::LensCommand { title: c.title, command: c.command, args_json: c.args_json });
        g::responses::CodeLens { id: l.id, range: l.range.into(), command }
    }
}

impl From<v::Position> for g::types::Position {
    fn from(p: v::Position) -> Self {
        g::types::Position { line: p.line, character: p.character }
    }
}

impl From<v::Range> for g::types::Range {
    fn from(r: v::Range) -> Self {
        g::types::Range { start: r.start.into(), end: r.end.into() }
    }
}

impl From<v::Why> for g::types::Why {
    fn from(w: v::Why) -> Self {
        use g::types::Why as W;
        match w {
            v::Why::NotDeclared => W::NotDeclared,
            v::Why::RevokedByUser => W::RevokedByUser,
            v::Why::UntrustedWorkspace => W::UntrustedWorkspace,
            v::Why::RevokedExtension => W::RevokedExtension,
        }
    }
}

impl From<v::Error> for g::types::Error {
    fn from(e: v::Error) -> Self {
        use g::types::Error as E;
        match e {
            v::Error::Denied(d) => E::Denied(g::types::Denied { permission: d.permission, why: d.why.into() }),
            v::Error::NotFound(s) => E::NotFound(s),
            v::Error::Cancelled => E::Cancelled,
            v::Error::Timeout => E::Timeout,
            v::Error::Invalid(s) => E::Invalid(s),
            v::Error::Other(s) => E::Other(s),
        }
    }
}

impl From<v::DiagnosticSeverity> for g::types::DiagnosticSeverity {
    fn from(s: v::DiagnosticSeverity) -> Self {
        use g::types::DiagnosticSeverity as S;
        match s {
            v::DiagnosticSeverity::Error => S::Error,
            v::DiagnosticSeverity::Warning => S::Warning,
            v::DiagnosticSeverity::Information => S::Information,
            v::DiagnosticSeverity::Hint => S::Hint,
        }
    }
}

impl From<v::Diagnostic> for g::types::Diagnostic {
    fn from(d: v::Diagnostic) -> Self {
        g::types::Diagnostic { range: d.range.into(), severity: d.severity.into(), message: d.message, code: d.code }
    }
}

impl From<v::Document> for g::types::Document {
    fn from(d: v::Document) -> Self {
        g::types::Document { uri: d.uri, language: d.language, version: d.version, text: d.text }
    }
}

impl From<v::Completion> for g::types::Completion {
    fn from(c: v::Completion) -> Self {
        use g::types::Completion as C;
        match c {
            v::Completion::Done => C::Done,
            v::Completion::Failed(e) => C::Failed(e.into()),
            v::Completion::Bytes(b) => C::Bytes(b),
            v::Completion::Text(t) => C::Text(t),
            v::Completion::Json(j) => C::Json(j),
            v::Completion::Chosen(c) => C::Chosen(c),
            v::Completion::Files(f) => C::Files(f.into_iter().map(|f| g::types::FileResult { uri: f.uri, contents: f.contents.map_err(Into::into) }).collect()),
            v::Completion::Uris(u) => C::Uris(u),
            v::Completion::Stat(s) => C::Stat(g::types::FileStat { is_dir: s.is_dir, size: s.size, modified: s.modified }),
            v::Completion::Document(d) => C::Document(d.map(Into::into)),
            v::Completion::Ranges(r) => C::Ranges(r.into_iter().map(Into::into).collect()),
            v::Completion::Exit(code) => C::Exit(code),
            v::Completion::Account(a) => C::Account(g::types::Account { provider: a.provider, label: a.label }),
        }
    }
}

impl From<v::StreamEvent> for g::types::StreamEvent {
    fn from(e: v::StreamEvent) -> Self {
        use g::types::StreamEvent as E;
        match e {
            v::StreamEvent::HttpHead(h) => E::HttpHead(g::types::HttpHead { status: h.status, headers: h.headers }),
            v::StreamEvent::Data(d) => E::Data(d),
            v::StreamEvent::Stdout(d) => E::Stdout(d),
            v::StreamEvent::Stderr(d) => E::Stderr(d),
            v::StreamEvent::SearchMatch(m) => E::SearchMatch(g::types::SearchMatch { uri: m.uri, range: m.range.into(), line: m.line }),
        }
    }
}

impl From<v::Request> for g::host_requests::Request {
    fn from(r: v::Request) -> Self {
        use g::host_requests::Request as R;
        match r {
            v::Request::Command { command, args_json } => R::Command((command, args_json)),
            v::Request::Render { view } => R::Render(view),
            v::Request::UiEvent { view, event_json } => R::UiEvent((view, event_json)),
            v::Request::Format { document, range, options } => R::Format((
                document.into(),
                range.map(Into::into),
                g::host_requests::FormatOptions { tab_size: options.tab_size, insert_spaces: options.insert_spaces },
            )),
            v::Request::CodeActions { document, range, diagnostics } => {
                R::CodeActions((document.into(), range.into(), diagnostics.into_iter().map(Into::into).collect()))
            }
            v::Request::RunCodeAction { id } => R::RunCodeAction(id),
            v::Request::ProvideTasks => R::ProvideTasks,
            v::Request::RunTask { id } => R::RunTask(id),
            v::Request::SettingsChanged { keys } => R::SettingsChanged(keys),
            v::Request::Timer { id } => R::Timer(id),
            v::Request::CancelProgress { id } => R::CancelProgress(id),
            v::Request::OpenUri { uri } => R::OpenUri(uri),
            v::Request::Hover { document, position } => R::Hover((document.into(), position.into())),
            v::Request::Completion { document, position, trigger } => R::Completion((document.into(), position.into(), trigger)),
            v::Request::CodeLens { document, range } => R::CodeLens((document.into(), range.into())),
            v::Request::ResolveCodeLens { document, lens } => R::ResolveCodeLens((document.into(), lens.into())),
            v::Request::InlayHints { document, range } => R::InlayHints((document.into(), range.into())),
        }
    }
}

pub fn environment(e: &v::EnvInfo) -> g::env::Environment {
    use g::env::{Platform as P, ThemeKind as T};
    g::env::Environment {
        codeyard_version: e.codeyard_version.clone(),
        api_version: e.api_version.clone(),
        language: e.language.clone(),
        theme: match e.theme {
            v::ThemeKind::Light => T::Light,
            v::ThemeKind::Dark => T::Dark,
            v::ThemeKind::HighContrastLight => T::HighContrastLight,
            v::ThemeKind::HighContrastDark => T::HighContrastDark,
        },
        platform: match e.platform {
            v::Platform::Macos => P::Macos,
            v::Platform::Linux => P::Linux,
            v::Platform::Windows => P::Windows,
        },
        remote: e.remote,
        workspace_trusted: e.workspace_trusted,
    }
}

pub fn activation(reason: &str) -> gx::Activation {
    gx::Activation { reason: reason.to_string() }
}

/// `responses.completion-list`, guest → app.
fn completion_list(l: g::responses::CompletionList) -> v::CompletionList {
    use g::responses::CompletionKind as K;
    let kind = |k: K| match k {
        K::Text => v::CompletionKind::Text,
        K::Method => v::CompletionKind::Method,
        K::Function => v::CompletionKind::Function,
        K::Constructor => v::CompletionKind::Constructor,
        K::Field => v::CompletionKind::Field,
        K::Variable => v::CompletionKind::Variable,
        K::Class => v::CompletionKind::Class,
        K::Interface => v::CompletionKind::Interface,
        K::Module => v::CompletionKind::Module,
        K::Property => v::CompletionKind::Property,
        K::Unit => v::CompletionKind::Unit,
        K::Value => v::CompletionKind::Value,
        K::Enum => v::CompletionKind::Enum,
        K::Keyword => v::CompletionKind::Keyword,
        K::Snippet => v::CompletionKind::Snippet,
        K::Color => v::CompletionKind::Color,
        K::File => v::CompletionKind::File,
        K::Reference => v::CompletionKind::Reference,
        K::Folder => v::CompletionKind::Folder,
        K::EnumMember => v::CompletionKind::EnumMember,
        K::Constant => v::CompletionKind::Constant,
        K::Struct => v::CompletionKind::Struct,
        K::Event => v::CompletionKind::Event,
        K::Operator => v::CompletionKind::Operator,
        K::TypeParameter => v::CompletionKind::TypeParameter,
    };
    let items = l
        .items
        .into_iter()
        .map(|i| v::CompletionItem {
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
    v::CompletionList { items, incomplete: l.incomplete }
}
