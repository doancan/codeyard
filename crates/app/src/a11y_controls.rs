//! Painted controls: a control drawn by any view (a status bar item, a header button, a row of a page, a gutter mark)
//! declares itself where it is drawn with [`control`], and the accessibility tree lists it, under the surface it
//! belongs to, with its role, name, state and painted frame. Its action is the same function its click runs. Only
//! what was laid out in a window's last frame is listed for that window, so a control scrolled away, hidden or on
//! another page is not in the tree, and an action from an outdated tree acts only while the same control with the
//! same name is still drawn there.

use std::collections::HashMap;
use std::rc::Rc;

use accesskit::{Action, ActionRequest, Node, NodeId, Role, Toggled};
use gpui::{App, Bounds, Context, FocusHandle, Global, IntoElement, Pixels, Window, WindowId, canvas, prelude::*, px};

use crate::workbench::Workbench;

/// Ids of painted controls: `CONTROL_BASE | hash(surface, key)`; their surfaces' groups `CONTROL_GROUP | hash(surface)`.
pub(crate) const CONTROL_BASE: u64 = 0x10_0000_0000;
pub(crate) const CONTROL_GROUP: u64 = 0x11_0000_0000;

/// What runs when the control is pressed (its click handler's work), after the tree's request has finished.
pub(crate) type ControlAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// A slider's setter: the new value, already clamped to its range.
pub(crate) type SliderAction = Rc<dyn Fn(f64, &mut Window, &mut App)>;

/// A slider's range: value, minimum, maximum and the step of Increment and Decrement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Range {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

/// A control as the view that draws it describes it.
#[derive(Clone)]
pub(crate) struct Control {
    /// The surface it belongs to (a group in the tree: "Status bar", "Welcome", "Merge editor"…).
    pub surface: String,
    /// What distinguishes it within its surface (an element id, a row key); stable while it means the same thing.
    pub key: String,
    pub role: Role,
    pub label: String,
    pub description: Option<String>,
    pub value: Option<String>,
    pub toggled: Option<bool>,
    pub selected: Option<bool>,
    pub expanded: Option<bool>,
    pub disabled: bool,
    pub focus: Option<FocusHandle>,
    /// A slider: its range, and what setting a value does.
    pub range: Option<(Range, SliderAction)>,
    pub action: Option<ControlAction>,
}

impl Control {
    /// A button of `surface`, keyed `key`, named `label`, that runs `action`.
    pub(crate) fn button(
        surface: impl Into<String>,
        key: impl Into<String>,
        label: impl Into<String>,
        action: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self::new(surface, key, Role::Button, label).on_press(action)
    }

    /// Text that is read (a heading, a line of a page) without an action.
    pub(crate) fn text(surface: impl Into<String>, key: impl Into<String>, role: Role, label: impl Into<String>) -> Self {
        Self::new(surface, key, role, label)
    }

    pub(crate) fn new(surface: impl Into<String>, key: impl Into<String>, role: Role, label: impl Into<String>) -> Self {
        Self {
            surface: surface.into(),
            key: key.into(),
            role,
            label: label.into(),
            description: None,
            value: None,
            toggled: None,
            selected: None,
            expanded: None,
            disabled: false,
            focus: None,
            range: None,
            action: None,
        }
    }

    /// Pressing it runs `f` on `entity` (as its click handler does), if the entity is still there.
    pub(crate) fn on_entity<T: 'static>(self, entity: gpui::WeakEntity<T>, f: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static) -> Self {
        self.on_press(move |window, cx| {
            entity.update(cx, |this, cx| f(this, window, cx)).ok();
        })
    }

    pub(crate) fn on_press(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.action = Some(Rc::new(action));
        self
    }

    pub(crate) fn description(mut self, d: impl Into<String>) -> Self {
        let d = d.into();
        self.description = (!d.is_empty()).then_some(d);
        self
    }

    pub(crate) fn toggled(mut self, on: bool) -> Self {
        self.toggled = Some(on);
        self
    }

    pub(crate) fn selected(mut self, on: bool) -> Self {
        self.selected = Some(on);
        self
    }

    pub(crate) fn expanded(mut self, open: bool) -> Self {
        self.expanded = Some(open);
        self
    }

    /// A slider over `range`: Increment, Decrement and a value set from the tree call `set` with the new value.
    pub(crate) fn slider(mut self, range: Range, set: impl Fn(f64, &mut Window, &mut App) + 'static) -> Self {
        self.range = Some((range, Rc::new(set)));
        self
    }

    /// A field's text.
    pub(crate) fn value(mut self, text: impl Into<String>) -> Self {
        self.value = Some(text.into());
        self
    }

    pub(crate) fn focus(mut self, handle: FocusHandle) -> Self {
        self.focus = Some(handle);
        self
    }
}

