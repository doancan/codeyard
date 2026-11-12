//! Workbench: `Cargo.toml` dependency intelligence (cargo_deps.rs). A manifest's editor is bound to a registry and gets its
//! hints in the background (`schedule_cargo`, called from `schedule_schema`, so every open, edit, reload and settings change
//! reaches it); Quick Fix and the palette update, add and remove dependencies. Adding and removing run `cargo add` and
//! `cargo remove` where they work and edit the text itself otherwise, keeping comments and layout.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Mutex;

use super::*;
use crate::cargo_project::deps::config::{self, Source as ConfigSource};
use crate::cargo_project::deps::edit::{self, Edit, EditError};
use crate::cargo_project::deps::fetch::Fetch;
use crate::cargo_project::deps::hints::{self, Lookups};
use crate::cargo_project::deps::lock::Lock;
use crate::cargo_project::deps::manifest::{self, Kind};
use crate::cargo_project::deps::registry::{Endpoints, Lookup, Registry, SearchHit, valid_crate_name};
use crate::cargo_project::deps::version::Update;
use crate::cargo_project::deps::{context, is_manifest};
use crate::editor::CargoBinding;
use crate::i18n::{t, tf, tn};

/// The question about contacting crates.io, and the command that does it once (workbench_cargo_consent.rs).
#[path = "workbench_cargo_consent.rs"]
mod consent;

gpui::actions!(cargo_deps, [AddDependency, RemoveDependency, UpdateDependency, UpdateAllCompatible, UpdateAllLatest, CheckDependencyUpdates]);

/// The registries by endpoint, shared with the background jobs.
pub struct Pool {
    fetch: Arc<dyn Fetch>,
    cache_dir: Option<PathBuf>,
    registries: Mutex<HashMap<Endpoints, Arc<Registry>>>,
}

impl Pool {
    pub fn new(fetch: Arc<dyn Fetch>, cache_dir: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self { fetch, cache_dir, registries: Mutex::default() })
    }

    pub fn registry(&self, endpoints: &Endpoints) -> Arc<Registry> {
        let mut all = self.registries.lock().unwrap_or_else(|e| e.into_inner());
        all.entry(endpoints.clone())
            .or_insert_with(|| {
                let disk = crate::cargo_project::deps::cache::DiskCache::new(self.cache_dir.clone().map(|d| d.join(sanitize(&endpoints.index))));
                if self.cache_dir.is_some() {
                    // Entries nobody refreshed for two months go; once per run, off the caller's thread.
                    let old = disk.clone();
                    std::thread::spawn(move || old.prune(crate::cargo_project::deps::cache::now_secs()));
                }
                Registry::new(self.fetch.clone(), disk, endpoints.clone())
            })
            .clone()
    }
}

