//! Workbench: the Extensions view. Lists the installed extensions (enabled and turned off), shows what one contributes
//! and may reach, installs a `.cyx` file or a folder after the consent sheet, and turns one off, uninstalls it, rolls it
//! back or checks its files. Every change goes through `ext_core::Store` and then the settings reload, so the
//! contributions follow without a restart (docs/plugin-architecture.md §6.3). Rendering is in `workbench_ext_view.rs`.

use std::sync::Arc;

use ext_core::Source as ExtSource;
use ext_manifest::{ExtId, Manifest, Permissions};
use gpui::{Context, Entity, FocusHandle, Window};

use super::*;
use crate::input::{FieldEvent, TextField};

gpui::actions!(workbench, [ShowExtensions, InstallExtensionFromFile, InstallExtensionFromFolder, InstallExtensionFromRegistry, CancelExtensionConsent]);

/// One installed extension as the view lists it.
#[derive(Clone)]
pub(super) struct ExtEntry {
    pub(super) manifest: Arc<Manifest>,
    pub(super) digest: ext_pkg::ContentDigest,
    pub(super) disabled: bool,
    pub(super) source: ExtSource,
    /// The version a rollback goes back to (still in the store).
    pub(super) previous: Option<String>,
    /// Why it does not run: its version was revoked, or it needs one that was.
    pub(super) stopped: Option<String>,
    /// It gets the registry's pre-releases.
    pub(super) pre_release: bool,
}

/// A package read and checked, waiting for the user's answer on the consent sheet.
pub(super) struct Pending {
    pub(super) manifest: Arc<Manifest>,
    bytes: Vec<u8>,
    pub(super) source: ExtSource,
    /// Reasons to look twice, shown in red above the buttons (a new publisher, a stale registry); Cancel takes the keys.
    pub(super) warnings: Vec<String>,
    /// Native programs: every platform's content digest (for the lock), and the other platforms' packages of a folder.
    targets: std::collections::BTreeMap<String, ext_pkg::ContentDigest>,
    others: Vec<Vec<u8>>,
}

impl Pending {
    fn new(manifest: Manifest, bytes: Vec<u8>, source: ExtSource) -> Pending {
        Pending { manifest: Arc::new(manifest), bytes, source, warnings: Vec::new(), targets: Default::default(), others: Vec::new() }
    }
}

/// State of the Extensions view (kept while the window lives).
#[derive(Default)]
pub(super) struct ExtViewState {
    pub(super) list: Vec<ExtEntry>,
    /// The store or lock could not be read.
    pub(super) error: Option<String>,
    pub(super) filter: Option<Entity<TextField>>,
    /// Id of the extension on the detail page.
    pub(super) selected: Option<String>,
    pub(super) pending: Option<Pending>,
    /// Install and Cancel on the consent sheet, so the safe answer can take the keyboard first.
    pub(super) consent_focus: Option<(FocusHandle, FocusHandle)>,
    /// What had the keyboard when the sheet opened; it gets it back when the sheet is answered.
    pub(super) consent_return: Option<FocusHandle>,
    /// Where the user types the extension's id to install one with native programs (§6.3).
    pub(super) consent_typed: Option<Entity<TextField>>,
    pub(super) tab: super::market_glue::ExtTab,
    pub(super) market: super::market_glue::MarketState,
    pub(super) updates: super::updates_glue::UpdateState,
    /// Running code extensions (workbench_ext_code.rs).
    pub(super) code: super::code_glue::CodeState,
    /// Their accounts, secrets and sign-ins (workbench_ext_accounts.rs).
    pub(super) accounts: super::accounts_glue::AccountsState,
    /// Permission switches and the Inspector (workbench_ext_inspector.rs).
    pub(super) inspector: super::inspector_glue::InspectorState,
    /// Views of code extensions (workbench_ext_views.rs).
    pub(super) views: super::ext_views_glue::ViewsState,
    /// What code extensions show: status items, diagnostics, decorations (workbench_ext_providers.rs).
    pub(super) providers: super::providers_glue::ProviderState,
    /// Source control providers (workbench_ext_scm.rs).
    pub(super) scm: super::ext_scm_glue::ScmProviders,
    /// First clipboard read and first remote process per session ask (workbench_ext_prompts.rs).
    pub(super) prompts: super::prompts_glue::PromptState,
    /// The resident-memory cap of exthosts (workbench_ext_memory.rs).
    pub(super) memory: super::memory_glue::MemoryState,
}

