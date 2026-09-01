//! The variables page.
//!
//! A variable is a piece of text with a name, or a formula that works one out.
//! Documents print them, workflow conditions read them, and a workflow can set
//! one as it runs.
//!
//! Each row is one line: name, value, and the buttons. A second line appears
//! only when there is something to put on it: the formula, a description, or
//! where the value came from. Rows with nothing to add stay one line high, so
//! a long list reads as a list.

use gpui::{
    AnyElement, AppContext, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};

use crate::ui::app::{App, VarPart};
use crate::ui::icons::icon;
use crate::ui::store::Var;
use crate::ui::theme::Theme;
use crate::ui::widgets::{Kind, Type, button, space};

use super::chrome::{Tip, ago, boxed_input, style_input};
use super::page::{Page, card, page_shell};

/// Width of the name column. Long enough for a real name, short enough that
/// the values still start near the left of the page.
const NAME: gpui::Pixels = px(160.);

/// What a row needs to know about one variable, worked out once.
struct Line {
    name: String,
    var: Var,
    /// What it comes to, and what kind of thing that is.
    answer: Result<(String, &'static str), String>,
    used_by: Vec<String>,
    reads: Vec<String>,
}

impl Line {
    /// The second line of the row: the formula or the description on the left,
    /// what kind of thing it is and where it came from on the right. Either
    /// half can be empty, and a row with both empty stays one line high.
    fn under(&self) -> (String, String) {
        let left = if self.var.formula {
            format!("= {}", self.var.value.trim())
        } else {
            self.var.about.clone()
        };
        // A formula's own description would be a third line, so it joins the
        // right-hand half instead.
        let mut right: Vec<String> = Vec::new();
        if let Ok((_, kind)) = &self.answer
            && self.var.formula
        {
            right.push(kind.to_string());
        }
        if self.var.formula && !self.var.about.is_empty() {
            right.push(self.var.about.clone());
        }
        match (&self.var.set_by, self.var.at) {
            (Some(by), Some(at)) => right.push(format!("set by {by} {}", ago(at))),
            (Some(by), None) => right.push(format!("set by {by}")),
            _ => {}
        }
        if !self.used_by.is_empty() {
            right.push(match self.used_by.len() {
                1 => format!("read by {}", self.used_by[0]),
                n => format!("read by {} and {} more", self.used_by[0], n - 1),
            });
        }
        if !self.reads.is_empty() {
            right.push(format!("reads {}", self.reads.join(", ")));
        }
        (left, right.join("   ·   "))
    }
}

impl App {
    pub(super) fn variables_page(
        &mut self,
        theme: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editing = self.editing_var.clone();
        if editing.is_some() {
            style_input(&self.var_input, theme, cx);
        }
        let input = self.var_input.clone();
        let filter = self.var_filter.trim().to_lowercase();

        let names: Vec<String> = self.workspace().vars.keys().cloned().collect();
        let all: Vec<Line> = names
            .into_iter()
            .map(|name| Line {
                var: self.workspace().var(&name).cloned().unwrap_or_default(),
                answer: self.var_answer(&name),
                used_by: self.var_used_by(&name),
                reads: self.var_reads(&name),
                name,
            })
            .collect();

        let total = all.len();
        let broken = all.iter().filter(|l| l.answer.is_err()).count();
        let shown: Vec<&Line> = all
            .iter()
            .filter(|l| {
                filter.is_empty()
                    || l.name.to_lowercase().contains(&filter)
                    || l.var.value.to_lowercase().contains(&filter)
                    || l.var.about.to_lowercase().contains(&filter)
            })
            .collect();

        let rows: Vec<AnyElement> = shown
            .iter()
            .map(|line| {
                let editing = (
                    editing.as_ref() == Some(&(line.name.clone(), VarPart::Name)),
                    editing.as_ref() == Some(&(line.name.clone(), VarPart::Value)),
                    editing.as_ref() == Some(&(line.name.clone(), VarPart::About)),
                );
                variable_row(line, editing, input.clone(), theme, cx)
            })
            .collect();

        let body = if total == 0 {
            vec![empty(theme, cx)]
        } else {
            vec![
                self.variables_toolbar(total, shown.len(), broken, theme, cx),
                if rows.is_empty() { no_match(theme) } else { card(theme).children(rows).into_any_element() },
                footnote(theme),
            ]
        };

        page_shell(
            Page::Variables,
            theme,
            vec![
                button("var-add", "New variable", Kind::Normal, theme)
                    .on_click(cx.listener(|app, _, w, cx| app.new_var(w, cx)))
                    .into_any_element(),
            ],
            body,
            cx,
        )
    }

