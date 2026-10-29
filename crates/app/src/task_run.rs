//! Running a task graph: the task engine plans and schedules, this module turns a planned task into a typed command for
//! the host and follows each run's output (problem matchers, background readiness) and exits. No window, no process:
//! the workbench owns those and feeds events in, so every transition can be tested on its own.

use std::collections::{BTreeMap, BTreeSet};

use task_engine::matchers::{Limits, Matcher, Problem};
use task_engine::model::{Argument, Kind, Plan, Platform, Quoting, Registry, Task};
use task_engine::schedule::{Failure, Scheduler, State};
use task_engine::text_stream::{Decoder, Event};
use task_engine::variables::{self, Context};

/// Commands that run at the same time in one run (the engine caps watchers separately).
const PARALLELISM: usize = 8;
/// Problems kept per task: a runaway compiler cannot fill memory.
const MAX_PROBLEMS: usize = 5_000;

/// The engine's platform for a host's `HostInfo::os`.
pub fn platform_of(os: &str) -> Platform {
    match os {
        "windows" => Platform::Windows,
        "macos" => Platform::Mac,
        _ => Platform::Linux,
    }
}

/// A task file (or detected tasks) parsed and planned for one label.
pub struct Prepared {
    pub registry: Registry,
    pub plan: Plan,
}

/// The message for a task file or plan error, in the user's language.
pub fn describe(error: &task_engine::model::Error) -> String {
    use task_engine::model::Error as E;
    match error {
        E::Limit => crate::i18n::t("The task file is too large or has too many tasks.").into(),
        E::Json => crate::i18n::t("The task file is not valid JSON.").into(),
        E::Invalid { task, field } if task.is_empty() => crate::i18n::tf("The task file has an invalid “{}”.", &[field]),
        E::Invalid { task, field } => crate::i18n::tf("Task “{0}” has an invalid “{1}”.", &[task, field]),
        E::Duplicate(label) => crate::i18n::tf("Two tasks are called “{}”.", &[label]),
        E::Missing { task, dependency } if task == dependency => crate::i18n::tf("There is no task called “{}”.", &[task]),
        E::Missing { task, dependency } => crate::i18n::tf("Task “{0}” depends on “{1}”, which does not exist.", &[task, dependency]),
        E::Cycle(labels) => crate::i18n::tf("Task dependency cycle: {}", &[&labels.join(" → ")]),
        E::Readiness(label) => {
            crate::i18n::tf("Task “{}” runs in the background but its problem matcher has no endsPattern, so nothing can wait for it.", &[label])
        }
    }
}

pub fn prepare(text: &str, platform: Platform, label: &str) -> Result<Prepared, String> {
    let registry = Registry::parse(text, platform).map_err(|e| describe(&e))?;
    let plan = registry.plan(label, false).map_err(|e| describe(&e))?;
    Ok(Prepared { registry, plan })
}

/// Every template of a task that is expanded before it runs.
fn templates(task: &Task) -> impl Iterator<Item = &str> {
    task.command
        .iter()
        .chain(task.args.iter())
        .map(|a| a.value.as_str())
        .chain(task.options.cwd.as_deref())
        .chain(task.options.env.values().filter_map(|v| v.as_deref()))
        .chain(task.options.shell.iter().flat_map(|(exe, args)| std::iter::once(exe.as_str()).chain(args.iter().map(String::as_str))))
}

/// The `${input:…}` ids the planned tasks use, in the order they are first needed. An id the file does not define, or a
/// `command` input (it would run an editor command), is an error before anything runs.
pub fn inputs_needed(prepared: &Prepared) -> Result<Vec<String>, String> {
    let mut seen = BTreeSet::new();
    let mut ids = Vec::new();
    for &node in &prepared.plan.order {
        let task = &prepared.registry.tasks[node];
        for template in templates(task) {
            let keys = variables::references(template).map_err(|_| crate::i18n::tf("Task “{}” has a variable that cannot be expanded.", &[&task.label]))?;
            for key in keys {
                if key.starts_with("command:") {
                    return Err(crate::i18n::tf("Task “{0}” uses “{1}”, which would run an editor command; use an input instead.", &[&task.label, &key]));
                }
                let Some(id) = key.strip_prefix("input:") else { continue };
                match prepared.registry.inputs.get(id) {
                    None => return Err(crate::i18n::tf("Task “{0}” uses the input “{1}”, which the task file does not define.", &[&task.label, &id])),
                    Some(task_engine::model::Input::Command { .. }) => {
                        return Err(crate::i18n::tf("The input “{}” would run an editor command; use promptString or pickString.", &[&id]));
                    }
                    Some(_) => {}
                }
                if seen.insert(id.to_string()) {
                    ids.push(id.to_string());
                }
            }
        }
    }
    Ok(ids)
}

