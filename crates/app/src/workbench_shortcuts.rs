//! The Keyboard Shortcuts page of the workbench: the rows it lists, and the changes it asks for written to
//! `keybindings.jsonc` (comments and layout kept) and applied at once.

use gpui::{AppContext as _, Context, Entity, Window};

use super::super::{OpenKeybindings, Page, ShowShortcutsPage, Workbench};
use crate::commands::{self, Run};
use crate::shortcuts_page::edit::{self, Change, FileEntry, Key};
use crate::shortcuts_page::rows::{self, Command, Row, Source};
use crate::shortcuts_page::{ShortcutsEvent, ShortcutsPage, Status};

impl Workbench {
    /// Every command with its keys: the palette's commands, then the commands only `keybindings.jsonc` can name.
    pub(in crate::workbench) fn shortcut_rows(&self, cx: &gpui::App) -> Vec<Row> {
        let registry: Vec<(&str, Box<dyn gpui::Action>)> = commands::COMMAND_IDS.iter().filter_map(|id| commands::action_by_id(id).map(|a| (*id, a))).collect();
        let mut listed: Vec<Command> = Vec::new();
        for command in self.commands() {
            let Run::Action(action) = command.run else { continue };
            let id = registry.iter().find(|(_, known)| known.partial_eq(action.as_ref())).map(|(id, _)| id.to_string());
            listed.push(Command { label: command.label, id, action });
        }
        for (id, action) in registry {
            if !listed.iter().any(|c| c.action.partial_eq(action.as_ref())) {
                listed.push(Command { label: id.to_string(), id: Some(id.to_string()), action });
            }
        }
        let user = self.user_key_entries();
        let extension: Vec<FileEntry> = crate::ext::bindings()
            .0
            .into_iter()
            .filter(|b| b.extension.is_none())
            .filter_map(|b| {
                Some(FileEntry {
                    index: 0,
                    keys: edit::canonical_keys(&b.key)?,
                    command: Some(b.command),
                    when: b.when.as_deref().map(edit::normalise_when),
                    extras: false,
                })
            })
            .collect();
        rows::build(&listed, &cx.key_bindings().borrow(), &user, &extension)
    }

    /// The page, when it is open, shows the keymap as it is now (after a reload: an edit of the file, a profile switch).
    pub(in crate::workbench) fn sync_shortcuts_page(&mut self, cx: &mut Context<Self>) {
        let Some(Page::Shortcuts(page)) = self.tabs.page.clone() else { return };
        let rows = self.shortcut_rows(cx);
        page.update(cx, |page, cx| page.refresh_rows(rows, cx));
    }

    /// The entries of the user's file; none when it is missing or cannot be read (the page then shows only defaults).
    fn user_key_entries(&self) -> Vec<FileEntry> {
        let Some((_, path)) = self.config_paths() else { return Vec::new() };
        std::fs::read_to_string(path).ok().and_then(|text| edit::read_entries(&text).ok()).unwrap_or_default()
    }

    /// "Preferences: Keyboard Shortcuts" and "Help: Keyboard Shortcuts Reference".
    pub(in crate::workbench) fn show_shortcuts_page(&mut self, _: &ShowShortcutsPage, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.shortcut_rows(cx);
        let editable = self.config_paths().is_some();
        let view = cx.new(|cx| ShortcutsPage::new(rows, editable, cx));
        self.tabs._page_sub =
            Some(cx.subscribe_in(&view, window, |this, page, event: &ShortcutsEvent, window, cx| this.on_shortcuts_event(page, event, window, cx)));
        view.read(cx).focus_search(window, cx);
        self.tabs.page = Some(Page::Shortcuts(view));
        self.reveal_editor_area(window, cx);
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        cx.notify();
    }

