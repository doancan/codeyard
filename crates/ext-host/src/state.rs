//! The host side of every `codeyard:api` import. Calls that reach the app become frames; a call whose permission is
//! not granted (revoked by the user, or one that sends, writes or runs in an untrusted folder) never leaves the
//! exthost: its ticket completes locally with `denied` once the current export returns, and the app hears only that
//! it was refused (`ToApp::Denied`, for its denials list). The app checks every call again: this is the first fence,
//! not the only one. The granted set changes when the user turns a permission off or on (`ToHost::Grants`).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use ext_api::Need;
use ext_api::value::{self as v, Completion, Notify, Start, Ticket};
use ext_manifest::limits;
use ext_proto::ToApp;
use wasmtime::StoreLimits;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::bindings::codeyard::api as g;
use crate::convert;
use crate::prints::Prints;
use crate::storage::Storage;

pub struct State {
    out: Box<dyn Write + Send>,
    next_ticket: Ticket,
    next_id: u64,
    /// Tickets that finished without leaving the exthost, delivered after the current export returns.
    pub local: Vec<(Ticket, Completion)>,
    /// Set when the pipe to the app broke: the exthost exits after the current call.
    pub broken: Option<String>,
    granted: BTreeSet<String>,
    untrusted: bool,
    /// The watches the guest started and has not stopped (`watch`), counted against `limits::WATCHES`.
    watches: BTreeSet<u64>,
    pub env: v::EnvInfo,
    pub settings: BTreeMap<String, Option<String>>,
    folders: Vec<String>,
    /// `Hello::status_items`.
    status_items: Vec<String>,
    storage: Storage,
    wasi: WasiCtx,
    table: ResourceTable,
    pub limits: StoreLimits,
    stdout: Prints,
    stderr: Prints,
}

impl State {
    pub fn new(out: Box<dyn Write + Send>, hello: &ext_proto::Hello, limits: StoreLimits) -> State {
        let stdout = Prints::default();
        let stderr = Prints::default();
        // No folders, no environment, no arguments, no network: files go through `workspace`, requests through `net`.
        let wasi = WasiCtx::builder().stdout(stdout.clone()).stderr(stderr.clone()).build();
        State {
            out,
            next_ticket: 1,
            next_id: 1,
            local: Vec::new(),
            broken: None,
            granted: hello.granted.iter().cloned().collect(),
            untrusted: hello.untrusted,
            watches: BTreeSet::new(),
            env: hello.env.clone(),
            settings: hello.settings.iter().cloned().collect(),
            folders: hello.folders.clone(),
            status_items: hello.status_items.clone(),
            storage: Storage::open(&hello.data_dir),
            wasi,
            table: ResourceTable::new(),
            limits,
            stdout,
            stderr,
        }
    }

    pub fn send(&mut self, msg: &ToApp) {
        if self.broken.is_none()
            && let Err(e) = ext_proto::write(&mut self.out, msg)
        {
            self.broken = Some(e.to_string());
        }
    }

    /// Why the interface's calls are refused right now, if they are.
    fn refused(&self, interface: &str) -> Option<(Need, v::Why)> {
        let need = ext_api::interface(interface)?.need;
        if need == Need::Nothing {
            return None;
        }
        let sends = matches!(need, Need::Write | Need::Net | Need::Auth | Need::Run | Need::Terminal | Need::ClipboardWrite);
        // Checked first: the app masks grants by trust before sending them, so in an untrusted folder a sending
        // interface is never in `granted` and would otherwise read as turned off by the user.
        if self.untrusted && sends {
            return Some((need, v::Why::UntrustedWorkspace));
        }
        (!self.granted.contains(interface)).then_some((need, v::Why::RevokedByUser))
    }

    /// The interfaces granted from now on.
    pub fn set_granted(&mut self, granted: Vec<String>) {
        self.granted = granted.into_iter().collect();
    }

