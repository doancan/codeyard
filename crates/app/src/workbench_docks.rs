//! Workbench: docking. A side view moves between the primary and the secondary side bar by dragging its activity bar
//! icon onto the other side: while a view is dragged, both side bars are drop zones (outlined in the accent colour,
//! filled when the view is over one). The panel docks below, left of or right of the editor. Every move also has a
//! command, which is the keyboard path, and screen readers read where the views are from the window's description.

use super::*;

gpui::actions!(workbench, [MovePanelLeft, MovePanelRight, MovePanelToBottom]);

/// Where the panel (terminal, Problems, Output…) is docked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PanelDock {
    #[default]
    Bottom,
    Left,
    Right,
}

impl PanelDock {
    fn id(self) -> &'static str {
        match self {
            PanelDock::Bottom => "bottom",
            PanelDock::Left => "left",
            PanelDock::Right => "right",
        }
    }

    fn of(id: &str) -> Option<Self> {
        [PanelDock::Bottom, PanelDock::Left, PanelDock::Right].into_iter().find(|d| d.id() == id)
    }

    /// The saved dock; the older `panel_right` setting still counts, an unknown value docks at the bottom.
    pub(super) fn load(state: Option<&State>) -> Self {
        let Some(s) = state else { return PanelDock::Bottom };
        s.get("panel_dock")
            .and_then(|v| Self::of(&v))
            .unwrap_or_else(|| if s.get("panel_right").as_deref() == Some("1") { PanelDock::Right } else { PanelDock::Bottom })
    }

    /// Beside the editor (left or right) rather than below it.
    pub(super) fn beside(self) -> bool {
        self != PanelDock::Bottom
    }
}

/// A side view dragged by its activity bar icon.
#[derive(Clone)]
pub(super) struct DraggedView {
    pub(super) panel: Panel,
    title: SharedString,
}

impl Render for DraggedView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        div()
            .px(px(theme::SPACE[4]))
            .py(px(theme::SPACE[1]))
            .rounded(px(theme::radius::SM))
            .bg(c(p.bg))
            .border_1()
            .border_color(c(p.accent))
            .text_size(px(text::SM.0))
            .text_color(c(p.text))
            .child(self.title.clone())
    }
}

/// The name a view is read out by (the screen reader's layout description).
fn view_name(panel: Panel) -> String {
    use crate::i18n::t;
    match panel {
        Panel::Explorer => t("Explorer"),
        Panel::Search => t("Search"),
        Panel::Scm => t("Source Control"),
        Panel::Debug => t("Run and Debug"),
        Panel::Testing => t("Testing"),
        Panel::Remote => t("Remote Explorer"),
        Panel::Containers => t("Containers"),
        Panel::Extensions => t("Extensions"),
        Panel::ExtView => t("Extension View"),
    }
    .to_string()
}

impl Workbench {
    /// Docks the panel at `dock` and shows it. A maximized panel covers the editor wherever it is docked, so moving it
    /// also restores its size: otherwise the move would not show.
    pub(super) fn dock_panel(&mut self, dock: PanelDock, cx: &mut Context<Self>) {
        self.chrome.panel_dock = dock;
        self.chrome.panel_max = false;
        self.chrome.panel_open = true;
        if let Some(s) = &self.persist.state {
            let _ = s.set("panel_dock", dock.id());
            // Older versions read this one.
            let _ = s.set("panel_right", if dock == PanelDock::Right { "1" } else { "0" });
        }
        cx.notify();
    }

    /// A view's activity bar icon starts a drag (not for Containers, which opens a page).
    pub(super) fn view_drag(&self, panel: Panel, d: gpui::Stateful<gpui::Div>, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        if panel == Panel::Containers {
            return d;
        }
        let home = cx.entity().downgrade();
        d.on_drag(DraggedView { panel, title: view_name(panel).into() }, move |v, _, _, cx| {
            // The drop zones appear for this drag only (not for a tab or a file being dragged).
            home.update(cx, |wb, cx| {
                wb.chrome.view_drag = Some(v.panel);
                cx.notify();
            })
            .ok();
            cx.new(|_| v.clone())
        })
    }

