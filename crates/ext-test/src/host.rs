//! The mock's side of every call: answer it from the fixtures, record it, or refuse it as the app's broker would.
//! Results are queued and delivered after the call returns, as from the exthost.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::rc::Rc;

use codeyard_extension::native::{Call, Host, Note};
use codeyard_extension::protocol::{Completion, Response, StreamEvent};
use codeyard_extension::types::*;
use codeyard_extension::{Denied, Error, Why};
use ext_manifest::Permissions;

use crate::ProcessFixture;

/// The workspace folder of every mock.
pub const FOLDER: &str = "file:///work";

pub enum Delivery {
    Event(Ticket, StreamEvent),
    Complete(Ticket, Completion),
}

pub struct HttpFixture {
    pub method: String,
    pub url: String,
    pub status: u16,
    pub body: Vec<u8>,
}

pub struct Timer {
    pub period: u64,
    pub next: u64,
}

#[derive(Default)]
pub struct State {
    next_ticket: Ticket,
    next_id: u64,
    pub queue: VecDeque<Delivery>,
    pub responses: HashMap<u64, Result<Response, Error>>,
    /// None: everything is allowed.
    pub permissions: Option<Permissions>,
    /// The manifest's `[[oauth]]` providers: id, then the name and the hosts `auth.fetch` may sign requests to.
    pub oauth: BTreeMap<String, (String, Vec<String>)>,
    pub untrusted: bool,
    pub files: BTreeMap<Uri, Vec<u8>>,
    pub http: Vec<HttpFixture>,
    pub processes: Vec<(String, Vec<String>, ProcessFixture)>,
    pub active: Option<Document>,
    pub selections: Vec<Range>,
    pub settings: BTreeMap<String, Option<String>>,
    pub storage: BTreeMap<String, Vec<u8>>,
    pub command_results: BTreeMap<String, String>,
    pub secrets: BTreeMap<String, String>,
    pub clipboard: Option<String>,
    pub inputs: VecDeque<Option<String>>,
    pub picks: VecDeque<Vec<u32>>,
    pub notice_answers: VecDeque<u32>,
    pub clock: u64,
    pub timers: BTreeMap<u64, Timer>,
    pub progress: BTreeMap<u64, String>,
    pub status_items: BTreeMap<String, StatusItem>,
    pub diagnostics: BTreeMap<Uri, Vec<Diagnostic>>,
    // Records.
    pub calls: Vec<Call>,
    pub notes: Vec<Note>,
    pub notices: Vec<(Severity, String)>,
    pub denied: Vec<Denied>,
    pub fetched: Vec<HttpRequest>,
    pub ran: Vec<ProcessCommand>,
    pub applied: Vec<(Uri, u32, Vec<TextEdit>)>,
    pub terminal: Vec<String>,
}

pub struct Shared(pub Rc<RefCell<State>>);

/// The workspace-relative path of a uri, for `read` and `write` globs.
fn relative(uri: &str) -> String {
    match uri.strip_prefix(FOLDER) {
        Some(rest) => rest.trim_start_matches('/').to_string(),
        None => uri.split_once("://").map_or(uri, |(_, p)| p).trim_start_matches('/').to_string(),
    }
}

/// The hosts `auth.fetch` signs requests to for the built-in GitHub account.
const GITHUB_HOSTS: &[&str] = &["api.github.com", "uploads.github.com"];

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}