/// The controls each window laid out in its last frame, in the order they were laid out.
#[derive(Default)]
pub(crate) struct Controls {
    windows: HashMap<WindowId, Vec<(Control, Bounds<Pixels>)>>,
}

impl Global for Controls {}

impl Controls {
    /// `window` draws again: forget its controls.
    pub(crate) fn clear(cx: &mut App, window: WindowId) {
        if let Some(c) = cx.try_global::<Controls>().is_some().then(|| cx.global_mut::<Controls>()) {
            c.windows.remove(&window);
        }
    }

    /// Keep only the records of these windows (the others are closed).
    pub(crate) fn retain(cx: &mut App, open: &[WindowId]) {
        if cx.try_global::<Controls>().is_some() {
            cx.global_mut::<Controls>().windows.retain(|id, _| open.contains(id));
        }
    }

    fn of(cx: &App, window: WindowId) -> &[(Control, Bounds<Pixels>)] {
        cx.try_global::<Controls>().and_then(|c| c.windows.get(&window)).map_or(&[], Vec::as_slice)
    }
}

/// Declares `control` where it is drawn (its parent must be `relative`): recorded when it is laid out, with its
/// visible bounds. Takes no space, input or focus.
pub(crate) fn control(control: Control) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            let bounds = bounds.intersect(&window.content_mask().bounds);
            if bounds.size.width > px(0.) && bounds.size.height > px(0.) {
                let id = window.window_handle().window_id();
                cx.default_global::<Controls>().windows.entry(id).or_default().push((control.clone(), bounds));
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Where the control `key` of `surface` was laid out in `window`'s last frame (logical pixels), to open a menu there.
pub(crate) fn bounds_of(cx: &App, window: &Window, surface: &str, key: &str) -> Option<Bounds<Pixels>> {
    let surface = crate::i18n::t(surface);
    Controls::of(cx, window.window_handle().window_id()).iter().find(|(c, _)| c.surface == surface && c.key == key).map(|(_, b)| *b)
}

fn fnv(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0)) {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
    h & 0xffff_ffff
}

/// A painted control as the tree shows it.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ControlSnap {
    pub id: NodeId,
    pub surface: String,
    pub role: Role,
    pub label: String,
    pub description: Option<String>,
    pub value: Option<String>,
    pub toggled: Option<bool>,
    pub selected: Option<bool>,
    pub expanded: Option<bool>,
    pub disabled: bool,
    pub pressable: bool,
    pub focusable: bool,
    pub focused: bool,
    pub range: Option<Range>,
    pub bounds: Option<accesskit::Rect>,
}

/// The controls `window` drew in its last frame (one per id: a control laid out twice keeps its first place).
pub(crate) fn snapshot(window: &Window, cx: &App) -> Vec<ControlSnap> {
    let scale = window.scale_factor();
    let mut seen = std::collections::HashSet::new();
    Controls::of(cx, window.window_handle().window_id())
        .iter()
        .filter_map(|(c, bounds)| {
            let id = NodeId(CONTROL_BASE | fnv(&[&c.surface, &c.key]));
            seen.insert(id).then(|| ControlSnap {
                id,
                surface: c.surface.clone(),
                role: c.role,
                label: c.label.clone(),
                description: c.description.clone(),
                value: c.value.clone(),
                toggled: c.toggled,
                selected: c.selected,
                expanded: c.expanded,
                disabled: c.disabled,
                pressable: c.action.is_some() && !c.disabled,
                focusable: c.focus.is_some(),
                focused: c.focus.as_ref().is_some_and(|f| f.is_focused(window)),
                range: c.range.as_ref().map(|(r, _)| *r),
                bounds: crate::painted::physical_scaled(*bounds, scale),
            })
        })
        .collect()
}

