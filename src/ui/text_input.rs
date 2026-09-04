//! A single-line text field.
//!
//! GPUI gives you the platform's input handling but not a widget, so this is
//! the widget: a shaped line, a caret, a selection, and the IME plumbing that
//! makes dead keys and the character palette work.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla, IntoElement, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    Render, ShapedLine, SharedString, Style, Styled, TextRun, UTF16Selection, UnderlineStyle,
    Window, actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    ntls_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

/// Binds the editing keys. Called once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new(&crate::sys::shortcut("cmd-a"), SelectAll, Some("TextInput")),
        KeyBinding::new(&crate::sys::shortcut("cmd-v"), Paste, Some("TextInput")),
        KeyBinding::new(&crate::sys::shortcut("cmd-c"), Copy, Some("TextInput")),
        KeyBinding::new(&crate::sys::shortcut("cmd-x"), Cut, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new(&crate::sys::shortcut("cmd-left"), Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new(&crate::sys::shortcut("cmd-right"), End, Some("TextInput")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("TextInput")),
    ]);
}

pub struct TextInput {
    pub focus_handle: FocusHandle,
    content: SharedString,
    pub placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// Bumped on every edit, so an owner can tell whether the value changed
    /// without comparing strings on every frame.
    pub revision: usize,
    pub mono: bool,
    /// When set, the line is coloured as a formula instead of drawn in one
    /// colour. A formula is code, and code in a single colour is read a word
    /// at a time.
    pub as_formula: Option<super::editor::Colours>,
    /// Colours, refreshed by the owner when the theme changes.
    pub text_color: Hsla,
    pub placeholder_color: Hsla,
    pub caret_color: Hsla,
    pub selection_color: Hsla,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, value: &str, placeholder: &str) -> TextInput {
        TextInput {
            focus_handle: cx.focus_handle(),
            content: value.to_string().into(),
            placeholder: placeholder.to_string().into(),
            selected_range: value.len()..value.len(),
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            revision: 0,
            mono: true,
            as_formula: None,
            text_color: gpui::white(),
            placeholder_color: gpui::opaque_grey(0.5, 1.0),
            caret_color: gpui::blue(),
            selection_color: gpui::blue().opacity(0.3),
        }
    }

    pub fn value(&self) -> &str {
        &self.content
    }

    /// Where the caret is, as a byte offset into the value.
    pub fn caret(&self) -> usize {
        self.cursor_offset()
    }

    /// Where the caret is in the window, at the bottom of the line: the point
    /// anything floating under the field hangs from.
    ///
    /// Nothing until the field has been painted once, so a list of completions
    /// appears on the frame after the one that asked for it.
    pub fn caret_at(&self) -> Option<Point<Pixels>> {
        let bounds = self.last_bounds.as_ref()?;
        let line = self.last_layout.as_ref()?;
        Some(point(bounds.left() + line.x_for_index(self.cursor_offset()), bounds.bottom()))
    }

    /// Replaces the `back` bytes before the caret with `insert`, and leaves the
    /// caret after what was inserted. How a completion is taken.
    pub fn splice(&mut self, back: usize, insert: &str, cx: &mut Context<Self>) {
        let at = self.cursor_offset();
        let mut start = at.saturating_sub(back);
        // A byte count that landed inside a character would split it in half.
        while start > 0 && !self.content.is_char_boundary(start) {
            start -= 1;
        }
        self.content = (self.content[..start].to_owned() + insert + &self.content[at..]).into();
        let caret = start + insert.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        self.revision += 1;
        cx.notify();
    }

    /// Replaces the whole value, putting the caret at the end. Used when a
    /// field is expanded or filled in from a handoff.
    pub fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
        if self.content.as_ref() == value {
            return;
        }
        self.content = value.to_string().into();
        self.selected_range = self.content.len()..self.content.len();
        self.marked_range = None;
        self.revision += 1;
        cx.notify();
    }

    pub fn select_all_now(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        let index = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &one_line(&text), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed { self.selected_range.start } else { self.selected_range.end }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let (mut utf8, mut utf16) = (0, 0);
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let (mut utf8, mut utf16) = (0, 0);
        for ch in self.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

/// What a paste becomes in a field that is only one line high.
///
/// A line break has nowhere to go here, so it becomes the space that separated
/// the two lines. The carriage return has to be named alongside the newline:
/// text copied out of a Windows file, a terminal or a mail client ends its
/// lines with both, and flattening only the newline left a bare carriage
/// return sitting in the value. It shapes to nothing, so the field looks like
/// what was copied, and it is then saved into a variable or sent as part of a
/// header exactly as if it had been typed. A `\r\n` pair is one break between
/// two lines and so becomes one space, not two.
fn one_line(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\r', '\n'], " ")
}

/// Where the selection an IME reports for its marked text lands in the value,
/// given that the marked text was put at byte offset `at`.
///
/// The IME counts that selection in UTF-16 units from the start of the text it
/// just marked, not from the start of the value, so it has to be measured
/// inside `marked` and only then shifted to where `marked` was placed. The
/// units are also the IME's own idea of the string, so an offset past its end
/// stops at the end rather than running off it.
fn marked_selection(at: usize, marked: &str, selected_utf16: &Range<usize>) -> Range<usize> {
    let byte_offset = |units: usize| {
        let (mut utf8, mut utf16) = (0, 0);
        for ch in marked.chars() {
            if utf16 >= units {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    };
    let start = byte_offset(selected_utf16.start);
    let end = byte_offset(selected_utf16.end).max(start);
    at + start..at + end
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.revision += 1;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        // The selection was read as if the IME counted it from the start of
        // the whole value and then had `range.end` added to its end, which
        // pushed it past the end of the content and often into the middle of a
        // character: composing one Japanese syllable into an empty field left
        // the selection at 3..4 over three bytes, and the next insertion, copy
        // or cancelled composition sliced the string out of bounds.
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| marked_selection(range.start, new_text, r))
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.revision += 1;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(bounds.left() + last_layout.x_for_index(range.start), bounds.top()),
            point(bounds.left() + last_layout.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        // `localize` has already answered the question: it gives the point
        // measured from the field's own origin, which is the coordinate the
        // shaped line indexes by. Subtracting it from the window point again
        // cancelled the pointer out and left the field's left edge, so every
        // lookup the system makes here, the dictionary panel or an IME asking
        // what sits under the pointer, was answered about a position that
        // depended on where the field happened to be on screen rather than on
        // where the pointer was.
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

/// The element that shapes and paints the line. A plain `div` cannot show a
/// caret or a selection, so the text itself is drawn by hand.
struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl gpui::Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let empty = content.is_empty();
        let (display_text, text_color) = if empty {
            (input.placeholder.clone(), input.placeholder_color)
        } else {
            (content, input.text_color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked) = input.marked_range.as_ref() {
            vec![
                TextRun { len: marked.start, ..run.clone() },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun { len: display_text.len() - marked.end, ..run },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect()
        } else if let Some(colours) = input.as_formula.filter(|_| !empty) {
            // The spans cover the line end to end, so the runs do too and
            // nothing has to be worked out about what was missed.
            super::syntax::expr_line(&display_text)
                .into_iter()
                .filter(|s| !s.range.is_empty())
                .map(|s| TextRun {
                    len: s.range.len(),
                    font: gpui::Font {
                        weight: super::editor::Colours::weight_of(s.kind),
                        ..style.font()
                    },
                    color: colours.of_kind(s.kind),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                })
                .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(display_text, font_size, &runs, None);

        let (selection, cursor) = if selected_range.is_empty() {
            let x = line.x_for_index(cursor);
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + x, bounds.top()),
                        size(px(1.6), bounds.bottom() - bounds.top()),
                    ),
                    input.caret_color,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(bounds.left() + line.x_for_index(selected_range.start), bounds.top()),
                        point(bounds.left() + line.x_for_index(selected_range.end), bounds.bottom()),
                    ),
                    input.selection_color,
                )),
                None,
            )
        };

        PrepaintState { line: Some(line), cursor, selection }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let Some(line) = prepaint.line.take() else { return };
        let _ = line.paint(bounds.origin, window.line_height(), window, cx);

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .when(self.mono, |d| d.font_family(super::theme::MONO))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasting_a_windows_line_break_leaves_no_carriage_return_in_the_value() {
        // Copying one line out of a CRLF file takes its ending with it. Only
        // the newline used to be flattened, so what landed in the field was
        // "10.0.0.1\r ": a host name that looks right, resolves to nothing,
        // and is saved to the workspace with the carriage return still in it.
        assert_eq!(one_line("10.0.0.1\r\n"), "10.0.0.1 ");
        // Two lines are one break apart, so they end up one space apart.
        assert_eq!(one_line("a\r\nb"), "a b");
        // A carriage return can also arrive on its own.
        assert_eq!(one_line("a\rb"), "a b");
        assert!(!one_line("a\r\nb\rc\nd").contains('\r'));
        // A plain newline still becomes the single space it always did, and a
        // paste that was one line to begin with is untouched.
        assert_eq!(one_line("a\nb"), "a b");
        assert_eq!(one_line("10.0.0.1"), "10.0.0.1");
    }

    #[test]
    fn a_marked_selection_is_measured_inside_the_text_that_was_marked() {
        // One Japanese syllable: three bytes, one UTF-16 unit, with the caret
        // reported at the end of the marked text.
        assert_eq!(marked_selection(0, "\u{304b}", &(1..1)), 3..3);
        // The same syllable composed further along the line. The offset is
        // counted in the marked text and only then moved to where the marked
        // text sits, so the three ASCII bytes before it do not shift it.
        assert_eq!(marked_selection(3, "\u{304b}", &(1..1)), 6..6);
        // A whole marked run selected rather than a caret inside it.
        assert_eq!(marked_selection(3, "\u{304b}\u{306a}", &(0..2)), 3..9);
    }

    #[test]
    fn a_composition_over_an_earlier_mark_leaves_a_selection_the_value_can_be_sliced_by() {
        // Typing "k" and then "a" with a Japanese IME replaces the mark on
        // "k" with "\u{304b}", so the value is three bytes long and the caret
        // is reported one UTF-16 unit in. The end of the replaced range used
        // to be added to the end of the selection, which gave 3..4 over three
        // bytes and panicked the moment anything sliced the value by it.
        let value = "\u{304b}";
        let selection = marked_selection(0, value, &(1..1));
        assert!(selection.start <= selection.end);
        assert!(selection.end <= value.len());
        assert!(value.is_char_boundary(selection.start) && value.is_char_boundary(selection.end));
        assert_eq!(&value[selection], "");
    }

    #[test]
    fn a_cancelled_composition_leaves_the_caret_where_the_marked_text_was() {
        // An IME cancels by marking an empty string over the range it had
        // marked before. That range was 3..6 in the old value, so adding its
        // end left a 3..6 selection over a value only three bytes long.
        let value = "abc";
        let selection = marked_selection(3, "", &(0..0));
        assert_eq!(selection, 3..3);
        assert!(selection.end <= value.len());
    }

    #[test]
    fn a_marked_selection_reported_past_the_end_of_the_marked_text_stops_at_its_end() {
        assert_eq!(marked_selection(2, "ab", &(0..9)), 2..4);
        // A reversed pair would produce a range that cannot index a string at
        // all, so the end never precedes the start.
        assert_eq!(marked_selection(2, "ab", &(2..0)), 4..4);
    }
}
