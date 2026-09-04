//! The frame around the work.
//!
//! It is a code editor's frame, because the shape of the job is a code
//! editor's shape: a set of things you have open, one of them in front of you,
//! and a lot of output underneath. So there is an activity bar of views down
//! the far edge, a side bar listing what this workspace holds, a tab per open
//! tool across the top of the editor, an output panel beneath it, and a status
//! bar along the bottom. Nothing here knows what a tool does: a tab, a tree
//! row and a breadcrumb are built from the same `Job` the pane is.

use gpui::{
    AnimationExt, AnyElement, AppContext, Context, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};

use crate::ui::app::{App, Dest, Page, View};
use crate::ui::chart::spark;
use crate::ui::icons::icon;
use crate::ui::job::{Job, State};
use crate::ui::theme::Theme;
use crate::ui::widgets::{Kind, Type, button, dot, keycap, motion, once, pill, ring, space, text};
use crate::ui::workspace::{Group, Item, Stage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DragItem(pub Item);

struct DragPreview {
    icon: &'static str,
    name: String,
    theme: Theme,
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // What is under the pointer while it is being carried: the row, small
        // and slightly see-through, so it reads as a thing in flight rather
        // than a second copy of the side bar.
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(24.))
            .px(px(8.))
            .max_w(px(220.))
            .rounded(px(6.))
            .bg(self.theme.panel)
            .border_1()
            .border_color(self.theme.accent)
            .shadow_lg()
            .opacity(0.92)
            .child(icon(self.icon, px(13.), self.theme.accent))
            .child(
                div()
                    .text_small()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(self.theme.text)
                    .whitespace_nowrap()
                    .truncate()
                    .child(self.name.clone()),
            )
    }
}

/// What a wordless control is, shown when the pointer rests on it.
///
/// The rail is icons and nothing else, which keeps it narrow, and a
/// row of actions on a variable is the same bargain; a name on hover is what
/// stops either being a guessing game.
pub(super) struct Tip {
    pub label: &'static str,
    pub theme: Theme,
}

impl Render for Tip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .h(px(22.))
            .px(px(space::SNUG))
            .rounded(px(5.))
            .bg(self.theme.panel)
            .border_1()
            .border_color(self.theme.border)
            .shadow_lg()
            .text_small()
            .text_color(self.theme.text)
            .whitespace_nowrap()
            .child(self.label)
    }
}

/// How a row says it will take what is being dragged.
///
/// One answer everywhere: the same tint and the same outline, so a drop
/// target is recognisable wherever the pointer happens to be.
fn takes_a_drop(style: gpui::StyleRefinement, tint: Hsla, edge: Hsla) -> gpui::StyleRefinement {
    style.bg(tint).border_1().border_color(edge)
}

/// The traffic lights need this much room before the titlebar's own content
/// can start.
/// The room the window's own buttons need at the left of the titlebar.
///
/// macOS puts them inside the window and ntls draws its own strip behind
/// them; Windows and Linux put theirs at the right, in a titlebar the system
/// draws itself, so there is nothing to leave room for.
const TRAFFIC_LIGHTS: Pixels = if cfg!(target_os = "macos") {
    px(crate::sys::TITLEBAR_INSET)
} else {
    px(8.)
};
/// Whether ntls draws the window's buttons into its own titlebar. Only
/// Windows: macOS draws its three itself and Linux keeps its titlebar.
const OWN_CAPTION: bool = cfg!(target_os = "windows");
/// What Windows turns the close button under the pointer. Every window on the
/// desktop uses this red, so a window that picked its own would stand out for
/// the wrong reason.
const CLOSE_HOVER: u32 = 0xc42b1c;
/// The rail of view icons.
const ACTIVITY_WIDTH: Pixels = px(48.);
/// One row of a tree, a tab strip, or the status bar. Three heights for the
/// whole frame, and every one of them a multiple of the line box.
const ROW: Pixels = px(24.);
const TAB_BAR: Pixels = px(35.);
const STATUS: Pixels = px(24.);

impl App {
    // --- the title bar ----------------------------------------------------

