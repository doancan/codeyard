//! Markdown preview: splits the document into blocks (`parse`) and renders them as GPUI elements. The preview
//! follows the source's editor and updates as you type.

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontStyle, FontWeight, HighlightStyle, InteractiveText, SharedString, StrikethroughStyle, StyledText,
    UnderlineStyle, Window, div, prelude::*, px,
};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use std::time::{Duration, Instant};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use crate::editor::EditorView;
use crate::ui::{self, Palette, c, palette, text};

#[cfg(test)]
thread_local! { static TEXT_LAYOUTS: std::cell::RefCell<Vec<gpui::TextLayout>> = const { std::cell::RefCell::new(Vec::new()) }; }

#[cfg(test)]
pub(crate) fn text_layouts_for_test() -> Vec<gpui::TextLayout> {
    TEXT_LAYOUTS.with(|layouts| layouts.borrow().clone())
}

/// Inline formatting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
    pub image: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Paragraph(Vec<Span>),
    Code {
        lang: String,
        text: String,
    },
    Quote(Vec<Block>),
    /// `start`: first number in an ordered list. Item: (task checkbox: `Some(checked)`, blocks).
    List {
        start: Option<u64>,
        items: Vec<(Option<bool>, Vec<Block>)>,
    },
    Rule,
    Table {
        head: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
        align: Vec<Alignment>,
    },
}

/// Excessively nested (`> > > …`, deep indentation) document: shown as plain text so that
/// the recursive rendering does not overflow the stack.
fn too_deeply_nested(src: &str) -> bool {
    src.lines().any(|l| nesting_depth(l) > 120)
}

/// Nesting at the start of a line: indentation, `>` and list markers (`- `, `* `, `+ `). A `-----` / `*****` rule without spaces is not nesting
/// (a long separator must not turn the whole document into plain text).
fn nesting_depth(line: &str) -> usize {
    let b = line.as_bytes();
    let (mut i, mut n) = (0, 0);
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'>' => {
                n += 1;
                i += 1;
            }
            b'-' | b'*' | b'+' if b.get(i + 1) == Some(&b' ') => {
                n += 2;
                i += 2;
            }
            _ => break,
        }
    }
    n
}

pub fn parse(src: &str) -> Vec<Block> {
    if too_deeply_nested(src) {
        return src
            .split("\n\n")
            .filter(|p| !p.trim().is_empty())
            .map(|p| Block::Paragraph(vec![Span { text: p.to_string(), style: Style::default() }]))
            .collect();
    }
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut events = Parser::new_ext(src, opts);
    blocks(&mut events, None)
}

/// Contents of a container up to `end` (or the end of the stream).
fn blocks<'a>(events: &mut Parser<'a>, end: Option<TagEnd>) -> Vec<Block> {
    let mut out = Vec::new();
    let mut inline: Vec<Span> = Vec::new();
    let flush = |out: &mut Vec<Block>, inline: &mut Vec<Span>| {
        if !inline.is_empty() {
            out.push(Block::Paragraph(std::mem::take(inline)));
        }
    };
    while let Some(ev) = events.next() {
        match ev {
            Event::End(t) if Some(t) == end => break,
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut out, &mut inline);
                let spans = spans_until(events, TagEnd::Heading(level));
                out.push(Block::Heading(heading_level(level), spans));
            }
            Event::Start(Tag::Paragraph) => {
                flush(&mut out, &mut inline);
                let spans = spans_until(events, TagEnd::Paragraph);
                if !spans.is_empty() {
                    out.push(Block::Paragraph(spans));
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut out, &mut inline);
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                let mut text = String::new();
                for e in events.by_ref() {
                    match e {
                        Event::Text(t) => text.push_str(&t),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                out.push(Block::Code { lang, text: text.trim_end_matches('\n').to_string() });
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut out, &mut inline);
                out.push(Block::Quote(blocks(events, Some(TagEnd::BlockQuote(None)))));
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut out, &mut inline);
                let mut items = Vec::new();
                while let Some(e) = events.next() {
                    match e {
                        Event::End(TagEnd::List(_)) => break,
                        Event::Start(Tag::Item) => {
                            let mut task = None;
                            let mut inner = blocks_item(events, &mut task);
                            if inner.is_empty() {
                                inner.push(Block::Paragraph(Vec::new()));
                            }
                            items.push((task, inner));
                        }
                        _ => {}
                    }
                }
                out.push(Block::List { start, items });
            }
            Event::Start(Tag::Table(align)) => {
                flush(&mut out, &mut inline);
                let (mut head, mut rows) = (Vec::new(), Vec::new());
                let mut in_head = false;
                let mut row: Vec<Vec<Span>> = Vec::new();
                while let Some(e) = events.next() {
                    match e {
                        Event::End(TagEnd::Table) => break,
                        Event::Start(Tag::TableHead) => in_head = true,
                        Event::End(TagEnd::TableHead) => {
                            head = std::mem::take(&mut row);
                            in_head = false;
                        }
                        Event::Start(Tag::TableRow) => row.clear(),
                        Event::End(TagEnd::TableRow) if !in_head => rows.push(std::mem::take(&mut row)),
                        Event::Start(Tag::TableCell) => row.push(spans_until(events, TagEnd::TableCell)),
                        _ => {}
                    }
                }
                out.push(Block::Table { head, rows, align });
            }
            Event::Rule => {
                flush(&mut out, &mut inline);
                out.push(Block::Rule);
            }
            // A tight list item has no paragraph tag: inline events come directly.
            other => push_inline(&mut inline, &mut Style::default(), &mut Vec::new(), other),
        }
    }
    flush(&mut out, &mut inline);
    out
}

