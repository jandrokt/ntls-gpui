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
            // Only once the first word actually names a tool. Without that,
            // `> work` offered to "Run with work", having taken the `>` for
            // a tool and the rest for its target.
            let names_a_tool =
                crate::ui::palette::tool_of(p.query(), &self.registry).is_some();
            let c = p.command();
            (names_a_tool && c.has_arguments()).then(|| {
                let mut parts = Vec::new();
                if !c.target().is_empty() {
                    parts.push(c.target());
                }
                parts.extend(c.named.iter().map(|(k, v)| format!("{k} {v}")));
                parts.join("  ·  ")
            })
        };

        // What was typed, for picking the matched characters out of each
        // label. Recomputed against the label actually shown rather than
        // carried along, because the label is what the reader is looking at.
        let needle = {
            let typed = palette.read(cx).query().trim().to_lowercase();
            typed.strip_prefix('>').map_or(typed.clone(), |rest| rest.trim().to_string())
        };

        // The row each answer was drawn on. Only the grouped list puts
        // anything between them, so for the others it is one to one.
        let mut placed: Vec<usize> = Vec::new();

        let (heading, rows): (Option<String>, Vec<AnyElement>) = match &suggestion {
            // Grouped under headings, best group first. A flat list mixing
            // commands, tools, runs, rows of results and documents is a
            // list nobody can scan.
            Suggest::Search(hits) => {
                let mut rows = Vec::new();
                let mut at = 0usize;
                for (kind, held) in crate::ui::search::grouped(hits.clone()) {
                    rows.push(group_heading(kind, theme));
                    for hit in held {
                        // Where in the drawn list this answer ended up, which
                        // is not its place among the answers once headings
                        // are drawn between them.
                        placed.push(rows.len());
                        rows.push(hit_row(at, &hit, at == cursor, &needle, theme, cx));
                        at += 1;
                    }
                }
                (None, rows)
            }
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
            // A dead end is the place to say what else the box takes: the
            // one thing nobody discovers on their own.
            Suggest::Search(_) => {
                "Nothing found. A tool's name runs it \u{2014} ping 1.1.1.1 \u{2014} and > lists what ntls can do."
            }
            Suggest::Params { .. } => "No setting by that name",
            Suggest::Values { .. } => "Type any value",
        };
        // What the keys do, which is how the second and third things this box
        // can do get found at all.
        let footing: Vec<(&'static str, &'static str)> = match &suggestion {
            Suggest::Search(_) if needle.is_empty() => {
                vec![("\u{21c5}", "move"), ("\u{23ce}", "open"), (">", "commands")]
            }
            Suggest::Search(_) => {
                vec![("\u{21c5}", "move"), ("\u{23ce}", "open"), ("\u{21e5}", "complete")]
            }
            Suggest::Params { .. } | Suggest::Values { .. } => {
                vec![("\u{21e5}", "fill in"), ("\u{23ce}", "run it")]
            }
        };

        // A search offers up to forty rows and the card shows about nine of
        // them, so the keyboard walks off the bottom of what is drawn long
        // before it runs out of list: the highlight goes below the fold and
        // Enter then runs a row nobody can see. Move the list to the row the
        // keyboard is on.
        if placed.is_empty() {
            placed = (0..rows.len()).collect();
        }
        let count = placed.len();
        let scroll = LIST.with(|list| {
            if let Some(at) = scroll_row(list.shown.get(), cursor, count) {
                // The heading above it, when there is one, so a group's
                // first answer is not scrolled to with its own name off the
                // top of the view.
                let row = placed[at];
                list.scroll.scroll_to_item(row.saturating_sub(usize::from(row > 0 && at == 0)));
            }
            if count > 0 {
                list.shown.set(Some(cursor.min(count - 1)));
            }
            list.scroll.clone()
        });

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
                            .track_scroll(&scroll)
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
                    // A line of keys along the bottom. The box is three
                    // things at once — a search, a command line and a list of
                    // what the application can do — and only the first is
                    // obvious from looking at it.
                    .child(super::rule(theme))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::ROOMY))
                            .flex_shrink_0()
                            .px(px(12.))
                            .h(px(28.))
                            .children(footing.into_iter().map(|(key, what)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::TIGHT))
                                    .child(keycap(key, theme))
                                    .child(
                                        div()
                                            .text_meta()
                                            .text_color(theme.faint)
                                            .child(what),
                                    )
                            }))
                            .child(div().flex_1())
                            .children((count > 0).then(|| {
                                div().text_meta().text_color(theme.faint).child(format!(
                                    "{count} answer{}",
                                    if count == 1 { "" } else { "s" }
                                ))
                            })),
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

/// Where the suggestion list is scrolled to, and the row it was put there
/// for. Both have to outlive the frame that drew them: a scroll position
/// that is forgotten is a list that starts at the top again every frame, and
/// a row that is forgotten is a list that is dragged back to the keyboard
/// the instant anyone touches the wheel.
///
/// There is one palette, drawn over the whole window, so one of these serves
/// it.
struct List {
    scroll: gpui::ScrollHandle,
    shown: std::cell::Cell<Option<usize>>,
}

