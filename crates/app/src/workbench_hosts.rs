//! Workbench: SSH host management: add, remove, test the connection, open `~/.ssh/config`.

use host::ssh_config as sshcfg;

use super::*;
use crate::i18n::tr;

/// What the host picker will do.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum HostOp {
    Remove,
    Test,
    /// Edit fields such as port, user, key file, ProxyJump.
    Edit,
    /// Copy the public key to the server: passwordless login.
    KeySetup,
}

/// Actions in the "Manage SSH hosts…" list.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum HostMenu {
    Add,
    Edit,
    Test,
    Key,
    Remove,
    Config,
}

impl HostMenu {
    fn describe(self) -> (&'static str, &'static str) {
        match self {
            HostMenu::Add => (tr("Add a host"), tr("user@host or user@host:port as name")),
            HostMenu::Edit => (tr("Edit a host"), tr("port, user, key file, jump host")),
            HostMenu::Test => (tr("Test a connection"), tr("reachable? architecture? agent ready?")),
            HostMenu::Key => (tr("Set up key login"), tr("copy your public key so no password is needed")),
            HostMenu::Remove => (tr("Remove a host"), tr("from ~/.ssh/config (a backup is kept)")),
            HostMenu::Config => (tr("Open ~/.ssh/config"), tr("edit the file by hand")),
        }
    }
}

impl Workbench {
    /// "Remote: Add SSH Host…": `user@host[:port] [as name]` → a block in `~/.ssh/config`.
    pub(super) fn add_ssh_host(&mut self, _: &AddSshHost, window: &mut Window, cx: &mut Context<Self>) {
        self.show_input(
            InputPurpose::AddSshHost,
            crate::i18n::t("Add"),
            crate::i18n::t("user@host, user@host:2222, or user@host as name  (saved to ~/.ssh/config)"),
            "",
            window,
            cx,
        );
    }

    pub(super) fn finish_add_ssh_host(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let result = sshcfg::parse_target(&text).and_then(|h| {
            let path = sshcfg::config_path().ok_or(crate::i18n::t("Could not find your home folder"))?;
            sshcfg::add_host(&path, &h).map(|()| h)
        });
        match result {
            Ok(h) => {
                let alias = h.alias.clone();
                let detail =
                    crate::i18n::t("Saved to ~/.ssh/config. Connecting uses your SSH keys and ssh-agent; a password prompt appears in this window if needed.");
                let answer = window.prompt(
                    gpui::PromptLevel::Info,
                    &crate::i18n::tf("Added “{}”", &[&alias]),
                    Some(detail),
                    &[crate::i18n::t("Connect"), crate::i18n::t("Not Now")],
                    cx,
                );
                cx.spawn_in(window, async move |this, cx| {
                    if answer.await == Ok(0) {
                        this.update_in(cx, |this, window, cx| this.connect(alias, window, cx)).ok();
                    }
                })
                .detach();
            }
            Err(e) => {
                self.notice = Some(Notice::Error(e.into()));
                cx.notify();
            }
        }
    }

    pub(super) fn remove_ssh_host(&mut self, _: &RemoveSshHost, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_ssh_host(HostOp::Remove, crate::i18n::t("Remove which SSH host from ~/.ssh/config?"), window, cx);
    }

    pub(super) fn test_ssh_host(&mut self, _: &TestSshHost, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_ssh_host(HostOp::Test, crate::i18n::t("Test the connection to which SSH host?"), window, cx);
    }