    fn on_shortcuts_event(&mut self, page: &Entity<ShortcutsPage>, event: &ShortcutsEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ShortcutsEvent::Run(label) => {
                if let Some(cmd) = self.commands().into_iter().find(|c| c.label == *label) {
                    // The page closes and the command is applied to the active editor.
                    self.close_page(cx);
                    self.focus_active_editor(window, cx);
                    // Run after focus has moved to the editor (on the next turn).
                    let me = cx.entity();
                    window.on_next_frame(move |window, cx| me.update(cx, |this, cx| this.run_command(cmd.run, window, cx)));
                }
            }
            ShortcutsEvent::OpenFile => self.open_keybindings(&OpenKeybindings, window, cx),
            ShortcutsEvent::Set { row, keys, add } => {
                let Some(id) = row.id.as_deref() else { return };
                // The new key works where the command's other keys do (an editor command stays an editor command).
                let when = self.shortcut_rows(cx).into_iter().filter(|r| r.label == row.label).find_map(|r| r.binding.and_then(|b| b.when));
                let replacing = row.binding.as_ref().filter(|_| !add).map(key_of);
                let change = Change::Set { command: id, keys, when: when.as_deref(), replacing };
                let done = crate::i18n::tf("“{0}” now uses {1}", &[&row.label, &rows::display_keys(keys)]);
                self.change_shortcuts(page, &change, Some((row.label.clone(), keys.clone())), done, window, cx);
            }
            ShortcutsEvent::Remove(row) => {
                let (Some(id), Some(binding)) = (row.id.as_deref(), row.binding.as_ref()) else { return };
                let mut done = crate::i18n::tf("Removed {0} from “{1}”", &[&binding.display, &row.label]);
                // An entry with `args` takes them along: say so, so it is not a surprise.
                let lost =
                    self.user_key_entries().iter().any(|e| e.extras && e.command.as_deref() == Some(id) && e.keys == binding.keys && e.when == binding.when);
                if lost {
                    done = format!("{done} {}", crate::i18n::t("Its “args” were removed with it."));
                }
                self.change_shortcuts(page, &Change::Remove { command: id, key: key_of(binding) }, None, done, window, cx);
            }
            ShortcutsEvent::Reset(row) => {
                let Some(id) = row.id.as_deref() else { return };
                let rows = self.shortcut_rows(cx);
                let off: Vec<Binding> = rows.iter().filter(|r| r.label == row.label).filter_map(|r| r.binding.clone()).filter(|b| b.disabled).collect();
                let turned_off: Vec<Key> = off.iter().map(key_of).collect();
                let done = crate::i18n::tf("“{}” is back to its default keys", &[&row.label]);
                self.change_shortcuts(page, &Change::Reset { command: id, turned_off: &turned_off }, None, done, window, cx);
            }
        }
    }

    /// Writes `change` into the file, applies it, and tells the page what happened. `bound`: the command and keys just
    /// given, whose clashes with other commands are reported.
    fn change_shortcuts(
        &mut self,
        page: &Entity<ShortcutsPage>,
        change: &Change,
        bound: Option<(String, String)>,
        done: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let problem = |text: String| Status { text, problem: true };
        let Some((_, path)) = self.config_paths() else {
            // The settings folder is gone (a profile deleted in another window): nothing is written, and the next reload falls back.
            let text = crate::i18n::t("The settings folder is gone. Nothing was changed.").to_string();
            page.update(cx, |p, cx| p.set_status(Status { text, problem: true }, cx));
            return;
        };
        let result = crate::settings::ensure_file(&path, crate::settings::KEYBINDINGS_TEMPLATE)
            .and_then(|_| std::fs::read_to_string(&path))
            .map_err(|e| crate::i18n::tf("Could not read {0}: {1}", &[&path.display(), &e]))
            .and_then(|text| {
                let new = edit::apply(&text, change).map_err(|e| crate::i18n::tf("{0}: {1}. Fix the file, then try again.", &[&"keybindings.jsonc", &e]))?;
                if new == text {
                    return Ok(false);
                }
                crate::fsutil::write_atomic(&path, new).map_err(|e| crate::i18n::tf("Could not write {0}: {1}", &[&path.display(), &e]))?;
                Ok(true)
            });
        let status = match result {
            Err(e) => {
                page.update(cx, |p, cx| p.set_status(problem(e), cx));
                return;
            }
            Ok(false) => Status { text: crate::i18n::t("Nothing changed").to_string(), problem: false },
            Ok(true) => {
                self.reload_config(window, cx);
                self.shortcut_status(bound, done, cx)
            }
        };
        let rows = self.shortcut_rows(cx);
        page.update(cx, |p, cx| p.set_rows(rows, Some(status), cx));
    }

    /// The message after a change: when the new key is also another command's, who else uses it.
    fn shortcut_status(&self, bound: Option<(String, String)>, done: String, cx: &gpui::App) -> Status {
        let clash = bound.and_then(|(label, keys)| {
            let keys = edit::canonical_keys(&keys)?;
            self.shortcut_rows(cx).into_iter().filter(|r| r.label == label).filter_map(|r| r.binding).find(|b| b.keys == keys && !b.conflicts.is_empty())
        });
        match clash {
            Some(b) => Status { text: crate::i18n::tf("{0} Warning: {1} is also used by {2}.", &[&done, &b.display, &b.conflicts.join(", ")]), problem: true },
            None => Status { text: done, problem: false },
        }
    }
}

use crate::shortcuts_page::rows::Binding;

/// The key of a row as `edit` names it.
fn key_of(b: &Binding) -> Key<'_> {
    Key { keys: &b.keys, when: b.when.as_deref(), user: b.source == Source::User && !b.disabled }
}
