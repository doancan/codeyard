//! Installed extensions as the app sees them: the set `ext_core` loads from the store and the lock file, minus the ones
//! the user turned off, or nothing at all with `--no-extensions`. Every contribution reader (themes, snippets, keymaps,
//! languages, schemas, settings, language servers) takes its entries from `current()`, so turning one off or installing
//! one in another window changes all of them on the next settings reload. Extension code runs in `codeyard-exthost`
//! (ext_runtime.rs); its contributions that need code are wired in workbench_ext_code.rs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(not(test))]
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use ext_manifest::Manifest;

/// The lock file's name in the settings folder.
pub const LOCK_FILE: &str = "extensions.lock.jsonc";

/// One extension whose contributions apply.
#[derive(Clone, Debug)]
pub struct Active {
    pub manifest: Arc<Manifest>,
    /// Its unpacked package folder (read-only).
    pub dir: PathBuf,
}

impl Active {
    pub fn id(&self) -> &str {
        self.manifest.id.as_str()
    }

    /// A file of the package, by its manifest path.
    pub fn file(&self, rel: &str) -> PathBuf {
        rel.split('/').fold(self.dir.clone(), |p, part| p.join(part))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Registry {
    /// Enabled extensions, in id order.
    pub active: Vec<Active>,
    /// Lines for the status bar: extensions the lock names but the store cannot serve, a lock written by a newer version.
    pub problems: Vec<String>,
}

#[cfg(not(test))]
static OFF: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
thread_local! {
    static OFF_TL: AtomicBool = const { AtomicBool::new(false) };
}

#[cfg(not(test))]
static CURRENT: RwLock<Option<Arc<Registry>>> = RwLock::new(None);

// Tests run in parallel on their own threads; each keeps its own set (as snippets do).
#[cfg(test)]
thread_local! {
    static CURRENT_TL: std::cell::RefCell<Option<Arc<Registry>>> = const { std::cell::RefCell::new(None) };
}

/// `--extension-dev <folder>` (what `cyx dev` starts): the author's folder, loaded as it is on disk.
#[cfg(not(test))]
static DEV: RwLock<Option<PathBuf>> = RwLock::new(None);
#[cfg(test)]
thread_local! {
    static DEV_TL: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Load `dir` as an extension in development: unpacked, checked like a package, reloaded when a file in it changes.
pub fn set_dev(dir: Option<PathBuf>) {
    #[cfg(not(test))]
    {
        *DEV.write().unwrap_or_else(|e| e.into_inner()) = dir;
    }
    #[cfg(test)]
    DEV_TL.with(|d| *d.borrow_mut() = dir);
}

pub fn dev_dir() -> Option<PathBuf> {
    #[cfg(not(test))]
    {
        DEV.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    #[cfg(test)]
    DEV_TL.with(|d| d.borrow().clone())
}

/// A cheap fingerprint of the dev folder (names, sizes, modification times), polled once a second for hot reload.
pub fn dev_stamp() -> u64 {
    use std::hash::{Hash, Hasher};
    let Some(dir) = dev_dir() else { return 0 };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    fn walk(dir: &Path, h: &mut std::collections::hash_map::DefaultHasher, budget: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if *budget == 0 || ext_pkg::EXCLUDED.iter().any(|x| e.file_name() == *x) {
                continue;
            }
            *budget -= 1;
            let Ok(m) = e.metadata() else { continue };
            e.file_name().hash(h);
            m.len().hash(h);
            m.modified().ok().hash(h);
            if m.is_dir() {
                walk(&e.path(), h, budget);
            }
        }
    }
    walk(&dir, &mut h, &mut 5000);
    h.finish()
}

/// What the settings watcher compares besides the settings, keymap and lock files: the dev folder and the list of
/// revoked versions (another window's update check may stop one).
pub fn watch_stamp(cfg_dir: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dev_stamp().hash(&mut h);
    std::fs::metadata(store(cfg_dir).revoked_path()).and_then(|m| m.modified()).ok().hash(&mut h);
    h.finish()
}

/// The dev folder as an extension, or the author-facing reason it cannot load (the same checks as `cyx check`).
fn load_dev(dir: &Path) -> Result<Active, String> {
    let files = ext_pkg::collect_dir(dir).map_err(|e| e.to_string())?;
    let (bytes, _) = ext_pkg::write(files).map_err(|e| e.to_string())?;
    let pkg = ext_pkg::read(&bytes).map_err(|e| e.to_string())?;
    Ok(Active { manifest: Arc::new(pkg.manifest), dir: dir.to_path_buf() })
}

/// `--no-extensions`: nothing is loaded for the rest of the session.
pub fn turn_off() {
    #[cfg(not(test))]
    OFF.store(true, Ordering::Relaxed);
    #[cfg(test)]
    OFF_TL.with(|o| o.store(true, Ordering::Relaxed));
}

pub fn turned_off() -> bool {
    #[cfg(not(test))]
    {
        OFF.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    OFF_TL.with(|o| o.load(Ordering::Relaxed))
}

/// The extensions in effect now (empty before the first `reload`).
pub fn current() -> Arc<Registry> {
    #[cfg(not(test))]
    {
        CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
    }
    #[cfg(test)]
    CURRENT_TL.with(|c| c.borrow().clone().unwrap_or_default())
}

fn set(r: Arc<Registry>) {
    #[cfg(not(test))]
    {
        *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(r);
    }
    #[cfg(test)]
    CURRENT_TL.with(|c| *c.borrow_mut() = Some(r));
}

/// The store for a settings folder. Packages live next to the state database (`CODEYARD_STATE_DIR` in scripts); in
/// tests, inside the test's own settings folder so no test ever reads the user's extensions.
pub fn store(cfg_dir: &Path) -> ext_core::Store {
    ext_core::Store::new(data_root(cfg_dir), cfg_dir.join(LOCK_FILE))
}

/// `<data>/extensions`: the store, and the registries' cached indexes in `registries/`.
pub fn data_root(cfg_dir: &Path) -> PathBuf {
    #[cfg(test)]
    let root = cfg_dir.join("extension-store");
    #[cfg(not(test))]
    let root = crate::workbench::state_path().and_then(|p| p.parent().map(|d| d.join("extensions"))).unwrap_or_else(|| cfg_dir.join("extension-store"));
    root
}

/// Load the installed set again (at startup and whenever settings are reloaded, which includes a change of the lock file).
/// Cheap when nothing changed: one small file is read. Returns problems worth showing.
pub fn reload(cfg_dir: Option<&Path>) -> Vec<String> {
    reload_with(cfg_dir, true)
}

/// `reload`, with `extensions.enabled` from the user's settings file.
pub fn reload_with(cfg_dir: Option<&Path>, enabled: bool) -> Vec<String> {
    if turned_off() || !enabled {
        set(Arc::new(Registry::default()));
        return Vec::new();
    }
    let Some(dir) = cfg_dir else {
        set(Arc::new(Registry::default()));
        return Vec::new();
    };
    let store = store(dir);
    let reg = match store.load() {
        Ok(loaded) => {
            let mut problems = loaded.problems;
            let mut active = Vec::new();
            // Revoked by its registry, or depending on one that is: stopped until a newer version replaces it.
            let stopped = ext_core::stopped(&loaded.extensions, &store.revoked());
            for e in loaded.extensions.into_iter().filter(|e| !e.disabled) {
                if let Some(why) = stopped.get(&e.manifest.id) {
                    problems.push(stopped_line(&e.manifest.id, why));
                    continue;
                }
                // Code runs in codeyard-exthost, which ships beside the app; without it none of the extension applies.
                // Nothing here starts a process: the sandbox is checked when code first starts.
                if let Some(Err(why)) = e.manifest.code.as_ref().map(|_| crate::ext_runtime::code_loads_here(&e.manifest.id)) {
                    problems.push(why);
                    continue;
                }
                active.push(Active { dir: store.store_dir(&e.digest), manifest: Arc::new(e.manifest) });
            }
            Registry { active, problems }
        }
        Err(e) => Registry { active: Vec::new(), problems: vec![e.to_string()] },
    };
    let mut reg = reg;
    let mut problems: Vec<String> = reg.problems.iter().map(|p| crate::i18n::tf("Extension not loaded: {}", &[p])).collect();
    // The extension in development replaces an installed copy of itself.
    if let Some(dir) = dev_dir() {
        match load_dev(&dir) {
            Ok(dev) => {
                reg.active.retain(|e| e.id() != dev.id());
                reg.active.push(dev);
                reg.active.sort_by(|a, b| a.id().cmp(b.id()));
            }
            Err(e) => problems.insert(0, crate::i18n::tf("Extension in development not loaded: {}", &[&e])),
        }
    }
    for e in &reg.active {
        for l in &e.manifest.contributions.languages {
            if let Some(g) = l.grammar.as_deref().filter(|g| !crate::editor::grammar_known(g)) {
                problems.push(format!("{}: {}", e.id(), crate::i18n::tf("language “{0}” names grammar “{1}”, which Codeyard does not have", &[&l.id, &g])));
            }
        }
    }
    set(Arc::new(reg));
    problems
}

/// Why an extension does not run, for the status bar and its page.
pub fn stopped_line(id: &ext_manifest::ExtId, why: &ext_core::Stopped) -> String {
    match why {
        ext_core::Stopped::Revoked { reason } => crate::i18n::tf("{0} was stopped: the registry revoked this version ({1})", &[id, reason]),
        ext_core::Stopped::Dependency { on } => crate::i18n::tf("{0} was stopped: it needs {1}, which was stopped", &[id, on]),
    }
}

/// Install the registry directly (tests).
#[cfg(test)]
pub fn set_for_test(active: Vec<Active>) {
    set(Arc::new(Registry { active, problems: Vec::new() }));
}

/// An extension's `when` in GPUI's key-context language. Key bindings run without extension code, so only what the
/// app's key contexts carry is accepted: `editor`, `terminal`, `file-tree`, `scm`, `search`, and
/// `editor.language == '…'`. Anything else (`ext.*`, `workspace.trusted`, `panel`) is refused with the reason.
pub fn gpui_when(src: &str) -> Result<String, String> {
    use ext_manifest::when::{Expr, Value};
    fn context(key: &str) -> Result<&'static str, String> {
        Ok(match key {
            "editor" => "Editor",
            "terminal" => "Terminal",
            "file-tree" => "FileTree",
            "scm" => "ScmPanel",
            "search" => "SearchPanel",
            other => return Err(crate::i18n::tf("“{}” cannot be used in a key binding", &[&other])),
        })
    }
    fn language(a: &Expr, b: &Expr) -> Result<String, String> {
        match (a, b) {
            (Expr::Key(k), Expr::Lit(Value::Str(s))) | (Expr::Lit(Value::Str(s)), Expr::Key(k))
                if k == "editor.language" && !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') =>
            {
                Ok(s.clone())
            }
            _ => Err(crate::i18n::t("in a key binding, == and != compare editor.language with a language id").into()),
        }
    }
    fn go(e: &Expr) -> Result<String, String> {
        Ok(match e {
            Expr::Key(k) => context(k)?.to_string(),
            Expr::Not(x) => format!("!({})", go(x)?),
            Expr::And(a, b) => format!("({} && {})", go(a)?, go(b)?),
            Expr::Or(a, b) => format!("({} || {})", go(a)?, go(b)?),
            Expr::Eq(a, b) => format!("language == {}", language(a, b)?),
            Expr::Ne(a, b) => format!("language != {}", language(a, b)?),
            Expr::Lit(_) => return Err(crate::i18n::t("a key binding condition needs a context, not a constant").into()),
        })
    }
    go(&ext_manifest::when::parse(src)?)
}

/// The key context value an editor carries for `editor.language == '…'`: the language id when it is a plain identifier.
pub fn language_context(lang: &str) -> Option<&str> {
    (!lang.is_empty() && lang.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')).then_some(lang)
}

/// One key binding contributed by an extension, ready for GPUI. `command` is a palette command id (`core:` removed), or
/// for an extension command its local id, with `extension` the extension that owns it.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub ext: String,
    pub key: String,
    pub command: String,
    pub when: Option<String>,
    pub extension: Option<String>,
}

/// Every extension key binding, and one warning per binding that cannot apply.
pub fn bindings() -> (Vec<Binding>, Vec<String>) {
    let (mut out, mut warnings) = (Vec::new(), Vec::new());
    let reg = current();
    for ext in &reg.active {
        for b in &ext.manifest.contributions.keybindings {
            // A core command, or a command of this extension or a dependency (the manifest checked which): the owner must
            // be active and have code.
            let (command, extension) = match b.command.strip_prefix("core:") {
                Some(command) => (command.to_string(), None),
                None => {
                    let owner = reg.active.iter().find(|o| b.command.strip_prefix(o.id()).is_some_and(|r| r.starts_with('.')));
                    match owner.filter(|o| o.manifest.code.is_some() && o.manifest.contributions.commands.iter().any(|c| c.id == b.command)) {
                        Some(o) => (b.command[o.id().len() + 1..].to_string(), Some(o.id().to_string())),
                        None => {
                            warnings.push(format!("{}: {}", ext.id(), crate::i18n::tf("“{}” is not the command of an active extension", &[&b.command])));
                            continue;
                        }
                    }
                }
            };
            let when = match b.when.as_deref().map(gpui_when).transpose() {
                Ok(w) => w,
                Err(e) => {
                    warnings.push(format!("{}: {e}", ext.id()));
                    continue;
                }
            };
            out.push(Binding { ext: ext.id().to_string(), key: b.key.clone(), command, when, extension });
        }
    }
    (out, warnings)
}

/// `editor.formatters`: language id → `lsp` or the id of an extension that formats it. Wrong entries are warnings.
pub fn formatters_setting(v: Option<&serde_json::Value>, warnings: &mut Vec<String>) -> Vec<(String, String)> {
    use serde_json::Value;
    let mut out = Vec::new();
    match v {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (lang, who) in m {
                match who.as_str().map(str::trim) {
                    Some(w) if !w.is_empty() => out.push((lang.clone(), w.to_string())),
                    _ => warnings.push(crate::i18n::tf("editor.formatters.{}: expected “lsp” or an extension id", &[&lang])),
                }
            }
        }
        Some(_) => warnings.push(crate::i18n::tf("{0}: expected an object {1}", &[&"editor.formatters", &"{ \"toml\": \"you.formatter\" }"])),
    }
    out
}

/// The keymap preset chosen with `keymap.preset` (`<publisher.name>/<label>`): its file, in `keybindings.jsonc` format.
pub fn preset(choice: &str) -> Option<Result<(String, PathBuf), String>> {
    if choice.is_empty() {
        return None;
    }
    let (id, label) = choice.split_once('/').unwrap_or((choice, ""));
    let reg = current();
    let found = reg.active.iter().find(|e| e.id() == id).and_then(|e| {
        let presets = &e.manifest.contributions.keymap_presets;
        presets.iter().find(|p| p.label == label || (label.is_empty() && presets.len() == 1)).map(|p| (format!("{id}: {}", p.path), e.file(&p.path)))
    });
    Some(found.ok_or_else(|| {
        let available = presets();
        if available.is_empty() {
            crate::i18n::tf("keymap preset “{}” is not installed", &[&choice])
        } else {
            crate::i18n::tf("keymap preset “{0}” is not installed (installed: {1})", &[&choice, &available.join(", ")])
        }
    }))
}

/// Every installed preset as `<publisher.name>/<label>`, for the settings page and the warning.
pub fn presets() -> Vec<String> {
    current().active.iter().flat_map(|e| e.manifest.contributions.keymap_presets.iter().map(move |p| format!("{}/{}", e.id(), p.label))).collect()
}

/// A string kept for the rest of the process. Language ids and comment tokens are few and short, and the code that uses
/// them takes `&'static str`; interning the same text twice returns the first copy, so reloading never grows it.
fn intern(s: &str) -> &'static str {
    static SEEN: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = seen.iter().find(|x| **x == s) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    seen.push(leaked);
    leaked
}

/// A language an extension adds for files no built-in language claims.
#[derive(Clone, Debug, PartialEq)]
pub struct Language {
    pub id: &'static str,
    /// A built-in grammar to highlight with, by language name (as in `files.associations`).
    pub grammar: Option<String>,
    pub line_comment: Option<&'static str>,
    pub block_comment: Option<(&'static str, &'static str)>,
}

/// The extension language for `path`, by exact file name first, then by extension (ignoring case). Callers ask only
/// after the built-in tables found nothing: extensions fill gaps, they do not replace a built-in language
/// (`files.associations` does that).
pub fn language_for(path: &str) -> Option<Language> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    let reg = current();
    let langs = || reg.active.iter().flat_map(|e| e.manifest.contributions.languages.iter());
    let hit = langs()
        .find(|l| l.filenames.iter().any(|f| f == name))
        .or_else(|| ext.as_deref().and_then(|x| langs().find(|l| l.extensions.iter().any(|e| e.eq_ignore_ascii_case(x)))))?;
    Some(Language {
        id: intern(&hit.id),
        grammar: hit.grammar.clone(),
        line_comment: hit.line_comment.as_deref().map(intern),
        block_comment: hit.block_comment.as_ref().map(|(a, b)| (intern(a), intern(b))),
    })
}

