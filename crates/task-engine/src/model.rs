//! Bounded task definitions and dependency plans. Planning never runs commands or requests input.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Map, Value};

const MAX_FILE: usize = 2 * 1024 * 1024;
const MAX_TASKS: usize = 256;
const MAX_DEPS: usize = 32;
const MAX_INPUTS: usize = 64;
const MAX_EXPANDED: usize = 8 * 1024 * 1024;

// Count the serialized shape without allocating another copy. Inherited defaults
// can otherwise multiply a small file into hundreds of megabytes of task state.
struct Budget(usize);
impl std::io::Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.checked_sub(bytes.len()).ok_or_else(|| std::io::Error::other("expanded task limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Mac,
    Windows,
}
impl Platform {
    fn key(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Mac => "osx",
            Self::Windows => "windows",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Shell,
    Process,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Parallel,
    Sequence,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quoting {
    Strong,
    Weak,
    Escape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Argument {
    pub value: String,
    pub quoting: Option<Quoting>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub cwd: Option<String>,
    pub env: BTreeMap<String, Option<String>>,
    pub shell: Option<(String, Vec<String>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub label: String,
    pub kind: Kind,
    pub command: Option<Argument>,
    pub args: Vec<Argument>,
    pub options: Options,
    pub depends: Vec<String>,
    pub order: Order,
    pub background: bool,
    pub matchers: Vec<Value>,
    pub auto_match: bool,
    pub group: Option<(String, bool)>,
    pub instance_limit: usize,
    pub reevaluate: bool,
    pub on_open: bool,
    pub presentation: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Prompt { description: String, default: Option<String>, password: bool },
    Pick { description: String, options: Vec<(String, String)>, default: Option<String> },
    Command { command: String, args: Value },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Json,
    Invalid { task: String, field: String },
    Duplicate(String),
    Missing { task: String, dependency: String },
    Cycle(Vec<String>),
    Readiness(String),
}

fn invalid(task: &str, field: &str) -> Error {
    Error::Invalid { task: task.into(), field: field.into() }
}
fn text(value: &Value, task: &str, field: &str) -> Result<String, Error> {
    value.as_str().filter(|s| s.len() <= 65536 && !s.contains('\0')).map(str::to_string).ok_or_else(|| invalid(task, field))
}
fn optional_text(value: &Value, task: &str, field: &str) -> Result<Option<String>, Error> {
    if value.is_null() { Ok(None) } else { text(value, task, field).map(Some) }
}
fn boolean(value: &Value, default: bool, task: &str, field: &str) -> Result<bool, Error> {
    if value.is_null() { Ok(default) } else { value.as_bool().ok_or_else(|| invalid(task, field)) }
}
fn strings(value: &Value, task: &str, field: &str, max: usize) -> Result<Vec<String>, Error> {
    let Some(values) = value.as_array() else { return if value.is_null() { Ok(Vec::new()) } else { Err(invalid(task, field)) } };
    if values.len() > max {
        return Err(Error::Limit);
    }
    values.iter().map(|v| text(v, task, field)).collect()
}
fn argument(value: &Value, task: &str) -> Result<Argument, Error> {
    if value.is_string() {
        return Ok(Argument { value: text(value, task, "command/args")?, quoting: None });
    }
    let quoting = match value["quoting"].as_str() {
        Some("strong") => Quoting::Strong,
        Some("weak") => Quoting::Weak,
        Some("escape") => Quoting::Escape,
        _ => return Err(invalid(task, "quoting")),
    };
    Ok(Argument { value: text(&value["value"], task, "command/args.value")?, quoting: Some(quoting) })
}

fn merge(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                if let Some(previous) = base.get_mut(key) {
                    merge(previous, value);
                } else {
                    base.insert(key.clone(), value.clone());
                }
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}

fn options(value: &Value, label: &str) -> Result<Options, Error> {
    if !value.is_object() && !value.is_null() {
        return Err(invalid(label, "options"));
    }
    let mut result = Options { cwd: optional_text(&value["cwd"], label, "options.cwd")?, ..Options::default() };
    if let Some(env) = value["env"].as_object() {
        if env.len() > 128 {
            return Err(Error::Limit);
        }
        for (key, value) in env {
            if key.is_empty() || key.len() > 256 || key.contains(['\0', '=']) {
                return Err(invalid(label, "options.env"));
            }
            result.env.insert(key.clone(), optional_text(value, label, "options.env")?);
        }
    } else if !value["env"].is_null() {
        return Err(invalid(label, "options.env"));
    }
    if !value["shell"].is_null() {
        result.shell =
            Some((text(&value["shell"]["executable"], label, "options.shell.executable")?, strings(&value["shell"]["args"], label, "options.shell.args", 64)?));
    }
    Ok(result)
}

fn task(value: &Value) -> Result<Task, Error> {
    let label = value["label"]
        .as_str()
        .or_else(|| value["command"].as_str())
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.contains('\0'))
        .ok_or_else(|| invalid("", "label"))?
        .to_string();
    let kind = match value["type"].as_str() {
        None | Some("shell") if value["type"].is_null() || value["type"].is_string() => Kind::Shell,
        Some("process") => Kind::Process,
        _ => return Err(invalid(&label, "type")),
    };
    let depends = if value["dependsOn"].is_string() {
        vec![text(&value["dependsOn"], &label, "dependsOn")?]
    } else {
        strings(&value["dependsOn"], &label, "dependsOn", MAX_DEPS)?
    };
    let command = if value["command"].is_null() { None } else { Some(argument(&value["command"], &label)?) };
    if command.as_ref().is_some_and(|c| c.value.trim().is_empty()) || command.is_none() && depends.is_empty() {
        return Err(invalid(&label, "command"));
    }
    let args = match value["args"].as_array() {
        Some(args) if args.len() <= 256 => args.iter().map(|a| argument(a, &label)).collect::<Result<_, _>>()?,
        None if value["args"].is_null() => Vec::new(),
        _ => return Err(invalid(&label, "args")),
    };
    let order = match value["dependsOrder"].as_str() {
        None if value["dependsOrder"].is_null() => Order::Parallel,
        Some("parallel") => Order::Parallel,
        Some("sequence") => Order::Sequence,
        _ => return Err(invalid(&label, "dependsOrder")),
    };
    let matchers = match &value["problemMatcher"] {
        Value::Null => Vec::new(),
        Value::Array(values) if values.len() <= 16 => values.clone(),
        value @ (Value::String(_) | Value::Object(_)) => vec![value.clone()],
        _ => return Err(invalid(&label, "problemMatcher")),
    };
    if matchers.iter().any(|v| !v.is_object() && !v.is_string()) {
        return Err(invalid(&label, "problemMatcher"));
    }
    let group = match &value["group"] {
        Value::Null => None,
        value => {
            let kind = value.as_str().or_else(|| value["kind"].as_str()).filter(|s| matches!(*s, "build" | "test")).ok_or_else(|| invalid(&label, "group"))?;
            Some((kind.into(), boolean(&value["isDefault"], false, &label, "group.isDefault")?))
        }
    };
    let run = &value["runOptions"];
    if !run.is_null() && !run.is_object() {
        return Err(invalid(&label, "runOptions"));
    }
    let instance_limit = match run["instanceLimit"].as_u64() {
        None if run["instanceLimit"].is_null() => 1,
        Some(n @ 1..=32) => n as usize,
        _ => return Err(invalid(&label, "runOptions.instanceLimit")),
    };
    let on_open = match run["runOn"].as_str() {
        None if run["runOn"].is_null() => false,
        Some("default") => false,
        Some("folderOpen") => true,
        _ => return Err(invalid(&label, "runOptions.runOn")),
    };
    Ok(Task {
        label: label.clone(),
        kind,
        command,
        args,
        options: options(&value["options"], &label)?,
        depends,
        order,
        background: boolean(&value["isBackground"], false, &label, "isBackground")?,
        matchers,
        auto_match: value.get("problemMatcher").is_none(),
        group,
        instance_limit,
        reevaluate: boolean(&run["reevaluateOnRerun"], true, &label, "runOptions.reevaluateOnRerun")?,
        on_open,
        presentation: value["presentation"].clone(),
    })
}

fn input(value: &Value) -> Result<(String, Input), Error> {
    let id = text(&value["id"], "input", "id")?;
    if id.is_empty() || id.len() > 512 {
        return Err(invalid("input", "id"));
    }
    let parsed = match value["type"].as_str() {
        Some("promptString") => Input::Prompt {
            description: text(&value["description"], &id, "description")?,
            default: optional_text(&value["default"], &id, "default")?,
            password: boolean(&value["password"], false, &id, "password")?,
        },
        Some("pickString") => {
            let Some(options) = value["options"].as_array().filter(|v| !v.is_empty() && v.len() <= 128) else { return Err(invalid(&id, "options")) };
            let options = options
                .iter()
                .map(|v| {
                    if v.is_string() {
                        let value = text(v, &id, "options")?;
                        Ok((value.clone(), value))
                    } else {
                        Ok((text(&v["label"], &id, "options.label")?, text(&v["value"], &id, "options.value")?))
                    }
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let default = optional_text(&value["default"], &id, "default")?;
            if default.as_ref().is_some_and(|default| !options.iter().any(|(_, value)| value == default)) {
                return Err(invalid(&id, "default"));
            }
            Input::Pick { description: text(&value["description"], &id, "description")?, options, default }
        }
        Some("command") => Input::Command { command: text(&value["command"], &id, "command")?, args: value["args"].clone() },
        _ => return Err(invalid(&id, "type")),
    };
    Ok((id, parsed))
}

pub struct Registry {
    pub tasks: Vec<Task>,
    pub inputs: BTreeMap<String, Input>,
    labels: HashMap<String, usize>,
}
pub struct Plan {
    pub root: usize,
    pub order: Vec<usize>,
    pub waits: BTreeMap<usize, BTreeSet<usize>>,
    pub readiness: BTreeSet<usize>,
}

impl Registry {
    pub fn parse(text: &str, platform: Platform) -> Result<Self, Error> {
        if text.len() > MAX_FILE {
            return Err(Error::Limit);
        }
        let root: Value = serde_json::from_str(&theme::themes::strip_jsonc(text)).map_err(|_| Error::Json)?;
        if !root.is_object() && !root.is_array() {
            return Err(Error::Json);
        }
        if let Some(version) = root.get("version")
            && version.as_str() != Some("2.0.0")
        {
            return Err(invalid("", "version"));
        }
        let mut defaults = Value::Object(Map::new());
        for key in ["type", "command", "args", "options", "isBackground", "problemMatcher", "presentation", "runOptions"] {
            if let Some(value) = root.get(key) {
                defaults[key] = value.clone();
            }
        }
        if let Some(value) = root.get(platform.key()) {
            merge(&mut defaults, value);
        }
        let items: &[Value] = if let Some(items) = root.as_array() {
            items
        } else if let Some(items) = root.get("tasks") {
            items.as_array().ok_or_else(|| invalid("", "tasks"))?
        } else if !root["command"].is_null() {
            std::slice::from_ref(&root)
        } else {
            return Err(invalid("", "tasks"));
        };
        if items.len() > MAX_TASKS {
            return Err(Error::Limit);
        }
        let mut tasks = Vec::new();
        let mut labels = HashMap::new();
        let mut budget = Budget(MAX_EXPANDED);
        for value in items {
            if !value.is_object() {
                return Err(invalid("", "task"));
            }
            let mut combined = defaults.clone();
            merge(&mut combined, value);
            if let Some(value) = value.get(platform.key()) {
                merge(&mut combined, value);
            }
            serde_json::to_writer(&mut budget, &combined).map_err(|_| Error::Limit)?;
            let task = task(&combined)?;
            if labels.insert(task.label.clone(), tasks.len()).is_some() {
                return Err(Error::Duplicate(task.label));
            }
            tasks.push(task);
        }
        let mut inputs = BTreeMap::new();
        if let Some(values) = root.get("inputs") {
            let Some(values) = values.as_array().filter(|v| v.len() <= MAX_INPUTS) else { return Err(invalid("", "inputs")) };
            for value in values {
                let (id, value) = input(value)?;
                if inputs.insert(id.clone(), value).is_some() {
                    return Err(Error::Duplicate(id));
                }
            }
        }
        Ok(Self { tasks, inputs, labels })
    }

    pub fn plan(&self, label: &str, require_root_ready: bool) -> Result<Plan, Error> {
        let root = *self.labels.get(label).ok_or_else(|| Error::Missing { task: label.into(), dependency: label.into() })?;
        let mut waits = BTreeMap::<usize, BTreeSet<usize>>::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if waits.contains_key(&id) {
                continue;
            }
            let task = &self.tasks[id];
            let mut dependencies = BTreeSet::new();
            for label in &task.depends {
                let dep = *self.labels.get(label).ok_or_else(|| Error::Missing { task: task.label.clone(), dependency: label.clone() })?;
                dependencies.insert(dep);
                pending.push(dep);
            }
            waits.insert(id, dependencies);
        }
        let _ = self.sorted(root, &waits)?;
        let closure = |start: usize| {
            let mut seen = BTreeSet::new();
            let mut pending = vec![start];
            while let Some(id) = pending.pop() {
                if seen.insert(id) {
                    pending.extend(waits[&id].iter().copied());
                }
            }
            seen
        };
        let mut barriers = Vec::new();
        let mut readiness = BTreeSet::new();
        for &id in waits.keys() {
            let task = &self.tasks[id];
            if task.command.is_some() && task.background && (id != root || require_root_ready) {
                if !task.matchers.iter().any(has_readiness) {
                    return Err(Error::Readiness(task.label.clone()));
                }
                readiness.insert(id);
            }
            if task.order != Order::Sequence {
                continue;
            }
            let mut earlier = BTreeSet::new();
            let mut previous = None;
            for label in &task.depends {
                let dep = self.labels[label];
                let branch = closure(dep);
                if let Some(previous) = previous {
                    for &node in branch.difference(&earlier) {
                        barriers.push((node, previous));
                    }
                }
                earlier.extend(branch);
                previous = Some(dep);
            }
        }
        for (node, previous) in barriers {
            if node != previous {
                waits.get_mut(&node).expect("planned node").insert(previous);
            }
        }
        let order = self.sorted(root, &waits)?;
        Ok(Plan { root, order, waits, readiness })
    }

    fn sorted(&self, root: usize, waits: &BTreeMap<usize, BTreeSet<usize>>) -> Result<Vec<usize>, Error> {
        fn visit(
            id: usize,
            registry: &Registry,
            waits: &BTreeMap<usize, BTreeSet<usize>>,
            done: &mut BTreeSet<usize>,
            stack: &mut Vec<usize>,
            out: &mut Vec<usize>,
        ) -> Result<(), Error> {
            if done.contains(&id) {
                return Ok(());
            }
            if let Some(start) = stack.iter().position(|&node| node == id) {
                let mut cycle: Vec<_> = stack[start..].iter().map(|id| registry.tasks[*id].label.clone()).collect();
                cycle.push(registry.tasks[id].label.clone());
                return Err(Error::Cycle(cycle));
            }
            stack.push(id);
            for &dependency in &waits[&id] {
                visit(dependency, registry, waits, done, stack, out)?;
            }
            stack.pop();
            done.insert(id);
            out.push(id);
            Ok(())
        }
        let mut out = Vec::new();
        visit(root, self, waits, &mut BTreeSet::new(), &mut Vec::new(), &mut out)?;
        Ok(out)
    }
}

fn has_readiness(value: &Value) -> bool {
    value.as_str() == Some("$tsc-watch")
        || value["base"].as_str() == Some("$tsc-watch")
        || value["background"]["endsPattern"].as_str().is_some()
        || value["background"]["endsPattern"]["regexp"].as_str().is_some()
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
