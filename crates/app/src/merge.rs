//! Merge editor: shows the conflicts of the open file card by card (Current | Incoming, plus the common ancestor for diff3),
//! each resolved with one click; hovering a button previews the result. Resolutions are written to the editor's own buffer
//! (a single undoable edit); when all are done the file is saved from here.

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, WeakEntity, Window, div, prelude::*, px};

use crate::conflict::{self, Choice, Conflict};
use crate::editor::EditorView;
use crate::settings::Settings;
use crate::ui::{self, MONO_FONT, Palette, c, icon, palette, text};

gpui::actions!(merge, [UndoMerge, RedoMerge]);

pub enum MergeEvent {
    /// All resolved and the user said "Save": the file was saved, the page can close.
    Saved,
    Close,
}

pub struct MergeView {
    /// Weak: when the tab closes the editor is dropped, and the page closes itself.
    source: WeakEntity<EditorView>,
    hover: Option<(usize, Choice)>,
    focus_handle: FocusHandle,
    _sub: Subscription,
    _released: Subscription,
}

impl EventEmitter<MergeEvent> for MergeView {}

impl Focusable for MergeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// How a conflict is displayed on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub first_line: usize,
    pub last_line: usize,
    pub current_label: String,
    pub incoming_label: String,
    pub current: Vec<String>,
    pub incoming: Vec<String>,
    pub base: Option<Vec<String>>,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

const CONTEXT: usize = 2;
/// Maximum conflict cards drawn at once and maximum lines shown in one pane.
const MAX_CARDS: usize = 40;
const MAX_PANE_LINES: usize = 200;

/// Break conflicts into cards (pure: testable).
pub fn cards(rope: &buffer::Rope) -> Vec<Card> {
    let lines = |r: std::ops::Range<usize>| -> Vec<String> { r.map(|l| rope.line(l).to_string().trim_end_matches(['\n', '\r']).to_string()).collect() };
    let all: Vec<Conflict> = conflict::find(rope);
    all.iter()
        .enumerate()
        .map(|(i, c)| {
            // Context must not spill into the neighboring conflict's marker lines.
            let floor = if i == 0 { 0 } else { all[i - 1].end + 1 };
            let ceil = all.get(i + 1).map_or(rope.len_lines(), |n| n.start);
            let before = lines(c.start.saturating_sub(CONTEXT).max(floor)..c.start);
            let after_end = (c.end + 1 + CONTEXT).min(ceil).min(rope.len_lines());
            // The file's empty last "line" (after the trailing newline) doesn't count as context.
            let after = lines((c.end + 1).min(after_end)..after_end)
                .into_iter()
                .enumerate()
                .filter(|(_, l)| !(l.is_empty() && c.end + 1 + CONTEXT > rope.len_lines().saturating_sub(1)))
                .map(|(_, l)| l)
                .collect();
            Card {
                first_line: c.start + 1,
                last_line: c.end + 1,
                current_label: c.current_label.clone(),
                incoming_label: c.incoming_label.clone(),
                current: lines(c.current_lines()),
                incoming: lines(c.incoming_lines()),
                base: c.base_lines().map(lines),
                before,
                after,
            }
        })
        .collect()
}

/// Lines that replace the conflict as the result of the choice.
pub fn preview(card: &Card, choice: Choice) -> Vec<String> {
    match choice {
        Choice::Current => card.current.clone(),
        Choice::Incoming => card.incoming.clone(),
        Choice::Both => card.current.iter().chain(&card.incoming).cloned().collect(),
        Choice::BothIncomingFirst => card.incoming.iter().chain(&card.current).cloned().collect(),
    }
}

