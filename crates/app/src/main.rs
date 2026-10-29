//! codeyard: lightweight, native code editor. Local + SSH.
//!
//!     codeyard [FOLDER | FILE] [--connect HOST] [--theme ID] [--terminal] [--search TEXT] [--panel explorer|search|scm|debug|testing|remote|welcome|settings] [--no-extensions] [--extension-dev FOLDER] [--install-extension FILE.cyx|ID[@VERSION]]

// Normal Windows launches are GUI applications in every profile. Test harnesses keep their console.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]
// Lints that fight the shape of this code on purpose: UI event plumbing passes (window, cx, ids…) and
// returns tuples that are not worth a named type each. Everything else stays at clippy's default.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]
// Tests build one-range inputs on purpose (`vec![0..5]` as "a single range").
#![cfg_attr(test, allow(clippy::single_range_in_vec_init))]

mod access;
mod bundled_agents;
mod cargo_args;
mod cargo_project;
mod color;
mod commands;
// --- compare ---
mod compare_folders;
mod compare_logic;
#[cfg(test)]
mod compare_tests;
mod conflict;
mod container_engine;
mod container_history;
mod container_logs;
mod container_monitor;
mod crash;
mod dap;
mod debug_model;
// --- data ---
#[cfg(test)]
mod a11y_ui_tests;
mod about_page;
mod datalens;
mod datalens_decode;
mod datalens_query;
#[cfg(test)]
mod datalens_tests;
#[cfg(test)]
mod datalens_ui_tests;
mod debug_config;
mod diff;
mod display;
mod docker;
#[cfg(test)]
mod docker_tests;
#[cfg(test)]
mod docs_tests;
mod editor;
mod editor_decor;
mod editor_layout;
mod editor_overlay;
#[cfg(test)]
mod editor_title_tests;
mod editorconfig;
mod emmet;
mod encoding;
#[cfg(test)]
mod error_ui_tests;
#[cfg(test)]
mod explorer_bulk_tests;
mod ext;
mod ext_registry;
#[cfg(test)]
mod ext_registry_tests;
#[cfg(test)]
mod ext_tests;
mod ext_updates;
#[cfg(test)]
mod ext_updates_tests;
#[cfg(test)]
mod ext_view_tests;
#[cfg(test)]
mod features_tests;
mod file_batch;
mod find;
mod folder_settings;
mod fsutil;
#[cfg(test)]
mod fuzz_parsers_tests;
mod fuzzy;
mod git;
#[cfg(test)]
mod git_sync_tests;
mod github;
mod globs;
mod graph;
#[cfg(test)]
mod hardening_tests;
mod hexdump;
mod history;
mod http_client;
mod i18n;
#[cfg(test)]
mod i18n_editor_tests;
#[cfg(test)]
mod i18n_lint_tests;
#[cfg(test)]
mod i18n_remote_tests;
#[cfg(test)]
mod i18n_tests;
mod icon_theme;
mod image_size;
mod input;
mod jsonfmt;
mod language_service_install;
mod language_services;
mod language_services_page;
#[cfg(all(test, unix))]
mod language_services_real_tests;
mod large;
mod linked_tag;
mod links;
mod lsp;
#[cfg(test)]
mod manage_menu_tests;
mod markdown;
#[cfg(test)]
mod market_tests;
mod menu;
mod menus;
mod merge;
mod outline;
mod perf;
mod picker;
mod port_forwards;
mod ports;
mod problems;
#[cfg(test)]
mod product_quality_tests;
#[cfg(test)]
mod qa_debug_rust_tests;
#[cfg(test)]
mod qa_debug_tests;
#[cfg(test)]
mod qa_editor_tests;
#[cfg(test)]
mod qa_integration_tests;
#[cfg(test)]
mod qa_lsp_diagnostics_tests;
#[cfg(test)]
mod qa_lsp_rust_commands_tests;
#[cfg(test)]
mod qa_lsp_settings_tests;
#[cfg(test)]
mod qa_workbench_tests;
#[cfg(test)]
mod qa_workspace_file_tests;
#[cfg(test)]
mod reliability_data_tests;
mod replace_apply;
mod replace_plan;
mod resource_monitor;
mod resources;
#[cfg(test)]
mod rust_hardening_tests;
mod scm;
mod search;
mod search_doc;
mod search_editor;
#[cfg(test)]
mod search_editor_tests;
mod sections;
mod settings;
#[cfg(test)]
mod settings_docs;
mod settings_page;
mod shortcuts_page;
mod snippets;
#[cfg(test)]
mod text_scale_tests;
#[cfg(test)]
mod ui_scale_tests;
mod web_preview;
mod web_preview_config;
// --- structural ---
mod shell_quote;
#[cfg(test)]
mod ssh_error_tests;
mod structural;
#[cfg(test)]
mod structural_tests;
mod sysopen;
mod system_prefs;
mod terminal;
mod terminal_image_paste;
mod terminal_osc;
mod terminal_paste;
mod terminal_profiles;
#[cfg(all(test, unix))]
mod test_files;
#[cfg(test)]
mod test_git;
#[cfg(test)]
mod test_lsp;
#[cfg(test)]
mod test_sshd;
mod testexp;
mod testrun;
#[cfg(test)]
mod tests;
mod textcase;
mod texttools;
mod trash;
mod tree;
mod ui;
mod ui_sash;
mod update;
mod update_backend;
mod update_install;
mod update_restart;
#[cfg(test)]
mod update_ui_tests;
mod vscode;
mod workbench;
mod workspace_file;

use gpui::{App, Application, Bounds, Focusable, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};

