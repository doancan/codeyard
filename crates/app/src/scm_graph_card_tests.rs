//! The hover card of a commit in the graph (#307): what it reads, what it shows for the awkward commits (a merge, the first
//! commit, a binary file, a huge one) and that hovering again does not read again. Counts are checked against git's own.

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::graph::Scope;
use crate::qa_integration_tests::{git_in, repo, wait_ticking};
use crate::qa_workbench_tests::globals;

fn commit_of(dir: &std::path::Path, rev: &str) -> GraphCommit {
    let hash = git_in(dir, &["rev-parse", rev]).trim().to_string();
    let host = host::local();
    graph::log(&host, &ui::local_path(dir), 200, Scope::All).unwrap().into_iter().find(|c| c.hash == hash).unwrap()
}

fn source(dir: &std::path::Path) -> CardSource {
    CardSource::new(Arc::new(host::local()), ui::local_path(dir), CardCache::default())
}

fn card(cx: &mut TestAppContext, commit: GraphCommit, source: Option<CardSource>) -> (Entity<CommitCard>, &mut VisualTestContext) {
    globals(cx);
    let (card, cx) = cx.add_window_view(|_, cx| CommitCard::new(commit, source, cx));
    cx.run_until_parked();
    (card, cx)
}

fn ready(card: &Entity<CommitCard>, cx: &mut VisualTestContext) -> Arc<CommitDetail> {
    wait_ticking(cx, "the changes", |cx| card.read_with(cx, |c, _| !matches!(c.changes, Changes::Loading)));
    match card.read_with(cx, |c, _| c.changes.clone()) {
        Changes::Ready(detail) => detail,
        Changes::Failed => panic!("the changes could not be read"),
        Changes::Loading => unreachable!(),
    }
}

/// Whether file row `i` of the card is drawn (selectors are `'static`, so the name is leaked: a few bytes in a test).
fn file_drawn(cx: &mut VisualTestContext, i: usize) -> bool {
    cx.debug_bounds(Box::leak(format!("commit-card-file-{i}").into_boxed_str())).is_some()
}

/// (files, added, deleted) the way git itself counts them, from `git show --numstat`.
fn git_totals(dir: &std::path::Path, rev: &str) -> (usize, usize, usize) {
    let out = git_in(dir, &["show", "--numstat", "--format=", "-M", "--first-parent", rev]);
    out.lines().filter(|l| !l.trim().is_empty()).fold((0, 0, 0), |(f, a, d), l| {
        let mut parts = l.split('\t');
        (f + 1, a + parts.next().and_then(|n| n.parse().ok()).unwrap_or(0), d + parts.next().and_then(|n| n.parse().ok()).unwrap_or(0))
    })
}

#[gpui::test]
fn the_card_reads_the_files_and_counts_git_reports(cx: &mut TestAppContext) {
    let _en = crate::i18n::LangGuard::set("en");
    let dir = repo("card-counts");
    std::fs::write(dir.join("README.md"), "# proje\nbir\niki\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), (1..=6).map(|i| format!("// satır {i}\n")).collect::<String>()).unwrap();
    std::fs::write(dir.join("yeni dosya ğ.txt"), "a\nb\nc\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "Üç dosya\n\nGövde satırı bir\nGövde satırı iki"]);
    let (card, cx) = card(cx, commit_of(&dir, "HEAD"), Some(source(&dir)));
    let detail = ready(&card, cx);
    let (files, added, deleted) = git_totals(&dir, "HEAD");
    assert_eq!((detail.files.len(), detail.totals()), (files, (added, deleted)), "same numbers as git show --numstat");
    assert_eq!(files, 3);
    assert!(detail.files.iter().any(|f| f.path == "yeni dosya ğ.txt" && f.status == 'A'), "a name with spaces and Turkish letters");
    assert!(cx.debug_bounds("commit-card-summary").is_some(), "the summary line is drawn");
    assert!((0..3).all(|i| file_drawn(cx, i)));
    assert!(!file_drawn(cx, 3) && cx.debug_bounds("commit-card-more").is_none());
    assert_eq!(message_body(&detail.message), vec!["Gövde satırı bir", "Gövde satırı iki"], "the message under the subject");
}

#[gpui::test]
fn a_huge_commit_lists_the_first_files_and_counts_the_rest(cx: &mut TestAppContext) {
    let dir = repo("card-huge");
    for i in 0..600 {
        std::fs::write(dir.join(format!("f{i:03}.txt")), "x\n").unwrap();
    }
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "many"]);
    let (card, cx) = card(cx, commit_of(&dir, "HEAD"), Some(source(&dir)));
    let detail = ready(&card, cx);
    assert_eq!(detail.files.len(), 600);
    assert!(file_drawn(cx, CARD_FILES - 1));
    assert!(!file_drawn(cx, CARD_FILES), "only the first {CARD_FILES} are drawn");
    assert!(cx.debug_bounds("commit-card-more").is_some(), "and the rest are counted");
}