    /// The strip the window is dragged by, with the workspace it is showing
    /// named in the middle of it.
    pub(super) fn titlebar(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = self.workspace().name();
        let tag = self.workspace().tag.color();
        let busy = self.workspaces.iter().filter(|w| w.is_busy()).count();
        let (sidebar_open, panel_open) = (self.sidebar_open, self.panel_open);

        div()
            .id("titlebar")
            // With the system titlebar hidden, this strip is what the user
            // drags the window by.
            //
            // On macOS and Linux saying so once, here, is enough: the system
            // moves a window dragged by a transparent titlebar, and anything
            // interactive on the strip takes its own clicks first.
            //
            // Windows is told nothing. Declaring a region a window control
            // there hands the whole press to the system before ntls sees it,
            // which is how the buttons came to do nothing and the strip came
            // to drag nothing: whether that press ever arrives depends on a
            // hit test that is asked and answered a frame out of step. So
            // every part of the strip stays ordinary content, gpui delivers
            // the clicks it always would, and the two things that need the
            // system's help ask for it by name in `crate::sys`.
            .when(!OWN_CAPTION, |d| {
                d.window_control_area(gpui::WindowControlArea::Drag).on_click(|event, window, _| {
                    if event.click_count() >= 2 {
                        window.zoom_window();
                    }
                })
            })
            .flex()
            .items_center()
            .h(px(crate::sys::TITLEBAR_HEIGHT))
            .flex_shrink_0()
            .pl(TRAFFIC_LIGHTS)
            // The buttons sit flush in the corner, the way every other window
            // on the desktop has them, so there is no padding to their right.
            .pr(if OWN_CAPTION { px(0.) } else { px(space::SNUG) })
            .gap(px(space::SNUG))
            .bg(theme.chrome)
            .border_b_1()
            .border_color(theme.border)
            // Which workspace you are in sits beside the window's own
            // controls, where a document's name sits in every other window.
            .child(
                div()
                    .id("titlebar-workspace")
                    .flex()
                    .items_center()
                    .gap(px(space::TIGHT))
                    .h(px(22.))
                    .px(px(space::SNUG))
                    .max_w(px(240.))
                    .flex_shrink_0()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.hover))
                    .child(icon("folder-open", px(13.), theme.dim))
                    .children(tag.map(|c| div().size(px(7.)).rounded_full().bg(c).flex_shrink_0()))
                    .child(
                        div()
                            .min_w_0()
                            .text_small()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .truncate()
                            .child(name),
                    )
                    .when(busy > 0, |d| d.child(ring(theme.accent)))
                    .on_click(cx.listener(|app, _, window, cx| {
                        app.show_view(View::Workspaces, window, cx)
                    })),
            )
            // The command centre, with the room either side of it given over
            // to dragging the window. Only the box takes clicks: a click
            // target wider than the thing it looks like is a click target
            // that catches drags of the window.
            .child(
                div().flex().items_center().h_full().flex_1().min_w_0().child(drag_spacer()).child(
                    div()
                        .id("command-centre")
                        .flex()
                        .items_center()
                        .gap(px(space::TIGHT))
                        .h(px(24.))
                        .pl(px(space::SNUG))
                        .pr(px(3.))
                        .w(px(300.))
                        .flex_shrink_0()
                        .rounded(px(6.))
                        .bg(theme.bg)
                        .border_1()
                        .border_color(theme.border)
                        .cursor_pointer()
                        .hover(|s| s.border_color(theme.focus))
                        .child(icon("search", px(12.), theme.faint))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_small()
                                .text_color(theme.faint)
                                .truncate()
                                .child("Search or run a command"),
                        )
                        .child(keycap(&crate::sys::shortcut_label("cmd-k"), theme))
                        .on_click(cx.listener(|app, _, window, cx| app.open_palette(window, cx))),
                )
                .child(drag_spacer()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .flex_shrink_0()
                    .child(
                        chrome_button("toggle-sidebar", "sidebar", sidebar_open, theme)
                            .on_click(cx.listener(|app, _, _, cx| app.toggle_sidebar(cx))),
                    )
                    .child(
                        chrome_button("toggle-panel", "panel", panel_open, theme)
                            .on_click(cx.listener(|app, _, _, cx| app.toggle_panel(cx))),
                    )
                    ,
            )
            .when(OWN_CAPTION, |d| d.child(caption_buttons(theme, window)))
            .into_any_element()
    }

    // --- the activity bar -------------------------------------------------

    /// The rail: everywhere the window goes, and the way to reach the palette
    /// from the mouse.
    ///
    /// Views and pages are drawn by the same function on purpose. Behind the
    /// icon they are different things (one fills the side bar, one fills the
    /// window) but in front of it they are all just places, and a rail whose
    /// icons disagree about their own size and colour reads as two rails.
    pub(super) fn activity_bar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        // A badge says something is happening somewhere you are not looking.
        // A count of things that merely exist is noise on every icon.
        let elsewhere = self
            .workspaces
            .iter()
            .enumerate()
            .filter(|(i, w)| *i != self.active && w.is_busy())
            .count();
        let here = self.workspace().jobs.iter().filter(|j| j.state.is_running()).count();
        let count_for = |dest: Dest| match dest {
            Dest::View(View::Workspaces) => elsewhere,
            Dest::View(View::Tools) => here,
            _ => 0,
        };
        let places: Vec<AnyElement> = Dest::RAIL
            .into_iter()
            .map(|dest| rail_icon(dest, self.showing(dest), count_for(dest), theme, cx))
            .collect();
        let settings = Dest::Page(Page::Settings);
        let settings = rail_icon(settings, self.showing(settings), 0, theme, cx);

        div()
            .flex()
            .flex_col()
            .items_center()
            .w(ACTIVITY_WIDTH)
            .flex_shrink_0()
            .py(px(space::TIGHT))
            .bg(theme.activity)
            .border_r_1()
            .border_color(theme.border)
            .children(places)
            .child(div().flex_1())
            .child(
                div()
                    .id("activity-add")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(44.))
                    .flex_shrink_0()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.hover))
                    .child(icon("plus", px(20.), theme.dim))
                    .on_click(cx.listener(|app, _, window, cx| app.open_palette(window, cx))),
            )
            // The application's own settings sit at the foot, under everything
            // they are the settings for.
            .child(settings)
            .into_any_element()
    }

    // --- the side bar -----------------------------------------------------

    pub(super) fn sidebar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let view = self.view;
        let width = self.sidebar_at();
        // Closing it is a slide, so the contents keep their own width while
        // the panel narrows past them and does not reflow into a column an
        // inch wide on the way out.
        let inner = self.sidebar_width;
        let body = match view {
            View::Workspaces => self.workspaces_view(theme, cx),
            View::Tools => self.tools_view(theme, cx),
        };

        div()
            .relative()
            .flex()
            .flex_col()
            .w(px(width))
            .flex_shrink_0()
            .overflow_hidden()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .w(px(inner))
                    .flex_shrink_0()
                    .h(px(crate::sys::TITLEBAR_HEIGHT))
                    .pl(px(space::ROOMY))
                    .pr(px(space::TIGHT))
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::TIGHT))
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_caps()
                                    .text_color(theme.dim)
                                    .flex_shrink_0()
                                    .child(view.title().to_uppercase()),
                            ),
                    )
                    .children(self.sidebar_actions(view, theme, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .w(px(inner))
                    .flex_shrink_0()
                    .child(body),
            )
            // The edge is a grab handle four pixels wide: enough to
            // hit and narrow enough not to look like a scrollbar.
            .child(
                div()
                    .id("sidebar-edge")
                    .absolute()
                    .top_0()
                    .right(px(-2.))
                    .w(px(4.))
                    .h_full()
                    .cursor_col_resize()
                    .hover(|s| s.bg(theme.focus.opacity(0.5)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|app, e: &gpui::MouseDownEvent, _, _| {
                            app.begin_sidebar_resize(f32::from(e.position.x));
                        }),
                    ),
            )
            .into_any_element()
    }

    /// The buttons in a view's header, which differ per view the way an
    /// editor's do.
    fn sidebar_actions(&self, view: View, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match view {
            View::Workspaces => vec![
                chrome_button("open-folder", "folder-open", false, theme)
                    .on_click(cx.listener(|app, _, _, cx| app.open_folder(cx)))
                    .into_any_element(),
                chrome_button("new-workspace", "plus", false, theme)
                    .on_click(cx.listener(|app, _, _, cx| app.new_workspace(cx)))
                    .into_any_element(),
            ],
            View::Tools => vec![
                // A workspace is a directory, and this is the switch between
                // being told that and being shown it.
                chrome_button("toggle-files", "note", self.files_view, theme)
                    .on_click(cx.listener(|app, _, _, cx| app.toggle_files_view(cx)))
                    .into_any_element(),
                chrome_button("add-tool", "plus", false, theme)
                    .on_click(cx.listener(|app, _, w, cx| app.open_palette(w, cx)))
                    .into_any_element(),
            ],
        }
    }

    /// Every workspace: which one you are in, and how to get another.
    fn workspaces_view(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active;
        let renaming = self.renaming_workspace;
        if renaming {
            style_input(&self.ws_rename_input, theme, cx);
        }
        let rows: Vec<AnyElement> = (0..self.workspaces.len())
            .map(|i| self.workspace_row(i, i == active, theme, cx))
            .collect();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("workspaces")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .pb(px(space::SNUG))
                    .overflow_y_scroll()
                    .children(rows),
            )
            // The ways to get another workspace sit at the foot of the panel,
            // not after the last row, so they cannot be mistaken for one.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .py(px(space::TIGHT))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(theme.rule)
                    .child(sidebar_action(
                        "ws-new",
                        "plus",
                        "New workspace",
                        theme,
                        cx.listener(|app, _, _, cx| app.new_workspace(cx)),
                    ))
                    .child(sidebar_action(
                        "ws-open",
                        "folder-open",
                        "Open folder\u{2026}",
                        theme,
                        cx.listener(|app, _, _, cx| app.open_folder(cx)),
                    )),
            )
            .into_any_element()
    }

    fn workspace_row(
        &self,
        index: usize,
        active: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(ws) = self.workspaces.get(index) else { return div().into_any_element() };
        let (id, name, busy, count) = (ws.id, ws.name(), ws.is_busy(), ws.jobs.len());
        let (tag_value, tag) = (ws.tag, ws.tag.color());
        let closable = self.workspaces.len() > 1;
        let renaming = active && self.renaming_workspace;
        let rename_input = self.ws_rename_input.clone();

        tree_shell(format!("ws-{id}"), active, theme)
            .on_click(cx.listener(move |app, e: &gpui::ClickEvent, window, cx| {
                if e.click_count() >= 2 {
                    app.begin_workspace_rename(window, cx);
                } else {
                    app.select_workspace(index, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                    app.open_menu(
                        e.position,
                        crate::ui::menu::for_workspace(index, tag_value, closable),
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(16.))
                    .flex_shrink_0()
                    .child(icon(
                        if active { "folder-open" } else { "explorer" },
                        px(14.),
                        tag.unwrap_or(if active { theme.accent } else { theme.dim }),
                    )),
            )
            .child(if renaming {
                div()
                    .id("ws-rename-box")
                    // Clicking anywhere else is a way out, the way it is
                    // everywhere: the name is kept, the box closes.
                    .on_mouse_down_out(cx.listener(|app, _, window, cx| {
                        app.end_workspace_rename(window, cx)
                    }))
                    .flex()
                    .items_center()
                    .flex_1()
                    .min_w_0()
                    .h(px(20.))
                    .px(px(space::TIGHT / 2.))
                    .rounded(px(4.))
                    .bg(theme.bg)
                    .border_1()
                    .border_color(theme.focus)
                    .text_small()
                    .child(rename_input)
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .text_small()
                    .font_weight(if active { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                    .text_color(if active { theme.text } else { theme.dim })
                    .truncate()
                    .child(name)
                    .into_any_element()
            })
            .when(busy, |d| d.child(ring(theme.accent)))
            .child(count_badge(count, theme))
            .when(closable, |d| {
                d.child(
                    close_button(format!("ws-close-{id}"), theme).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |app, _, _, cx| {
                            app.close_workspace(index, cx);
                            cx.stop_propagation();
                        }),
                    ),
                )
            })
            .into_any_element()
    }

    /// The workspace directory, as it is on disk.
    ///
    /// The same things the grouped view lists, in the shape the filesystem
    /// holds them: one row per file, the folders they are in, and `.closed`
    /// where anything removed went. Clicking a file shows what it is; a file
    /// the workspace does not hold is handed to the file manager.
    fn files_view(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let showing = self.workspace().showing();
        let dir = self.workspace().dir.clone();
        let rows: Vec<crate::ui::app::FileRow> = self.workspace_files().to_vec();

        let listed: Vec<AnyElement> = rows
            .into_iter()
            .map(|row| {
                let selected = row.item.is_some() && row.item == showing;
                let (icon_name, colour) = match (row.directory, row.item) {
                    (true, _) => ("folder", theme.accent),
                    (_, Some(Item::Doc(_))) => ("note", theme.dim),
                    (_, Some(Item::Flow(_))) => ("play", theme.dim),
                    (_, Some(Item::Tool(id))) => (
                        self.workspace().job(id).map(|j| j.tool.icon()).unwrap_or("note"),
                        theme.dim,
                    ),
                    _ => ("note", theme.faint),
                };
                let known = row.directory || row.item.is_some();
                let size = (!row.directory).then(|| crate::dl::names::bytes(row.size));
                let (open_row, menu_row) = (row.clone(), row.clone());

                div()
                    .id(SharedString::from(format!("file-{}", row.path.display())))
                    .flex()
                    .items_center()
                    .gap(px(space::TIGHT))
                    .h(ROW)
                    .mx(px(6.))
                    .px(px(6.))
                    .pl(px(6. + 12. * row.depth as f32))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme.selected))
                    .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.open_file_row(open_row.clone(), cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                            let path = menu_row.path.clone();
                            let items = match menu_row.item {
                                Some(Item::Tool(id)) => {
                                    let (tag, favourite, running, open) = app
                                        .workspace()
                                        .job(id)
                                        .map(|j| (j.tag, j.favorite, j.state.is_running(), j.open))
                                        .unwrap_or_default();
                                    crate::ui::menu::for_job(crate::ui::menu::JobMenu {
                                        id,
                                        tag,
                                        running,
                                        favourite,
                                        open,
                                        comparable: !app.comparable(id).is_empty(),
                                        comparing: app
                                            .workspace()
                                            .job(id)
                                            .is_some_and(|j| j.compare_with.is_some()),
                                        can_move: app.can_move(Item::Tool(id)),
                                    })
                                }
                                Some(Item::Doc(id)) => {
                                    let (tag, favourite) = app.mark_of(Item::Doc(id));
                                    crate::ui::menu::for_doc(
                                        id,
                                        tag,
                                        favourite,
                                        app.can_move(Item::Doc(id)),
                                    )
                                }
                                Some(Item::Flow(id)) => {
                                    let (tag, favourite) = app.mark_of(Item::Flow(id));
                                    let running =
                                        app.workspace().flow(id).is_some_and(|f| f.running());
                                    crate::ui::menu::for_flow(
                                        id,
                                        running,
                                        tag,
                                        favourite,
                                        app.can_move(Item::Flow(id)),
                                    )
                                }
                                None => vec![crate::ui::menu::Item::choice(
                                    "Reveal in file manager",
                                    "link",
                                    crate::ui::menu::Act::RevealPath(
                                        path.display().to_string(),
                                    ),
                                )],
                            };
                            app.open_menu(e.position, items, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(icon(icon_name, px(13.), colour))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .mono()
                            .text_small()
                            .text_color(if known { theme.text } else { theme.faint })
                            .truncate()
                            .child(row.name),
                    )
                    .children(size.map(|size| {
                        div()
                            .flex_shrink_0()
                            .text_meta()
                            .text_color(theme.faint)
                            .child(size)
                    }))
                    .into_any_element()
            })
            .collect();

        div()
            .id("files")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .py(px(space::TIGHT))
            .overflow_y_scroll()
            .child(
                div()
                    .px(px(space::ROOMY))
                    .pb(px(space::TIGHT))
                    .text_meta()
                    .text_color(theme.faint)
                    .truncate()
                    .child(dir.display().to_string()),
            )
            .children(listed)
            .into_any_element()
    }

    /// What the open workspace holds, grouped by what stage each tool is at.
    fn tools_view(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if self.files_view {
            return self.files_view(theme, cx);
        }
        let selected = self.workspace().selected;
        let showing = self.workspace().showing();
        let mut rows: Vec<AnyElement> = Vec::new();

        // Folders first, each with what is inside it, and what is loose in
        // the workspace after them, the way a file manager lists a directory,
        // so the shape of the workspace is the first thing the panel says.
        let mut places: Vec<Option<String>> =
            self.workspace().folders().into_iter().map(Some).collect();
        places.push(None);

        let data = crate::ui::notes::Data::of(self.workspace());

        for place in places {
            let folder = place.as_deref();
            let depth = folder.map(|f| f.split('/').count().saturating_sub(1)).unwrap_or(0);

            // If this is a subfolder and any parent folder is collapsed, skip it entirely
            if let Some(f) = folder {
                let mut is_hidden = false;
                let parts: Vec<&str> = f.split('/').collect();
                for i in 1..parts.len() {
                    let parent = parts[..i].join("/");
                    if self.is_collapsed(&format!("folder:{parent}")) {
                        is_hidden = true;
                        break;
                    }
                }
                if is_hidden {
                    continue;
                }
            }

            let folder_key = folder.map(|f| format!("folder:{f}"));
            let folder_collapsed = folder_key.as_ref().map(|k| self.is_collapsed(k)).unwrap_or(false);

            let mut inside: Vec<AnyElement> = Vec::new();

            if !folder_collapsed {
                // 1. Tools by Stage
                for stage in Stage::ALL {
                    let ids: Vec<usize> =
                        self.workspace().stage_in(folder, stage).into_iter().map(|j| j.id).collect();
                    if ids.is_empty() {
                        continue;
                    }
                    let section_key = format!("section:{}:{}", folder.unwrap_or("root"), stage.title());
                    let section_collapsed = self.is_collapsed(&section_key);
                    // Every heading, wherever it is, can be emptied, and
                    // empties the heading that was clicked and not every
                    // group of that name in the workspace.
                    let group = Group::Stage(stage);
                    let here = (group, folder.map(str::to_string));
                    inside.push(section(
                        ids.len(),
                        here.clone(),
                        self.clearing.as_ref() == Some(&here),
                        folder.is_some(),
                        depth,
                        &section_key,
                        section_collapsed,
                        folder.map(str::to_string),
                        theme,
                        cx,
                    ));
                    if !section_collapsed {
                        for id in ids {
                            let Some(job) = self.workspace().job(id) else { continue };
                            inside.push(tree_row(
                                job,
                                selected == Some(id) && showing == Some(Item::Tool(id)),
                                folder.is_some(),
                                depth,
                                theme,
                                cx,
                            ));
                        }
                    }
                }

                // 2. Documents in this place
                let mut docs: Vec<(usize, String, String, crate::ui::store::Mark)> = self
                    .workspace()
                    .docs_in(folder)
                    .into_iter()
                    .map(|d| {
                        let mark = self.workspace().mark(Item::Doc(d.id));
                        (d.id, d.title(&data), d.summary(&data), mark)
                    })
                    .collect();
                // Favourites sort to the top of their group, as they do
                // everywhere else.
                docs.sort_by_key(|(_, _, _, mark)| !mark.favorite);
                if !docs.is_empty() {
                    let doc_key = format!("section:{}:Documents", folder.unwrap_or("root"));
                    let doc_collapsed = self.is_collapsed(&doc_key);
                    let here = (Group::Documents, folder.map(str::to_string));
                    inside.push(section(
                        docs.len(),
                        here.clone(),
                        self.clearing.as_ref() == Some(&here),
                        folder.is_some(),
                        depth,
                        &doc_key,
                        doc_collapsed,
                        folder.map(str::to_string),
                        theme,
                        cx,
                    ));
                    if !doc_collapsed {
                        for (id, title, summary, mark) in docs {
                            inside.push(doc_row(
                                id,
                                &title,
                                &summary,
                                showing == Some(Item::Doc(id)),
                                folder.is_some(),
                                depth,
                                mark,
                                theme,
                                cx,
                            ));
                        }
                    }
                }

                // 3. Workflows in this place
                let mut flows: Vec<(usize, String, String, bool, crate::ui::store::Mark)> = self
                    .workspace()
                    .flows_in(folder)
                    .into_iter()
                    .map(|f| {
                        let mark = self.workspace().mark(Item::Flow(f.id));
                        (f.id, f.title(), f.summary(), f.running(), mark)
                    })
                    .collect();
                flows.sort_by_key(|(_, _, _, _, mark)| !mark.favorite);
                if !flows.is_empty() {
                    let flow_key = format!("section:{}:Workflows", folder.unwrap_or("root"));
                    let flow_collapsed = self.is_collapsed(&flow_key);
                    let here = (Group::Workflows, folder.map(str::to_string));
                    inside.push(section(
                        flows.len(),
                        here.clone(),
                        self.clearing.as_ref() == Some(&here),
                        folder.is_some(),
                        depth,
                        &flow_key,
                        flow_collapsed,
                        folder.map(str::to_string),
                        theme,
                        cx,
                    ));
                    if !flow_collapsed {
                        for (id, title, summary, running, mark) in flows {
                            inside.push(flow_row(
                                id,
                                &title,
                                &summary,
                                running,
                                showing == Some(Item::Flow(id)),
                                folder.is_some(),
                                depth,
                                mark,
                                theme,
                                cx,
                            ));
                        }
                    }
                }
            }

            if let Some(name) = folder {
                let total = self.workspace().stage_in(folder, Stage::Draft).len()
                    + self.workspace().stage_in(folder, Stage::Running).len()
                    + self.workspace().stage_in(folder, Stage::Results).len()
                    + self.workspace().docs_in(folder).len()
                    + self.workspace().flows_in(folder).len();
                let folder_key_str = folder_key.unwrap();
                rows.push(folder_header(name, total, depth, &folder_key_str, folder_collapsed, theme, cx));

                // Everything the folder holds is drawn against one continuous
                // rule at the folder's own indent, so a long folder still
                // reads as one thing and the row after it plainly is not in
                // it. The rule is drawn over the rows and not beside
                // them, so the indent stays where it was.
                if !inside.is_empty() {
                    rows.push(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .absolute()
                                    .left(px(13. + 12. * depth as f32))
                                    .top_0()
                                    .bottom_0()
                                    .w(px(1.))
                                    .bg(theme.rule),
                            )
                            .children(inside)
                            .into_any_element(),
                    );
                    continue;
                }
            }
            rows.extend(inside);
        }

        if rows.is_empty() {
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::SNUG))
                    .px(px(space::ROOMY))
                    .py(px(space::ROOMY))
                    .child(
                        div()
                            .text_small()
                            .text_color(theme.dim)
                            .child("No tools in this workspace."),
                    )
                    .child(
                        button("tools-add", "Add tool", Kind::Primary, theme)
                            .w_full()
                            .on_click(cx.listener(|app, _, w, cx| app.open_palette(w, cx))),
                    )
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("tools")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .pb(px(space::SNUG))
                    .overflow_y_scroll()
                    .track_scroll(&self.rail_scroll)
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .py(px(space::TIGHT))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(theme.rule)
                    .child(sidebar_action(
                        "tools-add-tool",
                        "plus",
                        "Add tool",
                        theme,
                        cx.listener(|app, _, w, cx| app.open_palette(w, cx)),
                    ))
                    .child(sidebar_action(
                        "tools-add-doc",
                        "note",
                        "New document",
                        theme,
                        cx.listener(|app, _, w, cx| app.new_document(w, cx)),
                    ))
                    .child(sidebar_action(
                        "tools-add-flow",
                        "play",
                        "New workflow",
                        theme,
                        cx.listener(|app, _, w, cx| app.new_workflow(w, cx)),
                    ))
                    .child(sidebar_action(
                        "tools-add-folder",
                        "folder-open",
                        "New folder",
                        theme,
                        cx.listener(|app, _, w, cx| app.ask_create_folder(w, cx)),
                    )),
            )
            .into_any_element()
    }

    /// What this machine is attached to, useful to have open while a
    /// scan runs.
    // --- the editor's tab strip -------------------------------------------

    /// A tab per open tool, in the order the keyboard walks them.
    pub(super) fn tab_bar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let showing = self.workspace().showing();
        let tabs: Vec<AnyElement> = self
            .workspace()
            .open_items()
            .into_iter()
            .filter_map(|item| match item {
                Item::Tool(id) => self
                    .workspace()
                    .job(id)
                    .map(|job| tab(job, showing == Some(item), theme, cx)),
                Item::Doc(id) => {
                    let data = crate::ui::notes::Data::of(self.workspace());
                    self.workspace()
                        .doc(id)
                        .map(|doc| doc_tab(doc, doc.title(&data), showing == Some(item), theme, cx))
                }
                Item::Flow(id) => self
                    .workspace()
                    .flow(id)
                    .map(|sheet| flow_tab(sheet, showing == Some(item), theme, cx)),
            })
            .collect();

        div()
            .flex()
            
            .h(TAB_BAR)
            .flex_shrink_0()
            .bg(theme.tab_inactive)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("tabs")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll)
                    .children(tabs),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(space::TIGHT))
                    .flex_shrink_0()
                    .child(
                        chrome_button("tab-add", "plus", false, theme)
                            .on_click(cx.listener(|app, _, w, cx| app.open_palette(w, cx))),
                    ),
            )
            .into_any_element()
    }

    /// The line under the tabs saying where you are: which workspace, which
    /// stage, and what the tool is pointed at.
    pub(super) fn breadcrumbs(&mut self, theme: &Theme) -> AnyElement {
        let workspace = self.workspace().name();
        let (crumbs, target) = match self.workspace().showing() {
            Some(Item::Flow(id)) => {
                let Some(sheet) = self.workspace().flow(id) else {
                    return div().into_any_element();
                };
                (
                    [workspace, "Workflows".to_string(), sheet.title()],
                    format!("{}.flow", sheet.stem),
                )
            }
            Some(Item::Doc(id)) => {
                let Some(doc) = self.workspace().doc(id) else {
                    return div().into_any_element();
                };
                let data = crate::ui::notes::Data::of(self.workspace());
                (
                    [workspace, "Documents".to_string(), doc.title(&data)],
                    format!("{}.md", doc.stem),
                )
            }
            _ => {
                let Some(job) = self.selected_job() else { return div().into_any_element() };
                (
                    [workspace, Stage::of(&job.state).title().to_string(), job.name()],
                    job.target(),
                )
            }
        };

        div()
            .flex()
            .items_center()
            .gap(px(space::TIGHT))
            .h(px(22.))
            .px(px(space::GUTTER))
            .flex_shrink_0()
            .overflow_hidden()
            .bg(theme.bg)
            .children(crumbs.iter().enumerate().flat_map(|(i, crumb)| {
                let mut parts: Vec<AnyElement> = Vec::new();
                if i > 0 {
                    parts.push(
                        div()
                            .flex()
                            .items_center()
                            .child(icon("chevron-right", px(11.), theme.faint))
                            .into_any_element(),
                    );
                }
                parts.push(
                    div()
                        .max_w(px(220.))
                        .text_meta()
                        .text_color(if i == 2 { theme.dim } else { theme.faint })
                        .truncate()
                        .child(crumb.clone())
                        .into_any_element(),
                );
                parts
            }))
            .when(target != "—", |d| {
                d.child(
                    div()
                        .min_w_0()
                        .text_meta()
                        .text_color(theme.accent)
                        .truncate()
                        .child(target),
                )
            })
            .into_any_element()
    }

    // --- the status bar ---------------------------------------------------

    pub(super) fn status_bar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let iface = self.ifaces.iter().find(|i| i.default).or_else(|| self.ifaces.first());
        let others = self.ifaces.len().saturating_sub(1);
        let busy = self.workspaces.iter().filter(|w| w.is_busy()).count();
        let results = self.selected_job().map(|j| j.rows.len());
        let unread = self.notices.unread();
        let on = theme.status_fg;

        div()
            .flex()
            .items_center()
            .h(STATUS)
            .flex_shrink_0()
            .bg(theme.status)
            .text_color(on)
            .child(
                status_item("status-workspace", theme)
                    .max_w(px(260.))
                    .child(icon("explorer", px(11.), on))
                    .child(div().min_w_0().text_meta().truncate().child(self.workspace().name()))
                    .on_click(cx.listener(|app, _, w, cx| app.show_view(View::Workspaces, w, cx))),
            )
            .children(iface.map(|i| {
                status_item("status-iface", theme)
                    .child(icon("globe", px(11.), on))
                    .child(div().text_meta().child(format!(
                        "{} · {}",
                        i.name,
                        i.addrs
                            .first()
                            .map(|p| p.to_string())
                            .unwrap_or_else(|| "no address".into())
                    )))
                    .when(others > 0, |d| d.child(div().text_meta().child(format!("+{others}"))))
                    .on_click(cx.listener(|app, _, w, cx| app.show_page(Page::Interfaces, w, cx)))
            }))
            .child(div().flex_1())
            .when(busy > 0, |d| {
                d.child(
                    status_item("status-busy", theme)
                        .child(ring(on))
                        .child(div().text_meta().child(format!("{busy} running"))),
                )
            })
            .children(results.map(|n| {
                status_item("status-results", theme).child(div().text_meta().child(match n {
                    1 => "1 result".to_string(),
                    n => format!("{n} results"),
                }))
            }))
            .child(
                status_item("status-panel", theme)
                    .child(icon("panel", px(11.), on))
                    .child(div().text_meta().child("Output"))
                    .on_click(cx.listener(|app, _, _, cx| app.toggle_panel(cx))),
            )
            // What the application has to say for itself, and how much of it
            // has not been read.
            .child(
                status_item("status-notices", theme)
                    .child(icon("bell", px(11.), on))
                    .when(unread > 0, |d| d.child(div().text_meta().child(unread.to_string())))
                    .on_click(cx.listener(|app, _, _, cx| app.toggle_notices(cx))),
            )
            .child(
                status_item("status-add", theme)
                    .child(div().text_meta().child(crate::sys::shortcut_label("cmd-k")))
                    .on_click(cx.listener(|app, _, window, cx| app.open_palette(window, cx))),
            )
            .into_any_element()
    }
}

