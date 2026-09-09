//! A multi-line text editor.
//!
//! GPUI gives you the platform's input handling but not a widget, so this is
//! the widget, the same shape as [`super::text_input`] grown to many lines:
//! a shaped line per visual row, a caret, a selection, soft wrapping, syntax
//! colouring and a completion list.
//!
//! The document is one `String` and every position is a byte offset into it.
//! That is the simplest thing that can work, and for the files this edits
//! (notes and workflows, kilobytes not megabytes) reshaping the
//! visible lines on each keystroke is not something anyone can perceive.

use std::borrow::Cow;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, GlobalElementId, Hsla, IntoElement, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollHandle,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div,
    fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use super::complete::{Candidate, Context as Where, candidates, context, insertion, replacing};
use super::syntax::{Kind, Language, Span, highlight};
use super::theme::Theme;

actions!(
    ntls_editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        DocStart,
        DocEnd,
        Newline,
        Indent,
        Paste,
        Cut,
        Copy,
        Save,
        Accept,
        Dismiss,
        ShowCharacterPalette,
    ]
);

/// Binds the editing keys. Called once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("Editor")),
        KeyBinding::new("delete", Delete, Some("Editor")),
        KeyBinding::new("left", Left, Some("Editor")),
        KeyBinding::new("right", Right, Some("Editor")),
        KeyBinding::new("up", Up, Some("Editor")),
        KeyBinding::new("down", Down, Some("Editor")),
        KeyBinding::new("shift-left", SelectLeft, Some("Editor")),
        KeyBinding::new("shift-right", SelectRight, Some("Editor")),
        KeyBinding::new("shift-up", SelectUp, Some("Editor")),
        KeyBinding::new("shift-down", SelectDown, Some("Editor")),
        KeyBinding::new(&crate::sys::shortcut("cmd-a"), SelectAll, Some("Editor")),
        KeyBinding::new("home", Home, Some("Editor")),
        KeyBinding::new("end", End, Some("Editor")),
        KeyBinding::new(&crate::sys::shortcut("cmd-left"), Home, Some("Editor")),
        KeyBinding::new(&crate::sys::shortcut("cmd-right"), End, Some("Editor")),
        KeyBinding::new(&crate::sys::shortcut("cmd-up"), DocStart, Some("Editor")),
        KeyBinding::new(&crate::sys::shortcut("cmd-down"), DocEnd, Some("Editor")),
        KeyBinding::new("enter", Newline, Some("Editor")),
        KeyBinding::new("tab", Indent, Some("Editor")),
        KeyBinding::new(&crate::sys::shortcut("cmd-v"), Paste, Some("Editor")),
        KeyBinding::new(&crate::sys::shortcut("cmd-c"), Copy, Some("Editor")),
        KeyBinding::new(&crate::sys::shortcut("cmd-x"), Cut, Some("Editor")),
        KeyBinding::new(&crate::sys::shortcut("cmd-s"), Save, Some("Editor")),
        KeyBinding::new("escape", Dismiss, Some("Editor")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("Editor")),
    ]);
}

/// One source line, shaped and wrapped, with where it sits.
///
/// GPUI does the wrapping and can map between a byte index and a point inside
/// a wrapped line, so a line is the unit here instead of a visual row, and
/// the caret arithmetic belongs to the text system, not to this.
struct Placed {
    wrapped: gpui::WrappedLine,
    /// Where the line starts in the document.
    start: usize,
    /// Its length in bytes, without the newline.
    len: usize,
    /// Where its first visual row begins, relative to the element.
    top: Pixels,
    /// How many visual rows it takes.
    rows: usize,
}

impl Placed {
    fn end(&self) -> usize {
        self.start + self.len
    }

    fn holds(&self, offset: usize) -> bool {
        (self.start..=self.end()).contains(&offset)
    }

    fn height(&self, line_height: Pixels) -> Pixels {
        line_height * self.rows as f32
    }
}

pub struct Editor {
    pub focus_handle: FocusHandle,
    text: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    is_selecting: bool,

    /// What is being edited, which decides the colouring and the completions.
    pub language: Language,
    /// The file this came from, and whether it has changed since it was read.
    pub path: PathBuf,
    pub dirty: bool,
    /// Bumped on every edit, so an owner can notice without comparing strings.
    pub revision: usize,

    /// What completion is offering, and which row of it is highlighted.
    pub offering: Vec<Candidate>,
    pub offering_at: usize,
    pub walked_offer: bool,
    /// What the offer would replace, worked out when it was made.
    offer_context: Where,

    /// The runs in the workspace, refreshed by the owner, which the
    /// completions are drawn from.
    pub tables: Vec<crate::expr::Table>,
    /// The variables of the workspace this is being written in, which are
    /// offered beside the runs.
    pub vars: Vec<String>,

    /// Laid out on the last paint, for hit-testing and for the caret.
    placed: Vec<Placed>,
    bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    pub scroll: ScrollHandle,

    /// Colours, refreshed by the owner when the theme changes.
    pub colours: Colours,
}

