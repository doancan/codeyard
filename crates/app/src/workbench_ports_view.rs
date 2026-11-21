//! The PORTS panel: a table of forwards and of ports listening on the server (Port, Label, Forwarded Address, Running
//! Process, Origin), each with its actions and a "…" menu. Rows come from [`port_rows`], which the Remote view's
//! Forwarded Ports section uses too, so both always agree.

use gpui::{AnyElement, Context, MouseButton, SharedString, div, prelude::*, px};
use host::forward::LocalBind;

use super::super::{MenuCmd, MenuTarget, Workbench, a11y, center};
use crate::menu::MenuItem;
use crate::port_forwards::{ForwardSet, ForwardState, Origin, Protocol, failure_text};
use crate::ports::ListeningPort;
use crate::ui::{self, Palette, c, icon, layout, text};

/// A command from a PORTS row's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortMenuCmd {
    OpenBrowser,
    CopyAddress,
    SetLabel,
    ChangeLocal,
    Protocol(Protocol),
    /// Private (this computer only) or public (every network interface).
    Visibility(LocalBind),
    Retry,
    Stop,
    /// A listening port that is not forwarded yet.
    Forward,
    /// The server's port number of a row.
    CopyPort,
}

/// State of a row as shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowState {
    Active,
    Pending,
    Failed(String),
    Interrupted,
    /// Listening on the server, not forwarded.
    Listening,
}

/// One row of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortRow {
    pub port: u16,
    pub label: Option<String>,
    /// Local address while forwarded.
    pub address: Option<String>,
    pub process: Option<String>,
    pub origin: Option<Origin>,
    pub protocol: Protocol,
    /// Where the forward listens locally; `None` for a port that is not forwarded.
    pub bind: Option<LocalBind>,
    pub state: RowState,
    /// Something listens on the server port (a forward to a port nobody listens on yet is still shown).
    pub listening: bool,
}

impl PortRow {
    /// Browser address of an active forward (`None` for TCP).
    pub fn url(&self) -> Option<String> {
        let local = self.address.as_ref()?.rsplit_once(':')?.1.parse().ok()?;
        (self.state == RowState::Active).then(|| self.protocol.url(local)).flatten()
    }
}

/// Rows: forwards first by server port, then ports listening on the server that are not forwarded.
pub fn port_rows(set: &ForwardSet, listening: &[ListeningPort], alias: &str) -> Vec<PortRow> {
    let process = |port: u16| listening.iter().find(|l| l.port == port).and_then(|l| l.process.as_ref()).map(|p| format!("{} ({})", p.name, p.pid));
    let mut forwards: Vec<PortRow> = set
        .iter()
        .filter(|f| f.alias == alias)
        .map(|f| PortRow {
            port: f.remote,
            label: f.label.clone(),
            address: f.local.filter(|_| f.is_active()).map(|l| match f.bind {
                LocalBind::Loopback => format!("localhost:{l}"),
                LocalBind::All => format!("0.0.0.0:{l}"),
            }),
            process: process(f.remote),
            origin: Some(f.origin),
            protocol: f.protocol,
            bind: Some(f.bind),
            state: match &f.state {
                ForwardState::Active => RowState::Active,
                ForwardState::Pending => RowState::Pending,
                ForwardState::Interrupted => RowState::Interrupted,
                ForwardState::Failed(e) => RowState::Failed(failure_text(e, f.wanted_local)),
            },
            listening: listening.iter().any(|l| l.port == f.remote),
        })
        .collect();
    forwards.sort_by_key(|r| r.port);
    let free = listening.iter().filter(|l| set.by_remote(alias, l.port).is_none()).map(|l| PortRow {
        port: l.port,
        label: None,
        address: None,
        process: process(l.port),
        origin: None,
        protocol: Protocol::default(),
        bind: None,
        state: RowState::Listening,
        listening: true,
    });
    forwards.extend(free);
    forwards
}

