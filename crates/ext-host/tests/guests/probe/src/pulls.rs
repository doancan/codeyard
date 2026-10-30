//! The probe's pull request provider (#145), as small as the app's tests need. Every forge call goes through
//! `auth.fetch` with the remote's account, so the app signs it.
//!
//! - `list` fetches `https://api.acme.example.com/repos/<path>/pulls`, one request per line: `id|title|author|source|target`.
//! - `details` fetches `…/pulls/<id>`: the body is the description.
//! - `add-comment` posts the comment to `…/pulls/<id>/comments`; `comments` lists the ones added since the start.
//! - `files`, `contents` and `checkout` answer without the network.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::codeyard::api::pull_requests::{
    Answer, ChangedFile, Check, CheckState, Checkout, Comment, Details, FileContents, FileStatus, Query, Remote, State, Summary,
};
use crate::codeyard::api::responses::Response;
use crate::codeyard::api::types::{Completion, Error, HttpRequest, Ticket};
use crate::codeyard::api::{auth, requests};

/// What a ticket's body becomes once it arrives.
enum Then {
    List,
    Details(String),
    Commented(String),
}

thread_local! {
    static WAITING: RefCell<HashMap<Ticket, (u64, Then)>> = RefCell::new(HashMap::new());
    static COMMENTS: RefCell<Vec<Comment>> = const { RefCell::new(Vec::new()) };
}

fn api(remote: &Remote, rest: &str) -> String {
    format!("https://api.acme.example.com/repos/{}/pulls{rest}", remote.path)
}

fn fetch(remote: &Remote, method: &str, url: String, body: &str) -> Ticket {
    let account = remote.account.clone().unwrap_or_else(|| "acme".into());
    let request = HttpRequest { method: method.into(), url, headers: vec![], body: body.as_bytes().to_vec(), timeout_ms: None };
    auth::fetch(&account, &request)
}

fn answer(id: u64, a: Answer) {
    requests::respond(id, Ok(&Response::PullRequests(a)));
}

fn summary(line: &str) -> Option<Summary> {
    let mut f = line.split('|');
    let (id, title, author, source, target) = (f.next()?, f.next()?, f.next()?, f.next()?, f.next()?);
    Some(Summary {
        id: id.into(),
        reference: format!("!{id}"),
        title: title.into(),
        author: author.into(),
        state: State::Open,
        source_branch: source.into(),
        target_branch: target.into(),
        updated: 0,
        url: Some(format!("https://git.acme.example.com/requests/{id}")),
    })
}

pub fn handle(id: u64, query: Query) {
    match query {
        Query::List(remote) => {
            let t = fetch(&remote, "GET", api(&remote, ""), "");
            WAITING.with(|w| w.borrow_mut().insert(t, (id, Then::List)));
        }
        Query::Details((remote, pr)) => {
            let t = fetch(&remote, "GET", api(&remote, &format!("/{pr}")), "");
            WAITING.with(|w| w.borrow_mut().insert(t, (id, Then::Details(pr))));
        }
        Query::Files(_) => answer(
            id,
            Answer::Files(vec![
                ChangedFile { path: "a.txt".into(), old_path: None, status: FileStatus::Modified },
                ChangedFile { path: "new.txt".into(), old_path: None, status: FileStatus::Added },
            ]),
        ),
        Query::Contents((_, _, file)) => {
            let base = (file.status != FileStatus::Added).then(|| "one\n".to_string());
            answer(id, Answer::Contents(FileContents { base, head: Some(format!("one\ntwo from {}\n", file.path)) }))
        }
        Query::Comments(_) => answer(id, Answer::Comments(COMMENTS.with(|c| c.borrow().clone()))),
        Query::AddComment((remote, pr, body)) => {
            let t = fetch(&remote, "POST", api(&remote, &format!("/{pr}/comments")), &body);
            WAITING.with(|w| w.borrow_mut().insert(t, (id, Then::Commented(body))));
        }
        Query::Checkout((_, pr)) => answer(id, Answer::Checkout(Checkout { fetch_url: None, ref_: format!("refs/pull/{pr}/head"), branch: format!("pr-{pr}") })),
    }
}

/// A ticket of this provider finished: `None` when it is not one of ours.
pub fn complete(ticket: Ticket, result: &Completion, body: Option<&[u8]>) -> Option<()> {
    let (id, then) = WAITING.with(|w| w.borrow_mut().remove(&ticket))?;
    if let Completion::Failed(e) = result {
        requests::respond(id, Err(e));
        return Some(());
    }
    let body = String::from_utf8_lossy(body.unwrap_or_default()).into_owned();
    match then {
        Then::List => answer(id, Answer::List(body.lines().filter_map(summary).collect())),
        Then::Details(pr) => match summary(&format!("{pr}|Request {pr}|ada|topic|main")) {
            Some(summary) => answer(
                id,
                Answer::Details(Details {
                    summary,
                    description: body,
                    checks: vec![Check { name: "ci".into(), state: CheckState::Success, url: None }],
                    labels: vec!["probe".into()],
                }),
            ),
            None => requests::respond(id, Err(&Error::Other("no summary".into()))),
        },
        Then::Commented(text) => {
            COMMENTS.with(|c| c.borrow_mut().push(Comment { author: "ada".into(), body: text, created: 0, path: None, line: None }));
            answer(id, Answer::Done)
        }
    }
    Some(())
}
