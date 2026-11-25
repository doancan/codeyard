//! The open pull requests of one project as a view of its own (agent mode's PR tab). It reads them the way the Pull
//! Requests list does (`github_listing`: `gh`, else GitHub's public API) and draws rows with their actions.
//!
//! The panel belongs to no window: it holds the project's host and folder, so a hub can draw it for any project.
//! A checkout changes the repository, so it is asked of the project's own workbench through `PullsEvent`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::EventEmitter;

use super::pulls_glue::{PullAction, PullRemote, github_listing};
use super::*;
use crate::git;
use crate::github::{self, Pull};
use crate::i18n::{t, tf};

/// What the panel cannot do itself.
pub(super) enum PullsEvent {
    /// Fetch the request's branch into the project's repository.
    Checkout { remote: PullRemote, pull: Pull },
}

enum Load {
    Loading,
    NoRepository,
    NoGitHub,
    Failed(String),
    Ready { repo: github::Repo, pulls: Vec<Pull>, source: github::Source, remote: PullRemote },
}

/// A listing older than this is read again when the tab is shown.
const STALE_AFTER: Duration = Duration::from_secs(120);

pub(super) struct PullsPanel {
    host: Arc<dyn Host>,
    /// The repository's root; `None` when the folder is not a repository.
    root: Option<String>,
    runner: Arc<dyn github::Runner + Send + Sync>,
    load: Load,
    /// Counts the readings: an answer to an older one is dropped.
    request: u64,
    loaded: Option<Instant>,
    expanded: Option<u64>,
    /// The action that needs no window and says so here (a copied link).
    said: Option<SharedString>,
    reading: Option<gpui::Task<()>>,
}

impl EventEmitter<PullsEvent> for PullsPanel {}

impl PullsPanel {
    pub(super) fn new(host: Arc<dyn Host>, root: Option<String>, runner: Arc<dyn github::Runner + Send + Sync>, cx: &mut Context<Self>) -> Self {
        let mut panel = Self { host, root, runner, load: Load::Loading, request: 0, loaded: None, expanded: None, said: None, reading: None };
        panel.reload(cx);
        panel
    }

    /// Reads the list again when the last one is old; the tab calls this each time it is drawn.
    pub(super) fn reload_if_stale(&mut self, cx: &mut Context<Self>) {
        if self.reading.is_none() && self.loaded.is_some_and(|at| at.elapsed() > STALE_AFTER) {
            self.reload(cx);
        }
    }

    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.load = Load::NoRepository;
            return cx.notify();
        };
        self.request += 1;
        let request = self.request;
        self.load = Load::Loading;
        self.said = None;
        let (host, runner) = (self.host.clone(), self.runner.clone());
        let read = cx.background_executor().spawn(async move {
            let remotes = git::remotes(host.as_ref(), &root)?;
            Ok::<_, String>(github_listing(host.as_ref(), &root, &remotes, runner.as_ref()))
        });
        self.reading = Some(cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                this.reading = None;
                this.loaded = Some(Instant::now());
                this.load = match read {
                    Err(e) => Load::Failed(first_line(&e)),
                    Ok(None) => Load::NoGitHub,
                    Ok(Some(Err(e))) => Load::Failed(first_line(&e)),
                    Ok(Some(Ok((remote, repo, pulls, source)))) => Load::Ready { repo, pulls, source, remote },
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn act(&mut self, pull: &Pull, remote: &PullRemote, action: PullAction, cx: &mut Context<Self>) {
        match action {
            PullAction::Open => cx.open_url(&pull.url),
            PullAction::CopyLink => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(pull.url.clone()));
                self.said = Some(t("Link copied.").into());
            }
            PullAction::CopyBranch => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(pull.head.clone()));
                self.said = Some(tf("Copied {}.", &[&pull.head]).into());
            }
            PullAction::Checkout => cx.emit(PullsEvent::Checkout { remote: remote.clone(), pull: pull.clone() }),
        }
        cx.notify();
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(t("could not load pull requests")).to_string()
}