/// The controls' nodes, gathered by surface in the order they were drawn: (surface, the controls' ids).
pub(crate) fn append_nodes(controls: &[ControlSnap], nodes: &mut Vec<(NodeId, Node)>) -> Vec<(String, Vec<NodeId>)> {
    let mut groups: Vec<(String, Vec<NodeId>)> = Vec::new();
    for c in controls {
        let mut n = Node::new(c.role);
        n.set_label(c.label.clone());
        if let Some(d) = &c.description {
            n.set_description(d.clone());
        }
        if let Some(v) = &c.value {
            n.set_value(v.clone());
        }
        if let Some(t) = c.toggled {
            n.set_toggled(Toggled::from(t));
        }
        if let Some(s) = c.selected {
            n.set_selected(s);
        }
        if let Some(e) = c.expanded {
            n.set_expanded(e);
        }
        if c.disabled {
            n.set_disabled();
        }
        if c.pressable {
            n.add_action(Action::Click);
            // A control that folds: Expand or Collapse is its click, offered only toward the other state.
            match c.expanded {
                Some(true) => n.add_action(Action::Collapse),
                Some(false) => n.add_action(Action::Expand),
                None => {}
            }
        }
        if c.focusable {
            n.add_action(Action::Focus);
        }
        if let Some(r) = c.range {
            n.set_numeric_value(r.value);
            n.set_min_numeric_value(r.min);
            n.set_max_numeric_value(r.max);
            n.set_numeric_value_step(r.step);
            n.add_action(Action::Increment);
            n.add_action(Action::Decrement);
            n.add_action(Action::SetValue);
        }
        if let Some(b) = c.bounds {
            n.set_bounds(b);
        }
        nodes.push((c.id, n));
        match groups.iter_mut().find(|(s, _)| *s == c.surface) {
            Some((_, ids)) => ids.push(c.id),
            None => groups.push((c.surface.clone(), vec![c.id])),
        }
    }
    groups
}

/// A surface's group of controls.
pub(crate) fn group(surface: String, ids: Vec<NodeId>, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
    let id = NodeId(CONTROL_GROUP | fnv(&[&surface]));
    let mut g = Node::new(Role::Group);
    g.set_label(surface);
    g.set_children(ids);
    nodes.push((id, g));
    id
}

/// UI Automation offers Expand and Collapse on every node with an expanded state. For a node that folds on Click only,
/// they mean that click when the node is in the other state: `Some(Some(click))`; `None` when there is nothing to do
/// (already in the state asked for); `Some(None)` for any other request, handled as it is. `tree` is the tree the
/// request was made from, built only for these two actions.
pub(crate) fn fold_as_click(req: &ActionRequest, tree: impl FnOnce() -> accesskit::TreeUpdate) -> Option<Option<ActionRequest>> {
    if !matches!(req.action, Action::Expand | Action::Collapse) {
        return Some(None);
    }
    let tree = tree();
    let Some((_, n)) = tree.nodes.iter().find(|(id, _)| *id == req.target_node) else { return Some(None) };
    if n.supports_action(req.action) {
        return Some(None);
    }
    (n.is_expanded() == Some(req.action == Action::Collapse) && n.supports_action(Action::Click))
        .then(|| Some(ActionRequest { action: Action::Click, ..req.clone() }))
}

/// The focused painted control, if one has the keyboard.
pub(crate) fn focus_node(controls: &[ControlSnap]) -> Option<NodeId> {
    controls.iter().find(|c| c.focused).map(|c| c.id)
}

