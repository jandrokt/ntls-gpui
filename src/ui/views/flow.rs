//! Building a workflow, and watching one run.
//!
//! A workflow is a tree of steps, and this is where it is built: every step is
//! a row, selecting one puts its controls on that row, and every control makes
//! one change to the tree. Nothing here writes the file — the change goes to
//! [`crate::flow::edit`], the tree comes back, and the file is written from
//! it, which is what keeps a workflow built here and one typed into the file
//! the same thing.
//!
//! What became of each step the last time it ran is shown against the step it
//! happened to, so the thing you are editing and the thing you are watching
//! are one list rather than two.

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder,
    px,
};

use crate::flow::edit::{Change, Guide, Recipe, Spot, Test, condition_of, takes_a_condition};
use crate::flow::{Step, seconds_text};
use crate::ui::app::App;
use crate::ui::flows::Outcome;
use crate::ui::icons::icon;
use crate::ui::menu::{Act, Item};
use crate::ui::theme::Theme;
use crate::ui::widgets::{Kind, Type, button, pill, space};

/// How far one level of nesting moves a step to the right.
const INDENT: f32 = 22.;

/// The colour a kind of step is drawn in.
///
/// Five kinds and one colour each, used for the icon, the word and the card:
/// what a workflow does is read down the left edge, and colour is what makes
/// that edge readable at a glance rather than five identical grey lines.
/// Blue does the work, amber decides, green goes round again, grey waits, red
/// ends it — which is the same vocabulary the rest of the interface uses for
/// running, ambiguous, good, idle and stopped.
fn hue(step: &Step, theme: &Theme) -> (Hsla, Hsla) {
    match step {
        Step::Run { .. } => (theme.accent, theme.accent_soft),
        Step::If { .. } => (theme.warn, theme.warn_soft),
        Step::Repeat { .. } => (theme.up, theme.up_soft),
        Step::Wait { .. } => (theme.dim, theme.track),
        // A set step and a run both produce something, and the word beside
        // the icon is what tells them apart.
        Step::Set { .. } => (theme.accent, theme.accent_soft),
        Step::Stop { .. } => (theme.down, theme.down_soft),
    }
}

/// The word a step is labelled with, and what it says after it.
///
/// The two are separate so the label can take the step's colour and the rest
/// can be read as the value it is.
fn parts(step: &Step) -> (&'static str, String) {
    match step {
        Step::Run { name, .. } => {
            ("Run", if name.trim().is_empty() { "\u{2014}".into() } else { name.clone() })
        }
        Step::If { .. } => ("If", String::new()),
        Step::Repeat { times, .. } => {
            ("Repeat", format!("{times} time{}", if *times == 1 { "" } else { "s" }))
        }
        Step::Wait { seconds } => ("Wait", seconds_text(*seconds)),
        Step::Set { name, value } => (
            "Set",
            match (name.trim().is_empty(), value.trim().is_empty()) {
                (true, _) => "\u{2014}".into(),
                (false, true) => name.clone(),
                (false, false) => format!("{name} = {value}"),
            },
        ),
        Step::Stop { .. } => ("Stop", String::new()),
    }
}

/// The lengths the wait control steps through. A pause is chosen from the ones
/// anybody means rather than typed to the second.
const WAITS: [f64; 9] = [5., 10., 30., 60., 120., 300., 600., 1800., 3600.];

/// One row of the editor.
enum Line {
    /// A step, its place in the tree, and its number depth-first — which is
    /// how a run reports back which step it is on.
    Step { spot: Spot, id: usize, step: Step, depth: usize },
    /// The word between the two halves of a branch.
    Otherwise { depth: usize },
    /// The end of a block inside a step, where another step can go.
    Slot { spot: Spot, block: usize, depth: usize },
}

/// Flattens the tree into the rows it is drawn as.
///
/// The numbering is depth-first, which is the order [`crate::flow::compile`]
/// gives the steps when it compiles them — so a row and the report of what
/// happened to it agree without either knowing about the other.
fn lay_out(
    steps: &[Step],
    inside: &[(usize, usize)],
    depth: usize,
    id: &mut usize,
    out: &mut Vec<Line>,
) {
    for (index, step) in steps.iter().enumerate() {
        let spot = Spot { inside: inside.to_vec(), index };
        let me = *id;
        *id += 1;
        out.push(Line::Step { spot: spot.clone(), id: me, step: step.clone(), depth });

        match step {
            Step::If { then, otherwise, .. } => {
                let mut within = inside.to_vec();
                within.push((index, 0));
                lay_out(then, &within, depth + 1, id, out);
                out.push(Line::Slot { spot: spot.clone(), block: 0, depth: depth + 1 });

                out.push(Line::Otherwise { depth });
                let mut within = inside.to_vec();
                within.push((index, 1));
                lay_out(otherwise, &within, depth + 1, id, out);
                out.push(Line::Slot { spot, block: 1, depth: depth + 1 });
            }
            Step::Repeat { body, .. } => {
                let mut within = inside.to_vec();
                within.push((index, 0));
                lay_out(body, &within, depth + 1, id, out);
                out.push(Line::Slot { spot, block: 0, depth: depth + 1 });
            }
            _ => {}
        }
    }
}