    fn start(&mut self, interface: &str, call: Start) -> Ticket {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        if let Some((need, why)) = self.refused(interface) {
            let denied = v::Denied { permission: need.name().to_string(), why };
            self.send(&ToApp::Denied { call: call.wit_name().to_string(), denied: denied.clone() });
            self.local.push((ticket, Completion::Failed(v::Error::Denied(denied))));
            return ticket;
        }
        match ext_proto::write(&mut self.out, &ToApp::Start { ticket, call }) {
            Ok(()) => {}
            Err(ext_proto::Error::TooLarge { .. }) => {
                let msg = format!("the call is larger than the {} MiB limit", limits::CALL_BYTES >> 20);
                self.local.push((ticket, Completion::Failed(v::Error::Invalid(msg))));
            }
            Err(e) => self.broken = Some(e.to_string()),
        }
        ticket
    }

    fn notify(&mut self, n: Notify) {
        match ext_proto::write(&mut self.out, &ToApp::Notify(n)) {
            Ok(()) | Err(ext_proto::Error::TooLarge { .. }) => {}
            Err(e) => self.broken = Some(e.to_string()),
        }
    }

    fn id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Send what the guest printed during the last call to its log.
    pub fn flush_prints(&mut self) {
        for (pipe, level) in [(self.stdout.clone(), v::LogLevel::Info), (self.stderr.clone(), v::LogLevel::Warn)] {
            let bytes = pipe.take();
            if !bytes.is_empty() {
                let message = String::from_utf8_lossy(&bytes).trim_end().to_string();
                self.notify(Notify::Log { level, message });
            }
        }
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl g::types::Host for State {}
impl g::responses::Host for State {}
impl g::host_requests::Host for State {}
impl g::pull_requests::Host for State {}

impl g::log::Host for State {
    fn log(&mut self, level: g::log::Level, message: String) {
        self.notify(Notify::Log { level: level.into(), message });
    }
}

impl g::env::Host for State {
    fn info(&mut self) -> g::env::Environment {
        convert::environment(&self.env)
    }
}

impl g::tickets::Host for State {
    fn cancel(&mut self, ticket: Ticket) {
        self.notify(Notify::Cancel { ticket });
    }
}

impl g::requests::Host for State {
    /// An answer too large for one frame fails its request with `invalid` instead of never arriving.
    fn respond(&mut self, id: u64, result: Result<g::responses::Response, g::types::Error>) {
        let result = result.map(Into::into).map_err(Into::into);
        // The id and the tags around the answer take a few bytes; the answer itself may have `CALL_BYTES`.
        match ext_proto::write_within(&mut self.out, &ToApp::Notify(Notify::Respond { id, result }), limits::CALL_BYTES + 64) {
            Ok(()) => {}
            Err(ext_proto::Error::TooLarge { len }) => {
                let msg = ext_manifest::calls::result_too_large(len).unwrap_or_else(|| format!("the result is {len} bytes, too large to send"));
                self.notify(Notify::Respond { id, result: Err(v::Error::Invalid(msg)) });
            }
            Err(e) => self.broken = Some(e.to_string()),
        }
    }
}

impl g::window::Host for State {
    fn show_notice(&mut self, severity: g::window::Severity, message: String, actions: Vec<String>) -> Ticket {
        self.start("window", Start::ShowNotice { severity: severity.into(), message, actions })
    }

    fn quick_pick(&mut self, items: Vec<g::window::PickItem>, options: g::window::PickOptions) -> Ticket {
        self.start("window", Start::QuickPick { items: items.into_iter().map(Into::into).collect(), options: options.into() })
    }

    fn input_box(&mut self, options: g::window::InputOptions) -> Ticket {
        self.start("window", Start::InputBox { options: options.into() })
    }

    fn start_progress(&mut self, title: String, cancellable: bool) -> u64 {
        let id = self.id();
        self.notify(Notify::StartProgress { id, title, cancellable });
        id
    }

    fn report_progress(&mut self, id: u64, message: Option<String>, percent: Option<u8>) {
        self.notify(Notify::ReportProgress { id, message, percent: percent.map(|p| p.min(100)) });
    }

    fn end_progress(&mut self, id: u64) {
        self.notify(Notify::EndProgress { id });
    }

    /// Refused here as the app would refuse it (`ext_manifest::calls::check_status_item`), so the extension gets the
    /// error instead of an item that never shows.
    fn set_status_item(&mut self, item: g::window::StatusItem) -> Result<(), g::types::Error> {
        let item: v::StatusItem = item.into();
        let declared = |id: &str| self.status_items.iter().any(|s| s == id);
        ext_manifest::calls::check_status_item(Some(&declared), &item.id, &item.text, item.tooltip.as_deref()).map_err(g::types::Error::Invalid)?;
        self.notify(Notify::SetStatusItem { item });
        Ok(())
    }

    fn remove_status_item(&mut self, id: String) {
        self.notify(Notify::RemoveStatusItem { id });
    }

    fn open_external(&mut self, url: String) -> Ticket {
        self.start("window", Start::OpenExternal { url })
    }
}

impl g::commands::Host for State {
    fn execute(&mut self, command: String, args_json: String) -> Ticket {
        self.start("commands", Start::Execute { command, args_json })
    }
}

impl g::storage::Host for State {
    fn get(&mut self, key: String) -> Option<Vec<u8>> {
        self.storage.get(&key)
    }

    fn set(&mut self, key: String, value: Vec<u8>) -> Result<(), g::types::Error> {
        self.storage.set(&key, &value).map_err(g::types::Error::Invalid)
    }

    fn delete(&mut self, key: String) {
        self.storage.delete(&key);
    }

    fn keys(&mut self) -> Vec<String> {
        self.storage.keys()
    }
}

impl g::settings::Host for State {
    fn get(&mut self, key: String) -> Option<String> {
        self.settings.get(&key).cloned().flatten()
    }
}

impl g::timers::Host for State {
    fn every(&mut self, seconds: u32) -> u64 {
        let id = self.id();
        self.notify(Notify::Every { id, seconds: seconds.max(limits::TIMER_MIN_SECONDS) });
        id
    }

    fn clear(&mut self, id: u64) {
        self.notify(Notify::ClearTimer { id });
    }
}

impl g::ui::Host for State {
    fn invalidate(&mut self, view: String) {
        self.notify(Notify::Invalidate { view });
    }
}

impl g::scm::Host for State {
    fn refresh(&mut self, provider: String) {
        self.notify(Notify::ScmRefresh { provider });
    }
}

impl State {
    /// A new watch: refused like a call when `watch` is not granted, and past the limit.
    fn watch(&mut self, call: &'static str, notify: impl FnOnce(u64) -> Notify) -> Result<u64, g::types::Error> {
        if let Some((need, why)) = self.refused("watch") {
            let denied = v::Denied { permission: need.name().to_string(), why };
            self.send(&ToApp::Denied { call: call.to_string(), denied: denied.clone() });
            return Err(v::Error::Denied(denied).into());
        }
        if self.watches.len() >= limits::WATCHES {
            return Err(g::types::Error::Invalid(format!("an extension may have {} watches at once; stop one first", limits::WATCHES)));
        }
        let id = self.id();
        self.watches.insert(id);
        self.notify(notify(id));
        Ok(id)
    }
}

impl g::watch::Host for State {
    fn documents(&mut self, text: bool) -> Result<u64, g::types::Error> {
        self.watch("watch/documents", |id| Notify::WatchDocuments { id, text })
    }

    fn files(&mut self, glob: String) -> Result<u64, g::types::Error> {
        if let Err(e) = ext_manifest::check_watch_glob(&glob) {
            return Err(g::types::Error::Invalid(e));
        }
        self.watch("watch/files", |id| Notify::WatchFiles { id, glob })
    }

    fn stop(&mut self, id: u64) {
        if self.watches.remove(&id) {
            self.notify(Notify::StopWatch { id });
        }
    }
}

impl g::documents::Host for State {
    fn active(&mut self) -> Ticket {
        self.start("documents", Start::ActiveDocument)
    }

    fn selections(&mut self) -> Ticket {
        self.start("documents", Start::Selections)
    }

    fn read(&mut self, uri: String) -> Ticket {
        self.start("documents", Start::ReadDocument { uri })
    }

    fn apply_edits(&mut self, uri: String, version: u32, edits: Vec<g::types::TextEdit>) -> Ticket {
        self.start("documents", Start::ApplyEdits { uri, version, edits: edits.into_iter().map(Into::into).collect() })
    }

    fn set_selections(&mut self, uri: String, selections: Vec<g::types::Range>) -> Ticket {
        self.start("documents", Start::SetSelections { uri, selections: selections.into_iter().map(Into::into).collect() })
    }
}

impl g::languages::Host for State {
    fn set_diagnostics(&mut self, uri: String, diagnostics: Vec<g::types::Diagnostic>) {
        self.notify(Notify::SetDiagnostics { uri, diagnostics: diagnostics.into_iter().map(Into::into).collect() });
    }

    fn clear_diagnostics(&mut self, uri: Option<String>) {
        self.notify(Notify::ClearDiagnostics { uri });
    }

    fn set_decorations(&mut self, uri: String, set: String, decorations: Vec<g::languages::Decoration>) {
        self.notify(Notify::SetDecorations { uri, set, decorations: decorations.into_iter().map(Into::into).collect() });
    }
}

impl g::workspace::Host for State {
    fn folders(&mut self) -> Vec<String> {
        if self.refused("workspace").is_some() { Vec::new() } else { self.folders.clone() }
    }

    fn read_file(&mut self, uri: String) -> Ticket {
        self.start("workspace", Start::ReadFile { uri })
    }

    fn read_many(&mut self, uris: Vec<String>) -> Ticket {
        self.start("workspace", Start::ReadMany { uris })
    }

    fn stat(&mut self, uri: String) -> Ticket {
        self.start("workspace", Start::Stat { uri })
    }

    fn find_files(&mut self, glob: String, max: u32) -> Ticket {
        self.start("workspace", Start::FindFiles { glob, max })
    }

    fn search(&mut self, query: g::workspace::SearchQuery) -> Ticket {
        self.start("workspace", Start::Search { query: query.into() })
    }
}

impl g::workspace_write::Host for State {
    fn write_file(&mut self, uri: String, contents: Vec<u8>) -> Ticket {
        self.start("workspace-write", Start::WriteFile { uri, contents })
    }

    fn delete(&mut self, uri: String) -> Ticket {
        self.start("workspace-write", Start::Delete { uri })
    }

    fn rename(&mut self, from: String, to: String) -> Ticket {
        self.start("workspace-write", Start::Rename { from, to })
    }

    fn create_directory(&mut self, uri: String) -> Ticket {
        self.start("workspace-write", Start::CreateDirectory { uri })
    }
}

impl g::net::Host for State {
    fn fetch(&mut self, request: g::types::HttpRequest) -> Ticket {
        self.start("net", Start::Fetch { request: request.into() })
    }
}

impl g::auth::Host for State {
    fn session(&mut self, provider: String, scopes: Vec<String>) -> Ticket {
        self.start("auth", Start::Session { provider, scopes })
    }

    fn fetch(&mut self, provider: String, request: g::types::HttpRequest) -> Ticket {
        self.start("auth", Start::AuthFetch { provider, request: request.into() })
    }
}

impl g::secrets::Host for State {
    fn get(&mut self, key: String) -> Ticket {
        self.start("secrets", Start::SecretGet { key })
    }

    fn set(&mut self, key: String, value: String) -> Ticket {
        self.start("secrets", Start::SecretSet { key, value })
    }

    fn delete(&mut self, key: String) -> Ticket {
        self.start("secrets", Start::SecretDelete { key })
    }
}

impl g::process::Host for State {
    fn spawn(&mut self, command: g::process::Command) -> Ticket {
        self.start("process", Start::Spawn { command: command.into() })
    }
}

impl g::clipboard_read::Host for State {
    fn read_text(&mut self) -> Ticket {
        self.start("clipboard-read", Start::ReadClipboard)
    }
}

impl g::clipboard_write::Host for State {
    fn write_text(&mut self, text: String) -> Ticket {
        self.start("clipboard-write", Start::WriteClipboard { text })
    }
}

impl g::terminal::Host for State {
    fn send_text(&mut self, text: String) -> Ticket {
        self.start("terminal", Start::SendText { text })
    }
}
