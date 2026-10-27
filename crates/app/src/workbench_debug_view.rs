//! The Run and Debug sidebar view: a configuration selector with ▶, and the sections Variables, Watch, Call Stack and
//! Breakpoints. The first open section takes the free height; the others keep the height they are dragged to. Lists are
//! virtualized, so a large array or a deep stack stays cheap to draw.

use gpui::{AnyElement, Context, MouseButton, SharedString, Window, div, px};

use super::super::*;
use super::{DebugState, DebugStatus};
use crate::debug_model::{BpRow, Verified};

/// The launch file the view creates when there is none.
pub(super) const LAUNCH_FILE: &str = ".codeyard/launch.jsonc";
const LAUNCH_TEMPLATE: &str = r#"{
  // Debug configurations: Run and Debug (F5) starts the selected one. Each needs a "name" and a "type"
  // (lldb, go, python, node or custom with "debugAdapter"); "request" is "launch" or "attach".
  // ${workspaceFolder} and ${file} are replaced when it starts.
  "version": "0.2.0",
  "configurations": []
}
"#;

/// A command from one of the view's menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugMenuCmd {
    /// Pick the configuration at this index of the catalog.
    Select(usize),
    OpenLaunchFile,
    EditBreakpoint,
    ToggleBreakpoint,
    RemoveBreakpoint,
    /// Variable rows carry "parent reference\tname".
    SetValue,
    CopyValue,
    CopyExpression,
    AddToWatch,
    /// Frame rows carry the frame index.
    RestartFrame,
    CopyCallStack,
}

/// Sections in order: (id, title, default height when not the first open one).
const SECTIONS: [(&str, &str, f32); 4] = [
    ("dbg-variables", crate::i18n::tr("Variables"), 220.),
    ("dbg-watch", crate::i18n::tr("Watch"), 110.),
    ("dbg-stack", crate::i18n::tr("Call Stack"), 170.),
    ("dbg-breakpoints", crate::i18n::tr("Breakpoints"), 140.),
];

/// A row of Variables or Watch.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct VarRow {
    pub depth: usize,
    pub name: String,
    pub value: String,
    pub ty: Option<String>,
    /// > 0: has children.
    pub reference: i64,
    pub open: bool,
    /// Index in the watch list (Watch rows).
    pub watch: Option<usize>,
    /// Scope or object this variable belongs to (0 for scopes and watches).
    pub parent: i64,
    /// A "… n more" row: the paged value to load more of.
    pub more: Option<i64>,
}

/// Scopes and their open children, depth first (depth capped like the old panel).
pub(super) fn variable_rows(d: &DebugState) -> Vec<VarRow> {
    fn walk(d: &DebugState, out: &mut Vec<VarRow>, reference: i64, depth: usize) {
        if depth >= 8 || !d.expanded.contains(&reference) {
            return;
        }
        for v in d.children.get(&reference).into_iter().flatten() {
            let open = v.reference > 0 && d.expanded.contains(&v.reference);
            out.push(VarRow {
                depth,
                name: v.name.clone(),
                value: v.value.clone(),
                ty: v.ty.clone(),
                reference: v.reference,
                open,
                watch: None,
                parent: reference,
                more: None,
            });
            walk(d, out, v.reference, depth + 1);
        }
        if let Some(&(loaded, total)) = d.pages.get(&reference).filter(|(l, t)| l < t) {
            let name = crate::i18n::tf("… {} more", &[&(total - loaded)]);
            out.push(VarRow { depth, name, value: String::new(), ty: None, reference: 0, open: false, watch: None, parent: reference, more: Some(reference) });
        }
    }
    let mut out = Vec::new();
    if !matches!(d.status, DebugStatus::Stopped(_)) {
        return out;
    }
    for s in &d.scopes {
        out.push(VarRow {
            depth: 0,
            name: s.name.clone(),
            value: String::new(),
            ty: None,
            reference: s.reference,
            open: d.expanded.contains(&s.reference),
            watch: None,
            parent: 0,
            more: None,
        });
        walk(d, &mut out, s.reference, 1);
    }
    out
}

/// A row of Call Stack.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum StackRow {
    /// Shown when there is more than one session.
    Session {
        id: u64,
        name: String,
        status: String,
        current: bool,
    },
    Thread {
        id: i64,
        name: String,
        current: bool,
    },
    Frame {
        index: usize,
        name: String,
        place: String,
        current: bool,
        source: bool,
    },
    LoadMore,
    Note(String),
}

