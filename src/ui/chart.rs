//! A live line chart.
//!
//! Anything producing a number over time (ping latency, either speed test)
//! gets one of these without asking, because the samples a tool emits
//! are enough to draw it.

use gpui::{
    Bounds, Hsla, IntoElement, ParentElement, PathBuilder, Pixels, Styled, Window, canvas, div,
    point, px, quad,
};

use super::job::Series;
use super::theme::Theme;
use super::widgets::Type;

/// A graph small enough to sit in a list row: the shape of the series and
/// nothing else. No axis, no numbers, no block characters pretending to be a
/// picture.
pub fn spark(
    values: Vec<Option<f64>>,
    color: gpui::Hsla,
    width: Pixels,
    height: Pixels,
) -> impl IntoElement {
    let (lo, hi) = bounds_of(&values);

    div().w(width).h(height).flex_shrink_0().py(px(3.)).child(
        canvas(
            move |_, _, _| {},
            move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
                if values.len() < 2 {
                    return;
                }
                let span = (hi - lo).max(f64::EPSILON);
                let step = bounds.size.width / (values.len() - 1) as f32;
                let at = |i: usize, v: f64| {
                    let y = bounds.bottom() - bounds.size.height * ((v - lo) / span) as f32;
                    point(bounds.left() + step * i as f32, y.clamp(bounds.top(), bounds.bottom()))
                };

                draw(
                    window,
                    &values,
                    &at,
                    bounds.bottom(),
                    color.opacity(0.18).into(),
                    color,
                    px(1.2),
                );
            },
        )
        .size_full(),
    )
}

/// Draws a series as a filled area under a line, one unbroken run of samples
/// at a time.
///
/// The runs are what makes a gap a gap. A ping that stops answering for a
/// minute used to be a straight line from the last reply to the first one
/// after it, which reads as a minute of steady latency rather than a minute
/// of nothing, and the longer the outage the more confident the lie looked.
fn draw(
    window: &mut Window,
    values: &[Option<f64>],
    at: &dyn Fn(usize, f64) -> gpui::Point<Pixels>,
    baseline: Pixels,
    fill: gpui::Background,
    line: Hsla,
    weight: Pixels,
) {
    for run in runs(values) {
        // A single reading between two outages has no line to be part of, so
        // it is drawn as the point it is rather than dropped.
        if let [(i, v)] = run[..] {
            let p = at(i, v);
            let r = weight.max(px(1.5));
            window.paint_quad(quad(
                Bounds::from_corners(point(p.x - r, p.y - r), point(p.x + r, p.y + r)),
                r,
                line,
                px(0.),
                gpui::transparent_black(),
                Default::default(),
            ));
            continue;
        }

        let (first, last) = (run[0], run[run.len() - 1]);
        let mut area = PathBuilder::fill();
        area.move_to(point(at(first.0, first.1).x, baseline));
        for &(i, v) in &run {
            area.line_to(at(i, v));
        }
        area.line_to(point(at(last.0, last.1).x, baseline));
        area.close();
        if let Ok(path) = area.build() {
            window.paint_path(path, fill);
        }

        let mut stroke = PathBuilder::stroke(weight);
        for (n, &(i, v)) in run.iter().enumerate() {
            let p = at(i, v);
            if n == 0 {
                stroke.move_to(p);
            } else {
                stroke.line_to(p);
            }
        }
        if let Ok(path) = stroke.build() {
            window.paint_path(path, line);
        }
    }
}

/// The stretches of consecutive samples that have a value, each with the
/// position it sits at, so the missing ones still take up their share of the
/// width.
fn runs(values: &[Option<f64>]) -> Vec<Vec<(usize, f64)>> {
    let mut runs: Vec<Vec<(usize, f64)>> = Vec::new();
    let mut open = false;
    for (i, v) in values.iter().enumerate() {
        match v {
            Some(v) => {
                if !open {
                    runs.push(Vec::new());
                    open = true;
                }
                runs.last_mut().expect("just opened a run").push((i, *v));
            }
            None => open = false,
        }
    }
    runs
}

/// Renders one series as a filled area with a line along its top, an axis, and
/// the numbers that make the axis readable.
pub fn chart(series: &Series, theme: &Theme, height: Pixels) -> impl IntoElement {
    let values = series.values.clone();
    let (line_color, fill_top, fill_bottom) = (
        theme.accent,
        theme.accent.opacity(0.22),
        theme.accent.opacity(0.02),
    );
    let (grid, axis_text) = (theme.rule, theme.faint);

    let (lo, hi) = bounds_of(&values);
    let unit = series.unit.trim().to_string();
    // What it reads right now, which is nothing at all while the thing being
    // watched is not answering: the last number known is not the current one.
    let latest = series.latest();

    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_meta()
                        .text_color(theme.dim)
                        .child(series.name.clone()),
                )
                .child(
                    // Same size and family as the label beside it: weight and
                    // colour are what make it the figure.
                    div()
                        .text_meta()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(match latest {
                            Some(v) => format!("{}{unit}", fmt_axis(v)),
                            None => "—".into(),
                        }),
                ),
        )
        .child(
            div().h(height).w_full().child(
                canvas(
                    move |_, _, _| {},
                    move |bounds: Bounds<Pixels>, _, window: &mut Window, cx: &mut gpui::App| {
                        paint(
                            bounds,
                            window,
                            cx,
                            &values,
                            lo,
                            hi,
                            line_color,
                            fill_top,
                            fill_bottom,
                            grid,
                            axis_text,
                            &unit,
                        );
                    },
                )
                .size_full(),
            ),
        )
}