    /// Moves `view` into the secondary side bar, or (`secondary` false) back to the primary one, and shows it there.
    /// The other side bar keeps the view it showed.
    pub(super) fn dock_view(&mut self, view: Panel, secondary: bool, cx: &mut Context<Self>) {
        self.chrome.view_drag = None;
        if view == Panel::Containers {
            return cx.notify();
        }
        if secondary {
            if self.chrome.panel == Some(view) {
                return self.move_view_to_secondary(cx);
            }
            if !self.chrome.in_secondary.contains(&view) {
                self.chrome.in_secondary.push(view);
            }
            self.chrome.secondary = Some(view);
            self.chrome.secondary_last = Some(view);
        } else {
            if self.chrome.secondary == Some(view) {
                return self.move_view_to_primary(cx);
            }
            self.chrome.in_secondary.retain(|v| *v != view);
            self.chrome.panel = Some(view);
        }
        self.place_views();
        self.save_views();
        cx.notify();
    }

    /// A drag ended (dropped anywhere, or cancelled): the drop zones go.
    pub(super) fn end_view_drag(&mut self, cx: &mut Context<Self>) {
        if self.chrome.view_drag.take().is_some() {
            cx.notify();
        }
    }

    /// While a view is dragged: the drop zones over the primary side (activity bar and side bar, `left_w` wide) and the
    /// secondary side (`right_w` wide, at the right edge).
    pub(super) fn view_drop_zones(&self, left_w: f32, right_w: f32, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.chrome.view_drag.is_none() {
            return Vec::new();
        }
        let zone = |id: &'static str, label: String, secondary: bool| {
            let fill = ui::ca(p.accent, 0.18);
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .absolute()
                .top_0()
                .bottom_0()
                .flex()
                .items_center()
                .justify_center()
                .border_2()
                .border_color(c(p.accent))
                .bg(ui::ca(p.accent, 0.06))
                .text_size(px(text::SM.0))
                .text_color(c(p.text))
                .child(label)
                .drag_over::<DraggedView>(move |s, _, _, _| s.bg(fill))
                .on_drop(cx.listener(move |this, v: &DraggedView, _, cx| this.dock_view(v.panel, secondary, cx)))
        };
        vec![
            zone("primary-drop-zone", crate::i18n::t("Primary Side Bar").to_string(), false).left_0().w(px(left_w)).into_any_element(),
            zone("secondary-drop-zone", crate::i18n::t("Secondary Side Bar").to_string(), true).right_0().w(px(right_w)).into_any_element(),
        ]
    }

    /// Where the views are, for screen readers (the window's description): each side bar's view and the panel's dock.
    pub(super) fn a11y_layout(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = self.chrome.panel {
            parts.push(crate::i18n::tf("Primary side bar: {}", &[&view_name(v)]));
        }
        if let Some(v) = self.chrome.secondary {
            parts.push(crate::i18n::tf("Secondary side bar: {}", &[&view_name(v)]));
        }
        let moved: Vec<String> = self.chrome.in_secondary.iter().map(|v| view_name(*v)).collect();
        if !moved.is_empty() {
            parts.push(crate::i18n::tf("In the secondary side bar: {}", &[&moved.join(", ")]));
        }
        parts.push(match self.chrome.panel_dock {
            PanelDock::Bottom => crate::i18n::t("Panel: below the editor").to_string(),
            PanelDock::Left => crate::i18n::t("Panel: left of the editor").to_string(),
            PanelDock::Right => crate::i18n::t("Panel: right of the editor").to_string(),
        });
        parts.join(". ")
    }
}

#[cfg(test)]
#[path = "workbench_docks_tests.rs"]
mod tests;