impl State {
    /// Whether the broker allows this; the manifest line that would, when it does not.
    fn refusal(&self, call: &Call) -> Option<Denied> {
        let p = self.permissions.as_ref();
        let active = self.active.as_ref().map(|d| d.uri.as_str());
        let check = |ok: &dyn Fn(&Permissions) -> bool, line: String, sends: bool| -> Option<Denied> {
            if p.is_some_and(|p| !ok(p)) {
                return Some(Denied { permission: line, why: Why::NotDeclared });
            }
            (sends && self.untrusted).then_some(Denied { permission: line, why: Why::UntrustedWorkspace })
        };
        let path_line = |perm: &str, uri: &str| format!(r#"{perm} = ["{}"]"#, relative(uri));
        match call {
            Call::ReadFile(uri) | Call::Stat(uri) => check(&|p| p.allows_read(&relative(uri)), path_line("read", uri), false),
            Call::ReadDocument(uri) if Some(uri.as_str()) != active => check(&|p| p.allows_read(&relative(uri)), path_line("read", uri), false),
            Call::FindFiles { glob, .. } => check(&|p| !p.read.is_empty(), format!(r#"read = ["{glob}"]"#), false),
            Call::WriteFile { uri, .. } | Call::DeleteFile(uri) => check(&|p| p.allows_write(&relative(uri)), path_line("write", uri), true),
            // A command may edit the active document without `write` (docs/plugin-architecture.md §5.2).
            Call::ApplyEdits { uri, .. } if Some(uri.as_str()) != active => check(&|p| p.allows_write(&relative(uri)), path_line("write", uri), true),
            Call::Fetch(r) => {
                let host = host_of(&r.url);
                check(&|p| p.allows_net(&host), format!(r#"net = ["{host}"]"#), true)
            }
            Call::Session { provider, .. } => check(&|p| p.auth.iter().any(|a| a == provider), format!(r#"auth = ["{provider}"]"#), true),
            Call::AuthFetch { provider, request } => check(&|p| p.auth.iter().any(|a| a == provider), format!(r#"auth = ["{provider}"]"#), true)
                .or_else(|| self.unsigned_host(provider, &request.url)),
            Call::Spawn(c) => check(&|p| p.allows_run(&c.program), format!(r#"run = ["{}"]"#, c.program), true),
            Call::SecretGet(_) | Call::SecretSet { .. } | Call::SecretDelete(_) => check(&|p| p.secrets, "secrets = true".into(), false),
            Call::ReadClipboard => check(&|p| p.clipboard.reads(), r#"clipboard = "read""#.into(), false),
            Call::WriteClipboard(_) => check(&|p| p.clipboard.writes(), r#"clipboard = "write""#.into(), true),
            Call::SendText(_) => check(&|p| p.terminal, "terminal = true".into(), true),
            Call::Execute { command, .. } if command.starts_with("core:") => {
                check(&|p| p.commands.iter().any(|c| c == command), format!(r#"commands = ["{command}"]"#), false)
            }
            _ => None,
        }
    }

    /// `auth.fetch` signs requests only to the hosts of the provider (`[[oauth]] hosts`, GitHub's API hosts), as the
    /// app does: a request to any other host is refused and not sent.
    fn unsigned_host(&self, provider: &str, url: &str) -> Option<Denied> {
        self.permissions.as_ref()?;
        let (name, hosts) = match self.oauth.get(provider) {
            Some((name, hosts)) => (name.clone(), hosts.clone()),
            None if provider == "github" => ("GitHub".to_string(), GITHUB_HOSTS.iter().map(|h| (*h).to_string()).collect()),
            None => return None,
        };
        let host = host_of(url);
        (!ext_manifest::net_allows(&hosts, &host))
            .then(|| Denied { permission: format!("a host of the {name} account ({}), not {host}", hosts.join(", ")), why: Why::NotDeclared })
    }

    /// The streamed events and the completion of an allowed call.
    fn answer(&mut self, call: Call) -> (Vec<StreamEvent>, Completion) {
        let not_found = |what: String| Completion::Failed(Error::NotFound(what));
        let none = Vec::new();
        match call {
            Call::ShowNotice { severity, message, actions } => {
                self.notices.push((severity, message));
                let chosen = if actions.is_empty() { None } else { self.notice_answers.pop_front() };
                (none, Completion::Chosen(chosen.into_iter().collect()))
            }
            Call::QuickPick { .. } => (none, Completion::Chosen(self.picks.pop_front().unwrap_or_default())),
            Call::InputBox(_) => (none, Completion::Text(self.inputs.pop_front().flatten())),
            Call::OpenExternal(_) | Call::SetSelections { .. } => (none, Completion::Done),
            Call::Execute { command, .. } => (none, Completion::Json(self.command_results.get(&command).cloned().unwrap_or_else(|| "null".into()))),
            Call::ActiveDocument => (none, Completion::Document(self.active.clone())),
            Call::Selections => (none, Completion::Ranges(self.selections.clone())),
            Call::ReadDocument(uri) => {
                let doc = match &self.active {
                    Some(d) if d.uri == uri => Some(d.clone()),
                    _ => None,
                };
                (none, Completion::Document(doc))
            }
            Call::ApplyEdits { uri, version, edits } => {
                if let Some(d) = self.active.as_mut().filter(|d| d.uri == uri) {
                    if d.version != version {
                        return (none, Completion::Failed(Error::Invalid(format!("the document is at version {}, not {version}", d.version))));
                    }
                    d.version += 1;
                }
                self.applied.push((uri, version, edits));
                (none, Completion::Done)
            }
            Call::ReadFile(uri) => match self.files.get(&uri) {
                Some(b) => (none, Completion::Bytes(b.clone())),
                None => (none, not_found(uri)),
            },
            Call::Stat(uri) => match self.files.get(&uri) {
                Some(b) => (none, Completion::Stat(FileStat { is_dir: false, size: b.len() as u64, modified: 0 })),
                None => (none, not_found(uri)),
            },
            Call::FindFiles { glob, max } => {
                let found = self.files.keys().filter(|u| ext_manifest::glob_match(&glob, &relative(u))).take(max as usize).cloned().collect();
                (none, Completion::Uris(found))
            }
            Call::WriteFile { uri, contents } => {
                self.files.insert(uri, contents);
                (none, Completion::Done)
            }
            Call::DeleteFile(uri) => match self.files.remove(&uri) {
                Some(_) => (none, Completion::Done),
                None => (none, not_found(uri)),
            },
            Call::AuthFetch { request: r, .. } if r.headers.iter().any(|(n, _)| n.trim().eq_ignore_ascii_case("authorization")) => {
                (none, Completion::Failed(Error::Invalid("the authorization header is added by Codeyard; remove it from the request".into())))
            }
            Call::Fetch(r) | Call::AuthFetch { request: r, .. } => {
                self.fetched.push(r.clone());
                let fixture = self.http.iter().find(|f| (f.method == "*" || f.method.eq_ignore_ascii_case(&r.method)) && f.url == r.url);
                match fixture {
                    Some(f) => (vec![StreamEvent::HttpHead { status: f.status, headers: Vec::new() }, StreamEvent::Data(f.body.clone())], Completion::Done),
                    None => {
                        (none, Completion::Failed(Error::Other(format!("the mock host has no response for {} {}; add one with host.http(…)", r.method, r.url))))
                    }
                }
            }
            Call::Session { provider, .. } => (none, Completion::Account(Account { provider, label: "test-user".into() })),
            Call::SecretGet(k) => (none, Completion::Text(self.secrets.get(&k).cloned())),
            Call::SecretSet { key, value } => {
                self.secrets.insert(key, value);
                (none, Completion::Done)
            }
            Call::SecretDelete(k) => {
                self.secrets.remove(&k);
                (none, Completion::Done)
            }
            Call::Spawn(c) => {
                self.ran.push(c.clone());
                let fixture = self.processes.iter().find(|(p, a, _)| *p == c.program && *a == c.args).map(|(_, _, f)| f.clone());
                match fixture {
                    Some(f) => {
                        let mut events = Vec::new();
                        if !f.stdout.is_empty() {
                            events.push(StreamEvent::Stdout(f.stdout));
                        }
                        if !f.stderr.is_empty() {
                            events.push(StreamEvent::Stderr(f.stderr));
                        }
                        (events, Completion::Exit(f.code))
                    }
                    None => {
                        let msg = format!("the mock host has no result for `{} {}`; add one with host.process(…)", c.program, c.args.join(" "));
                        (none, Completion::Failed(Error::Other(msg)))
                    }
                }
            }
            Call::ReadClipboard => (none, Completion::Text(self.clipboard.clone())),
            Call::WriteClipboard(t) => {
                self.clipboard = Some(t);
                (none, Completion::Done)
            }
            Call::SendText(t) => {
                self.terminal.push(t);
                (none, Completion::Done)
            }
        }
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

impl Host for Shared {
    fn start(&mut self, call: Call) -> Ticket {
        let mut s = self.0.borrow_mut();
        s.next_ticket += 1;
        let ticket = s.next_ticket;
        s.calls.push(call.clone());
        if let Some(d) = s.refusal(&call) {
            s.denied.push(d.clone());
            s.queue.push_back(Delivery::Complete(ticket, Completion::Failed(Error::Denied(d))));
            return ticket;
        }
        let (events, completion) = s.answer(call);
        for e in events {
            s.queue.push_back(Delivery::Event(ticket, e));
        }
        s.queue.push_back(Delivery::Complete(ticket, completion));
        ticket
    }

    fn note(&mut self, note: Note) {
        let mut s = self.0.borrow_mut();
        match &note {
            Note::Respond { id, result } => {
                s.responses.insert(*id, result.clone());
                return;
            }
            Note::EndProgress(id) => {
                s.progress.remove(id);
            }
            Note::RemoveStatusItem(id) => {
                s.status_items.remove(id);
            }
            Note::ClearTimer(id) => {
                s.timers.remove(id);
            }
            Note::SetDiagnostics { uri, diagnostics } => {
                s.diagnostics.insert(uri.clone(), diagnostics.clone());
            }
            Note::ClearDiagnostics(uri) => match uri {
                Some(u) => {
                    s.diagnostics.remove(u);
                }
                None => s.diagnostics.clear(),
            },
            _ => {}
        }
        s.notes.push(note);
    }

    fn env(&mut self) -> Env {
        Env {
            codeyard_version: "0.5.0".into(),
            api_version: "0.1.0".into(),
            language: "en".into(),
            theme: ThemeKind::Dark,
            platform: if cfg!(target_os = "macos") {
                Platform::Macos
            } else if cfg!(windows) {
                Platform::Windows
            } else {
                Platform::Linux
            },
            remote: false,
            workspace_trusted: !self.0.borrow().untrusted,
        }
    }

    fn setting(&mut self, key: &str) -> Option<String> {
        self.0.borrow().settings.get(key).cloned().flatten()
    }

    fn storage_get(&mut self, key: &str) -> Option<Vec<u8>> {
        self.0.borrow().storage.get(key).cloned()
    }

    fn storage_set(&mut self, key: &str, value: &[u8]) -> Result<(), Error> {
        let mut s = self.0.borrow_mut();
        let total: u64 = s.storage.iter().filter(|(k, _)| *k != key).map(|(k, v)| (k.len() + v.len()) as u64).sum();
        if total + (key.len() + value.len()) as u64 > ext_manifest::limits::DATA_DIR_BYTES {
            return Err(Error::Invalid("the store would grow past its 50 MiB limit".into()));
        }
        s.storage.insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn storage_delete(&mut self, key: &str) {
        self.0.borrow_mut().storage.remove(key);
    }

    fn storage_keys(&mut self) -> Vec<String> {
        self.0.borrow().storage.keys().cloned().collect()
    }

    fn folders(&mut self) -> Vec<Uri> {
        let s = self.0.borrow();
        if s.permissions.as_ref().is_some_and(|p| p.read.is_empty()) { Vec::new() } else { vec![FOLDER.to_string()] }
    }

    fn every(&mut self, seconds: u32) -> u64 {
        let mut s = self.0.borrow_mut();
        let id = s.id();
        let period = u64::from(seconds.max(ext_manifest::limits::TIMER_MIN_SECONDS));
        let next = s.clock + period;
        s.timers.insert(id, Timer { period, next });
        id
    }

    fn start_progress(&mut self, title: &str, _cancellable: bool) -> u64 {
        let mut s = self.0.borrow_mut();
        let id = s.id();
        s.progress.insert(id, title.to_string());
        id
    }

    fn set_status_item(&mut self, item: StatusItem) -> Result<(), Error> {
        if item.id.is_empty() {
            return Err(Error::Invalid("a status item needs an id".into()));
        }
        self.0.borrow_mut().status_items.insert(item.id.clone(), item);
        Ok(())
    }
}