/// The colours the editor draws with.
#[derive(Clone, Copy, Debug)]
pub struct Colours {
    pub text: Hsla,
    pub dim: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    pub caret: Hsla,
    pub selection: Hsla,
    pub strong: Hsla,
    pub code: Hsla,
    pub keyword: Hsla,
    pub string: Hsla,
    pub number: Hsla,
}

impl Colours {
    pub fn of(theme: &Theme) -> Colours {
        Colours {
            text: theme.text,
            dim: theme.dim,
            faint: theme.faint,
            accent: theme.accent,
            caret: theme.accent,
            selection: theme.selected,
            strong: theme.text,
            code: theme.up,
            keyword: theme.warn,
            string: theme.up,
            number: theme.accent,
        }
    }

    pub(crate) fn of_kind(&self, kind: Kind) -> Hsla {
        match kind {
            Kind::Text => self.text,
            Kind::Heading | Kind::Strong => self.strong,
            Kind::Emphasis => self.dim,
            Kind::Code => self.code,
            Kind::Link => self.accent,
            Kind::Marker | Kind::Delim => self.faint,
            Kind::Name => self.accent,
            Kind::Key => self.accent,
            Kind::Function => self.number,
            Kind::Keyword => self.keyword,
            Kind::Str => self.string,
            Kind::Number => self.number,
            Kind::Comment => self.faint,
        }
    }

    pub(crate) fn weight_of(kind: Kind) -> gpui::FontWeight {
        match kind {
            Kind::Heading | Kind::Strong | Kind::Keyword | Kind::Key => gpui::FontWeight::SEMIBOLD,
            _ => gpui::FontWeight::NORMAL,
        }
    }
}

