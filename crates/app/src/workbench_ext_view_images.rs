//! Workbench: `image` elements with an `https` URL in extension views (docs/extensions/views.md). Codeyard fetches them
//! for the extension, through its `net` permission: the URL's host must be one it declares, the folder must be
//! trusted, and the request goes through ext-net (peer address checks, re-checked redirects). The body may be at most
//! `limits::UI_IMAGE_BYTES` and is drawn only when it is a format GPUI decodes. A view fetches only while it is on
//! screen, a few images at a time, and keeps what it fetched by URL until it closes. A refusal is recorded for the
//! Inspector like any other denial; the image shows its alt text.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ext_api::value::{self as v, Completion, LogLevel, StreamEvent};
use ext_manifest::limits;

use super::*;
use crate::ext_broker::{Scope, Sink};
use crate::ext_fetch::{self, Transport};

/// Requests one view has on their way at once; the other images wait their turn.
const AT_ONCE: usize = 2;
/// One image request, from the first connection to the last byte.
const TIMEOUT_MS: u32 = 30_000;

/// One URL image of a view.
pub(in crate::workbench) enum Remote {
    Loading,
    Ready(Arc<gpui::Image>),
    /// Refused or failed; the alt text stays. Not asked again while the view is open.
    Failed,
}

/// The URL images of one view.
#[derive(Default)]
pub(in crate::workbench) struct RemoteImages {
    /// By URL, for as long as the view is open.
    pub(super) cache: HashMap<String, Remote>,
    /// URLs of the tree on screen not asked for yet, in tree order.
    wanted: Vec<String>,
    /// Requests on their way.
    fetching: usize,
}

impl RemoteImages {
    pub(in crate::workbench) fn image(&self, url: &str) -> Option<Arc<gpui::Image>> {
        match self.cache.get(url) {
            Some(Remote::Ready(i)) => Some(i.clone()),
            _ => None,
        }
    }

    /// The URLs of a new tree. Those already fetched are not asked again; past `limits::UI_IMAGE_URLS` URLs per view the
    /// rest keep their alt text.
    pub(super) fn want(&mut self, urls: Vec<String>) {
        let room = limits::UI_IMAGE_URLS.saturating_sub(self.cache.len());
        let mut wanted: Vec<String> = Vec::new();
        for u in urls {
            if !self.cache.contains_key(&u) && !wanted.contains(&u) && wanted.len() < room {
                wanted.push(u);
            }
        }
        self.wanted = wanted;
    }

    /// The next URL to ask for, when fewer than `AT_ONCE` are on their way.
    fn next(&mut self) -> Option<String> {
        if self.fetching >= AT_ONCE || self.wanted.is_empty() {
            return None;
        }
        let url = self.wanted.remove(0);
        self.cache.insert(url.clone(), Remote::Loading);
        self.fetching += 1;
        Some(url)
    }
}

/// Collects a response body up to `limits::UI_IMAGE_BYTES`; past that it stops the request.
struct Body {
    status: Mutex<u16>,
    bytes: Mutex<Vec<u8>>,
    too_big: AtomicBool,
    cancel: Arc<AtomicBool>,
}

impl Sink for Body {
    fn event(&self, e: StreamEvent) {
        match e {
            StreamEvent::HttpHead(h) => *self.status.lock().unwrap_or_else(|p| p.into_inner()) = h.status,
            StreamEvent::Data(d) if !self.too_big.load(Ordering::Relaxed) => {
                let mut b = self.bytes.lock().unwrap_or_else(|p| p.into_inner());
                if b.len() + d.len() > limits::UI_IMAGE_BYTES {
                    self.too_big.store(true, Ordering::Relaxed);
                    self.cancel.store(true, Ordering::Relaxed);
                    b.clear();
                } else {
                    b.extend_from_slice(&d);
                }
            }
            _ => {}
        }
    }
}