impl App {
    pub(super) fn flow_pane(
        &mut self,
        id: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item = crate::ui::workspace::Item::Flow(id);
        let renaming = self.renaming_item == Some(item);
        let rename_input = self.rename_input.clone();
        let editor = self.editor_for(item).cloned();
        let dirty = editor.as_ref().is_some_and(|e| e.read(cx).dirty);
        // Every run in the workspace and what each one can be asked, which is
        // all the condition controls offer — no rows are read, so this costs
        // the same whether a scan found four hosts or sixty-five thousand.
        let tables = crate::ui::notes::shapes(self.workspace());
        let value_input = self.step_input.clone();
        let name_input = self.step_name_input.clone();
        // The workspace's variables: offered as the subject of a condition,
        // as the name a `set` keeps its answer under, and worked out live so
        // the card can show what a `set` would set.
        let vars: Vec<String> = self.workspace().vars.keys().cloned().collect();
        let preview = self.workspace().flow(id).and_then(|sheet| {
            let (_, step) = sheet.selected()?;
            let Step::Set { value, .. } = step else { return None };
            (!value.trim().is_empty()).then(|| {
                let data = crate::ui::notes::Data::of(self.workspace());
                crate::expr::run(&value, &data)
                    .map(|answer| answer.show())
                    .map_err(|why| why.to_string())
            })
        });

        let Some(sheet) = self.workspace().flow(id) else { return div().into_any_element() };
        let (title, path, running) = (sheet.title(), sheet.path.clone(), sheet.running());
        let scroll = sheet.scroll.clone();
        let parsed = sheet.flow();
        let cursor = sheet.cursor.clone();
        // A file with a line in it nobody can read is not one to rewrite; a
        // step that is half-built is only half-built.
        let readable = !parsed.lossy;

        let mut lines = Vec::new();
        lay_out(&parsed.steps, &[], 0, &mut 0, &mut lines);

        let rows: Vec<AnyElement> = lines
            .iter()
            .map(|line| match line {
                Line::Step { spot, id: step_id, step, depth } => {
                    let outcome = sheet.mark(*step_id).cloned();
                    let passes = sheet.passes(*step_id);
                    let selected = readable && cursor.as_ref() == Some(spot);
                    if selected {
                        open_step(
                            id,
                            spot,
                            step,
                            *depth,
                            &tables,
                            &vars,
                            preview.clone(),
                            value_input.clone(),
                            name_input.clone(),
                            theme,
                            cx,
                        )
                    } else {
                        shut_step(
                            id, spot, *step_id, step, *depth, outcome, passes, readable, theme, cx,
                        )
                    }
                }
                Line::Otherwise { depth } => otherwise_row(*depth, theme),
                Line::Slot { spot, block, depth } => {
                    slot_row(id, spot, *block, *depth, readable, theme, cx)
                }
            })
            .collect();

        let problems: Vec<AnyElement> = parsed
            .problems
            .iter()
            .map(|(line, why)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::ROOMY))
                    .h(px(26.))
                    .px(px(space::ROOMY))
                    .child(
                        div()
                            .w(px(24.))
                            .flex_shrink_0()
                            .mono()
                            .text_meta()
                            .text_color(theme.down)
                            .text_right()
                            .child(line.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_small()
                            .text_color(theme.down)
                            .child(why.clone()),
                    )
                    .into_any_element()
            })
            .collect();

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
                    .child(icon("play", px(15.), theme.accent))
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
                            .id("flow-title")
                            .flex_1()
                            .min_w_0()
                            .text_heading()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .truncate()
                            .cursor_pointer()
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.begin_item_rename(item, window, cx)
                            }))
                            .child(title)
                            .into_any_element()
                    })
                    .child(
                        div()
                            .max_w(px(240.))
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
                    // Adding a step is the one thing a workflow with nothing
                    // in it needs, so it is a button rather than a menu item.
                    .when(readable, |d| {
                        d.child(
                            button("flow-add", "Add step", Kind::Normal, theme).on_click(
                                cx.listener(move |app, e: &ClickEvent, _, cx| {
                                    app.open_menu(
                                        e.position(),
                                        crate::ui::menu::for_new_step(id),
                                        cx,
                                    );
                                }),
                            ),
                        )
                    })
                    // The text is still there for anyone who would rather
                    // write it, and is the only way in when a line of the file
                    // cannot be read.
                    .child(
                        button(
                            "flow-text",
                            "Text",
                            Kind::Toggle(editor.is_some()),
                            theme,
                        )
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.toggle_editing(item, window, cx)
                        })),
                    )
                    .child(if running {
                        button("flow-stop", "Stop", Kind::Danger, theme)
                            .on_click(cx.listener(move |app, _, _, cx| app.stop_flow(id, cx)))
                    } else {
                        button("flow-run", "Run", Kind::Primary, theme).on_click(cx.listener(
                            move |app, _, window, cx| app.start_flow(id, window, cx),
                        ))
                    }),
            )
            .child(super::rule(theme))
            .children(editor.map(|editor| {
                div()
                    .flex()
                    .flex_col()
                    .h(gpui::relative(0.5))
                    .min_h_0()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(super::doc::source_pane(editor, theme, cx))
            }))
            .child(
                div()
                    .id(SharedString::from(format!("flow-{id}")))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(2.))
                    .p(px(space::ROOMY))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    // Clicking past the steps is how you stop editing one.
                    // Every step and every card takes its own click first.
                    .on_click(cx.listener(move |app, _, _, cx| app.select_step(id, None, cx)))
                    // A file with a line in it nobody can read is not one to
                    // rewrite: the controls would throw that line away without
                    // asking, so they are put away until it is fixed.
                    .when(!readable, |d| {
                        d.child(
                            div()
                                .mx(px(space::ROOMY))
                                .mb(px(space::SNUG))
                                .px(px(space::ROOMY))
                                .py(px(space::SNUG))
                                .rounded(px(6.))
                                .bg(theme.warn_soft)
                                .text_small()
                                .text_color(theme.warn)
                                .child(
                                    "Some of this file cannot be read, so the controls are put away rather than rewrite it. Text opens what is there.",
                                ),
                        )
                    })
                    .when(rows.is_empty() && readable, |d| {
                        d.child(
                            div()
                                .px(px(space::ROOMY))
                                .text_small()
                                .text_color(theme.faint)
                                .child("Nothing here yet. Add step starts it off."),
                        )
                    })
                    .children(rows)
                    .children(problems),
            )
            .into_any_element()
    }
}

