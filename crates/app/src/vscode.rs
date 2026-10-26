//! Import of VS Code settings and shortcuts. VS Code's files are never
//! touched; entries with an equivalent go into codeyard's files, the rest are
//! counted and reported. Pure converters (file I/O is in the workbench).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// VS Code's user directory (`None` if missing).
pub fn user_dir() -> Option<PathBuf> {
    user_dir_in(dirs::config_dir().as_deref(), dirs::home_dir().as_deref())
}

fn user_dir_in(config: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(config) = config {
        candidates.extend([config.join("Code/User"), config.join("VSCodium/User")]);
    }
    if let Some(home) = home {
        candidates.extend([
            home.join("Library/Application Support/Code/User"),
            home.join(".config/Code/User"),
            home.join("Library/Application Support/VSCodium/User"),
            home.join(".config/VSCodium/User"),
        ]);
    }
    candidates.into_iter().find(|d| d.join("settings.json").is_file() || d.join("keybindings.json").is_file())
}

#[derive(Debug, Default, PartialEq)]
pub struct Converted {
    /// codeyard key → value.
    pub settings: Vec<(String, Value)>,
    /// Number of keys VS Code has that have no equivalent here.
    pub skipped: usize,
    pub notes: Vec<String>,
}

/// `themes`: (id, display name), built-in and user themes.
pub fn convert_settings(text: &str, themes: &[(String, String)]) -> Converted {
    let mut out = Converted::default();
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&theme::themes::strip_jsonc(text)) else {
        out.notes.push(crate::i18n::t("VS Code settings.json could not be read").into());
        return out;
    };
    let mut set = |k: &str, v: Value| out.settings.push((k.to_string(), v));
    for (key, v) in &obj {
        let handled = match key.as_str() {
            "editor.fontSize"
            | "editor.tabSize"
            | "editor.insertSpaces"
            | "editor.formatOnSave"
            | "files.trimTrailingWhitespace"
            | "files.insertFinalNewline"
            | "files.autoSaveDelay"
            | "search.maxResults"
            | "files.hotExit" => {
                set(key, v.clone());
                true
            }
            "editor.fontFamily" => match v.as_str().and_then(first_font) {
                Some(f) => {
                    set(key, json!(f));
                    true
                }
                None => false,
            },
            // 0: auto; below 8 it's a multiple of the font size.
            "editor.lineHeight" => match v.as_f64() {
                Some(h) if h >= 8. => {
                    set(key, json!(h));
                    true
                }
                Some(m) if m > 0. => {
                    let size = obj.get("editor.fontSize").and_then(Value::as_f64).unwrap_or(14.);
                    set(key, json!((m * size).round()));
                    true
                }
                _ => false,
            },
            "files.autoSave" => {
                let mapped = match v.as_str() {
                    Some("onWindowChange") => Some("onFocusChange"),
                    Some(s @ ("off" | "afterDelay" | "onFocusChange")) => Some(s),
                    _ => None,
                };
                mapped.map(|m| set(key, json!(m))).is_some()
            }
            "files.exclude" => match v.as_object() {
                Some(m) => {
                    let globs: Vec<Value> = m.iter().filter(|(_, on)| on.as_bool() == Some(true)).map(|(g, _)| json!(g)).collect();
                    set(key, Value::Array(globs));
                    true
                }
                None => false,
            },
            "editor.autoClosingBrackets" => {
                set(key, json!(v.as_str() != Some("never")));
                true
            }
            "editor.occurrencesHighlight" => {
                set("editor.wordHighlight", json!(v.as_bool().unwrap_or(v.as_str() != Some("off"))));
                true
            }
            "editor.minimap.enabled" => {
                set(key, v.clone());
                true
            }
            "editor.guides.indentation" => {
                set("editor.indentGuides", v.clone());
                true
            }
            "terminal.integrated.fontSize" => {
                set("terminal.fontSize", v.clone());
                true
            }
            "terminal.integrated.scrollback" => {
                set("terminal.scrollback", v.clone());
                true
            }
            "workbench.colorTheme" => {
                let name = v.as_str().unwrap_or_default();
                match match_theme(name, themes) {
                    Some(id) => {
                        set("theme", json!(id));
                        true
                    }
                    None => {
                        out.notes.push(crate::i18n::tf(
                            "theme {} isn't installed here: copy its .json into ~/.config/codeyard/themes to use it",
                            &[&format!("\"{name}\"")],
                        ));
                        false
                    }
                }
            }
            _ => false,
        };
        if !handled {
            out.skipped += 1;
        }
    }
    out
}

