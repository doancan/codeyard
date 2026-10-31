//! The Run and Debug side view and the debug toolbar for assistive technology: the configuration choice and Start, the
//! current session's state, Variables (an expandable tree), Watch, Call Stack (sessions, threads, frames) and the
//! breakpoints with their enabled state. Every action calls what the pointer calls (`start_selected`, `debug_control`,
//! `select_frame`, `toggle_variable`, `set_breakpoint_enabled`, …). A row is named by what it shows (session id,
//! variable reference, frame, breakpoint place), so an action from an outdated tree does nothing when that row changed.

use super::super::row_menus::{MenuRow, add_menu, menu_request};
use super::*;
use crate::debug_model::{BpRow, Verified};
use crate::workbench::a11y::{Geometry, RowFrame};
use crate::workbench::debug::{DebugStatus, StackRow, VarRow, stack_rows, status_text, variable_rows};

const DEBUG: NodeId = NodeId(70);
const DEBUG_START: NodeId = NodeId(71);
const DEBUG_CONFIGS: NodeId = NodeId(72);
const DEBUG_OPEN_LAUNCH: NodeId = NodeId(73);
const DEBUG_STATE: NodeId = NodeId(74);
const DEBUG_VARS: NodeId = NodeId(75);
const DEBUG_WATCH: NodeId = NodeId(76);
const DEBUG_WATCH_ADD: NodeId = NodeId(77);
const DEBUG_STACK: NodeId = NodeId(78);
const DEBUG_BPS: NodeId = NodeId(79);
const DEBUG_BP_FUNCTION: NodeId = NodeId(80);
const DEBUG_BP_ACTIVE: NodeId = NodeId(81);
const DEBUG_TOOLBAR: NodeId = NodeId(82);
const DEBUG_CONTINUE: NodeId = NodeId(83);
const DEBUG_OVER: NodeId = NodeId(84);
const DEBUG_INTO: NodeId = NodeId(85);
const DEBUG_OUT: NodeId = NodeId(86);
const DEBUG_RESTART: NodeId = NodeId(87);
const DEBUG_STOP: NodeId = NodeId(88);
/// The configuration selector (the menu it opens lists the configurations and Open Launch Configuration).
const DEBUG_CHOOSE: NodeId = NodeId(159);
/// This module's fixed nodes, for the id layout check.
pub(in crate::workbench) const FIXED: [NodeId; 20] = [
    DEBUG,
    DEBUG_START,
    DEBUG_CONFIGS,
    DEBUG_OPEN_LAUNCH,
    DEBUG_STATE,
    DEBUG_VARS,
    DEBUG_WATCH,
    DEBUG_WATCH_ADD,
    DEBUG_STACK,
    DEBUG_BPS,
    DEBUG_BP_FUNCTION,
    DEBUG_BP_ACTIVE,
    DEBUG_TOOLBAR,
    DEBUG_CONTINUE,
    DEBUG_OVER,
    DEBUG_INTO,
    DEBUG_OUT,
    DEBUG_RESTART,
    DEBUG_STOP,
    DEBUG_CHOOSE,
];

/// The block of this module's rows; inside it: configurations, variables, watches (row and Remove), call stack rows,
/// breakpoints (check box and Remove).
pub(in crate::workbench) const DEBUG_ROW: u64 = 0x180_0000;
const CONFIG_ROW: u64 = DEBUG_ROW;
const VAR_ROW: u64 = DEBUG_ROW + 0x1000;
const WATCH_ROW: u64 = DEBUG_ROW + 0x2000;
const STACK_ROW: u64 = DEBUG_ROW + 0x3000;
const BP_ROW: u64 = DEBUG_ROW + 0x4000;
/// A line or data breakpoint's "Go to Breakpoint" button (breakpoint `i` at `BP_OPEN + i`).
const BP_OPEN: u64 = DEBUG_ROW + 0x8000;
/// Rows exposed at most (a deep stack or a large value can have many more).
const MAX_CONFIGS: usize = 256;
const MAX_VARS: usize = 2_000;
const MAX_WATCHES: usize = 256;
const MAX_STACK: usize = 2_000;
const MAX_BPS: usize = 2_000;
const _: () = {
    assert!(CONFIG_ROW + MAX_CONFIGS as u64 <= VAR_ROW);
    assert!(VAR_ROW + MAX_VARS as u64 <= WATCH_ROW);
    assert!(WATCH_ROW + 2 * MAX_WATCHES as u64 <= STACK_ROW);
    assert!(STACK_ROW + MAX_STACK as u64 <= BP_ROW);
    assert!(BP_ROW + 2 * MAX_BPS as u64 <= BP_OPEN && BP_OPEN + MAX_BPS as u64 <= DEBUG_ROW + BLOCK);
};