/// List item: captures the task checkbox, reads the content up to `End(Item)`.
fn blocks_item<'a>(events: &mut Parser<'a>, task: &mut Option<bool>) -> Vec<Block> {
    let mut out = Vec::new();
    let mut inline: Vec<Span> = Vec::new();
    let (mut style, mut links) = (Style::default(), Vec::new());
    while let Some(ev) = events.next() {
        match ev {
            Event::End(TagEnd::Item) => break,
            Event::TaskListMarker(done) => *task = Some(done),
            Event::Start(Tag::Paragraph) => {
                let spans = spans_until(events, TagEnd::Paragraph);
                inline.extend(spans);
            }
            Event::Start(Tag::List(start)) => {
                if !inline.is_empty() {
                    out.push(Block::Paragraph(std::mem::take(&mut inline)));
                }
                let mut items = Vec::new();
                while let Some(e) = events.next() {
                    match e {
                        Event::End(TagEnd::List(_)) => break,
                        Event::Start(Tag::Item) => {
                            let mut t = None;
                            let inner = blocks_item(events, &mut t);
                            items.push((t, inner));
                        }
                        _ => {}
                    }
                }
                out.push(Block::List { start, items });
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                if !inline.is_empty() {
                    out.push(Block::Paragraph(std::mem::take(&mut inline)));
                }
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                let mut text = String::new();
                for e in events.by_ref() {
                    match e {
                        Event::Text(t) => text.push_str(&t),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                out.push(Block::Code { lang, text: text.trim_end_matches('\n').to_string() });
            }
            other => push_inline(&mut inline, &mut style, &mut links, other),
        }
    }
    if !inline.is_empty() {
        out.insert(0, Block::Paragraph(inline));
    }
    out
}

fn heading_level(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Inline fragments up to the `end` tag.
fn spans_until<'a>(events: &mut Parser<'a>, end: TagEnd) -> Vec<Span> {
    let mut spans = Vec::new();
    let (mut style, mut links) = (Style::default(), Vec::new());
    for ev in events.by_ref() {
        if matches!(&ev, Event::End(t) if *t == end) {
            break;
        }
        push_inline(&mut spans, &mut style, &mut links, ev);
    }
    spans
}

fn push_inline(spans: &mut Vec<Span>, style: &mut Style, links: &mut Vec<String>, ev: Event) {
    let add = |spans: &mut Vec<Span>, text: &str, style: &Style| {
        if text.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some(last) if last.style == *style => last.text.push_str(text),
            _ => spans.push(Span { text: text.to_string(), style: style.clone() }),
        }
    };
    match ev {
        Event::Text(t) | Event::Code(t) if spans.last().is_some_and(|s| s.style.image.is_some()) => spans.last_mut().unwrap().text.push_str(&t),
        Event::End(TagEnd::Image) => spans.push(Span { text: String::new(), style: style.clone() }),
        Event::Text(t) => add(spans, &t, style),
        Event::Code(t) => add(spans, &t, &Style { code: true, ..style.clone() }),
        Event::SoftBreak => add(spans, " ", style),
        Event::HardBreak => add(spans, "\n", style),
        Event::Html(t) | Event::InlineHtml(t) => add(spans, &t, style),
        Event::Start(Tag::Strong) => style.bold = true,
        Event::End(TagEnd::Strong) => style.bold = false,
        Event::Start(Tag::Emphasis) => style.italic = true,
        Event::End(TagEnd::Emphasis) => style.italic = false,
        Event::Start(Tag::Strikethrough) => style.strike = true,
        Event::End(TagEnd::Strikethrough) => style.strike = false,
        Event::Start(Tag::Link { dest_url, .. }) => {
            links.push(dest_url.to_string());
            style.link = links.last().cloned();
        }
        Event::End(TagEnd::Link) => {
            links.pop();
            style.link = links.last().cloned();
        }
        Event::Start(Tag::Image { dest_url, .. }) => {
            spans.push(Span { text: String::new(), style: Style { image: Some(dest_url.to_string()), ..style.clone() } });
        }
        _ => {}
    }
}

/// Converts the fragments into a single text and emphasis ranges.
fn flatten(
    spans: &[Span],
    p: &Palette,
    base: Option<FontWeight>,
) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>, Vec<(std::ops::Range<usize>, String)>) {
    let mut text = String::new();
    let (mut hl, mut links) = (Vec::new(), Vec::new());
    for s in spans {
        let start = text.len();
        text.push_str(&s.text);
        let range = start..text.len();
        let mut st = HighlightStyle::default();
        let mut styled = false;
        if s.style.bold || base.is_some() {
            st.font_weight = Some(base.unwrap_or(FontWeight::BOLD));
            styled = true;
        }
        if s.style.italic {
            st.font_style = Some(FontStyle::Italic);
            styled = true;
        }
        if s.style.code {
            st.background_color = Some(ui::ca(p.text_muted, 0.18).into());
            st.color = Some(c(p.syntax_string).into());
            styled = true;
        }
        if s.style.strike {
            st.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None });
            styled = true;
        }
        if let Some(url) = &s.style.link {
            st.color = Some(c(p.accent).into());
            st.underline = Some(UnderlineStyle { thickness: px(1.), color: Some(c(p.accent).into()), wavy: false });
            links.push((range.clone(), url.clone()));
            styled = true;
        }
        if styled && !range.is_empty() {
            hl.push((range, st));
        }
    }
    (text, hl, links)
}

