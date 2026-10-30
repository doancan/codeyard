//! Workbench: editors in windows of their own. "View: Move Editor into New Window" shows the active editor in a window
//! of its own; "View: Copy Editor into New Window" does so and keeps the tab. Both show the SAME editor entity as the
//! tab did, through a view of its own (cursor, scroll and find bar per view, like a second editor group): one text, one
//! undo history, one dirty state and one save. The workbench keeps owning the editor, so language features, file
//! watching, backups, renames and the unsaved-changes questions cover it like any tab. Closing that window puts the
//! editor back as a tab of the focused group; ⌘W in it closes the view and asks to save when it was the last view of a
//! changed file. Where the windows were is saved with the session and restored like detached terminals.

use super::*;
use gpui::{Bounds, Pixels, WeakEntity, WindowBounds, WindowOptions};

use crate::editor_layout::{MAX_DETACHED, MAX_GROUPS};

gpui::actions!(workbench, [MoveEditorIntoNewWindow, CopyEditorIntoNewWindow, MoveEditorsBackToMainWindow]);

/// The size an editor's own window opens at.
const SIZE: gpui::Size<Pixels> = gpui::size(px(820.), px(620.));
/// State key of the saved windows, per workspace.
const SAVED_KEY: &str = "editor_windows";
const VERSION: u8 = 1;
const MAX_TEXT: usize = 64 * 1024;
const MAX_PATH: usize = 32 * 1024;

/// An editor shown in a window of its own: the editor, its view there, the window and where that window was last.
pub(super) struct DetachedEditor {
    pub(super) editor: Entity<EditorView>,
    pub(super) slot: usize,
    pub(super) window: gpui::WindowHandle<EditorWindow>,
    bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
}

/// The root view of an editor's own window. It carries the workbench's actions, so commands and shortcuts work there
/// and act on this editor; the context menu and pickers opened from it are drawn here.
pub(super) struct EditorWindow {
    editor: Entity<EditorView>,
    slot: usize,
    home: WeakEntity<Workbench>,
    title: String,
    bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    /// Set once the window exists: its first frame is drawn while the workbench is still opening it, so the
    /// workbench's actions and layers join from the next frame on.
    ready: bool,
    _subs: Vec<Subscription>,
}

impl EditorWindow {
    /// Runs `f` on the workbench in its own window, once this window's event is over.
    fn in_main(&self, cx: &mut App, f: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) {
        let Some(home) = self.home.upgrade() else { return };
        let main = home.read(cx).window_handle;
        cx.defer(move |cx| {
            main.update(cx, |_, window, cx| home.update(cx, |wb, cx| f(wb, window, cx))).ok();
        });
    }

    /// This window is in use: commands act on its editor.
    fn in_use(&self, cx: &mut App) {
        let slot = self.slot;
        self.home.update(cx, |wb, cx| wb.use_editor_window(slot, cx)).ok();
    }

    /// The editor, its view and the workbench (accessibility).
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub(super) fn parts(&self) -> (Entity<EditorView>, usize, WeakEntity<Workbench>) {
        (self.editor.clone(), self.slot, self.home.clone())
    }
}

impl Render for EditorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = {
            let e = self.editor.read(cx);
            format!("{}{}", if e.is_dirty() { "● " } else { "" }, e.name)
        };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        self.bounds.set(Some(window.bounds()));
        let p = *palette(cx);
        let root = div().id("editor-window");
        let (root, layers) = match self.home.upgrade().filter(|_| self.ready) {
            Some(home) => home.update(cx, |wb, cx| {
                let hosted = wb.chrome.editor_host == Some(self.slot);
                let layers: Vec<AnyElement> =
                    if hosted { wb.menu_layer(0., cx).into_iter().chain(wb.picker_layer(theme::SPACE[4], cx)).collect() } else { Vec::new() };
                (wb.with_actions(root, cx), layers)
            }),
            None => (root, Vec::new()),
        };
        root.key_context("EditorWindow")
            .size_full()
            .relative()
            .bg(c(p.bg))
            .text_color(c(p.text))
            .font_family(cx.global::<Settings>().ui_font.clone().map_or(SharedString::from(ui::UI_FONT), SharedString::from))
            .text_size(px(text::SM.0))
            .line_height(px(text::SM.1))
            // Input here makes this the window commands act on.
            .capture_key_down(cx.listener(|this, _, _, cx| this.in_use(cx)))
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| this.in_use(cx)))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    // Before the workbench's own: ⌘W closes this view, and "Move Editors Back" returns this one.
                    .on_action(cx.listener(|this, _: &CloseEditor, window, cx| {
                        let (ed, slot) = (this.editor.clone(), this.slot);
                        if let Some(home) = this.home.upgrade() {
                            home.update(cx, |wb, cx| wb.close_editor_window(&ed, slot, window, cx));
                        }
                    }))
                    .on_action(cx.listener(|this, _: &MoveEditorsBackToMainWindow, _, cx| {
                        let (ed, slot) = (this.editor.clone(), this.slot);
                        this.in_main(cx, move |wb, window, cx| wb.editor_window_returned(&ed, slot, true, window, cx));
                    }))
                    .child(editor::slot_view(self.editor.clone(), self.slot)),
            )
            .children(layers)
    }
}