use crate::ui::{ActiveTheme, Assets};
use crate::workbench::Workbench;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn bind_keys(cx: &mut App) {
    use editor::*;
    use tree as t;
    use workbench::*;
    let ed = Some("Editor");
    let tr = Some("FileTree");
    // ⌃T: on macOS Ctrl is a separate key; elsewhere Ctrl+T is already "go to symbol".
    #[cfg(target_os = "macos")]
    cx.bind_keys([KeyBinding::new("ctrl-t", TransposeCharacters, ed), KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None)]);
    // Windows / Linux: F11 is fullscreen; step into is Ctrl+F11 (macOS: F11).
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([KeyBinding::new("f11", ToggleFullScreen, None), KeyBinding::new("ctrl-f11", DebugStepInto, None)]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([KeyBinding::new("f11", DebugStepInto, None)]);
    cx.bind_keys([
        KeyBinding::new("secondary-o", OpenFolder, None),
        KeyBinding::new("secondary-b", ToggleSidebar, None),
        KeyBinding::new("secondary-shift-e", ShowExplorer, None),
        KeyBinding::new("secondary-e", SwitchEditor, None),
        KeyBinding::new("secondary-,", OpenSettingsPage, None),
        KeyBinding::new("secondary-shift-v", OpenMarkdownPreview, None),
        KeyBinding::new("secondary-k z", ToggleZenMode, None),
        KeyBinding::new("secondary-k secondary-q", GoToLastEdit, None),
        KeyBinding::new("secondary-w", CloseEditor, None),
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-k secondary-t", SelectTheme, None),
        KeyBinding::new("secondary-p", QuickOpen, None),
        KeyBinding::new("secondary-shift-p", CommandPalette, None),
        // Let Ctrl+G reach the shell in the terminal.
        KeyBinding::new("ctrl-g", GoToLine, Some("!Terminal")),
        KeyBinding::new("secondary-j", TogglePanel, None),
        KeyBinding::new("secondary-shift-f", FindInFiles, None),
        // --- structural ---
        KeyBinding::new("secondary-alt-shift-f", search::StructuralSearch, None),
        KeyBinding::new("up", search::HistoryPrev, Some("SearchPanel")),
        KeyBinding::new("down", search::HistoryNext, Some("SearchPanel")),
        KeyBinding::new("f4", search::NextResult, None),
        KeyBinding::new("shift-f4", search::PrevResult, None),
        KeyBinding::new("down", search_editor::SelectNextRow, Some("SearchEditorList")),
        KeyBinding::new("up", search_editor::SelectPrevRow, Some("SearchEditorList")),
        KeyBinding::new("enter", search_editor::ActivateRow, Some("SearchEditorList")),
        KeyBinding::new("space", search_editor::ToggleRow, Some("SearchEditorList")),
        KeyBinding::new("secondary-shift-r", search_editor::RerunSearch, Some("SearchEditor")),
        KeyBinding::new("ctrl-shift-g", ShowScm, None),
        // Ctrl+Shift+D duplicates the selection in the editor on Windows and Linux.
        KeyBinding::new(if cfg!(target_os = "macos") { "ctrl-shift-d" } else { "alt-shift-d" }, ShowRunAndDebug, None),
        KeyBinding::new("secondary-\\", SplitEditor, None),
        KeyBinding::new("ctrl-secondary-right", MoveEditorToOtherGroup, None),
        KeyBinding::new("ctrl-secondary-left", MoveEditorToOtherGroup, None),
        KeyBinding::new("secondary-1", FocusLeftGroup, None),
        KeyBinding::new("secondary-2", FocusRightGroup, None),
        KeyBinding::new("secondary-shift-m", ShowProblems, None),
        KeyBinding::new("secondary-alt-r", SendRequest, None),
        KeyBinding::new("alt-f5", diff::PrevChange, Some("DiffView")),
        KeyBinding::new("f7", diff::NextChange, Some("DiffView")),
        KeyBinding::new("shift-f7", diff::PrevChange, Some("DiffView")),
        KeyBinding::new("secondary-k secondary-i", diff::ToggleInline, Some("DiffView")),
        // Text field
        KeyBinding::new("left", input::FieldLeft, Some("TextField")),
        KeyBinding::new("right", input::FieldRight, Some("TextField")),
        KeyBinding::new("home", input::FieldHome, Some("TextField")),
        KeyBinding::new("end", input::FieldEnd, Some("TextField")),
        KeyBinding::new("backspace", input::FieldBackspace, Some("TextField")),
        KeyBinding::new("delete", input::FieldDelete, Some("TextField")),
        KeyBinding::new("secondary-a", input::FieldSelectAll, Some("TextField")),
        KeyBinding::new("secondary-v", input::FieldPaste, Some("TextField")),
        KeyBinding::new("secondary-c", input::FieldCopy, Some("TextField")),
        KeyBinding::new("enter", input::FieldSubmit, Some("TextField")),
        KeyBinding::new("shift-enter", input::FieldNewline, Some("TextField && multiline")),
        KeyBinding::new("escape", input::FieldEscape, Some("TextField")),
        KeyBinding::new("ctrl-`", ToggleTerminal, None),
        KeyBinding::new("ctrl-shift-`", NewTerminal, None),
        KeyBinding::new("secondary-shift-]", FocusNextTerminal, Some("Terminal")),
        KeyBinding::new("secondary-shift-[", FocusPreviousTerminal, Some("Terminal")),
        // Terminal
        // QuickPick
        KeyBinding::new("up", picker::PickPrev, Some("Picker")),
        KeyBinding::new("down", picker::PickNext, Some("Picker")),
        KeyBinding::new("enter", picker::PickConfirm, Some("Picker")),
        KeyBinding::new("escape", picker::PickDismiss, Some("Picker")),
        KeyBinding::new("backspace", picker::PickBackspace, Some("Picker")),
        KeyBinding::new("secondary-a", picker::PickSelectAll, Some("Picker")),
        KeyBinding::new("secondary-v", picker::PickPaste, Some("Picker")),
        KeyBinding::new("secondary-c", picker::PickCopy, Some("Picker")),
        KeyBinding::new("secondary-x", picker::PickCut, Some("Picker")),
        // The update dialog: Esc closes it, Tab walks its buttons, Enter and Space press the focused one (Enter: the primary one).
        KeyBinding::new("escape", UpdateDismiss, Some("UpdateDialog")),
        KeyBinding::new("escape", CancelExtensionConsent, Some("ExtensionConsent")),
        KeyBinding::new("up", MarketPrevious, Some("MarketSearch")),
        KeyBinding::new("down", MarketNext, Some("MarketSearch")),
        KeyBinding::new("cmd-w", UpdateDismiss, Some("UpdateDialog")),
        KeyBinding::new("tab", UpdateFocusNext, Some("UpdateDialog")),
        KeyBinding::new("shift-tab", UpdateFocusPrev, Some("UpdateDialog")),
        KeyBinding::new("enter", UpdateConfirm, Some("UpdateDialog")),
        KeyBinding::new("space", UpdateConfirm, Some("UpdateDialog")),
        // Editor
        KeyBinding::new("left", MoveLeft, ed),
        KeyBinding::new("right", MoveRight, ed),
        KeyBinding::new("up", MoveUp, ed),
        KeyBinding::new("down", MoveDown, ed),
        KeyBinding::new("shift-left", SelectLeft, ed),
        KeyBinding::new("shift-right", SelectRight, ed),
        KeyBinding::new("shift-up", SelectUp, ed),
        KeyBinding::new("shift-down", SelectDown, ed),
        KeyBinding::new("home", MoveHome, ed),
        KeyBinding::new("end", MoveEnd, ed),
        KeyBinding::new("shift-home", SelectHome, ed),
        KeyBinding::new("shift-end", SelectEnd, ed),
        KeyBinding::new("pageup", PageUp, ed),
        KeyBinding::new("pagedown", PageDown, ed),
        KeyBinding::new("backspace", Backspace, ed),
        KeyBinding::new("shift-backspace", Backspace, ed),
        KeyBinding::new("delete", Delete, ed),
        KeyBinding::new("enter", Newline, ed),
        KeyBinding::new("shift-enter", Newline, ed),
        KeyBinding::new("tab", Tab, ed),
        KeyBinding::new("shift-tab", Outdent, ed),
        KeyBinding::new("secondary-/", ToggleComment, ed),
        KeyBinding::new("shift-alt-a", ToggleBlockComment, ed),
        KeyBinding::new("alt-secondary-[", FoldRegion, ed),
        KeyBinding::new("secondary-shift-l", SelectAllOccurrences, ed),
        KeyBinding::new("secondary-enter", InsertLineBelow, ed),
        KeyBinding::new("secondary-shift-enter", InsertLineAbove, ed),
        KeyBinding::new("secondary-]", IndentLine, ed),
        KeyBinding::new("secondary-[", OutdentLine, ed),
        KeyBinding::new("shift-pageup", terminal::TermScrollPageUp, Some("Terminal")),
        KeyBinding::new("shift-pagedown", terminal::TermScrollPageDown, Some("Terminal")),
        KeyBinding::new("shift-pageup", SelectPageUp, ed),
        KeyBinding::new("shift-pagedown", SelectPageDown, ed),
        KeyBinding::new("secondary-shift-\\", JumpToBracket, ed),
        KeyBinding::new("secondary-u", CursorUndo, ed),
        KeyBinding::new("shift-alt-i", CursorsAtLineEnds, ed),
        KeyBinding::new("alt-secondary-k", ToggleBookmark, ed),
        KeyBinding::new("alt-secondary-l", NextBookmark, ed),
        KeyBinding::new("alt-secondary-j", PreviousBookmark, ed),
        KeyBinding::new("ctrl-shift-secondary-right", ExpandSelection, ed),
        KeyBinding::new("ctrl-shift-secondary-left", ShrinkSelection, ed),
        KeyBinding::new("ctrl-j", JoinLines, ed),
        KeyBinding::new("secondary-shift-d", DuplicateSelection, ed),
        KeyBinding::new("f8", NextProblem, ed),
        KeyBinding::new("shift-f8", PreviousProblem, ed),
        KeyBinding::new("secondary-f12", GoToImplementation, ed),
        KeyBinding::new("alt-secondary-]", UnfoldRegion, ed),
        KeyBinding::new("secondary-k secondary-0", FoldAll, ed),
        KeyBinding::new("secondary-k secondary-1", FoldLevel1, ed),
        KeyBinding::new("secondary-k secondary-2", FoldLevel2, ed),
        KeyBinding::new("secondary-k secondary-3", FoldLevel3, ed),
        KeyBinding::new("secondary-k secondary-4", FoldLevel4, ed),
        KeyBinding::new("secondary-k secondary-5", FoldLevel5, ed),
        KeyBinding::new("secondary-k secondary-6", FoldLevel6, ed),
        KeyBinding::new("secondary-k secondary-7", FoldLevel7, ed),
        KeyBinding::new("secondary-k secondary-[", FoldRecursively, ed),
        KeyBinding::new("secondary-k secondary-]", UnfoldRecursively, ed),
        KeyBinding::new("secondary-k secondary-j", UnfoldAll, ed),
        KeyBinding::new("alt-z", ToggleWordWrap, None),
        // macOS: ⌃- / ⌃⇧-. Elsewhere Ctrl+- is zoom out: like VS Code, Ctrl+Alt+-.
        KeyBinding::new(if cfg!(target_os = "macos") { "ctrl--" } else { "ctrl-alt--" }, GoBack, None),
        KeyBinding::new("ctrl-shift--", GoForward, None),
        KeyBinding::new("secondary-shift-t", ReopenClosedEditor, None),
        KeyBinding::new("secondary-n", NewFile, None),
        KeyBinding::new("secondary-shift-n", NewWindow, None),
        KeyBinding::new("ctrl-r", SwitchProject, Some("!Terminal")),
        KeyBinding::new("secondary-shift-b", RunBuildTask, None),
        KeyBinding::new("f5", StartDebugging, None),
        KeyBinding::new("shift-f5", StopDebugging, None),
        KeyBinding::new("secondary-shift-f5", RestartDebugging, None),
        KeyBinding::new("f10", DebugStepOver, None),
        KeyBinding::new("f6", DebugPause, None),
        KeyBinding::new("ctrl-f5", workbench::RunWithoutDebugging, None),
        KeyBinding::new("up", workbench::ConsoleHistoryPrev, Some("DebugRepl")),
        KeyBinding::new("down", workbench::ConsoleHistoryNext, Some("DebugRepl")),
        KeyBinding::new("tab", workbench::ConsoleComplete, Some("DebugRepl")),
        KeyBinding::new("shift-f11", DebugStepOut, None),
        KeyBinding::new("f9", ToggleBreakpoint, ed),
        KeyBinding::new("ctrl-tab", NextEditor, None),
        KeyBinding::new("ctrl-shift-tab", PreviousEditor, None),
        KeyBinding::new("ctrl-pagedown", NextEditor, None),
        KeyBinding::new("ctrl-pageup", PreviousEditor, None),
        KeyBinding::new("alt-secondary-right", NextEditor, None),
        KeyBinding::new("alt-secondary-left", PreviousEditor, None),
        KeyBinding::new("secondary-k secondary-w", CloseAllEditors, None),
        KeyBinding::new("secondary-k u", CloseSavedEditors, None),
        KeyBinding::new("secondary-k secondary-enter", KeepEditor, None),
        KeyBinding::new("secondary-k f", CloseFolder, None),
        KeyBinding::new("secondary-shift-s", SaveAs, None),
        KeyBinding::new("alt-up", MoveLineUp, ed),
        KeyBinding::new("alt-down", MoveLineDown, ed),
        KeyBinding::new("shift-alt-up", CopyLineUp, ed),
        KeyBinding::new("shift-alt-down", CopyLineDown, ed),
        KeyBinding::new("secondary-shift-k", DeleteLine, ed),
        KeyBinding::new("secondary-l", SelectLine, ed),
        KeyBinding::new("secondary-=", ZoomIn, None),
        KeyBinding::new("secondary-+", ZoomIn, None),
        KeyBinding::new("secondary--", ZoomOut, None),
        KeyBinding::new("secondary-0", ZoomReset, None),
        KeyBinding::new("escape", Escape, ed),
        KeyBinding::new("secondary-a", SelectAll, ed),
        KeyBinding::new("secondary-c", Copy, ed),
        KeyBinding::new("secondary-x", Cut, ed),
        KeyBinding::new("secondary-v", Paste, ed),
        KeyBinding::new("secondary-z", Undo, ed),
        KeyBinding::new("secondary-z", merge::UndoMerge, Some("MergeView")),
        KeyBinding::new("secondary-shift-z", merge::RedoMerge, Some("MergeView")),
        KeyBinding::new("secondary-shift-z", Redo, ed),
        KeyBinding::new("secondary-s", Save, ed),
        KeyBinding::new("secondary-d", SelectNextOccurrence, ed),
        // Find / replace (should work while the bar is focused too).
        KeyBinding::new("secondary-f", Find, ed),
        KeyBinding::new("secondary-f", Find, Some("FindBar")),
        KeyBinding::new("f3", FindNext, ed),
        KeyBinding::new("shift-f3", find::FindPrev, ed),
        KeyBinding::new("shift-enter", find::FindPrev, Some("FindBar")),
        KeyBinding::new("tab", find::SwitchField, Some("FindBar")),
        KeyBinding::new("shift-tab", find::SwitchField, Some("FindBar")),
        KeyBinding::new("alt-c", find::ToggleCase, Some("FindBar")),
        KeyBinding::new("alt-w", find::ToggleWord, Some("FindBar")),
        KeyBinding::new("alt-r", find::ToggleRegex, Some("FindBar")),
        KeyBinding::new("alt-p", find::TogglePreserveCase, Some("FindBar")),
        KeyBinding::new("secondary-f", large::FindInLarge, Some("LargeFile")),
        KeyBinding::new("end", large::FollowEnd, Some("LargeFile")),
        KeyBinding::new("secondary-down", large::FollowEnd, Some("LargeFile")),
        KeyBinding::new("shift-enter", large::LargeFindPrev, Some("LargeFind")),
        // In the editor ⌥⌘L is next bookmark; in the find bar it's find in selection (like VS Code).
        KeyBinding::new("alt-secondary-l", find::ToggleInSelection, Some("FindBar")),
        KeyBinding::new("alt-enter", find::SelectAllMatches, Some("FindBar")),
        KeyBinding::new("alt-secondary-enter", find::ReplaceAll, Some("FindBar")),
        KeyBinding::new("ctrl-space", TriggerCompletion, ed),
        KeyBinding::new("f12", GoToDefinition, ed),
        KeyBinding::new("f2", RenameSymbol, ed),
        KeyBinding::new("shift-f12", FindReferences, ed),
        KeyBinding::new("secondary-shift-o", GoToSymbol, ed),
        KeyBinding::new("secondary-t", GoToWorkspaceSymbol, None),
        KeyBinding::new("secondary-.", CodeActions, ed),
        KeyBinding::new("shift-alt-f", FormatDocument, ed),
        KeyBinding::new("secondary-k secondary-f", FormatSelection, ed),
        KeyBinding::new("secondary-k secondary-k", DeleteAllRight, ed),
        KeyBinding::new("secondary-k secondary-c", AddLineComment, ed),
        KeyBinding::new("secondary-k secondary-u", RemoveLineComment, ed),
        KeyBinding::new("secondary-k secondary-d", AddSelectionToPreviousFindMatch, ed),
        KeyBinding::new("alt-f3", NextChange, ed),
        KeyBinding::new("alt-f12", PeekDefinition, ed),
        KeyBinding::new("shift-alt-h", ShowIncomingCalls, ed),
        KeyBinding::new("shift-alt-f3", PreviousChange, ed),
        KeyBinding::new("alt-secondary-s", SaveAll, None),
        KeyBinding::new("secondary-k secondary-i", ShowHover, ed),
        // While the completion list is open (same depth as Editor; the later binding wins).
        KeyBinding::new("up", CompletionPrev, Some("completing")),
        KeyBinding::new("down", CompletionNext, Some("completing")),
        KeyBinding::new("enter", CompletionAccept, Some("completing")),
        KeyBinding::new("tab", CompletionAccept, Some("completing")),
        KeyBinding::new("escape", CompletionDismiss, Some("completing")),
        // File tree
        KeyBinding::new("up", t::SelectPrev, tr),
        KeyBinding::new("down", t::SelectNext, tr),
        KeyBinding::new("shift-up", t::ExtendPrev, tr),
        KeyBinding::new("shift-down", t::ExtendNext, tr),
        KeyBinding::new("shift-home", t::ExtendFirst, tr),
        KeyBinding::new("shift-end", t::ExtendLast, tr),
        KeyBinding::new("shift-pageup", t::ExtendPageUp, tr),
        KeyBinding::new("shift-pagedown", t::ExtendPageDown, tr),
        KeyBinding::new("secondary-a", t::SelectAllEntries, tr),
        KeyBinding::new("secondary-space", t::ToggleSelection, tr),
        KeyBinding::new("home", t::SelectFirst, tr),
        KeyBinding::new("end", t::SelectLast, tr),
        KeyBinding::new("pageup", t::PageUp, tr),
        KeyBinding::new("pagedown", t::PageDown, tr),
        KeyBinding::new("space", t::Confirm, tr),
        KeyBinding::new("right", t::Expand, tr),
        KeyBinding::new("shift-right", t::ExpandAll, tr),
        KeyBinding::new("shift-left", t::CollapseAllInside, tr),
        KeyBinding::new("left", t::Collapse, tr),
        KeyBinding::new("enter", t::Confirm, tr),
        KeyBinding::new("f2", t::RenameEntry, tr),
        KeyBinding::new("secondary-backspace", t::DeleteEntry, tr),
        KeyBinding::new("secondary-c", t::CopyEntry, tr),
        KeyBinding::new("secondary-x", t::CutEntry, tr),
        KeyBinding::new("secondary-v", t::PasteEntry, tr),
        // Context menu
        KeyBinding::new("up", menu::MenuPrev, Some("ContextMenu")),
        KeyBinding::new("down", menu::MenuNext, Some("ContextMenu")),
        KeyBinding::new("enter", menu::MenuConfirm, Some("ContextMenu")),
        KeyBinding::new("escape", menu::MenuDismiss, Some("ContextMenu")),
        KeyBinding::new("left", menu::MenuLeft, Some("ContextMenu")),
        KeyBinding::new("right", menu::MenuRight, Some("ContextMenu")),
    ]);
    // ⌘G / ⇧⌘G on macOS only: on Linux and Windows Ctrl+G is go to line, Ctrl+Shift+G is Source Control
    // (next / previous match is F3 / Shift+F3).
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-v", terminal::TermPaste, Some("Terminal")),
        KeyBinding::new("cmd-c", terminal::TermCopy, Some("Terminal")),
        KeyBinding::new("cmd-k", terminal::TermClear, Some("Terminal")),
        KeyBinding::new("cmd-\\", SplitTerminal, Some("Terminal")),
        KeyBinding::new("cmd-a", terminal::TermSelectAll, Some("Terminal")),
        KeyBinding::new("cmd-f", terminal::TermFind, Some("Terminal")),
        KeyBinding::new("cmd-g", terminal::TermFindNext, Some("Terminal")),
        KeyBinding::new("cmd-shift-g", terminal::TermFindPrevious, Some("Terminal")),
        KeyBinding::new("cmd-home", terminal::TermScrollToTop, Some("Terminal")),
        KeyBinding::new("cmd-up", terminal::TermPrevCommand, Some("Terminal")),
        KeyBinding::new("cmd-down", terminal::TermNextCommand, Some("Terminal")),
        KeyBinding::new("cmd-end", terminal::TermScrollToBottom, Some("Terminal")),
        KeyBinding::new("cmd-g", FindNext, ed),
        KeyBinding::new("cmd-g", FindNext, Some("FindBar")),
        KeyBinding::new("cmd-shift-g", find::FindPrev, ed),
        KeyBinding::new("cmd-shift-g", find::FindPrev, Some("FindBar")),
        KeyBinding::new("alt-left", MoveWordLeft, ed),
        KeyBinding::new("alt-right", MoveWordRight, ed),
        KeyBinding::new("alt-shift-left", SelectWordLeft, ed),
        KeyBinding::new("alt-shift-right", SelectWordRight, ed),
        KeyBinding::new("alt-backspace", DeleteWordLeft, ed),
        KeyBinding::new("alt-delete", DeleteWordRight, ed),
        KeyBinding::new("cmd-left", MoveHome, ed),
        KeyBinding::new("cmd-right", MoveEnd, ed),
        KeyBinding::new("cmd-shift-left", SelectHome, ed),
        KeyBinding::new("cmd-shift-right", SelectEnd, ed),
        KeyBinding::new("cmd-up", DocStart, ed),
        KeyBinding::new("cmd-down", DocEnd, ed),
        KeyBinding::new("cmd-shift-up", SelectDocStart, ed),
        KeyBinding::new("cmd-shift-down", SelectDocEnd, ed),
        KeyBinding::new("alt-cmd-f", FindReplace, ed),
        KeyBinding::new("alt-cmd-f", FindReplace, Some("FindBar")),
        KeyBinding::new("alt-cmd-up", AddCursorAbove, ed),
        KeyBinding::new("shift-alt-cmd-up", ColumnSelectUp, ed),
        KeyBinding::new("shift-alt-cmd-down", ColumnSelectDown, ed),
        KeyBinding::new("shift-alt-cmd-left", ColumnSelectLeft, ed),
        KeyBinding::new("shift-alt-cmd-right", ColumnSelectRight, ed),
        KeyBinding::new("alt-cmd-down", AddCursorBelow, ed),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, ed),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("alt-f", menus::FileMenu, None),
        KeyBinding::new("alt-e", menus::EditMenu, None),
        KeyBinding::new("alt-s", menus::SelectionMenu, None),
        KeyBinding::new("alt-v", menus::ViewMenu, None),
        KeyBinding::new("alt-g", menus::GoMenu, None),
        KeyBinding::new("alt-t", menus::TerminalMenu, None),
        KeyBinding::new("alt-w", menus::WindowMenu, None),
        KeyBinding::new("alt-h", menus::HelpMenu, None),
        // Ctrl+C copies a terminal selection or interrupts the program when nothing is selected.
        // Ctrl+V is the shell's "literal" key, so paste uses Ctrl+Shift+V.
        KeyBinding::new("ctrl-shift-c", terminal::TermCopy, Some("Terminal")),
        KeyBinding::new("ctrl-shift-v", terminal::TermPaste, Some("Terminal")),
        // Ctrl+K, Ctrl+A and Ctrl+F belong to the shell (kill line, line start, forward); the terminal's own commands take Shift.
        KeyBinding::new("ctrl-shift-k", terminal::TermClear, Some("Terminal")),
        KeyBinding::new("ctrl-shift-5", SplitTerminal, Some("Terminal")),
        KeyBinding::new("ctrl-shift-a", terminal::TermSelectAll, Some("Terminal")),
        KeyBinding::new("ctrl-shift-f", terminal::TermFind, Some("Terminal")),
        KeyBinding::new("f3", terminal::TermFindNext, Some("Terminal")),
        KeyBinding::new("shift-f3", terminal::TermFindPrevious, Some("Terminal")),
        KeyBinding::new("ctrl-shift-home", terminal::TermScrollToTop, Some("Terminal")),
        KeyBinding::new("ctrl-shift-up", terminal::TermPrevCommand, Some("Terminal")),
        KeyBinding::new("ctrl-shift-down", terminal::TermNextCommand, Some("Terminal")),
        KeyBinding::new("ctrl-shift-end", terminal::TermScrollToBottom, Some("Terminal")),
        KeyBinding::new("ctrl-left", MoveWordLeft, ed),
        KeyBinding::new("ctrl-right", MoveWordRight, ed),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, ed),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, ed),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, ed),
        KeyBinding::new("ctrl-delete", DeleteWordRight, ed),
        KeyBinding::new("ctrl-home", DocStart, ed),
        KeyBinding::new("ctrl-end", DocEnd, ed),
        KeyBinding::new("ctrl-shift-home", SelectDocStart, ed),
        KeyBinding::new("ctrl-shift-end", SelectDocEnd, ed),
        KeyBinding::new("ctrl-y", Redo, ed),
        KeyBinding::new("ctrl-h", FindReplace, ed),
        KeyBinding::new("ctrl-h", FindReplace, Some("FindBar")),
        KeyBinding::new("ctrl-alt-up", AddCursorAbove, ed),
        KeyBinding::new("ctrl-alt-down", AddCursorBelow, ed),
        KeyBinding::new("ctrl-shift-alt-up", ColumnSelectUp, ed),
        KeyBinding::new("ctrl-shift-alt-down", ColumnSelectDown, ed),
        KeyBinding::new("ctrl-shift-alt-left", ColumnSelectLeft, ed),
        KeyBinding::new("ctrl-shift-alt-right", ColumnSelectRight, ed),
    ]);
}