/// A breakpoint row: where it is (line breakpoints) or its function, and whether it is enabled.
#[derive(Clone, PartialEq, Debug)]
enum BpSnap {
    /// A data breakpoint of the session, by its place in the session's list.
    Data {
        index: usize,
        desc: String,
        enabled: bool,
    },
    Line {
        path: String,
        line: usize,
        enabled: bool,
        detail: String,
    },
    Function {
        index: usize,
        name: String,
        enabled: bool,
        detail: String,
    },
}

/// The Run and Debug side view while it is shown.
#[derive(Clone, PartialEq, Debug)]
struct DebugViewSnap {
    /// Launch configurations: (name, selected).
    configs: Vec<(String, bool)>,
    /// What Start starts: the selected configuration, or `None` to look for one like F5 does.
    start: Option<String>,
    /// The current session: (id, name, state).
    session: Option<(u64, String, String)>,
    vars: Vec<VarRow>,
    /// Watch expressions and their values.
    watch: Vec<(String, String)>,
    stack: Vec<StackRow>,
    bps: Vec<BpSnap>,
    active: bool,
}

/// The floating toolbar: the session it acts on and what it allows.
#[derive(Clone, PartialEq, Debug)]
struct ToolbarSnap {
    session: u64,
    name: String,
    stopped: bool,
    running: bool,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct DebugSnap {
    view: Option<DebugViewSnap>,
    toolbar: Option<ToolbarSnap>,
}

fn push_button(nodes: &mut Vec<(NodeId, Node)>, id: NodeId, label: &str, description: Option<String>, enabled: bool) -> NodeId {
    let mut b = Node::new(Role::Button);
    b.set_label(label.to_string());
    if let Some(d) = description {
        b.set_description(d);
    }
    if enabled {
        b.add_action(Action::Click);
    } else {
        b.set_disabled();
    }
    nodes.push((id, b));
    id
}

fn bp_detail(meta: &crate::debug_model::BpMeta) -> String {
    let mut parts = vec![crate::debug_model::breakpoint_meta_text(meta)];
    if let Verified::No(why) = &meta.verified {
        parts.push(why.clone().unwrap_or_else(|| t("The debugger could not bind this breakpoint.").into()));
    }
    parts.retain(|p| !p.is_empty());
    parts.join(". ")
}

impl DebugViewSnap {
    fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>, geometry: &Geometry) -> NodeId {
        let start = self.start.clone().unwrap_or_else(|| t("Auto-detect project").into());
        let mut kids = vec![push_button(nodes, DEBUG_START, t("Start Debugging (F5)"), Some(start), true)];
        if !self.configs.is_empty() {
            let mut group = Node::new(Role::RadioGroup);
            group.set_label(t("Select a configuration"));
            let mut ids = Vec::new();
            for (i, (name, selected)) in self.configs.iter().enumerate() {
                let id = NodeId(CONFIG_ROW + i as u64);
                let mut n = Node::new(Role::RadioButton);
                n.set_label(name.clone());
                n.set_toggled(Toggled::from(*selected));
                n.add_action(Action::Click);
                nodes.push((id, n));
                ids.push(id);
            }
            group.set_children(ids);
            nodes.push((DEBUG_CONFIGS, group));
            kids.push(DEBUG_CONFIGS);
        }
        // The selector itself: its menu offers the configurations and Open Launch Configuration.
        let chosen = self.configs.iter().find(|(_, s)| *s).map(|(n, _)| n.clone());
        kids.push(push_button(
            nodes,
            DEBUG_CHOOSE,
            &chosen.unwrap_or_else(|| t("Select a configuration").to_string()),
            Some(t("Launch configuration").to_string()),
            true,
        ));
        if let Some((_, n)) = nodes.last_mut() {
            n.set_has_popup(accesskit::HasPopup::Menu);
            geometry.apply_part(Part::Control("dbg-config"), n);
        }
        kids.push(push_button(nodes, DEBUG_OPEN_LAUNCH, t("Open Launch Configuration"), None, true));
        let mut state = Node::new(Role::Status);
        state.set_label(match &self.session {
            Some((_, name, status)) => format!("{name}: {status}"),
            None => t("No debug session.").into(),
        });
        state.set_live(Live::Polite);
        nodes.push((DEBUG_STATE, state));
        kids.push(DEBUG_STATE);

        // Variables: one tree item per row, its level giving the nesting.
        let mut vars = Node::new(Role::Tree);
        vars.set_label(t("Variables"));
        if self.vars.is_empty() {
            vars.set_description(t("Not paused"));
        }
        let mut ids = Vec::new();
        for (i, r) in self.vars.iter().enumerate() {
            let id = NodeId(VAR_ROW + i as u64);
            let from = nodes.len();
            let mut n = Node::new(Role::TreeItem);
            n.set_label(r.name.clone());
            n.set_level(r.depth + 1);
            if !r.value.is_empty() {
                n.set_value(r.value.clone());
            }
            if let Some(ty) = &r.ty {
                n.set_description(ty.clone());
            }
            geometry.apply_row(RowFrame::DebugVariable, i, &mut n);
            if r.more.is_some() {
                n.add_action(Action::Click);
            } else if r.reference > 0 {
                n.set_expanded(r.open);
                n.add_action(Action::Click);
                n.add_action(if r.open { Action::Collapse } else { Action::Expand });
            }
            // Variables (not scopes) have the view's menu.
            if r.parent != 0 && r.more.is_none() {
                add_menu(nodes, &mut n, MenuRow::Variable, i, &r.name);
            }
            geometry.apply_row_buttons(RowFrame::DebugVariable, i, id.0, &mut nodes[from..]);
            nodes.push((id, n));
            ids.push(id);
        }
        vars.set_children(ids);
        nodes.push((DEBUG_VARS, vars));
        kids.push(DEBUG_VARS);

        let mut watch = Node::new(Role::List);
        watch.set_label(t("Watch"));
        if self.watch.is_empty() {
            watch.set_description(t("No expressions: + adds one."));
        }
        let mut ids = Vec::new();
        for (i, (expr, value)) in self.watch.iter().enumerate() {
            let id = NodeId(WATCH_ROW + 2 * i as u64);
            let from = nodes.len();
            let mut n = Node::new(Role::ListItem);
            n.set_label(expr.clone());
            n.set_value(value.clone());
            n.set_children(vec![push_button(nodes, NodeId(id.0 + 1), t("Remove Expression"), Some(expr.clone()), true)]);
            geometry.apply_row(RowFrame::DebugWatch, i, &mut n);
            geometry.apply_row_buttons(RowFrame::DebugWatch, i, id.0, &mut nodes[from..]);
            nodes.push((id, n));
            ids.push(id);
        }
        watch.set_children(ids);
        nodes.push((DEBUG_WATCH, watch));
        kids.push(DEBUG_WATCH);
        kids.push(push_button(nodes, DEBUG_WATCH_ADD, t("Add Expression…"), None, true));

        let mut stack = Node::new(Role::Tree);
        stack.set_label(t("Call Stack"));
        let mut ids = Vec::new();
        for (i, row) in self.stack.iter().enumerate() {
            let id = NodeId(STACK_ROW + i as u64);
            let from = nodes.len();
            let mut n = Node::new(Role::TreeItem);
            match row {
                StackRow::Session { name, status, current, .. } => {
                    n.set_label(name.clone());
                    n.set_value(status.clone());
                    n.set_level(1);
                    n.set_selected(*current);
                    n.add_action(Action::Click);
                }
                StackRow::Thread { id: thread, name, current } => {
                    n.set_label(name.clone());
                    n.set_description(format!("#{thread}"));
                    n.set_level(2);
                    n.set_selected(*current);
                    n.add_action(Action::Click);
                }
                StackRow::Frame { name, place, current, .. } => {
                    n.set_label(name.clone());
                    if !place.is_empty() {
                        n.set_description(place.clone());
                    }
                    n.set_level(3);
                    n.set_selected(*current);
                    n.add_action(Action::Click);
                    add_menu(nodes, &mut n, MenuRow::Frame, i, name);
                }
                StackRow::LoadMore => {
                    n.set_label(t("Load More Stack Frames"));
                    n.set_level(3);
                    n.add_action(Action::Click);
                }
                StackRow::Note(text) => {
                    n.set_role(Role::Label);
                    n.set_label(text.clone());
                }
            }
            geometry.apply_row(RowFrame::DebugStack, i, &mut n);
            geometry.apply_row_buttons(RowFrame::DebugStack, i, id.0, &mut nodes[from..]);
            nodes.push((id, n));
            ids.push(id);
        }
        if ids.is_empty() {
            stack.set_description(t("No debug session."));
        }
        stack.set_children(ids);
        nodes.push((DEBUG_STACK, stack));
        kids.push(DEBUG_STACK);

        let mut bps = Node::new(Role::List);
        bps.set_label(t("Breakpoints"));
        if self.bps.is_empty() {
            bps.set_description(t("No breakpoints: F9 or a click in the gutter adds one."));
        }
        let mut ids = Vec::new();
        for (i, bp) in self.bps.iter().enumerate() {
            let id = NodeId(BP_ROW + 2 * i as u64);
            let from = nodes.len();
            let data = t("Breaks when the value is written; for this debug session only.").to_string();
            let (label, enabled, detail) = match bp {
                BpSnap::Data { desc, enabled, .. } => (tf("Data breakpoint: {}", &[desc]), *enabled, &data),
                BpSnap::Line { path, line, enabled, detail } => (tf("{0}, line {1}", &[&crate::ui::file_name(path), &(line + 1)]), *enabled, detail),
                BpSnap::Function { name, enabled, detail, .. } => (format!("{name}()"), *enabled, detail),
            };
            let mut n = Node::new(Role::CheckBox);
            n.set_label(label.clone());
            n.set_toggled(Toggled::from(enabled));
            if !detail.is_empty() {
                n.set_description(detail.clone());
            }
            n.add_action(Action::Click);
            let mut buttons = vec![push_button(nodes, NodeId(id.0 + 1), t("Remove Breakpoint"), Some(label.clone()), true)];
            if matches!(bp, BpSnap::Line { .. }) {
                // A click on the row opens the place; in the tree the check box's Click toggles, so this button opens it.
                buttons.push(push_button(nodes, NodeId(BP_OPEN + i as u64), t("Go to Breakpoint"), Some(label.clone()), true));
            }
            n.set_children(buttons);
            if matches!(bp, BpSnap::Line { .. } | BpSnap::Data { .. }) {
                add_menu(nodes, &mut n, MenuRow::Breakpoint, i, &label);
            }
            geometry.apply_row(RowFrame::DebugBreakpoint, i, &mut n);
            geometry.apply_row_buttons(RowFrame::DebugBreakpoint, i, id.0, &mut nodes[from..]);
            nodes.push((id, n));
            ids.push(id);
        }
        bps.set_children(ids);
        nodes.push((DEBUG_BPS, bps));
        kids.push(DEBUG_BPS);
        kids.push(push_button(nodes, DEBUG_BP_FUNCTION, t("Add Function Breakpoint…"), None, true));
        let active = if self.active { t("Deactivate Breakpoints") } else { t("Activate Breakpoints") };
        kids.push(push_button(nodes, DEBUG_BP_ACTIVE, active, None, true));

        let mut view = Node::new(Role::Group);
        view.set_label(t("Run and Debug"));
        view.set_children(kids);
        nodes.push((DEBUG, view));
        DEBUG
    }
}

