//! The plain-data snapshots the workbench hands to the accessibility tree (what each part of the window holds).
use super::*;

/// The explorer tree of one folder of the workspace.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ExplorerSnap {
    pub name: String,
    pub rows: Vec<RowSnap>,
    pub rows_total: usize,
    /// Index of the first row of `rows` in the whole tree.
    pub first: usize,
    pub focused: Option<u64>,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct RowSnap {
    pub id: u64,
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub selected: bool,
    /// Where the row was painted (`None`: scrolled out of view).
    pub frame: Option<accesskit::Rect>,
}

/// A text run: a slice of a single line (at most RUN_CHARS characters).
#[derive(Clone, PartialEq, Debug)]
pub struct RunSnap {
    /// Index of the first character in the document.
    pub start: usize,
    /// Character count including the line ending.
    pub chars: usize,
    pub text: String,
    /// Where it was painted (`None`: not on screen).
    pub frame: Option<RunFrame>,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct EditorSnap {
    pub name: String,
    pub dirty: bool,
    pub runs: Vec<RunSnap>,
    /// (anchor, cursor) character indices.
    pub selection: (usize, usize),
    pub total_lines: usize,
    pub first_line: usize,
}

/// The find bar of the active editor.
#[derive(Clone, PartialEq, Debug)]
pub struct FindSnap {
    pub query: String,
    /// The replacement, while the replace row is shown.
    pub replacement: Option<String>,
    /// "3 of 12", "No results" or a pattern error.
    pub status: Option<String>,
    /// Options (with their state) and buttons, in visual order.
    pub controls: Vec<(String, Option<bool>)>,
    /// Where its parts were painted.
    pub frames: Vec<(crate::find::FindFrame, accesskit::Rect)>,
}

impl FindSnap {
    pub(super) fn place(&self, part: crate::find::FindFrame, node: &mut Node) {
        if let Some((_, bounds)) = self.frames.iter().find(|(k, _)| *k == part) {
            node.set_bounds(*bounds);
        }
    }
}

/// A list of options: picker rows, suggestions or extensions.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ListSnap {
    pub rows: Vec<(String, Option<String>)>,
    pub rows_total: usize,
    pub selected: Option<usize>,
    /// Where each row was painted (physical pixels; `None` or missing: not on screen).
    pub frames: Vec<Option<accesskit::Rect>>,
    /// Where the list itself was painted.
    pub bounds: Option<accesskit::Rect>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct PickerSnap {
    pub label: String,
    pub message: Option<String>,
    pub query: String,
    pub list: ListSnap,
    pub empty: Option<String>,
    /// Where the picker and its query box were painted in this window.
    pub bounds: Option<accesskit::Rect>,
    pub input: Option<accesskit::Rect>,
}

impl PickerSnap {
    /// The picker as drawn in `window` (no bounds when it was drawn in another window).
    pub(super) fn of(picker: &crate::picker::Picker, window: &Window) -> Self {
        let v = picker.a11y_view(MAX_OPTIONS, window);
        PickerSnap {
            label: v.placeholder,
            message: v.message,
            query: v.query,
            list: ListSnap { rows: v.rows, rows_total: v.rows_total, selected: v.selected, frames: v.frames, bounds: v.list },
            empty: v.empty,
            bounds: v.bounds,
            input: v.input,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct TerminalSnap {
    pub label: String,
    pub runs: Vec<RunSnap>,
    /// Character index of the cursor, if it is on screen.
    pub cursor: Option<usize>,
    /// Lines scrolled back into the history, and the history's length.
    pub scroll: (usize, usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FocusSnap {
    #[default]
    Window,
    /// Index of the explorer tree.
    Explorer(usize),
    Editor,
    /// 0 query, 1 replacement.
    Find(usize),
    Picker,
    Terminal,
    ScmMessage,
    /// The change list of Source Control (its cursor row is the focused node).
    ScmChanges,
    /// The repository list of Source Control (the selected repository is the focused option).
    ScmRepos,
    AboutCopy,
    ExtSearch,
}

/// The update dialog (or the first-run question) while it is open.
#[derive(Clone, PartialEq, Debug)]
pub struct DialogSnap {
    pub title: String,
    pub description: String,
    /// `Some(None)`: working, no end in sight; `Some(Some(f))`: `f` of 1 done.
    pub progress: Option<Option<f64>>,
    /// Button labels in tab order, and the focused one.
    pub buttons: Vec<String>,
    pub focused: Option<usize>,
}

/// An editor tab of the focused group.
#[derive(Clone, PartialEq, Debug)]
pub struct TabSnap {
    pub name: String,
    pub dirty: bool,
}

/// The About tab and its visible contents.
#[derive(Clone, PartialEq, Debug)]
pub struct AboutSnap {
    pub active: bool,
    pub details: Vec<(String, String)>,
}

/// The notice shown in the status bar; errors interrupt the screen reader, info waits its turn.
#[derive(Clone, PartialEq, Debug)]
pub struct NoticeSnap {
    pub text: String,
    pub error: bool,
}