/// The saved windows: which file each showed (relative to the folder, like saved tabs), whether it was only there
/// (not also a tab), its view and its bounds.
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedEditorWindows {
    version: u8,
    windows: Vec<SavedEditorWindow>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedEditorWindow {
    path: String,
    #[serde(default)]
    only_here: bool,
    #[serde(default)]
    view: Option<[usize; 3]>,
    #[serde(default)]
    at: Option<[f32; 4]>,
}

impl SavedEditorWindows {
    /// The saved windows, if this version wrote them and they are within bounds; anything else is ignored whole.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        if text.len() > MAX_TEXT {
            return None;
        }
        let saved: Self = serde_json::from_str(text).ok()?;
        let valid = saved.version == VERSION
            && saved.windows.len() <= MAX_DETACHED
            && saved.windows.iter().all(|w| !w.path.is_empty() && w.path.len() <= MAX_PATH && !w.path.contains('\0'));
        valid.then_some(saved)
    }

    /// Relative paths of the files that were only in a window (they are opened with the tabs).
    pub(super) fn only_here(&self) -> impl Iterator<Item = &str> {
        self.windows.iter().filter(|w| w.only_here).map(|w| w.path.as_str())
    }
}

impl Workbench {
    /// Editors in windows of their own (an editor shown in two windows is listed once).
    pub(super) fn window_editors(&self) -> Vec<Entity<EditorView>> {
        let mut v: Vec<Entity<EditorView>> = Vec::new();
        for d in &self.chrome.editor_windows {
            if !v.contains(&d.editor) {
                v.push(d.editor.clone());
            }
        }
        v
    }

    /// Whether `ed` is shown in a window of its own.
    pub(super) fn in_editor_window(&self, ed: &Entity<EditorView>) -> bool {
        self.chrome.editor_windows.iter().any(|d| d.editor == *ed)
    }

    /// Whether `ed` is a tab of some group.
    fn in_some_group(&self, ed: &Entity<EditorView>) -> bool {
        self.tabs.editors.contains(ed) || self.tabs.groups.values().any(|g| g.editors.contains(ed))
    }

    /// The editor of the window in use, if one is (see `active_editor`).
    pub(super) fn hosted_editor(&self) -> Option<&Entity<EditorView>> {
        let slot = self.chrome.editor_host?;
        self.chrome.editor_windows.iter().find(|d| d.slot == slot).map(|d| &d.editor)
    }

    /// The editor window with view `slot` is in use: commands act on its editor (in that view), and its menus and
    /// pickers are drawn there.
    pub(super) fn use_editor_window(&mut self, slot: usize, cx: &mut Context<Self>) {
        let Some(ed) = self.chrome.editor_windows.iter().find(|d| d.slot == slot).map(|d| d.editor.clone()) else { return };
        if self.chrome.editor_host != Some(slot) {
            self.chrome.view_host = None;
            self.chrome.editor_host = Some(slot);
            cx.notify();
        }
        ed.update(cx, |e, _| e.set_home(slot));
    }

    /// The main window is in use again.
    pub(super) fn leave_editor_window(&mut self) {
        self.chrome.editor_host = None;
        self.chrome.view_host = None;
    }

