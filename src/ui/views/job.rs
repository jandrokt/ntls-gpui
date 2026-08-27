//! One tool: what it is set to, what it is doing, and what it found.

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
    uniform_list,
};

use crate::core::{Column, FieldKind, Row};
use crate::net::iface;
use crate::ui::app::App;
use crate::ui::chart::{chart, spark};
use crate::ui::icons::icon;
use crate::ui::job::Job;
use crate::ui::store::Tag;
use crate::ui::theme::Theme;
use crate::ui::widgets::{
    Kind, Type, button, dot, figure, icon_button, pill, progress_bar, space, text,
};

use super::chrome::status_pill;
use super::rule;

const ROW_HEIGHT: Pixels = px(26.);
/// The gutter every part of the pane lines up against.
const GUTTER: f32 = space::GUTTER;
/// How far a table row is inset inside that gutter, so its highlight has air
/// around it without the text moving.
const ROW_INSET: f32 = 3.;
/// One character of the table's monospace face, near enough for column
/// widths. Cells are clipped rather than wrapped, so a small error costs a
/// little padding and nothing else.
const CHAR_W: f32 = 7.3;

impl App {
    pub(super) fn job_pane(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(job) = self.selected_job() else {
            return self.nothing_selected(theme).into_any_element();
        };
        let show_form = job.show_form;
        let expanded_chart = job.chart_expanded && !job.charts.is_empty();
        let show_table = !(show_form && job.state == crate::ui::job::State::Setup);
        let columns = job.tool.columns();

        let comparison = self.comparison(theme, cx);
        let header = self.job_header(theme, cx);
        let form = show_form.then(|| self.form(theme, window, cx)).flatten();
        let summary = self.summary_strip(theme);
        let charts = expanded_chart.then(|| self.charts(theme)).flatten();
        let show_table = show_table && comparison.is_none();
        let table = show_table.then(|| self.table(columns, theme, cx));
        let handoff = show_table.then(|| self.handoff_bar(theme, cx)).flatten();

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
            .child(header)
            .children(form)
            .children(summary)
            .children(charts)
            .children(comparison)
            .children(table)
            .children(handoff)
            // With no table to take the slack, the form floats at the top
            // rather than stretching down the pane.
            .when(!show_table, |d| d.child(div().flex_1()))
            .into_any_element()
    }

    fn nothing_selected(&self, theme: &Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .bg(theme.bg)
            .text_small()
            .text_color(theme.faint)
            .child("No tool selected")
    }

