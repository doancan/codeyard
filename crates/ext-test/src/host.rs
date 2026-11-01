//! The mock's side of every call: answer it from the fixtures, record it, or refuse it as the app's broker would.
//! Results are queued and delivered after the call returns, as from the exthost.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::rc::Rc;

use codeyard_extension::native::{Call, Host, Note};
use codeyard_extension::protocol::{Completion, Response, StreamEvent};
use codeyard_extension::types::{self, *};
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

/// An account as the mock keeps it. A provider not in the map is signed in as `test-user`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Account {
    SignedIn(String),
    /// Not signed in: `auth.fetch` fails as in Codeyard, and the next `auth.session` asks (and the user agrees).
    SignedOut,
    /// Not signed in, and the user cancels every sign-in Codeyard asks for.
    Declines,
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
    /// The local ids of the manifest's `[[status-bar]]` items; none without a manifest.
    pub status_ids: Option<Vec<String>>,
    /// The request being handled is a user action (a command, a view event, …): it may open links.
    pub user_action: bool,
    /// The manifest's `[[oauth]]` providers: id, then the name and the hosts `auth.fetch` may sign requests to.
    pub oauth: BTreeMap<String, (String, Vec<String>)>,
    pub accounts: BTreeMap<String, Account>,
    /// Self-managed instances the user added (#184): (`[[oauth]]` id, host).
    pub instances: Vec<(String, String)>,
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
    /// Decorations by file, then set.
    pub decorations: BTreeMap<Uri, BTreeMap<String, Vec<Decoration>>>,
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

/// The workspace-relative path of a uri, as the app's broker makes it (`ext_manifest::relative`: `..` and `%2e`
/// resolved); none when it is outside the workspace folder.
fn relative(uri: &str) -> Option<String> {
    ext_manifest::relative(&[FOLDER.to_string()], uri)
}

/// The hosts `auth.fetch` signs requests to for the built-in GitHub account.
const GITHUB_HOSTS: &[&str] = &["api.github.com", "uploads.github.com"];