#[derive(Clone)]
pub enum PreviewEvent {
    Code,
    Open { host: Arc<dyn host::Host>, path: String },
}

impl gpui::EventEmitter<PreviewEvent> for MarkdownPreview {}

#[derive(Default)]
struct PreviewResources {
    images: HashMap<String, Result<Arc<gpui::Image>, String>>,
    pending: HashMap<String, u64>,
    next_request: u64,
}

impl PreviewResources {
    fn retain(&mut self, urls: &HashSet<String>) {
        self.images.retain(|url, _| urls.contains(url));
        self.pending.retain(|url, _| urls.contains(url));
    }

    fn begin(&mut self, url: String) -> u64 {
        self.next_request += 1;
        let request = self.next_request;
        self.pending.insert(url.clone(), request);
        self.images.insert(url, Err(crate::i18n::t("Loading image…").into()));
        request
    }

    fn complete(&mut self, url: String, request: u64, result: Result<Arc<gpui::Image>, String>) {
        // Removing and re-adding a URL starts a different read. An earlier read must not overwrite it.
        if self.pending.get(&url) == Some(&request) {
            self.pending.remove(&url);
            self.images.insert(url, result);
        }
    }
}

type LinkHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;

struct DocumentContext {
    link: LinkHandler,
    images: HashMap<String, Result<Arc<gpui::Image>, String>>,
    anchors: HashMap<String, gpui::ScrollAnchor>,
    code: HashMap<String, Vec<(std::ops::Range<usize>, syntax::Role)>>,
}

pub struct MarkdownPreview {
    code: HashMap<String, Vec<(std::ops::Range<usize>, syntax::Role)>>,
    resources: PreviewResources,
    scroll: gpui::ScrollHandle,
    anchors: HashMap<String, gpui::ScrollAnchor>,
    source: Entity<EditorView>,
    blocks: Vec<Block>,
    seen_version: u64,
    focus_handle: FocusHandle,
    _sub: gpui::Subscription,
    /// Duration and time of the last parse: to avoid re-parsing from scratch on every keystroke in a large document (see `should_parse`).
    last_cost: Duration,
    last_parse: Option<Instant>,
    catchup_scheduled: bool,
}

/// A document that parses slowly (≥ 20 ms) is not re-parsed until 250 ms after the last parse (an update is scheduled in between);
/// small documents are parsed immediately on every change.
fn should_parse(last_cost: Duration, since_last: Option<Duration>) -> bool {
    last_cost < Duration::from_millis(20) || since_last.is_none_or(|d| d >= Duration::from_millis(250))
}