// --- the pieces the frame is built from -----------------------------------

/// Points a text editor at the current theme. Every inline editor in the
/// frame wants exactly this, and forgetting one leaves an invisible caret.
/// How long ago something happened, in the words a person would use.
pub(super) fn ago(at: std::time::SystemTime) -> String {
    let Ok(since) = at.elapsed() else { return "just now".into() };
    let seconds = since.as_secs();
    match seconds {
        0..=45 => "just now".into(),
        46..=5400 => format!("{} min ago", (seconds as f64 / 60.).round().max(1.) as u64),
        5401..=79200 => format!("{} h ago", (seconds as f64 / 3600.).round() as u64),
        _ => format!("{} days ago", (seconds as f64 / 86400.).round().max(1.) as u64),
    }
}

/// A text box the size of the thing it stands in for.
pub(super) fn boxed_input(
    input: gpui::Entity<crate::ui::text_input::TextInput>,
    width: Pixels,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .h(px(20.))
        .px(px(4.))
        .when(width > px(0.), |d| d.w(width).flex_shrink_0())
        .when(width == px(0.), |d| d.flex_1().min_w_0())
        .rounded(px(4.))
        .bg(theme.bg)
        .border_1()
        .border_color(theme.focus)
        .mono()
        .text_small()
        .child(input)
}

