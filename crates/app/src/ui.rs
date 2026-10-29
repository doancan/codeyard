//! Shared UI pieces: theme, icons, fonts. Colors and sizes come only
//! from the `theme` crate (design tokens).

use std::borrow::Cow;

use gpui::{App, AssetSource, Global, Rgba, SharedString, Styled, Svg, px, rgb, svg};
pub use theme::{Palette, layout, text};

pub struct ActiveTheme(pub Palette);

impl Global for ActiveTheme {}

/// Whether the last input was the keyboard. Focus rings show only then: a click lights the button it presses instead of
/// outlining it, and Tab still shows where the focus went.
#[derive(Default)]
struct InputModality {
    keyboard: bool,
}

impl Global for InputModality {}

pub fn keyboard_focus(cx: &App) -> bool {
    cx.try_global::<InputModality>().is_some_and(|m| m.keyboard)
}

pub fn set_keyboard_focus(keyboard: bool, cx: &mut App) {
    if keyboard_focus(cx) != keyboard {
        cx.set_global(InputModality { keyboard });
    }
}

pub fn palette(cx: &App) -> &Palette {
    &cx.global::<ActiveTheme>().0
}

pub fn c(hex: u32) -> Rgba {
    rgb(hex)
}

/// Perceived brightness is high (light theme background).
pub fn luminance_is_light(hex: u32) -> bool {
    let (r, g, b) = (((hex >> 16) & 0xff) as f32, ((hex >> 8) & 0xff) as f32, (hex & 0xff) as f32);
    0.2126 * r + 0.7152 * g + 0.0722 * b > 140.
}

/// Darken a color (for readable badge text on a light background).
pub fn darken(hex: u32) -> u32 {
    let ch = |shift: u32| (((hex >> shift) & 0xff) as f32 * 0.55) as u32;
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// 0xRRGGBB color with alpha (0.0-1.0).
/// Mix two colors: `t` = 0 gives `a`, 1 gives `b`. For when a solid color is needed instead of transparency (hover background, border).
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0., 1.);
    let ch = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32).min(255)
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Shift `color` toward `toward` (usually the text color) until it has `min` contrast against every
/// given background; unchanged if already enough. Keeps status colors readable on hover and selected
/// backgrounds in third-party themes where they fall below the limit.
pub fn ensure_contrast(color: u32, toward: u32, backgrounds: &[u32], min: f64) -> u32 {
    let ok = |c: u32| backgrounds.iter().all(|&bg| theme::themes::contrast(c, bg) >= min);
    if ok(color) {
        return color;
    }
    for step in 1..=20 {
        let candidate = mix(color, toward, step as f32 * 0.05);
        if ok(candidate) {
            return candidate;
        }
    }
    toward
}

/// Mix `tint` into the `base` background by at most `fraction`; if `text` would drop below 4.6 contrast on it,
/// retry with the fraction halved (`base` in the worst case). A hover/selection fill must not hurt the text.
pub fn tinted_bg(base: u32, tint: u32, fraction: f32, text: u32) -> u32 {
    let mut f = fraction;
    for _ in 0..5 {
        let c = mix(base, tint, f);
        if theme::themes::contrast(text, c) >= 4.6 {
            return c;
        }
        f /= 2.;
    }
    base
}

/// Selection fill of the focused explorer row: a light accent over the selected background, just enough to keep text readable.
pub fn focused_selection(p: &Palette) -> u32 {
    tinted_bg(p.surface_active, p.accent, 0.18, p.text)
}

/// Normal border of an input box: between `border` and `border_input` (soft), but never below 3:1, the UI component contrast, against the
/// backgrounds the box sits on (if it would, it is shifted toward the text color).
pub fn input_border(p: &Palette) -> u32 {
    ensure_contrast(mix(p.border, p.border_input, 0.75), p.text, &[p.bg, p.surface, p.surface_raised], 3.0)
}

