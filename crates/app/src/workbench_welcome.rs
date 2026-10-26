//! Welcome screen, Settings page and "Clone Git Repository": pages that open as tabs
//! in the editor area. An empty editor shows the same welcome content (`workbench.startupEditor`).

use gpui::{AnyElement, AppContext as _, Context, Focusable as _, IntoElement as _, ParentElement as _, SharedString, Window, div, prelude::*, px};

use super::{
    CloneRepository, CommandPalette, FindInFiles, InputPurpose, NewFile, Notice, OpenFolder, OpenKeybindings, OpenMarkdownPreview, OpenMergeEditor,
    OpenSettingsPage, Page, QuickOpen, SelectIconTheme, SelectTheme, ShowAbout, ShowDiskAccess, ShowRemote, ShowScm, ShowShortcutsPage, ShowWelcome,
    ToggleTerminal, Tooltip, Workbench, kbd, tilde,
};
use crate::commands::{Command, Run};
use crate::settings_page::{SettingsEvent, SettingsPage};
use crate::ui::{self, Palette, c, icon, text};

/// Tips in the "Did you know?" box; one is shown per day.
pub const TIPS: &[&str] = &[
    "⌘P jumps to any file; type # to search symbols, : to go to a line.",
    "⇧⌘P opens the command palette: every feature has an entry there.",
    "Select a word and press ⌘D to add the next match as another cursor.",
    "⌥↑ / ⌥↓ move the current line; ⇧⌥↑ / ⇧⌥↓ copy it up or down.",
    "F4 steps through the search results, ⇧F4 goes back.",
    "Right-click a commit in the Source Control graph to branch, tag, cherry-pick, revert or reset.",
    "The Timeline section of the explorer lists the commits of the open file; click one to see its diff.",
    "Drag the edge of a sidebar section to resize it; click its title to fold it.",
    "⌘K Z toggles Zen mode; ⌘E switches between open editors.",
    "Connect over SSH and the files, terminal and language servers all run on the server.",
    "⌘K ⌘T previews color themes as you move through the list.",
    "In the file tree, type to jump to a name; ⇧→ opens everything inside a folder, ⇧← closes it.",
    "⌘. offers quick fixes when a language server is running.",
    "Right-click a folder and choose Find in Folder… to search only inside it.",
];

/// Today's tip (rotates by day count).
pub fn tip_of_the_day(days_since_epoch: u64) -> &'static str {
    crate::i18n::t(TIPS[(days_since_epoch as usize) % TIPS.len()])
}

/// Folder name from a repo URL: "https://x/y/repo.git" → "repo", "git@h:u/r" → "r".
pub fn repo_name(url: &str) -> String {
    let t = url.trim().trim_end_matches('/').trim_end_matches(".git");
    t.rsplit(['/', ':']).next().unwrap_or("").to_string()
}

/// Brand colors: separate for light and dark themes (on dark, a mint-green mark on a dark tile; on light, a dark-green
/// mark on a light mint tile). Returns: (tile background, tile border, mark).
pub(super) fn brand_colors(p: &Palette) -> (u32, u32, u32) {
    if ui::luminance_is_light(p.bg) { (0xe6f6ef, 0xbfe3d4, 0x137e69) } else { (0x182525, 0x2f4543, 0x67eab1) }
}

