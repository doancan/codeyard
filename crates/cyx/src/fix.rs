//! `cyx check --fix`: apply the fix lines of missing-permission errors (`CY0201`, from the manifest checks and the
//! component's import check) to `codeyard-extension.toml`. The file is edited in place with `toml_edit`, so comments,
//! order and spacing stay as the author wrote them. A fix with a placeholder (`net = ["<host>"]`) or a guess (which
//! sign-in provider the code means) is listed for the author instead.

use std::path::Path;

use ext_manifest::{Code, Diagnostic};
use toml_edit::{DocumentMut, Item, Table, Value};

const PREFIX: &str = "add to [permissions]:";

/// What `--fix` did and left.
#[derive(Debug, Default, PartialEq)]
pub struct Fixed {
    /// Lines added or changed in `[permissions]`, as they now read.
    pub applied: Vec<String>,
    /// Fix lines that need the author: the line and why.
    pub left: Vec<(String, String)>,
    /// For TypeScript and JavaScript code, whose hosts, files and programs `cyx` cannot see: what to add by hand.
    pub note: Option<String>,
}

/// TypeScript imports every interface its permissions grant, so the import check cannot say which host a `fetch`
/// needs; `cyx test` and the Inspector can, when the call is refused.
pub const SCRIPT_NOTE: &str = "TypeScript and JavaScript code reaches the host through the SDK, so cyx cannot see which hosts, files or programs it uses. Add the line from the CY0201 message that `cyx test` or the Inspector printed, such as net = [\"api.example.com\"]";

/// Apply every fix the checks find to the manifest in `dir`; repeats while fixes uncover others (the import check runs
/// only once the manifest itself passes).
pub fn fix(dir: &Path) -> Result<Fixed, String> {
    let path = dir.join(ext_manifest::FILE_NAME);
    let mut out = Fixed::default();
    for _ in 0..4 {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("error: cannot read {}: {e}", path.display()))?;
        let diagnostics = problems(dir, &text)?;
        let (new, round) = apply(&text, &diagnostics)?;
        for l in round.left {
            if !out.left.contains(&l) {
                out.left.push(l);
            }
        }
        if round.applied.is_empty() {
            break;
        }
        std::fs::write(&path, new).map_err(|e| format!("error: cannot write {}: {e}", path.display()))?;
        out.applied.extend(round.applied);
    }
    let script = crate::build::manifest(dir)
        .ok()
        .and_then(|m| m.code)
        .is_some_and(|c| matches!(c.language, ext_manifest::CodeLanguage::TypeScript | ext_manifest::CodeLanguage::JavaScript));
    if script && out.applied.is_empty() {
        out.note = Some(SCRIPT_NOTE.into());
    }
    Ok(out)
}

/// The diagnostics `cyx check` would print: the manifest's, then the package's (the import check among them).
fn problems(dir: &Path, text: &str) -> Result<Vec<Diagnostic>, String> {
    if let Err(errors) = ext_manifest::parse_with(text, ext_manifest::Options { require_why: false }) {
        return Ok(errors);
    }
    let files = ext_pkg::collect_dir_for(dir, ext_pkg::folder_target(dir).as_deref()).map_err(|e| format!("error: {e}"))?;
    let files = files
        .into_iter()
        .map(|mut e| {
            if e.path == ext_manifest::FILE_NAME {
                e.data = crate::with_placeholder_why(&String::from_utf8_lossy(&e.data)).into_bytes();
            }
            e
        })
        .collect();
    Ok(match ext_pkg::write(files) {
        Err(ext_pkg::Error::Manifest(errors) | ext_pkg::Error::Component(errors)) => errors,
        _ => Vec::new(),
    })
}