/// Fetch and check one image for the extension of `scope`. Blocking: run it on a background thread.
pub(super) fn fetch_image(t: &dyn Transport, scope: &Scope, url: &str) -> Result<Arc<gpui::Image>, v::Error> {
    if !scope.trusted {
        return Err(v::Error::Denied(v::Denied { permission: "net".into(), why: v::Why::UntrustedWorkspace }));
    }
    let host = ext_fetch::https_host(url)?;
    if !ext_manifest::net_allows(&scope.permissions.net, &host) {
        let d = if ext_manifest::net_allows(&scope.revoked.net, &host) {
            v::Denied { permission: "net".into(), why: v::Why::RevokedByUser }
        } else {
            ext_fetch::not_in_net(&host)
        };
        return Err(v::Error::Denied(d));
    }
    let req = v::HttpRequest {
        method: "GET".into(),
        url: url.to_string(),
        headers: vec![("accept".into(), "image/png, image/jpeg, image/gif, image/webp, image/svg+xml".into())],
        body: Vec::new(),
        timeout_ms: Some(TIMEOUT_MS),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let body = Body { status: Mutex::new(0), bytes: Mutex::new(Vec::new()), too_big: AtomicBool::new(false), cancel: cancel.clone() };
    let completion = ext_fetch::net_fetch(t, scope, &req, &body, &cancel);
    if body.too_big.load(Ordering::Relaxed) {
        return Err(v::Error::Invalid(crate::i18n::tf("the image is larger than {} bytes", &[&limits::UI_IMAGE_BYTES])));
    }
    if let Completion::Failed(e) = completion {
        return Err(e);
    }
    let status = *body.status.lock().unwrap_or_else(|p| p.into_inner());
    if !(200..300).contains(&status) {
        return Err(v::Error::Other(crate::i18n::tf("the server answered {}", &[&status])));
    }
    let bytes = body.bytes.into_inner().unwrap_or_else(|p| p.into_inner());
    let format = super::image_format(&bytes).ok_or_else(|| v::Error::Invalid(crate::i18n::t("not a PNG, JPEG, GIF, WebP or SVG image").into()))?;
    Ok(Arc::new(gpui::Image::from_bytes(format, bytes)))
}

impl Workbench {
    /// Start the URL images of the views on screen, a few per view at a time.
    pub(super) fn fetch_ext_view_images(&mut self, cx: &mut Context<Self>) {
        for id in self.ext_views_on_screen() {
            while let Some(url) = self.ext_view_mut(&id).and_then(|h| h.remote.next()) {
                self.start_ext_view_image(&id, url, cx);
            }
        }
    }

    fn start_ext_view_image(&mut self, id: &str, url: String, cx: &mut Context<Self>) {
        let Some(extension) = self.ext_view(id).map(|h| h.decl.extension.clone()) else { return };
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == extension) else {
            self.ext_view_image_done(id, &extension, url, Err(v::Error::NotFound(extension.clone())), cx);
            return;
        };
        let scope = self.extension_scope(active, cx);
        let transport = self.cfg.exts.accounts.transport.clone();
        let (view, target) = (id.to_string(), url.clone());
        let task = cx.background_executor().spawn(async move { fetch_image(transport.as_ref(), &scope, &target) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.ext_view_image_done(&view, &extension, url, result, cx)).ok();
        })
        .detach();
    }

    fn ext_view_image_done(&mut self, id: &str, extension: &str, url: String, result: Result<Arc<gpui::Image>, v::Error>, cx: &mut Context<Self>) {
        let local = self.ext_view(id).map_or_else(|| id.to_string(), |h| h.decl.local.clone());
        if let Err(e) = &result {
            if let v::Error::Denied(d) = e {
                self.cfg.exts.code.record().denied(extension, "ui.image", d);
            }
            self.log_extension(extension, LogLevel::Warn, format!("view {local}: image {url}: {}", super::super::code_glue::explain(e)));
        }
        if let Some(h) = self.ext_view_mut(id) {
            h.remote.fetching = h.remote.fetching.saturating_sub(1);
            h.remote.cache.insert(url, result.map_or(Remote::Failed, Remote::Ready));
        }
        cx.notify();
    }
}

#[cfg(test)]
impl Workbench {
    /// Images of extension views are fetched with `transport` (a fake server in tests).
    pub(crate) fn ext_view_transport_for_test(&mut self, transport: Arc<dyn Transport>) {
        self.cfg.exts.accounts.transport = transport;
    }

    /// The URL images of view `id`: (url, drawn, failed).
    pub(crate) fn ext_view_remote_for_test(&self, id: &str) -> Vec<(String, bool, bool)> {
        let Some(h) = self.ext_view(id) else { return Vec::new() };
        let mut out: Vec<_> = h.remote.cache.iter().map(|(u, r)| (u.clone(), matches!(r, Remote::Ready(_)), matches!(r, Remote::Failed))).collect();
        out.sort();
        out
    }
}
