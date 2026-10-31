//! Workbench: the Remote Explorer sidebar view. SSH hosts from `~/.ssh/config` (with their recent folders and connection
//! status), running containers and WSL distributions, and the forwarded ports of the current connection. Each row offers
//! the actions that fit it: connect here or in a new window, open or forget a folder, edit, test or remove a host.
//! The workspace switcher (⌃R) stays the quick keyboard path to the same targets.

use std::collections::{HashMap, HashSet};

use gpui::{AnyElement, Context, MouseButton, SharedString, Window, div, px};

use super::projects::ConnSummary;
use super::*;

#[path = "remote_tree.rs"]
pub(super) mod tree;
use tree::{Group, HostStatus, RemoteRow, RemoteTreeInput, Targets};

gpui::actions!(workbench, [ShowRemoteExplorer]);

/// State of the Remote Explorer view (kept while the window lives).
#[derive(Default)]
pub(super) struct RemoteView {
    /// Groups and hosts whose open/closed state differs from the default.
    toggled: HashSet<String>,
    records: HashMap<String, state::HostRecord>,
    /// "Test Connection" results this session.
    pub(super) probes: HashMap<String, Result<(), String>>,
    containers: Option<Targets>,
    wsl: Option<Targets>,
    pub(super) scroll: gpui::UniformListScrollHandle,
}

/// A command from a host row's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemoteMenuCmd {
    Connect,
    ConnectNewWindow,
    Disconnect,
    Edit,
    Test,
    KeySetup,
    Remove,
}

impl Workbench {
    /// The connection as the switcher and the Remote Explorer show it.
    pub(super) fn conn_summary(&self) -> ConnSummary {
        match &self.remote.conn {
            Conn::Local => ConnSummary::Local,
            Conn::Connecting { alias, .. } => ConnSummary::Connecting(alias.clone()),
            Conn::Connected { alias } if self.remote.host.is_remote() => ConnSummary::Connected(alias.clone()),
            Conn::Connected { .. } => ConnSummary::Local,
            Conn::Reconnecting { alias, .. } => ConnSummary::Reconnecting(alias.clone()),
            Conn::Failed { alias, message } => ConnSummary::Failed(alias.clone(), message.to_string()),
        }
    }

    /// SSH hosts to offer: (alias, `user@host`), without the git forges people keep in their config.
    pub(super) fn ssh_host_rows(&self) -> Vec<(String, Option<String>)> {
        let mut hosts: Vec<(String, Option<String>)> = self
            .remote
            .ssh_hosts
            .iter()
            .filter(|h| h.alias != "github.com" && h.alias != "gitlab.com")
            .map(|h| {
                let detail = match (&h.user, &h.hostname) {
                    (Some(u), Some(hn)) => Some(format!("{u}@{hn}")),
                    (None, Some(hn)) => Some(hn.clone()),
                    _ => None,
                };
                (h.alias.clone(), detail)
            })
            .collect();
        if self.dev {
            hosts.push((LOCAL_AGENT_ALIAS.into(), Some(crate::i18n::t("local agent, protocol test").into())));
        }
        hosts
    }