/// `text` with the fixes of `diagnostics` applied.
pub fn apply(text: &str, diagnostics: &[Diagnostic]) -> Result<(String, Fixed), String> {
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("error: {} is not valid TOML: {e}", ext_manifest::FILE_NAME))?;
    let mut fixed = Fixed::default();
    for d in diagnostics.iter().filter(|d| d.code == Code::NotDeclared) {
        let Some(line) = d.fix.as_deref().and_then(|f| f.strip_prefix(PREFIX)).map(str::trim) else {
            // Not a line to add (an `auth` with no provider to sign in to): the author decides.
            fixed.left.push((d.message.clone(), d.fix.clone().unwrap_or_else(|| "see `cyx check`".into())));
            continue;
        };
        if line.contains('<') {
            fixed.left.push((line.to_string(), "put in the real value".into()));
            continue;
        }
        let Ok(one) = line.parse::<DocumentMut>() else { continue };
        let Some((key, item)) = one.iter().next() else { continue };
        let Some(value) = item.as_value() else { continue };
        // The import check only knows the code signs in somewhere, not with which provider.
        if key == "auth" && d.message.starts_with("the code uses") {
            fixed.left.push((line.to_string(), "name the sign-in providers the code uses".into()));
            continue;
        }
        if let Some(changed) = add(&mut doc, key, value)
            && !fixed.applied.contains(&changed)
        {
            fixed.applied.push(changed);
        }
    }
    Ok((doc.to_string(), fixed))
}

