//! Application menu bar (top of the screen on macOS). Items invoke existing actions;
//! shortcuts are shown from the keymap (if the user changes them, the menu changes too).

use gpui::{App, Menu, MenuItem, OsAction, SystemMenuType, actions};

use crate::{editor as ed, workbench as wb};

/// Menu row text, in the current interface language (the key is the English text; see `i18n`).
fn tr(label: &str) -> String {
    crate::i18n::t(label).to_string()
}

actions!(
    app,
    [
        HideApp,
        HideOtherApps,
        ShowAllApps,
        MinimizeWindow,
        ZoomWindow,
        FileMenu,
        EditMenu,
        SelectionMenu,
        ViewMenu,
        GoMenu,
        TerminalMenu,
        WindowMenu,
        HelpMenu,
        OpenRecent0,
        OpenRecent1,
        OpenRecent2,
        OpenRecent3,
        OpenRecent4,
        OpenRecent5,
        OpenRecent6,
        OpenRecent7,
        OpenRecent8,
        OpenRecent9
    ]
);

/// "Recently opened" in the Dock menu (the row number is the digit at the end of the action).
static DOCK_RECENT: std::sync::Mutex<Vec<state::RecentWorkspace>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn remember_recent(recent: &[state::RecentWorkspace]) {
    *DOCK_RECENT.lock().unwrap_or_else(|e| e.into_inner()) = recent.iter().take(10).cloned().collect();
}

/// Windows/Linux draw these menus inside the window; macOS keeps its native menu.
pub(crate) fn desktop_menus(recent: &[state::RecentWorkspace]) -> Vec<Menu> {
    use MenuItem as I;
    let mut menus = menus(recent);
    menus.remove(0); // macOS application menu: rehome its portable commands below.
    menus[0].items.extend([
        I::separator(),
        I::action(tr("Settings…"), wb::OpenSettingsPage),
        I::action(tr("Open Settings (JSON)"), wb::OpenSettings),
        I::action(tr("Import VS Code Settings and Shortcuts"), wb::ImportVsCode),
        I::separator(),
        I::action(tr("Exit"), wb::Quit),
    ]);
    menus[3].items.extend([I::separator(), I::action(tr("Color Theme…"), wb::SelectTheme), I::action(tr("Keyboard Shortcuts"), wb::OpenKeybindings)]);
    menus.push(Menu {
        name: tr("Help").into(),
        items: vec![
            I::action(tr("Welcome"), wb::ShowWelcome),
            I::action(tr("About Codeyard"), wb::ShowAbout),
            I::action(tr("Check for Updates…"), wb::CheckForUpdates),
        ],
    });
    menus
}

/// Rows of the Dock menu. Built on every platform for the tests; only macOS has a Dock to show it.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn dock_items(recent: &[state::RecentWorkspace]) -> Vec<MenuItem> {
    use MenuItem as I;
    let mut items = vec![
        I::action(tr("New Window"), wb::NewWindow),
        I::action(tr("Open Folder…"), wb::OpenFolder),
        I::action(tr("Connect to SSH Host…"), wb::ShowRemote),
        I::action(tr("Go to File…"), wb::QuickOpen),
    ];
    if !recent.is_empty() {
        items.push(I::separator());
        items.extend(recent_items(recent));
    }
    items.push(I::separator());
    items.push(I::action(tr("Settings…"), wb::OpenSettingsPage));
    items.push(I::action(tr("Welcome"), wb::ShowWelcome));
    items
}

/// On right-clicking the Dock icon: shortcuts and recently opened projects. Rebuilt when the recent list (or the interface language)
/// changes.
#[cfg(target_os = "macos")]
pub fn set_dock_menu(cx: &mut App, recent: &[state::RecentWorkspace]) {
    *DOCK_RECENT.lock().unwrap_or_else(|e| e.into_inner()) = recent.iter().take(10).cloned().collect();
    // `setMainMenu:` makes AppKit validate every item at once, and GPUI answers by updating the active window. Callers are
    // usually inside an update of that very window (a setting that changes the language), so it would lease the `Workbench`
    // twice and abort the process: the rebuild waits for the end of the current effect cycle.
    let recent = recent.to_vec();
    cx.defer(move |cx| rebuild(cx, &recent));
}

#[cfg(target_os = "macos")]
fn rebuild(cx: &mut App, recent: &[state::RecentWorkspace]) {
    #[cfg(test)]
    tests::validate_like_appkit(cx);
    cx.set_dock_menu(dock_items(recent));
    // File ▸ Open Recent in the menu bar is refreshed with the same list.
    cx.set_menus(menus(recent));
}