/// A step as it reads when it is not being edited: one line.
#[allow(clippy::too_many_arguments)]
fn shut_step(
    flow: usize,
    spot: &Spot,
    step_id: usize,
    step: &Step,
    depth: usize,
    outcome: Option<Outcome>,
    passes: usize,
    readable: bool,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let (menu_spot, click_spot) = (spot.clone(), spot.clone());
    let step_for_menu = step.clone();
    let running = outcome == Some(Outcome::Running);
    let repeated = matches!(step, Step::Repeat { .. });
    let (colour, soft) = hue(step, theme);
    let (word, detail) = parts(step);
    let condition = condition_of(step).to_string();
    let branch = matches!(step, Step::If { .. });

    div()
        .id(SharedString::from(format!("step-{flow}-{step_id}")))
        .flex()
        .items_center()
        .gap(px(space::SNUG))
        .h(px(30.))
        .ml(px(depth as f32 * INDENT))
        .px(px(space::SNUG))
        .max_w(px(860.))
        .rounded(px(6.))
        .when(running, |d| d.bg(soft))
        .when(readable, |d| d.cursor_pointer().hover(|s| s.bg(theme.hover)))
        .child(icon(step.icon(), px(13.), colour))
        .child(
            div()
                .text_small()
                .font_weight(FontWeight::MEDIUM)
                .text_color(colour)
                .whitespace_nowrap()
                .child(word),
        )
        .when(!detail.is_empty(), |d| {
            d.child(
                div()
                    .min_w_0()
                    .mono()
                    .text_small()
                    .text_color(theme.text)
                    .truncate()
                    .child(detail),
            )
        })
        .when(!condition.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::TIGHT))
                    .min_w_0()
                    // A branch is already labelled "If"; everything else needs
                    // the word to read as a sentence.
                    .when(!branch, |d| {
                        d.child(div().text_meta().text_color(theme.faint).child("if"))
                    })
                    .child(
                        div()
                            .min_w_0()
                            .mono()
                            .text_small()
                            .text_color(theme.dim)
                            .truncate()
                            .child(condition),
                    ),
            )
        })
        .children(outcome.as_ref().map(|o| {
            pill(o.label(), theme.status(o.status()), theme.status_soft(o.status()))
        }))
        .children((repeated && passes > 1).then(|| {
            div().text_meta().text_color(theme.faint).child(format!("{passes} passes"))
        }))
        .child(div().flex_1())
        .children(outcome.as_ref().map(|o| o.detail()).filter(|d| !d.is_empty()).map(|detail| {
            div()
                .max_w(px(300.))
                .flex_shrink_0()
                .text_small()
                .text_color(theme.faint)
                .truncate()
                .child(detail)
        }))
        .on_click(cx.listener(move |app, _, _, cx| {
            if readable {
                app.select_step(flow, Some(click_spot.clone()), cx);
            }
            cx.stop_propagation();
        }))
        .on_mouse_down(
            gpui::MouseButton::Right,
            cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                if !readable {
                    return;
                }
                app.select_step(flow, Some(menu_spot.clone()), cx);
                let items =
                    crate::ui::menu::for_step(flow, menu_spot.clone(), &step_for_menu);
                app.open_menu(e.position, items, cx);
                cx.stop_propagation();
            }),
        )
        .into_any_element()
}

