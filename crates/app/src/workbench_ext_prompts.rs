//! Workbench: run-time prompts of code extensions (docs/plugin-architecture.md §6.3). The first clipboard read of an
//! extension per session, and its first process on a remote host per session, wait for the user's answer in a system
//! dialog, which no extension UI can draw over or imitate. Which calls ask and the answers given are in
//! `ext_prompts`; this file shows the dialog and lets the waiting calls go on, or refuses them.

use ext_api::value::Start;

use super::*;
use crate::ext_broker::Scope;
use crate::ext_prompts::{Kind, Prompts, Step};

/// A host call waiting for the user's answer.
pub(super) struct Waiting {
    ticket: u64,
    call: Start,
}

#[derive(Default)]
pub(super) struct PromptState {
    prompts: Prompts<Waiting>,
}

impl Workbench {
    /// Before a host call runs: `Some(call)` when it may go on now; `None` when it waits for the user's answer or was
    /// refused (its ticket is then completed here).
    pub(super) fn ask_first_use(
        &mut self,
        active: &crate::ext::Active,
        scope: &Scope,
        ticket: u64,
        call: Start,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Start> {
        let remote = self.remote.host.is_remote().then(|| self.remote.host.label());
        let Some(kind) = crate::ext_prompts::kind_of(&call, scope, remote.as_deref()) else { return Some(call) };
        let program = match &call {
            Start::Spawn { command } => Some(command.program.clone()),
            _ => None,
        };
        let id = active.id().to_string();
        match self.cfg.exts.prompts.prompts.check(&id, &kind, Waiting { ticket, call }) {
            Step::Go(w) => Some(w.call),
            Step::Deny(w) => {
                self.complete_extension_ticket(&id, w.ticket, kind.denied());
                None
            }
            Step::Wait => None,
            Step::Ask => {
                self.show_first_use_prompt(active, kind, program, window, cx);
                None
            }
        }
    }

    fn show_first_use_prompt(&mut self, active: &crate::ext::Active, kind: Kind, program: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let publisher = self.extension_publisher(active);
        let id = active.id().to_string();
        let (title, detail) = crate::ext_prompts::dialog(&kind, &active.manifest.name, &id, &publisher.shown, program.as_deref());
        let answer = crate::questions::first_use(&title, &detail).ask(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let allowed = answer.await == Some(0);
            let _ = this.update_in(cx, |this, window, cx| this.on_first_use_answer(&id, &kind, allowed, window, cx));
        })
        .detach();
    }

    /// The user answered: the waiting calls run, or are refused with `revoked-by-user` (which the Inspector lists).
    fn on_first_use_answer(&mut self, id: &str, kind: &Kind, allowed: bool, window: &mut Window, cx: &mut Context<Self>) {
        for w in self.cfg.exts.prompts.prompts.answer(id, kind, allowed) {
            if allowed {
                self.on_extension_call(id, w.ticket, w.call, window, cx);
            } else {
                self.complete_extension_ticket(id, w.ticket, kind.denied());
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
impl Workbench {
    /// What the user answered this session for `extension`'s clipboard reads.
    pub(crate) fn clipboard_answer_for_test(&self, extension: &str) -> Option<bool> {
        self.cfg.exts.prompts.prompts.answered(extension, &Kind::ClipboardRead)
    }

    pub(crate) fn set_host_for_test(&mut self, host: std::sync::Arc<dyn host::Host>) {
        self.remote.host = host;
    }
}
