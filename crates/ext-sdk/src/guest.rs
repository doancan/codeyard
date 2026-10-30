//! The guest's exports, shared by the WebAssembly glue and the native mock host: activation, and each request routed to
//! what the extension registered. Every export ends by running the tasks it woke.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::protocol::{Completion, Request, Response, StreamEvent};
use crate::types::*;
use crate::{Context, Extension, runtime, sys};

pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = T>>>;
pub(crate) type CommandFn = Rc<dyn Fn(Context, Value) -> BoxFuture<Result<Value>>>;
pub(crate) type ViewFn = Rc<dyn Fn(Context) -> BoxFuture<Result<Value>>>;
pub(crate) type ViewEventFn = Rc<dyn Fn(Context, Value) -> BoxFuture<Result<()>>>;
pub(crate) type FormatFn = Rc<dyn Fn(Context, Document, Option<Range>, FormatOptions) -> BoxFuture<Result<Vec<TextEdit>>>>;
pub(crate) type CodeActionsFn = Rc<dyn Fn(Context, Document, Range, Vec<Diagnostic>) -> BoxFuture<Result<Vec<CodeAction>>>>;
pub(crate) type HoverFn = Rc<dyn Fn(Context, Document, Position) -> BoxFuture<Result<Option<Hover>>>>;
pub(crate) type CompletionFn = Rc<dyn Fn(Context, Document, Position, Option<String>) -> BoxFuture<Result<CompletionList>>>;
pub(crate) type CodeLensFn = Rc<dyn Fn(Context, Document, Range) -> BoxFuture<Result<Vec<CodeLens>>>>;
pub(crate) type ResolveLensFn = Rc<dyn Fn(Context, Document, CodeLens) -> BoxFuture<Result<CodeLens>>>;
pub(crate) type InlayHintsFn = Rc<dyn Fn(Context, Document, Range) -> BoxFuture<Result<Vec<InlayHint>>>>;
pub(crate) type StringFn = Rc<dyn Fn(Context, String) -> BoxFuture<Result<()>>>;
pub(crate) type KeysFn = Rc<dyn Fn(Context, Vec<String>) -> BoxFuture<()>>;
pub(crate) type TickFn = Rc<dyn Fn(Context) -> BoxFuture<()>>;

/// What the extension registered.
#[derive(Default)]
pub(crate) struct Handlers {
    pub commands: HashMap<String, CommandFn>,
    pub views: HashMap<String, ViewFn>,
    pub view_events: HashMap<String, ViewEventFn>,
    pub formatter: Option<FormatFn>,
    pub code_actions: Option<CodeActionsFn>,
    pub run_code_action: Option<StringFn>,
    pub open_uri: Option<StringFn>,
    pub code_lenses: Option<CodeLensFn>,
    pub resolve_code_lens: Option<ResolveLensFn>,
    pub inlay_hints: Option<InlayHintsFn>,
    pub settings_changed: Vec<KeysFn>,
    pub timers: HashMap<u64, TickFn>,
    pub hover: Option<HoverFn>,
    pub completion: Option<CompletionFn>,
    /// Progress the user cancelled.
    pub cancelled_progress: HashSet<u64>,
    pub reason: String,
    /// `ctx.state()`: kept across a hot reload in `cyx dev` (`RELOAD_STATE_KEY`).
    pub state: serde_json::Map<String, Value>,
}

thread_local! {
    pub(crate) static HANDLERS: RefCell<Handlers> = RefCell::new(Handlers::default());
}

