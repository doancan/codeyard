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
use crate::cargo_project::deps::fetch::{CurlFetch, Fetch, FetchError, Response};
use crate::cargo_project::deps::hints::{self, Lookups};
use crate::cargo_project::deps::lock::Lock;
use crate::cargo_project::deps::manifest::{self, Kind};
use crate::cargo_project::deps::registry::{Endpoints, Lookup, Registry, SearchHit, valid_crate_name};
use crate::cargo_project::deps::version::Update;
use crate::cargo_project::deps::{context, is_manifest};
use crate::editor::CargoBinding;
use crate::i18n::{t, tf, tn};

gpui::actions!(cargo_deps, [AddDependency, RemoveDependency, UpdateDependency, UpdateAllCompatible, UpdateAllLatest]);

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
struct Unreachable;

impl Fetch for Unreachable {
    fn get(&self, _: &str, _: Option<&str>) -> Result<Response, FetchError> {
        Err(FetchError::Offline)
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
}

impl CargoState {
    pub(crate) fn problems_of(&self, path: &str) -> Vec<crate::lsp::Diagnostic> {
        self.problems.get(path).cloned().unwrap_or_default()
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

/// Parse, look up and build the hints. `None`: the text is not valid TOML right now, so what is shown stays.
fn analyse(job: Job) -> Option<Analysis> {
    let manifest = manifest::parse(&job.text).ok()?;
    let local_dir = (!job.host.is_remote()).then(|| job.path.rsplit_once('/').map(|(d, _)| PathBuf::from(d))).flatten();
    let endpoints = match &local_dir {
        Some(dir) => match config::resolve(&config::read_chain(dir, job.cargo_home.as_deref(), job.trusted)) {
            ConfigSource::Endpoints(e) => e,
            ConfigSource::Unsupported => {
                return Some(Analysis { binding: None, lookups: Lookups::new(), manifest });
            }
        },
        None => Endpoints::default(),
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
                if cfg!(test) {
                    Pool::new(Arc::new(Unreachable), None)
                } else {
                    Pool::new(Arc::new(CurlFetch::new()), dirs::cache_dir().map(|d| d.join(host::ssh::APP_DIR).join("cargo-deps")))
                }
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
        if !cx.global::<Settings>().rust_dependency_lookups {
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
            return;
        }
        let root = self.root_of(&path, cx);
        let (host, pool) = (ed.read(cx).host_arc(), self.cargo_pool());
        let trusted = root.as_deref().is_none_or(|r| self.is_trusted_root(r, cx));
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
        cx.notify();
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
        if !cx.global::<Settings>().rust_dependency_lookups {
            notify(self, t("Dependency lookups are off. Turn on rust.cargo.dependencyLookups in Settings."), cx);
            return None;
        }
        Some(ed)
    }

    /// Update one dependency (`only` is its key's span) or all of them from the lookups made for this file.
    fn cargo_update(&mut self, only: Option<Range<usize>>, how: Update, cx: &mut Context<Self>) {
        let Some(ed) = self.cargo_editor(cx) else { return };
        let path = ed.read(cx).path.clone();
        let Some(lookups) = self.lsp.cargo.lookups.get(&path).cloned() else {
            notify(self, t("The dependency versions are not loaded yet. Try again in a moment."), cx);
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
        let mut args = vec!["remove".to_string(), dep.key.clone()];
        args.extend(section_flags(dep.kind, dep.target.as_deref()));
        let use_cargo = !dirty && !dep.in_workspace_table && self.cargo_may_run(&path, cx);
        let host = ed.read(cx).host_arc();
        let task = cx.background_executor().spawn(async move {
            let mut why = None;
            if use_cargo {
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
                Ok(None) => {
                    ed.update(cx, |e, cx| e.reload(cx));
                    notify(this, t("Removed with cargo remove"), cx);
                }
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
        let registry = self.active_editor().filter(|e| e.read(cx).path == path).and_then(|e| e.read(cx).cargo_registry());
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
                    match registry {
                        Some(r) => r.search(&q),
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
                rows.extend(hits);
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
        let dirty = ed.read(cx).is_dirty();
        let spec = if hit.version.is_empty() { name.clone() } else { format!("{name}@{}", hit.version) };
        let mut args = vec!["add".to_string(), spec];
        args.extend(section_flags(kind, None));
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
                Ok(None) => {
                    ed.update(cx, |e, cx| e.reload(cx));
                    notify(this, t("Added with cargo add"), cx);
                }
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

    /// `cargo` may run in the file's folder: not in an untrusted folder (a project's `.cargo/config.toml` can run programs).
    fn cargo_may_run(&self, path: &str, cx: &App) -> bool {
        self.root_of(path, cx).is_none_or(|r| self.is_trusted_root(&r, cx)) && !cfg!(test)
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

/// `cargo add` / `cargo remove` flags for a dependency table.
fn section_flags(kind: Kind, target: Option<&str>) -> Vec<String> {
    let mut flags = match kind {
        Kind::Normal => Vec::new(),
        Kind::Dev => vec!["--dev".to_string()],
        Kind::Build => vec!["--build".to_string()],
    };
    if let Some(cfg) = target {
        flags.extend(["--target".to_string(), cfg.to_string()]);
    }
    flags
}

enum CargoRun {
    Done,
    /// Cargo or its `add` / `remove` subcommand is not there.
    Unavailable(String),
    Failed(String),
}

fn run_cargo(host: &dyn Host, manifest: &str, args: &[String]) -> CargoRun {
    let mut full: Vec<&str> = args.iter().map(String::as_str).collect();
    full.extend(["--manifest-path", manifest]);
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