/// Language server entries for `lsp.servers`, before the user's own (the user's entry for the same key wins). Each
/// extension server is entered under every language it serves. Programs come from `PATH` where the files are, so they
/// work over SSH too; a program shipped inside the package waits for signed native packages.
pub fn server_entries() -> (Vec<(String, Option<crate::lsp::ServerSpec>)>, Vec<String>) {
    let (mut out, mut warnings) = (Vec::new(), Vec::new());
    for ext in &current().active {
        for s in &ext.manifest.contributions.language_servers {
            if s.command.contains('/') {
                warnings.push(format!(
                    "{}: {}",
                    ext.id(),
                    crate::i18n::tf("“{}” ships inside the package; this version only starts language servers found on PATH", &[&s.command])
                ));
                continue;
            }
            for lang in &s.languages {
                let key = crate::lsp::key_for_language(lang).unwrap_or(lang.as_str()).to_string();
                out.push((key, Some(crate::lsp::ServerSpec::from((s.command.clone(), s.args.clone())))));
            }
        }
    }
    (out, warnings)
}

/// JSON and YAML schema associations, to follow the user's own: (file globs, absolute path of the schema file). The files
/// are on this machine, so `package_dirs` tells the schema service to read them here for a remote workspace too.
pub fn schema_entries() -> Vec<(Vec<String>, String)> {
    let mut out = Vec::new();
    for ext in &current().active {
        let c = &ext.manifest.contributions;
        for s in c.json_schemas.iter().chain(&c.yaml_schemas) {
            out.push((s.file_match.clone(), ext.file(&s.path).to_string_lossy().into_owned()));
        }
    }
    out
}