impl Workbench {
    /// A press on a painted control: its action runs after this request (it may update any view, the workbench
    /// included), only while the same control with the same name is drawn in this window and can be pressed.
    pub(crate) fn a11y_control_action(&mut self, req: &ActionRequest, last: &[ControlSnap], window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        if !(CONTROL_BASE..CONTROL_BASE + 0x1_0000_0000).contains(&id.0) && !(CONTROL_GROUP..CONTROL_GROUP + 0x1_0000_0000).contains(&id.0) {
            return false;
        }
        let Some(was) = last.iter().find(|c| c.id == id) else { return true };
        let now = Controls::of(cx, window.window_handle().window_id())
            .iter()
            .find(|(c, _)| CONTROL_BASE | fnv(&[&c.surface, &c.key]) == id.0 && c.label == was.label)
            .map(|(c, _)| c.clone());
        let Some(now) = now else { return true };
        match req.action {
            Action::Expand | Action::Collapse if !now.disabled && now.expanded == Some(req.action == Action::Collapse) => {
                if let Some(action) = now.action {
                    window.defer(cx, move |window, cx| action(window, cx));
                }
            }
            Action::Click if !now.disabled => {
                if let Some(action) = now.action {
                    window.defer(cx, move |window, cx| action(window, cx));
                }
            }
            Action::Focus => {
                if let Some(f) = now.focus {
                    window.focus(&f);
                }
            }
            Action::Increment | Action::Decrement | Action::SetValue if !now.disabled => {
                let Some((r, set)) = now.range else { return true };
                let to = match (req.action, &req.data) {
                    (Action::Increment, _) => r.value + r.step,
                    (Action::Decrement, _) => r.value - r.step,
                    (_, Some(accesskit::ActionData::NumericValue(v))) => *v,
                    (_, Some(accesskit::ActionData::Value(v))) => match v.trim().parse::<f64>() {
                        Ok(v) => v,
                        Err(_) => return true,
                    },
                    _ => return true,
                };
                let to = to.clamp(r.min, r.max);
                window.defer(cx, move |window, cx| set(to, window, cx));
            }
            _ => {}
        }
        true
    }
}

impl Workbench {
    /// A button of the workbench's own chrome for the tree (the element it is in must be `relative`): `surface`,
    /// keyed `key`, named `label`, pressing it runs `f` as the click does.
    /// A drawn button `el` with its control `c` for the tree: the click and the tree's press run the same `f`.
    pub(crate) fn pressable(
        &self,
        el: gpui::Stateful<gpui::Div>,
        c: Control,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        press_on(el, c, cx, f)
    }

    pub(crate) fn ctl<L: Into<String>, F: Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static>(
        &self,
        surface: &str,
        key: &'static str,
        label: L,
        cx: &Context<Self>,
        f: F,
    ) -> impl IntoElement + use<L, F> {
        control(Control::button(surface, key, label, |_, _| {}).on_entity(cx.entity().downgrade(), f))
    }
}

/// A button for the tree on any view `T` (the element it is in must be `relative`): pressing it runs `f` on the view
/// as its click does.
pub(crate) fn button_on<T: 'static, L: Into<String>, F: Fn(&mut T, &mut Window, &mut Context<T>) + 'static>(
    cx: &Context<T>,
    surface: &str,
    key: String,
    label: L,
    f: F,
) -> impl IntoElement + use<T, L, F> {
    control(Control::button(surface, key, label, |_, _| {}).on_entity(cx.entity().downgrade(), f))
}

/// A drawn button `el` of any view with its control `c` for the tree: the click and the tree's press run the same `f`.
pub(crate) fn press_on<T: 'static>(
    el: gpui::Stateful<gpui::Div>,
    c: Control,
    cx: &Context<T>,
    f: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let f = Rc::new(f);
    let press = f.clone();
    el.relative()
        .child(control(c.on_entity(cx.entity().downgrade(), move |this: &mut T, w, cx| press(this, w, cx))))
        .on_click(cx.listener(move |this, _, w, cx| f(this, w, cx)))
}

/// Text a page draws, for the tree: a label (or heading, document) of `surface` where it is drawn.
pub(crate) fn said(surface: impl Into<String>, key: impl Into<String>, role: Role, text: impl Into<String>) -> gpui::AnyElement {
    control(Control::text(surface, key, role, text)).into_any_element()
}

/// A drawn button `el` of any view whose click handler takes the click event, with its control `c` for the tree: the
/// tree's press runs the same handler with a default event (no pointer position, no modifiers).
pub(crate) fn click_on<T: 'static>(
    el: gpui::Stateful<gpui::Div>,
    c: Control,
    cx: &Context<T>,
    f: impl Fn(&mut T, &gpui::ClickEvent, &mut Window, &mut Context<T>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let f = Rc::new(f);
    let press = f.clone();
    el.relative()
        .child(control(c.on_entity(cx.entity().downgrade(), move |this: &mut T, w, cx| press(this, &gpui::ClickEvent::default(), w, cx))))
        .on_click(cx.listener(move |this, e, w, cx| f(this, e, w, cx)))
}
