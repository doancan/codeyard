//! Workbench: `.http` / `.rest` requests. "HTTP: Send Request" sends the cursor's request with `curl` on the workspace's host
//! (on the server over SSH, for `localhost` services); the response body opens in a syntax-highlighted tab,
//! status line, duration and size show in the notification. Responses of named requests can be used in later requests as
//! `{{name.response.body.$.x}}`.

use super::*;
use crate::http_client::{self as http, Response, Stored};

/// Cheap random number for `{{$randomInt}}` only; not cryptographic.
fn random_between(a: i64, b: i64) -> i64 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() as i64);
    let span = (b - a + 1).max(1);
    a + (nanos.wrapping_mul(2654435761) & 0x7fff_ffff) % span
}

/// "https://host/path?x" → "host/path" (shortened for the notification).
fn short_url(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    if rest.chars().count() > 60 { rest.chars().take(59).collect::<String>() + "…" } else { rest.to_string() }
}

impl Workbench {
    pub(super) fn send_request(&mut self, _: &SendRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.send_request_at(None, window, cx);
    }

    /// `line`: 0-based line (when a link is clicked); otherwise the cursor's line.
    pub(super) fn send_request_at(&mut self, line: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let path = ed.read(cx).path.clone();
        if !http::is_http_file(&path) {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a .http or .rest file to send requests.").into()));
            cx.notify();
            return;
        }
        let text = ed.read(cx).rope_snapshot().to_string();
        let line = line.unwrap_or_else(|| ed.read(cx).cursor_position().0.saturating_sub(1));
        let parsed = http::parse(&text);
        let Some(ix) = http::request_at(&parsed, line) else {
            self.notice = Some(Notice::Info(crate::i18n::t("There is no request here. Write METHOD URL on a line, separate requests with ###.").into()));
            cx.notify();
            return;
        };
        let request = parsed.requests[ix].clone();
        // Two reasons to ask first, both answered by one prompt: the request does something a file from someone else should not do
        // silently (read the environment, upload files from elsewhere; once per file and session), or it goes to a host this
        // session has not contacted yet (`http.confirmRequests`).
        let sensitive = if self.lsp.http_trusted.contains(&self.trust_key(&path)) { Vec::new() } else { http::sensitive_parts(&request) };
        let mut reasons = sensitive.clone();
        let (mode, gate) = {
            let mode = schema::gate::Confirm::parse(&cx.global::<Settings>().http_confirm).unwrap_or_default();
            let gate = self.schema_service(cx).store.gate().clone();
            gate.set_mode(mode);
            (mode, gate)
        };
        let mut approve_host = None;
        if mode != schema::gate::Confirm::Never {
            let responses = self.lsp.http_responses.clone();
            let env = |n: &str| std::env::var(n).ok();
            let uuid = crate::texttools::uuid_v4;
            let ctx = http::Context { vars: &parsed.vars, responses: &responses, now: now_secs(), env: &env, uuid: &uuid, random: &random_between };
            // A URL that does not resolve is reported by the send itself; nothing goes out for it.
            if let Ok(url) = http::substitute(&request.url, &ctx, 0) {
                match schema::gate::host_of(&url) {
                    Some(host) if mode == schema::gate::Confirm::Always || gate.needs_approval(&host) => {
                        reasons.push(if mode == schema::gate::Confirm::Always {
                            crate::i18n::tf("sends {} to {}", &[&request.method, &host])
                        } else {
                            crate::i18n::tf("goes to {}, which you have not sent anything to in this session", &[&host])
                        });
                        approve_host = Some(host);
                    }
                    Some(_) => {}
                    None => reasons.push(crate::i18n::t("goes to an address whose host cannot be read reliably").to_string()),
                }
            }
        }
        if reasons.is_empty() {
            self.dispatch_request(request, parsed.vars.clone(), path, window, cx);
            return;
        }
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let detail = format!(
            "{}\n• {}\n\n{}",
            crate::i18n::tf("{} contains a request that:", &[&name]),
            reasons.join("\n• "),
            crate::i18n::t("Only send it if you trust this file.")
        );
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            crate::i18n::t("Send this request?"),
            Some(&detail),
            &[crate::i18n::t("Send"), crate::i18n::t("Cancel")],
            cx,
        );
        let vars = parsed.vars.clone();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| {
                    if !sensitive.is_empty() {
                        this.lsp.http_trusted.insert(this.trust_key(&path));
                    }
                    if let Some(host) = &approve_host {
                        gate.approve(host);
                    }
                    this.dispatch_request(request, vars, path, window, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// After confirmation (if needed), prepare the request and send it with `curl`.
    pub(super) fn dispatch_request(&mut self, request: http::Request, vars: Vec<(String, String)>, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.lsp.http_busy {
            self.notice = Some(Notice::Info(crate::i18n::t("A request is still being sent; wait for its answer first.").into()));
            cx.notify();
            return;
        }
        self.lsp.http_busy = true;
        let responses = self.lsp.http_responses.clone();
        let local = cx.global::<Settings>().http_run_on == "local";
        let file_host = self.active_editor().map_or_else(|| self.remote.host.clone(), |e| e.read(cx).host_arc());
        let exec_host = if local { self.remote.local_host.clone() } else { self.remote.host.clone() };
        let dir = path.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string());
        self.notice = Some(Notice::Info(crate::i18n::tf("Sending {} {}…", &[&request.method, &short_url(&request.url)]).into()));
        cx.notify();
        let name = request.name.clone();
        let task = cx.background_executor().spawn(async move {
            let now = now_secs();
            let env = |n: &str| std::env::var(n).ok();
            let uuid = crate::texttools::uuid_v4;
            let ctx = http::Context { vars: &vars, responses: &responses, now, env: &env, uuid: &uuid, random: &random_between };
            let read = |p: &str| -> Result<Vec<u8>, String> {
                let full = if p.starts_with('/') { p.to_string() } else { ui::join(&dir, p.trim_start_matches("./")) };
                file_host.read(&full).map(|(d, _)| d).map_err(|e| crate::i18n::tf("could not read {}: {}", &[&p, &e]))
            };
            let prepared = http::prepare(&request, &ctx, &read)?;
            let args = http::curl_args(&prepared);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let place = if local { crate::i18n::t("this computer").to_string() } else { exec_host.label() };
            let out = exec_host.exec("curl", &args, None, prepared.body.clone()).map_err(|e| crate::i18n::tf("could not run curl on {}: {}", &[&place, &e]))?;
            if out.code == Some(127) {
                return Err(crate::i18n::tf("curl is not installed on {0}; install it or set http.runOn to {1}", &[&place, &"\"local\""]));
            }
            match http::parse_output(&out.stdout) {
                Ok(r) => Ok((prepared, r)),
                Err(_) => Err(http::explain_curl(out.code, &out.stderr_str(), &place, &prepared.url)),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.lsp.http_busy = false;
                match result {
                    Ok((prepared, response)) => this.show_response(name, &prepared, response, window, cx),
                    Err(e) => {
                        this.notice = Some(Notice::Error(e.into()));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn show_response(&mut self, name: Option<String>, prepared: &http::Prepared, r: Response, window: &mut Window, cx: &mut Context<Self>) {
        let summary = format!(
            "{} {} → {} · {} ms · {}",
            prepared.method,
            short_url(&prepared.url),
            http::status_line(&r),
            r.millis,
            http::human_size(r.size.max(r.body.len() as u64))
        );
        if let Some(n) = name {
            self.lsp.http_responses.insert(n, Stored { headers: r.headers.clone(), body: r.decoded().map(|(t, _)| t).unwrap_or_default() });
        }
        // Close the previous response tab so sends don't pile up tabs.
        if let Some(old) = self.lsp.http_tab.take()
            && let Some(ix) = self.tabs.editors.iter().position(|e| e.read(cx).path == old)
        {
            self.close_editor_at(ix, window, cx);
            let _ = std::fs::remove_file(&old);
        }
        let r_empty = r.body.is_empty();
        let decoded = r.decoded();
        let mut note = decoded.as_ref().and_then(|(_, n)| n.clone());
        if let Some(to) = http::redirect_target(&r) {
            // Not followed: the next hop was never approved, so the user decides whether to send there.
            note = Some(crate::i18n::tf("redirects to {}; not followed, send a request to that address to continue", &[&to]));
        }
        let (name, content) = match decoded {
            Some((t, _)) if r.extension() == "json" => {
                let pretty = crate::jsonfmt::format(&t, "  ").unwrap_or(t);
                (format!("response.{}", r.extension()), pretty)
            }
            Some((t, _)) => (format!("response.{}", r.extension()), t),
            None => {
                note = Some(if r.body.len() > crate::hexdump::MAX_BYTES {
                    crate::i18n::tf("binary content, first {} shown as hex", &[&http::human_size(crate::hexdump::MAX_BYTES as u64)])
                } else {
                    crate::i18n::t("binary content shown as hex").to_string()
                });
                ("response.hex.txt".to_string(), crate::hexdump::dump(&r.body))
            }
        };
        self.lsp.http_last = Some(r);
        // The file just written, not the active editor: the tab opens asynchronously, so that would still be the `.http` file
        // and the next request (or quitting) would delete the user's request file.
        self.lsp.http_tab = self.create_scratch_with(name, &content, window, cx);
        self.notice = Some(Notice::Info(
            match note {
                Some(n) => format!("{summary} · {n}"),
                None if r_empty => format!("{summary} · {}", crate::i18n::t("empty body")),
                None => summary,
            }
            .into(),
        ));
        cx.notify();
    }

    /// The response file holds whatever the server sent (tokens, personal data): it is removed when its tab closes and at quit,
    /// not left behind until the next request.
    pub(super) fn discard_response_file(&mut self, path: &str) {
        if self.lsp.http_tab.as_deref() == Some(path) {
            self.lsp.http_tab = None;
            let _ = std::fs::remove_file(path);
        }
    }

    pub(super) fn discard_response_on_quit(&mut self) {
        if let Some(path) = self.lsp.http_tab.take() {
            let _ = std::fs::remove_file(path);
        }
    }

    /// "HTTP: Show Last Response Headers"
    pub(super) fn show_response_headers(&mut self, _: &ShowResponseHeaders, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.lsp.http_last.clone() else {
            self.notice = Some(Notice::Info(crate::i18n::t("Send a request first.").into()));
            cx.notify();
            return;
        };
        let mut text = format!("{}\n", http::status_line(&r));
        for (k, v) in &r.headers {
            text.push_str(&format!("{k}: {v}\n"));
        }
        text.push_str(&format!("\n# {} ms · {}\n", r.millis, http::human_size(r.size)));
        self.create_scratch_with("response-headers.txt".into(), &text, window, cx);
    }
}