impl Focusable for MarkdownPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl MarkdownPreview {
    pub fn new(source: Entity<EditorView>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&source, |_, _, cx| cx.notify());
        let mut p = MarkdownPreview {
            code: HashMap::new(),
            resources: PreviewResources::default(),
            scroll: gpui::ScrollHandle::new(),
            anchors: HashMap::new(),
            source,
            blocks: Vec::new(),
            seen_version: u64::MAX,
            focus_handle: cx.focus_handle(),
            _sub: sub,
            last_cost: Duration::ZERO,
            last_parse: None,
            catchup_scheduled: false,
        };
        p.refresh(cx);
        p
    }

    pub fn source(&self) -> Entity<EditorView> {
        self.source.clone()
    }

    pub fn title(&self, cx: &App) -> String {
        crate::i18n::tf("Preview {}", &[&self.source.read(cx).name])
    }

    /// Re-parse if the source changed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let v = self.source.read(cx).version();
        if v == self.seen_version {
            return;
        }
        if !should_parse(self.last_cost, self.last_parse.map(|t| t.elapsed())) {
            // Slow document, a very recent parse exists: redraw shortly with the final state.
            if !self.catchup_scheduled {
                self.catchup_scheduled = true;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(250)).await;
                    this.update(cx, |this, cx| {
                        this.catchup_scheduled = false;
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            return;
        }
        self.seen_version = v;
        let started = Instant::now();
        self.blocks = parse(&self.source.read(cx).text_lines().join("\n"));
        self.code.clear();
        highlight_blocks(&self.blocks, "md", &mut self.code);
        self.load_images(cx);
        self.last_cost = started.elapsed();
        self.last_parse = Some(Instant::now());
    }

    fn load_images(&mut self, cx: &mut Context<Self>) {
        let source = self.source.read(cx);
        let path = source.path.replace('\\', "/");
        let host = source.host_arc();
        let mut urls = Vec::new();
        collect_images(&self.blocks, &mut urls);
        let urls: HashSet<_> = urls.into_iter().collect();
        self.resources.retain(&urls);
        for url in urls {
            if self.resources.images.contains_key(&url) {
                continue;
            }
            let Some(path) = relative_path(&path, &url) else {
                self.resources.images.insert(url, Err(crate::i18n::t("External image: open the link to view it.").into()));
                continue;
            };
            let Some(format) = crate::large::image_format(&path) else {
                continue;
            };
            let request = self.resources.begin(url.clone());
            let host = host.clone();
            let task = cx.background_executor().spawn(async move { crate::image_size::load(host.as_ref(), &path, format).map(|(image, _)| image) });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    this.resources.complete(url, request, result);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    #[cfg(test)]
    pub fn scroll_for_test(&self) -> gpui::ScrollHandle {
        self.scroll.clone()
    }

    #[cfg(test)]
    pub fn blocks_for_test(&self, cx: &App) -> Vec<Block> {
        parse(&self.source.read(cx).text_lines().join("\n"))
    }

    #[cfg(test)]
    pub fn image_results_for_test(&self) -> HashMap<String, Result<(), String>> {
        self.resources.images.iter().map(|(url, image)| (url.clone(), image.as_ref().map(|_| ()).map_err(Clone::clone))).collect()
    }
}

/// In the preview only web and email links are opened by the system: a README must not trigger system apps with `file://` (local file/app), `x-apple-*`,
/// `ssh://` or other registered schemes.
pub(crate) fn is_safe_link(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:"].iter().any(|s| lower.starts_with(s))
}

/// Renders a paragraph (with clickable links).
fn text_view(id: impl Into<gpui::ElementId>, spans: &[Span], p: &Palette, base: Option<FontWeight>, document: Option<&DocumentContext>) -> gpui::AnyElement {
    let (t, hl, links) = flatten(spans, p, base);
    let styled = StyledText::new(SharedString::from(t)).with_highlights(hl);
    #[cfg(test)]
    TEXT_LAYOUTS.with(|layouts| layouts.borrow_mut().push(styled.layout().clone()));
    if links.is_empty() {
        return styled.into_any_element();
    }
    let handler = document.map(|d| d.link.clone());
    let ranges: Vec<_> = links.iter().map(|(r, _)| r.clone()).collect();
    let urls: Vec<String> = links.into_iter().map(|(_, u)| u).collect();
    InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, window, cx| {
            if let Some(u) = urls.get(ix) {
                if let Some(handler) = &handler {
                    handler(u, window, cx);
                } else if is_safe_link(u) {
                    cx.open_url(u);
                }
            }
        })
        .into_any_element()
}

/// A parsed document as elements, for places that show Markdown outside the preview tab (the update dialog's release notes).
/// Links open through the same safe-scheme check as in the preview.
pub(crate) fn document_view(blocks: &[Block], id: &str, p: &Palette) -> Vec<gpui::AnyElement> {
    blocks.iter().enumerate().map(|(i, b)| block_view(b, &format!("{id}{i}"), p, 0, false, None)).collect()
}

/// The same, set small and tight: headings only a little larger than the text, no rules under them, list items one line apart.
/// For a document that is read in a small window (the release notes).
pub(crate) fn compact_document_view(blocks: &[Block], id: &str, p: &Palette) -> Vec<gpui::AnyElement> {
    blocks.iter().enumerate().map(|(i, b)| block_view(b, &format!("{id}{i}"), p, 0, true, None)).collect()
}

fn block_view(b: &Block, id: &str, p: &Palette, depth: usize, compact: bool, document: Option<&DocumentContext>) -> gpui::AnyElement {
    match b {
        Block::Heading(level, spans) => {
            let size = match (compact, level) {
                (true, _) => 13.,
                (false, 1) => 28.,
                (false, 2) => 22.,
                (false, 3) => 18.,
                (false, 4) => 16.,
                (false, _) => 14.,
            };
            div()
                .id(SharedString::from(id.to_string()))
                .anchor_scroll(document.and_then(|d| d.anchors.get(id)).cloned())
                .pt(px(match (compact, *level <= 2) {
                    (true, _) => 6.,
                    (false, true) => 12.,
                    (false, false) => 6.,
                }))
                .pb(px(if compact { 2. } else { 4. }))
                .text_size(px(size))
                .when(*level <= 2 && !compact, |d| d.border_b_1().border_color(c(p.border)))
                .child(text_view(SharedString::from(format!("{id}h")), spans, p, Some(FontWeight::BOLD), document))
                .into_any_element()
        }
        Block::Paragraph(spans) => {
            let (size, line) = if compact { (text::SM.0, 18.) } else { (text::BASE.0, 22.) };
            let mut children = Vec::new();
            let mut start = 0;
            for (index, span) in spans.iter().enumerate() {
                let Some(url) = &span.style.image else {
                    continue;
                };
                if start < index {
                    children.push(text_view(SharedString::from(format!("{id}p{index}")), &spans[start..index], p, None, document));
                }
                let image = document.and_then(|d| d.images.get(url));
                children.push(match image {
                    Some(Ok(image)) => div()
                        .w_full()
                        .child(
                            gpui::img(gpui::ImageSource::Image(image.clone()))
                                .with_fallback({
                                    let alt = span.text.clone();
                                    move || div().child(alt.clone()).into_any_element()
                                })
                                .max_w_full()
                                .max_h(px(600.))
                                .object_fit(gpui::ObjectFit::Contain),
                        )
                        .into_any_element(),
                    _ => div()
                        .id(SharedString::from(format!("{id}-image-{index}")))
                        .when(is_safe_link(url), |d| {
                            let url = url.clone();
                            d.cursor_pointer().on_click(move |_, _, cx| cx.open_url(&url))
                        })
                        .text_color(c(p.text_muted))
                        .child(format!(
                            "{} — {}",
                            if span.text.is_empty() { url } else { &span.text },
                            image.and_then(|i| i.as_ref().err()).map(String::as_str).unwrap_or(crate::i18n::t("Image unavailable"))
                        ))
                        .into_any_element(),
                });
                start = index + 1;
            }
            if start < spans.len() {
                children.push(text_view(SharedString::from(format!("{id}p")), &spans[start..], p, None, document));
            }
            div().w_full().min_w_0().text_size(px(size)).line_height(px(line)).children(children).into_any_element()
        }
        Block::Code { text: t, lang } => {
            let copy = t.clone();
            let highlights = document
                .and_then(|d| d.code.get(id))
                .map(|spans| {
                    spans
                        .iter()
                        .map(|(range, role)| {
                            let color = match role {
                                syntax::Role::Keyword => p.syntax_keyword,
                                syntax::Role::String => p.syntax_string,
                                syntax::Role::Number => p.syntax_number,
                                syntax::Role::Function => p.syntax_function,
                                syntax::Role::Type => p.syntax_type,
                                syntax::Role::Comment => p.syntax_comment,
                                syntax::Role::Tag => p.syntax_tag,
                                syntax::Role::Punct => p.syntax_punct,
                            };
                            (range.clone(), HighlightStyle { color: Some(c(color).into()), ..Default::default() })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .rounded(px(theme::radius::SM))
                .bg(c(p.surface))
                .border_1()
                .border_color(c(p.border))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(theme::SPACE[4]))
                        .py(px(theme::SPACE[2]))
                        .child(div().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(lang.clone()))
                        .child(
                            div()
                                .id(SharedString::from(format!("{id}-copy")))
                                .cursor_pointer()
                                .px(px(8.))
                                .py(px(4.))
                                .rounded(px(theme::radius::SM))
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .text_size(px(text::XS.0))
                                .child(crate::i18n::t("Copy code"))
                                .on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-code")))
                        .flex()
                        .overflow_x_scroll()
                        .px(px(theme::SPACE[5]))
                        .py(px(theme::SPACE[3]))
                        .font_family(ui::MONO_FONT)
                        .text_size(px(text::SM.0))
                        .line_height(px(text::SM.1))
                        .child(div().min_w_full().flex_none().whitespace_nowrap().child(StyledText::new(t.clone()).with_highlights(highlights))),
                )
                .into_any_element()
        }
        Block::Quote(inner) => div()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[3]))
            .pl(px(theme::SPACE[6]))
            .border_l_2()
            .border_color(c(p.border_input))
            .text_color(c(p.text_muted))
            .children(inner.iter().enumerate().map(|(i, b)| block_view(b, &format!("{id}q{i}"), p, depth, compact, document)))
            .into_any_element(),
        Block::List { start, items } => div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(if compact { 0. } else { theme::SPACE[2] }))
            .children(items.iter().enumerate().map(|(i, (task, inner))| {
                let marker = match (task, start) {
                    (Some(true), _) => "☑".to_string(),
                    (Some(false), _) => "☐".to_string(),
                    (None, Some(n)) => format!("{}.", n + i as u64),
                    (None, None) => if depth.is_multiple_of(2) { "•" } else { "◦" }.to_string(),
                };
                div()
                    .debug_selector(|| format!("{id}-row-{i}"))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .gap(px(theme::SPACE[4]))
                    .child(div().flex_none().w(px(if compact { 14. } else { 22. })).text_color(c(p.text_muted)).child(marker))
                    .child(
                        div()
                            .debug_selector(|| format!("{id}-item-{i}"))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(if compact { 0. } else { theme::SPACE[2] }))
                            .children(inner.iter().enumerate().map(|(k, b)| block_view(b, &format!("{id}l{i}-{k}"), p, depth + 1, compact, document))),
                    )
            }))
            .into_any_element(),
        Block::Rule => div().my(px(theme::SPACE[4])).h(px(1.)).w_full().bg(c(p.border)).into_any_element(),
        Block::Table { head, rows, align } => {
            let cell = |spans: &[Span], col: usize, header: bool, key: String| {
                let a = align.get(col).copied().unwrap_or(Alignment::None);
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(theme::SPACE[4]))
                    .py(px(theme::SPACE[2]))
                    .border_r_1()
                    .border_color(c(p.border))
                    .when(a == Alignment::Center, |d| d.flex().justify_center())
                    .when(a == Alignment::Right, |d| d.flex().justify_end())
                    .child(text_view(SharedString::from(key), spans, p, header.then_some(FontWeight::BOLD), document))
            };
            let row = |cells: &Vec<Vec<Span>>, header: bool, r: usize| {
                div()
                    .flex()
                    .border_b_1()
                    .border_color(c(p.border))
                    .when(header, |d| d.bg(c(p.surface)))
                    .children(cells.iter().enumerate().map(|(ci, s)| cell(s, ci, header, format!("{id}t{r}-{ci}"))))
            };
            div()
                .w_full()
                .flex()
                .flex_col()
                .border_t_1()
                .border_l_1()
                .border_color(c(p.border))
                .child(row(head, true, 0))
                .children(rows.iter().enumerate().map(|(ri, r)| row(r, false, ri + 1)))
                .into_any_element()
        }
    }
}

