//! `codeyard-extension-test`: a mock Codeyard for an extension's unit tests, run natively with `cargo test`
//! (docs/extensions/testing.md). Fake files, fixture HTTP responses, recorded processes, scripted answers to pickers
//! and inputs, and a virtual clock for timers. Calls are checked against the extension's `codeyard-extension.toml`, so
//! a test fails where Codeyard would refuse the call, with the same fix.
//!
//! ```ignore
//! let host = MockHost::new();
//! host.answer_input("Ada");
//! host.activate::<Hello>().unwrap();
//! host.command("greet", json!(null)).unwrap();
//! assert_eq!(host.notices(), ["Hello, Ada!"]);
//! ```

mod host;
mod scm;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use codeyard_extension::native::{self, Note};
use codeyard_extension::protocol::{Request, Response};
use codeyard_extension::types::*;
use codeyard_extension::{Denied, Error, Extension, Result, Value};

use host::{Shared, State};

pub use codeyard_extension::native::Call;

/// A recorded or scripted process run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessFixture {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: Option<i32>,
}

impl ProcessFixture {
    pub fn ok(stdout: impl Into<Vec<u8>>) -> ProcessFixture {
        ProcessFixture { stdout: stdout.into(), stderr: Vec::new(), code: Some(0) }
    }

    pub fn failed(code: i32, stderr: impl Into<Vec<u8>>) -> ProcessFixture {
        ProcessFixture { stdout: Vec::new(), stderr: stderr.into(), code: Some(code) }
    }
}

/// A mock Codeyard for one extension on the current thread. Every method takes `&self`, so set up and inspect it in
/// any order.
pub struct MockHost {
    state: Rc<RefCell<State>>,
    next_request: RefCell<u64>,
}

impl Default for MockHost {
    fn default() -> Self {
        MockHost::new()
    }
}

impl MockHost {
    /// A mock with the permissions and setting defaults of the `codeyard-extension.toml` next to the crate's
    /// `Cargo.toml` (or everything allowed when there is none). The workspace folder is `file:///work`.
    pub fn new() -> MockHost {
        let manifest = std::env::var_os("CARGO_MANIFEST_DIR").map(|d| Path::new(&d).join(ext_manifest::FILE_NAME)).filter(|p| p.is_file());
        let host = MockHost::allow_all();
        if let Some(path) = manifest {
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            host.manifest(&text);
        }
        host
    }

    /// A mock that allows every call.
    pub fn allow_all() -> MockHost {
        let state = Rc::new(RefCell::new(State::default()));
        native::install(Box::new(Shared(state.clone())));
        MockHost { state, next_request: RefCell::new(0) }
    }

    /// Check calls against this manifest's permissions, and use its setting defaults.
    pub fn manifest(&self, text: &str) -> &Self {
        let m = ext_manifest::parse_with(text, ext_manifest::Options { require_why: false }).unwrap_or_else(|errors| {
            let all: Vec<String> = errors.iter().map(|d| d.render(ext_manifest::FILE_NAME, text)).collect();
            panic!("the manifest is invalid:\n{}", all.join("\n"))
        });
        let mut s = self.state.borrow_mut();
        for setting in &m.contributions.settings {
            let json = setting.default.as_ref().and_then(|v| serde_json::to_string(v).ok());
            s.settings.entry(setting.key.clone()).or_insert(json);
        }
        s.permissions = Some(m.permissions);
        drop(s);
        self
    }

    /// Refuse sending, writing and running, as in an untrusted folder.
    pub fn untrusted(&self) -> &Self {
        self.state.borrow_mut().untrusted = true;
        self
    }

    // ---- fixtures ----------------------------------------------------------------------------------------------------

    /// A workspace file. A path without a scheme is relative to the workspace folder.
    pub fn file(&self, path: &str, contents: impl Into<Vec<u8>>) -> &Self {
        let uri = self.uri(path);
        self.state.borrow_mut().files.insert(uri, contents.into());
        self
    }

    /// The uri the mock uses for a workspace-relative path.
    pub fn uri(&self, path: &str) -> String {
        if path.contains("://") { path.to_string() } else { format!("{}/{}", host::FOLDER, path.trim_start_matches('/')) }
    }

    /// The response to `method url` (any method when `method` is `*`).
    pub fn http(&self, method: &str, url: &str, status: u16, body: impl Into<Vec<u8>>) -> &Self {
        self.state.borrow_mut().http.push(host::HttpFixture { method: method.to_string(), url: url.to_string(), status, body: body.into() });
        self
    }

    /// What running `program` with these exact `args` prints and returns.
    pub fn process(&self, program: &str, args: &[&str], result: ProcessFixture) -> &Self {
        let args = args.iter().map(|a| a.to_string()).collect();
        self.state.borrow_mut().processes.push((program.to_string(), args, result));
        self
    }

    /// The active editor's document.
    pub fn active_document(&self, path: &str, language: &str, text: &str) -> &Self {
        let uri = self.uri(path);
        self.state.borrow_mut().active = Some(Document { uri, language: language.into(), version: 1, text: text.into() });
        self
    }

