//! Editor view (S03): virtualized lines, cursor, selection, IME, saving.

use std::ops::Range;
use std::sync::Arc;

use buffer::{Buffer, LineEnding};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollStrategy, ShapedLine, SharedString,
    Style, TextRun, UTF16Selection, UniformListScrollHandle, Window, actions, canvas, div, fill, point, prelude::*, px, relative, size, uniform_list,
};
use host::{Host, HostError};
use syntax::{Role, Syntax};

use crate::display::{self, DisplayMap};
use crate::editor_decor;
use crate::encoding::Encoding;
use crate::find::{self, FindBar, FindEvent};
use crate::settings::{AutoSave, Settings};
use crate::ui::{self, MONO_FONT, Palette, c, icon, palette, text};

actions!(
    editor,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveHome,
        MoveEnd,
        SelectHome,
        SelectEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        PageUp,
        PageDown,
        Backspace,
        Delete,
        Newline,
        Tab,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Save,
        Escape,
        AddCursorAbove,
        AddCursorBelow,
        SelectNextOccurrence,
        Find,
        FindReplace,
        FindNext,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        TriggerCompletion,
        CompletionNext,
        CompletionPrev,
        CompletionAccept,
        CompletionDismiss,
        GoToDefinition,
        ShowHover,
        ToggleComment,
        Outdent,
        MoveLineUp,
        MoveLineDown,
        CopyLineUp,
        CopyLineDown,
        DeleteLine,
        SelectLine,
        FoldRegion,
        UnfoldRegion,
        FoldAll,
        UnfoldAll,
        SelectAllOccurrences,
        ExpandSelection,
        ShrinkSelection,
        JoinLines,
        SortLinesAscending,
        SortLinesDescending,
        TransformToUppercase,
        TransformToLowercase,
        NextProblem,
        PreviousProblem,
        GoToTypeDefinition,
        GoToImplementation,
        RevertFile,
        DeleteToLineStart,
        InsertLineBelow,
        InsertLineAbove,
        IndentLine,
        OutdentLine,
        SelectPageUp,
        SelectPageDown,
        JumpToBracket,
        CursorUndo,
        CursorsAtLineEnds,
        ToggleBookmark,
        ToggleBreakpoint,
        NextBookmark,
        PreviousBookmark,
        ClearBookmarks,
        AcceptCurrentChange,
        AcceptIncomingChange,
        AcceptBothChanges,
        AcceptAllCurrent,
        AcceptAllIncoming,
        NextConflict,
        PreviousConflict,
        DuplicateSelection,
        TrimTrailingWhitespace,
        TransposeCharacters,
        ReverseLines,
        DeleteDuplicateLines,
        TransformToTitleCase,
        TransformToCamelCase,
        TransformToPascalCase,
        TransformToSnakeCase,
        TransformToKebabCase,
        TransformToConstantCase,
        ToggleBlockComment,
        StageHunk,
        RevertHunk,
        RunCodeLens,
        PeekDefinition,
        ShowIncomingCalls,
        ShowOutgoingCalls,
        ShowSupertypes,
        ShowSubtypes,
        DeleteAllRight,
        SelectToBracket,
        AddLineComment,
        RemoveLineComment,
        AddSelectionToPreviousFindMatch,
        NextChange,
        PreviousChange,
        FoldLevel1,
        FoldLevel2,
        FoldLevel3,
        FoldLevel4,
        FoldLevel5,
        FoldLevel6,
        FoldLevel7,
        FoldRecursively,
        UnfoldRecursively,
        ColumnSelectUp,
        ColumnSelectDown,
        ColumnSelectLeft,
        ColumnSelectRight,
        TransformBase64Encode,
        TransformBase64Decode,
        TransformUrlEncode,
        TransformUrlDecode,
        TransformHtmlEscape,
        TransformHtmlUnescape,
        PickColor,
        SortLinesInsensitive,
        SortLinesNatural,
        RemoveBlankLines,
        CollapseBlankLines,
        InsertDate,
        InsertDateTime,
        InsertUuid,
    ]
);

#[path = "editor_color.rs"]
mod color_glue;
#[path = "editor_complete.rs"]
mod complete_glue;
// --- data ---
#[path = "editor_data.rs"]
mod data_glue;
pub use data_glue::DataSource;
#[path = "editor_display.rs"]
mod display_glue;
#[path = "editor_edit.rs"]
mod edit_glue;
#[path = "editor_find.rs"]
mod find_glue;
#[path = "editor_git.rs"]
mod git_glue;
#[path = "editor_indent.rs"]
mod indent_glue;
#[path = "editor_io.rs"]
mod io_glue;
#[path = "editor_lsp.rs"]
mod lsp_glue;
#[path = "editor_marks.rs"]
mod marks_glue;
#[path = "editor_mouse.rs"]
mod mouse_glue;
#[path = "editor_moves.rs"]
mod moves_glue;
#[path = "editor_popups.rs"]
mod popups_glue;
#[cfg(test)]
pub use popups_glue::HoverRow;
#[path = "editor_render.rs"]
mod render_glue;
#[path = "editor_schema.rs"]
mod schema_glue;
#[path = "editor_spell.rs"]
mod spell_glue;
pub use schema_glue::SchemaBinding;

pub enum EditorEvent {
    Changed,
    /// Written to disk (refresh the git status).
    Saved,
    /// A forced save replaced another version of the file (its text): keep a copy so it can be recovered.
    Overwrote(String),
    /// Breakpoints changed (sent to the adapter if a session is open).
    BreakpointsChanged,
    /// Go to definition: position in another (or the same) file (0-based line, UTF-16 column).
    OpenLocation {
        path: String,
        line: u32,
        col: u32,
    },
    /// Short message for the user (status bar).
    Message(String),
    /// A line for the Output panel (a language-server request failed).
    Log(String),
    /// Multiple results (implementations): let the user pick from a list.
    ShowLocations {
        locs: Vec<(String, (u32, u32))>,
        what: &'static str,
    },
    /// Bookmarks changed (for persistent storage).
    BookmarksChanged,
    /// Right click: context menu (window coordinates).
    ContextMenu(gpui::Point<Pixels>),
    /// The "Open Merge Editor" link on a conflict line.
    OpenMergeEditor,
    /// The "Send Request" link of a request in an `.http` file (request line, 0-based).
    SendRequest(usize),
}

/// Open completion list.
struct Completion {
    items: Vec<crate::lsp::CompletionItem>,
    /// Filtered order (indices into items).
    shown: Vec<usize>,
    selected: usize,
    /// Start of the word being typed (character).
    start: usize,
    /// The server returned an incomplete list: re-request as the user types.
    incomplete: bool,
    /// Typed prefix used for filtering (matching letters in the label are highlighted).
    prefix: String,
}

/// Info shown while the mouse hovers: diagnostics and the server's description.
struct Hover {
    at: usize,
    diagnostics: Vec<crate::lsp::Diagnostic>,
    text: Option<String>,
    position: Point<Pixels>,
}

/// "Peek Definition": a few lines around the target, shown as a card under the cursor.
struct Peek {
    path: String,
    /// Target line (0-based) and column.
    line: u32,
    col: u32,
    /// The line of `lines[0]` in the file (0-based).
    first: usize,
    lines: Vec<String>,
    /// How many other definitions there are.
    more: usize,
}