    /// How many there are, how many are broken, and the box that narrows the
    /// list down.
    fn variables_toolbar(
        &mut self,
        total: usize,
        shown: usize,
        broken: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        style_input(&self.var_filter_input, theme, cx);
        let counted = if shown == total {
            match total {
                1 => "1 variable".to_string(),
                n => format!("{n} variables"),
            }
        } else {
            format!("{shown} of {total}")
        };

        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(space::SNUG))
            .child(div().text_meta().text_color(theme.faint).child(counted))
            .when(broken > 0, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::TIGHT))
                        .child(icon("alert", px(11.), theme.down))
                        .child(div().text_meta().text_color(theme.down).child(match broken {
                            1 => "1 does not work".to_string(),
                            n => format!("{n} do not work"),
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::TIGHT))
                    .w(px(200.))
                    .h(px(26.))
                    .px(px(space::SNUG))
                    .rounded(px(6.))
                    .bg(theme.raised)
                    .border_1()
                    .border_color(theme.border)
                    .child(icon("search", px(12.), theme.faint))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_small()
                            .child(self.var_filter_input.clone()),
                    ),
            )
            .into_any_element()
    }
}

/// One variable. Click any part of it to type into that part.
fn variable_row(
    line: &Line,
    editing: (bool, bool, bool),
    input: gpui::Entity<crate::ui::text_input::TextInput>,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let Line { name, var, answer, .. } = line;
    let (editing_name, editing_value, editing_about) = editing;
    let (for_name, for_value, for_about) = (name.clone(), name.clone(), name.clone());

    let (shown, wrong) = match answer {
        Ok((value, _)) if value.is_empty() => ("\u{2014}".to_string(), false),
        Ok((value, _)) => (value.clone(), false),
        Err(why) => (why.clone(), true),
    };
    let (under_left, under_right) = line.under();
    let second_line = !under_left.is_empty() || !under_right.is_empty() || editing_about;

    div()
        .id(SharedString::from(format!("var-{name}")))
        .group("var")
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(1.))
        .px(px(space::ROOMY))
        .py(px(space::TIGHT))
        .border_b_1()
        .border_color(theme.rule)
        .hover(|s| s.bg(theme.hover))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::SNUG))
                .h(px(24.))
                .child(if editing_name {
                    boxed_input(input.clone(), NAME, theme).into_any_element()
                } else {
                    div()
                        .id("var-name")
                        .w(NAME)
                        .flex_shrink_0()
                        .mono()
                        .text_small()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .truncate()
                        .cursor_text()
                        .child(name.clone())
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.edit_var(for_name.clone(), VarPart::Name, window, cx);
                        }))
                        .into_any_element()
                })
                .child(if editing_value {
                    boxed_input(input.clone(), px(0.), theme).into_any_element()
                } else {
                    div()
                        .id("var-value")
                        .flex_1()
                        .min_w_0()
                        .mono()
                        .text_small()
                        .text_color(if wrong {
                            theme.down
                        } else if shown == "\u{2014}" {
                            theme.faint
                        } else {
                            theme.text
                        })
                        .truncate()
                        .cursor_text()
                        .child(shown.clone())
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.edit_var(for_value.clone(), VarPart::Value, window, cx);
                        }))
                        .into_any_element()
                })
                .child(row_actions(name, var.formula, &shown, theme, cx)),
        )
        .when(second_line, |d| {
            d.child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(space::ROOMY))
                    .pl(NAME + px(space::SNUG))
                    .child(if editing_about {
                        boxed_input(input, px(0.), theme).into_any_element()
                    } else {
                        div()
                            .id("var-under")
                            .flex_1()
                            .min_w_0()
                            .when(var.formula, |d| d.mono())
                            .text_meta()
                            .text_color(if var.formula { theme.dim } else { theme.faint })
                            .truncate()
                            .cursor_text()
                            .child(under_left)
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.edit_var(for_about.clone(), VarPart::About, window, cx);
                            }))
                            .into_any_element()
                    })
                    .when(!under_right.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .max_w(px(320.))
                                .text_meta()
                                .text_color(theme.faint)
                                .truncate()
                                .child(under_right),
                        )
                    }),
            )
        })
        .into_any_element()
}

