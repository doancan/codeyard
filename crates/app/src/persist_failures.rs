//! Failures that happen where nothing can post a notification (a `&self` method that saves the session): they are queued here and
//! posted at the next frame. A failure that would repeat on every save (a read-only state database) is reported once per session
//! per area. No UI and no clock.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt::Display;
use std::path::Path;

use super::model::{NoticeAction, NoticeSource, Notification, Severity};
use crate::i18n::t;

/// What could not be saved; each area has its own sentence (sentences are never assembled from translated parts).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Area {
    Tabs,
    WindowPosition,
    Explorer,
    ExtraFolders,
    Terminals,
    /// What the update flow remembers (the last check, the version skipped): without it Codeyard asks or checks again.
    UpdateChoice,
}

impl Area {
    fn key(self) -> &'static str {
        match self {
            Area::Tabs => "persist:tabs",
            Area::WindowPosition => "persist:window",
            Area::Explorer => "persist:explorer",
            Area::ExtraFolders => "persist:folders",
            Area::Terminals => "persist:terminals",
            Area::UpdateChoice => "persist:update",
        }
    }

    fn message(self) -> String {
        use crate::i18n::tr;
        t(match self {
            Area::Tabs => tr("Could not save your open tabs and layout; they will not be restored next time."),
            Area::WindowPosition => tr("Could not save the window position; it will not be restored next time."),
            Area::Explorer => tr("Could not save the open folders of the Explorer; they will not be restored next time."),
            Area::ExtraFolders => tr("Could not save your additional folders; they will not be restored next time."),
            Area::Terminals => tr("Could not save your terminals; they will not be restored next time."),
            Area::UpdateChoice => tr("Could not remember your update choices; Codeyard may ask or check again."),
        })
        .to_string()
    }
}

#[derive(Default)]
pub struct Deferred {
    queue: RefCell<Vec<Notification>>,
    reported: RefCell<HashSet<String>>,
}

impl Deferred {
    /// Queue `n`, once per session for each `once` key; `false` when it was already queued before.
    pub fn defer_once(&self, once: &str, n: Notification) -> bool {
        if !self.reported.borrow_mut().insert(once.to_string()) {
            return false;
        }
        self.queue.borrow_mut().push(n);
        true
    }

    /// A save of `area` failed with `err`. One warning per area and session, with the state folder to look at.
    pub fn persist_failed(&self, area: Area, err: &dyn Display, state_dir: Option<&Path>) -> bool {
        let mut n = Notification::new(Severity::Warning, NoticeSource::App, area.message()).id(area.key()).detail(match state_dir {
            Some(dir) => format!("{err}\n{}", dir.display()),
            None => err.to_string(),
        });
        if let Some(dir) = state_dir {
            n = n.action(NoticeAction::Reveal(dir.to_path_buf()));
        }
        self.defer_once(area.key(), n)
    }

    pub fn take(&self) -> Vec<Notification> {
        std::mem::take(&mut *self.queue.borrow_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_area_reports_once_per_session() {
        let d = Deferred::default();
        let dir = Path::new("/state");
        assert!(d.persist_failed(Area::Tabs, &"disk I/O error", Some(dir)));
        assert!(!d.persist_failed(Area::Tabs, &"disk I/O error", Some(dir)));
        assert!(d.persist_failed(Area::Explorer, &"disk I/O error", Some(dir)), "another area has its own warning");
        let posted = d.take();
        assert_eq!(posted.len(), 2);
        assert_eq!(posted[0].severity, Severity::Warning);
        assert_eq!(posted[0].action, Some(NoticeAction::Reveal(dir.to_path_buf())));
        assert!(posted[0].detail.as_deref().is_some_and(|d| d.contains("disk I/O error") && d.contains("/state")));
        assert!(d.take().is_empty(), "taking empties the queue");
    }

    #[test]
    fn without_a_state_folder_there_is_no_reveal_button() {
        let d = Deferred::default();
        d.persist_failed(Area::WindowPosition, &"x", None);
        assert_eq!(d.take()[0].action, None);
    }
}