/// The text of `blocks` without formatting, one line per paragraph, heading, list item or table row: what a screen reader
/// is given for a document that is drawn as many separate elements.
pub(crate) fn plain_text(blocks: &[Block]) -> String {
    fn spans(s: &[Span]) -> String {
        s.iter().map(|s| s.text.as_str()).collect()
    }
    fn walk(blocks: &[Block], out: &mut Vec<String>) {
        for b in blocks {
            match b {
                Block::Heading(_, s) | Block::Paragraph(s) => out.push(spans(s)),
                Block::Code { text, .. } => out.extend(text.lines().map(str::to_string)),
                Block::Quote(inner) => walk(inner, out),
                Block::List { items, .. } => {
                    for (_, inner) in items {
                        let mut item = Vec::new();
                        walk(inner, &mut item);
                        out.push(format!("- {}", item.join(" ")));
                    }
                }
                Block::Rule => {}
                Block::Table { head, rows, .. } => {
                    for row in std::iter::once(head).chain(rows) {
                        out.push(row.iter().map(|c| spans(c)).collect::<Vec<_>>().join("\t"));
                    }
                }
            }
        }
    }
    let mut lines = Vec::new();
    walk(blocks, &mut lines);
    lines.join("\n")
}

impl Render for MarkdownPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh(cx);
        #[cfg(test)]
        TEXT_LAYOUTS.with(|layouts| layouts.borrow_mut().clear());
        let p = *palette(cx);
        let source = self.source.read(cx);
        let name = source.name.to_string();
        let path = source.path.replace('\\', "/");
        let host = source.host_arc();
        let entity = cx.entity();
        let mut headings = Vec::new();
        collect_headings(&self.blocks, "md", &mut headings);
        self.anchors.clear();
        let mut slugs = HashMap::new();
        for (id, title) in headings {
            let slug = unique_heading_slug(&title, &mut slugs);
            let anchor = self.anchors.entry(id.clone()).or_insert_with(|| gpui::ScrollAnchor::for_handle(self.scroll.clone())).clone();
            self.anchors.insert(format!("#{slug}"), anchor);
        }
        let anchors = self.anchors.clone();
        let document = DocumentContext {
            link: Rc::new(move |url, window, cx| {
                if is_safe_link(url) {
                    cx.open_url(url);
                } else if let Some(anchor) = decode_url_path(url).and_then(|key| anchors.get(&key)) {
                    anchor.scroll_to(window, cx);
                } else if let Some(target) = relative_path(&path, url) {
                    entity.update(cx, |_, cx| cx.emit(PreviewEvent::Open { host: host.clone(), path: target }));
                }
            }),
            code: self.code.clone(),
            images: self.resources.images.clone(),
            anchors: self.anchors.clone(),
        };
        div()
            .id("markdown-preview")
            .key_context("MarkdownPreview")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(PreviewEvent::Code);
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .bg(c(p.bg))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(theme::SPACE[5]))
                    .py(px(theme::SPACE[3]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(crate::i18n::tf("Preview of {}", &[&name])))
                    .child(crate::editor::button("markdown-code", crate::i18n::t("Code"), false, &p, {
                        let entity = cx.entity();
                        move |_, _, cx| entity.update(cx, |_, cx| cx.emit(PreviewEvent::Code))
                    })),
            )
            .child(
                div().id("markdown-scroll").track_scroll(&self.scroll).flex_1().min_h_0().overflow_y_scroll().flex().flex_col().items_center().child(
                    div()
                        .flex_none()
                        .w_full()
                        .min_w_0()
                        .max_w(px(820.))
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE[4]))
                        .px(px(theme::SPACE[9]))
                        .py(px(theme::SPACE[8]))
                        .children(self.blocks.iter().enumerate().map(|(i, b)| block_view(b, &format!("md{i}"), &p, 0, false, Some(&document))))
                        .when(self.blocks.is_empty(), |d| d.child(div().text_color(c(p.text_muted)).child(crate::i18n::t("The file is empty.")))),
                ),
            )
    }
}