/// The step that is selected: the same line, with what can be changed about it
/// on it.
#[allow(clippy::too_many_arguments)]
fn open_step(
    flow: usize,
    spot: &Spot,
    step: &Step,
    depth: usize,
    tables: &[crate::expr::Table],
    vars: &[String],
    preview: Option<Result<String, String>>,
    value_input: gpui::Entity<crate::ui::text_input::TextInput>,
    value_name: gpui::Entity<crate::ui::text_input::TextInput>,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let (colour, soft) = hue(step, theme);
    let mut top = div()
        .flex()
        .items_center()
        .gap(px(space::SNUG))
        .child(icon(step.icon(), px(13.), colour))
        .child(
            div()
                .px(px(6.))
                .rounded(px(4.))
                .bg(soft)
                .text_small()
                .font_weight(FontWeight::MEDIUM)
                .text_color(colour)
                .whitespace_nowrap()
                .child(parts(step).0),
        );

    // What each kind of step has to be told, and nothing else.
    match step {
        Step::Run { name, .. } => {
            let named = !name.trim().is_empty();
            let at = spot.clone();
            top = top.child(
                div()
                    .id("step-run-name")
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(24.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .bg(theme.raised)
                    .border_1()
                    .border_color(if named { theme.border } else { theme.warn })
                    .text_small()
                    .text_color(if named { theme.text } else { theme.faint })
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.hover))
                    .child(if named { name.clone() } else { "choose a run\u{2026}".into() })
                    .child(icon("chevron-down", px(11.), theme.faint))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.ask_step_run(flow, at.clone(), window, cx);
                    })),
            );
        }
        Step::Repeat { times, .. } => {
            let times = *times;
            top = top
                .child(stepper(
                    "step-times",
                    format!("{times} time{}", if times == 1 { "" } else { "s" }),
                    (flow, spot.clone(), Change::SetTimes(times.saturating_sub(1).max(1))),
                    (flow, spot.clone(), Change::SetTimes(times + 1)),
                    theme,
                    cx,
                ))
                .child(div().text_meta().text_color(theme.faint).child("round"));
        }
        Step::Wait { seconds } => {
            let (shorter, longer) = (nearer(*seconds, -1), nearer(*seconds, 1));
            top = top.child(stepper(
                "step-wait",
                seconds_text(*seconds),
                (flow, spot.clone(), Change::SetWait(shorter)),
                (flow, spot.clone(), Change::SetWait(longer)),
                theme,
                cx,
            ));
        }
        Step::Set { value, .. } => {
            // Every variable the workspace already has, so a step that keeps
            // something is usually one click and no typing.
            let known: Vec<Item> = vars
                .iter()
                .map(|name| {
                    Item::choice(
                        name.clone(),
                        "gear",
                        Act::ChangeStep(flow, spot.clone(), Change::SetVarName(name.clone())),
                    )
                })
                .collect();

            top = top
                .child(boxed(value_name.clone(), px(120.), theme))
                .when(!known.is_empty(), |d| {
                    d.child(pick("step-var-name", known, theme, cx))
                })
                .child(div().text_meta().text_color(theme.faint).child("="))
                // What it keeps is chosen the way a condition is: which run,
                // which of its figures, and how a column of many rows becomes
                // one value. An expression the controls cannot describe is
                // typed instead, and says so.
                .children(match Recipe::read(value) {
                    Some(recipe) => {
                        value_controls(flow, spot, &recipe, tables, vars, theme, cx)
                    }
                    None => vec![
                        boxed(value_input.clone(), px(220.), theme).into_any_element(),
                        pill("as written", theme.dim, theme.track).into_any_element(),
                        small_icon("step-value-clear", "close", theme, cx, {
                            let at = spot.clone();
                            move |app, _, cx| {
                                app.change_step(flow, &at, &Change::SetVarValue(String::new()), cx)
                            }
                        }),
                    ],
                })
                // What the expression comes to right now, which is what the
                // step would keep if it ran this second.
                .children(preview.map(|answer| match answer {
                    Ok(value) => div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .min_w_0()
                        .child(div().text_meta().text_color(theme.faint).child("\u{2192}"))
                        .child(
                            div()
                                .mono()
                                .text_small()
                                .text_color(theme.up)
                                .truncate()
                                .child(if value.is_empty() { "\u{2014}".into() } else { value }),
                        ),
                    Err(why) => div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .min_w_0()
                        .child(
                            div()
                                .text_meta()
                                .text_color(theme.down)
                                .truncate()
                                .child(why),
                        ),
                }));
        }
        Step::If { .. } | Step::Stop { .. } => {}
    }

    let (up, down) = (spot.clone(), spot.clone());
    let (menu_spot, gone) = (spot.clone(), spot.clone());
    let step_for_menu = step.clone();
    top = top
        .child(div().flex_1())
        .child(small_icon("step-up", "chevron-up", theme, cx, move |app, _, cx| {
            app.change_step(flow, &up, &Change::Move(false), cx);
        }))
        .child(small_icon("step-down", "chevron-down", theme, cx, move |app, _, cx| {
            app.change_step(flow, &down, &Change::Move(true), cx);
        }))
        .child(small_icon("step-more", "plus", theme, cx, move |app, e: &ClickEvent, cx| {
            let items = crate::ui::menu::for_step(flow, menu_spot.clone(), &step_for_menu);
            app.open_menu(e.position(), items, cx);
        }))
        .child(small_icon("step-gone", "close", theme, cx, move |app, _, cx| {
            app.change_step(flow, &gone, &Change::Remove, cx);
        }));

    let condition = takes_a_condition(step)
        .then(|| condition_row(flow, spot, step, tables, vars, value_input, theme, cx));

    div()
        .id(SharedString::from(format!("step-card-{flow}-{spot:?}")))
        .flex()
        .flex_col()
        .gap(px(space::TIGHT))
        .ml(px(depth as f32 * INDENT))
        .p(px(space::SNUG))
        .max_w(px(860.))
        .rounded(px(8.))
        .bg(theme.panel)
        .border_1()
        .border_color(colour)
        // The card is not "elsewhere": a click that lands on it, or on a
        // control inside it, leaves the step being edited.
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(top)
        .children(condition)
        .into_any_element()
}