/// Default shortcuts + the user's file (if any). The user's are bound later:
/// in GPUI the later binding wins. Returns the list of warnings.
/// Rebuild the keymap in layers, each able to change the one before: the built-in defaults, the keymap preset an extension
/// ships if the user chose one (`keymap.preset`), key bindings extensions contribute, and the user's `keybindings.jsonc`.
/// An extension binding never takes a key the defaults or the preset use: a pressed key must keep doing what the user
/// knows, so such a binding is left out with a warning.
pub fn apply_keymap(cx: &mut App, user_file: Option<&std::path::Path>) -> Vec<String> {
    cx.clear_key_bindings();
    bind_keys(cx);
    let mut warnings = Vec::new();
    let preset = cx.try_global::<settings::Settings>().map(|s| s.keymap_preset.clone()).unwrap_or_default();
    match ext::preset(&preset) {
        None => {}
        Some(Ok((label, path))) => match std::fs::read_to_string(&path) {
            Ok(text) => warnings.extend(bind_file(cx, &text, &label)),
            Err(e) => warnings.push(format!("{label}: {e}")),
        },
        Some(Err(e)) => warnings.push(format!("keymap.preset: {e}")),
    }
    warnings.extend(bind_extension_keys(cx));
    if let Some(text) = user_file.and_then(|p| std::fs::read_to_string(p).ok()) {
        warnings.extend(bind_file(cx, &text, "keybindings.jsonc"));
    }
    warnings
}