pub(super) fn style_input(input: &gpui::Entity<crate::ui::text_input::TextInput>, theme: &Theme, cx: &mut Context<App>) {
    input.update(cx, |input, _| {
        input.mono = false;
        input.text_color = theme.text;
        input.placeholder_color = theme.faint;
        input.caret_color = theme.accent;
        input.selection_color = theme.accent.opacity(0.28);
    });
}

/// A full-width row at the foot of a side bar view: an icon and a verb.
fn sidebar_action(
    id: &'static str,
    icon_name: &'static str,
    label: &str,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(space::SNUG))
        .h(ROW)
        .pl(px(space::ROOMY))
        .pr(px(space::TIGHT))
        .cursor_pointer()
        .hover(|s| s.bg(theme.hover))
        .on_click(on_click)
        .child(icon(icon_name, px(13.), theme.dim))
        .child(div().flex_1().min_w_0().text_small().text_color(theme.dim).truncate().child(label.to_string()))
        .into_any_element()
}

/// Empty room in the titlebar that the window can be dragged by.
///
/// It takes whatever width is going, so the two of them either side of the
/// command centre keep it in the middle of the strip and turn everything that
/// is not a control into somewhere to pick the window up.
///
/// Both measurements are stated rather than inherited. The row this sits in is
/// only as tall as the box it centres, so a spacer that took its height from
/// its parent would be a drag area two thirds of the way down a strip nobody
/// aims at the middle of. The margin is what leaves the top edge to the
/// system, which is what the window is resized by: Windows only offers that
/// edge where nothing else has claimed the point first.
fn drag_spacer() -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .mt(px(crate::sys::TOP_RESIZE_EDGE))
        .h(px(crate::sys::TITLEBAR_HEIGHT - crate::sys::TOP_RESIZE_EDGE))
        .when(OWN_CAPTION, |d| {
            d.on_mouse_down(MouseButton::Left, |event: &gpui::MouseDownEvent, window, _| {
                // A titlebar answers a press by moving and a double press by
                // filling the screen. Both are the system's to do.
                if event.click_count >= 2 {
                    crate::sys::toggle_maximized(window);
                } else {
                    crate::sys::drag_window(window);
                }
            })
        })
        .into_any_element()
}

