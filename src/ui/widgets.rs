//! The small pieces the screens are assembled from.
//!
//! Everything here obeys one type scale and one spacing rhythm, so the
//! screens stay in proportion without each one deciding for itself.

use std::time::Duration;

use gpui::{
    AnimationExt, AnyElement, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, ease_in_out,
    prelude::FluentBuilder, px, relative,
};

use super::theme::{MONO, Theme};

/// How long things take to move.
///
/// Two durations, for the same reason there are four text sizes. `QUICK` is a
/// control answering a click and should feel like the click caused it.
/// `SETTLE` is something arriving unasked, and can take long enough to be
/// noticed.
pub mod motion {
    use std::time::Duration;
    pub const QUICK: Duration = Duration::from_millis(130);
    pub const SETTLE: Duration = Duration::from_millis(240);
}

/// Whether the interface moves at all. Read wherever an animation is built,
/// so one switch covers all of them.
static MOVES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_motion(on: bool) {
    MOVES.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn motion_on() -> bool {
    MOVES.load(std::sync::atomic::Ordering::Relaxed)
}

/// An animation that runs once, easing in and out.
///
/// With motion off it still runs, over a single frame: the animator gets one
/// call with a delta of 1, the finished state. Callers need not check.
pub fn once(duration: Duration) -> gpui::Animation {
    let duration = if motion_on() { duration } else { Duration::from_millis(1) };
    gpui::Animation::new(duration).with_easing(ease_in_out)
}

/// One choice in a [`Segments`]: what it says, and what picking it does.
pub struct Segment {
    pub label: SharedString,
    pub pick: Box<dyn Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static>,
}

impl Segment {
    pub fn new(
        label: impl Into<SharedString>,
        pick: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    ) -> Segment {
        Segment { label: label.into(), pick: Box::new(pick) }
    }
}

/// A row of choices with the current one lit, and the light sliding between
/// them.
///
/// The options are equal widths, so the lit part can be one
/// element that moves instead of a highlight that goes out here and comes on
/// over there. `from` is where the light is coming from; the caller remembers
/// that, because an element cannot remember what it was last time.
pub struct Segments {
    pub id: String,
    pub options: Vec<Segment>,
    pub current: usize,
    pub from: usize,
}

impl Segments {
    pub fn render(self, theme: &Theme) -> AnyElement {
        let Segments { id, options, current, from } = self;
        let count = options.len().max(1);
        let share = 1. / count as f32;
        let (from, current) = (from.min(count - 1), current.min(count - 1));

        let labels: Vec<AnyElement> = options
            .into_iter()
            .enumerate()
            .map(|(i, option)| {
                div()
                    .id(SharedString::from(format!("{id}-{i}")))
                    .relative()
                    .flex()
                    .flex_1()
                    // Equal widths, and it takes both of these to get them:
                    // without `min_w_0` the longest label sets a floor the
                    // others cannot match, and the light is one element
                    // sliding across equal shares, so it lands between two of
                    // them.
                    .min_w_0()
                    .items_center()
                    .justify_center()
                    .h(px(22.))
                    .px(px(space::TIGHT))
                    .text_small()
                    // `truncate` and not the three styles it stands for:
                    // setting them by hand left the label drawing at its full
                    // width and running into its neighbour, so a row of seven
                    // choices in a narrow box read as one word of nonsense.
                    // A choice too narrow to read is still a poor choice, and
                    // that is what `Field::wide` is for, but no arrangement
                    // of them should ever overlap.
                    .truncate()
                    .cursor_pointer()
                    .text_color(if i == current { theme.on_accent } else { theme.dim })
                    .when(i != current, |d| d.hover(|s| s.text_color(theme.text)))
                    .child(option.label)
                    .on_click(option.pick)
                    .into_any_element()
            })
            .collect();

        div()
            .relative()
            .flex()
            // The row fills whatever it is given, so the shares it divides
            // into are the same width whatever the labels say.
            .w_full()
            .p(px(2.))
            .rounded(px(6.))
            .bg(theme.raised)
            .border_1()
            .border_color(theme.border)
            // The light, under the labels and moving between them.
            .child(
                div()
                    .absolute()
                    .top(px(2.))
                    .bottom(px(2.))
                    .w(relative(share))
                    .rounded(px(4.))
                    .bg(theme.accent)
                    .with_animation(
                        SharedString::from(format!("{id}-lit-{from}-{current}")),
                        once(motion::QUICK),
                        move |d, delta| {
                            let at = from as f32 + (current as f32 - from as f32) * delta;
                            d.left(relative(at * share))
                        },
                    ),
            )
            .child(div().relative().flex().flex_1().min_w_0().children(labels))
            .into_any_element()
    }
}

/// A switch, and the knob sliding across it.
///
/// The movement is the whole point: a switch that changes without moving
/// leaves you checking whether you actually hit it. The element ids carry the
/// state, so flipping it starts the animation over from where the knob was
/// instead of from wherever it last happened to be.
pub fn switch(id: &str, on: bool, theme: &Theme) -> AnyElement {
    const TRACK: f32 = 32.;
    const KNOB: f32 = 14.;
    const INSET: f32 = 2.;
    let travel = TRACK - KNOB - INSET * 2.;
    // Where the knob is going, as a fraction of its travel: forwards when the
    // switch is being turned on, back again when it is being turned off.
    let at = move |delta: f32| if on { delta } else { 1. - delta };

    div()
        .relative()
        .w(px(TRACK))
        .h(px(KNOB + INSET * 2.))
        .rounded_full()
        .bg(theme.track)
        // The colour arrives with the knob and does not switch under it.
        .child(
            div().absolute().inset_0().rounded_full().bg(theme.accent).with_animation(
                SharedString::from(format!("{id}-fill-{on}")),
                once(motion::QUICK),
                move |d, delta| d.opacity(at(delta)),
            ),
        )
        .child(
            div()
                .absolute()
                .top(px(INSET))
                .size(px(KNOB))
                .rounded_full()
                .bg(if on { theme.on_accent } else { theme.faint })
                .with_animation(
                    SharedString::from(format!("{id}-knob-{on}")),
                    once(motion::QUICK),
                    move |d, delta| d.left(px(INSET + travel * at(delta))),
                ),
        )
        .into_any_element()
}

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
    /// Group headings, which are shouted, not sized up. The same size
    /// as a label, in capitals and heavier.
    pub const CAPS: Pixels = META;

    /// The one line box, shared by every size in the interface.
    ///
    /// GPUI places a run's baseline at
    /// `(line_height - ascent - descent) / 2 + ascent`, so two runs that
    /// differ in size or family sit at different heights however the row
    /// around them is aligned: the drifting, superscript look. Pinning every
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

/// A ring instead of a disc, for a status that is in progress.
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
        // A floor wide enough that a row of figures reads as a row of
        // columns. At 52 a wide value like `254/254` pushed its neighbour
        // along and the strip came out ragged, which is the one thing a line
        // of summary numbers should not be. Not wider: seven figures still
        // have to fit the pane at the narrowest the window opens to.
        .min_w(px(64.))
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
    /// One of several sitting inside a [`bar`]: the same toggle, without a
    /// frame of its own, because the bar around it is the frame.
    Tab(bool),
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
        Kind::Tab(true) => (theme.bg, theme.text, transparent),
        Kind::Tab(false) => (transparent, theme.dim, transparent),
    };
    let hover_bg = match kind {
        Kind::Primary => theme.accent.opacity(0.86),
        Kind::Danger => theme.down.opacity(0.16),
        _ => theme.hover.blend(bg),
    };

    let inside = matches!(kind, Kind::Tab(_));
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px(px(10.))
        .h(px(if inside { 22. } else { 26. }))
        .rounded(px(if inside { 5. } else { 6. }))
        .bg(bg)
        .when(!inside, |d| d.border_1().border_color(border))
        .text_small()
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .active(|s| s.opacity(0.7))
        .child(label.into())
}

/// The frame around a row of [`Kind::Tab`] buttons.
///
/// Four toggles side by side with nothing around them read as four loose
/// words, not as one control with one of its choices lit: the only thing
/// telling you they belong together was that they happened to be adjacent.
/// The frame is what says "pick one of these", and it is what the lit one is
/// lit against.
pub fn bar(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(1.))
        .flex_shrink_0()
        .p(px(2.))
        .rounded(px(7.))
        .bg(theme.raised)
        .border_1()
        .border_color(theme.border)
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
