//! What the Extension Inspector and the extension page show about running code extensions
//! (docs/plugin-architecture.md §6.3, §9): when and why each one activated, every call with its guest, I/O and
//! overhead time, recent denials with the manifest line that fixes them, and the crash report. In memory only; nothing
//! here is written to disk or sent anywhere. No GPUI: the workbench records into a `Record` shared with the background
//! threads that run broker calls, and draws from it.
//!
//! Times are what the app can measure: I/O is the work the app did for a call (the broker's run, the window's answer,
//! a pick waiting for the user); overhead is the rest of the time the call spent in the app (frame handling, queues);
//! guest time is a request's time in the exthost minus the I/O and overhead of the calls it waited for.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ext_api::value::{self as v, Completion};
use ext_manifest::Permissions;

/// Entries kept, oldest dropped first.
pub const CALLS: usize = 200;
pub const DENIALS: usize = 50;

/// When and why an extension's exthost last started.
#[derive(Clone, Debug, PartialEq)]
pub struct Activation {
    pub at: u64,
    /// `command:refresh`, `timer`, …
    pub reason: String,
    pub count: u32,
}

/// One finished call.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub extension: String,
    pub at: u64,
    /// `interface/function` for a host call, `command:<id>` for a request to the extension.
    pub name: String,
    /// For requests: time spent in the extension's own code.
    pub guest: Option<Duration>,
    pub io: Duration,
    pub overhead: Duration,
    /// `None` when it succeeded, else a short reason (`denied`, `failed`).
    pub error: Option<String>,
}

/// A call that was refused.
#[derive(Clone, Debug, PartialEq)]
pub struct Denial {
    pub extension: String,
    pub at: u64,
    pub call: String,
    pub permission: String,
    pub why: v::Why,
}

struct OpenTicket {
    name: &'static str,
    started: Instant,
    /// Spent in the app before the call's work began (looking up the extension and its scope).
    before: Duration,
}

struct OpenRequest {
    name: String,
    started: Instant,
    waited: Duration,
}

/// Everything the Inspector shows, shared between the window and background threads.
#[derive(Default)]
pub struct Record {
    pub activations: HashMap<String, Activation>,
    pub calls: VecDeque<Call>,
    pub denials: VecDeque<Denial>,
    tickets: HashMap<(String, u64), OpenTicket>,
    requests: HashMap<(String, u64), OpenRequest>,
}

/// Seconds since 1970, for the lists.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn push<T>(list: &mut VecDeque<T>, cap: usize, item: T) {
    if list.len() >= cap {
        list.pop_front();
    }
    list.push_back(item);
}

impl Record {
    pub fn activated(&mut self, extension: &str, reason: &str) {
        let count = self.activations.get(extension).map_or(0, |a| a.count) + 1;
        self.activations.insert(extension.to_string(), Activation { at: now(), reason: reason.to_string(), count });
    }

    /// A request (`ToHost::Handle`) was sent to the extension.
    pub fn request_sent(&mut self, extension: &str, id: u64, name: &str) {
        let r = OpenRequest { name: name.to_string(), started: Instant::now(), waited: Duration::ZERO };
        self.requests.insert((extension.to_string(), id), r);
    }

    /// The extension answered request `id` (`requests.respond`).
    pub fn request_answered(&mut self, extension: &str, id: u64, failed: bool) {
        let Some(r) = self.requests.remove(&(extension.to_string(), id)) else { return };
        let total = r.started.elapsed();
        let call = Call {
            extension: extension.to_string(),
            at: now(),
            name: r.name,
            guest: Some(total.saturating_sub(r.waited)),
            io: Duration::ZERO,
            overhead: Duration::ZERO,
            error: failed.then(|| "failed".to_string()),
        };
        push(&mut self.calls, CALLS, call);
    }

    /// A host call arrived (`ToApp::Start`) at `arrived`; its work starts now.
    pub fn ticket_started(&mut self, extension: &str, ticket: u64, name: &'static str, arrived: Instant) {
        let now = Instant::now();
        self.tickets.insert((extension.to_string(), ticket), OpenTicket { name, started: now, before: now.saturating_duration_since(arrived) });
    }

    /// A host call completed. `io` is the time its work took when it ran elsewhere (a broker call on a background
    /// thread); `None` when the window did the work from the start until now.
    pub fn ticket_done(&mut self, extension: &str, ticket: u64, completion: &Completion, io: Option<Duration>) {
        let Some(t) = self.tickets.remove(&(extension.to_string(), ticket)) else { return };
        let since = t.started.elapsed();
        let io = io.unwrap_or(since).min(since);
        let overhead = t.before + since.saturating_sub(io);
        let error = match completion {
            Completion::Failed(v::Error::Denied(d)) => {
                self.denied(extension, t.name, d);
                Some("denied".to_string())
            }
            Completion::Failed(_) => Some("failed".to_string()),
            _ => None,
        };
        for (_, r) in self.requests.iter_mut().filter(|((e, _), _)| e == extension) {
            r.waited += io + overhead;
        }
        push(&mut self.calls, CALLS, Call { extension: extension.to_string(), at: now(), name: t.name.to_string(), guest: None, io, overhead, error });
    }

