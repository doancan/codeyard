//! Installed extensions as the app sees them: the set `ext_core` loads from the store and the lock file, minus the ones
//! the user turned off, or nothing at all with `--no-extensions`. Every contribution reader (themes, snippets, keymaps,
//! languages, schemas, settings, language servers) takes its entries from `current()`, so turning one off or installing
//! one in another window changes all of them on the next settings reload. No extension code runs: Phase 0 extensions only
//! contribute data (docs/plugin-architecture.md §5.1).

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
    let lock = cfg_dir.join(LOCK_FILE);
    #[cfg(test)]
    let root = cfg_dir.join("extension-store");
    #[cfg(not(test))]
    let root = crate::workbench::state_path().and_then(|p| p.parent().map(|d| d.join("extensions"))).unwrap_or_else(|| cfg_dir.join("extension-store"));
    ext_core::Store::new(root, lock)
}

/// Load the installed set again (at startup and whenever settings are reloaded, which includes a change of the lock file).
/// Cheap when nothing changed: one small file is read. Returns problems worth showing.
pub fn reload(cfg_dir: Option<&Path>) -> Vec<String> {
    if turned_off() {
        set(Arc::new(Registry::default()));
        return Vec::new();
    }
    let Some(dir) = cfg_dir else {
        set(Arc::new(Registry::default()));
        return Vec::new();
    };
    let store = store(dir);
    let reg = match store.load() {
        Ok(loaded) => Registry {
            active: loaded
                .extensions
                .into_iter()
                .filter(|e| !e.disabled)
                .map(|e| Active { dir: store.store_dir(&e.digest), manifest: Arc::new(e.manifest) })
                .collect(),
            problems: loaded.problems,
        },
        Err(e) => Registry { active: Vec::new(), problems: vec![e.to_string()] },
    };
    let mut problems: Vec<String> = reg.problems.iter().map(|p| crate::i18n::tf("Extension not loaded: {}", &[p])).collect();
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

/// One key binding contributed by an extension, ready for GPUI. `command` is a palette command id (`core:` removed).
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub ext: String,
    pub key: String,
    pub command: String,
    pub when: Option<String>,
}

/// Every extension key binding, and one warning per binding that cannot apply.
pub fn bindings() -> (Vec<Binding>, Vec<String>) {
    let (mut out, mut warnings) = (Vec::new(), Vec::new());
    for ext in &current().active {
        for b in &ext.manifest.contributions.keybindings {
            // Phase 0 extensions have no code, so only core commands can be bound.
            let Some(command) = b.command.strip_prefix("core:") else {
                warnings.push(format!("{}: {}", ext.id(), crate::i18n::tf("“{}” needs extension code, which this version does not run", &[&b.command])));
                continue;
            };
            let when = match b.when.as_deref().map(gpui_when).transpose() {
                Ok(w) => w,
                Err(e) => {
                    warnings.push(format!("{}: {e}", ext.id()));
                    continue;
                }
            };
            out.push(Binding { ext: ext.id().to_string(), key: b.key.clone(), command: command.to_string(), when });
        }
    }
    (out, warnings)
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