/// Key bindings from extensions, minus any whose keys are already bound.
fn bind_extension_keys(cx: &mut App) -> Vec<String> {
    let (list, mut warnings) = ext::bindings();
    if list.is_empty() {
        return warnings;
    }
    let taken: std::collections::HashSet<String> =
        cx.key_bindings().borrow().bindings().map(|b| b.keystrokes().iter().map(|k| k.unparse()).collect::<Vec<_>>().join(" ")).collect();
    let mut bindings = Vec::new();
    for b in list {
        let Some(action) = commands::action_by_id(&b.command) else {
            warnings.push(format!("{}: {}", b.ext, i18n::tf("unknown command “{}”", &[&b.command])));
            continue;
        };
        let predicate = match b.when.as_deref().map(gpui::KeyBindingContextPredicate::parse) {
            None => None,
            Some(Ok(p)) => Some(std::rc::Rc::new(p)),
            Some(Err(e)) => {
                warnings.push(format!("{}: \"when\": {e}", b.ext));
                continue;
            }
        };
        match KeyBinding::load(&b.key, action, predicate, false, None, &gpui::DummyKeyboardMapper) {
            Ok(k) => {
                let keys = k.keystrokes().iter().map(|k| k.unparse()).collect::<Vec<_>>().join(" ");
                if taken.contains(&keys) {
                    warnings.push(format!("{}: {}", b.ext, i18n::tf("“{}” is a built-in shortcut; an extension cannot change it", &[&b.key])));
                } else {
                    bindings.push(k);
                }
            }
            Err(e) => warnings.push(format!("{}: \"{}\": {e}", b.ext, b.key)),
        }
    }
    cx.bind_keys(bindings);
    warnings
}

