//! The small pieces the screens are assembled from.
//!
//! Everything here obeys one type scale and one spacing rhythm, so the
//! screens stay in proportion without each one deciding for itself.

use gpui::{
    AnyElement, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px, relative,
};

use super::theme::{MONO, Theme};

/// The type scale.
///
/// Four sizes and one line box. Naming the sizes is what stops a fourteenth
/// variant of "slightly smaller text" appearing halfway down a file; sharing
/// the line box is what stops two of them drifting apart when they land in the
/// same row.
pub mod text {
    use gpui::{Pixels, px};
    /// Page and panel headings, and the name of the thing you are looking at.
    pub const HEADING: Pixels = px(15.);
    /// Everything you read.
    pub const BODY: Pixels = px(13.);
    /// Controls, tabs, table cells, secondary lines.
    pub const SMALL: Pixels = px(12.);
    /// Labels, counts, hints, the status bar.
    pub const META: Pixels = px(11.);
    /// Group headings, which are shouted rather than sized up. The same size
    /// as a label, in capitals and heavier.
    pub const CAPS: Pixels = META;

    /// The one line box, shared by every size in the interface.
    ///
    /// GPUI places a run's baseline at
    /// `(line_height - ascent - descent) / 2 + ascent`, so two runs that
    /// differ in size or family sit at different heights however the row
    /// around them is aligned — the drifting, superscript look. Pinning every
    /// size to one box is what removes the whole class of problem, and it is
    /// why nothing in this interface calls `text_size` without also calling
    /// `line_height`. Use the [`Type`] methods and neither can be forgotten.
    pub const LINE: Pixels = px(18.);
}

/// The type scale, as methods. Every piece of text in ntls picks its size
/// through one of these, so size and line box are never set apart.
pub trait Type: Styled + Sized {
    fn text_heading(self) -> Self {
        self.text_size(text::HEADING).line_height(text::LINE)
    }
    fn text_body(self) -> Self {
        self.text_size(text::BODY).line_height(text::LINE)
    }
    fn text_small(self) -> Self {
        self.text_size(text::SMALL).line_height(text::LINE)
    }
    fn text_meta(self) -> Self {
        self.text_size(text::META).line_height(text::LINE)
    }
    /// A group heading: small, heavy capitals.
    fn text_caps(self) -> Self {
        self.text_size(text::CAPS).line_height(text::LINE).font_weight(FontWeight::SEMIBOLD)
    }
    /// Figures, addresses and anything else read column by column.
    fn mono(self) -> Self {
        self.font_family(MONO)
    }
}

impl<T: Styled + Sized> Type for T {}

/// The spacing scale. Every gap in the interface is one of these.
pub mod space {
    /// Inside a control: an icon and its label.
    pub const TIGHT: f32 = 6.;
    /// Between controls that belong together.
    pub const SNUG: f32 = 8.;
    /// Between groups of controls.
    pub const ROOMY: f32 = 12.;
    /// The gutter every pane lines up against.
    pub const GUTTER: f32 = 16.;
    /// Between sections down a page.
    pub const SECTION: f32 = 20.;
}

/// A rounded label: a status, a count, a mode.
pub fn pill(label: impl Into<SharedString>, fg: Hsla, bg: Hsla) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .px(px(6.))
        .h(text::LINE)
        .rounded(px(4.))
        .bg(bg)
        .text_caps()
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .whitespace_nowrap()
        .child(label.into())
}

/// A small dot standing in for a status where there is no room for a word.
pub fn dot(color: Hsla) -> impl IntoElement {
    div().size(px(6.)).rounded_full().bg(color).flex_shrink_0()
}

/// A ring rather than a disc, for a status that is in progress.
pub fn ring(color: Hsla) -> impl IntoElement {
    div().size(px(7.)).rounded_full().border_2().border_color(color).flex_shrink_0()
}

