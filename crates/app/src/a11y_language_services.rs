//! Setup page semantics and guarded actions. Geometry comes from its actual clipped paint bounds.

use super::*;
use crate::language_services_page::{AccessAction, AccessView};

pub(super) fn append_tree(view: &AccessView, nodes: &mut Vec<(NodeId, Node)>) {
    let mut page = Node::new(Role::Group);
    page.set_label(t("Language Services"));
    page.add_action(Action::Focus);
    if view.busy {
        page.set_busy();
    }
    if view.scrollable {
        page.add_action(Action::ScrollDown);
        page.add_action(Action::ScrollUp);
    }
    if let Some(bounds) = view.bounds[0] {
        page.set_bounds(bounds);
    }
    let mut children = vec![SERVICES_INFO, SERVICES_STATUS, SERVICES_INSTALL];
    let mut info = Node::new(Role::Label);
    info.set_label(view.summary.clone());
    if let Some(bounds) = view.bounds[1] {
        info.set_bounds(bounds);
    }
    nodes.push((SERVICES_INFO, info));
    let mut status = Node::new(if view.error { Role::Alert } else { Role::Status });
    status.set_label(view.status.clone());
    status.set_live(if view.error { Live::Assertive } else { Live::Polite });
    status.set_live_atomic();
    if let Some(bounds) = view.bounds[2] {
        status.set_bounds(bounds);
    }
    nodes.push((SERVICES_STATUS, status));
    let mut install = Node::new(Role::Button);
    install.set_label(view.install_label.clone());
    if view.can_install {
        install.add_action(Action::Click);
        install.add_action(Action::Focus);
    } else {
        install.set_disabled();
    }
    if let Some(bounds) = view.bounds[3] {
        install.set_bounds(bounds);
    }
    nodes.push((SERVICES_INSTALL, install));
    if view.cancel_visible {
        let mut cancel = Node::new(Role::Button);
        cancel.set_label(t("Cancel"));
        if view.can_cancel {
            cancel.add_action(Action::Click);
            cancel.add_action(Action::Focus);
        } else {
            cancel.set_disabled();
        }
        if let Some(bounds) = view.bounds[4] {
            cancel.set_bounds(bounds);
        }
        nodes.push((SERVICES_CANCEL, cancel));
        children.push(SERVICES_CANCEL);
    }
    page.set_children(children);
    nodes.push((SERVICES, page));
}

pub(super) fn focus_node(view: &AccessView) -> NodeId {
    match view.control_focus {
        Some(AccessAction::Install) if view.can_install => SERVICES_INSTALL,
        Some(AccessAction::Cancel) if view.can_cancel => SERVICES_CANCEL,
        _ => SERVICES,
    }
}

impl Workbench {
    pub(super) fn a11y_services_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if ![SERVICES, SERVICES_INFO, SERVICES_STATUS, SERVICES_INSTALL, SERVICES_CANCEL].contains(&req.target_node) {
            return false;
        }
        let Some(previous) = &last.services else { return true };
        let Some(Page::LanguageServices(page)) = self.tabs.page.clone().filter(|_| self.tabs.page_active) else { return true };
        let current = page.read(cx).access_view(window, cx);
        if current.token != previous.token || req.action == Action::Click && current.revision != previous.revision {
            return true;
        }
        let control = match req.target_node {
            SERVICES_INSTALL => Some(AccessAction::Install),
            SERVICES_CANCEL => Some(AccessAction::Cancel),
            _ => None,
        };
        page.update(cx, |page, cx| match req.action {
            Action::Focus => page.access_focus(control, window, cx),
            Action::Click => {
                if let Some(control) = control {
                    page.perform(control, window, cx);
                }
            }
            Action::ScrollDown if req.target_node == SERVICES => page.access_scroll(true, cx),
            Action::ScrollUp if req.target_node == SERVICES => page.access_scroll(false, cx),
            _ => {}
        });
        true
    }
}

#[cfg(test)]
#[path = "a11y_language_services_tests.rs"]
mod tests;
