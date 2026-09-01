//! Drawing the notices.
//!
//! Two shapes for one list: the corner shows what is new and then gets out of
//! the way, the panel behind the bell shows everything and stays until it is
//! closed. Both are drawn over the window, so nothing else moves when a
//! notice arrives.

use gpui::{
    AnimationExt, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px,
};

use crate::core::Level;
use crate::ui::app::App;
use crate::ui::icons::icon;
use crate::ui::notify::Notice;
use crate::ui::theme::Theme;
use crate::ui::widgets::{Kind, Type, button, motion, once, space};

/// How many can be in the corner at once. A scan finishing in every one of
/// eight workspaces should not cover the window.
const AT_ONCE: usize = 4;

/// The width of both shapes. One width, so a notice does not re-wrap when it
/// moves from the corner into the list.
const WIDTH: gpui::Pixels = px(340.);

impl App {
    /// What is new, in the corner above the status bar.
    pub(super) fn toasts(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let showing = self.notices.showing(AT_ONCE);
        if showing.is_empty() {
            return None;
        }
        let cards: Vec<AnyElement> = showing.into_iter().map(|n| toast(n, theme, cx)).collect();

        Some(
            div()
                .absolute()
                .right(px(space::ROOMY))
                .bottom(px(34.))
                .flex()
                .flex_col()
                .items_end()
                .gap(px(space::TIGHT))
                .children(cards)
                .into_any_element(),
        )
    }

    /// Everything that has happened, behind the bell.
    ///
    /// It comes up out of the status bar, where the bell is: a panel
    /// that simply exists on the next frame reads as a thing that was always
    /// there and you had not noticed.
    pub(super) fn notice_list(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .notices
            .newest_first()
            .map(|n| listed(n, theme, cx))
            .collect();
        let empty = rows.is_empty();

        div()
            .absolute()
            .right(px(space::TIGHT))
            .bottom(px(28.))
            .w(WIDTH)
            .max_h(px(420.))
            .flex()
            .flex_col()
            .rounded(px(10.))
            .bg(theme.panel)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::TIGHT))
                    .px(px(space::ROOMY))
                    .py(px(space::TIGHT))
                    .border_b_1()
                    .border_color(theme.rule)
                    .child(
                        div()
                            .flex_1()
                            .text_caps()
                            .text_color(theme.faint)
                            .child("NOTIFICATIONS"),
                    )
                    .when(!empty, |d| {
                        d.child(
                            button("notices-clear", "Clear", Kind::Ghost, theme)
                                .on_click(cx.listener(|app, _, _, cx| app.clear_notices(cx))),
                        )
                    })
                    .child(
                        button("notices-close", "Close", Kind::Ghost, theme)
                            .on_click(cx.listener(|app, _, _, cx| app.toggle_notices(cx))),
                    ),
            )
            .child(if empty {
                div()
                    .px(px(space::ROOMY))
                    .py(px(space::ROOMY))
                    .text_small()
                    .text_color(theme.faint)
                    .child("Nothing has happened yet.")
                    .into_any_element()
            } else {
                div()
                    .id("notice-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .into_any_element()
            })
            .with_animation("notices-in", once(motion::QUICK), |d, delta| {
                d.opacity(delta).bottom(px(16. + 12. * delta))
            })
            .into_any_element()
    }
}

/// The colour a notice is drawn in, which is the colour its level already has
/// everywhere else in the window.
fn tint(level: Level, theme: &Theme) -> gpui::Hsla {
    theme.level(level)
}

/// One card in the corner.
fn toast(n: &Notice, theme: &Theme, cx: &mut Context<App>) -> AnyElement {
    let (id, colour) = (n.id, tint(n.level, theme));
    let goes_somewhere = n.about.is_some();

    div()
        .id(SharedString::from(format!("toast-{id}")))
        .flex()
        .items_start()
        .gap(px(space::TIGHT))
        .w(WIDTH)
        .px(px(space::ROOMY))
        .py(px(space::SNUG))
        .rounded(px(8.))
        .bg(theme.panel)
        .border_1()
        .border_color(theme.border)
        // The level is a stripe instead of a coloured card: a green panel in
        // the corner of a dark window is a lamp, not a message.
        .border_l_2()
        .border_color(colour)
        .shadow_lg()
        .occlude()
        .when(goes_somewhere, |d| {
            d.cursor_pointer().hover(|s| s.bg(theme.raised))
        })
        .on_click(cx.listener(move |app, _, window, cx| app.open_notice(id, window, cx)))
        .child(div().pt(px(2.)).child(icon(n.icon(), px(13.), colour)))
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
                        .truncate()
                        .child(n.title.clone()),
                )
                .when(!n.body.is_empty(), |d| {
                    d.child(
                        div()
                            .text_meta()
                            .text_color(theme.dim)
                            .max_h(px(48.))
                            .overflow_hidden()
                            .child(n.body.clone()),
                    )
                }),
        )
        .child(
            div()
                .id(SharedString::from(format!("toast-close-{id}")))
                .flex()
                .items_center()
                .justify_center()
                .size(px(16.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.hover))
                .child(icon("close", px(9.), theme.faint))
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.dismiss_notice(id, cx);
                    cx.stop_propagation();
                })),
        )
        // It rises into the corner and fades up. The movement is what makes it
        // read as something that has just happened instead of as something
        // that was always sitting there.
        .with_animation(
            SharedString::from(format!("toast-in-{id}")),
            once(motion::SETTLE),
            |d, delta| d.opacity(delta).mt(px(12. * (1. - delta))),
        )
        .into_any_element()
}

/// One line in the list.
fn listed(n: &Notice, theme: &Theme, cx: &mut Context<App>) -> AnyElement {
    let (id, colour) = (n.id, tint(n.level, theme));
    let goes_somewhere = n.about.is_some();

    div()
        .id(SharedString::from(format!("notice-{id}")))
        .flex()
        .items_start()
        .gap(px(space::TIGHT))
        .px(px(space::ROOMY))
        .py(px(space::TIGHT))
        .border_b_1()
        .border_color(theme.rule)
        .when(goes_somewhere, |d| {
            d.cursor_pointer().hover(|s| s.bg(theme.hover))
        })
        .on_click(cx.listener(move |app, _, window, cx| app.open_notice(id, window, cx)))
        .child(div().pt(px(2.)).child(icon(n.icon(), px(12.), colour)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(space::TIGHT))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_small()
                                .text_color(theme.text)
                                .truncate()
                                .child(n.title.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_meta()
                                .text_color(theme.faint)
                                .child(n.ago()),
                        ),
                )
                .when(!n.body.is_empty(), |d| {
                    d.child(
                        div()
                            .text_meta()
                            .text_color(theme.dim)
                            .max_h(px(48.))
                            .overflow_hidden()
                            .child(n.body.clone()),
                    )
                }),
        )
        .into_any_element()
}