/// The node of row `index` of a Run and Debug list (`None` for another list).
pub(in crate::workbench) fn row_node(kind: RowFrame, index: usize) -> Option<NodeId> {
    let i = index as u64;
    match kind {
        RowFrame::DebugVariable if index < MAX_VARS => Some(NodeId(VAR_ROW + i)),
        RowFrame::DebugWatch if index < MAX_WATCHES => Some(NodeId(WATCH_ROW + 2 * i)),
        RowFrame::DebugStack if index < MAX_STACK => Some(NodeId(STACK_ROW + i)),
        RowFrame::DebugBreakpoint if index < MAX_BPS => Some(NodeId(BP_ROW + 2 * i)),
        _ => None,
    }
}

impl ToolbarSnap {
    fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
        let resume = if self.running {
            push_button(nodes, DEBUG_CONTINUE, t("Pause (F6)"), None, true)
        } else {
            push_button(nodes, DEBUG_CONTINUE, t("Continue (F5)"), None, self.stopped)
        };
        let kids = vec![
            resume,
            push_button(nodes, DEBUG_OVER, t("Step Over (F10)"), None, self.stopped),
            push_button(nodes, DEBUG_INTO, t("Step Into (F11)"), None, self.stopped),
            push_button(nodes, DEBUG_OUT, t("Step Out (⇧F11)"), None, self.stopped),
            push_button(nodes, DEBUG_RESTART, t("Restart (⇧⌘F5)"), None, true),
            push_button(nodes, DEBUG_STOP, t("Stop (⇧F5)"), None, true),
        ];
        let mut bar = Node::new(Role::Toolbar);
        bar.set_label(t("Debug toolbar"));
        bar.set_description(self.name.clone());
        bar.set_children(kids);
        nodes.push((DEBUG_TOOLBAR, bar));
        DEBUG_TOOLBAR
    }
}

