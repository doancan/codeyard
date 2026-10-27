"""Keep the Codeyard project board in step with issues and pull requests.

    PROJECT_TOKEN=… python3 scripts/project-sync.py event              # in Actions: the event in GITHUB_EVENT_PATH
    PROJECT_TOKEN=… python3 scripts/project-sync.py reconcile [--dry-run]

Run by .github/workflows/project.yml. The token needs the `project` and `public_repo` scopes (a user-owned project
cannot be written with the workflow's own token). The rules are `next_status` and `epic_status`; everything else
only reads GitHub and writes the result. CONTRIBUTING.md ("Issues and labels") describes them for people.
"""
import datetime
import json
import os
import sys
import urllib.request

OWNER, REPO, PROJECT = "doancan", "codeyard", 2

ACTIVE = ("In Progress", "In Review", "Test")


# --- Rules (pure) ---


def next_status(current, *, closed, blocked, assigned, signal=None):
    """The Status an issue should have. `current` is None when it is not on the board yet; `signal` is what a pull
    request that closes it just did (`draft`, `ready`, `approved`, `changes`, `ci_passed`, `ci_failed`, `pr_closed`).
    Without a reason to move, the current status stays, so a status set by hand is kept."""
    if closed:
        return "Done"
    if blocked:
        return "Blocked"
    if current in (None, "Done", "Blocked"):  # new, reopened or just unblocked: start over
        current = "In Progress" if assigned else "Backlog"
    if signal in ("draft", "changes"):
        return "In Progress"
    if signal == "ready":
        return "In Review"
    if signal == "approved":
        return "Test"
    if signal == "ci_passed":
        return "Test" if current == "In Review" else current
    if signal == "ci_failed":
        return "In Progress" if current in ("In Review", "Test") else current
    if signal == "pr_closed":
        return ("In Progress" if assigned else "Backlog") if current in ACTIVE else current
    if current == "Backlog" and assigned:
        return "In Progress"
    if current == "In Progress" and not assigned:
        return "Backlog"
    return current


def epic_status(*, closed, children):
    """An epic follows its sub-issues: `children` is a list of (closed, status). All closed: Test (its own
    verification list is left); any started: In Progress; otherwise Backlog."""
    if closed:
        return "Done"
    if children and all(c for c, _ in children):
        return "Test"
    if any(c or s in ACTIVE for c, s in children):
        return "In Progress"
    return "Backlog"


def signal_for(event_name, payload):
    """(signal, pull request number) for a pull request event; None when the event does not move anything."""
    if event_name == "pull_request_target":
        pr = payload["pull_request"]
        action = payload["action"]
        if action == "closed":
            return (None if pr.get("merged") else "pr_closed"), pr["number"]
        if action in ("opened", "reopened", "edited", "ready_for_review", "converted_to_draft"):
            return ("draft" if pr.get("draft") else "ready"), pr["number"]
        return None
    if event_name == "pull_request_review" and payload.get("action") == "submitted":
        state = (payload["review"].get("state") or "").lower()
        signal = {"approved": "approved", "changes_requested": "changes"}.get(state)
        return (signal, payload["pull_request"]["number"]) if signal else None
    if event_name == "workflow_run":
        run = payload["workflow_run"]
        signal = {"success": "ci_passed", "failure": "ci_failed"}.get(run.get("conclusion"))
        prs = run.get("pull_requests") or []
        return (signal, prs[0]["number"]) if signal and prs else None
    return None


# --- GitHub ---


class GitHub:
    def __init__(self, token, dry_run=False):
        self.token, self.dry_run = token, dry_run

    def call(self, method, url, body=None):
        req = urllib.request.Request(url, method=method, data=None if body is None else json.dumps(body).encode())
        req.add_header("Authorization", f"Bearer {self.token}")
        req.add_header("Accept", "application/vnd.github+json")
        with urllib.request.urlopen(req, timeout=30) as r:
            text = r.read().decode()
        return json.loads(text) if text else None

    def gql(self, query, **variables):
        out = self.call("POST", "https://api.github.com/graphql", {"query": query, "variables": variables})
        if out.get("errors"):
            raise RuntimeError(json.dumps(out["errors"]))
        return out["data"]

    def write(self, what, method, url=None, body=None, query=None, **variables):
        print(("would " if self.dry_run else "") + what)
        if self.dry_run:
            return None
        return self.gql(query, **variables) if query else self.call(method, url, body)