/// Bind the entries of a `keybindings.jsonc`-format file; `label` names it in warnings.
fn bind_file(cx: &mut App, text: &str, label: &str) -> Vec<String> {
    let (entries, warnings) = settings::parse_keybindings(text);
    let mut warnings: Vec<String> = warnings.into_iter().map(|w| w.replacen("keybindings.jsonc", label, 1)).collect();
    let mut bindings = Vec::new();
    for b in entries {
        let action: Box<dyn gpui::Action> = match &b.command {
            None => Box::new(gpui::NoAction),
            Some(id) => match commands::action_by_id(id) {
                Some(a) => a,
                None => {
                    // Suggest the closest command (usually a typo).
                    let guess = commands::COMMAND_IDS
                        .iter()
                        .filter_map(|c| fuzzy::score(id, c).map(|(s, _)| (s, *c)))
                        .max_by_key(|(s, _)| *s)
                        .map(|(_, c)| format!(", did you mean \"{c}\"?"))
                        .unwrap_or_default();
                    warnings.push(format!("{label}: unknown command \"{id}\"{guess}"));
                    continue;
                }
            },
        };
        let predicate = match b.when.as_deref().map(gpui::KeyBindingContextPredicate::parse) {
            None => None,
            Some(Ok(p)) => Some(std::rc::Rc::new(p)),
            Some(Err(e)) => {
                warnings.push(format!("{label}: \"when\": {e}"));
                continue;
            }
        };
        match KeyBinding::load(&b.key, action, predicate, false, None, &gpui::DummyKeyboardMapper) {
            Ok(k) => bindings.push(k),
            Err(e) => warnings.push(format!("{label}: \"{}\": {e}", b.key)),
        }
    }
    cx.bind_keys(bindings);
    warnings
}