impl DebugSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>, kids: &mut Vec<NodeId>, geometry: &Geometry) {
        kids.extend(self.view.as_ref().map(|v| v.append_tree(nodes, geometry)));
        kids.extend(self.toolbar.as_ref().map(|b| b.append_tree(nodes)));
    }
}

impl Workbench {
    pub(super) fn a11y_debug(&self, cx: &App) -> DebugSnap {
        let toolbar = self.dbg.cur().filter(|d| !matches!(d.status, DebugStatus::Ended(_))).map(|d| ToolbarSnap {
            session: d.id,
            name: d.config.name.clone(),
            stopped: matches!(d.status, DebugStatus::Stopped(_)),
            running: d.status == DebugStatus::Running,
        });
        DebugSnap { view: (self.chrome.panel == Some(Panel::Debug)).then(|| self.a11y_debug_view(cx)), toolbar }
    }

    fn a11y_debug_view(&self, _cx: &App) -> DebugViewSnap {
        let d = &self.dbg;
        let start = d.last.clone().filter(|n| d.catalog.contains(n));
        let configs = d.catalog.iter().take(MAX_CONFIGS).map(|n| (n.clone(), start.as_ref() == Some(n))).collect();
        let cur = d.cur();
        let mut vars = cur.map(variable_rows).unwrap_or_default();
        vars.truncate(MAX_VARS);
        let stopped = cur.is_some_and(|s| matches!(s.status, DebugStatus::Stopped(_)));
        let values = cur.map(|s| s.watch_values.clone()).unwrap_or_default();
        let watch = d
            .watch_exprs
            .iter()
            .take(MAX_WATCHES)
            .enumerate()
            .map(|(i, e)| (e.clone(), values.get(i).cloned().unwrap_or_else(|| if stopped { "…".into() } else { t("not available").into() })))
            .collect();
        let mut stack = stack_rows(&d.sessions, cur.map(|s| s.id));
        stack.truncate(MAX_STACK);
        let bps = self
            .bp_rows()
            .into_iter()
            .take(MAX_BPS)
            .map(|row| match row {
                BpRow::Data { index, bp } => BpSnap::Data { index, desc: bp.desc, enabled: bp.enabled },
                BpRow::Line { path, line, meta } => BpSnap::Line { enabled: !meta.disabled, detail: bp_detail(&meta), path, line },
                BpRow::Function { index, bp } => {
                    let detail = match &bp.verified {
                        Verified::No(why) => why.clone().unwrap_or_else(|| t("The debugger could not bind this breakpoint.").into()),
                        _ => bp.condition.clone().unwrap_or_default(),
                    };
                    BpSnap::Function { index, name: bp.name, enabled: !bp.disabled, detail }
                }
            })
            .collect();
        DebugViewSnap {
            configs,
            start,
            session: cur.map(|s| (s.id, s.config.name.clone(), status_text(&s.status))),
            vars,
            watch,
            stack,
            bps,
            active: d.bps.active,
        }
    }