/// Minimise, maximise and close, drawn into the titlebar because the system
/// one they would otherwise sit in is hidden.
///
/// Each button acts on its own click, like any other button in the window.
/// Declaring them window controls to Windows instead, which is the documented
/// way and the way that would also offer the snap layouts under the maximise
/// button, means the system decides what the press was before ntls is told
/// about it, and a press that the system decides was a caption press never
/// arrives at all. A button that works is worth more than the hover menu.
fn caption_buttons(theme: &Theme, window: &Window) -> AnyElement {
    // Maximised, the middle button puts the window back, and says so.
    let maximized = window.is_maximized();
    let (middle, middle_tip) =
        if maximized { ("window-restore", "Restore") } else { ("window-maximize", "Maximise") };

    div()
        .flex()
        .h_full()
        .flex_shrink_0()
        .child(caption_button(
            "caption-minimise",
            "window-minimize",
            "Minimise",
            (theme.hover, theme.dim),
            theme,
            |window, _| window.minimize_window(),
        ))
        .child(caption_button(
            "caption-maximise",
            middle,
            middle_tip,
            (theme.hover, theme.dim),
            theme,
            |window, _| crate::sys::toggle_maximized(window),
        ))
        // The close button is the one that goes red, and its glyph turns white
        // to stay legible on it.
        .child(caption_button(
            "caption-close",
            "window-close",
            "Close",
            (gpui::rgb(CLOSE_HOVER).into(), gpui::white()),
            theme,
            // ntls has one window, so closing it is quitting, and quitting is
            // what writes out everything a run finished a moment ago.
            |_, cx| cx.quit(),
        ))
        .into_any_element()
}