/// Input box: 28 px, rounded corners; soft border normally, stronger toward `border_input` on hover,
/// accent color when focused. Holds (an optional magnifier,) the field and buttons; `focused` says whether the field inside the box
/// has focus.
pub fn input_box(id: impl Into<gpui::ElementId>, p: &Palette, bg: u32, focused: bool) -> gpui::Stateful<gpui::Div> {
    use gpui::{InteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _};
    let (normal, hover_border) = (input_border(p), p.border_input);
    div()
        .id(id)
        .h(px(28.))
        .px(px(theme::SPACE[3]))
        .gap(px(theme::SPACE[2]))
        .flex()
        .items_center()
        .bg(c(bg))
        .border_1()
        .border_color(c(if focused { p.accent } else { normal }))
        .rounded(px(theme::radius::MD))
        .when(!focused, move |d| d.hover(move |s| s.border_color(c(hover_border))))
}

/// Magnifier at the start of the input box.
pub fn search_icon(p: &Palette) -> gpui::Svg {
    icon("search", 14., p.text_muted)
}

/// Option button (Aa, ab, .*, AB): 22×20, when on, background and text slide to the accent color; when off, dimmed, appears on
/// hover. The caller binds the click.
pub fn option_toggle(id: &'static str, label: &'static str, active: bool, tip: &'static str, p: &Palette, box_bg: u32) -> gpui::Stateful<gpui::Div> {
    use gpui::{AppContext as _, InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _};
    let bg = if active { mix(box_bg, p.accent, 0.26) } else { box_bg };
    let fg = if active { ensure_contrast(p.accent, p.text, &[bg], 4.6) } else { p.text_muted };
    let hover = p.surface_hover;
    let text_color = p.text;
    div()
        .id(id)
        .flex_none()
        .min_w(px(22.))
        .h(px(20.))
        .px(px(theme::SPACE[2]))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::radius::SM))
        .bg(c(bg))
        .text_size(px(text::XXS.0))
        .font_family(MONO_FONT)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(c(fg))
        .cursor_pointer()
        .when(!active, move |d| d.hover(move |s| s.bg(c(hover)).text_color(c(text_color))))
        .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.into())).into())
        .child(label)
}

/// Most readable solid color on `bg`: black or white (avatar initials, filled chips).
pub fn readable_on(bg: u32) -> u32 {
    let (white, black) = (theme::themes::contrast(0xffffff, bg), theme::themes::contrast(0x000000, bg));
    if white >= black { 0xffffff } else { 0x000000 }
}

pub fn ca(hex: u32, alpha: f32) -> Rgba {
    let mut c = rgb(hex);
    c.a = alpha;
    c
}

#[cfg(target_os = "macos")]
pub const UI_FONT: &str = ".SystemUIFont";
#[cfg(target_os = "macos")]
pub const MONO_FONT: &str = "Menlo";
#[cfg(target_os = "windows")]
pub const UI_FONT: &str = "Segoe UI";
#[cfg(target_os = "windows")]
pub const MONO_FONT: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const UI_FONT: &str = "Ubuntu";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const MONO_FONT: &str = "DejaVu Sans Mono";

pub fn icon(name: &str, size: f32, color: u32) -> Svg {
    svg().path(format!("icons/{name}.svg")).size(px(size)).flex_none().text_color(c(color))
}

/// Spinning "loading" icon (one turn per second; still with reduced motion). `id` must be unique among siblings.
pub fn spinner(id: impl Into<gpui::ElementId>, size: f32, color: u32) -> gpui::AnyElement {
    use gpui::{Animation, AnimationExt as _, IntoElement as _, Transformation, percentage};
    if crate::system_prefs::motion_reduced() {
        return icon("loader", size, color).into_any_element();
    }
    icon("loader", size, color)
        .with_animation(id, Animation::new(std::time::Duration::from_millis(1000)).repeat(), |svg, delta| {
            svg.with_transformation(Transformation::rotate(percentage(delta)))
        })
        .into_any_element()
}