    pub(super) fn show_remote_explorer(&mut self, _: &ShowRemoteExplorer, _: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Remote);
        self.refresh_remote_view(cx);
    }

    /// Re-reads the hosts (the config may have changed) and their saved status; lists open groups' targets again.
    pub(super) fn refresh_remote_view(&mut self, cx: &mut Context<Self>) {
        self.remote.ssh_hosts = host::ssh::list_hosts();
        self.remote.view.records = self.persist.state.as_ref().map(|s| s.host_records()).unwrap_or_default();
        for g in [Group::Containers, Group::Wsl] {
            if self.remote.view.toggled.contains(g.key()) {
                self.list_targets(g, cx);
            }
        }
        cx.notify();
    }

    /// Lists running containers or WSL distributions in the background.
    fn list_targets(&mut self, group: Group, cx: &mut Context<Self>) {
        let wsl = group == Group::Wsl;
        if !wsl && !cx.global::<Settings>().docker_enabled {
            return;
        }
        if wsl {
            self.remote.view.wsl = None;
        } else {
            self.remote.view.containers = None;
        }
        let task = cx.background_executor().spawn(async move {
            if wsl {
                host::ssh::list_wsl().map(|v| v.into_iter().map(|d| (format!("wsl:{d}"), d, crate::i18n::t("WSL distribution").to_string())).collect())
            } else {
                host::ssh::list_containers().map(|v| v.into_iter().map(|(n, i)| (format!("docker:{n}"), n, i)).collect())
            }
        });
        cx.spawn(async move |this, cx| {
            let listed: Targets = task.await.map_err(|e| e.lines().next().unwrap_or_default().to_string());
            this.update(cx, |this, cx| {
                if wsl {
                    this.remote.view.wsl = Some(listed);
                } else if cx.global::<Settings>().docker_enabled {
                    this.remote.view.containers = Some(listed);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn toggle_remote(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.remote.view.toggled.remove(&key) {
            self.remote.view.toggled.insert(key.clone());
        }
        // A target group listed when it is first opened.
        for g in [Group::Containers, Group::Wsl] {
            let loaded = if g == Group::Wsl { self.remote.view.wsl.is_some() } else { self.remote.view.containers.is_some() };
            if key == g.key() && self.remote.view.toggled.contains(&key) && !loaded {
                self.list_targets(g, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn remote_rows(&self, cx: &App) -> Vec<RemoteRow> {
        let hosts = self.ssh_host_rows();
        let root = self.root(cx);
        let ports = self.ssh_alias().map(|alias| ports_glue::port_rows(&self.ports.set, &self.ports.listening, &alias));
        tree::build_remote_tree(&RemoteTreeInput {
            hosts: &hosts,
            recent: &self.persist.recent,
            pinned: &self.persist.pinned,
            conn: self.conn_summary(),
            records: &self.remote.view.records,
            probes: &self.remote.view.probes,
            containers: self.remote.view.containers.as_ref(),
            wsl: self.remote.view.wsl.as_ref(),
            show_wsl: cfg!(windows),
            show_containers: cx.global::<Settings>().docker_enabled,
            toggled: &self.remote.view.toggled,
            current_root: root.as_deref(),
            ports,
        })
    }

    /// Opens a target in a new window: a recent folder, or the host's folder picker.
    fn connect_in_new_window(&mut self, alias: String, path: Option<String>, cx: &mut Context<Self>) {
        let Some(w) = crate::open_workbench_window(cx) else { return };
        w.update(cx, move |wb, window, cx| match path {
            Some(path) => wb.open_recent(state::RecentWorkspace { host: Some(alias), path, last_opened_at: 0 }, window, cx),
            None => wb.connect(alias, window, cx),
        })
        .ok();
    }

    pub(super) fn connected_to(&self, alias: &str) -> bool {
        matches!(&self.remote.conn, Conn::Connected { alias: a } if a == alias) && self.remote.host.is_remote()
    }

    pub(super) fn run_remote_menu(&mut self, cmd: RemoteMenuCmd, alias: String, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            RemoteMenuCmd::Connect if self.connected_to(&alias) => self.open_remote_folder(window, cx),
            RemoteMenuCmd::Connect => self.connect(alias, window, cx),
            RemoteMenuCmd::ConnectNewWindow => self.connect_in_new_window(alias, None, cx),
            RemoteMenuCmd::Disconnect => self.disconnect(cx),
            RemoteMenuCmd::Edit => self.run_host_op(hosts::HostOp::Edit, alias, window, cx),
            RemoteMenuCmd::Test => self.run_host_op(hosts::HostOp::Test, alias, window, cx),
            RemoteMenuCmd::KeySetup => self.run_host_op(hosts::HostOp::KeySetup, alias, window, cx),
            RemoteMenuCmd::Remove => self.run_host_op(hosts::HostOp::Remove, alias, window, cx),
        }
    }

    pub(super) fn open_host_menu(&mut self, alias: &str, group: Group, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let connected = self.connected_to(alias);
        let mut items = vec![
            MenuItem::new(if connected { crate::i18n::t("Open Folder…") } else { crate::i18n::t("Connect in Current Window") }),
            MenuItem::new(crate::i18n::t("Connect in New Window")),
        ];
        let mut commands = vec![RemoteMenuCmd::Connect, RemoteMenuCmd::ConnectNewWindow];
        if connected {
            items.push(MenuItem::new(crate::i18n::t("Disconnect")));
            commands.push(RemoteMenuCmd::Disconnect);
        }
        if group == Group::Ssh && alias != LOCAL_AGENT_ALIAS {
            items.extend([
                MenuItem::new(crate::i18n::t("Edit Host…")).separator(),
                MenuItem::new(crate::i18n::t("Test Connection")),
                MenuItem::new(crate::i18n::t("Set Up Key Login…")),
                MenuItem::new(crate::i18n::t("Remove Host…")).separator(),
            ]);
            commands.extend([RemoteMenuCmd::Edit, RemoteMenuCmd::Test, RemoteMenuCmd::KeySetup, RemoteMenuCmd::Remove]);
        }
        let target = MenuTarget { path: alias.to_string(), is_dir: false, tab: None };
        self.show_menu(items, commands.into_iter().map(MenuCmd::Remote).collect(), Vec::new(), target, position, window, cx);
    }

    pub(super) fn remote_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.remote_rows(cx);
        // Same look as `view_header`, with the view's buttons on the right.
        let header = div()
            .h(px(layout::VIEW_HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .pl(px(theme::SPACE[6]))
            .pr(px(theme::SPACE[3]))
            .text_size(px(text::XXS.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(c(p.text_muted))
            .child(crate::i18n::upper(crate::i18n::t("Remote Explorer")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[1]))
                    .child(
                        ui::icon_button("remote-add", "plus", crate::i18n::t("Add SSH Host…"), p)
                            .relative()
                            .child(a11y::part_frame(a11y::Part::Control("remote-add"), cx))
                            .on_click(cx.listener(|this, _, w, cx| this.add_ssh_host(&AddSshHost, w, cx))),
                    )
                    .child(
                        ui::icon_button("remote-config", "settings", crate::i18n::t("Open SSH Config File"), p)
                            .relative()
                            .child(a11y::part_frame(a11y::Part::Control("remote-config"), cx))
                            .on_click(cx.listener(|this, _, w, cx| this.edit_ssh_config(&EditSshConfig, w, cx))),
                    )
                    .child(
                        ui::icon_button("remote-refresh", "refresh-cw", crate::i18n::t("Refresh"), p)
                            .relative()
                            .child(a11y::part_frame(a11y::Part::Control("remote-refresh"), cx))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_remote_view(cx))),
                    ),
            );
        let p = *p;
        div()
            .id("remote-view")
            .flex()
            .flex_col()
            .size_full()
            .text_size(px(text::SM.0))
            .child(header)
            .child(
                // The list fills the view, so rows are as wide as the sidebar.
                div().flex_1().min_h_0().child(
                    self.row_list(
                        a11y::RowFrame::Remote,
                        gpui::uniform_list(
                            "remote-rows",
                            rows.len(),
                            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                                range
                                    .filter_map(|ix| {
                                        let row = this.remote_row(ix, rows.get(ix)?, &p, cx);
                                        Some(this.nav_row(a11y::RowFrame::Remote, ix, row, window, cx))
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(self.remote.view.scroll.clone())
                        .size_full(),
                        cx,
                    ),
                ),
            )
            .into_any_element()
    }

    fn remote_row(&self, ix: usize, row: &RemoteRow, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let base = |depth: f32| {
            div()
                .id(("remote-row", ix))
                .w_full()
                .h(px(layout::ROW_H))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .pl(px(theme::SPACE[3] + depth * layout::TREE_INDENT))
                .pr(px(theme::SPACE[3]))
                .relative()
                .group(SharedString::from(format!("remote-row-{ix}")))
                .hover(|d| d.bg(c(p.surface_hover)))
        };
        let group_name = SharedString::from(format!("remote-row-{ix}"));
        // Buttons that show over the right end of the row while it is hovered (they take no room otherwise).
        let hover_actions = || {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .pl(px(theme::SPACE[2]))
                .pr(px(theme::SPACE[3]))
                .flex()
                .items_center()
                .gap(px(theme::SPACE[1]))
                .bg(c(p.surface_hover))
                .invisible()
                .group_hover(group_name.clone(), |d| d.visible())
        };
        let chevron = |open: bool| icon(if open { "chevron-down" } else { "chevron-right" }, layout::ICON - 2., p.text_muted);
        // The name takes its own width (shrinking when the sidebar is narrow); a spacer pushes the status and buttons to
        // the right edge.
        let label = |s: SharedString| div().flex_shrink_0().max_w(px(180.)).overflow_hidden().whitespace_nowrap().child(s);
        let spacer = || div().flex_1().min_w(px(theme::SPACE[2]));
        // Secondary text gives way first when the sidebar is narrow.
        let muted = |s: SharedString| {
            div().flex_shrink().min_w_0().overflow_hidden().whitespace_nowrap().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(s)
        };
        match row {
            RemoteRow::Group { group, count, expanded } => {
                let key = group.key().to_string();
                let name = match group {
                    Group::Ssh => crate::i18n::t("SSH Hosts"),
                    Group::Containers => crate::i18n::t("Containers"),
                    Group::Wsl => crate::i18n::t("WSL Targets"),
                };
                base(0.)
                    .cursor_pointer()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(chevron(*expanded))
                    .child(label(crate::i18n::upper(name).into()).text_size(px(text::XXS.0)).text_color(c(p.text_muted)))
                    .child(spacer())
                    .when(*count > 0, |d| d.child(muted(count.to_string().into())))
                    .when(*group != Group::Ssh, |d| {
                        let g = *group;
                        d.child(hover_actions().child(ui::icon_button(("remote-relist", ix), "refresh-cw", crate::i18n::t("Refresh"), p).on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.list_targets(g, cx);
                            }),
                        )))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_remote(key.clone(), cx)))
                    .into_any_element()
            }
            RemoteRow::Host { group, alias, detail, status, expanded } => {
                let (dot, tip): (u32, SharedString) = match status {
                    HostStatus::Connected => (p.success, crate::i18n::t("Connected").into()),
                    HostStatus::Connecting | HostStatus::Reconnecting => (p.warning, crate::i18n::t("Connecting…").into()),
                    HostStatus::Failed(why) => (p.danger, why.clone().into()),
                    HostStatus::Reachable(_) => (p.text_muted, crate::i18n::t("Connected before").into()),
                    HostStatus::Unknown => (p.border, crate::i18n::t("Not connected yet").into()),
                };
                let glyph = match group {
                    Group::Ssh => "server",
                    Group::Containers => "container",
                    Group::Wsl => "terminal",
                };
                let key = tree::host_key(alias);
                let connected = self.connected_to(alias);
                let (a1, a2, a3) = (alias.clone(), alias.clone(), alias.clone());
                let g = *group;
                base(1.)
                    .cursor_pointer()
                    .child(chevron(*expanded))
                    .child(icon(glyph, layout::ICON - 2., p.text_muted))
                    .child(label(alias.clone().into()))
                    .children(detail.clone().map(|d| muted(d.into())))
                    .child(spacer())
                    .child(
                        div()
                            .id(("remote-dot", ix))
                            .flex_none()
                            .size(px(8.))
                            .rounded_full()
                            .bg(c(dot))
                            .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into()),
                    )
                    .child(
                        hover_actions()
                            .child(a11y::row_button(
                                ui::icon_button(
                                    ("remote-connect", ix),
                                    if connected { "folder-open" } else { "plug" },
                                    if connected { crate::i18n::t("Open Folder…") } else { crate::i18n::t("Connect in Current Window") },
                                    p,
                                )
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    cx.stop_propagation();
                                    this.run_remote_menu(RemoteMenuCmd::Connect, a1.clone(), w, cx);
                                })),
                                a11y::RowFrame::Remote,
                                ix,
                                1,
                                cx,
                            ))
                            .child(a11y::row_button(
                                ui::icon_button(("remote-window", ix), "external-link", crate::i18n::t("Connect in New Window"), p).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.connect_in_new_window(a2.clone(), None, cx);
                                    },
                                )),
                                a11y::RowFrame::Remote,
                                ix,
                                2,
                                cx,
                            ))
                            .child(a11y::row_button(
                                ui::icon_button(("remote-more", ix), "ellipsis", crate::i18n::t("More Actions…"), p).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &gpui::MouseDownEvent, w, cx| {
                                        cx.stop_propagation();
                                        this.open_host_menu(&a3, g, ev.position, w, cx);
                                    }),
                                ),
                                a11y::RowFrame::Remote,
                                ix,
                                a11y::MENU_BUTTON,
                                cx,
                            )),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_remote(key.clone(), cx)))
                    .into_any_element()
            }
            RemoteRow::Folder { alias, path, current, pinned, last_opened } => {
                let recent = state::RecentWorkspace { host: Some(alias.clone()), path: path.clone(), last_opened_at: *last_opened };
                let (open_here, open_new, forget) = (recent.clone(), (alias.clone(), path.clone()), recent);
                base(2.)
                    .cursor_pointer()
                    .when(*current, |d| d.bg(c(p.surface_active)))
                    .child(icon(if *pinned { "pin" } else { "folder" }, layout::ICON - 2., p.text_muted))
                    .child(label(ui::file_name(path).to_string().into()))
                    .child(muted(path.clone().into()))
                    .child(spacer())
                    .child(
                        hover_actions()
                            .child(ui::icon_button(("remote-folder-window", ix), "external-link", crate::i18n::t("Open in New Window"), p).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.connect_in_new_window(open_new.0.clone(), Some(open_new.1.clone()), cx);
                                }),
                            ))
                            .child(ui::icon_button(("remote-folder-forget", ix), "x", crate::i18n::t("Remove from Recent"), p).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.forget_recent(forget.clone(), cx);
                                },
                            ))),
                    )
                    .on_click(cx.listener(move |this, _, w, cx| this.open_recent(open_here.clone(), w, cx)))
                    .into_any_element()
            }
            RemoteRow::NoFolders { .. } => base(2.).text_color(c(p.text_muted)).child(label(crate::i18n::t("No recent folders").into())).into_any_element(),
            RemoteRow::Empty { group, message } => {
                let text: SharedString = match (message, group) {
                    (Some(m), _) => m.clone().into(),
                    (None, Group::Ssh) => crate::i18n::t("No hosts in ~/.ssh/config. Add one with +.").into(),
                    (None, Group::Containers) => crate::i18n::t("No running containers.").into(),
                    (None, Group::Wsl) => crate::i18n::t("No WSL distributions found.").into(),
                };
                base(1.).text_color(c(p.text_muted)).child(label(text)).into_any_element()
            }
            RemoteRow::Loading { .. } => base(1.)
                .text_color(c(p.text_muted))
                .child(ui::spinner(("remote-loading", ix), layout::ICON - 2., p.text_muted))
                .child(label(crate::i18n::t("Loading…").into()))
                .into_any_element(),
            RemoteRow::PortsHeader { count } => base(0.)
                .cursor_pointer()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .border_t_1()
                .border_color(c(p.border))
                .child(label(crate::i18n::upper(crate::i18n::t("Forwarded Ports")).into()).text_size(px(text::XXS.0)).text_color(c(p.text_muted)))
                .child(spacer())
                .when(*count > 0, |d| d.child(muted(count.to_string().into())))
                .on_click(cx.listener(|this, _, w, cx| this.show_ports(&ShowPorts, w, cx)))
                .into_any_element(),
            RemoteRow::Port(r) => {
                let port = r.port;
                let active = r.state == ports_glue::RowState::Active;
                let url = r.url();
                base(1.)
                    .child(icon(if active { "plug" } else { "server" }, layout::ICON - 2., if active { p.success } else { p.text_muted }))
                    .child(div().flex_none().font_family(ui::MONO_FONT).child(port.to_string()))
                    .child(label(r.label.clone().unwrap_or_default().into()).text_color(c(p.text_muted)))
                    .children(r.address.clone().map(|a| muted(a.into())))
                    .child(spacer())
                    .child(
                        hover_actions()
                            .children(url.map(|url| {
                                ui::icon_button(("remote-port-open", ix), "external-link", crate::i18n::t("Open in Browser"), p)
                                    .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url)))
                            }))
                            .child(if r.state == ports_glue::RowState::Listening {
                                ui::icon_button(("remote-port-fwd", ix), "plug", crate::i18n::t("Forward Port"), p)
                                    .on_click(cx.listener(move |this, _, _, cx| this.forward_port(port, cx)))
                            } else {
                                ui::icon_button(("remote-port-stop", ix), "x", crate::i18n::t("Stop Forwarding"), p)
                                    .on_click(cx.listener(move |this, _, _, cx| this.stop_forward(port, cx)))
                            }),
                    )
                    .into_any_element()
            }
        }
    }

    /// Remote Explorer rows as text: `group:…`, `host:alias:status`, `folder:path[:current]`, `port:N`, …
    #[cfg(test)]
    pub fn remote_rows_for_test(&self, cx: &App) -> Vec<String> {
        self.remote_rows(cx)
            .into_iter()
            .map(|r| match r {
                RemoteRow::Group { group, expanded, .. } => format!("{}{}", group.key(), if expanded { "" } else { ":closed" }),
                RemoteRow::Host { alias, status, .. } => format!(
                    "host:{alias}:{}",
                    match status {
                        HostStatus::Connected => "connected",
                        HostStatus::Connecting | HostStatus::Reconnecting => "connecting",
                        HostStatus::Failed(_) => "failed",
                        HostStatus::Reachable(_) => "reachable",
                        HostStatus::Unknown => "unknown",
                    }
                ),
                RemoteRow::Folder { path, current, .. } => format!("folder:{path}{}", if current { ":current" } else { "" }),
                RemoteRow::NoFolders { alias } => format!("nofolders:{alias}"),
                RemoteRow::Empty { group, .. } => format!("empty:{}", group.key()),
                RemoteRow::Loading { group } => format!("loading:{}", group.key()),
                RemoteRow::PortsHeader { .. } => "ports".into(),
                RemoteRow::Port(p) => format!("port:{}", p.port),
            })
            .collect()
    }

    #[cfg(test)]
    pub fn toggle_remote_for_test(&mut self, key: &str, cx: &mut Context<Self>) {
        self.toggle_remote(key.into(), cx);
    }
}
