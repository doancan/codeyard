//! Agent mode: the list and its status, switching without stopping anything, the plus, closing a project, typing into
//! another project's shell, the size of a shared terminal, the diff under Diff, limits and the bar, marking by hand and the Dock.

use super::*;
use gpui::{TestAppContext, VisualTestContext};

fn open_window(cx: &mut TestAppContext) -> gpui::WindowHandle<Workbench> {
    cx.update(|app| {
        app.open_window(gpui::WindowOptions::default(), |window, cx| cx.new(|cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx)))
            .expect("a window")
    })
}

#[test]
fn a_limit_message_outranks_an_attention_request_and_a_running_launcher() {
    let idle = Activity::Idle;
    assert_eq!(status_of(false, true, Some(Finished::Attention), &idle, true), Status::Limit);
    assert_eq!(status_of(false, false, Some(Finished::Attention), &idle, true), Status::Attention);
    assert_eq!(status_of(false, false, None, &idle, true), Status::Running);
    assert_eq!(status_of(false, false, None, &Activity::Running("claude".into()), false), Status::Running);
    assert_eq!(status_of(false, false, None, &idle, false), Status::Idle);
}

#[test]
fn an_ended_launcher_session_is_done_and_an_ended_shell_is_idle() {
    let idle = Activity::Idle;
    assert_eq!(status_of(true, false, None, &idle, true), Status::Done);
    assert_eq!(status_of(true, true, Some(Finished::Attention), &idle, false), Status::Idle);
}

#[test]
fn the_last_line_is_the_last_row_with_text() {
    let rows: Vec<String> = ["a", "  running tests  ", "", ""].into_iter().map(String::from).collect();
    assert_eq!(last_line(&rows), "running tests");
    assert_eq!(last_line(&[]), "");
}

#[test]
fn urgent_states_sort_before_the_rest() {
    let mut all = [Status::Idle, Status::Done, Status::Running, Status::Limit, Status::Attention];
    all.sort();
    assert_eq!(all, [Status::Attention, Status::Limit, Status::Running, Status::Done, Status::Idle]);
    assert!(Status::Attention.needs_you() && Status::Limit.needs_you() && !Status::Running.needs_you());
}

#[gpui::test]
fn a_window_shows_the_terminal_of_another_window(cx: &mut TestAppContext) {
    crate::qa_workbench_tests::globals(cx);
    let (first, second) = (open_window(cx), open_window(cx));
    second.update(cx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx)).unwrap();
    cx.run_until_parked();
    let theirs = second.read_with(cx, |wb, _| wb.terminals_for_test()).unwrap();
    assert_eq!(theirs.len(), 1, "the other window has its terminal");

    let second_id = second.window_id();
    first
        .update(cx, |wb, window, cx| {
            wb.toggle_agent_mode(&ToggleAgentMode, window, cx);
            let peers = wb.agent_mode_peers(window, cx);
            assert_eq!(peers.len(), 2, "both windows are listed");
            let other = peers.iter().find(|p| p.id == second_id).expect("the other window is read");
            assert_eq!(other.terminals.len(), 1);
            assert_eq!(other.terminals[0].entity_id(), theirs[0].entity_id(), "the very same terminal, not a copy");
            wb.chrome.agent_mode.as_mut().expect("agent mode is on").selected = Some(second_id);
            cx.notify();
        })
        .unwrap();

    let mut visual = VisualTestContext::from_window(first.into(), cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("agent-mode-terminal").expect("the other window's terminal is drawn in this window");
    assert!(bounds.size.width > px(100.) && bounds.size.height > px(50.), "and it has room: {bounds:?}");
}

#[test]
fn the_scope_switch_lists_local_and_ssh_projects_apart() {
    assert!(Scope::All.includes(true) && Scope::All.includes(false));
    assert!(Scope::Local.includes(false) && !Scope::Local.includes(true));
    assert!(Scope::Ssh.includes(true) && !Scope::Ssh.includes(false));
}