/// How a shell reads quotes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Posix,
    Cmd,
    PowerShell,
}

fn family(shell: Option<&str>, platform: Platform) -> Family {
    let name = shell.map(|s| s.rsplit(['/', '\\']).next().unwrap_or(s).to_ascii_lowercase());
    match name.as_deref() {
        Some(n) if n.starts_with("cmd") => Family::Cmd,
        Some(n) if n.starts_with("powershell") || n.starts_with("pwsh") => Family::PowerShell,
        Some(_) => Family::Posix,
        None if platform == Platform::Windows => Family::Cmd,
        None => Family::Posix,
    }
}

fn needs_quotes(value: &str, family: Family) -> bool {
    let special: &[char] = match family {
        Family::Posix => &['\'', '"', '\\', '$', '`', '&', '|', ';', '<', '>', '(', ')', '*', '?', '[', ']', '{', '}', '!', '#', '~'],
        Family::Cmd => &['"', '&', '|', '<', '>', '(', ')', '^', '%', '!'],
        Family::PowerShell => &['\'', '"', '`', '$', '&', '|', ';', '<', '>', '(', ')', '{', '}', '@', '#'],
    };
    value.is_empty() || value.chars().any(|c| c.is_whitespace() || special.contains(&c))
}

/// `value` as one word for the shell. `None` quotes only when needed (strongly); `Weak` keeps variables live.
fn quote(value: &str, quoting: Option<Quoting>, family: Family) -> String {
    let quoting = match quoting {
        Some(q) => q,
        None if needs_quotes(value, family) => Quoting::Strong,
        None => return value.to_string(),
    };
    match (family, quoting) {
        (Family::Posix, Quoting::Strong) => format!("'{}'", value.replace('\'', r"'\''")),
        (Family::Posix, Quoting::Weak) => format!("\"{}\"", value.replace('\\', r"\\").replace('"', "\\\"").replace('`', "\\`")),
        (Family::Posix, Quoting::Escape) => value.chars().fold(String::new(), |mut out, c| {
            if c.is_whitespace() || needs_quotes(&c.to_string(), Family::Posix) {
                out.push('\\');
            }
            out.push(c);
            out
        }),
        (Family::Cmd, Quoting::Strong | Quoting::Weak) => format!("\"{}\"", value.replace('"', "\"\"")),
        (Family::Cmd, Quoting::Escape) => value.chars().fold(String::new(), |mut out, c| {
            if c == ' ' || needs_quotes(&c.to_string(), Family::Cmd) {
                out.push('^');
            }
            out.push(c);
            out
        }),
        (Family::PowerShell, Quoting::Strong) => format!("'{}'", value.replace('\'', "''")),
        (Family::PowerShell, Quoting::Weak) => format!("\"{}\"", value.replace('`', "``").replace('"', "`\"")),
        (Family::PowerShell, Quoting::Escape) => value.chars().fold(String::new(), |mut out, c| {
            if c.is_whitespace() || needs_quotes(&c.to_string(), Family::PowerShell) {
                out.push('`');
            }
            out.push(c);
            out
        }),
    }
}

/// What the host runs for one task.
#[derive(Clone, Debug, PartialEq)]
pub struct Spawn {
    pub command: proto::TaskCommand,
    pub cwd: Option<String>,
    pub env: Vec<(String, Option<String>)>,
}

