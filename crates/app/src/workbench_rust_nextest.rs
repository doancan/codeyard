//! Workbench: running Rust tests through cargo-nextest when `rust.testRunner` and the machine allow it. Whether
//! `cargo nextest` exists is asked once per folder (and only a yes is remembered), on the machine that runs the tests.

use gpui::{Context, Window};

use super::super::*;
use crate::i18n::t;
use crate::rust_toolchain::nextest;
use crate::testexp::{Job, Kind};

/// Folders `probe` was asked about, for tests that check nothing runs in a folder that is not trusted.
#[cfg(test)]
pub(super) static PROBED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Whether `cargo nextest` runs on the machine behind `host`, within `rust_probe::LIMIT`. Cargo reads the folder's
/// configuration (aliases, a pinned toolchain), so this runs only in a folder the user trusts.
pub(super) fn probe(host: Arc<dyn Host>, root: &str) -> bool {
    #[cfg(test)]
    PROBED.lock().unwrap().push(root.to_string());
    let root = root.to_string();
    crate::rust_toolchain::probe::within(crate::rust_toolchain::probe::LIMIT, move || {
        host.exec("cargo", &["nextest", "--version"], Some(&root), None).is_ok_and(|output| output.ok())
    })
    .unwrap_or(false)
}

/// What `use_nextest` found out in one look at the workbench.
enum Plan {
    /// Nextest is not used (the setting says cargo, or the folder is not trusted).
    No,
    Known(bool),
    Ask(gpui::Task<bool>),
}

/// Whether tests in `root` run through nextest: `rust.testRunner` says so and, for `auto` and `nextest`, `cargo nextest`
/// is installed on the machine that runs them. Asked off the UI thread and only in a trusted folder; the answer, yes or
/// no, is remembered (a no for a few minutes) so that repeated runs do not start Cargo each time.
pub(in super::super) async fn use_nextest(this: &gpui::WeakEntity<Workbench>, cx: &mut gpui::AsyncApp, root: &str) -> bool {
    let asked = root.to_string();
    let Ok((runner, plan)) = this.update(cx, |this, cx| {
        let runner = cx.global::<Settings>().rust_build.test_runner;
        let plan = if runner == nextest::TestRunner::Cargo || !this.is_trusted_root(&asked, cx) {
            Plan::No
        } else if let Some(found) = this.lsp.rust.nextest.known(&asked, std::time::Instant::now()) {
            Plan::Known(found)
        } else {
            this.notice = Some(Notice::Info(t("Looking for cargo-nextest…").into()));
            cx.notify();
            let host = this.remote.host.clone();
            Plan::Ask(cx.background_executor().spawn(async move { probe(host, &asked) }))
        };
        (runner, plan)
    }) else {
        return false;
    };
    let (found, probed) = match plan {
        Plan::No => return false,
        Plan::Known(found) => (found, false),
        Plan::Ask(task) => (task.await, true),
    };
    let remembered = root.to_string();
    this.update(cx, |this, cx| {
        if probed {
            this.lsp.rust.nextest.remember(&remembered, found, std::time::Instant::now());
            this.notice = None;
        }
        if !found && runner.insists() {
            this.notice =
                Some(Notice::Info(t("cargo-nextest was not found, so the tests run with cargo test. Install it with `cargo install cargo-nextest`.").into()));
        }
        cx.notify();
    })
    .ok();
    runner.uses_nextest(found)
}

impl Workbench {
    /// `then` with whether tests in `root` run through nextest. When the setting asks for nextest in a folder that is not
    /// trusted, the trust question comes first: nothing of the project runs before it.
    pub(in super::super) fn with_nextest_choice(
        &mut self,
        root: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, bool, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if cx.global::<Settings>().rust_build.test_runner == nextest::TestRunner::Cargo {
            return then(self, false, window, cx);
        }
        if !self.is_trusted_root(&root, cx) {
            let again = root.clone();
            return self.require_trust_for(Some(root), window, cx, move |this, window, cx| this.with_nextest_choice(again, window, cx, then));
        }
        let generation = self.lsp.generation;
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let nextest = use_nextest(&this, cx, &root).await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if this.lsp.generation == generation {
                        then(this, nextest, window, cx);
                    }
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    /// Whether any of the jobs is a Rust one (the others never need to know about nextest).
    pub(in super::super) fn has_rust_jobs(jobs: &[Job]) -> bool {
        jobs.iter().any(|job| job.kind == Kind::Rust)
    }

    /// The jobs with their Rust commands as nextest commands (what nextest cannot run stays as it is).
    pub(in super::super) fn nextest_jobs(jobs: Vec<Job>, windows: bool) -> Vec<Job> {
        jobs.into_iter()
            .map(|job| match job.kind {
                Kind::Rust => Job { command: nextest::convert(&job.command, windows, true).unwrap_or(job.command), ..job },
                _ => job,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(kind: Kind, command: &str) -> Job {
        Job { kind, command: command.into(), scope: Vec::new() }
    }

    #[test]
    fn only_rust_commands_nextest_can_run_are_converted() {
        let jobs = vec![
            job(Kind::Rust, "cargo test -p app problems:: "),
            job(Kind::Rust, "cargo test --doc"),
            job(Kind::Go, "go test ./..."),
            job(Kind::Rust, "cargo test --workspace --no-fail-fast"),
        ];
        let got: Vec<String> = Workbench::nextest_jobs(jobs, false).into_iter().map(|j| j.command).collect();
        assert_eq!(
            got,
            [
                "cargo nextest run --no-fail-fast --status-level skip -p app problems::",
                "cargo test --doc",
                "go test ./...",
                "cargo nextest run --status-level skip --workspace --no-fail-fast"
            ]
        );
        assert!(Workbench::has_rust_jobs(&[job(Kind::Go, "go test"), job(Kind::Rust, "cargo test")]));
        assert!(!Workbench::has_rust_jobs(&[job(Kind::Go, "go test")]));
    }
}