    fn job_header(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(job) = self.selected_job() else { return div().into_any_element() };
        let (running, state) = (job.state.is_running(), job.state.clone());
        let (tool_icon, title, target) =
            (job.tool.icon(), job.name(), job.target());
        let (tag, favorite) = (job.tag, job.favorite);
        let renaming = self.renaming;
        let rename_input = self.rename_input.clone();
        if renaming {
            rename_input.update(cx, |input, _| {
                input.mono = false;
                input.text_color = theme.text;
                input.placeholder_color = theme.faint;
                input.caret_color = theme.accent;
                input.selection_color = theme.accent.opacity(0.28);
            });
        }

        let elapsed = running.then(|| job.elapsed()).flatten().map(crate::tools::stats::elapsed);
        let show_form = job.show_form;
        let panel_open = self.panel_open;
        let (has_charts, chart_expanded) = (!job.charts.is_empty(), job.chart_expanded);
        let failure = match &job.state {
            crate::ui::job::State::Failed(e) => Some(e.clone()),
            _ => None,
        };

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SNUG))
                    .px(px(GUTTER))
                    .h(px(52.))
                    .child(icon(tool_icon, px(17.), tag.color().unwrap_or(theme.accent)))
                    .when(renaming, |d| {
                        // Naming a run and colouring it are one act, so the
                        // box, the colours and the way out sit together where
                        // the name was — not in a second row with the button
                        // that ends it pushed to the far side of the window.
                        d.child(
                            div()
                                .id("rename-group")
                                // Clicking away keeps the name and closes the
                                // box, which is what every other editor does.
                                .on_mouse_down_out(cx.listener(|app, _, window, cx| {
                                    app.end_rename(window, cx)
                                }))
                                .flex()
                                .items_center()
                                .gap(px(space::SNUG))
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .w_full()
                                        .max_w(px(300.))
                                        .h(px(28.))
                                        .px(px(space::SNUG))
                                        .rounded(px(6.))
                                        .bg(theme.bg)
                                        .border_1()
                                        .border_color(theme.focus)
                                        .text_body()
                                        .child(rename_input.clone()),
                                )
                                .children(Tag::ALL.into_iter().map(|t| {
                                    swatch(t, t == tag, "run", theme, cx)
                                }))
                                .child(
                                    button("rename-done", "Done", Kind::Primary, theme).on_click(
                                        cx.listener(|app, _, window, cx| {
                                            app.end_rename(window, cx)
                                        }),
                                    ),
                                )
                                .child(div().flex_1()),
                        )
                    })
                    .when(!renaming, |d| {
                        d.child(
                        // The name is editable in place: clicking it is how a
                        // run gets called something other than its tool.
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child({
                                div()
                                    .id("rename-open")
                                    .flex()
                                    .items_center()
                                    .gap(px(space::TIGHT))
                                    .h(px(20.))
                                    .text_heading()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .whitespace_nowrap()
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme.accent))
                                    .child(title)
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.begin_rename(window, cx)
                                    }))
                            })
                            .child(
                                // A second line, so the address keeps its
                                // monospace without sharing a baseline with
                                // the name.
                                div()
                                    .mono()
                                    .text_meta()
                                    .text_color(theme.faint)
                                    .truncate()
                                    .child(target),
                            ),
                        )
                    })
                    .children(elapsed.map(|e| {
                        div()
                            .text_meta()
                            .text_color(theme.faint)
                            .whitespace_nowrap()
                            .child(e)
                    }))
                    .child(status_pill(&state, theme))
                    .child(
                        icon_button(
                            "favorite",
                            if favorite { "star-filled" } else { "star" },
                            favorite,
                            theme,
                        )
                        .on_click(cx.listener(|app, _, _, cx| app.toggle_favorite(cx))),
                    )
                    .child(
                        // Words, not symbols. A row of small glyphs is a
                        // puzzle; three short labels are read at a glance.
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(
                                button("toggle-chart", "Graph", Kind::Toggle(chart_expanded), theme)
                                    .when(!has_charts, |d| d.opacity(0.3))
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        if let Some(job) = app.selected_job_mut() {
                                            job.chart_expanded = !job.chart_expanded;
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                button("toggle-log", "Output", Kind::Toggle(panel_open), theme)
                                    .on_click(cx.listener(|app, _, _, cx| app.toggle_panel(cx))),
                            )
                            .child(
                                button("toggle-form", "Settings", Kind::Toggle(show_form), theme)
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        if let Some(job) = app.selected_job_mut() {
                                            job.show_form = !job.show_form;
                                        }
                                        app.focus_form(window, cx);
                                    })),
                            ),
                    )
                    .children(self.run_controls(theme, cx)),
            )
            .children(failure.map(|e| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(GUTTER))
                    .py(px(7.))
                    .bg(theme.down_soft)
                    .child(
                        div()
                            .text_small()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.down)
                            .child("Failed"),
                    )
                    .child(
                        div()
                            .text_small()
                            .text_color(theme.dim)
                            .min_w_0()
                            .truncate()
                            .child(e),
                    )
            }))
            .child(rule(theme))
            .into_any_element()
    }

    /// Run, or — for a tool that was interrupted with results already in hand
    /// — resume where it stopped, or start over.
    fn run_controls(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(job) = self.selected_job() else { return Vec::new() };
        if job.state.is_running() {
            return vec![
                button("stop", "Stop", Kind::Danger, theme)
                    .on_click(cx.listener(|app, _, _, cx| app.stop_selected(cx)))
                    .into_any_element(),
            ];
        }

        // Resuming only means anything for a tool that can be told what it
        // already covered — and only while there is something left. A run
        // whose own progress says it finished has nothing to carry on with,
        // so it offers to start again instead.
        let finished = matches!(job.progress, Some((done, total, _)) if total > 0 && done >= total);
        if job.tool.resumable()
            && !job.rows.is_empty()
            && !finished
            && job.state != crate::ui::job::State::Setup
        {
            return vec![
                button("restart", "Restart", Kind::Normal, theme)
                    .on_click(cx.listener(|app, _, window, cx| app.restart_selected(window, cx)))
                    .into_any_element(),
                button("resume", "Resume", Kind::Primary, theme)
                    .on_click(cx.listener(|app, _, window, cx| app.resume_selected(window, cx)))
                    .into_any_element(),
            ];
        }

        let started = job.state != crate::ui::job::State::Setup;
        vec![
            button(
                "run",
                if started { "Run again" } else { "Run" },
                Kind::Primary,
                theme,
            )
            .on_click(cx.listener(|app, _, window, cx| app.run_selected(window, cx)))
            .into_any_element(),
        ]
    }

    /// The settings for a run.
    ///
    /// The target is the question and everything else is a preference, so the
    /// target gets a line of its own at full width and the rest sit in a grid
    /// beneath it. The help for a field appears when the field does — under
    /// the caret, or under the mistake.
    fn form(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.collect_params(cx);
        let job = self.selected_job()?;
        let fields = job.tool.fields();
        let error = job.field_error.clone();

        let mut primary: Vec<AnyElement> = Vec::new();
        let mut rest: Vec<AnyElement> = Vec::new();

        for (i, field) in fields.iter().enumerate() {
            let job = self.selected_job()?;
            if !field.visible(&job.params) {
                continue;
            }
            let focused = job
                .inputs
                .get(i)
                .and_then(Option::as_ref)
                .is_some_and(|e| e.read(cx).focus_handle.is_focused(window));
            let bad = error.as_ref().filter(|(fi, _)| *fi == i).map(|(_, m)| m.clone());
            let wide = field.role == crate::core::Role::Target;

            let editor: AnyElement = match field.kind {
                FieldKind::Text => {
                    let Some(Some(input)) = job.inputs.get(i) else { continue };
                    input.update(cx, |input, _| {
                        input.text_color = theme.text;
                        input.placeholder_color = theme.faint;
                        input.caret_color = theme.accent;
                        input.selection_color = theme.accent.opacity(0.28);
                    });
                    div()
                        .flex()
                        .items_center()
                        .h(px(if wide { 34. } else { 28. }))
                        .px(px(space::SNUG))
                        .rounded(px(6.))
                        .bg(theme.raised)
                        .border_1()
                        .border_color(if bad.is_some() {
                            theme.down
                        } else if focused {
                            theme.accent
                        } else {
                            theme.border
                        })
                        .map(|d| if wide { d.text_body() } else { d.text_small() })
                        .child(input.clone())
                        .into_any_element()
                }
                FieldKind::Bool => switch(i, job.params.bool(field.key), theme, cx).into_any_element(),
                FieldKind::Select => {
                    segmented(field, &job.params.str(field.key), theme, cx).into_any_element()
                }
            };

            let hint = bad.clone().or_else(|| {
                focused.then(|| selected_desc(field, self.selected_job())).filter(|s| !s.is_empty())
            });
            let hint_color = if bad.is_some() { theme.down } else { theme.faint };

            let row = div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .when(!wide, |d| d.w(px(232.)))
                .when(wide, |d| d.w_full())
                .child(
                    div()
                        .text_meta()
                        .text_color(theme.dim)
                        .child(field.label),
                )
                .child(editor)
                .child(
                    // The hint has a reserved line, so a field does not jump
                    // when its help appears. It is exactly one line box tall:
                    // anything shorter clips the text it is reserving room
                    // for, which is how a hint turns into an ellipsis.
                    div()
                        .w_full()
                        .h(text::LINE)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_meta()
                        .text_color(hint_color)
                        .child(hint.unwrap_or_default()),
                )
                .into_any_element();

            if wide { primary.push(row) } else { rest.push(row) }
        }

        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .max_h(px(460.))
                .child(
                    div()
                        .id("form")
                        .flex()
                        .flex_col()
                        .gap(px(space::ROOMY))
                        .px(px(GUTTER))
                        .py(px(space::ROOMY))
                        .overflow_y_scroll()
                        .child(div().flex().flex_col().max_w(px(560.)).children(primary))
                        .when(!rest.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_x(px(space::SECTION))
                                    .gap_y(px(space::SNUG))
                                    .children(rest),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::SNUG))
                                .pt(px(space::TIGHT))
                                .children(self.run_controls(theme, cx))
                                .child(
                                    button("form-defaults", "Reset", Kind::Ghost, theme)
                                        .on_click(cx.listener(|app, _, _, cx| app.reset_fields(cx))),
                                ),
                        ),
                )
                .child(rule(theme))
                .into_any_element(),
        )
    }

    /// The toolbar: the figures a tool publishes, the shape of what it is
    /// measuring, and how far along it is.
    fn summary_strip(&mut self, theme: &Theme) -> Option<AnyElement> {
        let job = self.selected_job()?;
        if job.stats.is_empty() && job.progress.is_none() && job.charts.is_empty() {
            return None;
        }

        let figures: Vec<AnyElement> = job
            .stats
            .iter()
            .filter(|kv| !kv.k.is_empty())
            .map(|kv| figure(&kv.k, &kv.v, theme, None))
            .collect();
        // The small graph lives here, next to the numbers. Pressing Graph
        // gives the full one the room below.
        let inline = (!job.chart_expanded)
            .then(|| job.charts.first())
            .flatten()
            .filter(|s| s.values.len() > 1)
            .map(|s| spark(s.values.clone(), theme.accent, px(132.), px(30.)));
        let bar = job
            .progress
            .as_ref()
            .filter(|(_, total, _)| *total > 0)
            .map(|(done, total, label)| progress_bar(*done, *total, label.as_deref(), theme));

        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(space::SNUG))
                .px(px(GUTTER))
                .py(px(space::ROOMY))
                .flex_shrink_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SECTION))
                        .child(div().flex().flex_wrap().gap(px(space::SECTION)).children(figures))
                        .child(div().flex_1())
                        .children(inline),
                )
                .children(bar)
                .child(rule(theme).mt(px(2.)))
                .into_any_element(),
        )
    }

    fn charts(&mut self, theme: &Theme) -> Option<AnyElement> {
        let job = self.selected_job()?;
        if job.charts.is_empty() {
            return None;
        }
        let height = if job.charts.len() > 1 { px(150.) } else { px(260.) };
        let charts: Vec<AnyElement> = job
            .charts
            .iter()
            .map(|s| div().flex_1().min_w_0().child(chart(s, theme, height)).into_any_element())
            .collect();

        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .child(
                    div()
                        .flex()
                        .gap(px(space::SECTION))
                        .px(px(GUTTER))
                        .pb(px(space::ROOMY))
                        .children(charts),
                )
                .child(rule(theme))
                .into_any_element(),
        )
    }

    fn table(&mut self, columns: Vec<Column>, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        // The note column is always there. A note you can only add through a
        // separate box is a note nobody adds, so the column is a place to
        // type rather than a place notes appear once they exist.
        let notes = self.workspace().notes.clone();
        // Whichever row is this machine gets said so plainly, rather than
        // hidden in a sentence at the end of the line.
        let local: Vec<String> = iface::local_addrs().iter().map(|a| a.to_string()).collect();

        let Some(job) = self.selected_job_mut() else { return div().flex_1().into_any_element() };
        let widths = column_widths(&columns, &job.content_width, &job.column_override);
        let total = job.rows.len();
        let count = job.view_len();
        let sort = job.sort;
        let running = job.state.is_running();
        let filtering = !job.filter.trim().is_empty();
        let never_run = job.state == crate::ui::job::State::Setup;

        job.filter_input.update(cx, |input, _| {
            input.text_color = theme.text;
            input.placeholder_color = theme.faint;
            input.caret_color = theme.accent;
            input.selection_color = theme.accent.opacity(0.28);
        });
        let scroll = job.table_scroll.clone();

        let headers: Vec<AnyElement> = columns
            .iter()
            .zip(&widths)
            .enumerate()
            .map(|(i, (c, w))| {
                let sorted = matches!(sort, Some((s, _)) if s == i);
                let arrow = match sort {
                    Some((s, false)) if s == i => " ↑",
                    Some((s, true)) if s == i => " ↓",
                    _ => "",
                };
                let current = w.map(f32::from).unwrap_or(220.);
                let cell = div()
                    .id(SharedString::from(format!("head-{i}")))
                    .relative()
                    .flex()
                    .items_center()
                    .h(px(18.))
                    .child(
                        div()
                            .id(SharedString::from(format!("head-label-{i}")))
                            .flex_1()
                            .min_w_0()
                            .px(px(7.))
                            .text_caps()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if sorted { theme.accent } else { theme.faint })
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.accent))
                            .child(format!("{}{arrow}", c.title))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if let Some(job) = app.selected_job_mut() {
                                    job.set_sort(i);
                                    cx.notify();
                                }
                            })),
                    )
                    // The grip sits on the column's right edge. Dragging it
                    // sets a width; double-clicking hands the column back to
                    // whatever is in it.
                    .child(
                        div()
                            .id(SharedString::from(format!("grip-{i}")))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(-3.))
                            .w(px(7.))
                            .cursor(gpui::CursorStyle::ResizeLeftRight)
                            .hover(|s| s.bg(theme.accent.opacity(0.4)))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                                    if e.click_count >= 2 {
                                        app.reset_column(i, cx);
                                    } else {
                                        app.begin_resize(i, f32::from(e.position.x), current);
                                    }
                                    cx.stop_propagation();
                                }),
                            ),
                    );
                match w {
                    Some(w) => cell.w(*w).flex_shrink_0().into_any_element(),
                    None => cell.flex_1().min_w_0().into_any_element(),
                }
            })
            .collect();

        let theme_copy = *theme;
        let kinds: Vec<crate::core::Cells> = columns.iter().map(|c| c.cells).collect();
        self.note_input.update(cx, |input, _| {
            input.mono = false;
            input.text_color = theme.text;
            input.placeholder_color = theme.faint;
            input.caret_color = theme.accent;
            input.selection_color = theme.accent.opacity(0.28);
        });
        let row_style = RowStyle {
            widths: widths.clone(),
            kinds,
            notes,
            local,
            note_input: self.note_input.clone(),
            editing_note: self.editing_note,
        };
        let list = uniform_list(
            "results",
            count,
            cx.processor(move |app: &mut App, range: std::ops::Range<usize>, _, cx| {
                let Some(job) = app.selected_job_mut() else { return Vec::new() };
                let view: Vec<usize> = job.view().to_vec();
                let selected = job.selected;
                let rows: Vec<(usize, Row)> = range
                    .filter_map(|i| Some((i, job.rows.get(*view.get(i)?)?.clone())))
                    .collect();
                rows.into_iter()
                    .map(|(i, row)| {
                        table_row(i, row, selected == Some(i), &row_style, &theme_copy, cx)
                    })
                    .collect()
            }),
        )
        .track_scroll(scroll);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.filter_row(
                count,
                total,
                theme,
                cx,
                match sort {
                    Some(_) => vec![
                        button("clear-sort", "Clear sort", Kind::Ghost, theme)
                            .on_click(cx.listener(|app, _, _, cx| {
                                if let Some(job) = app.selected_job_mut() {
                                    job.sort = None;
                                    job.set_filter(job.filter.clone());
                                    cx.notify();
                                }
                            }))
                            .into_any_element(),
                    ],
                    None => Vec::new(),
                },
            ))
            .child(
                div()
                    .flex()
                    .px(px(GUTTER))
                    .pb(px(6.))
                    .flex_shrink_0()
                    .children(headers)
                    .when(true, |d| {
                        d.child(
                            div()
                                .w(NOTE_WIDTH)
                                .flex_shrink_0()
                                .px(px(7.))
                                .text_caps()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.faint)
                                .child("NOTE"),
                        )
                    }),
            )
            .child(rule(theme))
            .child(if count == 0 {
                self.empty_table(running, filtering, never_run, theme, cx)
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .px(px(GUTTER - ROW_INSET))
                    .pt(px(3.))
                    .child(list.h_full())
                    .into_any_element()
            })
            .into_any_element()
    }

    fn empty_table(
        &self,
        running: bool,
        filtering: bool,
        never_run: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (line, action) = match (running, filtering, never_run) {
            (true, _, _) => ("Running", None),
            (_, true, _) => ("No matches", None),
            (_, _, true) => ("Not run yet", Some(())),
            _ => ("No results", None),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .child(div().text_small().text_color(theme.faint).child(line))
            .children(action.map(|_| {
                button("empty-run", "Run", Kind::Primary, theme)
                    .on_click(cx.listener(|app, _, window, cx| app.run_selected(window, cx)))
            }))
            .into_any_element()
    }

    /// The row of tools a selected result can be sent to. Enter sends it to
    /// the first one, which is the move worth knowing.
    /// The row above a table: the filter box, a count, and whatever the view
    /// puts beside them.
    fn filter_row(
        &mut self,
        count: usize,
        total: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
        trailing: Vec<AnyElement>,
    ) -> AnyElement {
        let Some(job) = self.selected_job_mut() else { return div().into_any_element() };
        let filtering = !job.filter.trim().is_empty();
        let filter_input = job.filter_input.clone();
        filter_input.update(cx, |input, _| {
            input.text_color = theme.text;
            input.placeholder_color = theme.faint;
            input.caret_color = theme.accent;
            input.selection_color = theme.accent.opacity(0.28);
        });

        div()
            .flex()
            .items_center()
            .gap(px(space::ROOMY))
            .px(px(GUTTER))
            .py(px(space::SNUG))
            .flex_shrink_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::TIGHT))
                    .h(px(26.))
                    .px(px(space::SNUG))
                    .w(px(240.))
                    .rounded(px(6.))
                    .bg(theme.raised)
                    .border_1()
                    .border_color(theme.border)
                    .text_small()
                    .child(icon("search", px(12.), theme.faint))
                    .child(div().flex_1().min_w_0().child(filter_input)),
            )
            .child(
                div().text_small().text_color(theme.faint).flex_shrink_0().child(if filtering {
                    format!("{count} of {total}")
                } else {
                    format!("{total} rows")
                }),
            )
            .child(div().flex_1())
            .children(trailing)
            .into_any_element()
    }

    /// One run read against another, in place of the table.
    ///
    /// The rows are matched on their targets, so what comes out is what
    /// appeared, what went, and what changed — which is the question a scan
    /// run twice is asking.
    fn comparison(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let job = self.selected_job()?;
        let (id, other_id) = (job.id, job.compare_with?);
        let columns = job.tool.columns();
        let after = job.rows.clone();
        let other = self.workspace().job(other_id)?;
        let (other_name, before) = (other.name(), other.rows.clone());

        let needle = job.filter.trim().to_lowercase();
        let mut diff = crate::ui::diff::compare(&before, &after, &columns);
        let total = diff.entries.len();
        if !needle.is_empty() {
            diff.entries.retain(|e| {
                e.target.to_lowercase().contains(&needle)
                    || e.cells.iter().any(|c| c.to_lowercase().contains(&needle))
                    || e.was.iter().any(|(_, w)| w.to_lowercase().contains(&needle))
                    || e.change.label().contains(&needle)
            });
        }
        let shown = diff.entries.len();
        // A changed cell shows both values side by side, so the column has to
        // be wide enough for the pair.
        let mut content = vec![0usize; columns.len()];
        for entry in &diff.entries {
            for (i, cell) in entry.cells.iter().enumerate().take(content.len()) {
                let was = entry.was.iter().find(|(at, _)| *at == i).map(|(_, w)| w.chars().count());
                let wanted = cell.chars().count() + was.map_or(0, |n| n + 2);
                content[i] = content[i].max(wanted);
            }
        }
        let widths = column_widths(&columns, &content, &[]);
        let summary = diff.summary();

        let headers: Vec<AnyElement> = columns
            .iter()
            .zip(&widths)
            .map(|(column, width)| {
                let cell = div()
                    .px(px(7.))
                    .text_caps()
                    .text_color(theme.faint)
                    .whitespace_nowrap()
                    .child(column.title);
                match width {
                    Some(w) => cell.w(*w).flex_shrink_0().into_any_element(),
                    None => cell.flex_1().min_w_0().into_any_element(),
                }
            })
            .collect();

        let rows: Vec<AnyElement> = diff
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| diff_row(i, entry, &widths, theme))
            .collect();

        Some(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(rule(theme))
                // What is being compared, and what it adds up to.
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::TIGHT))
                        .px(px(GUTTER))
                        .pt(px(space::SNUG))
                        .flex_shrink_0()
                        .child(icon("compare", px(14.), theme.accent))
                        .child(
                            div()
                                .text_small()
                                .text_color(theme.dim)
                                .flex_shrink_0()
                                .child("Compared with"),
                        )
                        .child(
                            div()
                                .max_w(px(220.))
                                .text_small()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .truncate()
                                .child(other_name),
                        )
                        .child(
                            div()
                                .w(px(1.))
                                .h(px(14.))
                                .mx(px(space::TIGHT))
                                .flex_shrink_0()
                                .bg(theme.rule),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_small()
                                .text_color(theme.faint)
                                .truncate()
                                .child(summary),
                        ),
                )
                .child(self.filter_row(
                    shown,
                    total,
                    theme,
                    cx,
                    vec![
                        button("stop-comparing", "Stop comparing", Kind::Normal, theme)
                            .on_click(
                                cx.listener(move |app, _, _, cx| app.compare_job(id, None, cx)),
                            )
                            .into_any_element(),
                    ],
                ))
                .child(
                    div()
                        .flex()
                        .px(px(GUTTER))
                        .pb(px(6.))
                        .flex_shrink_0()
                        .child(
                            div()
                                .w(px(76.))
                                .flex_shrink_0()
                                .px(px(7.))
                                .text_caps()
                                .text_color(theme.faint)
                                .child("CHANGE"),
                        )
                        .children(headers),
                )
                .child(
                    div()
                        .id("comparison")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .px(px(GUTTER - ROW_INSET))
                        .overflow_y_scroll()
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    /// The bar under the table: what is selected, and where it can be sent.
    ///
    /// A result is rarely the end of a question — a host from a sweep goes
    /// into a port scan, a name from a certificate log goes into a lookup — so
    /// the row you are on carries the tools it can become.
    fn handoff_bar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let job = self.selected_job_mut()?;
        let from = job.tool.id();
        let row = job.selected_row()?;
        let (target, status) = (row.target.clone(), row.status);
        let row_note = row.note.clone();
        if target.is_empty() {
            return None;
        }
        let note = row_note.or_else(|| self.workspace().note(&target).map(str::to_string));

        let destinations: Vec<AnyElement> = self
            .handoff_targets(from)
            .into_iter()
            .map(|t| {
                let (id, title, glyph) = (t.id(), t.title(), t.icon());
                div()
                    .id(SharedString::from(format!("handoff-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(space::TIGHT))
                    .h(px(26.))
                    .px(px(space::SNUG))
                    .flex_shrink_0()
                    .rounded(px(6.))
                    .bg(theme.raised)
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.accent_soft).border_color(theme.accent.opacity(0.5)))
                    .child(icon(glyph, px(13.), theme.accent))
                    .child(div().text_small().text_color(theme.text).child(title))
                    .on_click(cx.listener(move |app, _, window, cx| app.handoff(id, window, cx)))
                    .into_any_element()
            })
            .collect();

        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .child(rule(theme))
                .child(
                    div()
                        .id("handoff")
                        .flex()
                        .items_center()
                        .gap(px(space::ROOMY))
                        .px(px(GUTTER))
                        .py(px(space::SNUG))
                        .overflow_x_scroll()
                        // What is selected, stated once and read as one thing.
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::TIGHT))
                                .flex_shrink_0()
                                .child(dot(theme.status(status)))
                                .child(
                                    div()
                                        .mono()
                                        .text_small()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.text)
                                        .child(target),
                                )
                                .children(note.map(|note| {
                                    div()
                                        .max_w(px(220.))
                                        .text_small()
                                        .text_color(theme.faint)
                                        .truncate()
                                        .child(note)
                                })),
                        )
                        .child(
                            div()
                                .w(px(1.))
                                .h(px(18.))
                                .flex_shrink_0()
                                .bg(theme.rule),
                        )
                        .child(
                            div()
                                .text_small()
                                .text_color(theme.faint)
                                .flex_shrink_0()
                                .child("Send to"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::TIGHT))
                                .children(destinations),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The panel under the editor: what the run had to say, and how far
    /// through it is.
    ///
    /// It is one panel rather than one per tool, the way an editor has one
    /// terminal drawer: whatever is in front of you is what it is showing.
    pub(super) fn panel(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let job = self.selected_job()?;
        let (level_counts, running) = (job.log_counts(), job.state.is_running());
        let lines: Vec<AnyElement> = job
            .log
            .iter()
            .map(|l| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .mono()
                            .text_meta()
                            .text_color(theme.faint)
                            .w(px(44.))
                            .flex_shrink_0()
                            .text_right()
                            .child(format!("{:.1}s", l.at.as_secs_f64())),
                    )
                    .child(
                        div()
                            .mono()
                            .text_meta()
                            .text_color(theme.level(l.level))
                            .child(l.text.clone()),
                    )
                    .into_any_element()
            })
            .collect();

        Some(
            div()
                .flex()
                .flex_col()
                .h(px(196.))
                .flex_shrink_0()
                .bg(theme.panel)
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::ROOMY))
                        .h(px(30.))
                        .px(px(GUTTER))
                        .flex_shrink_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::TIGHT))
                                .child(div().text_caps().text_color(theme.text).child("OUTPUT")),
                        )
                        .when(level_counts.1 > 0, |d| {
                            d.child(
                                div()
                                    .text_meta()
                                    .text_color(theme.warn)
                                    .child(format!("{} warning(s)", level_counts.1)),
                            )
                        })
                        .when(level_counts.2 > 0, |d| {
                            d.child(
                                div()
                                    .text_meta()
                                    .text_color(theme.down)
                                    .child(format!("{} error(s)", level_counts.2)),
                            )
                        })
                        .child(div().flex_1())
                        .when(running, |d| {
                            d.child(div().text_meta().text_color(theme.accent).child("running"))
                        })
                        .child(
                            div()
                                .id("panel-close")
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(20.))
                                .rounded(px(4.))
                                .text_meta()
                                .text_color(theme.faint)
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.hover).text_color(theme.text))
                                .child("×")
                                .on_click(cx.listener(|app, _, _, cx| app.toggle_panel(cx))),
                        ),
                )
                .child(
                    div()
                        .id("log")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .px(px(GUTTER))
                        .pb(px(space::SNUG))
                        .overflow_y_scroll()
                        .track_scroll(&self.selected_job().expect("a job").log_scroll)
                        .children(if lines.is_empty() {
                            vec![
                                div()
                                    .text_meta()
                                    .text_color(theme.faint)
                                    .child("No output")
                                    .into_any_element(),
                            ]
                        } else {
                            lines
                        }),
                )
                .into_any_element(),
        )
    }
}

