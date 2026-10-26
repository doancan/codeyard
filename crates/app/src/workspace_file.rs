//! VS Code `.code-workspace` files. Codeyard opens one root folder per window
//! (git, search, language servers and session saving assume it), so a multi-folder
//! workspace opens as one window per folder. Plain text only.

use serde_json::Value;

/// `folders[].path` or `folders[].uri` (file://) → absolute, normalized folder paths;
/// relative paths are resolved against the file's folder, duplicates dropped, order kept.
/// Folders that don't exist are filtered out with `exists` (`Path::is_dir` on the local disk).
pub fn folders(text: &str, base_dir: &str, exists: impl Fn(&str) -> bool) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(&theme::themes::strip_jsonc(text)) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for f in v["folders"].as_array().into_iter().flatten() {
        let raw = match (f["path"].as_str(), f["uri"].as_str()) {
            (Some(p), _) => p.to_string(),
            (None, Some(u)) => match schema::file_uri::to_path(u) {
                Some(p) => p,
                None => continue, // like vscode-remote://: unsupported
            },
            _ => continue,
        };
        if raw.contains('\0') {
            continue;
        }
        let windows_base = windows_path(base_dir);
        let raw = if windows_base || windows_path(&raw) { raw.replace('\\', "/") } else { raw };
        let base = if windows_base { base_dir.replace('\\', "/") } else { base_dir.to_string() };
        let abs = normalize(&crate::ui::join(&base, &raw));
        if exists(&abs) && !out.contains(&abs) {
            out.push(abs);
        }
    }
    out
}

fn normalize(path: &str) -> String {
    let path = if let Some(rest) = path.strip_prefix("//?/UNC/") { format!("//{rest}") } else { path.strip_prefix("//?/").unwrap_or(path).to_string() };
    let drive = path.len() >= 3 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1..3] == *b":/";
    let unc = path.starts_with("//");
    let (prefix, rest, floor) = if drive {
        (format!("{}/", &path[..2]), &path[3..], 0)
    } else if unc {
        ("//".to_string(), &path[2..], 2)
    } else {
        ("/".to_string(), path.as_str(), 0)
    };
    let mut parts: Vec<&str> = Vec::new();
    for p in rest.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.len() > floor {
                    parts.pop();
                }
            }
            p => parts.push(p),
        }
    }
    format!("{prefix}{}", parts.join("/"))
}

fn windows_path(path: &str) -> bool {
    path.starts_with(['\\']) || path.starts_with("//") || path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':'
}

pub fn is_workspace_file(path: &str) -> bool {
    path.ends_with(".code-workspace")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_absolute_and_uri_folders() {
        let text = r#"{
            // yorum
            "folders": [
                { "path": "api" },
                { "path": "../web", "name": "Web" },
                { "path": "/opt/tools" },
                { "uri": "file:///srv/my%20app" },
                { "uri": "vscode-remote://ssh-remote+box/home/x" },
                { "path": "./api/" },
                { "name": "no path" },
                { "path": "gone" }
            ],
            "settings": { "editor.tabSize": 2 }
        }"#;
        let all = |p: &str| !p.ends_with("/gone");
        assert_eq!(folders(text, "/work/mono", all), ["/work/mono/api", "/work/web", "/opt/tools", "/srv/my app"]);
        assert!(folders("{ not json", "/w", all).is_empty());
        assert!(folders("{}", "/w", all).is_empty());
    }

    #[test]
    fn recognises_workspace_files() {
        assert!(is_workspace_file("/a/b.code-workspace"));
        assert!(!is_workspace_file("/a/b.json"));
    }

    #[test]
    fn windows_workspace_folders_resolve_drives_unc_and_backslashes() {
        let text = serde_json::json!({ "folders": [
            { "path": r"src\nested\.." }, { "path": r"..\web" },
            { "path": r"D:\tools\..\sdk" }, { "uri": "file:///E:/my%20app" },
            { "path": r"\\server\share\dir\..\..\other" },
            { "uri": "file://server/share/other" }, { "uri": "file://localhost/F:/work" },
            { "path": "C:/../../" }, { "path": r"\\?\G:\long\dir" },
            { "path": r"\\?\UNC\host\share\dir" },
            { "uri": "file:///bad%XX" }, { "uri": "file:///a%00b" }
        ] })
        .to_string();
        assert_eq!(
            folders(&text, r"C:\work\mono", |_| true),
            ["C:/work/mono/src", "C:/work/web", "D:/sdk", "E:/my app", "//server/share/other", "F:/work", "C:/", "G:/long/dir", "//host/share/dir"]
        );
        assert_eq!(folders(r#"{"folders":[{"path":"../api"}]}"#, r"\\server\share\work", |_| true), ["//server/share/api"]);
    }
}
