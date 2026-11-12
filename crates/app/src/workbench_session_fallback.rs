//! What the session restore does when the saved layout does not fit what it finds: it never stops the start, it never hides the
//! reason (each fallback is logged), and it keeps one copy of an unreadable layout instead of overwriting the only evidence.

use std::collections::{BTreeMap, HashSet};
#[cfg(not(test))]
use std::sync::Mutex;

use super::Group;

/// An unreadable layout is kept up to this size (characters); it was already refused above 1 MB.
const KEEP_CHARS: usize = 64 * 1024;

/// Workspaces whose unreadable layout was already announced in this session.
#[cfg(not(test))]
static ANNOUNCED: Mutex<Option<HashSet<i64>>> = Mutex::new(None);
// Tests run in parallel, each on its own thread and with a database of its own, whose workspace ids all start at 1.
#[cfg(test)]
thread_local! {
    static ANNOUNCED: std::cell::RefCell<HashSet<i64>> = std::cell::RefCell::new(HashSet::new());
}

/// The key under which the one copy of workspace `ws`'s unreadable layout is kept.
pub(super) fn corrupt_key(ws: i64) -> String {
    format!("editor_layout:{ws}.corrupt")
}

/// Keep the unreadable text (the newest replaces the older) and say so in the log.
pub(super) fn keep_copy(state: &state::State, ws: i64, text: &str) {
    let kept: String = text.chars().take(KEEP_CHARS).collect();
    match state.set(&corrupt_key(ws), &kept) {
        Ok(()) => crate::diagnostics::log::warn(
            "session",
            format_args!("the saved editor layout of workspace {ws} could not be read ({} characters, kept as {})", text.chars().count(), corrupt_key(ws)),
        ),
        Err(e) => crate::diagnostics::log::warn(
            "session",
            format_args!("the saved editor layout of workspace {ws} could not be read and its copy could not be kept: {e}"),
        ),
    }
}

/// Whether the user is told now: once per workspace per session, however often the layout is read.
pub(super) fn first_announcement(ws: i64) -> bool {
    #[cfg(not(test))]
    {
        let mut announced = ANNOUNCED.lock().unwrap_or_else(|e| e.into_inner());
        announced.get_or_insert_with(HashSet::new).insert(ws)
    }
    #[cfg(test)]
    ANNOUNCED.with(|announced| announced.borrow_mut().insert(ws))
}

#[cfg(test)]
pub(super) fn forget_announcements() {
    ANNOUNCED.with(|announced| announced.borrow_mut().clear());
}

/// The state of group `id`, an empty one when the layout names a group that has none. That is a bug somewhere else (a validated
/// layout and its groups agree), so it is logged every time instead of being papered over.
pub(super) fn group_or_empty<'a>(groups: &'a mut BTreeMap<u8, Group>, id: u8, why: &str) -> &'a mut Group {
    if !groups.contains_key(&id) {
        crate::diagnostics::log::warn("session", format_args!("editor group {id} has no state while restoring ({why}); starting it empty"));
    }
    groups.entry(id).or_default()
}

/// `groups.remove(id)` for the group that becomes the current one; an empty one when it is missing, logged.
pub(super) fn take_group_or_empty(groups: &mut BTreeMap<u8, Group>, id: u8) -> Group {
    groups.remove(&id).unwrap_or_else(|| {
        crate::diagnostics::log::warn("session", format_args!("the focused editor group {id} has no state while restoring; starting it empty"));
        Group::default()
    })
}