/// Cases that can occur when opening a file, where a message is shown instead of the editor.
pub enum OpenError {
    Host(HostError),
    Binary,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Host(e) => e.fmt(f),
            OpenError::Binary => f.write_str(crate::i18n::t("This looks like a binary file, so it was not opened as text.")),
        }
    }
}

/// Syntax highlighting is off above this size. The tree takes ~30x the source's size
/// (1 MB -> ~30 MB), so it is cut off at 1 MB to meet the low-RAM target.
const SYNTAX_MAX_BYTES: usize = 1024 * 1024;

pub struct Loaded {
    pub buffer: Buffer,
    pub hash: u64,
    pub syntax: Option<Syntax>,
    pub encoding: Encoding,
    /// `.editorconfig` rules (resolved in the background).
    pub rules: crate::editorconfig::Rules,
}

/// `files.associations`: (glob, language name or extension). Updated when settings load.
static ASSOCIATIONS: std::sync::RwLock<Vec<(String, String)>> = std::sync::RwLock::new(Vec::new());

pub fn set_associations(a: Vec<(String, String)>) {
    *ASSOCIATIONS.write().unwrap_or_else(|e| e.into_inner()) = a;
}

/// Language name -> extension the highlighter recognizes.
fn language_ext(lang: &str) -> String {
    let l = lang.trim_start_matches('.').to_ascii_lowercase();
    match l.as_str() {
        "rust" => "rs",
        "javascript" | "javascriptreact" => "js",
        "typescript" => "ts",
        "typescriptreact" => "tsx",
        "python" => "py",
        "markdown" => "md",
        "shellscript" | "shell" | "bash" => "sh",
        "ruby" => "rb",
        "csharp" => "cs",
        "kotlin" => "kt",
        "make" | "makefile" => "mk",
        "plaintext" | "text" => "txt",
        other => return other.to_string(),
    }
    .to_string()
}

/// A grammar name (a language name or file extension, as in `files.associations`) the highlighter has.
pub fn grammar_known(name: &str) -> bool {
    syntax::language_for_path(&format!("x.{}", language_ext(name))).is_some()
}

#[cfg(test)]
pub fn comment_token_for_test(path: &str) -> Option<&'static str> {
    comment_token(path)
}

/// The path the highlighter looks at: if `files.associations` matches, with the language's extension.
pub fn syntax_path(path: &str) -> std::borrow::Cow<'_, str> {
    let assoc = ASSOCIATIONS.read().unwrap_or_else(|e| e.into_inner());
    let name = path.rsplit('/').next().unwrap_or(path);
    for (glob, lang) in assoc.iter() {
        let hit = if glob.contains('/') {
            crate::globs::is_excluded(std::slice::from_ref(glob), path.trim_start_matches('/'))
        } else {
            crate::globs::is_excluded(std::slice::from_ref(glob), name)
        };
        if hit {
            return format!("{path}.{}", language_ext(lang)).into();
        }
    }
    drop(assoc);
    // An extension language highlights with the built-in grammar it names, for files no built-in grammar knows.
    if syntax::language_for_path(path).is_none()
        && let Some(grammar) = crate::ext::language_for(path).and_then(|l| l.grammar)
    {
        return format!("{path}.{}", language_ext(&grammar)).into();
    }
    path.into()
}

/// Read in the background, convert to a buffer, do the first parse (without blocking the UI thread).
/// If not UTF-8, decoded with the `fallback` encoding (UTF-16 with a BOM is recognized automatically).
pub fn load(host: &dyn Host, path: &str, fallback: Encoding) -> Result<Loaded, OpenError> {
    let (data, hash) = host.read(path).map_err(OpenError::Host)?;
    let (text, encoding) = crate::encoding::decode(&data, fallback).ok_or(OpenError::Binary)?;
    drop(data);
    let buffer = Buffer::from_str(&text);
    let syntax =
        (text.len() <= SYNTAX_MAX_BYTES).then(|| syntax::language_for_path(&syntax_path(path))).flatten().and_then(|lang| Syntax::new(lang, buffer.rope()));
    let rules = crate::editorconfig::resolve(host, path);
    Ok(Loaded { buffer, hash, syntax, encoding, rules })
}

/// Cursor style (`editor.cursorStyle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorShape {
    Line,
    Block,
    Underline,
}

impl CursorShape {
    fn from_setting(s: &str) -> CursorShape {
        match s {
            "block" => CursorShape::Block,
            "underline" => CursorShape::Underline,
            _ => CursorShape::Line,
        }
    }
}

/// Bracket colors are off for files larger than this (they are rescanned from the start on every edit).
const BRACKET_COLOR_MAX_CHARS: usize = 1_000_000;

struct VisibleLine {
    line: usize,
    /// Start of the line segment (character in the document) and its text: when wrapping, a line
    /// is split into multiple segments.
    start: usize,
    text: SharedString,
    bounds: Bounds<Pixels>,
    layout: ShapedLine,
    /// On-screen x of the text's column 0 (horizontal scroll offset subtracted).
    text_left: Pixels,
}

/// A new buffer epoch (never repeats in this process).
pub(crate) fn next_epoch() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// One exact state of an open document: which buffer object, which undo step, which text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocIdentity {
    pub epoch: u64,
    pub version: u64,
    pub hash: u64,
}

