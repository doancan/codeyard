//! One substitution pass over the original template. Input/command results are never recursively interpreted.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use serde_json::Value;

use crate::model::Platform;

const MAX_OUTPUT: usize = 256 * 1024;
const MAX_REFERENCES: usize = 256;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Nested,
    MissingFile,
    Missing(String),
    Invalid(String),
}

pub struct Context<'a> {
    pub platform: Platform,
    pub root: &'a str,
    pub folders: &'a BTreeMap<String, String>,
    pub file: Option<&'a str>,
    pub cursor: Option<(u32, u32)>,
    pub selected: Option<&'a str>,
    pub cwd: &'a str,
    pub home: &'a str,
    pub executable: &'a str,
    pub default_build: Option<&'a str>,
    /// Environment from the file host, never implicitly from the editor process.
    pub environment: &'a BTreeMap<String, String>,
    pub config: &'a BTreeMap<String, Value>,
    /// Values resolved once by the caller under the workspace's trust and input UI.
    pub results: &'a BTreeMap<String, String>,
}

fn known(key: &str) -> bool {
    key.starts_with("env:")
        || key.starts_with("config:")
        || key.starts_with("input:")
        || key.starts_with("command:")
        || key.starts_with("workspaceFolder:")
        || key.starts_with("workspaceFolderBasename:")
        || matches!(
            key,
            "workspaceFolder"
                | "workspaceFolderBasename"
                | "file"
                | "fileWorkspaceFolder"
                | "fileBasename"
                | "fileBasenameNoExtension"
                | "fileExtname"
                | "fileDirname"
                | "fileDirnameBasename"
                | "relativeFile"
                | "relativeFileDirname"
                | "lineNumber"
                | "columnNumber"
                | "selectedText"
                | "cwd"
                | "userHome"
                | "execPath"
                | "pathSeparator"
                | "/"
                | "defaultBuildTask"
        )
}

fn tokens(template: &str) -> Result<Vec<(Range<usize>, &str)>, Error> {
    if template.len() > MAX_OUTPUT {
        return Err(Error::Limit);
    }
    let mut tokens = Vec::new();
    let mut offset = 0;
    while let Some(start) = template[offset..].find("${") {
        let start = offset + start;
        let Some(end) = template[start + 2..].find('}') else {
            break;
        };
        let end = start + 2 + end;
        let key = &template[start + 2..end];
        if key.contains("${") {
            return Err(Error::Nested);
        }
        if known(key) {
            if key.len() > 512 || tokens.len() >= MAX_REFERENCES {
                return Err(Error::Limit);
            }
            tokens.push((start..end + 1, key));
        }
        offset = end + 1;
    }
    Ok(tokens)
}

pub fn references(template: &str) -> Result<BTreeSet<&str>, Error> {
    Ok(tokens(template)?.into_iter().map(|(_, key)| key).collect())
}

/// Raw string substitution. Process argv remains literal; shell escaping belongs to the shell compiler.
pub fn expand(template: &str, context: &Context<'_>) -> Result<String, Error> {
    let mut out = String::new();
    let mut offset = 0;
    for (range, key) in tokens(template)? {
        let value = context.value(key)?;
        if value.contains('\0') {
            return Err(Error::Invalid(key.into()));
        }
        if out.len() + range.start - offset + value.len() > MAX_OUTPUT {
            return Err(Error::Limit);
        }
        out.push_str(&template[offset..range.start]);
        out.push_str(&value);
        offset = range.end;
    }
    if out.len() + template.len() - offset > MAX_OUTPUT {
        return Err(Error::Limit);
    }
    out.push_str(&template[offset..]);
    Ok(out)
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}
fn dirname(path: &str) -> &str {
    path.rsplit_once('/').map_or(".", |(dir, _)| if dir.is_empty() { "/" } else { dir })
}

impl Context<'_> {
    fn path(&self, path: &str) -> String {
        if self.platform == Platform::Windows { path.replace('\\', "/") } else { path.into() }
    }
    fn relative<'a>(&self, file: &'a str, root: &str) -> Option<&'a str> {
        let root = root.trim_end_matches('/');
        let prefix = file.get(..root.len())?;
        let equal = if self.platform == Platform::Windows { prefix.eq_ignore_ascii_case(root) } else { prefix == root };
        if equal && file.as_bytes().get(root.len()) == Some(&b'/') { file.get(root.len() + 1..) } else { None }
    }
    pub fn value(&self, key: &str) -> Result<String, Error> {
        let missing = || Error::Missing(key.into());
        if let Some(name) = key.strip_prefix("env:") {
            return Ok(self.environment.get(name).cloned().unwrap_or_default());
        }
        if let Some(name) = key.strip_prefix("config:") {
            return match self.config.get(name).ok_or_else(missing)? {
                Value::String(value) => Ok(value.clone()),
                value @ (Value::Bool(_) | Value::Number(_)) => Ok(value.to_string()),
                _ => Err(Error::Invalid(key.into())),
            };
        }
        if key.starts_with("input:") || key.starts_with("command:") {
            return self.results.get(key).cloned().ok_or_else(missing);
        }
        if let Some(name) = key.strip_prefix("workspaceFolder:") {
            return self.folders.get(name).cloned().ok_or_else(missing);
        }
        if let Some(name) = key.strip_prefix("workspaceFolderBasename:") {
            return self.folders.get(name).map(|path| basename(&self.path(path)).to_string()).ok_or_else(missing);
        }
        match key {
            "workspaceFolder" => return Ok(self.root.into()),
            "workspaceFolderBasename" => return Ok(basename(&self.path(self.root)).into()),
            "cwd" => return Ok(self.cwd.into()),
            "userHome" => return Ok(self.home.into()),
            "execPath" => return Ok(self.executable.into()),
            "pathSeparator" | "/" => return Ok(if self.platform == Platform::Windows { "\\" } else { "/" }.into()),
            "defaultBuildTask" => return self.default_build.map(str::to_string).ok_or_else(missing),
            _ => {}
        }
        let file = self.file.ok_or(Error::MissingFile)?;
        let normalized = self.path(file);
        let name = basename(&normalized);
        let extension = name.rfind('.').filter(|&dot| dot > 0);
        let root = self.path(self.root);
        Ok(match key {
            "file" => file.into(),
            "fileBasename" => name.into(),
            "fileBasenameNoExtension" => extension.map_or(name, |dot| &name[..dot]).into(),
            "fileExtname" => extension.map_or("", |dot| &name[dot..]).into(),
            "fileDirname" => dirname(&normalized).into(),
            "fileDirnameBasename" => basename(dirname(&normalized)).into(),
            "relativeFile" => self.relative(&normalized, &root).ok_or_else(missing)?.into(),
            "relativeFileDirname" => dirname(self.relative(&normalized, &root).ok_or_else(missing)?).into(),
            "fileWorkspaceFolder" => self
                .folders
                .values()
                .map(|root| self.path(root))
                .chain(std::iter::once(root))
                .filter(|root| self.relative(&normalized, root).is_some())
                .max_by_key(String::len)
                .ok_or_else(missing)?,
            "lineNumber" => self.cursor.ok_or_else(missing)?.0.saturating_add(1).to_string(),
            "columnNumber" => self.cursor.ok_or_else(missing)?.1.saturating_add(1).to_string(),
            "selectedText" => self.selected.unwrap_or_default().into(),
            _ => return Err(missing()),
        })
    }
}

#[cfg(test)]
#[path = "variables_tests.rs"]
mod tests;