/// The package folders of the active extensions.
pub fn package_dirs() -> Vec<PathBuf> {
    current().active.iter().map(|e| e.dir.clone()).collect()
}

/// Check a `settings.jsonc` value for `ext.<publisher.name>.<key>` against what the extension declares.
pub fn check_setting(key: &str, value: &serde_json::Value) -> Result<(), String> {
    use ext_manifest::SettingKind as K;
    use serde_json::Value as V;
    let rest = key.strip_prefix("ext.").unwrap_or(key);
    let reg = current();
    let found = reg
        .active
        .iter()
        .find_map(|e| rest.strip_prefix(e.id()).and_then(|k| k.strip_prefix('.')).and_then(|k| e.manifest.contributions.settings.iter().find(|s| s.key == k)));
    let Some(decl) = found else {
        return Err(crate::i18n::tf("{} is not a setting of an installed extension", &[&format!("\"{key}\"")]));
    };
    let ok = match (decl.kind, value) {
        (K::String, V::String(s)) => decl.choices.is_empty() || decl.choices.contains(s),
        (K::Number, V::Number(_)) | (K::Boolean, V::Bool(_)) | (K::Array, V::Array(_)) | (K::Object, V::Object(_)) => true,
        _ => false,
    };
    if ok {
        return Ok(());
    }
    let kind = match decl.kind {
        K::String if !decl.choices.is_empty() => decl.choices.join(" | "),
        K::String => "string".into(),
        K::Number => "number".into(),
        K::Boolean => "boolean".into(),
        K::Array => "array".into(),
        K::Object => "object".into(),
    };
    Err(crate::i18n::tf("{0}: expected {1}", &[&format!("\"{key}\""), &kind]))
}

/// Install a package file (`codeyard --install-extension`): the checks of the Extensions view, without its sheet. A
/// package with code installs only when it asks for no permissions: anything more needs the consent sheet.
pub fn install_file(store: &ext_core::Store, path: &Path) -> Result<Manifest, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let pkg = ext_pkg::read(&bytes).map_err(|e| e.to_string())?;
    if pkg.manifest.code.is_some() && !pkg.manifest.permissions.is_empty() {
        return Err(crate::i18n::tf("{} asks for permissions: install it from the Extensions view to review them", &[&pkg.manifest.id]));
    }
    let installed = store.install(&bytes, ext_core::Source::File { path: path.display().to_string() }).map_err(|e| e.to_string())?;
    Ok(installed.manifest)
}
