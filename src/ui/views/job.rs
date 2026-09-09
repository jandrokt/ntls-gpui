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
use crate::ui::widgets::{bar, 
    Kind, Segment, Segments, Type, button, dot, figure, icon_button, pill, progress_bar, space,
    switch, text,
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
/// widths. Cells are clipped, not wrapped, so a small error costs a
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
        // The answer takes the room the table would have. They are two
        // readings of the same run and stacking them leaves neither enough
        // height to be worth having.
        let answer = self.response(theme, window, cx);
        let has_answer = answer.is_some();
        let show_table = show_table && comparison.is_none() && !has_answer;
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
            .children(answer)
            .children(table)
            .children(handoff)
            // With no table to take the slack, the form floats at the top
            // and does not stretch down the pane.
            .when(!show_table && !has_answer, |d| d.child(div().flex_1()))
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
        // Only the tools that receive a whole answer have one to offer, so
        // the button is not there at all for the ones that never will.
        let has_answer_to_show = job.answer.is_some();
        let showing_response = job.show_response;
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
                        // the name was, not in a second row with the button
                        // that ends it pushed to the far side of the window.
                        d.child(
                            div()
                                .id("rename-group")
                                // Clicking away keeps the name and closes the
                                // box, as every other editor does.
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
                        //
                        // Sized to what it says rather than taking the whole
                        // row, so the star that belongs to it stays beside
                        // it instead of drifting into the middle of the
                        // header. A long name still gives way.
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .max_w(px(420.))
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
                    // Everything but the name, which is not what
                    // renaming is about. Naming a run and colouring it
                    // are one act: the box, the swatches and the way out
                    // take the row to themselves. Leaving the switcher
                    // and the run buttons alongside them put more in the
                    // row than fits, and two things asking for the
                    // leftover space at once, so they overlapped.
                    .when(!renaming, |d| {
                        d
                        .child(
                            icon_button(
                                "favorite",
                                if favorite { "star-filled" } else { "star" },
                                favorite,
                                theme,
                            )
                            .on_click(cx.listener(|app, _, _, cx| app.toggle_favorite(cx))),
                        )
                        // A gap, so the name and the controls are two groups and
                        // not one crowded row.
                        .child(div().flex_1().min_w(px(space::SNUG)))
                        .children(elapsed.map(|e| {
                            div()
                                .mono()
                                .text_meta()
                                .text_color(theme.faint)
                                .whitespace_nowrap()
                                .child(e)
                        }))
                        .child(status_pill(&state, theme))
                        // What is on screen, as one control. Four toggles side
                        // by side read as four loose words; the frame is what
                        // says they are a set with one of them chosen. A tool
                        // that never draws a graph or never receives an answer
                        // has no such choice, so it is not offered one.
                        .child(
                            bar(theme)
                                .when(has_charts, |d| {
                                    d.child(
                                        button(
                                            "toggle-chart",
                                            "Graph",
                                            Kind::Tab(chart_expanded),
                                            theme,
                                        )
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            if let Some(job) = app.selected_job_mut() {
                                                job.chart_expanded = !job.chart_expanded;
                                                cx.notify();
                                            }
                                        })),
                                    )
                                })
                                .when(has_answer_to_show, |d| {
                                    d.child(
                                        button(
                                            "toggle-response",
                                            "Response",
                                            Kind::Tab(showing_response),
                                            theme,
                                        )
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            if let Some(job) = app.selected_job_mut() {
                                                job.show_response = !job.show_response;
                                                // The two cannot both have the
                                                // pane, and the answer is what
                                                // was just asked for.
                                                if job.show_response {
                                                    job.show_form = false;
                                                }
                                            }
                                            cx.notify();
                                        })),
                                    )
                                })
                                .child(
                                    button("toggle-log", "Output", Kind::Tab(panel_open), theme)
                                        .on_click(cx.listener(|app, _, _, cx| app.toggle_panel(cx))),
                                )
                                .child(
                                    button("toggle-form", "Settings", Kind::Tab(show_form), theme)
                                        .on_click(cx.listener(|app, _, window, cx| {
                                            if let Some(job) = app.selected_job_mut() {
                                                job.show_form = !job.show_form;
                                            }
                                            app.focus_form(window, cx);
                                        })),
                                ),
                        )
                        .children(self.run_controls(theme, cx))
                    }),
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

    /// Run, or for a tool interrupted with results already in hand, resume
    /// where it stopped, or start over.
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
        // already covered, and only while there is something left. A run
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
    /// beneath it. The help for a field appears when the field does, under
    /// the caret, or under the mistake.
    ///
    /// A field that asks for a line of its own is laid out where the tool put
    /// it, not gathered with the other wide ones at the top. Gathering them
    /// read fine while the only wide field was the target, but the HTTP tool
    /// has three, in the middle: the body ended up above the Body type that
    /// says what the body is, and the query, headers and credentials were
    /// pushed below the payload. So the narrow fields pack into a wrapped
    /// row, and a wide field closes whichever row is open and takes the next
    /// line.
    ///
    /// Two things keep it from being a wall. The settings most runs never
    /// touch fold away, so what is in front of you is the question and not
    /// the whole of the tool; and the help has one line at the foot of the
    /// form rather than a reserved line under every field, which was costing
    /// a third of the form's height to say nothing at all.
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

        // The form top to bottom, one entry per line of it, and the row of
        // narrow fields still being filled. Twice over: what is always shown,
        // and what is folded away behind "More settings".
        let mut blocks: Vec<AnyElement> = Vec::new();
        let mut packing: Vec<AnyElement> = Vec::new();
        let mut folded: Vec<AnyElement> = Vec::new();
        let mut folding: Vec<AnyElement> = Vec::new();
        // The one line of help, which belongs to whichever field has the
        // caret. A mistake outranks it: that is the thing to fix.
        let mut helping: Option<(String, bool)> = None;
        // Whether anything folded away has been set away from its default,
        // which is what stops the fold from hiding a surprise.
        let mut touched = 0usize;

        for (i, field) in fields.iter().enumerate() {
            let job = self.selected_job()?;
            if !field.visible(&job.params) {
                continue;
            }
            let focused = job
                .inputs
                .get(i)
                .and_then(Option::as_ref)
                .is_some_and(|e| e.read(cx).focus_handle.is_focused(window))
                || job
                    .bodies
                    .get(i)
                    .and_then(Option::as_ref)
                    .is_some_and(|e| e.read(cx).focus_handle.is_focused(window));
            let bad = error.as_ref().filter(|(fi, _)| *fi == i).map(|(_, m)| m.clone());
            // Read while the job is still borrowed; the cell below is built
            // from `self` and cannot hold on to it.
            let away_from_default =
                field.advanced && job.params.str(field.key) != field.default;
            // The target is the question, so it gets a line of its own, and
            // so does anything that says it needs one.
            let wide = field.wide || field.role == crate::core::Role::Target;

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
                FieldKind::Bool => {
                    field_switch(i, job.params.bool(field.key), theme, cx).into_any_element()
                }
                FieldKind::Select => {
                    let current = job.params.str(field.key);
                    segmented(self, field, &current, theme, cx)
                }
                FieldKind::Code => {
                    let Some(Some(body)) = job.bodies.get(i) else { continue };
                    // The language follows whatever the field it takes its
                    // answer from is set to, so switching the body type
                    // recolours what is already typed.
                    let language = crate::ui::app::body_language(field, &job.params);
                    let colours = crate::ui::editor::Colours::of(theme);
                    body.update(cx, |body, cx| {
                        body.colours = colours;
                        if body.language != language {
                            body.language = language;
                            cx.notify();
                        }
                    });
                    div()
                        .id(SharedString::from(format!("body-{i}")))
                        .flex()
                        .flex_col()
                        .h(px(180.))
                        .w_full()
                        .px(px(space::SNUG))
                        .py(px(space::TIGHT))
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
                        .overflow_hidden()
                        .child(body.clone())
                        .into_any_element()
                }
            };

            // The help goes to the one line at the foot of the form. A
            // mistake claims it whether or not the field has the caret,
            // because a form that will not run has to say why somewhere.
            if let Some(why) = bad.clone() {
                helping = Some((why, true));
            } else if focused && helping.as_ref().is_none_or(|(_, bad)| !bad) {
                let help = selected_desc(field, self.selected_job());
                if !help.is_empty() {
                    helping = Some((help, false));
                }
            }

            let row = div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .when(!wide, |d| d.w(px(232.)))
                .when(wide, |d| d.w_full())
                .child(div().text_meta().text_color(theme.dim).child(field.label))
                .child(editor)
                .into_any_element();

            let (into, packing_here) = if field.advanced {
                if away_from_default {
                    touched += 1;
                }
                (&mut folded, &mut folding)
            } else {
                (&mut blocks, &mut packing)
            };
            if wide {
                if !packing_here.is_empty() {
                    into.push(narrow_row(std::mem::take(packing_here)));
                }
                into.push(div().flex().flex_col().max_w(px(560.)).child(row).into_any_element());
            } else {
                packing_here.push(row);
            }
        }
        if !packing.is_empty() {
            blocks.push(narrow_row(packing));
        }
        if !folding.is_empty() {
            folded.push(narrow_row(folding));
        }

        // A fold that hides something set away from its default is a fold
        // that hides a surprise, so it opens itself and says how many.
        let job = self.selected_job()?;
        let open = job.more_open || touched > 0;
        let more = folded.len();
        let (hint, wrong) = helping.map_or((String::new(), false), |(h, bad)| (h, bad));

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
                        .children(blocks)
                        .when(more > 0, |d| {
                            d.child(fold(open, touched, theme, cx)).when(open, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(space::ROOMY))
                                        .children(folded),
                                )
                            })
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
                                )
                                .child(div().flex_1())
                                // One line of help for the whole form,
                                // always in the same place, so nothing has
                                // to reserve room for it.
                                .child(
                                    div()
                                        .min_w_0()
                                        .max_w(px(520.))
                                        .h(text::LINE)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_meta()
                                        .text_color(if wrong { theme.down } else { theme.faint })
                                        .child(hint),
                                ),
                        ),
                )
                .child(rule(theme))
                .into_any_element(),
        )
    }

    /// What the server actually said.
    ///
    /// A row says what happened and the log says a little about it; this is
    /// the answer itself, and it is the thing you open a request tool to
    /// read. JSON and XML are coloured so the shape can be seen at a glance;
    /// anything else is left alone but still monospaced, because an answer is
    /// data and data does not line up in a proportional face.
    fn response(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let job = self.selected_job()?;
        if !job.show_response {
            return None;
        }
        let answer = job.answer.as_ref()?;

        let (headers_shown, raw) = (job.show_headers, job.raw_body);
        let (body, language) = shown(answer, headers_shown, raw);
        // The tool keeps up to eight megabytes of an answer so a value can be
        // captured out of it. Drawing eight megabytes is a different matter:
        // it is one text element with a highlight range per token, hundreds
        // of thousands of them, laid out on the thread that draws the window.
        // What is shown is the part anybody reads, and the strip above says
        // when there is more.
        let (body, clipped) = clip_for_reading(body);

        let colours = crate::ui::editor::Colours::of(theme);
        let highlights: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> =
            crate::ui::syntax::highlight_flat(language, &body)
                .into_iter()
                .filter(|s| !s.range.is_empty())
                .map(|s| {
                    (
                        s.range,
                        gpui::HighlightStyle {
                            color: Some(colours.of_kind(s.kind)),
                            font_weight: Some(crate::ui::editor::Colours::weight_of(s.kind)),
                            ..Default::default()
                        },
                    )
                })
                .collect();

        let ok = (200..400).contains(&answer.status);
        let head = format!("{} {}", answer.status, answer.reason);
        let kind = if answer.content_type.is_empty() {
            "no content type".to_string()
        } else {
            answer.content_type.clone()
        };
        let size = crate::dl::names::bytes(answer.body.len() as u64);
        let lines = body.lines().count();
        let truncated = answer.truncated;
        let headers = answer.headers.len();
        // The two halves of the time it took. A server thinking for a second
        // and a megabyte crawling down a slow link are different problems,
        // and one number for both cannot say which you have.
        let timing = (answer.waited > 0. || answer.read > 0.).then(|| {
            format!(
                "waited {} \u{b7} read {}",
                crate::tools::stats::ms(std::time::Duration::from_secs_f64(
                    answer.waited / 1000.
                )),
                crate::tools::stats::ms(std::time::Duration::from_secs_f64(
                    answer.read / 1000.
                ))
            )
        });
        let landed = (!answer.landed.is_empty()).then(|| answer.landed.clone());
        let asked = (!answer.url.is_empty())
            .then(|| format!("{} {}", answer.method, answer.url));
        let can_copy = answer.body.clone();
        // The button says so for a moment after it worked. Long enough to
        // read, short enough not to be mistaken for a state.
        const SAID_FOR: std::time::Duration = std::time::Duration::from_millis(1400);
        let just_copied = job.copied_at.is_some_and(|at| at.elapsed() < SAID_FOR);
        // The label goes back on its own, which needs a frame to go back on.
        if just_copied {
            window.request_animation_frame();
        }
        let pretty_possible =
            !headers_shown && response_language(&answer.content_type)
                == crate::ui::syntax::Language::Json;

        Some(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
                    // What the answer was, before what it said.
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SNUG))
                        .flex_shrink_0()
                        .px(px(GUTTER))
                        .py(px(space::SNUG))
                        .child(if ok {
                            pill(head.clone(), theme.up, theme.up_soft)
                        } else {
                            pill(head.clone(), theme.down, theme.down_soft)
                        })
                        .child(
                            div()
                                .min_w_0()
                                .text_meta()
                                .text_color(theme.dim)
                                .truncate()
                                .child(format!("{kind} · {size} · {lines} lines")),
                        )
                        .children(timing.map(|timing| {
                            div().text_meta().text_color(theme.dim).child(timing)
                        }))
                        .when(truncated, |d| {
                            d.child(pill("cut short", theme.warn, theme.warn_soft))
                        })
                        .when(clipped, |d| {
                            d.child(pill("showing the first part", theme.warn, theme.warn_soft))
                        })
                        .child(div().flex_1())
                        // Which half of the answer, how it is laid out, and
                        // a way to take it somewhere else.
                        .child(
                            button(
                                "response-body-half",
                                "Body",
                                Kind::Toggle(!headers_shown),
                                theme,
                            )
                            .on_click(cx.listener(|app, _, _, cx| {
                                if let Some(job) = app.selected_job_mut() {
                                    job.show_headers = false;
                                    cx.notify();
                                }
                            })),
                        )
                        .child(
                            button(
                                "response-headers-half",
                                format!("Headers ({headers})"),
                                Kind::Toggle(headers_shown),
                                theme,
                            )
                            .on_click(cx.listener(|app, _, _, cx| {
                                if let Some(job) = app.selected_job_mut() {
                                    job.show_headers = true;
                                    cx.notify();
                                }
                            })),
                        )
                        .when(pretty_possible, |d| {
                            d.child(
                                button("response-raw", "Raw", Kind::Toggle(raw), theme).on_click(
                                    cx.listener(|app, _, _, cx| {
                                        if let Some(job) = app.selected_job_mut() {
                                            job.raw_body = !job.raw_body;
                                            cx.notify();
                                        }
                                    }),
                                ),
                            )
                        })
                        .child(
                            button(
                                "response-copy",
                                if just_copied { "Copied" } else { "Copy" },
                                if just_copied { Kind::Toggle(true) } else { Kind::Ghost },
                                theme,
                            )
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    can_copy.clone(),
                                ));
                                if let Some(job) = app.selected_job_mut() {
                                    job.copied_at = Some(std::time::Instant::now());
                                }
                                cx.notify();
                            })),
                        ),
                )
                // What was asked, and where it ended up when that is not
                // where it was sent. A redirect that lands somewhere else is
                // the answer to a different question than the one typed.
                .children(asked.map(|asked| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SNUG))
                        .flex_shrink_0()
                        .px(px(GUTTER))
                        .pb(px(space::SNUG))
                        .child(
                            div()
                                .min_w_0()
                                .mono()
                                .text_meta()
                                .text_color(theme.faint)
                                .truncate()
                                .child(asked),
                        )
                        .children(landed.map(|landed| {
                            div()
                                .min_w_0()
                                .mono()
                                .text_meta()
                                .text_color(theme.warn)
                                .truncate()
                                .child(format!("\u{2192} {landed}"))
                        }))
                }))
                .child(rule(theme))
                .child(
                    div()
                        .id("response-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_scroll()
                        .px(px(GUTTER))
                        .py(px(space::SNUG))
                        .font_family(crate::ui::theme::MONO)
                        .text_small()
                        .text_color(theme.text)
                        .child(
                            gpui::StyledText::new(body).with_highlights(highlights),
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
        // The note column is always there: a note you could only add through
        // a separate box would not get used, so the column itself is where
        // you type.
        let notes = self.workspace().notes.clone();
        // Whichever row is this machine gets said so plainly, and not
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
                            .on_click(cx.listener(move |app, _, _, cx| app.sort_results(i, cx))),
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
                            .on_click(cx.listener(|app, _, _, cx| app.clear_sort(cx)))
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
    /// the first one, the move worth knowing.
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
    /// appeared, what went, and what changed. That is the question a scan
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
        let layout = diff_columns(&columns, &content);
        // A row lays out the tool's own columns between the change pill and
        // the target; those two have their width from the layout itself.
        let widths: Vec<Option<Pixels>> =
            layout.iter().skip(1).take(columns.len()).map(|(_, w)| *w).collect();
        let summary = diff.summary();

        let headers: Vec<AnyElement> = layout
            .iter()
            .map(|(title, width)| {
                let cell = div()
                    .px(px(7.))
                    .text_caps()
                    .text_color(theme.faint)
                    .whitespace_nowrap()
                    .child(*title);
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
    /// A result is rarely the end of a question. A host from a sweep goes into
    /// a port scan, a name from a certificate log goes into a lookup, so
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
    /// It is one panel instead of one per tool, the way an editor has one
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
    app: &mut App,
    field: &crate::core::Field,
    current: &str,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let at = field.options.iter().position(|o| o.value == current).unwrap_or(0);
    let from = app.segment_from(&format!("field-{}", field.key), at);
    let options = field
        .options
        .iter()
        .map(|o| {
            let (key, value) = (field.key, o.value.clone());
            Segment::new(
                o.label.clone(),
                cx.listener(move |app: &mut App, _, _, cx| app.set_field(key, &value, cx)),
            )
        })
        .collect();
    Segments { id: format!("opt-{}", field.key), options, current: at, from }.render(theme)
}

fn field_switch(
    field_index: usize,
    on: bool,
    theme: &Theme,
    cx: &mut Context<App>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("switch-{field_index}")))
        .flex()
        .items_center()
        .h(px(28.))
        .cursor_pointer()
        .on_click(cx.listener(move |app, _, _, cx| app.cycle_field(field_index, 1, cx)))
        .child(switch(&format!("field-{field_index}"), on, theme))
}

/// What the response pane shows, and how to colour it.
///
/// Three choices in one place: which half of the answer, whether the body is
/// laid out or left as it arrived, and what language that makes it. Minified
/// JSON is one line a thousand characters wide, unreadable and unscrollable
/// at once; laid out it is the shape the service actually sent. Raw is for
/// when the question is about the bytes, and the headers are a list of names
/// and values whatever the body happens to be.
fn shown(
    answer: &crate::core::Answer,
    headers: bool,
    raw: bool,
) -> (String, crate::ui::syntax::Language) {
    use crate::ui::syntax::Language;

    if headers {
        let listed = answer
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");
        return (listed, Language::Text);
    }
    let language = response_language(&answer.content_type);
    match language {
        Language::Json if !raw => (pretty_json(&answer.body), language),
        _ => (answer.body.clone(), language),
    }
}

/// How much of an answer the pane draws.
///
/// Generous enough that a real answer arrives whole, small enough that the
/// window still draws in a frame. An answer larger than this is a file, and
/// the download tool is the one that fetches files.
const READABLE: usize = 256 * 1024;

/// The part of an answer worth drawing, and whether anything was left out.
///
/// Cut on a line boundary where there is one nearby, so the last line shown
/// is a whole line rather than half a token, and on a character boundary
/// always, because slicing a `str` anywhere else is not allowed.
fn clip_for_reading(body: String) -> (String, bool) {
    if body.len() <= READABLE {
        return (body, false);
    }
    let mut end = READABLE;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    let head = &body[..end];
    let end = head.rfind('\n').map_or(end, |at| at + 1);
    (body[..end].to_string(), true)
}

/// Which language an answer is read in, from what the server said it was.
///
/// The content type and not the body: a service that says `application/json`
/// and sends an HTML error page is telling you something, and guessing from
/// the bytes would hide it. HTML is read as XML, which is close enough to see
/// the shape of a page.
fn response_language(content_type: &str) -> crate::ui::syntax::Language {
    use crate::ui::syntax::Language;
    let kind = content_type.to_ascii_lowercase();
    if kind.contains("json") {
        Language::Json
    } else if kind.contains("xml") || kind.contains("html") {
        Language::Xml
    } else {
        Language::Text
    }
}

/// An answer laid out over several lines, when it is JSON.
///
/// Almost nothing sends JSON with newlines in it, and one line a thousand
/// characters wide cannot be read or scrolled. Anything that does not parse is
/// handed back untouched: a truncated body is still worth showing, and so is
/// one that was never JSON whatever its content type claimed.
fn pretty_json(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| body.to_string())
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

/// The line that opens and shuts the settings most runs never touch.
///
/// It says how many are set away from their default, because a fold that
/// quietly holds a changed setting is worse than no fold: the run would do
/// something the visible half of the form does not account for.
fn fold(open: bool, touched: usize, theme: &Theme, cx: &mut Context<App>) -> AnyElement {
    div()
        .id("form-more")
        .flex()
        .items_center()
        .gap(px(space::SNUG))
        .h(px(26.))
        .w(px(232.))
        .px(px(space::SNUG))
        .rounded(px(6.))
        .cursor_pointer()
        .text_meta()
        .text_color(theme.dim)
        .hover(|s| s.bg(theme.hover))
        .child(icon(if open { "chevron-down" } else { "chevron-right" }, px(11.), theme.faint))
        .child("More settings")
        .when(touched > 0, |d| {
            d.child(pill(format!("{touched} changed"), theme.accent, theme.accent_soft))
        })
        .on_click(cx.listener(|app, _, _, cx| {
            if let Some(job) = app.selected_job_mut() {
                job.more_open = !job.more_open;
                cx.notify();
            }
        }))
        .into_any_element()
}

/// One line of the form: the narrow fields that fit side by side on it. They
/// wrap when the window is too tight for the lot, which is why the row is a
/// wrap and not a fixed set of columns.
fn narrow_row(fields: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap_x(px(space::SECTION))
        .gap_y(px(space::SNUG))
        .children(fields)
        .into_any_element()
}

/// What the row for this machine's own address is called, in the words the
/// system it is running on uses for itself.
const SELF_LABEL: &str = if cfg!(target_os = "macos") {
    "this Mac"
} else if cfg!(target_os = "windows") {
    "this PC"
} else {
    "this host"
};

/// The room the marker on that row needs at the right-hand end.
const SELF_BADGE: Pixels = px(72.);

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

/// The width of the change pill's column, and of the target the two runs
/// were matched on at the far end of the row.
const CHANGE_WIDTH: Pixels = px(76.);
const TARGET_WIDTH: Pixels = px(180.);

/// Every column of a comparison, left to right: what became of the target,
/// the tool's own columns, and the target itself.
///
/// The heading row and the rows below it are laid out from this one list.
/// The target used to be a cell no heading knew about, and a cell the heading
/// row does not have takes its width out of the flexible column's share, so
/// every column a tool declares after its flexible one — a sweep's SEEN,
/// after VENDOR — sat well to the left of its own heading, and by a
/// different amount on each row, the target being as wide as whatever
/// address was in it.
fn diff_columns(columns: &[Column], content: &[usize]) -> Vec<(&'static str, Option<Pixels>)> {
    let mut layout = Vec::with_capacity(columns.len() + 2);
    layout.push(("CHANGE", Some(CHANGE_WIDTH)));
    layout.extend(columns.iter().map(|c| c.title).zip(column_widths(columns, content, &[])));
    layout.push(("TARGET", Some(TARGET_WIDTH)));
    layout
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
                .w(CHANGE_WIDTH)
                .flex_shrink_0()
                .px(px(7.))
                .child(pill(entry.change.label(), colour, theme.status_soft(entry.change.status()))),
        )
        .children(cells)
        // The target is what the two runs were matched on, so it is worth
        // stating for a row whose first cell is something else. It takes the
        // same width on every row, headings included: sized to whatever
        // address it held, it moved the columns beside it around as you read
        // down the table.
        .child(
            div()
                .w(TARGET_WIDTH)
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
        // The marker on this machine's own row is drawn over the row rather
        // than added to it: as another child it took its width out of the
        // flexible column, and that one row's columns then lined up with
        // nothing else in the table.
        .relative()
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
        // row under the pointer and not about whatever was selected
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
                // The marker is drawn over the end of this cell, so the box
                // typed into stops short of it and does not run under it.
                .when(is_self, |d| d.pr(SELF_BADGE))
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
                // The marker is drawn over the end of this cell, so the note
                // stops short of it and does not run underneath it. The
                // cell keeps its width either way.
                .when(is_self, |d| d.pr(SELF_BADGE))
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
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(ROW_INSET + 7.))
                    .flex()
                    .items_center()
                    .child(pill(SELF_LABEL, theme.accent, theme.accent_soft)),
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
/// is left, where a banner or a hostname gets its room.
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

/// The description of whichever option a select is currently on, which
/// where the real explanation of a choice lives.
fn selected_desc(field: &crate::core::Field, job: Option<&Job>) -> String {
    let Some(job) = job else { return field.help.to_string() };
    if field.kind != FieldKind::Select {
        return field.help.to_string();
    }
    let current = job.params.str(field.key);
    match field.options.iter().find(|o| o.value == current) {
        Some(o) if !o.desc.is_empty() => format!("{}. {}", field.help, o.desc),
        _ => field.help.to_string(),
    }
}

#[cfg(test)]
mod tests {

    fn answered() -> crate::core::Answer {
        crate::core::Answer {
            status: 200,
            reason: "OK".into(),
            content_type: "application/json".into(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("server".into(), "gunicorn".into()),
            ],
            body: r#"{"a":1,"b":[2,3]}"#.into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_response_pane_shows_the_half_that_was_asked_for() {
        use crate::ui::syntax::Language;
        let answer = answered();

        // The body, laid out, and coloured as what it is.
        let (body, language) = shown(&answer, false, false);
        assert!(body.contains("\n"), "minified JSON is laid out: {body:?}");
        assert_eq!(language, Language::Json);

        // Raw is exactly what arrived, still read as JSON.
        let (body, language) = shown(&answer, false, true);
        assert_eq!(body, r#"{"a":1,"b":[2,3]}"#);
        assert_eq!(language, Language::Json);

        // The headers are a list of names and values, whatever the body is.
        let (body, language) = shown(&answer, true, false);
        assert_eq!(body, "content-type: application/json\nserver: gunicorn");
        assert_eq!(language, Language::Text);
    }

    #[test]
    fn an_answer_that_is_not_json_is_left_exactly_as_it_came() {
        let answer = crate::core::Answer {
            content_type: "text/plain".into(),
            body: "79.116.21.76\n".into(),
            ..answered()
        };
        // Laying out is a thing that can be done to JSON and to nothing
        // else, so the toggle changes nothing here.
        let (laid, _) = shown(&answer, false, false);
        let (raw, _) = shown(&answer, false, true);
        assert_eq!(laid, "79.116.21.76\n");
        assert_eq!(raw, laid);
    }

    #[test]
    fn an_answer_is_read_in_the_language_the_server_said_it_was() {
        use crate::ui::syntax::Language;
        // The content type and not the body: a service that says JSON and
        // sends an HTML error page is telling you something, and guessing
        // from the bytes would hide it.
        assert_eq!(super::response_language("application/json"), Language::Json);
        assert_eq!(super::response_language("application/vnd.api+json"), Language::Json);
        assert_eq!(super::response_language("text/xml"), Language::Xml);
        assert_eq!(super::response_language("TEXT/HTML; charset=utf-8"), Language::Xml);
        assert_eq!(super::response_language("text/plain"), Language::Text);
        assert_eq!(super::response_language(""), Language::Text);
    }

    #[test]
    fn minified_json_is_laid_out_before_it_is_shown() {
        // Almost nothing sends JSON with newlines in it, and one line a
        // thousand characters wide can be neither read nor scrolled.
        let laid_out = super::pretty_json(r#"{"a":1,"b":[2,3]}"#);
        assert!(laid_out.lines().count() > 1, "{laid_out}");
        assert!(laid_out.contains("\"a\": 1"), "{laid_out}");

        // Anything that is not JSON comes back untouched: a body cut short
        // is still worth showing, and so is one that was never JSON whatever
        // its content type claimed.
        assert_eq!(super::pretty_json("<html>"), "<html>");
        assert_eq!(super::pretty_json("{\"a\":1"), "{\"a\":1");
    }

    #[test]
    fn an_answer_too_big_to_draw_is_cut_on_a_line_and_says_so() {
        // The tool keeps megabytes so a value can be captured out of them.
        // Drawing megabytes is one text element with a highlight range per
        // token, on the thread that draws the window.
        let small = "one\ntwo\n".to_string();
        let (body, clipped) = super::clip_for_reading(small.clone());
        assert_eq!((body, clipped), (small, false));

        let huge = "abcdefgh\n".repeat(60_000);
        let (body, clipped) = super::clip_for_reading(huge);
        assert!(clipped);
        assert!(body.len() <= super::READABLE);
        // On a line boundary, so the last line shown is a whole one.
        assert!(body.ends_with('\n'), "{:?}", &body[body.len() - 12..]);
    }

    #[test]
    fn a_wide_character_at_the_cut_is_not_sliced_through() {
        // Slicing a `str` anywhere but a character boundary is not allowed,
        // and an answer full of accents or CJK puts one wherever it likes.
        let body = "\u{4e16}\u{754c}".repeat(200_000);
        let (shown, clipped) = super::clip_for_reading(body);
        assert!(clipped);
        // It got back a valid string at all, which is the whole assertion.
        assert!(shown.chars().count() > 0);
        assert!(shown.len() <= super::READABLE);
    }
    use super::*;
    use crate::core::col;

    #[test]
    fn every_column_a_compared_row_lays_out_is_named_in_the_heading_row() {
        // A sweep declares a fixed column after its flexible one, which is
        // the arrangement the unnamed target column showed up in: the
        // target's width came out of VENDOR's share on the rows but not in
        // the heading, so SEEN and the word SEEN were nowhere near each
        // other.
        let columns = vec![col("HOST", 16), col("VENDOR", 0), col("SEEN", 12)];
        let layout = diff_columns(&columns, &[0, 0, 0]);

        assert_eq!(
            layout.iter().map(|(title, _)| *title).collect::<Vec<_>>(),
            vec!["CHANGE", "HOST", "VENDOR", "SEEN", "TARGET"]
        );
    }

    #[test]
    fn the_two_columns_a_comparison_adds_have_a_fixed_width() {
        // Only the tool's own flexible column may take up the slack. Either
        // of these left to size itself to its contents would divide the
        // leftover space differently on every row.
        let columns = vec![col("HOST", 16), col("BANNER", 0)];
        let layout = diff_columns(&columns, &[]);

        assert_eq!(layout.first().map(|(_, w)| *w), Some(Some(CHANGE_WIDTH)));
        assert_eq!(layout.last().map(|(_, w)| *w), Some(Some(TARGET_WIDTH)));
        assert_eq!(layout.iter().filter(|(_, w)| w.is_none()).count(), 1);
    }
}