    pub fn selections(&self, ranges: &[Range]) -> &Self {
        self.state.borrow_mut().selections = ranges.to_vec();
        self
    }

    /// A setting's value as JSON (`"60"`, `"\"text\""`), or unset.
    pub fn setting(&self, key: &str, json: Option<&str>) -> &Self {
        self.state.borrow_mut().settings.insert(key.to_string(), json.map(Into::into));
        self
    }

    /// The result of `ctx.execute(command, …)`, as JSON.
    pub fn command_result(&self, command: &str, json: &str) -> &Self {
        self.state.borrow_mut().command_results.insert(command.to_string(), json.to_string());
        self
    }

    pub fn secret(&self, key: &str, value: &str) -> &Self {
        self.state.borrow_mut().secrets.insert(key.to_string(), value.to_string());
        self
    }

    pub fn clipboard_text(&self, text: &str) -> &Self {
        self.state.borrow_mut().clipboard = Some(text.to_string());
        self
    }

    // ---- scripted answers (the next input box, pick or notice gets the next answer; none means dismissed) ------------

    pub fn answer_input(&self, text: &str) -> &Self {
        self.state.borrow_mut().inputs.push_back(Some(text.to_string()));
        self
    }

    pub fn dismiss_input(&self) -> &Self {
        self.state.borrow_mut().inputs.push_back(None);
        self
    }

    /// The indices picked in the next quick pick.
    pub fn answer_pick(&self, indices: &[u32]) -> &Self {
        self.state.borrow_mut().picks.push_back(indices.to_vec());
        self
    }

    /// The button clicked on the next notice that has buttons.
    pub fn answer_notice(&self, index: u32) -> &Self {
        self.state.borrow_mut().notice_answers.push_back(index);
        self
    }

    // ---- driving the extension ------------------------------------------------------------------------------------

    /// Call the extension's `activate`, then deliver what it waits on.
    pub fn activate<E: Extension>(&self) -> std::result::Result<(), String> {
        self.activate_for::<E>("command")
    }

    pub fn activate_for<E: Extension>(&self, reason: &str) -> std::result::Result<(), String> {
        let r = native::activate::<E>(reason);
        self.pump();
        r
    }

    pub fn deactivate<E: Extension>(&self) {
        native::deactivate::<E>();
        self.pump();
    }

    /// A hot reload as `cyx dev` does it: `deactivate`, forget everything the extension registered, then `activate`
    /// with the reason `reload`. Storage is kept, and with it what `ctx.state()` held.
    pub fn reload<E: Extension>(&self) -> std::result::Result<(), String> {
        self.deactivate::<E>();
        native::forget();
        self.activate_for::<E>(codeyard_extension::RELOAD)
    }

    /// Run the manifest's command `id` (its local id) and return its result.
    pub fn command(&self, id: &str, args: Value) -> Result<Value> {
        let args_json = if args.is_null() { String::new() } else { args.to_string() };
        match self.request(Request::Command { command: id.to_string(), args_json })? {
            Response::Json(j) if j.is_empty() => Ok(Value::Null),
            Response::Json(j) => Ok(serde_json::from_str(&j)?),
            other => Err(Error::Other(format!("command {id} answered {other:?}"))),
        }
    }

    /// Draw view `id`: its element tree.
    pub fn render(&self, id: &str) -> Result<Value> {
        match self.request(Request::Render(id.to_string()))? {
            Response::View(v) => Ok(serde_json::from_str(&v)?),
            other => Err(Error::Other(format!("view {id} answered {other:?}"))),
        }
    }

    /// Send an event to view `id`.
    pub fn view_event(&self, id: &str, event: Value) -> Result<()> {
        self.request(Request::UiEvent { view: id.to_string(), event_json: event.to_string() }).map(|_| ())
    }

    pub fn format(&self, document: Document, range: Option<Range>, options: FormatOptions) -> Result<Vec<TextEdit>> {
        match self.request(Request::Format { document, range, options })? {
            Response::Edits(e) => Ok(e),
            other => Err(Error::Other(format!("the formatter answered {other:?}"))),
        }
    }

    pub fn code_actions(&self, document: Document, range: Range, diagnostics: Vec<Diagnostic>) -> Result<Vec<CodeAction>> {
        match self.request(Request::CodeActions { document, range, diagnostics })? {
            Response::CodeActions(a) => Ok(a),
            other => Err(Error::Other(format!("the code action provider answered {other:?}"))),
        }
    }

    pub fn run_code_action(&self, id: &str) -> Result<()> {
        self.request(Request::RunCodeAction(id.to_string())).map(|_| ())
    }

    pub fn open_uri(&self, uri: &str) -> Result<()> {
        self.request(Request::OpenUri(uri.to_string())).map(|_| ())
    }

    /// Change a setting and tell the extension.
    pub fn change_setting(&self, key: &str, json: Option<&str>) {
        self.setting(key, json);
        let _ = self.request(Request::SettingsChanged(vec![key.to_string()]));
    }