/// "Something runs here": three bars rising and falling out of step, `size` wide and high. Still bars with reduced motion.
/// `id` must be unique among siblings.
pub fn activity_bars(id: impl Into<gpui::ElementId>, size: f32, color: u32) -> gpui::AnyElement {
    use gpui::{Animation, AnimationExt as _, IntoElement as _, ParentElement as _, Styled as _, div};
    let id: gpui::ElementId = id.into();
    let bar_w = (size / 5.).round().max(2.);
    let still = crate::system_prefs::motion_reduced();
    let bars = (0..3).map(|i| {
        // Resting heights for reduced motion, and the phase of each bar while it moves.
        let rest = [0.55, 0.9, 0.7][i];
        let bar = div().w(px(bar_w)).rounded(px(bar_w / 2.)).bg(c(color));
        if still {
            return bar.h(px(size * rest)).into_any_element();
        }
        let phase = i as f32 / 3.;
        bar.h(px(size * rest))
            .with_animation(
                gpui::ElementId::NamedChild(Box::new(id.clone()), ["a", "b", "c"][i].into()),
                Animation::new(std::time::Duration::from_millis(900)).repeat(),
                move |d, delta| {
                    let t = ((delta + phase) % 1.) * std::f32::consts::TAU;
                    d.h(px(size * (0.3 + 0.7 * (0.5 - 0.5 * t.cos()))))
                },
            )
            .into_any_element()
    });
    div().size(px(size)).flex_none().flex().items_end().justify_between().children(bars).into_any_element()
}

/// Lucide (ISC) icons, embedded in the binary.
pub struct Assets;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*]
    };
}

const ICONS: &[(&str, &[u8])] = icons![
    "activity",
    "alert",
    "arrow-down",
    "arrow-up",
    "check",
    "chevron-down",
    "chevron-right",
    "chevron-up",
    "circle-x",
    "file",
    "file-code",
    "file-image",
    "file-json",
    "file-terminal",
    "file-text",
    "folder",
    "folder-open",
    "git-branch",
    "git-compare",
    "info",
    "loader",
    "lock",
    "minus",
    "panel-left",
    "pin",
    "plug",
    "plus",
    "refresh-cw",
    "search",
    "server",
    "sliders",
    "terminal",
    "trash",
    "undo-2",
    "x",
    "file-plus",
    "folder-plus",
    "star",
    "bookmark",
    "chevrons-down-up",
    "play",
    "command",
    "settings",
    "files",
    "square-minus",
    "maximize-2",
    "minimize-2",
    "panel-right",
    "panel-bottom",
    "git-commit",
    "history",
    "copy",
    "list",
    "keyboard",
    "palette",
    "lightbulb",
    "book-open",
    "eye",
    "code",
    "clock",
    "tag",
    "regex",
    "chevrons-up-down",
    "sun",
    "external-link",
    "rocket",
    "list-tree",
    "scissors",
    "align-left",
    "eraser",
    "folder-tree",
    "pause",
    "square",
    "step-over",
    "step-into",
    "step-out",
    "rotate-ccw",
    "folder-solid",
    "folder-open-solid",
    "file-solid",
    "codeyard-mark",
    "ellipsis",
    // --- panel toolbar: one optical size for every glyph ---
    "tb-plus",
    "tb-dropdown",
    "tb-split",
    "tb-trash",
    "tb-panel-right",
    "tb-panel-bottom",
    "tb-maximize",
    "tb-restore",
    "tb-close",
    // --- git-sync ---
    "cloud-upload",
    "cloud-download",
    "arrow-left",
    "arrow-right",
    // --- remote ---
    "screen-share",
    "container",
    "key-round",
    "pencil",
    // --- run and debug ---
    "bug-play",
    "circle-slash-2",
    // --- testing ---
    "flask-conical",
    "circle-check",
    "circle-dashed",
    "list-filter",
    "blocks",
];

/// Embedded icon names (tests: is every name used in source embedded, does every embedded file exist).
#[cfg(test)]
pub fn embedded_icon_names() -> Vec<&'static str> {
    ICONS.iter().filter_map(|(p, _)| p.strip_prefix("icons/")?.strip_suffix(".svg")).collect()
}