/// The consent sheet's text and every other plain-words line of an extension.
pub(super) fn source_line(s: &ExtSource) -> String {
    match s {
        ExtSource::File { path } => crate::i18n::tf("Installed from the file {}", &[path]),
        ExtSource::Folder { path } => crate::i18n::tf("Installed from the folder {}", &[path]),
        ExtSource::Registry { registry, publisher } => crate::i18n::tf("From the {0} registry, published by {1}", &[registry, publisher]),
    }
}

/// The consent sheet's lines (`Permissions::lines`) in the UI language.
pub(super) fn permission_lines(p: &Permissions) -> Vec<String> {
    use ext_manifest::{Clipboard, Line};
    p.lines()
        .iter()
        .map(|l| match l {
            Line::ReadAll => crate::i18n::t("Read all files in the workspace").to_string(),
            Line::Read(g) => crate::i18n::tf("Read {}", &[g]),
            Line::ReadSecrets => crate::i18n::t("Read files that can hold passwords and keys").to_string(),
            Line::ChangeAll => crate::i18n::t("Change all files in the workspace").to_string(),
            Line::Change(g) => crate::i18n::tf("Change {}", &[g]),
            Line::ConnectAny => crate::i18n::t("Connect to any website").to_string(),
            Line::Connect(h) => crate::i18n::tf("Connect to {}", &[h]),
            Line::Account(a) => crate::i18n::tf("Act as your {} account", &[a]),
            Line::Secrets => crate::i18n::t("Store secrets in your keychain").to_string(),
            Line::Run(r) => crate::i18n::tf("Run {} with any arguments", &[r]),
            Line::Clipboard(Clipboard::Read) => crate::i18n::t("Read the clipboard").to_string(),
            Line::Clipboard(Clipboard::Write) => crate::i18n::t("Write to the clipboard").to_string(),
            Line::Clipboard(_) => crate::i18n::t("Read and write the clipboard").to_string(),
            Line::Terminal => crate::i18n::t("Type into terminals").to_string(),
            Line::Commands(c) => crate::i18n::tf("Run Codeyard commands: {}", &[c]),
        })
        .collect()
}

/// The red line on the sheet and the detail page, for a High-risk permission set.
pub(super) fn high_risk_line(m: &Manifest) -> Option<String> {
    let r = m.risk();
    (r.level == ext_manifest::Level::High).then(|| {
        if r.runs_code {
            crate::i18n::t("High risk: this extension can run programs as you.").to_string()
        } else {
            crate::i18n::t("High risk: this extension can read private data and send it elsewhere.").to_string()
        }
    })
}