/// The controls that build a condition: one field of one run, against one
/// value.
///
/// A condition too involved for that is shown as it was written rather than
/// pretended about — the language is bigger than the controls, and a workflow
/// that used the rest of it is still a workflow.
#[allow(clippy::too_many_arguments)]
fn condition_row(
    flow: usize,
    spot: &Spot,
    step: &Step,
    tables: &[crate::expr::Table],
    vars: &[String],
    value_input: gpui::Entity<crate::ui::text_input::TextInput>,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let condition = condition_of(step);
    let branch = matches!(step, Step::If { .. });
    let row = div()
        .flex()
        .items_center()
        .flex_wrap()
        .gap(px(space::TIGHT))
        .pl(px(21.))
        .child(
            div()
                .text_meta()
                .text_color(theme.faint)
                .whitespace_nowrap()
                .child(if branch { "if" } else { "only if" }),
        );

    // Nothing chosen yet: one word to start, rather than four empty controls
    // on every step that could have a condition and does not.
    if condition.trim().is_empty() {
        let first = tables.first().map(|t| t.name.clone());
        let at = spot.clone();
        return row
            .child(
                button("step-condition-add", "choose\u{2026}", Kind::Ghost, theme).on_click(
                    cx.listener(move |app, _, _, cx| {
                        let Some(name) = first.clone() else { return };
                        app.change_step(flow, &at, &Change::SetSubject(name), cx);
                    }),
                ),
            )
            .when(tables.is_empty(), |d| {
                d.child(
                    div()
                        .text_meta()
                        .text_color(theme.faint)
                        .child("— a condition is about a run, and there are none here yet"),
                )
            })
            .into_any_element();
    }

    let Some(guide) = Guide::read_partial(condition) else {
        // Written by hand, and beyond what the controls describe.
        let at = spot.clone();
        return row
            .child(
                div()
                    .mono()
                    .text_small()
                    .text_color(theme.text)
                    .child(condition.to_string()),
            )
            .child(pill("as written", theme.dim, theme.track))
            .child(small_icon("step-condition-clear", "close", theme, cx, move |app, _, cx| {
                app.change_step(flow, &at, &Change::ClearCondition, cx);
            }))
            .into_any_element();
    };

    let subject = tables
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(&guide.subject))
        .or_else(|| tables.iter().find(|t| t.tool.eq_ignore_ascii_case(&guide.subject)));

    // A condition is about a run, or about one of the workspace's variables.
    let mut subjects: Vec<Item> = Vec::new();
    if !tables.is_empty() {
        subjects.push(Item::Heading("Runs".into()));
        for table in tables {
            subjects.push(Item::choice(
                table.name.clone(),
                "play",
                Act::ChangeStep(flow, spot.clone(), Change::SetSubject(table.name.clone())),
            ));
        }
    }
    if !vars.is_empty() {
        subjects.push(Item::Heading("Variables".into()));
        for name in vars {
            subjects.push(Item::choice(
                name.clone(),
                "gear",
                Act::ChangeStep(flow, spot.clone(), Change::SetSubject(name.clone())),
            ));
        }
    }
    // A variable is a value already: it has no columns and no figures, so the
    // control that asks which one is not shown for it.
    let about_a_var = vars.iter().any(|name| name.eq_ignore_ascii_case(&guide.subject));

    let mut fields: Vec<Item> = vec![Item::Heading("Every run".into())];
    for (name, detail) in crate::ui::complete::FIELDS {
        let _ = detail;
        fields.push(Item::plain(
            name,
            Act::ChangeStep(flow, spot.clone(), Change::SetField(name.into())),
        ));
    }
    if let Some(table) = subject {
        let columns: Vec<&String> = table
            .columns
            .iter()
            .filter(|c| c.chars().all(|c| c.is_alphanumeric() || c == '_'))
            .collect();
        if !columns.is_empty() {
            fields.push(Item::Separator);
            fields.push(Item::Heading("Its columns".into()));
            for column in columns {
                let name = column.to_lowercase();
                fields.push(Item::plain(
                    name.clone(),
                    Act::ChangeStep(flow, spot.clone(), Change::SetField(name)),
                ));
            }
        }
        if !table.stats.is_empty() {
            fields.push(Item::Separator);
            fields.push(Item::Heading("What it reported".into()));
            for (key, value) in &table.stats {
                fields.push(Item::plain(
                    format!("{key} \u{b7} {value}"),
                    Act::ChangeStep(flow, spot.clone(), Change::SetField(key.clone())),
                ));
            }
        }
    }

    let tests: Vec<Item> = Test::ALL
        .iter()
        .map(|test| {
            Item::plain(test.label(), Act::ChangeStep(flow, spot.clone(), Change::SetTest(*test)))
        })
        .collect();

    let at = spot.clone();
    row.child(chooser("step-subject", guide.subject.clone(), false, subjects, theme, cx))
        .when(!about_a_var, |d| {
            d.child(chooser(
                "step-field",
                if guide.field.is_empty() { "what\u{2026}".into() } else { guide.field.clone() },
                guide.field.is_empty(),
                fields,
                theme,
                cx,
            ))
        })
        .child(chooser("step-test", guide.test.label().into(), false, tests, theme, cx))
        .child(
            div()
                .w(px(110.))
                .h(px(24.))
                .flex()
                .items_center()
                .px(px(8.))
                .rounded(px(6.))
                .bg(theme.bg)
                .border_1()
                .border_color(theme.border)
                .text_small()
                .child(value_input),
        )
        .child(small_icon("step-condition-clear", "close", theme, cx, move |app, _, cx| {
            app.change_step(flow, &at, &Change::ClearCondition, cx);
        }))
        .into_any_element()
}