/// The `n`th recently opened project from the Dock menu: open it in the front window.
fn open_recent(n: usize, cx: &mut App) {
    let Some(r) = DOCK_RECENT.lock().unwrap_or_else(|e| e.into_inner()).get(n).cloned() else { return };
    // The menu action arrives while the window is updating: updating the window immediately fails
    // (verified in a real window), so it is deferred to the next effect cycle.
    cx.defer(move |cx| {
        let handle = cx.active_window().or_else(|| cx.windows().first().copied()).and_then(|w| w.downcast::<wb::Workbench>());
        if let Some(w) = handle {
            w.update(cx, |wb, window, cx| {
                window.activate_window();
                wb.open_recent(r, window, cx)
            })
            .ok();
        }
    });
}

pub fn install(cx: &mut App) {
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOtherApps, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAllApps, cx| cx.unhide_other_apps());
    cx.on_action(|_: &MinimizeWindow, cx| {
        if let Some(w) = cx.active_window() {
            w.update(cx, |_, window, _| window.minimize_window()).ok();
        }
    });
    cx.on_action(|_: &ZoomWindow, cx| {
        if let Some(w) = cx.active_window() {
            w.update(cx, |_, window, _| window.zoom_window()).ok();
        }
    });
    cx.on_action(|_: &OpenRecent0, cx| open_recent(0, cx));
    cx.on_action(|_: &OpenRecent1, cx| open_recent(1, cx));
    cx.on_action(|_: &OpenRecent2, cx| open_recent(2, cx));
    cx.on_action(|_: &OpenRecent3, cx| open_recent(3, cx));
    cx.on_action(|_: &OpenRecent4, cx| open_recent(4, cx));
    cx.on_action(|_: &OpenRecent5, cx| open_recent(5, cx));
    cx.on_action(|_: &OpenRecent6, cx| open_recent(6, cx));
    cx.on_action(|_: &OpenRecent7, cx| open_recent(7, cx));
    cx.on_action(|_: &OpenRecent8, cx| open_recent(8, cx));
    cx.on_action(|_: &OpenRecent9, cx| open_recent(9, cx));
    cx.set_menus(menus(&[]));
}

/// Shared rows for the "Open Recent" submenu and the Dock menu (at most 10).
fn recent_items(recent: &[state::RecentWorkspace]) -> Vec<MenuItem> {
    let actions: [Box<dyn gpui::Action>; 10] = [
        Box::new(OpenRecent0),
        Box::new(OpenRecent1),
        Box::new(OpenRecent2),
        Box::new(OpenRecent3),
        Box::new(OpenRecent4),
        Box::new(OpenRecent5),
        Box::new(OpenRecent6),
        Box::new(OpenRecent7),
        Box::new(OpenRecent8),
        Box::new(OpenRecent9),
    ];
    recent
        .iter()
        .take(10)
        .zip(actions)
        .map(|(r, action)| {
            let name = r.path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(&r.path);
            let label = match &r.host {
                Some(h) => format!("{name}  (ssh://{h})"),
                None => name.to_string(),
            };
            MenuItem::Action { name: label.into(), action, os_action: None }
        })
        .collect()
}