fn heading_slug(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || c.is_whitespace())
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect()
}

fn unique_heading_slug(title: &str, slugs: &mut HashMap<String, usize>) -> String {
    let base = heading_slug(title);
    let mut count = slugs.get(&base).copied().unwrap_or(0);
    let mut slug = if count == 0 { base.clone() } else { format!("{base}-{count}") };
    while slugs.contains_key(&slug) {
        count += 1;
        slug = format!("{base}-{count}");
    }
    slugs.insert(base, count + 1);
    slugs.entry(slug.clone()).or_insert(1);
    slug
}

/// Resolve on the document's Host, never through the system URL handler.
pub(crate) fn relative_path(source: &str, target: &str) -> Option<String> {
    let target = target.trim();
    if target.is_empty() || target.starts_with(['#', '/', '\\']) || target.contains(':') || target.contains('\\') {
        return None;
    }
    let target = decode_url_path(target.split(['#', '?']).next()?)?;
    let target = target.as_str();
    if target.contains([':', '\\', '\0']) || target.starts_with('/') {
        return None;
    }
    if target.is_empty() {
        return None;
    }
    let parent = source.rsplit_once('/').map(|(p, _)| p).unwrap_or(".");
    let mut parts: Vec<&str> = parent.split('/').collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| !p.is_empty() && *p != ".." && !p.ends_with(':')) {
                    parts.pop();
                } else if !source.starts_with('/') && !source.as_bytes().get(1).is_some_and(|b| *b == b':') {
                    parts.push("..");
                }
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn decode_url_path(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let a = (input.next()? as char).to_digit(16)?;
            let b = (input.next()? as char).to_digit(16)?;
            bytes.push((a * 16 + b) as u8);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

fn highlight_blocks(blocks: &[Block], prefix: &str, out: &mut HashMap<String, Vec<(std::ops::Range<usize>, syntax::Role)>>) {
    for (i, block) in blocks.iter().enumerate() {
        let id = format!("{prefix}{i}");
        match block {
            Block::Code { lang, text } if text.len() <= 100_000 => {
                let ext = match lang.as_str() {
                    "rust" => "rs",
                    "javascript" => "js",
                    "typescript" => "ts",
                    "python" => "py",
                    "shell" | "bash" => "sh",
                    "c++" => "cpp",
                    other => other,
                };
                if let Some(language) = syntax::language_for_path(&format!("code.{ext}")) {
                    let rope = buffer::Rope::from_str(text);
                    if let Some(syntax) = syntax::Syntax::new(language, &rope) {
                        let spans = syntax.highlights(&rope, 0..text.len());
                        let mut offset = 0;
                        let highlights = syntax::line_runs(0, text.len(), &spans)
                            .into_iter()
                            .filter_map(|(len, role)| {
                                let range = offset..offset + len;
                                offset += len;
                                role.map(|role| (range, role))
                            })
                            .collect();
                        out.insert(id, highlights);
                    }
                }
            }
            Block::Quote(inner) => highlight_blocks(inner, &format!("{id}q"), out),
            Block::List { items, .. } => {
                for (j, (_, inner)) in items.iter().enumerate() {
                    highlight_blocks(inner, &format!("{id}l{j}-"), out);
                }
            }
            _ => {}
        }
    }
}

fn collect_images(blocks: &[Block], urls: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Heading(_, spans) | Block::Paragraph(spans) => urls.extend(spans.iter().filter_map(|s| s.style.image.clone())),
            Block::Quote(inner) => collect_images(inner, urls),
            Block::List { items, .. } => {
                for (_, inner) in items {
                    collect_images(inner, urls);
                }
            }
            _ => {}
        }
    }
}

