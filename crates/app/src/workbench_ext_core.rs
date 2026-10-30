//! Workbench: core commands extensions run (docs/extensions/reference/core-commands.md, the table in
//! `ext_manifest::core_commands`). `commands.execute("core:…")` arrives here after `Scope::may_execute` checked that
//! the command is listed for extensions and in the manifest's `permissions.commands`. Here the permission the command
//! implies is checked on what it would touch (the files it saves, formats or opens, the task's program), with the
//! same effective permissions (grants, minus the user's switches, masked by Workspace Trust); then it runs as the
//! palette runs it.

use std::sync::Arc;

use ext_api::value::{self as v, Completion};
use ext_manifest::core_commands::{CoreCommand, Implies};
use ext_proto::ToHost;

use super::*;
use crate::ext_broker::Scope;
use crate::ext_runtime::Process;

/// Shell syntax that makes a task line more than one program with its arguments.
const SHELL_SYNTAX: [&str; 10] = ["&&", "||", ";", "|", ">", "<", "$", "`", "\n", "&"];

/// The program a task line runs, when the line is one program with its arguments; `None` for a shell line.
pub(super) fn task_program(command: &str) -> Option<String> {
    let line = command.trim();
    if line.is_empty() || SHELL_SYNTAX.iter().any(|s| line.contains(s)) {
        return None;
    }
    let first = line.split_whitespace().next()?;
    let program = first.trim_matches(|c| c == '\'' || c == '"');
    // `cd dir && …` is caught above; a bare `cd`, `exec` or `env` would run something else in the end.
    (!program.is_empty() && !matches!(program, "cd" | "exec" | "env" | "sudo" | "command" | "builtin")).then(|| program.to_string())
}

impl Workbench {
    /// `commands.execute("core:…")` for `caller`: check what the command implies, run it, and complete the ticket.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn execute_core_for_extension(
        &mut self,
        caller: &str,
        ticket: u64,
        scope: Scope,
        command: &str,
        args_json: &str,
        proc: Arc<Process>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let record = self.cfg.exts.code.record.clone();
        let who = caller.to_string();
        let done = move |c: Completion| {
            record.lock().unwrap_or_else(|p| p.into_inner()).ticket_done(&who, ticket, &c, None);
            let _ = proc.send(&ToHost::Complete { ticket, completion: c });
        };
        let Some(core) = ext_manifest::core_command(command) else {
            return done(Completion::Failed(v::Error::Invalid(format!("{command} is not a core command extensions may run"))));
        };
        let arg: serde_json::Value = serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
        match self.run_core_command(core, &scope, &arg, window, cx) {
            Ok(Some(c)) => done(c),
            Err(e) => {
                self.log_extension(caller, ext_api::value::LogLevel::Warn, format!("core:{} refused: {}", core.id, super::code_glue::explain(&e)));
                done(Completion::Failed(e));
            }
            Ok(None) if self.active_root(cx).is_none() => done(Completion::Failed(v::Error::NotFound("no folder is open, so there are no tasks".into()))),
            // `tasks.run` with a label: the task list is read first.
            Ok(None) => {
                let label = arg.as_str().unwrap_or_default().to_string();
                self.detect_tasks(cx, move |this, tasks, window, cx| {
                    this.with_extension_tasks(tasks, window, cx, move |this, tasks, window, cx| {
                        done(this.run_task_for_extension(&scope, &label, tasks, window, cx));
                    })
                });
            }
        }
    }

    /// Check and run `core`: its completion, or `None` when it finishes later (a task by label).
    fn run_core_command(
        &mut self,
        core: &CoreCommand,
        scope: &Scope,
        arg: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Option<Completion>, v::Error> {
        match core.implies {
            Implies::ReadArgument => {
                let uri = arg.as_str().ok_or_else(|| v::Error::Invalid(format!("core:{} takes a file:// URI", core.id)))?;
                let path = scope.may_read(uri)?;
                self.open_file(path, window, cx);
                return Ok(Some(Completion::Done));
            }
            Implies::RunTask if arg.is_string() => return Ok(None),
            Implies::WriteActiveFile => {
                let path = self.active_editor().map(|e| e.read(cx).path.clone());
                let path = path.ok_or_else(|| v::Error::NotFound("no file is open in the editor".into()))?;
                scope.may_imply(core.implies, &[path])?;
            }
            Implies::WriteUnsaved => {
                let unsaved: Vec<String> = self.all_editors().iter().map(|e| e.read(cx)).filter(|e| e.is_dirty()).map(|e| e.path.clone()).collect();
                scope.may_imply(core.implies, &unsaved)?;
            }
            Implies::Nothing | Implies::RunTask => {}
        }
        let action = crate::commands::action_by_id(core.id).ok_or_else(|| v::Error::NotFound(format!("core:{} is not available", core.id)))?;
        window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
        Ok(Some(Completion::Done))
    }

    /// `tasks.run` with a label: a task an extension provides runs in that extension; any other must be one program the
    /// caller may run.
    fn run_task_for_extension(&mut self, scope: &Scope, label: &str, tasks: Vec<tasks::Task>, window: &mut Window, cx: &mut Context<Self>) -> Completion {
        let Some(task) = tasks.into_iter().find(|t| t.label == label) else {
            return Completion::Failed(v::Error::NotFound(format!("no task is labelled \"{label}\"")));
        };
        if task.extension().is_none() {
            let Some(program) = task_program(&task.command) else {
                let why = format!("a task that runs one program (\"{label}\" is a shell line)");
                return Completion::Failed(v::Error::Denied(v::Denied { permission: why, why: v::Why::NotDeclared }));
            };
            if let Err(e) = scope.may_imply(Implies::RunTask, &[program]) {
                return Completion::Failed(e);
            }
        }
        self.run_task(task, window, cx);
        Completion::Done
    }
}