/// "Menlo, 'Courier New', monospace" → "Menlo".
fn first_font(list: &str) -> Option<String> {
    list.split(',').map(|f| f.trim().trim_matches(['"', '\'']).to_string()).find(|f| !f.is_empty() && f != "monospace")
}

fn match_theme(name: &str, themes: &[(String, String)]) -> Option<String> {
    let n = name.to_lowercase();
    themes.iter().find(|(id, label)| label.to_lowercase() == n || id.to_lowercase() == n).map(|(id, _)| id.clone())
}

/// Find a theme installed in VS Code by name (extensions and built-ins);
/// returns one JSON with `include` inheritance merged.
pub fn find_theme(name: &str) -> Option<Value> {
    let mut installs = Vec::new();
    if cfg!(windows) {
        if let Some(local) = dirs::data_local_dir() {
            installs.extend([local.join("Programs/Microsoft VS Code"), local.join("Programs/VSCodium")]);
        }
        for dir in ["ProgramFiles", "ProgramFiles(x86)"].into_iter().filter_map(std::env::var_os) {
            let dir = PathBuf::from(dir);
            installs.extend([dir.join("Microsoft VS Code"), dir.join("VSCodium")]);
        }
    }
    find_theme_in(&theme_roots(dirs::home_dir().as_deref(), &installs), name)
}

fn theme_roots(home: Option<&Path>, installs: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = home {
        roots.extend([home.join(".vscode/extensions"), home.join(".vscode-oss/extensions")]);
    }
    roots.extend(installs.iter().map(|dir| dir.join("resources/app/extensions")));
    roots.extend([
        PathBuf::from("/Applications/Visual Studio Code.app/Contents/Resources/app/extensions"),
        PathBuf::from("/usr/share/code/resources/app/extensions"),
        PathBuf::from("/usr/lib/code/resources/app/extensions"),
    ]);
    roots
}