/// A second window with a terminal, and a first one in agent mode that lists both.
fn two_projects(cx: &mut TestAppContext) -> (gpui::WindowHandle<Workbench>, gpui::WindowHandle<Workbench>) {
    crate::qa_workbench_tests::globals(cx);
    let (first, second) = (open_window(cx), open_window(cx));
    second.update(cx, |wb, window, cx| wb.new_terminal(&NewTerminal, window, cx)).unwrap();
    cx.run_until_parked();
    first.update(cx, |wb, window, cx| wb.toggle_agent_mode(&ToggleAgentMode, window, cx)).unwrap();
    (first, second)
}

fn status_of_second(first: gpui::WindowHandle<Workbench>, second: gpui::WindowHandle<Workbench>, cx: &mut TestAppContext) -> Status {
    first
        .update(cx, |wb, window, cx| wb.agent_mode_peers(window, cx).into_iter().find(|p| p.id == second.window_id()).expect("the project is listed").status)
        .unwrap()
}

#[gpui::test]
fn a_terminal_that_asks_for_the_person_puts_its_project_under_needs_you_until_it_is_looked_at(cx: &mut TestAppContext) {
    let (first, second) = two_projects(cx);
    assert_eq!(status_of_second(first, second, cx), Status::Idle, "a shell at its prompt");
    let term = second.read_with(cx, |wb, _| wb.terminals_for_test()).unwrap().remove(0);
    term.update(cx, |t, cx| t.feed_and_report_for_test(b"\x1b]9;Claude needs your permission\x07", cx));
    assert_eq!(status_of_second(first, second, cx), Status::Attention);
    // Selecting the project is looking at it.
    first
        .update(cx, |wb, window, cx| {
            let peers = wb.agent_mode_peers(window, cx);
            let peer = peers.iter().find(|p| p.id == second.window_id()).expect("listed");
            wb.agent_mode_select(peer, 0, window, cx);
        })
        .unwrap();
    assert_eq!(status_of_second(first, second, cx), Status::Idle, "the request is answered once the terminal is shown");
}

