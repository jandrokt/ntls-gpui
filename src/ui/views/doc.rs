//! Showing a document.
//!
//! The file is the source and something else edits it, so this pane only
//! renders: markdown as blocks, with every `{{ … }}` worked out against the
//! tools in the same workspace.

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px, relative,
};

use crate::doc::{Block, Span, blocks};
use crate::ui::app::App;
use crate::ui::icons::icon;
use crate::ui::notes::Data;
use crate::ui::theme::Theme;
use crate::ui::widgets::{Kind, Type, button, space, text};

/// How wide a column of prose is allowed to get. Beyond this a line is hard to
/// come back from at the start of the next one.
const MEASURE: gpui::Pixels = px(760.);

impl App {
    pub(super) fn doc_pane(
        &mut self,
        id: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item = crate::ui::workspace::Item::Doc(id);
        let editor = self.editor_for(item).cloned();
        let renaming = self.renaming_item == Some(item);
        let rename_input = self.rename_input.clone();
        let data = Data::of(self.workspace());
        let Some(doc) = self.workspace().doc(id) else { return div().into_any_element() };
        let (title, path, scroll) = (doc.title(&data), doc.path.clone(), doc.scroll.clone());
        let dirty = editor.as_ref().is_some_and(|e| e.read(cx).dirty);
        // The expressions are worked out here, not in the file: the document
        // is the question and the tools are the answer, and the answer changes.
        let rendered = crate::doc::expand(&doc.source, &data);
        let body: Vec<AnyElement> =
            blocks(&rendered).iter().map(|b| block(b, theme)).collect();
        let empty = body.is_empty();

        // A flex item will not shrink below its content unless it is told
        // it may, and a pane that never shrinks pushes its scroll area past
        // the bottom of the window instead of scrolling it.
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(theme.bg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SNUG))
                    .px(px(space::GUTTER))
                    .h(px(44.))
                    .flex_shrink_0()
                    .child(icon("note", px(16.), theme.accent))
                    .child(if renaming {
                        div()
                            .flex()
                            .items_center()
                            .flex_1()
                            .min_w_0()
                            .h(px(24.))
                            .px(px(6.))
                            .rounded(px(6.))
                            .bg(theme.bg)
                            .border_1()
                            .border_color(theme.focus)
                            .text_heading()
                            .child(rename_input)
                            .into_any_element()
                    } else {
                        div()
                            .id("doc-title")
                            .flex_1()
                            .min_w_0()
                            .text_heading()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .truncate()
                            .cursor_pointer()
                            // Names are edited where the names are.
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.begin_item_rename(item, window, cx)
                            }))
                            .child(title)
                            .into_any_element()
                    })
                    .child(
                        div()
                            .max_w(px(260.))
                            .text_meta()
                            .text_color(theme.faint)
                            .truncate()
                            .child(path.display().to_string()),
                    )
                    .when(dirty, |d| {
                        d.child(
                            div()
                                .text_meta()
                                .text_color(theme.warn)
                                .flex_shrink_0()
                                .child("unsaved"),
                        )
                    })
                    .child(
                        button(
                            "doc-edit",
                            if editor.is_some() { "Done" } else { "Edit" },
                            if editor.is_some() { Kind::Primary } else { Kind::Normal },
                            theme,
                        )
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.toggle_editing(item, window, cx)
                        })),
                    )
                    .child(
                        button("doc-external", "Open\u{2026}", Kind::Ghost, theme)
                            .on_click(cx.listener(move |app, _, _, cx| app.edit_doc(id, cx))),
                    ),
            )
            .child(super::rule(theme))
            // While it is being edited the pane is split: the source on the
            // left, and what it comes to on the right. A document's figures
            // are the reason to write one, so they are worth watching as you
            // write it.
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .children(editor.clone().map(|editor| {
                        div()
                            .flex()
                            .flex_col()
                            .w(relative(0.5))
                            .min_w_0()
                            .border_r_1()
                            .border_color(theme.border)
                            .child(source_pane(editor, theme, cx))
                    }))
                    .child(
                        div()
                            .id(SharedString::from(format!("doc-{id}")))
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .px(px(space::GUTTER))
                            .py(px(space::SECTION))
                            .overflow_y_scroll()
                            .track_scroll(&scroll)
                            .when(empty, |d| {
                                d.child(
                                    div()
                                        .text_small()
                                        .text_color(theme.faint)
                                        .child("This document is empty. Edit writes into it."),
                                )
                            })
                            .children(body),
                    ),
            )
            .into_any_element()
    }
}