    /// The user cancels progress `id`.
    pub fn cancel_progress(&self, id: u64) {
        let _ = self.request(Request::CancelProgress(id));
    }

    /// Move the virtual clock forward, firing each timer once per period that ends in that time.
    pub fn advance(&self, by: Duration) {
        let end = self.state.borrow().clock + by.as_secs();
        loop {
            let due = {
                let s = self.state.borrow();
                s.timers.iter().filter(|(_, t)| t.next <= end).min_by_key(|(id, t)| (t.next, **id)).map(|(id, t)| (*id, t.next))
            };
            let Some((id, at)) = due else { break };
            {
                let mut s = self.state.borrow_mut();
                s.clock = at;
                if let Some(t) = s.timers.get_mut(&id) {
                    t.next += t.period;
                }
            }
            let _ = self.request(Request::Timer(id));
        }
        self.state.borrow_mut().clock = end;
    }

    /// Send any request and wait for its answer.
    pub fn request(&self, request: Request) -> Result<Response> {
        let id = {
            let mut n = self.next_request.borrow_mut();
            *n += 1;
            *n
        };
        native::handle(id, request);
        self.pump();
        self.state
            .borrow_mut()
            .responses
            .remove(&id)
            .unwrap_or_else(|| Err(Error::Other("the extension did not answer: it waits on something the mock host never finishes".into())))
    }

    /// Deliver every completion and event the mock has queued, until the extension stops making calls.
    fn pump(&self) {
        loop {
            let next = self.state.borrow_mut().queue.pop_front();
            match next {
                Some(host::Delivery::Event(t, e)) => native::event(t, e),
                Some(host::Delivery::Complete(t, c)) => native::complete(t, c),
                None => return,
            }
        }
    }

    // ---- what happened ----------------------------------------------------------------------------------------------

    /// The text of every notice shown, in order.
    pub fn notices(&self) -> Vec<String> {
        self.state.borrow().notices.iter().map(|(_, m)| m.clone()).collect()
    }

    pub fn notices_with_severity(&self) -> Vec<(Severity, String)> {
        self.state.borrow().notices.clone()
    }

    /// Every call that was refused, with the manifest line that would allow it.
    pub fn denied(&self) -> Vec<Denied> {
        self.state.borrow().denied.clone()
    }

    /// Every request the extension sent, fixture or not.
    pub fn fetched(&self) -> Vec<HttpRequest> {
        self.state.borrow().fetched.clone()
    }

    /// Every program the extension started.
    pub fn ran(&self) -> Vec<ProcessCommand> {
        self.state.borrow().ran.clone()
    }

    /// Every call that returned a ticket, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.state.borrow().calls.clone()
    }

    /// Every call without a result (log lines, diagnostics, progress, …), in order; answers are left out.
    pub fn notes(&self) -> Vec<Note> {
        self.state.borrow().notes.clone()
    }

    pub fn logs(&self) -> Vec<(LogLevel, String)> {
        self.state.borrow().notes.iter().filter_map(|n| if let Note::Log { level, message } = n { Some((*level, message.clone())) } else { None }).collect()
    }

    /// A workspace file as it is now (after the extension's writes).
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let uri = self.uri(path);
        self.state.borrow().files.get(&uri).cloned()
    }

    pub fn storage(&self) -> BTreeMap<String, Vec<u8>> {
        self.state.borrow().storage.clone()
    }

    pub fn status_items(&self) -> BTreeMap<String, StatusItem> {
        self.state.borrow().status_items.clone()
    }

    pub fn diagnostics(&self, path: &str) -> Vec<Diagnostic> {
        let uri = self.uri(path);
        self.state.borrow().diagnostics.get(&uri).cloned().unwrap_or_default()
    }

    /// Views the extension asked to draw again.
    pub fn invalidated(&self) -> Vec<String> {
        self.state.borrow().notes.iter().filter_map(|n| if let Note::Invalidate(v) = n { Some(v.clone()) } else { None }).collect()
    }

    /// Edits applied to documents: uri, the version they were made against, the edits.
    pub fn applied_edits(&self) -> Vec<(Uri, u32, Vec<TextEdit>)> {
        self.state.borrow().applied.clone()
    }

    /// Timers that are running, with their period in seconds.
    pub fn timers(&self) -> Vec<(u64, u64)> {
        self.state.borrow().timers.iter().map(|(id, t)| (*id, t.period)).collect()
    }

    /// Progress that has not ended: id and title.
    pub fn progress(&self) -> Vec<(u64, String)> {
        self.state.borrow().progress.iter().map(|(id, t)| (*id, t.clone())).collect()
    }

    pub fn clipboard(&self) -> Option<String> {
        self.state.borrow().clipboard.clone()
    }

    /// Text typed into the terminal.
    pub fn terminal(&self) -> Vec<String> {
        self.state.borrow().terminal.clone()
    }
}

impl Drop for MockHost {
    fn drop(&mut self) {
        native::uninstall();
    }
}

#[cfg(test)]
mod scm_tests;
#[cfg(test)]
mod tests;