pub struct EditorView {
    host: Arc<dyn Host>,
    pub path: String,
    pub name: SharedString,
    buffer: Buffer,
    syntax: Option<Syntax>,
    /// Encoding on disk; saved with the same one.
    encoding: Encoding,
    saved_version: u64,
    /// Line ending on disk: changing it marks the file dirty.
    saved_line_ending: LineEnding,
    disk_hash: u64,
    /// One indent step ("\t" or spaces).
    indent: String,
    /// `.editorconfig` rules.
    rules: crate::editorconfig::Rules,
    /// The language server's folding ranges (header, last line; sorted) and line-end hints.
    lsp_folds: Vec<(usize, usize)>,
    /// `documentHighlight`: all usages of the symbol under the cursor (character ranges).
    symbol_hl: Vec<Range<usize>>,
    /// A block stage operation is in progress (so two writes to the same index don't collide).
    stage_busy: bool,
    lenses: std::collections::HashMap<usize, Vec<crate::lsp::Lens>>,
    /// Semantic token roles: line -> (start, length (UTF-16), role).
    sem: std::collections::HashMap<usize, Vec<(u32, u32, Role)>>,
    hl_task: Option<gpui::Task<()>>,
    inlays: std::collections::HashMap<usize, String>,
    /// Document version counter: when it changes, in-flight LSP requests don't apply their result (see `reset_lsp_data`).
    lsp_epoch: Arc<std::sync::atomic::AtomicU64>,
    /// Line where the debugger is stopped (0-based): yellow band.
    debug_line: Option<usize>,
    /// If the program is stopped: session and selected frame (the value is shown when the mouse is over an expression).
    debug_eval: Option<(Arc<crate::dap::Session>, i64)>,
    /// Folder settings of a file in an additional folder (the first folder's are already merged into the global settings).
    folder: Option<Arc<crate::folder_settings::FolderSettings>>,
    /// Extra settings, in the same order as `BREAKPOINT_MARKS`.
    bp_meta: Vec<BpMeta>,
    /// Locals of the stopped frame: line -> "name = value" (dim text at the end of the line).
    debug_values: std::collections::HashMap<usize, String>,
    /// Document line count when the last extra data arrived (line-based data is dropped when stale).
    lsp_lines: usize,
    /// Methods whose failure was written to Output (to avoid duplicate lines).
    lsp_reported: std::collections::HashSet<String>,
    /// `documentLink` results: (character range, target URI).
    links: Vec<(Range<usize>, String)>,
    extras_task: Option<gpui::Task<()>>,
    /// "Change Language Mode": `Some(extension)` pins the language for this file, `Some(None)` is plain text.
    lang_override: Option<Option<&'static str>>,
    autosave: Option<gpui::Task<()>>,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    /// IME composition (character range).
    marked: Option<Range<usize>>,
    /// Lines drawn in the last frame: for mouse and IME positions.
    visible: Vec<VisibleLine>,
    selecting: bool,
    /// Column selection (anchor line, anchor column, end line, end column); any other movement clears it.
    column: Option<(usize, usize, usize, usize)>,
    /// The mouse is dragging a column selection.
    column_drag: bool,
    saving: bool,
    conflict: bool,
    /// When `conflict` was raised: a Cmd+S right after it is not a decision to overwrite.
    conflict_at: Option<std::time::Instant>,
    /// File was deleted from disk (re-created on save).
    deleted_on_disk: bool,
    /// Identity of the buffer object: a new one is taken whenever the buffer is replaced (reload, revert), whose undo ids
    /// start over. With the version and a content hash it names one exact document state (see `doc_identity`).
    pub(crate) epoch: u64,
    /// Preview tab (italic; replaced by the next preview).
    pub preview: bool,
    /// Pinned tab (on the left, "close others" skips it).
    pub pinned: bool,
    error: Option<SharedString>,
    /// Find/replace bar (if open) and the matches found (character ranges).
    find: Option<(Entity<FindBar>, gpui::Subscription)>,
    find_re: Option<regex::bytes::Regex>,
    matches: Vec<Range<usize>>,
    /// Git: repository root, content at HEAD, and line markers (gutter).
    git_root: Option<String>,
    /// Inline blame: (line, text version, text) and the task fetching it.
    blame: Option<(usize, u64, SharedString)>,
    blame_task: Option<gpui::Task<()>>,
    /// Content at HEAD. Cloning a rope is O(1) and shares memory: for an unchanged
    /// file the base is a clone of the buffer itself (no copy is carried over from the agent).
    git_base: Option<buffer::Rope>,
    git_marks: Vec<Option<crate::git::LineMark>>,
    git_task: Option<gpui::Task<()>>,
    /// Language server (if any), language id, document version and latest diagnostics.
    lsp: Option<Arc<crate::lsp::Client>>,
    lsp_lang: &'static str,
    lsp_version: i64,
    diagnostics: Vec<crate::lsp::Diagnostic>,
    /// Workspace root of the language server; related locations under it are shown relative to it.
    lsp_root: Option<String>,
    /// JSON/YAML schema (completion and hover come from here when there is no language server); bound by the Workbench.
    schema: Option<Arc<SchemaBinding>>,
    /// Open color picker (if any).
    color_picker: Option<color_glue::ColorPicker>,
    /// Spell checker (bound by the Workbench); `None` when off.
    spell: Option<Arc<spell::Speller>>,
    /// Markdown fence state cache: (document version, per-line "inside fence").
    fence_cache: std::cell::RefCell<Option<(u64, Vec<bool>)>>,
    /// `.http` request lines cache: (document version, lines).
    http_lines: std::cell::RefCell<Option<(u64, Vec<usize>)>>,
    completion: Option<Completion>,
    completion_task: Option<gpui::Task<()>>,
    /// Signature help: (label, byte range of the active parameter).
    signature: Option<(String, Option<Range<usize>>)>,
    signature_task: Option<gpui::Task<()>>,
    hover: Option<Hover>,
    peek: Option<Peek>,
    /// Peek request counter: when the cursor moves, the result of an in-flight request is discarded.
    peek_seq: u64,
    hover_task: Option<gpui::Task<()>>,
    /// Origin of the editor surface within the window (to position popups).
    surface_origin: Point<Pixels>,
    /// First minimap line at the last paint (to convert a click to a line).
    minimap_start: usize,
    minimap_hidden: bool,
    /// Height of the editor surface (scrollbar computation) and where the dragged thumb
    /// was grabbed (relative to the top of the thumb).
    surface_h: Pixels,
    scrollbar_drag: Option<Pixels>,
    /// Horizontal scroll offset (long lines).
    hscroll: Pixels,
    /// When the same file is open in two groups, each group has its own view (cursors,
    /// scroll, focus, find bar, popups); the document (text, undo, syntax,
    /// LSP, git) is single. `slot`: the view currently in the fields; the others are in `parked`.
    slot: usize,
    parked: Vec<Option<ViewState>>,
    /// Drag started by a double click: the selection grows word by word (the starting word).
    word_drag: Option<Range<usize>>,
    /// Current state of the blinking cursor and the signature that resets the cycle (version, cursor, focus).
    cursor_on: bool,
    /// Merge conflicts and the document version they were computed for.
    conflicts: Vec<crate::conflict::Conflict>,
    conflicts_version: u64,
    conflicts_task: Option<gpui::Task<()>>,
    /// Word suggestions: (document version, cursor, list); so they aren't rescanned while typing.
    /// Selection when the find bar opened (candidate scope for find in selection).
    find_origin: Option<Range<usize>>,
    word_cache: Option<(u64, usize, std::sync::Arc<[crate::lsp::CompletionItem]>)>,
    conflicts_pending: u64,
    /// Line layout the popups are positioned on, and whether there are positioned items.
    popup_basis: ((usize, usize, Pixels), bool),
    /// Bracket colors: (document version, depth at the start of each line).
    bracket_cache: (u64, Vec<i32>),
    /// Sticky header lines visible in the last frame.
    sticky_lines: Vec<usize>,
    blink_sig: (u64, usize, bool),
    blink_task: Option<gpui::Task<()>>,
    /// In tests the window is never active; count it as active to exercise blinking.
    #[cfg(test)]
    blink_test_active: bool,
    /// While selecting with the mouse outside the text area: (direction and speed: screen lines/step), continuous scrolling.
    drag_outside: Option<isize>,
    autoscroll_task: Option<gpui::Task<()>>,
    /// Current stop in the active snippet session (navigated with Tab/Shift+Tab).
    snippet_stop: Option<usize>,
    /// Expand/shrink selection history: (previous selections, expanded selections).
    expand_stack: Vec<(Vec<Range<usize>>, Selections)>,
    /// Line <-> screen line (wrapping and folding; 1:1 if neither exists) and what was used
    /// to build it (not rebuilt unless they change).
    display: DisplayMap,
    display_key: Option<DisplayKey>,
    /// Hidden line ranges (folds) computed along with the mapping.
    hidden: Vec<Range<usize>>,
    /// Width of the text area at the last paint (the wrap column comes from here).
    text_width: Pixels,
    /// Settings at the last paint: (wrap enabled, tab width, width of one column).
    layout_params: (bool, usize, Pixels),
}

type Selections = Vec<Range<usize>>;

