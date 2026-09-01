//! The command palette, drawn over everything else.

use gpui::{
    AnimationExt, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder,
    px,
};

use crate::ui::app::App;
use crate::ui::icons::icon;
use crate::ui::palette::{Param, Suggest, Value};
use crate::ui::search::Hit;
use crate::ui::theme::Theme;
use crate::ui::widgets::{Type, card, keycap, motion, once, space};

impl App {
    pub(super) fn palette_overlay(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette.clone();
        let (input, cursor) = {
            let p = palette.read(cx);
            (p.input.clone(), p.cursor)
        };
        input.update(cx, |input, _| {
            input.text_color = theme.text;
            input.placeholder_color = theme.faint;
            input.caret_color = theme.accent;
            input.selection_color = theme.accent.opacity(0.28);
        });

        // What the box is offering depends on how far the command has got:
        // which tool, then which of its settings, then which value.
        let suggestion = self.suggestion(cx);
        let command = {
            let p = palette.read(cx);
            let c = p.command();
            c.has_arguments().then(|| {
                let mut parts = Vec::new();
                if !c.target().is_empty() {
                    parts.push(c.target());
                }
                parts.extend(c.named.iter().map(|(k, v)| format!("{k} {v}")));
                parts.join("  ·  ")
            })
        };

        let (heading, rows): (Option<String>, Vec<AnyElement>) = match &suggestion {
            Suggest::Search(hits) => (
                None,
                hits.iter()
                    .enumerate()
                    .map(|(i, hit)| hit_row(i, hit, i == cursor, theme, cx))
                    .collect(),
            ),
            Suggest::Params { tool, items } => (
                Some(format!("{} settings", tool.title())),
                items
                    .iter()
                    .enumerate()
                    .map(|(i, param)| param_row(i, param, i == cursor, theme, cx))
                    .collect(),
            ),
            Suggest::Values { key, items } => (
                Some(format!("{key} accepts")),
                items
                    .iter()
                    .enumerate()
                    .map(|(i, value)| value_row(i, value, i == cursor, theme, cx))
                    .collect(),
            ),
        };
        let empty_line = match &suggestion {
            Suggest::Search(_) => "Nothing found",
            Suggest::Params { .. } => "No setting by that name",
            Suggest::Values { .. } => "Type any value",
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            // A scrim, so the palette reads as being in front of the work
            // and not part of it.
            .bg(gpui::black().opacity(if theme.mode == crate::ui::theme::Mode::Dark {
                0.45
            } else {
                0.16
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, window, cx| app.close_palette(window, cx)),
            )
            .child(
                card(theme)
                    .id("palette")
                    .occlude()
                    .w(px(520.))
                    .max_h(px(420.))
                    .flex()
                    .flex_col()
                    // The card is the boundary: without this the list runs
                    // past the rounded corner and out onto the scrim.
                    .overflow_hidden()
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .px(px(14.))
                            .h(px(44.))
                            .child(
                                div()
                                    .text_body()
                                    .text_color(theme.faint)
                                    .child("⌕"),
                            )
                            .child(div().flex_1().text_heading().child(input)),
                    )
                    .children(command.map(|summary| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(14.))
                            .pb(px(10.))
                            .child(
                                div()
                                    .text_meta()
                                    .text_color(theme.accent)
                                    .flex_shrink_0()
                                    .child("Run with"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_meta()
                                    .text_color(theme.dim)
                                    .truncate()
                                    .child(summary),
                            )
                            .child(keycap("⏎", theme))
                    }))
                    .child(super::rule(theme))
                    .children(heading.map(|heading| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SNUG))
                            .px(px(14.))
                            .pt(px(8.))
                            .child(div().text_caps().text_color(theme.faint).child(heading))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_meta()
                                    .text_color(theme.faint)
                                    .child("tab to complete"),
                            )
                    }))
                    .child(
                        div()
                            .id("palette-list")
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
                                        .h(px(60.))
                                        .text_small()
                                        .text_color(theme.faint)
                                        .child(empty_line)
                                        .into_any_element(),
                                ]
                            } else {
                                rows
                            }),
                    )
                    // The card drops into place under the pointer's own
                    // gesture: a panel that is simply there on the next frame
                    // reads as the window having jumped and not as
                    // something having opened.
                    .with_animation("palette-in", once(motion::QUICK), |d, delta| {
                        d.opacity(delta).mt(px(96. - 8. * (1. - delta)))
                    }),
            )
            .into_any_element()
    }
}