impl MergeView {
    pub fn new(source: Entity<EditorView>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&source, |_, _, cx| cx.notify());
        // If the editor (tab) closes, the merge page has no meaning left.
        let released = cx.observe_release(&source, |_, _, cx| cx.emit(MergeEvent::Close));
        MergeView { source: source.downgrade(), hover: None, focus_handle: cx.focus_handle(), _sub: sub, _released: released }
    }

    pub fn title(&self, cx: &App) -> String {
        self.source.upgrade().map_or_else(|| crate::i18n::t("Merge").to_string(), |e| crate::i18n::tf("Merge {}", &[&e.read(cx).name]))
    }

    #[cfg(test)]
    pub fn conflict_count(&self, cx: &App) -> usize {
        self.source.upgrade().map_or(0, |e| conflict::find(&e.read(cx).rope_snapshot()).len())
    }

    /// Test: preview as if a button on the card were hovered.
    #[cfg(test)]
    pub fn hover_for_test(&mut self, index: usize, choice: Choice, cx: &App) -> Vec<String> {
        self.hover = Some((index, choice));
        self.source.upgrade().and_then(|e| cards(&e.read(cx).rope_snapshot()).get(index).map(|c| preview(c, choice))).unwrap_or_default()
    }

    pub fn resolve(&mut self, index: usize, choice: Choice, cx: &mut Context<Self>) {
        self.hover = None;
        if let Some(e) = self.source.upgrade() {
            e.update(cx, |e, cx| e.resolve_conflict_at(index, choice, cx));
        }
    }

    pub fn resolve_all(&mut self, choice: Choice, cx: &mut Context<Self>) {
        self.hover = None;
        if let Some(e) = self.source.upgrade() {
            e.update(cx, |e, cx| e.resolve_all_conflicts(choice, cx));
        }
    }

    fn pane(
        &self,
        surface: &str,
        id: String,
        title: String,
        subtitle: Option<String>,
        lines: &[String],
        accent: u32,
        p: &Palette,
        font: &SharedString,
        size: f32,
    ) -> gpui::AnyElement {
        // Very long blocks must not slow the UI: the first `MAX_PANE_LINES` lines are drawn, the rest is reported in a note.
        let extra = lines.len().saturating_sub(MAX_PANE_LINES);
        let lines = &lines[..lines.len() - extra];
        let heard = match &subtitle {
            Some(s) => format!("{title} ({s})"),
            None => title.clone(),
        };
        let text = if lines.is_empty() { crate::i18n::t("(nothing)").to_string() } else { lines.join("\n") };
        div()
            .id(SharedString::from(id.clone()))
            .relative()
            .child(crate::a11y_controls::control(crate::a11y_controls::Control::text(surface, id, accesskit::Role::Document, heard).value(text)))
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .rounded(px(theme::radius::MD))
            .border_1()
            .border_color(ui::ca(accent, 0.45))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .px(px(theme::SPACE[4]))
                    .h(px(26.))
                    .bg(ui::ca(accent, 0.18))
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                    .children(subtitle.map(|s| div().text_color(c(p.text_muted)).truncate().child(s))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .py(px(theme::SPACE[2]))
                    .bg(ui::ca(accent, 0.07))
                    .font_family(font.clone())
                    .text_size(px(size))
                    .text_color(c(p.text))
                    .when(lines.is_empty(), |d| d.child(div().px(px(theme::SPACE[4])).text_color(c(p.text_muted)).child(crate::i18n::t("(nothing)"))))
                    .children(lines.iter().map(|l| {
                        div().px(px(theme::SPACE[4])).h(px(size * 1.5)).whitespace_nowrap().overflow_hidden().child(if l.is_empty() {
                            " ".to_string()
                        } else {
                            l.replace('\t', "    ")
                        })
                    }))
                    .when(extra > 0, |d| {
                        d.child(div().px(px(theme::SPACE[4])).text_color(c(p.text_muted)).child(crate::i18n::tn(
                            "… {} more line",
                            "… {} more lines",
                            extra,
                            &[],
                        )))
                    }),
            )
            .into_any_element()
    }
}