#[gpui::test]
fn leaving_a_project_and_coming_back_finds_the_same_live_terminal(cx: &mut TestAppContext) {
    let (first, second) = two_projects(cx);
    let theirs = second.read_with(cx, |wb, _| wb.terminals_for_test()).unwrap().remove(0);
    for target in [first.window_id(), second.window_id(), first.window_id(), second.window_id()] {
        first
            .update(cx, |wb, window, cx| {
                let peers = wb.agent_mode_peers(window, cx);
                let peer = peers.iter().find(|p| p.id == target).expect("listed");
                wb.agent_mode_select(peer, 0, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
    }
    let after = second.read_with(cx, |wb, _| wb.terminals_for_test()).unwrap();
    assert_eq!(after.len(), 1, "no terminal was opened or closed");
    assert_eq!(after[0].entity_id(), theirs.entity_id());
    assert!(!theirs.read_with(cx, |t, _| t.has_exited()), "and it is still running");
}

#[gpui::test]
fn the_plus_opens_a_terminal_in_the_project_shown_and_selects_it(cx: &mut TestAppContext) {
    let (first, second) = two_projects(cx);
    first.update(cx, |wb, window, cx| wb.agent_mode_new_terminal(second.window_id(), window, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(second.read_with(cx, |wb, _| wb.terminals_for_test().len()).unwrap(), 2, "the terminal belongs to that project");
    assert_eq!(first.read_with(cx, |wb, _| wb.terminals_for_test().len()).unwrap(), 0, "not to the window that shows it");
    let (selected, tab) = first.read_with(cx, |wb, _| wb.chrome.agent_mode.as_ref().map(|m| (m.selected, m.tab)).expect("on")).unwrap();
    assert_eq!((selected, tab), (Some(second.window_id()), 1));
}

#[gpui::test]
fn a_project_window_that_closes_while_shown_leaves_the_mode_on_the_next_project(cx: &mut TestAppContext) {
    let (first, second) = two_projects(cx);
    first.update(cx, |wb, _, _| wb.chrome.agent_mode.as_mut().expect("on").selected = Some(second.window_id())).unwrap();
    second.update(cx, |_, window, _| window.remove_window()).unwrap();
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(first.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("agent-mode").is_some(), "the mode still draws");
    let listed = first.update(&mut visual, |wb, window, cx| wb.agent_mode_peers(window, cx).len()).unwrap();
    assert_eq!(listed, 1, "only the window that is left");
}

#[gpui::test]
fn typing_in_the_hub_reaches_the_other_projects_shell_and_its_output_comes_back(cx: &mut TestAppContext) {
    let (first, second) = two_projects(cx);
    let theirs = second.read_with(cx, |wb, _| wb.terminals_for_test()).unwrap().remove(0);
    first
        .update(cx, |wb, window, cx| {
            let peers = wb.agent_mode_peers(window, cx);
            let peer = peers.iter().find(|p| p.id == second.window_id()).expect("listed");
            wb.agent_mode_select(peer, 0, window, cx);
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(first.into(), cx);
    visual.run_until_parked();
    // The shell may still be starting: keystrokes wait in the pty until it reads them.
    visual.simulate_keystrokes("e c h o space h u b o k enter");
    let start = std::time::Instant::now();
    loop {
        visual.run_until_parked();
        let screen = theirs.read_with(&visual, |t, _| t.a11y_view().0);
        // The typed command and, on a line of its own, what it printed.
        if screen.iter().any(|row| row.trim() == "hubok") {
            break;
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(15), "no output reached the terminal; screen: {screen:?}");
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[gpui::test]
fn the_terminal_takes_its_size_from_the_hub_that_draws_it(cx: &mut TestAppContext) {
    let (first, second) = two_projects(cx);
    let theirs = second.read_with(cx, |wb, _| wb.terminals_for_test()).unwrap().remove(0);
    let before = theirs.read_with(cx, |t, _| t.a11y_view().0.len());
    first
        .update(cx, |wb, _, cx| {
            wb.chrome.agent_mode.as_mut().expect("on").selected = Some(second.window_id());
            cx.notify();
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(first.into(), cx);
    visual.run_until_parked();
    let bounds = visual.debug_bounds("agent-mode-terminal").expect("drawn in the hub");
    let rows = theirs.read_with(&visual, |t, _| t.a11y_view().0.len());
    assert_ne!(rows, before, "the grid was resized to the hub's area ({bounds:?}), not left at the size of the window that made it");
    visual.simulate_resize(gpui::size(px(1400.), px(500.)));
    visual.run_until_parked();
    let smaller = theirs.read_with(&visual, |t, _| t.a11y_view().0.len());
    assert!(smaller < rows, "and it follows the hub when the window changes: {rows} then {smaller} rows");
}

#[gpui::test]
fn a_terminal_drawn_in_two_windows_keeps_the_size_of_the_window_that_leads(cx: &mut TestAppContext) {
    let (first, second) = two_projects(cx);
    let theirs = second.read_with(cx, |wb, _| wb.terminals_for_test()).unwrap().remove(0);
    first
        .update(cx, |wb, _, cx| {
            wb.chrome.agent_mode.as_mut().expect("on").selected = Some(second.window_id());
            cx.notify();
        })
        .unwrap();
    let mut hub = VisualTestContext::from_window(first.into(), cx);
    hub.simulate_resize(gpui::size(px(1500.), px(900.)));
    hub.run_until_parked();
    let led = theirs.read_with(&hub, |t, _| t.a11y_view().0.len());
    // The project's own window draws the same terminal in its panel, at another size, again and again.
    let own = VisualTestContext::from_window(second.into(), cx);
    own.simulate_resize(gpui::size(px(900.), px(420.)));
    for _ in 0..5 {
        own.run_until_parked();
        hub.run_until_parked();
    }
    assert_eq!(theirs.read_with(&own, |t, _| t.a11y_view().0.len()), led, "the leading window's size stands");
    // Leaving agent mode gives the size back to the window that owns the terminal.
    first.update(&mut hub, |wb, window, cx| wb.toggle_agent_mode(&ToggleAgentMode, window, cx)).unwrap();
    own.run_until_parked();
    assert_ne!(theirs.read_with(&own, |t, _| t.a11y_view().0.len()), led, "and it follows its own window again");
}

fn wait_for(cx: &mut TestAppContext, what: &str, mut done: impl FnMut(&mut TestAppContext) -> bool) {
    let start = std::time::Instant::now();
    while !done(cx) {
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "timeout: {what}");
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[gpui::test]
fn a_diff_asked_for_by_the_hosted_project_opens_in_the_hub_and_not_in_its_own_window(cx: &mut TestAppContext) {
    let dir = crate::qa_integration_tests::repo("agent-mode-diff");
    std::fs::write(dir.join("src/lib.rs"), "// changed\n").unwrap();
    crate::qa_workbench_tests::globals(cx);
    let (first, second) = (open_window(cx), open_window(cx));
    let path = dir.to_string_lossy().into_owned();
    second.update(cx, |wb, window, cx| wb.open_path(&path, window, cx)).unwrap();
    wait_for(cx, "the status of the project", |cx| second.read_with(cx, |wb, _| wb.vcs.status.as_ref().is_some_and(|s| !s.changes.is_empty())).unwrap());

    first
        .update(cx, |wb, window, cx| {
            wb.toggle_agent_mode(&ToggleAgentMode, window, cx);
            wb.chrome.agent_mode.as_mut().expect("on").selected = Some(second.window_id());
            cx.notify();
        })
        .unwrap();
    let mut hub = VisualTestContext::from_window(first.into(), cx);
    hub.run_until_parked();
    let hub_handle: gpui::AnyWindowHandle = first.into();
    assert!(second.read_with(&hub, |wb, _| wb.chrome.hub_host).unwrap() == Some(hub_handle), "the project knows where its views are shown");

    // Input in the hub's panel makes the hub the window its menus and pickers open in.
    second.update(&mut hub, |wb, _, cx| wb.use_hub_window(hub_handle, cx)).unwrap();
    assert!(second.read_with(&hub, |wb, _| wb.view_host_window()).unwrap() == Some(hub_handle), "menus and pickers open in the hub");

    second.update(&mut hub, |wb, window, cx| wb.open_diff("src/lib.rs".into(), false, window, cx)).unwrap();
    wait_for(cx, "the diff in the hub", |cx| first.read_with(cx, |wb, _| wb.chrome.agent_mode.as_ref().is_some_and(|m| m.diff.is_some())).unwrap());
    let (owner, tab) = first.read_with(&hub, |wb, _| wb.chrome.agent_mode.as_ref().map(|m| (m.diff.as_ref().map(|d| d.0), m.right)).expect("on")).unwrap();
    assert_eq!((owner, tab), (Some(second.window_id()), RightTab::Diff));
    assert!(second.read_with(&hub, |wb, _| wb.diff_view().is_none()).unwrap(), "nothing opened in the project's own window");

    // Leaving agent mode lets the project's menus open in its own window again.
    first.update(&mut hub, |wb, window, cx| wb.toggle_agent_mode(&ToggleAgentMode, window, cx)).unwrap();
    assert!(second.read_with(&hub, |wb, _| wb.chrome.hub_host.is_none() && wb.chrome.view_host.is_none()).unwrap(), "the project opens its own menus again");
}

#[test]
fn token_counts_are_short() {
    assert_eq!(short_tokens(0), "0");
    assert_eq!(short_tokens(950), "950");
    assert_eq!(short_tokens(12_400), "12k");
    assert_eq!(short_tokens(1_340_000), "1.3M");
}

/// A second project whose agent launcher terminal shows Claude Code's limit message, listed by a first window in agent mode.
fn limited_agent(cx: &mut TestAppContext) -> (gpui::WindowHandle<Workbench>, gpui::WindowHandle<Workbench>) {
    crate::qa_workbench_tests::globals(cx);
    let (first, second) = (open_window(cx), open_window(cx));
    let data = crate::qa_workbench_tests::fresh("agent-mode-accounts-data");
    let folder = crate::qa_workbench_tests::fresh("agent-mode-accounts-project");
    second
        .update(cx, |wb, window, cx| {
            wb.prepare_agent_project_for_test(data, folder, cx);
            wb.new_terminal(&NewTerminal, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let term = second.read_with(cx, |wb, _| wb.terminals_for_test()).unwrap().remove(0);
    // The terminal that only the panel's list held is the launcher's now.
    second.update(cx, |wb, _, _| wb.show_agent_terminal_for_test(term.clone())).unwrap();
    term.update(cx, |t, cx| t.feed_and_report_for_test(b"Claude usage limit reached. Your limit will reset at 3pm.\r\n", cx));
    first
        .update(cx, |wb, window, cx| {
            wb.toggle_agent_mode(&ToggleAgentMode, window, cx);
            wb.chrome.agent_mode.as_mut().expect("on").selected = Some(second.window_id());
            cx.notify();
        })
        .unwrap();
    (first, second)
}

#[gpui::test]
fn a_limit_message_marks_the_account_and_the_bar_offers_the_other_one(cx: &mut TestAppContext) {
    let (first, second) = limited_agent(cx);
    let mut hub = VisualTestContext::from_window(first.into(), cx);
    hub.run_until_parked();
    let peer = first
        .update(&mut hub, |wb, window, cx| {
            let peer = wb.agent_mode_peers(window, cx).into_iter().find(|p| p.id == second.window_id()).expect("listed");
            (peer.status, peer.limit.and_then(|l| l.resets), peer.agent.map(|a| (a.provider, a.profile, a.profile_count)))
        })
        .unwrap();
    assert_eq!(peer.0, Status::Limit);
    assert_eq!(peer.1.as_deref(), Some("3pm"));
    let (provider, profile, count) = peer.2.expect("an agent session");
    assert_eq!((provider, count), (Provider::Claude, 3), "the project has the accounts the Agents panel made");
    // The view noted the limit for that account where every window can see it...
    let marked = hub.update(|_, cx| cx.try_global::<Limits>().and_then(|l| l.limited("claude", profile, now_secs()).cloned()));
    assert_eq!(marked.and_then(|m| m.resets).as_deref(), Some("3pm"));
    // ...and draws the bar that offers the other account, the account button, and both accounts in the sidebar.
    assert!(hub.debug_bounds("agent-mode-limit").is_some(), "the limit bar is drawn");
    assert!(hub.debug_bounds("agent-mode-account").is_some(), "the account button is drawn");
    for selector in ["agent-mode-account-claude-0", "agent-mode-account-claude-1", "agent-mode-account-claude-2"] {
        assert!(hub.debug_bounds(selector).is_some(), "{selector} is listed");
    }
    assert_eq!(count, 3);
    assert!(hub.debug_bounds("agent-mode-continue").is_some(), "the other account is offered");
}

#[gpui::test]
fn marking_the_account_available_ends_the_limit_though_the_message_is_still_on_screen(cx: &mut TestAppContext) {
    let (first, second) = limited_agent(cx);
    let mut hub = VisualTestContext::from_window(first.into(), cx);
    hub.run_until_parked();
    assert!(hub.debug_bounds("agent-mode-limit").is_some());
    first.update(&mut hub, |wb, _, cx| wb.mark_agent_account(false, cx)).unwrap();
    hub.run_until_parked();
    let status = first
        .update(&mut hub, |wb, window, cx| wb.agent_mode_peers(window, cx).into_iter().find(|p| p.id == second.window_id()).expect("listed").status)
        .unwrap();
    // The bar is built from the same reading (`limit`), so it is gone with the status. (The test platform keeps the last
    // frame's `debug_bounds` of a selector once drawn, so the bar is not asked for by its bounds here.)
    assert_ne!(status, Status::Limit, "the old message is ignored");
    // By hand it can be marked again.
    first.update(&mut hub, |wb, _, cx| wb.mark_agent_account(true, cx)).unwrap();
    assert!(hub.update(|_, cx| cx.try_global::<Limits>().is_some_and(|l| (0..3).any(|p| l.limited("claude", p, now_secs()).is_some()))));
}

#[gpui::test]
fn with_no_other_account_free_the_bar_says_so(cx: &mut TestAppContext) {
    let (first, _second) = limited_agent(cx);
    let mut hub = VisualTestContext::from_window(first.into(), cx);
    hub.update(|_, cx| (0..3).for_each(|profile| cx.default_global::<Limits>().mark_by_hand("claude", profile, now_secs())));
    hub.run_until_parked();
    assert!(hub.debug_bounds("agent-mode-limit").is_some());
    assert!(hub.debug_bounds("agent-mode-continue").is_none(), "every other account is limited too");
}

#[gpui::test]
fn the_dock_counts_the_projects_that_need_the_person(cx: &mut TestAppContext) {
    let (first, second) = two_projects(cx);
    let mut hub = VisualTestContext::from_window(first.into(), cx);
    hub.run_until_parked();
    let badge = |hub: &mut VisualTestContext| first.read_with(hub, |wb, _| wb.chrome.agent_mode.as_ref().map(|m| m.badge)).unwrap();
    assert_eq!(badge(&mut hub), Some(0), "nothing asks for the person");
    let term = second.read_with(&hub, |wb, _| wb.terminals_for_test()).unwrap().remove(0);
    term.update(&mut hub, |t, cx| t.feed_and_report_for_test(b"\x1b]9;Codex needs your approval\x07", cx));
    hub.simulate_resize(gpui::size(px(1301.), px(801.)));
    hub.run_until_parked();
    assert_eq!(badge(&mut hub), Some(1), "one project waits");
    // Looking at it answers the request: the count goes back.
    first
        .update(&mut hub, |wb, window, cx| {
            let peers = wb.agent_mode_peers(window, cx);
            let peer = peers.iter().find(|p| p.id == second.window_id()).expect("listed");
            wb.agent_mode_select(peer, 0, window, cx);
        })
        .unwrap();
    hub.simulate_resize(gpui::size(px(1300.), px(800.)));
    hub.run_until_parked();
    assert_eq!(badge(&mut hub), Some(0));
}

#[gpui::test]
fn a_project_that_starts_waiting_out_of_sight_gets_a_notice_that_opens_it(cx: &mut TestAppContext) {
    let (first, second) = two_projects(cx);
    let mut hub = VisualTestContext::from_window(first.into(), cx);
    hub.run_until_parked();
    let notices = |hub: &mut VisualTestContext| first.read_with(hub, |wb, _| wb.history_for_test()).unwrap();
    assert!(notices(&mut hub).is_empty(), "what the projects were at the first look is not news");
    let term = second.read_with(&hub, |wb, _| wb.terminals_for_test()).unwrap().remove(0);
    term.update(&mut hub, |t, cx| t.feed_and_report_for_test(b"\x1b]9;Codex needs your approval\x07", cx));
    hub.simulate_resize(gpui::size(px(1301.), px(801.)));
    hub.run_until_parked();
    let news = notices(&mut hub);
    assert_eq!(news.len(), 1, "{news:?}");
    assert!(news[0].message.ends_with(" needs you"), "{}", news[0].message);
    let Some(NoticeAction::AgentProject(id)) = news[0].action.clone() else { panic!("the notice has a button: {:?}", news[0].action) };
    assert_eq!(id, second.window_id().as_u64());
    // The button shows that project's terminal.
    first.update(&mut hub, |wb, window, cx| wb.run_notice_action(NoticeAction::AgentProject(id), window, cx)).unwrap();
    let shown = first.read_with(&hub, |wb, _| wb.chrome.agent_mode.as_ref().and_then(|m| m.selected)).unwrap();
    assert_eq!(shown, Some(second.window_id()));
    // And it is said once, not every second.
    hub.simulate_resize(gpui::size(px(1300.), px(800.)));
    hub.run_until_parked();
    assert_eq!(notices(&mut hub).len(), 1);
}

fn account(tokens: Option<u64>, percent: Option<u32>) -> AccountRow {
    AccountRow { provider: Provider::Claude, profile: 0, limited: None, tokens, share: 0., percent }
}

#[test]
fn an_accounts_use_reads_as_tokens_and_with_a_budget_as_their_share() {
    assert_eq!(usage_text(&account(None, None)), "Available");
    assert_eq!(usage_text(&account(Some(412_000), None)), "412k tokens");
    assert_eq!(usage_text(&account(Some(412_000), Some(21))), "412k tokens · 21%");
}

#[test]
fn the_token_budget_is_a_setting_that_is_off_by_default() {
    assert_eq!(crate::settings::parse("{}").0.agents_claude_token_budget, 0);
    let (s, warnings) = crate::settings::parse(r#"{ "agents.claudeTokenBudget": 2000000 }"#);
    assert_eq!((s.agents_claude_token_budget, warnings.is_empty()), (2_000_000, true), "{warnings:?}");
    let (s, warnings) = crate::settings::parse(r#"{ "agents.claudeTokenBudget": -5 }"#);
    assert_eq!(s.agents_claude_token_budget, 0, "a negative budget is not taken");
    assert!(warnings.iter().any(|w| w.contains("agents.claudeTokenBudget")), "{warnings:?}");
}