    /// A refused call: by the broker or the window (through `ticket_done`), or by the exthost itself (`ToApp::Denied`).
    pub fn denied(&mut self, extension: &str, call: &str, d: &v::Denied) {
        let entry = Denial { extension: extension.to_string(), at: now(), call: call.to_string(), permission: d.permission.clone(), why: d.why };
        push(&mut self.denials, DENIALS, entry);
    }

    /// The exthost went away: what it owed is no longer timed.
    pub fn exited(&mut self, extension: &str) {
        self.tickets.retain(|(e, _), _| e != extension);
        self.requests.retain(|(e, _), _| e != extension);
    }

    pub fn denials_of<'a>(&'a self, extension: &'a str) -> impl DoubleEndedIterator<Item = &'a Denial> + 'a {
        self.denials.iter().filter(move |d| d.extension == extension)
    }
}

/// The `[permissions]` line that makes a denied call pass. For a permission the user turned off (`revoked-by-user`,
/// where the exthost knows only the kind, `net`), the entries of that kind they turned off: turning those on again on
/// the extension page is the fix, and the line says which. `None` when no manifest line helps (an untrusted folder, a
/// path outside the workspace, a call that needs a user action).
pub fn fix_line(d: &Denial, revoked: &Permissions) -> Option<String> {
    let p = d.permission.trim();
    match d.why {
        v::Why::UntrustedWorkspace | v::Why::RevokedExtension => None,
        _ if p.contains(" = ") => Some(p.to_string()),
        v::Why::NotDeclared => None,
        v::Why::RevokedByUser => {
            let off: Vec<_> = ext_core::grants::entries(revoked).into_iter().filter(|e| e.kind == p).collect();
            let first = off.first()?;
            match p {
                // Several hosts (globs, programs) of one kind make one line.
                "read" | "write" | "net" | "auth" | "run" | "commands" => {
                    Some(format!("{p} = [{}]", off.iter().map(|e| format!("{:?}", e.value)).collect::<Vec<_>>().join(", ")))
                }
                _ => Some(first.manifest_line()),
            }
        }
    }
}

/// A path in a trap frame or message, shortened so the report names nothing outside the extension: the part from its
/// last `src/` folder, or only the file name.
fn short_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    if let Some(i) = p.rfind("/src/") {
        return p[i + 1..].to_string();
    }
    p.rsplit('/').next().unwrap_or_default().to_string()
}

fn absolute(token: &str) -> bool {
    let b = token.as_bytes();
    token.starts_with('/') || token.starts_with('~') || token.starts_with("\\\\") || (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// `s` with every absolute path replaced by `short_path`.
pub fn scrub(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if absolute(token) && (token.contains('/') || token.contains('\\')) {
            out.push_str(&short_path(token));
        } else {
            out.push_str(token);
        }
        token.clear();
    };
    for ch in s.chars() {
        if ch.is_whitespace() || matches!(ch, '(' | ')' | '"' | '\'' | '`' | ',') {
            flush(&mut token, &mut out);
            out.push(ch);
        } else {
            token.push(ch);
        }
    }
    flush(&mut token, &mut out);
    out
}

/// **Copy report** on the crash card: the extension and its version, Codeyard's version and platform, the trap and
/// its frames. No log lines, no file contents, no paths outside the extension.
pub fn crash_report(id: &str, version: &str, trap: Option<&ext_proto::Trap>) -> String {
    let mut out = format!("{id} {version} crashed three times within a minute\n");
    out.push_str(&format!("Codeyard {} on {}\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS));
    match trap {
        Some(t) => {
            out.push_str(&format!("\ntrap: {}\n", scrub(&t.message)));
            for f in &t.frames {
                out.push_str(&format!("  at {}\n", scrub(f)));
            }
        }
        None => out.push_str("\nThe extension host stopped without a trap.\n"),
    }
    out
}

/// **Report**: a new issue in the extension's repository with the report filled in, for `https:` repositories only.
pub fn issue_url(repository: &str, id: &str, version: &str, report: &str) -> Option<String> {
    let repo = repository.trim().trim_end_matches('/').trim_end_matches(".git");
    if !repo.starts_with("https://") || repo.contains(['?', '#', ' ']) {
        return None;
    }
    let title = format!("{id} {version} crashes");
    let body = format!("```\n{report}```\n");
    Some(format!("{repo}/issues/new?title={}&body={}", crate::texttools::url_encode(&title), crate::texttools::url_encode(&body)))
}

/// `1.2 ms`, `340 µs`, `2.5 s`.
pub fn duration(d: Duration) -> String {
    let us = d.as_micros();
    if us < 1000 {
        format!("{us} µs")
    } else if us < 1_000_000 {
        format!("{:.1} ms", us as f64 / 1000.0)
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

/// `14:03:22`, in UTC like the rest of the app's logs.
pub fn clock(at: u64) -> String {
    let t = at % 86_400;
    format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}

#[cfg(test)]
#[path = "ext_inspector_tests.rs"]
mod tests;
