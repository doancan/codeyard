//! Read-only file views. Large file: files too big to read in one go;
//! the agent keeps the line index, and only 256-line chunks near the viewport
//! are requested here, with at most `MAX_CHUNKS` chunks kept. Image: preview of png, jpg,
//! gif, webp, svg, bmp (instead of the binary-file message).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ScrollStrategy, SharedString, Subscription, Task, UniformListScrollHandle, Window, actions, div, prelude::*,
    px, uniform_list,
};
use host::Host;
use proto::Response;

use crate::input::{FieldEvent, TextField};

actions!(large, [FindInLarge, LargeFindPrev, ToggleFollow, FollowEnd]);

/// Maximum matches shown (also caps the search on the agent).
const MAX_HITS: u32 = 10_000;

/// Find in a large file: the search runs on the agent (the file is never shipped to the client); results are line numbers.
struct LargeFind {
    field: Entity<TextField>,
    /// (1-based line, column)
    hits: Vec<(u32, u32)>,
    current: Option<usize>,
    state: FindState,
    running: Option<(u32, Task<()>)>,
    debounce: Option<Task<()>>,
    _sub: Subscription,
}

#[derive(Clone, PartialEq)]
enum FindState {
    Idle,
    Searching,
    Done { truncated: bool },
    Failed(SharedString),
}

use crate::settings::Settings;
use crate::ui::{self, MONO_FONT, c, icon, palette, text};

const CHUNK: u64 = 256;
/// Poll interval for a growing file.
const POLL: std::time::Duration = std::time::Duration::from_millis(1000);
/// Max chunks held in memory (≈16k lines).
const MAX_CHUNKS: usize = 64;

pub struct LargeFileView {
    host: Arc<dyn Host>,
    pub path: String,
    pub name: SharedString,
    lines: u64,
    size: u64,
    chunks: HashMap<u64, Vec<String>>,
    pending: HashSet<u64>,
    error: Option<SharedString>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    /// In image mode: the loaded image (`None` until loaded).
    image: Option<Arc<gpui::Image>>,
    is_image: bool,
    find: Option<LargeFind>,
    /// The found line shown (0-based), highlighted.
    highlight: Option<u64>,
    /// `tail -f`: stick to the end as the file grows (stops when the user scrolls up).
    follow: bool,
    /// Why the file can no longer be read (deleted, moved, connection lost); cleared once it is readable again.
    stale: Option<String>,
    /// New lines that arrived while following was paused (the "N new lines" indicator).
    unseen: u64,
    /// The loop watching for growth (only while `watching`).
    _watch: Option<Task<()>>,
}

/// Severity of a log line (for color): a case-insensitive keyword in the first 160 characters.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Level {
    Error,
    Warn,
    Quiet,
    Plain,
}

pub fn line_level(line: &str) -> Level {
    let head: String = line.chars().take(160).collect::<String>().to_ascii_uppercase();
    let has = |words: &[&str]| words.iter().any(|w| head.match_indices(w).any(|(i, _)| is_word_edge(&head, i, w.len())));
    if has(&["ERROR", "ERR", "FATAL", "CRITICAL", "PANIC", "SEVERE", "EXCEPTION"]) {
        Level::Error
    } else if has(&["WARN", "WARNING"]) {
        Level::Warn
    } else if has(&["DEBUG", "TRACE", "VERBOSE"]) {
        Level::Quiet
    } else {
        Level::Plain
    }
}

/// Is `text[at..at+len]` on a word boundary (no adjacent letter/digit)?
fn is_word_edge(text: &str, at: usize, len: usize) -> bool {
    let before = text[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
    let after = text[at + len..].chars().next().is_none_or(|c| !c.is_alphanumeric());
    before && after
}

/// Image format from the extension (previewable ones).
pub fn image_format(path: &str) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat::*;
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => Png,
        "jpg" | "jpeg" => Jpeg,
        "gif" => Gif,
        "webp" => Webp,
        "svg" => Svg,
        "bmp" => Bmp,
        _ => return None,
    })
}