/// Merge `key = value` into `[permissions]`: arrays gain the missing entries, `clipboard` widens to `read-write`, other
/// values are set. The line as it now reads, or none when nothing changed.
fn add(doc: &mut DocumentMut, key: &str, value: &Value) -> Option<String> {
    let table = doc.entry("permissions").or_insert_with(|| Item::Table(Table::new())).as_table_mut()?;
    match (table.get_mut(key).and_then(Item::as_value_mut), value) {
        (Some(Value::Array(have)), Value::Array(want)) => {
            let mut changed = false;
            for v in want.iter() {
                if !have.iter().any(|h| h.as_str() == v.as_str()) {
                    // Laid out like the entry before it (one line, or one entry per line).
                    let prefix = have.iter().last().and_then(|l| l.decor().prefix()).and_then(|p| p.as_str()).unwrap_or("").to_string();
                    let mut v = v.clone();
                    v.decor_mut().set_prefix(if have.is_empty() {
                        String::new()
                    } else if prefix.is_empty() {
                        " ".into()
                    } else {
                        prefix
                    });
                    v.decor_mut().set_suffix("");
                    have.push_formatted(v);
                    changed = true;
                }
            }
            if !changed {
                return None;
            }
        }
        (Some(Value::String(have)), Value::String(want)) if key == "clipboard" => {
            let merged = if have.value() == want.value() || have.value() == "read-write" { return None } else { "read-write" };
            table.insert(key, toml_edit::value(merged));
        }
        (Some(have), want) if have.to_string().trim() == want.to_string().trim() => return None,
        (_, want) => {
            let mut v = want.clone();
            v.decor_mut().clear();
            if let Value::Array(a) = &mut v {
                a.fmt();
            }
            table.insert(key, Item::Value(v));
        }
    }
    let mut now = table.get(key)?.as_value()?.clone();
    now.decor_mut().clear();
    Some(format!("{key} = {now}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing(line: &str) -> Diagnostic {
        Diagnostic::new(Code::NotDeclared, "run access to \"taplo\" is not declared").fix(format!("{PREFIX}  {line}"))
    }

    #[test]
    fn adds_to_permissions_and_keeps_the_rest() {
        let text = "# My tools\n[extension]\nid = \"a.b\"   # keep me\n\n[permissions]\n# programs\nrun = [\"git\"]  # trailing\n\n[[commands]]\nid = \"x\"\n";
        let d = [missing("run = [\"taplo\"]"), missing("secrets = true"), missing("run = [\"git\"]")];
        let (out, f) = apply(text, &d).unwrap();
        assert_eq!(
            out,
            "# My tools\n[extension]\nid = \"a.b\"   # keep me\n\n[permissions]\n# programs\nrun = [\"git\", \"taplo\"]  # trailing\nsecrets = true\n\n[[commands]]\nid = \"x\"\n"
        );
        assert_eq!(f.applied, ["run = [\"git\", \"taplo\"]", "secrets = true"]);
        assert!(f.left.is_empty());
    }

    #[test]
    fn a_list_on_several_lines_stays_that_way() {
        let text = "[permissions]\nrun = [\n    \"git\",\n    \"cargo\",\n]\n";
        let (out, _) = apply(text, &[missing("run = [\"taplo\"]")]).unwrap();
        assert_eq!(out, "[permissions]\nrun = [\n    \"git\",\n    \"cargo\",\n    \"taplo\",\n]\n");
    }

    #[test]
    fn a_missing_table_is_added_and_placeholders_are_left() {
        let text = "[extension]\nid = \"a.b\"\n";
        let d = [
            missing("net = [\"<host>\"]"),
            missing("clipboard = \"read\""),
            Diagnostic::new(Code::NotDeclared, "the code uses `auth`, which needs the auth permission").fix(format!("{PREFIX}  auth = [\"github\"]")),
            Diagnostic::new(Code::WhyRequired, "why").fix(format!("{PREFIX}  why = \"One honest sentence.\"")),
        ];
        let (out, f) = apply(text, &d).unwrap();
        assert_eq!(out, "[extension]\nid = \"a.b\"\n\n[permissions]\nclipboard = \"read\"\n");
        assert_eq!(f.applied, ["clipboard = \"read\""]);
        assert_eq!(f.left.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["net = [\"<host>\"]", "auth = [\"github\"]"]);
        // A missing permission that is not a line to add is listed too, never skipped (e2e trial: "Nothing to fix."
        // and then a CY0201 error).
        let unknown = Diagnostic::new(Code::NotDeclared, "permissions.auth 'acme' is not a built-in provider and has no [[oauth]] table").fix("add [[oauth]]");
        let (_, f) = apply(text, &[unknown]).unwrap();
        assert_eq!(f.left, [("permissions.auth 'acme' is not a built-in provider and has no [[oauth]] table".to_string(), "add [[oauth]]".to_string())]);
        // Reading and writing the clipboard is `read-write`.
        let (out, _) = apply(&out, &[missing("clipboard = \"write\"")]).unwrap();
        assert!(out.ends_with("clipboard = \"read-write\"\n"), "{out}");
    }

    #[test]
    fn fixes_the_manifest_on_disk_until_it_passes() {
        let dir = std::env::temp_dir().join(format!("cyx-fix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = "[extension]\nid = \"you.fmt\"\nversion = \"0.1.0\"\nname = \"Fmt\"\nlicense = \"MIT\"\ndescription = \"d\"\n\n# The server.\n[[language-servers]]\ncommand = \"taplo\"\nargs = [\"lsp\", \"stdio\"]\nlanguages = [\"toml\"]\n";
        std::fs::write(dir.join(ext_manifest::FILE_NAME), manifest).unwrap();
        assert!(crate::check(&dir).unwrap_err().contains("CY0201"));
        let f = fix(&dir).unwrap();
        assert_eq!(f.applied, ["run = [\"taplo\"]"]);
        let text = std::fs::read_to_string(dir.join(ext_manifest::FILE_NAME)).unwrap();
        assert!(text.starts_with(manifest) && text.ends_with("\n[permissions]\nrun = [\"taplo\"]\n"), "{text}");
        // Running a program is high risk: `why` is the author's to write, so check still says so.
        assert!(crate::check(&dir).unwrap_err().contains("CY0202"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// TypeScript's hosts cannot be inferred: --fix says what to add instead of "Nothing to fix." (e2e trial).
    #[test]
    fn typescript_gets_a_note_on_what_to_add() {
        let dir = std::env::temp_dir().join(format!("cyx-fix-ts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = "[extension]\nid = \"you.ts\"\nversion = \"0.1.0\"\nname = \"Ts\"\nlicense = \"MIT\"\n\n[code]\nentry = \"src/extension.ts\"\n\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n";
        std::fs::write(dir.join(ext_manifest::FILE_NAME), manifest).unwrap();
        let f = fix(&dir).unwrap();
        assert!(f.applied.is_empty() && f.left.is_empty(), "{f:?}");
        assert_eq!(f.note.as_deref(), Some(SCRIPT_NOTE));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