/// The host of a request URL that passed `ext_manifest::calls::check_url` (calls that fail it never get this far).
fn host_of(url: &str) -> String {
    ext_manifest::calls::check_url(url).map(|t| t.host).unwrap_or_default()
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
        // A path outside the workspace folder is refused whatever the globs say, as the app's broker refuses it.
        let path = |perm: &str, uri: &str, sends: bool| match relative(uri) {
            None if p.is_some() => Some(Denied { permission: ext_manifest::outside_line(uri), why: Why::NotDeclared }),
            None => None,
            Some(rel) => {
                let ok = |p: &Permissions| if perm == "read" { p.allows_read(&rel) } else { p.allows_write(&rel) };
                check(&ok, format!(r#"{perm} = ["{rel}"]"#), sends)
            }
        };
        match call {
            Call::ReadFile(uri) | Call::Stat(uri) => path("read", uri, false),
            Call::ReadDocument(uri) if Some(uri.as_str()) != active => path("read", uri, false),
            Call::FindFiles { glob, .. } => check(&|p| !p.read.is_empty(), format!(r#"read = ["{glob}"]"#), false),
            // Each path as the app checks it; the first one refused names the line.
            Call::ReadMany(uris) => uris.iter().find_map(|u| path("read", u, false)),
            Call::Search(q) => check(&|p| !p.read.is_empty(), format!(r#"read = ["{}"]"#, q.include.first().map_or("**", String::as_str)), false),
            Call::WriteFile { uri, .. } | Call::DeleteFile(uri) | Call::CreateDirectory(uri) => path("write", uri, true),
            Call::Rename { from, to } => path("write", from, true).or_else(|| path("write", to, true)),
            // A command may edit the active document without `write` (docs/plugin-architecture.md §5.2).
            Call::ApplyEdits { uri, .. } if Some(uri.as_str()) != active => path("write", uri, true),
            Call::Fetch(r) => {
                let host = host_of(&r.url);
                check(&|p| p.allows_net(&host), format!(r#"net = ["{host}"]"#), true)
            }
            Call::Session { provider, .. } => (!self.granted(provider)).then(|| self.account_refusal(provider)),
            Call::AuthFetch { provider, request } => {
                if self.granted(provider) {
                    self.unsigned_host(provider, &request.url)
                } else {
                    Some(self.account_refusal(provider))
                }
            }
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

    /// Whether `account` is declared, as Codeyard decides: its provider is in `auth` (an instance's is its provider's)
    /// and, for an instance (`gitlab@git.example.com`), the user added it.
    fn declared(&self, account: &str) -> bool {
        let (base, host) = ext_manifest::split_account(account);
        self.permissions.as_ref().is_none_or(|p| p.auth.iter().any(|a| a == base))
            && host.is_none_or(|h| self.instances.iter().any(|(p, i)| p == base && i == h))
    }

    fn granted(&self, account: &str) -> bool {
        self.declared(account) && !self.untrusted
    }

    /// Why `account` may not be used: Codeyard names its provider's `auth` line.
    fn account_refusal(&self, account: &str) -> Denied {
        let line = format!(r#"auth = ["{}"]"#, ext_manifest::split_account(account).0);
        Denied { permission: line, why: if self.declared(account) { Why::UntrustedWorkspace } else { Why::NotDeclared } }
    }

    /// `auth.fetch` signs requests only to the hosts of the provider (`[[oauth]] hosts`, GitHub's API hosts), as the
    /// app does: a request to any other host is refused and not sent.
    fn unsigned_host(&self, provider: &str, url: &str) -> Option<Denied> {
        self.permissions.as_ref()?;
        let hosts: Vec<String> = match (self.oauth.get(provider), ext_manifest::split_account(provider)) {
            (_, (_, Some(instance))) => vec![instance.to_string()],
            (Some((_, hosts)), _) => hosts.clone(),
            (None, _) if provider == "github" => GITHUB_HOSTS.iter().map(|h| (*h).to_string()).collect(),
            (None, _) => return None,
        };
        let host = host_of(url);
        (!ext_manifest::net_allows(&hosts, &host)).then(|| Denied { permission: ext_manifest::oauth_host_line(provider, &host), why: Why::NotDeclared })
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
            // As the app: a link opens only while the extension handles a user action.
            Call::OpenExternal(_) if !self.user_action => {
                (none, Completion::Failed(Error::Denied(Denied { permission: "a user action (a command or a link click)".into(), why: Why::NotDeclared })))
            }
            Call::OpenExternal(url) if !url.starts_with("https://") => (none, Completion::Failed(Error::Invalid("only https: links open".into()))),
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
            // As the app: files the glob matches and `read` allows.
            Call::FindFiles { glob, max } => {
                let readable = |rel: &str| self.permissions.as_ref().is_none_or(|p| p.allows_read(rel));
                let found = self
                    .files
                    .keys()
                    .filter(|u| relative(u).is_some_and(|rel| ext_manifest::glob::path_matches(&glob, &rel) && readable(&rel)))
                    .take(max as usize)
                    .cloned()
                    .collect();
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
            Call::ReadMany(uris) => {
                let files = uris.into_iter().map(|u| (u.clone(), self.files.get(&u).cloned().ok_or(Error::NotFound(u)))).collect();
                (none, Completion::Files(files))
            }
            // Plain text only: the mock does not run regular expressions.
            Call::Search(q) => {
                let readable = |u: &Uri| relative(u).is_some_and(|rel| self.permissions.as_ref().is_none_or(|p| p.allows_read(&rel)));
                (search(&self.files, &q, readable), Completion::Done)
            }
            Call::Rename { from, to } => match self.files.remove(&from) {
                Some(b) => {
                    self.files.insert(to, b);
                    (none, Completion::Done)
                }
                None => (none, not_found(from)),
            },
            Call::CreateDirectory(_) => (none, Completion::Done),
            Call::AuthFetch { request: r, .. } if r.headers.iter().any(|(n, _)| n.trim().eq_ignore_ascii_case("authorization")) => {
                (none, Completion::Failed(Error::Invalid("the authorization header is added by Codeyard; remove it from the request".into())))
            }
            Call::AuthFetch { provider, .. } if !matches!(self.accounts.get(&provider), None | Some(Account::SignedIn(_))) => {
                // Codeyard's words when the keychain has no account for the provider.
                let name = self.oauth.get(&provider).map_or_else(|| if provider == "github" { "GitHub".into() } else { provider.clone() }, |(n, _)| n.clone());
                (none, Completion::Failed(Error::Invalid(format!("not signed in to {name}; call auth.session with “{provider}” first"))))
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
            // The first use asks; the user agrees (as `test-user`) unless the test said they decline, which Codeyard
            // reports as `cancelled`.
            Call::Session { provider, .. } => match self.accounts.get(&provider).cloned() {
                Some(Account::Declines) => (none, Completion::Failed(Error::Cancelled)),
                Some(Account::SignedIn(label)) => (none, Completion::Account(types::Account { provider, label })),
                None | Some(Account::SignedOut) => {
                    self.accounts.insert(provider.clone(), Account::SignedIn("test-user".into()));
                    (none, Completion::Account(types::Account { provider, label: "test-user".into() }))
                }
            },
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
        // A URL Codeyard never fetches is `invalid` before any permission is looked at, as in the app.
        if let Call::Fetch(r) | Call::AuthFetch { request: r, .. } = &call
            && let Err(e) = ext_manifest::calls::check_url(&r.url)
        {
            s.queue.push_back(Delivery::Complete(ticket, Completion::Failed(Error::Invalid(e.message))));
            return ticket;
        }
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
                // An answer over `CALL_BYTES` never reaches the app: the request fails, as from the exthost.
                let bytes = match result {
                    Ok(Response::Json(t) | Response::View(t) | Response::Text(Some(t))) => t.len(),
                    _ => 0,
                };
                let result = match ext_manifest::calls::result_too_large(bytes) {
                    Some(msg) => Err(Error::Invalid(msg)),
                    None => result.clone(),
                };
                s.responses.insert(*id, result);
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
            Note::SetDecorations { uri, set, decorations } => {
                let sets = s.decorations.entry(uri.clone()).or_default();
                if decorations.is_empty() {
                    sets.remove(set);
                } else {
                    sets.insert(set.clone(), decorations.clone());
                }
            }
            _ => {}
        }
        s.notes.push(note);
    }

    fn env(&mut self) -> Env {
        Env {
            codeyard_version: "0.5.0".into(),
            api_version: "1.0.0".into(),
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

    /// Refused as Codeyard refuses it (`ext_manifest::calls::check_status_item`): an id the manifest does not
    /// declare in `[[status-bar]]`, an empty or too long text.
    fn set_status_item(&mut self, mut item: StatusItem) -> Result<(), Error> {
        let mut s = self.0.borrow_mut();
        let ids = s.status_ids.clone();
        let declared = |id: &str| ids.as_ref().is_some_and(|ids| ids.iter().any(|i| i == id));
        let check: Option<&dyn Fn(&str) -> bool> = if ids.is_some() { Some(&declared) } else { None };
        item.text = ext_manifest::calls::check_status_item(check, &item.id, &item.text, item.tooltip.as_deref()).map_err(Error::Invalid)?;
        s.status_items.insert(item.id.clone(), item);
        Ok(())
    }
}

/// The mock's `workspace.search`: plain-text matches in the fake files the extension may read, line by line.
fn search(files: &BTreeMap<Uri, Vec<u8>>, q: &SearchQuery, readable: impl Fn(&Uri) -> bool) -> Vec<StreamEvent> {
    // ASCII case folding keeps byte offsets, so a match's range is found in the line as it is.
    let fold = |t: &str| if q.case_sensitive { t.to_string() } else { t.to_ascii_lowercase() };
    let pattern = fold(&q.pattern);
    let mut out = Vec::new();
    for (uri, bytes) in files.iter().filter(|(u, _)| readable(u)) {
        // The app hands `include` to ripgrep's walker, where a glob without `/` names a file in any folder, as in
        // `.gitignore` (unlike `read` globs).
        let rel = relative(uri).unwrap_or_default();
        let name = rel.rsplit('/').next().unwrap_or_default();
        let included = |g: &String| if g.contains('/') { ext_manifest::glob::path_matches(g, &rel) } else { ext_manifest::glob::name_matches(g, name) };
        if !q.include.is_empty() && !q.include.iter().any(included) {
            continue;
        }
        for (n, line) in String::from_utf8_lossy(bytes).lines().enumerate() {
            let Some(at) = fold(line).find(&pattern).filter(|_| !pattern.is_empty()) else { continue };
            if out.len() >= q.max as usize {
                return out;
            }
            let (start, end) = (line[..at].chars().count() as u32, line[..at + pattern.len()].chars().count() as u32);
            let pos = |character| types::Position { line: n as u32, character };
            out.push(StreamEvent::SearchMatch { uri: uri.clone(), range: types::Range { start: pos(start), end: pos(end) }, line: line.to_string() });
        }
    }
    out
}
