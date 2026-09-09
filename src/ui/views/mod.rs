//! What the window shows: the chrome around a workspace, and the pane for
//! whichever tool inside it you are looking at.

mod chrome;
mod doc;
mod flow;
mod job;
mod interfaces;
mod menu;
mod notices;
mod page;
mod palette;
mod picker;
mod settings;
mod variables;
mod welcome;

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div,
    prelude::FluentBuilder, px,
};

use super::app::{self, App};
use super::widgets::Type;

impl Render for App {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // With the theme following the desktop, this is where a desktop that
        // has changed is noticed.
        self.apply_theme(window, cx);
        let theme = self.theme(cx);
        self.sync_filter(cx);
        if self.sync_note(cx) {
            cx.notify();
        }
        if self.commit_rename(cx) {
            self.save_current();
        }
        self.commit_item_rename(cx);
        self.commit_var(cx);
        self.sync_var_offer(cx);
        self.sync_var_filter(cx);
        // A document is a file something else edits, so a save elsewhere shows
        // up here.
        if self.refresh_docs() {
            cx.notify();
        }
        self.resume_flows(window, cx);
        self.sync_step_value(cx);
        self.sync_editor(&theme, cx);
        if let Some(name) = self.sync_workspace_rename(cx) {
            let at = self.active;
            self.rename_workspace(at, &name, cx);
        }
        if self.palette_open {
            let palette = self.palette.clone();
            palette.update(cx, |p, cx| {
                if p.sync(cx) {
                    cx.notify();
                }
            });
        }
        if self.picker_open {
            let picker = self.picker.clone();
            picker.update(cx, |p, cx| {
                if p.sync(cx) {
                    cx.notify();
                }
            });
        }

        // The tab strip and the pane have to name the same tool. Opening a
        // document clears the tool selection, and closing the document's tab
        // does not put one back, so `showing` falls through to the first tool
        // with a tab: the strip draws that tab as active while the pane, which
        // looks its tool up by the selection, finds none and says no tool is
        // selected. The keys that act on the selection do nothing there too.
        let out_of_step = tool_out_of_step(self.workspace().showing(), self.workspace().selected);
        if let Some(id) = out_of_step {
            self.workspace_mut().selected = Some(id);
        }

        // A page takes the whole window: no side bar, no tab strip, no output
        // panel. What it is about is not a thing inside the workspace, so a
        // list of what is inside the workspace is only in the way.
        let page = self.page;
        let has_jobs = self.workspace().any_open();
        // The side bar is drawn whenever it has any width at all: it slides
        // shut and does not vanish, and something has to be there to slide.
        let sidebar_at = self.sidebar_at();
        let sidebar = (page.is_none() && sidebar_at > 0.).then(|| self.sidebar(&theme, cx));
        // The slide is a layout instead of one element's own style, so it
        // asks for the next frame itself. `notify` is not enough: a repaint
        // asked for from inside a repaint is the one thing it will not do.
        if self.sidebar_moving() {
            window.request_animation_frame();
        }
        let tabs = (has_jobs && page.is_none()).then(|| self.tab_bar(&theme, cx));
        let crumbs = (has_jobs && page.is_none()).then(|| self.breadcrumbs(&theme));
        let body = match page {
            Some(app::Page::Settings) => self.settings_page(&theme, window, cx),
            Some(app::Page::Variables) => self.variables_page(&theme, window, cx),
            Some(app::Page::Interfaces) => self.interfaces_page(&theme, window, cx),
            None => match self.workspace().showing() {
                _ if !has_jobs => self.welcome(&theme, cx),
                Some(super::workspace::Item::Doc(id)) => self.doc_pane(id, &theme, cx),
                Some(super::workspace::Item::Flow(id)) => self.flow_pane(id, &theme, cx),
                _ => self.job_pane(&theme, window, cx),
            },
        };
        let showing_doc = matches!(
            self.workspace().showing(),
            Some(super::workspace::Item::Doc(_) | super::workspace::Item::Flow(_))
        );
        let panel = (self.panel_open && has_jobs && !showing_doc && page.is_none())
            .then(|| self.panel(&theme, cx))
            .flatten();
        let toasts = self.toasts(&theme, cx);
        let notices = self.notices.open.then(|| self.notice_list(&theme, cx));