/// The typed command for `task`, with every variable expanded once from `context` (values are never expanded again).
/// A process task keeps each argument as one argv entry; a shell task quotes them for its shell.
pub fn spawn_for(task: &Task, context: &Context<'_>) -> Result<Spawn, String> {
    let expand = |template: &str| {
        variables::expand(template, context).map_err(|e| match e {
            variables::Error::MissingFile => crate::i18n::tf("Task “{}” needs an open file.", &[&task.label]),
            variables::Error::Missing(key) => crate::i18n::tf("Task “{0}” uses “{1}”, which has no value here.", &[&task.label, &key]),
            _ => crate::i18n::tf("Task “{}” has a variable that cannot be expanded.", &[&task.label]),
        })
    };
    let argument = |a: &Argument| expand(&a.value).map(|value| Argument { value, quoting: a.quoting });
    let Some(command) = task.command.as_ref() else {
        return Err(crate::i18n::tf("Task “{}” has no command.", &[&task.label]));
    };
    let command = argument(command)?;
    let args = task.args.iter().map(argument).collect::<Result<Vec<_>, _>>()?;
    let shell = match &task.options.shell {
        Some((exe, args)) => Some((expand(exe)?, args.iter().map(|a| expand(a)).collect::<Result<Vec<_>, _>>()?)),
        None => None,
    };
    let command = match task.kind {
        Kind::Process => proto::TaskCommand::Process { program: command.value, args: args.into_iter().map(|a| a.value).collect() },
        Kind::Shell => {
            let family = family(shell.as_ref().map(|(exe, _)| exe.as_str()), context.platform);
            // The command is the user's shell text unless they asked for it to be quoted.
            let mut line = match command.quoting {
                Some(q) => quote(&command.value, Some(q), family),
                None => command.value,
            };
            for a in &args {
                line.push(' ');
                line.push_str(&quote(&a.value, a.quoting, family));
            }
            let (shell, shell_args) = match shell {
                Some((exe, args)) => (Some(exe), (!args.is_empty()).then_some(args)),
                None => (None, None),
            };
            proto::TaskCommand::Shell { shell, args: shell_args, line }
        }
    };
    let cwd = match &task.options.cwd {
        Some(cwd) => expand(cwd)?,
        None => context.root.to_string(),
    };
    let env = task.options.env.iter().map(|(k, v)| Ok((k.clone(), v.as_deref().map(expand).transpose()?))).collect::<Result<Vec<_>, String>>()?;
    Ok(Spawn { command, cwd: Some(cwd), env })
}

/// One run of a planned graph. Node ids are the registry's task indexes; `token` tells a run from the next one, so a
/// late event from a replaced run changes nothing.
pub struct Run {
    pub token: u64,
    pub label: String,
    pub prepared: Prepared,
    scheduler: Scheduler,
    /// Host process of each started node.
    procs: BTreeMap<usize, u32>,
    decoders: BTreeMap<usize, Decoder>,
    matchers: BTreeMap<usize, Vec<Matcher>>,
    problems: BTreeMap<usize, Vec<Problem>>,
    /// Nodes asked to stop whose exit has not arrived yet.
    stopping: BTreeSet<usize>,
}

/// What the workbench does after an event.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// Nodes to start now, in order.
    pub start: Vec<usize>,
    /// Host processes to stop.
    pub kill: Vec<u32>,
    /// The problems changed.
    pub problems: bool,
}

