//! Context menu: opens at the mouse position (or from a given corner), navigated with the keyboard. The look follows the menu of the platform it runs on
//! (`Flavor`): on macOS rounded corners and a blue highlight; on Windows and Linux a neutral highlight and taller rows.

use gpui::{App, BoxShadow, Context, Corner, EventEmitter, FocusHandle, Focusable, MouseButton, SharedString, Window, actions, div, point, prelude::*, px};

use crate::ui::{c, ca, palette};

actions!(menu, [MenuPrev, MenuNext, MenuConfirm, MenuDismiss, MenuLeft, MenuRight]);

/// Which platform's menu look is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    MacOs,
    Windows,
    Linux,
}

/// Dimensions of a menu look (logical pixels). `Flavor::metrics` gives values close to the platform's own menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// Corner radius and inner padding of the menu box.
    pub radius: f32,
    pub pad: f32,
    /// Row height, corner radius and horizontal padding.
    pub row_h: f32,
    pub row_radius: f32,
    pub row_px: f32,
    /// Label and shortcut font size.
    pub font: f32,
    pub shortcut_font: f32,
    pub min_w: f32,
    /// Minimum gap between the label and the shortcut.
    pub gap: f32,
    /// Inset of the separator line from the left and right, and its vertical spacing.
    pub sep_inset: f32,
    pub sep_my: f32,
    /// `true`: the selected row is filled with the accent color (macOS); `false`: neutral hover color (Windows, Linux).
    pub accent_highlight: bool,
    /// Shadow: blur, vertical offset, darkness.
    pub shadow_blur: f32,
    pub shadow_dy: f32,
    pub shadow_alpha: f32,
}

impl Flavor {
    pub const fn current() -> Flavor {
        if cfg!(target_os = "macos") {
            Flavor::MacOs
        } else if cfg!(windows) {
            Flavor::Windows
        } else {
            Flavor::Linux
        }
    }

    /// `macos`, `windows` or `linux` (any case); anything else is `None`.
    pub fn parse(s: &str) -> Option<Flavor> {
        match s.trim().to_ascii_lowercase().as_str() {
            "macos" | "mac" => Some(Flavor::MacOs),
            "windows" | "win" => Some(Flavor::Windows),
            "linux" => Some(Flavor::Linux),
            _ => None,
        }
    }

    /// The look to draw: the running platform's, or the one named by `CODEYARD_MENU_FLAVOR` so that a reviewer can check the
    /// Windows and Linux menus on a Mac (and the other way round). Tests never read the variable.
    pub fn detect() -> Flavor {
        if cfg!(test) {
            return Flavor::current();
        }
        std::env::var("CODEYARD_MENU_FLAVOR").ok().and_then(|v| Flavor::parse(&v)).unwrap_or(Flavor::current())
    }

    pub const fn metrics(self) -> Metrics {
        match self {
            // NSMenu: 10 corner radius, 5 padding, 13 pt text, blue highlight, thin separator that starts inset.
            Flavor::MacOs => Metrics {
                radius: 10.,
                pad: 5.,
                row_h: 24.,
                row_radius: 5.,
                row_px: 10.,
                font: 13.,
                shortcut_font: 13.,
                min_w: 200.,
                gap: 28.,
                sep_inset: 10.,
                sep_my: 5.,
                accent_highlight: true,
                shadow_blur: 28.,
                shadow_dy: 10.,
                shadow_alpha: 0.38,
            },
            // Windows 11: 8 corner radius, 4 padding, 32 px row, 14 px text, neutral highlight, full-width separator.
            Flavor::Windows => Metrics {
                radius: 8.,
                pad: 4.,
                row_h: 32.,
                row_radius: 4.,
                row_px: 12.,
                font: 14.,
                shortcut_font: 12.,
                min_w: 216.,
                gap: 32.,
                sep_inset: 0.,
                sep_my: 4.,
                accent_highlight: false,
                shadow_blur: 16.,
                shadow_dy: 8.,
                shadow_alpha: 0.30,
            },
            // GTK/Qt: in between the two; neutral highlight.
            Flavor::Linux => Metrics {
                radius: 6.,
                pad: 4.,
                row_h: 28.,
                row_radius: 4.,
                row_px: 10.,
                font: 13.,
                shortcut_font: 12.,
                min_w: 200.,
                gap: 28.,
                sep_inset: 0.,
                sep_my: 4.,
                accent_highlight: false,
                shadow_blur: 14.,
                shadow_dy: 6.,
                shadow_alpha: 0.35,
            },
        }
    }
}

#[derive(Clone)]
pub struct MenuItem {
    pub label: SharedString,
    pub shortcut: Option<SharedString>,
    pub enabled: bool,
    /// Separator line before this item.
    pub separator: bool,
    pub submenu: bool,
    /// A toggle or one-of-many choice: `Some(true)` draws a check in front of the label.
    pub checked: Option<bool>,
}

impl MenuItem {
    pub fn new(label: &str) -> Self {
        MenuItem { label: label.to_string().into(), shortcut: None, enabled: true, separator: false, submenu: false, checked: None }
    }