/// The buttons on a row, out of the way until the pointer is on it.
fn row_actions(
    name: &str,
    formula: bool,
    shown: &str,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let (toggle, duplicate, reference, remove) =
        (name.to_string(), name.to_string(), name.to_string(), name.to_string());
    let value = shown.to_string();

    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .flex_shrink_0()
        .opacity(0.)
        .group_hover("var", |s| s.opacity(1.))
        .child(action(
            format!("var-formula-{name}"),
            "variables",
            if formula { "Make it text" } else { "Make it a formula" },
            formula,
            theme,
            cx.listener(move |app, _, _, cx| app.toggle_var_formula(&toggle, cx)),
        ))
        .child(action(
            format!("var-copy-{name}"),
            "link",
            "Copy the value",
            false,
            theme,
            cx.listener(move |app, _, _, cx| app.copy_text("Value", value.clone(), cx)),
        ))
        .child(action(
            format!("var-ref-{name}"),
            "note",
            "Copy it as {{ a reference }}",
            false,
            theme,
            cx.listener(move |app, _, _, cx| {
                app.copy_text("Reference", format!("{{{{ {reference} }}}}"), cx);
            }),
        ))
        .child(action(
            format!("var-dup-{name}"),
            "plus",
            "Duplicate",
            false,
            theme,
            cx.listener(move |app, _, window, cx| app.duplicate_var(&duplicate, window, cx)),
        ))
        .child(action(
            format!("var-close-{name}"),
            "close",
            "Remove",
            false,
            theme,
            cx.listener(move |app, _, _, cx| app.remove_var(&remove, cx)),
        ))
        .into_any_element()
}

/// One of those buttons. `lit` marks the one that is currently on.
fn action(
    id: String,
    glyph: &'static str,
    tip: &'static str,
    lit: bool,
    theme: &Theme,
    click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let theme = *theme;
    let fg = if lit { theme.accent } else { theme.faint };
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded(px(4.))
        .cursor_pointer()
        .when(lit, |d| d.bg(theme.accent_soft))
        .hover(move |s| s.bg(theme.hover))
        .tooltip(move |_, cx| cx.new(|_| Tip { label: tip, theme }).into())
        .child(icon(glyph, px(12.), fg))
        .on_click(click)
        .into_any_element()
}

fn no_match(theme: &Theme) -> AnyElement {
    card(theme)
        .px(px(space::ROOMY))
        .py(px(space::ROOMY))
        .child(div().text_small().text_color(theme.faint).child("Nothing matches that."))
        .into_any_element()
}

fn footnote(theme: &Theme) -> AnyElement {
    div()
        .flex_shrink_0()
        .text_meta()
        .text_color(theme.faint)
        .child(
            "Write {{ name }} in a document to print one. A formula is worked out again every time it is read; text stays as it was written.",
        )
        .into_any_element()
}

/// What the page shows before there is anything on it.
fn empty(theme: &Theme, cx: &mut Context<App>) -> AnyElement {
    card(theme)
        .flex()
        .flex_col()
        .gap(px(space::ROOMY))
        .items_start()
        .px(px(space::SECTION))
        .py(px(space::SECTION))
        .child(
            div()
                .max_w(px(520.))
                .text_small()
                .text_color(theme.dim)
                .child(
                    "A variable is a piece of text with a name, or a formula that works one out. Documents print them, workflow conditions read them, and a workflow can set one as it runs.",
                ),
        )
        .child(
            button("var-add-empty", "New variable", Kind::Primary, theme)
                .on_click(cx.listener(|app, _, w, cx| app.new_var(w, cx))),
        )
        .into_any_element()
}