/// One line per contribution, in plain words.
pub(super) fn contribution_lines(m: &Manifest) -> Vec<String> {
    let c = &m.contributions;
    let mut out = Vec::new();
    out.extend(c.themes.iter().map(|t| crate::i18n::tf("Color theme: {}", &[&t.label])));
    out.extend(c.snippets.iter().map(|s| crate::i18n::tf("Snippets for {}", &[&s.language])));
    out.extend(c.keymap_presets.iter().map(|k| crate::i18n::tf("Keymap preset: {}", &[&k.label])));
    out.extend(c.keybindings.iter().map(|b| crate::i18n::tf("Key binding: {0} runs {1}", &[&b.key, &b.command])));
    for l in &c.languages {
        let files: Vec<String> = l.filenames.iter().cloned().chain(l.extensions.iter().map(|e| format!("*.{e}"))).collect();
        out.push(crate::i18n::tf("Language: {0} ({1})", &[&l.name.as_deref().unwrap_or(&l.id), &files.join(", ")]));
    }
    out.extend(c.json_schemas.iter().map(|s| crate::i18n::tf("JSON schema for {}", &[&s.file_match.join(", ")])));
    out.extend(c.yaml_schemas.iter().map(|s| crate::i18n::tf("YAML schema for {}", &[&s.file_match.join(", ")])));
    out.extend(c.settings.iter().map(|s| crate::i18n::tf("Setting: {}", &[&format!("ext.{}.{}", m.id, s.key)])));
    out.extend(c.language_servers.iter().map(|s| crate::i18n::tf("Language server: {0} for {1}", &[&s.command, &s.languages.join(", ")])));
    out.extend(c.commands.iter().map(|x| crate::i18n::tf("Command: {}", &[&x.title])));
    out.extend(c.views.iter().map(|v| crate::i18n::tf("View: {}", &[&v.title])));
    out.extend(c.scm.iter().map(|s| crate::i18n::tf("Source control: {0} (folders with {1})", &[&s.label, &s.markers.join(", ")])));
    out
}

/// Read and check a package file, or package a folder, off the main thread. Errors from `ext-pkg` and `ext-manifest` are
/// written for authors and shown as they are.
fn read_package(source: &ExtSource) -> Result<Pending, String> {
    let mut packages = match source {
        ExtSource::File { path } => vec![std::fs::read(path).map_err(|e| crate::i18n::tf("Cannot read {0}: {1}", &[path, &e]))?],
        ExtSource::Folder { path } => folder_packages(std::path::Path::new(path))?,
        ExtSource::Registry { .. } => return Err("a registry package is downloaded by review_registry_extension".into()),
    };
    let bytes = packages.remove(0);
    let pkg = ext_pkg::read(&bytes).map_err(|e| e.to_string())?;
    if pkg.manifest.code.is_some() {
        crate::ext_runtime::code_runs_here(&pkg.manifest.id)?;
    }
    let mut pending = Pending::new(pkg.manifest, bytes, source.clone());
    for other in packages {
        let o = ext_pkg::read(&other).map_err(|e| e.to_string())?;
        pending.targets.extend(o.target.map(|t| (t, o.digest)));
        pending.others.push(other);
    }
    pending.targets.extend(pkg.target.map(|t| (t, pkg.digest)));
    Ok(pending)
}

/// A folder packaged as install does it: one package, or one per platform in `native/` with this machine's first.
fn folder_packages(dir: &std::path::Path) -> Result<Vec<Vec<u8>>, String> {
    let mut targets = ext_pkg::native_targets(dir);
    let manifest = std::fs::read_to_string(dir.join(ext_manifest::FILE_NAME)).ok().and_then(|t| ext_manifest::parse(&t).ok());
    if targets.is_empty() || !manifest.is_some_and(|m| m.has_native_programs()) {
        let files = ext_pkg::collect_dir(dir).map_err(|e| e.to_string())?;
        return Ok(vec![ext_pkg::write(files).map_err(|e| e.to_string())?.0]);
    }
    if let Some(i) = targets.iter().position(|t| Some(t.as_str()) == ext_manifest::this_target()) {
        targets.swap(0, i);
    }
    targets
        .iter()
        .map(|t| {
            let files = ext_pkg::collect_dir_for(dir, Some(t)).map_err(|e| e.to_string())?;
            Ok(ext_pkg::write(files).map_err(|e| e.to_string())?.0)
        })
        .collect()
}

