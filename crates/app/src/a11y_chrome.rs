//! The window's chrome for assistive technology: the activity bar (a `TabList` "Views": one `Tab` per view with its
//! badge, the view shown selected; `Click` shows or hides it as a click does) and its Manage button (the menu it
//! opens), the title bar's Back, Forward and command center (macOS draws them), the caption buttons the app draws on
//! Windows (Minimize, Maximize or Restore, Close) and the Remote Explorer's header buttons (Add SSH Host…, Open SSH
//! Config File, Refresh). Every control has its painted frame; an action from an outdated tree acts only while the
//! control is there with the same meaning (a view still in the bar, Back still possible).

use super::*;

const VIEWS: NodeId = NodeId(151);
const MANAGE: NodeId = NodeId(152);
const BACK: NodeId = NodeId(153);
const FORWARD: NodeId = NodeId(154);
const COMMAND_CENTER: NodeId = NodeId(155);
const REMOTE_TOOLS: NodeId = NodeId(156);
const CAPTION: NodeId = NodeId(157);
/// Views of the activity bar (`VIEW_TAB + i`, at most 16), the Remote Explorer's header buttons (`REMOTE_TOOL + i`)
/// and the caption buttons (`CAPTION_BUTTON + i`).
const VIEW_TAB: u64 = 160;
const REMOTE_TOOL: u64 = 176;
const CAPTION_BUTTON: u64 = 180;
/// This module's fixed nodes, for the id layout check.
pub(super) const FIXED: [NodeId; 29] = {
    let mut ids = [NodeId(0); 29];
    let fixed = [VIEWS, MANAGE, BACK, FORWARD, COMMAND_CENTER, REMOTE_TOOLS, CAPTION];
    let mut i = 0;
    while i < fixed.len() {
        ids[i] = fixed[i];
        i += 1;
    }
    let mut k = 0;
    while k < 16 {
        ids[7 + k] = NodeId(VIEW_TAB + k as u64);
        k += 1;
    }
    let mut k = 0;
    while k < 3 {
        ids[23 + k] = NodeId(REMOTE_TOOL + k as u64);
        ids[26 + k] = NodeId(CAPTION_BUTTON + k as u64);
        k += 1;
    }
    ids
};

/// The Remote Explorer's header buttons, in the order they are drawn: (element id, label).
const REMOTE_BUTTONS: [&str; 3] = ["remote-add", "remote-config", "remote-refresh"];

/// The window's chrome as the tree shows it.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct ChromeSnap {
    /// Views of the activity bar: (view, label, badge, shown); empty while the bar is hidden.
    views: Vec<(Panel, String, usize, bool)>,
    /// The title bar the app draws on macOS: Back and Forward possible, the command center's label.
    title_bar: Option<(bool, bool, String)>,
    /// The caption buttons the app draws on Windows: whether the window is maximized.
    caption: Option<bool>,
    /// The Remote Explorer is shown (its header buttons are drawn).
    remote: bool,
}

fn button(nodes: &mut Vec<(NodeId, Node)>, geometry: &Geometry, id: NodeId, label: &str, part: &'static str, enabled: bool) -> NodeId {
    let mut b = Node::new(Role::Button);
    b.set_label(label.to_string());
    if enabled {
        b.add_action(Action::Click);
    } else {
        b.set_disabled();
    }
    geometry.apply_part(Part::Control(part), &mut b);
    nodes.push((id, b));
    id
}

/// The element id of the activity bar item of `panel` (its frame's key).
pub(in crate::workbench) fn view_part(panel: Panel) -> &'static str {
    match panel {
        Panel::Explorer => "act-explorer",
        Panel::Search => "act-search",
        Panel::Scm => "act-scm",
        Panel::Debug => "act-debug",
        Panel::Testing => "act-testing",
        Panel::Remote => "act-remote",
        Panel::Extensions => "act-extensions",
        Panel::Containers => "act-containers",
        _ => "act-other",
    }
}

