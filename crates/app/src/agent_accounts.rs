//! Which agent accounts are out of quota, and which other account to offer when one is.
//!
//! An account is a provider and a profile number (the Agents panel's *Personal*, *Work*, *Account 3*). The tools say nothing
//! about their quota except by printing a limit message (`agent_limits`), so an account counts as limited from the moment one is
//! seen on screen until it has had time to reset or the person says otherwise.

use std::collections::HashMap;

/// After the person says an account works again, the message that may still be on screen is not taken for a new limit for this long.
pub const QUIET_AFTER_CLEAR: i64 = 10 * 60;

/// A limit is believed for this long when the tool's own reset time is not known: the shortest window the tools use is five hours.
pub const BELIEVED_FOR: i64 = 5 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Seconds since the epoch when the limit was first seen.
    pub since: i64,
    /// The reset time as the tool printed it ("3pm").
    pub resets: Option<String>,
}

/// The limits seen, by `(provider key, profile)`.
#[derive(Default)]
pub struct Limits {
    marks: HashMap<(String, u32), Mark>,
    /// Until when a limit message on screen is ignored, by account.
    quiet: HashMap<(String, u32), i64>,
}

impl gpui::Global for Limits {}

impl Limits {
    /// The person says the account is limited: always taken, and it ends any quiet time.
    pub fn mark_by_hand(&mut self, provider: &str, profile: u32, now: i64) {
        self.quiet.remove(&(provider.to_string(), profile));
        self.marks.insert((provider.to_string(), profile), Mark { since: now, resets: None });
    }

    /// A limit message is on screen for this account. The first sighting counts: the same message staying on screen does not
    /// push the end of the limit further out, but a reset time that was missing is filled in.
    pub fn mark(&mut self, provider: &str, profile: u32, resets: Option<String>, now: i64) {
        let key = (provider.to_string(), profile);
        if self.quiet.get(&key).is_some_and(|until| now < *until) {
            return;
        }
        match self.marks.get_mut(&key) {
            Some(mark) if now - mark.since <= BELIEVED_FOR => {
                if mark.resets.is_none() {
                    mark.resets = resets;
                }
            }
            _ => {
                self.marks.insert(key, Mark { since: now, resets });
            }
        }
    }

    /// The person says the account works again; the message still on screen is ignored for a while.
    pub fn clear(&mut self, provider: &str, profile: u32, now: i64) {
        let key = (provider.to_string(), profile);
        self.marks.remove(&key);
        self.quiet.insert(key, now + QUIET_AFTER_CLEAR);
    }

    /// The person said the account works again a short while ago: a limit message still on screen is theirs to ignore.
    pub fn is_quiet(&self, provider: &str, profile: u32, now: i64) -> bool {
        self.quiet.get(&(provider.to_string(), profile)).is_some_and(|until| now < *until)
    }

    /// The limit that is believed now, if any.
    pub fn limited(&self, provider: &str, profile: u32, now: i64) -> Option<&Mark> {
        self.marks.get(&(provider.to_string(), profile)).filter(|mark| now - mark.since <= BELIEVED_FOR)
    }

    /// The account to offer instead of `current`: not limited, and the least used of those (`used` is the tokens spent
    /// recently, `None` when unknown, which counts as nothing but loses a tie to an account known to be unused).
    /// `None` when every other account is limited, or there is none.
    pub fn best_other(&self, provider: &str, current: u32, count: u32, now: i64, used: &dyn Fn(u32) -> Option<u64>) -> Option<u32> {
        (0..count)
            .filter(|profile| *profile != current && self.limited(provider, *profile, now).is_none())
            .min_by_key(|profile| (used(*profile).unwrap_or(0), used(*profile).is_none(), *profile))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    #[test]
    fn a_limit_seen_on_screen_counts_until_it_has_had_time_to_reset() {
        let mut limits = Limits::default();
        assert!(limits.limited("claude", 0, NOW).is_none());
        limits.mark("claude", 0, Some("3pm".into()), NOW);
        assert_eq!(limits.limited("claude", 0, NOW + 60).and_then(|m| m.resets.as_deref()), Some("3pm"));
        assert!(limits.limited("claude", 0, NOW + BELIEVED_FOR + 1).is_none(), "an old limit is not believed any more");
        assert!(limits.limited("claude", 1, NOW).is_none(), "another account is unaffected");
        assert!(limits.limited("codex", 0, NOW).is_none(), "and so is another tool's");
    }

    #[test]
    fn the_same_message_staying_on_screen_does_not_extend_the_limit() {
        let mut limits = Limits::default();
        limits.mark("claude", 0, None, NOW);
        limits.mark("claude", 0, Some("3pm".into()), NOW + 3600);
        let mark = limits.limited("claude", 0, NOW + 3600).expect("limited");
        assert_eq!((mark.since, mark.resets.as_deref()), (NOW, Some("3pm")), "the first sighting stands, the missing time is filled in");
        // After it expired, a new sighting is a new limit.
        limits.mark("claude", 0, None, NOW + BELIEVED_FOR + 10);
        assert_eq!(limits.limited("claude", 0, NOW + BELIEVED_FOR + 10).map(|m| m.since), Some(NOW + BELIEVED_FOR + 10));
    }

    #[test]
    fn the_person_can_say_an_account_works_again() {
        let mut limits = Limits::default();
        limits.mark("claude", 0, None, NOW);
        limits.clear("claude", 0, NOW);
        assert!(limits.limited("claude", 0, NOW).is_none());
        // The old message is still on screen: it is not a new limit...
        limits.mark("claude", 0, None, NOW + 60);
        assert!(limits.limited("claude", 0, NOW + 60).is_none());
        // ...until the quiet time is over, or the person says otherwise.
        limits.mark("claude", 0, None, NOW + QUIET_AFTER_CLEAR + 1);
        assert!(limits.limited("claude", 0, NOW + QUIET_AFTER_CLEAR + 1).is_some());
        limits.clear("claude", 0, NOW + 1000);
        limits.mark_by_hand("claude", 0, NOW + 1010);
        assert!(limits.limited("claude", 0, NOW + 1010).is_some(), "by hand always counts");
    }

    #[test]
    fn the_account_offered_is_the_least_used_one_that_is_not_limited() {
        let mut limits = Limits::default();
        let used = |profile: u32| match profile {
            0 => Some(900),
            1 => Some(500),
            2 => Some(10),
            _ => None,
        };
        assert_eq!(limits.best_other("claude", 0, 3, NOW, &used), Some(2));
        limits.mark("claude", 2, None, NOW);
        assert_eq!(limits.best_other("claude", 0, 3, NOW, &used), Some(1), "a limited account is not offered");
        limits.mark("claude", 1, None, NOW);
        assert_eq!(limits.best_other("claude", 0, 3, NOW, &used), None, "nothing to offer when every other account is limited");
        assert_eq!(limits.best_other("claude", 0, 1, NOW, &used), None, "or when there is no other account");
    }

    #[test]
    fn an_account_with_unknown_use_loses_a_tie_to_one_known_to_be_unused() {
        let limits = Limits::default();
        let used = |profile: u32| (profile == 2).then_some(0);
        assert_eq!(limits.best_other("codex", 0, 3, NOW, &used), Some(2));
        assert_eq!(limits.best_other("codex", 0, 3, NOW, &|_| None), Some(1), "all unknown: the first other account");
    }
}