/// The menu of a PORTS row: each item with the command it runs, in one list so that the two cannot drift apart.
pub fn port_menu(row: &PortRow) -> Vec<(MenuItem, PortMenuCmd)> {
    let t = crate::i18n::t;
    if row.state == RowState::Listening {
        return vec![(MenuItem::new(t("Forward Port")), PortMenuCmd::Forward), (MenuItem::new(t("Copy Port Number")), PortMenuCmd::CopyPort)];
    }
    let active = row.state == RowState::Active;
    vec![
        (MenuItem::new(t("Open in Browser")).enabled(row.url().is_some()), PortMenuCmd::OpenBrowser),
        (MenuItem::new(t("Copy Address")).enabled(active), PortMenuCmd::CopyAddress),
        (MenuItem::new(t("Copy Port Number")), PortMenuCmd::CopyPort),
        (MenuItem::new(t("Set Label…")).separator(), PortMenuCmd::SetLabel),
        (MenuItem::new(t("Change Local Port…")), PortMenuCmd::ChangeLocal),
        (
            MenuItem::new(t("Visibility: Private (this computer)")).checked(row.bind == Some(LocalBind::Loopback)).separator(),
            PortMenuCmd::Visibility(LocalBind::Loopback),
        ),
        (MenuItem::new(t("Visibility: Public (every network interface)")).checked(row.bind == Some(LocalBind::All)), PortMenuCmd::Visibility(LocalBind::All)),
        (MenuItem::new(t("Protocol: HTTP")).checked(row.protocol == Protocol::Http).separator(), PortMenuCmd::Protocol(Protocol::Http)),
        (MenuItem::new(t("Protocol: HTTPS")).checked(row.protocol == Protocol::Https), PortMenuCmd::Protocol(Protocol::Https)),
        (MenuItem::new(t("Protocol: TCP")).checked(row.protocol == Protocol::Tcp), PortMenuCmd::Protocol(Protocol::Tcp)),
        (MenuItem::new(t("Retry")).enabled(matches!(row.state, RowState::Failed(_))).separator(), PortMenuCmd::Retry),
        (MenuItem::new(t("Stop Forwarding")), PortMenuCmd::Stop),
    ]
}

/// Column widths (the process column takes the rest).
const W_PORT: f32 = 96.;
const W_LABEL: f32 = 140.;
const W_ADDRESS: f32 = 170.;
/// Wide enough for the longest label in the languages we ship ("Otomatik Yönlendirildi"); the table clips with no ellipsis.
const W_ORIGIN: f32 = 156.;
const W_ACTIONS: f32 = 132.;

impl Workbench {
    /// Right-click or "…" on a row.
    pub(in crate::workbench) fn open_port_menu(
        &mut self,
        row: &PortRow,
        position: gpui::Point<gpui::Pixels>,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let (items, commands): (Vec<MenuItem>, Vec<MenuCmd>) = port_menu(row).into_iter().map(|(item, cmd)| (item, MenuCmd::Port(cmd))).unzip();
        let target = MenuTarget { path: row.port.to_string(), is_dir: false, tab: None };
        self.show_menu(items, commands, Vec::new(), target, position, window, cx);
    }