/// Target of a vertical move: `Ok` document edge, `Err((segment start, max column, column))`.
type RowTarget = Result<usize, (usize, usize, usize)>;

/// (version, length, wrap column, fold headers)
type DisplayKey = (u64, usize, Option<usize>, Vec<usize>);

/// Key of snippet stops in buffer marks: [start0, end0, start1, end1, ...].
const SNIPPET_MARKS: usize = 97;
/// Linked tag editing: [start of the typed name, start of its counterpart].
const LINK_MARKS: usize = 95;
/// View-specific marks have disjoint namespaces, separate from document marks.
const FIND_SCOPE_MARKS: usize = 0x1000;

/// Key of line bookmarks in buffer marks (tied to line starts, they shift).
const BOOKMARK_MARKS: usize = 98;

pub use crate::debug_model::BpMeta;

/// Debugger breakpoints (tied to line starts, they shift with edits).
const BREAKPOINT_MARKS: usize = 96;

/// Key of fold headers in buffer marks (+ view slot).
const FOLD_MARKS: usize = 0x2000;

/// Fields belonging to a view (swapped with the same-named fields in EditorView).
#[derive(Default)]
struct ViewState {
    focus_handle: Option<FocusHandle>,
    scroll: UniformListScrollHandle,
    marked: Option<Range<usize>>,
    visible: Vec<VisibleLine>,
    selecting: bool,
    find: Option<(Entity<FindBar>, gpui::Subscription)>,
    find_re: Option<regex::bytes::Regex>,
    find_origin: Option<Range<usize>>,
    matches: Vec<Range<usize>>,
    completion: Option<Completion>,
    completion_task: Option<gpui::Task<()>>,
    signature: Option<(String, Option<Range<usize>>)>,
    signature_task: Option<gpui::Task<()>>,
    hover: Option<Hover>,
    hover_task: Option<gpui::Task<()>>,
    surface_origin: Point<Pixels>,
    minimap_start: usize,
    minimap_hidden: bool,
    hscroll: Pixels,
    surface_h: Pixels,
    scrollbar_drag: Option<Pixels>,
    display: Option<DisplayMap>,
    display_key: Option<DisplayKey>,
    text_width: Pixels,
}

thread_local! {
    /// Line(s) last copied without a selection (the line is pasted as long as the clipboard carries this text).
    static LINE_CLIP: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// View being painted: listeners set up during painting switch to this view
    /// first when an event arrives (two groups paint the same editor with different views).
    static RENDER_SLOT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    #[cfg(test)]
    static SLOT_PAINTS: std::cell::RefCell<std::collections::HashMap<(gpui::EntityId, usize), usize>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

#[cfg(test)]
pub fn slot_paint_count(editor: &Entity<EditorView>, slot: usize) -> usize {
    SLOT_PAINTS.with(|counts| counts.borrow().get(&(editor.entity_id(), slot)).copied().unwrap_or(0))
}

#[cfg(test)]
pub fn slot_text_width(editor: &Entity<EditorView>, slot: usize, cx: &mut App) -> Pixels {
    editor.update(cx, |e, _| {
        let previous = e.slot;
        e.enter(slot);
        let width = e.text_width;
        e.enter(previous);
        width
    })
}

/// The view-aware version of `cx.listener`: runs in the view it was painted in.
trait SlotListener {
    fn on<E: ?Sized + 'static>(
        &self,
        f: impl Fn(&mut EditorView, &E, &mut Window, &mut Context<EditorView>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static;
}

impl SlotListener for Context<'_, EditorView> {
    fn on<E: ?Sized + 'static>(
        &self,
        f: impl Fn(&mut EditorView, &E, &mut Window, &mut Context<EditorView>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let slot = RENDER_SLOT.with(|s| s.get());
        self.listener(move |this, e, window, cx| {
            let prev = this.slot;
            this.enter(slot);
            f(this, e, window, cx);
            this.enter(prev);
        })
    }
}

#[path = "editor_slots.rs"]
mod slots;
pub use slots::slot_view;

impl Drop for EditorView {
    fn drop(&mut self) {
        if let Some(c) = &self.lsp {
            c.notify("textDocument/didClose", serde_json::json!({ "textDocument": { "uri": crate::lsp::path_to_uri(&self.path) } }));
        }
    }
}

impl EventEmitter<EditorEvent> for EditorView {}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn detect_indent(buffer: &Buffer) -> &'static str {
    let (mut tabs, mut two, mut four) = (0, 0, 0);
    for i in 0..buffer.len_lines().min(200) {
        let line = buffer.line(i);
        let spaces = line.chars().take_while(|c| *c == ' ').count();
        if line.starts_with('\t') {
            tabs += 1;
        } else if spaces > 0 && line.len() > spaces {
            if spaces % 4 == 0 {
                four += 1
            } else if spaces % 2 == 0 {
                two += 1
            }
        }
    }
    if tabs > two + four {
        "\t"
    } else if two > 0 && two * 2 >= four {
        "  "
    } else {
        "    "
    }
}

/// "Change Language Mode" options: (display name, extension). The highlighter chooses by extension.
pub const LANGUAGES: [(&str, &str); 25] = [
    ("Rust", "rs"),
    ("JavaScript", "js"),
    ("JavaScript JSX", "jsx"),
    ("TypeScript", "ts"),
    ("TypeScript JSX", "tsx"),
    ("Python", "py"),
    ("Go", "go"),
    ("JSON", "json"),
    ("Shell Script", "sh"),
    ("CSS", "css"),
    ("HTML", "html"),
    ("TOML", "toml"),
    ("YAML", "yaml"),
    ("Markdown", "md"),
    ("C", "c"),
    ("C++", "cpp"),
    ("Java", "java"),
    ("C#", "cs"),
    ("Ruby", "rb"),
    ("PHP", "php"),
    ("Swift", "swift"),
    ("Kotlin", "kt"),
    ("SQL", "sql"),
    ("Lua", "lua"),
    ("Dockerfile", "dockerfile"),
];

/// Tab width of the GPUI line painter (columns).
const TAB_COLS: usize = 4;

/// Indent level of a line (`None` for an empty line).
fn indent_level(line: &str, unit: usize) -> Option<usize> {
    if line.trim().is_empty() {
        return None;
    }
    let cols: usize = line.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { unit } else { 1 }).sum();
    Some(cols / unit.max(1))
}

/// Kinds of highlighted ranges in a line.
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    /// Other occurrences of the word under the cursor.
    Word,
    /// Matching bracket.
    Bracket,
    /// Find: the matches and the current match.
    Find,
    FindCurrent,
    /// Find-in-selection scope.
    FindScope,
}

/// Whole-word occurrences of a word in a line (byte ranges).
fn word_occurrences(text: &str, word: &str) -> Vec<Range<usize>> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word)
        .filter(|(i, _)| {
            let before = text[..*i].chars().next_back().is_none_or(|c| !is_word(c));
            let after = text[i + word.len()..].chars().next().is_none_or(|c| !is_word(c));
            before && after
        })
        .map(|(i, _)| i..i + word.len())
        .collect()
}

