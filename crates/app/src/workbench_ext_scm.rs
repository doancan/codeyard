//! Workbench: source control providers of code extensions (`[[scm]]`, #144; docs/plugin-architecture.md §5.1).
//!
//! A provider claims a trusted workspace folder that has one of its markers at the top (a `.hg` or `.jj` folder);
//! the first provider that matches wins, and git keeps every other folder. For a claimed folder Source Control shows
//! the provider's groups and input box (scm_ext.rs), and the gutter's change marks compare open files with the
//! original text the provider gives. The state is asked again when files change (debounced), on the Refresh button,
//! after one of the provider's buttons ran, and when the extension asks (`scm.refresh`). Buttons run the extension's
//! own commands as user actions; any program they start goes through `run` and the broker on the workspace host, so it
//! works the same over SSH. Untrusted folders are never claimed, so the provider does not start there.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use ext_api::value::{self as v, LogLevel, Request, Response};
use ext_manifest::limits;
use ext_proto::ToHost;
use futures::StreamExt;
use futures::channel::oneshot;

use super::*;
use crate::ext::Active;
use crate::ext_broker;
use crate::scm::{ExtEvent, ExtSource};

/// File changes that come in a burst ask once.
const REFRESH_DELAY: Duration = Duration::from_millis(300);
/// Typing in the input box asks for a check once the user pauses.
const VALIDATE_DELAY: Duration = Duration::from_millis(300);

/// One folder a provider claims.
struct Claim {
    root: String,
    extension: String,
    /// The provider's local id.
    provider: String,
    label: String,
    markers: Vec<String>,
    state: Option<Arc<v::ScmState>>,
    error: Option<String>,
    /// A button's command runs.
    busy: bool,
    validation: Option<String>,
    /// A state request is out; `again`: ask once more when it is answered.
    asking: bool,
    again: bool,
}

/// Source control providers in this window.
#[derive(Default)]
pub(super) struct ScmProviders {
    /// What the claims were found for (host, trusted folders, providers); found again when it changes.
    key: Option<String>,
    claims: Vec<Claim>,
    /// Folders whose files changed since the last frame.
    dirty: HashSet<String>,
    /// Waiting refreshes and checks, per folder: replacing one cancels the wait.
    waits: HashMap<(String, bool), gpui::Task<()>>,
    /// Editors whose change marks come from a provider.
    owned: HashSet<String>,
}

/// Provider text shown in the panel: trimmed to `max` characters, without control or invisible characters.
fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !ext_manifest::is_deceptive(*c)).take(max).collect::<String>().trim().to_string()
}

fn under(root: &str, path: &str) -> bool {
    path.starts_with(&format!("{}/", root.trim_end_matches('/')))
}

/// The provider's state as Source Control may show it: files under the folder only, at most `SCM_RESOURCES` of them,
/// text cleaned, buttons only for the extension's own commands.
fn checked(active: &Active, root: &str, mut s: v::ScmState) -> v::ScmState {
    let own = |a: &v::ScmAction| active.manifest.contributions.commands.iter().any(|c| c.id == active.manifest.id.qualify(&a.command));
    let action = |a: v::ScmAction| v::ScmAction { title: clean(&a.title, limits::UI_LABEL_LEN), ..a };
    let mut left = limits::SCM_RESOURCES;
    s.head = s.head.map(|h| clean(&h, limits::UI_LABEL_LEN)).filter(|h| !h.is_empty());
    s.input.placeholder = clean(&s.input.placeholder, limits::UI_LABEL_LEN);
    s.input.accept = s.input.accept.filter(own).map(action);
    for g in &mut s.groups {
        g.label = clean(&g.label, limits::UI_LABEL_LEN);
        g.resources.retain(|r| ext_broker::normalize(&r.uri).is_some_and(|p| under(root, &p)));
        g.resources.truncate(left);
        left -= g.resources.len();
        for r in &mut g.resources {
            r.detail = r.detail.as_deref().map(|d| clean(d, limits::UI_LABEL_LEN)).filter(|d| !d.is_empty());
        }
        g.resource_actions = std::mem::take(&mut g.resource_actions).into_iter().filter(own).map(action).collect();
        g.group_actions = std::mem::take(&mut g.group_actions).into_iter().filter(own).map(action).collect();
    }
    s
}

fn active(id: &str) -> Option<Active> {
    crate::ext::current().active.iter().find(|e| e.id() == id).cloned()
}

impl Workbench {
    // --- which folders are claimed ---