struct Args {
    path: Option<String>,
    connect: Option<String>,
    theme: Option<String>,
    terminal: bool,
    search: Option<String>,
    /// Sidebar to show on startup (explorer, search, scm, remote).
    panel: Option<String>,
    /// Hidden developer flag: `;`-separated command names, run in order after startup.
    run: Vec<String>,
    /// Started by the relaunch after an update: reopen the windows the old version left (`update_restart.rs`).
    after_update: bool,
    /// `--no-extensions`: start with no extension loaded (safe mode).
    no_extensions: bool,
    /// `--extension-dev FOLDER` (`cyx dev`): load that folder as an extension and reload it on every save.
    extension_dev: Option<String>,
    /// `--install-extension FILE.cyx` (repeatable): install these packages and exit, without a window.
    install: Vec<String>,
}

/// `--install-extension`: install each package into the user's store, as the Extensions view does after its consent
/// sheet (the person running the command is the one consenting), and report one line per package. Exit status 1 when any
/// failed; the others are still installed.
fn install_extensions(files: &[String]) -> i32 {
    let Some(cfg) = settings::config_dir() else {
        eprintln!("codeyard: no settings folder");
        return 1;
    };
    let store = ext::store(&cfg);
    let settings =
        std::fs::read_to_string(cfg.join("settings.jsonc")).ok().and_then(|t| settings::parse_layers(&t, &[]).ok()).map(|(s, _)| s).unwrap_or_default();
    let registries = ext_registry::registries(&settings);
    let mut status = 0;
    for f in files {
        let path = std::fs::canonicalize(f).unwrap_or_else(|_| f.into());
        // A file (or anything that looks like one) is a package; `publisher.name[@version]` comes from a registry.
        let from_file = path.exists() || f.ends_with(".cyx") || f.contains(['/', '\\']);
        let result = if from_file { ext::install_file(&store, &path) } else { ext_registry::install_from_command_line(&cfg, &registries, f) };
        match result {
            Ok(m) => println!("installed {} {}", m.id, m.version),
            Err(e) => {
                eprintln!("{f}: {e}");
                status = 1;
            }
        }
    }
    status
}

/// The windows an update restart left for this process; the file is consumed here. Anything wrong with it is logged and means
/// an ordinary start.
fn update_handover() -> Option<update_restart::Handover> {
    let dir = workbench::state_path()?.parent()?.to_path_buf();
    match update_restart::take(&dir, env!("CARGO_PKG_VERSION"), std::time::SystemTime::now()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("not reopening the windows from before the update: {e}");
            None
        }
    }
}

/// The first-run question and the daily check belong to a real session: not to debug builds (unless a developer asked for a
/// scripted update with `CODEYARD_DEV_UPDATE`), and not to a window that a script drives (`--do`, `--panel`).
fn update_checks_enabled(args: &Args) -> bool {
    let scripted_update = cfg!(debug_assertions) && std::env::var_os("CODEYARD_DEV_UPDATE").is_some();
    (!cfg!(debug_assertions) || scripted_update) && args.run.is_empty() && args.panel.is_none()
}

/// What `codeyard --version` prints (also what a bug report should quote).
fn version_line() -> String {
    format!("codeyard {}", env!("CARGO_PKG_VERSION"))
}

/// Developer commands (`--do`) are for debug builds, or a release build started with `CODEYARD_DEV` set.
fn developer_mode() -> bool {
    cfg!(debug_assertions) || std::env::var_os("CODEYARD_DEV").is_some()
}

fn parse_args() -> Args {
    parse_args_from(std::env::args().skip(1), developer_mode())
}

/// `--do` can press keys, click and run any palette command, "Workspace: Trust This Folder" and tasks among them: whoever builds the
/// command line (a launcher, a `codeyard://`-style handler, a document's "open with") must not get that from a release build.
fn parse_args_from(argv: impl Iterator<Item = String>, dev: bool) -> Args {
    let mut args = Args {
        path: None,
        connect: None,
        theme: None,
        terminal: false,
        search: None,
        panel: None,
        run: Vec::new(),
        after_update: false,
        no_extensions: false,
        extension_dev: None,
        install: Vec::new(),
    };
    let mut it = argv;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--connect" => args.connect = it.next(),
            "--theme" => args.theme = it.next(),
            "--terminal" => args.terminal = true,
            "--no-extensions" => args.no_extensions = true,
            "--extension-dev" => args.extension_dev = it.next(),
            "--install-extension" => args.install.extend(it.next()),
            update_restart::FLAG => args.after_update = true,
            "--search" => args.search = it.next(),
            "--panel" => args.panel = it.next(),
            // The value is consumed either way, so it is not mistaken for a folder to open.
            "--do" => {
                let steps = it.next().map(|v| v.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
                if dev {
                    args.run = steps;
                } else {
                    eprintln!("--do is ignored: it is for developer builds (set CODEYARD_DEV=1)");
                }
            }
            "-V" | "--version" => {
                println!("{}", version_line());
                std::process::exit(0);
            }
            "-h" | "--help" => {
                println!(
                    "usage: codeyard [FOLDER | FILE] [--connect HOST] [--theme ID] [--terminal] [--search TEXT] [--panel explorer|search|scm|debug|testing|remote|welcome|settings] [--no-extensions] [--extension-dev FOLDER] [--install-extension FILE.cyx|ID[@VERSION]]"
                );
                std::process::exit(0);
            }
            _ => args.path = Some(a),
        }
    }
    args
}

#[cfg(test)]
mod args_tests {
    use super::parse_args_from;

    fn parse(dev: bool, a: &[&str]) -> super::Args {
        parse_args_from(a.iter().map(|s| s.to_string()), dev)
    }

    #[test]
    fn scripted_commands_need_developer_mode() {
        let line = ["/work", "--do", "Workspace: Trust This Folder;Tasks: Run Task", "--theme", "dark"];
        let release = parse(false, &line);
        assert!(release.run.is_empty(), "{:?}", release.run);
        assert_eq!((release.path.as_deref(), release.theme.as_deref()), (Some("/work"), Some("dark")), "the rest of the line still works");
        assert_eq!(parse(true, &line).run, ["Workspace: Trust This Folder", "Tasks: Run Task"]);
        // Its value is not taken for a path, and a trailing --do is harmless.
        assert_eq!(parse(false, &["--do", "key:enter"]).path, None);
        assert!(parse(false, &["--do"]).run.is_empty());
    }

