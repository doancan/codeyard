//! Session library controls and explicit account or agent handoffs.

use std::collections::BTreeSet;

use super::*;
use crate::input::{FieldEvent, TextField};

#[derive(Default)]
pub(super) struct Library {
    pub(super) field: Option<Entity<TextField>>,
    pub(super) query: String,
    pub(super) pins: BTreeSet<String>,
}

impl Library {
    pub(super) fn matches(&self, session: &Session) -> bool {
        self.query.is_empty() || session.title.to_lowercase().contains(&self.query) || session.id.to_lowercase().contains(&self.query)
    }
}

#[derive(Clone)]
pub(super) struct Handoff {
    project: Project,
    profile: u32,
    target: u32,
    mode: HistoryMode,
    session: Session,
}

pub(super) fn section(label: &'static str, p: &Palette) -> Div {
    div().px(px(theme::SPACE[3])).pt(px(theme::SPACE[2])).text_size(px(text::XS.0)).font_weight(FontWeight::SEMIBOLD).text_color(c(p.text_muted)).child(label)
}

fn pins_key(state: &AgentPanel) -> Option<String> {
    let project = state.project.as_ref()?;
    Some(match state.mode {
        HistoryMode::Shared => format!("{}.shared.pins", project.state_key()),
        HistoryMode::Private => format!("{}.{}.pins", project.state_key(), state.profile),
    })
}

impl Workbench {
    pub(super) fn ensure_agent_search(&mut self, cx: &mut Context<Self>) {
        if self.chrome.agents.library.field.is_some() {
            return;
        }
        let field = cx.new(|cx| TextField::new(t("Search sessions…"), cx));
        self._subs.push(cx.subscribe(&field, |this, field, event: &FieldEvent, cx| {
            if matches!(event, FieldEvent::Changed) {
                this.chrome.agents.library.query = field.read(cx).text().trim().to_lowercase();
                cx.notify();
            }
        }));
        self.chrome.agents.library.field = Some(field);
    }

    pub(super) fn load_agent_pins(&mut self) {
        let key = pins_key(&self.chrome.agents);
        self.chrome.agents.library.pins =
            key.and_then(|key| self.persist.state.as_ref()?.get(&key)).and_then(|value| serde_json::from_str(&value).ok()).unwrap_or_default();
    }