        div()
            .key_context("Ntls")
            .track_focus(&self.focus_handle)
            .actions(cx)
            .relative()
            .on_mouse_move(cx.listener(|app, e: &gpui::MouseMoveEvent, _, cx| {
                if app.resizing.is_some() {
                    // A pointer over the window with nothing held down is not
                    // dragging anything. Without this the edge goes on
                    // following the pointer after a release the window never
                    // saw, and the only way out is to click somewhere.
                    if e.pressed_button == Some(gpui::MouseButton::Left) {
                        app.drag_resize(f32::from(e.position.x), cx);
                    } else {
                        app.end_resize();
                    }
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|app, _, _, _| app.end_resize()),
            )
            // That handler only hears a release landing on the window itself
            // with nothing drawn over that spot. Let go of an edge over a
            // notice in the corner, or past the window's own border, and the
            // drag would never be told it was over: the width would not be
            // remembered, and the next move would carry on resizing.
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|app, _, _, _| app.end_resize()),
            )
            // A workspace is a directory and a tool is a file, so both can be
            // dragged in from the Finder.
            .on_drop(cx.listener(|app, paths: &gpui::ExternalPaths, _, cx| {
                app.drop_hover = false;
                app.open_paths(paths.paths(), cx);
            }))
            .drag_over::<gpui::ExternalPaths>(|style, _, _, _| style)
            // A right-click on the frame itself still has something to offer.
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(|app, e: &gpui::MouseDownEvent, _, cx| {
                    app.open_menu(e.position, super::menu::for_background(), cx);
                }),
            )
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(super::theme::UI_FONT)
            .text_body()
            .child(self.titlebar(&theme, window, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.activity_bar(&theme, cx))
                    .children(sidebar)
                    // The editor: what is open across the top, what you are
                    // looking at in the middle, what it had to say underneath.
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .bg(theme.bg)
                            .children(tabs)
                            .children(crumbs)
                            .child(div().flex().flex_col().flex_1().min_h_0().child(body))
                            .children(panel),
                    ),
            )
            .child(self.status_bar(&theme, cx))
            // Dropping is shown as an outline drawn over the window rather
            // than as a border on it: a border would inset everything by its
            // own width, and the status bar would stop short of the
            // corner.
            .when(self.drop_hover, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_2()
                        .border_color(theme.focus)
                        .bg(theme.accent.opacity(0.06)),
                )
            })
            .children(toasts)
            .children(notices)
            .when(self.palette_open, |d| d.child(self.palette_overlay(&theme, cx)))
            .when(self.picker_open, |d| d.child(self.picker_overlay(&theme, cx)))
            .children(self.var_offer_overlay(&theme, window, cx))
            .when(self.menu.is_some(), |d| d.child(self.menu_overlay(&theme, window, cx)))
    }
}

/// Every action the window answers to, in one place.
///
/// The four navigation keys mean different things depending on what is open,
/// so each resolves to a single action and the handler decides, which
/// simpler to reason about than four overlapping key contexts.
trait Actions: Sized {
    fn actions(self, cx: &mut Context<App>) -> Self;
}