pub fn find_theme_in(roots: &[PathBuf], name: &str) -> Option<Value> {
    let want = name.to_lowercase();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        for ext in rd.flatten() {
            let dir = ext.path();
            let Some(pkg) = std::fs::read_to_string(dir.join("package.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
            let nls: Value = std::fs::read_to_string(dir.join("package.nls.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            let Some(list) = pkg["contributes"]["themes"].as_array() else { continue };
            for t in list {
                let raw = t["label"].as_str().unwrap_or_default();
                // Built-in labels are "%key%": resolve them from package.nls.json.
                let label = raw.strip_prefix('%').and_then(|k| k.strip_suffix('%')).and_then(|k| nls[k].as_str()).unwrap_or(raw);
                let id = t["id"].as_str().unwrap_or_default();
                let hit = [label.to_lowercase(), format!("default {}", label.to_lowercase()), id.to_lowercase()].contains(&want);
                if hit && let Some(path) = t["path"].as_str() {
                    let mut merged = load_theme(&dir.join(path), 0)?;
                    merged["name"] = json!(name);
                    if merged["type"].is_null() {
                        merged["type"] = json!(if t["uiTheme"].as_str() == Some("vs") { "light" } else { "dark" });
                    }
                    return Some(merged);
                }
            }
        }
    }
    None
}

/// Theme file + its `include` chain: this file's colors go on top of the base's.
fn load_theme(path: &std::path::Path, depth: usize) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    // A theme is an object; anything else (a theme file can come from any extension) is not one, and would panic below
    // where fields are written.
    let mut v: Value = serde_json::from_str(&theme::themes::strip_jsonc(&text)).ok().filter(Value::is_object)?;
    if let (Some(inc), true) = (v["include"].as_str().map(str::to_string), depth < 4)
        && let Some(base) = path.parent().and_then(|d| load_theme(&d.join(&inc), depth + 1))
    {
        let mut colors = base["colors"].as_object().cloned().unwrap_or_default();
        colors.extend(v["colors"].as_object().cloned().unwrap_or_default());
        let mut tokens = base["tokenColors"].as_array().cloned().unwrap_or_default();
        tokens.extend(v["tokenColors"].as_array().cloned().unwrap_or_default());
        v["colors"] = Value::Object(colors);
        v["tokenColors"] = Value::Array(tokens);
        if v["type"].is_null() {
            v["type"] = base["type"].clone();
        }
        v.as_object_mut()?.remove("include");
    }
    Some(v)
}

/// VS Code command → codeyard command id.
pub fn map_command(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "workbench.action.quickOpen" => "workbench.quickOpen",
        "workbench.action.showCommands" => "workbench.commandPalette",
        "workbench.action.gotoLine" => "workbench.goToLine",
        "workbench.action.findInFiles" | "workbench.view.search" => "workbench.findInFiles",
        "workbench.action.toggleSidebarVisibility" => "workbench.toggleSidebar",
        "workbench.view.explorer" => "workbench.showFiles",
        "workbench.view.scm" => "workbench.showSourceControl",
        "workbench.actions.view.problems" => "workbench.showProblems",
        "workbench.action.closeActiveEditor" => "workbench.closeEditor",
        "workbench.action.selectTheme" => "workbench.selectTheme",
        "workbench.action.togglePanel" => "workbench.togglePanel",
        "workbench.action.terminal.toggleTerminal" => "terminal.toggle",
        "workbench.action.terminal.new" => "terminal.new",
        "workbench.action.terminal.kill" => "terminal.kill",
        "workbench.action.files.save" => "editor.save",
        "workbench.action.files.saveAll" => "workbench.saveAll",
        "workbench.action.splitEditor" | "workbench.action.splitEditorRight" => "workbench.splitEditor",
        "workbench.action.focusFirstEditorGroup" => "workbench.focusLeftGroup",
        "workbench.action.focusSecondEditorGroup" => "workbench.focusRightGroup",
        "workbench.action.zoomIn" => "workbench.zoomIn",
        "workbench.action.zoomOut" => "workbench.zoomOut",
        "workbench.action.zoomReset" => "workbench.zoomReset",
        "workbench.action.openSettings" | "workbench.action.openSettingsJson" => "workbench.openSettings",
        "workbench.action.openGlobalKeybindings" | "workbench.action.openGlobalKeybindingsFile" => "workbench.openKeybindings",
        "workbench.action.gotoSymbol" => "editor.goToSymbol",
        "workbench.action.showAllSymbols" => "workbench.goToWorkspaceSymbol",
        "editor.action.toggleWordWrap" => "editor.toggleWordWrap",
        "workbench.action.navigateBack" => "workbench.goBack",
        "workbench.action.navigateForward" => "workbench.goForward",
        "workbench.action.reopenClosedEditor" => "workbench.reopenClosedEditor",
        "workbench.action.files.newUntitledFile" | "explorer.newFile" => "workbench.newFile",
        "workbench.action.files.saveAs" => "workbench.saveAs",
        "workbench.action.newWindow" => "workbench.newWindow",
        "workbench.action.tasks.runTask" => "tasks.run",
        "workbench.action.positionPanelRight" | "workbench.action.positionPanelBottom" => "workbench.togglePanelPosition",
        "workbench.action.toggleMaximizedPanel" => "workbench.toggleMaximizedPanel",
        "workbench.action.tasks.build" => "tasks.build",
        "workbench.action.tasks.reRunTask" => "tasks.rerun",
        "workbench.action.tasks.configureTaskRunner" => "tasks.configure",
        "workbench.action.openRecent" | "projectManager.listProjects" => "projects.switch",
        "projectManager.saveProject" => "projects.save",
        "projectManager.editProjects" => "projects.edit",
        "bookmarks.toggle" => "bookmarks.toggle",
        "bookmarks.jumpToNext" => "bookmarks.jumpToNext",
        "bookmarks.jumpToPrevious" => "bookmarks.jumpToPrevious",
        "bookmarks.list" | "bookmarks.listFromAllFiles" => "bookmarks.list",
        "workbench.files.action.showActiveFileInExplorer" => "workbench.revealActiveFile",
        "workbench.files.action.collapseExplorerFolders" => "workbench.collapseFolders",
        "git.checkout" => "git.checkout",
        "git.branch" => "git.branch",
        "git.pull" => "git.pull",
        "git.push" => "git.push",
        "git.fetch" => "git.fetch",
        "git.sync" => "git.sync",
        "editor.action.selectHighlights" => "editor.selectAllOccurrences",
        "editor.action.smartSelect.expand" => "editor.expandSelection",
        "editor.action.smartSelect.shrink" => "editor.shrinkSelection",
        "editor.action.joinLines" => "editor.joinLines",
        "editor.action.sortLinesAscending" => "editor.sortLinesAscending",
        "editor.action.sortLinesDescending" => "editor.sortLinesDescending",
        "editor.action.transformToUppercase" => "editor.transformToUppercase",
        "editor.action.transformToLowercase" => "editor.transformToLowercase",
        "editor.action.marker.nextInFiles" | "editor.action.marker.next" => "editor.nextProblem",
        "editor.action.marker.prevInFiles" | "editor.action.marker.prev" => "editor.previousProblem",
        "editor.action.goToTypeDefinition" => "editor.goToTypeDefinition",
        "editor.action.goToImplementation" => "editor.goToImplementation",
        "workbench.action.files.revert" => "workbench.revertFile",
        "editor.fold" => "editor.fold",
        "editor.unfold" => "editor.unfold",
        "editor.foldAll" => "editor.foldAll",
        "editor.unfoldAll" => "editor.unfoldAll",
        "workbench.action.moveEditorToNextGroup" | "workbench.action.moveEditorToPreviousGroup" => "workbench.moveEditorToOtherGroup",
        "workbench.action.quit" => "workbench.quit",
        "undo" => "editor.undo",
        "redo" => "editor.redo",
        "editor.action.selectAll" => "editor.selectAll",
        "editor.action.clipboardCopyAction" => "editor.copy",
        "editor.action.clipboardCutAction" => "editor.cut",
        "editor.action.clipboardPasteAction" => "editor.paste",
        "editor.action.commentLine" => "editor.toggleComment",
        "editor.action.moveLinesUpAction" => "editor.moveLinesUp",
        "editor.action.moveLinesDownAction" => "editor.moveLinesDown",
        "editor.action.copyLinesUpAction" => "editor.copyLinesUp",
        "editor.action.copyLinesDownAction" => "editor.copyLinesDown",
        "editor.action.deleteLines" => "editor.deleteLines",
        "expandLineSelection" => "editor.selectLine",
        "editor.action.outdentLines" => "editor.outdent",
        "editor.action.addSelectionToNextFindMatch" => "editor.addNextOccurrence",
        "editor.action.insertCursorAbove" => "editor.addCursorAbove",
        "editor.action.insertCursorBelow" => "editor.addCursorBelow",
        "actions.find" => "editor.find",
        "editor.action.startFindReplaceAction" => "editor.findReplace",
        "editor.action.nextMatchFindAction" => "editor.findNext",
        "editor.action.previousMatchFindAction" => "editor.findPrevious",
        "editor.action.revealDefinition" | "editor.action.goToDeclaration" => "editor.goToDefinition",
        "editor.action.goToReferences" | "editor.action.referenceSearch.trigger" => "editor.findReferences",
        "editor.action.rename" => "editor.rename",
        "editor.action.formatDocument" => "editor.formatDocument",
        "editor.action.quickFix" => "editor.quickFix",
        "editor.action.triggerSuggest" => "editor.triggerSuggest",
        "editor.action.showHover" => "editor.showHover",
        "cursorWordLeft" => "editor.cursorWordLeft",
        "cursorWordRight" => "editor.cursorWordRight",
        "cursorWordLeftSelect" => "editor.selectWordLeft",
        "cursorWordRightSelect" => "editor.selectWordRight",
        "deleteWordLeft" => "editor.deleteWordLeft",
        "deleteWordRight" => "editor.deleteWordRight",
        "cursorTop" => "editor.cursorTop",
        "cursorBottom" => "editor.cursorBottom",
        "cursorHome" => "editor.cursorHome",
        "cursorEnd" => "editor.cursorEnd",
        _ => return None,
    })
}