impl Run {
    /// Compile every matcher before anything runs: a malformed pattern fails the whole run with a clear message.
    pub fn new(token: u64, label: String, prepared: Prepared) -> Result<Self, String> {
        let mut matchers = BTreeMap::new();
        let mut limits = Limits::default();
        for &node in &prepared.plan.order {
            let task = &prepared.registry.tasks[node];
            let compiled = task
                .matchers
                .iter()
                .map(|spec| Matcher::compile(spec, &mut limits))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| crate::i18n::tf("Task “{0}” has an invalid problem matcher: {1}", &[&task.label, &format!("{e:?}")]))?;
            matchers.insert(node, compiled);
        }
        let scheduler = Scheduler::new(&prepared.registry, clone_plan(&prepared.plan), PARALLELISM);
        Ok(Self {
            token,
            label,
            prepared,
            scheduler,
            procs: BTreeMap::new(),
            decoders: BTreeMap::new(),
            matchers,
            problems: BTreeMap::new(),
            stopping: BTreeSet::new(),
        })
    }

    pub fn task(&self, node: usize) -> &Task {
        &self.prepared.registry.tasks[node]
    }

    pub fn state(&self, node: usize) -> Option<&State> {
        self.scheduler.state(node)
    }

    /// The whole run: its root's state.
    pub fn outcome(&self) -> &State {
        self.scheduler.root_state()
    }

    /// Finished: nothing runs and nothing waits.
    pub fn finished(&self) -> bool {
        self.procs.is_empty()
            && self.prepared.plan.order.iter().all(|n| !matches!(self.scheduler.state(*n), Some(State::Waiting | State::Running)))
            && !matches!(self.outcome(), State::Ready)
    }

    /// Start what may start now; stop what a failed dependency made pointless.
    pub fn advance(&mut self) -> Effects {
        let changes = self.scheduler.advance();
        let kill = changes.stop.iter().filter_map(|n| self.stop_node(*n)).collect();
        Effects { start: changes.start, kill, problems: false }
    }

    fn stop_node(&mut self, node: usize) -> Option<u32> {
        let proc = *self.procs.get(&node)?;
        self.stopping.insert(node).then_some(proc)
    }

    /// The host started `node` as `proc`.
    pub fn started(&mut self, node: usize, proc: u32) {
        self.procs.insert(node, proc);
        self.decoders.entry(node).or_default();
    }

    /// The host could not start `node`: it fails like a command that exited with 127, and its dependents stop.
    pub fn spawn_failed(&mut self, node: usize) -> Effects {
        self.scheduler.exited(node, Some(127));
        self.advance()
    }

    /// Output of `node`: lines go through its matchers. A background task's end pattern releases its dependents.
    pub fn output(&mut self, node: usize, bytes: &[u8]) -> Effects {
        let mut lines = Vec::new();
        let decoder = self.decoders.entry(node).or_default();
        decoder.push(bytes, |event| {
            if let Event::Line(line) = event {
                lines.push(line.to_string());
            }
        });
        self.lines(node, lines)
    }

    fn lines(&mut self, node: usize, lines: Vec<String>) -> Effects {
        let mut effects = Effects::default();
        let mut ready = false;
        let Some(matchers) = self.matchers.get_mut(&node) else { return effects };
        let problems = self.problems.entry(node).or_default();
        for line in lines {
            for matcher in matchers.iter_mut() {
                let Ok(update) = matcher.line(&line) else { continue };
                if update.began {
                    // A watcher started a new compilation: the previous one's problems are stale.
                    if !problems.is_empty() {
                        problems.clear();
                        effects.problems = true;
                    }
                }
                if let Some(problem) = update.problem
                    && problems.len() < MAX_PROBLEMS
                {
                    problems.push(problem);
                    effects.problems = true;
                }
                ready |= update.ready;
            }
        }
        if ready && self.scheduler.ready(node) {
            let more = self.advance();
            effects.start.extend(more.start);
            effects.kill.extend(more.kill);
        }
        effects
    }

    /// `node`'s process ended (`None`: killed or disconnected). A partial last line is still matched.
    pub fn exited(&mut self, node: usize, code: Option<i32>) -> Effects {
        let mut tail = Vec::new();
        if let Some(mut decoder) = self.decoders.remove(&node) {
            decoder.finish(|event| {
                if let Event::Line(line) = event {
                    tail.push(line.to_string());
                }
            });
        }
        let mut effects = self.lines(node, tail);
        self.procs.remove(&node);
        self.stopping.remove(&node);
        self.scheduler.exited(node, code);
        let more = self.advance();
        effects.start.extend(more.start);
        effects.kill.extend(more.kill);
        effects
    }

    /// Stop everything: queued work never starts; each live process is asked to stop once.
    pub fn cancel(&mut self) -> Vec<u32> {
        self.scheduler.cancel();
        let nodes: Vec<usize> = self.procs.keys().copied().collect();
        nodes.into_iter().filter_map(|n| self.stop_node(n)).collect()
    }

    /// Processes asked to stop that have not confirmed it.
    pub fn unconfirmed(&self) -> usize {
        self.stopping.len()
    }

    /// Every problem of the run, with the matcher's file location rule for resolving its path.
    pub fn problems(&self) -> Vec<(usize, &Problem)> {
        self.problems.iter().flat_map(|(node, list)| list.iter().map(move |p| (*node, p))).collect()
    }

    /// How a node's problem paths are resolved (its first matcher's rule).
    pub fn location(&self, node: usize) -> Option<&task_engine::matchers::FileLocation> {
        self.matchers.get(&node)?.first().map(|m| &m.location)
    }

    /// Node and host process of a live run.
    pub fn node_of(&self, proc: u32) -> Option<usize> {
        self.procs.iter().find(|(_, p)| **p == proc).map(|(n, _)| *n)
    }
}

fn clone_plan(plan: &Plan) -> Plan {
    Plan { root: plan.root, order: plan.order.clone(), waits: plan.waits.clone(), readiness: plan.readiness.clone() }
}

/// The user-facing word for a node's state.
pub fn state_label(state: &State) -> String {
    match state {
        State::Waiting => crate::i18n::t("queued").into(),
        State::Running => crate::i18n::t("running").into(),
        State::Ready => crate::i18n::t("ready").into(),
        State::Succeeded => crate::i18n::t("succeeded").into(),
        State::Cancelled => crate::i18n::t("cancelled").into(),
        State::Failed(Failure::Exit(code)) => crate::i18n::tf("failed (exit code {})", &[code]),
        State::Failed(Failure::Disconnected) => crate::i18n::t("stopped").into(),
        State::Failed(Failure::BeforeReady) => crate::i18n::t("ended before it was ready").into(),
        State::Failed(Failure::Dependency(_)) => crate::i18n::t("not run: a dependency failed").into(),
        State::Failed(Failure::Capacity) => crate::i18n::t("not run: too many tasks in the background").into(),
    }
}

#[cfg(test)]
#[path = "task_run_tests.rs"]
mod tests;