/// The editable half: the source, and whatever completion is offering.
pub(super) fn source_pane(
    editor: gpui::Entity<crate::ui::editor::Editor>,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let (offering, at, offer) = {
        let held = editor.read(cx);
        (held.offering.clone(), held.offering_at, held.offer_at())
    };
    let scroll = editor.read(cx).scroll.clone();

    div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(
            div()
                .id("editor-scroll")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .px(px(space::GUTTER))
                .py(px(space::ROOMY))
                .mono()
                .text_small()
                .line_height(px(20.))
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .child(editor.clone()),
        )
        // The list follows the caret, so it reads as belonging to what is
        // being typed instead of to the pane.
        .children(offer.map(|origin| {
            div()
                .absolute()
                .left(origin.x + px(space::GUTTER))
                .top(origin.y + px(space::ROOMY) - scroll.offset().y)
                .w(px(280.))
                .max_h(px(220.))
                .flex()
                .flex_col()
                .p(px(4.))
                .rounded(px(8.))
                .bg(theme.panel)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .overflow_hidden()
                .children(offering.iter().enumerate().map(|(i, candidate)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SNUG))
                        .h(px(22.))
                        .px(px(space::SNUG))
                        .rounded(px(4.))
                        .when(i == at, |d| d.bg(theme.accent_soft))
                        .child(
                            div()
                                .flex_shrink_0()
                                .mono()
                                .text_small()
                                .text_color(if i == at { theme.accent } else { theme.text })
                                .child(candidate.text.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_meta()
                                .text_color(theme.faint)
                                .child(candidate.kind),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_meta()
                                .text_color(theme.faint)
                                .truncate()
                                .child(candidate.detail.clone()),
                        )
                }))
        }))
        .into_any_element()
}

fn block(block: &Block, theme: &Theme) -> AnyElement {

    match block {
        Block::Heading(level, spans) => {
            let (size, weight, top) = match level {
                1 => (px(21.), FontWeight::BOLD, 4.),
                2 => (px(17.), FontWeight::SEMIBOLD, 22.),
                _ => (text::BODY, FontWeight::SEMIBOLD, 18.),
            };
            line(spans, theme)
                .max_w(MEASURE)
                .mt(px(top))
                .mb(px(6.))
                .text_size(size)
                // A heading is a line of its own, so it may have a line box of
                // its own. The rule is about runs sharing a row, and every
                // run in this one shares this.
                .line_height(size * 1.35)
                .font_weight(weight)
                .text_color(theme.text)
                .into_any_element()
        }

        Block::Paragraph(spans) => line(spans, theme)
            .max_w(MEASURE)
            .mb(px(10.))
            .text_body()
            .line_height(px(22.))
            .text_color(theme.text)
            .into_any_element(),

        Block::Item { depth, marker, spans } => div()
            .flex()
            .max_w(MEASURE)
            .mb(px(4.))
            .pl(px(12. + *depth as f32 * 18.))
            .text_body()
            .line_height(px(22.))
            .text_color(theme.text)
            .child(
                div()
                    .w(px(20.))
                    .flex_shrink_0()
                    .text_color(theme.faint)
                    .child(match marker {
                        Some(n) => format!("{n}."),
                        None => "\u{2022}".to_string(),
                    }),
            )
            .child(line(spans, theme).flex_1().min_w_0())
            .into_any_element(),

        Block::Quote(spans) => line(spans, theme)
            .max_w(MEASURE)
            .mb(px(10.))
            .pl(px(12.))
            .border_l_2()
            .border_color(theme.border)
            .text_body()
            .line_height(px(22.))
            .text_color(theme.dim)
            .into_any_element(),

        Block::Code { language, text: body } => div()
            .flex()
            .flex_col()
            .max_w(MEASURE)
            .mb(px(12.))
            .p(px(space::ROOMY))
            .rounded(px(8.))
            .bg(theme.panel)
            .border_1()
            .border_color(theme.border)
            .when(!language.is_empty(), |d| {
                d.child(
                    div()
                        .mb(px(6.))
                        .text_caps()
                        .text_color(theme.faint)
                        .child(language.to_uppercase()),
                )
            })
            .children(body.lines().map(|line| {
                div()
                    .mono()
                    .text_small()
                    .text_color(theme.text)
                    .child(line.to_string())
            }))
            .into_any_element(),

        Block::Rule => div()
            .max_w(MEASURE)
            .h(px(1.))
            .my(px(18.))
            .bg(theme.rule)
            .into_any_element(),
    }
}