/// The setting if present, otherwise guessed from the file.
fn resolve_indent(buffer: &Buffer, insert_spaces: Option<bool>, tab_size: Option<usize>) -> String {
    let detected = detect_indent(buffer);
    match (insert_spaces, tab_size) {
        (Some(false), _) => "\t".into(),
        (Some(true), Some(n)) => " ".repeat(n),
        (Some(true), None) if detected == "\t" => "    ".into(),
        (None, Some(n)) if detected != "\t" => " ".repeat(n),
        _ => detected.into(),
    }
}

/// Layer the `.editorconfig` indent over the setting and file guess (if the `editor.editorconfig` setting is on).
fn indent_with_rules(
    buffer: &Buffer,
    rules: &crate::editorconfig::Rules,
    editorconfig: bool,
    (insert_spaces, tab_size): (Option<bool>, Option<usize>),
) -> String {
    if editorconfig && let Some(unit) = rules.indent_unit(detect_indent(buffer)) {
        return unit;
    }
    resolve_indent(buffer, insert_spaces, tab_size)
}

/// Expression at column `col`: an `a.b.c` or `p->x` chain; `None` on a number or whitespace.
pub(crate) fn expression_in_line(chars: &[char], col: usize) -> Option<String> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let on = |i: usize| chars.get(i).is_some_and(|c| word(*c) || *c == '.');
    let at = if on(col) {
        col
    } else if col > 0 && on(col - 1) {
        col - 1
    } else {
        return None;
    };
    let (mut a, mut b) = (at, at + 1);
    loop {
        if a > 0 && on(a - 1) {
            a -= 1;
        } else if a >= 2 && chars[a - 1] == '>' && chars[a - 2] == '-' && a >= 3 && word(chars[a - 3]) {
            a -= 2;
        } else {
            break;
        }
    }
    loop {
        if on(b) {
            b += 1;
        } else if chars.get(b) == Some(&'-') && chars.get(b + 1) == Some(&'>') && chars.get(b + 2).is_some_and(|c| word(*c)) {
            b += 2;
        } else {
            break;
        }
    }
    let expr: String = chars[a..b].iter().collect::<String>().trim_matches('.').to_string();
    (!expr.is_empty() && !expr.chars().next().is_some_and(|c| c.is_ascii_digit())).then_some(expr)
}

fn byte_of(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map_or(s.len(), |(b, _)| b)
}

impl EditorView {
    pub fn new(host: Arc<dyn Host>, path: String, loaded: Loaded, cx: &mut Context<Self>) -> Self {
        let Loaded { mut buffer, hash, syntax, encoding, rules } = loaded;
        if syntax.is_some() {
            buffer.track_edits();
        }
        let settings = cx.global::<Settings>();
        let indent = indent_with_rules(&buffer, &rules, settings.editorconfig, (settings.insert_spaces, settings.tab_size));
        let conflicts = crate::conflict::find(buffer.rope());
        let conflicts_version = buffer.version();
        EditorView {
            host,
            name: SharedString::from(ui::file_name(&path).to_string()),
            path,
            saved_version: buffer.version(),
            saved_line_ending: buffer.line_ending(),
            buffer,
            syntax,
            encoding,
            disk_hash: hash,
            indent,
            rules,
            lsp_folds: Vec::new(),
            symbol_hl: Vec::new(),
            stage_busy: false,
            lenses: Default::default(),
            sem: Default::default(),
            hl_task: None,
            inlays: Default::default(),
            links: Vec::new(),
            lsp_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            debug_line: None,
            debug_eval: None,
            folder: None,
            bp_meta: Vec::new(),
            debug_values: Default::default(),
            lsp_lines: 0,
            lsp_reported: Default::default(),
            extras_task: None,
            lang_override: None,
            autosave: None,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            marked: None,
            visible: Vec::new(),
            selecting: false,
            column: None,
            column_drag: false,
            saving: false,
            conflict: false,
            conflict_at: None,
            deleted_on_disk: false,
            epoch: next_epoch(),
            preview: false,
            pinned: false,
            error: None,
            find: None,
            find_re: None,
            matches: Vec::new(),
            git_root: None,
            blame: None,
            blame_task: None,
            git_base: None,
            git_marks: Vec::new(),
            git_task: None,
            lsp: None,
            lsp_lang: "",
            lsp_version: 0,
            diagnostics: Vec::new(),
            lsp_root: None,
            completion: None,
            completion_task: None,
            signature: None,
            signature_task: None,
            hover: None,
            peek: None,
            peek_seq: 0,
            hover_task: None,
            schema: None,
            color_picker: None,
            spell: None,
            fence_cache: Default::default(),
            http_lines: Default::default(),
            surface_origin: Point::default(),
            minimap_start: 0,
            minimap_hidden: false,
            surface_h: px(0.),
            scrollbar_drag: None,
            hscroll: px(0.),
            slot: 0,
            parked: vec![None, None],
            snippet_stop: None,
            word_drag: None,
            cursor_on: true,
            conflicts,
            conflicts_version,
            conflicts_task: None,
            word_cache: None,
            find_origin: None,
            conflicts_pending: u64::MAX,
            popup_basis: ((usize::MAX, 0, px(0.)), false),
            bracket_cache: (u64::MAX, Vec::new()),
            sticky_lines: Vec::new(),
            blink_sig: (u64::MAX, 0, false),
            blink_task: None,
            #[cfg(test)]
            blink_test_active: false,
            drag_outside: None,
            autoscroll_task: None,
            expand_stack: Vec::new(),
            display: DisplayMap::identity(0),
            display_key: None,
            hidden: Vec::new(),
            text_width: px(0.),
            layout_params: (false, 4, px(8.)),
        }
    }

    /// Swap the view's fields with the current ones.
    fn swap_view(&mut self, v: &mut ViewState) {
        use std::mem::swap;
        let mut focus = v.focus_handle.take().expect("parked view keeps its focus handle");
        swap(&mut self.focus_handle, &mut focus);
        v.focus_handle = Some(focus);
        swap(&mut self.scroll, &mut v.scroll);
        swap(&mut self.marked, &mut v.marked);
        swap(&mut self.visible, &mut v.visible);
        swap(&mut self.selecting, &mut v.selecting);
        swap(&mut self.find, &mut v.find);
        swap(&mut self.find_origin, &mut v.find_origin);
        swap(&mut self.find_re, &mut v.find_re);
        swap(&mut self.matches, &mut v.matches);
        swap(&mut self.completion, &mut v.completion);
        swap(&mut self.completion_task, &mut v.completion_task);
        swap(&mut self.signature, &mut v.signature);
        swap(&mut self.signature_task, &mut v.signature_task);
        swap(&mut self.hover, &mut v.hover);
        swap(&mut self.hover_task, &mut v.hover_task);
        swap(&mut self.surface_origin, &mut v.surface_origin);
        swap(&mut self.minimap_start, &mut v.minimap_start);
        swap(&mut self.minimap_hidden, &mut v.minimap_hidden);
        swap(&mut self.surface_h, &mut v.surface_h);
        swap(&mut self.scrollbar_drag, &mut v.scrollbar_drag);
        swap(&mut self.hscroll, &mut v.hscroll);
        let mut display = v.display.take().unwrap_or_else(|| DisplayMap::identity(self.buffer.len_lines()));
        swap(&mut self.display, &mut display);
        v.display = Some(display);
        swap(&mut self.display_key, &mut v.display_key);
        swap(&mut self.text_width, &mut v.text_width);
    }