    /// Every frame: find the claimed folders again when the folders, their trust, the host or the providers changed,
    /// and ask about folders whose files changed.
    pub(super) fn sync_ext_scm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for root in std::mem::take(&mut self.cfg.exts.scm.dirty) {
            self.refresh_ext_scm(&root, REFRESH_DELAY, window, cx);
        }
        let reg = crate::ext::current();
        let providers: Vec<(String, ext_manifest::ScmProvider)> = reg
            .active
            .iter()
            .filter(|e| e.manifest.code.is_some() && !self.cfg.exts.code.disabled.contains(e.id()))
            .flat_map(|e| e.manifest.contributions.scm.iter().map(|p| (e.id().to_string(), p.clone())))
            .collect();
        // Trust is read only when something else changed: trusting or distrusting a folder changes `cfg.trusted`.
        let roots = self.all_roots(cx);
        let gate = cx.global::<Settings>().workspace_trust;
        let key = format!("{}\n{roots:?}\n{providers:?}\n{gate} {}", self.remote.host.label(), self.cfg.trusted.len());
        if self.cfg.exts.scm.key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.cfg.exts.scm.key = Some(key.clone());
        let trusted: Vec<String> = roots.into_iter().filter(|r| self.is_trusted_root(r, cx)).collect();
        if providers.is_empty() || trusted.is_empty() {
            self.set_ext_scm_claims(Vec::new(), window, cx);
            return;
        }
        let host = self.remote.host.clone();
        let found = cx.background_executor().spawn(async move {
            let mut out = Vec::new();
            for root in trusted {
                let base = root.trim_end_matches('/').to_string();
                if let Some((ext, p)) = providers.iter().find(|(_, p)| p.markers.iter().any(|m| host.stat(&format!("{base}/{m}")).is_ok())) {
                    out.push((root, ext.clone(), p.clone()));
                }
            }
            out
        });
        cx.spawn_in(window, async move |this, cx| {
            let found = found.await;
            this.update_in(cx, |this, window, cx| {
                if this.cfg.exts.scm.key.as_deref() == Some(key.as_str()) {
                    this.set_ext_scm_claims(found, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Replace the claims: a folder that stays with the same provider keeps its state; new ones are asked at once.
    fn set_ext_scm_claims(&mut self, found: Vec<(String, String, ext_manifest::ScmProvider)>, window: &mut Window, cx: &mut Context<Self>) {
        let mut old = std::mem::take(&mut self.cfg.exts.scm.claims);
        let mut new = Vec::new();
        for (root, extension, p) in found {
            let local = p.id.strip_prefix(&format!("{extension}.")).unwrap_or(&p.id).to_string();
            match old.iter().position(|c| c.root == root && c.extension == extension && c.provider == local) {
                Some(i) => new.push(old.remove(i)),
                None => new.push(Claim {
                    root,
                    extension,
                    provider: local,
                    label: p.label.clone(),
                    markers: p.markers.clone(),
                    state: None,
                    error: None,
                    busy: false,
                    validation: None,
                    asking: false,
                    again: false,
                }),
            }
        }
        let fresh: Vec<String> = new.iter().filter(|c| c.state.is_none() && !c.asking).map(|c| c.root.clone()).collect();
        self.cfg.exts.scm.claims = new;
        self.cfg.exts.scm.waits.retain(|(root, _), _| fresh.contains(root) || !old.iter().any(|c| &c.root == root));
        if !old.is_empty() {
            self.release_ext_scm_editors(cx);
        }
        self.push_ext_scm(cx);
        for root in fresh {
            self.ask_ext_scm_status(&root, window, cx);
        }
    }

    /// Editors of folders no longer claimed get git's marks back.
    fn release_ext_scm_editors(&mut self, cx: &mut Context<Self>) {
        let roots: Vec<String> = self.cfg.exts.scm.claims.iter().map(|c| c.root.clone()).collect();
        let owned = std::mem::take(&mut self.cfg.exts.scm.owned);
        for ed in self.all_editors() {
            let path = ed.read(cx).path.clone();
            if !owned.contains(&path) {
                continue;
            }
            if roots.iter().any(|r| under(r, &path)) {
                self.cfg.exts.scm.owned.insert(path);
                continue;
            }
            let (root, changed) = git_context(self.vcs.status.as_deref(), &path);
            ed.update(cx, |e, cx| {
                e.release_scm_base(cx);
                if root.is_some() {
                    e.set_git_root(root, changed, cx);
                }
            });
        }
    }

    /// Files changed (the watcher): folders they are in are asked again on the next frame. Changes inside a marker
    /// (the provider's own data, which asking may write) are left out.
    pub(super) fn ext_scm_files_changed(&mut self, paths: &[String]) {
        for c in &self.cfg.exts.scm.claims {
            let inside = |p: &String| {
                under(&c.root, p)
                    && !c
                        .markers
                        .iter()
                        .any(|m| p == &format!("{}/{m}", c.root.trim_end_matches('/')) || under(&format!("{}/{m}", c.root.trim_end_matches('/')), p))
            };
            if paths.iter().any(inside) {
                self.cfg.exts.scm.dirty.insert(c.root.clone());
            }
        }
    }

    /// `scm.refresh` from the extension.
    pub(super) fn on_ext_scm_refresh(&mut self, extension: &str, provider: &str, window: &mut Window, cx: &mut Context<Self>) {
        let roots: Vec<String> =
            self.cfg.exts.scm.claims.iter().filter(|c| c.extension == extension && c.provider == provider).map(|c| c.root.clone()).collect();
        for root in roots {
            self.refresh_ext_scm(&root, Duration::ZERO, window, cx);
        }
    }

    // --- asking ---

    /// Ask for `root`'s state after `delay`; a newer call replaces a waiting one.
    fn refresh_ext_scm(&mut self, root: &str, delay: Duration, window: &mut Window, cx: &mut Context<Self>) {
        let r = root.to_string();
        let task = cx.spawn_in(window, async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            this.update_in(cx, |this, window, cx| {
                this.cfg.exts.scm.waits.remove(&(r.clone(), false));
                this.ask_ext_scm_status(&r, window, cx);
            })
            .ok();
        });
        self.cfg.exts.scm.waits.insert((root.to_string(), false), task);
    }

    /// Send `request` to the provider's extension (starting it if needed) and wait up to `SCM_DEADLINE_MILLIS`.
    fn ask_ext_scm(
        &mut self,
        active: &Active,
        provider: &str,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Result<Response, v::Error>> {
        let ext = active.id().to_string();
        let running = self.cfg.exts.code.procs.contains_key(&ext);
        let deadline = Duration::from_millis(limits::SCM_DEADLINE_MILLIS + if running { 0 } else { limits::ACTIVATE_MILLIS });
        let (id, answer) = self.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).add(&ext);
        let msg = ToHost::Handle { id, deadline_ms: deadline.as_millis() as u32, request };
        self.send_to_extension(active, &format!("scm:{provider}"), Some(msg), window, cx);
        let (tx, out) = oneshot::channel();
        let mut answer = crate::ui::bridge(answer);
        cx.spawn(async move |this, cx| {
            let timer = cx.background_executor().timer(deadline);
            match futures::future::select(answer.next(), timer).await {
                futures::future::Either::Left((a, _)) => {
                    let _ = tx.send(a.unwrap_or(Err(v::Error::Cancelled)));
                }
                futures::future::Either::Right(_) => {
                    let _ = tx.send(Err(v::Error::Timeout));
                    this.update(cx, |this, _| {
                        if let Some(p) = this.cfg.exts.code.procs.get(&ext) {
                            let _ = p.send(&ToHost::Cancel { id });
                        }
                        this.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).answer(&ext, id, Err(v::Error::Cancelled));
                        this.log_extension(&ext, LogLevel::Warn, format!("source control request {id} did not answer within {} ms", deadline.as_millis()));
                    })
                    .ok();
                }
            }
        })
        .detach();
        out
    }

    fn claim(&self, root: &str) -> Option<&Claim> {
        self.cfg.exts.scm.claims.iter().find(|c| c.root == root)
    }

    fn claim_mut(&mut self, root: &str) -> Option<&mut Claim> {
        self.cfg.exts.scm.claims.iter_mut().find(|c| c.root == root)
    }

    /// The claim of `root` and its extension, while both are there.
    fn claim_active(&self, root: &str) -> Option<(Active, String)> {
        let c = self.claim(root)?;
        Some((active(&c.extension)?, c.provider.clone()))
    }

    fn ask_ext_scm_status(&mut self, root: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((active, provider)) = self.claim_active(root) else { return };
        let Some(c) = self.claim_mut(root) else { return };
        if c.asking {
            c.again = true;
            return;
        }
        c.asking = true;
        let request = Request::ScmStatus { provider: provider.clone(), root: ext_broker::uri(root) };
        let answer = self.ask_ext_scm(&active, &provider, request, window, cx);
        let root = root.to_string();
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.unwrap_or(Err(v::Error::Cancelled));
            this.update_in(cx, |this, window, cx| {
                let Some(c) = this.claim_mut(&root) else { return };
                c.asking = false;
                match answer {
                    Ok(Response::Scm(s)) => {
                        c.state = Some(Arc::new(checked(&active, &root, s)));
                        c.error = None;
                    }
                    Ok(other) => c.error = Some(crate::i18n::tf("The provider answered with something else: {}", &[&format!("{other:?}")])),
                    Err(e) => c.error = Some(super::code_glue::explain(&e)),
                }
                let again = std::mem::take(&mut c.again);
                this.push_ext_scm(cx);
                this.apply_ext_scm_marks(&root, None, window, cx);
                if again {
                    this.ask_ext_scm_status(&root, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The sources as the panel draws them.
    fn push_ext_scm(&mut self, cx: &mut Context<Self>) {
        let list: Vec<ExtSource> = self
            .cfg
            .exts
            .scm
            .claims
            .iter()
            .map(|c| ExtSource {
                root: c.root.clone(),
                title: format!("{} · {}", c.label, crate::ui::file_name(&c.root)),
                state: c.state.clone(),
                error: c.error.clone(),
                busy: c.busy,
                validation: c.validation.clone(),
            })
            .collect();
        self.scm.update(cx, |s, cx| s.set_ext_sources(list, cx));
    }

    // --- the gutter ---

    /// Change marks of the open files in `root` (or only `only`) come from the provider: a changed file is compared
    /// with its original text, a new one has none, and a file the provider does not list is compared with itself as
    /// saved.
    fn apply_ext_scm_marks(&mut self, root: &str, only: Option<&Entity<EditorView>>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.claim(root).and_then(|c| c.state.clone()) else { return };
        let Some((active, provider)) = self.claim_active(root) else { return };
        let host = self.remote.host.clone();
        let editors: Vec<Entity<EditorView>> = match only {
            Some(ed) => vec![ed.clone()],
            None => self.all_editors(),
        };
        for ed in editors {
            let (path, dirty) = {
                let e = ed.read(cx);
                if !e.uses_host(&host) || !under(root, &e.path) {
                    continue;
                }
                (e.path.clone(), e.is_dirty())
            };
            self.cfg.exts.scm.owned.insert(path.clone());
            let listed =
                state.groups.iter().flat_map(|g| &g.resources).find(|r| ext_broker::normalize(&r.uri).as_deref() == Some(path.as_str())).map(|r| r.state);
            match listed {
                Some(v::FileState::Added | v::FileState::Untracked | v::FileState::Ignored) => ed.update(cx, |e, cx| e.set_scm_base(None, cx)),
                None if !dirty => ed.update(cx, |e, cx| e.set_scm_base_saved(cx)),
                _ => {
                    let request = Request::ScmOriginal { provider: provider.clone(), uri: ext_broker::uri(&path) };
                    let answer = self.ask_ext_scm(&active, &provider, request, window, cx);
                    let ed = ed.downgrade();
                    cx.spawn(async move |_, cx| {
                        let text = match answer.await {
                            Ok(Ok(Response::Text(t))) => t.filter(|t| t.len() <= limits::SCM_ORIGINAL_BYTES),
                            _ => None,
                        };
                        ed.update(cx, |e, cx| e.set_scm_base(text, cx)).ok();
                    })
                    .detach();
                }
            }
        }
    }

    /// A file opened in a claimed folder.
    pub(super) fn attach_ext_scm(&mut self, ed: &Entity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        if let Some(root) = self.cfg.exts.scm.claims.iter().find(|c| under(&c.root, &path)).map(|c| c.root.clone()) {
            self.apply_ext_scm_marks(&root, Some(ed), window, cx);
        }
    }

    // --- what the user does in the panel ---

    pub(super) fn on_ext_scm_event(&mut self, event: ExtEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ExtEvent::Run { root, command, uris, message, accept } => self.run_ext_scm_action(&root, &command, uris, message, accept, window, cx),
            ExtEvent::Typed { root, text } => self.validate_ext_scm(&root, text, window, cx),
            ExtEvent::Open { root, uri } => self.open_ext_scm_change(&root, &uri, window, cx),
            ExtEvent::Refresh => {
                let roots: Vec<String> = self.cfg.exts.scm.claims.iter().map(|c| c.root.clone()).collect();
                for root in roots {
                    self.refresh_ext_scm(&root, Duration::ZERO, window, cx);
                }
            }
        }
    }

    /// Ask the provider about the box's text once the user pauses.
    fn validate_ext_scm(&mut self, root: &str, text: String, window: &mut Window, cx: &mut Context<Self>) {
        // An empty box (also after a commit) has nothing to say yet; the accept button still asks.
        if text.trim().is_empty() {
            self.cfg.exts.scm.waits.remove(&(root.to_string(), true));
            return self.set_ext_scm_validation(root, None, cx);
        }
        let r = root.to_string();
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(VALIDATE_DELAY).await;
            let Ok(Some(answer)) = this.update_in(cx, |this, window, cx| this.ask_ext_scm_validate(&r, text, window, cx)) else { return };
            let message = answer.await;
            this.update(cx, |this, cx| this.set_ext_scm_validation(&r, message, cx)).ok();
        });
        self.cfg.exts.scm.waits.insert((root.to_string(), true), task);
    }

    fn ask_ext_scm_validate(
        &mut self,
        root: &str,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl std::future::Future<Output = Option<String>> + use<>> {
        let (active, provider) = self.claim_active(root)?;
        let request = Request::ScmValidate { provider: provider.clone(), root: ext_broker::uri(root), value: text };
        let answer = self.ask_ext_scm(&active, &provider, request, window, cx);
        Some(async move {
            match answer.await {
                Ok(Ok(Response::Text(t))) => t.map(|t| clean(&t, limits::UI_TEXT_LEN)).filter(|t| !t.is_empty()),
                _ => None,
            }
        })
    }

    fn set_ext_scm_validation(&mut self, root: &str, message: Option<String>, cx: &mut Context<Self>) {
        if let Some(c) = self.claim_mut(root)
            && c.validation != message
        {
            c.validation = message;
            self.push_ext_scm(cx);
        }
    }

    /// A button: the extension's command with the files and the box's text, as a user action. The box's own button
    /// asks for a check first and does not run while there is a message; when its command succeeds the box empties.
    #[allow(clippy::too_many_arguments)]
    fn run_ext_scm_action(&mut self, root: &str, command: &str, uris: Vec<String>, message: String, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((active, provider)) = self.claim_active(root) else { return };
        if self.claim(root).is_some_and(|c| c.busy) {
            return;
        }
        if !active.manifest.contributions.commands.iter().any(|c| c.id == active.manifest.id.qualify(command)) {
            self.log_extension(active.id(), LogLevel::Warn, format!("{command} is not one of its commands"));
            return;
        }
        if let Some(c) = self.claim_mut(root) {
            c.busy = true;
        }
        self.push_ext_scm(cx);
        let check = if accept { self.ask_ext_scm_validate(root, message.clone(), window, cx) } else { None };
        let args = serde_json::json!({ "provider": provider, "root": ext_broker::uri(root), "uris": uris, "message": message }).to_string();
        let (root, command) = (root.to_string(), command.to_string());
        cx.spawn_in(window, async move |this, cx| {
            if let Some(check) = check
                && let Some(problem) = check.await
            {
                this.update(cx, |this, cx| {
                    if let Some(c) = this.claim_mut(&root) {
                        c.busy = false;
                    }
                    this.set_ext_scm_validation(&root, Some(problem), cx);
                    this.push_ext_scm(cx);
                })
                .ok();
                return;
            }
            let Ok(answer) = this.update_in(cx, |this, window, cx| this.run_ext_scm_command(&active, &command, args, window, cx)) else { return };
            let result = answer.await.unwrap_or(Err(v::Error::Cancelled));
            this.update_in(cx, |this, window, cx| {
                if let Some(c) = this.claim_mut(&root) {
                    c.busy = false;
                    if accept && result.is_ok() {
                        c.validation = None;
                    }
                }
                if accept && result.is_ok() {
                    this.scm.update(cx, |s, cx| s.set_ext_message(&root, "", cx));
                }
                this.push_ext_scm(cx);
                this.refresh_ext_scm(&root, Duration::ZERO, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Run a command of the extension as a user action; its answer is kept for the Inspector and an error is shown.
    fn run_ext_scm_command(
        &mut self,
        active: &Active,
        command: &str,
        args: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Result<Response, v::Error>> {
        let ext = active.id().to_string();
        let (tx, out) = oneshot::channel();
        if self.cfg.exts.code.disabled.contains(&ext) {
            self.notice = Some(Notice::Error(crate::i18n::tf("{} crashed three times and is turned off until Codeyard restarts.", &[&ext]).into()));
            let _ = tx.send(Err(v::Error::Cancelled));
            return out;
        }
        let (id, answer) = self.cfg.exts.code.pending.lock().unwrap_or_else(|p| p.into_inner()).add(&ext);
        self.cfg.exts.code.user_action.insert(ext.clone(), id);
        self.cfg.exts.code.record().request_sent(&ext, id, &format!("command:{command}"));
        let request = crate::ext_runtime::command(command, &args);
        self.send_to_extension(active, &format!("command:{command}"), Some(ToHost::Handle { id, deadline_ms: 0, request }), window, cx);
        let mut answer = crate::ui::bridge(answer);
        cx.spawn(async move |this, cx| {
            let result = answer.next().await.unwrap_or(Err(v::Error::Cancelled));
            this.update(cx, |this, cx| {
                if this.cfg.exts.code.user_action.get(&ext) == Some(&id) {
                    this.cfg.exts.code.user_action.remove(&ext);
                }
                if let Ok(Response::Json(j)) = &result {
                    this.cfg.exts.code.last_answer.insert(ext.clone(), j.clone());
                }
                if let Err(e) = &result
                    && !this.cfg.exts.code.disabled.contains(&ext)
                {
                    this.notice = Some(Notice::Error(format!("{ext}: {}", super::code_glue::explain(e)).into()));
                    cx.notify();
                }
            })
            .ok();
            let _ = tx.send(result);
        })
        .detach();
        out
    }

    /// A file's changes: its original text from the provider beside the file as it is now.
    fn open_ext_scm_change(&mut self, root: &str, uri: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((active, provider)) = self.claim_active(root) else { return };
        let Some(path) = ext_broker::normalize(uri).filter(|p| under(root, p)) else { return };
        let answer = self.ask_ext_scm(&active, &provider, Request::ScmOriginal { provider: provider.clone(), uri: uri.to_string() }, window, cx);
        let host = self.remote.host.clone();
        let p = path.clone();
        let now = cx.background_executor().spawn(async move { host.read(&p).ok().map(|(d, _)| String::from_utf8_lossy(&d).replace("\r\n", "\n")) });
        let title = crate::scm::ext_relative(root, uri);
        cx.spawn_in(window, async move |this, cx| {
            let original = match answer.await {
                Ok(Ok(Response::Text(t))) => t.filter(|t| t.len() <= limits::SCM_ORIGINAL_BYTES).unwrap_or_default(),
                _ => String::new(),
            };
            let now = now.await.unwrap_or_default();
            this.update_in(cx, |this, window, cx| {
                this.open_compare(title, crate::i18n::t("Source Control"), &original.replace("\r\n", "\n"), &now, window, cx)
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
impl Workbench {
    pub fn ext_scm_sources_for_test(&self, cx: &App) -> Vec<ExtSource> {
        self.scm.read(cx).ext_sources().to_vec()
    }

    /// Type `message` into the box of `root` and press its button, as the user does.
    pub fn ext_scm_accept_for_test(&mut self, root: &str, message: &str, cx: &mut Context<Self>) {
        self.scm.update(cx, |s, cx| {
            s.set_ext_message(root, message, cx);
            s.ext_accept(root, cx);
        });
    }

    /// Press the button titled `title` on the row of `file` (a path relative to `root`), or on the header of group
    /// `file` when there is no such row.
    pub fn ext_scm_press_for_test(&mut self, root: &str, file: &str, title: &str, cx: &mut Context<Self>) {
        let Some(state) = self.claim(root).and_then(|c| c.state.clone()) else { panic!("no state for {root}") };
        for g in &state.groups {
            if let Some(r) = g.resources.iter().find(|r| crate::scm::ext_relative(root, &r.uri) == file) {
                let a = g.resource_actions.iter().find(|a| a.title == title).expect("the button").clone();
                let uris = vec![r.uri.clone()];
                return self.scm.update(cx, |s, cx| s.ext_run(root, &a, uris, cx));
            }
            if g.id == file {
                let a = g.group_actions.iter().find(|a| a.title == title).expect("the button").clone();
                let uris = g.resources.iter().map(|r| r.uri.clone()).collect();
                return self.scm.update(cx, |s, cx| s.ext_run(root, &a, uris, cx));
            }
        }
        panic!("no row or group {file}");
    }

    pub fn refresh_ext_scm_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.on_ext_scm_event(ExtEvent::Refresh, window, cx);
    }

    pub fn trust_root_for_test(&mut self, root: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.trust_root(root, window, cx);
    }
}
