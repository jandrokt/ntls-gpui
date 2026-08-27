//! The icon set, compiled into the binary.
//!
//! Unicode has a symbol for almost anything, and almost none of them are the
//! symbol you wanted at twelve pixels. These are drawn for the size they are
//! shown at, take the colour of the text around them, and mean one thing each.

use std::borrow::Cow;

use gpui::{AssetSource, Hsla, IntoElement, Pixels, Result, SharedString, Styled, svg};

/// Every icon, by the name the interface refers to it as.
macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const FILES: &[(&str, &[u8])] = &[
            $((
                concat!("icons/", $name, ".svg"),
                include_bytes!(concat!("../../assets/icons/", $name, ".svg")),
            )),*
        ];
    };
}

icons![
    "ping",
    "traceroute",
    "ipscan",
    "portscan",
    "dns",
    "subdomains",
    "speedtest",
    "throughput",
    "star",
    "star-filled",
    "plus",
    "close",
    "search",
    "moon",
    "sun",
    "download",
    "explorer",
    "panel",
    "sidebar",
    "gear",
    "chevron-right",
    "chevron-down",
    "chevron-up",
    "play",
    "stop",
    "refresh",
    "filter",
    "chart",
    "globe",
    "link",
    "folder-open",
    "folder",
    "note",
    "compare",
];

/// Serves the icons to GPUI's SVG renderer.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES.iter().find(|(name, _)| *name == path).map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

/// An icon at a given size, in a given colour.
pub fn icon(name: &str, size: Pixels, color: Hsla) -> impl IntoElement {
    svg().path(format!("icons/{name}.svg")).size(size).flex_none().text_color(color)
}