    /// Something is shown in the main window's tabs (a file opened from an editor's window, a group focused): the
    /// main window is in use again. When `window` is another window, `f` runs in the main window instead, which comes
    /// to the front, and `true` is returned.
    pub(super) fn run_in_main_window(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> bool {
        if self.chrome.editor_host.take().is_some() {
            self.sync_slots(cx);
        }
        if window.window_handle() == self.window_handle {
            return false;
        }
        let (main, wb) = (self.window_handle, cx.entity());
        cx.defer(move |cx| {
            main.update(cx, |_, window, cx| {
                window.activate_window();
                wb.update(cx, |wb, cx| f(wb, window, cx));
            })
            .ok();
        });
        true
    }

    /// The window in use, when it shows `ed`: events of `ed` (a context menu, go to definition) came from there.
    pub(super) fn host_window_of(&self, ed: &Entity<EditorView>) -> Option<(usize, gpui::WindowHandle<EditorWindow>)> {
        let slot = self.chrome.editor_host?;
        self.chrome.editor_windows.iter().find(|d| d.slot == slot && d.editor == *ed).map(|d| (d.slot, d.window))
    }

    /// Runs `f` on the workbench in the editor window `handle` (after the current event).
    pub(super) fn run_in_editor_window(
        &self,
        handle: gpui::WindowHandle<EditorWindow>,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let wb = cx.entity();
        cx.defer(move |cx| {
            handle.update(cx, |_, window, cx| wb.update(cx, |wb, cx| f(wb, window, cx))).ok();
        });
    }

    /// Go to definition inside the same file, asked in an editor's own window: it moves there, not in a tab.
    pub(super) fn reveal_in_editor_window(&mut self, ed: &Entity<EditorView>, path: &str, line: u32, col: u32, cx: &mut Context<Self>) -> bool {
        let Some((slot, handle)) = self.host_window_of(ed).filter(|_| ed.read(cx).path == path) else { return false };
        let ed = ed.clone();
        self.run_in_editor_window(handle, cx, move |_, window, cx| {
            ed.update(cx, |e, cx| {
                e.set_home(slot);
                e.reveal_lsp(line, col, window, cx);
            })
        });
        true
    }

    /// "View: Move Editor into New Window" (`keep_tab`: "View: Copy Editor into New Window").
    pub(super) fn editor_into_new_window(&mut self, keep_tab: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ed = self.tab_editor().filter(|_| !self.tabs.diff_active && !self.tabs.large_active && !self.tabs.page_active).cloned();
        let Some(ed) = ed else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a file in the editor first.").into()));
            return cx.notify();
        };
        let at = window.bounds();
        let bounds = Bounds::new(at.origin + gpui::point(px(40.), px(40.)), SIZE);
        if let Err(e) = self.show_in_editor_window(ed.clone(), bounds, None, cx) {
            self.notice = Some(Notice::Error(e.into()));
            return cx.notify();
        }
        // The window exists, so the document is still open somewhere when its tab goes.
        if !keep_tab && let Some(ix) = self.tabs.editors.iter().position(|e| *e == ed) {
            self.remove_editor(ix, window, cx);
        }
        self.refresh_active_flags(cx);
        self.persist_session(cx);
        cx.notify();
    }