/// A set of mutually exclusive choices, shown all at once. A dropdown hides
/// the options behind a click; there are never more than a handful here, so
/// showing them costs a row and saves the click.
fn segmented(
    field: &crate::core::Field,
    current: &str,
    theme: &Theme,
    cx: &mut Context<App>,
) -> impl IntoElement {
    let options: Vec<AnyElement> = field
        .options
        .iter()
        .map(|o| {
            let selected = o.value == current;
            let (key, value) = (field.key, o.value.clone());
            div()
                .id(SharedString::from(format!("opt-{}-{}", field.key, o.value)))
                .flex()
                .items_center()
                .h(px(22.))
                .px(px(space::SNUG))
                .rounded(px(4.))
                .text_small()
                .whitespace_nowrap()
                .cursor_pointer()
                .when(selected, |d| d.bg(theme.accent).text_color(theme.on_accent))
                .when(!selected, |d| d.text_color(theme.dim).hover(|s| s.bg(theme.hover)))
                .child(o.label.clone())
                .on_click(cx.listener(move |app, _, _, cx| app.set_field(key, &value, cx)))
                .into_any_element()
        })
        .collect();

    div()
        .flex()
        .flex_wrap()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(6.))
        .bg(theme.raised)
        .border_1()
        .border_color(theme.border)
        .children(options)
}

