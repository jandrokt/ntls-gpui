//! Drawing the menu that a right-click opened.
//!
//! It is one absolutely-positioned panel over everything else, with a
//! transparent sheet behind it that closes it when you click away — the same
//! shape as the palette, and for the same reason: a menu that can be left open
//! behind another click is a menu you have to think about.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::ui::app::App;
use crate::ui::icons::icon;
use crate::ui::menu::{Act, Item};
use crate::ui::store::Tag;
use crate::ui::theme::Theme;
use crate::ui::widgets::{Type, space};

/// Wide enough for the longest thing a menu says, narrow enough to sit under
/// the pointer rather than across the window.
const WIDTH: Pixels = px(238.);
/// The gap kept between the menu and the edge of the window.
const MARGIN: f32 = 8.;

impl App {
    pub(super) fn menu_overlay(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(menu) = self.menu.clone() else { return div().into_any_element() };

        // A menu that would hang off the bottom opens upwards from the
        // pointer instead, which is what every menu everywhere does.
        let viewport = window.viewport_size();
        let height = menu.height();
        let left = f32::from(menu.at.x).min(f32::from(viewport.width - WIDTH) - MARGIN).max(MARGIN);
        let below = f32::from(menu.at.y);
        let top = if below + f32::from(height) + MARGIN > f32::from(viewport.height) {
            (below - f32::from(height)).max(MARGIN)
        } else {
            below
        };

        let rows: Vec<AnyElement> =
            menu.items.iter().enumerate().map(|(i, item)| row(i, item, theme, cx)).collect();

        div()
            .absolute()
            .inset_0()
            // The sheet takes the click that dismisses the menu, including the
            // right-click that would otherwise open a second one.
            .child(
                div()
                    .id("menu-sheet")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|app, _, _, cx| app.close_menu(cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|app, _, _, cx| app.close_menu(cx)),
                    ),
            )
            .child(
                div()
                    .id("menu-panel")
                    // The panel takes its own clicks. Without this the press
                    // falls through to the sheet behind it, the menu closes,
                    // and the release lands on nothing.
                    .occlude()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(WIDTH)
                    .flex()
                    .flex_col()
                    .py(px(4.))
                    .rounded(px(8.))
                    .bg(theme.panel)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .children(rows),
            )
            .into_any_element()
    }
}

fn row(index: usize, item: &Item, theme: &Theme, cx: &mut Context<App>) -> AnyElement {
    match item {
        Item::Separator => div()
            .h(px(1.))
            .my(px(3.))
            .mx(px(space::SNUG))
            .bg(theme.rule)
            .into_any_element(),

        Item::Heading(label) => div()
            .flex()
            .items_center()
            .h(px(22.))
            .px(px(space::ROOMY))
            .text_caps()
            .text_color(theme.faint)
            .child(label.clone())
            .into_any_element(),

        Item::Choice { label, icon: glyph, act, danger } => {
            let (fg, act) = (if *danger { theme.down } else { theme.text }, act.clone());
            div()
                .id(SharedString::from(format!("menu-{index}")))
                .flex()
                .items_center()
                .gap(px(space::SNUG))
                .h(px(24.))
                .px(px(space::ROOMY))
                .cursor_pointer()
                .hover(|s| s.bg(theme.hover))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(14.))
                        .flex_shrink_0()
                        .children(glyph.map(|g| icon(g, px(13.), fg.opacity(0.75)))),
                )
                .child(div().flex_1().min_w_0().text_small().text_color(fg).truncate().child(label.clone()))
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.choose(act.clone(), window, cx)
                }))
                .into_any_element()
        }

        Item::Colours { current, act, subject } => {
            let (act, subject, current) = (*act, *subject, *current);
            div()
                .flex()
                .items_center()
                .gap(px(space::TIGHT))
                .h(px(30.))
                .px(px(space::ROOMY))
                .children(Tag::ALL.into_iter().map(|tag| {
                    swatch(tag, tag == current, act(subject, tag), theme, cx)
                }))
                .into_any_element()
        }
    }
}

/// One colour in a menu's colour row.
fn swatch(tag: Tag, selected: bool, act: Act, theme: &Theme, cx: &mut Context<App>) -> AnyElement {
    let body = div()
        .id(SharedString::from(format!("menu-tag-{}", tag.label())))
        .flex()
        .items_center()
        .justify_center()
        .size(px(18.))
        .rounded_full()
        .cursor_pointer()
        .border_2()
        .border_color(if selected { theme.text } else { gpui::transparent_black() })
        .on_click(cx.listener(move |app, _, window, cx| app.choose(act.clone(), window, cx)));

    match tag.color() {
        Some(colour) => body.child(div().size(px(12.)).rounded_full().bg(colour)),
        // "No colour" is a ring, not an absence.
        None => body.child(
            div().size(px(12.)).rounded_full().border_1().border_color(theme.faint),
        ),
    }
    .into_any_element()
}