/// The word between the halves of a branch.
fn otherwise_row(depth: usize, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .h(px(22.))
        .ml(px(depth as f32 * INDENT + 8.))
        .text_meta()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.warn)
        .child("otherwise")
        .into_any_element()
}

/// The end of a block: where the next step inside it goes.
fn slot_row(
    flow: usize,
    spot: &Spot,
    block: usize,
    depth: usize,
    readable: bool,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    if !readable {
        return div().into_any_element();
    }
    let at = spot.clone();
    div()
        .id(SharedString::from(format!("slot-{flow}-{depth}-{block}-{}", at.index)))
        .flex()
        .items_center()
        .gap(px(space::TIGHT))
        .h(px(24.))
        .ml(px(depth as f32 * INDENT))
        .px(px(space::SNUG))
        .rounded(px(6.))
        .text_meta()
        .text_color(theme.faint)
        .cursor_pointer()
        .hover(|s| s.bg(theme.hover).text_color(theme.text))
        .child(icon("plus", px(11.), theme.faint))
        .child("step")
        .on_click(cx.listener(move |app, e: &ClickEvent, _, cx| {
            let items = crate::ui::menu::for_new_step_inside(flow, at.clone(), block);
            app.open_menu(e.position(), items, cx);
            cx.stop_propagation();
        }))
        .into_any_element()
}

/// A text box, sized to what it holds.
fn boxed(
    input: gpui::Entity<crate::ui::text_input::TextInput>,
    width: gpui::Pixels,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .w(width)
        .h(px(24.))
        .flex()
        .items_center()
        .px(px(8.))
        .rounded(px(6.))
        .bg(theme.bg)
        .border_1()
        .border_color(theme.border)
        .text_small()
        .child(input)
}