impl Actions for gpui::Div {
    fn actions(self, cx: &mut Context<App>) -> Self {
        self.on_action(cx.listener(|app, _: &app::NewWorkspace, _, cx| app.new_workspace(cx)))
            .on_action(cx.listener(|app, _: &app::CloseWorkspace, _, cx| {
                let at = app.active;
                app.close_workspace(at, cx);
            }))
            .on_action(cx.listener(|app, _: &app::CloseJob, _, cx| {
                // Closing with nothing selected closes the workspace, as
                // what the same key does in every tabbed application.
                match app.workspace().selected {
                    Some(id) => app.close_job(id, cx),
                    None => {
                        let at = app.active;
                        app.close_workspace(at, cx);
                    }
                }
            }))
            .on_action(cx.listener(|app, _: &app::NextWorkspace, _, cx| app.cycle_workspace(1, cx)))
            .on_action(cx.listener(|app, _: &app::PrevWorkspace, _, cx| app.cycle_workspace(-1, cx)))
            .on_action(cx.listener(|app, _: &app::NextJob, _, cx| {
                app.workspace_mut().select_neighbour(1);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &app::PrevJob, _, cx| {
                app.workspace_mut().select_neighbour(-1);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &app::AddTool, window, cx| app.open_palette(window, cx)))
            // Run and Stop act on whatever is on screen. A workflow has a
            // Run button of its own and did not answer the key or the menu
            // beside it, which left the one way of starting one that could
            // not be reached from the keyboard.
            .on_action(cx.listener(|app, _: &app::Run, window, cx| {
                match app.workspace().showing() {
                    Some(super::workspace::Item::Flow(id)) => app.start_flow(id, window, cx),
                    _ => app.run_selected(window, cx),
                }
            }))
            .on_action(cx.listener(|app, _: &app::Stop, _, cx| {
                match app.workspace().showing() {
                    Some(super::workspace::Item::Flow(id)) => app.stop_flow(id, cx),
                    _ => app.stop_selected(cx),
                }
            }))
            .on_action(cx.listener(|app, _: &app::Settings, window, cx| {
                // On a document or a workflow, the settings key edits the
                // source. Same idea: show me the thing behind
                // what I am looking at.
                match app.workspace().showing() {
                    Some(item @ (super::workspace::Item::Doc(_) | super::workspace::Item::Flow(_))) => {
                        app.toggle_editing(item, window, cx)
                    }
                    _ => {
                        if let Some(job) = app.selected_job_mut() {
                            job.show_form = !job.show_form;
                        }
                        app.focus_form(window, cx);
                    }
                }
            }))
            .on_action(cx.listener(|app, _: &app::FocusFilter, window, cx| {
                if let Some(job) = app.selected_job() {
                    let handle = job.filter_input.read(cx).focus_handle.clone();
                    window.focus(&handle);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|app, _: &app::ToggleProperties, window, cx| {
                if app.renaming {
                    app.end_rename(window, cx);
                } else {
                    app.begin_rename(window, cx);
                }
            }))
            .on_action(cx.listener(|app, _: &app::FocusNote, window, cx| {
                // Only useful with a result selected, the only time
                // there is anything to annotate.
                if let Some(line) = app.selected_job().and_then(|j| j.selected) {
                    app.begin_note(line, window, cx);
                }
            }))
            .on_action(cx.listener(|app, _: &app::Preferences, window, cx| {
                app.show_page(app::Page::Settings, window, cx)
            }))
            .on_action(cx.listener(|app, _: &app::ShowNotices, _, cx| app.toggle_notices(cx)))
            .on_action(cx.listener(|app, _: &app::ToggleLog, _, cx| app.toggle_panel(cx)))
            .on_action(cx.listener(|app, _: &app::ToggleSidebar, _, cx| app.toggle_sidebar(cx)))
            .on_action(cx.listener(|app, _: &app::ShowWorkspaces, w, cx| {
                app.show_view(app::View::Workspaces, w, cx)
            }))
            .on_action(cx.listener(|app, _: &app::ShowTools, w, cx| {
                app.show_view(app::View::Tools, w, cx)
            }))
            .on_action(cx.listener(|app, _: &app::ToggleChart, _, cx| {
                if let Some(job) = app.selected_job_mut() {
                    job.chart_expanded = !job.chart_expanded;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|app, _: &app::ToggleResponse, _, cx| {
                if let Some(job) = app.selected_job_mut()
                    && job.answer.is_some()
                {
                    job.show_response = !job.show_response;
                    // The two cannot both have the pane.
                    if job.show_response {
                        job.show_form = false;
                    }
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|app, _: &app::ToggleTheme, _, cx| app.toggle_theme(cx)))
            .on_action(cx.listener(|app, _: &app::MoveUp, window, cx| app.move_by(-1, window, cx)))
            .on_action(cx.listener(|app, _: &app::MoveDown, window, cx| app.move_by(1, window, cx)))
            .on_action(cx.listener(|app, _: &app::PageUp, window, cx| app.move_by(-20, window, cx)))
            .on_action(cx.listener(|app, _: &app::PageDown, window, cx| app.move_by(20, window, cx)))
            .on_action(cx.listener(|app, _: &app::First, _, cx| {
                if let Some(job) = app.selected_job_mut() {
                    job.selected = None;
                    job.move_selection(1);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|app, _: &app::Last, _, cx| {
                if let Some(job) = app.selected_job_mut() {
                    job.selected = None;
                    job.move_selection(-1);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|app, _: &app::Confirm, window, cx| app.confirm(window, cx)))
            .on_action(cx.listener(|app, _: &app::Dismiss, window, cx| app.dismiss(window, cx)))
            .on_action(cx.listener(|app, _: &app::ExpandField, window, cx| {
                // Tab means "finish what I am typing" in both places: in the
                // palette it completes the suggestion, in a form it expands
                // the shorthand.
                if app.palette_open {
                    app.complete_palette(window, cx);
                } else if !app.take_var_offer(cx) {
                    app.expand_focused(window, cx);
                }
            }))
            .on_action(cx.listener(|app, _: &app::Open1, w, cx| app.add_nth(1, w, cx)))
            .on_action(cx.listener(|app, _: &app::Open2, w, cx| app.add_nth(2, w, cx)))
            .on_action(cx.listener(|app, _: &app::Open3, w, cx| app.add_nth(3, w, cx)))
            .on_action(cx.listener(|app, _: &app::Open4, w, cx| app.add_nth(4, w, cx)))
            .on_action(cx.listener(|app, _: &app::Open5, w, cx| app.add_nth(5, w, cx)))
            .on_action(cx.listener(|app, _: &app::Open6, w, cx| app.add_nth(6, w, cx)))
            .on_action(cx.listener(|app, _: &app::Open7, w, cx| app.add_nth(7, w, cx)))
            .on_action(cx.listener(|app, _: &app::Open8, w, cx| app.add_nth(8, w, cx)))
            .on_action(cx.listener(|app, _: &app::Save, _, cx| app.save_active_editor(cx)))
            .on_action(cx.listener(|_, _: &app::Quit, _, cx| cx.quit()))
    }
}

impl App {
    /// Up and down: the palette's list, or the results, whichever is in front.
    fn move_by(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open {
            let picker = self.picker.clone();
            picker.update(cx, |p, _| p.move_cursor(delta.signum()));
            cx.notify();
            return;
        }
        if self.palette_open {
            let len = self.suggestion(cx).len();
            let palette = self.palette.clone();
            palette.update(cx, |p, _| p.move_cursor(delta.signum(), len));
            cx.notify();
            return;
        }
        if self.walk_var_offer(delta.signum(), cx) {
            return;
        }
        // While a form field has the caret, the arrows belong to the text.
        if self.typing_in_form(window, cx) {
            return;
        }
        if let Some(job) = self.selected_job_mut() {
            job.move_selection(delta);
            cx.notify();
        }
    }

    /// Enter: take the palette's choice, run the form, or send a result on.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open {
            self.confirm_picker(window, cx);
            return;
        }
        if self.editing_note.is_some() {
            self.end_note(window, cx);
            return;
        }
        if self.palette_open {
            self.confirm_palette(window, cx);
            return;
        }
        if self.renaming {
            self.end_rename(window, cx);
            return;
        }
        if self.renaming_item.is_some() {
            self.end_item_rename(window, cx);
            return;
        }
        if self.editing_var.is_some() {
            if self.var_offer_is_the_answer() {
                self.take_var_offer(cx);
                return;
            }
            self.end_var_edit(window, cx);
            return;
        }
        if self.renaming_workspace {
            self.end_workspace_rename(window, cx);
            return;
        }
        // The box for a condition's value is written into as it is typed, so
        // there is nothing to confirm: return just puts the caret back.
        if self.typing_step_value(window, cx) {
            window.focus(&self.focus_handle);
            cx.notify();
            return;
        }
        // A step is finished with the same key everything else is finished
        // with, and its controls go away.
        if let Some(super::workspace::Item::Flow(id)) = self.workspace().showing()
            && self.workspace().flow(id).is_some_and(|f| f.cursor.is_some())
        {
            self.select_step(id, None, cx);
            return;
        }
        if self.typing_in_form(window, cx) {
            self.run_selected(window, cx);
            return;
        }
        if self.typing_in_filter(window, cx) {
            window.focus(&self.focus_handle);
            cx.notify();
            return;
        }
        let Some(from) = self.selected_job().map(|j| j.tool.id()) else { return };
        if let Some(to) = self.handoff_targets(from).first().map(|t| t.id()) {
            self.handoff(to, window, cx);
        }
    }

    /// Escape: back out of whatever is in front, one layer at a time.
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open {
            self.close_picker(window, cx);
            return;
        }
        // The palette and the picker are drawn over the window rather than
        // in it, so they come off first. Anything this escape might otherwise
        // have reached is behind them, and backing out of something behind
        // what you are looking at reads as the key having done nothing.
        if self.palette_open {
            self.close_palette(window, cx);
            return;
        }
        if self.notices.open {
            self.toggle_notices(cx);
            return;
        }
        // What is being typed into a page comes off before the page does:
        // escape peels one layer at a time, and the completion list, the box
        // and the page are three layers.
        if self.editing_var.is_some() {
            if self.clear_var_offer(cx) {
                return;
            }
            self.end_var_edit(window, cx);
            return;
        }
        // A menu is the last thing the window draws, over a page that has
        // taken the whole window included, so it is the layer an escape
        // reaches first. Taking the page off first left the menu where it was
        // opened, hanging over a workspace it had nothing to do with and still
        // offering to copy an address from a list that was no longer on
        // screen.
        match front_layer(self.menu.is_some(), self.page.is_some()) {
            Some(Front::Menu) => {
                self.close_menu(cx);
                return;
            }
            Some(Front::Page) => {
                self.close_page(window, cx);
                return;
            }
            None => {}
        }
        if self.editing_note.is_some() {
            self.end_note(window, cx);
            return;
        }
        if self.clearing.is_some() {
            self.cancel_clear(cx);
            return;
        }
        if self.renaming {
            self.end_rename(window, cx);
            return;
        }
        if self.renaming_item.is_some() {
            self.end_item_rename(window, cx);
            return;
        }
        if self.renaming_workspace {
            self.end_workspace_rename(window, cx);
            return;
        }
        if self.typing_in_form(window, cx)
            || self.typing_in_filter(window, cx)
            || self.typing_step_value(window, cx)
        {
            window.focus(&self.focus_handle);
            cx.notify();
            return;
        }
        // A step's controls are put away the way a selected row is.
        if let Some(super::workspace::Item::Flow(id)) = self.workspace().showing()
            && self.workspace().flow(id).is_some_and(|f| f.cursor.is_some())
        {
            self.select_step(id, None, cx);
            return;
        }
        if let Some(job) = self.selected_job_mut()
            && job.selected.is_some()
        {
            job.selected = None;
            cx.notify();
        }
    }

}

/// A hairline that separates without drawing attention.
pub(crate) fn rule(theme: &super::theme::Theme) -> gpui::Div {
    div().h(px(1.)).w_full().flex_shrink_0().bg(theme.rule)
}

/// The tool whose tab is drawn as active while the selection points somewhere
/// else, if there is one.
///
/// What the tab strip draws as active is whatever the workspace is showing,
/// and with nothing selected that is the first tool with a tab. The pane for a
/// tool goes by the selection instead, so the two can end up describing
/// different tools: the answer here is the tool the selection has to be moved
/// to for them to agree again.
fn tool_out_of_step(
    showing: Option<super::workspace::Item>,
    selected: Option<usize>,
) -> Option<usize> {
    match showing {
        Some(super::workspace::Item::Tool(id)) if selected != Some(id) => Some(id),
        _ => None,
    }
}

/// One of the two things that cover the whole window: the menu drawn over it,
/// and the page drawn in it.
#[derive(Debug, PartialEq, Eq)]
enum Front {
    Menu,
    Page,
}

/// Which of the two an escape reaches first, if either is up.
///
/// A page takes the whole window, and a menu is drawn over everything the
/// window has, the page included, so the menu is always the one in front of
/// the other. Closing the page first left the menu behind it, over a
/// workspace it was never opened on and naming a row that had gone with the
/// page.
fn front_layer(menu_open: bool, page_open: bool) -> Option<Front> {
    match (menu_open, page_open) {
        (true, _) => Some(Front::Menu),
        (false, true) => Some(Front::Page),
        (false, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Front, front_layer, tool_out_of_step};
    use crate::ui::workspace::Item;

    #[test]
    fn the_tool_whose_tab_is_drawn_as_active_is_the_one_the_pane_shows() {
        // Closing the last document tab leaves no document selected, and no
        // tool selected either because opening the document had cleared that,
        // so the strip falls back to the first tool with a tab and draws it
        // active over a pane that says no tool is selected.
        assert_eq!(tool_out_of_step(Some(Item::Tool(4)), None), Some(4));
        // Same story with the selection left on a tool whose tab has since
        // been closed: the strip is drawing tab 4, the pane is showing 9.
        assert_eq!(tool_out_of_step(Some(Item::Tool(4)), Some(9)), Some(4));

        // A selection that already agrees is left alone.
        assert_eq!(tool_out_of_step(Some(Item::Tool(4)), Some(4)), None);
        // A document or a workflow pane is handed the id of what it draws and
        // never asks which tool is selected, so neither is out of step.
        assert_eq!(tool_out_of_step(Some(Item::Doc(1)), None), None);
        assert_eq!(tool_out_of_step(Some(Item::Flow(1)), Some(9)), None);
        // Nothing open at all is the welcome screen, not a disagreement.
        assert_eq!(tool_out_of_step(None, Some(4)), None);
    }

    #[test]
    fn a_menu_opened_over_a_page_comes_off_before_the_page_does() {
        // The interfaces page answers a right-click with a menu, and that
        // menu is drawn over the window rather than on the page. Closing the
        // page first put the workspace back underneath a menu that was still
        // open, offering to copy an address from a list that had gone.
        assert_eq!(front_layer(true, true), Some(Front::Menu));

        // Either one on its own is still the thing that comes off.
        assert_eq!(front_layer(true, false), Some(Front::Menu));
        assert_eq!(front_layer(false, true), Some(Front::Page));
        // With neither up, the escape belongs to something inside the window.
        assert_eq!(front_layer(false, false), None);
    }
}