    pub(in crate::workbench) fn toggle_agent_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        let state = &self.chrome.agents;
        if !state.sessions.iter().any(|session| session.id == id) {
            return;
        }
        let Some(key) = pins_key(state) else { return };
        let mut pins = state.library.pins.clone();
        if !pins.remove(id) {
            pins.insert(id.to_owned());
        }
        let Ok(value) = serde_json::to_string(&pins) else { return };
        if self.save_agent_value(&key, &value, cx) {
            self.chrome.agents.library.pins = pins;
            cx.notify();
        }
    }

    pub(super) fn agent_session_row(&self, session: &Session, compact: bool, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let state = &self.chrome.agents;
        let selected = state.selected.as_deref() == Some(&session.id);
        let running = selected && state.running.is_some();
        let busy = state.running.is_some() || state.transferring;
        let pinned = state.library.pins.contains(&session.id);
        let scope = if !state.provider.uses_local_profiles() {
            t("System account").to_owned()
        } else if state.mode == HistoryMode::Shared {
            t("Shared sessions").to_owned()
        } else {
            profile_label(state.profile)
        };
        let description = format!("{} · {scope}", state.provider.label());
        let id = session.id.clone();
        let open = panel_button(SharedString::from(format!("agent-session-{id}")), p)
            .flex_1()
            .min_w_0()
            .border_color(c(if selected { p.border } else { p.surface }))
            .when(selected, |d| d.bg(c(p.surface_active)))
            .py(px(theme::SPACE[2]))
            .child(icon(if pinned { "pin" } else { "history" }, 13., if selected { p.accent } else { p.text_muted }))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().truncate().font_weight(FontWeight::MEDIUM).child(session.title.clone()))
                    .child(div().truncate().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(description.clone())),
            )
            .when(running, |d| d.child(div().size(px(6.)).rounded_full().bg(c(p.accent)).flex_none()));
        let open = if busy {
            open.tab_stop(false).cursor_default()
        } else {
            let control = Control::new(t("Agents"), format!("agent-session-{id}"), accesskit::Role::Button, session.title.clone())
                .description(description)
                .selected(selected);
            let id = session.id.clone();
            self.pressable(open, control, cx, move |this, w, cx| this.start_agent(Start::Resume(id.clone()), w, cx))
        };
        let mut row = div().flex().items_center().min_w_0().gap(px(theme::SPACE[1])).when(compact, |d| d.w(px(230.)).flex_none()).child(open);
        if !state.transferring && !state.stopping && (!busy || selected) {
            let key = format!("agent-session-menu-{id}");
            let menu = ui::icon_button(SharedString::from(key.clone()), "ellipsis", t("Session actions"), p)
                .size(px(27.))
                .tab_index(0)
                .focus(|d| d.bg(c(p.surface_active)));
            row = row.child(self.pressable(
                menu,
                Control::new(t("Agents"), key, accesskit::Role::Button, tf("Session actions for {}", &[&session.title])),
                cx,
                move |this, window, cx| this.open_agent_session_menu(&id, window, cx),
            ));
        }
        row
    }

    fn open_agent_session_menu(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.chrome.agents;
        let Some(session) = state.sessions.iter().find(|session| session.id == id) else { return };
        let available = state.provider == Provider::Claude || session.path.is_some();
        let mut items = vec![
            MenuItem::new(if state.library.pins.contains(id) { t("Unpin session") } else { t("Pin session") }),
            MenuItem::new(t("Copy recent messages")).enabled(available),
        ];
        let mut commands = vec![MenuCmd::AgentPin, MenuCmd::AgentCopyContext];
        for other in Provider::ALL.into_iter().filter(|provider| *provider != state.provider) {
            items.push(MenuItem::new(&tf("Transfer context to {}…", &[&other.label()])).enabled(available && state.running.is_none()));
            commands.push(MenuCmd::AgentContinueIn(other));
        }
        for (index, profile) in (0..state.profile_count).filter(|profile| *profile != state.profile).enumerate() {
            let item = MenuItem::new(&tf("Continue with {}…", &[&profile_label(profile)])).enabled(available);
            items.push(if index == 0 { item.separator() } else { item });
            commands.push(MenuCmd::AgentHandoff(profile));
        }
        let key = format!("agent-session-menu-{id}");
        let position = crate::a11y_controls::bounds_of(cx, window, t("Agents"), &key).map(|b| b.bottom_right()).unwrap_or_else(|| window.mouse_position());
        self.show_menu(items, commands, Vec::new(), MenuTarget { path: id.to_owned(), is_dir: false, tab: None }, position, window, cx);
        if let Some(menu) = &self.menu {
            menu.menu.update(cx, |menu, _| menu.set_anchor(gpui::Corner::TopRight));
        }
    }

    pub(in crate::workbench) fn copy_agent_context(&mut self, id: &str, target: Option<Provider>, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.chrome.agents;
        let Some(project) = state.project.clone() else { return };
        let Some(session) = state.sessions.iter().find(|session| session.id == id).cloned() else { return };
        let (profile, mode) = (state.profile, state.mode);
        let source = project.clone();
        let data = cx.background_executor().spawn(async move {
            let path = source.transcript(profile, mode, &session)?;
            super::super::agent_transfer::context(&path, source.provider, &session.title)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = data.await;
            this.update_in(cx, |this, _, cx| match result {
                Ok(context) => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(context));
                    let state = &this.chrome.agents;
                    if let Some(target) = target
                        && state.running.is_none()
                        && !state.transferring
                        && state.project.as_ref() == Some(&project)
                        && state.profile == profile
                        && state.mode == mode
                        && this.chrome.bottom == Bottom::Agents
                    {
                        this.select_agent_provider(target, cx);
                        this.notify(NoticeSource::App, Notice::Info(t("Context copied. Paste it into a new session when ready.").into()));
                    } else {
                        this.notify(NoticeSource::App, Notice::Info(t("Recent messages copied.").into()));
                    }
                }
                Err(error) => this.agent_problem(tf("Could not copy session context: {}", &[&error]), cx),
            })
            .ok();
        })
        .detach();
    }

    pub(in crate::workbench) fn request_agent_handoff(&mut self, id: &str, target: u32, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.chrome.agents;
        if state.stopping
            || state.transferring
            || target == state.profile
            || target >= state.profile_count
            || (state.running.is_some() && state.selected.as_deref() != Some(id))
        {
            return;
        }
        let Some(project) = state.project.clone() else { return };
        let Some(session) = state.sessions.iter().find(|session| session.id == id).cloned() else { return };
        let handoff = Handoff { project, profile: state.profile, target, mode: state.mode, session };
        let detail = if state.running.is_some() {
            t("The running terminal will stop. The selected account will open the last saved conversation.")
        } else {
            t("Codeyard will open the last saved conversation on the selected account.")
        };
        let answer = crate::question::Question::new(gpui::PromptLevel::Info, tf("Continue this session with {}?", &[&profile_label(target)]))
            .detail(detail)
            .button(t("Continue session"))
            .cancel(t("Cancel"))
            .ask(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Some(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                let state = &mut this.chrome.agents;
                if !handoff.matches(state) || state.stopping || state.transferring {
                    return;
                }
                if state.running.is_some() {
                    if state.selected.as_deref() != Some(handoff.session.id.as_str()) {
                        return;
                    }
                    state.handoff = Some(handoff);
                    this.stop_agent_terminal(cx);
                } else {
                    this.begin_agent_handoff(handoff, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn begin_agent_handoff(&mut self, handoff: Handoff, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.agents.transferring = true;
        let request = handoff.clone();
        let transfer =
            cx.background_executor().spawn(async move { request.project.transfer_session(request.profile, request.target, request.mode, &request.session) });
        cx.spawn_in(window, async move |this, cx| {
            let result = transfer.await;
            this.update_in(cx, |this, window, cx| {
                let state = &mut this.chrome.agents;
                if !state.transferring || !handoff.matches(state) {
                    return;
                }
                state.transferring = false;
                match result {
                    Ok(session) => {
                        if this.apply_agent_profile(handoff.target, cx) {
                            let state = &mut this.chrome.agents;
                            let id = session.id.clone();
                            if !state.sessions.iter().any(|row| row.id == id) {
                                state.sessions.insert(0, session);
                            }
                            this.start_agent(Start::Resume(id), window, cx);
                        }
                    }
                    Err(error) => this.agent_problem(tf("Could not continue the session on that account: {}", &[&error]), cx),
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Handoff {
    fn matches(&self, state: &AgentPanel) -> bool {
        state.project.as_ref() == Some(&self.project) && state.profile == self.profile && state.mode == self.mode
    }
}