    /// An action on a node of this module; `false` if the node is not one of them.
    pub(super) fn a11y_debug_action(&mut self, req: &ActionRequest, last: &DebugSnap, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        if FIXED[12..19].contains(&id) {
            if req.action == Action::Click {
                self.a11y_toolbar_action(id, last.toolbar.as_ref(), window, cx);
            }
            return true;
        }
        let menu_button = [MenuRow::Variable, MenuRow::Frame, MenuRow::Breakpoint].into_iter().any(|k| menu_request(req, k, |_| None).is_some());
        if !((FIXED[..12].contains(&id) || id == DEBUG_CHOOSE) || (DEBUG_ROW..DEBUG_ROW + BLOCK).contains(&id.0) || menu_button) {
            return false;
        }
        let Some(old) = last.view.as_ref() else { return true };
        if self.chrome.panel != Some(Panel::Debug) {
            return true;
        }
        let now = self.a11y_debug_view(cx);
        if self.a11y_debug_menu(req, old, &now, window, cx) {
            return true;
        }
        let click = req.action == Action::Click;
        match id {
            // Start only what the tree said Start would start.
            DEBUG_START if click && now.start == old.start => self.start_selected(window, cx),
            DEBUG_OPEN_LAUNCH if click => self.open_launch_file(window, cx),
            DEBUG_WATCH_ADD if click => self.add_watch(&AddWatch, window, cx),
            DEBUG_BP_FUNCTION if click => self.add_function_breakpoint(&debug::AddFunctionBreakpoint, window, cx),
            DEBUG_BP_ACTIVE if click && now.active == old.active => self.toggle_breakpoints_active(&debug::ToggleBreakpointsActive, window, cx),
            NodeId(n) if (CONFIG_ROW..VAR_ROW).contains(&n) && click => {
                let Some((name, _)) = old.configs.get((n - CONFIG_ROW) as usize) else { return true };
                if let Some(i) = self.dbg.catalog.iter().position(|c| c == name) {
                    let target = MenuTarget { path: String::new(), is_dir: false, tab: None };
                    self.run_debug_menu(debug::DebugMenuCmd::Select(i), target, window, cx);
                }
            }
            NodeId(n) if (VAR_ROW..WATCH_ROW).contains(&n) => self.a11y_variable_action(req.action, (n - VAR_ROW) as usize, old, &now, cx),
            NodeId(n) if (WATCH_ROW..STACK_ROW).contains(&n) && click => {
                let (index, part) = (((n - WATCH_ROW) / 2) as usize, (n - WATCH_ROW) % 2);
                // The same expression, still at that place in the list.
                if part == 1 && old.watch.get(index).is_some_and(|(e, _)| self.dbg.watch_exprs.get(index) == Some(e)) {
                    self.remove_watch(index, cx);
                }
            }
            NodeId(n) if (STACK_ROW..BP_ROW).contains(&n) && click => self.a11y_stack_action((n - STACK_ROW) as usize, old, &now, cx),
            NodeId(n) if (BP_OPEN..BP_OPEN + MAX_BPS as u64).contains(&n) && click => {
                // Only the same line breakpoint, still listed.
                if let Some(BpSnap::Line { path, line, .. }) = old.bps.get((n - BP_OPEN) as usize)
                    && now.bps.iter().any(|b| matches!(b, BpSnap::Line { path: p, line: l, .. } if p == path && l == line))
                {
                    self.open_location(path.clone(), *line as u32, 0, window, cx);
                }
            }
            DEBUG_CHOOSE if click => {
                let at = self.chrome.a11y_bounds.part_in(window.window_handle().window_id(), Part::Control("dbg-config")).map(|b| b.bottom_left());
                self.open_config_menu(at.unwrap_or_default(), window, cx);
            }
            NodeId(n) if (BP_ROW..BP_OPEN).contains(&n) && click => {
                let (index, part) = (((n - BP_ROW) / 2) as usize, (n - BP_ROW) % 2);
                let Some(bp) = old.bps.get(index) else { return true };
                // The breakpoint must still be listed as the tree showed it (a toggle from an old tree is not repeated).
                let same = |b: &&BpSnap| match (b, bp) {
                    (BpSnap::Data { index, desc, enabled }, BpSnap::Data { index: i, desc: d, enabled: e }) => (index, desc, enabled) == (i, d, e),
                    (BpSnap::Line { path, line, enabled, .. }, BpSnap::Line { path: p, line: l, enabled: e, .. }) => (path, line, enabled) == (p, l, e),
                    (BpSnap::Function { index, name, enabled, .. }, BpSnap::Function { index: i, name: m, enabled: e, .. }) => {
                        (index, name, enabled) == (i, m, e)
                    }
                    _ => false,
                };
                let Some(current) = now.bps.iter().find(same).cloned() else { return true };
                match (current, part) {
                    (BpSnap::Data { index, enabled, .. }, 0) => self.set_data_breakpoint_enabled(index, !enabled, cx),
                    (BpSnap::Data { index, .. }, _) => self.remove_data_breakpoint(index, cx),
                    (BpSnap::Line { path, line, enabled, .. }, 0) => self.set_breakpoint_enabled(path, line, !enabled, cx),
                    (BpSnap::Line { path, line, .. }, _) => {
                        let target = MenuTarget { path: format!("{path}\t{line}"), is_dir: false, tab: None };
                        self.run_debug_menu(debug::DebugMenuCmd::RemoveBreakpoint, target, window, cx);
                    }
                    (BpSnap::Function { index, enabled, .. }, 0) => self.set_function_breakpoint_enabled(index, !enabled, cx),
                    (BpSnap::Function { index, .. }, _) => self.remove_function_breakpoint(index, cx),
                }
            }
            _ => {}
        }
        true
    }