/// A row in the search results: what it is, what it is called, and where it
/// lives. One shape for tools, workspaces, open tools, result rows and
/// interfaces, because the list mixes them.
fn hit_row(
    index: usize,
    hit: &Hit,
    on_cursor: bool,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let colour = match hit {
        Hit::Row { status, .. } => theme.status(*status),
        _ => theme.accent,
    };
    let hit = hit.clone();
    let (kind, label, context) = (hit.kind(), hit.label(), hit.context());
    let mono = matches!(hit, Hit::Row { .. } | Hit::Iface { .. });

    row_shell(format!("pal-hit-{index}"), on_cursor, theme)
        .child(icon(hit.icon(), px(15.), colour))
        .child(
            div()
                .w(px(52.))
                .flex_shrink_0()
                .text_meta()
                .text_color(theme.faint)
                .whitespace_nowrap()
                .child(kind),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .when(mono, |d| d.mono())
                .text_small()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text)
                .truncate()
                .child(label),
        )
        .child(
            div()
                .max_w(px(190.))
                .flex_shrink_0()
                .text_small()
                .text_color(theme.dim)
                .truncate()
                .child(context),
        )
        .when(on_cursor, |d| d.child(keycap("⏎", theme)))
        .on_click(cx.listener(move |app, _, window, cx| {
            app.palette.update(cx, |p, _| p.cursor = index);
            app.confirm_palette(window, cx);
        }))
        .into_any_element()
}

/// A row in the settings list: what it is called, what it takes, what it is
/// set to if you say nothing.
fn param_row(
    index: usize,
    param: &Param,
    on_cursor: bool,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let accepts = if param.values.is_empty() {
        if param.free { String::new() } else { "true / false".into() }
    } else {
        param.values.join(" · ")
    };
    let hint = if accepts.is_empty() { param.help.to_string() } else { accepts };

    row_shell(format!("pal-param-{index}"), on_cursor, theme)
        .child(
            div()
                .w(px(112.))
                .flex_shrink_0()
                .mono()
                .text_small()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.accent)
                .whitespace_nowrap()
                .child(format!("{}=", param.key)),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_small()
                .text_color(theme.text)
                .whitespace_nowrap()
                .child(param.label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_small()
                .text_color(theme.dim)
                .truncate()
                .child(hint),
        )
        .when(!param.default.is_empty(), |d| {
            d.child(
                div()
                    .flex_shrink_0()
                    .text_small()
                    .text_color(theme.faint)
                    .whitespace_nowrap()
                    .child(format!("({})", param.default)),
            )
        })
        .when(on_cursor, |d| d.child(keycap("⇥", theme)))
        .on_click(cx.listener(move |app, _, window, cx| {
            app.palette.update(cx, |p, _| p.cursor = index);
            app.complete_palette(window, cx);
        }))
        .into_any_element()
}

/// A row in the value list for one setting.
fn value_row(
    index: usize,
    value: &Value,
    on_cursor: bool,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    row_shell(format!("pal-value-{index}"), on_cursor, theme)
        .child(
            div()
                .w(px(132.))
                .flex_shrink_0()
                .mono()
                .text_small()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.accent)
                .whitespace_nowrap()
                .child(value.value.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_small()
                .text_color(theme.dim)
                .truncate()
                .child(if value.desc.is_empty() {
                    value.label.clone()
                } else {
                    format!("{}. {}", value.label, value.desc)
                }),
        )
        .when(on_cursor, |d| d.child(keycap("⇥", theme)))
        .on_click(cx.listener(move |app, _, window, cx| {
            app.palette.update(cx, |p, _| p.cursor = index);
            app.complete_palette(window, cx);
        }))
        .into_any_element()
}

/// The shape every suggestion row shares.
fn row_shell(id: String, on_cursor: bool, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(34.))
        .px(px(10.))
        .rounded(px(7.))
        .cursor_pointer()
        .when(on_cursor, |d| d.bg(theme.accent_soft))
        .when(!on_cursor, |d| d.hover(|s| s.bg(theme.hover)))
}