/// A package found in a registry and checked, as the consent sheet shows it.
fn registry_pending(found: crate::ext_registry::Found) -> Result<Pending, String> {
    let m = found.download.manifest;
    if m.code.is_some() {
        crate::ext_runtime::code_runs_here(&m.id)?;
    }
    let mut warnings = Vec::new();
    if let ext_core::publisher::Pin::Changed { was } = &found.pin {
        warnings.push(crate::i18n::tf("Another publisher now: {0}. The version you have came from {1}.", &[&found.download.signed.publisher, was]));
    }
    if found.stale {
        warnings.push(crate::i18n::tf(
            "The {} registry's list is more than 3 days old, so a version withdrawn since may still show. Install only if you expected this.",
            &[&found.registry.name],
        ));
    }
    let source = ExtSource::Registry { registry: found.registry.name, publisher: found.download.signed.publisher };
    let mut pending = Pending::new(m, found.download.bytes, source);
    pending.warnings = warnings;
    pending.targets = found.download.targets;
    Ok(pending)
}

impl Workbench {
    pub(super) fn show_extensions(&mut self, _: &ShowExtensions, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Extensions);
        self.extensions_opened(window, cx);
        // The list on screen keeps its tab; its search field takes the keyboard.
        self.set_extensions_tab(self.cfg.exts.tab, window, cx);
    }

    /// The view was opened: the filter field exists and the list is read again.
    pub(super) fn extensions_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cfg.exts.filter.is_none() {
            let field = cx.new(|cx| TextField::new(crate::i18n::tr("Search installed extensions"), cx));
            self._subs.push(cx.subscribe_in(&field, window, |_, _, ev: &FieldEvent, _, cx| {
                if matches!(ev, FieldEvent::Changed) {
                    cx.notify();
                }
            }));
            self.cfg.exts.filter = Some(field);
        }
        self.refresh_extension_list();
    }

    /// Settings were reloaded (which includes a change of the lock file by another window): the list follows while
    /// it is on screen.
    pub(super) fn sync_extensions_view(&mut self) {
        if self.view_shown(Panel::Extensions) || matches!(self.tabs.page, Some(Page::Extension)) {
            self.refresh_extension_list();
        }
    }

    pub(super) fn refresh_extension_list(&mut self) {
        let s = &mut self.cfg.exts;
        s.list.clear();
        s.error = None;
        let Some(dir) = self.cfg.dir.as_deref().filter(|_| !crate::ext::turned_off()) else { return };
        let store = crate::ext::store(dir);
        let loaded = match store.load() {
            Ok(l) => l,
            Err(e) => {
                s.error = Some(e.to_string());
                return;
            }
        };
        let locked = store.locked().unwrap_or_default();
        let version_in_store = |d: &ext_pkg::ContentDigest| {
            let text = std::fs::read_to_string(store.store_dir(d).join(ext_manifest::FILE_NAME)).ok()?;
            ext_manifest::parse(&text).ok().map(|m| m.version.to_string())
        };
        let stopped = ext_core::stopped(&loaded.extensions, &store.revoked());
        s.list = loaded
            .extensions
            .into_iter()
            .map(|e| ExtEntry {
                previous: locked.get(&e.manifest.id).and_then(|l| l.previous.as_ref()).and_then(version_in_store),
                stopped: stopped.get(&e.manifest.id).map(|why| crate::ext::stopped_line(&e.manifest.id, why)),
                pre_release: locked.get(&e.manifest.id).is_some_and(|l| l.pre_release),
                manifest: Arc::new(e.manifest),
                digest: e.digest,
                disabled: e.disabled,
                source: e.source,
            })
            .collect();
    }

    /// Indexes into the list that match the search field (name, id or description, ignoring case).
    pub(super) fn visible_extensions(&self, cx: &App) -> Vec<usize> {
        let s = &self.cfg.exts;
        let q = s.filter.as_ref().map(|f| f.read(cx).text().trim().to_lowercase()).unwrap_or_default();
        (0..s.list.len())
            .filter(|&i| {
                let m = &s.list[i].manifest;
                q.is_empty()
                    || m.name.to_lowercase().contains(&q)
                    || m.id.as_str().contains(&q)
                    || m.description.as_deref().is_some_and(|d| d.to_lowercase().contains(&q))
            })
            .collect()
    }

    pub(super) fn selected_extension(&self) -> Option<&ExtEntry> {
        let id = self.cfg.exts.selected.as_deref()?;
        self.cfg.exts.list.iter().find(|e| e.manifest.id.as_str() == id)
    }

    /// Show one extension's page (a pending consent sheet is dropped).
    pub(super) fn select_extension(&mut self, id: &str, cx: &mut Context<Self>) {
        self.cfg.exts.pending = None;
        self.cfg.exts.market.page = None;
        self.cfg.exts.selected = Some(id.to_string());
        self.refresh_extension_accounts(id, cx);
        self.open_extension_page(cx);
    }

    pub(super) fn open_extension_page(&mut self, cx: &mut Context<Self>) {
        self.tabs.page = Some(Page::Extension);
        self.tabs._page_sub = None;
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        cx.notify();
    }

    /// The page's tab title: the extension on it.
    pub(super) fn extension_page_title(&self) -> String {
        if let Some(p) = &self.cfg.exts.pending {
            return crate::i18n::tf("Install {}", &[&p.manifest.name]);
        }
        if let Some((_, e)) = self.cfg.exts.market.page_entry() {
            return e.name.clone();
        }
        match self.selected_extension() {
            Some(e) => e.manifest.name.clone(),
            None => crate::i18n::t("Extensions").to_string(),
        }
    }

    /// Store changes refuse to run with `--no-extensions` or without a settings folder; the reason is shown.
    fn extension_store(&mut self, cx: &mut Context<Self>) -> Option<ext_core::Store> {
        if crate::ext::turned_off() {
            self.notice = Some(Notice::Error(crate::i18n::t("Extensions are turned off for this session (--no-extensions).").into()));
            cx.notify();
            return None;
        }
        self.cfg.dir.as_deref().map(crate::ext::store)
    }

    pub(super) fn install_extension_from_file(&mut self, _: &InstallExtensionFromFile, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_extension_source(true, window, cx);
    }

    pub(super) fn install_extension_from_folder(&mut self, _: &InstallExtensionFromFolder, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_extension_source(false, window, cx);
    }

    pub(super) fn install_extension_from_registry(&mut self, _: &InstallExtensionFromRegistry, window: &mut Window, cx: &mut Context<Self>) {
        if self.extension_store(cx).is_none() {
            return;
        }
        self.show_input(
            InputPurpose::InstallFromRegistry,
            crate::i18n::t("Install"),
            crate::i18n::t("Extension id, like publisher.name or publisher.name@1.2.0"),
            "",
            window,
            cx,
        );
    }

    /// Find, download and check the package in the background (on this computer, also in an SSH window), then the
    /// consent sheet, which also names the publisher the registry signed it for. `only` limits the search to one
    /// registry (Install on a Market page).
    pub(super) fn review_registry_extension(&mut self, spec: String, only: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.cfg.dir.clone() else { return };
        let mut registries = crate::ext_registry::registries(cx.global::<Settings>());
        if let Some(only) = only {
            registries.retain(|r| r.name == only);
        }
        let spec = spec.trim().to_string();
        self.notice = Some(Notice::Info(crate::i18n::tf("Looking for {} in your registries…", &[&spec]).into()));
        cx.notify();
        // Native programs run on the workspace's host: its platform's package is the one to install.
        let target = crate::ext_programs::host_target(self.remote.host.as_ref()).or(ext_manifest::this_target()).unwrap_or("unknown");
        let task = cx
            .background_executor()
            .spawn(async move { crate::ext_registry::find(&dir, &registries, &spec, target, crate::ext_registry::now()).and_then(registry_pending) });
        cx.spawn_in(window, async move |this, cx| {
            let found = task.await;
            this.update_in(cx, |this, window, cx| {
                this.notice = None;
                match found {
                    Ok(p) => this.show_consent(p, window, cx),
                    Err(e) => this.notice = Some(Notice::Error(crate::i18n::tf("Cannot install the extension: {}", &[&e]).into())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The system file dialog, then the consent sheet. Packages are installed on this machine, also in an SSH window.
    fn pick_extension_source(&mut self, file: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.extension_store(cx).is_none() {
            return;
        }
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: file,
            directories: !file,
            multiple: false,
            prompt: Some(crate::i18n::t("Install").to_string().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let path = path.to_string_lossy().into_owned();
                let source = if file { ExtSource::File { path } } else { ExtSource::Folder { path } };
                this.update_in(cx, |this, window, cx| this.review_extension(source, window, cx)).ok();
            }
        })
        .detach();
    }

    /// Read the package in the background, then show what it contributes and may reach before anything is installed.
    pub(super) fn review_extension(&mut self, source: ExtSource, window: &mut Window, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move { read_package(&source) });
        cx.spawn_in(window, async move |this, cx| {
            let read = task.await;
            this.update_in(cx, |this, window, cx| match read {
                Ok(p) => this.show_consent(p, window, cx),
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Cannot install the extension: {}", &[&e]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn show_consent(&mut self, pending: Pending, window: &mut Window, cx: &mut Context<Self>) {
        let high = high_risk_line(&pending.manifest).is_some() || !pending.warnings.is_empty();
        let (install, cancel) = self.cfg.exts.consent_focus.get_or_insert_with(|| (cx.focus_handle().tab_stop(true), cx.focus_handle().tab_stop(true))).clone();
        let typed = self.cfg.exts.consent_typed.get_or_insert_with(|| cx.new(|cx| TextField::new("", cx))).clone();
        let typed_focus = typed.focus_handle(cx);
        // A sheet that replaces another keeps the first one's place to return to.
        if self.cfg.exts.pending.is_none() {
            self.cfg.exts.consent_return = window.focused(cx).filter(|f| *f != install && *f != cancel && *f != typed_focus);
        }
        let native = pending.manifest.has_native_programs();
        typed.update(cx, |f, cx| f.set_text("", cx));
        self.cfg.exts.pending = Some(pending);
        self.open_extension_page(cx);
        // Install is never the default answer for a High-risk set: Enter on the sheet cancels. Native programs need the
        // id typed first, so the field takes the keyboard.
        window.focus(if native {
            &typed_focus
        } else if high {
            &cancel
        } else {
            &install
        });
    }

    /// An extension with native programs installs only once its id is typed on the sheet (§6.3).
    pub(super) fn consent_typed_ok(&self, cx: &App) -> bool {
        let Some(p) = &self.cfg.exts.pending else { return false };
        !p.manifest.has_native_programs() || self.cfg.exts.consent_typed.as_ref().is_some_and(|f| f.read(cx).text().trim() == p.manifest.id.as_str())
    }

    pub(super) fn cancel_consent_key(&mut self, _: &CancelExtensionConsent, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_consent(window, cx);
    }

    pub(super) fn cancel_consent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cfg.exts.pending = None;
        if self.cfg.exts.selected.is_none() && self.cfg.exts.market.page.is_none() {
            self.close_page(cx);
        }
        self.return_consent_focus(window);
        cx.notify();
    }

    /// The keyboard goes back to what had it before the sheet (Escape, Cancel and Install alike).
    fn return_consent_focus(&mut self, window: &mut Window) {
        if let Some(f) = self.cfg.exts.consent_return.take() {
            window.focus(&f);
        }
    }

    /// "Install" on the consent sheet.
    pub(super) fn confirm_consent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.extension_store(cx) else { return };
        if !self.consent_typed_ok(cx) {
            if let Some(p) = &self.cfg.exts.pending {
                self.notice = Some(Notice::Error(crate::i18n::tf("Type {} to install it.", &[&p.manifest.id]).into()));
            }
            if let Some(f) = &self.cfg.exts.consent_typed {
                window.focus(&f.focus_handle(cx));
            }
            cx.notify();
            return;
        }
        let Some(p) = self.cfg.exts.pending.take() else { return };
        self.return_consent_focus(window);
        let id = p.manifest.id.clone();
        let installed = store
            .install_with_targets(&p.bytes, p.source, p.targets)
            .and_then(|i| p.others.iter().try_for_each(|o| store.add_target(&id, o).map(drop)).map(|()| i));
        match installed {
            Ok(i) => {
                self.cfg.exts.selected = Some(i.manifest.id.to_string());
                self.cfg.exts.market.page = None;
                let done = crate::i18n::tf("Installed {0} {1}.", &[&i.manifest.name, &i.manifest.version]);
                self.after_extension_change(done, window, cx);
            }
            Err(e) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("Cannot install {0}: {1}", &[&p.manifest.name, &e]).into()));
                cx.notify();
            }
        }
    }

    /// Every contribution reader follows the store through the settings reload; a warning from it wins over `done`.
    pub(super) fn after_extension_change(&mut self, done: String, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_config(window, cx);
        self.refresh_extension_list();
        if self.notice.is_none() {
            self.notice = Some(Notice::Info(done.into()));
        }
        cx.notify();
    }

    fn report_store_error(&mut self, e: ext_core::Error, cx: &mut Context<Self>) {
        self.notice = Some(Notice::Error(e.to_string().into()));
        cx.notify();
    }

    pub(super) fn set_extension_enabled(&mut self, id: ExtId, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.extension_store(cx) else { return };
        match store.set_disabled(&id, !enabled) {
            Ok(()) => {
                let done = if enabled { crate::i18n::tf("{} is enabled.", &[&id]) } else { crate::i18n::tf("{} is disabled.", &[&id]) };
                self.after_extension_change(done, window, cx);
            }
            Err(e) => self.report_store_error(e, cx),
        }
    }

    /// Asks first: the extension's own storage goes with it.
    pub(super) fn uninstall_extension(&mut self, id: ExtId, window: &mut Window, cx: &mut Context<Self>) {
        let answer = crate::questions::uninstall_extension(id.as_str()).ask(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Some(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                let Some(store) = this.extension_store(cx) else { return };
                match store.uninstall(&id) {
                    Ok(()) => {
                        if this.cfg.exts.selected.as_deref() == Some(id.as_str()) {
                            this.cfg.exts.selected = None;
                            if matches!(this.tabs.page, Some(Page::Extension)) && this.cfg.exts.pending.is_none() {
                                this.close_page(cx);
                            }
                        }
                        this.after_extension_change(crate::i18n::tf("{} is uninstalled.", &[&id]), window, cx);
                    }
                    Err(e) => this.report_store_error(e, cx),
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn roll_back_extension(&mut self, id: ExtId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.extension_store(cx) else { return };
        match store.rollback(&id) {
            Ok(i) => self.after_extension_change(crate::i18n::tf("{0} is back at version {1}.", &[&id, &i.manifest.version]), window, cx),
            Err(e) => self.report_store_error(e, cx),
        }
    }

    /// Check the installed files against the package's list of hashes.
    pub(super) fn verify_extension(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(store) = self.extension_store(cx) else { return };
        let Some(e) = self.cfg.exts.list.iter().find(|e| e.manifest.id.as_str() == id) else { return };
        self.notice = Some(match store.verify(&e.digest) {
            Ok(bad) if bad.is_empty() => Notice::Info(crate::i18n::tf("Every file of {} matches its package.", &[&id]).into()),
            Ok(bad) => Notice::Error(crate::i18n::tf("Files of {0} that are missing or changed: {1}. Reinstall it.", &[&id, &bad.join(", ")]).into()),
            Err(e) => Notice::Error(e.to_string().into()),
        });
        cx.notify();
    }
}