/// A value with a way down and a way up either side of it.
fn stepper(
    id: &'static str,
    label: String,
    less: (usize, Spot, Change),
    more: (usize, Spot, Change),
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .child(small_icon(
            SharedString::from(format!("{id}-less")),
            "chevron-down",
            theme,
            cx,
            move |app, _, cx| app.change_step(less.0, &less.1, &less.2, cx),
        ))
        .child(
            div()
                .min_w(px(64.))
                .text_small()
                .text_color(theme.text)
                .text_center()
                .whitespace_nowrap()
                .child(label),
        )
        .child(small_icon(
            SharedString::from(format!("{id}-more")),
            "chevron-up",
            theme,
            cx,
            move |app, _, cx| app.change_step(more.0, &more.1, &more.2, cx),
        ))
        .into_any_element()
}

/// A small borderless button holding one icon, with the click it means.
fn small_icon(
    id: impl Into<SharedString>,
    name: &'static str,
    theme: &Theme,
    cx: &mut Context<App>,
    click: impl Fn(&mut App, &ClickEvent, &mut Context<App>) + 'static,
) -> AnyElement {
    let (dim, hover, text) = (theme.dim, theme.hover, theme.text);
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded(px(5.))
        .text_color(dim)
        .cursor_pointer()
        .hover(move |s| s.bg(hover).text_color(text))
        .child(icon(name, px(13.), dim))
        .on_click(cx.listener(move |app, e: &ClickEvent, _, cx| click(app, e, cx)))
        .into_any_element()
}

/// The controls that build what a `set` step keeps.
///
/// The same three questions the condition controls ask, minus the comparison:
/// which run or variable, which of its figures, and — because a column is a
/// list — how to make one value out of it.
#[allow(clippy::too_many_arguments)]
fn value_controls(
    flow: usize,
    spot: &Spot,
    recipe: &Recipe,
    tables: &[crate::expr::Table],
    vars: &[String],
    theme: &Theme,
    cx: &mut Context<App>,
) -> Vec<AnyElement> {
    let mut subjects: Vec<Item> = Vec::new();
    if !tables.is_empty() {
        subjects.push(Item::Heading("Runs".into()));
        for table in tables {
            subjects.push(Item::choice(
                table.name.clone(),
                "play",
                Act::ChangeStep(flow, spot.clone(), Change::SetValueSubject(table.name.clone())),
            ));
        }
    }
    if !vars.is_empty() {
        subjects.push(Item::Heading("Variables".into()));
        for name in vars {
            subjects.push(Item::choice(
                name.clone(),
                "gear",
                Act::ChangeStep(flow, spot.clone(), Change::SetValueSubject(name.clone())),
            ));
        }
    }

    let mut out = vec![chooser(
        "step-value-subject",
        if recipe.subject.is_empty() { "choose\u{2026}".into() } else { recipe.subject.clone() },
        recipe.subject.is_empty(),
        subjects,
        theme,
        cx,
    )];
    if recipe.subject.is_empty() {
        return out;
    }

    // A variable is a value already; only a run has figures to pick from.
    let subject = tables
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(&recipe.subject))
        .or_else(|| tables.iter().find(|t| t.tool.eq_ignore_ascii_case(&recipe.subject)));
    let Some(table) = subject else { return out };

    let mut fields: Vec<Item> = vec![Item::Heading("Every run".into())];
    for (name, _) in crate::ui::complete::FIELDS {
        fields.push(Item::plain(
            name,
            Act::ChangeStep(flow, spot.clone(), Change::SetValueField(name.into())),
        ));
    }
    let columns: Vec<&String> = table
        .columns
        .iter()
        .filter(|c| c.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .collect();
    if !columns.is_empty() {
        fields.push(Item::Separator);
        fields.push(Item::Heading("Its columns".into()));
        for column in columns {
            let name = column.to_lowercase();
            fields.push(Item::plain(
                name.clone(),
                Act::ChangeStep(flow, spot.clone(), Change::SetValueField(name)),
            ));
        }
    }
    if !table.stats.is_empty() {
        fields.push(Item::Separator);
        fields.push(Item::Heading("What it reported".into()));
        for (key, value) in &table.stats {
            fields.push(Item::plain(
                format!("{key} \u{b7} {value}"),
                Act::ChangeStep(flow, spot.clone(), Change::SetValueField(key.clone())),
            ));
        }
    }

    out.push(chooser(
        "step-value-field",
        if recipe.field.is_empty() { "what\u{2026}".into() } else { recipe.field.clone() },
        recipe.field.is_empty(),
        fields,
        theme,
        cx,
    ));
    if recipe.field.is_empty() {
        return out;
    }

    // Only a column is a list, and only a list needs summarising — but the
    // control is harmless where it is not needed and unmissable where it is.
    let summaries: Vec<Item> = crate::flow::edit::SUMMARIES
        .iter()
        .map(|(name, about)| {
            let label = if name.is_empty() { "as it is".to_string() } else { format!("{name} \u{b7} {about}") };
            Item::plain(
                label,
                Act::ChangeStep(
                    flow,
                    spot.clone(),
                    Change::SetValueSummary((*name).to_string()),
                ),
            )
        })
        .collect();
    out.push(chooser(
        "step-value-summary",
        if recipe.summary.is_empty() { "as it is".into() } else { recipe.summary.clone() },
        recipe.summary.is_empty(),
        summaries,
        theme,
        cx,
    ));
    out
}

