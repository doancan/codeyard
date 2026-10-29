//! Validated custom and built-in problem patterns, compiled before any command starts.

use std::collections::BTreeMap;

use fancy_regex::{Regex, RegexBuilder};
use serde_json::{Value, json};

#[path = "matcher_state.rs"]
mod state;
pub use state::{Problem, Update};

const FIELDS: [&str; 9] = ["file", "line", "column", "endLine", "endColumn", "location", "severity", "message", "code"];

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Invalid(&'static str),
    Unknown(String),
    Regex(String),
    Runtime,
}

/// Share one budget across all distinct matchers in a dependency plan, not one per task.
pub struct Limits {
    patterns: usize,
    text: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { patterns: 128, text: 64 * 1024 }
    }
}

fn regex(value: &Value, limits: &mut Limits) -> Result<Regex, Error> {
    let text = value.as_str().ok_or(Error::Invalid("regexp"))?;
    if text.len() > 4096 || limits.patterns == 0 || limits.text < text.len() {
        return Err(Error::Limit);
    }
    limits.patterns -= 1;
    limits.text -= text.len();
    RegexBuilder::new(text)
        .backtrack_limit(100_000)
        .delegate_size_limit(1024 * 1024)
        .delegate_dfa_size_limit(128 * 1024)
        .build()
        .map_err(|error| Error::Regex(error.to_string().chars().take(256).collect()))
}

fn boolean(value: &Value, field: &'static str) -> Result<bool, Error> {
    if value.is_null() { Ok(false) } else { value.as_bool().ok_or(Error::Invalid(field)) }
}
fn string(value: &Value, default: &str, field: &'static str) -> Result<String, Error> {
    if value.is_null() {
        return Ok(default.into());
    }
    value.as_str().filter(|value| value.len() <= 4096 && !value.contains('\0')).map(str::to_string).ok_or(Error::Invalid(field))
}
fn index(value: &Value, len: usize) -> Result<Option<usize>, Error> {
    if value.is_null() {
        return Ok(None);
    }
    value.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|&n| n < len).map(Some).ok_or(Error::Invalid("capture index"))
}