    fn port_row(&self, ix: usize, row: &PortRow, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let port = row.port;
        let (glyph, color) = match &row.state {
            RowState::Active => ("plug", p.success),
            RowState::Pending => ("loader", p.text_muted),
            RowState::Interrupted => ("alert", p.warning),
            RowState::Failed(_) => ("circle-x", p.danger),
            RowState::Listening => ("server", p.text_muted),
        };
        let cell = |w: f32| div().w(px(w)).flex_none().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().pr(px(theme::SPACE[4]));
        let muted = |s: SharedString| div().text_color(c(p.text_muted)).child(s);
        let address: AnyElement = match (&row.state, &row.address) {
            (RowState::Active, Some(a)) => div().font_family(ui::MONO_FONT).text_color(c(p.accent)).child(a.clone()).into_any_element(),
            (RowState::Pending, _) => muted(crate::i18n::t("forwarding…").into()).into_any_element(),
            (RowState::Interrupted, _) => muted(crate::i18n::t("waiting for the connection to come back").into()).into_any_element(),
            (RowState::Failed(why), _) => {
                // Cut to the column with an ellipsis: the whole reason is in the tooltip.
                let full: SharedString = why.clone().into();
                div()
                    .id(("port-why", ix))
                    .text_color(c(p.danger))
                    .text_ellipsis()
                    .overflow_hidden()
                    .child(why.clone())
                    .tooltip(move |_, cx| cx.new(|_| super::super::Tooltip(full.clone())).into())
                    .into_any_element()
            }
            _ => div().into_any_element(),
        };
        let origin = match row.origin {
            Some(Origin::Auto) => crate::i18n::t("Auto Forwarded"),
            Some(Origin::User) => crate::i18n::t("User Forwarded"),
            None => "",
        };
        let row_for_menu = row.clone();
        let menu_row = row.clone();
        let mut actions = div().w(px(W_ACTIONS)).flex_none().flex().items_center().justify_end().gap(px(theme::SPACE[1]));
        if let Some(url) = row.url() {
            actions = actions.child(a11y::row_button(
                ui::icon_button(("port-open", ix), "external-link", crate::i18n::t("Open in Browser"), p)
                    .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
                a11y::RowFrame::Port,
                ix,
                1,
                cx,
            ));
        }
        if row.state == RowState::Listening {
            actions = actions.child(a11y::row_button(
                ui::icon_button(("port-fwd", ix), "plug", crate::i18n::t("Forward Port"), p)
                    .on_click(cx.listener(move |this, _, _, cx| this.forward_port(port, cx))),
                a11y::RowFrame::Port,
                ix,
                4,
                cx,
            ));
        } else {
            if let Some(a) = row.address.clone().filter(|_| row.state == RowState::Active) {
                actions = actions.child(a11y::row_button(
                    ui::icon_button(("port-copy", ix), "copy", crate::i18n::t("Copy Address"), p)
                        .on_click(cx.listener(move |_, _, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(a.clone())))),
                    a11y::RowFrame::Port,
                    ix,
                    2,
                    cx,
                ));
            }
            if matches!(row.state, RowState::Failed(_)) {
                actions = actions.child(a11y::row_button(
                    ui::icon_button(("port-retry", ix), "refresh-cw", crate::i18n::t("Retry"), p)
                        .on_click(cx.listener(move |this, _, _, cx| this.retry_forward(port, cx))),
                    a11y::RowFrame::Port,
                    ix,
                    5,
                    cx,
                ));
            }
            actions = actions
                .child(a11y::row_button(
                    ui::icon_button(("port-stop", ix), "x", crate::i18n::t("Stop Forwarding"), p)
                        .on_click(cx.listener(move |this, _, _, cx| this.stop_forward(port, cx))),
                    a11y::RowFrame::Port,
                    ix,
                    3,
                    cx,
                ))
                .child(a11y::row_button(
                    ui::icon_button(("port-more", ix), "ellipsis", crate::i18n::t("More Actions…"), p).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_port_menu(&menu_row, ev.position, window, cx);
                        }),
                    ),
                    a11y::RowFrame::Port,
                    ix,
                    a11y::MENU_BUTTON,
                    cx,
                ));
        }
        div()
            .id(("port-row", ix))
            .h(px(layout::ROW_H))
            .w_full()
            .flex()
            .items_center()
            .px(px(theme::SPACE[6]))
            .hover(|d| d.bg(c(p.surface_hover)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_port_menu(&row_for_menu, ev.position, window, cx);
                }),
            )
            .child(
                cell(W_PORT).flex().items_center().gap(px(theme::SPACE[3])).child(icon(glyph, layout::ICON - 2., color)).child(
                    div()
                        .font_family(ui::MONO_FONT)
                        .when(!row.listening && row.state != RowState::Listening, |d| d.text_color(c(p.text_muted)))
                        .child(port.to_string()),
                ),
            )
            .child(cell(W_LABEL).child(row.label.clone().unwrap_or_default()))
            .child(cell(W_ADDRESS).child(address))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .pr(px(theme::SPACE[4]))
                    .text_color(c(p.text_muted))
                    .child(row.process.clone().unwrap_or_default()),
            )
            .child(cell(W_ORIGIN).text_color(c(p.text_muted)).child(origin))
            .child(actions)
            .into_any_element()
    }

    pub(in crate::workbench) fn ports_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(alias) = self.ssh_alias() else {
            return center(p).text_color(c(p.text_muted)).child(crate::i18n::t("Port forwarding works while connected to an SSH host.")).into_any_element();
        };
        let rows = port_rows(&self.ports.set, &self.ports.listening, &alias);
        let header_cell = |w: Option<f32>, label: &'static str| {
            let d = div().min_w_0().overflow_hidden().whitespace_nowrap().pr(px(theme::SPACE[4])).child(crate::i18n::upper(crate::i18n::t(label)));
            match w {
                Some(w) => d.w(px(w)).flex_none(),
                None => d.flex_1(),
            }
        };
        let header = div()
            .h(px(layout::ROW_H))
            .flex_none()
            .flex()
            .items_center()
            .px(px(theme::SPACE[6]))
            .border_b_1()
            .border_color(c(p.border))
            .text_size(px(text::XXS.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(c(p.text_muted))
            .child(header_cell(Some(W_PORT), crate::i18n::tr("Port")))
            .child(header_cell(Some(W_LABEL), crate::i18n::tr("Label")))
            .child(header_cell(Some(W_ADDRESS), crate::i18n::tr("Forwarded Address")))
            .child(header_cell(None, crate::i18n::tr("Running Process")))
            .child(header_cell(Some(W_ORIGIN), crate::i18n::tr("Origin")))
            .child(div().w(px(W_ACTIONS)).flex_none());
        let body: AnyElement = if rows.is_empty() {
            div()
                .px(px(theme::SPACE[6]))
                .py(px(theme::SPACE[3]))
                .text_color(c(p.text_muted))
                .child(crate::i18n::t("Nothing is listening on the server yet."))
                .into_any_element()
        } else {
            let p = *p;
            // Virtualized: a busy server can list hundreds of ports.
            let list = gpui::uniform_list(
                "port-rows",
                rows.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                    range
                        .filter_map(|ix| {
                            let row = this.port_row(ix, rows.get(ix)?, &p, cx);
                            Some(this.nav_row(a11y::RowFrame::Port, ix, row, window, cx))
                        })
                        .collect()
                }),
            )
            .track_scroll(self.row_scroll(a11y::RowFrame::Port))
            .size_full();
            let list = self.row_list(a11y::RowFrame::Port, list, cx);
            div().flex_1().min_h_0().child(list).into_any_element()
        };
        div()
            .id("ports")
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(text::SM.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(body)
            .child(
                div()
                    .h(px(layout::ROW_H + 8.))
                    .border_t_1()
                    .border_color(c(p.border))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[4]))
                    .px(px(theme::SPACE[6]))
                    .child(div().text_color(c(p.text_muted)).child(crate::i18n::t("Forward a port:")))
                    .child(ui::input_box("ports-field-box", p, p.bg, false).w(px(200.)).child(self.ports.field.clone()))
                    .children(self.ports_note().map(|e| div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(c(p.warning)).child(e))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_forwards::ForwardRequest;
    use crate::ports::{BindAddr, ProcessInfo};
    use host::forward::{FailKind, ForwardError, Opened, RemoteTarget};

    fn listening(port: u16, name: Option<&str>) -> ListeningPort {
        ListeningPort { port, binds: vec![BindAddr::Loopback4], process: name.map(|n| ProcessInfo { pid: 7, name: n.into() }) }
    }

    #[test]
    fn rows_show_forwards_first_then_free_listening_ports() {
        let mut set = ForwardSet::default();
        let open = |set: &mut ForwardSet, port: u16| {
            let (id, e) = set.request(ForwardRequest::user("box", port, RemoteTarget::Localhost, LocalBind::Loopback));
            let Some(crate::port_forwards::Effect::Open { seq, .. }) = e.last() else { panic!() };
            (id, *seq)
        };
        let (a, sa) = open(&mut set, 8000);
        let (b, sb) = open(&mut set, 3000);
        set.on_result(a, sa, Ok(Opened { local: 8001, spec: "s".into() }));
        set.on_result(b, sb, Err(ForwardError { kind: FailKind::AddrInUse, message: "x".into() }));
        set.request(ForwardRequest::user("other", 9999, RemoteTarget::Localhost, LocalBind::Loopback));
        let rows = port_rows(&set, &[listening(8000, Some("node")), listening(5432, None), listening(3000, None)], "box");
        assert_eq!(rows.iter().map(|r| r.port).collect::<Vec<_>>(), vec![3000, 8000, 5432], "other connections are left out");
        assert!(matches!(&rows[0].state, RowState::Failed(_)));
        assert_eq!(rows[1].address.as_deref(), Some("localhost:8001"));
        assert_eq!(rows[1].process.as_deref(), Some("node (7)"));
        assert_eq!(rows[1].url().as_deref(), Some("http://localhost:8001"));
        assert_eq!(rows[2].state, RowState::Listening);
        assert_eq!(rows[2].url(), None);
    }

    fn row(state: RowState, bind: Option<LocalBind>, protocol: Protocol) -> PortRow {
        PortRow {
            port: 3000,
            label: None,
            address: Some("localhost:3001".into()),
            process: None,
            origin: Some(Origin::User),
            protocol,
            bind,
            state,
            listening: true,
        }
    }

    #[test]
    fn the_menu_of_a_private_http_forward_shows_what_is_set_and_what_can_be_done() {
        let menu = port_menu(&row(RowState::Active, Some(LocalBind::Loopback), Protocol::Http));
        let line = |cmd: PortMenuCmd| menu.iter().find(|(_, c)| *c == cmd).map(|(i, _)| (i.label.to_string(), i.enabled, i.checked)).unwrap();
        assert_eq!(line(PortMenuCmd::Visibility(LocalBind::Loopback)).2, Some(true), "private is the one that is on");
        assert_eq!(line(PortMenuCmd::Visibility(LocalBind::All)).2, Some(false));
        assert_eq!(line(PortMenuCmd::Protocol(Protocol::Http)).2, Some(true));
        assert_eq!(line(PortMenuCmd::Protocol(Protocol::Tcp)).2, Some(false));
        assert!(line(PortMenuCmd::OpenBrowser).1, "an active HTTP forward opens in the browser");
        assert!(line(PortMenuCmd::CopyAddress).1);
        assert!(!line(PortMenuCmd::Retry).1, "nothing failed, so there is nothing to retry");
        assert_eq!(menu.len(), 12);
    }

    #[test]
    fn the_menu_of_a_public_forward_and_of_a_failed_one_differ_where_they_should() {
        let public = port_menu(&row(RowState::Active, Some(LocalBind::All), Protocol::Tcp));
        let checked = |cmd: PortMenuCmd| public.iter().find(|(_, c)| *c == cmd).and_then(|(i, _)| i.checked);
        assert_eq!((checked(PortMenuCmd::Visibility(LocalBind::All)), checked(PortMenuCmd::Visibility(LocalBind::Loopback))), (Some(true), Some(false)));
        assert!(!public.iter().find(|(_, c)| *c == PortMenuCmd::OpenBrowser).unwrap().0.enabled, "a TCP forward has no page to open");
        let failed = port_menu(&row(RowState::Failed("busy".into()), Some(LocalBind::Loopback), Protocol::Http));
        assert!(failed.iter().find(|(_, c)| *c == PortMenuCmd::Retry).unwrap().0.enabled, "a failed forward can be retried");
        assert!(!failed.iter().find(|(_, c)| *c == PortMenuCmd::CopyAddress).unwrap().0.enabled, "and has no address to copy");
    }

    #[test]
    fn a_port_that_is_only_listening_can_be_forwarded_or_copied() {
        let menu = port_menu(&row(RowState::Listening, None, Protocol::Http));
        assert_eq!(menu.iter().map(|(_, c)| *c).collect::<Vec<_>>(), [PortMenuCmd::Forward, PortMenuCmd::CopyPort]);
    }
}