fn switch(field_index: usize, on: bool, theme: &Theme, cx: &mut Context<App>) -> impl IntoElement {
    let (track, knob) = if on {
        (theme.accent, theme.on_accent)
    } else {
        (theme.track, theme.faint)
    };
    div()
        .id(SharedString::from(format!("switch-{field_index}")))
        .flex()
        .items_center()
        .h(px(28.))
        .cursor_pointer()
        .on_click(cx.listener(move |app, _, _, cx| app.cycle_field(field_index, 1, cx)))
        .child(
            div()
                .flex()
                .items_center()
                .w(px(32.))
                .h(px(18.))
                .p(px(2.))
                .rounded_full()
                .bg(track)
                .when(on, |d| d.justify_end())
                .child(div().size(px(14.)).rounded_full().bg(knob)),
        )
}

/// One colour a thing can be tagged with.
pub(super) fn swatch(
    tag: Tag,
    selected: bool,
    scope: &str,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let workspace = scope == "workspace";
    let body = div()
        .id(SharedString::from(format!("tag-{scope}-{}", tag.label())))
        .flex()
        .items_center()
        .justify_center()
        .size(px(18.))
        .rounded_full()
        .cursor_pointer()
        .border_2()
        .border_color(if selected { theme.text } else { gpui::transparent_black() })
        .on_click(cx.listener(move |app, _, _, cx| {
            if workspace {
                let at = app.active;
                app.tag_workspace(at, tag, cx);
            } else {
                app.tag_job(tag, cx);
            }
        }));
    match tag.color() {
        Some(c) => body.child(div().size(px(12.)).rounded_full().bg(c)),
        // "No colour" is a ring, not an absence.
        None => body
            .child(div().size(px(12.)).rounded_full().border_1().border_color(theme.faint)),
    }
    .into_any_element()
}