impl ChromeSnap {
    /// The Remote Explorer's header buttons, moved to the view's own window.
    pub(super) fn take_remote(&mut self) -> ChromeSnap {
        ChromeSnap { remote: std::mem::take(&mut self.remote), ..Default::default() }
    }

    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>, kids: &mut Vec<NodeId>, geometry: &Geometry) {
        if let Some((back, forward, label)) = &self.title_bar {
            kids.push(button(nodes, geometry, BACK, t("Back"), "tb-back", *back));
            kids.push(button(nodes, geometry, FORWARD, t("Forward"), "tb-forward", *forward));
            kids.push(button(nodes, geometry, COMMAND_CENTER, label, "command-center", true));
            if let Some((_, n)) = nodes.last_mut() {
                n.set_description(t("Go to File…"));
                n.set_has_popup(accesskit::HasPopup::Dialog);
            }
        }
        if let Some(maximized) = self.caption {
            let mut group = Node::new(Role::Group);
            group.set_label(t("Window"));
            let restore = if maximized { t("Restore window") } else { t("Maximize window") };
            let ids = vec![
                button(nodes, geometry, NodeId(CAPTION_BUTTON), t("Minimize window"), "window-minimize", true),
                button(nodes, geometry, NodeId(CAPTION_BUTTON + 1), restore, "window-maximize", true),
                button(nodes, geometry, NodeId(CAPTION_BUTTON + 2), t("Close window"), "window-close", true),
            ];
            group.set_children(ids);
            nodes.push((CAPTION, group));
            kids.push(CAPTION);
        }
        if !self.views.is_empty() {
            let mut list = Node::new(Role::TabList);
            list.set_label(t("Views"));
            let mut ids = Vec::new();
            for (i, (panel, label, badge, shown)) in self.views.iter().enumerate() {
                let id = NodeId(VIEW_TAB + i as u64);
                let mut tab = Node::new(Role::Tab);
                tab.set_label(label.clone());
                if *badge > 0 {
                    tab.set_description(badge.to_string());
                }
                tab.set_selected(*shown);
                tab.set_position_in_set(i + 1);
                tab.set_size_of_set(self.views.len());
                tab.add_action(Action::Click);
                geometry.apply_part(Part::Control(view_part(*panel)), &mut tab);
                nodes.push((id, tab));
                ids.push(id);
            }
            list.set_children(ids);
            nodes.push((VIEWS, list));
            kids.push(VIEWS);
            kids.push(button(nodes, geometry, MANAGE, t("Manage: About, Settings, Color Theme…"), "act-manage", true));
            if let Some((_, n)) = nodes.last_mut() {
                n.set_has_popup(accesskit::HasPopup::Menu);
            }
        }
        if self.remote {
            let mut bar = Node::new(Role::Toolbar);
            bar.set_label(t("Remote Explorer"));
            let labels = [t("Add SSH Host…"), t("Open SSH Config File"), t("Refresh")];
            let ids = REMOTE_BUTTONS
                .iter()
                .zip(labels)
                .enumerate()
                .map(|(i, (part, label))| button(nodes, geometry, NodeId(REMOTE_TOOL + i as u64), label, part, true))
                .collect::<Vec<_>>();
            bar.set_children(ids);
            nodes.push((REMOTE_TOOLS, bar));
            kids.push(REMOTE_TOOLS);
        }
    }
}

impl Workbench {
    pub(super) fn a11y_chrome(&self, window: &Window, cx: &App) -> ChromeSnap {
        let settings = cx.global::<Settings>();
        let bar = self.chrome.zen.is_none() && settings.activity_bar;
        let views = if bar {
            self.activity_items(cx).into_iter().map(|(panel, label, badge, shown)| (panel, label.to_string(), badge, shown)).collect()
        } else {
            Vec::new()
        };
        let title_bar = cfg!(target_os = "macos").then(|| (!self.nav.back.is_empty(), !self.nav.forward.is_empty(), self.command_center_label(cx)));
        let caption = (cfg!(target_os = "windows") && !window.is_fullscreen()).then(|| window.is_maximized());
        ChromeSnap { views, title_bar, caption, remote: self.view_shown(Panel::Remote) }
    }

    /// An action on the chrome; `false` if the request is not for it.
    pub(super) fn a11y_chrome_action(&mut self, req: &ActionRequest, last: &ChromeSnap, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        if !FIXED.contains(&id) {
            return false;
        }
        if req.action != Action::Click {
            return true;
        }
        let now = self.a11y_chrome(window, cx);
        match id {
            BACK if now.title_bar.as_ref().is_some_and(|t| t.0) => self.go_back(&GoBack, window, cx),
            FORWARD if now.title_bar.as_ref().is_some_and(|t| t.1) => self.go_forward(&GoForward, window, cx),
            COMMAND_CENTER if now.title_bar.is_some() => self.quick_open(&QuickOpen, window, cx),
            MANAGE if !now.views.is_empty() => self.open_manage_menu(window, cx),
            NodeId(n) if (VIEW_TAB..VIEW_TAB + 16).contains(&n) => {
                let Some((panel, ..)) = last.views.get((n - VIEW_TAB) as usize) else { return true };
                // Still in the bar, shown or hidden as the tree said: a press toggles it as a click does.
                if now.views.iter().any(|v| v.0 == *panel)
                    && now.views.iter().find(|v| v.0 == *panel).map(|v| v.3) == last.views.iter().find(|v| v.0 == *panel).map(|v| v.3)
                {
                    self.activity_click(*panel, window, cx);
                }
            }
            NodeId(n) if (REMOTE_TOOL..REMOTE_TOOL + 3).contains(&n) && now.remote => match n - REMOTE_TOOL {
                0 => self.add_ssh_host(&AddSshHost, window, cx),
                1 => self.edit_ssh_config(&EditSshConfig, window, cx),
                _ => self.refresh_remote_view(cx),
            },
            NodeId(n) if (CAPTION_BUTTON..CAPTION_BUTTON + 3).contains(&n) && now.caption.is_some() => match n - CAPTION_BUTTON {
                0 => window.minimize_window(),
                1 => window.zoom_window(),
                _ => self.request_close_window(window, cx),
            },
            _ => {}
        }
        cx.notify();
        true
    }

    /// The caption's Close button: what the system close does (`main.rs`): wait for an install, close after a hot-exit
    /// backup, or ask about unsaved changes.
    fn request_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hold_close_for_install(window, cx) {
            return;
        }
        if self.prepare_quit(cx) {
            window.remove_window();
        } else {
            self.confirm_close(window, cx);
        }
    }
}