/// (icon, color) by file name. Colors come from the theme's syntax roles: they follow theme changes.
pub fn file_icon(name: &str, p: &Palette) -> (&'static str, u32) {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" => ("file-code", p.syntax_tag),
        "ts" | "tsx" | "mts" | "cts" => ("file-code", p.info),
        "js" | "jsx" | "mjs" | "cjs" => ("file-code", p.syntax_number),
        "py" | "pyi" => ("file-code", p.syntax_function),
        "go" => ("file-code", p.syntax_type),
        "c" | "h" | "cpp" | "hpp" | "cc" | "java" | "kt" | "swift" | "rb" | "php" | "lua" | "zig" => ("file-code", p.syntax_keyword),
        "html" | "htm" | "vue" | "svelte" => ("file-code", p.syntax_tag),
        "css" | "scss" | "sass" | "less" => ("file-code", p.syntax_keyword),
        "json" | "jsonc" => ("file-json", p.syntax_number),
        "toml" | "yaml" | "yml" | "ini" | "conf" | "env" => ("file-json", p.text_muted),
        "sh" | "bash" | "zsh" | "fish" => ("file-terminal", p.success),
        "md" | "mdx" | "txt" | "rst" => ("file-text", p.info),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => ("file-image", p.syntax_string),
        "lock" => ("file", p.text_muted),
        _ => match name {
            "Dockerfile" | "Makefile" | "Justfile" => ("file-terminal", p.info),
            n if n.starts_with('.') => ("file", p.text_muted),
            _ => ("file", p.text_muted),
        },
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICONS.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}

/// POSIX path join (local macOS/Linux and remote servers).
pub fn join(dir: &str, name: &str) -> String {
    // An absolute name stays as is: files in the workspace's extra folders are kept
    // with absolute paths in persisted records (tabs, bookmarks).
    if is_absolute_path(name) {
        return name.to_string();
    }
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

pub fn is_absolute_path(path: &str) -> bool {
    path.starts_with('/') || windows_path(path)
}

pub fn file_name(path: &str) -> &str {
    if windows_path(path) {
        path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(path)
    } else {
        path.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(path)
    }
}

fn windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\") || bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\')
}

/// Keep local Windows paths in the same slash-separated form as workspace paths.
/// Rust adds the extended path prefix again when needed for long filesystem paths.
pub fn local_path(path: &std::path::Path) -> String {
    let path = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    let path = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!("//{rest}").replace('\\', "/")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).replace('\\', "/")
    };
    path
}

/// Bridge a blocking channel to an async channel: a small separate thread does the receiving,
/// the UI side `await`s. Doesn't block the executor thread (the test executor either).
pub fn bridge<T: Send + 'static>(rx: std::sync::mpsc::Receiver<T>) -> futures::channel::mpsc::UnboundedReceiver<T> {
    let (tx, out) = futures::channel::mpsc::unbounded();
    std::thread::Builder::new()
        .name("bridge".into())
        .spawn(move || {
            while let Ok(item) = rx.recv() {
                if tx.unbounded_send(item).is_err() {
                    break; // receiver is gone
                }
            }
        })
        .expect("bridge thread");
    out
}

/// `bridge`, but each item passes `admit` on the bridge thread: if it returns `false` the item is dropped (so memory doesn't balloon if the UI lags).
pub fn bridge_with<T: Send + 'static>(
    rx: std::sync::mpsc::Receiver<T>,
    mut admit: impl FnMut(&T) -> bool + Send + 'static,
) -> futures::channel::mpsc::UnboundedReceiver<T> {
    let (tx, out) = futures::channel::mpsc::unbounded();
    std::thread::Builder::new()
        .name("bridge".into())
        .spawn(move || {
            while let Ok(item) = rx.recv() {
                if admit(&item) && tx.unbounded_send(item).is_err() {
                    break;
                }
            }
        })
        .expect("bridge thread");
    out
}