/// One figure in the summary strip: the number first, its name underneath.
///
/// A stat bar that reads `sent 14 recv 14 loss 0%` makes you parse pairs; this
/// makes the numbers scannable and the labels available when you need them.
pub fn figure(label: &str, value: &str, theme: &Theme, accent: Option<Hsla>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .min_w(px(52.))
        .child(
            div()
                .mono()
                .text_body()
                .font_weight(FontWeight::MEDIUM)
                .text_color(accent.unwrap_or(theme.text))
                .whitespace_nowrap()
                .child(value.to_string()),
        )
        .child(
            div()
                .text_caps()
                .text_color(theme.faint)
                .whitespace_nowrap()
                .child(label.to_string()),
        )
        .into_any_element()
}

/// A slim progress bar. `label` replaces the percentage for progress that is
/// not a count of things.
pub fn progress_bar(done: usize, total: usize, label: Option<&str>, theme: &Theme) -> AnyElement {
    let fraction = if total == 0 { 0.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) };
    let text = match label {
        Some(l) => l.to_string(),
        None => format!("{done} / {total}"),
    };

    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_1()
                .h(px(4.))
                .rounded_full()
                .bg(theme.track)
                .overflow_hidden()
                .child(div().h_full().w(relative(fraction)).rounded_full().bg(theme.accent)),
        )
        .child(
            div()
                .mono()
                .text_meta()
                .text_color(theme.dim)
                .whitespace_nowrap()
                .child(text),
        )
        .into_any_element()
}

/// How a button reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The one thing this screen is for.
    Primary,
    /// Everything else.
    Normal,
    /// Destructive or interrupting.
    Danger,
    /// A borderless word.
    Ghost,
    /// A borderless word that stays lit while whatever it controls is on.
    Toggle(bool),
}

/// A clickable button. The caller attaches the handler, since only it knows
/// what the click means.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: Kind,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let transparent = gpui::transparent_black();
    let (bg, fg, border) = match kind {
        Kind::Primary => (theme.accent, theme.on_accent, theme.accent),
        Kind::Normal => (theme.raised, theme.text, theme.border),
        Kind::Danger => (theme.down_soft, theme.down, theme.down.opacity(0.3)),
        Kind::Ghost => (transparent, theme.dim, transparent),
        Kind::Toggle(true) => (theme.accent_soft, theme.accent, transparent),
        Kind::Toggle(false) => (transparent, theme.dim, transparent),
    };
    let hover_bg = match kind {
        Kind::Primary => theme.accent.opacity(0.86),
        Kind::Danger => theme.down.opacity(0.16),
        _ => theme.hover.blend(bg),
    };

    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px(px(10.))
        .h(px(26.))
        .rounded(px(6.))
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_small()
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .active(|s| s.opacity(0.7))
        .child(label.into())
}

/// A borderless square button holding one icon.
pub fn icon_button(
    id: impl Into<ElementId>,
    name: &str,
    active: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let (bg, fg) = if active {
        (theme.accent_soft, theme.accent)
    } else {
        (gpui::transparent_black(), theme.dim)
    };
    let hover = theme.hover;
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .size(px(26.))
        .rounded(px(6.))
        .bg(bg)
        .text_color(fg)
        .cursor_pointer()
        .hover(move |s| s.bg(hover).text_color(theme.text))
        .child(super::icons::icon(name, px(15.), fg))
}

/// A keycap, for the one or two shortcuts worth putting on screen.
pub fn keycap(key: &str, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .h(text::LINE)
        .px(px(4.))
        .rounded(px(3.))
        .bg(theme.track)
        .text_caps()
        .text_color(theme.faint)
        .whitespace_nowrap()
        .child(key.to_string())
        .into_any_element()
}

/// A raised surface: cards and the palette.
pub fn card(theme: &Theme) -> gpui::Div {
    div().rounded(px(10.)).bg(theme.panel).border_1().border_color(theme.border)
}
