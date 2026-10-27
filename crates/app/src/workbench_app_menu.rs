//! In-window application menu for platforms without a native global menu bar.

use super::*;

pub(super) const MENU_BAR_H: f32 = 28.;

impl Workbench {
    pub(super) fn application_menu_bottom(&self, window: &Window) -> f32 {
        MENU_BAR_H + if cfg!(target_os = "windows") && !window.is_fullscreen() { super::view_editor_glue::WINDOWS_TITLEBAR_H } else { 0. }
    }

    pub(super) fn application_menu_bar(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.menu.as_ref().and_then(|m| m.app_path.as_ref()).and_then(|p| p.first()).copied();
        div()
            .id("application-menu-bar")
            .debug_selector(|| "application-menu-bar".into())
            .h(px(MENU_BAR_H))
            .flex_none()
            .flex()
            .items_center()
            .px(px(theme::SPACE[2]))
            .bg(c(p.surface))
            .border_b_1()
            .border_color(c(p.border))
            .text_size(px(text::XS.0))
            .children(crate::menus::desktop_menus(&self.persist.recent).into_iter().enumerate().map(|(index, menu)| {
                div()
                    .id(("application-menu", index))
                    .debug_selector(move || format!("application-menu-{index}"))
                    .h_full()
                    .px(px(theme::SPACE[4]))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .when(active == Some(index), |d| d.bg(c(p.surface_active)))
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(menu.name)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                            let same = this.menu.as_ref().and_then(|m| m.app_path.as_ref()).is_some_and(|p| p.first() == Some(&index));
                            this.close_menu(window, cx);
                            if !same {
                                this.open_application_menu(vec![index], gpui::point(ev.position.x, px(this.application_menu_bottom(window))), window, cx);
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, ev: &gpui::MouseMoveEvent, window, cx| {
                        let current = this.menu.as_ref().and_then(|m| m.app_path.as_ref()).and_then(|p| p.first()).copied();
                        if current.is_some_and(|i| i != index) {
                            this.close_menu(window, cx);
                            this.open_application_menu(vec![index], gpui::point(ev.position.x, px(this.application_menu_bottom(window))), window, cx);
                        }
                    }))
            }))
    }

    pub(super) fn open_application_menu(&mut self, path: Vec<usize>, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&index) = path.first() else { return };
        let Some(mut menu) = crate::menus::desktop_menus(&self.persist.recent).into_iter().nth(index) else { return };
        for &child in &path[1..] {
            let Some(next) = menu.items.into_iter().filter_map(|i| if let gpui::MenuItem::Submenu(m) = i { Some(m) } else { None }).nth(child) else { return };
            menu = next;
        }
        self.close_menu(window, cx);
        let (mut items, mut commands, mut actions) = (Vec::new(), Vec::new(), Vec::new());
        let (mut separator, mut submenu_index) = (false, 0);
        for entry in menu.items {
            match entry {
                gpui::MenuItem::Separator => separator = true,
                gpui::MenuItem::Action { name, action, .. } => {
                    let shortcut = Command { label: String::new(), run: Run::Action(action.boxed_clone()) }.shortcut(cx);
                    let available = window.is_action_available(action.as_ref(), cx) || cx.is_action_available(action.as_ref());
                    let mut item = MenuItem::new(&name).shortcut(shortcut).enabled(available);
                    item.separator = separator;
                    items.push(item);
                    commands.push(MenuCmd::Dispatch(actions.len()));
                    actions.push(action);
                    separator = false;
                }
                gpui::MenuItem::Submenu(menu) => {
                    let mut item = MenuItem::new(&menu.name);
                    item.submenu = true;
                    item.separator = separator;
                    items.push(item);
                    commands.push(MenuCmd::AppSubmenu(submenu_index));
                    submenu_index += 1;
                    separator = false;
                }
                gpui::MenuItem::SystemMenu(_) => {}
            }
        }
        let target = MenuTarget { path: String::new(), is_dir: false, tab: None };
        self.show_menu(items, commands, actions, target, position, window, cx);
        self.menu.as_mut().unwrap().app_path = Some(path);
    }

    pub(super) fn open_application_menu_by_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_application_menu(vec![index], gpui::point(px(4.), px(self.application_menu_bottom(window))), window, cx);
    }

    pub(super) fn move_application_menu(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.menu.as_ref().filter(|m| m.app_path.is_some()) else { return };
        let mut path = state.app_path.clone().unwrap();
        let position = state.position;
        if !forward && path.len() > 1 {
            path.pop();
        } else {
            let n = crate::menus::desktop_menus(&self.persist.recent).len();
            path = vec![(path[0] + if forward { 1 } else { n - 1 }) % n];
        }
        self.open_application_menu(path, position, window, cx);
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use crate::qa_workbench_tests::{open, open_in, project, setup};
    use gpui::{Modifiers, TestAppContext};

    #[gpui::test]
    fn windows_application_menu_opens_from_header_and_keyboard(cx: &mut TestAppContext) {
        let (wb, cx) = setup(cx);
        cx.run_until_parked();
        let menu_button = cx.debug_bounds("application-menu-0").unwrap().center();
        cx.simulate_click(menu_button, Modifiers::none());
        assert_eq!(wb.read_with(cx, |w, _| w.menu.as_ref().unwrap().app_path.clone()), Some(vec![0]));
        assert_eq!(wb.read_with(cx, |w, _| w.menu.as_ref().unwrap().position.y), cx.debug_bounds("application-menu-bar").unwrap().bottom());
        assert!(wb.read_with(cx, |w, cx| w.menu_labels(cx).unwrap()).contains(&"Open Folder…".into()));
        cx.simulate_click(menu_button, Modifiers::none());
        assert!(wb.read_with(cx, |w, _| w.menu.is_none()));
        cx.simulate_keystrokes("alt-v");
        assert_eq!(wb.read_with(cx, |w, _| w.menu.as_ref().unwrap().app_path.clone()), Some(vec![3]));
        cx.simulate_keystrokes("right");
        assert_eq!(wb.read_with(cx, |w, _| w.menu.as_ref().unwrap().app_path.clone()), Some(vec![4]));
        cx.simulate_keystrokes("escape");
        assert!(wb.read_with(cx, |w, _| w.menu.is_none()));
    }

    #[gpui::test]
    fn windows_application_menu_dispatches_to_the_editor(cx: &mut TestAppContext) {
        let dir = project("windows-menu-editor");
        let (wb, cx) = setup(cx);
        open(&wb, &dir, cx);
        open_in(&wb, &dir.join("src/main.rs"), cx);
        let editor = wb.read_with(cx, |w, _| w.active_editor().unwrap().clone());
        wb.update_in(cx, |w, window, cx| w.open_application_menu_by_index(2, window, cx));
        wb.update(cx, |w, cx| w.select_menu_for_test("Select All", cx));
        cx.run_until_parked();
        assert!(editor.read_with(cx, |e, _| e.selection_len()).is_some_and(|n| n > 0));
        assert!(wb.read_with(cx, |w, _| w.menu.is_none()));
        assert!(cx.update(|window, cx| editor.read(cx).focus_handle(cx).is_focused(window)));
    }

    #[gpui::test]
    fn windows_application_menu_submenus_restore_focus(cx: &mut TestAppContext) {
        let dir = project("windows-menu-recent");
        let (wb, cx) = setup(cx);
        open(&wb, &dir, cx);
        wb.update_in(cx, |w, window, cx| w.open_application_menu_by_index(0, window, cx));
        wb.update(cx, |w, cx| w.select_menu_for_test("Open Recent", cx));
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |w, _| w.menu.as_ref().unwrap().app_path.clone()), Some(vec![0, 0]));
        assert!(wb.read_with(cx, |w, cx| w.menu_labels(cx).unwrap()).iter().any(|l| l.contains("windows-menu-recent")));
        cx.simulate_keystrokes("left");
        assert_eq!(wb.read_with(cx, |w, _| w.menu.as_ref().unwrap().app_path.clone()), Some(vec![0]));
        cx.simulate_keystrokes("escape");
        assert!(wb.read_with(cx, |w, _| w.menu.is_none()));
    }
}