/// A line of prose: a row of words that wraps.
///
/// GPUI lays a `div` out as a block, so a bold run in the middle of a sentence
/// would start a new line. The sentence is therefore split into words, each
/// its own child of a wrapping row, which puts the emphasis back
/// inside the line it belongs to.
fn line(spans: &[Span], theme: &Theme) -> gpui::Div {
    let words: Vec<AnyElement> =
        words(spans).into_iter().map(|(span, spaced)| run(&span, spaced, theme)).collect();
    div().flex().flex_wrap().items_baseline().children(words)
}

/// Splits a line into the pieces that lay out: one per word of plain text, one
/// per emphasised run, each knowing whether a space follows it.
///
/// The space has to be worked out across the whole line and not inside one
/// run, because `**5** of 6` puts the space at the start of the run *after*
/// the bold one.
fn words(spans: &[Span]) -> Vec<(Span, bool)> {
    let mut out: Vec<(Span, bool)> = Vec::new();

    for span in spans {
        if span.bold || span.italic || span.code || span.link.is_some() {
            out.push((span.clone(), false));
            continue;
        }
        if span.text.starts_with(char::is_whitespace)
            && let Some(previous) = out.last_mut()
        {
            previous.1 = true;
        }
        let trailing = span.text.ends_with(char::is_whitespace);
        let mut parts = span.text.split_whitespace().peekable();
        while let Some(part) = parts.next() {
            let followed = parts.peek().is_some() || trailing;
            out.push((Span { text: part.to_string(), ..span.clone() }, followed));
        }
    }
    out
}

/// One run inside a line. Every run keeps the line's size and family, and only
/// weight, slant and colour vary, so a bold word does not sit at a different
/// height from the words around it.
fn run(span: &Span, spaced: bool, theme: &Theme) -> AnyElement {
    let mut element = div().when(span.bold, |d| d.font_weight(FontWeight::SEMIBOLD));

    if span.italic {
        element = element.italic();
    }
    if span.code {
        element = element
            .px(px(4.))
            .rounded(px(4.))
            .bg(theme.track)
            .text_color(theme.accent);
    } else if span.link.is_some() {
        element = element.text_color(theme.accent).underline();
    }

    // An expression that could not be worked out is quoted where it was
    // written, and drawn as the mistake it is.
    if span.text.starts_with('\u{27e8}') && span.text.ends_with('\u{27e9}') {
        element = element
            .px(px(4.))
            .rounded(px(4.))
            .bg(theme.down_soft)
            .text_color(theme.down);
    }

    // A space at the edge of a text run is not drawn, so the gap between
    // words is a margin instead.
    element.when(spaced, |d| d.mr(px(4.))).child(span.text.clone()).into_any_element()
}