ITEM_FIELDS = """projectItems(first: 10) { nodes { id project { number }
  status: fieldValueByName(name: "Status") { ... on ProjectV2ItemFieldSingleSelectValue { name } }
  start: fieldValueByName(name: "Start date") { ... on ProjectV2ItemFieldDateValue { date } } } }"""

ISSUE = """query($owner: String!, $repo: String!, $n: Int!) { repository(owner: $owner, name: $repo) { issue(number: $n) {
  id number state assignees(first: 1) { totalCount } parent { number }
  blockedBy(first: 50) { nodes { state } } blocking(first: 50) { nodes { number } }
  subIssues(first: 100) { nodes { state %s } } %s } } }""" % (ITEM_FIELDS, ITEM_FIELDS)


def board_item(issue):
    return next((i for i in issue["projectItems"]["nodes"] if i["project"]["number"] == PROJECT), None)


class Board:
    def __init__(self, gh):
        self.gh = gh
        data = gh.gql(
            """query($owner: String!, $n: Int!) { user(login: $owner) { projectV2(number: $n) { id
            fields(first: 50) { nodes { ... on ProjectV2FieldCommon { id name }
            ... on ProjectV2SingleSelectField { options { id name } } } } } } }""",
            owner=OWNER,
            n=PROJECT,
        )["user"]["projectV2"]
        self.id = data["id"]
        fields = {f["name"]: f for f in data["fields"]["nodes"] if f}
        self.status_field = fields["Status"]["id"]
        self.status_options = {o["name"]: o["id"] for o in fields["Status"]["options"]}
        self.start_field = fields["Start date"]["id"]

    def issue(self, n):
        return self.gh.gql(ISSUE, owner=OWNER, repo=REPO, n=n)["repository"]["issue"]

    def set_status(self, issue, item, status):
        """Add the issue to the board if needed, then write Status (and today's Start date on the first start)."""
        item_id = item["id"] if item else None
        if item_id is None:
            out = self.gh.write(
                f"#{issue['number']}: add to project",
                "POST",
                query="mutation($p: ID!, $c: ID!) { addProjectV2ItemById(input: {projectId: $p, contentId: $c}) { item { id } } }",
                p=self.id,
                c=issue["id"],
            )
            item_id = out["addProjectV2ItemById"]["item"]["id"] if out else None
        update = "mutation($p: ID!, $i: ID!, $f: ID!, $v: ProjectV2FieldValue!) { updateProjectV2ItemFieldValue(input: {projectId: $p, itemId: $i, fieldId: $f, value: $v}) { clientMutationId } }"
        current = (item or {}).get("status") or {}
        if current.get("name") != status:
            self.gh.write(f"#{issue['number']}: {current.get('name')} -> {status}", "POST", query=update, p=self.id, i=item_id, f=self.status_field, v={"singleSelectOptionId": self.status_options[status]})
        if status in ACTIVE and not ((item or {}).get("start") or {}).get("date"):
            today = datetime.date.today().isoformat()
            self.gh.write(f"#{issue['number']}: Start date {today}", "POST", query=update, p=self.id, i=item_id, f=self.start_field, v={"date": today})

    def sync(self, n, signal=None, cascade=True):
        """Bring one issue to the status the rules give it; returns whether it is an epic (has sub-issues)."""
        issue = self.issue(n)
        if issue is None:
            return False
        item = board_item(issue)
        closed = issue["state"] == "CLOSED"
        children = issue["subIssues"]["nodes"]
        if children:
            states = [(c["state"] == "CLOSED", ((board_item(c) or {}).get("status") or {}).get("name")) for c in children]
            status = epic_status(closed=closed, children=states)
        else:
            current = ((item or {}).get("status") or {}).get("name")
            blocked = any(b["state"] == "OPEN" for b in issue["blockedBy"]["nodes"])
            status = next_status(current, closed=closed, blocked=blocked, assigned=issue["assignees"]["totalCount"] > 0, signal=signal)
        self.set_status(issue, item, status)
        if cascade:
            # Closing or reopening an issue changes what it blocks, and every change can move its epic.
            for b in issue["blocking"]["nodes"]:
                self.sync(b["number"], cascade=False)
            if issue["parent"]:
                self.sync(issue["parent"]["number"], cascade=False)
        return bool(children)


