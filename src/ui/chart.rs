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
pub fn spark(values: Vec<f64>, color: gpui::Hsla, width: Pixels, height: Pixels) -> impl IntoElement {
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

                let mut area = PathBuilder::fill();
                area.move_to(point(bounds.left(), bounds.bottom()));
                for (i, v) in values.iter().enumerate() {
                    area.line_to(at(i, *v));
                }
                area.line_to(point(bounds.right(), bounds.bottom()));
                area.close();
                if let Ok(path) = area.build() {
                    window.paint_path(path, color.opacity(0.18));
                }

                let mut stroke = PathBuilder::stroke(px(1.2));
                for (i, v) in values.iter().enumerate() {
                    let p = at(i, *v);
                    if i == 0 {
                        stroke.move_to(p);
                    } else {
                        stroke.line_to(p);
                    }
                }
                if let Ok(path) = stroke.build() {
                    window.paint_path(path, color);
                }
            },
        )
        .size_full(),
    )
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
    let latest = values.last().copied();

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
/// and a spike does not touch the ceiling.
fn bounds_of(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 1.0);
    }
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
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
    values: &[f64],
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

    // The filled area first, so the line sits on top of its own shading.
    let mut area = PathBuilder::fill();
    area.move_to(point(plot.left(), plot.bottom()));
    for (i, v) in values.iter().enumerate() {
        area.line_to(at(i, *v));
    }
    area.line_to(point(plot.right(), plot.bottom()));
    area.close();
    if let Ok(path) = area.build() {
        window.paint_path(
            path,
            gpui::linear_gradient(
                180.,
                gpui::linear_color_stop(fill_top, 0.),
                gpui::linear_color_stop(fill_bottom, 1.),
            ),
        );
    }

    let mut stroke = PathBuilder::stroke(px(1.6));
    for (i, v) in values.iter().enumerate() {
        let p = at(i, *v);
        if i == 0 {
            stroke.move_to(p);
        } else {
            stroke.line_to(p);
        }
    }
    if let Ok(path) = stroke.build() {
        window.paint_path(path, line_color);
    }

    // A dot on the newest sample, the one being watched.
    if let Some(last) = values.last() {
        let p = at(values.len() - 1, *last);
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