    /// Switch to the `slot` view (does nothing if it isn't open).
    fn enter(&mut self, slot: usize) {
        if slot == self.slot {
            return;
        }
        let Some(mut v) = self.parked.get_mut(slot).and_then(Option::take) else { return };
        self.swap_view(&mut v);
        self.buffer.switch_selections(self.slot, slot);
        self.parked[self.slot] = Some(v);
        self.slot = slot;
        RENDER_SLOT.with(|s| s.set(slot));
    }

    // --- Line wrapping and folding ---

    /// Platform text input goes to the focused view.
    fn enter_focused(&mut self, window: &Window) {
        if let Some(slot) =
            (0..self.parked.len()).find(|&s| self.parked[s].as_ref().is_some_and(|v| v.focus_handle.as_ref().is_some_and(|f| f.is_focused(window))))
        {
            self.enter(slot);
        }
    }

    /// Workbench: this view belongs to the focused group (calls from outside go to it).
    pub fn set_home(&mut self, slot: usize) {
        self.enter(slot);
    }

    pub fn has_slot(&self, slot: usize) -> bool {
        self.slot == slot || self.parked.get(slot).is_some_and(Option::is_some)
    }

    /// Opened in another group: that group's view (cursor and scroll same as the current one).
    pub fn open_slot(&mut self, slot: usize, cx: &mut Context<Self>) {
        if self.has_slot(slot) || slot >= crate::editor_layout::MAX_GROUPS {
            return;
        }
        self.parked.resize_with(self.parked.len().max(slot + 1), || None);
        let line = self.buffer.char_to_line(self.buffer.cursor());
        let scroll = UniformListScrollHandle::new();
        scroll.scroll_to_item(line, ScrollStrategy::Center);
        // Folds are per view: the new view starts with the current view's folds.
        let folds = self.buffer.marks(FOLD_MARKS + self.slot).to_vec();
        self.buffer.set_marks(FOLD_MARKS + slot, folds);
        self.parked[slot] =
            Some(ViewState { focus_handle: Some(cx.focus_handle()), scroll, hscroll: self.hscroll, text_width: self.text_width, ..Default::default() });
    }

    /// A group closed this tab; the editor lives on in the other group.
    pub fn close_slot(&mut self, slot: usize) {
        if !self.has_slot(slot) {
            return;
        }
        self.buffer.set_marks(FIND_SCOPE_MARKS + slot, Vec::new());
        if self.slot == slot {
            let Some(other) = (0..self.parked.len()).find(|&s| s != slot && self.parked[s].is_some()) else { return };
            self.enter(other);
        }
        self.parked[slot] = None;
        self.buffer.forget_selections(slot);
        self.buffer.set_marks(FOLD_MARKS + slot, Vec::new());
    }

    /// Number of lines each view painted in its last frame (in slot order).
    #[cfg(test)]
    pub fn painted_lines(&self) -> Vec<usize> {
        (0..self.parked.len())
            .filter(|&s| self.has_slot(s))
            .map(|s| if s == self.slot { self.visible.len() } else { self.parked[s].as_ref().map_or(0, |v| v.visible.len()) })
            .collect()
    }

    /// (screen line, buffer line) counts.
    #[cfg(test)]
    pub fn row_counts(&self) -> (usize, usize) {
        (self.display.row_count(), self.buffer.len_lines())
    }

    /// Whether the widest line segment fit in the text area at the last paint.
    #[cfg(test)]
    pub fn rows_fit(&self) -> bool {
        self.visible.iter().all(|v| v.layout.width <= v.bounds.size.width)
    }

    #[cfg(test)]
    pub fn toggle_fold_for_test(&mut self, line: usize, cx: &mut Context<Self>) {
        self.toggle_fold_line(line);
        cx.notify();
    }

    /// A point in the scrollbar strip at `frac` of the height, downward (tests).
    #[cfg(test)]
    pub fn scrollbar_point(&self, frac: f32) -> Option<Point<Pixels>> {
        let right = self.visible.first()?.bounds.right();
        Some(point(right - px(6.), self.surface_origin.y + self.surface_h * frac))
    }

    /// First visible screen line (tests).
    #[cfg(test)]
    pub fn first_visible_line(&self) -> Option<usize> {
        self.visible.first().map(|v| v.line)
    }

    #[cfg(test)]
    pub fn find_open(&self) -> bool {
        self.find.is_some()
    }

    /// In how many views it is open.
    #[cfg(test)]
    pub fn view_count(&self) -> usize {
        1 + self.parked.iter().filter(|v| v.is_some()).count()
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.version() != self.saved_version || self.buffer.line_ending() != self.saved_line_ending
    }