pub(crate) fn menus(recent: &[state::RecentWorkspace]) -> Vec<Menu> {
    use MenuItem as I;
    let sep = I::separator;
    let mut open_recent = recent_items(recent);
    if open_recent.is_empty() {
        open_recent.push(I::action(tr("Welcome (no recent folders yet)"), wb::ShowWelcome));
    } else {
        open_recent.push(sep());
        open_recent.push(I::action(tr("More…"), wb::SwitchProject));
    }
    vec![
        Menu {
            name: "codeyard".into(),
            items: vec![
                I::action(tr("About codeyard"), wb::ShowAbout),
                I::action(tr("Check for Updates…"), wb::CheckForUpdates),
                sep(),
                I::action(tr("Settings…"), wb::OpenSettingsPage),
                I::action(tr("Open Settings (JSON)"), wb::OpenSettings),
                I::action(tr("Keyboard Shortcuts"), wb::OpenKeybindings),
                I::action(tr("Color Theme…"), wb::SelectTheme),
                I::action(tr("Import VS Code Settings and Shortcuts"), wb::ImportVsCode),
                sep(),
                I::os_submenu(tr("Services"), SystemMenuType::Services),
                sep(),
                I::action(tr("Hide codeyard"), HideApp),
                I::action(tr("Hide Others"), HideOtherApps),
                I::action(tr("Show All"), ShowAllApps),
                sep(),
                I::action(tr("Quit codeyard"), wb::Quit),
            ],
        },
        Menu {
            name: tr("File").into(),
            items: vec![
                I::action(tr("New File…"), wb::NewFile),
                I::action(tr("New Window"), wb::NewWindow),
                sep(),
                I::action(tr("Open Folder…"), wb::OpenFolder),
                I::submenu(Menu { name: tr("Open Recent").into(), items: open_recent }),
                I::action(tr("Go to File…"), wb::QuickOpen),
                I::action(tr("Connect to SSH Host…"), wb::ShowRemote),
                sep(),
                I::action(tr("Save"), ed::Save),
                I::action(tr("Save As…"), wb::SaveAs),
                I::action(tr("Save All"), wb::SaveAll),
                sep(),
                I::action(tr("Revert File"), ed::RevertFile),
                I::action(tr("Reopen Closed Editor"), wb::ReopenClosedEditor),
                I::action(tr("Close Editor"), wb::CloseEditor),
            ],
        },
        Menu {
            name: tr("Edit").into(),
            items: vec![
                I::os_action(tr("Undo"), ed::Undo, OsAction::Undo),
                I::os_action(tr("Redo"), ed::Redo, OsAction::Redo),
                sep(),
                I::os_action(tr("Cut"), ed::Cut, OsAction::Cut),
                I::os_action(tr("Copy"), ed::Copy, OsAction::Copy),
                I::os_action(tr("Paste"), ed::Paste, OsAction::Paste),
                sep(),
                I::action(tr("Find"), ed::Find),
                I::action(tr("Replace"), ed::FindReplace),
                I::action(tr("Find in Files"), wb::FindInFiles),
                sep(),
                I::action(tr("Toggle Line Comment"), ed::ToggleComment),
                I::action(tr("Format Document"), wb::FormatDocument),
                I::action(tr("Format Selection"), wb::FormatSelection),
                I::action(tr("Rename Symbol"), wb::RenameSymbol),
            ],
        },
        Menu {
            name: tr("Selection").into(),
            items: vec![
                I::os_action(tr("Select All"), ed::SelectAll, OsAction::SelectAll),
                I::action(tr("Expand Selection"), ed::ExpandSelection),
                I::action(tr("Shrink Selection"), ed::ShrinkSelection),
                sep(),
                I::action(tr("Copy Line Up"), ed::CopyLineUp),
                I::action(tr("Copy Line Down"), ed::CopyLineDown),
                I::action(tr("Move Line Up"), ed::MoveLineUp),
                I::action(tr("Move Line Down"), ed::MoveLineDown),
                sep(),
                I::action(tr("Add Cursor Above"), ed::AddCursorAbove),
                I::action(tr("Add Cursor Below"), ed::AddCursorBelow),
                I::action(tr("Add Next Occurrence"), ed::SelectNextOccurrence),
                I::action(tr("Select All Occurrences"), ed::SelectAllOccurrences),
            ],
        },
        Menu {
            name: tr("View").into(),
            items: vec![
                I::action(tr("Command Palette…"), wb::CommandPalette),
                sep(),
                I::action(tr("Explorer"), wb::ShowExplorer),
                I::action(tr("Search"), wb::FindInFiles),
                I::action(tr("Source Control"), wb::ShowScm),
                I::action(tr("Run and Debug"), wb::ShowRunAndDebug),
                I::action(tr("Remote Explorer"), wb::ShowRemoteExplorer),
                I::action(tr("Switch Workspace or Connect to Host…"), wb::ShowRemote),
                sep(),
                I::action(tr("Problems"), wb::ShowProblems),
                I::action(tr("Ports"), wb::ShowPorts),
                I::action(tr("Terminal"), wb::ToggleTerminal),
                sep(),
                I::action(tr("Toggle Sidebar"), wb::ToggleSidebar),
                I::action(tr("Toggle Panel"), wb::TogglePanel),
                I::action(tr("Split Editor"), wb::SplitEditor),
                I::action(tr("Word Wrap"), wb::ToggleWordWrap),
                sep(),
                I::action(tr("Fold"), ed::FoldRegion),
                I::action(tr("Unfold"), ed::UnfoldRegion),
                I::action(tr("Fold All"), ed::FoldAll),
                I::action(tr("Unfold All"), ed::UnfoldAll),
                sep(),
                I::action(tr("Zoom In"), wb::ZoomIn),
                I::action(tr("Zoom Out"), wb::ZoomOut),
                I::action(tr("Reset Zoom"), wb::ZoomReset),
            ],
        },
        Menu {
            name: tr("Go").into(),
            items: vec![
                I::action(tr("Back"), wb::GoBack),
                I::action(tr("Forward"), wb::GoForward),
                sep(),
                I::action(tr("Go to File…"), wb::QuickOpen),
                I::action(tr("Go to Symbol in File…"), wb::GoToSymbol),
                I::action(tr("Go to Symbol in Workspace…"), wb::GoToWorkspaceSymbol),
                I::action(tr("Go to Line…"), wb::GoToLine),
                sep(),
                I::action(tr("Go to Definition"), ed::GoToDefinition),
                I::action(tr("Go to Type Definition"), ed::GoToTypeDefinition),
                I::action(tr("Go to Implementations"), ed::GoToImplementation),
                I::action(tr("Go to References"), wb::FindReferences),
                sep(),
                I::action(tr("Next Problem"), ed::NextProblem),
                I::action(tr("Previous Problem"), ed::PreviousProblem),
            ],
        },
        Menu {
            name: tr("Run").into(),
            items: vec![
                I::action(tr("Start Debugging"), wb::StartDebugging),
                I::action(tr("Run Without Debugging"), wb::RunWithoutDebugging),
                I::action(tr("Stop Debugging"), wb::StopDebugging),
                I::action(tr("Restart Debugging"), wb::RestartDebugging),
                sep(),
                I::action(tr("Continue"), wb::DebugContinue),
                I::action(tr("Pause"), wb::DebugPause),
                I::action(tr("Step Over"), wb::DebugStepOver),
                I::action(tr("Step Into"), wb::DebugStepInto),
                I::action(tr("Step Out"), wb::DebugStepOut),
                I::action(tr("Run to Cursor"), wb::RunToCursor),
                sep(),
                I::action(tr("Toggle Breakpoint"), ed::ToggleBreakpoint),
                I::action(tr("Add Function Breakpoint…"), wb::AddFunctionBreakpoint),
                I::action(tr("Enable All Breakpoints"), wb::EnableAllBreakpoints),
                I::action(tr("Disable All Breakpoints"), wb::DisableAllBreakpoints),
                I::action(tr("Remove All Breakpoints"), wb::RemoveAllBreakpoints),
                sep(),
                I::action(tr("Add Configuration…"), wb::AddDebugConfiguration),
                I::action(tr("Open Launch Configuration"), wb::OpenLaunchConfiguration),
            ],
        },
        Menu {
            name: tr("Terminal").into(),
            items: vec![
                I::action(tr("New Terminal"), wb::NewTerminal),
                I::action(tr("Toggle Terminal"), wb::ToggleTerminal),
                I::action(tr("Kill Terminal"), wb::KillTerminal),
                sep(),
                I::action(tr("Find"), crate::terminal::TermFind),
                I::action(tr("Select All"), crate::terminal::TermSelectAll),
                I::action(tr("Clear Terminal"), wb::ClearTerminal),
                sep(),
                I::action(tr("Scroll to Top"), crate::terminal::TermScrollToTop),
                I::action(tr("Scroll to Bottom"), crate::terminal::TermScrollToBottom),
            ],
        },
        Menu {
            name: tr("Window").into(),
            items: vec![
                I::action(tr("Minimize"), MinimizeWindow),
                I::action(tr("Zoom"), ZoomWindow),
                sep(),
                I::action(tr("Focus Left Editor Group"), wb::FocusLeftGroup),
                I::action(tr("Focus Right Editor Group"), wb::FocusRightGroup),
            ],
        },
    ]
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) mod tests {
    use std::cell::Cell;

    use gpui::{App, TestAppContext};

    use crate::qa_workbench_tests::setup;
    use crate::workbench as wb;

    thread_local! {
        static REBUILDS: Cell<usize> = const { Cell::new(0) };
    }

    /// The test platform's `set_menus` does nothing, so this stands in for what AppKit does inside `setMainMenu:`: it validates the
    /// items at once, and GPUI answers by asking the active window (`App::is_action_available`).
    pub(super) fn validate_like_appkit(cx: &mut App) {
        REBUILDS.with(|r| r.set(r.get() + 1));
        cx.is_action_available(&wb::Quit);
    }

    /// Rebuilding the menus inside an update of the window (a click on "Check Daily" that changes the language) used to lease the
    /// `Workbench` twice and abort the process.
    #[gpui::test]
    fn the_menu_bar_is_rebuilt_after_the_update_that_asked_for_it(cx: &mut TestAppContext) {
        let (wb, cx) = setup(cx);
        cx.run_until_parked();
        REBUILDS.with(|r| r.set(0));
        wb.update(cx, |wb, cx| {
            wb.sync_dock_menu(cx);
            assert_eq!(REBUILDS.with(Cell::get), 0, "the menus were rebuilt while the workbench was still being updated");
        });
        cx.run_until_parked();
        assert_eq!(REBUILDS.with(Cell::get), 1, "rebuilt once, after the update had finished");
    }
}
