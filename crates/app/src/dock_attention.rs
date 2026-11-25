//! The Dock icon says when a project needs the person (agent mode): a badge with how many do, and one bounce when one more
//! does while Codeyard is in the background. macOS only; other systems have no equivalent that GPUI can ask for yet (Windows
//! flashes the taskbar button from the terminal's own attention request), so there nothing happens. Tests never touch the
//! real Dock.

/// The badge on the Dock icon: `count` projects need the person; zero removes it.
#[cfg(all(target_os = "macos", not(test)))]
pub fn set_badge(count: usize) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
    NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(label.as_deref());
}

/// One bounce of the Dock icon (it stops by itself; a critical request would bounce until the app is opened).
#[cfg(all(target_os = "macos", not(test)))]
pub fn bounce() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
    let Some(mtm) = MainThreadMarker::new() else { return };
    NSApplication::sharedApplication(mtm).requestUserAttention(NSRequestUserAttentionType::InformationalRequest);
}

#[cfg(not(all(target_os = "macos", not(test))))]
pub fn set_badge(_: usize) {}

#[cfg(not(all(target_os = "macos", not(test))))]
pub fn bounce() {}
