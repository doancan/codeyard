//! File nesting in the explorer (`explorer.fileNesting.*`): a file such as `Cargo.lock` is listed under the file it belongs to
//! (`Cargo.toml`). The rules are the same shape as VS Code's: a parent pattern (an exact name or one with a single `*`) and a
//! list of child names, which may use `${capture}` for the text the `*` matched. Pure: it works on the names of one folder.

use proto::DirEntry;
use serde_json::Value;
use std::collections::HashMap;

/// The rules a profile starts with: `(parent pattern, comma-separated children)`.
pub const DEFAULT_PATTERNS: &[(&str, &str)] = &[
    ("*.ts", "${capture}.test.ts, ${capture}.spec.ts, ${capture}.d.ts, ${capture}.js, ${capture}.js.map"),
    ("*.tsx", "${capture}.test.tsx, ${capture}.spec.tsx, ${capture}.module.css"),
    ("*.js", "${capture}.js.map, ${capture}.min.js, ${capture}.d.ts"),
    ("*.scss", "${capture}.css, ${capture}.css.map"),
    ("Cargo.toml", "Cargo.lock"),
    ("package.json", "package-lock.json, npm-shrinkwrap.json, yarn.lock, pnpm-lock.yaml, bun.lock, bun.lockb"),
    ("go.mod", "go.sum"),
    ("pyproject.toml", "uv.lock, poetry.lock, pdm.lock"),
    ("Gemfile", "Gemfile.lock"),
    ("composer.json", "composer.lock"),
];

/// The most children one file shows; the rest stay in the list on their own.
const MAX_CHILDREN: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    /// The key as written in the setting.
    pattern: String,
    /// Text before and after the `*`; without a `*` the name must equal `prefix`.
    prefix: String,
    suffix: Option<String>,
    children: Vec<String>,
}

impl Rule {
    /// The text the `*` matched (the whole name for an exact pattern); `None` when `name` does not fit.
    fn capture<'a>(&self, name: &'a str) -> Option<&'a str> {
        let Some(suffix) = &self.suffix else { return (name == self.prefix).then_some(name) };
        let middle = name.strip_prefix(self.prefix.as_str())?.strip_suffix(suffix.as_str())?;
        (!middle.is_empty()).then_some(middle)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nesting {
    pub enabled: bool,
    rules: Vec<Rule>,
}

impl Default for Nesting {
    fn default() -> Self {
        let rules = DEFAULT_PATTERNS.iter().filter_map(|(parent, children)| rule(parent, split_children(children))).collect();
        Nesting { enabled: true, rules }
    }
}

fn split_children(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect()
}

/// A rule for a parent pattern, or `None` when the pattern cannot match anything (blank, more than one `*`) or has no children.
fn rule(parent: &str, children: Vec<String>) -> Option<Rule> {
    let parent = parent.trim();
    if children.is_empty() || parent.is_empty() || parent.matches('*').count() > 1 {
        return None;
    }
    let (prefix, suffix) = match parent.split_once('*') {
        Some((before, after)) => (before.to_string(), Some(after.to_string())),
        None => (parent.to_string(), None),
    };
    Some(Rule { pattern: parent.to_string(), prefix, suffix, children })
}

/// What a row of the tree is, as far as nesting goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    None,
    /// Holds this many files.
    Parent(usize),
    Child,
}

/// What a folder's entries are grouped into: for each parent entry its children, all as positions in the entries given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Groups {
    pub children: HashMap<usize, Vec<usize>>,
    pub is_child: Vec<bool>,
}

impl Nesting {
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    #[cfg(test)]
    pub fn off() -> Self {
        Nesting { enabled: false, rules: Vec::new() }
    }

    /// Rules from the setting's value, on top of the defaults: an object of parent pattern -> child names (a comma-separated
    /// text, or a list of texts). An entry replaces the default of the same pattern, `false` or an empty list removes it.
    /// Returns the rules and a warning for each entry that is ignored. The order of the entries decides who is the parent when
    /// two rules claim the same file: defaults first, new patterns after them.
    pub fn from_setting(enabled: bool, value: &Value) -> (Nesting, Vec<String>) {
        let mut nesting = Nesting::default().with_enabled(enabled);
        let Value::Object(map) = value else { return (nesting, vec![crate::i18n::t("expected an object of patterns").to_string()]) };
        let mut warnings = Vec::new();
        for (parent, children) in map {
            let list = match children {
                Value::Bool(false) | Value::Null => Vec::new(),
                Value::String(text) => split_children(text),
                Value::Array(items) if items.iter().all(Value::is_string) => items.iter().filter_map(Value::as_str).flat_map(split_children).collect(),
                _ => {
                    warnings.push(crate::i18n::tf("{}: expected text or a list of text", &[parent]));
                    continue;
                }
            };
            if parent.matches('*').count() > 1 {
                warnings.push(crate::i18n::tf("{}: a pattern may hold one *", &[parent]));
                continue;
            }
            let at = nesting.rules.iter().position(|r| r.pattern == parent.trim());
            match (at, rule(parent, list)) {
                (Some(at), Some(r)) => nesting.rules[at] = r,
                (Some(at), None) => {
                    nesting.rules.remove(at);
                }
                (None, Some(r)) => nesting.rules.push(r),
                (None, None) => {}
            }
        }
        (nesting, warnings)
    }

    /// Group the entries of one folder. Folders (and links to folders) never nest or hold children; a file is the child of at
    /// most one parent, the first by rule order, and a file that is a parent or a child stays out of the other role.
    pub fn group(&self, entries: &[DirEntry]) -> Groups {
        let mut groups = Groups { children: HashMap::new(), is_child: vec![false; entries.len()] };
        if !self.enabled || self.rules.is_empty() {
            return groups;
        }
        let files: HashMap<&str, usize> = entries.iter().enumerate().filter(|(_, e)| !e.is_dir()).map(|(i, e)| (e.name.as_str(), i)).collect();
        for rule in &self.rules {
            for (parent, entry) in entries.iter().enumerate() {
                if entry.is_dir() || groups.is_child[parent] {
                    continue;
                }
                let Some(capture) = rule.capture(&entry.name) else { continue };
                for template in &rule.children {
                    let name = template.replace("${capture}", capture).replace("$(capture)", capture);
                    let Some(&child) = files.get(name.as_str()) else { continue };
                    let known = groups.children.get(&parent).map_or(0, Vec::len);
                    if child == parent || groups.is_child[child] || groups.children.contains_key(&child) || known >= MAX_CHILDREN {
                        continue;
                    }
                    groups.is_child[child] = true;
                    groups.children.entry(parent).or_default().push(child);
                }
            }
        }
        for kids in groups.children.values_mut() {
            kids.sort_unstable();
        }
        groups
    }
}

#[cfg(test)]
#[path = "tree_nesting_tests.rs"]
mod tests;