def on_pull_request(board, gh, signal, number):
    pr = gh.gql(
        """query($owner: String!, $repo: String!, $n: Int!) { repository(owner: $owner, name: $repo) { pullRequest(number: $n) {
        author { login } closingIssuesReferences(first: 20) { nodes { number assignees(first: 1) { totalCount } } } } } }""",
        owner=OWNER,
        repo=REPO,
        n=number,
    )["repository"]["pullRequest"]
    for issue in pr["closingIssuesReferences"]["nodes"]:
        # Opening a pull request for an issue is starting it: its author takes the issue if nobody has.
        if signal in ("draft", "ready") and issue["assignees"]["totalCount"] == 0 and pr["author"]:
            gh.write(f"#{issue['number']}: assign {pr['author']['login']}", "POST", f"https://api.github.com/repos/{OWNER}/{REPO}/issues/{issue['number']}/assignees", {"assignees": [pr["author"]["login"]]})
        board.sync(issue["number"], signal=signal)


def reconcile(board, gh):
    """Everything an event can miss: relationships changed in the sidebar (no event), runs that failed, issues not on
    the board. Visits every open issue and every board item."""
    numbers = set()
    page = 1
    while True:
        batch = gh.call("GET", f"https://api.github.com/repos/{OWNER}/{REPO}/issues?state=open&per_page=100&page={page}")
        numbers |= {i["number"] for i in batch if "pull_request" not in i}
        if len(batch) < 100:
            break
        page += 1
    cursor = None
    while True:
        data = gh.gql(
            """query($owner: String!, $n: Int!, $after: String) { user(login: $owner) { projectV2(number: $n) {
            items(first: 100, after: $after) { pageInfo { hasNextPage endCursor }
            nodes { status: fieldValueByName(name: "Status") { ... on ProjectV2ItemFieldSingleSelectValue { name } }
            content { ... on Issue { number repository { nameWithOwner } } } } } } } }""",
            owner=OWNER,
            n=PROJECT,
            after=cursor,
        )["user"]["projectV2"]["items"]
        for node in data["nodes"]:
            c = node["content"] or {}
            if c.get("repository", {}).get("nameWithOwner") == f"{OWNER}/{REPO}" and (node["status"] or {}).get("name") != "Done":
                numbers.add(c["number"])
        if not data["pageInfo"]["hasNextPage"]:
            break
        cursor = data["pageInfo"]["endCursor"]
    epics = [n for n in sorted(numbers) if board.sync(n, cascade=False)]
    # Again, now that every child has its new status.
    for n in epics:
        board.sync(n, cascade=False)


def main(argv):
    token = os.environ.get("PROJECT_TOKEN")
    if not token:
        print("::warning::PROJECT_TOKEN is not set (or not available to this event); the project board was not updated")
        return 0
    mode = argv[1] if len(argv) > 1 else ""
    gh = GitHub(token, dry_run="--dry-run" in argv)
    board = Board(gh)
    if mode == "reconcile":
        reconcile(board, gh)
        return 0
    if mode != "event":
        print(__doc__, file=sys.stderr)
        return 2
    event_name = os.environ["GITHUB_EVENT_NAME"]
    with open(os.environ["GITHUB_EVENT_PATH"], encoding="utf-8") as f:
        payload = json.load(f)
    if event_name == "issues":
        if "pull_request" not in payload["issue"]:
            board.sync(payload["issue"]["number"])
        return 0
    found = signal_for(event_name, payload)
    if found:
        on_pull_request(board, gh, *found)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
