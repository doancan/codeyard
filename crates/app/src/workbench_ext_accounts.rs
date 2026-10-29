//! Workbench: accounts, secrets and HTTPS for code extensions (docs/plugin-architecture.md §6.3, §6.4). `net.fetch`,
//! `auth.*` and `secrets.*` calls arrive here from `on_extension_call`; the checks and requests are in `ext_fetch` and
//! `ext_accounts` and run on a background thread.
//!
//! The first use of an account asks with a system dialog, which no extension UI can draw over or imitate; it names the
//! extension's publisher and the account. GitHub uses the sign-in Codeyard already has (the GitHub CLI, as for pull
//! requests); an `[[oauth]]` provider opens the browser and comes back through `codeyard://ext/<id>/callback`. Tokens go
//! to the system keychain through this window (GPUI's credential store), never to the exthost.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use ext_api::value::{self as v, Completion, Start};
use ext_proto::ToHost;
use futures::StreamExt;

use super::ext_view_glue::{action_button, section_title};
use super::*;
use crate::ext_accounts::{self as acc, Keychain, Provider, Publisher, Token, Vault};
use crate::ext_broker::{Scope, Sink};
use crate::ext_fetch::{self, Transport};
use crate::ext_runtime::Process;

/// GitHub's account endpoint, for the account's name.
const GITHUB_USER: &str = "https://api.github.com/user";

type GithubToken = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

pub(super) struct AccountsState {
    keychain: Arc<dyn Keychain>,
    /// Keychain requests from background threads, answered in this window (`ensure_keychain_pump`).
    keychain_rx: Option<mpsc::Receiver<KeychainOp>>,
    keychain_pump: Option<gpui::Task<()>>,
    transport: Arc<dyn Transport>,
    /// The token of the GitHub sign-in Codeyard already has.
    github_token: GithubToken,
    flows: acc::Flows,
    /// Signed-in accounts seen in this window: (extension, provider) → the account's name.
    pub(super) known: BTreeMap<(String, String), String>,
}

impl Default for AccountsState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let keychain: Arc<dyn Keychain> = if cfg!(test) { Arc::new(acc::MemoryKeychain::default()) } else { Arc::new(WindowKeychain(Mutex::new(tx))) };
        AccountsState {
            keychain,
            keychain_rx: Some(rx),
            keychain_pump: None,
            transport: Arc::new(ext_fetch::NetTransport),
            github_token: Arc::new(github_cli_token),
            flows: acc::Flows::default(),
            known: BTreeMap::new(),
        }
    }
}

/// `gh auth token`, else `GH_TOKEN` / `GITHUB_TOKEN`: what the pull request list uses too.
fn github_cli_token() -> Result<String, String> {
    let args = ["auth".to_string(), "token".to_string()];
    if let Ok(t) = crate::github::run_with_timeout("gh", &args, Duration::from_secs(15)) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(t) = std::env::var(var)
            && !t.trim().is_empty()
        {
            return Ok(t.trim().to_string());
        }
    }
    Err(crate::i18n::t("Codeyard is not signed in to GitHub: run `gh auth login` in a terminal, then try again.").to_string())
}

enum KeychainOp {
    Read(String, mpsc::Sender<Result<Option<Vec<u8>>, String>>),
    Write(String, Vec<u8>, mpsc::Sender<Result<(), String>>),
    Delete(String, mpsc::Sender<Result<(), String>>),
}

/// The system keychain, reached through the window: GPUI's credential calls need the app, and accounts are used from
/// background threads.
struct WindowKeychain(Mutex<mpsc::Sender<KeychainOp>>);

impl WindowKeychain {
    fn ask<T>(&self, op: impl FnOnce(mpsc::Sender<Result<T, String>>) -> KeychainOp) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        self.0.lock().unwrap_or_else(|p| p.into_inner()).send(op(tx)).map_err(|_| crate::i18n::t("the window closed").to_string())?;
        // The system may ask the user to unlock the keychain first.
        rx.recv_timeout(Duration::from_secs(120)).map_err(|_| crate::i18n::t("the keychain did not answer").to_string())?
    }
}