pub(super) fn stack_rows(sessions: &[DebugState], current: Option<u64>) -> Vec<StackRow> {
    let mut out = Vec::new();
    let many = sessions.len() > 1;
    for d in sessions {
        let is_current = Some(d.id) == current;
        if many {
            out.push(StackRow::Session { id: d.id, name: d.config.name.clone(), status: status_text(&d.status), current: is_current });
        }
        if !is_current {
            continue;
        }
        match &d.status {
            DebugStatus::Stopped(_) => {
                if d.threads.len() > 1 {
                    for (id, name) in &d.threads {
                        out.push(StackRow::Thread { id: *id, name: name.clone(), current: *id == d.thread });
                        if *id == d.thread {
                            out.extend(frame_rows(d));
                        }
                    }
                } else {
                    out.extend(frame_rows(d));
                }
            }
            DebugStatus::Running => out.push(StackRow::Note(crate::i18n::t("Running…").into())),
            DebugStatus::Starting => out.push(StackRow::Note(crate::i18n::t("Starting…").into())),
            DebugStatus::Ended(_) if !many => out.push(StackRow::Note(crate::i18n::t("Not paused").into())),
            DebugStatus::Ended(_) => {}
        }
    }
    out
}

fn frame_rows(d: &DebugState) -> impl Iterator<Item = StackRow> + '_ {
    let more = Workbench::more_frames(d).then_some(StackRow::LoadMore);
    d.frames
        .iter()
        .enumerate()
        .map(|(i, f)| StackRow::Frame {
            index: i,
            name: f.name.clone(),
            place: f.path.as_deref().map(|p| format!("{}:{}", ui::file_name(p), f.line)).unwrap_or_default(),
            current: i == d.frame,
            source: f.path.is_some(),
        })
        .chain(more)
}

/// Short status of a session.
pub(super) fn status_text(s: &DebugStatus) -> String {
    match s {
        DebugStatus::Starting => crate::i18n::t("starting").into(),
        DebugStatus::Running => crate::i18n::t("running").into(),
        DebugStatus::Stopped(r) => crate::i18n::tf("paused on {}", &[&super::stop_reason(r)]),
        DebugStatus::Ended(_) => crate::i18n::t("ended").into(),
    }
}