/// "cmd+shift+p cmd+k" → "cmd-shift-p cmd-k". Scan codes ([BracketRight]) are not supported.
pub fn map_key(key: &str) -> Option<String> {
    let chords: Option<Vec<String>> = key
        .split_whitespace()
        .map(|chord| {
            let parts: Vec<String> = chord
                .split('+')
                .map(|p| match p.to_lowercase().as_str() {
                    "meta" | "win" => "cmd".to_string(),
                    "option" => "alt".to_string(),
                    "esc" => "escape".to_string(),
                    other => other.to_string(),
                })
                .collect();
            (!parts.iter().any(|p| p.starts_with('[') || p.is_empty())).then(|| parts.join("-"))
        })
        .collect();
    chords.filter(|c| !c.is_empty()).map(|c| c.join(" "))
}

/// VS Code "when" → codeyard context. Unknown condition gives `Err` (the shortcut
/// is skipped rather than firing in the wrong place).
pub fn map_when(when: Option<&str>) -> Result<Option<String>, ()> {
    let Some(when) = when.map(str::trim).filter(|w| !w.is_empty()) else { return Ok(None) };
    let mut out = Vec::new();
    for term in when.split("&&").map(str::trim) {
        let mapped = match term {
            "editorTextFocus" | "editorFocus" | "textInputFocus" => Some("Editor"),
            "terminalFocus" => Some("Terminal"),
            "filesExplorerFocus" | "explorerViewletFocus" | "explorerViewletVisible" => Some("FileTree"),
            "!editorReadonly" | "editorHasSelection" | "!editorHasSelection" | "editorLangId != 'markdown'" => None,
            _ => return Err(()),
        };
        if let Some(m) = mapped
            && !out.contains(&m)
        {
            out.push(m);
        }
    }
    Ok(match out.as_slice() {
        [] => None,
        [one] => Some(one.to_string()),
        _ => return Err(()), // two different contexts at once: meaningless
    })
}