impl Focusable for LargeFileView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl LargeFileView {
    pub fn new(host: Arc<dyn Host>, path: String, cx: &mut Context<Self>) -> Self {
        let view = LargeFileView {
            name: ui::file_name(&path).to_string().into(),
            host: host.clone(),
            path: path.clone(),
            lines: 0,
            size: 0,
            chunks: HashMap::new(),
            pending: HashSet::new(),
            error: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            image: None,
            is_image: false,
            find: None,
            highlight: None,
            follow: false,
            unseen: 0,
            stale: None,
            _watch: None,
        };
        // The line index is built on the agent (the file is scanned once).
        let task = cx.background_executor().spawn(async move { host.line_index(&path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |v, cx| {
                match result {
                    Ok((lines, size)) => (v.lines, v.size) = (lines, size),
                    Err(e) => v.error = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        view
    }

    /// Image preview: the file is read, the platform decodes and scales it.
    pub fn new_image(host: Arc<dyn Host>, path: String, format: gpui::ImageFormat, cx: &mut Context<Self>) -> Self {
        let view = LargeFileView {
            name: ui::file_name(&path).to_string().into(),
            host: host.clone(),
            path: path.clone(),
            lines: 0,
            size: 0,
            chunks: HashMap::new(),
            pending: HashSet::new(),
            error: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            image: None,
            is_image: true,
            find: None,
            highlight: None,
            follow: false,
            unseen: 0,
            stale: None,
            _watch: None,
        };
        let task = cx.background_executor().spawn(async move { crate::image_size::load(host.as_ref(), &path, format) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |v, cx| {
                match result {
                    Ok((image, size)) => {
                        v.size = size;
                        v.image = Some(image);
                    }
                    Err(e) => v.error = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        view
    }

    /// Toggle following the end of the file (`tail -f`): the index is refreshed every second (the agent only scans
    /// the appended part), and new lines scroll to the end.
    pub fn set_follow(&mut self, on: bool, cx: &mut Context<Self>) {
        self.follow = on;
        if on {
            self.unseen = 0;
        }
        if on && !self.is_image {
            self.scroll_to_end();
        }
        cx.notify();
    }

    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// Start the watch loop on open (polls as long as the view lives; `follow` only decides sticking to the end).
    pub fn watch(&mut self, cx: &mut Context<Self>) {
        if self.is_image || self._watch.is_some() {
            return;
        }
        self._watch = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let Ok(Some((host, path))) =
                    this.update(cx, |v, _| (!v.is_image && v.error.is_none() && v.lines > 0).then(|| (v.host.clone(), v.path.clone())))
                else {
                    if this.upgrade().is_none() {
                        break;
                    }
                    continue;
                };
                let result = cx.background_executor().spawn(async move { host.line_index(&path) }).await;
                if this.update(cx, |v, cx| v.apply_index(result, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// New (lines, size): if it grew, drop only the changed tail chunks; if it shrank (rotated), drop all of them.
    fn apply_index(&mut self, result: Result<(u64, u64), host::HostError>, cx: &mut Context<Self>) {
        let (lines, size) = match result {
            Ok(v) => v,
            Err(e) => {
                let msg = unreadable_reason(&e.to_string());
                if self.stale.as_deref() != Some(msg.as_str()) {
                    self.stale = Some(msg);
                    cx.notify();
                }
                return;
            }
        };
        if self.stale.take().is_some() {
            // The file came back: shown chunks may be stale, re-read them.
            self.chunks.clear();
            self.pending.clear();
            cx.notify();
        }
        if (lines, size) == (self.lines, self.size) {
            return;
        }
        if size < self.size || lines < self.lines {
            self.chunks.clear();
            self.pending.clear();
            self.highlight = None;
        } else {
            // The old last line may have been partial: re-read from the chunk that contains it.
            let from = self.lines.saturating_sub(1) / CHUNK;
            self.chunks.retain(|k, _| *k < from);
            self.pending.retain(|k| *k < from);
        }
        let added = lines.saturating_sub(self.lines);
        (self.lines, self.size) = (lines, size);
        if self.follow {
            self.scroll_to_end();
        } else {
            self.unseen += added;
        }
        cx.notify();
    }

    fn scroll_to_end(&self) {
        if self.lines > 0 {
            self.scroll.scroll_to_item_strict((self.lines - 1) as usize, ScrollStrategy::Bottom);
        }
    }

    /// End / ⌘↓: jump to the end and resume following (like `less +F`).
    fn follow_end(&mut self, _: &FollowEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.set_follow(true, cx);
    }

    #[cfg(test)]
    pub fn stale_message(&self) -> Option<String> {
        self.stale.clone()
    }

    #[cfg(test)]
    pub fn unseen_lines(&self) -> u64 {
        self.unseen
    }

    fn toggle_follow(&mut self, _: &ToggleFollow, _: &mut Window, cx: &mut Context<Self>) {
        self.set_follow(!self.follow, cx);
    }

    pub fn is_image(&self) -> bool {
        self.is_image
    }

    #[cfg(test)]
    pub fn image_loaded(&self) -> bool {
        self.image.is_some()
    }

    #[cfg(test)]
    pub fn error_for_test(&self) -> Option<String> {
        self.error.as_ref().map(|e| e.to_string())
    }

    pub fn line_count(&self) -> u64 {
        self.lines
    }

    /// ⌘F: find bar (focus it if open).
    fn open_find(&mut self, _: &FindInLarge, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_image {
            return;
        }
        if let Some(f) = &self.find {
            window.focus(&f.field.focus_handle(cx));
            return;
        }
        let field = cx.new(|cx| TextField::new(crate::i18n::tr("Find in file (searched on the host)"), cx));
        let sub = cx.subscribe_in(&field, window, |this, _, ev: &FieldEvent, window, cx| match ev {
            FieldEvent::Changed => this.schedule_search(cx),
            FieldEvent::Submit => this.step(true, window, cx),
            FieldEvent::Escape => {
                this.close_find(cx);
                window.focus(&this.focus_handle);
            }
        });
        window.focus(&field.focus_handle(cx));
        self.find = Some(LargeFind { field, hits: Vec::new(), current: None, state: FindState::Idle, running: None, debounce: None, _sub: sub });
        cx.notify();
    }

    fn close_find(&mut self, cx: &mut Context<Self>) {
        if let Some((id, _)) = self.find.take().and_then(|f| f.running) {
            self.host.cancel(id);
        }
        self.highlight = None;
        cx.notify();
    }

    fn find_prev(&mut self, _: &LargeFindPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.step(false, window, cx);
    }

    /// Wait a moment while typing: don't start a hundred-MB scan on every keystroke.
    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let Some(f) = &mut self.find else { return };
        f.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
            this.update(cx, |v, cx| v.start_search(cx)).ok();
        }));
    }

    fn start_search(&mut self, cx: &mut Context<Self>) {
        let Some(f) = &mut self.find else { return };
        if let Some((id, _)) = f.running.take() {
            self.host.cancel(id);
        }
        f.hits.clear();
        f.current = None;
        let pattern = f.field.read(cx).text().to_string();
        if pattern.is_empty() {
            f.state = FindState::Idle;
            cx.notify();
            return;
        }
        let query = proto::SearchQuery {
            root: self.path.clone(),
            pattern,
            regex: false,
            case_sensitive: false,
            whole_word: false,
            include: Vec::new(),
            exclude: Vec::new(),
            max_results: MAX_HITS,
        };
        let stream = match self.host.search(query) {
            Ok(s) => s,
            Err(e) => {
                f.state = FindState::Failed(e.to_string().into());
                cx.notify();
                return;
            }
        };
        f.state = FindState::Searching;
        let id = stream.id;
        let task = cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            let mut rx = ui::bridge(stream.rx);
            while let Some(r) = rx.next().await {
                let more = this
                    .update(cx, |v, cx| {
                        let Some(f) = &mut v.find else { return false };
                        let more = match r {
                            Response::SearchChunk(files) => {
                                f.hits.extend(files.into_iter().flat_map(|fm| fm.matches).map(|m| (m.line, m.column)));
                                true
                            }
                            Response::SearchDone { truncated, .. } => {
                                f.state = FindState::Done { truncated };
                                false
                            }
                            Response::Error(e) => {
                                f.state = FindState::Failed(e.to_string().into());
                                false
                            }
                            _ => true,
                        };
                        // Jump there as soon as the first result arrives (don't wait for the end of the file).
                        if f.current.is_none() && !f.hits.is_empty() {
                            f.current = Some(0);
                            let line = f.hits[0].0 as u64;
                            v.show_hit(line);
                        }
                        cx.notify();
                        more
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        });
        if let Some(f) = &mut self.find {
            f.running = Some((id, task));
        }
        cx.notify();
    }

    /// ↵ / ⇧↵: next / previous match (wraps to the start).
    fn step(&mut self, forward: bool, _: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = &mut self.find else { return };
        if f.hits.is_empty() {
            return;
        }
        let n = f.hits.len();
        let next = match (f.current, forward) {
            (None, _) => 0,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        f.current = Some(next);
        let line = f.hits[next].0 as u64;
        self.show_hit(line);
        cx.notify();
    }

    /// Center and highlight the 1-based line.
    fn show_hit(&mut self, line: u64) {
        let ix = line.saturating_sub(1);
        self.highlight = Some(ix);
        self.scroll.scroll_to_item_strict(ix as usize, ScrollStrategy::Center);
    }

    #[cfg(test)]
    pub fn find_summary(&self) -> Option<(usize, Option<usize>, bool)> {
        self.find.as_ref().map(|f| (f.hits.len(), f.current, f.state == FindState::Searching))
    }

    #[cfg(test)]
    pub fn highlighted_line(&self) -> Option<u64> {
        self.highlight
    }

    pub fn go_to_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.scroll.scroll_to_item_strict(line.saturating_sub(1), ScrollStrategy::Center);
        window.focus(&self.focus_handle);
        cx.notify();
    }

    #[cfg(test)]
    pub fn loaded_line(&self, line: u64) -> Option<String> {
        self.chunks.get(&(line / CHUNK)).and_then(|c| c.get((line % CHUNK) as usize)).cloned()
    }

    /// Test: request a line chunk without rendering (in practice uniform_list calls this for the visible range).
    #[cfg(test)]
    pub fn request_line(&mut self, line: u64, cx: &mut Context<Self>) {
        self.ensure(line / CHUNK, cx);
    }

    #[cfg(test)]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Request the line chunk if missing; drop far-away chunks.
    fn ensure(&mut self, chunk: u64, cx: &mut Context<Self>) {
        if self.chunks.contains_key(&chunk) || !self.pending.insert(chunk) {
            return;
        }
        if self.chunks.len() >= MAX_CHUNKS {
            let far = self.chunks.keys().copied().max_by_key(|k| k.abs_diff(chunk));
            if let Some(far) = far {
                self.chunks.remove(&far);
            }
        }
        let (host, path) = (self.host.clone(), self.path.clone());
        let task = cx.background_executor().spawn(async move { host.read_lines(&path, chunk * CHUNK, CHUNK as u32) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |v, cx| {
                v.pending.remove(&chunk);
                match result {
                    Ok(lines) => {
                        v.chunks.insert(chunk, lines);
                    }
                    Err(e) => v.error = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl LargeFileView {
    fn render_image(&self, p: &ui::Palette) -> gpui::AnyElement {
        let kb = self.size as f64 / 1024.;
        let header = match (&self.error, &self.image) {
            (Some(e), _) => e.to_string(),
            (None, None) => crate::i18n::t("Loading…").to_string(),
            (None, Some(_)) if kb < 1024. => crate::i18n::tf("{} KB · image preview", &[&format!("{kb:.0}")]),
            (None, Some(_)) => crate::i18n::tf("{} MB · image preview", &[&format!("{:.1}", kb / 1024.)]),
        };
        div()
            .key_context("LargeFile")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .child(
                div()
                    .h(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .px(px(theme::SPACE[6]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .text_size(px(text::XS.0))
                    .text_color(c(if self.error.is_some() { p.danger } else { p.text_muted }))
                    .child(icon("file-image", 14., p.text_muted))
                    .child(header),
            )
            .child(div().flex_1().min_h_0().p(px(theme::SPACE[8])).flex().items_center().justify_center().children(
                self.image.clone().map(|image| gpui::img(gpui::ImageSource::Image(image)).max_w_full().max_h_full().object_fit(gpui::ObjectFit::Contain)),
            ))
            .into_any_element()
    }
}

impl Render for LargeFileView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        if self.is_image {
            return self.render_image(&p);
        }
        let settings = cx.global::<Settings>();
        let (font_size, line_h) = (settings.editor_font_size, settings.editor_line_height);
        let font: SharedString = settings.editor_font_family.clone().map_or(MONO_FONT.into(), Into::into);
        let digits = self.lines.max(1).to_string().len().max(3) as f32;
        let gutter = digits * font_size * 0.62 + theme::SPACE[8];
        let (follow, unseen) = (self.follow, self.unseen);
        let mb = self.size as f64 / (1024. * 1024.);
        let header = match (&self.error, self.lines) {
            (Some(e), _) => e.to_string(),
            (None, n) if n > 0 && self.stale.is_some() => crate::i18n::tf("{0} lines · {1}", &[&n, &self.stale.clone().unwrap_or_default()]),
            (None, 0) => crate::i18n::t("Indexing lines…").to_string(),
            (None, n) => crate::i18n::tf("{0} lines · {1} MB · read-only", &[&n, &format!("{mb:.1}")]),
        };
        let find_bar = self.find.as_ref().map(|f| {
            let status = match (&f.state, f.hits.len(), f.current) {
                (FindState::Failed(e), _, _) => e.to_string(),
                (FindState::Idle, _, _) => String::new(),
                (FindState::Searching, 0, _) => crate::i18n::t("Searching…").into(),
                (_, 0, _) => crate::i18n::t("No results").into(),
                (state, n, cur) => {
                    let more = if matches!(state, FindState::Searching | FindState::Done { truncated: true }) { "+" } else { "" };
                    crate::i18n::tf("{0} of {1}{2}", &[&cur.map_or(0, |c| c + 1), &n, &more])
                }
            };
            let failed = matches!(f.state, FindState::Failed(_));
            div()
                .key_context("LargeFind")
                .flex_none()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .px(px(theme::SPACE[6]))
                .py(px(theme::SPACE[2]))
                .border_b_1()
                .border_color(c(p.border))
                .text_size(px(text::SM.0))
                .child(ui::input_box("large-goto-box", &p, p.bg, f.field.focus_handle(cx).is_focused(window)).w(px(320.)).child(f.field.clone()))
                .child(div().text_size(px(text::XS.0)).text_color(c(if failed { p.danger } else { p.text_muted })).child(status))
                .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(crate::i18n::t("↵ next · ⇧↵ previous · Esc")))
        });
        div()
            .key_context("LargeFile")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::toggle_follow))
            .on_action(cx.listener(Self::follow_end))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .child(
                div()
                    .h(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .px(px(theme::SPACE[6]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .text_size(px(text::XS.0))
                    .text_color(c(if self.error.is_some() || self.stale.is_some() { p.danger } else { p.text_muted }))
                    .child(icon("file-text", 14., p.text_muted))
                    .child(div().flex_1().min_w_0().overflow_hidden().child(header))
                    .when(!follow && unseen > 0, |d| {
                        d.child(
                            div()
                                .id("large-unseen")
                                .cursor_pointer()
                                .px(px(theme::SPACE[3]))
                                .rounded(px(theme::radius::MD))
                                .bg(ui::ca(p.accent, 0.18))
                                .text_color(c(p.accent))
                                .hover(|d| d.bg(ui::ca(p.accent, 0.28)))
                                .child(crate::i18n::tn("↓ {} new line", "↓ {} new lines", unseen as usize, &[]))
                                .on_click(cx.listener(|this, _, _, cx| this.set_follow(true, cx))),
                        )
                    })
                    .child(
                        ui::option_toggle(
                            "large-follow",
                            crate::i18n::t("Follow"),
                            follow,
                            crate::i18n::t("Follow the end of the file as it grows (tail -f). Scrolling up pauses."),
                            &p,
                            p.bg,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.set_follow(!this.follow, cx))),
                    ),
            )
            .children(find_bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .on_scroll_wheel(cx.listener(|this, ev: &gpui::ScrollWheelEvent, _, cx| {
                        // Scrolling up (content moves down) pauses following, like in `less +F`.
                        if this.follow && ev.delta.pixel_delta(px(1.)).y > px(0.) {
                            this.set_follow(false, cx);
                        }
                    }))
                    .font_family(font)
                    .text_size(px(font_size))
                    .line_height(px(line_h))
                    .child(
                        uniform_list(
                            "large-lines",
                            self.lines as usize,
                            cx.processor(move |this, range: Range<usize>, _, cx| {
                                // Request chunks for the visible range (and the next one).
                                let (a, b) = (range.start as u64 / CHUNK, (range.end as u64 + CHUNK) / CHUNK);
                                for ch in a..=b.min(this.lines / CHUNK) {
                                    this.ensure(ch, cx);
                                }
                                range
                                    .map(|ix| {
                                        let line = ix as u64;
                                        let text = this.chunks.get(&(line / CHUNK)).and_then(|c| c.get((line % CHUNK) as usize)).cloned();
                                        let hit = this.highlight == Some(line);
                                        let color = match text.as_deref().map_or(Level::Plain, line_level) {
                                            Level::Error => ui::ensure_contrast(p.danger, p.text, &[p.bg], 4.5),
                                            Level::Warn => ui::ensure_contrast(p.warning, p.text, &[p.bg], 4.5),
                                            Level::Quiet => p.text_muted,
                                            Level::Plain => p.text,
                                        };
                                        div()
                                            .h(px(line_h))
                                            .flex()
                                            .when(hit, |d| d.bg(ui::ca(p.warning, 0.22)))
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .w(px(gutter))
                                                    .flex_none()
                                                    .flex()
                                                    .justify_end()
                                                    .pr(px(theme::SPACE[6]))
                                                    .text_color(c(p.line_number))
                                                    .child((ix + 1).to_string()),
                                            )
                                            .child(div().text_color(c(color)).children(text.map(|t| t.replace('\t', "    "))))
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(self.scroll.clone())
                        .size_full(),
                    ),
            )
            .into_any_element()
    }
}

/// The user-facing form of a poll failure: what happened and what to expect.
pub fn unreadable_reason(raw: &str) -> String {
    let l = raw.to_ascii_lowercase();
    if l.contains("not found") || l.contains("no such file") || l.contains("notfound") {
        crate::i18n::t("the file is gone (deleted or moved); following continues if it comes back").into()
    } else if l.contains("permission") {
        crate::i18n::t("the file is no longer readable (permission denied)").into()
    } else if l.contains("is a directory") {
        crate::i18n::t("the path is now a folder, not a file").into()
    } else if l.contains("disconnected") || l.contains("connection") || l.contains("broken pipe") {
        crate::i18n::t("the connection to the host is down; following resumes when it is back").into()
    } else {
        crate::i18n::tf("the file cannot be read right now ({})", &[&raw.lines().next().unwrap_or(crate::i18n::t("unknown error"))])
    }
}

/// Growing files like logs: `.log` extension (including rotated `app.log.1`) or under `/var/log`. Following starts on its own when opened.
pub fn looks_like_log(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let rotated = name.rsplit_once(".log.").is_some_and(|(_, rest)| rest.chars().all(|c| c.is_ascii_alphanumeric()));
    name.ends_with(".log") || rotated || path.starts_with("/var/log/")
}

#[cfg(test)]
mod log_tests {
    use super::*;

    #[test]
    fn polling_errors_are_translated_into_what_happened() {
        assert!(unreadable_reason("not found: /x/app.log").contains("deleted or moved"));
        assert!(unreadable_reason("No such file or directory (os error 2)").contains("deleted or moved"));
        assert!(unreadable_reason("Permission denied (os error 13)").contains("permission denied"));
        assert!(unreadable_reason("Is a directory (os error 21)").contains("folder"));
        assert!(unreadable_reason("disconnected: ssh exited").contains("connection to the host is down"));
        assert!(unreadable_reason("weird\nsecond line").contains("(weird)"));
    }

    #[test]
    fn logs_are_recognized_by_name_and_location() {
        for p in ["/srv/app/server.log", "C:/x/Debug.LOG", "/a/app.log.1", "/a/app.log.gz", "/var/log/syslog", "/var/log/nginx/error"] {
            assert!(looks_like_log(p), "{p}");
        }
        for p in ["/a/catalog.txt", "/a/logo.png", "/a/log/readme.md", "/a/blog.rs", "/a/app.logic"] {
            assert!(!looks_like_log(p), "{p}");
        }
    }
}