impl Render for MergeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let s = cx.global::<Settings>();
        let size = (s.editor_font_size - 1.).max(11.);
        let font: SharedString = s.editor_font_family.clone().map_or(MONO_FONT.into(), Into::into);
        let editor = self.source.upgrade();
        let name = editor.as_ref().map(|e| e.read(cx).name.to_string()).unwrap_or_default();
        let all_cards = editor.as_ref().map(|e| cards(&e.read(cx).rope_snapshot())).unwrap_or_default();
        // Drawing all of them in a file with thousands of conflicts is slow: the first `MAX_CARDS` are shown, the next ones appear as you resolve.
        let hidden = all_cards.len().saturating_sub(MAX_CARDS);
        let cards: Vec<Card> = all_cards.into_iter().take(MAX_CARDS).collect();
        let total = cards.len() + hidden;
        let hover = self.hover;
        let surface = crate::i18n::tf("Merge {}", &[&name]);
        let ctl = |key: String, label: &str| crate::a11y_controls::Control::button(surface.clone(), key, label, |_, _| {});
        let button = |id: SharedString, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px(px(theme::SPACE[5]))
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(theme::radius::MD))
                .cursor_pointer()
                .text_size(px(text::XS.0))
                .when(primary, |d| d.bg(c(p.accent)).text_color(c(p.on_accent)).hover(|d| d.opacity(0.9)))
                .when(!primary, |d| d.border_1().border_color(c(p.border)).text_color(c(p.text)).hover(|d| d.bg(c(p.surface_hover))))
                .child(label)
        };
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[4]))
            .h(px(44.))
            .px(px(theme::SPACE[7]))
            .border_b_1()
            .border_color(c(p.border))
            .child(icon("git-compare", 16., p.accent))
            .child(div().relative().font_weight(gpui::FontWeight::SEMIBOLD).child(crate::i18n::tf("Merge {}", &[&name])).child(crate::a11y_controls::said(
                surface.clone(),
                "title",
                accesskit::Role::Heading,
                crate::i18n::tf("Merge {}", &[&name]),
            )))
            .child(div().text_size(px(text::XS.0)).text_color(c(if total == 0 { p.success } else { p.warning })).child(match total {
                0 => crate::i18n::t("All conflicts are resolved").to_string(),
                n => crate::i18n::tn("{} conflict left", "{} conflicts left", n, &[]),
            }))
            .child(div().flex_1())
            .when(total > 0, |d| {
                d.child(crate::a11y_controls::press_on(
                    button("merge-all-current".into(), crate::i18n::t("Accept All Current"), false),
                    ctl("all-current".into(), crate::i18n::t("Accept All Current")),
                    cx,
                    |this, _, cx| this.resolve_all(Choice::Current, cx),
                ))
                .child(crate::a11y_controls::press_on(
                    button("merge-all-incoming".into(), crate::i18n::t("Accept All Incoming"), false),
                    ctl("all-incoming".into(), crate::i18n::t("Accept All Incoming")),
                    cx,
                    |this, _, cx| this.resolve_all(Choice::Incoming, cx),
                ))
            })
            .when(total == 0, |d| {
                d.child(crate::a11y_controls::press_on(
                    button("merge-save".into(), crate::i18n::t("Save File"), true),
                    ctl("save".into(), crate::i18n::t("Save File")),
                    cx,
                    |this, _, cx| {
                        if let Some(e) = this.source.upgrade() {
                            e.update(cx, |e, cx| e.save_now(cx));
                        }
                        cx.emit(MergeEvent::Saved);
                    },
                ))
            })
            .child(crate::a11y_controls::press_on(
                button("merge-close".into(), crate::i18n::t("Close"), false),
                ctl("close".into(), crate::i18n::t("Close")),
                cx,
                |_, _, cx| cx.emit(MergeEvent::Close),
            ));
        let body = div().id("merge-body").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().items_center().child(
            div()
                .w_full()
                .max_w(px(1100.))
                .flex()
                .flex_col()
                .gap(px(theme::SPACE[7]))
                .px(px(theme::SPACE[7]))
                .py(px(theme::SPACE[7]))
                .when(total == 0, |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(theme::SPACE[4]))
                            .py(px(theme::SPACE[9]))
                            .text_color(c(p.text_muted))
                            .child(icon("check", 28., p.success))
                            .child(
                                div().relative().text_size(px(text::LG.0)).text_color(c(p.text)).child(crate::i18n::t("Nothing left to resolve")).child(
                                    crate::a11y_controls::said(surface.clone(), "done", accesskit::Role::Label, crate::i18n::t("Nothing left to resolve")),
                                ),
                            )
                            .child(crate::i18n::t("Save the file to finish, then stage it in Source Control.")),
                    )
                })
                .children(cards.into_iter().enumerate().map(|(i, card)| {
                    let ctx_lines = |lines: &[String], tag: String| {
                        div().flex().flex_col().font_family(font.clone()).text_size(px(size)).text_color(c(p.text_muted)).children(
                            lines.iter().enumerate().map(move |(n, l)| {
                                div()
                                    .id(SharedString::from(format!("{tag}{n}")))
                                    .px(px(theme::SPACE[4]))
                                    .h(px(size * 1.5))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(if l.is_empty() { " ".to_string() } else { l.replace('\t', "    ") })
                            }),
                        )
                    };
                    let result: Option<Vec<String>> = hover.filter(|(ix, _)| *ix == i).map(|(_, ch)| preview(&card, ch));
                    let conflict = crate::i18n::tf("Conflict {0} of {1}", &[&(i + 1), &total]);
                    let choose = |id: &'static str, label: &'static str, choice: Choice| {
                        crate::a11y_controls::press_on(
                            button(SharedString::from(format!("{id}{i}")), label, false).on_hover(cx.listener(move |this, over: &bool, _, cx| {
                                this.hover = over.then_some((i, choice));
                                cx.notify();
                            })),
                            ctl(format!("{id}{i}"), label).description(conflict.clone()),
                            cx,
                            move |this, _, cx| this.resolve(i, choice, cx),
                        )
                    };
                    div()
                        .id(SharedString::from(format!("conflict{i}")))
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE[3]))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(theme::SPACE[3]))
                                .text_size(px(text::XS.0))
                                .text_color(c(p.text_muted))
                                .child(
                                    div()
                                        .text_color(c(p.text))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(crate::i18n::tf("Conflict {0} of {1}", &[&(i + 1), &total])),
                                )
                                .child(crate::i18n::tf("lines {0}–{1}", &[&card.first_line, &card.last_line]))
                                .relative()
                                .child(crate::a11y_controls::said(
                                    surface.clone(),
                                    format!("conflict{i}"),
                                    accesskit::Role::Heading,
                                    format!("{}, {}", conflict, crate::i18n::tf("lines {0}–{1}", &[&card.first_line, &card.last_line])),
                                )),
                        )
                        .child(ctx_lines(&card.before, format!("b{i}-")))
                        .child(
                            div()
                                .flex()
                                .gap(px(theme::SPACE[4]))
                                .child(self.pane(
                                    &surface,
                                    format!("cur{i}"),
                                    crate::i18n::t("Current").into(),
                                    Some(card.current_label.clone()).filter(|l| !l.is_empty()),
                                    &card.current,
                                    p.success,
                                    &p,
                                    &font,
                                    size,
                                ))
                                .children(card.base.as_ref().map(|b| {
                                    self.pane(&surface, format!("base{i}"), crate::i18n::t("Common ancestor").into(), None, b, p.text_muted, &p, &font, size)
                                }))
                                .child(self.pane(
                                    &surface,
                                    format!("inc{i}"),
                                    crate::i18n::t("Incoming").into(),
                                    Some(card.incoming_label.clone()).filter(|l| !l.is_empty()),
                                    &card.incoming,
                                    p.info,
                                    &p,
                                    &font,
                                    size,
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(theme::SPACE[3]))
                                .child(choose("accept-current", crate::i18n::t("Accept Current"), Choice::Current))
                                .child(choose("accept-incoming", crate::i18n::t("Accept Incoming"), Choice::Incoming))
                                .child(choose("accept-both", crate::i18n::t("Accept Both"), Choice::Both))
                                .child(choose("accept-both-rev", crate::i18n::t("Both, Incoming First"), Choice::BothIncomingFirst)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .rounded(px(theme::radius::MD))
                                .border_1()
                                .border_color(c(p.border))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .px(px(theme::SPACE[4]))
                                        .h(px(24.))
                                        .bg(c(p.surface))
                                        .text_size(px(text::XS.0))
                                        .text_color(c(p.text_muted))
                                        .child(crate::i18n::t("Result")),
                                )
                                .child(match result {
                                    Some(lines) => div()
                                        .flex()
                                        .flex_col()
                                        .py(px(theme::SPACE[2]))
                                        .font_family(font.clone())
                                        .text_size(px(size))
                                        .text_color(c(p.text))
                                        .when(lines.is_empty(), |d| {
                                            d.child(div().px(px(theme::SPACE[4])).text_color(c(p.text_muted)).child(crate::i18n::t("(the lines are removed)")))
                                        })
                                        .children(lines.into_iter().map(|l| {
                                            div().px(px(theme::SPACE[4])).h(px(size * 1.5)).whitespace_nowrap().overflow_hidden().child(if l.is_empty() {
                                                " ".to_string()
                                            } else {
                                                l.replace('\t', "    ")
                                            })
                                        }))
                                        .into_any_element(),
                                    None => div()
                                        .px(px(theme::SPACE[4]))
                                        .py(px(theme::SPACE[3]))
                                        .text_size(px(text::XS.0))
                                        .text_color(c(p.line_number))
                                        .child(crate::i18n::t("Hover an option to preview the result."))
                                        .into_any_element(),
                                }),
                        )
                        .child(ctx_lines(&card.after, format!("a{i}-")))
                })),
        );
        div()
            .id("merge-view")
            .key_context("MergeView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &UndoMerge, window, cx| {
                if let Some(e) = this.source.upgrade() {
                    e.update(cx, |e, cx| e.undo_edit(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &RedoMerge, window, cx| {
                if let Some(e) = this.source.upgrade() {
                    e.update(cx, |e, cx| e.redo_edit(window, cx));
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .text_color(c(p.text))
            .child(header)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREE: &str = "top\nctx1\nctx2\n<<<<<<< HEAD\nours 1\nours 2\n||||||| base\norig\n=======\ntheirs\n>>>>>>> feature\nmid\n<<<<<<< HEAD\n=======\nnew\n>>>>>>> feature\nend\n";

    #[test]
    fn cards_carry_labels_panes_base_and_bounded_context() {
        let rope = buffer::Rope::from_str(THREE);
        let c = cards(&rope);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].first_line, c[0].last_line), (4, 11));
        assert_eq!((c[0].current_label.as_str(), c[0].incoming_label.as_str()), ("HEAD", "feature"));
        assert_eq!(c[0].current, ["ours 1", "ours 2"]);
        assert_eq!(c[0].incoming, ["theirs"]);
        assert_eq!(c[0].base.as_deref(), Some(&["orig".to_string()][..]));
        assert_eq!(c[0].before, ["ctx1", "ctx2"], "two lines of context before");
        assert_eq!(c[0].after, ["mid"], "context stops at the next conflict");
        assert_eq!(c[1].before, ["mid"], "and does not swallow the previous conflict's markers");
        assert_eq!((c[1].current.len(), c[1].incoming.clone()), (0, vec!["new".to_string()]));
        assert!(c[1].base.is_none());
        assert_eq!(c[1].after, ["end"]);
    }

    #[test]
    fn previews_match_what_resolution_writes() {
        let rope = buffer::Rope::from_str(THREE);
        let card = &cards(&rope)[0];
        assert_eq!(preview(card, Choice::Current), ["ours 1", "ours 2"]);
        assert_eq!(preview(card, Choice::Incoming), ["theirs"]);
        assert_eq!(preview(card, Choice::Both), ["ours 1", "ours 2", "theirs"]);
        assert_eq!(preview(card, Choice::BothIncomingFirst), ["theirs", "ours 1", "ours 2"]);
        let c = &conflict::find(&rope)[0];
        for choice in [Choice::Current, Choice::Incoming, Choice::Both, Choice::BothIncomingFirst] {
            let written = conflict::resolution(&rope, c, choice);
            assert_eq!(written.lines().map(str::to_string).collect::<Vec<_>>(), preview(card, choice), "{choice:?}");
        }
    }

    #[test]
    fn no_conflicts_no_cards() {
        assert!(cards(&buffer::Rope::from_str("just text\n")).is_empty());
    }
}
