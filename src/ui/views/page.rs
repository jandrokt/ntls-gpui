//! The shape every full-window page has.
//!
//! There is nothing to confirm and nothing to dismiss. You are on a page until
//! you go somewhere else, so there is no Done button: the rail is how you
//! leave, the same way you arrived.
//!
//! One column width, one header, one card and one settings row, shared, so the
//! pages look like each other.

use gpui::{
    AnimationExt, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px, relative,
};

use crate::ui::app::App;
pub(super) use crate::ui::app::Page;
use crate::ui::icons::icon;
use crate::ui::theme::Theme;
use crate::ui::widgets::{Type, motion, once, space};

use super::rule;

/// The width a page's content is held to.
///
/// Lines the width of a maximised window are hard to read: the eye loses the
/// start of the next one somewhere around the middle of the screen.
const COLUMN: gpui::Pixels = px(760.);

/// The width of the column a setting's control sits in.
const CONTROL: gpui::Pixels = px(340.);

/// A page: its heading, whatever it offers at the top right, and its content.
///
/// The content arrives and does not simply appear. It is a small movement and a
/// short one, but without it the whole window changing in a single frame reads
/// as a glitch instead of as going somewhere.
pub(super) fn page_shell(
    page: Page,
    theme: &Theme,
    actions: Vec<AnyElement>,
    body: Vec<AnyElement>,
    _cx: &mut Context<App>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .bg(theme.bg)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::SNUG))
                .h(px(44.))
                .flex_shrink_0()
                .px(px(space::GUTTER))
                .child(icon(page.icon(), px(16.), theme.dim))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_heading()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(page.title()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::TIGHT))
                        .flex_shrink_0()
                        .children(actions),
                ),
        )
        .child(rule(theme))
        .child(
            div()
                .id(SharedString::from(format!("page-{}", page.icon())))
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                // The column is centred, not left-aligned. A page has no
                // side bar beside it to be aligned to, and a measure of text
                // against the far edge of a wide window is a page with its
                // content pushed into a corner.
                //
                // Neither of these two may give way: a flex child shrinks to
                // its container by default, and a column squashed to the
                // height of the window is a column whose last card hangs
                // below the end of what can be scrolled to.
                .child(
                    div().flex().flex_col().items_center().w_full().flex_shrink_0().child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_shrink_0()
                            .gap(px(space::SECTION))
                            // A width, not a maximum. The height of this
                            // column is worked out from its contents, and a
                            // row's height depends on how many lines its
                            // description wraps to, which depends on the
                            // width. With `max_w` the width is not settled
                            // when the height is measured, every wrapped
                            // description is measured as one line, and the
                            // column comes out short by 18px a time: the last
                            // card of a long page ends up below the end of
                            // what can be scrolled to.
                            .w(COLUMN)
                            // The window cannot currently be made narrower
                            // than this, but if it ever can the column gives
                            // way rather than running off the side.
                            .max_w(relative(1.))
                            .px(px(space::GUTTER * 2.))
                            .pt(px(space::SECTION))
                            // A page ends in air, not against the
                            // bottom of the window: a last row flush with the
                            // edge reads as a page that has been cut off.
                            .pb(px(space::SECTION * 3.))
                            .children(body)
                            .with_animation(
                                SharedString::from(format!("page-in-{}", page.icon())),
                                once(motion::SETTLE),
                                |d, delta| d.opacity(delta).mt(px(10. * (1. - delta))),
                            ),
                    ),
                ),
        )
        .into_any_element()
}

/// One headed group on a page.
pub(super) fn section(title: &str, theme: &Theme, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(space::TIGHT))
        .child(
            div()
                .text_caps()
                .text_color(theme.faint)
                .child(title.to_uppercase()),
        )
        .child(card(theme).children(rows))
        .into_any_element()
}

/// The surface a page's rows sit on.
///
/// It never shrinks. A card inside a scrolling column is a flex child like any
/// other, and letting it give way is how the last row of a list ends up cut in
/// half by the card's own bottom edge.
pub(super) fn card(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .rounded(px(8.))
        .bg(theme.panel)
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
}

/// One setting: what it is on the left, what it is set to on the right.
pub(super) fn setting(label: &str, about: &str, theme: &Theme, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(space::ROOMY))
        .px(px(space::ROOMY))
        .py(px(space::SNUG))
        // The last row's line is the card's own border, so every row can carry
        // one without the bottom of the card being drawn twice.
        .border_b_1()
        .border_color(theme.rule)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_small()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(label.to_string()),
                )
                .when(!about.is_empty(), |d| {
                    d.child(
                        div()
                            .text_meta()
                            .text_color(theme.faint)
                            .child(about.to_string()),
                    )
                }),
        )
        // Every setting's control sits in a column of one width, so a page of
        // them reads as a column instead of a ragged right edge, and so
        // a row of choices has a width to divide into equal shares.
        .child(
            div()
                .flex()
                .justify_end()
                .w(CONTROL)
                .flex_shrink_0()
                .child(control),
        )
        .into_any_element()
}