    #[test]
    fn safe_mode_turns_extensions_off() {
        let a = parse(false, &["--no-extensions", "/work"]);
        assert!(a.no_extensions);
        assert_eq!(a.path.as_deref(), Some("/work"));
        assert!(!parse(false, &["/work"]).no_extensions);
        let dev = parse(false, &["--extension-dev", "/ext", "/work"]);
        assert_eq!((dev.extension_dev.as_deref(), dev.path.as_deref()), (Some("/ext"), Some("/work")), "its value is not the folder to open");
        let inst = parse(false, &["--install-extension", "a.cyx", "--install-extension", "b.cyx"]);
        assert_eq!((inst.install.as_slice(), inst.path.as_deref()), (&["a.cyx".to_string(), "b.cyx".to_string()][..], None));
    }
}

/// ssh is asking (password, host key): ask in the frontmost window, wait for the answer.
#[cfg(unix)]
fn install_askpass(cx: &mut App) {
    use futures::StreamExt;
    let (tx, rx) = std::sync::mpsc::channel();
    let handler: host::askpass::Handler = std::sync::Arc::new(move |request| {
        let (reply, answer) = std::sync::mpsc::channel();
        tx.send((request, reply)).ok()?;
        // Don't let ssh wait forever if the user never answers.
        answer.recv_timeout(std::time::Duration::from_secs(300)).ok().flatten()
    });
    if let Err(e) = host::askpass::install(handler) {
        eprintln!("askpass unavailable, ssh will not prompt: {e}");
        return;
    }
    let mut requests = ui::bridge(rx);
    cx.spawn(async move |cx| {
        while let Some((request, reply)) = requests.next().await {
            let target = cx
                .update(|cx| cx.active_window().and_then(|w| w.downcast::<Workbench>()).or_else(|| cx.windows().iter().find_map(|w| w.downcast::<Workbench>())))
                .ok()
                .flatten();
            if let Some(window) = target {
                let _ = window.update(cx, |wb, window, cx| wb.ask(request, reply, window, cx));
            }
        }
    })
    .detach();
}

/// Dock icon: when run without an `.app` bundle (cargo run) macOS shows the default icon.
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    // In a packaged app Info.plist provides the icon (no extra memory); can be disabled for measurements.
    let bundled = std::env::current_exe().is_ok_and(|e| e.to_string_lossy().contains(".app/Contents/MacOS/"));
    if bundled || std::env::var_os("CODEYARD_NO_DOCK_ICON").is_some() {
        return;
    }
    static ICON: &[u8] = include_bytes!("../assets/macos/icon-256.png");
    let Some(mtm) = MainThreadMarker::new() else { return };
    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(ICON));
    if let Some(image) = image {
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    }
}

/// Verification: asks the menu macOS would ask the Dock for (`applicationDockMenu:`) ourselves and writes its titles
/// to stderr. With `CODEYARD_DUMP_DOCK=1`, a few seconds after startup.
#[cfg(target_os = "macos")]
fn dump_dock_menu() {
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, msg_send};
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    unsafe {
        let delegate: *mut AnyObject = msg_send![&*app, delegate];
        if delegate.is_null() {
            eprintln!("DOCK: no delegate");
            return;
        }
        let menu: *mut AnyObject = msg_send![delegate, applicationDockMenu: &*app];
        if menu.is_null() {
            eprintln!("DOCK: no menu");
            return;
        }
        let items: *mut AnyObject = msg_send![menu, itemArray];
        let n: usize = msg_send![items, count];
        // CODEYARD_DUMP_DOCK=<index>: press that row as if clicked from the Dock.
        if let Some(ix) = std::env::var("CODEYARD_DUMP_DOCK").ok().and_then(|v| v.parse::<isize>().ok()) {
            let _: () = msg_send![menu, performActionForItemAtIndex: ix];
            eprintln!("DOCK: performed item {ix}");
        }
        for i in 0..n {
            let item: *mut AnyObject = msg_send![items, objectAtIndex: i];
            let sep: bool = msg_send![item, isSeparatorItem];
            let title: *mut AnyObject = msg_send![item, title];
            let utf8: *const std::os::raw::c_char = msg_send![title, UTF8String];
            let t = if sep { "---".to_string() } else { std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned() };
            eprintln!("DOCK[{i}]: {t}");
        }
    }
}

/// Open a workbench window; on close, back up unsaved changes or ask.
pub fn open_workbench_window(cx: &mut App) -> Option<gpui::WindowHandle<Workbench>> {
    // Open the new window slightly down and right of the frontmost one (so they don't stack exactly).
    let offset = px(28. * cx.windows().len() as f32);
    let mut bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    bounds.origin.x += offset;
    bounds.origin.y += offset;
    // The first window opens where and at the size the last closed window had; default if the saved position
    // is on a screen that's no longer attached.
    let saved = cx
        .windows()
        .is_empty()
        .then(workbench::saved_window_bounds)
        .flatten()
        .filter(|b| cx.displays().iter().any(|d| workbench::visible_enough(d.bounds(), b.get_bounds())));
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(saved.unwrap_or(WindowBounds::Windowed(bounds))),
                // macOS and Windows use themed title bars; Windows controls keep native hit testing.
                titlebar: Some(TitlebarOptions {
                    title: Some("codeyard".into()),
                    appears_transparent: cfg!(any(target_os = "macos", target_os = "windows")),
                    traffic_light_position: Some(gpui::point(px(12.), px(10.))),
                }),
                window_min_size: Some(size(px(480.), px(320.))),
                // Windows: the UI Automation adapter must be attached before the window is first shown.
                show: !cfg!(windows),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Workbench::new(window, cx)),
        )
        .ok()?;
    #[cfg(any(target_os = "macos", windows))]
    workbench::a11y::attach(window, cx);
    #[cfg(windows)]
    window.update(cx, |_, window, _| window.activate_window()).ok();
    window
        .update(cx, |_, window, cx| {
            let entity = cx.entity().downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                let Some(wb) = entity.upgrade() else { return true };
                // Not in the middle of swapping a new version in: the last window would end the app, another one would lose the result.
                if wb.update(cx, |wb, cx| wb.hold_close_for_install(window, cx)) {
                    return false;
                }
                // Hot exit: back up and close. If it's off and the buffer is dirty, ask.
                if wb.update(cx, |wb, cx| wb.prepare_quit(cx)) {
                    return true;
                }
                wb.update(cx, |wb, cx| wb.confirm_close(window, cx));
                false
            });
        })
        .ok();
    Some(window)
}