impl Keychain for WindowKeychain {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        self.ask(|tx| KeychainOp::Read(key.to_string(), tx))
    }
    fn write(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.ask(|tx| KeychainOp::Write(key.to_string(), value.to_vec(), tx))
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.ask(|tx| KeychainOp::Delete(key.to_string(), tx))
    }
}

/// Streams a ticket's data to its exthost.
struct ToExthost {
    proc: Arc<Process>,
    ticket: u64,
}

impl Sink for ToExthost {
    fn event(&self, event: v::StreamEvent) {
        let _ = self.proc.send(&ToHost::Event { ticket: self.ticket, event });
    }
}

fn denied(permission: String, scope: &Scope) -> Completion {
    let why = if scope.trusted { v::Why::NotDeclared } else { v::Why::UntrustedWorkspace };
    Completion::Failed(v::Error::Denied(v::Denied { permission, why }))
}

static LISTENING: AtomicBool = AtomicBool::new(false);

/// A `codeyard://ext/…` address from the system: every window looks for the sign-in it completes.
pub(crate) fn deliver_callback(url: &str, cx: &mut App) {
    for w in cx.windows().into_iter().filter_map(|w| w.downcast::<Workbench>()) {
        if w.update(cx, |wb, window, cx| wb.on_extension_callback(url, window, cx)).unwrap_or(false) {
            return;
        }
    }
}