impl Workbench {
    pub(in crate::workbench) fn show_run_debug(&mut self, _: &super::ShowRunAndDebug, _: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Debug);
        self.load_launch_catalog(cx);
        cx.notify();
    }

    /// Opens the launch file, creating `.codeyard/launch.jsonc` when the folder has none.
    pub(in crate::workbench) fn open_launch_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else { return };
        let host = self.remote.host.clone();
        let task = cx.background_executor().spawn(async move {
            for rel in [LAUNCH_FILE, ".vscode/launch.json"] {
                let path = ui::join(&root, rel);
                if host.stat(&path).is_ok() {
                    return Ok(path);
                }
            }
            let path = ui::join(&root, LAUNCH_FILE);
            host.create_file(&path)?;
            host.write(&path, LAUNCH_TEMPLATE.as_bytes().to_vec(), None)?;
            Ok::<_, host::HostError>(path)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(path) => this.open_file(path, window, cx),
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not create {0}: {1}", &[&LAUNCH_FILE, &e]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(in crate::workbench) fn run_debug_menu(&mut self, cmd: DebugMenuCmd, target: MenuTarget, window: &mut Window, cx: &mut Context<Self>) {
        // Breakpoint rows carry "path\tline".
        let at = target.path.rsplit_once('\t').and_then(|(p, l)| Some((p.to_string(), l.parse::<usize>().ok()?)));
        match (cmd, at) {
            (DebugMenuCmd::Select(i), _) => {
                if let Some(name) = self.dbg.catalog.get(i).cloned() {
                    self.dbg.last = Some(name);
                    cx.notify();
                }
            }
            (DebugMenuCmd::OpenLaunchFile, _) => self.open_launch_file(window, cx),
            (DebugMenuCmd::EditBreakpoint, Some((path, line))) => self.start_edit_breakpoint(path, line, window, cx),
            (DebugMenuCmd::ToggleBreakpoint, Some((path, line))) => {
                let on = self.dbg.bps.file(&path).iter().any(|(l, m)| *l == line && m.disabled);
                self.set_breakpoint_enabled(path, line, on, cx);
            }
            (DebugMenuCmd::RemoveBreakpoint, Some((path, line))) => {
                self.dbg.bps.remove(&path, line);
                self.breakpoints_changed_in_store(&path, true, cx);
            }
            (DebugMenuCmd::RestartFrame, _) => self.restart_frame(target.path.parse().unwrap_or(0), cx),
            (DebugMenuCmd::CopyCallStack, _) => self.copy_call_stack(cx),
            (DebugMenuCmd::SetValue | DebugMenuCmd::CopyValue | DebugMenuCmd::CopyExpression | DebugMenuCmd::AddToWatch, _) => {
                let Some((parent, name)) = target.path.split_once('\t').and_then(|(p, n)| Some((p.parse::<i64>().ok()?, n.to_string()))) else { return };
                let Some((expr, value)) = self.variable_expression(parent, &name) else { return };
                match cmd {
                    DebugMenuCmd::SetValue => self.start_set_variable(parent, name, window, cx),
                    DebugMenuCmd::CopyValue => cx.write_to_clipboard(gpui::ClipboardItem::new_string(value)),
                    DebugMenuCmd::CopyExpression => cx.write_to_clipboard(gpui::ClipboardItem::new_string(expr)),
                    _ => self.finish_add_watch(expr, cx),
                }
            }
            _ => {}
        }
    }

    fn open_variable_menu(&mut self, parent: i64, name: String, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let stopped = self.dbg.cur().is_some_and(|d| matches!(d.status, DebugStatus::Stopped(_)));
        let can_set = self.dbg.cur().and_then(|d| d.session.as_ref()).is_some_and(|s| s.supports("supportsSetVariable") || s.supports("supportsSetExpression"));
        let items = vec![
            MenuItem::new(crate::i18n::t("Set Value…")).enabled(stopped && can_set),
            MenuItem::new(crate::i18n::t("Copy Value")).separator(),
            MenuItem::new(crate::i18n::t("Copy as Expression")),
            MenuItem::new(crate::i18n::t("Add to Watch")).separator(),
        ];
        let commands = [DebugMenuCmd::SetValue, DebugMenuCmd::CopyValue, DebugMenuCmd::CopyExpression, DebugMenuCmd::AddToWatch].map(MenuCmd::Debug).to_vec();
        let target = MenuTarget { path: format!("{parent}\t{name}"), is_dir: false, tab: None };
        self.show_menu(items, commands, Vec::new(), target, position, window, cx);
    }

    fn open_frame_menu(&mut self, index: usize, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let can_restart = self
            .dbg
            .cur()
            .is_some_and(|d| d.frames.get(index).is_some_and(|f| f.can_restart) && d.session.as_ref().is_some_and(|s| s.supports("supportsRestartFrame")));
        let items = vec![MenuItem::new(crate::i18n::t("Restart Frame")).enabled(can_restart), MenuItem::new(crate::i18n::t("Copy Call Stack")).separator()];
        let commands = [DebugMenuCmd::RestartFrame, DebugMenuCmd::CopyCallStack].map(MenuCmd::Debug).to_vec();
        let target = MenuTarget { path: index.to_string(), is_dir: false, tab: None };
        self.show_menu(items, commands, Vec::new(), target, position, window, cx);
    }

    fn open_config_menu(&mut self, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let mut items: Vec<MenuItem> = self.dbg.catalog.iter().map(|n| MenuItem::new(n).checked(self.dbg.last.as_ref() == Some(n))).collect();
        let mut commands: Vec<MenuCmd> = (0..items.len()).map(|i| MenuCmd::Debug(DebugMenuCmd::Select(i))).collect();
        items.push(MenuItem::new(crate::i18n::t("Open Launch Configuration")).separator());
        commands.push(MenuCmd::Debug(DebugMenuCmd::OpenLaunchFile));
        let target = MenuTarget { path: String::new(), is_dir: false, tab: None };
        self.show_menu(items, commands, Vec::new(), target, position, window, cx);
    }

    fn open_breakpoint_menu(
        &mut self,
        path: String,
        line: usize,
        disabled: bool,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = vec![
            MenuItem::new(crate::i18n::t("Edit Condition…")),
            MenuItem::new(if disabled { crate::i18n::t("Enable Breakpoint") } else { crate::i18n::t("Disable Breakpoint") }),
            MenuItem::new(crate::i18n::t("Remove Breakpoint")).separator(),
        ];
        let commands = [DebugMenuCmd::EditBreakpoint, DebugMenuCmd::ToggleBreakpoint, DebugMenuCmd::RemoveBreakpoint].map(MenuCmd::Debug).to_vec();
        let target = MenuTarget { path: format!("{path}\t{line}"), is_dir: false, tab: None };
        self.show_menu(items, commands, Vec::new(), target, position, window, cx);
    }

    pub(in crate::workbench) fn run_debug_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let header = div()
            .h(px(layout::VIEW_HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[1]))
            .pl(px(theme::SPACE[6]))
            .pr(px(theme::SPACE[3]))
            .child(
                div()
                    .flex_1()
                    .text_size(px(text::XXS.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::upper(crate::i18n::t("Run and Debug"))),
            )
            .child(
                ui::icon_button("dbg-open-launch", "settings", crate::i18n::t("Open Launch Configuration"), p)
                    .on_click(cx.listener(|this, _, w, cx| this.open_launch_file(w, cx))),
            );
        let selected = self.dbg.last.clone().filter(|n| self.dbg.catalog.contains(n));
        let launcher = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .px(px(theme::SPACE[5]))
            .pb(px(theme::SPACE[3]))
            .child(
                div()
                    .id("dbg-start")
                    .flex_none()
                    .h(px(24.))
                    .w(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::radius::SM))
                    .bg(c(p.accent))
                    .cursor_pointer()
                    .hover(|d| d.opacity(0.9))
                    .child(icon("play", layout::ICON - 2., p.on_accent))
                    .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Start Debugging (F5)").into())).into())
                    .on_click(cx.listener(|this, _, w, cx| this.start_selected(w, cx))),
            )
            .child(
                div()
                    .id("dbg-config")
                    .flex_1()
                    .min_w_0()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .px(px(theme::SPACE[3]))
                    .rounded(px(theme::radius::SM))
                    .border_1()
                    .border_color(c(p.border))
                    .bg(c(p.bg))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(selected.clone().unwrap_or_else(|| {
                        if self.dbg.catalog.is_empty() { crate::i18n::t("No Configurations").into() } else { crate::i18n::t("Select a configuration").into() }
                    })))
                    .child(icon("chevron-down", layout::ICON - 2., p.text_muted))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &gpui::MouseDownEvent, w, cx| {
                            cx.stop_propagation();
                            this.open_config_menu(ev.position, w, cx);
                        }),
                    ),
            );
        let mut view = div().id("run-debug-view").flex().flex_col().size_full().text_size(px(text::SM.0)).child(header).child(launcher);
        // Nothing to show yet: say how to start.
        if !self.dbg.has_sessions() && self.dbg.catalog.is_empty() {
            view = view.child(self.debug_empty_state(p, cx));
        }
        let mut flex_taken = false;
        for (id, title, default_h) in SECTIONS {
            let open = self.sections.is_open(id, true);
            let count = match id {
                "dbg-watch" => Some(self.dbg.watch_exprs.len()),
                "dbg-breakpoints" => Some(self.dbg.bps.count()),
                _ => None,
            };
            let mut head = sections::header(SharedString::from(format!("sec-{id}")), crate::i18n::t(title), count, open, p);
            head = match id {
                "dbg-watch" => {
                    head.child(ui::icon_button("dbg-watch-add", "plus", crate::i18n::t("Add Expression…"), p).on_click(cx.listener(|this, _, w, cx| {
                        cx.stop_propagation();
                        this.add_watch(&super::super::AddWatch, w, cx);
                    })))
                }
                "dbg-breakpoints" => head
                    .child(ui::icon_button("dbg-function-bp", "plus", crate::i18n::t("Add Function Breakpoint…"), p).on_click(cx.listener(|this, _, w, cx| {
                        cx.stop_propagation();
                        this.add_function_breakpoint(&super::AddFunctionBreakpoint, w, cx);
                    })))
                    .child(
                        ui::icon_button(
                            "dbg-bp-active",
                            "circle-slash-2",
                            if self.dbg.bps.active { crate::i18n::t("Deactivate Breakpoints") } else { crate::i18n::t("Activate Breakpoints") },
                            p,
                        )
                        .when(!self.dbg.bps.active, |d| d.bg(c(p.surface_active)))
                        .on_click(cx.listener(|this, _, w, cx| {
                            cx.stop_propagation();
                            this.toggle_breakpoints_active(&super::ToggleBreakpointsActive, w, cx);
                        })),
                    )
                    .child(ui::icon_button("dbg-bp-clear", "x", crate::i18n::t("Remove All Breakpoints"), p).on_click(cx.listener(|this, _, w, cx| {
                        cx.stop_propagation();
                        this.remove_all_breakpoints(&super::super::RemoveAllBreakpoints, w, cx);
                    }))),
                _ => head,
            };
            view = view.child(head.on_click(cx.listener(move |this, _, _, cx| {
                this.sections.toggle(id, true);
                this.persist_sections(cx);
            })));
            if !open {
                continue;
            }
            let body = match id {
                "dbg-variables" => self.variables_body(p, cx),
                "dbg-watch" => self.watch_body(p, cx),
                "dbg-stack" => self.stack_body(p, cx),
                _ => self.breakpoints_body(p, cx),
            };
            if flex_taken {
                let h = self.sections.height(id, default_h);
                let sash = sections::sash(SharedString::from(format!("sash-{id}")), p, self.sections.dragging_section(id)).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.sections.begin_drag(id, ev.position.y / px(1.), h, -1.);
                    }),
                );
                view = view.child(sash).child(div().h(px(h)).flex_shrink().min_h(px(40.)).child(body));
            } else {
                flex_taken = true;
                view = view.child(div().flex_1().min_h(px(60.)).child(body));
            }
        }
        if !flex_taken {
            view = view.child(div().flex_1());
        }
        view.into_any_element()
    }

    fn debug_empty_state(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[3]))
            .px(px(theme::SPACE[6]))
            .pb(px(theme::SPACE[5]))
            .text_color(c(p.text_muted))
            .child(div().child(crate::i18n::t("To customize Run and Debug, create a launch configuration.")))
            .child(
                div()
                    .id("dbg-create-launch")
                    .text_color(c(p.accent))
                    .cursor_pointer()
                    .hover(|d| d.underline())
                    .child(crate::i18n::t("Create a launch configuration"))
                    .on_click(cx.listener(|this, _, w, cx| this.open_launch_file(w, cx))),
            )
            .child(div().child(crate::i18n::t("Without one, F5 finds a configuration from the project (Cargo, Go, or the open Python or JavaScript file).")))
            .into_any_element()
    }

    fn note(&self, text: impl Into<SharedString>, p: &Palette) -> AnyElement {
        div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).child(text.into()).into_any_element()
    }

    fn variables_body(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.dbg.cur().map(variable_rows).unwrap_or_default();
        if rows.is_empty() {
            return self.note(crate::i18n::t("Not paused"), p);
        }
        self.var_list("dbg-var-rows", rows, p, cx)
    }

    fn watch_body(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.dbg.watch_exprs.is_empty() {
            return self.note(crate::i18n::t("No expressions: + adds one."), p);
        }
        let stopped = self.dbg.cur().is_some_and(|d| matches!(d.status, DebugStatus::Stopped(_)));
        let values = self.dbg.cur().map(|d| d.watch_values.clone()).unwrap_or_default();
        let rows = self
            .dbg
            .watch_exprs
            .iter()
            .enumerate()
            .map(|(i, e)| VarRow {
                depth: 0,
                name: e.clone(),
                value: values.get(i).cloned().unwrap_or_else(|| if stopped { "…".into() } else { crate::i18n::t("not available").into() }),
                ty: None,
                reference: 0,
                open: false,
                watch: Some(i),
                parent: 0,
                more: None,
            })
            .collect();
        self.var_list("dbg-watch-rows", rows, p, cx)
    }

    fn var_list(&self, id: &'static str, rows: Vec<VarRow>, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let p = *p;
        gpui::uniform_list(
            id,
            rows.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|ix| {
                        let r = rows.get(ix)?;
                        let reference = r.reference;
                        let watch = r.watch;
                        let group = SharedString::from(format!("{id}-{ix}"));
                        if let Some(paged) = r.more {
                            return Some(
                                div()
                                    .id((id, ix))
                                    .h(px(layout::ROW_H))
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .pl(px(theme::SPACE[5] + 12. + theme::SPACE[2] + layout::TREE_INDENT * r.depth as f32))
                                    .text_color(c(p.accent))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(c(p.surface_hover)))
                                    .child(r.name.clone())
                                    .on_click(cx.listener(move |this, _, _, cx| this.load_more_variables(paged, cx)))
                                    .into_any_element(),
                            );
                        }
                        // Variables (not scopes or watches) have a menu.
                        let menu = (r.parent != 0).then(|| (r.parent, r.name.clone()));
                        Some(
                            div()
                                .id((id, ix))
                                .group(group.clone())
                                .relative()
                                .h(px(layout::ROW_H))
                                .w_full()
                                .flex()
                                .items_center()
                                .gap(px(theme::SPACE[2]))
                                .pl(px(theme::SPACE[5] + layout::TREE_INDENT * r.depth as f32))
                                .pr(px(theme::SPACE[3]))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .child(
                                    div()
                                        .w(px(12.))
                                        .flex_none()
                                        .when(reference > 0, |d| d.child(icon(if r.open { "chevron-down" } else { "chevron-right" }, 12., p.text_muted))),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(c(if r.depth == 0 && watch.is_none() { p.text } else { p.syntax_function }))
                                        .child(r.name.clone()),
                                )
                                .when(!r.value.is_empty(), |d| {
                                    d.child(div().flex_none().text_color(c(p.text_muted)).child(if watch.is_some() { "=" } else { ":" }))
                                        .child(div().min_w_0().overflow_hidden().font_family(ui::MONO_FONT).child(r.value.clone()))
                                })
                                .when_some(r.ty.clone(), |d, t| d.child(div().flex_none().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(t)))
                                .when_some(watch, |d, i| {
                                    d.child(div().absolute().right(px(theme::SPACE[3])).invisible().group_hover(group.clone(), |d| d.visible()).child(
                                        ui::icon_button(("dbg-watch-x", i), "x", crate::i18n::t("Remove Expression"), &p).on_click(cx.listener(
                                            move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.remove_watch(i, cx);
                                            },
                                        )),
                                    ))
                                })
                                .when(reference > 0, |d| d.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.toggle_variable(reference, cx))))
                                .when_some(menu, |d, (parent, name)| {
                                    d.on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, ev: &gpui::MouseDownEvent, w, cx| {
                                            cx.stop_propagation();
                                            this.open_variable_menu(parent, name.clone(), ev.position, w, cx);
                                        }),
                                    )
                                })
                                .into_any_element(),
                        )
                    })
                    .collect()
            }),
        )
        .size_full()
        .into_any_element()
    }

    fn stack_body(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows = stack_rows(&self.dbg.sessions, self.dbg.cur().map(|d| d.id));
        if rows.is_empty() {
            return self.note(crate::i18n::t("No debug session."), p);
        }
        let p = *p;
        gpui::uniform_list(
            "dbg-stack-rows",
            rows.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|ix| {
                        let row = rows.get(ix)?;
                        let base = div()
                            .id(("dbg-stack", ix))
                            .h(px(layout::ROW_H))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[2]))
                            .pr(px(theme::SPACE[3]))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .hover(|d| d.bg(c(p.surface_hover)));
                        let el = match row {
                            StackRow::Session { id, name, status, current } => {
                                let id = *id;
                                base.pl(px(theme::SPACE[5]))
                                    .cursor_pointer()
                                    .when(*current, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                                    .child(icon("bug-play", 12., if *current { p.accent } else { p.text_muted }))
                                    .child(name.clone())
                                    .child(div().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(status.clone()))
                                    .on_click(cx.listener(move |this, _, _, cx| this.focus_debug_session(id, cx)))
                            }
                            StackRow::Thread { id, name, current } => {
                                let id = *id;
                                base.pl(px(theme::SPACE[5] + layout::TREE_INDENT))
                                    .cursor_pointer()
                                    .when(*current, |d| d.bg(c(p.surface_active)))
                                    .child(icon("list", 12., p.text_muted))
                                    .child(name.clone())
                                    .child(div().text_color(c(p.text_muted)).child(format!("#{id}")))
                                    .on_click(cx.listener(move |this, _, _, cx| this.select_thread(id, cx)))
                            }
                            StackRow::Frame { index, name, place, current, source } => {
                                let index = *index;
                                base.pl(px(theme::SPACE[5] + layout::TREE_INDENT * 2.))
                                    .cursor_pointer()
                                    .when(*current, |d| d.bg(c(p.surface_active)))
                                    .when(!*source, |d| d.text_color(c(p.text_muted)))
                                    .child(div().flex_shrink().min_w_0().overflow_hidden().child(name.clone()))
                                    .child(div().flex_1())
                                    .child(div().flex_none().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(place.clone()))
                                    .on_click(cx.listener(move |this, _, _, cx| this.select_frame(index, cx)))
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, ev: &gpui::MouseDownEvent, w, cx| {
                                            cx.stop_propagation();
                                            this.open_frame_menu(index, ev.position, w, cx);
                                        }),
                                    )
                            }
                            StackRow::LoadMore => base
                                .pl(px(theme::SPACE[5] + layout::TREE_INDENT * 2.))
                                .cursor_pointer()
                                .text_color(c(p.accent))
                                .child(crate::i18n::t("Load More Stack Frames"))
                                .on_click(cx.listener(|this, _, _, cx| this.load_more_frames(cx))),
                            StackRow::Note(t) => base.pl(px(theme::SPACE[6])).text_color(c(p.text_muted)).child(t.clone()),
                        };
                        Some(el.into_any_element())
                    })
                    .collect()
            }),
        )
        .size_full()
        .into_any_element()
    }

    fn breakpoints_body(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.dbg.bps.rows();
        // Data breakpoints belong to the session (the adapter's id for a value), so they are listed while it lives.
        let data: Vec<String> = self.dbg.cur().map(|d| d.data_breakpoints.iter().map(|(_, desc)| desc.clone()).collect()).unwrap_or_default();
        if !data.is_empty() {
            let list = div().flex().flex_col().children(data.into_iter().map(|desc| {
                div()
                    .h(px(layout::ROW_H))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .pl(px(theme::SPACE[5] + 22.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(div().flex_none().size(px(8.)).rounded_full().bg(c(p.danger)))
                    .child(div().flex_none().text_color(c(p.text_muted)).child(crate::i18n::t("breaks when written:")))
                    .child(div().min_w_0().overflow_hidden().child(desc))
            }));
            let rest = if rows.is_empty() { div().into_any_element() } else { self.breakpoint_list(rows, p, cx) };
            return div().size_full().flex().flex_col().child(list).child(div().flex_1().min_h_0().child(rest)).into_any_element();
        }
        if rows.is_empty() {
            return self.note(crate::i18n::t("No breakpoints: F9 or a click in the gutter adds one."), p);
        }
        self.breakpoint_list(rows, p, cx)
    }

    fn breakpoint_list(&self, rows: Vec<BpRow>, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let root = self.root(cx);
        let active = self.dbg.bps.active;
        let p = *p;
        gpui::uniform_list(
            "dbg-bp-rows",
            rows.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|ix| {
                        let row = rows.get(ix)?;
                        let (enabled, verified) = match row {
                            BpRow::Line { meta, .. } => (!meta.disabled, meta.verified.clone()),
                            BpRow::Function { bp, .. } => (!bp.disabled, bp.verified.clone()),
                        };
                        let dot = match (&verified, enabled && active) {
                            (_, false) => p.text_muted,
                            (Verified::No(_), _) => p.warning,
                            _ => p.danger,
                        };
                        let checkbox = |id: SharedString| {
                            div()
                                .id(id)
                                .flex_none()
                                .size(px(14.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(c(if enabled { p.accent } else { p.border }))
                                .when(enabled, |d| d.bg(c(p.accent)).child(icon("check", 11., p.on_accent)))
                        };
                        let group = SharedString::from(format!("dbg-bp-{ix}"));
                        let base = div()
                            .id(("dbg-bp", ix))
                            .group(group.clone())
                            .relative()
                            .h(px(layout::ROW_H))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[2]))
                            .pl(px(theme::SPACE[5]))
                            .pr(px(theme::SPACE[3]))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_hover)))
                            .when(!enabled || !active, |d| d.text_color(c(p.text_muted)));
                        let el = match row {
                            BpRow::Line { path, line, meta } => {
                                let (path, line) = (path.clone(), *line);
                                let shown = root.as_deref().and_then(|r| path.strip_prefix(&format!("{r}/"))).unwrap_or(&path).to_string();
                                let detail = crate::debug_model::breakpoint_meta_text(meta);
                                let tip: Option<SharedString> = match &meta.verified {
                                    Verified::No(why) => {
                                        Some(why.clone().unwrap_or_else(|| crate::i18n::t("The debugger could not bind this breakpoint.").into()).into())
                                    }
                                    _ => None,
                                };
                                let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path.clone());
                                let disabled = meta.disabled;
                                base.child(checkbox(SharedString::from(format!("dbg-bp-check-{ix}"))).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.set_breakpoint_enabled(p1.clone(), line, disabled, cx);
                                })))
                                .child(div().flex_none().size(px(8.)).rounded_full().bg(c(dot)))
                                .child(div().flex_shrink().min_w_0().overflow_hidden().child(ui::file_name(&shown).to_string()))
                                .child(div().flex_none().text_color(c(p.text_muted)).child(format!("{}", line + 1)))
                                .when(!detail.is_empty(), |d| {
                                    d.child(div().flex_shrink().min_w_0().overflow_hidden().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(detail))
                                })
                                .when_some(tip, |d, tip| d.tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into()))
                                .child(div().absolute().right(px(theme::SPACE[3])).invisible().group_hover(group.clone(), |d| d.visible()).child(
                                    ui::icon_button(("dbg-bp-x", ix), "x", crate::i18n::t("Remove Breakpoint"), &p).on_click(cx.listener(
                                        move |this, _, w, cx| {
                                            cx.stop_propagation();
                                            this.run_debug_menu(
                                                DebugMenuCmd::RemoveBreakpoint,
                                                MenuTarget { path: format!("{p2}\t{line}"), is_dir: false, tab: None },
                                                w,
                                                cx,
                                            );
                                        },
                                    )),
                                ))
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, ev: &gpui::MouseDownEvent, w, cx| {
                                        cx.stop_propagation();
                                        this.open_breakpoint_menu(p3.clone(), line, disabled, ev.position, w, cx);
                                    }),
                                )
                                .on_click(cx.listener(move |this, _, w, cx| this.open_location(p4.clone(), line as u32, 0, w, cx)))
                            }
                            BpRow::Function { index, bp } => {
                                let index = *index;
                                let disabled = bp.disabled;
                                base.child(checkbox(SharedString::from(format!("dbg-fbp-check-{ix}"))).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.set_function_breakpoint_enabled(index, disabled, cx);
                                })))
                                .child(div().flex_none().size(px(8.)).rounded(px(2.)).bg(c(dot)))
                                .child(div().flex_shrink().min_w_0().overflow_hidden().font_family(ui::MONO_FONT).child(format!("{}()", bp.name)))
                                .child(
                                    div().absolute().right(px(theme::SPACE[3])).invisible().group_hover(group.clone(), |d| d.visible()).child(
                                        ui::icon_button(("dbg-fbp-x", ix), "x", crate::i18n::t("Remove Breakpoint"), &p).on_click(cx.listener(
                                            move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.remove_function_breakpoint(index, cx);
                                            },
                                        )),
                                    ),
                                )
                            }
                        };
                        Some(el.into_any_element())
                    })
                    .collect()
            }),
        )
        .size_full()
        .into_any_element()
    }

    /// The view's rows as text: Variables (`name: value`, indented by depth) and Call Stack (`session|thread|frame:…`).
    #[cfg(test)]
    pub fn run_debug_rows_for_test(&self) -> (Vec<String>, Vec<String>) {
        let vars =
            self.dbg.cur().map(variable_rows).unwrap_or_default().into_iter().map(|r| format!("{}{}: {}", "  ".repeat(r.depth), r.name, r.value)).collect();
        let stack = stack_rows(&self.dbg.sessions, self.dbg.cur().map(|d| d.id))
            .into_iter()
            .map(|r| match r {
                StackRow::Session { name, status, .. } => format!("session:{name} ({status})"),
                StackRow::Thread { name, current, .. } => format!("thread:{name}{}", if current { "*" } else { "" }),
                StackRow::Frame { name, current, .. } => format!("frame:{name}{}", if current { "*" } else { "" }),
                StackRow::LoadMore => "more".into(),
                StackRow::Note(t) => format!("note:{t}"),
            })
            .collect();
        (vars, stack)
    }

    #[cfg(test)]
    pub fn toolbar_shown_for_test(&self) -> bool {
        self.dbg.cur().is_some_and(|d| !matches!(d.status, DebugStatus::Ended(_)))
    }
}