    /// A row menu (`ShowContextMenu` or "More Actions…"): opened only for the same variable, frame or breakpoint, still at
    /// that place in the same session, as a right click on the row opens it.
    fn a11y_debug_menu(&mut self, req: &ActionRequest, old: &DebugViewSnap, now: &DebugViewSnap, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows = |from: u64, to: u64| move |id: u64| (from..to).contains(&id).then(|| (id - from) as usize);
        let same_session = old.session.as_ref().map(|s| s.0) == now.session.as_ref().map(|s| s.0);
        if let Some(i) = menu_request(req, MenuRow::Variable, rows(VAR_ROW, WATCH_ROW)) {
            if let Some(r) = old.vars.get(i).filter(|r| same_session && r.parent != 0 && r.more.is_none() && now.vars.get(i) == Some(*r)) {
                let at = self.row_menu_anchor(MenuRow::Variable, i, window);
                self.open_variable_menu(r.parent, r.name.clone(), at, window, cx);
            }
        } else if let Some(i) = menu_request(req, MenuRow::Frame, rows(STACK_ROW, BP_ROW)) {
            if let Some(StackRow::Frame { index, .. }) = old.stack.get(i).filter(|r| same_session && now.stack.get(i) == Some(*r)) {
                let at = self.row_menu_anchor(MenuRow::Frame, i, window);
                self.open_frame_menu(*index, at, window, cx);
            }
        } else if let Some(i) = menu_request(req, MenuRow::Breakpoint, |id| rows(BP_ROW, BP_OPEN)(id).filter(|k| k.is_multiple_of(2)).map(|k| k / 2)) {
            match old.bps.get(i).filter(|b| now.bps.get(i) == Some(*b)) {
                Some(BpSnap::Line { path, line, enabled, .. }) => {
                    let at = self.row_menu_anchor(MenuRow::Breakpoint, i, window);
                    self.open_breakpoint_menu(path.clone(), *line, !enabled, at, window, cx);
                }
                Some(BpSnap::Data { index, enabled, .. }) => {
                    let at = self.row_menu_anchor(MenuRow::Breakpoint, i, window);
                    self.open_data_breakpoint_menu(*index, *enabled, at, window, cx);
                }
                _ => {}
            }
        } else {
            return false;
        }
        true
    }

