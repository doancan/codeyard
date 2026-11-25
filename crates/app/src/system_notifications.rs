//! Banners in macOS's Notification Center for agent mode: a project that waits, hits a limit or finishes while Codeyard is in the
//! background says so, and a click on the banner brings that project up (`take_opened`, asked by the window that draws next).
//!
//! The project travels in the request's identifier (`codeyard-project-<window id>-<n>`), so no dictionary crosses the Objective-C
//! boundary. `UNUserNotificationCenter` needs an application bundle: run unbundled (`cargo run`, a bare binary) it raises an
//! Objective-C exception, so without a bundle identifier nothing is done. Elsewhere, and in tests, nothing is done either.

use std::sync::Mutex;

/// Projects whose banner was clicked, oldest first.
static OPENED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// The identifier of a request for project `project`; `serial` keeps banners of one project apart.
pub fn request_id(project: u64, serial: u64) -> String {
    format!("codeyard-project-{project}-{serial}")
}

/// The project a request identifier names.
pub fn project_of(identifier: &str) -> Option<u64> {
    identifier.strip_prefix("codeyard-project-")?.split('-').next()?.parse().ok()
}

/// A banner was clicked.
#[cfg_attr(not(all(target_os = "macos", not(test))), allow(dead_code))]
fn opened(project: u64) {
    if let Ok(mut list) = OPENED.lock() {
        list.push(project);
    }
}

/// The projects whose banners were clicked since the last call.
pub fn take_opened() -> Vec<u64> {
    OPENED.lock().map(|mut list| std::mem::take(&mut *list)).unwrap_or_default()
}

#[cfg(all(target_os = "macos", not(test)))]
mod mac {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU64, Ordering};

    use block2::{DynBlock, RcBlock};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };

    define_class!(
        // Receives the click on a banner.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "CodeyardNotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(&self, _center: &UNUserNotificationCenter, response: &UNNotificationResponse, done: &DynBlock<dyn Fn()>) {
                let identifier = response.notification().request().identifier().to_string();
                if let Some(project) = super::project_of(&identifier) {
                    super::opened(project);
                }
                done.call(());
            }
        }
    );

    thread_local! {
        /// The delegate lives as long as the process: the center keeps only a weak reference.
        static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    }

    static SERIAL: AtomicU64 = AtomicU64::new(0);

    /// Sets the delegate and asks for permission, the first time only.
    fn prepare(mtm: MainThreadMarker, center: &UNUserNotificationCenter) {
        DELEGATE.with(|slot| {
            if slot.borrow().is_some() {
                return;
            }
            let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(mtm), init] };
            center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            // The system asks the person once; a refusal only means that no banner shows.
            let done = RcBlock::new(|_granted: objc2::runtime::Bool, _error: *mut NSError| {});
            center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &done);
            *slot.borrow_mut() = Some(delegate);
        });
    }

    pub fn post(title: &str, body: &str, project: u64) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        // Without a bundle identifier (an unbundled run) the center raises an exception instead of answering.
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        prepare(mtm, &center);
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let identifier = NSString::from_str(&super::request_id(project, SERIAL.fetch_add(1, Ordering::Relaxed)));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
        center.addNotificationRequest_withCompletionHandler(&request, None);
    }
}

/// A banner for `project`, when Codeyard runs as a bundled application on macOS and the person allowed banners.
#[cfg(all(target_os = "macos", not(test)))]
pub fn post(title: &str, body: &str, project: u64) {
    mac::post(title, body, project);
}

#[cfg(not(all(target_os = "macos", not(test))))]
pub fn post(_: &str, _: &str, _: u64) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_project_travels_in_the_request_identifier() {
        let id = request_id(42, 7);
        assert_eq!(id, "codeyard-project-42-7");
        assert_eq!(project_of(&id), Some(42));
        assert_eq!(project_of("codeyard-project-18446744073709551615-0"), Some(u64::MAX));
    }

    #[test]
    fn an_identifier_that_is_not_ours_names_no_project() {
        for other in ["", "codeyard-project-", "codeyard-project-x-1", "other-42-1", "codeyard-project--1"] {
            assert_eq!(project_of(other), None, "{other:?}");
        }
    }

    #[test]
    fn a_click_is_handed_over_once() {
        take_opened();
        opened(3);
        opened(5);
        assert_eq!(take_opened(), [3, 5]);
        assert!(take_opened().is_empty());
    }
}
