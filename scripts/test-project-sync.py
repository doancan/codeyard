"""Rules of scripts/project-sync.py: which status an issue gets for every event."""
import importlib.util
import pathlib
import unittest

spec = importlib.util.spec_from_file_location("project_sync", pathlib.Path(__file__).with_name("project-sync.py"))
sync = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sync)
next_status, epic_status, signal_for = sync.next_status, sync.epic_status, sync.signal_for

STATUSES = (None, "Backlog", "In Progress", "Blocked", "In Review", "Test", "Done")
SIGNALS = (None, "draft", "ready", "approved", "changes", "ci_passed", "ci_failed", "pr_closed")


class IssueRules(unittest.TestCase):
    def test_closed_is_done_and_blocked_is_blocked_whatever_else_happens(self):
        for current in STATUSES:
            for signal in SIGNALS:
                for assigned in (False, True):
                    self.assertEqual(next_status(current, closed=True, blocked=True, assigned=assigned, signal=signal), "Done")
                    self.assertEqual(next_status(current, closed=False, blocked=True, assigned=assigned, signal=signal), "Blocked")

    def test_new_reopened_and_unblocked_issues_start_over(self):
        for current in (None, "Done", "Blocked"):
            self.assertEqual(next_status(current, closed=False, blocked=False, assigned=False), "Backlog", current)
            self.assertEqual(next_status(current, closed=False, blocked=False, assigned=True), "In Progress", current)

    def test_assignment_starts_and_unassignment_returns_to_backlog(self):
        self.assertEqual(next_status("Backlog", closed=False, blocked=False, assigned=True), "In Progress")
        self.assertEqual(next_status("In Progress", closed=False, blocked=False, assigned=False), "Backlog")

    def test_statuses_without_a_reason_to_move_are_kept(self):
        # Includes statuses set by hand, such as Test with no reviewer.
        for current in ("Backlog", "In Review", "Test"):
            self.assertEqual(next_status(current, closed=False, blocked=False, assigned=False), current)
        for current in ("In Progress", "In Review", "Test"):
            self.assertEqual(next_status(current, closed=False, blocked=False, assigned=True), current)

    def test_pull_request_signals(self):
        cases = [
            ("Backlog", "draft", "In Progress"),
            ("In Review", "draft", "In Progress"),
            ("In Progress", "ready", "In Review"),
            ("Backlog", "ready", "In Review"),
            ("In Review", "approved", "Test"),
            ("In Review", "changes", "In Progress"),
            ("Test", "changes", "In Progress"),
            ("In Review", "ci_passed", "Test"),
            ("In Progress", "ci_passed", "In Progress"),  # a draft's green run is not a review
            ("Test", "ci_passed", "Test"),
            ("Test", "ci_failed", "In Review"),  # verified work is back to being reviewed, not restarted
            ("In Review", "ci_failed", "In Review"),
            ("In Progress", "ci_failed", "In Progress"),
            ("Backlog", "ci_failed", "Backlog"),
            ("In Review", "pr_closed", "In Progress"),
            ("Backlog", "pr_closed", "Backlog"),
        ]
        for current, signal, expected in cases:
            self.assertEqual(next_status(current, closed=False, blocked=False, assigned=True, signal=signal), expected, (current, signal))
        self.assertEqual(next_status("Test", closed=False, blocked=False, assigned=False, signal="pr_closed"), "Backlog")

    def test_a_green_run_after_a_red_one_reaches_test(self):
        # #26: a red run must not leave a ready pull request's issue somewhere the next green run cannot move it from.
        for start in ("In Review", "Test"):
            status = start
            for signal in ("ci_failed", "ci_passed"):
                status = next_status(status, closed=False, blocked=False, assigned=True, signal=signal)
            self.assertEqual(status, "Test", start)

    def test_every_result_is_a_board_status(self):
        for current in STATUSES:
            for signal in SIGNALS:
                for closed in (False, True):
                    for blocked in (False, True):
                        for assigned in (False, True):
                            got = next_status(current, closed=closed, blocked=blocked, assigned=assigned, signal=signal)
                            self.assertIn(got, STATUSES[1:], (current, signal, closed, blocked, assigned))


