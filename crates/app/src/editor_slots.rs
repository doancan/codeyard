//! Per-group editor elements retain independent view state through layout, paint and input.

use super::*;

/// Element that paints the editor with a given view (one per group).
pub struct SlotView {
    editor: Entity<EditorView>,
    slot: usize,
}

pub fn slot_view(editor: Entity<EditorView>, slot: usize) -> SlotView {
    SlotView { editor, slot }
}

impl SlotView {
    /// While `f` runs the editor is in this view; afterwards it returns to its old view.
    fn within<R>(&self, cx: &mut App, f: impl FnOnce(&mut App) -> R) -> R {
        let slot = self.slot;
        let prev = self.editor.update(cx, |e, _| {
            let prev = e.slot;
            e.enter(slot);
            prev
        });
        let r = f(cx);
        self.editor.update(cx, |e, _| e.enter(prev));
        r
    }
}

impl IntoElement for SlotView {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for SlotView {
    type RequestLayoutState = gpui::AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, gpui::AnyElement) {
        let editor = self.editor.clone();
        let slot = self.slot;
        let mut el = self.within(cx, |cx| {
            editor.update(cx, |e, cx| {
                e.enter(slot);
                e.render(window, cx).into_any_element()
            })
        });
        let id = self.within(cx, |cx| el.request_layout(window, cx));
        (id, el)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        el: &mut gpui::AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        let hidden = bounds.size.width < px(320.);
        let changed = self.within(cx, |cx| self.editor.update(cx, |e, _| std::mem::replace(&mut e.minimap_hidden, hidden) != hidden));
        if changed {
            let editor = self.editor.downgrade();
            window.defer(cx, move |_, cx| {
                editor.update(cx, |_, cx| cx.notify()).ok();
            });
        }
        self.within(cx, |cx| el.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        el: &mut gpui::AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(test)]
        SLOT_PAINTS.with(|counts| *counts.borrow_mut().entry((self.editor.entity_id(), self.slot)).or_default() += 1);
        self.within(cx, |cx| el.paint(window, cx));
    }
}