    /// A variable row: opened or closed only if the same variable is still there in the same state.
    fn a11y_variable_action(&mut self, action: Action, index: usize, old: &DebugViewSnap, now: &DebugViewSnap, cx: &mut Context<Self>) {
        let Some(row) = old.vars.get(index) else { return };
        if old.session.as_ref().map(|s| s.0) != now.session.as_ref().map(|s| s.0) || !now.vars.contains(row) {
            return;
        }
        match (action, row.more) {
            (Action::Click, Some(paged)) => self.load_more_variables(paged, cx),
            (Action::Click, None) | (Action::Expand, None) | (Action::Collapse, None) if row.reference > 0 => {
                let wanted = match action {
                    Action::Expand => !row.open,
                    Action::Collapse => row.open,
                    _ => true,
                };
                if wanted {
                    self.toggle_variable(row.reference, cx);
                }
            }
            _ => {}
        }
    }

    /// A Call Stack row: the same session, thread or frame must still be there.
    fn a11y_stack_action(&mut self, index: usize, old: &DebugViewSnap, now: &DebugViewSnap, cx: &mut Context<Self>) {
        let Some(row) = old.stack.get(index) else { return };
        let same_session = old.session.as_ref().map(|s| s.0) == now.session.as_ref().map(|s| s.0);
        match row {
            StackRow::Session { id, .. } if now.stack.iter().any(|r| matches!(r, StackRow::Session { id: i, .. } if i == id)) => {
                self.focus_debug_session(*id, cx)
            }
            StackRow::Thread { id, .. } if same_session && now.stack.iter().any(|r| matches!(r, StackRow::Thread { id: i, .. } if i == id)) => {
                self.select_thread(*id, cx)
            }
            StackRow::Frame { index, name, place, .. }
                if same_session
                    && now.stack.iter().any(|r| matches!(r, StackRow::Frame { index: i, name: n, place: p, .. } if (i, n, p) == (index, name, place))) =>
            {
                self.select_frame(*index, cx)
            }
            StackRow::LoadMore if same_session && now.stack.contains(&StackRow::LoadMore) => self.load_more_frames(cx),
            _ => {}
        }
    }

    /// A toolbar button: it acts on the session the tree showed, and only while that session is still current.
    fn a11y_toolbar_action(&mut self, id: NodeId, last: Option<&ToolbarSnap>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(old) = last else { return };
        let live = self.dbg.cur().filter(|d| !matches!(d.status, DebugStatus::Ended(_))).map(|d| d.id);
        if live != Some(old.session) {
            return;
        }
        match id {
            DEBUG_CONTINUE => self.debug_control(if old.running { "pause" } else { "continue" }, cx),
            DEBUG_OVER => self.debug_control("next", cx),
            DEBUG_INTO => self.debug_control("stepIn", cx),
            DEBUG_OUT => self.debug_control("stepOut", cx),
            DEBUG_RESTART => self.restart_debugging(&RestartDebugging, window, cx),
            DEBUG_STOP => self.stop_debugging(&StopDebugging, window, cx),
            _ => {}
        }
    }
}