/// The width the note column takes once there is anything in it.
const NOTE_WIDTH: Pixels = px(200.);

/// What the table needs to know about a row beyond the tool's own cells.
struct RowStyle {
    widths: Vec<Option<Pixels>>,
    /// How each column draws its cells, taken from the tool's own declaration.
    kinds: Vec<crate::core::Cells>,
    notes: std::collections::BTreeMap<String, String>,
    local: Vec<String>,
    /// The one editor, which appears in the note cell of the row being typed
    /// into.
    note_input: gpui::Entity<crate::ui::text_input::TextInput>,
    /// The row whose note is being typed into, if any.
    editing_note: Option<usize>,
}

/// A cell whose value is a fraction of the way done, drawn as a bar with the
/// figure beside it. The tool says "0.42" and the table decides what that
/// looks like.
fn progress_cell(raw: &str, fill: gpui::Hsla, theme: &Theme) -> gpui::Div {
    let fraction = raw.trim().parse::<f32>().unwrap_or(0.0).clamp(0.0, 1.0);
    div()
        .flex()
        .items_center()
        .gap(px(space::SNUG))
        .px(px(7.))
        .child(
            div()
                .flex_1()
                .min_w(px(40.))
                .h(px(5.))
                .rounded_full()
                .bg(theme.track)
                .overflow_hidden()
                .child(
                    div().h_full().w(gpui::relative(fraction)).rounded_full().bg(fill),
                ),
        )
        .child(
            div()
                .w(px(38.))
                .flex_shrink_0()
                .mono()
                .text_small()
                .text_color(theme.dim)
                .text_right()
                .child(format!("{:.0}%", fraction * 100.)),
        )
}