/// One of the window's own buttons.
///
/// An svg is not told what colour to be by the element around it, so the glyph
/// names both of its own states: an icon left to inherit would be painted in
/// no colour at all, which is to say not painted. The group is what lets
/// hovering the button change the panel behind the glyph and the glyph itself
/// in one move.
fn caption_button(
    id: &'static str,
    glyph: &str,
    tip: &'static str,
    // What the button turns under the pointer: the panel behind the glyph,
    // and the glyph.
    hover: (Hsla, Hsla),
    theme: &Theme,
    press: fn(&mut Window, &mut gpui::App),
) -> AnyElement {
    let (hover_bg, hover_fg) = hover;
    let group = SharedString::from(id);
    div()
        .id(id)
        .group(group.clone())
        .flex()
        .items_center()
        .justify_center()
        .w(px(crate::sys::CAPTION_BUTTON))
        .h_full()
        .flex_shrink_0()
        .hover(move |s| s.bg(hover_bg))
        .on_click(move |_, window, cx| press(window, cx))
        .tooltip({
            let theme = *theme;
            move |_, cx| cx.new(|_| Tip { label: tip, theme }).into()
        })
        .child(
            gpui::svg()
                .path(format!("icons/{glyph}.svg"))
                .size(px(10.))
                .flex_none()
                .text_color(theme.dim)
                .group_hover(group, move |s| s.text_color(hover_fg)),
        )
        .into_any_element()
}

/// A borderless icon button, for the frame's own controls.
fn chrome_button(
    id: impl Into<gpui::ElementId>,
    name: &str,
    active: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let fg = if active { theme.text } else { theme.dim };
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded(px(5.))
        .when(active, |d| d.bg(theme.hover))
        .cursor_pointer()
        .hover(move |s| s.bg(theme.hover))
        .child(icon(name, px(14.), fg))
}

/// One place in the rail, lit down its inner edge while it is the one showing.
///
/// The indicator grows into place instead of appearing, which makes
/// moving between two of them read as one thing moving.
fn rail_icon(
    dest: Dest,
    active: bool,
    count: usize,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    // A badge on the place you are already looking at is noise; on the ones
    // you are not, it is the only sign there is anything there.
    let badge = (!active && count > 0).then_some(count);
    let fg = if active { theme.text } else { theme.dim };
    div()
        .id(SharedString::from(format!("rail-{}", dest.icon())))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .size(px(44.))
        .flex_shrink_0()
        .cursor_pointer()
        .hover(move |s| s.bg(theme.hover))
        .tooltip({
            let (label, theme) = (dest.title(), *theme);
            move |_, cx| cx.new(|_| Tip { label, theme }).into()
        })
        .child(icon(dest.icon(), px(20.), fg))
        .when(active, |d| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .w(px(2.))
                    .rounded_r(px(2.))
                    .bg(theme.accent)
                    .with_animation(
                        SharedString::from(format!("rail-mark-{}", dest.icon())),
                        once(motion::QUICK),
                        |d, delta| {
                            let inset = px(6. + 12. * (1. - delta));
                            d.opacity(delta).top(inset).bottom(inset)
                        },
                    ),
            )
        })
        .children(badge.map(|n| {
            div()
                .absolute()
                .top(px(6.))
                .right(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .min_w(text::LINE)
                .h(text::LINE)
                .px(px(3.))
                .rounded_full()
                .bg(theme.accent)
                .text_meta()
                .text_color(theme.on_accent)
                .child(n.to_string())
        }))
        .on_click(cx.listener(move |app, _, window, cx| app.go(dest, window, cx)))
        .into_any_element()
}


fn section(
    count: usize,
    group: (Group, Option<String>),
    armed: bool,
    nested: bool,
    depth: usize,
    key: &str,
    collapsed: bool,
    folder: Option<String>,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let col_key = key.to_string();
    let folder_for_drop = folder.clone();
    let (tint, edge) = (theme.accent_soft, theme.accent);
    div()
        .id(SharedString::from(format!("section-{key}")))
        .group("section")
        .flex()
        .items_center()
        .gap(px(space::TIGHT / 2.))
        .h(ROW)
        // A heading inside a folder is indented under it.
        .pl(px(space::SNUG + if nested { 16. + 12. * depth as f32 } else { 0. }))
        .pr(px(space::SNUG))
        .mt(px(space::TIGHT))
        .cursor_pointer()
        .drag_over::<DragItem>(move |style, _, _, _| takes_a_drop(style, tint, edge))
        .on_drop(cx.listener(move |app, dragged: &DragItem, _, cx| {
            app.drop_in_folder(dragged.0, folder_for_drop.clone(), cx);
        }))
        .on_click(cx.listener(move |app, _, _, cx| {
            app.toggle_collapsed(&col_key, cx);
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener({
                let (group, folder) = group.clone();
                move |app, e: &gpui::MouseDownEvent, _, cx| {
                    let items = crate::ui::menu::for_group(group, folder.clone());
                    app.open_menu(e.position, items, cx);
                    cx.stop_propagation();
                }
            }),
        )
        .child(icon(if collapsed { "chevron-right" } else { "chevron-down" }, px(12.), theme.faint))
        .child(div().text_caps().text_color(theme.dim).child(group.0.title().to_uppercase()))
        .child(count_badge(count, theme))
        .child(div().flex_1())
        .child({
            let (group, folder) = group;
            let word = match (armed, group.is_stop()) {
                (true, true) => format!("Stop {count}"),
                (true, false) => format!("Delete {count}"),
                (false, true) => "Stop all".to_string(),
                (false, false) => "Delete all".to_string(),
            };
            div()
                // The key names the place as well as the group, so the two
                // headings called Documents have two ids.
                .id(SharedString::from(format!("clear-{key}")))
                .px(px(space::TIGHT))
                .rounded(px(4.))
                .text_meta()
                .when(armed, |d| d.bg(theme.down_soft).text_color(theme.down))
                .when(!armed, |d| d.text_color(theme.faint).hover(|s| s.text_color(theme.down)))
                .cursor_pointer()
                .child(word)
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.close_group(group, folder.clone(), cx);
                    cx.stop_propagation();
                }))
        })
        .into_any_element()
}

fn count_badge(count: usize, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(text::LINE)
        .h(text::LINE)
        .px(px(4.))
        .rounded_full()
        .bg(theme.track)
        .text_meta()
        .text_color(theme.faint)
        .flex_shrink_0()
        .child(count.to_string())
        .into_any_element()
}

/// A folder's name, above the groups inside it.
fn folder_header(
    name: &str,
    count: usize,
    depth: usize,
    key: &str,
    collapsed: bool,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let folder_name = name.to_string();
    let folder_name_drop = folder_name.clone();
    let col_key = key.to_string();
    let display_name = name.rsplit_once('/').map(|(_, sub)| sub).unwrap_or(name);
    let (tint, edge) = (theme.accent_soft, theme.accent);
    div()
        .id(SharedString::from(format!("folder-{name}")))
        .flex()
        .items_center()
        .gap(px(space::TIGHT))
        .h(px(26.))
        .mx(px(6.))
        .pl(px(6. + 12. * depth as f32))
        .pr(px(6.))
        .mt(px(space::SNUG))
        .rounded(px(4.))
        .bg(theme.chrome.opacity(0.5))
        .cursor_pointer()
        .hover(|s| s.bg(theme.hover))
        .drag_over::<DragItem>(move |style, _, _, _| takes_a_drop(style, tint, edge))
        .on_drop(cx.listener(move |app, dragged: &DragItem, _, cx| {
            app.drop_in_folder(dragged.0, Some(folder_name_drop.clone()), cx);
        }))
        .on_click(cx.listener(move |app, _, _, cx| {
            app.toggle_collapsed(&col_key, cx);
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                app.open_menu(e.position, crate::ui::menu::for_folder(folder_name.clone()), cx);
                cx.stop_propagation();
            }),
        )
        .child(icon(if collapsed { "chevron-right" } else { "chevron-down" }, px(11.), theme.faint))
        .child(icon(if collapsed { "folder" } else { "folder-open" }, px(13.), theme.accent))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_small()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .truncate()
                .child(display_name.to_string()),
        )
        .child(count_badge(count, theme))
        .into_any_element()
}

