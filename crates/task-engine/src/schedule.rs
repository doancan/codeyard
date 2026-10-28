//! Deterministic task transitions. The caller owns processes, output and UI; this module only reserves work.

use std::collections::BTreeMap;

use crate::model::{Plan, Registry};

const MAX_LIVE: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Exit(i32),
    Disconnected,
    BeforeReady,
    Dependency(usize),
    Capacity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Waiting,
    Running,
    Ready,
    Succeeded,
    Failed(Failure),
    Cancelled,
}
impl State {
    pub fn satisfied(&self) -> bool {
        matches!(self, Self::Ready | Self::Succeeded)
    }
    fn failed(&self) -> bool {
        matches!(self, Self::Failed(_) | Self::Cancelled)
    }
    fn live(&self) -> bool {
        matches!(self, Self::Running | Self::Ready)
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Changes {
    pub start: Vec<usize>,
    pub stop: Vec<usize>,
}

pub struct Scheduler {
    plan: Plan,
    states: BTreeMap<usize, State>,
    background: BTreeMap<usize, bool>,
    commands: BTreeMap<usize, bool>,
    parallelism: usize,
}

impl Scheduler {
    pub fn new(registry: &Registry, plan: Plan, parallelism: usize) -> Self {
        let states = plan.order.iter().map(|&id| (id, State::Waiting)).collect();
        let background = plan.order.iter().map(|&id| (id, registry.tasks[id].background)).collect();
        let commands = plan.order.iter().map(|&id| (id, registry.tasks[id].command.is_some())).collect();
        Self { plan, states, background, commands, parallelism: parallelism.clamp(1, MAX_LIVE) }
    }

    pub fn state(&self, id: usize) -> Option<&State> {
        self.states.get(&id)
    }
    pub fn root_state(&self) -> &State {
        &self.states[&self.plan.root]
    }
    pub fn live(&self) -> usize {
        self.states.values().filter(|state| state.live()).count()
    }

    /// A watch matcher explicitly releases dependents; merely spawning the command does not.
    pub fn ready(&mut self, id: usize) -> bool {
        if self.background.get(&id) == Some(&true) && self.states.get(&id) == Some(&State::Running) {
            self.states.insert(id, State::Ready);
            true
        } else {
            false
        }
    }

    /// Events received after cancellation, replacement or a prior exit cannot revive work.
    pub fn exited(&mut self, id: usize, code: Option<i32>) -> bool {
        let Some(previous) = self.states.get(&id).filter(|state| state.live()) else { return false };
        let state = match code {
            None => State::Failed(Failure::Disconnected),
            Some(0) if self.plan.readiness.contains(&id) && *previous != State::Ready => State::Failed(Failure::BeforeReady),
            Some(0) => State::Succeeded,
            Some(code) => State::Failed(Failure::Exit(code)),
        };
        self.states.insert(id, state);
        true
    }

    /// Reserve each eligible command exactly once. A second call without events returns no duplicate starts.
    pub fn advance(&mut self) -> Changes {
        let mut changes = Changes::default();
        let mut running = self.states.values().filter(|state| **state == State::Running).count();
        let mut live = self.live();
        for &id in &self.plan.order {
            if matches!(self.states[&id], State::Succeeded | State::Failed(_) | State::Cancelled) {
                continue;
            }
            if let Some(&dependency) = self.plan.waits[&id].iter().find(|dep| self.states[dep].failed()) {
                if self.states[&id].live() {
                    changes.stop.push(id);
                    live -= 1;
                    running -= usize::from(self.states[&id] == State::Running);
                }
                self.states.insert(id, State::Failed(Failure::Dependency(dependency)));
                continue;
            }
            if self.states[&id] != State::Waiting || !self.plan.waits[&id].iter().all(|dep| self.states[dep].satisfied()) {
                continue;
            }
            if !self.commands[&id] {
                self.states.insert(id, State::Succeeded);
            } else if live >= MAX_LIVE && running == 0 {
                // Thirty-two ready watchers must produce a clear failure rather than an unresolvable wait.
                self.states.insert(id, State::Failed(Failure::Capacity));
            } else if running < self.parallelism && live < MAX_LIVE {
                self.states.insert(id, State::Running);
                changes.start.push(id);
                running += 1;
                live += 1;
            }
        }
        changes
    }

    /// Return only owned live processes. Repeated cancellation is harmless and never replays a command.
    pub fn cancel(&mut self) -> Vec<usize> {
        let mut stop = Vec::new();
        for (&id, state) in &mut self.states {
            if state.live() {
                stop.push(id);
            }
            if matches!(state, State::Waiting | State::Running | State::Ready) {
                *state = State::Cancelled;
            }
        }
        stop
    }
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