/// One line of a comparison: what became of a target, and what it says now.
fn diff_row(
    index: usize,
    entry: &crate::ui::diff::Entry,
    widths: &[Option<Pixels>],
    theme: &Theme,
) -> AnyElement {
    let colour = theme.status(entry.change.status());
    let cells: Vec<AnyElement> = widths
        .iter()
        .enumerate()
        .map(|(i, width)| {
            let now = entry.cells.get(i).cloned().unwrap_or_default();
            let then = entry.was.iter().find(|(at, _)| *at == i).map(|(_, was)| was.clone());
            let cell = div()
                .flex()
                .items_center()
                .gap(px(space::TIGHT))
                .px(px(7.))
                .mono()
                .text_small()
                .whitespace_nowrap()
                .overflow_hidden()
                // What was there is shown beside what is there now, struck
                // through, so a change reads without opening the other run.
                .children(then.map(|was| {
                    div().text_color(theme.faint).line_through().child(was)
                }))
                .child(
                    div()
                        .text_color(if i == 0 { colour } else { theme.text })
                        .text_ellipsis()
                        .child(now),
                );
            match width {
                Some(w) => cell.w(*w).flex_shrink_0().into_any_element(),
                None => cell.flex_1().min_w_0().into_any_element(),
            }
        })
        .collect();

    div()
        .id(index)
        .flex()
        .w_full()
        .items_center()
        .h(ROW_HEIGHT)
        .px(px(ROW_INSET))
        .rounded(px(5.))
        .child(
            div()
                .w(px(76.))
                .flex_shrink_0()
                .px(px(7.))
                .child(pill(entry.change.label(), colour, theme.status_soft(entry.change.status()))),
        )
        .children(cells)
        // The target is what the two runs were matched on, so it is worth
        // stating for a row whose first cell is something else.
        .child(
            div()
                .max_w(px(180.))
                .flex_shrink_0()
                .px(px(7.))
                .mono()
                .text_small()
                .text_color(theme.faint)
                .truncate()
                .child(entry.target.clone()),
        )
        .into_any_element()
}

