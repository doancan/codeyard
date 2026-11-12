//! Workbench: *Rust: Select Target…* lists the targets installed for the project's toolchain and writes the choice to
//! `rust.target`, which Run, Debug, tests and the runnables build with.

use gpui::{Context, Window};
use serde_json::Value;

use super::super::*;
use crate::i18n::{t, tf};
use crate::picker::{PickItem, Picker, Source as PickSource};
use crate::rust_toolchain::target;

gpui::actions!(workbench, [RustSelectTarget]);

/// What listing the installed targets found.
enum Listing {
    Installed(Vec<String>),
    /// Neither rustup nor rustc could be run: the text to show.
    Failed(String),
}

/// `rustup target list --installed`; without rustup (a distribution's own Rust), the host `rustc` builds for is the one
/// target known. Each tool gets `rust_probe::LIMIT` to answer.
fn list_installed(host: Arc<dyn Host>, root: &str) -> Listing {
    let run = |program: &'static str, args: &'static [&'static str]| {
        let (host, root) = (host.clone(), root.to_string());
        crate::rust_toolchain::probe::within(crate::rust_toolchain::probe::LIMIT, move || host.exec(program, args, Some(&root), None))
    };
    let waited = || Listing::Failed(tf("{} did not answer within 5 seconds", &[&"rustup"]));
    match run("rustup", &["target", "list", "--installed"]) {
        Some(Ok(output)) if output.ok() => return Listing::Installed(target::parse_installed(&output.stdout_str())),
        None => return waited(),
        // Not installed, or failing: the compiler's own host is still known.
        Some(_) => {}
    }
    match run("rustc", &["-vV"]) {
        Some(Ok(output)) if output.ok() => Listing::Installed(target::parse_host(&output.stdout_str()).into_iter().collect()),
        Some(Ok(output)) => Listing::Failed(crate::cargo_project::explain_failure(&format!("{}{}", output.stdout_str(), output.stderr_str()))),
        Some(Err(e)) => Listing::Failed(crate::cargo_project::explain_failure(&e.to_string())),
        None => Listing::Failed(tf("{} did not answer within 5 seconds", &[&"rustc"])),
    }
}

impl Workbench {
    pub(in super::super) fn rust_select_target(&mut self, _: &RustSelectTarget, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.active_root(cx) else {
            self.notice = Some(Notice::Info(t("Open a folder first").into()));
            cx.notify();
            return;
        };
        // rustup can install the toolchain a project pins, and runs it: only in a folder the user trusts.
        self.require_trust_for(Some(root.clone()), window, cx, move |this, window, cx| this.list_rust_targets(root, window, cx));
    }

    fn list_rust_targets(&mut self, root: String, window: &mut Window, cx: &mut Context<Self>) {
        self.notice = Some(Notice::Info(t("Looking for installed targets…").into()));
        cx.notify();
        let host = self.remote.host.clone();
        let generation = self.lsp.generation;
        let listing = cx.background_executor().spawn(async move { list_installed(host, &root) });
        cx.spawn_in(window, async move |this, cx| {
            let listing = listing.await;
            this.update_in(cx, |this, window, cx| {
                if this.lsp.generation != generation {
                    return;
                }
                match listing {
                    Listing::Failed(why) => {
                        this.notice = Some(Notice::Error(tf("Could not list the installed targets: {}", &[&why]).into()));
                        cx.notify();
                    }
                    Listing::Installed(installed) => {
                        this.notice = None;
                        this.show_rust_targets(installed, window, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn show_rust_targets(&mut self, installed: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let current = cx.global::<Settings>().rust_build.target.clone();
        let rows = target::choices(&installed, current.as_deref());
        let items = rows
            .iter()
            .map(|row| PickItem {
                label: row.triple.clone().unwrap_or_else(|| t("Default: build for this machine").to_string()).into(),
                detail: row.current.then(|| t("current").into()),
                keys: None,
            })
            .collect();
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], t("Build for target"), "", None, cx));
        let triples = rows.into_iter().map(|row| row.triple).collect();
        self.show_overlay(picker, OverlayKind::RustChoice(super::Choice::Targets(triples)), window, cx);
    }

    pub(super) fn chose_rust_target(&mut self, triples: Vec<Option<String>>, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(triple) = triples.get(index) else { return };
        self.write_setting("rust.target", triple.clone().map_or(Value::Null, Value::String), window, cx);
    }
}