/// A control that only offers: no value of its own, just the arrow.
fn pick(
    id: &'static str,
    items: Vec<Item>,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let hover = theme.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded(px(5.))
        .text_color(theme.faint)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(icon("chevron-down", px(12.), theme.faint))
        .on_click(cx.listener(move |app, e: &ClickEvent, _, cx| {
            app.open_menu(e.position(), items.clone(), cx);
        }))
        .into_any_element()
}

/// A control that shows what is chosen and offers the rest when it is clicked.
fn chooser(
    id: &'static str,
    label: String,
    empty: bool,
    items: Vec<Item>,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let (raised, border, hover): (Hsla, Hsla, Hsla) = (theme.raised, theme.border, theme.hover);
    let colour = if empty { theme.faint } else { theme.text };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(24.))
        .px(px(8.))
        .rounded(px(6.))
        .bg(raised)
        .border_1()
        .border_color(border)
        .text_small()
        .text_color(colour)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(label)
        .child(icon("chevron-down", px(11.), theme.faint))
        .on_click(cx.listener(move |app, e: &ClickEvent, _, cx| {
            app.open_menu(e.position(), items.clone(), cx);
        }))
        .into_any_element()
}

/// The next length of pause up or down from this one.
fn nearer(seconds: f64, direction: i32) -> u32 {
    let at = WAITS.iter().position(|w| *w >= seconds - 0.001).unwrap_or(WAITS.len() - 1);
    let next = if direction < 0 {
        at.saturating_sub(1)
    } else if WAITS.get(at).is_some_and(|w| *w > seconds + 0.001) {
        at
    } else {
        (at + 1).min(WAITS.len() - 1)
    };
    WAITS[next] as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::parse;

    #[test]
    fn the_rows_are_numbered_the_way_a_running_workflow_numbers_its_steps() {
        // A row and the report of what happened to it agree only because both
        // count depth-first; if this drifts, a branch marks the wrong line.
        let flow = parse("run A\nif c {\n  run B\n}\nrun D");
        let mut lines = Vec::new();
        lay_out(&flow.steps, &[], 0, &mut 0, &mut lines);

        let numbered: Vec<(usize, String)> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Step { id, step, .. } => {
                    let (word, detail) = parts(step);
                    Some((*id, format!("{word} {detail}").trim_end().to_string()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            numbered,
            vec![
                (0, "Run A".to_string()),
                (1, "If".to_string()),
                (2, "Run B".to_string()),
                (3, "Run D".to_string()),
            ]
        );
    }

    #[test]
    fn every_block_ends_in_somewhere_to_put_the_next_step() {
        // Including the empty half of a branch, which otherwise could never be
        // filled in with the mouse.
        let flow = parse("if c {\n  run B\n}");
        let mut lines = Vec::new();
        lay_out(&flow.steps, &[], 0, &mut 0, &mut lines);

        let slots: Vec<(usize, usize)> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Slot { block, spot, .. } => Some((*block, spot.index)),
                _ => None,
            })
            .collect();
        assert_eq!(slots, vec![(0, 0), (1, 0)]);
        assert!(lines.iter().any(|l| matches!(l, Line::Otherwise { .. })));
    }

    #[test]
    fn a_step_inside_a_branch_is_indented_and_knows_where_it_is() {
        let flow = parse("repeat 2 {\n  if c {\n    run B\n  }\n}");
        let mut lines = Vec::new();
        lay_out(&flow.steps, &[], 0, &mut 0, &mut lines);

        let deepest = lines
            .iter()
            .filter_map(|l| match l {
                Line::Step { spot, depth, step, .. } => Some((spot.clone(), *depth, step.clone())),
                _ => None,
            })
            .max_by_key(|(_, depth, _)| *depth)
            .expect("a step");
        assert_eq!(deepest.1, 2);
        assert_eq!(deepest.0.inside, vec![(0, 0), (0, 0)]);
        assert_eq!(crate::flow::edit::at(&flow.steps, &deepest.0), Some(&deepest.2));
    }

    #[test]
    fn a_pause_steps_through_the_lengths_anybody_means() {
        assert_eq!(nearer(30., 1), 60);
        assert_eq!(nearer(30., -1), 10);
        // Something typed into the file that is not one of them moves to the
        // nearest one in that direction rather than snapping about.
        assert_eq!(nearer(45., 1), 60);
        assert_eq!(nearer(45., -1), 30);
        // And it stops at either end.
        assert_eq!(nearer(5., -1), 5);
        assert_eq!(nearer(3600., 1), 3600);
    }
}