impl Editor {
    pub fn new(cx: &mut Context<Self>, text: &str, path: PathBuf, language: Language) -> Editor {
        Editor {
            focus_handle: cx.focus_handle(),
            text: flattened(text).into_owned(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            is_selecting: false,
            language,
            path,
            dirty: false,
            revision: 0,
            offering: Vec::new(),
            offering_at: 0,
            walked_offer: false,
            offer_context: Where::Nowhere,
            tables: Vec::new(),
            vars: Vec::new(),
            placed: Vec::new(),
            bounds: None,
            line_height: px(20.),
            scroll: ScrollHandle::new(),
            colours: Colours {
                text: gpui::white(),
                dim: gpui::opaque_grey(0.7, 1.0),
                faint: gpui::opaque_grey(0.5, 1.0),
                accent: gpui::blue(),
                caret: gpui::blue(),
                selection: gpui::blue().opacity(0.3),
                strong: gpui::white(),
                code: gpui::green(),
                keyword: gpui::yellow(),
                string: gpui::green(),
                number: gpui::blue(),
            },
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces everything, as when the file changed underneath, or when the
    /// same workflow was changed with the controls beside it.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = flattened(text);
        if self.text.as_str() == &*text {
            return;
        }
        self.text = text.into_owned();
        self.selected_range = 0..0;
        self.marked_range = None;
        self.offering.clear();
        self.dirty = false;
        self.revision += 1;
        cx.notify();
    }

    /// Writes the file back. Saving is explicit, so an edit is never half
    /// applied to something a workflow might be reading.
    pub fn save(&mut self, cx: &mut Context<Self>) -> bool {
        // A request body has no file behind it, and is given an empty path to
        // say so. Writing one would put a payload in the working directory
        // the moment somebody pressed the save key out of habit.
        if self.path.as_os_str().is_empty() {
            return false;
        }
        if std::fs::write(&self.path, &self.text).is_err() {
            return false;
        }
        self.dirty = false;
        cx.notify();
        true
    }

    // --- where the caret is ------------------------------------------------

    fn cursor(&self) -> usize {
        if self.selection_reversed { self.selected_range.start } else { self.selected_range.end }
    }

    /// The byte range of the source line the offset is in.
    fn line_at(&self, offset: usize) -> Range<usize> {
        let start = self.text[..offset].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[offset..].find('\n').map_or(self.text.len(), |i| offset + i);
        start..end
    }

    /// The line the caret is on, and where in it, which completion
    /// works from.
    fn caret_in_line(&self) -> (String, usize) {
        let range = self.line_at(self.cursor());
        (self.text[range.clone()].to_string(), self.cursor() - range.start)
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp(offset);
        self.selected_range = offset..offset;
        self.refresh_offer(cx);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp(offset);
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

    /// Nudges an offset onto a character boundary, since a byte in the middle
    /// of one would panic on the next slice.
    fn clamp(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .rev()
            .find_map(|(at, _)| (at < offset).then_some(at))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(at, _)| (at > offset).then_some(at))
            .unwrap_or(self.text.len())
    }

    // --- keys ---------------------------------------------------------------

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor())
        } else {
            self.selected_range.start
        };
        self.move_to(to, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.selected_range.is_empty() {
            self.next_boundary(self.cursor())
        } else {
            self.selected_range.end
        };
        self.move_to(to, cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.walk_offer(-1, cx) {
            return;
        }
        let to = self.vertical(-1);
        self.move_to(to, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.walk_offer(1, cx) {
            return;
        }
        let to = self.vertical(1);
        self.move_to(to, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(-1);
        self.select_to(to, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(1);
        self.select_to(to, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.previous_boundary(self.cursor());
        self.select_to(to, cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.next_boundary(self.cursor());
        self.select_to(to, cx);
    }

    pub fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.text.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        // The first press goes to the first thing on the line, the second to
        // the very start: the indent is usually not where you meant.
        let line = self.line_at(self.cursor());
        let text = &self.text[line.clone()];
        let indented = line.start + text.len() - text.trim_start().len();
        let to = if self.cursor() == indented { line.start } else { indented };
        self.move_to(to, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.line_at(self.cursor()).end;
        self.move_to(to, cx);
    }

    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.text.len();
        self.move_to(to, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let to = self.previous_boundary(self.cursor());
            self.select_to(to, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let to = self.next_boundary(self.cursor());
            self.select_to(to, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        // Return takes the completion if a prefix was typed or the user walked the list;
        // otherwise it is a newline, so return doesn't hijack empty line breaks.
        let has_typed = replacing(&self.offer_context) > 0;
        if (self.walked_offer || has_typed) && self.take_offer(window, cx) {
            return;
        }
        self.offering.clear();
        self.walked_offer = false;
        // The new line starts where the last one's text did, so a list or an
        // indented block carries on.
        let line = self.line_at(self.cursor());
        let text = &self.text[line.start..self.cursor()];
        let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        self.replace_text_in_range(None, &format!("\n{indent}"), window, cx);
    }

    fn indent(&mut self, _: &Indent, window: &mut Window, cx: &mut Context<Self>) {
        if self.take_offer(window, cx) {
            return;
        }
        self.replace_text_in_range(None, "  ", window, cx);
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if !self.offering.is_empty() {
            self.offering.clear();
            self.walked_offer = false;
            cx.notify();
        }
    }

    fn save_action(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.save(cx);
    }

    pub fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    pub fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text[self.selected_range.clone()].to_string(),
            ));
        }
    }

    pub fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.text[self.selected_range.clone()].to_string(),
        ));
        self.replace_text_in_range(None, "", window, cx);
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    // --- completion ---------------------------------------------------------

    /// Works out what to offer for wherever the caret now is.
    fn refresh_offer(&mut self, _cx: &mut Context<Self>) {
        let (line, at) = self.caret_in_line();
        let found = context(self.language, &line, at);
        self.offering = match &found {
            Where::Nowhere => Vec::new(),
            _ => candidates(&found, self.language, &self.tables, &self.vars),
        };
        self.offer_context = found;
        self.offering_at = 0;
        self.walked_offer = false;
    }

    /// Moves through the offer, if one is showing. Returns whether it did, so
    /// the caller knows whether the key was used.
    fn walk_offer(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        if self.offering.is_empty() {
            return false;
        }
        self.walked_offer = true;
        let len = self.offering.len() as isize;
        self.offering_at = (self.offering_at as isize + delta).rem_euclid(len) as usize;
        cx.notify();
        true
    }

    /// Inserts whatever is highlighted. Returns whether there was anything.
    fn take_offer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(candidate) = self.offering.get(self.offering_at).cloned() else { return false };
        // An offer belongs to the word right before the caret, and changing
        // the selection does not work one out again: a shift-arrow, a drag or
        // a double click leaves the list describing a word the caret has left.
        // Taking it then counted those same bytes back from the new caret and
        // wrote over whatever was sitting there, and dropped the selected text
        // instead of replacing it. In a workflow something is offered almost
        // everywhere, so a double click and then tab silently mangled a line.
        if !self.selected_range.is_empty() {
            return false;
        }
        let back = replacing(&self.offer_context);
        let at = self.cursor();
        let start = self.clamp(at.saturating_sub(back));

        self.selected_range = start..at;
        self.selection_reversed = false;
        self.replace_text_in_range(None, &insertion(&self.offer_context, &candidate), window, cx);
        self.offering.clear();
        self.walked_offer = false;
        cx.notify();
        true
    }

    /// One markdown formatting action, applied to the selection.
    ///
    /// The point of a toolbar is that it does what you would have typed, so
    /// each of these is a toggle: pressing Bold on text that is already bold
    /// takes the marks off again rather than doubling them.
    pub fn markup(&mut self, what: Markup, cx: &mut Context<Self>) {
        let here = self.selected_range.clone();
        let (range, text, select) = match what {
            Markup::Around(marks) => around(&self.text, &here, marks),
            Markup::Prefix(mark) => prefix(&self.text, &here, mark),
            Markup::Numbered => numbered(&self.text, &here),
            Markup::Link => {
                let inner = &self.text[here.clone()];
                let shown = if inner.is_empty() { "text" } else { inner };
                let put = format!("[{shown}](url)");
                // The caret lands on `url`, which is the part that has to be
                // typed whatever else was selected.
                let at = here.start + put.len() - 4;
                (here.clone(), put, at..at + 3)
            }
            Markup::Fence => {
                let inner = &self.text[here.clone()];
                let put = format!("```\n{inner}\n```");
                let at = here.start + 4;
                (here.clone(), put, at..at + inner.len())
            }
            Markup::Rule => {
                let put = "\n---\n".to_string();
                let at = here.start + put.len();
                (here.clone(), put, at..at)
            }
            Markup::Expression => {
                let inner = &self.text[here.clone()];
                let put = format!("{{{{ {inner} }}}}");
                let at = here.start + 3;
                (here.clone(), put, at..at + inner.len())
            }
            Markup::Table => {
                let put = "| Name | Value |\n| --- | --- |\n|  |  |\n".to_string();
                let at = here.start + put.len();
                (here.clone(), put, at..at)
            }
        };

        self.text.replace_range(range, &text);
        self.selected_range = select;
        self.selection_reversed = false;
        self.marked_range = None;
        self.dirty = true;
        self.revision += 1;
        self.refresh_offer(cx);
        cx.notify();
    }

    /// Where to draw the completion list, if there is one to draw.
    pub fn offer_at(&self) -> Option<Point<Pixels>> {
        if self.offering.is_empty() {
            return None;
        }
        self.caret_point()
    }

    /// Whether anything is selected, which decides what a right-click can
    /// usefully offer.
    pub fn has_selection(&self) -> bool {
        !self.selected_range.is_empty()
    }

    /// How wide the text is laid out, once it has been laid out once.
    ///
    /// What the completion list is kept inside: it follows the caret, and a
    /// caret near the right-hand end would otherwise put a 280-pixel list out
    /// over whatever is beside the editor, which for a document is the
    /// preview of the very text being typed.
    pub fn laid_out_width(&self) -> Option<Pixels> {
        self.bounds.map(|b| b.size.width)
    }

    // --- mouse ---------------------------------------------------------------

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        let at = self.offset_for(event.position);
        if event.modifiers.shift {
            self.select_to(at, cx);
        } else if event.click_count >= 3 {
            let line = self.line_at(at);
            self.selected_range = line.clone();
            self.selection_reversed = false;
            cx.notify();
        } else if event.click_count == 2 {
            self.selected_range = self.word_at(at);
            self.selection_reversed = false;
            cx.notify();
        } else {
            self.move_to(at, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            let at = self.offset_for(event.position);
            self.select_to(at, cx);
        }
    }

    fn word_at(&self, at: usize) -> Range<usize> {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let start = self.text[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(at, |(i, _)| i);
        let end = self.text[at..]
            .char_indices()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(at, |(i, c)| at + i + c.len_utf8());
        start..end
    }

    /// Which byte of the document is under a point.
    fn offset_for(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.bounds else { return 0 };
        let Some(placed) = self.placed_at(position.y - bounds.top()) else {
            return self.text.len();
        };
        let inside = point(position.x - bounds.left(), position.y - bounds.top() - placed.top);
        let within = placed
            .wrapped
            .closest_index_for_position(inside, self.line_height)
            .unwrap_or_else(|at| at);
        self.clamp(placed.start + within.min(placed.len))
    }

    /// The line whose rows cover a height within the element.
    fn placed_at(&self, y: Pixels) -> Option<&Placed> {
        if y < px(0.) {
            return self.placed.first();
        }
        self.placed
            .iter()
            .find(|p| y >= p.top && y < p.top + p.height(self.line_height))
            .or_else(|| self.placed.last())
    }

    /// Where a byte offset sits within the element.
    ///
    /// The layout is from the last paint, so an offset typed since then falls
    /// past the end of it. The caret is a character away from where it will
    /// be, so rather than answer with nothing, the last line answers
    /// for anything beyond it.
    fn point_for(&self, offset: usize) -> Option<Point<Pixels>> {
        let placed = self
            .placed
            .iter()
            .find(|p| p.holds(offset))
            .or_else(|| self.placed.last().filter(|p| offset > p.end()))?;
        let within = placed
            .wrapped
            .position_for_index(offset.saturating_sub(placed.start).min(placed.len), self.line_height)
            .unwrap_or_else(|| point(px(0.), px(0.)));
        Some(point(within.x, placed.top + within.y))
    }

    // --- text as UTF-16, for the platform ------------------------------------

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let (mut utf8, mut utf16) = (0, 0);
        for ch in self.text.chars() {
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
        for ch in self.text.chars() {
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

    /// Moves the caret one visual row up or down, keeping roughly the column.
    ///
    /// It is done in pixels instead of in characters because a wrapped line
    /// has no characters at its row boundaries to count.
    fn vertical(&self, delta: isize) -> usize {
        let at = self.cursor();
        let Some(here) = self.point_for(at) else { return at };
        let wanted = here.y + self.line_height * delta as f32;

        if wanted < px(0.) {
            return 0;
        }
        let bottom = self
            .placed
            .last()
            .map(|p| p.top + p.height(self.line_height))
            .unwrap_or(px(0.));
        if wanted >= bottom {
            return self.text.len();
        }

        let Some(placed) = self.placed_at(wanted) else { return at };
        let inside = point(here.x, wanted - placed.top);
        let within = placed
            .wrapped
            .closest_index_for_position(inside, self.line_height)
            .unwrap_or_else(|e| e);
        self.clamp(placed.start + within.min(placed.len))
    }

    /// Shapes and wraps the document to a width, returning how many visual
    /// rows it came to.
    ///
    /// Called from layout, which needs the height, and again from prepaint,
    /// which needs the positions. Shaping is cached by the text system, so
    /// the second call is a lookup.
    fn wrap(&mut self, width: Pixels, window: &mut Window) -> usize {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let wrap = Some(width.max(px(120.)));
        let colours = self.colours;
        let coloured = highlight(self.language, &self.text);

        self.placed.clear();
        let (mut at, mut top, mut rows) = (0usize, px(0.), 0usize);

        // `lines()` drops a trailing newline, which would take the last line
        // (the one you are usually typing on) off the screen.
        let mut source: Vec<&str> = self.text.split('\n').collect();
        if source.is_empty() {
            source.push("");
        }

        for (index, line) in source.iter().enumerate() {
            let runs = runs_for(line, coloured.get(index), &style, colours);
            let shaped = window
                .text_system()
                .shape_text(SharedString::from(line.to_string()), font_size, &runs, wrap, None)
                .unwrap_or_default();
            // The text has no newline in it, so this is one wrapped line.
            for wrapped in shaped.into_iter() {
                let count = wrapped.wrap_boundaries().len() + 1;
                self.placed.push(Placed { wrapped, start: at, len: line.len(), top, rows: count });
                top += line_height * count as f32;
                rows += count;
            }
            at += line.len() + 1;
        }
        rows.max(1)
    }

    /// Where the caret is, for placing the completion list under it.
    pub fn caret_point(&self) -> Option<Point<Pixels>> {
        let here = self.point_for(self.cursor())?;
        Some(point(here.x, here.y + self.line_height))
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.text[range].to_string())
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

        let new_text = flattened(new_text);
        self.text.replace_range(range.clone(), &new_text);
        let at = range.start + new_text.len();
        self.selected_range = at..at;
        self.selection_reversed = false;
        self.marked_range = None;
        self.dirty = true;
        self.revision += 1;
        self.refresh_offer(cx);
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

        self.text.replace_range(range.clone(), new_text);
        self.marked_range = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        self.selected_range = marked_caret(range.start, new_text, new_selected_range_utf16.as_ref());
        self.dirty = true;
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
        let at = self.range_from_utf16(&range_utf16).start;
        let here = self.point_for(at)?;
        Some(Bounds::new(
            point(bounds.left() + here.x, bounds.top() + here.y),
            size(px(1.), self.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.offset_for(point)))
    }
}

/// Text arriving from outside, with Windows line endings flattened to bare
/// line feeds.
///
/// A line boundary here is one `\n` and every position is a byte offset, so a
/// `\r\n` — out of a file written on Windows, or pasted from a Windows program
/// or a browser — left the carriage return sitting at the end of the line's own
/// text, where nothing could tell it from a character anyone typed. End put the
/// caret between the two bytes, as did a click past the end of the line, and
/// the next character typed there went in between them: the line ending was
/// split and a stray carriage return was left in the middle of a line, which
/// the colouring, the completion and the file that was then written all read as
/// text. A lone carriage return is left as it is, since it is not a line ending
/// here and inventing a line break the document never had would be a worse
/// guess than showing the character.
fn flattened(text: &str) -> Cow<'_, str> {
    if text.contains('\r') { Cow::Owned(text.replace("\r\n", "\n")) } else { Cow::Borrowed(text) }
}

/// Where the selection the IME asks for inside the text it has just marked
/// falls in the document.
///
/// `at` is where that text starts, and the range the platform gives is counted
/// in UTF-16 units from there, not in bytes. Adding those units straight onto
/// a byte offset agrees only for text that is ASCII: two Japanese characters
/// are two units and six bytes, so the caret was left two bytes into the first
/// of them, inside a character. Anything that then sliced the document there,
/// the next edit's `replace_range` or working out which line the caret is on,
/// panicked, so composing a word and carrying on typing took the program down.
fn marked_caret(at: usize, marked: &str, selected_utf16: Option<&Range<usize>>) -> Range<usize> {
    let byte_of = |wanted: usize| {
        let (mut utf8, mut utf16) = (0, 0);
        for ch in marked.chars() {
            if utf16 >= wanted {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    };
    match selected_utf16 {
        Some(range) => at + byte_of(range.start)..at + byte_of(range.end),
        // Nothing said where to put it, so it goes after what was marked.
        None => at + marked.len()..at + marked.len(),
    }
}

/// The element that shapes and paints the document.
struct Body {
    editor: Entity<Editor>,
}

struct Painted {
    placed: Vec<Placed>,
    caret: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl IntoElement for Body {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl gpui::Element for Body {
    type RequestLayoutState = ();
    type PrepaintState = Painted;

    fn id(&self) -> Option<gpui::ElementId> {
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
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // How tall the document is depends on how wide it is allowed to be, so
        // the height is measured, not declared.
        let editor = self.editor.clone();
        let mut style = Style::default();
        style.size.width = relative(1.).into();

        let id = window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.unwrap_or(match available.width {
                gpui::AvailableSpace::Definite(w) => w,
                _ => px(720.),
            });
            let rows = editor.update(cx, |editor, _| editor.wrap(width, window));
            gpui::Size { width, height: window.line_height() * rows as f32 }
        });
        (id, ())
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
        let line_height = window.line_height();
        let placed = self.editor.update(cx, |editor, _| {
            editor.line_height = line_height;
            editor.wrap(bounds.size.width, window);
            std::mem::take(&mut editor.placed)
        });

        let editor = self.editor.read(cx);
        let colours = editor.colours;
        let selected = editor.selected_range.clone();
        let cursor = editor.cursor();

        let mut selection = Vec::new();
        let mut caret = None;

        for line in &placed {
            // The selection, drawn a visual row at a time so a wrapped line
            // highlights the way it reads.
            if !selected.is_empty() && selected.start <= line.end() && selected.end >= line.start {
                let from = selected.start.max(line.start) - line.start;
                let to = selected.end.min(line.end()) - line.start;
                let (a, b) = (
                    line.wrapped.position_for_index(from, line_height),
                    line.wrapped.position_for_index(to, line_height),
                );
                if let (Some(a), Some(b)) = (a, b) {
                    let width = line.wrapped.width();
                    let mut row_top = a.y;
                    while row_top <= b.y {
                        let left = if row_top == a.y { a.x } else { px(0.) };
                        let right = if row_top == b.y { b.x } else { width };
                        // A selection that swallows a line ending shows it as
                        // a sliver, so an empty line still reads as selected.
                        let right = if right <= left { left + px(4.) } else { right };
                        selection.push(fill(
                            Bounds::from_corners(
                                point(bounds.left() + left, bounds.top() + line.top + row_top),
                                point(
                                    bounds.left() + right,
                                    bounds.top() + line.top + row_top + line_height,
                                ),
                            ),
                            colours.selection,
                        ));
                        row_top += line_height;
                    }
                }
            }

            if selected.is_empty()
                && caret.is_none()
                && line.holds(cursor)
                && let Some(at) = line.wrapped.position_for_index(cursor - line.start, line_height)
            {
                caret = Some(fill(
                    Bounds::new(
                        point(bounds.left() + at.x, bounds.top() + line.top + at.y),
                        size(px(1.6), line_height),
                    ),
                    colours.caret,
                ));
            }
        }

        Painted { placed, caret, selection }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        painted: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.editor.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);

        for quad in painted.selection.drain(..) {
            window.paint_quad(quad);
        }

        let line_height = window.line_height();
        for line in &painted.placed {
            let origin = point(bounds.left(), bounds.top() + line.top);
            let _ = line.wrapped.paint(
                origin,
                line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            );
        }
        if focus.is_focused(window)
            && let Some(caret) = painted.caret.take()
        {
            window.paint_quad(caret);
        }

        let placed = std::mem::take(&mut painted.placed);
        self.editor.update(cx, |editor, _| {
            editor.placed = placed;
            editor.bounds = Some(bounds);
            editor.line_height = line_height;
        });
    }
}

/// The coloured runs of one line, from its spans.
fn runs_for(
    text: &str,
    spans: Option<&Vec<Span>>,
    style: &gpui::TextStyle,
    colours: Colours,
) -> Vec<TextRun> {
    let mut default_font = style.font();
    default_font.family = super::theme::MONO.into();

    let Some(spans) = spans else {
        return vec![TextRun {
            len: text.len(),
            font: default_font,
            color: colours.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
    };

    let mut runs: Vec<TextRun> = Vec::with_capacity(spans.len());
    for span in spans {
        // The spans belong to the whole source line; a wrapped piece of it
        // only wants the part that falls inside it.
        let start = span.range.start.min(text.len());
        let end = span.range.end.min(text.len());
        if end <= start {
            continue;
        }
        let mut font = default_font.clone();
        font.weight = Colours::weight_of(span.kind);
        if span.kind == Kind::Emphasis {
            font.style = gpui::FontStyle::Italic;
        }
        runs.push(TextRun {
            len: end - start,
            font,
            color: colours.of_kind(span.kind),
            background_color: None,
            underline: (span.kind == Kind::Link).then(|| UnderlineStyle {
                color: Some(colours.accent),
                thickness: px(1.),
                wavy: false,
            }),
            strikethrough: None,
        });
    }
    let covered: usize = runs.iter().map(|r| r.len).sum();
    if covered < text.len() {
        runs.push(TextRun {
            len: text.len() - covered,
            font: default_font,
            color: colours.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    runs
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Editor")
            .track_focus(&self.focus_handle)
            // The editor sets its own face and does not inherit one: source
            // is read column by column, and a proportional font makes an
            // indent or an alignment impossible to see.
            .font_family(super::theme::MONO)
            .cursor(gpui::CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::save_action))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::show_character_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .size_full()
            .child(Body { editor: cx.entity() })
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a formatting action leaves behind: the whole text, and what is
    /// selected afterwards.
    fn did(text: &str, select: std::ops::Range<usize>, what: Markup) -> (String, String) {
        let (range, put, chosen) = match what {
            Markup::Around(marks) => around(text, &select, marks),
            Markup::Prefix(mark) => prefix(text, &select, mark),
            Markup::Numbered => numbered(text, &select),
            _ => unreachable!("only the three that reach outside the selection"),
        };
        let mut out = text.to_string();
        out.replace_range(range, &put);
        let chosen = out[chosen].to_string();
        (out, chosen)
    }

    #[test]
    fn bold_goes_on_and_comes_off_again() {
        // On.
        let (text, chosen) = did("a word here", 2..6, Markup::Around("**"));
        assert_eq!(text, "a **word** here");
        assert_eq!(chosen, "word");

        // Off, with the whole of it selected.
        let (text, chosen) = did("a **word** here", 2..10, Markup::Around("**"));
        assert_eq!(text, "a word here");
        assert_eq!(chosen, "word");

        // Off, with only the word selected, which is where pressing the
        // button twice in a row leaves the selection.
        let (text, chosen) = did("a **word** here", 4..8, Markup::Around("**"));
        assert_eq!(text, "a word here");
        assert_eq!(chosen, "word");
    }

    #[test]
    fn a_line_mark_applies_to_every_line_the_selection_touches() {
        let source = "one\ntwo\nthree";
        // Selecting the middle of the first line and the middle of the last
        // marks all three: a bullet is a property of a line, not of a word.
        let (text, _) = did(source, 1..10, Markup::Prefix("- "));
        assert_eq!(text, "- one\n- two\n- three");

        // And again takes them off.
        let (back, _) = did(&text, 1..16, Markup::Prefix("- "));
        assert_eq!(back, source);
    }

    #[test]
    fn a_numbered_list_counts_from_one_and_undoes_cleanly() {
        let (text, _) = did("one\ntwo\nthree", 0..13, Markup::Numbered);
        assert_eq!(text, "1. one\n2. two\n3. three");
        let (back, _) = did(&text, 0..22, Markup::Numbered);
        assert_eq!(back, "one\ntwo\nthree");
    }

    #[test]
    fn a_mark_on_one_line_leaves_its_neighbours_alone() {
        let source = "one\ntwo\nthree";
        let (text, _) = did(source, 5..6, Markup::Prefix("## "));
        assert_eq!(text, "one\n## two\nthree");
    }

    #[test]
    fn the_caret_the_ime_asks_for_while_composing_japanese_stays_on_a_character() {
        // The platform counts the caret in UTF-16 units from the start of the
        // text it marked. Two Japanese characters are two of those units and
        // six bytes, so taking the count for a count of bytes left the caret
        // two bytes into the first character, and the next thing to slice the
        // document there brought the program down.
        let document = "ab にほ";
        let caret = marked_caret(3, "にほ", Some(&(2..2)));
        assert_eq!(caret, 9..9);
        assert_eq!(document.len(), 9);
        assert!(document.is_char_boundary(caret.start));
    }

    #[test]
    fn the_segment_the_ime_is_converting_lands_on_character_boundaries() {
        // The segment being converted is underlined, and it arrives in UTF-16
        // units as well: the second and third characters of "にほん" are bytes
        // 3 to 9, not bytes 1 to 3.
        let marked = "にほん";
        assert_eq!(marked_caret(0, marked, Some(&(0..1))), 0..3);
        assert_eq!(marked_caret(0, marked, Some(&(1..3))), 3..9);
        for range in [0..1, 1..3, 0..3] {
            let caret = marked_caret(4, marked, Some(&range));
            assert!(marked.is_char_boundary(caret.start - 4));
            assert!(marked.is_char_boundary(caret.end - 4));
        }
    }

    #[test]
    fn a_character_outside_the_basic_plane_is_two_units_and_four_bytes() {
        // An emoji is one character and four bytes, but the platform counts it
        // as the two UTF-16 units a surrogate pair takes.
        assert_eq!(marked_caret(0, "🙂", Some(&(2..2))), 4..4);
        assert_eq!(marked_caret(0, "🙂", Some(&(0..2))), 0..4);
    }

    #[test]
    fn nothing_asked_for_leaves_the_caret_after_what_was_marked() {
        assert_eq!(marked_caret(7, "にほ", None), 13..13);
    }

    #[test]
    fn typing_at_the_end_of_a_line_from_a_windows_file_keeps_the_line_ending_whole() {
        // End puts the caret at the end of the line, which is worked out by
        // looking for the `\n`. On "one\r\ntwo" that byte is one past the
        // carriage return, so what was typed there went between the two halves
        // of the line ending and left a stray carriage return in the middle of
        // the line above it.
        let mut document = flattened("one\r\ntwo").into_owned();
        let end_of_first_line = document.find('\n').unwrap_or(document.len());
        document.insert(end_of_first_line, '!');
        assert_eq!(document, "one!\ntwo");
    }

    #[test]
    fn a_document_written_on_windows_arrives_with_no_carriage_returns_in_it() {
        let document = flattened("alpha\r\nbeta\r\n");
        assert_eq!(&*document, "alpha\nbeta\n");
        assert!(!document.contains('\r'));
    }

    #[test]
    fn a_document_that_already_uses_bare_line_feeds_is_not_copied() {
        assert!(matches!(flattened("alpha\nbeta\n"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_carriage_return_with_no_line_feed_after_it_stays_the_character_it_is() {
        // It is not a line ending here, and showing a line break the document
        // does not have is a worse guess than showing the character.
        assert_eq!(&*flattened("one\rtwo"), "one\rtwo");
    }
}

/// One thing the formatting controls can do to the selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Markup {
    /// Put these marks on both sides, or take them off if they are there.
    Around(&'static str),
    /// Put this at the start of every line the selection touches, or take it
    /// off if every one of them already has it.
    Prefix(&'static str),
    Numbered,
    Link,
    Fence,
    Rule,
    /// `{{ … }}`, which is what a document is written for.
    Expression,
    Table,
}

impl Markup {
    pub fn label(self) -> &'static str {
        match self {
            Markup::Around("**") => "Bold",
            Markup::Around("*") => "Italic",
            Markup::Around("`") => "Code",
            Markup::Around("~~") => "Strikethrough",
            Markup::Around(_) => "Wrap",
            Markup::Prefix("# ") => "Heading 1",
            Markup::Prefix("## ") => "Heading 2",
            Markup::Prefix("### ") => "Heading 3",
            Markup::Prefix("- ") => "Bullet list",
            Markup::Prefix("> ") => "Quote",
            Markup::Prefix(_) => "Prefix",
            Markup::Numbered => "Numbered list",
            Markup::Link => "Link",
            Markup::Fence => "Code block",
            Markup::Rule => "Divider",
            Markup::Expression => "Expression",
            Markup::Table => "Table",
        }
    }
}

/// Marks on both sides of the selection, or off it if they are already there.
///
/// Returns what to replace as well as what to put there, because taking the
/// marks off `**word**` with only `word` selected means reaching outside the
/// selection for them.
fn around(text: &str, range: &Range<usize>, marks: &str) -> (Range<usize>, String, Range<usize>) {
    let inner = &text[range.clone()];
    let width = marks.len();

    // `**word**` with the whole of it selected.
    if inner.len() >= 2 * width && inner.starts_with(marks) && inner.ends_with(marks) {
        let bare = inner[width..inner.len() - width].to_string();
        let end = range.start + bare.len();
        return (range.clone(), bare, range.start..end);
    }
    // `**word**` with only `word` selected, which is what pressing Bold
    // twice in a row leaves behind.
    if text[..range.start].ends_with(marks) && text[range.end..].starts_with(marks) {
        let wider = range.start - width..range.end + width;
        let bare = inner.to_string();
        let end = wider.start + bare.len();
        return (wider.clone(), bare, wider.start..end);
    }

    let put = format!("{marks}{inner}{marks}");
    let at = range.start + width;
    (range.clone(), put, at..at + inner.len())
}

/// A mark at the start of every line the selection touches.
fn prefix(text: &str, range: &Range<usize>, mark: &str) -> (Range<usize>, String, Range<usize>) {
    // The whole of the first and last lines, not just what was selected: a
    // heading is a property of a line.
    let start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[range.end..].find('\n').map_or(text.len(), |at| range.end + at);
    let block = &text[start..end];

    let all_have = block.lines().all(|line| line.trim_start().starts_with(mark.trim_end()));
    let put: Vec<String> = block
        .lines()
        .map(|line| {
            if all_have {
                // Off again: the mark and the one space after it.
                line.trim_start().strip_prefix(mark.trim_end()).map_or_else(
                    || line.to_string(),
                    |rest| rest.strip_prefix(' ').unwrap_or(rest).to_string(),
                )
            } else {
                format!("{mark}{line}")
            }
        })
        .collect();
    let put = put.join("\n");
    let end = start + put.len();
    (start..end_of_block(text, range), put, start..end)
}

/// `1. `, `2. `, `3. ` down the selected lines.
fn numbered(text: &str, range: &Range<usize>) -> (Range<usize>, String, Range<usize>) {
    let start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[range.end..].find('\n').map_or(text.len(), |at| range.end + at);
    let block = &text[start..end];

    let numbered_already = block.lines().all(|line| {
        let trimmed = line.trim_start();
        trimmed
            .split_once(". ")
            .is_some_and(|(head, _)| !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()))
    });
    let put: Vec<String> = block
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if numbered_already {
                line.trim_start().split_once(". ").map_or_else(|| line.to_string(), |(_, rest)| rest.to_string())
            } else {
                format!("{}. {line}", i + 1)
            }
        })
        .collect();
    let put = put.join("\n");
    let end = start + put.len();
    (start..end_of_block(text, range), put, start..end)
}

/// The end of the last line the selection touches.
fn end_of_block(text: &str, range: &Range<usize>) -> usize {
    text[range.end..].find('\n').map_or(text.len(), |at| range.end + at)
}