fn main() {
    perf::init();
    if std::env::args_os().len() == 2 && std::env::args_os().nth(1).is_some_and(|arg| arg == "--check-agent-bundle") {
        match bundled_agents::check() {
            Ok(()) => return,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    // If ssh invoked this binary as SSH_ASKPASS: forward the question to the window, then exit.
    #[cfg(unix)]
    if let Some(code) = host::askpass::client_main() {
        std::process::exit(code);
    }
    // SAFETY: first thing in `main` after the askpass early exit; no parser has been created yet.
    unsafe { syntax::use_mimalloc() };
    workbench::migrate_legacy_dirs();
    crash::install(workbench::crash_dir());
    let args = parse_args();
    if !args.install.is_empty() {
        std::process::exit(install_extensions(&args.install));
    }
    if args.no_extensions {
        ext::turn_off();
    }
    if let Some(dir) = &args.extension_dev {
        ext::set_dev(Some(std::fs::canonicalize(dir).unwrap_or_else(|_| dir.into())));
    }
    // A folder or host on the command line wins; the file is consumed either way.
    let reopen = args.after_update.then(update_handover).flatten().filter(|_| args.path.is_none() && args.connect.is_none());
    // Finder's "Open With", dropping on the Dock icon and `open -a Codeyard x` addresses are forwarded to the window through this channel
    // (the handler must be set up before the app opens; a request that arrives on first launch isn't lost either).
    let (open_tx, mut open_rx) = futures::channel::mpsc::unbounded::<Vec<String>>();
    let app = Application::new().with_assets(Assets);
    app.on_open_urls(move |urls| {
        open_tx.unbounded_send(urls).ok();
    });
    app.run(move |cx: &mut App| {
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(urls) = open_rx.next().await {
                cx.update(|cx| sysopen::open_urls(urls, cx)).ok();
            }
        })
        .detach();
        bind_keys(cx);
        #[cfg(target_os = "macos")]
        set_dock_icon();
        #[cfg(target_os = "macos")]
        cx.bind_keys([
            KeyBinding::new("cmd-h", menus::HideApp, None),
            KeyBinding::new("alt-cmd-h", menus::HideOtherApps, None),
            KeyBinding::new("cmd-m", menus::MinimizeWindow, None),
        ]);
        menus::install(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(settings::Settings::default());
        system_prefs::refresh(cx);
        let window = open_workbench_window(cx).expect("could not open window");
        #[cfg(unix)]
        install_askpass(cx);
        // ⇧⌘N (or menu): empty new window.
        cx.on_action(|_: &workbench::NewWindow, cx| {
            if let Some(w) = open_workbench_window(cx) {
                w.update(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx))).ok();
            }
        });
        window
            .update(cx, |wb, window, cx| {
                if let Some(id) = &args.theme {
                    wb.use_theme(id, window, cx);
                }
                if let Some(path) = &args.path {
                    wb.open_path(path, window, cx);
                } else if args.connect.is_none() && reopen.is_none() {
                    wb.restore_last(window, cx);
                }
                if args.terminal {
                    wb.open_terminal(window, cx);
                }
                if let Some(q) = args.search.clone() {
                    wb.search_for(&q, window, cx);
                }
                wb.sync_dock_menu(cx);
                if let Some(name) = args.panel.clone() {
                    wb.show_panel_named(&name, window, cx);
                }
                if let Some(alias) = args.connect.clone() {
                    wb.connect(alias, window, cx);
                }
                if update_checks_enabled(&args) {
                    wb.schedule_update_checks(window, cx);
                    update_install::sweep_at_start(&update_install::install_kind(), update::updates_dir().ok());
                }
                window.focus(&wb.focus_handle(cx));
                // A measured run stays in the background: it must not take the keyboard from whoever is working while it runs.
                if !perf::enabled() {
                    cx.activate(true);
                }
            })
            .ok();
        perf::first_window(window, cx);
        if let Some(h) = &reopen {
            update_restart::reopen_windows(h, window, &mut open_workbench_window, cx);
        }
        #[cfg(target_os = "macos")]
        if std::env::var_os("CODEYARD_DUMP_DOCK").is_some() {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                dump_dock_menu();
            })
            .detach();
        }
        if !args.run.is_empty() {
            let steps = args.run.clone();
            cx.spawn(async move |cx| {
                let delay = std::env::var("CODEYARD_DO_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1800);
                for label in steps {
                    cx.background_executor().timer(std::time::Duration::from_millis(delay)).await;
                    // `click:x,y` / `rclick:x,y` (window coordinates): click / right-click.
                    if let Some((right, coords)) = label.strip_prefix("click:").map(|c| (false, c)).or_else(|| label.strip_prefix("rclick:").map(|c| (true, c)))
                    {
                        let n: Vec<f32> = coords.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                        if let [x, y] = n[..] {
                            cx.update_window(window.into(), |_, window, cx| {
                                use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, point};
                                let (pos, mods) = (point(px(x), px(y)), Modifiers::none());
                                let button = if right { MouseButton::Right } else { MouseButton::Left };
                                window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: pos, pressed_button: None, modifiers: mods }), cx);
                                window.dispatch_event(
                                    PlatformInput::MouseDown(MouseDownEvent { button, position: pos, modifiers: mods, click_count: 1, first_mouse: false }),
                                    cx,
                                );
                                window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button, position: pos, modifiers: mods, click_count: 1 }), cx);
                            })
                            .ok();
                        }
                        continue;
                    }
                    // `key:tab`, `key:cmd-shift-p`: keystroke; `type:fn`: typing text (into the real window from inside).
                    if let Some(spec) = label.strip_prefix("key:").or_else(|| label.strip_prefix("type:")) {
                        let typing = label.starts_with("type:");
                        cx.update_window(window.into(), |_, window, cx| {
                            use gpui::{Keystroke, Modifiers};
                            let strokes: Vec<Keystroke> = if typing {
                                spec.chars().map(|c| Keystroke { modifiers: Modifiers::none(), key: c.to_string(), key_char: Some(c.to_string()) }).collect()
                            } else {
                                spec.split(' ').filter_map(|k| Keystroke::parse(k).ok()).collect()
                            };
                            for keystroke in strokes {
                                window.dispatch_keystroke(keystroke, cx);
                            }
                        })
                        .ok();
                        continue;
                    }
                    if let Some(coords) = label.strip_prefix("drag:") {
                        // `drag:x,y,x2,y2` (window coordinates): sends mouse press, drag and
                        // release events to the drawn layout (not while the Workbench is updating: it can't be re-entered).
                        let n: Vec<f32> = coords.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                        if let [x, y, x2, y2] = n[..] {
                            cx.update_window(window.into(), |_, window, cx| {
                                use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, point};
                                let at = |x: f32, y: f32| point(px(x), px(y));
                                let mods = Modifiers::none();
                                window
                                    .dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: at(x, y), pressed_button: None, modifiers: mods }), cx);
                                window.dispatch_event(
                                    PlatformInput::MouseDown(MouseDownEvent {
                                        button: MouseButton::Left,
                                        position: at(x, y),
                                        modifiers: mods,
                                        click_count: 1,
                                        first_mouse: false,
                                    }),
                                    cx,
                                );
                                for i in 1..=10 {
                                    let t = i as f32 / 10.;
                                    let p = at(x + (x2 - x) * t, y + (y2 - y) * t);
                                    window.dispatch_event(
                                        PlatformInput::MouseMove(MouseMoveEvent { position: p, pressed_button: Some(MouseButton::Left), modifiers: mods }),
                                        cx,
                                    );
                                }
                                window.dispatch_event(
                                    PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: at(x2, y2), modifiers: mods, click_count: 1 }),
                                    cx,
                                );
                            })
                            .ok();
                        }
                        continue;
                    }
                    window.update(cx, |wb, window, cx| wb.run_command_label(&label, window, cx)).ok();
                }
            })
            .detach();
        }
        // All ssh ControlMasters are closed on every exit, including ⌘Q and the last window closing.
        cx.on_app_quit(|_| {
            host::ssh::close_all_masters();
            async {}
        })
        .detach();
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            } else {
                #[cfg(target_os = "macos")]
                crate::menus::refresh_dock_windows(cx);
            }
        })
        .detach();
    });
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn version_line_names_the_binary_and_the_cargo_version() {
        let line = super::version_line();
        assert_eq!(line, format!("codeyard {}", env!("CARGO_PKG_VERSION")));
        assert!(line.split(' ').nth(1).is_some_and(|v| v.split('.').count() == 3), "{line}");
    }
}
