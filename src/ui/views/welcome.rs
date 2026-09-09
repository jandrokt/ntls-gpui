//! The start page: what an empty workspace shows.
//!
//! The tools on the left with the shortcut that opens each, and the other ways
//! in on the right.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px,
};

use crate::core::Tool;
use crate::ui::app::{App, Page};
use crate::ui::icons::icon;
use crate::ui::theme::Theme;
use crate::ui::widgets::{Type, keycap, space};

impl App {
    pub(super) fn welcome(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let tools: Vec<AnyElement> = self
            .registry
            .all()
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, t)| tool_line(i, t, theme, cx))
            .collect();

        div()
            .id("welcome")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .bg(theme.bg)
            .overflow_y_scroll()
            .track_scroll(&self.welcome_scroll)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::SECTION))
                    .w_full()
                    // Wide enough for the longest thing a tool says about
                    // itself. At 860 the description column came out around
                    // 280 pixels and every other line was cut off mid-word,
                    // with half the window empty beside it.
                    .max_w(px(1080.))
                    .px(px(48.))
                    .pt(px(56.))
                    .pb(px(48.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::TIGHT))
                            .child(
                                div()
                                    .text_heading()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child("ntls"),
                            )
                            .child(
                                div()
                                    .text_small()
                                    .text_color(theme.dim)
                                    .child(format!(
                                        "Network diagnostics and transfers · {}",
                                        crate::sys::build_label()
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(48.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(space::TIGHT))
                                    .child(heading("Tools", theme))
                                    .children(tools),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .w(px(260.))
                                    .flex_shrink_0()
                                    .gap(px(space::TIGHT))
                                    .child(heading("Actions", theme))
                                    .child(shortcut(
                                        "welcome-palette",
                                        "search",
                                        "Search",
                                        &crate::sys::shortcut_label("cmd-k"),
                                        theme,
                                        cx.listener(|app, _, window, cx| {
                                            app.open_palette(window, cx)
                                        }),
                                    ))
                                    .child(shortcut(
                                        "welcome-open",
                                        "folder-open",
                                        "Open folder as workspace",
                                        "",
                                        theme,
                                        cx.listener(|app, _, _, cx| app.open_folder(cx)),
                                    ))
                                    .child(shortcut(
                                        "welcome-machine",
                                        "globe",
                                        "Network interfaces",
                                        "",
                                        theme,
                                        cx.listener(|app, _, window, cx| {
                                            app.show_page(Page::Interfaces, window, cx)
                                        }),
                                    ))
                                    .child(shortcut(
                                        "welcome-workspace",
                                        "explorer",
                                        "New workspace",
                                        &crate::sys::shortcut_label("cmd-t"),
                                        theme,
                                        cx.listener(|app, _, _, cx| app.new_workspace(cx)),
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn heading(label: &str, theme: &Theme) -> AnyElement {
    div()
        .text_caps()
        .text_color(theme.faint)
        .pb(px(space::TIGHT))
        .child(label.to_uppercase())
        .into_any_element()
}

/// One tool, on one line, with the key that opens it.
fn tool_line(
    position: usize,
    tool: Arc<dyn Tool>,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let id = tool.id();
    div()
        .id(SharedString::from(format!("open-{id}")))
        .flex()
        .items_center()
        .gap(px(space::ROOMY))
        .h(px(30.))
        .px(px(space::SNUG))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(|s| s.bg(theme.hover))
        .on_click(cx.listener(move |app, _, window, cx| {
            if let Some(t) = app.registry.get(id) {
                app.add_tool(t, None, None, window, cx);
            }
        }))
        .child(icon(tool.icon(), px(15.), theme.accent))
        .child(
            div()
                .w(px(96.))
                .flex_shrink_0()
                .text_small()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(tool.title()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_small()
                .text_color(theme.dim)
                .truncate()
                .child(tool.desc()),
        )
        // The tools a number key opens say so, since this page is the only
        // place those keys are advertised.
        .when_some(number_key(position), |d, key| d.child(keycap(&key, theme)))
        .into_any_element()
}

/// How many tools a number key opens: `app::bind_keys` binds ⌘1 through ⌘8,
/// and there is no ninth action for a ninth key to reach.
const NUMBER_KEYS: usize = 8;

/// The number key that opens the tool in this position, labelled for this
/// system, or nothing if no key reaches it.
fn number_key(position: usize) -> Option<String> {
    // This page counted to nine while the bindings stopped at eight, so the
    // ninth tool wore a ⌘9 keycap that nothing answered to: the one place the
    // number keys are taught was teaching a key that does nothing.
    (position < NUMBER_KEYS).then(|| crate::sys::shortcut_label(&format!("cmd-{}", position + 1)))
}

fn shortcut(
    id: &'static str,
    icon_name: &'static str,
    label: &str,
    key: &str,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(space::SNUG))
        .h(px(30.))
        .px(px(space::SNUG))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(|s| s.bg(theme.hover))
        .on_click(on_click)
        .child(icon(icon_name, px(14.), theme.accent))
        .child(div().flex_1().min_w_0().text_small().text_color(theme.text).truncate().child(label.to_string()))
        .when(!key.is_empty(), |d| d.child(keycap(key, theme)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{NUMBER_KEYS, number_key};

    /// There are ten built-in tools and eight number keys, so the line
    /// between a tool that has one and a tool that does not falls inside the
    /// list and has to be drawn in the right place. A keycap on a tool past
    /// the last binding is a shortcut the application will not honour.
    #[test]
    fn a_tool_is_given_a_keycap_only_when_a_number_key_actually_opens_it() {
        for position in 0..NUMBER_KEYS {
            assert!(number_key(position).is_some(), "tool {position} lost its number key");
        }
        assert_eq!(number_key(NUMBER_KEYS), None, "ninth tool advertises an unbound key");
        assert_eq!(number_key(NUMBER_KEYS + 1), None);
    }

    /// The keycap counts from one while the position counts from zero, and
    /// the keycap has to name the key `app::bind_keys` bound, not the one
    /// next to it.
    #[test]
    fn the_keycap_names_the_key_that_opens_that_particular_tool() {
        let first = crate::sys::shortcut_label("cmd-1");
        let last = crate::sys::shortcut_label("cmd-8");
        assert_eq!(number_key(0), Some(first));
        assert_eq!(number_key(NUMBER_KEYS - 1), Some(last));
    }
}
