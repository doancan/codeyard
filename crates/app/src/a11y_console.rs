//! The Debug Console for assistive technology: the focused session's output as a polite `Log` (new lines are spoken),
//! the expression field (`Focus`, `SetValue`) and an Evaluate button that does what Enter in the field does. Every
//! action applies only to the session the tree showed.

use super::*;

const CONSOLE: NodeId = NodeId(52);
const CONSOLE_LOG: NodeId = NodeId(53);
const CONSOLE_INPUT: NodeId = NodeId(147);
const CONSOLE_EVALUATE: NodeId = NodeId(148);
/// This module's fixed nodes, for the id layout check.
pub(in crate::workbench) const FIXED: [NodeId; 4] = [CONSOLE, CONSOLE_LOG, CONSOLE_INPUT, CONSOLE_EVALUATE];
/// Output lines exposed at most (the console draws the last 300).
const MAX_LINES: usize = 300;

/// The Debug Console while it is the panel's view.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ConsoleSnap {
    /// The session it shows (`None`: no session, only a note).
    session: Option<(u64, String)>,
    lines: Vec<String>,
    input: String,
    pub(super) focused: bool,
}

impl ConsoleSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>, geometry: &Geometry) -> NodeId {
        let mut group = Node::new(Role::Group);
        group.set_label(t("Debug Console"));
        geometry.apply_part(Part::Console, &mut group);
        let Some((_, name)) = &self.session else {
            let mut empty = Node::new(Role::Status);
            empty.set_label(t("No debug session. F5 starts one."));
            empty.set_live(Live::Polite);
            nodes.push((CONSOLE_LOG, empty));
            group.set_children(vec![CONSOLE_LOG]);
            nodes.push((CONSOLE, group));
            return CONSOLE;
        };
        group.set_description(name.clone());
        let mut log = Node::new(Role::Log);
        log.set_label(tf("Output of {}", &[name]));
        log.set_value(self.lines.join("\n"));
        log.set_live(Live::Polite);
        geometry.apply_part(Part::ConsoleLog, &mut log);
        nodes.push((CONSOLE_LOG, log));
        let mut input = Node::new(Role::TextInput);
        input.set_label(t("Evaluate an expression (↵)"));
        input.set_value(self.input.clone());
        input.add_action(Action::Focus);
        input.add_action(Action::SetValue);
        geometry.apply_part(Part::ConsoleInput, &mut input);
        nodes.push((CONSOLE_INPUT, input));
        let mut evaluate = Node::new(Role::Button);
        evaluate.set_label(t("Evaluate"));
        if self.input.trim().is_empty() {
            evaluate.set_disabled();
        } else {
            evaluate.add_action(Action::Click);
        }
        nodes.push((CONSOLE_EVALUATE, evaluate));
        group.set_children(vec![CONSOLE_LOG, CONSOLE_INPUT, CONSOLE_EVALUATE]);
        nodes.push((CONSOLE, group));
        CONSOLE
    }

    pub(super) fn focus_node(&self) -> Option<NodeId> {
        self.focused.then_some(CONSOLE_INPUT)
    }
}

impl Workbench {
    /// The Debug Console while the panel shows it.
    pub(super) fn a11y_console(&self, window: &Window, cx: &App) -> Option<ConsoleSnap> {
        if !(self.chrome.panel_open && self.chrome.bottom == Bottom::Debug) {
            return None;
        }
        let Some(d) = self.dbg.cur() else { return Some(ConsoleSnap { session: None, lines: Vec::new(), input: String::new(), focused: false }) };
        let skip = d.console.len().saturating_sub(MAX_LINES);
        let lines = d.console[skip..].iter().chain(d.partial.iter()).map(|(_, line)| line.clone()).collect();
        let input = d.input.read(cx);
        Some(ConsoleSnap {
            session: Some((d.id, d.config.name.clone())),
            lines,
            input: input.text().to_string(),
            focused: input.focus_handle(cx).is_focused(window),
        })
    }

    /// An action on the console: only for the session the tree showed, still the one shown.
    pub(super) fn a11y_console_action(&mut self, req: &ActionRequest, last: Option<&ConsoleSnap>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !FIXED.contains(&req.target_node) {
            return false;
        }
        let (Some(was), Some(now)) = (last, self.a11y_console(window, cx)) else { return true };
        let Some((id, _)) = was.session.as_ref().filter(|_| was.session == now.session) else { return true };
        let Some(input) = self.dbg.cur().filter(|d| d.id == *id).map(|d| d.input.clone()) else { return true };
        match (req.action, req.target_node) {
            (Action::Focus, CONSOLE_INPUT) => window.focus(&input.focus_handle(cx)),
            (Action::SetValue, CONSOLE_INPUT) => {
                if let Some(ActionData::Value(text)) = &req.data {
                    input.update(cx, |f, cx| f.set_text(text, cx));
                }
            }
            // As Enter in the field: the text is evaluated and the field emptied (only the text the tree showed).
            (Action::Click, CONSOLE_EVALUATE) if now.input == was.input && !now.input.trim().is_empty() => {
                input.update(cx, |f, cx| f.set_text("", cx));
                self.debug_evaluate(now.input, cx);
            }
            _ => {}
        }
        cx.notify();
        true
    }
}