pub(crate) fn with<R>(f: impl FnOnce(&mut Handlers) -> R) -> R {
    HANDLERS.with(|h| f(&mut h.borrow_mut()))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn reset() {
    runtime::reset();
    let old = HANDLERS.with(|h| std::mem::take(&mut *h.borrow_mut()));
    drop(old);
}

/// The storage key under which `ctx.state()` waits for the next activation during a hot reload. Reserved: the SDKs
/// write it on `deactivate` and delete it on the next `activate`.
pub const RELOAD_STATE_KEY: &str = "codeyard.reload-state";

/// The activation reason of a hot reload (`cyx dev` swapped in a new build).
pub const RELOAD: &str = "reload";

/// `guest.activate`.
pub fn activate<E: Extension>(reason: &str) -> std::result::Result<(), String> {
    with(|h| h.reason = reason.to_string());
    // The snapshot is for the activation right after a reload; any other start forgets it.
    if let Some(bytes) = sys::storage_get(RELOAD_STATE_KEY) {
        sys::storage_delete(RELOAD_STATE_KEY);
        if reason == RELOAD
            && let Ok(Value::Object(state)) = serde_json::from_slice(&bytes)
        {
            with(|h| h.state = state);
        }
    }
    let r = E::activate(Context::new()).map_err(|e| e.to_string());
    runtime::run();
    r
}

/// `guest.deactivate`.
pub fn deactivate<E: Extension>() {
    E::deactivate(Context::new());
    runtime::run();
    let state = with(|h| std::mem::take(&mut h.state));
    if !state.is_empty() {
        let _ = serde_json::to_vec(&Value::Object(state)).map(|bytes| sys::storage_set(RELOAD_STATE_KEY, &bytes));
    }
}

/// `guest.complete`.
pub fn complete(ticket: Ticket, c: Completion) {
    runtime::complete(ticket, c);
    runtime::run();
}

/// `guest.event`.
pub fn event(ticket: Ticket, e: StreamEvent) {
    runtime::event(ticket, e);
    runtime::run();
}

/// `guest.cancel`.
pub fn cancel(id: u64) {
    runtime::cancel_request(id);
    runtime::run();
}

/// `guest.handle`: answer now, or spawn the task that answers.
pub fn handle(id: u64, request: Request) {
    let ctx = Context::new();
    let missing = |what: String| sys::respond(id, Err(Error::NotFound(what)));
    match request {
        Request::Command { command, args_json } => match with(|h| h.commands.get(&command).cloned()) {
            Some(f) => {
                let args = if args_json.trim().is_empty() { Ok(Value::Null) } else { serde_json::from_str(&args_json) };
                match args {
                    Ok(args) => answer(id, async move { f(ctx, args).await.map(|v| Response::Json(v.to_string())) }),
                    Err(e) => sys::respond(id, Err(Error::Invalid(format!("the arguments of {command} are not JSON: {e}")))),
                }
            }
            None => missing(format!("no command '{command}' is registered; call ctx.command(\"{command}\", …) in activate")),
        },
        Request::Render(view) => match with(|h| h.views.get(&view).cloned()) {
            Some(f) => answer(id, async move { f(ctx).await.map(|v| Response::View(v.to_string())) }),
            None => missing(format!("no view '{view}' is registered; call ctx.view(\"{view}\", …) in activate")),
        },
        Request::UiEvent { view, event_json } => match with(|h| h.view_events.get(&view).cloned()) {
            Some(f) => match serde_json::from_str(&event_json) {
                Ok(event) => answer(id, async move { f(ctx, event).await.map(|()| Response::None) }),
                Err(e) => sys::respond(id, Err(Error::Invalid(format!("the event is not JSON: {e}")))),
            },
            None => sys::respond(id, Ok(Response::None)),
        },
        Request::Format { document, range, options } => match with(|h| h.formatter.clone()) {
            Some(f) => answer(id, async move { f(ctx, document, range, options).await.map(Response::Edits) }),
            None => missing("no formatter is registered; call ctx.formatter(…) in activate".into()),
        },
        Request::CodeActions { document, range, diagnostics } => match with(|h| h.code_actions.clone()) {
            Some(f) => answer(id, async move { f(ctx, document, range, diagnostics).await.map(Response::CodeActions) }),
            None => sys::respond(id, Ok(Response::CodeActions(Vec::new()))),
        },
        Request::RunCodeAction(action) => match with(|h| h.run_code_action.clone()) {
            Some(f) => answer(id, async move { f(ctx, action).await.map(|()| Response::None) }),
            None => missing("no code action handler is registered; call ctx.on_code_action(…) in activate".into()),
        },
        Request::OpenUri(uri) => match with(|h| h.open_uri.clone()) {
            Some(f) => answer(id, async move { f(ctx, uri).await.map(|()| Response::None) }),
            None => sys::respond(id, Ok(Response::None)),
        },
        Request::SettingsChanged(keys) => {
            // Answered at once; the handlers run in the background.
            sys::respond(id, Ok(Response::None));
            for f in with(|h| h.settings_changed.clone()) {
                runtime::spawn(f(ctx, keys.clone()));
            }
        }
        Request::Timer(timer) => {
            sys::respond(id, Ok(Response::None));
            if let Some(f) = with(|h| h.timers.get(&timer).cloned()) {
                runtime::spawn(f(ctx));
            }
        }
        Request::CancelProgress(p) => {
            with(|h| h.cancelled_progress.insert(p));
            sys::respond(id, Ok(Response::None));
        }
        // Without a provider, nothing to add: the language server's answer shows alone.
        Request::Hover { document, position } => match with(|h| h.hover.clone()) {
            Some(f) => answer(id, async move { f(ctx, document, position).await.map(Response::Hover) }),
            None => sys::respond(id, Ok(Response::Hover(None))),
        },
        Request::Completion { document, position, trigger } => match with(|h| h.completion.clone()) {
            Some(f) => answer(id, async move { f(ctx, document, position, trigger).await.map(Response::Completions) }),
            None => sys::respond(id, Ok(Response::Completions(CompletionList::default()))),
        },
        Request::CodeLens { document, range } => match with(|h| h.code_lenses.clone()) {
            Some(f) => answer(id, async move { f(ctx, document, range).await.map(Response::CodeLenses) }),
            None => sys::respond(id, Ok(Response::CodeLenses(Vec::new()))),
        },
        // Without a resolver, the lens comes back as it was (and stays hidden).
        Request::ResolveCodeLens { document, lens } => match with(|h| h.resolve_code_lens.clone()) {
            Some(f) => answer(id, async move { f(ctx, document, lens).await.map(|l| Response::CodeLenses(vec![l])) }),
            None => sys::respond(id, Ok(Response::CodeLenses(vec![lens]))),
        },
        Request::InlayHints { document, range } => match with(|h| h.inlay_hints.clone()) {
            Some(f) => answer(id, async move { f(ctx, document, range).await.map(Response::InlayHints) }),
            None => sys::respond(id, Ok(Response::InlayHints(Vec::new()))),
        },
        Request::ProvideTasks | Request::RunTask(_) => sys::respond(id, Err(Error::Invalid("this SDK does not provide tasks yet".into()))),
    }
    runtime::run();
}

fn answer(id: u64, fut: impl Future<Output = Result<Response>> + 'static) {
    runtime::spawn_request(id, async move {
        let r = fut.await;
        sys::respond(id, r);
    });
}
