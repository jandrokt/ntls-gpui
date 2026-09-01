//! Drawing the picker: a searchable list over the window.

use gpui::{
    AnimationExt, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder,
    px,
};

use crate::ui::app::App;
use crate::ui::icons::icon;
use crate::ui::theme::Theme;
use crate::ui::widgets::{Type, card, keycap, motion, once, space};

impl App {
    pub(super) fn picker_overlay(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let picker = self.picker.clone();
        let (input, cursor, title, offering) = {
            let p = picker.read(cx);
            (p.input.clone(), p.cursor, p.title.clone(), p.offering_new())
        };
        input.update(cx, |input, _| {
            input.text_color = theme.text;
            input.placeholder_color = theme.faint;
            input.caret_color = theme.accent;
            input.selection_color = theme.accent.opacity(0.28);
        });

        let matches: Vec<(usize, &'static str, String, String)> = picker
            .read(cx)
            .matches()
            .into_iter()
            .map(|c| (c.id, c.icon, c.label.clone(), c.detail.clone()))
            .collect();
        let count = matches.len();

        let mut rows: Vec<AnyElement> = matches
            .into_iter()
            .enumerate()
            .map(|(i, (_, glyph, label, detail))| {
                row(i, glyph, &label, &detail, i == cursor, theme, cx)
            })
            .collect();
        if let Some(label) = offering {
            rows.push(row(count, "plus", &label, "", count == cursor, theme, cx));
        }

        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .bg(gpui::black().opacity(if theme.mode == crate::ui::theme::Mode::Dark {
                0.45
            } else {
                0.16
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, window, cx| app.close_picker(window, cx)),
            )
            .child(
                card(theme)
                    .id("picker")
                    .occlude()
                    .w(px(460.))
                    .max_h(px(380.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .px(px(14.))
                            .pt(px(12.))
                            .child(div().text_caps().text_color(theme.faint).child(title)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .px(px(14.))
                            .h(px(38.))
                            .child(icon("search", px(13.), theme.faint))
                            .child(div().flex_1().text_body().child(input)),
                    )
                    .child(super::rule(theme))
                    .child(
                        div()
                            .id("picker-list")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .gap(px(1.))
                            .p(px(6.))
                            .overflow_y_scroll()
                            .children(if rows.is_empty() {
                                vec![
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .h(px(56.))
                                        .text_small()
                                        .text_color(theme.faint)
                                        .child("Nothing to choose")
                                        .into_any_element(),
                                ]
                            } else {
                                rows
                            }),
                    )
                    .with_animation("picker-in", once(motion::QUICK), |d, delta| {
                        d.opacity(delta).mt(px(120. - 8. * (1. - delta)))
                    }),
            )
            .into_any_element()
    }
}

fn row(
    index: usize,
    glyph: &'static str,
    label: &str,
    detail: &str,
    on_cursor: bool,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("pick-{index}")))
        .flex()
        .items_center()
        .gap(px(space::ROOMY))
        .h(px(32.))
        .px(px(10.))
        .rounded(px(6.))
        .cursor_pointer()
        .when(on_cursor, |d| d.bg(theme.accent_soft))
        .when(!on_cursor, |d| d.hover(|s| s.bg(theme.hover)))
        .child(icon(glyph, px(14.), theme.accent))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_small()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text)
                .truncate()
                .child(label.to_string()),
        )
        .when(!detail.is_empty(), |d| {
            d.child(
                div()
                    .max_w(px(180.))
                    .text_small()
                    .text_color(theme.faint)
                    .truncate()
                    .child(detail.to_string()),
            )
        })
        .when(on_cursor, |d| d.child(keycap("⏎", theme)))
        .on_click(cx.listener(move |app, _, window, cx| {
            app.picker.update(cx, |p, _| p.cursor = index);
            app.confirm_picker(window, cx);
        }))
        .into_any_element()
}