class EpicRules(unittest.TestCase):
    def test_epic_follows_its_children(self):
        self.assertEqual(epic_status(closed=True, children=[(False, "Backlog")]), "Done")
        self.assertEqual(epic_status(closed=False, children=[]), "Backlog")
        self.assertEqual(epic_status(closed=False, children=[(False, "Backlog"), (False, "Blocked")]), "Backlog")
        self.assertEqual(epic_status(closed=False, children=[(False, "Backlog"), (False, "In Review")]), "In Progress")
        self.assertEqual(epic_status(closed=False, children=[(True, "Done"), (False, "Blocked")]), "In Progress")
        self.assertEqual(epic_status(closed=False, children=[(True, "Done"), (True, None)]), "Test")


class Events(unittest.TestCase):
    def pr(self, action, draft=False, merged=False):
        return {"action": action, "pull_request": {"number": 7, "draft": draft, "merged": merged}}

    def test_pull_request_target(self):
        self.assertEqual(signal_for("pull_request_target", self.pr("opened", draft=True)), ("draft", 7))
        self.assertEqual(signal_for("pull_request_target", self.pr("opened")), ("ready", 7))
        self.assertEqual(signal_for("pull_request_target", self.pr("ready_for_review")), ("ready", 7))
        self.assertEqual(signal_for("pull_request_target", self.pr("converted_to_draft", draft=True)), ("draft", 7))
        self.assertEqual(signal_for("pull_request_target", self.pr("edited")), ("ready", 7))
        self.assertEqual(signal_for("pull_request_target", self.pr("closed")), ("pr_closed", 7))
        self.assertEqual(signal_for("pull_request_target", self.pr("closed", merged=True)), (None, 7))
        self.assertIsNone(signal_for("pull_request_target", self.pr("labeled")))

    def test_reviews(self):
        review = lambda state: {"action": "submitted", "review": {"state": state}, "pull_request": {"number": 7}}
        self.assertEqual(signal_for("pull_request_review", review("APPROVED")), ("approved", 7))
        self.assertEqual(signal_for("pull_request_review", review("changes_requested")), ("changes", 7))
        self.assertIsNone(signal_for("pull_request_review", review("commented")))
        self.assertIsNone(signal_for("pull_request_review", {"action": "dismissed", "review": {}, "pull_request": {"number": 7}}))

    def test_ci_runs(self):
        run = lambda conclusion, prs: {"workflow_run": {"conclusion": conclusion, "pull_requests": prs}}
        self.assertEqual(signal_for("workflow_run", run("success", [{"number": 7}])), ("ci_passed", 7))
        self.assertEqual(signal_for("workflow_run", run("failure", [{"number": 7}])), ("ci_failed", 7))
        self.assertIsNone(signal_for("workflow_run", run("cancelled", [{"number": 7}])))
        self.assertIsNone(signal_for("workflow_run", run("success", [])), "a push to main has no pull request")

    def test_other_events_move_nothing(self):
        self.assertIsNone(signal_for("schedule", {}))
        self.assertIsNone(signal_for("issues", {"action": "opened"}))


class MissingToken(unittest.TestCase):
    def test_without_a_token_nothing_runs_and_the_job_does_not_fail(self):
        import io
        import os
        from contextlib import redirect_stdout
        from unittest import mock

        with mock.patch.dict(os.environ, {}, clear=True), redirect_stdout(io.StringIO()) as out:
            self.assertEqual(sync.main(["project-sync.py", "event"]), 0)
        self.assertIn("PROJECT_TOKEN is not set", out.getvalue())

    def test_a_rejected_token_says_so_and_fails_the_job(self):
        import io
        import os
        import urllib.error
        from contextlib import redirect_stdout
        from unittest import mock

        rejected = urllib.error.HTTPError("https://api.github.com/graphql", 401, "Unauthorized", {}, None)
        with mock.patch.dict(os.environ, {"PROJECT_TOKEN": "x"}, clear=True), mock.patch.object(sync.GitHub, "call", side_effect=rejected), redirect_stdout(io.StringIO()) as out:
            self.assertEqual(sync.main(["project-sync.py", "reconcile"]), 1)
        self.assertIn("rejected PROJECT_TOKEN (401)", out.getvalue())


if __name__ == "__main__":
    unittest.main()