#[gpui::test]
fn a_binary_file_shows_binary_and_not_a_count_of_lines(cx: &mut TestAppContext) {
    let dir = repo("card-binary");
    std::fs::write(dir.join("blob.bin"), b"\0\x01\x02\x03").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "binary"]);
    let (card, cx) = card(cx, commit_of(&dir, "HEAD"), Some(source(&dir)));
    let detail = ready(&card, cx);
    assert_eq!((detail.files[0].added, detail.files[0].deleted), (None, None));
    assert_eq!(detail.totals(), (0, 0));
}

#[gpui::test]
fn the_first_commit_and_a_merge_each_show_a_sensible_card(cx: &mut TestAppContext) {
    let dir = repo("card-merge");
    git_in(&dir, &["checkout", "-q", "-b", "side"]);
    std::fs::write(dir.join("side.txt"), "side\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "side work"]);
    git_in(&dir, &["checkout", "-q", "-"]);
    std::fs::write(dir.join("main.txt"), "main\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "main work"]);
    git_in(&dir, &["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    let src = source(&dir);
    let (merge_card, cx) = card(cx, commit_of(&dir, "HEAD"), Some(src.clone()));
    let merge = ready(&merge_card, cx);
    assert_eq!(merge.parents.len(), 2);
    assert_eq!(merge.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["side.txt"], "a merge shows what it brought in, against its first parent");
    let root = git_in(&dir, &["rev-list", "--max-parents=0", "HEAD"]);
    let (root_card, cx) = card(cx, commit_of(&dir, root.trim()), Some(src));
    let first = ready(&root_card, cx);
    assert!(first.files.iter().all(|f| f.status == 'A') && first.files.len() == 2, "the first commit adds its files");
}

#[gpui::test]
fn a_commit_is_read_once_and_a_failure_is_a_failure(cx: &mut TestAppContext) {
    let dir = repo("card-cache");
    let src = source(&dir);
    let commit = commit_of(&dir, "HEAD");
    let (first, cx) = card(cx, commit.clone(), Some(src.clone()));
    ready(&first, cx);
    assert_eq!(src.cache.borrow().len(), 1);
    let again = cx.update(|_, cx| cx.new(|cx| CommitCard::new(commit.clone(), Some(src.clone()), cx)));
    assert!(matches!(again.read_with(cx, |c, _| c.changes.clone()), Changes::Ready(_)), "ready the moment it is built: nothing is read again");
    let lost = CardSource::new(Arc::new(host::local()), "/no/such/repository".into(), CardCache::default());
    let failed = cx.update(|_, cx| cx.new(|cx| CommitCard::new(commit, Some(lost), cx)));
    wait_ticking(cx, "the failure", |cx| failed.read_with(cx, |c, _| !matches!(c.changes, Changes::Loading)));
    assert!(matches!(failed.read_with(cx, |c, _| c.changes.clone()), Changes::Failed), "an unreadable repository is not an empty commit");
}

#[test]
fn a_long_message_body_is_cut_to_what_a_card_holds() {
    let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let lines = message_body(&format!("Subject\n\n{body}"));
    assert_eq!(lines.len(), CARD_MESSAGE_LINES + 1);
    assert_eq!(lines.last().unwrap(), "…");
    assert!(message_body("Subject only").is_empty());
    let wide = "x".repeat(CARD_LINE_CHARS * 3);
    assert_eq!(message_body(&format!("S\n\n{wide}"))[0].chars().count(), CARD_LINE_CHARS);
}

/// The card of a commit in a repository opened over SSH (the throwaway `sshd`): the changes are read through the agent.
#[gpui::test]
fn the_card_reads_a_commit_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("card-ssh") else { return };
    let _en = crate::i18n::LangGuard::set("en");
    let dir = repo("card-ssh");
    std::fs::write(dir.join("one.txt"), "a\nb\n").unwrap();
    std::fs::write(dir.join("two.txt"), "c\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "two files"]);
    let commit = commit_of(&dir, "HEAD");
    globals(cx);
    let (wb, wcx) = crate::qa_workbench_tests::setup(cx);
    wb.update_in(wcx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    wait_ticking(wcx, "connected", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(300));
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    let host = wb.read_with(wcx, |wb, _| wb.remote_host_for_test());
    assert_eq!(wb.read_with(wcx, |wb, _| wb.remote_host_label_for_test()), Some(format!("ssh://{}", crate::test_sshd::ALIAS)));
    let source = CardSource::new(host, ui::local_path(&dir), CardCache::default());
    let card = wcx.update(|_, cx| cx.new(|cx| CommitCard::new(commit, Some(source), cx)));
    let detail = ready(&card, wcx);
    let (files, added, deleted) = git_totals(&dir, "HEAD");
    assert_eq!((detail.files.len(), detail.totals()), (files, (added, deleted)));
    assert_eq!(files, 2);
}

#[test]
fn commits_the_upstream_has_and_this_branch_does_not_are_incoming_until_they_are_pulled() {
    use crate::qa_integration_tests::repo_with_remote;
    let (dir, _bare, other) = repo_with_remote("graph-incoming");
    let root = ui::local_path(&dir);
    let host = host::local();
    assert!(graph::incoming(&host, &root, 1000).is_empty(), "nothing to pull in a repository that is level with its upstream");

    std::fs::write(other.join("theirs.txt"), "x\n").unwrap();
    git_in(&other, &["add", "-A"]);
    git_in(&other, &["commit", "-q", "-m", "theirs"]);
    git_in(&other, &["push", "-q"]);
    // Not fetched yet: this clone cannot know.
    assert!(graph::incoming(&host, &root, 1000).is_empty());

    git_in(&dir, &["fetch", "-q"]);
    let theirs = git_in(&dir, &["rev-parse", "origin/main"]).trim().to_string();
    assert_eq!(graph::incoming(&host, &root, 1000), std::collections::HashSet::from([theirs.clone()]));
    assert!(graph::incoming(&host, &root, 0).is_empty(), "the limit is the graph's: no rows, nothing to mark");
    // The commit is in the graph too, so the row can be drawn.
    assert!(graph::log(&host, &root, 50, Scope::All).unwrap().iter().any(|c| c.hash == theirs));

    git_in(&dir, &["pull", "-q"]);
    assert!(graph::incoming(&host, &root, 1000).is_empty(), "pulled: drawn as ours again");

    // Our own commit is ahead, not incoming.
    std::fs::write(dir.join("ours.txt"), "y\n").unwrap();
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-q", "-m", "ours"]);
    assert!(graph::incoming(&host, &root, 1000).is_empty());

    // No upstream, no HEAD: empty, never an error.
    git_in(&dir, &["branch", "--unset-upstream"]);
    assert!(graph::incoming(&host, &root, 1000).is_empty());
}

/// The panel marks what a pull would bring in (the dot, the hover card and the row's description for a screen reader all read
/// this), takes the mark back once it is pulled, and never marks a commit the branch already has.
#[gpui::test]
fn the_graph_marks_the_commits_a_pull_would_bring_in(cx: &mut TestAppContext) {
    use crate::qa_integration_tests::{repo_with_remote, wait_ticking};
    let (dir, _bare, other) = repo_with_remote("graph-incoming-ui");
    std::fs::write(other.join("theirs.txt"), "x\n").unwrap();
    git_in(&other, &["add", "-A"]);
    git_in(&other, &["commit", "-q", "-m", "theirs"]);
    git_in(&other, &["push", "-q"]);
    git_in(&dir, &["fetch", "-q"]);
    let (theirs, ours) = (git_in(&dir, &["rev-parse", "origin/main"]).trim().to_string(), git_in(&dir, &["rev-parse", "HEAD"]).trim().to_string());
    let (wb, cx) = crate::qa_workbench_tests::setup(cx);
    crate::qa_workbench_tests::open(&wb, &dir, cx);
    let scm = wb.read_with(cx, |wb, _| wb.scm_panel());
    wait_ticking(cx, "the graph", |cx| scm.read_with(cx, |s, _| s.commit_by_hash(&theirs).is_some()));
    assert!(scm.read_with(cx, |s, _| s.commit_by_hash(&theirs).unwrap().incoming), "the fetched commit is incoming");
    assert!(!scm.read_with(cx, |s, _| s.commit_by_hash(&ours).unwrap().incoming), "a commit the branch has is not");
    // A screen reader hears it too: colour alone says nothing to it.
    let rows = cx.update(|window, app| scm.read(app).a11y_history(window, app).graph.rows);
    let detail_of = |subject: &str| {
        rows.iter().find_map(|r| match r {
            crate::scm::AccessGraphRow::Commit { subject: s, detail, .. } if s == subject => Some(detail.clone()),
            _ => None,
        })
    };
    assert!(detail_of("theirs").is_some_and(|d| d.contains("not pulled yet")), "{:?}", detail_of("theirs"));
    assert!(detail_of("ilk").is_some_and(|d| !d.contains("not pulled yet")), "{:?}", detail_of("ilk"));

    // Pulled: the next reload of the graph takes the mark back.
    git_in(&dir, &["pull", "-q"]);
    wb.update(cx, |wb, cx| wb.refresh_git(0, cx));
    wait_ticking(cx, "the mark to go", |cx| scm.read_with(cx, |s, _| s.commit_by_hash(&theirs).is_some_and(|c| !c.incoming)));
}