fn table_row(
    display_index: usize,
    row: Row,
    selected: bool,
    style: &RowStyle,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let accent = theme.status(row.status);
    // The host part, so a `host:port` target still matches this machine.
    let host = row.target.rsplit_once(':').map_or(row.target.as_str(), |(h, _)| h);
    let is_self = style.local.iter().any(|a| a == host);
    let note = row.note.clone().or_else(|| style.notes.get(&row.target).cloned());
    let first_cell = row.cells.first().cloned().unwrap_or_default();
    let row_target = row.target.clone();
    let editing = selected && style.editing_note == Some(display_index);

    let cells: Vec<AnyElement> = style
        .widths
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let text_value = row.cells.get(i).cloned().unwrap_or_default();
            // The first cell carries the row's status colour; the rest read as
            // ordinary text, so a page of results is not a page of colour.
            let color = if i == 0 { accent } else { theme.text };
            let cell = match style.kinds.get(i) {
                Some(crate::core::Cells::Bar) => {
                    progress_cell(&text_value, if selected { accent } else { theme.accent }, theme)
                }
                _ => div()
                    .px(px(7.))
                    .mono()
                    .text_small()
                    .text_color(color)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(text_value),
            };
            match w {
                Some(w) => cell.w(*w).flex_shrink_0().into_any_element(),
                None => cell.flex_1().min_w_0().into_any_element(),
            }
        })
        .collect();

    div()
        .id(display_index)
        .flex()
        .w_full()
        .items_center()
        .h(ROW_HEIGHT)
        .px(px(ROW_INSET))
        .rounded(px(5.))
        .cursor_pointer()
        .when(selected, |d| d.bg(theme.selected))
        .when(!selected && is_self, |d| d.bg(theme.accent_soft))
        .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
        .on_click(cx.listener(move |app, _, _, cx| {
            if let Some(job) = app.selected_job_mut() {
                job.select_index(display_index);
                cx.notify();
            }
        }))
        // Right-clicking a result selects it first, so the menu is about the
        // row under the pointer rather than about whatever was selected
        // before.
        .on_mouse_down(
            gpui::MouseButton::Right,
            cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                if let Some(job) = app.selected_job_mut() {
                    job.select_index(display_index);
                }
                let (cell, target) = (first_cell.clone(), row_target.clone());
                let from = app.selected_job().map(|j| j.tool.id()).unwrap_or("");
                let sends: Vec<(&'static str, &'static str, &'static str)> = app
                    .handoff_targets(from)
                    .iter()
                    .map(|t| (t.id(), t.title(), t.icon()))
                    .collect();
                app.open_menu(e.position, crate::ui::menu::for_row(cell, target, &sends), cx);
                cx.stop_propagation();
            }),
        )
        .children(cells)
        // The note is edited where it is read: double-click the cell to type
        // into it, and Enter or a click anywhere else finishes.
        .child(if editing {
            div()
                .id(SharedString::from(format!("note-edit-{display_index}")))
                .flex()
                .items_center()
                .w(NOTE_WIDTH)
                .flex_shrink_0()
                .px(px(5.))
                .on_mouse_down_out(cx.listener(|app, _, window, cx| app.end_note(window, cx)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .h(px(20.))
                        .px(px(5.))
                        .rounded(px(4.))
                        .bg(theme.bg)
                        .border_1()
                        .border_color(theme.focus)
                        .text_small()
                        .child(style.note_input.clone()),
                )
                .into_any_element()
        } else {
            div()
                .id(SharedString::from(format!("note-{display_index}")))
                .w(NOTE_WIDTH)
                .flex_shrink_0()
                .px(px(7.))
                .text_small()
                .text_color(if note.is_some() { theme.dim } else { theme.faint.opacity(0.5) })
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .cursor_text()
                .hover(|s| s.text_color(theme.text))
                .child(note.clone().unwrap_or_else(|| "\u{2014}".into()))
                .on_click(cx.listener(move |app, e: &gpui::ClickEvent, window, cx| {
                    if e.click_count() < 2 {
                        return;
                    }
                    app.begin_note(display_index, window, cx);
                    cx.stop_propagation();
                }))
                .into_any_element()
        })
        .when(is_self, |d| {
            d.child(
                div()
                    .pr(px(7.))
                    .flex_shrink_0()
                    .child(pill("this Mac", theme.accent, theme.accent_soft)),
            )
        })
        .into_any_element()
}