fn collect_headings(blocks: &[Block], prefix: &str, out: &mut Vec<(String, String)>) {
    for (i, block) in blocks.iter().enumerate() {
        let id = format!("{prefix}{i}");
        match block {
            Block::Heading(_, spans) => out.push((id, spans.iter().map(|s| s.text.as_str()).collect())),
            Block::Quote(inner) => collect_headings(inner, &format!("{id}q"), out),
            Block::List { items, .. } => {
                for (j, (_, inner)) in items.iter().enumerate() {
                    collect_headings(inner, &format!("{id}l{j}-"), out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsolete_image_reads_cannot_replace_a_new_request_for_the_same_url() {
        let mut resources = PreviewResources::default();
        let url = "image.png".to_string();
        let old = resources.begin(url.clone());
        resources.retain(&HashSet::new());
        let new = resources.begin(url.clone());
        resources.complete(url.clone(), old, Err("obsolete read".into()));
        assert_eq!(resources.pending.get(&url), Some(&new), "new read is still pending");
        resources.complete(url.clone(), new, Err("current read".into()));
        assert_eq!(resources.images[&url].as_ref().unwrap_err(), "current read");
        resources.complete(url.clone(), old, Err("late obsolete read".into()));
        assert_eq!(resources.images[&url].as_ref().unwrap_err(), "current read");
        resources.retain(&HashSet::new());
        resources.complete(url, new, Err("removed image".into()));
        assert!(resources.images.is_empty(), "removed resources stay removed");
    }

    #[test]
    fn heading_links_do_not_collide_with_numbered_titles() {
        let mut slugs = HashMap::new();
        let titles = ["Setup", "Setup-1", "Setup", "Setup", "Setup-1"];
        let actual: Vec<_> = titles.iter().map(|title| unique_heading_slug(title, &mut slugs)).collect();
        assert_eq!(actual, ["setup", "setup-1", "setup-2", "setup-3", "setup-1-1"]);
    }

    #[test]
    fn document_links_resolve_without_launching_protocol_handlers() {
        assert_eq!(relative_path("/repo/docs/start.md", "../README.md#intro").as_deref(), Some("/repo/README.md"));
        assert_eq!(relative_path("/repo/start.md", "images/a%20b.png").as_deref(), Some("/repo/images/a b.png"));
        assert_eq!(relative_path("C:/repo/docs/start.md", "../README.md").as_deref(), Some("C:/repo/README.md"));
        for target in ["#intro", "https://example.com", "javascript:alert(1)", "//example.com/a", "%2fetc/passwd", "a%00b", "a%3ab", "bad%zz"] {
            assert!(relative_path("/repo/start.md", target).is_none(), "{target}");
        }
    }

    #[test]
    fn image_alt_text_is_separate_from_surrounding_text() {
        let blocks = parse("Before ![a **small** cat](images/cat.png) after.");
        let Block::Paragraph(spans) = &blocks[0] else { panic!() };
        let image = spans.iter().find(|s| s.style.image.is_some()).unwrap();
        assert_eq!(image.text, "a small cat");
        assert_eq!(image.style.image.as_deref(), Some("images/cat.png"));
        assert!(spans.iter().any(|s| s.style.image.is_none() && s.text.contains("after.")));
    }

    #[test]
    fn syntax_ranges_are_ordered_non_overlapping_and_utf8_safe() {
        let blocks = parse("```rust\nfn main() { let name = \"猫\"; }\n```");
        let mut code = HashMap::new();
        highlight_blocks(&blocks, "md", &mut code);
        let spans = &code["md0"];
        assert!(!spans.is_empty());
        let Block::Code { text, .. } = &blocks[0] else { panic!() };
        let mut end = 0;
        for (range, _) in spans {
            assert!(range.start >= end);
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            end = range.end;
        }
        assert_eq!(heading_slug("Hello, Dünya!"), "hello-dünya");
    }

    #[test]
    fn slow_documents_are_not_reparsed_on_every_keystroke() {
        let fast = Duration::from_millis(2);
        let slow = Duration::from_millis(80);
        assert!(should_parse(fast, Some(Duration::ZERO)), "small documents update at once");
        assert!(should_parse(slow, None), "the first parse always runs");
        assert!(!should_parse(slow, Some(Duration::from_millis(100))), "a slow document waits");
        assert!(should_parse(slow, Some(Duration::from_millis(250))), "then catches up");
    }

    #[test]
    fn long_rules_do_not_turn_the_preview_into_plain_text_but_real_nesting_does() {
        let rule = format!("# Title\n\n{}\n\n{}\n\ntext\n", "-".repeat(200), "*".repeat(200));
        assert!(!too_deeply_nested(&rule), "a long ruler is not nesting");
        assert!(matches!(parse(&rule)[0], Block::Heading(1, _)), "the document is still rendered as Markdown");
        assert!(too_deeply_nested(&format!("{} deep", ">".repeat(130))));
        assert!(too_deeply_nested(&format!("{}x", "- ".repeat(70))));
        assert!(too_deeply_nested(&format!("{}x", " ".repeat(130))));
        assert!(!too_deeply_nested("> quote\n  - item\n    - nested\n"));
    }

    #[test]
    fn only_web_and_mail_links_are_opened_from_the_preview() {
        for ok in ["https://example.com", "HTTP://example.com/a?b=c", "mailto:a@b.c", " https://x"] {
            assert!(is_safe_link(ok), "{ok}");
        }
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "ssh://host", "x-apple.systempreferences:", "vscode://x", "./README.md", "", "//evil.com"] {
            assert!(!is_safe_link(bad), "{bad}");
        }
    }

    fn plain(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn headings_paragraphs_and_inline_styles() {
        let b = parse("# Title\n\nHello **bold** and *it* and `code` and [link](http://x.io) ~~gone~~.\n");
        assert_eq!(b.len(), 2);
        assert!(matches!(&b[0], Block::Heading(1, s) if plain(s) == "Title"));
        let Block::Paragraph(s) = &b[1] else { panic!("{b:?}") };
        assert_eq!(plain(s), "Hello bold and it and code and link gone.");
        assert!(s.iter().any(|x| x.text == "bold" && x.style.bold));
        assert!(s.iter().any(|x| x.text == "it" && x.style.italic));
        assert!(s.iter().any(|x| x.text == "code" && x.style.code));
        assert!(s.iter().any(|x| x.text == "link" && x.style.link.as_deref() == Some("http://x.io")));
        assert!(s.iter().any(|x| x.text == "gone" && x.style.strike));
    }

    #[test]
    fn code_quote_rule_and_lists() {
        let b = parse("```rust\nfn main() {}\n```\n\n> quoted\n\n---\n\n1. one\n2. two\n\n- [x] done\n- [ ] todo\n  - nested\n");
        assert!(matches!(&b[0], Block::Code { lang, text } if lang == "rust" && text == "fn main() {}"));
        assert!(matches!(&b[1], Block::Quote(inner) if matches!(&inner[0], Block::Paragraph(s) if plain(s) == "quoted")));
        assert!(matches!(&b[2], Block::Rule));
        let Block::List { start, items } = &b[3] else { panic!("{b:?}") };
        assert_eq!((*start, items.len()), (Some(1), 2));
        let Block::List { start, items } = &b[4] else { panic!("{b:?}") };
        assert_eq!((*start, items.len()), (None, 2));
        assert_eq!((items[0].0, items[1].0), (Some(true), Some(false)));
        assert!(items[1].1.iter().any(|b| matches!(b, Block::List { items, .. } if items.len() == 1)), "nested list: {:?}", items[1]);
    }

    #[test]
    fn tables_keep_alignment() {
        let b = parse("| a | b |\n|:-:|--:|\n| 1 | 2 |\n| 3 | 4 |\n");
        let Block::Table { head, rows, align } = &b[0] else { panic!("{b:?}") };
        assert_eq!((head.len(), rows.len()), (2, 2));
        assert_eq!(plain(&rows[1][1]), "4");
        assert_eq!(align, &[Alignment::Center, Alignment::Right]);
    }

    #[test]
    fn empty_and_tight_lists() {
        assert!(parse("").is_empty());
        let b = parse("- a\n- b\n");
        let Block::List { items, .. } = &b[0] else { panic!() };
        assert!(matches!(&items[0].1[0], Block::Paragraph(s) if plain(s) == "a"));
    }
}

#[cfg(test)]
mod hardening {
    use super::*;

    /// Small, reproducible random generator.
    fn rng(seed: u64) -> impl FnMut() -> u64 {
        let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
        move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        }
    }

    #[test]
    fn pathological_nesting_does_not_overflow_the_stack() {
        let deep = format!("{}text\n", "> ".repeat(5000));
        let b = parse(&deep);
        assert!(!b.is_empty());
        let deep_list = (0..3000).map(|i| format!("{}- item\n", "  ".repeat(i % 100))).collect::<String>();
        let _ = parse(&deep_list);
        let spaces = format!("{}x", " ".repeat(100_000));
        let _ = parse(&spaces);
    }

    #[test]
    fn random_markdown_never_panics() {
        let alphabet: Vec<&str> = vec![
            "#", "# ", "\n", "\n\n", "> ", "- ", "1. ", "* ", "`", "```", "**", "*", "_", "[", "](", ")", "![", "|", "---", "~~", "<b>", "&amp;", "\\", " ",
            "\t", "ç", "🙂", "[x] ", "[ ] ",
        ];
        for seed in 1..400u64 {
            let mut r = rng(seed);
            let n = 1 + (r() % 80) as usize;
            let src: String = (0..n).map(|_| alphabet[(r() % alphabet.len() as u64) as usize]).collect();
            let blocks = parse(&src);
            // Rendering helpers must not crash either.
            for b in &blocks {
                let _ = format!("{b:?}");
            }
        }
    }
}