    /// A new window showing `ed` through a new view of it (with `view`'s cursor and scroll when given).
    pub(super) fn show_in_editor_window(
        &mut self,
        ed: Entity<EditorView>,
        bounds: Bounds<Pixels>,
        view: Option<state::View>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let used: Vec<usize> = self.chrome.editor_windows.iter().map(|d| d.slot).collect();
        let Some(slot) = (MAX_GROUPS..MAX_GROUPS + MAX_DETACHED).find(|s| !used.contains(s)) else {
            return Err(crate::i18n::tf("Up to {} editors can be in windows of their own.", &[&MAX_DETACHED.to_string()]));
        };
        ed.update(cx, |e, cx| {
            e.open_slot(slot, cx);
            if let Some(view) = view {
                e.set_view_for_slot(slot, view);
            }
        });
        let title = ed.read(cx).name.to_string();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions { title: Some(title.clone().into()), appears_transparent: false, traffic_light_position: None }),
            window_min_size: Some(gpui::size(px(240.), px(160.))),
            // Windows: the UI Automation adapter must be attached before the window is first shown.
            show: !cfg!(windows),
            ..Default::default()
        };
        let home = cx.entity().downgrade();
        let shared = std::rc::Rc::new(std::cell::Cell::new(Some(bounds)));
        let (e, cell) = (ed.clone(), shared.clone());
        let opened = cx.open_window(options, move |window, cx| {
            if let Some(focus) = e.read(cx).slot_focus(slot) {
                window.focus(&focus);
            }
            // Closing the window puts the editor back as a tab; nothing is closed, so nothing needs saving.
            let (back, home_close) = (e.clone(), home.clone());
            window.on_window_should_close(cx, move |_, cx| {
                if let Some(wb) = home_close.upgrade() {
                    let main = wb.read(cx).window_handle;
                    let back = back.clone();
                    cx.defer(move |cx| {
                        main.update(cx, |_, window, cx| wb.update(cx, |wb, cx| wb.editor_window_returned(&back, slot, false, window, cx))).ok();
                    });
                }
                true
            });
            cx.new(|cx| {
                let mut subs = vec![
                    cx.observe(&e, |_, _, cx| cx.notify()),
                    cx.observe_window_bounds(window, |this: &mut EditorWindow, window, _| this.bounds.set(Some(window.bounds()))),
                    // Coming to the front makes it the window commands act on.
                    cx.observe_window_activation(window, |this: &mut EditorWindow, window, cx| {
                        if window.is_window_active() {
                            this.in_use(cx);
                        }
                    }),
                ];
                // So does the keyboard coming back into its editor.
                if let Some(focus) = e.read(cx).slot_focus(slot) {
                    subs.push(cx.on_focus_in(&focus, window, |this: &mut EditorWindow, _, cx| this.in_use(cx)));
                }
                // Its menus and pickers are drawn here while it is in use.
                if let Some(wb) = home.upgrade() {
                    subs.push(cx.observe(&wb, move |this: &mut EditorWindow, wb, cx| {
                        let wb = wb.read(cx);
                        if wb.chrome.editor_host == Some(this.slot) || wb.menu.is_some() || wb.overlay.is_some() {
                            cx.notify();
                        }
                    }));
                }
                cx.defer_in(window, |this: &mut EditorWindow, _, cx| {
                    this.ready = true;
                    cx.notify();
                });
                EditorWindow { editor: e.clone(), slot, home, title, bounds: cell, ready: false, _subs: subs }
            })
        });
        match opened {
            Ok(handle) => {
                #[cfg(any(target_os = "macos", windows))]
                a11y::platform::attach_with(handle.into(), a11y::windows_access::EDITOR_WINDOW, cx);
                #[cfg(windows)]
                handle.update(cx, |_, window, _| window.activate_window()).ok();
                self.chrome.editor_windows.push(DetachedEditor { editor: ed, slot, window: handle, bounds: shared });
                // It opens in front, with the keyboard: the window in use.
                self.use_editor_window(slot, cx);
                Ok(())
            }
            Err(e) => {
                ed.update(cx, |ed, _| ed.close_slot(slot));
                Err(crate::i18n::tf("The editor could not move to a new window: {}", &[&e.to_string()]))
            }
        }
    }

    /// Forgets the window showing `ed` through `slot` and, with `close`, closes it (after the current event: it may be
    /// the window the event came from). `false` when it was already gone.
    fn drop_editor_window(&mut self, ed: &Entity<EditorView>, slot: usize, close: bool, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.chrome.editor_windows.iter().position(|d| d.editor == *ed && d.slot == slot) else { return false };
        let handle = self.chrome.editor_windows.remove(ix).window;
        if self.chrome.editor_host == Some(slot) {
            self.chrome.editor_host = None;
        }
        if close {
            cx.defer(move |cx| {
                handle.update(cx, |_, window, _| window.remove_window()).ok();
            });
        }
        true
    }

    /// The editor's window closed, or "View: Move Editors Back to Main Window": it is a tab of the focused group again
    /// (the same editor, with its text, undo history and unsaved changes), in front and with the keyboard. `close`:
    /// the window is still open and closes now.
    pub(super) fn editor_window_returned(&mut self, ed: &Entity<EditorView>, slot: usize, close: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.drop_editor_window(ed, slot, close, cx) {
            return;
        }
        let view = ed.update(cx, |e, _| e.view_for_slot(slot));
        let ix = self.tabs.editors.iter().position(|e| e == ed).unwrap_or_else(|| {
            self.tabs.editors.push(ed.clone());
            self.tabs.editors.len() - 1
        });
        self.sync_slots(cx);
        ed.update(cx, |e, _| {
            e.close_slot(slot);
            e.set_view_for_slot(self.focused_slot(), view);
        });
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        self.tabs.page_active = false;
        self.activate(ix, window, cx);
        self.persist_session(cx);
        cx.notify();
    }

    /// "View: Move Editors Back to Main Window": every editor in a window of its own comes back as a tab.
    pub(super) fn return_editor_windows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.editor_windows.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("No editor is in a window of its own.").into()));
            return cx.notify();
        }
        let all: Vec<(Entity<EditorView>, usize)> = self.chrome.editor_windows.iter().map(|d| (d.editor.clone(), d.slot)).collect();
        for (ed, slot) in all {
            self.editor_window_returned(&ed, slot, true, window, cx);
        }
    }

    /// ⌘W in an editor's own window: that view closes. When it was the last view of a changed file, the same question
    /// as closing its last tab comes first (in that window).
    pub(super) fn close_editor_window(&mut self, ed: &Entity<EditorView>, slot: usize, window: &mut Window, cx: &mut Context<Self>) {
        let elsewhere = self.in_some_group(ed) || self.chrome.editor_windows.iter().any(|d| d.editor == *ed && d.slot != slot);
        self.close_view_asking(ed.clone(), !elsewhere, window, cx, move |this, ed, _, cx| {
            if this.drop_editor_window(ed, slot, true, cx) {
                this.forget_closed_view(ed, cx);
                ed.update(cx, |e, _| e.close_slot(slot));
                this.persist_session(cx);
                cx.notify();
            }
        });
    }

    /// Closing a view: when `last` (no other tab or window shows the document) and it has unsaved changes, asks first.
    /// Save closes once the write succeeds (and keeps it open on a conflict or an error), Don't Save closes, Cancel
    /// keeps it.
    pub(super) fn close_view_asking(
        &mut self,
        ed: Entity<EditorView>,
        last: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        close: impl Fn(&mut Self, &Entity<EditorView>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if !last || !ed.read(cx).is_dirty() {
            return close(self, &ed, window, cx);
        }
        let name = ed.read(cx).name.clone();
        let answer = window.prompt(
            PromptLevel::Warning,
            &crate::i18n::tf("Do you want to save the changes you made to {}?", &[&name]),
            Some(crate::i18n::t("Your changes will be lost if you don't save them.")),
            &[crate::i18n::t("Save"), crate::i18n::t("Don't Save"), crate::i18n::t("Cancel")],
            cx,
        );
        let close = std::rc::Rc::new(close);
        let ed = ed.downgrade();
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            this.update_in(cx, |this, window, cx| {
                let Some(entity) = ed.upgrade() else { return };
                match choice {
                    Ok(0) => {
                        let done = std::rc::Rc::new(std::cell::Cell::new(false));
                        cx.subscribe_in(&entity, window, move |this, e, ev: &EditorEvent, window, cx| {
                            if done.get() || !matches!(ev, EditorEvent::Saved) {
                                return;
                            }
                            done.set(true);
                            close(this, e, window, cx);
                        })
                        .detach();
                        entity.update(cx, |e, cx| e.save_now(cx));
                    }
                    Ok(1) => close(this, &entity, window, cx),
                    _ => {}
                }
            })
            .ok();
        })
        .detach();
    }

    /// A view of `ed` closed on purpose: when no tab or window shows the document any more, its backup and pending
    /// response file go and it is offered by "Reopen Closed Editor".
    pub(super) fn forget_closed_view(&mut self, ed: &Entity<EditorView>, cx: &mut Context<Self>) {
        if self.in_some_group(ed) || self.in_editor_window(ed) {
            return;
        }
        let path = ed.read(cx).path.clone();
        self.discard_response_file(&path);
        self.drop_backup(&path, cx);
        let (line, col) = ed.read(cx).cursor_position();
        self.nav.closed.retain(|c| c.0 != path);
        self.nav.closed.push((path, line as u32, col.saturating_sub(1) as u32));
        if self.nav.closed.len() > 20 {
            self.nav.closed.remove(0);
        }
    }

    /// The folder or the connection changes, or the main window closes: the editors' windows close with it. Unsaved
    /// changes were already asked about or backed up like the tabs'.
    pub(super) fn close_editor_windows(&mut self, cx: &mut App) {
        self.chrome.editor_host = None;
        for d in std::mem::take(&mut self.chrome.editor_windows) {
            let handle = d.window;
            cx.defer(move |cx| {
                handle.update(cx, |_, window, _| window.remove_window()).ok();
            });
        }
    }

    /// Windows showing a file that was deleted close, unless it has unsaved changes (then it stays, marked deleted).
    pub(super) fn close_editor_windows_of_deleted(&mut self, gone: impl Fn(&Entity<EditorView>, &App) -> bool, cx: &mut Context<Self>) {
        let doomed: Vec<(Entity<EditorView>, usize)> =
            self.chrome.editor_windows.iter().filter(|d| gone(&d.editor, cx) && !d.editor.read(cx).is_dirty()).map(|d| (d.editor.clone(), d.slot)).collect();
        for (ed, slot) in doomed {
            self.drop_editor_window(&ed, slot, true, cx);
            ed.update(cx, |e, _| e.close_slot(slot));
        }
    }

    /// Save which files are in windows of their own and where those are (with the session).
    pub(super) fn persist_editor_windows(&self, cx: &mut App) {
        let (Some(ws), Some(s)) = (self.persist.workspace_id, &self.persist.state) else { return };
        let mut windows = Vec::new();
        for d in &self.chrome.editor_windows {
            if !d.editor.read(cx).uses_host(&self.remote.host) {
                continue;
            }
            let Some(path) = self.rel(&d.editor.read(cx).path, cx).filter(|p| p.len() <= MAX_PATH && !p.contains('\0')) else { continue };
            let view = d.editor.update(cx, |e, _| e.view_for_slot(d.slot));
            let at = d.bounds.get().map(|b| [b.origin.x, b.origin.y, b.size.width, b.size.height].map(f32::from));
            let only_here = !self.in_some_group(&d.editor);
            windows.push(SavedEditorWindow { path, only_here, view: Some([view.line, view.column, view.scroll_line]), at });
        }
        if let Ok(json) = serde_json::to_string(&SavedEditorWindows { version: VERSION, windows }) {
            let _ = s.set(&format!("{SAVED_KEY}:{ws}"), &json);
        }
    }

    /// The windows saved for workspace `ws`; `None` when there are none or the value is damaged.
    pub(super) fn saved_editor_windows(&self, ws: i64) -> Option<SavedEditorWindows> {
        let text = self.persist.state.as_ref()?.get(&format!("{SAVED_KEY}:{ws}"))?;
        SavedEditorWindows::parse(&text)
    }

    /// After the tabs are back: each saved window opens again on its file, on the display that shows most of it (or
    /// the first one, made to fit). A file in `loaded_for_windows` (relative paths loaded only for their windows) leaves
    /// the tab it was loaded into; any other file keeps its tabs and the window shows the same document, so restoring
    /// never closes a tab. A file that could not be opened is skipped. Only at a start (no editor window is open).
    pub(super) fn restore_editor_windows(
        &mut self,
        saved: Option<SavedEditorWindows>,
        loaded_for_windows: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(saved), Some(root)) = (saved, self.root(cx)) else { return };
        if !self.chrome.editor_windows.is_empty() {
            return;
        }
        let mut displays: Vec<Bounds<Pixels>> = Vec::new();
        for d in cx.primary_display().into_iter().chain(cx.displays()).map(|d| d.bounds()) {
            if !displays.contains(&d) {
                displays.push(d);
            }
        }
        for w in saved.windows {
            let path = ui::join(&root, &w.path);
            let Some(ed) = self.all_editors().into_iter().find(|e| e.read(cx).path == path && e.read(cx).uses_host(&self.remote.host)) else { continue };
            let bounds = term_tabs_glue::restore::place_window(w.at, &displays)
                .unwrap_or_else(|| Bounds::new(window.bounds().origin + gpui::point(px(40.), px(40.)), SIZE));
            let view = w.view.map(|[line, column, scroll_line]| state::View { line, column, scroll_line });
            if self.show_in_editor_window(ed.clone(), bounds, view, cx).is_err() {
                continue;
            }
            if w.only_here && loaded_for_windows.contains(&w.path) {
                // It was loaded as a tab only to be shown here: whichever group took it lets it go.
                for id in self.tabs.layout.groups() {
                    let Some(group) = self.tabs.group_mut(id) else { continue };
                    let Some(ix) = group.editors.iter().position(|e| *e == ed) else { continue };
                    group.editors.remove(ix);
                    group.active = match group.active {
                        _ if group.editors.is_empty() => None,
                        Some(a) if a > ix => Some(a - 1),
                        Some(a) => Some(a.min(group.editors.len() - 1)),
                        None => None,
                    };
                }
            }
        }
        self.sync_slots(cx);
        self.refresh_active_flags(cx);
        // Restoring the tabs saved a session without these windows; save it again with them.
        self.persist_editor_windows(cx);
        cx.notify();
    }
}

#[cfg(test)]
#[path = "workbench_editor_windows_tests.rs"]
mod tests;