std::thread_local! {
    static LIST: List = List {
        scroll: gpui::ScrollHandle::new(),
        shown: std::cell::Cell::new(None),
    };
}

/// The row the list has to be moved to, if any: the row the keyboard is on,
/// once that is somewhere other than where the list was last put for it.
///
/// Asking on every frame instead would take the list away from anyone
/// scrolling it by hand, since the wheel would be undone as soon as the
/// keyboard's row left the view.
fn scroll_row(shown: Option<usize>, cursor: usize, rows: usize) -> Option<usize> {
    // An empty list draws one line of apology and has no row to move to.
    let last = rows.checked_sub(1)?;
    let row = cursor.min(last);
    // Nothing has been drawn yet, so the list is at the top with the cursor
    // on the first row and is already where it should be. Asking now, before
    // the rows have been laid out, would scroll against bounds that are
    // still empty and leave the list at some arbitrary offset.
    (shown? != row).then_some(row)
}

/// The heading over one kind of answer.
fn group_heading(kind: &'static str, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .h(px(22.))
        .px(px(9.))
        .pt(px(4.))
        .text_caps()
        .text_color(theme.faint)
        .child(kind)
        .into_any_element()
}

/// A label with the characters that answered the query picked out.
///
/// With a list this mixed, a row matched three words in is otherwise
/// indistinguishable from one matched at the start, and a fuzzy match looks
/// like no match at all.
fn lit(
    label: &str,
    needle: &str,
    mono: bool,
    theme: &Theme,
) -> gpui::AnyElement {
    let found = (!needle.is_empty())
        .then(|| crate::ui::search::hit_of(&label.to_lowercase(), needle))
        .flatten();
    let highlights: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = found
        .map(|m| m.at)
        .unwrap_or_default()
        .into_iter()
        // One range per character: the matched positions may be scattered.
        .filter_map(|at| {
            let end = label[at..].chars().next()?.len_utf8() + at;
            Some((
                at..end,
                gpui::HighlightStyle {
                    color: Some(theme.accent),
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            ))
        })
        .collect();

    div()
        .flex_1()
        .min_w_0()
        .when(mono, |d| d.mono())
        .text_small()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text)
        .overflow_hidden()
        .whitespace_nowrap()
        .child(gpui::StyledText::new(label.to_string()).with_highlights(highlights))
        .into_any_element()
}

/// A row in the search results: what it is, what it is called, and where it
/// lives. One shape for tools, workspaces, open tools, result rows and
/// interfaces, because the list mixes them.
fn hit_row(
    index: usize,
    hit: &Hit,
    on_cursor: bool,
    needle: &str,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let colour = match hit {
        Hit::Row { status, .. } => theme.status(*status),
        _ => theme.accent,
    };
    let hit = hit.clone();
    let (label, context) = (hit.label(), hit.context());
    let mono = matches!(hit, Hit::Row { .. } | Hit::Iface { .. });
    // A command already says what it does; the rest of the row says where it
    // is, and the keys that also do it go on the right in a keycap.
    let keys = match &hit {
        Hit::Command(c) => c.keys,
        _ => None,
    };

    row_shell(format!("pal-hit-{index}"), on_cursor, theme)
        .child(icon(hit.icon(), px(15.), colour))
        .child(lit(&label, needle, mono, theme))
        .child(
            div()
                .max_w(px(190.))
                .flex_shrink_0()
                .text_small()
                .text_color(theme.dim)
                .truncate()
                .child(context),
        )
        .children(keys.map(|keys| keycap(keys, theme)))
        .when(on_cursor && keys.is_none(), |d| d.child(keycap("⏎", theme)))
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

#[cfg(test)]
mod tests {
    use super::scroll_row;

    #[test]
    fn walking_the_keyboard_below_the_visible_rows_moves_the_list_to_it() {
        // Forty search hits and room for about nine of them: the twentieth
        // row is drawn, but under the bottom edge of the card.
        assert_eq!(scroll_row(Some(19), 20, 40), Some(20));
    }

    #[test]
    fn wrapping_from_the_first_row_round_to_the_last_moves_the_list_to_it() {
        // Up from the top row is the one jump that always lands out of sight.
        assert_eq!(scroll_row(Some(0), 39, 40), Some(39));
    }

    #[test]
    fn a_keyboard_that_has_not_moved_leaves_the_list_where_it_is() {
        assert_eq!(scroll_row(Some(7), 7, 40), None);
    }

    #[test]
    fn the_first_frame_leaves_the_list_where_it_is() {
        assert_eq!(scroll_row(None, 0, 40), None);
    }

    #[test]
    fn a_list_with_nothing_in_it_has_no_row_to_move_to() {
        assert_eq!(scroll_row(Some(3), 0, 0), None);
    }

    #[test]
    fn a_cursor_past_the_end_of_a_shortened_list_moves_the_list_to_its_last_row() {
        // The count the keyboard was bounded by and the rows now drawn can
        // disagree for a frame while a query is being typed.
        assert_eq!(scroll_row(Some(0), 12, 3), Some(2));
    }
}
