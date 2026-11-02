//! Row context menus for assistive technology: a row of the Run and Debug view, the Remote Explorer or the Ports panel
//! that has a pointer menu (right click or its "…" button) gets `ShowContextMenu` and a "More Actions…" button, and
//! both open that same menu under the row. The native adapters do not forward `ShowContextMenu` yet, so the button is
//! what a screen reader reaches today. The menu then has the keyboard (arrows, Enter, Escape) and gives the focus back
//! when it closes, as it does after a right click. Each view checks that the row is still the one the tree showed.

use super::*;
use crate::workbench::a11y::RowFrame;

/// "More Actions…" buttons: `PER_KIND` ids for each kind of row, one per row.
pub(in crate::workbench) const ROW_MENU: u64 = 0x1c0_0000;
const PER_KIND: u64 = 0x1000;

/// The rows that have a menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::workbench) enum MenuRow {
    Variable,
    Frame,
    Breakpoint,
    Host,
    Port,
}

impl MenuRow {
    const fn base(self) -> u64 {
        ROW_MENU
            + PER_KIND
                * match self {
                    MenuRow::Variable => 0,
                    MenuRow::Frame => 1,
                    MenuRow::Breakpoint => 2,
                    MenuRow::Host => 3,
                    MenuRow::Port => 4,
                }
    }

    /// Where the menu opens: the painted row it belongs to.
    pub(in crate::workbench) fn frame(self) -> RowFrame {
        match self {
            MenuRow::Variable => RowFrame::DebugVariable,
            MenuRow::Frame => RowFrame::DebugStack,
            MenuRow::Breakpoint => RowFrame::DebugBreakpoint,
            MenuRow::Host => RowFrame::Remote,
            MenuRow::Port => RowFrame::Port,
        }
    }
}

const _: () = {
    assert!(MenuRow::Port.base() + PER_KIND <= ROW_MENU + BLOCK);
};

/// Gives `row` (the `index`th of its kind, named `label`) its menu: `ShowContextMenu` on the row and a "More Actions…"
/// button among its children.
pub(in crate::workbench) fn add_menu(nodes: &mut Vec<(NodeId, Node)>, row: &mut Node, kind: MenuRow, index: usize, label: &str) {
    if index as u64 >= PER_KIND {
        return;
    }
    row.add_action(Action::ShowContextMenu);
    let id = NodeId(kind.base() + index as u64);
    let mut b = Node::new(Role::Button);
    b.set_label(t("More Actions…"));
    b.set_description(label.to_string());
    b.set_has_popup(accesskit::HasPopup::Menu);
    b.add_action(Action::Click);
    nodes.push((id, b));
    row.push_child(id);
}

/// The index of the `kind` row whose menu `req` asks for: a click on its "More Actions…" button, or `ShowContextMenu` on
/// the row itself (`row_index` gives the index of a row node of this kind).
pub(in crate::workbench) fn menu_request(req: &ActionRequest, kind: MenuRow, row_index: impl Fn(u64) -> Option<usize>) -> Option<usize> {
    let id = req.target_node.0;
    match req.action {
        Action::Click if (kind.base()..kind.base() + PER_KIND).contains(&id) => Some((id - kind.base()) as usize),
        Action::ShowContextMenu => row_index(id),
        _ => None,
    }
}

impl Workbench {
    /// Where a menu opened from the tree appears: under its row, as with Shift+F10.
    pub(in crate::workbench) fn row_menu_anchor(&self, kind: MenuRow, index: usize, window: &Window) -> gpui::Point<gpui::Pixels> {
        self.row_anchor(kind.frame(), index, window)
    }
}

#[cfg(test)]
#[path = "a11y_row_menus_tests.rs"]
mod tests;
