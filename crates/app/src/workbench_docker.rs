//! Docker module UI orchestration. No discovery runs until explicitly requested.

use super::*;
use crate::container_engine::Engine;
use crate::docker::{self, Command as DockerCommand};

#[derive(Clone)]
pub(super) enum Action {
    Attach,
    Services,
    Open(docker::Project, bool),
}

pub(super) fn is_command(label: &str) -> bool {
    label.starts_with("Docker:") || label == "Remote: Connect to Dev Container…" || label == "View: Containers"
}

impl Workbench {
    pub(super) fn open_container_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let labels = [
            crate::i18n::t("Connect to a Container…"),
            crate::i18n::t("Create Dev Container…"),
            crate::i18n::t("Rebuild Dev Container…"),
            crate::i18n::t("Containers and Service Logs…"),
            crate::i18n::t("Show Output"),
            crate::i18n::t("Cancel Operation"),
        ];
        let items = labels.into_iter().enumerate().map(|(i, label)| MenuItem::new(label).enabled(i != 5 || self.remote.docker.cancel.is_some())).collect();
        self.show_menu(
            items,
            (0..6).map(MenuCmd::ContainerTool).collect(),
            Vec::new(),
            MenuTarget { path: String::new(), is_dir: false, tab: None },
            window.mouse_position(),
            window,
            cx,
        );
    }

    pub(super) fn run_container_tool(&mut self, command: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.docker_available(cx) {
            return;
        }
        match command {
            0 => self.docker_contexts(Action::Attach, window, cx),
            1 => self.docker_open_project(false, window, cx),
            2 => self.docker_open_project(true, window, cx),
            3 => self.docker_contexts(Action::Services, window, cx),
            4 => {
                self.chrome.panel_open = true;
                self.chrome.bottom = Bottom::Output;
                cx.notify();
            }
            5 => self.cancel_docker(cx),
            _ => {}
        }
    }

    fn container_ssh_host(&self) -> Option<String> {
        match &self.remote.conn {
            Conn::Connected { alias } if self.remote.host.is_remote() && matches!(host::ssh::target_of(alias), host::ssh::Target::Ssh(_)) => {
                Some(alias.clone())
            }
            Conn::Connected { alias } => host::ssh::ssh_docker_host(alias),
            _ => None,
        }
    }
    fn show_container_setup(&mut self, name: &str, url: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let detail = crate::i18n::tf("{} is not installed. Open the setup instructions, then try again.", &[&name]);
        self.notice = Some(Notice::Info(detail.clone().into()));
        let answer = crate::questions::container_setup(name).ask(window, cx);
        cx.spawn(async move |_, cx| {
            if answer.await == Some(0) {
                cx.update(|cx| cx.open_url(url)).ok();
            }
        })
        .detach();
        cx.notify();
    }

    pub(super) fn container_target_ready(&mut self, alias: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use host::ssh::Target;
        let (engine, needs_cli) = match host::ssh::target_of(alias) {
            Target::Docker(_) | Target::DockerContext { .. } => (Some(Engine::Docker), false),
            Target::DevContainer { .. } => (Some(Engine::Docker), true),
            Target::NativeContainer { engine, .. } => (Engine::from_id(engine), false),
            Target::DevContainerRuntime { engine, .. } => (Engine::from_id(engine), true),
            _ => (None, false),
        };
        if let Some(engine) = engine.filter(|e| host::find_program(e.program()).is_none()) {
            self.show_container_setup(engine.label(), engine.setup_url(), window, cx);
            return false;
        }
        if needs_cli && host::find_program("devcontainer").is_none() {
            self.show_container_setup("Dev Container CLI", "https://github.com/devcontainers/cli#try-it-out", window, cx);
            return false;
        }
        true
    }

    pub(super) fn show_containers(&mut self, cx: &mut Context<Self>) {
        if !self.docker_available(cx) {
            return;
        }
        self.open_container_dashboard(cx);
        self.remote.docker.monitor.browser.overview.details = false;
    }

    pub(super) fn any_container_connected(&self, cx: &App) -> bool {
        self.docker_connected()
            || cx
                .windows()
                .into_iter()
                .filter(|w| *w != self.window_handle)
                .filter_map(|w| w.downcast::<Workbench>())
                .any(|w| w.read(cx).is_ok_and(|wb| wb.docker_connected()))
    }

    pub(super) fn docker_connected(&self) -> bool {
        match &self.remote.conn {
            Conn::Connected { alias } | Conn::Reconnecting { alias, .. } => docker::is_target(alias),
            Conn::Failed { alias, .. } => docker::is_target(alias) && self.remote.host.is_remote(),
            Conn::Local | Conn::Connecting { .. } => false,
        }
    }

    pub(super) fn docker_available(&mut self, cx: &mut Context<Self>) -> bool {
        if cx.global::<Settings>().docker_enabled {
            return true;
        }
        self.notice = Some(Notice::Info(crate::i18n::t("Container module is off. Enable modules.containers.enabled in Settings.").into()));
        cx.notify();
        false
    }

    pub(super) fn cancel_docker(&mut self, cx: &mut Context<Self>) {
        if !self.docker_available(cx) {
            return;
        }
        self.remote.docker.cancel();
        self.notice = Some(Notice::Info(crate::i18n::t("Container operation cancelled. Existing containers and volumes are kept.").into()));
        cx.notify();
    }

    fn docker_run(
        &mut self,
        command: DockerCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, String, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if !self.docker_available(cx) {
            return;
        }
        if self.remote.docker.cancel.is_some() {
            self.notice = Some(Notice::Info(crate::i18n::t("A container operation is running. Use Docker: Cancel Operation before starting another.").into()));
            cx.notify();
            return;
        }
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.remote.docker.cancel = Some(cancel.clone());
        self.remote.docker.generation += 1;
        let generation = self.remote.docker.generation;
        let is_cli = command.program == "devcontainer";
        let ssh = command.ssh.clone();
        let native_cli = is_cli && command.args.iter().any(|a| a == "--docker-path");
        let runtime = match command.program.as_str() {
            "container" => "Apple Container",
            "wslc.exe" => "WSL Containers",
            "podman" => "Podman",
            _ => "Docker",
        };
        self.log_output(format!(
            "[Containers] {}{} {}",
            ssh.as_ref().map(|s| format!("SSH {s}: ")).unwrap_or_default(),
            command.program,
            command.args.join(" ")
        ));
        self.notice = Some(Notice::Info(crate::i18n::t("Container operation running. See Output for progress; Docker: Cancel Operation stops it.").into()));
        let (tx, rx) = std::sync::mpsc::sync_channel(64);
        let source =
            if command.program == "docker" && command.ssh == self.remote.docker.monitor.host { self.remote.docker.monitor.source.clone() } else { None };
        let task = cx.background_executor().spawn(async move {
            if let Some(source) = source {
                return crate::container_monitor::execute(source.as_ref(), &command, &cancel);
            }
            docker::run(&command, &cancel, |line| {
                let _ = tx.try_send(line);
            })
        });
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            let mut lines = ui::bridge(rx);
            while let Some(line) = lines.next().await {
                if this
                    .update(cx, |this, cx| {
                        if this.remote.docker.generation == generation && cx.global::<Settings>().docker_enabled {
                            this.log_output(format!("[Containers] {line}"));
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.remote.docker.generation != generation || !cx.global::<Settings>().docker_enabled {
                    return;
                }
                this.remote.docker.cancel = None;
                match result {
                    Ok(text) => {
                        this.notice = None;
                        then(this, text, window, cx);
                    }
                    Err(error) => {
                        this.log_output(format!("[Containers] {error}"));
                        let message = if let Some(ssh) = &ssh {
                            crate::i18n::tf("SSH {0}: {1}. Docker must be installed and accessible to the SSH user on the remote host.", &[&ssh, &error])
                        } else if native_cli {
                            crate::i18n::tf("Dev Container setup failed: {}. Check Output and install the Dev Container CLI if it is missing.", &[&error])
                        } else if is_cli {
                            crate::i18n::tf("Docker: {}. Check Output. Dev Container creation requires the separately installed Dev Container CLI.", &[&error])
                        } else if runtime == "Docker" {
                            crate::i18n::tf("Docker: {}. Check Output and verify that the selected engine is running.", &[&error])
                        } else {
                            crate::i18n::tf("{0}: {1}. Start the selected container engine and check Output.", &[&runtime, &error])
                        };
                        this.notice = Some(Notice::Error(message.into()));
                        this.chrome.panel_open = true;
                        this.chrome.bottom = Bottom::Output;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn docker_contexts(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if !self.docker_available(cx) {
            return;
        }
        if self.container_ssh_host().is_some() {
            self.show_containers(cx);
            return;
        }
        let choices = Engine::discover();
        let items = choices
            .iter()
            .map(|choice| PickItem {
                label: choice.engine.label().into(),
                detail: Some(
                    if choice.installed {
                        crate::i18n::t("Installed; choose this container engine")
                    } else {
                        crate::i18n::t("Not installed; open setup instructions")
                    }
                    .into(),
                ),
                keys: None,
            })
            .collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Choose a container engine"), "", None, cx));
        self.show_overlay(picker, OverlayKind::ContainerEngines { choices, action }, window, cx);
    }

    pub(super) fn container_engine_picked(&mut self, choice: crate::container_engine::Choice, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if !self.docker_available(cx) {
            return;
        }
        let engine = choice.engine;
        if !choice.installed || host::find_program(engine.program()).is_none() {
            self.show_container_setup(engine.label(), engine.setup_url(), window, cx);
            return;
        }
        if engine == Engine::Docker {
            self.docker_environments(action, window, cx);
        } else {
            self.docker_context_picked(docker::DockerContext { engine, ssh: None, name: engine.label().into(), endpoint: "local".into() }, action, window, cx);
        }
    }

    fn docker_environments(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let ssh = self.container_ssh_host();
        let mut command = DockerCommand::docker(None, &["context", "ls", "--format", "{{json .}}"]);
        command.ssh = ssh.clone();
        self.docker_run(command, window, cx, move |this, text, window, cx| match docker::json_lines::<docker::DockerContext>(&text) {
            Ok(mut contexts) if !contexts.is_empty() => {
                for context in &mut contexts {
                    context.ssh = ssh.clone();
                }
                let items = contexts
                    .iter()
                    .map(|c| PickItem {
                        label: c.name.clone().into(),
                        detail: Some(format!("{}{}", c.ssh.as_ref().map(|s| format!("SSH: {s} · ")).unwrap_or_default(), c.endpoint).into()),
                        keys: None,
                    })
                    .collect();
                let picker =
                    cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Choose a Docker environment"), "", None, cx));
                this.show_overlay(picker, OverlayKind::DockerContexts { contexts, action }, window, cx);
            }
            Ok(_) => this.notice = Some(Notice::Info(crate::i18n::t("No Docker contexts found. Install Docker and start its engine.").into())),
            Err(e) => this.notice = Some(Notice::Error(crate::i18n::tf("Could not read Docker environments: {}", &[&e]).into())),
        });
    }

    pub(super) fn docker_context_picked(&mut self, context: docker::DockerContext, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if !self.docker_available(cx) {
            return;
        }
        if matches!(action, Action::Services) || context.engine != Engine::Docker {
            self.docker_context_ready(context, action, window, cx);
            return;
        }
        self.docker_run(context.route(DockerCommand::docker(Some(&context.name), &["info", "--format", "{{.OSType}}"])), window, cx, move |this, text, window, cx| {
            if text.trim() != "linux" {
                this.notice = Some(Notice::Info(crate::i18n::t("This engine runs Windows containers. Editing requires a Linux container; Windows container editing needs a separate Windows agent. Container logs remain available.").into()));
            } else {
                this.docker_context_ready(context, action, window, cx);
            }
        });
    }

    fn docker_context_ready(&mut self, context: docker::DockerContext, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::Attach | Action::Services => {
                let services = matches!(action, Action::Services);
                self.docker_run(context.route(context.engine.list(&context.name)), window, cx, move |this, text, window, cx| {
                    match context.engine.containers(&text) {
                        Ok(containers) if !containers.is_empty() => {
                            let items = containers
                                .iter()
                                .map(|c| PickItem {
                                    label: c.name.clone().into(),
                                    detail: Some(format!("{} · {} · {} · {}", c.image, c.state, c.status, c.ports).into()),
                                    keys: None,
                                })
                                .collect();
                            let picker = cx.new(|cx| {
                                Picker::new(
                                    vec![PickSource::List { prefix: "", items }],
                                    crate::i18n::t("Choose a container (stopped containers can be started)"),
                                    "",
                                    None,
                                    cx,
                                )
                            });
                            this.show_overlay(picker, OverlayKind::DockerContainers { context, containers, services }, window, cx);
                        }
                        Ok(_) => {
                            this.notice = Some(Notice::Info(
                                if context.engine == Engine::Docker {
                                    crate::i18n::t("No containers in this Docker environment.")
                                } else {
                                    crate::i18n::t("No containers in this environment.")
                                }
                                .into(),
                            ))
                        }
                        Err(e) => this.notice = Some(Notice::Error(crate::i18n::tf("Could not read containers: {}", &[&e]).into())),
                    }
                });
            }
            Action::Open(project, rebuild) => {
                if context.engine == Engine::Apple {
                    self.notice = Some(Notice::Info(crate::i18n::t("Apple Container supports connecting, starting containers and viewing logs. Dev Container project creation is not supported by this adapter yet; connect to an existing Apple container.").into()));
                    cx.notify();
                    return;
                }
                if project.compose && context.engine == Engine::Wslc {
                    self.notice = Some(Notice::Info(
                        crate::i18n::t("WSL Containers does not support Compose in this workflow. Choose Docker or Podman for a Compose project.").into(),
                    ));
                    cx.notify();
                    return;
                }
                if project.compose && context.engine == Engine::Podman && host::find_program("podman-compose").is_none() {
                    self.notice = Some(Notice::Info(crate::i18n::t("Install podman-compose to open a Compose project with Podman, then try again.").into()));
                    cx.notify();
                    return;
                }
                if context.endpoint.starts_with("ssh:") || context.endpoint.starts_with("tcp:") {
                    self.notice = Some(Notice::Info(
                        crate::i18n::t(
                            "Remote Docker environments currently support attaching to existing containers. Create Dev Containers with a local Docker engine.",
                        )
                        .into(),
                    ));
                    cx.notify();
                    return;
                }
                let label = if rebuild { crate::i18n::t("Rebuild and Open") } else { crate::i18n::t("Build and Open") };
                let detail = crate::i18n::t("Project lifecycle commands will run. Rebuild replaces the container; persistent volumes are kept.");
                let items = vec![
                    PickItem { label: label.into(), detail: Some(detail.into()), keys: None },
                    PickItem { label: crate::i18n::t("Cancel").into(), detail: None, keys: None },
                ];
                let summary = format!("{} · {}\n{}\n{}", project.name, context.name, project.root.display(), project.summary);
                let picker = cx.new(|cx| {
                    Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Review the Dev Container environment"), "", None, cx)
                        .with_message(summary)
                });
                self.show_overlay(picker, OverlayKind::DockerPlan { project, context, rebuild }, window, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn docker_container_picked(
        &mut self,
        context: docker::DockerContext,
        container: docker::Container,
        services: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.docker_available(cx) {
            return;
        }
        if services {
            self.docker_run(context.route(context.engine.logs(&context.name, &container.id)), window, cx, |this, text, _, cx| {
                for line in text.lines() {
                    this.log_output(format!("[Containers] {line}"));
                }
                this.chrome.panel_open = true;
                this.chrome.bottom = Bottom::Output;
                cx.notify();
            });
            return;
        }
        if self.refuse_if_dirty(cx) {
            return;
        }
        let alias = match context.container_alias(&container.id) {
            Ok(alias) => alias,
            Err(e) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("Docker: {}", &[&e]).into()));
                return;
            }
        };
        if container.state == "running" {
            self.connect(alias, window, cx);
        } else if container.state == "exited" || container.state == "created" {
            let answer = crate::questions::start_container(&container.name).ask(window, cx);
            cx.spawn_in(window, async move |this, cx| {
                if answer.await != Some(0) {
                    return;
                }
                this.update_in(cx, |this, window, cx| {
                    this.docker_run(context.route(context.engine.command(&context.name, &["start", &container.id])), window, cx, move |this, _, window, cx| {
                        this.connect(alias, window, cx)
                    });
                })
                .ok();
            })
            .detach();
        } else {
            self.notice = Some(Notice::Info(crate::i18n::t("This container is paused or restarting. Resume it in Docker before connecting.").into()));
        }
        cx.notify();
    }

    pub(super) fn docker_open_project(&mut self, rebuild: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.docker_available(cx) || self.refuse_if_dirty(cx) {
            return;
        }
        if rebuild {
            let Some((project, context)) = self.remote.docker.project.clone() else {
                self.notice = Some(Notice::Info(crate::i18n::t("Open this project with Docker: Open Project in Dev Container first.").into()));
                cx.notify();
                return;
            };
            let project = match docker::Project::discover(&project.root) {
                Ok(p) => p,
                Err(e) => {
                    self.notice = Some(Notice::Error(crate::i18n::tf("Dev Container configuration: {}", &[&e]).into()));
                    cx.notify();
                    return;
                }
            };
            self.docker_context_picked(context, Action::Open(project, true), window, cx);
            return;
        }
        if self.remote.host.is_remote() {
            self.notice = Some(Notice::Info(
                crate::i18n::t(
                    "Open the source folder on this computer to create a Dev Container. Remote Docker contexts support attaching to existing containers.",
                )
                .into(),
            ));
            cx.notify();
            return;
        }
        let Some(root) = self.active_root(cx) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a folder first.").into()));
            cx.notify();
            return;
        };
        match docker::Project::discover(std::path::Path::new(&root)) {
            Ok(project) => self.docker_contexts(Action::Open(project, false), window, cx),
            Err(e) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("Dev Container configuration: {}", &[&e]).into()));
                cx.notify();
            }
        }
    }

    pub(super) fn docker_build(
        &mut self,
        project: docker::Project,
        context: docker::DockerContext,
        rebuild: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.docker_available(cx) || self.refuse_if_dirty(cx) {
            return;
        }
        if host::find_program("devcontainer").is_none() {
            self.show_container_setup("Dev Container CLI", "https://github.com/devcontainers/cli#try-it-out", window, cx);
            return;
        }
        self.require_trust(window, cx, move |this, window, cx| {
            let current_source = !this.remote.host.is_remote() && this.active_root(cx)
                .and_then(|r| std::fs::canonicalize(r).ok()).is_some_and(|r| ui::local_path(&r) == ui::local_path(&project.root));
            let current_container = rebuild && matches!(&this.remote.conn, Conn::Connected { alias } if *alias == project.runtime_alias(&context));
            if !current_source && !current_container {
                this.notice = Some(Notice::Info(crate::i18n::t("The workspace changed. Open the source project and review its container configuration again.").into()));
                cx.notify();
                return;
            }
            let before = this.workspace_key(cx);
            let command = project.runtime_command(&context, rebuild);
            this.docker_run(command, window, cx, move |this, text, window, cx| {
                match docker::opened(&text).and_then(|opened| context.engine.alias(&context.name, &opened.container).map(|_| opened)) {
                    Ok(opened) => {
                        let alias = project.runtime_alias(&context);
                        this.remote.docker.project = Some((project, context));
                        if this.workspace_key(cx) != before || this.any_dirty(cx) {
                            this.notice = Some(Notice::Info(crate::i18n::t("Dev Container is ready. The current workspace or unsaved files changed, so it was not switched automatically. Connect through the Docker picker.").into()));
                        } else {
                            this.remote.pending_path = Some(opened.folder);
                            this.connect(alias, window, cx);
                        }
                    }
                    Err(e) => this.notice = Some(Notice::Error(crate::i18n::tf("Dev Container result: {}", &[&e]).into())),
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn ssh_docker_uses_the_connected_host_and_skips_local_engine_prerequisites(cx: &mut gpui::TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, resp_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || agent::serve(req_r, resp_w));
        let remote = host::RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "ssh://linux-host").unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.remote.host = Arc::new(remote);
            wb.remote.conn = Conn::Connected { alias: "linux-host".into() };
            assert_eq!(wb.container_ssh_host().as_deref(), Some("linux-host"));
            let alias = host::ssh::ssh_docker_target("linux-host", "default", "web").unwrap();
            assert!(wb.container_target_ready(&alias, window, cx));
            wb.remote.conn = Conn::Connected { alias };
            assert_eq!(wb.container_ssh_host().as_deref(), Some("linux-host"));
        });
        assert!(!cx.has_pending_prompt());
    }

    #[gpui::test]
    fn missing_container_engine_shows_setup_without_executing_it(cx: &mut gpui::TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        cx.update(|_, cx| cx.global_mut::<Settings>().docker_enabled = true);
        wb.update_in(cx, |wb, window, cx| {
            wb.container_engine_picked(crate::container_engine::Choice { engine: Engine::Docker, installed: false }, Action::Attach, window, cx)
        });
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        assert!(wb.read_with(cx, |wb, _| wb.output_lines()).is_empty());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
    }
}