struct Pattern {
    regex: Regex,
    fields: BTreeMap<&'static str, usize>,
    repeat: bool,
}
struct WatchPattern {
    regex: Regex,
    file: Option<usize>,
}
impl WatchPattern {
    fn parse(value: &Value, limits: &mut Limits) -> Result<Self, Error> {
        let pattern = regex(if value.is_string() { value } else { &value["regexp"] }, limits)?;
        let file = if value.is_object() { index(&value["file"], pattern.captures_len())? } else { None };
        Ok(Self { regex: pattern, file })
    }
}
struct Watch {
    begin: Option<WatchPattern>,
    end: WatchPattern,
    active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileLocation {
    Absolute,
    Relative(String),
    Auto(String),
    Search { include: Vec<String>, exclude: Vec<String> },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyTo {
    All,
    Open,
    Closed,
}

pub struct Matcher {
    pub owner: String,
    pub source: String,
    pub location: FileLocation,
    pub apply_to: ApplyTo,
    patterns: Vec<Pattern>,
    file_kind: bool,
    severity: u8,
    watch: Option<Watch>,
    state: state::State,
}

fn paths(value: &Value) -> Result<Vec<String>, Error> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let values = value.as_array().filter(|items| items.len() <= 32).ok_or(Error::Invalid("fileLocation search paths"))?;
    values.iter().map(|value| string(value, "", "fileLocation search path")).collect()
}
fn location(value: &Value) -> Result<FileLocation, Error> {
    if value.is_null() {
        return Ok(FileLocation::Relative("${workspaceFolder}".into()));
    }
    let kind = value.as_str().or_else(|| value[0].as_str()).ok_or(Error::Invalid("fileLocation"))?;
    let root = || string(if value.is_array() { &value[1] } else { &Value::Null }, "${workspaceFolder}", "fileLocation root");
    match kind {
        "absolute" => Ok(FileLocation::Absolute),
        "relative" => Ok(FileLocation::Relative(root()?)),
        "autoDetect" | "autodetect" => Ok(FileLocation::Auto(root()?)),
        "search" => Ok(FileLocation::Search { include: paths(&value[1]["include"])?, exclude: paths(&value[1]["exclude"])? }),
        _ => Err(Error::Invalid("fileLocation")),
    }
}

fn builtin(name: &str) -> Result<Value, Error> {
    let tsc = json!({"owner":"typescript","source":"tsc","pattern":{"regexp":r"^(.*?)\((\d+)(?:,(\d+))?\):\s*(error|warning|info)\s+(TS\d+):\s*(.*)$","file":1,"line":2,"column":3,"severity":4,"code":5,"message":6}});
    Ok(match name {
        "$tsc" => tsc,
        "$tsc-watch" => {
            let mut value = tsc;
            value["background"] = json!({"activeOnStart":true,"beginsPattern":r"(?:Starting compilation|File change detected).*","endsPattern":r"(?:Found \d+ errors?\. Watching for file changes\.|Compilation complete\. Watching for file changes\.)"});
            value
        }
        "$gcc" | "$go" => {
            json!({"pattern":{"regexp":r"^(.*?):(\d+):(?:(\d+):)?\s*(?:(error|warning|info):\s*)?(.*)$","file":1,"line":2,"column":3,"severity":4,"message":5}})
        }
        "$msCompile" | "$mscompile" => {
            json!({"fileLocation":"absolute","pattern":{"regexp":r"^\s*(.*?)\((\d+)(?:,(\d+))?\)\s*:\s*(error|warning|info)\s+([^: ]+)\s*:\s*(.*)$","file":1,"line":2,"column":3,"severity":4,"code":5,"message":6}})
        }
        "$eslint-stylish" => {
            json!({"owner":"javascript","source":"eslint","pattern":[{"regexp":r"^([^\s].*)$","file":1},{"regexp":r"^\s+(\d+):(\d+)\s+(error|warning|info)\s+(.*?)\s{2,}(\S+)\s*$","line":1,"column":2,"severity":3,"message":4,"code":5,"loop":true}]})
        }
        "$eslint-compact" => {
            json!({"owner":"javascript","source":"eslint","pattern":{"regexp":r"^(.*?): line (\d+), col (\d+), (Error|Warning) - (.*)$","file":1,"line":2,"column":3,"severity":4,"message":5}})
        }
        "$rustc" => {
            json!({"source":"rustc","pattern":[{"regexp":r"^(error|warning)(?:\[([^\]]+)\])?:\s*(.*)$","severity":1,"code":2,"message":3},{"regexp":r"^\s*-->\s*(.*?):(\d+):(\d+)\s*$","file":1,"line":2,"column":3}]})
        }
        _ => return Err(Error::Unknown(name.into())),
    })
}

impl Matcher {
    pub fn compile(spec: &Value, limits: &mut Limits) -> Result<Self, Error> {
        let value = if let Some(name) = spec.as_str() {
            builtin(name)?
        } else if spec.is_object() {
            if !spec["base"].is_null() && !spec["base"].is_string() {
                return Err(Error::Invalid("base"));
            }
            if let Some(name) = spec["base"].as_str() {
                let mut base = builtin(name)?;
                for (key, value) in spec.as_object().ok_or(Error::Invalid("matcher"))? {
                    base[key] = value.clone();
                }
                base
            } else {
                spec.clone()
            }
        } else {
            return Err(Error::Invalid("matcher"));
        };
        let owner = string(&value["owner"], "external", "owner")?;
        let source = string(&value["source"], &owner, "source")?;
        let location = location(&value["fileLocation"])?;
        let apply_to = match value["applyTo"].as_str() {
            None if value["applyTo"].is_null() => ApplyTo::All,
            Some("allDocuments") => ApplyTo::All,
            Some("openDocuments") => ApplyTo::Open,
            Some("closedDocuments") => ApplyTo::Closed,
            _ => return Err(Error::Invalid("applyTo")),
        };
        let severity = match value["severity"].as_str() {
            None if value["severity"].is_null() => 1,
            Some("error") => 1,
            Some("warning") => 2,
            Some("info") => 3,
            _ => return Err(Error::Invalid("severity")),
        };
        let pattern = if let Some(name) = value["pattern"].as_str() { builtin(name)?["pattern"].clone() } else { value["pattern"].clone() };
        let values = if pattern.is_null() {
            Vec::new()
        } else if let Some(values) = pattern.as_array() {
            values.clone()
        } else {
            vec![pattern]
        };
        if values.len() > 16 {
            return Err(Error::Limit);
        }
        let mut patterns = Vec::new();
        let mut fields = BTreeMap::new();
        let mut file_kind = false;
        for (position, value) in values.iter().enumerate() {
            if !value.is_object() {
                return Err(Error::Invalid("pattern"));
            }
            match value["kind"].as_str() {
                None if value["kind"].is_null() => {}
                Some("file") => file_kind = true,
                Some("location") => {}
                _ => return Err(Error::Invalid("kind")),
            }
            let regex = regex(&value["regexp"], limits)?;
            let mut captures = BTreeMap::new();
            for field in FIELDS {
                if let Some(index) = index(&value[field], regex.captures_len())? {
                    captures.insert(field, index);
                    fields.insert(field, ());
                }
            }
            let repeat = boolean(&value["loop"], "loop")?;
            if repeat && (position + 1 != values.len() || position == 0) {
                return Err(Error::Invalid("loop"));
            }
            patterns.push(Pattern { regex, fields: captures, repeat });
        }
        if !patterns.is_empty()
            && (!fields.contains_key("file")
                || !fields.contains_key("message")
                || !file_kind && !fields.contains_key("line") && !fields.contains_key("location"))
        {
            return Err(Error::Invalid("required capture"));
        }
        let watch = if value["background"].is_null() {
            None
        } else {
            let config = &value["background"];
            if !config.is_object() {
                return Err(Error::Invalid("background"));
            }
            let begin = if config["beginsPattern"].is_null() { None } else { Some(WatchPattern::parse(&config["beginsPattern"], limits)?) };
            Some(Watch { begin, end: WatchPattern::parse(&config["endsPattern"], limits)?, active: boolean(&config["activeOnStart"], "activeOnStart")? })
        };
        if patterns.is_empty() && watch.is_none() {
            return Err(Error::Invalid("pattern"));
        }
        let state = state::State::new(watch.as_ref());
        Ok(Self { owner, source, location, apply_to, patterns, file_kind, severity, watch, state })
    }
}

#[cfg(test)]
#[path = "matcher_tests.rs"]
mod tests;
