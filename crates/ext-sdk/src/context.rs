//! [`Context`]: everything an extension registers and every call it makes. Calls that reach the app are `async`; the
//! rest (storage, settings, log, diagnostics, status items) return at once.

use std::future::Future;
use std::marker::PhantomData;
use std::rc::Rc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{Error, Result, line_for, path_line};
use crate::guest::{self, BoxFuture};
use crate::net::{self, Request, Response};
use crate::process::{Chunk, Command, Output, Process};
use crate::protocol::{Completion, StreamEvent};
use crate::runtime::{self, Pending};
use crate::sys;
use crate::types::*;

/// The extension's handle on Codeyard. Cheap to copy: capture it in closures and tasks freely.
#[derive(Clone, Copy)]
pub struct Context {
    // Not `Send`: an extension has one thread.
    _one_thread: PhantomData<*const ()>,
}

pub(crate) fn unexpected(c: Completion) -> Error {
    Error::Other(format!("the host answered with something unexpected: {c:?}"))
}

/// Wait for a ticket; a refusal gets its manifest line from `line`.
async fn call(ticket: Ticket, line: impl Fn() -> String) -> Result<Completion> {
    match Pending::new(ticket).await {
        Completion::Failed(e) => Err(e.detailed(&line)),
        c => Ok(c),
    }
}

fn free() -> String {
    String::new()
}

impl Context {
    pub(crate) fn new() -> Context {
        Context { _one_thread: PhantomData }
    }

    // ---- registration -------------------------------------------------------------------------------------------

    /// Run `f` for the manifest's command `id` (its local id, like `refresh`). The arguments are JSON (`null` when
    /// none); whatever `f` returns is serialized as the command's result.
    ///
    /// ```ignore
    /// ctx.command("greet", async |ctx, _args| {
    ///     ctx.notice("Hello!");
    ///     Ok(())
    /// });
    /// ```
    pub fn command<F, R>(&self, id: &str, f: F)
    where
        F: AsyncFn(Context, Value) -> Result<R> + 'static,
        R: Serialize,
    {
        let f = Rc::new(f);
        let cb: guest::CommandFn = Rc::new(move |ctx, args| {
            let f = f.clone();
            Box::pin(async move { Ok(serde_json::to_value(f(ctx, args).await?)?) })
        });
        guest::with(|h| h.commands.insert(id.to_string(), cb));
    }

    /// Draw the manifest's view `id`: `f` returns its element tree, built with [`ui`](crate::ui) (or JSON in the form
    /// of docs/extensions/reference/ui.md). The view is drawn again after [`invalidate`](Self::invalidate).
    pub fn view<F, R>(&self, id: &str, f: F)
    where
        F: AsyncFn(Context) -> Result<R> + 'static,
        R: Serialize,
    {
        let f = Rc::new(f);
        let cb: guest::ViewFn = Rc::new(move |ctx| {
            let f = f.clone();
            Box::pin(async move { Ok(serde_json::to_value(f(ctx).await?)?) })
        });
        guest::with(|h| h.views.insert(id.to_string(), cb));
    }