/// The receiving end of `bridge_coalescing`: a queue the producer thread folds new items into while the UI is busy.
pub struct Coalesced<T> {
    shared: std::sync::Arc<CoalescedShared<T>>,
}

struct CoalescedShared<T> {
    queue: std::sync::Mutex<std::collections::VecDeque<T>>,
    waker: futures::task::AtomicWaker,
    closed: std::sync::atomic::AtomicBool,
}

impl<T> futures::Stream for Coalesced<T> {
    type Item = T;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<T>> {
        use std::task::Poll;
        let shared = &self.shared;
        let next = |shared: &CoalescedShared<T>| shared.queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
        if let Some(item) = next(shared) {
            return Poll::Ready(Some(item));
        }
        shared.waker.register(cx.waker());
        // An item may have arrived between the first look and the registration.
        if let Some(item) = next(shared) {
            return Poll::Ready(Some(item));
        }
        if shared.closed.load(std::sync::atomic::Ordering::Acquire) {
            // The producer closed after the last push: look once more so nothing is lost.
            return Poll::Ready(next(shared));
        }
        Poll::Pending
    }
}

impl<T> Drop for Coalesced<T> {
    fn drop(&mut self) {
        self.shared.closed.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// `bridge` for events where only the latest state of something matters (diagnostics of a file, a progress line, the changed
/// paths of a directory): each new item is handed to `merge` together with the items the UI has not taken yet, and `merge` may
/// fold it into one of them instead of appending. A slow UI therefore holds one item per distinct key, not one per message.
/// Nothing is ever dropped without being merged: `merge` must keep every piece of information that matters.
pub fn bridge_coalescing<T: Send + 'static>(
    rx: std::sync::mpsc::Receiver<T>,
    mut merge: impl FnMut(&mut std::collections::VecDeque<T>, T) + Send + 'static,
) -> Coalesced<T> {
    use std::sync::atomic::Ordering;
    let shared = std::sync::Arc::new(CoalescedShared {
        queue: std::sync::Mutex::default(),
        waker: futures::task::AtomicWaker::new(),
        closed: std::sync::atomic::AtomicBool::new(false),
    });
    let producer = shared.clone();
    std::thread::Builder::new()
        .name("bridge".into())
        .spawn(move || {
            while let Ok(item) = rx.recv() {
                if producer.closed.load(Ordering::Acquire) {
                    return; // the receiver is gone
                }
                merge(&mut producer.queue.lock().unwrap_or_else(|e| e.into_inner()), item);
                producer.waker.wake();
            }
            producer.closed.store(true, Ordering::Release);
            producer.waker.wake();
        })
        .expect("bridge thread");
    Coalesced { shared }
}

/// Small icon button (view headers, panel tools): 22 px, hover background,
/// tooltip required (it's icon-only, so what it does must be written).
/// A toolbar button that stays pressed while `on` (a maximized panel): pressed background, full-strength glyph and a hover
/// of its own so it still answers the pointer.
pub fn toggle_button(id: impl Into<gpui::ElementId>, glyph: &'static str, tip: &'static str, on: bool, p: &Palette) -> gpui::Stateful<gpui::Div> {
    use gpui::{InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _, prelude::FluentBuilder as _};
    let (bg, hover, fg) = if on { (Some(p.surface_active), p.selection, p.text) } else { (None, p.surface_hover, p.text_muted) };
    gpui::div()
        .id(id)
        .size(gpui::px(22.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(gpui::px(theme::radius::SM))
        .cursor_pointer()
        .when_some(bg, |d, bg| d.bg(c(bg)))
        .hover(move |d| d.bg(c(hover)))
        .active(|d| d.bg(c(p.selection)))
        .child(icon(glyph, 16., fg))
        .tooltip(move |_, cx| gpui::AppContext::new(cx, |_| crate::workbench::Tooltip(tip.into())).into())
}

pub fn icon_button(id: impl Into<gpui::ElementId>, glyph: &'static str, tip: &'static str, p: &Palette) -> gpui::Stateful<gpui::Div> {
    use gpui::{InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _};
    gpui::div()
        .id(id)
        .size(gpui::px(22.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(gpui::px(theme::radius::SM))
        .cursor_pointer()
        .hover(|d| d.bg(c(p.surface_hover)))
        .active(|d| d.bg(c(p.surface_active)))
        .child(icon(glyph, 16., p.text_muted))
        .tooltip(move |_, cx| gpui::AppContext::new(cx, |_| crate::workbench::Tooltip(tip.into())).into())
}

/// Two-line text column of a commit row (git graph and Timeline share this layout):
/// subject on top and time at the right, `meta` below (grows from the left) and short hash at the right. Both lines
/// have fixed height, text on the same baseline; the hash is always rightmost and aligned.
pub fn commit_lines(subject: SharedString, when: SharedString, meta: impl gpui::IntoElement, hash: SharedString, bold: bool, p: &Palette) -> gpui::Div {
    use gpui::{FontWeight, div, prelude::*};
    let line = || div().h(px(18.)).flex().items_center().gap(px(theme::SPACE[3])).overflow_hidden().whitespace_nowrap();
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(4.))
        .pr(px(theme::SPACE[3]))
        .child(
            line()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_size(px(text::SM.0))
                        .text_color(c(p.text))
                        .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .child(subject),
                )
                .child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(when)),
        )
        .child(
            line()
                .text_size(px(text::XXS.0))
                .text_color(c(p.text_muted))
                .child(div().flex_1().min_w_0().flex().items_center().gap(px(theme::SPACE[2])).overflow_hidden().child(meta))
                .child(
                    div()
                        .flex_none()
                        .font_family(MONO_FONT)
                        .text_size(px(text::XXS.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c(ensure_contrast(mix(p.text_muted, p.accent, 0.6), p.text, &[p.bg, p.surface, p.surface_hover, p.surface_active], 4.6)))
                        .child(hash),
                ),
        )
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn windows_absolute_paths_are_not_joined_to_the_workspace() {
        assert_eq!(join("C:/work", r"D:\other\main.rs"), r"D:\other\main.rs");
        assert_eq!(join("C:/work", r"\\server\share\main.rs"), r"\\server\share\main.rs");
        assert_eq!(file_name(r"C:\şablon boşluk\main.rs"), "main.rs");
        assert_eq!(file_name(r"\\server\share\folder\"), "folder");
        assert_eq!(file_name("/home/ada/a\\b.rs"), "a\\b.rs", "a backslash can be part of a POSIX filename");
        assert_eq!(join("/home/ada", "/srv/app.rs"), "/srv/app.rs");
    }

    #[cfg(windows)]
    #[test]
    fn windows_local_paths_normalize_drive_and_unc_prefixes() {
        assert_eq!(local_path(std::path::Path::new(r"\\?\C:\şablon boşluk\main.rs")), "C:/şablon boşluk/main.rs");
        assert_eq!(local_path(std::path::Path::new(r"\\?\UNC\server\share\main.rs")), "//server/share/main.rs");
        assert_eq!(local_path(std::path::Path::new(r"C:\work\")), "C:/work/");
    }
}

#[cfg(test)]
mod mix_tests {
    use super::mix;

    #[test]
    fn mixing_is_linear_and_clamped() {
        assert_eq!(mix(0x000000, 0xffffff, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 1.0), 0xffffff);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(mix(0x102030, 0x102030, 0.7), 0x102030);
        assert_eq!(mix(0x102030, 0xffffff, 9.0), 0xffffff, "t is clamped");
        assert_eq!(mix(0x102030, 0xffffff, -3.0), 0x102030);
    }
}

#[cfg(test)]
mod contrast_tests {
    use super::ensure_contrast;

    #[test]
    fn readable_colors_are_left_alone_and_weak_ones_are_pushed_toward_the_text_color() {
        // Already readable: unchanged.
        assert_eq!(ensure_contrast(0xffffff, 0xffffff, &[0x000000], 4.5), 0xffffff);
        // Dark gray on a dark background: lightens toward the text color (white) and passes the threshold.
        let fixed = ensure_contrast(0x404040, 0xffffff, &[0x1b1b1d, 0x2c2c30], 4.5);
        assert!(theme::themes::contrast(fixed, 0x2c2c30) >= 4.5, "{fixed:06x}");
        assert_ne!(fixed, 0x404040);
        // Unreachable threshold: the target color in the worst case.
        assert_eq!(ensure_contrast(0x808080, 0x808080, &[0x808080], 21.0), 0x808080);
    }
}

#[cfg(test)]
mod readable_tests {
    use super::readable_on;

    #[test]
    fn picks_the_color_with_more_contrast() {
        assert_eq!(readable_on(0x000000), 0xffffff);
        assert_eq!(readable_on(0xffffff), 0x000000);
        assert_eq!(readable_on(0xf2c744), 0x000000, "yellow needs dark text");
        assert_eq!(readable_on(0x1a5bd1), 0xffffff, "deep blue needs light text");
    }
}

#[cfg(test)]
mod coalescing_tests {
    use super::*;
    use futures::StreamExt as _;
    use std::collections::VecDeque;

    fn by_key(q: &mut VecDeque<(u8, u32)>, item: (u8, u32)) {
        match q.iter_mut().find(|(k, _)| *k == item.0) {
            Some(slot) => *slot = item,
            None => q.push_back(item),
        }
    }

    /// Fault injection, slow consumer: a million updates arrive for three keys before the UI reads anything; it then
    /// sees three items with the latest values, in the order the keys first appeared, and the stream ends.
    #[test]
    fn a_slow_consumer_gets_the_latest_value_per_key_and_the_stream_still_ends() {
        let (tx, rx) = std::sync::mpsc::channel::<(u8, u32)>();
        let mut out = bridge_coalescing(rx, by_key);
        for i in 0..1_000_000u32 {
            tx.send(((i % 3) as u8, i)).unwrap();
        }
        drop(tx);
        // Wait for the producer thread to finish folding (it sets `closed` when the channel ends) so the result is exact.
        let start = std::time::Instant::now();
        while !out.shared.closed.load(std::sync::atomic::Ordering::Acquire) && start.elapsed() < std::time::Duration::from_secs(30) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(out.shared.queue.lock().unwrap().len(), 3, "a million updates for three keys are three queued items");
        let got: Vec<(u8, u32)> = futures::executor::block_on(out.by_ref().collect());
        assert_eq!(got, vec![(0, 999_999), (1, 999_997), (2, 999_998)]);
    }

    /// A consumer that waits is woken by the next item, and items sent after the first read are not lost.
    #[test]
    fn an_awaiting_consumer_is_woken_and_nothing_is_lost_without_a_merge() {
        let (tx, rx) = std::sync::mpsc::channel::<u32>();
        let mut out = bridge_coalescing(rx, |q, i| q.push_back(i));
        let producer = std::thread::spawn(move || {
            for i in 0..2000u32 {
                tx.send(i).unwrap();
                if i % 500 == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        });
        let got: Vec<u32> = futures::executor::block_on(out.by_ref().collect());
        producer.join().unwrap();
        assert_eq!(got, (0..2000).collect::<Vec<_>>());
    }

    /// Dropping the receiver releases the producer thread at the next item (no thread and no queue are left behind).
    #[test]
    fn dropping_the_receiver_stops_the_producer() {
        let (tx, rx) = std::sync::mpsc::channel::<u32>();
        let out = bridge_coalescing(rx, |q, i| q.push_back(i));
        drop(out);
        tx.send(1).unwrap(); // the thread notices and exits ...
        let start = std::time::Instant::now();
        while tx.send(2).is_ok() && start.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(tx.send(3).is_err(), "... which closes the channel");
    }
}