/// Brand header: logo on a tile and the name; no tagline.
fn brand_header(p: &Palette) -> impl IntoElement {
    let (tile, edge, mark) = brand_colors(p);
    div()
        .flex()
        .items_center()
        .gap(px(theme::SPACE[6]))
        .child(
            div().flex_none().size(px(56.)).flex().items_center().justify_center().rounded(px(14.)).bg(c(tile)).border_1().border_color(c(edge)).child(icon(
                "codeyard-mark",
                32.,
                mark,
            )),
        )
        .child(div().text_size(px(30.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(p.text)).child("Codeyard")) // i18n-keep: product name
}

impl Workbench {
    pub(super) fn show_welcome(&mut self, _: &ShowWelcome, _: &mut Window, cx: &mut Context<Self>) {
        self.tabs.page = Some(Page::Welcome);
        self.tabs._page_sub = None;
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        cx.notify();
    }

    /// "About Codeyard": version info is here, not on the welcome screen (gear menu, app menu, palette).
    pub(super) fn show_about(&mut self, _: &ShowAbout, window: &mut Window, cx: &mut Context<Self>) {
        let version = env!("CARGO_PKG_VERSION");
        let detail = format!("{}\n{} · {}", crate::i18n::tf("Version {}", &[&version]), std::env::consts::OS, std::env::consts::ARCH);
        let answer = window.prompt(gpui::PromptLevel::Info, "Codeyard", Some(&detail), &[crate::i18n::t("Close"), crate::i18n::t("Copy Version")], cx);
        cx.spawn_in(window, async move |_, cx| {
            if answer.await == Ok(1) {
                cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(format!("Codeyard {version}")))).ok();
            }
        })
        .detach();
    }

    /// Palette: show the full disk access dialog on demand.
    pub(super) fn show_disk_access(&mut self, _: &ShowDiskAccess, window: &mut Window, cx: &mut Context<Self>) {
        if dirs::home_dir().and_then(|h| crate::access::full_disk_access(&h)) == Some(true) {
            self.notice = Some(Notice::Info(crate::i18n::t("Codeyard already has Full Disk Access.").into()));
            cx.notify();
            return;
        }
        self.ask_disk_access(window, cx);
    }

    fn ask_disk_access(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // macOS never lists an app here by itself: it has to be added with "+" (or dragged in) by hand.
        let detail = format!(
            "{}\n\n{}\n\n{}",
            crate::i18n::t("Without Full Disk Access, Desktop, Documents or external drives may look empty."),
            crate::i18n::t("In Privacy & Security, press + and add Codeyard, then turn it on."),
            crate::i18n::t("Already listed after an update? Remove it with − and add it again.")
        );
        let answer = window.prompt(
            gpui::PromptLevel::Info,
            crate::i18n::t("Allow file access"),
            Some(&detail),
            &[crate::i18n::t("Open System Settings"), crate::i18n::t("Show Codeyard in Finder"), crate::i18n::t("Not Now")],
            cx,
        );
        cx.spawn_in(window, async move |_, cx| {
            match answer.await {
                Ok(0) => cx.update(|_, cx| cx.open_url(crate::access::FULL_DISK_ACCESS_URL)).ok(),
                // Both windows side by side: the app can be dragged straight from Finder into the list.
                Ok(1) => cx
                    .update(|_, cx| {
                        cx.open_url(crate::access::FULL_DISK_ACCESS_URL);
                        if let Some(bundle) = crate::access::app_bundle() {
                            cx.reveal_path(&bundle);
                        }
                    })
                    .ok(),
                _ => None,
            };
        })
        .detach();
    }

    pub(super) fn open_settings_page(&mut self, _: &OpenSettingsPage, window: &mut Window, cx: &mut Context<Self>) {
        let view = match &self.tabs.page {
            Some(Page::Settings(v)) => v.clone(),
            _ => {
                let view = cx.new(SettingsPage::new);
                self.tabs._page_sub = Some(cx.subscribe_in(&view, window, |this, _, ev: &SettingsEvent, window, cx| match ev {
                    SettingsEvent::Write(key, value) => this.write_setting(key, value.clone(), window, cx),
                    SettingsEvent::PickTheme => this.select_theme(&SelectTheme, window, cx),
                    SettingsEvent::PickIconTheme => this.select_icon_theme(&SelectIconTheme, window, cx),
                    SettingsEvent::OpenJson => this.open_settings(&super::OpenSettings, window, cx),
                    SettingsEvent::OpenKeybindings => this.open_keybindings(&OpenKeybindings, window, cx),
                }));
                self.tabs.page = Some(Page::Settings(view.clone()));
                view
            }
        };
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        view.read(cx).focus_search(window, cx);
        cx.notify();
    }

    /// ⇧⌘V: live preview of the active Markdown file (updates as you type).
    pub(super) fn open_markdown_preview(&mut self, _: &OpenMarkdownPreview, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let ext = ed.read(cx).path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        if !matches!(ext.as_str(), "md" | "markdown" | "mdx") {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a Markdown file (.md) to preview it.").into()));
            cx.notify();
            return;
        }
        let view = cx.new(|cx| crate::markdown::MarkdownPreview::new(ed, cx));
        self.tabs.page = Some(Page::Markdown(view));
        self.tabs._page_sub = None;
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        cx.notify();
    }

    /// "Merge Conflict: Open Merge Editor": conflicts in the open file card by card (Current | Incoming | Result).
    pub(super) fn open_merge_editor(&mut self, _: &OpenMergeEditor, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a file with merge conflicts first.").into()));
            cx.notify();
            return;
        };
        if crate::conflict::find(&ed.read(cx).rope_snapshot()).is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("No merge conflicts in this file.").into()));
            cx.notify();
            return;
        }
        let view = cx.new(|cx| crate::merge::MergeView::new(ed, cx));
        self.tabs._page_sub = Some(cx.subscribe_in(&view, window, |this, _, _: &crate::merge::MergeEvent, window, cx| {
            this.close_page(cx);
            this.focus_active_editor(window, cx);
        }));
        window.focus(&view.focus_handle(cx));
        self.tabs.page = Some(Page::Merge(view));
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        cx.notify();
    }

    /// "Help: Keyboard Shortcuts Reference": command and key list (searchable, runs on click).
    pub(super) fn show_shortcuts_page(&mut self, _: &ShowShortcutsPage, window: &mut Window, cx: &mut Context<Self>) {
        let rows: Vec<(String, Option<String>)> = self.commands().into_iter().map(|c| (c.label.clone(), c.shortcut(cx))).collect();
        let view = cx.new(|cx| crate::shortcuts_page::ShortcutsPage::new(rows, cx));
        self.tabs._page_sub = Some(cx.subscribe_in(&view, window, |this, _, ev: &crate::shortcuts_page::ShortcutsEvent, window, cx| {
            let crate::shortcuts_page::ShortcutsEvent::Run(label) = ev;
            if let Some(cmd) = this.commands().into_iter().find(|c| c.label == *label) {
                // The page closes and the command is applied to the active editor.
                this.close_page(cx);
                this.focus_active_editor(window, cx);
                // Run after focus has moved to the editor (on the next turn).
                let me = cx.entity();
                window.on_next_frame(move |window, cx| me.update(cx, |this, cx| this.run_command(cmd.run, window, cx)));
            }
        }));
        view.read(cx).focus_search(window, cx);
        self.tabs.page = Some(Page::Shortcuts(view));
        self.tabs.page_active = true;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        cx.notify();
    }

    pub(super) fn close_page(&mut self, cx: &mut Context<Self>) {
        self.tabs.page = None;
        self.tabs._page_sub = None;
        self.tabs.page_active = false;
        cx.notify();
    }

    /// The settings file changed while the Settings page is open: refresh the fields.
    pub(super) fn sync_settings_page(&mut self, cx: &mut Context<Self>) {
        if let Some(Page::Settings(v)) = &self.tabs.page {
            v.update(cx, |v, cx| v.sync_fields(cx));
        }
    }

    #[cfg(test)]
    pub fn close_page_for_test(&mut self, cx: &mut Context<Self>) {
        self.close_page(cx);
    }

    #[cfg(test)]
    pub fn page_for_test(&self) -> Option<(&'static str, bool)> {
        self.tabs.page.as_ref().map(|p| {
            (
                match p {
                    Page::Welcome => "welcome",
                    Page::Settings(_) => "settings",
                    Page::Markdown(_) => "markdown",
                    Page::Shortcuts(_) => "shortcuts",
                    Page::Merge(_) => "merge",
                    Page::Compare(_) => "compare",
                },
                self.tabs.page_active,
            )
        })
    }

    #[cfg(test)]
    pub fn markdown_blocks_for_test(&self, cx: &gpui::App) -> Option<Vec<crate::markdown::Block>> {
        match &self.tabs.page {
            Some(Page::Markdown(v)) => Some(v.read(cx).blocks_for_test(cx)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn shortcuts_page_for_test(&self) -> Option<gpui::Entity<crate::shortcuts_page::ShortcutsPage>> {
        match &self.tabs.page {
            Some(Page::Shortcuts(v)) => Some(v.clone()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn merge_view_for_test(&self) -> Option<gpui::Entity<crate::merge::MergeView>> {
        match &self.tabs.page {
            Some(Page::Merge(v)) => Some(v.clone()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn settings_page_for_test(&self) -> Option<gpui::Entity<SettingsPage>> {
        match &self.tabs.page {
            Some(Page::Settings(v)) => Some(v.clone()),
            _ => None,
        }
    }

    /// A command's shortcut (`None` if there is none).
    fn shortcut_of(&self, action: impl gpui::Action, cx: &mut Context<Self>) -> Option<String> {
        Command { label: String::new(), run: Run::Action(action.boxed_clone()) }.shortcut(cx)
    }

    /// Large action card on the welcome screen: icon, name and shortcut.
    fn start_tile(
        &self,
        id: &'static str,
        glyph: &'static str,
        label: &'static str,
        action: impl gpui::Action,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let shortcut = self.shortcut_of_boxed(action.boxed_clone(), cx);
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .h(px(56.))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[5]))
            .px(px(theme::SPACE[5]))
            .rounded(px(theme::radius::LG))
            .bg(c(p.surface))
            .border_1()
            .border_color(c(p.border))
            .cursor_pointer()
            // Hover: `surface_hover` fill as before; the border isn't blue, just the existing border made slightly more visible.
            .hover(|d| d.bg(c(p.surface_hover)).border_color(c(ui::mix(p.border, p.text, 0.07))))
            .child(
                div()
                    .flex_none()
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::radius::MD))
                    .bg(ui::ca(p.accent, 0.14))
                    .child(icon(glyph, 18., p.accent)),
            )
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().font_weight(gpui::FontWeight::MEDIUM).child(label))
            .when_some(shortcut, |d, k| d.child(kbd(k, p)))
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
    }

    fn shortcut_of_boxed(&self, action: Box<dyn gpui::Action>, cx: &mut Context<Self>) -> Option<String> {
        Command { label: String::new(), run: Run::Action(action) }.shortcut(cx)
    }

    /// Welcome content: faint logo, start cards, recents, customize, learn, tip of the day.
    /// Version info is not here but under "About" in the gear menu.
    pub(super) fn welcome_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let heading = |h: &'static str| {
            div()
                .pb(px(theme::SPACE[3]))
                .text_size(px(text::XS.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c(p.text_muted))
                .child(crate::i18n::upper(h))
        };
        // Pinned ones at the very top (all visible), then filled up with recents to 6 in total.
        let ordered = super::projects::ordered_recent(&self.persist.recent, &self.persist.pinned);
        let shown = ordered.iter().filter(|(_, pinned)| *pinned).count().max(6);
        let recent: Vec<_> = ordered.iter().take(shown).cloned().collect();
        let more_recent = ordered.len() > recent.len();
        let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400);
        let tip = tip_of_the_day(days);
        let learn = |id: &'static str, label: &'static str, key: Option<String>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(theme::SPACE[4]))
                .h(px(26.))
                .child(div().flex_1().text_color(c(p.text_muted)).child(label))
                .when_some(key, |d, k| d.child(kbd(k, p)))
        };
        let learn_rows = vec![
            learn("w-pal", crate::i18n::t("Command palette"), self.shortcut_of(CommandPalette, cx)),
            learn("w-file", crate::i18n::t("Go to file"), self.shortcut_of(QuickOpen, cx)),
            learn("w-find", crate::i18n::t("Find in files"), self.shortcut_of(FindInFiles, cx)),
            learn("w-scm", crate::i18n::t("Source control"), self.shortcut_of(ShowScm, cx)),
            learn("w-term", crate::i18n::t("Terminal"), self.shortcut_of(ToggleTerminal, cx)),
        ];
        let recent_rows: Vec<AnyElement> = recent
            .into_iter()
            .enumerate()
            .map(|(i, (r, pinned))| {
                let remote = r.host.clone();
                let group: SharedString = format!("w-recent-{i}").into();
                let name = ui::file_name(&r.path).to_string();
                let detail = match &remote {
                    Some(h) => format!("{h}:{}", r.path),
                    None => tilde(&r.path),
                };
                div()
                    .id(("w-recent", i))
                    .group(group.clone())
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[5]))
                    .px(px(theme::SPACE[4]))
                    .rounded(px(theme::radius::MD))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(icon(if remote.is_some() { "server" } else { "folder" }, ui::layout::ICON, p.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().overflow_hidden().whitespace_nowrap().text_color(c(p.text)).child(name))
                            .child(div().overflow_hidden().whitespace_nowrap().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(detail)),
                    )
                    .when(remote.is_some(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .px(px(theme::SPACE[3]))
                                .rounded(px(theme::radius::SM))
                                .bg(ui::ca(p.accent, 0.14))
                                .text_size(px(text::XXS.0))
                                .text_color(c(p.accent))
                                .child("SSH"), // i18n-keep: protocol name
                        )
                    })
                    // On the right, on hover: pin (always visible if pinned) and remove from list.
                    .child({
                        let (pin_target, forget_target) = (r.clone(), r.clone());
                        let button = |id: &'static str, glyph: &'static str, color: u32, tip: &'static str| {
                            div()
                                .id((id, i))
                                .flex_none()
                                .size(px(22.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(theme::radius::SM))
                                .cursor_pointer()
                                .hover(|d| d.bg(c(p.surface_active)))
                                .child(icon(glyph, 14., color))
                                .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.into())).into())
                        };
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[1]))
                            .child(
                                button(
                                    "w-pin",
                                    "pin",
                                    if pinned { p.accent } else { p.text_muted },
                                    if pinned { crate::i18n::t("Unpin") } else { crate::i18n::t("Pin to the top") },
                                )
                                .when(!pinned, |d| d.invisible().group_hover(group.clone(), |s| s.visible()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_pin_recent(pin_target.clone(), cx);
                                })),
                            )
                            .child(
                                button("w-forget", "x", p.text_muted, crate::i18n::t("Remove from Recent"))
                                    .invisible()
                                    .group_hover(group.clone(), |s| s.visible())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.forget_recent(forget_target.clone(), cx);
                                    })),
                            )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.open_recent(r.clone(), window, cx)))
                    .into_any_element()
            })
            .collect();
        div()
            .id("welcome")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(c(p.bg))
            .child(
                div().id("welcome-scroll").size_full().overflow_y_scroll().flex().justify_center().child(
                    div()
                        .w_full()
                        .max_w(px(880.))
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE[9]))
                        .px(px(theme::SPACE[9]))
                        .py(px(56.))
                        .child(brand_header(p))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(theme::SPACE[4]))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(theme::SPACE[4]))
                                        .child(self.start_tile("w-new", "file-plus", crate::i18n::t("New File"), NewFile, p, cx))
                                        .child(self.start_tile("w-open", "folder", crate::i18n::t("Open Folder…"), OpenFolder, p, cx)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(theme::SPACE[4]))
                                        .child(self.start_tile("w-clone", "git-branch", crate::i18n::t("Clone Repository…"), CloneRepository, p, cx))
                                        .child(self.start_tile("w-ssh", "server", crate::i18n::t("Connect over SSH"), ShowRemote, p, cx)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(56.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(heading(crate::i18n::t("Recent")))
                                        .when(self.persist.recent.is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .px(px(theme::SPACE[4]))
                                                    .text_color(c(p.text_muted))
                                                    .child(crate::i18n::t("Nothing yet: open a folder to see it here.")),
                                            )
                                        })
                                        .children(recent_rows)
                                        .when(more_recent, |d| {
                                            d.child(
                                                div()
                                                    .id("w-more")
                                                    .px(px(theme::SPACE[4]))
                                                    .h(px(28.))
                                                    .flex()
                                                    .items_center()
                                                    .cursor_pointer()
                                                    .text_color(c(p.accent))
                                                    .hover(|d| d.underline())
                                                    .child(crate::i18n::t("More…"))
                                                    .on_click(cx.listener(|this, _, window, cx| this.show_project_switcher(window, cx))),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .w(px(280.))
                                        .flex_none()
                                        .flex()
                                        .flex_col()
                                        .gap(px(theme::SPACE[7]))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .child(heading(crate::i18n::t("Customize")))
                                                .child(self.start_row("w-settings", "settings", crate::i18n::t("Settings"), OpenSettingsPage, p, cx))
                                                .child(self.start_row("w-themes", "palette", crate::i18n::t("Color Theme"), SelectTheme, p, cx))
                                                .child(self.start_row("w-icons", "folder-tree", crate::i18n::t("File Icon Theme"), SelectIconTheme, p, cx))
                                                .child(self.start_row("w-keys", "keyboard", crate::i18n::t("Keyboard Shortcuts"), OpenKeybindings, p, cx)),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .child(heading(crate::i18n::t("Learn")))
                                                .children(learn_rows.into_iter().map(|r| r.px(px(theme::SPACE[4])))),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(theme::SPACE[4]))
                                .text_size(px(text::XS.0))
                                .text_color(c(p.text_muted))
                                .child(icon("lightbulb", 14., p.warning))
                                .child(div().flex_1().min_w_0().child(tip)),
                        ),
                ),
            )
            .into_any_element()
    }

    /// "Clone Git Repository…": URL, then target folder; the folder opens once cloned.
    pub(super) fn clone_repository(&mut self, _: &CloneRepository, window: &mut Window, cx: &mut Context<Self>) {
        self.show_input(InputPurpose::CloneUrl, crate::i18n::t("Next"), crate::i18n::t("Repository URL (https://… or git@host:owner/repo)"), "", window, cx);
    }

    pub(super) fn ask_clone_target(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        let name = repo_name(&url);
        if name.is_empty() {
            self.notice = Some(Notice::Error(crate::i18n::t("That does not look like a repository address.").into()));
            cx.notify();
            return;
        }
        let initial = format!("~/{name}");
        self.show_input(InputPurpose::CloneTarget { url }, crate::i18n::t("Clone"), crate::i18n::t("Folder to clone into"), &initial, window, cx);
    }

    pub(super) fn run_clone(&mut self, url: String, target: String, cx: &mut Context<Self>) {
        let host = self.remote.host.clone();
        self.notice = Some(Notice::Info(crate::i18n::tf("Cloning {}…", &[&url]).into()));
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            // No shell here, so expand "~" to the home folder ourselves.
            let target = match target.strip_prefix('~') {
                Some(rest) => {
                    let home = if host.is_remote() {
                        host.exec("printenv", &["HOME"], None, None).ok().filter(|o| o.ok()).map(|o| o.stdout_str().trim().to_string())
                    } else {
                        dirs::home_dir().map(|h| h.to_string_lossy().into_owned())
                    };
                    match home {
                        Some(h) => format!("{}{rest}", h.trim_end_matches('/')),
                        None => return Err(crate::i18n::t("Could not find the home folder; type an absolute path.").to_string()),
                    }
                }
                None => target,
            };
            let out = host.exec("git", &["clone", "--", &url, &target], None, None).map_err(|e| e.to_string())?;
            if out.ok() { Ok(target) } else { Err(out.stderr_str().lines().last().unwrap_or(crate::i18n::t("git clone failed")).to_string()) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(path) => {
                    this.notice = Some(Notice::Info(crate::i18n::t("Cloned. Opening the folder…").into()));
                    this.set_root(path, cx);
                    cx.notify();
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Clone failed: {}", &[&e]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Workbench {
    /// The path if the open folder is on this machine (`None` in an SSH workspace: paths coming from the system belong to this machine).
    pub(crate) fn local_folder(&self, cx: &gpui::App) -> Option<String> {
        if self.remote.host.is_remote() { None } else { self.root(cx) }
    }

    /// File arriving via Finder / Dock / `open -a`: if a folder is open (local or remote) the project stays put and the file opens
    /// as a tab from this machine; with no folder, the file's folder opens like on the command line.
    pub(crate) fn open_external_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if crate::workspace_file::is_workspace_file(path) || self.root(cx).is_none() {
            return self.open_path(path, window, cx);
        }
        let local = self.remote.local_host.clone();
        self.open_file_on(local, path.to_string(), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_names_come_from_the_address() {
        assert_eq!(repo_name("https://github.com/doancan/codeyard.git"), "codeyard");
        assert_eq!(repo_name("git@github.com:owner/repo"), "repo");
        assert_eq!(repo_name("https://host/a/b/"), "b");
        assert_eq!(repo_name("  /tmp/local.git "), "local");
    }

    #[test]
    fn brand_colors_differ_per_theme_and_stay_readable() {
        let (dark, light) = (brand_colors(&theme::DARK), brand_colors(&theme::LIGHT));
        assert_ne!(dark, light, "light and dark themes get their own logo colors");
        for (name, (tile, edge, mark)) in [("dark", dark), ("light", light)] {
            assert!(theme::themes::contrast(mark, tile) >= 3.0, "{name}: the mark must stand out from its tile ({:.2})", theme::themes::contrast(mark, tile));
            assert_ne!(tile, edge, "{name}");
        }
    }

    #[test]
    fn tips_rotate_daily() {
        assert_ne!(tip_of_the_day(0), tip_of_the_day(1));
        assert_eq!(tip_of_the_day(0), tip_of_the_day(TIPS.len() as u64));
        assert!(TIPS.iter().all(|t| !t.is_empty()));
    }
}