    /// "Manage SSH hosts…": the list of host actions.
    pub(super) fn show_host_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choices = vec![HostMenu::Add, HostMenu::Edit, HostMenu::Test, HostMenu::Key, HostMenu::Remove, HostMenu::Config];
        let items = choices
            .iter()
            .map(|c| {
                let (label, detail) = c.describe();
                PickItem { label: crate::i18n::t(label).into(), detail: Some(crate::i18n::t(detail).into()), keys: None }
            })
            .collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Manage SSH hosts"), "", None, cx));
        self.show_overlay(picker, OverlayKind::HostMenu(choices), window, cx);
    }

    pub(super) fn run_host_menu(&mut self, choice: HostMenu, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            HostMenu::Add => self.add_ssh_host(&AddSshHost, window, cx),
            HostMenu::Edit => self.edit_ssh_host(&EditSshHost, window, cx),
            HostMenu::Test => self.test_ssh_host(&TestSshHost, window, cx),
            HostMenu::Key => self.setup_key_login(&SetupKeyLogin, window, cx),
            HostMenu::Remove => self.remove_ssh_host(&RemoveSshHost, window, cx),
            HostMenu::Config => self.edit_ssh_config(&EditSshConfig, window, cx),
        }
    }

    pub(super) fn edit_ssh_host(&mut self, _: &EditSshHost, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_ssh_host(HostOp::Edit, crate::i18n::t("Edit which SSH host?"), window, cx);
    }

    pub(super) fn setup_key_login(&mut self, _: &SetupKeyLogin, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_ssh_host(HostOp::KeySetup, crate::i18n::t("Set up key login (no password) for which SSH host?"), window, cx);
    }

    fn pick_ssh_host(&mut self, op: HostOp, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let hosts = host::ssh::list_hosts();
        if hosts.is_empty() {
            self.notice = Some(Notice::Info(crate::i18n::t("No SSH hosts yet. Use Remote: Add SSH Host…").into()));
            cx.notify();
            return;
        }
        let items = hosts
            .iter()
            .map(|h| {
                let detail = match (&h.user, &h.hostname) {
                    (Some(u), Some(n)) => Some(format!("{u}@{n}")),
                    (None, Some(n)) => Some(n.clone()),
                    _ => None,
                };
                PickItem { label: h.alias.clone().into(), detail: detail.map(Into::into), keys: None }
            })
            .collect();
        let aliases = hosts.into_iter().map(|h| h.alias).collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], title, "", None, cx));
        self.show_overlay(picker, OverlayKind::HostOp { op, aliases }, window, cx);
    }

    pub(super) fn run_host_op(&mut self, op: HostOp, alias: String, window: &mut Window, cx: &mut Context<Self>) {
        match op {
            HostOp::Test => self.probe_host(alias, cx),
            HostOp::Edit => self.pick_host_field(alias, window, cx),
            HostOp::KeySetup => self.confirm_key_setup(alias, window, cx),
            HostOp::Remove => {
                let in_use = match &self.remote.conn {
                    Conn::Connected { alias: a } | Conn::Connecting { alias: a, .. } | Conn::Reconnecting { alias: a, .. } => *a == alias,
                    _ => false,
                };
                if in_use {
                    self.notice = Some(Notice::Error(crate::i18n::tf("You are connected to “{}”. Disconnect first.", &[&alias]).into()));
                    cx.notify();
                    return;
                }
                let answer = window.prompt(
                    gpui::PromptLevel::Warning,
                    &crate::i18n::tf("Remove “{}” from ~/.ssh/config?", &[&alias]),
                    Some(crate::i18n::t("Only this host's entry is removed. The previous file is kept as config.codeyard-bak.")),
                    &[crate::i18n::t("Remove"), crate::i18n::t("Cancel")],
                    cx,
                );
                cx.spawn_in(window, async move |this, cx| {
                    if answer.await != Ok(0) {
                        return;
                    }
                    this.update(cx, |this, cx| {
                        let result = sshcfg::config_path()
                            .ok_or_else(|| crate::i18n::t("Could not find your home folder").to_string())
                            .and_then(|p| sshcfg::remove_host(&p, &alias));
                        this.notice = Some(match result {
                            Ok(true) => Notice::Info(crate::i18n::tf("Removed “{}” from ~/.ssh/config", &[&alias]).into()),
                            Ok(false) => Notice::Info(crate::i18n::tf("“{}” was not found in ~/.ssh/config", &[&alias]).into()),
                            Err(e) => Notice::Error(e.into()),
                        });
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// Test the connection: auth + architecture + agent package present. Result goes to the status bar and error card.
    fn probe_host(&mut self, alias: String, cx: &mut Context<Self>) {
        self.notice = Some(Notice::Info(crate::i18n::tf("Testing {}…", &[&alias]).into()));
        cx.notify();
        let bundle = self.remote.bundle.clone();
        let a = alias.clone();
        let task = cx.background_executor().spawn(async move { host::ssh::probe(&a, &bundle) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                // The Remote Explorer shows the outcome on the host's row.
                let reachable = match &result {
                    Ok(p) if p.agent_bundled => Ok(()),
                    Ok(p) => Err(p.uname.clone()),
                    Err(e) => Err(e.to_string()),
                };
                this.remote.view.probes.insert(alias.clone(), reachable);
                this.notice = Some(match result {
                    Ok(p) if p.agent_bundled => Notice::Info(crate::i18n::tf("{}: reachable in {} ms · {} · ready", &[&alias, &p.millis, &p.uname]).into()),
                    Ok(p) => Notice::Error(
                        match p.triple {
                            Some(t) => crate::i18n::tf("{}: reachable ({}), but this app has no agent for {}", &[&alias, &p.uname, &t]),
                            None => crate::i18n::tf("{}: reachable, but {} is not a supported server", &[&alias, &p.uname]),
                        }
                        .into(),
                    ),
                    Err(e) => Notice::Error(crate::i18n::tf("{}: {}", &[&alias, &e]).into()),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open `~/.ssh/config` on this machine (also while connected over SSH).
    pub(super) fn edit_ssh_config(&mut self, _: &EditSshConfig, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = sshcfg::config_path() else {
            self.notice = Some(Notice::Error(crate::i18n::t("Could not find your home folder").into()));
            cx.notify();
            return;
        };
        if let Err(e) = sshcfg::ensure_exists(&path) {
            self.notice = Some(Notice::Error(e.into()));
            cx.notify();
            return;
        }
        let host = self.remote.local_host.clone();
        self.open_file_on(host, path.to_string_lossy().into_owned(), window, cx);
    }

    /// Pick the field to edit (with the current values).
    fn pick_host_field(&mut self, alias: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = sshcfg::config_path() else { return };
        let Some(d) = sshcfg::details(&path, &alias) else {
            self.notice = Some(Notice::Error(
                crate::i18n::tf("“{}” is not in the SSH config (or shares a Host line with other names: edit the file).", &[&alias]).into(),
            ));
            cx.notify();
            return;
        };
        let items = sshcfg::Field::ALL
            .iter()
            .map(|f| PickItem {
                label: f.key().into(),
                detail: Some(d.get(*f).map_or_else(|| crate::i18n::t("not set").to_string(), str::to_string).into()),
                keys: None,
            })
            .collect();
        let picker =
            cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], &crate::i18n::tf("Edit “{}”: which setting?", &[&alias]), "", None, cx));
        self.show_overlay(picker, OverlayKind::HostField { alias, fields: sshcfg::Field::ALL.to_vec() }, window, cx);
    }

    pub(super) fn edit_host_field(&mut self, alias: String, field: sshcfg::Field, window: &mut Window, cx: &mut Context<Self>) {
        let current = sshcfg::config_path().and_then(|p| sshcfg::details(&p, &alias)).and_then(|d| d.get(field).map(str::to_string)).unwrap_or_default();
        self.show_input(
            InputPurpose::EditHostField { alias, field },
            crate::i18n::t("Save"),
            &crate::i18n::tf("{} (empty removes the setting)", &[&field.hint()]),
            &current,
            window,
            cx,
        );
    }

    pub(super) fn finish_edit_host_field(&mut self, alias: String, field: sshcfg::Field, value: String, cx: &mut Context<Self>) {
        let value = value.trim().to_string();
        let result = sshcfg::config_path()
            .ok_or_else(|| crate::i18n::t("Could not find your home folder").to_string())
            .and_then(|p| sshcfg::set_field(&p, &alias, field, Some(&value)));
        self.notice = Some(match result {
            Ok(()) if value.is_empty() => Notice::Info(crate::i18n::tf("Removed {} from “{}”.", &[&field.key(), &alias]).into()),
            Ok(()) => Notice::Info(crate::i18n::tf("“{}”: {} = {}", &[&alias, &field.key(), &value]).into()),
            Err(e) => Notice::Error(e.into()),
        });
        cx.notify();
    }

    fn confirm_key_setup(&mut self, alias: String, window: &mut Window, cx: &mut Context<Self>) {
        let detail = crate::i18n::t(
            "Your public key (~/.ssh/id_ed25519.pub; created if you have none) is added to the server's authorized_keys, so later connections need no password. You may be asked for the server's password once.",
        );
        let answer = window.prompt(
            gpui::PromptLevel::Info,
            &crate::i18n::tf("Set up key login for “{}”?", &[&alias]),
            Some(detail),
            &[crate::i18n::t("Set Up"), crate::i18n::t("Cancel")],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| {
                    let dir = dirs::home_dir().map(|h| h.join(".ssh"));
                    this.run_key_setup(alias, dir, None, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    /// Find/generate the key and copy it to the server (in the background). `authorized_keys`: path on the server, for tests.
    pub(super) fn run_key_setup(&mut self, alias: String, ssh_dir: Option<std::path::PathBuf>, authorized_keys: Option<String>, cx: &mut Context<Self>) {
        let Some(dir) = ssh_dir else {
            self.notice = Some(Notice::Error(crate::i18n::t("Could not find your home folder").into()));
            cx.notify();
            return;
        };
        self.notice = Some(Notice::Info(crate::i18n::tf("Setting up key login for {}…", &[&alias]).into()));
        cx.notify();
        let a = alias.clone();
        let task = cx.background_executor().spawn(async move {
            let (line, path, generated) = sshcfg::ensure_key(&dir)?;
            let added = host::ssh::install_public_key(&a, &line, authorized_keys.as_deref()).map_err(|e| e.to_string())?;
            Ok::<_, String>((path, generated, added))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.notice = Some(match result {
                    Ok((path, generated, added)) => {
                        let shown = path.display();
                        Notice::Info(
                            match (added, generated) {
                                (true, true) => {
                                    crate::i18n::tf("Key login is set up for “{}” (created {}). The next connection needs no password.", &[&alias, &shown])
                                }
                                (true, false) => crate::i18n::tf("Key login is set up for “{}”. The next connection needs no password.", &[&alias]),
                                (false, true) => crate::i18n::tf(
                                    "Key login was already set up for “{}” (created {}). The next connection needs no password.",
                                    &[&alias, &shown],
                                ),
                                (false, false) => crate::i18n::tf("Key login was already set up for “{}”. The next connection needs no password.", &[&alias]),
                            }
                            .into(),
                        )
                    }
                    Err(e) => Notice::Error(
                        crate::i18n::tf("Could not set up key login for {}: {}", &[&alias, &e.lines().last().unwrap_or(crate::i18n::t("failed"))]).into(),
                    ),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
