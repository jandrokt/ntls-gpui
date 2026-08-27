//! One palette, in a light and a dark cut.
//!
//! The surfaces are named for their place in the frame rather than for what
//! they look like, and the frame is a code editor's: an activity bar of view
//! icons, a side bar listing what is open, an editor area with a tab per
//! thing, a panel underneath it, and a status bar along the bottom. Naming
//! them this way is what keeps the two cuts in step, and a third would be one
//! more constructor.

use gpui::{App, Global, Hsla, WindowAppearance, rgb, rgba};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Light,
    Dark,
}

impl Mode {
    pub fn from_appearance(a: WindowAppearance) -> Mode {
        match a {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
            _ => Mode::Light,
        }
    }

    pub fn flipped(self) -> Mode {
        match self {
            Mode::Light => Mode::Dark,
            Mode::Dark => Mode::Light,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub mode: Mode,

    /// The narrow rail of view icons down the far edge.
    pub activity: Hsla,
    /// The list beside it: what this workspace holds.
    pub sidebar: Hsla,
    /// The titlebar, and anything else that frames rather than holds.
    pub chrome: Hsla,
    /// The editor: the content everything else frames.
    pub bg: Hsla,
    /// The panel under the editor, and cards lifted off the content.
    pub panel: Hsla,
    /// A control sitting on a panel.
    pub raised: Hsla,
    /// A groove cut into a control: the empty half of a progress bar, a
    /// keycap.
    pub track: Hsla,

    /// The tab you are looking at, which is the editor surface continued
    /// upwards.
    pub tab_active: Hsla,
    /// Every other open tab.
    pub tab_inactive: Hsla,

    /// The bar along the bottom, which takes the accent so the window has one
    /// piece of colour that is always there.
    pub status: Hsla,
    pub status_fg: Hsla,

    /// A row or button under the pointer.
    pub hover: Hsla,
    /// The selected row.
    pub selected: Hsla,
    /// The outline on whatever has the keyboard.
    pub focus: Hsla,

    pub border: Hsla,
    /// Separators inside a panel, where a full border would be too loud.
    pub rule: Hsla,

    pub text: Hsla,
    pub dim: Hsla,
    pub faint: Hsla,

    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub on_accent: Hsla,

    pub up: Hsla,
    pub down: Hsla,
    pub warn: Hsla,

    /// The tint behind a status colour, for pills and row highlights.
    pub up_soft: Hsla,
    pub down_soft: Hsla,
    pub warn_soft: Hsla,
}

impl Theme {
    pub fn dark() -> Theme {
        Theme {
            mode: Mode::Dark,
            activity: rgb(0x121212).into(),
            sidebar: rgb(0x181818).into(),
            chrome: rgb(0x141414).into(),
            bg: rgb(0x1e1e1e).into(),
            panel: rgb(0x181818).into(),
            raised: rgb(0x282828).into(),
            track: rgba(0xffffff14).into(),
            tab_active: rgb(0x1e1e1e).into(),
            tab_inactive: rgb(0x141414).into(),
            status: rgb(0x262626).into(),
            status_fg: rgb(0xeeeeee).into(),
            hover: rgba(0xffffff08).into(),
            selected: rgba(0xffffff10).into(),
            focus: rgb(0x5ea2ff).into(),
            border: rgb(0x2d2d2d).into(),
            rule: rgba(0xffffff0f).into(),
            text: rgb(0xeeeeee).into(),
            dim: rgb(0xa0a0a0).into(),
            faint: rgb(0x666666).into(),
            accent: rgb(0x5ea2ff).into(),
            accent_soft: rgba(0x5ea2ff1f).into(),
            on_accent: rgb(0x121212).into(),
            up: rgb(0x4ec965).into(),
            down: rgb(0xf85149).into(),
            warn: rgb(0xe5b542).into(),
            up_soft: rgba(0x4ec9651f).into(),
            down_soft: rgba(0xf851491f).into(),
            warn_soft: rgba(0xe5b5421f).into(),
        }
    }

    pub fn light() -> Theme {
        Theme {
            mode: Mode::Light,
            activity: rgb(0xe4e9f0).into(),
            sidebar: rgb(0xf1f4f8).into(),
            chrome: rgb(0xe9edf3).into(),
            bg: rgb(0xffffff).into(),
            panel: rgb(0xf7f9fc).into(),
            raised: rgb(0xeef2f7).into(),
            track: rgba(0x0b0e1414).into(),
            tab_active: rgb(0xffffff).into(),
            tab_inactive: rgb(0xe7ecf3).into(),
            status: rgb(0x0b6bcb).into(),
            status_fg: rgb(0xffffff).into(),
            hover: rgba(0x0b0e1410).into(),
            selected: rgba(0x0b6bcb1f).into(),
            focus: rgb(0x0b6bcb).into(),
            border: rgb(0xdae0e9).into(),
            rule: rgba(0x0b0e1412).into(),
            text: rgb(0x18202b).into(),
            dim: rgb(0x55637a).into(),
            faint: rgb(0x8593a6).into(),
            accent: rgb(0x0b6bcb).into(),
            accent_soft: rgba(0x0b6bcb17).into(),
            on_accent: rgb(0xffffff).into(),
            up: rgb(0x18794e).into(),
            down: rgb(0xc02a26).into(),
            warn: rgb(0x9a6700).into(),
            up_soft: rgba(0x18794e16).into(),
            down_soft: rgba(0xc02a2616).into(),
            warn_soft: rgba(0x9a670016).into(),
        }
    }

    pub fn of(mode: Mode) -> Theme {
        match mode {
            Mode::Dark => Theme::dark(),
            Mode::Light => Theme::light(),
        }
    }

    /// The colour a result row's status is drawn in.
    pub fn status(&self, s: crate::core::Status) -> Hsla {
        match s {
            crate::core::Status::Up => self.up,
            crate::core::Status::Down => self.down,
            crate::core::Status::Warn => self.warn,
            crate::core::Status::Info => self.dim,
        }
    }

    /// The tint behind a status pill.
    pub fn status_soft(&self, s: crate::core::Status) -> Hsla {
        match s {
            crate::core::Status::Up => self.up_soft,
            crate::core::Status::Down => self.down_soft,
            crate::core::Status::Warn => self.warn_soft,
            crate::core::Status::Info => self.track,
        }
    }

    /// The colour a log line's level is drawn in.
    pub fn level(&self, l: crate::core::Level) -> Hsla {
        match l {
            crate::core::Level::Good => self.up,
            crate::core::Level::Warn => self.warn,
            crate::core::Level::Error => self.down,
            crate::core::Level::Info => self.dim,
        }
    }
}

impl Global for Theme {}

/// The active theme.
pub fn theme(cx: &App) -> Theme {
    cx.try_global::<Theme>().copied().unwrap_or_else(Theme::dark)
}

/// Data reads better in a monospace face; the chrome reads better without
/// one. The text system falls back to whatever it can find.
///
/// Reach for these through [`crate::ui::widgets::Type`] rather than directly:
/// a line of text must be all one family and all one size, and the type scale
/// is what enforces it.
pub const MONO: &str = "Menlo";
pub const UI_FONT: &str = ".SystemUIFont";