    /// Handle what happens in view `id` (a click, a changed input): the event as JSON, as Codeyard sends it
    /// (`{"click": {"path": ["add"]}}`, docs/extensions/reference/ui.md#events).
    pub fn on_view_event<F>(&self, id: &str, f: F)
    where
        F: AsyncFn(Context, Value) -> Result<()> + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::ViewEventFn = Rc::new(move |ctx, event| {
            let f = f.clone();
            Box::pin(async move { f(ctx, event).await })
        });
        guest::with(|h| h.view_events.insert(id.to_string(), cb));
    }

    /// Format documents: return the edits for the document, or for `range` of it.
    pub fn formatter<F>(&self, f: F)
    where
        F: AsyncFn(Context, Document, Option<Range>, FormatOptions) -> Result<Vec<TextEdit>> + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::FormatFn = Rc::new(move |ctx, doc, range, options| {
            let f = f.clone();
            Box::pin(async move { f(ctx, doc, range, options).await })
        });
        guest::with(|h| h.formatter = Some(cb));
    }

    /// Offer code actions for a range and the diagnostics on it.
    pub fn code_actions<F>(&self, f: F)
    where
        F: AsyncFn(Context, Document, Range, Vec<Diagnostic>) -> Result<Vec<CodeAction>> + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::CodeActionsFn = Rc::new(move |ctx, doc, range, diagnostics| {
            let f = f.clone();
            Box::pin(async move { f(ctx, doc, range, diagnostics).await })
        });
        guest::with(|h| h.code_actions = Some(cb));
    }

    /// The user chose a code action that has a `command`: its `id`.
    pub fn on_code_action<F>(&self, f: F)
    where
        F: AsyncFn(Context, String) -> Result<()> + 'static,
    {
        guest::with(|h| h.run_code_action = Some(string_fn(f)));
    }

    /// Describe what is at a position of a document of the manifest's `[[hover]]` languages: shown in the hover popup
    /// next to the language server's, under the extension's name. Answer within the provider deadline.
    pub fn hover<F>(&self, f: F)
    where
        F: AsyncFn(Context, Document, Position) -> Result<Option<Hover>> + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::HoverFn = Rc::new(move |ctx, doc, position| {
            let f = f.clone();
            Box::pin(async move { f(ctx, doc, position).await })
        });
        guest::with(|h| h.hover = Some(cb));
    }

    /// Offer completions at a position of a document of the manifest's `[[completion]]` languages: they join the
    /// language server's in the list. `trigger` is the character typed when one of the manifest's
    /// `trigger-characters` asked, none when the user typed a word or asked. Return a `Vec<CompletionItem>` or a
    /// [`CompletionList`]. Answer within the provider deadline: a late answer is dropped.
    pub fn completion<F, R>(&self, f: F)
    where
        F: AsyncFn(Context, Document, Position, Option<String>) -> Result<R> + 'static,
        R: Into<CompletionList>,
    {
        let f = Rc::new(f);
        let cb: guest::CompletionFn = Rc::new(move |ctx, doc, position, trigger| {
            let f = f.clone();
            Box::pin(async move { f(ctx, doc, position, trigger).await.map(Into::into) })
        });
        guest::with(|h| h.completion = Some(cb));
    }

    /// Code lenses for the lines of `document` in view (`range`): clickable text above a line that runs a command
    /// with arguments. Asked again after an edit (debounced) and when the view scrolls; a slow answer is dropped.
    pub fn code_lenses<F>(&self, f: F)
    where
        F: AsyncFn(Context, Document, Range) -> Result<Vec<CodeLens>> + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::CodeLensFn = Rc::new(move |ctx, doc, range| {
            let f = f.clone();
            Box::pin(async move { f(ctx, doc, range).await })
        });
        guest::with(|h| h.code_lenses = Some(cb));
    }

    /// Give a lens made with [`CodeLens::unresolved`] its command, once it is on screen.
    pub fn resolve_code_lens<F>(&self, f: F)
    where
        F: AsyncFn(Context, Document, CodeLens) -> Result<CodeLens> + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::ResolveLensFn = Rc::new(move |ctx, doc, lens| {
            let f = f.clone();
            Box::pin(async move { f(ctx, doc, lens).await })
        });
        guest::with(|h| h.resolve_code_lens = Some(cb));
    }

    /// Inlay hints for the lines of `document` in view (`range`): short text at the end of a line, such as a type.
    pub fn inlay_hints<F>(&self, f: F)
    where
        F: AsyncFn(Context, Document, Range) -> Result<Vec<InlayHint>> + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::InlayHintsFn = Rc::new(move |ctx, doc, range| {
            let f = f.clone();
            Box::pin(async move { f(ctx, doc, range).await })
        });
        guest::with(|h| h.inlay_hints = Some(cb));
    }

    /// A `codeyard://ext/<id>/…` link was opened: its path and query.
    pub fn on_uri<F>(&self, f: F)
    where
        F: AsyncFn(Context, String) -> Result<()> + 'static,
    {
        guest::with(|h| h.open_uri = Some(string_fn(f)));
    }

    /// The extension's settings changed: their keys.
    pub fn on_settings_changed<F>(&self, f: F)
    where
        F: AsyncFn(Context, Vec<String>) + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::KeysFn = Rc::new(move |ctx, keys| {
            let f = f.clone();
            Box::pin(async move { f(ctx, keys).await })
        });
        guest::with(|h| h.settings_changed.push(cb));
    }

    /// Run `f` every `seconds` (at least 30), paused while the window is unfocused. An active timer keeps the
    /// extension running.
    pub fn every<F>(&self, seconds: u32, f: F) -> Timer
    where
        F: AsyncFn(Context) + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::TickFn = Rc::new(move |ctx| {
            let f = f.clone();
            Box::pin(async move { f(ctx).await })
        });
        let id = sys::every(seconds);
        guest::with(|h| h.timers.insert(id, cb));
        Timer { id }
    }

    /// Run `f` for each document the extension may read (`read`) that opens, changes or is saved: the documents already
    /// open arrive first as [`DocumentChange::Opened`], and changes once typing pauses, with the latest version. With
    /// `text`, each event carries the document's whole text. An active watch keeps the extension running; stop it with
    /// [`Watch::stop`].
    ///
    /// ```ignore
    /// ctx.on_documents(false, async |ctx, e| {
    ///     if e.change == DocumentChange::Saved {
    ///         ctx.log(LogLevel::Info, format!("saved {}", e.uri));
    ///     }
    /// })?;
    /// ```
    pub fn on_documents<F>(&self, text: bool, f: F) -> Result<Watch>
    where
        F: AsyncFn(Context, DocumentEvent) + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::DocumentsFn = Rc::new(move |ctx, e| {
            let f = f.clone();
            Box::pin(async move { f(ctx, e).await })
        });
        let id = sys::watch_documents(text).map_err(|e| e.detailed(&|| r#"read = ["<glob>"]"#.to_string()))?;
        guest::with(|h| h.document_watches.insert(id, cb));
        Ok(Watch { id })
    }

    /// Run `f` with the files matching `glob` (relative to the workspace folders, as in `read`) that were created,
    /// changed or deleted, by Codeyard or anything else: one event per file, at most one batch a second, and only for
    /// files the extension may read. An active watch keeps the extension running; stop it with [`Watch::stop`].
    pub fn on_files<F>(&self, glob: &str, f: F) -> Result<Watch>
    where
        F: AsyncFn(Context, Vec<FileEvent>) + 'static,
    {
        let f = Rc::new(f);
        let cb: guest::FilesFn = Rc::new(move |ctx, events| {
            let f = f.clone();
            Box::pin(async move { f(ctx, events).await })
        });
        let id = sys::watch_files(glob).map_err(|e| e.detailed(&|| format!(r#"read = ["{glob}"]"#)))?;
        guest::with(|h| h.file_watches.insert(id, cb));
        Ok(Watch { id })
    }

    /// Run a task in the background, such as work started from `activate`.
    pub fn background(&self, task: impl Future<Output = ()> + 'static) {
        runtime::spawn(task);
    }

    /// Why the extension started: `command:<id>`, `view:<id>`, `workspace-contains`, …
    pub fn activation_reason(&self) -> String {
        guest::with(|h| h.reason.clone())
    }

    // ---- free calls ----------------------------------------------------------------------------------------------

    pub fn log(&self, level: LogLevel, message: impl AsRef<str>) {
        sys::log(level, message.as_ref());
    }

    pub fn env(&self) -> Env {
        sys::env()
    }

    /// A setting declared in the manifest, as `T`; none when unset and without a default.
    pub fn setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match sys::setting(key) {
            Some(json) => Ok(Some(serde_json::from_str(&json).map_err(|e| Error::Invalid(format!("setting {key}: {e}")))?)),
            None => Ok(None),
        }
    }

    pub fn storage(&self) -> Storage {
        Storage { _one_thread: PhantomData }
    }

    /// In-memory values that survive a hot reload in `cyx dev`: the SDK saves them when the old build stops and gives
    /// them back to the new one. Any other start begins empty. Use `storage` for what must outlast the session.
    pub fn state(&self) -> State {
        State { _one_thread: PhantomData }
    }

    /// Ask for view `id` to be drawn again.
    pub fn invalidate(&self, view: &str) {
        sys::invalidate(view);
    }

    // ---- window --------------------------------------------------------------------------------------------------

    /// An information notice; does not wait for it to be dismissed.
    pub fn notice(&self, message: impl AsRef<str>) {
        sys::show_notice(Severity::Info, message.as_ref(), &[]);
    }

    /// A notice with buttons: the index of the one clicked, or none when dismissed.
    pub async fn notify(&self, severity: Severity, message: &str, actions: &[&str]) -> Result<Option<usize>> {
        let actions: Vec<String> = actions.iter().map(|s| s.to_string()).collect();
        match call(sys::show_notice(severity, message, &actions), free).await? {
            Completion::Chosen(c) => Ok(c.first().map(|&i| i as usize)),
            Completion::Done => Ok(None),
            c => Err(unexpected(c)),
        }
    }

    /// One item from a list, or none when dismissed.
    pub async fn quick_pick<I: Into<PickItem>>(&self, items: impl IntoIterator<Item = I>, options: PickOptions) -> Result<Option<usize>> {
        let items: Vec<PickItem> = items.into_iter().map(Into::into).collect();
        let options = PickOptions { many: false, ..options };
        match call(sys::quick_pick(&items, &options), free).await? {
            Completion::Chosen(c) => Ok(c.first().map(|&i| i as usize)),
            c => Err(unexpected(c)),
        }
    }

    /// Any number of items; empty when dismissed.
    pub async fn quick_pick_many<I: Into<PickItem>>(&self, items: impl IntoIterator<Item = I>, options: PickOptions) -> Result<Vec<usize>> {
        let items: Vec<PickItem> = items.into_iter().map(Into::into).collect();
        let options = PickOptions { many: true, ..options };
        match call(sys::quick_pick(&items, &options), free).await? {
            Completion::Chosen(c) => Ok(c.into_iter().map(|i| i as usize).collect()),
            c => Err(unexpected(c)),
        }
    }

    /// What the user typed, or none when dismissed.
    pub async fn input(&self, options: InputOptions) -> Result<Option<String>> {
        match call(sys::input_box(&options), free).await? {
            Completion::Text(t) => Ok(t),
            c => Err(unexpected(c)),
        }
    }

    /// Show progress until the returned handle is dropped or ended.
    pub fn progress(&self, title: &str, cancellable: bool) -> Progress {
        Progress { id: sys::start_progress(title, cancellable), ended: false }
    }

    pub fn set_status_item(&self, item: StatusItem) -> Result<()> {
        sys::set_status_item(&item)
    }

    pub fn remove_status_item(&self, id: &str) {
        sys::remove_status_item(id);
    }

    /// Open an `https:` URL in the browser; only while handling a user action.
    pub async fn open_external(&self, url: &str) -> Result<()> {
        call(sys::open_external(url), free).await.map(|_| ())
    }

    /// Run a command: the extension's own, a dependency's, or a core one the manifest lists.
    pub async fn execute(&self, command: &str, args: Value) -> Result<Value> {
        match call(sys::execute(command, &args.to_string()), || format!(r#"commands = ["{command}"]"#)).await? {
            Completion::Json(j) if j.trim().is_empty() => Ok(Value::Null),
            Completion::Json(j) => Ok(serde_json::from_str(&j)?),
            Completion::Done => Ok(Value::Null),
            c => Err(unexpected(c)),
        }
    }

    /// Open a workspace file in the editor, with `selection` selected and scrolled into view. Runs
    /// `core:editor.open`, so the manifest lists it in `permissions.commands` and `read` covers the file.
    pub async fn open_in_editor(&self, uri: &str, selection: Option<Range>) -> Result<()> {
        let args = match selection {
            None => Value::String(uri.to_string()),
            Some(r) => serde_json::json!({
                "uri": uri,
                "selection": {
                    "start": { "line": r.start.line, "character": r.start.character },
                    "end": { "line": r.end.line, "character": r.end.character },
                },
            }),
        };
        self.execute("core:editor.open", args).await.map(drop)
    }

    // ---- documents and languages ---------------------------------------------------------------------------------

    /// The active editor's document. A command the user started may read it without `read`.
    pub async fn active_document(&self) -> Result<Option<Document>> {
        match call(sys::active_document(), || line_for("read")).await? {
            Completion::Document(d) => Ok(d),
            c => Err(unexpected(c)),
        }
    }

    pub async fn selections(&self) -> Result<Vec<Range>> {
        match call(sys::selections(), || line_for("read")).await? {
            Completion::Ranges(r) => Ok(r),
            c => Err(unexpected(c)),
        }
    }

    /// An open document, or none when it is not open.
    pub async fn read_document(&self, uri: &str) -> Result<Option<Document>> {
        match call(sys::read_document(uri), || path_line("read", uri)).await? {
            Completion::Document(d) => Ok(d),
            c => Err(unexpected(c)),
        }
    }

    /// Apply edits made against `version`; fails with `Invalid` when the document changed since.
    pub async fn apply_edits(&self, uri: &str, version: u32, edits: &[TextEdit]) -> Result<()> {
        call(sys::apply_edits(uri, version, edits), || path_line("write", uri)).await.map(|_| ())
    }

    pub async fn set_selections(&self, uri: &str, selections: &[Range]) -> Result<()> {
        call(sys::set_selections(uri, selections), free).await.map(|_| ())
    }

    /// Replace this extension's diagnostics for `uri`.
    pub fn set_diagnostics(&self, uri: &str, diagnostics: &[Diagnostic]) {
        sys::set_diagnostics(uri, diagnostics);
    }

    /// Remove this extension's diagnostics for `uri`, or everywhere.
    pub fn clear_diagnostics(&self, uri: Option<&str>) {
        sys::clear_diagnostics(uri);
    }

    /// Replace the named set of decorations on `uri`.
    pub fn set_decorations(&self, uri: &str, set: &str, decorations: &[Decoration]) {
        sys::set_decorations(uri, set, decorations);
    }

    // ---- workspace (`read`, `write`) -----------------------------------------------------------------------------

    /// The workspace's folders; empty without `read`.
    pub fn folders(&self) -> Vec<Uri> {
        sys::folders()
    }

    pub async fn read_file(&self, uri: &str) -> Result<Vec<u8>> {
        match call(sys::read_file(uri), || path_line("read", uri)).await? {
            Completion::Bytes(b) => Ok(b),
            c => Err(unexpected(c)),
        }
    }

    /// Read several files in one round trip, each with its own result, in the order asked. Much faster than one
    /// `read_file` after another: 2000 sequential reads take seconds, one `read_many` a fraction of that.
    pub async fn read_many(&self, uris: &[&str]) -> Result<Vec<(Uri, Result<Vec<u8>>)>> {
        let first = uris.first().copied().unwrap_or_default().to_string();
        match call(sys::read_many(uris), || path_line("read", &first)).await? {
            Completion::Files(files) => Ok(files
                .into_iter()
                .map(|(uri, r)| {
                    let line = path_line("read", &uri);
                    (uri, r.map_err(|e| e.detailed(&|| line.clone())))
                })
                .collect()),
            c => Err(unexpected(c)),
        }
    }

    pub async fn read_text(&self, uri: &str) -> Result<String> {
        Ok(String::from_utf8(self.read_file(uri).await?)?)
    }

    pub async fn stat(&self, uri: &str) -> Result<FileStat> {
        match call(sys::stat(uri), || path_line("read", uri)).await? {
            Completion::Stat(s) => Ok(s),
            c => Err(unexpected(c)),
        }
    }

    /// Files matching `glob` under the workspace folders, at most `max`.
    pub async fn find_files(&self, glob: &str, max: u32) -> Result<Vec<Uri>> {
        match call(sys::find_files(glob, max), || format!(r#"read = ["{glob}"]"#)).await? {
            Completion::Uris(u) => Ok(u),
            c => Err(unexpected(c)),
        }
    }

    /// Search the workspace's files; each match is passed to `found` as it arrives.
    pub async fn search(&self, query: &SearchQuery, mut found: impl FnMut(SearchMatch)) -> Result<()> {
        let glob = query.include.first().cloned().unwrap_or_else(|| "<glob>".into());
        let mut pending = Pending::new(sys::search(query));
        while let Some(e) = pending.next_event().await {
            if let StreamEvent::SearchMatch { uri, range, line } = e {
                found(SearchMatch { uri, range, line });
            }
        }
        match pending.await {
            Completion::Done => Ok(()),
            Completion::Failed(e) => Err(e.detailed(&|| format!(r#"read = ["{glob}"]"#))),
            c => Err(unexpected(c)),
        }
    }

    pub async fn write_file(&self, uri: &str, contents: &[u8]) -> Result<()> {
        call(sys::write_file(uri, contents), || path_line("write", uri)).await.map(|_| ())
    }

    pub async fn delete_file(&self, uri: &str) -> Result<()> {
        call(sys::delete_file(uri), || path_line("write", uri)).await.map(|_| ())
    }

    /// Move or rename a file or folder; both paths need `write`.
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        call(sys::rename(from, to), || path_line("write", to)).await.map(|_| ())
    }

    /// Create a folder and any missing parents.
    pub async fn create_directory(&self, uri: &str) -> Result<()> {
        call(sys::create_directory(uri), || path_line("write", uri)).await.map(|_| ())
    }

    // ---- network (`net`, `auth`) ---------------------------------------------------------------------------------

    /// Send an HTTPS request to a host listed in `net`, and read the whole response.
    pub async fn fetch(&self, request: Request) -> Result<Response> {
        let host = request.host();
        net::collect(sys::fetch(&request.0), host, "net").await
    }

    /// Like [`fetch`](Self::fetch), signed for an `auth` provider; the extension never sees the token.
    pub async fn fetch_as(&self, provider: &str, request: Request) -> Result<Response> {
        net::collect(sys::auth_fetch(provider, &request.0), provider.to_string(), "auth").await
    }

    /// Sign in to `provider` if needed (the first use asks the user).
    pub async fn session(&self, provider: &str, scopes: &[&str]) -> Result<Account> {
        let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
        match call(sys::session(provider, &scopes), || format!(r#"auth = ["{provider}"]"#)).await? {
            Completion::Account(a) => Ok(a),
            c => Err(unexpected(c)),
        }
    }

    // ---- processes (`run`) ---------------------------------------------------------------------------------------

    /// Start a program listed in `run`; read its output as it comes with [`Process::next`].
    pub fn spawn(&self, command: Command) -> Process {
        let program = command.program().to_string();
        Process::new(sys::spawn(&command.0), program)
    }

    /// Run a program to the end and collect its output.
    pub async fn run(&self, command: Command) -> Result<Output> {
        let mut p = self.spawn(command);
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        while let Some(chunk) = p.next().await {
            match chunk {
                Chunk::Stdout(b) => stdout.extend_from_slice(&b),
                Chunk::Stderr(b) => stderr.extend_from_slice(&b),
            }
        }
        Ok(Output { code: p.wait().await?, stdout, stderr })
    }

    // ---- secrets, clipboard, terminal ----------------------------------------------------------------------------

    pub async fn secret(&self, key: &str) -> Result<Option<String>> {
        match call(sys::secret_get(key), || line_for("secrets")).await? {
            Completion::Text(t) => Ok(t),
            c => Err(unexpected(c)),
        }
    }

    pub async fn set_secret(&self, key: &str, value: &str) -> Result<()> {
        call(sys::secret_set(key, value), || line_for("secrets")).await.map(|_| ())
    }

    pub async fn delete_secret(&self, key: &str) -> Result<()> {
        call(sys::secret_delete(key), || line_for("secrets")).await.map(|_| ())
    }

    pub async fn read_clipboard(&self) -> Result<Option<String>> {
        match call(sys::read_clipboard(), || r#"clipboard = "read""#.into()).await? {
            Completion::Text(t) => Ok(t),
            c => Err(unexpected(c)),
        }
    }

    pub async fn write_clipboard(&self, text: &str) -> Result<()> {
        call(sys::write_clipboard(text), || r#"clipboard = "write""#.into()).await.map(|_| ())
    }

    /// Type `text` into the active terminal.
    pub async fn send_to_terminal(&self, text: &str) -> Result<()> {
        call(sys::send_text(text), || line_for("terminal")).await.map(|_| ())
    }
}

fn string_fn<F>(f: F) -> guest::StringFn
where
    F: AsyncFn(Context, String) -> Result<()> + 'static,
{
    let f = Rc::new(f);
    Rc::new(move |ctx, s| -> BoxFuture<Result<()>> {
        let f = f.clone();
        Box::pin(async move { f(ctx, s).await })
    })
}

/// The extension's persisted key-value store (50 MiB in all).
pub struct Storage {
    _one_thread: PhantomData<*const ()>,
}

impl Storage {
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        sys::storage_get(key)
    }

    pub fn set(&self, key: &str, value: &[u8]) -> Result<()> {
        sys::storage_set(key, value)
    }

    pub fn get_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        self.get(key).map(|b| serde_json::from_slice(&b)).transpose().map_err(Into::into)
    }

    pub fn set_json(&self, key: &str, value: &impl Serialize) -> Result<()> {
        self.set(key, &serde_json::to_vec(value)?)
    }

    pub fn delete(&self, key: &str) {
        sys::storage_delete(key);
    }

    pub fn keys(&self) -> Vec<String> {
        sys::storage_keys()
    }
}

/// The extension's in-memory state (`Context::state`), as JSON values by key.
pub struct State {
    _one_thread: PhantomData<*const ()>,
}

impl State {
    /// The value under `key` as `T`; none when unset or of another shape.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        guest::with(|h| h.state.get(key).cloned()).and_then(|v| serde_json::from_value(v).ok())
    }

    pub fn set(&self, key: &str, value: &impl Serialize) -> Result<()> {
        let v = serde_json::to_value(value)?;
        guest::with(|h| h.state.insert(key.to_string(), v));
        Ok(())
    }

    /// Forget the value under `key`. (Named `remove` before 1.0; `delete` as in [`Storage::delete`] and the
    /// TypeScript SDK.)
    pub fn delete(&self, key: &str) {
        guest::with(|h| h.state.remove(key));
    }

    /// The keys that have a value.
    pub fn keys(&self) -> Vec<String> {
        guest::with(|h| h.state.keys().cloned().collect())
    }
}

/// Progress shown until ended or dropped.
pub struct Progress {
    id: u64,
    ended: bool,
}

impl Progress {
    pub fn report(&self, message: Option<&str>, percent: Option<u8>) {
        sys::report_progress(self.id, message, percent);
    }

    /// The user cancelled it (only when started as cancellable).
    pub fn is_cancelled(&self) -> bool {
        guest::with(|h| h.cancelled_progress.contains(&self.id))
    }

    pub fn end(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        if !self.ended {
            self.ended = true;
            sys::end_progress(self.id);
            guest::with(|h| h.cancelled_progress.remove(&self.id));
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.finish();
    }
}

/// A watch from [`Context::on_documents`] or [`Context::on_files`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watch {
    id: u64,
}

impl Watch {
    pub fn stop(self) {
        sys::stop_watch(self.id);
        guest::with(|h| {
            h.document_watches.remove(&self.id);
            h.file_watches.remove(&self.id);
        });
    }
}

/// A repeating timer from [`Context::every`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timer {
    id: u64,
}

impl Timer {
    pub fn clear(self) {
        sys::clear_timer(self.id);
        guest::with(|h| h.timers.remove(&self.id));
    }
}