/// In a second Codeyard started with a `codeyard://` address (Windows, Linux): hand it to the running one. The exit code.
pub(crate) fn forward_callback_from_command_line(url: &str) -> i32 {
    let Some(dir) = crate::workbench::state_path().and_then(|p| p.parent().map(std::path::Path::to_path_buf)) else { return 1 };
    match acc::forward_callback(url, &acc::callback_port_file(&dir)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

impl Workbench {
    /// `net.fetch`, `auth.*` and `secrets.*`, after `on_extension_call` found the extension and its scope.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn extension_account_call(
        &mut self,
        active: &crate::ext::Active,
        scope: Scope,
        ticket: u64,
        call: Start,
        proc: Arc<Process>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_keychain_pump(cx);
        let id = active.id().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cfg.exts.code.cancels.insert((id.clone(), ticket), cancel.clone());
        let transport = self.cfg.exts.accounts.transport.clone();
        let keychain = self.cfg.exts.accounts.keychain.clone();
        let publisher = self.extension_publisher(active);
        let complete = move |proc: &Arc<Process>, completion: Completion| {
            let _ = proc.send(&ToHost::Complete { ticket, completion });
        };
        match call {
            Start::Fetch { request } => {
                cx.background_executor()
                    .spawn(async move {
                        let sink = ToExthost { proc: proc.clone(), ticket };
                        complete(&proc, ext_fetch::net_fetch(transport.as_ref(), &scope, &request, &sink, &cancel));
                    })
                    .detach();
            }
            Start::SecretGet { .. } | Start::SecretSet { .. } | Start::SecretDelete { .. } if !scope.permissions.secrets => {
                complete(&proc, denied("secrets = true".into(), &scope));
            }
            call @ (Start::SecretGet { .. } | Start::SecretSet { .. } | Start::SecretDelete { .. }) => {
                detached(move || {
                    let vault = Vault { keychain: keychain.as_ref(), publisher: &publisher, extension: &id };
                    let c = match call {
                        Start::SecretGet { key } => vault.secret(&key).map(Completion::Text),
                        Start::SecretSet { key, value } => vault.set_secret(&key, &value).map(|()| Completion::Done),
                        Start::SecretDelete { key } => vault.delete_secret(&key).map(|()| Completion::Done),
                        _ => Ok(Completion::Done),
                    };
                    complete(&proc, c.unwrap_or_else(Completion::Failed));
                });
            }
            Start::Session { provider, scopes } => self.extension_session(active, &scope, ticket, &provider, &scopes, proc, window, cx),
            Start::AuthFetch { provider, request } => {
                let Some(p) = acc::provider(&active.manifest, &provider).filter(|_| scope.permissions.auth.contains(&provider)) else {
                    complete(&proc, denied(format!("auth = [\"{provider}\"]"), &scope));
                    return;
                };
                // Checked before the keychain is touched: a request to another host is never signed.
                if let Err(e) = ext_fetch::may_sign(&scope, &p, &request) {
                    complete(&proc, Completion::Failed(e));
                    return;
                }
                detached(move || {
                    let vault = Vault { keychain: keychain.as_ref(), publisher: &publisher, extension: &id };
                    let token = match current_token(&vault, &p, transport.as_ref()) {
                        Ok(t) => t,
                        Err(e) => return complete(&proc, Completion::Failed(e)),
                    };
                    let sink = ToExthost { proc: proc.clone(), ticket };
                    complete(&proc, ext_fetch::auth_fetch(transport.as_ref(), &scope, &p, &token, &request, &sink, &cancel));
                });
            }
            _ => {}
        }
    }

    /// Who the extension is: its lock entry's source, or the folder in development.
    fn extension_publisher(&self, active: &crate::ext::Active) -> Publisher {
        let dev = crate::ext::dev_dir().filter(|d| *d == active.dir);
        let source = match (&dev, &self.cfg.dir) {
            (None, Some(dir)) => crate::ext::store(dir).locked().ok().and_then(|l| l.get(&active.manifest.id).map(|e| e.source.clone())),
            _ => None,
        };
        Publisher::of(active.id(), source.as_ref(), dev.as_deref())
    }

    /// `auth.session`: the stored account, else ask (the first use) and sign in.
    #[allow(clippy::too_many_arguments)]
    fn extension_session(
        &mut self,
        active: &crate::ext::Active,
        scope: &Scope,
        ticket: u64,
        provider: &str,
        scopes: &[String],
        proc: Arc<Process>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let complete = move |c: Completion| {
            let _ = proc.send(&ToHost::Complete { ticket, completion: c });
        };
        let Some(p) = acc::provider(&active.manifest, provider).filter(|_| scope.permissions.auth.iter().any(|a| a == provider)) else {
            complete(denied(format!("auth = [\"{provider}\"]"), scope));
            return;
        };
        let scopes = match &p.oauth {
            Some(o) => match acc::scopes_for(o, scopes) {
                Ok(s) => s,
                Err(e) => return complete(Completion::Failed(e)),
            },
            None => scopes.to_vec(),
        };
        let keychain = self.cfg.exts.accounts.keychain.clone();
        let publisher = self.extension_publisher(active);
        let (id, name) = (active.id().to_string(), active.manifest.name.clone());
        let stored = off_thread({
            let (publisher, id, provider) = (publisher.clone(), id.clone(), p.id.clone());
            move || Vault { keychain: keychain.as_ref(), publisher: &publisher, extension: &id }.account(&provider)
        });
        cx.spawn_in(window, async move |this, cx| {
            let stored = stored.await.unwrap_or_else(|| Err(crate::i18n::t("the keychain did not answer").to_string()));
            let _ = this.update_in(cx, |this, window, cx| match stored {
                Ok(Some(t)) => {
                    this.cfg.exts.accounts.known.insert((id.clone(), p.id.clone()), t.label.clone());
                    complete(Completion::Account(v::Account { provider: p.id.clone(), label: t.label }));
                }
                Err(e) => complete(Completion::Failed(v::Error::Other(e))),
                Ok(None) => match p.oauth.clone() {
                    None => this.ask_github(Ask { id, name, publisher, provider: p, scopes, ticket }, Box::new(complete), window, cx),
                    Some(o) => this.ask_oauth(Ask { id, name, publisher, provider: p, scopes, ticket }, o, Box::new(complete), window, cx),
                },
            });
        })
        .detach();
    }

    /// GitHub: the account of Codeyard's GitHub sign-in, after the user agrees.
    fn ask_github(&mut self, ask: Ask, complete: Complete, window: &mut Window, cx: &mut Context<Self>) {
        let (source, transport) = (self.cfg.exts.accounts.github_token.clone(), self.cfg.exts.accounts.transport.clone());
        let found = cx.background_executor().spawn(async move {
            let token = source()?;
            let label = ext_fetch::fetch_label(transport.as_ref(), GITHUB_USER, &token)
                .ok_or_else(|| crate::i18n::t("GitHub did not accept Codeyard's sign-in: run `gh auth login` in a terminal, then try again.").to_string())?;
            Ok::<_, String>((token, label))
        });
        cx.spawn_in(window, async move |this, cx| {
            let (token, label) = match found.await {
                Ok(t) => t,
                Err(e) => return complete(Completion::Failed(v::Error::Other(e))),
            };
            let answer = this.update_in(cx, |_, window, cx| {
                let title = crate::i18n::tf("Let {0} act as your GitHub account {1}?", &[&ask.name, &format!("@{label}")]);
                window.prompt(PromptLevel::Warning, &title, Some(&ask.detail()), &[crate::i18n::t("Allow"), crate::i18n::t("Cancel")], cx)
            });
            let Ok(answer) = answer else { return };
            if answer.await != Ok(0) {
                return complete(Completion::Failed(v::Error::Cancelled));
            }
            let token = Token { access: token, refresh: None, expires: None, label, scopes: ask.scopes.clone() };
            let _ = this.update(cx, |this, cx| this.save_account(ask, token, complete, cx));
        })
        .detach();
    }

    /// An `[[oauth]]` provider: after the user agrees, the browser sign-in with PKCE; the callback finishes it.
    fn ask_oauth(&mut self, ask: Ask, o: ext_manifest::OAuthProvider, complete: Complete, window: &mut Window, cx: &mut Context<Self>) {
        let title = crate::i18n::tf("Let {0} sign in to your {1} account?", &[&ask.name, &ask.provider.name]);
        let answer = window.prompt(PromptLevel::Warning, &title, Some(&ask.detail()), &[crate::i18n::t("Sign In…"), crate::i18n::t("Cancel")], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return complete(Completion::Failed(v::Error::Cancelled));
            }
            let _ = this.update(cx, |this, cx| {
                let pkce = acc::Pkce::new();
                let state = acc::random_token();
                let url = acc::authorize_url(&o, &ask.id, &ask.scopes, &state, &pkce.challenge);
                // The ticket completes from the callback; `complete` is dropped here.
                drop(complete);
                this.cfg.exts.accounts.flows.start(acc::Flow {
                    extension: ask.id.clone(),
                    provider: ask.provider.id.clone(),
                    state,
                    pkce,
                    scopes: ask.scopes.clone(),
                    ticket: ask.ticket,
                    started: std::time::Instant::now(),
                });
                ensure_callback_listener(cx);
                cx.open_url(&url);
                this.notice = Some(Notice::Info(crate::i18n::tf("{0}: finish signing in to {1} in your browser.", &[&ask.id, &ask.provider.name]).into()));
                cx.notify();
            });
        })
        .detach();
    }

    /// Store the account, then answer the ticket with it.
    fn save_account(&mut self, ask: Ask, token: Token, complete: Complete, cx: &mut Context<Self>) {
        let keychain = self.cfg.exts.accounts.keychain.clone();
        let label = token.label.clone();
        let (id, provider) = (ask.id.clone(), ask.provider.id.clone());
        let saved =
            off_thread(move || Vault { keychain: keychain.as_ref(), publisher: &ask.publisher, extension: &ask.id }.save_account(&ask.provider.id, &token));
        cx.spawn(async move |this, cx| {
            let saved = saved.await.unwrap_or_else(|| Err(crate::i18n::t("the keychain did not answer").to_string()));
            let _ = this.update(cx, |this, cx| match saved {
                Ok(()) => {
                    this.cfg.exts.accounts.known.insert((id.clone(), provider.clone()), label.clone());
                    complete(Completion::Account(v::Account { provider, label }));
                    cx.notify();
                }
                Err(e) => complete(Completion::Failed(v::Error::Other(crate::i18n::tf("the keychain refused the account: {}", &[&e])))),
            });
        })
        .detach();
    }

    /// A `codeyard://ext/<id>/callback` address. True when it belonged to a sign-in of this window.
    pub(super) fn on_extension_callback(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(cb) = acc::parse_callback(url) else { return false };
        let flow = match self.cfg.exts.accounts.flows.take(&cb) {
            Ok(f) => f,
            Err(acc::Refused::Unknown) => return false,
            Err(acc::Refused::OtherExtension { owner }) => {
                let msg = crate::i18n::tf("A sign-in answer for {0} carried the code of {1}'s sign-in; it was ignored.", &[&cb.extension, &owner]);
                self.notice = Some(Notice::Error(msg.into()));
                cx.notify();
                return true;
            }
        };
        window.activate_window();
        let complete = self.ticket_completer(&flow.extension, flow.ticket);
        let code = match cb.result {
            Ok(code) => code,
            Err(e) => {
                complete(Completion::Failed(v::Error::Other(crate::i18n::tf("the sign-in failed: {}", &[&acc::clean(&e, 200)]))));
                return true;
            }
        };
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == flow.extension).cloned() else { return true };
        let Some(provider) = acc::provider(&active.manifest, &flow.provider) else { return true };
        let Some(o) = provider.oauth.clone() else { return true };
        let ask = Ask {
            id: flow.extension.clone(),
            name: active.manifest.name.clone(),
            publisher: self.extension_publisher(&active),
            provider,
            scopes: flow.scopes.clone(),
            ticket: flow.ticket,
        };
        let transport = self.cfg.exts.accounts.transport.clone();
        let traded = cx.background_executor().spawn(async move {
            let mut t = ext_fetch::exchange_code(transport.as_ref(), &o, &flow.extension, &code, &flow.pkce.verifier, acc::now_secs())?;
            t.label = o.account_url.as_deref().and_then(|u| ext_fetch::fetch_label(transport.as_ref(), u, &t.access)).unwrap_or_else(|| o.name.clone());
            if t.scopes.is_empty() {
                t.scopes = flow.scopes.clone();
            }
            Ok::<_, String>(t)
        });
        cx.spawn(async move |this, cx| {
            let traded = traded.await;
            let _ = this.update(cx, |this, cx| match traded {
                Ok(t) => {
                    this.notice = Some(Notice::Info(crate::i18n::tf("{0} is signed in to {1} as {2}.", &[&ask.id, &ask.provider.name, &t.label]).into()));
                    this.save_account(ask, t, complete, cx);
                }
                Err(e) => complete(Completion::Failed(v::Error::Other(e))),
            });
        })
        .detach();
        true
    }

    /// Answers `ticket` of `extension`'s exthost, if it still runs.
    fn ticket_completer(&self, extension: &str, ticket: u64) -> Complete {
        let proc = self.cfg.exts.code.procs.get(extension).cloned();
        Box::new(move |completion| {
            if let Some(p) = &proc {
                let _ = p.send(&ToHost::Complete { ticket, completion });
            }
        })
    }

    /// The accounts of the extension on its page: read from the keychain when the page opens.
    pub(super) fn refresh_extension_accounts(&mut self, id: &str, cx: &mut Context<Self>) {
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == id).cloned() else { return };
        if active.manifest.permissions.auth.is_empty() {
            return;
        }
        self.ensure_keychain_pump(cx);
        let keychain = self.cfg.exts.accounts.keychain.clone();
        let publisher = self.extension_publisher(&active);
        let providers = active.manifest.permissions.auth.clone();
        let ext = id.to_string();
        let read = off_thread(move || {
            let vault = Vault { keychain: keychain.as_ref(), publisher: &publisher, extension: &ext };
            providers.into_iter().map(|p| (vault.account(&p).ok().flatten().map(|t| t.label), p)).collect::<Vec<_>>()
        });
        let ext = id.to_string();
        cx.spawn(async move |this, cx| {
            let found = read.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                for (label, provider) in found {
                    match label {
                        Some(l) => this.cfg.exts.accounts.known.insert((ext.clone(), provider), l),
                        None => this.cfg.exts.accounts.known.remove(&(ext.clone(), provider)),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// **Sign Out** on the extension page: the token leaves the keychain.
    pub(super) fn sign_out_extension_account(&mut self, ext: String, provider: String, cx: &mut Context<Self>) {
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == ext).cloned() else { return };
        self.ensure_keychain_pump(cx);
        let keychain = self.cfg.exts.accounts.keychain.clone();
        let publisher = self.extension_publisher(&active);
        let name = acc::provider(&active.manifest, &provider).map_or_else(|| provider.clone(), |p| p.name);
        let (e, p) = (ext.clone(), provider.clone());
        let done = off_thread(move || Vault { keychain: keychain.as_ref(), publisher: &publisher, extension: &e }.sign_out(&p));
        cx.spawn(async move |this, cx| {
            let done = done.await.unwrap_or_else(|| Err(crate::i18n::t("the keychain did not answer").to_string()));
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(match done {
                    Ok(()) => {
                        this.cfg.exts.accounts.known.remove(&(ext.clone(), provider));
                        Notice::Info(crate::i18n::tf("{0} is signed out of {1}.", &[&ext, &name]).into())
                    }
                    Err(e) => Notice::Error(crate::i18n::tf("Could not sign out: {}", &[&e]).into()),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// The Accounts section of an extension's page: each provider it may use, with **Sign Out** for signed-in ones.
    pub(super) fn extension_accounts_section(&self, m: &ext_manifest::Manifest, p: &Palette, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if m.permissions.auth.is_empty() {
            return None;
        }
        let rows = m.permissions.auth.iter().enumerate().map(|(i, provider)| {
            let name = acc::provider(m, provider).map_or_else(|| provider.clone(), |p| p.name);
            let row = div().id(("ext-account", i)).flex().items_center().gap(px(theme::SPACE[2]));
            match self.cfg.exts.accounts.known.get(&(m.id.to_string(), provider.clone())) {
                Some(label) => {
                    let (ext, prov) = (m.id.to_string(), provider.clone());
                    row.child(div().child(crate::i18n::tf("{0}: signed in as {1}", &[&name, label]))).child(
                        action_button("ext-sign-out", crate::i18n::t("Sign Out"), p)
                            .on_click(cx.listener(move |this, _, _, cx| this.sign_out_extension_account(ext.clone(), prov.clone(), cx))),
                    )
                }
                None => row.child(div().text_color(c(p.text_muted)).child(crate::i18n::tf("{}: not signed in", &[&name]))),
            }
        });
        Some(div().flex().flex_col().gap(px(theme::SPACE[1])).child(section_title(crate::i18n::t("Accounts"), p)).children(rows))
    }

    /// Answer keychain requests from background threads with GPUI's credential store.
    fn ensure_keychain_pump(&mut self, cx: &mut Context<Self>) {
        let Some(rx) = self.cfg.exts.accounts.keychain_rx.take() else { return };
        let mut ops = crate::ui::bridge(rx);
        let task = cx.spawn(async move |_, cx| {
            while let Some(op) = ops.next().await {
                let started = cx.update(|cx| match op {
                    KeychainOp::Read(key, reply) => {
                        let t = cx.read_credentials(&key);
                        cx.background_executor().spawn(async move {
                            let _ = reply.send(t.await.map(|found| found.map(|(_, secret)| secret)).map_err(|e| e.to_string()));
                        })
                    }
                    KeychainOp::Write(key, value, reply) => {
                        let t = cx.write_credentials(&key, "codeyard", &value);
                        cx.background_executor().spawn(async move {
                            let _ = reply.send(t.await.map_err(|e| e.to_string()));
                        })
                    }
                    KeychainOp::Delete(key, reply) => {
                        let t = cx.delete_credentials(&key);
                        cx.background_executor().spawn(async move {
                            let _ = reply.send(t.await.map_err(|e| e.to_string()));
                        })
                    }
                });
                match started {
                    Ok(t) => t.detach(),
                    Err(_) => break,
                }
            }
        });
        self.cfg.exts.accounts.keychain_pump = Some(task);
    }
}

type Complete = Box<dyn FnOnce(Completion) + Send>;

/// Work that touches the keychain runs on its own thread: keychain calls wait for this window, and the keychain's own
/// tasks need the background executor's threads to be free.
fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> impl std::future::Future<Output = Option<T>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    detached(move || {
        let _ = tx.send(f());
    });
    async move { rx.await.ok() }
}

fn detached(f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name("ext-accounts".into()).spawn(f);
}

/// A first use of an account, waiting for the user.
struct Ask {
    id: String,
    name: String,
    publisher: Publisher,
    provider: Provider,
    scopes: Vec<String>,
    ticket: u64,
}

impl Ask {
    /// The dialog's text: who asks, what Codeyard does with the account, how to undo it.
    fn detail(&self) -> String {
        let who = crate::i18n::tf("{0} ({1}) is published by {2}.", &[&self.name, &self.id, &self.publisher.shown]);
        let what = crate::i18n::tf(
            "Codeyard signs its requests to {} with this account. The extension never sees your password or token. You can sign out on its page in Extensions.",
            &[&self.provider.hosts.join(", ")],
        );
        format!("{who}\n\n{what}")
    }
}

/// The token for a request: from the keychain, refreshed first when it is about to expire.
fn current_token(vault: &Vault, p: &Provider, transport: &dyn Transport) -> Result<Token, v::Error> {
    let Some(token) = vault.account(&p.id).map_err(v::Error::Other)? else {
        return Err(v::Error::Invalid(crate::i18n::tf("not signed in to {0}; call auth.session with “{1}” first", &[&p.name, &p.id])));
    };
    let now = acc::now_secs();
    match &p.oauth {
        Some(o) if token.stale(now) => {
            let fresh = ext_fetch::refresh(transport, o, &token, now).map_err(v::Error::Other)?;
            vault.save_account(&p.id, &fresh).map_err(v::Error::Other)?;
            Ok(fresh)
        }
        _ => Ok(token),
    }
}

/// Listen for callbacks handed over by a second Codeyard (once per process), and register the scheme on Linux.
fn ensure_callback_listener(cx: &mut App) {
    if cfg!(test) || LISTENING.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(dir) = crate::workbench::state_path().and_then(|p| p.parent().map(std::path::Path::to_path_buf)) else { return };
    #[cfg(target_os = "linux")]
    std::thread::spawn(|| {
        let _ = acc::register_scheme();
    });
    match acc::listen_for_callbacks(&acc::callback_port_file(&dir)) {
        Ok(rx) => {
            let mut urls = crate::ui::bridge(rx);
            cx.spawn(async move |cx| {
                while let Some(url) = urls.next().await {
                    if cx.update(|cx| deliver_callback(&url, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        Err(_) => LISTENING.store(false, Ordering::SeqCst),
    }
}

#[cfg(test)]
impl Workbench {
    /// Accounts against a fake server and an in-memory keychain; GitHub's sign-in answers `github_token`.
    pub(crate) fn use_test_accounts(&mut self, transport: Arc<dyn Transport>, github_token: &str) -> Arc<acc::MemoryKeychain> {
        let kc = Arc::new(acc::MemoryKeychain::default());
        let token = github_token.to_string();
        let a = &mut self.cfg.exts.accounts;
        a.keychain = kc.clone();
        a.transport = transport;
        a.github_token = Arc::new(move || Ok(token.clone()));
        kc
    }

    /// The state of the pending browser sign-in of `extension`.
    pub(crate) fn sign_in_state_for_test(&self, extension: &str) -> Option<String> {
        self.cfg.exts.accounts.flows.state_of(extension)
    }

    pub(crate) fn extension_callback_for_test(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.on_extension_callback(url, window, cx)
    }

    pub(crate) fn sign_out_for_test(&mut self, extension: &str, provider: &str, cx: &mut Context<Self>) {
        self.sign_out_extension_account(extension.to_string(), provider.to_string(), cx);
    }

    pub(crate) fn accounts_known_for_test(&self) -> Vec<(String, String, String)> {
        self.cfg.exts.accounts.known.iter().map(|((e, p), l)| (e.clone(), p.clone(), l.clone())).collect()
    }
}