/// The shell every side bar row shares: one height, one indent, and a lit
/// The shell every side bar row shares: one height, one indent.
fn tree_shell(id: String, selected: bool, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .gap(px(space::TIGHT))
        .h(ROW)
        .pl(px(space::ROOMY))
        .pr(px(space::TIGHT))
        .flex_shrink_0()
        .cursor_pointer()
        .when(selected, |d| d.bg(theme.selected))
        .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
}

pub(super) fn close_button(id: String, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .justify_center()
        .size(text::LINE)
        .flex_shrink_0()
        .rounded(px(3.))
        .text_meta()
        .text_color(theme.faint)
        .hover(|s| s.bg(theme.track).text_color(theme.text))
        .child("×")
}

/// One tool in the side bar: what it is, what it is pointed at, and how it is
/// getting on.
fn tree_row(
    job: &Job,
    selected: bool,
    nested: bool,
    depth: usize,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let id = job.id;
    let running = job.state.is_running();
    let (tag_value, tag) = (job.tag, job.tag.color());
    let (favourite, open) = (job.favorite, job.open);
    let digest = job.digest();
    let samples: Option<Vec<f64>> = job.spark(36).map(<[f64]>::to_vec);
    let has_secondary = !digest.is_empty() || samples.is_some();
    let theme_drag = *theme;
    let job_name = job.name().to_string();
    let tool_icon = job.tool.icon();
    let (tint, edge) = (theme.accent_soft, theme.accent);

    div()
        .id(SharedString::from(format!("row-{id}")))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .mx(px(6.))
        .my(px(1.))
        .px(px(6.))
        .py(px(4.))
        .when(nested, |d| d.ml(px(18. + 12. * depth as f32)))
        .rounded(px(6.))
        .cursor_pointer()
        .when(selected, |d| d.bg(theme.selected))
        .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
        .on_drag(DragItem(Item::Tool(id)), move |_, _, _, cx| {
            cx.new(|_| DragPreview {
                icon: tool_icon,
                name: job_name.clone(),
                theme: theme_drag,
            })
        })
        .drag_over::<DragItem>(move |style, _, _, _| takes_a_drop(style, tint, edge))
        .on_drop(cx.listener(move |app, dragged: &DragItem, _, cx| {
            app.drop_on_item(dragged.0, Item::Tool(id), cx);
        }))
        .on_click(cx.listener(move |app, _, _, cx| app.select_job(id, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                app.select_job(id, cx);
                app.open_menu(
                    e.position,
                    crate::ui::menu::for_job(crate::ui::menu::JobMenu {
                        id,
                        tag: tag_value,
                        running,
                        favourite,
                        open,
                        comparable: !app.comparable(id).is_empty(),
                        comparing: app
                            .workspace()
                            .job(id)
                            .is_some_and(|j| j.compare_with.is_some()),
                        can_move: app.can_move(Item::Tool(id)),
                    }),
                    cx,
                );
                cx.stop_propagation();
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(20.))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(16.))
                        .flex_shrink_0()
                        .child(icon(
                            job.tool.icon(),
                            px(14.),
                            tag.unwrap_or(if selected { theme.accent } else { theme.dim }),
                        )),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_small()
                        .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                        .text_color(if job.open { theme.text } else { theme.dim })
                        .truncate()
                        .child(job.name()),
                )
                .when(favourite, |d| {
                    d.child(div().flex_shrink_0().child(icon("star-filled", px(11.), theme.warn)))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_small()
                        .text_color(theme.faint)
                        .truncate()
                        .child(job.target()),
                )
                .child(match &job.state {
                    s if s.is_running() => ring(theme.accent).into_any_element(),
                    State::Failed(_) | State::Stopped => {
                        dot(theme.status(job.state.status())).into_any_element()
                    }
                    _ => div().w(px(0.)).into_any_element(),
                })
                .child(close_button(format!("row-close-{id}"), theme).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |app, _, _, cx| {
                        app.close_job(id, cx);
                        cx.stop_propagation();
                    }),
                )),
        )
        .when(has_secondary, |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pl(px(22.))
                    .pt(px(2.))
                    .min_w_0()
                    .children(samples.map(|s| {
                        div()
                            .flex()
                            .items_center()
                            .px(px(3.))
                            .py(px(1.))
                            .rounded(px(3.))
                            .bg(theme.track.opacity(0.6))
                            .child(spark(s, if running { theme.accent } else { theme.faint }, px(36.), px(12.)))
                    }))
                    .when(!digest.is_empty(), |d| {
                        d.child(
                            div()
                                .mono()
                                .text_meta()
                                .text_color(if running { theme.accent } else { theme.dim })
                                .min_w_0()
                                .truncate()
                                .child(digest.clone()),
                        )
                    }),
            )
        })
        .into_any_element()
}

/// One document in the side bar.
#[allow(clippy::too_many_arguments)]
fn doc_row(
    id: usize,
    title: &str,
    summary: &str,
    selected: bool,
    nested: bool,
    depth: usize,
    mark: crate::ui::store::Mark,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let has_summary = !summary.is_empty();
    let (tint, edge) = (theme.accent_soft, theme.accent);
    let (theme_drag, doc_name) = (*theme, title.to_string());

    div()
        .id(SharedString::from(format!("doc-{id}")))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .mx(px(6.))
        .my(px(1.))
        .px(px(6.))
        .py(px(4.))
        .when(nested, |d| d.ml(px(18. + 12. * depth as f32)))
        .rounded(px(6.))
        .cursor_pointer()
        .when(selected, |d| d.bg(theme.selected))
        .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
        .on_drag(DragItem(Item::Doc(id)), move |_, _, _, cx| {
            cx.new(|_| DragPreview {
                icon: "note",
                name: doc_name.clone(),
                theme: theme_drag,
            })
        })
        .drag_over::<DragItem>(move |style, _, _, _| takes_a_drop(style, tint, edge))
        .on_drop(cx.listener(move |app, dragged: &DragItem, _, cx| {
            app.drop_on_item(dragged.0, Item::Doc(id), cx);
        }))
        .on_click(cx.listener(move |app, _, _, cx| app.select_doc(id, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                let (tag, favourite) = app.mark_of(Item::Doc(id));
                app.open_menu(e.position, crate::ui::menu::for_doc(id, tag, favourite, app.can_move(Item::Doc(id))), cx);
                cx.stop_propagation();
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(20.))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(16.))
                        .flex_shrink_0()
                        .child(icon(
                            "note",
                            px(14.),
                            mark.tag.color().unwrap_or(if selected { theme.accent } else { theme.dim }),
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_small()
                        .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                        .text_color(theme.text)
                        .truncate()
                        .child(title.to_string()),
                )
                .when(mark.favorite, |d| {
                    d.child(div().flex_shrink_0().child(icon("star-filled", px(11.), theme.warn)))
                })
                .child(close_button(format!("doc-close-{id}"), theme).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |app, _, _, cx| {
                        app.close_doc_tab(id, cx);
                        cx.stop_propagation();
                    }),
                )),
        )
        .when(has_summary, |d| {
            d.child(
                div()
                    .pl(px(22.))
                    .pt(px(1.))
                    .text_meta()
                    .text_color(theme.faint)
                    .truncate()
                    .child(summary.to_string()),
            )
        })
        .into_any_element()
}