    pub fn shortcut(mut self, k: Option<String>) -> Self {
        self.shortcut = k.map(Into::into);
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn separator(mut self) -> Self {
        self.separator = true;
        self
    }

    pub fn checked(mut self, on: bool) -> Self {
        self.checked = Some(on);
        self
    }
}

pub enum MenuEvent {
    Select(usize),
    Dismiss,
    Horizontal(bool),
}

pub struct ContextMenu {
    items: Vec<MenuItem>,
    selected: Option<usize>,
    focus_handle: FocusHandle,
    /// Which corner of the menu sits at the given position (default: top left, for the mouse position).
    anchor: Corner,
    flavor: Flavor,
    scroll: gpui::ScrollHandle,
}

impl EventEmitter<MenuEvent> for ContextMenu {}

impl Focusable for ContextMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ContextMenu {
    pub fn new(items: Vec<MenuItem>, cx: &mut Context<Self>) -> Self {
        ContextMenu {
            items,
            selected: None,
            focus_handle: cx.focus_handle(),
            anchor: Corner::TopLeft,
            flavor: Flavor::detect(),
            scroll: gpui::ScrollHandle::new(),
        }
    }

    /// Which corner of the menu the position corresponds to: menus that open upward next to a button want `Corner::BottomLeft`.
    pub fn set_anchor(&mut self, anchor: Corner) {
        self.anchor = anchor;
    }

    pub fn anchor(&self) -> Corner {
        self.anchor
    }

    #[cfg(test)]
    pub fn labels(&self) -> Vec<String> {
        self.items.iter().map(|i| i.label.to_string()).collect()
    }

    /// Before which item there is a separator line (tests verify the grouping).
    #[cfg(test)]
    pub fn separators(&self) -> Vec<bool> {
        self.items.iter().map(|i| i.separator).collect()
    }

    #[cfg(test)]
    pub fn select_for_test(&mut self, label: &str, cx: &mut Context<Self>) {
        let i = self.items.iter().position(|i| i.label == label).unwrap_or_else(|| panic!("not in the menu: {label}"));
        cx.emit(MenuEvent::Select(i));
    }

    /// The next/previous enabled item (disabled ones are skipped).
    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let mut i = self.selected.unwrap_or(if forward { n - 1 } else { 0 });
        for _ in 0..n {
            i = if forward { (i + 1) % n } else { (i + n - 1) % n };
            if self.items[i].enabled {
                self.selected = Some(i);
                self.scroll.scroll_to_item(i);
                break;
            }
        }
        cx.notify();
    }

    fn prev(&mut self, _: &MenuPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.step(false, cx);
    }

    fn next(&mut self, _: &MenuNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(true, cx);
    }

    fn confirm(&mut self, _: &MenuConfirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.selected.filter(|&i| self.items[i].enabled) {
            cx.emit(MenuEvent::Select(i));
        }
    }

    fn dismiss(&mut self, _: &MenuDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(MenuEvent::Dismiss);
    }

    fn left(&mut self, _: &MenuLeft, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(MenuEvent::Horizontal(false));
    }

    fn right(&mut self, _: &MenuRight, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.selected.filter(|&i| self.items[i].enabled && self.items[i].submenu) {
            cx.emit(MenuEvent::Select(i));
        } else {
            cx.emit(MenuEvent::Horizontal(true));
        }
    }
}