/// A folder name for an index address: letters and digits only.
fn sanitize(url: &str) -> String {
    url.trim_start_matches("https://").chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// What tests run against when they do not bring their own registry: nothing is reachable.
#[cfg(test)]
struct Unreachable;

#[cfg(test)]
impl Fetch for Unreachable {
    fn get(&self, _: &str, _: Option<&str>) -> Result<crate::cargo_project::deps::fetch::Response, crate::cargo_project::deps::fetch::FetchError> {
        Err(crate::cargo_project::deps::fetch::FetchError::Offline)
    }
}

/// An edit Quick Fix can offer. Spans identify the dependency; the edit is computed again from the document as it is when run.
#[derive(Clone)]
pub(crate) enum FixAction {
    Update { key_span: Range<usize>, how: Update },
    UpdateAll(Update),
    Remove { key_span: Range<usize> },
}

#[derive(Clone)]
pub(crate) struct CargoFix {
    pub title: String,
    pub action: FixAction,
}

#[derive(Default)]
pub(crate) struct CargoState {
    pool: Option<Arc<Pool>>,
    tasks: HashMap<String, gpui::Task<()>>,
    problems: HashMap<String, Vec<crate::lsp::Diagnostic>>,
    lookups: HashMap<String, Arc<Lookups>>,
    search: Option<gpui::Task<()>>,
    /// Manifests the user asked to look up (Check Dependency Updates, Update All) while `rust.cargo.dependencyLookups` is off:
    /// they are looked up and kept fresh until their tab closes.
    explicit: std::collections::HashSet<String>,
    /// An update waiting for the lookups of a manifest that was just asked for: (one dependency's key span or all, how).
    pending: HashMap<String, (Option<Range<usize>>, Update)>,
    /// "Not now" to the question about contacting crates.io: not asked again until the next start.
    declined: bool,
}

impl CargoState {
    pub(crate) fn problems_of(&self, path: &str) -> Vec<crate::lsp::Diagnostic> {
        self.problems.get(path).cloned().unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn has_state_for_test(&self, path: &str) -> bool {
        self.tasks.contains_key(path) || self.lookups.contains_key(path) || self.problems.contains_key(path) || self.explicit.contains(path)
    }

    #[cfg(test)]
    pub(crate) fn pool_dropped_for_test(&self) -> bool {
        self.pool.is_none()
    }

    pub(crate) fn all_problems(&self) -> impl Iterator<Item = (&String, &Vec<crate::lsp::Diagnostic>)> {
        self.problems.iter()
    }
}

/// What a refresh needs, taken on the UI thread.
struct Job {
    text: String,
    path: String,
    host: Arc<dyn Host>,
    root: Option<String>,
    trusted: bool,
    pool: Arc<Pool>,
    cargo_home: Option<PathBuf>,
}

struct Analysis {
    binding: Option<Arc<CargoBinding>>,
    lookups: Lookups,
    manifest: manifest::Manifest,
}

/// The lock file that governs `manifest`: the nearest `Cargo.lock` up to the workspace root.
fn read_lock(host: &dyn Host, manifest: &str, root: Option<&str>) -> Option<Lock> {
    const MAX_LOCK_BYTES: u64 = 16 * 1024 * 1024;
    let (mut dir, _) = manifest.rsplit_once('/')?;
    let stop = root.unwrap_or(dir).trim_end_matches('/').to_string();
    loop {
        let candidate = ui::join(dir, "Cargo.lock");
        if host.stat(&candidate).is_ok_and(|m| matches!(m.kind, proto::Kind::File | proto::Kind::Symlink) && m.size <= MAX_LOCK_BYTES) {
            return host.read(&candidate).ok().and_then(|(bytes, _)| Lock::parse(&String::from_utf8_lossy(&bytes)));
        }
        if dir == stop || !dir.starts_with(&stop) {
            return None;
        }
        dir = dir.rsplit_once('/')?.0;
    }
}

/// Where the lookups for a manifest go: crates.io, or what the user's Cargo configuration replaces it with; `None` when the
/// configuration points somewhere that cannot be asked. Over SSH the default is used. Reads files: not on the UI thread.
fn resolve_endpoints(remote: bool, manifest: &str, cargo_home: Option<&std::path::Path>, trusted: bool) -> Option<Endpoints> {
    let dir = if remote { None } else { manifest.rsplit_once('/').map(|(d, _)| PathBuf::from(d)) };
    let Some(dir) = dir else { return Some(Endpoints::default()) };
    match config::resolve(&config::read_chain(&dir, cargo_home, trusted)) {
        ConfigSource::Endpoints(e) => Some(e),
        ConfigSource::Unsupported => None,
    }
}

/// A folder counts as trusted for running `cargo` and reading a project's Cargo configuration only when the file is inside a
/// folder the user trusted: a file outside every folder has no author to trust.
pub(crate) fn root_is_trusted(root: Option<&str>, trusted: impl Fn(&str) -> bool) -> bool {
    root.is_some_and(trusted)
}

/// Parse, look up and build the hints. `None`: the text is not valid TOML right now, so what is shown stays.
fn analyse(job: Job) -> Option<Analysis> {
    let manifest = manifest::parse(&job.text).ok()?;
    let Some(endpoints) = resolve_endpoints(job.host.is_remote(), &job.path, job.cargo_home.as_deref(), job.trusted) else {
        return Some(Analysis { binding: None, lookups: Lookups::new(), manifest });
    };
    let registry = job.pool.registry(&endpoints);
    let lock = read_lock(job.host.as_ref(), &job.path, job.root.as_deref()).map(Arc::new);
    let mut names: Vec<String> = hints::wanted(&manifest).iter().map(|d| d.package.to_ascii_lowercase()).collect();
    names.sort();
    names.dedup();
    let lookups: Lookups = registry.lookup_many(&names).into_iter().collect();
    Some(Analysis { binding: Some(Arc::new(CargoBinding { registry, lock })), lookups, manifest })
}

impl Workbench {
    fn cargo_pool(&mut self) -> Arc<Pool> {
        self.lsp
            .cargo
            .pool
            .get_or_insert_with(|| {
                #[cfg(test)]
                return Pool::new(Arc::new(Unreachable), None);
                #[cfg(not(test))]
                Pool::new(
                    Arc::new(crate::cargo_project::deps::fetch::CurlFetch::new()),
                    dirs::cache_dir().map(|d| d.join(host::ssh::APP_DIR).join("cargo-deps")),
                )
            })
            .clone()
    }

    /// Bind `ed` (a `Cargo.toml`) to the registry and refresh its hints after `delay`. A file that is not a manifest, or the
    /// setting `rust.cargo.dependencyLookups` being off, unbinds it.
    pub(in crate::workbench) fn schedule_cargo(&mut self, ed: &Entity<EditorView>, delay: Duration, cx: &mut Context<Self>) {
        let path = ed.read(cx).path.clone();
        if !is_manifest(&path) {
            return;
        }
        // Off by default: nothing is looked up on its own. A manifest the user asked about (Check Dependency Updates, Update
        // All) is the exception, until its tab closes.
        if !cx.global::<Settings>().rust_dependency_lookups && !self.lsp.cargo.explicit.contains(&path) {
            self.lsp.cargo.tasks.remove(&path);
            self.lsp.cargo.lookups.remove(&path);
            if self.lsp.cargo.problems.remove(&path).is_some() {
                let merged = self.diagnostics_for(&path);
                ed.update(cx, |e, cx| e.set_diagnostics(merged, cx));
            }
            ed.update(cx, |e, cx| {
                e.set_cargo(None, cx);
                e.set_cargo_hints(Vec::new(), 0, cx);
            });
            self.cargo_consider_consent(ed, cx);
            return;
        }
        ed.update(cx, |e, cx| e.set_cargo_consent(false, cx));
        let root = self.root_of(&path, cx);
        let (host, pool) = (ed.read(cx).host_arc(), self.cargo_pool());
        let trusted = root_is_trusted(root.as_deref(), |r| self.is_trusted_root(r, cx));
        let cargo_home = (!cfg!(test)).then(|| config::cargo_home(std::env::var_os("CARGO_HOME"), dirs::home_dir())).flatten();
        let weak = ed.downgrade();
        let slot = path.clone();
        let task = cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let Ok((text, version)) = weak.update(cx, |e, _| (e.rope_snapshot().to_string(), e.version())) else { return };
            let job = Job { text: text.clone(), path: path.clone(), host, root, trusted, pool, cargo_home };
            let Some(analysis) = cx.background_executor().spawn(async move { analyse(job) }).await else { return };
            this.update(cx, |this, cx| {
                let Some(ed) = weak.upgrade() else { return };
                // Edited meanwhile: a newer refresh is already scheduled.
                if ed.read(cx).version() != version {
                    return;
                }
                this.apply_cargo_analysis(&ed, &path, &text, analysis, cx);
            })
            .ok();
        });
        self.lsp.cargo.tasks.insert(slot, task);
    }

    fn apply_cargo_analysis(&mut self, ed: &Entity<EditorView>, path: &str, text: &str, analysis: Analysis, cx: &mut Context<Self>) {
        // The words are made here, on the UI thread, in the language that is set now.
        let lock = analysis.binding.as_ref().and_then(|b| b.lock.clone());
        let (hints, problems) = match &analysis.binding {
            Some(_) => {
                (hints::hints(&analysis.manifest, &analysis.lookups, lock.as_deref()), hints::problems(&analysis.manifest, &analysis.lookups, lock.as_deref()))
            }
            None => (Vec::new(), Vec::new()),
        };
        let diagnostics: Vec<crate::lsp::Diagnostic> = problems
            .iter()
            .map(|p| crate::lsp::Diagnostic {
                start: schema::doc::position(text, p.span.start),
                end: schema::doc::position(text, p.span.end),
                severity: 2,
                message: p.message.clone(),
                source: Some("cargo".into()),
                ..Default::default()
            })
            .collect();
        if diagnostics.is_empty() {
            self.lsp.cargo.problems.remove(path);
        } else {
            self.lsp.cargo.problems.insert(path.to_string(), diagnostics);
        }
        self.lsp.cargo.lookups.insert(path.to_string(), Arc::new(analysis.lookups));
        let merged = self.diagnostics_for(path);
        ed.update(cx, |e, cx| {
            e.set_cargo(analysis.binding, cx);
            e.set_cargo_hints(hints, analysis.manifest.lines, cx);
            e.set_diagnostics(merged, cx);
        });
        // An update the user asked for before the lookups existed.
        if let Some((only, how)) = self.lsp.cargo.pending.remove(path) {
            self.cargo_update_in(ed, only, how, cx);
        }
        cx.notify();
    }

    /// The tab of a manifest closed: its tasks, lookups and problems go, and with the last manifest the registries (and what they
    /// hold in memory) too.
    pub(in crate::workbench) fn forget_cargo(&mut self, path: &str, cx: &App) {
        let cargo = &mut self.lsp.cargo;
        cargo.tasks.remove(path);
        cargo.lookups.remove(path);
        cargo.problems.remove(path);
        cargo.pending.remove(path);
        cargo.explicit.remove(path);
        if !self.all_editors().iter().any(|e| is_manifest(&e.read(cx).path)) {
            self.lsp.cargo.pool = None;
        }
    }

    // --- Quick Fix ---

    /// The fixes for the dependency under the cursor and for the whole file; empty when the active file is no bound manifest.
    pub(in crate::workbench) fn cargo_fixes(&self, cx: &App) -> Vec<CargoFix> {
        let Some(ed) = self.active_editor().filter(|e| e.read(cx).has_cargo()) else { return Vec::new() };
        let e = ed.read(cx);
        let Some(lookups) = self.lsp.cargo.lookups.get(&e.path) else { return Vec::new() };
        let (text, _, cursor) = e.cargo_snapshot();
        let Ok(manifest) = manifest::parse(&text) else { return Vec::new() };
        let mut fixes = Vec::new();
        if let Some(dep) = manifest.dep_at(cursor) {
            if let Some(Lookup::Found { info, .. }) = lookups.get(&dep.package.to_ascii_lowercase()) {
                let latest = edit::new_requirement(dep, info, Update::Latest);
                let compatible = edit::new_requirement(dep, info, Update::Compatible).filter(|c| Some(c) != latest.as_ref());
                if let Some(l) = latest {
                    fixes.push(CargoFix {
                        title: tf("Update “{0}” to the latest version, {1}", &[&dep.key, &l.text]),
                        action: FixAction::Update { key_span: dep.key_span.clone(), how: Update::Latest },
                    });
                }
                if let Some(c) = compatible {
                    fixes.push(CargoFix {
                        title: tf("Update “{0}” to {1}, the newest compatible version", &[&dep.key, &c.text]),
                        action: FixAction::Update { key_span: dep.key_span.clone(), how: Update::Compatible },
                    });
                }
            }
            fixes.push(CargoFix { title: tf("Remove dependency “{}”", &[&dep.key]), action: FixAction::Remove { key_span: dep.key_span.clone() } });
        }
        for how in [Update::Compatible, Update::Latest] {
            let n = edit::update_edits(&manifest, lookups, how, None).len();
            if n == 0 {
                continue;
            }
            let title = match how {
                Update::Compatible => tn(
                    "Update {} dependency in this file to its newest compatible version",
                    "Update {} dependencies in this file to their newest compatible versions",
                    n,
                    &[],
                ),
                Update::Latest => {
                    tn("Update {} dependency in this file to the latest version", "Update {} dependencies in this file to the latest versions", n, &[])
                }
            };
            fixes.push(CargoFix { title, action: FixAction::UpdateAll(how) });
        }
        fixes
    }

    pub(in crate::workbench) fn run_cargo_fix(&mut self, fix: CargoFix, window: &mut Window, cx: &mut Context<Self>) {
        match fix.action {
            FixAction::Update { key_span, how } => self.cargo_update(Some(key_span), how, cx),
            FixAction::UpdateAll(how) => self.cargo_update(None, how, cx),
            FixAction::Remove { key_span } => self.cargo_remove(key_span, window, cx),
        }
    }

    /// The active editor when it is a manifest with hints loaded; otherwise says why not.
    fn cargo_editor(&mut self, cx: &mut Context<Self>) -> Option<Entity<EditorView>> {
        let ed = self.active_editor().cloned().filter(|e| is_manifest(&e.read(cx).path));
        let Some(ed) = ed else {
            notify(self, t("Open a Cargo.toml first"), cx);
            return None;
        };
        Some(ed)
    }

    /// Update one dependency (`only` is its key's span) or all of them from the lookups made for this file.
    fn cargo_update(&mut self, only: Option<Range<usize>>, how: Update, cx: &mut Context<Self>) {
        let Some(ed) = self.cargo_editor(cx) else { return };
        self.cargo_update_in(&ed, only, how, cx);
    }

    fn cargo_update_in(&mut self, ed: &Entity<EditorView>, only: Option<Range<usize>>, how: Update, cx: &mut Context<Self>) {
        let ed = ed.clone();
        let path = ed.read(cx).path.clone();
        let Some(lookups) = self.lsp.cargo.lookups.get(&path).cloned() else {
            if cx.global::<Settings>().rust_dependency_lookups {
                notify(self, t("The dependency versions are not loaded yet. Try again in a moment."), cx);
            } else {
                // Asking for an update is asking for the lookup: one fetch for this file, then the update.
                self.lsp.cargo.pending.insert(path.clone(), (only, how));
                self.cargo_look_up_now(&ed, cx);
            }
            return;
        };
        let (text, version, _) = ed.read(cx).cargo_snapshot();
        let Ok(manifest) = manifest::parse(&text) else {
            notify(self, t("Cargo.toml is not valid TOML yet."), cx);
            return;
        };
        let target = only.as_ref().and_then(|span| manifest.deps.iter().find(|d| &d.key_span == span));
        let edits = edit::update_edits(&manifest, &lookups, how, target);
        if edits.is_empty() {
            notify(self, t("Nothing to update: the requirements are current."), cx);
            return;
        }
        let parts: Vec<(Range<usize>, String)> = edits.iter().map(|e| (e.range.clone(), e.text.clone())).collect();
        let applied = ed.update(cx, |e, cx| e.apply_text_edits(version, &parts, cx));
        notify(
            self,
            if applied {
                tn("Updated {} dependency", "Updated {} dependencies", edits.len(), &[])
            } else {
                t("The file changed meanwhile. Try again.").to_string()
            },
            cx,
        );
    }

    pub(in crate::workbench) fn cargo_update_dependency(&mut self, _: &UpdateDependency, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.cargo_editor(cx) else { return };
        let (text, _, cursor) = ed.read(cx).cargo_snapshot();
        let key = manifest::parse(&text).ok().and_then(|m| m.dep_at(cursor).map(|d| d.key_span.clone()));
        match key {
            Some(span) => self.cargo_update(Some(span), Update::Latest, cx),
            None => notify(self, t("Put the cursor on a dependency first"), cx),
        }
    }

    pub(in crate::workbench) fn cargo_update_all_compatible(&mut self, _: &UpdateAllCompatible, _: &mut Window, cx: &mut Context<Self>) {
        self.cargo_update(None, Update::Compatible, cx);
    }

    pub(in crate::workbench) fn cargo_update_all_latest(&mut self, _: &UpdateAllLatest, _: &mut Window, cx: &mut Context<Self>) {
        self.cargo_update(None, Update::Latest, cx);
    }

    // --- Remove ---

    pub(in crate::workbench) fn cargo_remove_dependency(&mut self, _: &RemoveDependency, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.cargo_editor(cx) else { return };
        let (text, _, cursor) = ed.read(cx).cargo_snapshot();
        let key = manifest::parse(&text).ok().and_then(|m| m.dep_at(cursor).map(|d| d.key_span.clone()));
        match key {
            Some(span) => self.cargo_remove(span, window, cx),
            None => notify(self, t("Put the cursor on a dependency first"), cx),
        }
    }

    fn cargo_remove(&mut self, key_span: Range<usize>, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.cargo_editor(cx) else { return };
        let (text, version, _) = ed.read(cx).cargo_snapshot();
        let Some(dep) = manifest::parse(&text).ok().and_then(|m| m.deps.into_iter().find(|d| d.key_span == key_span)) else {
            notify(self, t("That dependency is gone."), cx);
            return;
        };
        let (path, dirty) = (ed.read(cx).path.clone(), ed.read(cx).is_dirty());
        // A key that is no crate name (a manifest can say anything) never reaches the command line: the text edit does it.
        let args = remove_args(&dep);
        let use_cargo = !dirty && !dep.in_workspace_table && args.is_some() && self.cargo_may_run(&path, cx);
        let host = ed.read(cx).host_arc();
        let task = cx.background_executor().spawn(async move {
            let mut why = None;
            if let (true, Some(args)) = (use_cargo, args) {
                match run_cargo(host.as_ref(), &path, &args) {
                    CargoRun::Done => return Ok(None),
                    CargoRun::Unavailable(m) | CargoRun::Failed(m) => why = Some(first_line(&m)),
                }
            }
            edit::remove_dependency(&text, &dep).map(|new| Some((new, text, dep.key.clone(), why)))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(None) => this.cargo_finished(&ed, version, t("Removed with cargo remove").to_string(), cx),
                Ok(Some((new, old, key, why))) => {
                    let applied = ed.update(cx, |e, cx| e.apply_text_edits(version, &diff(&old, &new), cx));
                    let done = match why {
                        Some(why) => tf("Removed “{0}” by editing the file (cargo remove: {1}). Save the file to keep it.", &[&key, &why]),
                        None => tf("Removed “{}”. Save the file to keep it.", &[&key]),
                    };
                    notify(this, if applied { done } else { t("The file changed meanwhile. Try again.").to_string() }, cx);
                }
                Err(e) => notify(this, explain(&e), cx),
            })
            .ok();
        })
        .detach();
    }

    // --- Add ---

    pub(in crate::workbench) fn cargo_add_dependency(&mut self, _: &AddDependency, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.cargo_editor(cx) else { return };
        let path = ed.read(cx).path.clone();
        let status = t("Type a crate name to search crates.io");
        let picker =
            cx.new(|cx| Picker::new(vec![PickSource::Remote { prefix: "", items: Vec::new(), status: status.into() }], t("Add dependency"), "", None, cx));
        self.show_overlay(picker, OverlayKind::CargoAdd { path, hits: Vec::new() }, window, cx);
    }

    /// The picker's query changed: search crates.io after a pause (a newer query drops the old one).
    pub(in crate::workbench) fn cargo_search(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(picker) = self.overlay.as_ref().map(|o| o.picker.downgrade()) else { return };
        let path = match &self.overlay {
            Some(Overlay { kind: OverlayKind::CargoAdd { path, .. }, .. }) => path.clone(),
            _ => return,
        };
        // The search is the user's own request, so it needs no setting: the registry comes from the pool, not from the binding.
        let Some(ed) = self.all_editors().into_iter().find(|e| e.read(cx).path == path) else { return };
        let (remote, pool) = (ed.read(cx).host_arc().is_remote(), self.cargo_pool());
        let trusted = root_is_trusted(self.root_of(&path, cx).as_deref(), |r| self.is_trusted_root(r, cx));
        let cargo_home = (!cfg!(test)).then(|| config::cargo_home(std::env::var_os("CARGO_HOME"), dirs::home_dir())).flatten();
        self.lsp.cargo.search = None;
        let typed = query.trim().to_string();
        if typed.is_empty() {
            picker.update(cx, |p, cx| p.set_remote(0, Vec::new(), t("Type a crate name to search crates.io"), cx)).ok();
            return;
        }
        self.lsp.cargo.search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(350)).await;
            let q = typed.clone();
            let found = cx
                .background_executor()
                .spawn(async move {
                    match resolve_endpoints(remote, &path, cargo_home.as_deref(), trusted) {
                        Some(endpoints) => pool.registry(&endpoints).search(&q),
                        None => Err(crate::cargo_project::deps::registry::Failure::Unavailable),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                let (hits, status) = match found {
                    Ok(hits) if hits.is_empty() => (Vec::new(), tf("No crates matching “{}”", &[&typed])),
                    Ok(hits) => (hits, String::new()),
                    Err(crate::cargo_project::deps::registry::Failure::Offline) => (Vec::new(), t("crates.io could not be reached (offline?)").to_string()),
                    Err(crate::cargo_project::deps::registry::Failure::Unavailable) => {
                        (Vec::new(), t("Search is not available for this project's registry").to_string())
                    }
                    Err(crate::cargo_project::deps::registry::Failure::Other(m)) => (Vec::new(), tf("Search failed: {}", &[&m])),
                };
                // The name as typed is always offered: it works offline, and for a crate the search ranks low.
                let mut rows: Vec<SearchHit> = Vec::new();
                if valid_crate_name(&typed) && !hits.iter().any(|h| h.name == typed) {
                    rows.push(SearchHit { name: typed.clone(), version: String::new(), description: t("Use the name as typed").to_string() });
                }
                rows.extend(crate::cargo_project::deps::items::ranked(&hits, &typed).into_iter().cloned());
                let items = rows
                    .iter()
                    .map(|h| PickItem {
                        label: h.name.clone().into(),
                        detail: Some(if h.version.is_empty() { h.description.clone() } else { format!("{} · {}", h.version, h.description) }.into()),
                        keys: None,
                    })
                    .collect();
                if let Some(Overlay { kind: OverlayKind::CargoAdd { hits: shown, .. }, .. }) = &mut this.overlay {
                    *shown = rows;
                }
                picker.update(cx, |p, cx| p.set_remote(0, items, status, cx)).ok();
            })
            .ok();
        }));
    }

    /// The user chose a crate in the Add Dependency picker.
    pub(in crate::workbench) fn cargo_add_chosen(&mut self, path: String, hit: SearchHit, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(window, cx);
        let Some(ed) = self.all_editors().into_iter().find(|e| e.read(cx).path == path) else { return };
        let (text, version, cursor) = ed.read(cx).cargo_snapshot();
        let kind = context::kind_at(&text, cursor).unwrap_or(Kind::Normal);
        let name = hit.name.clone();
        // The name came from a search answer or from the keyboard; the version from a search answer.
        let Some(args) = add_args(&hit, kind) else {
            notify(self, tf("“{}” is not a crate name", &[&name]), cx);
            return;
        };
        let hit = SearchHit { version: version_of(&hit), ..hit };
        let dirty = ed.read(cx).is_dirty();
        let use_cargo = !dirty && !edit::is_virtual_root(&text) && self.cargo_may_run(&path, cx);
        let host = ed.read(cx).host_arc();
        let task = cx.background_executor().spawn(async move {
            let mut why = None;
            if use_cargo {
                match run_cargo(host.as_ref(), &path, &args) {
                    CargoRun::Done => return Ok(None),
                    CargoRun::Unavailable(m) | CargoRun::Failed(m) => why = Some(first_line(&m)),
                }
            }
            if hit.version.is_empty() {
                return Err(EditError::Missing(name));
            }
            edit::add_dependency(&text, &edit::add_target(&text, kind), &hit.name, &hit.version).map(|new| Some((new, text, hit.name, why)))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(None) => this.cargo_finished(&ed, version, t("Added with cargo add").to_string(), cx),
                Ok(Some((new, old, name, why))) => {
                    let applied = ed.update(cx, |e, cx| e.apply_text_edits(version, &diff(&old, &new), cx));
                    let done = match why {
                        Some(why) => tf("Added “{0}” by editing the file (cargo add: {1}). Save the file to keep it.", &[&name, &why]),
                        None => tf("Added “{}”. Save the file to keep it.", &[&name]),
                    };
                    notify(this, if applied { done } else { t("The file changed meanwhile. Try again.").to_string() }, cx);
                }
                Err(EditError::Missing(name)) => {
                    notify(this, tf("Could not add “{}”: cargo is not available and its version is not known offline.", &[&name]), cx)
                }
                Err(e) => notify(this, explain(&e), cx),
            })
            .ok();
        })
        .detach();
    }

    /// `cargo` may run in the file's folder: not in an untrusted folder or outside every folder (a project's
    /// `.cargo/config.toml` can run programs), and never in tests.
    fn cargo_may_run(&self, path: &str, cx: &App) -> bool {
        root_is_trusted(self.root_of(path, cx).as_deref(), |r| self.is_trusted_root(r, cx)) && !cfg!(test)
    }

    /// Cargo finished changing the file on disk: show it, unless the user typed meanwhile (their edits are not replaced; saving
    /// then shows the usual conflict).
    pub(in crate::workbench) fn cargo_finished(&mut self, ed: &Entity<EditorView>, version: u64, done: String, cx: &mut Context<Self>) {
        let (dirty, current) = {
            let e = ed.read(cx);
            (e.is_dirty(), e.version())
        };
        if dirty || current != version {
            notify(self, t("Cargo changed Cargo.toml on disk while you were editing it. Your edits were kept; saving asks which version to keep."), cx);
            return;
        }
        ed.update(cx, |e, cx| e.reload(cx));
        notify(self, done, cx);
    }

    #[cfg(test)]
    pub fn cargo_finished_for_test(&mut self, ed: &Entity<EditorView>, version: u64, cx: &mut Context<Self>) {
        self.cargo_finished(ed, version, "done".into(), cx);
    }

    #[cfg(test)]
    pub fn set_cargo_fetch_for_test(&mut self, fetch: Arc<dyn Fetch>) {
        self.lsp.cargo.pool = Some(Pool::new(fetch, None));
    }

    #[cfg(test)]
    pub fn cargo_hints_for_test(&self, path: &str, cx: &App) -> Vec<(usize, String)> {
        self.all_editors()
            .into_iter()
            .find(|e| e.read(cx).path == path)
            .map(|e| e.read(cx).cargo_hints().iter().map(|h| (h.line, h.text.clone())).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "workbench_cargo_tests.rs"]
mod tests;

fn notify(wb: &mut Workbench, text: impl Into<String>, cx: &mut Context<Workbench>) {
    wb.notice = Some(Notice::Info(SharedString::from(text.into())));
    cx.notify();
}

fn explain(e: &EditError) -> String {
    match e {
        EditError::Invalid(m) => tf("Cargo.toml is not valid TOML: {}", &[m]),
        EditError::Exists(n) => tf("“{}” is already a dependency here", &[n]),
        EditError::Missing(n) => tf("“{}” was not found in the file", &[n]),
        EditError::Shape(n) => tf("“{}” in Cargo.toml is not a table", &[n]),
    }
}

/// `cargo add` / `cargo remove` flags for a dependency table. A target is joined to its flag with `=`, so a value that looks
/// like an option stays a value.
fn section_flags(kind: Kind, target: Option<&str>) -> Vec<String> {
    let mut flags = match kind {
        Kind::Normal => Vec::new(),
        Kind::Dev => vec!["--dev".to_string()],
        Kind::Build => vec!["--build".to_string()],
    };
    if let Some(cfg) = target {
        flags.push(format!("--target={cfg}"));
    }
    flags
}

/// The arguments of `cargo remove` for `dep`: flags first, then `--`, then the name. `None` when the key is no crate name.
pub(crate) fn remove_args(dep: &manifest::Dep) -> Option<Vec<String>> {
    valid_crate_name(&dep.key).then(|| {
        let mut args = vec!["remove".to_string()];
        args.extend(section_flags(dep.kind, dep.target.as_deref()));
        args.extend(["--".to_string(), dep.key.clone()]);
        args
    })
}

/// A version from a search answer is used only when it is one.
fn version_of(hit: &SearchHit) -> String {
    semver::Version::parse(&hit.version).map(|v| v.to_string()).unwrap_or_default()
}

/// The arguments of `cargo add`: flags, `--`, then `name` or `name@version`. `None` when the name is no crate name.
pub(crate) fn add_args(hit: &SearchHit, kind: Kind) -> Option<Vec<String>> {
    if !valid_crate_name(&hit.name) {
        return None;
    }
    let version = version_of(hit);
    let spec = if version.is_empty() { hit.name.clone() } else { format!("{}@{version}", hit.name) };
    let mut args = vec!["add".to_string()];
    args.extend(section_flags(kind, None));
    args.extend(["--".to_string(), spec]);
    Some(args)
}

enum CargoRun {
    Done,
    /// Cargo or its `add` / `remove` subcommand is not there.
    Unavailable(String),
    Failed(String),
}

/// `args` with `--manifest-path` right after the subcommand: everything after `--` is a name.
pub(crate) fn cargo_argv(manifest: &str, args: &[String]) -> Vec<String> {
    let mut full = args.to_vec();
    full.splice(1..1, ["--manifest-path".to_string(), manifest.to_string()]);
    full
}

fn run_cargo(host: &dyn Host, manifest: &str, args: &[String]) -> CargoRun {
    let argv = cargo_argv(manifest, args);
    let full: Vec<&str> = argv.iter().map(String::as_str).collect();
    match host.exec("cargo", &full, manifest.rsplit_once('/').map(|(d, _)| d), None) {
        Err(e) => CargoRun::Unavailable(crate::cargo_project::explain_failure(&e.to_string())),
        Ok(out) if out.ok() => CargoRun::Done,
        Ok(out) => {
            let text = format!("{}{}", out.stdout_str(), out.stderr_str());
            if text.contains("no such command") || text.contains("no such subcommand") {
                CargoRun::Unavailable(text)
            } else {
                CargoRun::Failed(crate::cargo_project::explain_failure(&text))
            }
        }
    }
}

fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_string()
}

/// `old` → `new` as byte-range edits the editor applies as one undo step.
fn diff(old: &str, new: &str) -> Vec<(Range<usize>, String)> {
    edit::minimal_edit(old, new).map(|Edit { range, text }| vec![(range, text)]).unwrap_or_default()
}
