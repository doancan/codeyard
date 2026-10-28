//! The system's accessibility display preferences: Reduce motion and Increase contrast (macOS, Windows).
//! Read at startup and again whenever a window comes forward (the user changes them in System Settings).
//! `workbench.reduceMotion` overrides the first; "Auto" themes follow the second.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{App, Global};

use crate::settings::Settings;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct SystemPrefs {
    pub reduce_motion: bool,
    pub increase_contrast: bool,
}

impl Global for SystemPrefs {}

#[cfg(target_os = "macos")]
fn read() -> SystemPrefs {
    let ws = objc2_app_kit::NSWorkspace::sharedWorkspace();
    SystemPrefs { reduce_motion: ws.accessibilityDisplayShouldReduceMotion(), increase_contrast: ws.accessibilityDisplayShouldIncreaseContrast() }
}

#[cfg(windows)]
fn read() -> SystemPrefs {
    use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let none = SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0);
    // "Show animations in Windows" (a BOOL); off means reduce motion.
    let mut animate: i32 = 1;
    // SAFETY: the pointers point at locals of the size the call writes.
    let animate_ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut animate as *mut i32 as *mut _), none) }.is_ok();
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    let hc_ok = unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, Some(&mut hc as *mut HIGHCONTRASTW as *mut _), none) }.is_ok();
    SystemPrefs { reduce_motion: animate_ok && animate == 0, increase_contrast: hc_ok && hc.dwFlags.contains(HCF_HIGHCONTRASTON) }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn read() -> SystemPrefs {
    SystemPrefs::default()
}

/// `reduce_motion` for drawing helpers that have no context (spinners, progress bars); kept current by `sync`.
static REDUCED: AtomicBool = AtomicBool::new(false);

/// Read the system preferences again; `true` if they changed. Tests keep what they set.
pub fn refresh(cx: &mut App) -> bool {
    if cfg!(test) && cx.has_global::<SystemPrefs>() {
        return false;
    }
    let now = if cfg!(test) { SystemPrefs::default() } else { read() };
    let changed = cx.try_global::<SystemPrefs>() != Some(&now);
    cx.set_global(now);
    sync(cx);
    changed
}

/// After the settings or the system preferences change.
pub fn sync(cx: &App) {
    REDUCED.store(reduce_motion(cx), Ordering::Relaxed);
}

/// `reduce_motion` as of the last `sync`, for code without a context.
pub fn motion_reduced() -> bool {
    REDUCED.load(Ordering::Relaxed)
}

/// Whether to leave out animations: `workbench.reduceMotion`, or the system when it is "auto".
pub fn reduce_motion(cx: &App) -> bool {
    cx.try_global::<Settings>().and_then(|s| s.reduce_motion).unwrap_or_else(|| cx.try_global::<SystemPrefs>().is_some_and(|p| p.reduce_motion))
}

/// Whether the system asks for more contrast ("Auto" themes switch to the high-contrast pair).
pub fn increase_contrast(cx: &App) -> bool {
    cx.try_global::<SystemPrefs>().is_some_and(|p| p.increase_contrast)
}