/// The value range to draw, padded so a flat line does not sit on the floor
/// and a spike does not touch the ceiling. Samples with no value say nothing
/// about the range.
fn bounds_of(values: &[Option<f64>]) -> (f64, f64) {
    let mut present = values.iter().flatten().copied().peekable();
    if present.peek().is_none() {
        return (0.0, 1.0);
    }
    let lo = present.clone().fold(f64::INFINITY, f64::min);
    let hi = present.fold(f64::NEG_INFINITY, f64::max);
    if (hi - lo).abs() < f64::EPSILON {
        let pad = if hi.abs() < f64::EPSILON { 1.0 } else { hi.abs() * 0.2 };
        return (lo - pad, hi + pad);
    }
    let pad = (hi - lo) * 0.12;
    ((lo - pad).max(0.0), hi + pad)
}

#[allow(clippy::too_many_arguments)]
fn paint(
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut gpui::App,
    values: &[Option<f64>],
    lo: f64,
    hi: f64,
    line_color: Hsla,
    fill_top: Hsla,
    fill_bottom: Hsla,
    grid: Hsla,
    axis_text: Hsla,
    unit: &str,
) {
    // Room on the left for the axis labels, which would otherwise sit on top
    // of the line.
    let gutter = px(52.);
    let plot = Bounds::from_corners(
        point(bounds.left() + gutter, bounds.top() + px(4.)),
        point(bounds.right(), bounds.bottom() - px(2.)),
    );
    if plot.size.width <= px(4.) || plot.size.height <= px(4.) {
        return;
    }

    // Four horizontal rules with their values: enough to read a
    // magnitude off without turning the panel into graph paper.
    const LINES: usize = 4;
    let style = window.text_style();
    let font_size = px(10.);
    for i in 0..=LINES {
        let t = i as f32 / LINES as f32;
        let y = plot.top() + plot.size.height * t;
        window.paint_quad(quad(
            Bounds::from_corners(point(plot.left(), y), point(plot.right(), y + px(1.))),
            px(0.),
            grid,
            px(0.),
            gpui::transparent_black(),
            Default::default(),
        ));

        let value = hi - (hi - lo) * t as f64;
        let label = format!("{}{unit}", fmt_axis(value));
        let run = gpui::TextRun {
            len: label.len(),
            font: style.font(),
            color: axis_text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(label.into(), font_size, &[run], None);
        let x = (plot.left() - px(8.) - line.width).max(bounds.left());
        let _ = line.paint(point(x, y - font_size / 2.), font_size * 1.3, window, cx);
    }

    if values.len() < 2 {
        return;
    }

    let span = (hi - lo).max(f64::EPSILON);
    let step = plot.size.width / (values.len() - 1) as f32;
    let at = |i: usize, v: f64| {
        let y = plot.bottom() - plot.size.height * ((v - lo) / span) as f32;
        point(plot.left() + step * i as f32, y.clamp(plot.top(), plot.bottom()))
    };

    draw(
        window,
        values,
        &at,
        plot.bottom(),
        gpui::linear_gradient(
            180.,
            gpui::linear_color_stop(fill_top, 0.),
            gpui::linear_color_stop(fill_bottom, 1.),
        ),
        line_color,
        px(1.6),
    );

    // A dot on the newest sample, the one being watched. There is none while
    // the series is in a gap: nothing is arriving to watch.
    if let Some(last) = values.last().copied().flatten() {
        let p = at(values.len() - 1, last);
        let r = px(3.);
        window.paint_quad(quad(
            Bounds::from_corners(point(p.x - r, p.y - r), point(p.x + r, p.y + r)),
            r,
            line_color,
            px(0.),
            gpui::transparent_black(),
            Default::default(),
        ));
    }
}

/// Axis labels want to be short before they want to be precise.
fn fmt_axis(v: f64) -> String {
    let a = v.abs();
    match a {
        _ if a >= 1000.0 => format!("{v:.0}"),
        _ if a >= 100.0 => format!("{v:.0}"),
        _ if a >= 10.0 => format!("{v:.1}"),
        _ if a >= 1.0 => format!("{v:.2}"),
        _ if a > 0.0 => format!("{v:.3}"),
        _ => "0".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{bounds_of, runs};

    #[test]
    fn a_missing_sample_breaks_the_line_in_two() {
        let drawn = runs(&[Some(1.0), Some(2.0), None, None, Some(9.0), Some(8.0)]);

        assert_eq!(drawn.len(), 2, "a gap ends one run and starts another");
        assert_eq!(drawn[0], vec![(0, 1.0), (1, 2.0)]);
        // The second run keeps its distance from the first: the outage takes
        // up the width it lasted for.
        assert_eq!(drawn[1], vec![(4, 9.0), (5, 8.0)]);
    }

    #[test]
    fn a_series_with_nothing_in_it_draws_nothing() {
        assert!(runs(&[None, None]).is_empty());
        // And it asks for no particular range, rather than one built out of
        // infinities.
        assert_eq!(bounds_of(&[None, None]), (0.0, 1.0));
    }

    #[test]
    fn the_range_comes_from_the_samples_that_have_a_value() {
        let (lo, hi) = bounds_of(&[Some(10.0), None, Some(20.0)]);
        assert!(lo < 10.0 && lo >= 0.0, "padded below, never under zero");
        assert!(hi > 20.0, "padded above");
    }
}