/// keybindings.json → codeyard shortcuts (order kept) and the number skipped.
pub fn convert_keybindings(text: &str) -> (Vec<Value>, usize) {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&theme::themes::strip_jsonc(text)) else { return (Vec::new(), 0) };
    let mut out = Vec::new();
    let mut skipped = 0;
    for it in &items {
        let (Some(key), Some(cmd)) = (it["key"].as_str(), it["command"].as_str()) else {
            skipped += 1;
            continue;
        };
        let (removal, cmd) = match cmd.strip_prefix('-') {
            Some(c) => (true, c),
            None => (false, cmd),
        };
        let (Some(k), Some(id), Ok(when)) = (map_key(key), map_command(cmd), map_when(it["when"].as_str())) else {
            skipped += 1;
            continue;
        };
        let mut entry = Map::new();
        entry.insert("key".into(), json!(k));
        entry.insert("command".into(), if removal { Value::Null } else { json!(id) });
        if let Some(w) = when {
            entry.insert("when".into(), json!(w));
        }
        out.push(Value::Object(entry));
    }
    (out, skipped)
}

pub const BEGIN: &str = "  // --- imported from VS Code (re-import replaces this block) ---";
pub const END: &str = "  // --- end of VS Code import ---";

/// Write the shortcuts into codeyard's keybindings.jsonc; a previous import block is replaced.
pub fn merge_keybindings(ours: &str, entries: &[Value]) -> String {
    let block = {
        let lines: Vec<String> = entries.iter().map(|e| format!("  {e},")).collect();
        format!("{BEGIN}\n{}\n{END}\n", lines.join("\n"))
    };
    if let (Some(a), Some(b)) = (ours.find(BEGIN), ours.find(END)) {
        let end = ours[b..].find('\n').map_or(ours.len(), |i| b + i + 1);
        return format!("{}{block}{}", &ours[..a], &ours[end..]);
    }
    match ours.rfind(']') {
        Some(i) => {
            let before = ours[..i].trim_end();
            // Add a comma after the previous entry if missing (unless the array is empty).
            let needs_comma = !before.ends_with('[') && !before.ends_with(',') && !before.trim_end().ends_with("*/");
            let sep = if needs_comma && before.trim_end().ends_with('}') { "," } else { "" };
            format!("{before}{sep}\n{block}{}", &ours[i..])
        }
        None => format!("[\n{block}]\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A theme file from an extension can hold anything: a top level that is not an object is no theme, and including one
    /// does not panic either.
    #[test]
    fn theme_files_that_are_not_objects_are_skipped() {
        let dir = std::env::temp_dir().join(format!("codeyard-theme-shapes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in [("array.json", "[1, 2]"), ("string.json", "\"dark\""), ("number.json", "7")] {
            std::fs::write(dir.join(name), text).unwrap();
            assert_eq!(load_theme(&dir.join(name), 0), None, "{name}");
        }
        std::fs::write(dir.join("child.json"), r##"{ "include": "./array.json", "colors": { "editor.background": "#000000" } }"##).unwrap();
        assert!(load_theme(&dir.join("child.json"), 0).is_some_and(|t| t["colors"]["editor.background"] == "#000000"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn windows_import_finds_roaming_settings_and_installed_themes_without_home() {
        let dir = std::env::temp_dir().join(format!("codeyard-vscode-windows-{}", std::process::id()));
        let roaming = dir.join("AppData/Roaming");
        let user = roaming.join("Code/User");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join("settings.json"), "{}").unwrap();
        assert_eq!(user_dir_in(Some(&roaming), None), Some(user));
        let codium = roaming.join("VSCodium/User");
        std::fs::create_dir_all(&codium).unwrap();
        std::fs::write(codium.join("keybindings.json"), "[]").unwrap();
        assert_eq!(user_dir_in(Some(&roaming), None).unwrap(), roaming.join("Code/User"));
        std::fs::remove_file(roaming.join("Code/User/settings.json")).unwrap();
        assert_eq!(user_dir_in(Some(&roaming), None), Some(codium));

        let install = dir.join("Local/Programs/Microsoft VS Code");
        let extension = install.join("resources/app/extensions/theme-test");
        std::fs::create_dir_all(&extension).unwrap();
        std::fs::write(extension.join("package.json"), r#"{"contributes":{"themes":[{"label":"Windows Fixture","path":"theme.json"}]}}"#).unwrap();
        std::fs::write(extension.join("theme.json"), r##"{"colors":{"editor.background":"#123456"}}"##).unwrap();
        let roots = theme_roots(None, &[install]);
        let theme = find_theme_in(&roots, "Windows Fixture").unwrap();
        assert_eq!(theme["colors"]["editor.background"], "#123456");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn converts_settings() {
        let vs = r#"{
            // yorum
            "editor.fontSize": 15, "editor.fontFamily": "'JetBrains Mono', Menlo, monospace",
            "editor.lineHeight": 1.5, "files.autoSave": "onWindowChange", "files.exclude": { "**/.git": true, "**/node_modules": false },
            "workbench.colorTheme": "Studio Dark", "editor.autoClosingBrackets": "never", "gitlens.ai.model": "x",
            "terminal.integrated.fontSize": 12,
        }"#;
        let themes = vec![("studio-dark".to_string(), "Studio Dark".to_string())];
        let c = convert_settings(vs, &themes);
        let get = |k: &str| c.settings.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("editor.fontSize"), Some(json!(15)));
        assert_eq!(get("editor.fontFamily"), Some(json!("JetBrains Mono")));
        assert_eq!(get("editor.lineHeight"), Some(json!(23.0)), "1.5 × 15");
        assert_eq!(get("files.autoSave"), Some(json!("onFocusChange")));
        assert_eq!(get("files.exclude"), Some(json!(["**/.git"])));
        assert_eq!(get("theme"), Some(json!("studio-dark")));
        assert_eq!(get("editor.autoClosingBrackets"), Some(json!(false)));
        assert_eq!(get("terminal.fontSize"), Some(json!(12)));
        assert_eq!(c.skipped, 1, "gitlens");
        let c = convert_settings(r#"{"workbench.colorTheme": "Monokai"}"#, &themes);
        assert!(c.settings.is_empty() && c.notes[0].contains("Monokai"));
    }

    #[test]
    fn converts_keybindings() {
        let vs = r#"// yorum
        [
          { "key": "cmd+shift+[BracketRight]", "command": "python.execInTerminal" },
          { "key": "ctrl+shift+k", "command": "editor.action.deleteLines", "when": "editorTextFocus && !editorReadonly" },
          { "key": "cmd+k cmd+c", "command": "editor.action.commentLine" },
          { "key": "cmd+d", "command": "-editor.action.addSelectionToNextFindMatch", "when": "editorFocus" },
          { "key": "f5", "command": "workbench.action.debug.start" },
          { "key": "cmd+j", "command": "workbench.action.togglePanel", "when": "notebookEditorFocused" },
        ]"#;
        let (entries, skipped) = convert_keybindings(vs);
        assert_eq!(skipped, 3, "scan code, unknown command, unknown condition");
        assert_eq!(entries[0], json!({ "key": "ctrl-shift-k", "command": "editor.deleteLines", "when": "Editor" }));
        assert_eq!(entries[1], json!({ "key": "cmd-k cmd-c", "command": "editor.toggleComment" }));
        assert_eq!(entries[2], json!({ "key": "cmd-d", "command": null, "when": "Editor" }), "removal");

        // Into the codeyard file: its own entries are kept, a re-import replaces the block.
        let ours = "[\n  // benim\n  { \"key\": \"cmd-1\", \"command\": \"workbench.quickOpen\" }\n]\n";
        let once = merge_keybindings(ours, &entries);
        let twice = merge_keybindings(&once, &entries[..1]);
        assert!(once.contains("// benim") && once.contains("editor.toggleComment"));
        assert!(!twice.contains("editor.toggleComment") && twice.contains("editor.deleteLines"));
        let (parsed, warnings) = crate::settings::parse_keybindings(&twice);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn finds_installed_theme_with_include() {
        let root = std::env::temp_dir().join(format!("codeyard-vsext-{}", std::process::id()));
        let ext = root.join("acme.tema-1.0.0");
        std::fs::create_dir_all(ext.join("themes")).unwrap();
        std::fs::write(ext.join("package.json"), r#"{"contributes":{"themes":[{"label":"%t%","uiTheme":"vs-dark","path":"./themes/kat.json"}]}}"#).unwrap();
        std::fs::write(ext.join("package.nls.json"), r#"{"t":"Gece Mavisi"}"#).unwrap();
        std::fs::write(ext.join("themes/taban.json"), r##"{"colors":{"editor.background":"#000000","editor.foreground":"#cccccc"},"tokenColors":[{"scope":"comment","settings":{"foreground":"#666666"}}]}"##).unwrap();
        std::fs::write(
            ext.join("themes/kat.json"),
            r##"{ // yorum
            "include": "./taban.json", "colors": { "editor.background": "#101830" } }"##,
        )
        .unwrap();
        let t = find_theme_in(std::slice::from_ref(&root), "Default Gece Mavisi").expect("theme should be found");
        assert_eq!(t["colors"]["editor.background"], "#101830", "the layer overrides the base");
        assert_eq!(t["colors"]["editor.foreground"], "#cccccc", "taban rengi miras");
        assert_eq!(t["tokenColors"].as_array().unwrap().len(), 1);
        assert_eq!((t["name"].as_str(), t["type"].as_str()), (Some("Default Gece Mavisi"), Some("dark")));
        // Our own importer must be able to read it.
        let dir = root.join("ours");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("vscode-gece.json"), t.to_string()).unwrap();
        let (themes, errors) = theme::themes::load_dir(&dir);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(themes[0].name, "Default Gece Mavisi");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Dry run against real VS Code files, read-only (run by hand):
/// `cargo test -p app real_vscode_files -- --ignored --nocapture`
#[cfg(test)]
#[test]
#[ignore = "reads the developer's real VS Code settings; manual dry run, prints a report, asserts nothing"]
fn real_vscode_files() {
    let Some(dir) = user_dir() else {
        eprintln!("real_vscode_files: no VS Code user directory on this machine, nothing was checked");
        return;
    };
    if let Ok(t) = std::fs::read_to_string(dir.join("settings.json")) {
        let c = convert_settings(&t, &[]);
        eprintln!("settings: {} imported, {} unmapped; notes: {:?}", c.settings.len(), c.skipped, c.notes);
        eprintln!("imported keys: {:?}", c.settings.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>());
    }
    if let Ok(t) = std::fs::read_to_string(dir.join("keybindings.json")) {
        let (e, skipped) = convert_keybindings(&t);
        eprintln!("shortcuts: {} imported, {} unmapped", e.len(), skipped);
        eprintln!("imported: {:?}", e.iter().map(|x| format!("{} → {}", x["key"], x["command"])).collect::<Vec<_>>());
    }
}