    /// From the status bar: line ending (applied on save).
    pub fn set_line_ending(&mut self, crlf: bool, cx: &mut Context<Self>) {
        self.buffer.set_line_ending(if crlf { LineEnding::Crlf } else { LineEnding::Lf });
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    /// Pin the language for this file (`None`: plain text). Preserved even if the file is reloaded.
    pub fn set_language(&mut self, ext: Option<&'static str>, cx: &mut Context<Self>) {
        self.lang_override = Some(ext);
        self.apply_language_override();
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn apply_language_override(&mut self) {
        let Some(ext) = self.lang_override else { return };
        self.syntax = ext.and_then(|e| syntax::language_for_path(&format!("x.{e}"))).and_then(|lang| Syntax::new(lang, self.buffer.rope()));
        if self.syntax.is_some() {
            self.buffer.track_edits();
        }
        // The new tree was built from the current text: stale queued edits must not be applied to it.
        self.buffer.take_edits();
        self.display_key = None;
    }

    /// Source-scoped bool setting: the folder's if it gave one, otherwise the global value.
    fn scoped(&self, pick: impl Fn(&crate::folder_settings::FolderSettings) -> Option<bool>, global: bool) -> bool {
        self.folder.as_deref().and_then(pick).unwrap_or(global)
    }

    /// 1-based (line, column).
    pub fn cursor_position(&self) -> (usize, usize) {
        let (l, col) = self.buffer.cursor_line_col();
        (l + 1, col + 1)
    }

    pub fn line_ending(&self) -> &'static str {
        match self.buffer.line_ending() {
            LineEnding::Lf => "LF",
            LineEnding::Crlf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    /// Tab moved to the foreground/background: a background tab doesn't keep the syntax tree.
    pub fn set_active(&mut self, active: bool) {
        let Some(syntax) = &mut self.syntax else { return };
        if active {
            self.buffer.take_edits(); // edits accumulated while discarded (should not happen) are invalid
            syntax.ensure_loaded(self.buffer.rope());
        } else {
            syntax.unload();
        }
    }

    /// Number of selected characters (`None` if there is no selection).
    pub fn selection_len(&self) -> Option<usize> {
        self.buffer.selection().map(|r| r.len())
    }

    /// For the accessibility tree (a11y.rs): (anchor, cursor) character indices, line count,
    /// first character index of each line and line texts (`from..to`, end exclusive).
    pub fn a11y_view(&self, from: usize, to: usize) -> ((usize, usize), usize, Vec<(usize, String)>) {
        let head = self.buffer.cursor();
        let anchor = match self.buffer.selection() {
            Some(r) if self.buffer.selection_reversed() => r.end,
            Some(r) => r.start,
            None => head,
        };
        let n = self.buffer.len_lines();
        let lines = (from.min(n)..to.min(n)).map(|l| (self.buffer.line_to_char(l), self.buffer.line(l))).collect();
        ((anchor, head), n, lines)
    }

    /// Accessibility action: move the selection to the (anchor, cursor) character indices.
    pub fn a11y_set_selection(&mut self, anchor: usize, head: usize, cx: &mut Context<Self>) {
        let n = self.buffer.len_chars();
        let (anchor, head) = (anchor.min(n), head.min(n));
        if anchor == head {
            self.buffer.set_cursor(head, false);
        } else {
            self.buffer.set_selection(anchor.min(head)..anchor.max(head), head < anchor);
        }
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    /// For the accessibility tree (a11y.rs): the shown suggestions (label, detail), at most `limit`, and the selected one.
    pub fn a11y_completion(&self, limit: usize) -> Option<(Vec<(String, Option<String>)>, usize)> {
        let c = self.completion.as_ref()?;
        let rows = c.shown.iter().take(limit).map(|&i| (c.items[i].label.clone(), c.items[i].detail.clone())).collect();
        Some((rows, c.selected))
    }

    /// Accessibility action: select suggestion `ix`, and insert it if `accept`.
    pub fn a11y_choose_completion(&mut self, ix: usize, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = &mut self.completion else { return };
        if ix >= c.shown.len() {
            return;
        }
        c.selected = ix;
        if accept {
            self.completion_accept(&CompletionAccept, window, cx);
        }
        cx.notify();
    }

    /// File was renamed/moved.
    pub fn set_path(&mut self, path: String, cx: &mut Context<Self>) {
        self.name = SharedString::from(ui::file_name(&path).to_string());
        self.path = path;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    pub fn line_count(&self) -> usize {
        self.buffer.len_lines()
    }

    /// Search result: select the match on the line, center it, take focus.
    pub fn reveal(&mut self, line: usize, column: usize, len: usize, window: &mut Window, cx: &mut Context<Self>) {
        let ix = line.saturating_sub(1).min(self.buffer.len_lines().saturating_sub(1));
        let start = self.buffer.line_to_char(ix) + column.min(self.buffer.line_len_chars(ix));
        let end = (start + len).min(self.buffer.line_to_char(ix) + self.buffer.line_len_chars(ix));
        self.buffer.set_selection(start..end, false);
        self.unfold_at_cursors();
        let row = self.row_of_char(start);
        self.scroll.scroll_to_item_strict(row, ScrollStrategy::Center);
        window.focus(&self.focus_handle);
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    pub fn selected_text(&self) -> Option<String> {
        self.buffer.selected_text()
    }

    /// Go to a language server position (0-based line, UTF-16 column).
    pub fn reveal_lsp(&mut self, line: u32, col: u32, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.buffer.from_lsp_position(line, col);
        self.buffer.set_cursor(at, false);
        self.unfold_at_cursors();
        let row = self.row_of_char(at);
        self.scroll.scroll_to_item_strict(row, ScrollStrategy::Center);
        window.focus(&self.focus_handle);
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    /// Go to a 1-based line, center the line, take focus.
    pub fn go_to_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let ix = line.saturating_sub(1).min(self.buffer.len_lines().saturating_sub(1));
        let at = self.buffer.line_to_char(ix);
        let indent = self.buffer.line(ix).chars().take_while(|c| c.is_whitespace()).count();
        self.buffer.set_cursor(at + indent, false);
        self.unfold_at_cursors();
        let row = self.row_of_char(at);
        self.scroll.scroll_to_item_strict(row, ScrollStrategy::Center);
        window.focus(&self.focus_handle);
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    pub fn language(&self) -> &'static str {
        self.syntax.as_ref().map_or("Plain Text", |s| s.language_name())
    }

    /// The language name for display (language names are proper names; only "Plain Text" is translated).
    pub fn language_label(&self) -> &'static str {
        self.syntax.as_ref().map_or(crate::i18n::t("Plain Text"), |s| s.language_name())
    }

    /// Apply the buffer's edits to the syntax tree, reparse the changed part.
    fn sync_syntax(&mut self) {
        self.sync_lsp();
        let Some(syntax) = &mut self.syntax else { return };
        let edits = self.buffer.take_edits();
        if edits.is_empty() {
            return;
        }
        for e in &edits {
            syntax.edit(&syntax::Edit {
                start_byte: e.start_byte,
                old_end_byte: e.old_end_byte,
                new_end_byte: e.new_end_byte,
                start: e.start_point,
                old_end: e.old_end_point,
                new_end: e.new_end_point,
            });
        }
        syntax.reparse(self.buffer.rope());
    }

    pub fn cursor_count(&self) -> usize {
        self.buffer.cursor_count()
    }

    // --- Find / replace ---
}

/// Line comment marker by file type (`None` if there is none).
fn comment_token(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if matches!(name, "Makefile" | "GNUmakefile" | "makefile" | "Dockerfile" | "Containerfile" | ".gitignore" | ".env" | "Gemfile" | "Rakefile")
        || name.starts_with("Dockerfile.")
    {
        return Some("#");
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    Some(match ext.as_str() {
        "rs" | "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "go" | "java" | "kt" | "kts" | "swift"
        | "cs" | "scala" | "dart" | "zig" | "proto" | "jsonc" | "scss" | "less" | "php" | "groovy" => "//",
        "py" | "pyi" | "sh" | "bash" | "zsh" | "fish" | "rb" | "rake" | "gemspec" | "pl" | "r" | "toml" | "yaml" | "yml" | "conf" | "ini" | "cfg"
        | "properties" | "nix" | "ex" | "exs" | "tf" | "cmake" | "ps1" | "mk" | "dockerfile" => "#",
        "sql" | "lua" | "hs" | "elm" | "ada" => "--",
        "vim" => "\"",
        "el" | "lisp" | "clj" | "scm" | "asm" | "s" => ";",
        "erl" | "hrl" | "tex" | "m" => "%",
        // A language an installed extension adds, for files the table does not know.
        _ => return crate::ext::language_for(path).and_then(|l| l.line_comment),
    })
}

/// Block comment for languages without a line comment (each line wrapped separately, like VS Code).
fn block_comment(path: &str) -> Option<(&'static str, &'static str)> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    Some(match ext.as_str() {
        "css" => ("/*", "*/"),
        "html" | "htm" | "xml" | "svg" | "md" | "markdown" | "vue" | "xhtml" | "plist" => ("<!--", "-->"),
        _ => return crate::ext::language_for(path).filter(|l| l.line_comment.is_none()).and_then(|l| l.block_comment),
    })
}

/// Block comment markers (also for languages with a line comment): for Shift+Alt+A.
fn block_delims(path: &str) -> Option<(&'static str, &'static str)> {
    if let Some(d) = block_comment(path) {
        return Some(d);
    }
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    Some(match ext.as_str() {
        "rs" | "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "go" | "java" | "kt" | "kts" | "swift"
        | "cs" | "scala" | "dart" | "zig" | "proto" | "scss" | "less" | "php" | "groovy" | "sql" => ("/*", "*/"),
        "lua" => ("--[[", "]]"),
        "hs" => ("{-", "-}"),
        "py" | "pyi" => ("\"\"\"", "\"\"\""),
        "rb" => ("=begin", "=end"),
        _ => return crate::ext::language_for(path).and_then(|l| l.block_comment),
    })
}

/// Minimap width (px).
const MINIMAP_W: f32 = 90.;

fn role_color(role: Option<Role>, p: &Palette) -> u32 {
    match role {
        None => p.text,
        Some(Role::Keyword) => p.syntax_keyword,
        Some(Role::String) => p.syntax_string,
        Some(Role::Number) => p.syntax_number,
        Some(Role::Function) => p.syntax_function,
        Some(Role::Type) => p.syntax_type,
        Some(Role::Comment) => p.syntax_comment,
        Some(Role::Tag) => p.syntax_tag,
        Some(Role::Punct) => p.syntax_punct,
    }
}

/// LSP CompletionItemKind -> (short marker, color).
fn kind_glyph(kind: u8, p: &Palette) -> (&'static str, u32) {
    match kind {
        2..=4 => ("ƒ", p.syntax_function),
        5 | 10 => ("▪", p.syntax_keyword),
        6 | 21 => ("𝑥", p.syntax_number),
        7 | 22 | 25 => ("◇", p.syntax_type),
        8 => ("◈", p.syntax_type),
        9 => ("{}", p.syntax_tag),
        13 | 20 => ("≡", p.syntax_number),
        14 => ("k", p.syntax_keyword),
        1 => ("abc", p.text_muted),
        15 => ("⧉", p.text_muted),
        17 | 19 => ("▤", p.text_muted),
        _ => ("·", p.text_muted),
    }
}

pub fn button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    p: &Palette,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (bg, fg) = if primary { (p.accent, p.on_accent) } else { (p.surface_hover, p.text) };
    div()
        .id(id)
        .h(px(24.))
        .px(px(theme::SPACE[4]))
        .flex()
        .items_center()
        .rounded(px(theme::radius::SM))
        .bg(c(bg))
        .text_color(c(fg))
        .cursor_pointer()
        .hover(|d| d.opacity(0.9))
        .child(label)
        .on_click(on_click)
}

#[cfg(test)]
pub(crate) fn title_case(s: &str) -> String {
    crate::textcase::title(s, crate::textcase::CaseLocale::Invariant)
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        self.enter_focused(window);
        let start = self.buffer.utf16_to_char(range_utf16.start);
        let end = self.buffer.utf16_to_char(range_utf16.end);
        actual_range.replace(self.buffer.char_to_utf16(start)..self.buffer.char_to_utf16(end));
        Some(self.buffer.text_range(start..end))
    }

    fn selected_text_range(&mut self, _: bool, window: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        self.enter_focused(window);
        let r = self.buffer.selection_or_cursor();
        Some(UTF16Selection { range: self.buffer.char_to_utf16(r.start)..self.buffer.char_to_utf16(r.end), reversed: self.buffer.selection_reversed() })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| self.buffer.char_to_utf16(m.start)..self.buffer.char_to_utf16(m.end))
    }

    fn unmark_text(&mut self, window: &mut Window, _: &mut Context<Self>) {
        self.enter_focused(window);
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.enter_focused(window);
        // Composition ending (dead key "´" + "e" -> "é"): at every cursor, a single undo step.
        if range_utf16.is_none() && self.marked.is_some() {
            let ranges = self.compose_ranges();
            self.marked = None;
            self.buffer.compose(ranges, new_text);
            self.buffer.end_compose();
            self.after_edit(cx);
            self.after_typed(new_text, cx);
            return;
        }
        let range = range_utf16.map(|r| self.buffer.utf16_to_char(r.start)..self.buffer.utf16_to_char(r.end)).or(self.marked.take());
        match range {
            Some(range) => {
                let typing = new_text.chars().count() == 1 && range.is_empty();
                self.buffer.replace(range, new_text, typing);
            }
            // Plain typing: at every cursor (IME composition only at the primary one).
            None => {
                // Single character: auto-close brackets/quotes (per setting).
                let mut chars = new_text.chars();
                match (chars.next(), chars.next()) {
                    (Some(ch), None) => {
                        let auto = cx.global::<Settings>().auto_close_brackets;
                        self.buffer.type_char(ch, auto);
                    }
                    _ => self.buffer.insert(new_text),
                }
                self.marked = None;
                let open = self.completion.take();
                let sig = self.signature.take();
                self.after_edit(cx);
                self.completion = open;
                self.signature = sig;
                self.after_typed(new_text, cx);
                return;
            }
        }
        self.marked = None;
        self.after_edit(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.enter_focused(window);
        let start = match range_utf16 {
            // Open range (IMEs with a single cursor): the old path.
            Some(r) => {
                let range = self.buffer.utf16_to_char(r.start)..self.buffer.utf16_to_char(r.end);
                let start = range.start;
                self.buffer.compose(vec![range], new_text);
                start
            }
            // Dead key/IME: the composition continues at every cursor.
            None => {
                let ranges = self.compose_ranges();
                self.buffer.compose(ranges, new_text);
                // Cursor at the end of the inserted text.
                self.buffer.cursor().saturating_sub(new_text.chars().count())
            }
        };
        let len = new_text.chars().count();
        self.marked = (len > 0).then(|| start..start + len);
        if len == 0 {
            self.buffer.end_compose();
        }
        if let Some(sel) = new_selected_range_utf16.filter(|_| self.buffer.cursor_count() == 1) {
            // Range in UTF-16, relative to the start of the inserted text.
            let to_chars = |cu: usize| {
                let mut units = 0;
                new_text
                    .chars()
                    .take_while(|ch| {
                        units += ch.len_utf16();
                        units <= cu
                    })
                    .count()
            };
            self.buffer.set_selection(start + to_chars(sel.start)..start + to_chars(sel.end), false);
        }
        self.sync_syntax();
        let row = self.row_of_char(self.buffer.cursor());
        self.scroll.scroll_to_item(row, ScrollStrategy::Top);
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.enter_focused(window);
        let start = self.buffer.utf16_to_char(range_utf16.start);
        let (v, x) = self.screen_pos(start)?;
        Some(Bounds::new(point(x, v.bounds.top()), size(px(2.), v.bounds.size.height)))
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, window: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        self.enter_focused(window);
        self.index_for_position(point).map(|ix| self.buffer.char_to_utf16(ix))
    }
}

/// Workspace edit of the action run on save: the first action of the requested kind,
/// completed with `codeAction/resolve` if it has no edit. Actions that run via a command are skipped
/// (we don't want side effects outside the file during a save).
fn source_action_edit(client: &crate::lsp::Client, params: serde_json::Value, kind: &str) -> Option<serde_json::Value> {
    let timeout = std::time::Duration::from_secs(2);
    let actions = crate::lsp::parse_code_actions(&client.request("textDocument/codeAction", params, timeout).ok()?);
    let action = actions.into_iter().find(|a| a.kind == kind || a.kind.starts_with(&format!("{kind}.")))?;
    let mut raw = action.raw;
    if raw["edit"].is_null() && !raw["data"].is_null() {
        raw = client.request("codeAction/resolve", raw.clone(), timeout).unwrap_or(raw);
    }
    (!raw["edit"].is_null()).then(|| raw["edit"].clone())
}