impl Render for PullsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let note = |text: SharedString| div().p(px(16.)).text_color(c(p.text_muted)).child(text);
        let root = div().id("pulls-panel").debug_selector(|| "pulls-panel".into()).size_full().flex().flex_col().min_h_0();
        let mut header = div().flex_none().flex().items_center().gap(px(8.)).px(px(12.)).py(px(6.)).border_b_1().border_color(c(p.border));
        header = header.child(div().flex_1().font_weight(gpui::FontWeight::SEMIBOLD).child(t("Pull requests"))).child(
            div()
                .id("pulls-refresh")
                .px(px(8.))
                .py(px(2.))
                .rounded(px(theme::radius::MD))
                .cursor_pointer()
                .text_color(c(p.text_muted))
                .hover(|d| d.bg(c(p.surface_hover)).text_color(c(p.text)))
                .child(t("Refresh"))
                .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
        );
        let body = match &self.load {
            Load::Loading => note(t("Loading pull requests…").into()).into_any_element(),
            Load::NoRepository => note(t("This folder is not a git repository.").into()).into_any_element(),
            Load::NoGitHub => note(t("This project has no GitHub remote.").into()).into_any_element(),
            Load::Failed(message) => note(SharedString::from(message.clone())).text_color(c(p.danger)).into_any_element(),
            Load::Ready { pulls, .. } if pulls.is_empty() => note(t("No open pull requests.").into()).into_any_element(),
            Load::Ready { repo, pulls, source, remote } => {
                let now = now_secs();
                let via = match source {
                    github::Source::Cli => t("via the gh CLI"),
                    github::Source::Api { authenticated: true } => t("via the GitHub API"),
                    github::Source::Api { authenticated: false } => t("via the public GitHub API"),
                };
                let mut list = div().id("pulls-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
                list = list.child(div().px(px(12.)).py(px(6.)).text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(format!("{} · {via}", repo.slug())));
                for pull in pulls {
                    let open = self.expanded == Some(pull.number);
                    let number = pull.number;
                    let checks = pull.checks.as_ref().map(|k| (k.passed, k.failed, k.pending));
                    let mut row =
                        div().id(("pull", number as usize)).px(px(12.)).py(px(6.)).border_b_1().border_color(c(p.border)).flex().flex_col().gap(px(2.)).child(
                            div()
                                .id(("pull-title", number as usize))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.expanded = if this.expanded == Some(number) { None } else { Some(number) };
                                    this.said = None;
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(6.))
                                        .child(div().text_color(c(p.text_muted)).child(format!("#{number}")))
                                        .child(div().flex_1().child(pull.title.clone()))
                                        .children(pull.draft.then(|| div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(t("Draft")))),
                                )
                                .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(github::detail(pull, now))),
                        );
                    if let Some((passed, failed, pending)) = checks {
                        let dot = |color: u32, n: u32| {
                            div().flex().items_center().gap(px(3.)).child(div().size(px(7.)).rounded_full().bg(c(color))).child(n.to_string())
                        };
                        row = row.child(
                            div()
                                .flex()
                                .gap(px(10.))
                                .text_size(px(text::XS.0))
                                .text_color(c(p.text_muted))
                                .when(passed > 0, |d| d.child(dot(p.success, passed)))
                                .when(failed > 0, |d| d.child(dot(p.danger, failed)))
                                .when(pending > 0, |d| d.child(dot(p.warning, pending))),
                        );
                    }
                    if open {
                        let actions = [
                            (PullAction::Checkout, tf("Check out #{} as pr-{}", &[&number, &number])),
                            (PullAction::Open, t("Open on GitHub").to_string()),
                            (PullAction::CopyLink, t("Copy link").to_string()),
                            (PullAction::CopyBranch, t("Copy branch name").to_string()),
                        ];
                        row = row.child(div().mt(px(4.)).flex().flex_wrap().gap(px(6.)).children(actions.into_iter().enumerate().map(
                            |(ix, (action, label))| {
                                let (pull, remote) = (pull.clone(), remote.clone());
                                div()
                                    .id(("pull-action", number as usize * 8 + ix))
                                    .px(px(8.))
                                    .py(px(3.))
                                    .rounded(px(theme::radius::MD))
                                    .border_1()
                                    .border_color(c(p.border))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(c(p.surface_hover)))
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| this.act(&pull, &remote, action, cx)))
                            },
                        )));
                        if let Some(said) = self.said.clone() {
                            row = row.child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(said));
                        }
                    }
                    list = list.child(row);
                }
                list.into_any_element()
            }
        };
        root.child(header).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    /// What `gh` and `curl` would answer.
    struct Canned {
        gh: Result<String, String>,
    }

    impl github::Runner for Canned {
        fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
            match (program, args.first().map(String::as_str)) {
                ("gh", Some("auth")) => Ok(String::new()),
                ("gh", _) => self.gh.clone(),
                _ => Err("no network".into()),
            }
        }

        fn run_with_input(&self, program: &str, args: &[String], _: &str) -> Result<String, String> {
            self.run(program, args)
        }
    }

    const OPEN: &str = r#"[{"number": 7, "title": "Faster invoices", "author": {"login": "ayse"}, "headRefName": "pdf-speed", "baseRefName": "main", "isDraft": false,
        "updatedAt": "2026-11-24T10:00:00Z", "url": "https://github.com/o/r/pull/7", "reviewDecision": null,
        "statusCheckRollup": [{"status": "COMPLETED", "conclusion": "SUCCESS"}, {"status": "COMPLETED", "conclusion": "FAILURE"}]}]"#;

    fn host() -> Arc<dyn Host> {
        Arc::new(host::local())
    }

    fn git_in(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git").args(args).current_dir(dir).output().expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn repo(name: &str, remote: Option<&str>) -> String {
        let dir = crate::qa_integration_tests::repo(name);
        if let Some(url) = remote {
            git_in(&dir, &["remote", "add", "origin", url]);
        }
        dir.to_string_lossy().into_owned()
    }

    fn wait_for(cx: &mut TestAppContext, what: &str, mut done: impl FnMut(&mut TestAppContext) -> bool) {
        let start = Instant::now();
        while !done(cx) {
            assert!(start.elapsed() < Duration::from_secs(20), "timeout: {what}");
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn panel(cx: &mut TestAppContext, root: Option<String>, gh: Result<&str, &str>) -> Entity<PullsPanel> {
        crate::qa_workbench_tests::globals(cx);
        let runner = Arc::new(Canned { gh: gh.map(String::from).map_err(String::from) });
        let (panel, _) = cx.add_window_view(|_, cx| PullsPanel::new(host(), root, runner, cx));
        panel
    }

    fn loaded_title(panel: &Entity<PullsPanel>, cx: &mut TestAppContext) -> Option<String> {
        panel.read_with(cx, |p, _| match &p.load {
            Load::Ready { pulls, .. } => pulls.first().map(|pull| pull.title.clone()),
            _ => None,
        })
    }

    #[gpui::test]
    fn a_project_on_github_lists_its_open_pull_requests(cx: &mut TestAppContext) {
        let panel = panel(cx, Some(repo("pulls-panel-ok", Some("https://github.com/o/r.git"))), Ok(OPEN));
        wait_for(cx, "the list", |cx| loaded_title(&panel, cx).is_some());
        assert_eq!(loaded_title(&panel, cx).as_deref(), Some("Faster invoices"));
    }

    #[gpui::test]
    fn a_project_without_a_github_remote_says_so_and_asks_nothing(cx: &mut TestAppContext) {
        let panel = panel(cx, Some(repo("pulls-panel-none", None)), Err("must not be asked"));
        wait_for(cx, "the answer", |cx| panel.read_with(cx, |p, _| matches!(p.load, Load::NoGitHub)));
    }

    #[gpui::test]
    fn a_folder_that_is_not_a_repository_says_so(cx: &mut TestAppContext) {
        let panel = panel(cx, None, Err("must not be asked"));
        cx.run_until_parked();
        assert!(panel.read_with(cx, |p, _| matches!(p.load, Load::NoRepository)));
    }

    #[gpui::test]
    fn a_failed_listing_shows_its_reason_and_the_refresh_reads_again(cx: &mut TestAppContext) {
        // `gh` fails and the public API is unreachable (no network): the reason is shown, not an empty list.
        let panel = panel(cx, Some(repo("pulls-panel-fail", Some("https://github.com/o/r.git"))), Err("no access"));
        wait_for(cx, "the failure", |cx| panel.read_with(cx, |p, _| matches!(p.load, Load::Failed(_))));
        let before = panel.read_with(cx, |p, _| p.request);
        panel.update(cx, |p, cx| p.reload(cx));
        assert_eq!(panel.read_with(cx, |p, _| p.request), before + 1);
        assert!(panel.read_with(cx, |p, _| matches!(p.load, Load::Loading)), "reading again shows that it is reading");
        wait_for(cx, "the failure again", |cx| panel.read_with(cx, |p, _| matches!(p.load, Load::Failed(_))));
    }

    #[gpui::test]
    fn the_rows_draw_with_their_actions_and_a_checkout_is_left_to_the_project(cx: &mut TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let root = repo("pulls-panel-draw", Some("https://github.com/o/r.git"));
        let runner = Arc::new(Canned { gh: Ok(OPEN.to_string()) });
        let (panel, vcx) = cx.add_window_view(|_, cx| PullsPanel::new(host(), Some(root), runner, cx));
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u64>::new()));
        let seen = events.clone();
        vcx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &PullsEvent, _| {
                let PullsEvent::Checkout { pull, .. } = event;
                seen.borrow_mut().push(pull.number);
            })
            .detach();
        });
        let start = Instant::now();
        while loaded_title(&panel, vcx).is_none() {
            assert!(start.elapsed() < Duration::from_secs(20), "timeout: the list");
            vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(20));
        }
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pulls-panel").is_some(), "the panel is drawn");
        // The row opens its actions; the checkout is passed on, not carried out here.
        panel.update(vcx, |p, cx| {
            p.expanded = Some(7);
            let (pull, remote) = match &p.load {
                Load::Ready { pulls, remote, .. } => (pulls[0].clone(), remote.clone()),
                _ => unreachable!("loaded above"),
            };
            p.act(&pull, &remote, PullAction::Checkout, cx);
        });
        assert_eq!(*events.borrow(), [7]);
        panel.update(vcx, |p, cx| {
            let (pull, remote) = match &p.load {
                Load::Ready { pulls, remote, .. } => (pulls[0].clone(), remote.clone()),
                _ => unreachable!("loaded above"),
            };
            p.act(&pull, &remote, PullAction::CopyLink, cx);
            assert_eq!(p.said.as_ref().map(|s| s.to_string()).as_deref(), Some("Link copied."));
        });
        let _: &mut VisualTestContext = vcx;
    }
}