/// Assigns a width to every column.
///
/// A declared width is a preference, not a fixed size: a column first shrinks
/// to the widest thing actually in it, so an address column does not reserve
/// room for text that is never there, and a column holding nothing but its
/// heading stays that narrow. The one column declared flexible takes whatever
/// is left, which is where a banner or a hostname gets its room.
fn column_widths(
    columns: &[Column],
    content: &[usize],
    overrides: &[Option<f32>],
) -> Vec<Option<Pixels>> {
    columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            // A width the user dragged to is a width they meant, including on
            // the column that would otherwise flex.
            if let Some(Some(w)) = overrides.get(i) {
                return Some(px(*w));
            }
            if c.width == 0 {
                return None;
            }
            let seen = content.get(i).copied().unwrap_or(0);
            let chars = c.title.chars().count().max(seen.min(c.width)).max(3);
            Some(px(chars as f32 * CHAR_W + 16.0))
        })
        .collect()
}

/// The description of whichever option a select is currently on, which is
/// where the real explanation of a choice lives.
fn selected_desc(field: &crate::core::Field, job: Option<&Job>) -> String {
    let Some(job) = job else { return field.help.to_string() };
    if field.kind != FieldKind::Select {
        return field.help.to_string();
    }
    let current = job.params.str(field.key);
    match field.options.iter().find(|o| o.value == current) {
        Some(o) if !o.desc.is_empty() => format!("{} — {}", field.help, o.desc),
        _ => field.help.to_string(),
    }
}