impl Render for ContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let m = self.flavor.metrics();
        let selected = self.selected;
        // A menu with toggles keeps a check column in front of every label so the labels stay aligned.
        let checks = self.items.iter().any(|i| i.checked.is_some());
        div()
            .id("context-menu")
            .debug_selector(|| "context-menu".into())
            .key_context("ContextMenu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::prev))
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .occlude()
            .min_w(px(m.min_w))
            .max_h((window.viewport_size().height - px(16.)).max(px(24.)))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(px(m.pad))
            .flex()
            .flex_col()
            .bg(c(p.surface_raised))
            .border_1()
            .border_color(c(p.border))
            .rounded(px(m.radius))
            .shadow(vec![BoxShadow {
                color: ca(0x000000, m.shadow_alpha).into(),
                offset: point(px(0.), px(m.shadow_dy)),
                blur_radius: px(m.shadow_blur),
                spread_radius: px(0.),
            }])
            .text_size(px(m.font))
            .children(self.items.iter().enumerate().map(|(i, item)| {
                let active = selected == Some(i) && item.enabled;
                // macOS: the highlighted row is filled with the accent color, text and shortcut stay readable on top; others use a neutral background.
                let on_accent = active && m.accent_highlight;
                let label_color = if !item.enabled {
                    p.text_muted
                } else if on_accent {
                    p.on_accent
                } else {
                    p.text
                };
                let shortcut_color = if on_accent { p.on_accent } else { p.text_muted };
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .when(item.separator && i > 0, |d| d.child(div().h(px(1.)).mx(px(m.sep_inset)).my(px(m.sep_my)).bg(c(p.border))))
                    .child(
                        div()
                            .id(("menu-item", i))
                            .debug_selector(move || format!("menu-item-{i}"))
                            .h(px(m.row_h))
                            .flex_none()
                            .px(px(m.row_px))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(m.gap))
                            .rounded(px(m.row_radius))
                            .text_color(c(label_color))
                            .when(active, |d| d.bg(c(if m.accent_highlight { p.accent } else { p.surface_hover })))
                            .when(item.enabled, |d| {
                                d.cursor_pointer().on_mouse_move(cx.listener(move |this, _, _, cx| {
                                    if this.selected != Some(i) {
                                        this.selected = Some(i);
                                        cx.notify();
                                    }
                                }))
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .when(checks, |d| {
                                        d.child(
                                            div()
                                                .w(px(14.))
                                                .flex_none()
                                                .when(item.checked == Some(true), |d| d.child(crate::ui::icon("check", 14., label_color))),
                                        )
                                    })
                                    .child(item.label.clone()),
                            )
                            .when_some(item.shortcut.clone(), |d, k| {
                                d.child(
                                    div().text_size(px(m.shortcut_font)).text_color(c(shortcut_color)).opacity(if item.enabled { 0.85 } else { 0.6 }).child(k),
                                )
                            })
                            .when(item.submenu, |d| d.child(crate::ui::icon("chevron-right", 12., shortcut_color)))
                            .when(item.enabled, |d| d.on_mouse_up(MouseButton::Left, cx.listener(move |_, _, _, cx| cx.emit(MenuEvent::Select(i))))),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Flavor; 3] = [Flavor::MacOs, Flavor::Windows, Flavor::Linux];

    #[gpui::test]
    fn long_menus_keep_row_height_and_scroll_to_keyboard_selection(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (menu, cx) = cx.add_window_view(|_, cx| ContextMenu::new((0..40).map(|i| MenuItem::new(&format!("Item {i}"))).collect(), cx));
        cx.simulate_resize(gpui::size(px(500.), px(300.)));
        cx.run_until_parked();
        let height = menu.read_with(cx, |menu, _| menu.flavor.metrics().row_h);
        assert_eq!(cx.debug_bounds("menu-item-0").unwrap().size.height, px(height));
        menu.update(cx, |menu, cx| menu.step(false, cx));
        cx.run_until_parked();
        assert_eq!(menu.read_with(cx, |menu, _| menu.selected), Some(39));
        assert!(menu.read_with(cx, |menu, _| menu.scroll.offset().y) < px(-100.), "keyboard navigation reveals the last item");
        let last = cx.debug_bounds("menu-item-39").unwrap();
        let bounds = cx.debug_bounds("context-menu").unwrap();
        assert_eq!(last.size.height, px(height));
        assert!(last.bottom() <= bounds.bottom() && last.top() >= bounds.top(), "selected item must be visible: {last:?} inside {bounds:?}");
    }

    #[test]
    fn every_flavor_has_sane_proportions() {
        for f in ALL {
            let m = f.metrics();
            // Padding around the text; the row's corner is smaller than the box's (nested rounded corners look consistent).
            assert!(m.row_h >= m.font * 1.5, "{f:?}: row {} for font {}", m.row_h, m.font);
            assert!(m.row_radius < m.radius, "{f:?}: inner radius must be smaller than the box radius");
            assert!(m.shortcut_font <= m.font, "{f:?}: the shortcut is never larger than the label");
            assert!(m.min_w >= 160. && m.gap >= 16., "{f:?}");
            assert!((0.1..=0.6).contains(&m.shadow_alpha), "{f:?}");
        }
    }

    #[test]
    fn macos_follows_nsmenu_and_the_others_use_a_neutral_highlight() {
        let (mac, win, linux) = (Flavor::MacOs.metrics(), Flavor::Windows.metrics(), Flavor::Linux.metrics());
        assert!(mac.accent_highlight, "macOS fills the highlighted row with the accent color");
        assert!(!win.accent_highlight && !linux.accent_highlight, "Windows and Linux use a neutral hover");
        assert!(mac.sep_inset > 0. && win.sep_inset == 0., "macOS separators are inset, Windows ones run the full width");
        assert!(win.row_h > mac.row_h, "Windows rows are taller (touch friendly)");
        assert_eq!((mac.font, win.font), (13., 14.));
    }

    #[test]
    fn flavor_names_parse_and_unknown_ones_do_not() {
        assert_eq!(
            ["macos", "MacOS", " mac ", "windows", "WIN", "linux", "Linux"].map(Flavor::parse),
            [
                Some(Flavor::MacOs),
                Some(Flavor::MacOs),
                Some(Flavor::MacOs),
                Some(Flavor::Windows),
                Some(Flavor::Windows),
                Some(Flavor::Linux),
                Some(Flavor::Linux)
            ]
        );
        assert_eq!(["", "beos", "mac os"].map(Flavor::parse), [None, None, None]);
        assert_eq!(Flavor::detect(), Flavor::current(), "tests ignore the environment variable");
    }

    #[test]
    fn the_current_flavor_matches_the_target_platform() {
        let expect = if cfg!(target_os = "macos") {
            Flavor::MacOs
        } else if cfg!(windows) {
            Flavor::Windows
        } else {
            Flavor::Linux
        };
        assert_eq!(Flavor::current(), expect);
    }
}