/// One workflow in the side bar.
#[allow(clippy::too_many_arguments)]
fn flow_row(
    id: usize,
    title: &str,
    summary: &str,
    running: bool,
    selected: bool,
    nested: bool,
    depth: usize,
    mark: crate::ui::store::Mark,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let (tint, edge) = (theme.accent_soft, theme.accent);
    let (theme_drag, flow_name) = (*theme, title.to_string());

    div()
        .id(SharedString::from(format!("flow-{id}")))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .mx(px(6.))
        .my(px(1.))
        .px(px(6.))
        .py(px(4.))
        .when(nested, |d| d.ml(px(18. + 12. * depth as f32)))
        .rounded(px(6.))
        .cursor_pointer()
        .when(selected, |d| d.bg(theme.selected))
        .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
        .on_drag(DragItem(Item::Flow(id)), move |_, _, _, cx| {
            cx.new(|_| DragPreview {
                icon: "play",
                name: flow_name.clone(),
                theme: theme_drag,
            })
        })
        .drag_over::<DragItem>(move |style, _, _, _| takes_a_drop(style, tint, edge))
        .on_drop(cx.listener(move |app, dragged: &DragItem, _, cx| {
            app.drop_on_item(dragged.0, Item::Flow(id), cx);
        }))
        .on_click(cx.listener(move |app, _, _, cx| app.select_flow(id, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                let (tag, favourite) = app.mark_of(Item::Flow(id));
                app.open_menu(
                    e.position,
                    crate::ui::menu::for_flow(
                        id,
                        running,
                        tag,
                        favourite,
                        app.can_move(Item::Flow(id)),
                    ),
                    cx,
                );
                cx.stop_propagation();
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(20.))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(16.))
                        .flex_shrink_0()
                        .child(icon(
                            "play",
                            px(13.),
                            mark.tag.color().unwrap_or(if selected { theme.accent } else { theme.dim }),
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_small()
                        .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                        .text_color(theme.text)
                        .truncate()
                        .child(title.to_string()),
                )
                .when(mark.favorite, |d| {
                    d.child(div().flex_shrink_0().child(icon("star-filled", px(11.), theme.warn)))
                })
                .when(running, |d| d.child(ring(theme.accent)))
                .child(close_button(format!("flow-close-{id}"), theme).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |app, _, _, cx| {
                        app.close_flow_tab(id, cx);
                        cx.stop_propagation();
                    }),
                )),
        )
        .when(!summary.is_empty(), |d| {
            d.child(
                div()
                    .pl(px(22.))
                    .pt(px(1.))
                    .text_meta()
                    .text_color(theme.faint)
                    .truncate()
                    .child(summary.to_string()),
            )
        })
        .into_any_element()
}

/// One workflow's tab.
fn flow_tab(
    sheet: &crate::ui::flows::Sheet,
    active: bool,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let id = sheet.id;
    tab_shell(format!("flowtab-{id}"), active, theme)
        .on_click(cx.listener(move |app, _, _, cx| app.select_flow(id, cx)))
        .child(icon("play", px(13.), if active { theme.accent } else { theme.faint }))
        .child(
            div()
                .min_w_0()
                .text_small()
                .text_color(if active { theme.text } else { theme.dim })
                .truncate()
                .child(sheet.title()),
        )
        .when(sheet.running(), |d| d.child(ring(theme.accent)))
        .child(close_button(format!("flowtab-close-{id}"), theme).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app, _, _, cx| {
                app.close_flow_tab(id, cx);
                cx.stop_propagation();
            }),
        ))
        .into_any_element()
}

/// One document's tab.
fn doc_tab(
    doc: &crate::ui::notes::Doc,
    title: String,
    active: bool,
    theme: &Theme,
    cx: &mut Context<App>,
) -> AnyElement {
    let id = doc.id;
    tab_shell(format!("doctab-{id}"), active, theme)
        .on_click(cx.listener(move |app, _, _, cx| app.select_doc(id, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                app.select_doc(id, cx);
                let (tag, favourite) = app.mark_of(Item::Doc(id));
                app.open_menu(e.position, crate::ui::menu::for_doc(id, tag, favourite, app.can_move(Item::Doc(id))), cx);
                cx.stop_propagation();
            }),
        )
        .child(icon("note", px(14.), if active { theme.accent } else { theme.faint }))
        .child(
            div()
                .min_w_0()
                .text_small()
                .text_color(if active { theme.text } else { theme.dim })
                .truncate()
                .child(title),
        )
        .child(close_button(format!("doctab-close-{id}"), theme).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app, _, _, cx| {
                app.close_doc_tab(id, cx);
                cx.stop_propagation();
            }),
        ))
        .into_any_element()
}

/// The shape every tab shares.
fn tab_shell(id: String, active: bool, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .relative()
        .flex()
        .items_center()
        .gap(px(space::TIGHT))
        .h_full()
        .px(px(space::ROOMY))
        .max_w(px(240.))
        .flex_shrink_0()
        .cursor_pointer()
        .bg(if active { theme.tab_active } else { theme.tab_inactive })
        .border_r_1()
        .border_color(theme.border)
        .when(!active, |d| d.hover(|s| s.bg(theme.hover.blend(theme.tab_inactive))))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top_0()
                .h(px(2.))
                .bg(if active { theme.accent } else { gpui::transparent_black() }),
        )
}

/// One editor tab. The one you are on is the editor surface continued upward,
/// with a lit top edge, the way every editor says "this one".
fn tab(job: &Job, active: bool, theme: &Theme, cx: &mut Context<App>) -> AnyElement {
    let id = job.id;
    let running = job.state.is_running();
    let (tag_value, tag) = (job.tag, job.tag.color());
    let (favourite, open) = (job.favorite, true);

    div()
        .id(SharedString::from(format!("tab-{id}")))
        .flex()
        .items_center()
        .gap(px(space::TIGHT))
        .h_full()
        .px(px(space::ROOMY))
        .max_w(px(240.))
        .flex_shrink_0()
        .cursor_pointer()
        .bg(if active { theme.tab_active } else { theme.tab_inactive })
        .border_r_1()
        .border_color(theme.border)
        .when(!active, |d| d.hover(|s| s.bg(theme.hover.blend(theme.tab_inactive))))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top_0()
                .h(px(2.))
                .bg(if active { theme.accent } else { gpui::transparent_black() }),
        )
        .on_click(cx.listener(move |app, _, _, cx| app.select_job(id, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |app, e: &gpui::MouseDownEvent, _, cx| {
                app.select_job(id, cx);
                app.open_menu(
                    e.position,
                    crate::ui::menu::for_job(crate::ui::menu::JobMenu {
                        id,
                        tag: tag_value,
                        running,
                        favourite,
                        open,
                        comparable: !app.comparable(id).is_empty(),
                        comparing: app
                            .workspace()
                            .job(id)
                            .is_some_and(|j| j.compare_with.is_some()),
                        can_move: app.can_move(Item::Tool(id)),
                    }),
                    cx,
                );
                cx.stop_propagation();
            }),
        )
        .child(icon(job.tool.icon(), px(14.), tag.unwrap_or(if active { theme.accent } else { theme.faint })))
        .child(
            div()
                .min_w_0()
                .text_small()
                .text_color(if active { theme.text } else { theme.dim })
                .truncate()
                .child(job.name()),
        )
        .when(running, |d| d.child(ring(theme.accent)))
        // A tab is a view of a tool, so closing it closes the view. The tool
        // stays in the side bar and its file stays on disk.
        .child(close_button(format!("tab-close-{id}"), theme).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |app, _, _, cx| {
                app.close_tab(id, cx);
                cx.stop_propagation();
            }),
        ))
        .into_any_element()
}

/// One clickable group in the status bar.
fn status_item(id: &'static str, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(space::TIGHT / 2.))
        .h_full()
        .px(px(space::SNUG))
        .cursor_pointer()
        .hover(|s| s.bg(theme.status_fg.opacity(0.14)))
}

/// Renders a job's state as a coloured label.
pub(super) fn status_pill(state: &State, theme: &Theme) -> AnyElement {
    let (fg, bg) = if state.is_running() {
        (theme.accent, theme.accent_soft)
    } else {
        let s = state.status();
        (theme.status(s), theme.status_soft(s))
    };
    pill(state.label(), fg, bg).into_any_element()
}
