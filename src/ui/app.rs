//! The application: the open workspaces, the tools that fill them, and the
//! keyboard and mouse that move between them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App as GpuiApp, AppContext, Context, Entity, FocusHandle, Focusable, KeyBinding, ScrollHandle,
    Window, actions,
};

use crate::core::{Cancel, Emitter, Event, FieldKind, Params, Role, Tool};
use crate::net::iface::{self, Iface};

use crate::flow::Step;
use crate::flow::edit::{Change, Shape, Spot};

use super::job::{Job, Msg, State};
use super::menu::{Act, Menu};
use super::palette::Palette;
use super::runtime::runtime;
use super::store::{self, Tag};
use super::text_input::TextInput;
use super::theme::{Mode, Theme};
use super::workspace::{Group, Workspace};

actions!(
    ntls,
    [
        NewWorkspace,
        CloseWorkspace,
        CloseJob,
        NextWorkspace,
        PrevWorkspace,
        NextJob,
        PrevJob,
        AddTool,
        Run,
        Stop,
        /// The selected tool's form. The application's own settings are
        /// `Preferences`, the name every other desktop uses.
        Settings,
        Preferences,
        ShowNotices,
        FocusFilter,
        FocusNote,
        ToggleProperties,
        ToggleLog,
        ToggleSidebar,
        ShowWorkspaces,
        ShowTools,
        ToggleChart,
        ToggleTheme,
        MoveUp,
        MoveDown,
        PageUp,
        PageDown,
        First,
        Last,
        Confirm,
        Dismiss,
        ExpandField,
        Open1,
        Open2,
        Open3,
        Open4,
        Open5,
        Open6,
        Open7,
        Open8,
        Save,
        Quit,
    ]
);

pub fn bind_keys(cx: &mut GpuiApp) {
    // Written the macOS way and rewritten for whichever system this is: ⌘ on
    // macOS, Ctrl on Windows and Linux, where ⌘ is the key that opens the
    // start menu.
    cx.bind_keys([
        KeyBinding::new(&crate::sys::shortcut("cmd-s"), Save, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-s"), Save, None),
        KeyBinding::new(&crate::sys::shortcut("cmd-t"), NewWorkspace, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-shift-w"), CloseWorkspace, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-w"), CloseJob, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-shift-]"), NextWorkspace, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-shift-["), PrevWorkspace, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-]"), NextJob, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-["), PrevJob, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-k"), AddTool, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-n"), AddTool, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-r"), Run, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-."), Stop, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-e"), Settings, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-,"), Preferences, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-,"), Preferences, None),
        KeyBinding::new(&crate::sys::shortcut("cmd-shift-m"), ShowNotices, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-f"), FocusFilter, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-shift-n"), FocusNote, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-i"), ToggleProperties, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-j"), ToggleLog, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-b"), ToggleSidebar, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-shift-o"), ShowWorkspaces, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-shift-e"), ShowTools, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-g"), ToggleChart, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-d"), ToggleTheme, Some("Ntls")),
        // These four mean different things depending on what is open, so they
        // resolve to one action each and the handler decides.
        KeyBinding::new("up", MoveUp, Some("Ntls")),
        KeyBinding::new("down", MoveDown, Some("Ntls")),
        KeyBinding::new("enter", Confirm, Some("Ntls")),
        KeyBinding::new("escape", Dismiss, Some("Ntls")),
        KeyBinding::new("tab", ExpandField, Some("Ntls")),
        KeyBinding::new("pageup", PageUp, Some("Ntls")),
        KeyBinding::new("pagedown", PageDown, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-up"), First, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-down"), Last, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-1"), Open1, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-2"), Open2, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-3"), Open3, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-4"), Open4, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-5"), Open5, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-6"), Open6, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-7"), Open7, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-8"), Open8, Some("Ntls")),
        KeyBinding::new(&crate::sys::shortcut("cmd-q"), Quit, None),
    ]);
}

pub struct App {
    pub registry: crate::core::Registry,
    pub workspaces: Vec<Workspace>,
    pub active: usize,

    pub palette: Entity<Palette>,
    pub palette_open: bool,

    /// A searchable list over the window: which run to compare against, which
    /// folder to move a tool into.
    pub picker: Entity<super::picker::Picker>,
    pub picker_open: bool,

    pub focus_handle: FocusHandle,
    next_job_id: usize,
    next_ws_id: usize,

    /// The interfaces this machine has addresses on, refreshed while you
    /// watch.
    pub ifaces: Vec<Iface>,
    _iface_poll: gpui::Task<()>,
    /// Repaints while something is running, so elapsed times and rates move.
    _tick: gpui::Task<()>,

    /// Leaving a set of results remembers whichever host was highlighted, and
    /// any tool added afterwards with an empty target box begins on it.
    pub last_target: Option<String>,
    pub last_ports: Option<String>,

    /// Which of the side bar's views is showing.
    pub view: View,
    /// Whether the side bar is showing at all, and how wide it is.
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    /// When the side bar started opening or closing, so it can be drawn part
    /// of the way there.
    ///
    /// A panel that is gone on the next frame takes the layout with it in one
    /// jump. Sliding keeps the rest of the window still.
    sidebar_moved: Option<(std::time::Instant, bool)>,
    /// Whether the output panel under the editor is showing.
    pub panel_open: bool,

    pub rail_scroll: ScrollHandle,
    pub tab_scroll: ScrollHandle,
    pub welcome_scroll: ScrollHandle,

    /// One editor, re-pointed at whichever result is selected.
    pub note_input: Entity<TextInput>,
    /// The line of the table whose note is being typed into, and which row
    /// that line stood for when the typing started.
    ///
    /// The line is where the editor is drawn; the row is where the words go.
    /// Sorting and filtering reorder the lines, and a running scan adds rows
    /// underneath, so the two drift apart. Keeping both stops a note about one
    /// host being written onto another.
    pub editing_note: Option<usize>,
    editing_note_row: Option<(usize, String)>,
    note_revision: usize,
    /// Folded/collapsed sections and folders in the sidebar.
    pub collapsed: std::collections::HashSet<String>,
    /// One editor for naming the selected run.
    pub rename_input: Entity<TextInput>,
    pub rename_for: Option<usize>,
    rename_revision: usize,
    /// And one for the value in the condition of whichever workflow step is
    /// selected. That is the only part of a condition that is typed instead of
    /// chosen from what exists.
    pub step_input: Entity<TextInput>,
    step_value_for: Option<(usize, Spot)>,
    step_value_revision: usize,
    /// And one for the name a `set` step keeps its answer under.
    pub step_name_input: Entity<TextInput>,
    step_name_for: Option<(usize, Spot)>,
    step_name_revision: usize,
    /// And one for naming the workspace.
    pub ws_rename_input: Entity<TextInput>,
    ws_rename_for: Option<usize>,
    ws_rename_revision: usize,
    /// Whether the tools side bar is listing the workspace directory as it is
    /// on disk, and does not group what is open by the stage it is at.
    pub files_view: bool,
    /// What that directory held when it was last read, and when.
    files_seen: Option<(std::path::PathBuf, Instant, Vec<FileRow>)>,
    /// One box for the variable editor, and which half of which variable it
    /// is pointed at.
    pub var_input: Entity<TextInput>,
    pub editing_var: Option<(String, VarPart)>,
    var_revision: usize,
    /// The box that narrows the variables page down, and what is in it.
    pub var_filter_input: Entity<TextInput>,
    pub var_filter: String,
    var_filter_revision: usize,
    /// The document or workflow whose name is being edited in its pane.
    pub renaming_item: Option<super::workspace::Item>,
    item_rename_revision: usize,
    /// Whether the selected run's name is being edited in place.
    pub renaming: bool,
    /// Whether the workspace's name is being edited in the rail.
    pub renaming_workspace: bool,
    /// The column being dragged, with where the drag started and how wide the
    /// column was then.
    pub resizing: Option<Resize>,
    /// The context menu, if one is open. There is only ever one.
    pub menu: Option<Menu>,
    /// The file being edited in the pane, if any, and which item it belongs
    /// to. One editor at a time: the pane shows one thing.
    pub editing: Option<(super::workspace::Item, Entity<super::editor::Editor>)>,
    /// A run a workflow was waiting on that has just finished, with whether it
    /// failed. Acted on at the next repaint, which has a window.
    flow_pending: Option<(usize, bool)>,
    /// The editor revision already folded back into what is on screen.
    last_edit_revision: usize,
    /// A stage whose "clear" has been pressed once. Pressing it again does it;
    /// pressing anything else forgets it.
    /// The heading whose "Delete all" is armed, and the folder it is in. A
    /// group inside a folder empties that folder, not the workspace.
    pub clearing: Option<(Group, Option<String>)>,
    /// Everything the application remembers about itself between sessions:
    /// what new tools start on, how it looks, what it says and when. The
    /// settings page is a view of this, and it is written as it changes.
    pub settings: store::AppState,
    /// What the application has to say for itself: runs that finished while
    /// you were elsewhere, and anything that went wrong.
    pub notices: super::notify::Notices,
    /// The page in front of the workspace, if any. Unlike a view, a page takes
    /// the whole window, side bar and tabs included: what it is about does not
    /// live inside the workspace.
    pub page: Option<Page>,
    /// Where each row of choices is sliding from and to, by the control's own
    /// name.
    ///
    /// An element cannot remember what it was showing a moment ago, and a
    /// light that slides has to know where it started. The next frame sees the
    /// light already at its destination, so without both ends it would cut the
    /// slide short.
    pub segment_from: std::collections::HashMap<String, (usize, usize)>,
    /// Set while files are being dragged over the window, so it can say it
    /// will take them.
    pub drop_hover: bool,
}

/// A drag in progress, of a table column or of the side bar's edge.
#[derive(Clone, Copy, Debug)]
pub struct Resize {
    /// The column being dragged, or `None` for the side bar.
    pub column: Option<usize>,
    pub from_x: f32,
    pub from_width: f32,
}

/// What the side bar is showing. One per activity-bar icon, in the order they
/// appear down the rail.
///
/// Workspaces sits above tools because it is the wider choice: which
/// investigation you are in, and then what is inside it.
/// Whether a piece of text names something, as a word instead of as a run of
/// letters inside another word.
fn mentions(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let (text, name) = (text.to_lowercase(), name.to_lowercase());
    let mut from = 0;
    while let Some(at) = text[from..].find(&name) {
        let start = from + at;
        let end = start + name.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        if !word(before) && !word(after) {
            return true;
        }
        from = end;
    }
    false
}

/// One entry of the workspace directory, as the side bar lists it.
#[derive(Clone, Debug)]
pub struct FileRow {
    pub name: String,
    pub path: std::path::PathBuf,
    /// How deep inside the workspace it sits.
    pub depth: usize,
    pub directory: bool,
    pub size: u64,
    /// What the workspace makes of it, when it makes anything: this file is
    /// that tool, that document, that workflow.
    pub item: Option<super::workspace::Item>,
}

/// Which part of a variable is being typed into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VarPart {
    Name,
    Value,
    /// What it is for, in the words of whoever made it.
    About,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// Every workspace, and the folders you can open as one.
    Workspaces,
    /// Everything open in the current workspace, grouped by what stage it is
    /// at.
    Tools,
}

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Workspaces => "Workspaces",
            View::Tools => "Tools",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            View::Workspaces => "folder-open",
            View::Tools => "explorer",
        }
    }
}

/// A page: something the window is given over to entirely.
///
/// The difference from a [`View`] is what it is about. A view lists what is
/// inside the workspace, so it sits beside it in the side bar. A page is about
/// something else: the workspace's own named values, the application's own
/// settings. A list of tools next to either would just be in the way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    /// The workspace's variables: what has been written down, and what the
    /// workflows have worked out.
    Variables,
    /// What this machine is attached to, in full.
    Interfaces,
    /// Everything the application remembers about itself.
    Settings,
}

impl Page {
    pub fn title(self) -> &'static str {
        match self {
            Page::Variables => "Variables",
            Page::Interfaces => "This machine",
            Page::Settings => "Settings",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Page::Variables => "variables",
            Page::Interfaces => "globe",
            Page::Settings => "gear",
        }
    }
}

/// Somewhere the rail goes. Views and pages are different things behind the
/// icon, and deliberately identical in front of it: they are all just places.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dest {
    View(View),
    Page(Page),
}

impl Dest {
    /// The rail, top to bottom. Everything about the workspace is in this
    /// group; the application's own settings sit apart, at the foot.
    pub const RAIL: [Dest; 4] = [
        Dest::View(View::Workspaces),
        Dest::View(View::Tools),
        Dest::Page(Page::Variables),
        Dest::Page(Page::Interfaces),
    ];

    pub fn icon(self) -> &'static str {
        match self {
            Dest::View(v) => v.icon(),
            Dest::Page(p) => p.icon(),
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Dest::View(v) => v.title(),
            Dest::Page(p) => p.title(),
        }
    }
}

/// How wide the side bar starts, and the range a drag may take it to.
pub const SIDEBAR_DEFAULT: gpui::Pixels = gpui::px(268.);
pub const SIDEBAR_MIN: f32 = 180.;
pub const SIDEBAR_MAX: f32 = 520.;

/// Where a row that was at `at` is now.
///
/// A note is written to a row while the table it is in may be changing: a
/// scan that is still going adds rows, and one that is keeping what earlier
/// runs found can insert one beside another. So the position is checked
/// against what the row said it was, and the row is looked up by that if it
/// has moved.
fn locate_row(rows: &[crate::core::Row], at: usize, identity: &str) -> Option<usize> {
    match rows.get(at) {
        Some(row) if row.identity() == identity => Some(at),
        _ => rows.iter().position(|r| r.identity() == identity),
    }
}

/// Whether a target is worth carrying into the next tool opened.
///
/// A host, an address or a subnet is; a placeholder that means "work it out
/// for me" is not, and neither is a pasted link: a downloader's target is a
/// URL, and a URL in a port scanner's target box is only ever a mistake.
fn is_carryable(target: &str) -> bool {
    !target.is_empty()
        && !matches!(target, "auto" | "local" | "lan" | "discover")
        && !target.contains("://")
        && !target.contains(char::is_whitespace)
}

/// Builds the editors a form needs, one per text field.
fn build_inputs(
    fields: &[crate::core::Field],
    params: &Params,
    cx: &mut Context<App>,
) -> Vec<Option<Entity<TextInput>>> {
    fields
        .iter()
        .map(|f| {
            (f.kind == FieldKind::Text).then(|| {
                let (value, placeholder) = (params.raw(f.key).to_string(), f.placeholder.clone());
                cx.new(|cx| TextInput::new(cx, &value, &placeholder))
            })
        })
        .collect()
}

fn new_filter_input(cx: &mut Context<App>) -> Entity<TextInput> {
    cx.new(|cx| {
        let mut input = TextInput::new(cx, "", "Filter results");
        input.mono = false;
        input
    })
}

impl App {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> App {
        let saved = store::read_state();
        cx.set_global(Theme::of(match saved.theme {
            store::ThemeChoice::System => Mode::from_appearance(window.appearance()),
            store::ThemeChoice::Light => Mode::Light,
            store::ThemeChoice::Dark => Mode::Dark,
        }));

        let _ = store::ensure_root();
        let mut app = App {
            registry: crate::tools::all(),
            workspaces: Vec::new(),
            active: 0,
            palette: cx.new(Palette::new),
            palette_open: false,
            picker: cx.new(super::picker::Picker::new),
            picker_open: false,
            focus_handle: cx.focus_handle(),
            next_job_id: 1,
            next_ws_id: 2,
            ifaces: iface::interfaces(),
            _iface_poll: spawn_iface_poll(cx),
            _tick: spawn_tick(cx),
            last_target: None,
            last_ports: None,
            view: View::Tools,
            sidebar_open: true,
            sidebar_width: f32::from(SIDEBAR_DEFAULT),
            sidebar_moved: None,
            panel_open: false,
            rail_scroll: ScrollHandle::new(),
            tab_scroll: ScrollHandle::new(),
            welcome_scroll: ScrollHandle::new(),
            note_input: cx.new(|cx| {
                let mut input = TextInput::new(cx, "", "Add a note");
                input.mono = false;
                input
            }),
            editing_note: None,
            editing_note_row: None,
            note_revision: 0,
            collapsed: std::collections::HashSet::new(),
            rename_input: cx.new(|cx| {
                let mut input = TextInput::new(cx, "", "Name this run");
                input.mono = false;
                input
            }),
            rename_for: None,
            rename_revision: 0,
            renaming_item: None,
            item_rename_revision: 0,
            files_view: false,
            files_seen: None,
            var_input: cx.new(|cx| TextInput::new(cx, "", "value")),
            editing_var: None,
            var_revision: 0,
            var_filter_input: cx.new(|cx| {
                let mut input = TextInput::new(cx, "", "Filter");
                input.mono = false;
                input
            }),
            var_filter: String::new(),
            var_filter_revision: 0,
            step_input: cx.new(|cx| {
                let mut input = TextInput::new(cx, "", "a value");
                input.mono = true;
                input
            }),
            step_value_for: None,
            step_value_revision: 0,
            step_name_input: cx.new(|cx| {
                let mut input = TextInput::new(cx, "", "name");
                input.mono = true;
                input
            }),
            step_name_for: None,
            step_name_revision: 0,
            ws_rename_input: cx.new(|cx| {
                let mut input = TextInput::new(cx, "", "Name this workspace");
                input.mono = false;
                input
            }),
            ws_rename_for: None,
            ws_rename_revision: 0,
            renaming: false,
            renaming_workspace: false,
            resizing: None,
            menu: None,
            editing: None,
            flow_pending: None,
            last_edit_revision: 0,
            clearing: None,
            settings: saved,
            notices: super::notify::Notices::default(),
            page: None,
            segment_from: std::collections::HashMap::new(),
            drop_hover: false,
        };

        super::widgets::set_motion(app.settings.animate);

        // The side bar comes back the way it was left, when it was asked to.
        if app.settings.remember_layout {
            if let Some(open) = app.settings.sidebar_open {
                app.sidebar_open = open;
            }
            if let Some(width) = app.settings.sidebar_width {
                app.sidebar_width = width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
            }
        }

        app.load_from_disk(cx);
        if app.workspaces.is_empty() {
            app.workspaces.push(Workspace::new(1, store::claim_dir("New workspace", &[])));
            app.next_ws_id = 2;
        }
        app
    }

    /// Reads every workspace directory back in at startup.
    ///
    /// A workspace is a directory and every tool in it is a file, so the state
    /// the application starts in is whatever is on disk, including the results
    /// of runs from previous sessions.
    fn load_from_disk(&mut self, cx: &mut Context<Self>) {
        for (dir, record, jobs) in store::load_all() {
            let mut ws = Workspace::new(self.next_ws_id, dir);
            self.next_ws_id += 1;
            ws.restore(&record);

            for (folder, stem, job_record) in jobs {
                if let Some(mut job) = self.restore_job(&stem, &job_record, cx) {
                    job.folder = folder;
                    ws.observe_stem(&stem);
                    ws.add(job);
                }
            }
            ws.docs = super::notes::load(&ws.dir, &mut self.next_job_id);
            ws.flows = super::flows::load(&ws.dir, &mut self.next_job_id);
            // Opening a workspace should show its first tool, not its last.
            ws.selected = ws.jobs.first().map(|j| j.id);
            self.workspaces.push(ws);
        }
        self.active = 0;
    }

    /// Rebuilds one tool from what was written down: its settings, its
    /// editors and its results. It is how a workspace is read at startup, how
    /// a folder is opened, and how a tool is duplicated or dropped in.
    fn restore_job(
        &mut self,
        stem: &str,
        record: &store::JobRecord,
        cx: &mut Context<Self>,
    ) -> Option<Job> {
        let tool = self.registry.get(&record.tool)?;
        let fields = tool.fields();
        let params = record.params.clone();
        let inputs = build_inputs(&fields, &params, cx);
        let filter_input = new_filter_input(cx);

        let id = self.next_job_id;
        self.next_job_id += 1;
        let mut job = Job::new(id, stem.to_string(), tool, params, inputs, filter_input);
        job.restore(record);
        Some(job)
    }

    // --- writing back -----------------------------------------------------

    /// Writes a workspace's own file: its name, colour, notes and order.
    ///
    /// A write that fails is said out loud. Everything here is written as it
    /// changes, so a full disk or a directory that has been moved away under
    /// the application would otherwise lose a session's work without anything
    /// on screen ever mentioning it.
    pub fn save_workspace(&mut self, index: usize) {
        let Some(ws) = self.workspaces.get(index) else { return };
        let (dir, name) = (ws.dir.clone(), ws.name());
        if let Err(e) = store::write_workspace(&dir, &ws.record()) {
            self.cannot_write(&format!("the workspace {name}"), &dir, &e);
        }
    }

    /// Writes one tool's file.
    pub fn save_job(&mut self, ws_index: usize, job_id: usize) {
        let Some(ws) = self.workspaces.get(ws_index) else { return };
        let Some(job) = ws.job(job_id) else { return };
        let (dir, folder, stem, name) =
            (ws.dir.clone(), job.folder.clone(), job.stem.clone(), job.name());
        if let Err(e) = store::write_job_in(&dir, folder.as_deref(), &stem, &job.record()) {
            let path = store::job_path_in(&dir, folder.as_deref(), &stem);
            self.cannot_write(&name, &path, &e);
        }
    }

    /// Says that something could not be written, once.
    ///
    /// Once, because a save happens on every keystroke and a disk that is full
    /// is full for all of them; a hundred identical notices would bury the
    /// first one.
    fn cannot_write(&mut self, what: &str, path: &std::path::Path, e: &std::io::Error) {
        let title = format!("Cannot save {what}");
        if self.notices.newest_first().any(|n| n.title == title) {
            return;
        }
        self.notify(
            crate::core::Level::Error,
            title,
            format!("{}: {e}", path.display()),
            Some(super::notify::About::File(path.to_path_buf())),
        );
    }

    /// Writes the current workspace and the job in front of the user. Almost
    /// every change touches one or both.
    pub fn save_current(&mut self) {
        self.save_workspace(self.active);
        if let Some(id) = self.workspace().selected {
            self.save_job(self.active, id);
        }
    }

    /// Writes everything, for shutdown.
    pub fn save_all(&mut self) {
        let all: Vec<(usize, Vec<usize>)> = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(i, ws)| (i, ws.jobs.iter().map(|j| j.id).collect()))
            .collect();
        for (i, jobs) in all {
            self.save_workspace(i);
            for id in jobs {
                self.save_job(i, id);
            }
        }
    }

    pub fn theme(&self, cx: &GpuiApp) -> Theme {
        super::theme::theme(cx)
    }

    // --- workspaces -------------------------------------------------------

    pub fn workspace(&self) -> &Workspace {
        &self.workspaces[self.active.min(self.workspaces.len() - 1)]
    }

    pub fn workspace_mut(&mut self) -> &mut Workspace {
        let at = self.active.min(self.workspaces.len() - 1);
        &mut self.workspaces[at]
    }

    pub fn selected_job(&self) -> Option<&Job> {
        self.workspace().selected_job()
    }

    pub fn selected_job_mut(&mut self) -> Option<&mut Job> {
        self.workspace_mut().selected_job_mut()
    }

    /// Finds a job anywhere, since events arrive for workspaces that are not
    /// on screen.
    pub fn job_anywhere(&self, id: usize) -> Option<&Job> {
        self.workspaces.iter().find_map(|w| w.job(id))
    }

    pub fn job_anywhere_mut(&mut self, id: usize) -> Option<&mut Job> {
        self.workspaces.iter_mut().find_map(|w| w.job_mut(id))
    }

    pub fn new_workspace(&mut self, cx: &mut Context<Self>) {
        let taken: Vec<_> = self.workspaces.iter().map(|w| w.dir.clone()).collect();
        let ws = Workspace::new(self.next_ws_id, store::claim_dir("New workspace", &taken));
        self.next_ws_id += 1;
        self.workspaces.push(ws);
        self.active = self.workspaces.len() - 1;
        self.save_workspace(self.active);
        cx.notify();
    }

    /// Renames a workspace, and the directory it lives in with it.
    pub fn rename_workspace(&mut self, index: usize, name: &str, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        let Some(ws) = self.workspaces.get(index) else { return };
        let old = ws.dir.clone();

        let taken: Vec<_> = self
            .workspaces
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, w)| w.dir.clone())
            .collect();
        let wanted = store::claim_dir(if name.is_empty() { "New workspace" } else { &name }, &taken);

        if wanted != old && std::fs::rename(&old, &wanted).is_ok() {
            if let Some(ws) = self.workspaces.get_mut(index) {
                ws.dir = wanted;
            }
        }
        if let Some(ws) = self.workspaces.get_mut(index) {
            ws.name_override = (!name.is_empty()).then_some(name);
        }
        self.save_workspace(index);
        cx.notify();
    }

    pub fn tag_workspace(&mut self, index: usize, tag: Tag, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspaces.get_mut(index) {
            ws.tag = tag;
        }
        self.save_workspace(index);
        cx.notify();
    }

    pub fn select_workspace(&mut self, index: usize, cx: &mut Context<Self>) {
        self.clearing = None;
        if index < self.workspaces.len() {
            self.stop_editing(cx);
            self.end_note_edit();
            self.active = index;
            self.rename_for = None;
            self.ws_rename_for = None;
            cx.notify();
        }
    }

    pub fn cycle_workspace(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.workspaces.len() as isize;
        self.active = (self.active as isize + delta).rem_euclid(n) as usize;
        cx.notify();
    }

    /// Closes a workspace and everything in it. There is always one left: an
    /// application with no window to work in has nothing to show.
    pub fn close_workspace(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.workspaces.len() {
            return;
        }
        self.workspaces[index].cancel_all();
        let gone = self.workspaces.remove(index);
        // Closing takes a workspace out of the application and nothing else:
        // the directory stays exactly where it is, and is recorded so that it
        // is not opened again at startup.
        let dir = gone.dir.display().to_string();
        if !self.settings.closed.contains(&dir) {
            self.settings.closed.push(dir);
            self.save_settings();
        }
        self.settle_after_close(cx);
    }

    /// Deletes a workspace and its directory, after asking.
    ///
    /// This is the one action in the program that destroys something, so it is
    /// the one that asks first. It says what it is about to remove instead of
    /// asking whether you are sure.
    pub fn delete_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get(index) else { return };
        let (name, dir, tools) = (ws.name(), ws.dir.clone(), ws.jobs.len());
        let detail = format!(
            "{} and its {} tool{} will be removed from {}. This cannot be undone.",
            name,
            tools,
            if tools == 1 { "" } else { "s" },
            dir.display()
        );
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &format!("Delete the workspace “{name}”?"),
            Some(&detail),
            &["Delete", "Cancel"],
            cx,
        );

        cx.spawn(async move |app, cx| {
            if answer.await != Ok(0) {
                return;
            }
            app.update(cx, |app: &mut App, cx| {
                // The workspace may have moved in the list while the sheet was
                // open, so it is found again by the directory it is.
                let Some(at) = app.workspaces.iter().position(|w| w.dir == dir) else { return };
                app.workspaces[at].cancel_all();
                app.workspaces.remove(at);
                if let Err(e) = store::delete_workspace(&dir) {
                    // Nothing useful can be done about a directory that will
                    // not go, but the user must not be told it went.
                    app.notify(
                        crate::core::Level::Error,
                        "Cannot delete the workspace",
                        format!("{}: {e}", dir.display()),
                        Some(super::notify::About::File(dir.clone())),
                    );
                }
                app.settle_after_close(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Keeps the program in a workable state after a workspace goes: there is
    /// always one to work in, and the selection is inside it.
    fn settle_after_close(&mut self, cx: &mut Context<Self>) {
        if self.workspaces.is_empty() {
            let ws = Workspace::new(self.next_ws_id, store::claim_dir("New workspace", &[]));
            self.next_ws_id += 1;
            self.workspaces.push(ws);
            self.active = 0;
            self.save_workspace(0);
        } else {
            self.active = self.active.min(self.workspaces.len() - 1);
        }
        cx.notify();
    }

    // --- jobs -------------------------------------------------------------

    /// Adds a tool to the current workspace, with its target and ports filled
    /// in when there is something worth filling them with.
    pub fn add_tool(
        &mut self,
        tool: Arc<dyn Tool>,
        target: Option<String>,
        ports: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let fields = tool.fields();
        let mut params = Params::defaults(&fields);

        // Defaults that mean something (auto, discover) are left alone, and
        // a field the tool is currently hiding is not a place to put an
        // address.
        if let Some(field) = fields.iter().find(|f| f.role == Role::Target)
            && field.visible(&params)
        {
            let default = params.str(field.key);
            let carried = target.clone().or_else(|| self.last_target.clone());
            if let Some(t) = carried
                && !t.is_empty()
                && (default.is_empty() || target.is_some())
            {
                params.set(field.key, &t);
            }
        }
        if let Some(key) = tool.ports_key()
            && let Some(p) = ports.or_else(|| self.last_ports.clone())
            && !p.is_empty()
        {
            params.set(key, &p);
        }

        // A machine with two networks on it has one you meant, and saying so
        // once in the settings beats choosing it in every form. It is matched
        // by field instead of by tool, so a tool added later gets it too.
        for field in &fields {
            if let Some(value) = self.settings.default_for(field.key) {
                params.set(field.key, value);
            }
        }

        let inputs = build_inputs(&fields, &params, cx);
        let filter_input = new_filter_input(cx);

        let id = self.next_job_id;
        self.next_job_id += 1;
        let stem = self.workspace_mut().next_stem(tool.id());
        self.workspace_mut().add(Job::new(id, stem, tool, params, inputs, filter_input));

        self.save_current();
        self.save_workspace(self.active);
        self.focus_form(window, cx);
        cx.notify();
        id
    }

    pub fn tag_job(&mut self, tag: Tag, cx: &mut Context<Self>) {
        if let Some(job) = self.selected_job_mut() {
            job.tag = tag;
        }
        self.save_current();
        cx.notify();
    }

    pub fn toggle_favorite(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = self.selected_job_mut() {
            job.favorite = !job.favorite;
        }
        self.save_current();
        cx.notify();
    }

    /// Starts the nth tool, counting from one, as the number keys do.
    pub fn add_nth(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tool) = self.registry.all().get(n.wrapping_sub(1)).cloned() {
            self.add_tool(tool, None, None, window, cx);
        }
    }

    /// Shows a tool. Picking one from the side bar puts it back in the tab
    /// strip, so closing a tab can be undone.
    pub fn select_job(&mut self, id: usize, cx: &mut Context<Self>) {
        self.clearing = None;
        // Asking for a tool is asking to be shown it, so a page in front of
        // the workspace gets out of the way.
        self.page = None;
        self.end_note_edit();
        // Reopening writes the flag back, so the tab strip is the same on the
        // next run as it was on this one.
        let reopened = self.workspace_mut().job_mut(id).is_some_and(|job| {
            let was_shut = !job.open;
            job.open = true;
            was_shut
        });
        if reopened {
            self.save_job(self.active, id);
        }
        self.workspace_mut().select_job(id);
        self.rename_for = None;
        cx.notify();
    }

    /// Closes a tool's tab. The tool stays in the workspace and on disk.
    pub fn close_tab(&mut self, id: usize, cx: &mut Context<Self>) {
        self.workspace_mut().close_tab(id);
        // The job that closed is the one to write, not whichever is selected
        // now: by this point those are different jobs.
        self.save_job(self.active, id);
        self.save_workspace(self.active);
        cx.notify();
    }

    /// Takes a tool out of the workspace. Its file is moved aside, not
    /// deleted.
    pub fn close_job(&mut self, id: usize, cx: &mut Context<Self>) {
        let removed = {
            let ws = self.workspace_mut();
            let where_it_was = ws.job(id).map(|j| (j.folder.clone(), j.stem.clone()));
            ws.close(id);
            where_it_was
        };
        if let Some((folder, stem)) = removed {
            let dir = self.workspace().dir.clone();
            store::remove_job_in(&dir, folder.as_deref(), &stem);
        }
        self.save_workspace(self.active);
        cx.notify();
    }

    /// Closes every tool at a stage, on the second press.
    ///
    /// One click that throws away a morning's scans is one click too few, and
    /// a modal for it would be one dialogue too many; so the word changes to
    /// say what it is about to do, and changes back if you go elsewhere.
    pub fn close_group(
        &mut self,
        group: Group,
        folder: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let here = (group, folder);
        if self.clearing.as_ref() != Some(&here) {
            self.clearing = Some(here);
            cx.notify();
            return;
        }
        self.close_group_now(group, here.1, cx);
    }

    /// Empties a group without asking, for the menu item that spells out what
    /// it does.
    ///
    /// It empties the heading that was clicked and nothing else: a group
    /// inside a folder is that folder's, and the same group at the top of the
    /// workspace is the loose one.
    pub fn close_group_now(
        &mut self,
        group: Group,
        folder: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.clearing = None;
        let place = folder.as_deref();
        match group {
            Group::Stage(stage) => {
                let ids: Vec<usize> =
                    self.workspace().stage_in(place, stage).into_iter().map(|j| j.id).collect();
                for id in ids {
                    self.close_job(id, cx);
                }
            }
            Group::Documents => {
                let ids: Vec<usize> =
                    self.workspace().docs_in(place).into_iter().map(|d| d.id).collect();
                for id in ids {
                    self.delete_doc(id, cx);
                }
            }
            Group::Workflows => {
                let ids: Vec<usize> =
                    self.workspace().flows_in(place).into_iter().map(|f| f.id).collect();
                for id in ids {
                    self.delete_flow(id, cx);
                }
            }
        }
    }

    /// Forgets a pending clear. Anything the user does other than confirming
    /// it counts as changing their mind.
    pub fn cancel_clear(&mut self, cx: &mut Context<Self>) {
        if self.clearing.take().is_some() {
            cx.notify();
        }
    }

    // --- the workspace as files ----------------------------------------------

    /// Shows the workspace directory instead of what is open in it, or the
    /// other way round.
    pub fn toggle_files_view(&mut self, cx: &mut Context<Self>) {
        self.files_view = !self.files_view;
        self.files_seen = None;
        self.view = View::Tools;
        self.sidebar_open = true;
        cx.notify();
    }

    /// The workspace directory, as it is on disk.
    ///
    /// Read at most once a second: the side bar is drawn on every frame, and
    /// a directory read that often is a directory being read for no reason. A
    /// second is also short enough that a run which has just saved, or a file
    /// dropped in from somewhere else, turns up on its own.
    pub fn workspace_files(&mut self) -> &[FileRow] {
        const FRESH: Duration = Duration::from_secs(1);
        let dir = self.workspace().dir.clone();

        let stale = match &self.files_seen {
            Some((seen, at, _)) => *seen != dir || at.elapsed() > FRESH,
            None => true,
        };
        if stale {
            let rows = self.read_workspace_files(&dir);
            self.files_seen = Some((dir, Instant::now(), rows));
        }
        self.files_seen.as_ref().map(|(_, _, rows)| rows.as_slice()).unwrap_or(&[])
    }

    fn read_workspace_files(&self, dir: &std::path::Path) -> Vec<FileRow> {
        let mut rows = Vec::new();
        self.read_files_in(dir, 0, &mut rows);
        rows
    }

    fn read_files_in(&self, dir: &std::path::Path, depth: usize, out: &mut Vec<FileRow>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let (mut dirs, mut files): (Vec<_>, Vec<_>) =
            entries.flatten().partition(|e| e.path().is_dir());
        dirs.sort_by_key(std::fs::DirEntry::path);
        files.sort_by_key(std::fs::DirEntry::path);

        for entry in dirs {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            // `.closed` is where removed files go, and being able to see that
            // it is there is most of the reason it is not a deletion.
            let hidden = name.starts_with('.');
            out.push(FileRow {
                name,
                path: path.clone(),
                depth,
                directory: true,
                size: 0,
                item: None,
            });
            if !hidden {
                self.read_files_in(&path, depth + 1, out);
            }
        }

        for entry in files {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(FileRow { item: self.item_of(&path), name, path, depth, directory: false, size });
        }
    }

    /// Which tool, document or workflow a file is, if the workspace has it
    /// open.
    fn item_of(&self, path: &std::path::Path) -> Option<super::workspace::Item> {
        use super::workspace::Item;
        let ws = self.workspace();
        if let Some(doc) = ws.docs.iter().find(|d| d.path == path) {
            return Some(Item::Doc(doc.id));
        }
        if let Some(flow) = ws.flows.iter().find(|f| f.path == path) {
            return Some(Item::Flow(flow.id));
        }
        let dir = ws.dir.clone();
        ws.jobs
            .iter()
            .find(|job| store::job_path_in(&dir, job.folder.as_deref(), &job.stem) == path)
            .map(|job| Item::Tool(job.id))
    }

    /// Shows whatever a file is: the run, the document, the workflow.
    pub fn open_file_row(&mut self, row: FileRow, cx: &mut Context<Self>) {
        use super::workspace::Item;
        match row.item {
            Some(Item::Tool(id)) => self.select_job(id, cx),
            Some(Item::Doc(id)) => self.select_doc(id, cx),
            Some(Item::Flow(id)) => self.select_flow(id, cx),
            // A file the workspace does not hold (its own record, something
            // dropped in by hand) is still a file, and unrecognised files go
            // to the file manager.
            None => cx.reveal_path(&row.path),
        }
    }

    // --- variables ----------------------------------------------------------

    /// Adds a variable and opens its name for typing.
    pub fn new_var(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut name = String::from("name");
        let mut n = 2;
        while self.workspace().vars.contains_key(&name) {
            name = format!("name{n}");
            n += 1;
        }
        self.workspace_mut().set_var(&name, "");
        self.page = Some(Page::Variables);
        self.save_workspace(self.active);
        self.edit_var(name, VarPart::Name, window, cx);
    }

    /// Puts the caret in one half of one variable.
    pub fn edit_var(
        &mut self,
        name: String,
        part: VarPart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = match part {
            VarPart::Name => name.clone(),
            VarPart::Value => {
                self.workspace().var(&name).map(|v| v.value.clone()).unwrap_or_default()
            }
            VarPart::About => {
                self.workspace().var(&name).map(|v| v.about.clone()).unwrap_or_default()
            }
        };
        self.editing_var = Some((name, part));
        let input = self.var_input.clone();
        input.update(cx, |input, cx| {
            input.set_value(&current, cx);
            input.select_all_now(cx);
        });
        self.var_revision = self.var_input.read(cx).revision;
        let handle = self.var_input.read(cx).focus_handle.clone();
        window.focus(&handle);
        cx.notify();
    }

    /// Reads the variables page's filter box back, as it is typed.
    pub fn sync_var_filter(&mut self, cx: &mut GpuiApp) {
        let (revision, value) = {
            let input = self.var_filter_input.read(cx);
            (input.revision, input.value().to_string())
        };
        if revision != self.var_filter_revision {
            self.var_filter_revision = revision;
            self.var_filter = value;
        }
    }

    pub fn end_var_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_var.take().is_none() {
            return;
        }
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Takes what has been typed into the variable editor, as it is typed.
    pub fn commit_var(&mut self, cx: &mut Context<Self>) {
        let Some((name, part)) = self.editing_var.clone() else { return };
        let (revision, value) = {
            let input = self.var_input.read(cx);
            (input.revision, input.value().to_string())
        };
        if revision == self.var_revision {
            return;
        }
        self.var_revision = revision;
        match part {
            VarPart::Value => {
                // Typing into the value of a formula rewrites the formula;
                // typing into the value of a piece of text rewrites the text.
                let mut var = self.workspace().var(&name).cloned().unwrap_or_default();
                var.value = value;
                var.set_by = None;
                var.at = Some(std::time::SystemTime::now());
                self.workspace_mut().put_var(&name, var);
            }
            VarPart::About => {
                let mut var = self.workspace().var(&name).cloned().unwrap_or_default();
                var.about = value.trim().to_string();
                self.workspace_mut().put_var(&name, var);
            }
            VarPart::Name => {
                let wanted = value.trim().to_string();
                if wanted.is_empty() || wanted == name {
                    return;
                }
                self.workspace_mut().rename_var(&name, &wanted);
                self.editing_var = Some((wanted, VarPart::Name));
            }
        }
        self.save_workspace(self.active);
        cx.notify();
    }

    /// Turns a variable from a piece of text into a formula, or back.
    ///
    /// The same text is kept either way, so a value that was already an
    /// expression starts working the moment it is made one, and a formula
    /// that is made text again leaves its expression behind as the text.
    pub fn toggle_var_formula(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(mut var) = self.workspace().var(name).cloned() else { return };
        var.formula = !var.formula;
        var.set_by = None;
        var.at = Some(std::time::SystemTime::now());
        self.workspace_mut().put_var(name, var);
        self.save_workspace(self.active);
        cx.notify();
    }

    /// What a variable comes to right now: the text, or the answer to the
    /// formula, and what kind of thing that is, so a formula that comes to a
    /// number can be told from one that comes to text. What is wrong with the
    /// formula, when something is, comes back instead.
    ///
    /// A formula that comes to a number and one that comes to text look
    /// identical once they are written out, and the difference is what decides
    /// whether a condition can compare it or a chart can plot it.
    pub fn var_answer(&self, name: &str) -> Result<(String, &'static str), String> {
        let Some(var) = self.workspace().var(name) else { return Ok((String::new(), "text")) };
        if !var.formula {
            return Ok((var.value.clone(), "text"));
        }
        if var.value.trim().is_empty() {
            return Ok((String::new(), "nothing"));
        }
        let data = super::notes::Data::of(self.workspace());
        crate::expr::run(&var.value, &data)
            .map(|value| (value.show(), value.kind()))
            .map_err(|e| e.to_string())
    }

    /// What a formula reads: the other variables and the runs it names.
    ///
    /// The inverse of [`App::var_used_by`], and the other half of being able
    /// to see what a rename or a deletion will break.
    pub fn var_reads(&self, name: &str) -> Vec<String> {
        let ws = self.workspace();
        let Some(var) = ws.var(name).filter(|v| v.formula) else { return Vec::new() };
        let mut out = Vec::new();
        for other in ws.vars.keys() {
            if !other.eq_ignore_ascii_case(name) && mentions(&var.value, other) {
                out.push(other.clone());
            }
        }
        for job in &ws.jobs {
            let title = job.name();
            if mentions(&var.value, &title) && !out.contains(&title) {
                out.push(title);
            }
        }
        out
    }

    /// Copies a variable, name and all, under a name that is not taken.
    pub fn duplicate_var(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(var) = self.workspace().var(name).cloned() else { return };
        let mut copy = format!("{name}_copy");
        let mut n = 2;
        while self.workspace().vars.contains_key(&copy) {
            copy = format!("{name}_copy{n}");
            n += 1;
        }
        self.workspace_mut().put_var(&copy, var);
        self.page = Some(Page::Variables);
        self.save_workspace(self.active);
        self.edit_var(copy, VarPart::Name, window, cx);
    }

    /// Puts text on the clipboard and says so.
    pub fn copy_text(&mut self, what: &str, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.notify(crate::core::Level::Info, format!("{what} copied"), "", None);
        cx.notify();
    }

    /// Where a variable is spoken of: the documents and workflows that name
    /// it, so an unused one is visible as unused and a used one cannot be
    /// renamed in ignorance.
    pub fn var_used_by(&self, name: &str) -> Vec<String> {
        let mut out = Vec::new();
        let ws = self.workspace();
        let data = super::notes::Data::of(ws);
        for doc in &ws.docs {
            if mentions(&doc.source, name) {
                out.push(doc.title(&data));
            }
        }
        for flow in &ws.flows {
            if mentions(&flow.source, name) {
                out.push(flow.title());
            }
        }
        for (other, var) in &ws.vars {
            if var.formula && !other.eq_ignore_ascii_case(name) && mentions(&var.value, name) {
                out.push(other.clone());
            }
        }
        out
    }

    pub fn remove_var(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.editing_var.as_ref().is_some_and(|(n, _)| n == name) {
            self.editing_var = None;
        }
        self.workspace_mut().remove_var(name);
        self.save_workspace(self.active);
        cx.notify();
    }

    /// The interface new tools start on, when one has been chosen.
    pub fn default_iface(&self) -> Option<&str> {
        self.settings.default_for("iface")
    }

    /// Says what a field starts on in every tool that has one, or stops
    /// saying anything about it.
    pub fn set_field_default(&mut self, key: &str, value: Option<&str>, cx: &mut Context<Self>) {
        self.settings.set_default(key, value);
        self.save_settings();
        cx.notify();
    }

    /// Writes the settings file. It is small and written rarely, so it is
    /// written whole every time, not kept in step by hand.
    pub fn save_settings(&self) {
        store::write_state(&self.settings);
        super::widgets::set_motion(self.settings.animate);
    }

    /// Where a row of choices should slide its light from.
    pub fn segment_from(&mut self, id: &str, current: usize) -> usize {
        let slot = self.segment_from.entry(id.to_string()).or_insert((current, current));
        if slot.1 != current {
            *slot = (slot.1, current);
        }
        slot.0
    }

    /// Opens every workspace that was closed, and stops recording them as
    /// closed. Their directories were never touched, so this is only a matter
    /// of reading them again.
    pub fn reopen_closed(&mut self, cx: &mut Context<Self>) {
        let closed = std::mem::take(&mut self.settings.closed);
        self.save_settings();
        for dir in closed {
            let dir = std::path::PathBuf::from(dir);
            if dir.exists() {
                self.open_workspace_dir(&dir, cx);
            }
        }
        cx.notify();
    }

    // --- pages ---------------------------------------------------------------

    /// Goes to a page, or comes back from it when it is the one already
    /// showing. There is no third state and nothing to confirm: a page is
    /// somewhere you are, not something you opened.
    pub fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = (self.page != Some(page)).then_some(page);
        if self.page.is_some() {
            self.end_note_edit();
            window.focus(&self.focus_handle);
        }
        cx.notify();
    }

    /// Goes back to the workspace, from wherever.
    pub fn close_page(&mut self, cx: &mut Context<Self>) {
        if self.page.take().is_some() {
            cx.notify();
        }
    }

    /// Goes wherever the rail was clicked.
    pub fn go(&mut self, dest: Dest, window: &mut Window, cx: &mut Context<Self>) {
        match dest {
            // Asking for a list of what is in the workspace is asking to be
            // back in the workspace.
            Dest::View(view) => self.show_view(view, window, cx),
            Dest::Page(page) => self.show_page(page, window, cx),
        }
    }

    /// Whether the rail should light this one up.
    pub fn showing(&self, dest: Dest) -> bool {
        match dest {
            Dest::View(view) => self.page.is_none() && self.sidebar_open && self.view == view,
            Dest::Page(page) => self.page == Some(page),
        }
    }

    // --- what the application has to say -----------------------------------

    /// Records something worth telling the user, and shows it in the corner.
    ///
    /// Anything that went wrong stays there until it is dismissed; anything
    /// else fades. Everything, either way, stays in the list behind the bell,
    /// so news that was missed can still be found.
    pub fn notify(
        &mut self,
        level: crate::core::Level,
        title: impl Into<String>,
        body: impl Into<String>,
        about: Option<super::notify::About>,
    ) {
        let bad = matches!(level, crate::core::Level::Error);
        let keep_for = if bad { None } else { Some(self.settings.toast_for()) };
        let id = self.notices.push(level, title, body, about, keep_for);
        // With the corner turned off the bell still counts: the setting is
        // about what interrupts you, not about what you are told.
        if !self.settings.toasts && !bad {
            self.notices.dismiss(id);
        }
    }

    pub fn notify_error(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.notify(crate::core::Level::Error, title, body, None);
    }

    /// Takes one out of the corner. It stays in the list.
    pub fn dismiss_notice(&mut self, id: usize, cx: &mut Context<Self>) {
        self.notices.dismiss(id);
        cx.notify();
    }

    /// Opens or closes the list of what has happened. Opening it is reading
    /// it, so the bell stops counting.
    pub fn toggle_notices(&mut self, cx: &mut Context<Self>) {
        self.notices.open = !self.notices.open;
        if self.notices.open {
            self.notices.mark_all_read();
        }
        cx.notify();
    }

    pub fn clear_notices(&mut self, cx: &mut Context<Self>) {
        self.notices.clear();
        self.notices.open = false;
        cx.notify();
    }

    /// Goes to whatever a notice is about.
    pub fn open_notice(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let about = self.notices.get(id).and_then(|n| n.about.clone());
        self.notices.dismiss(id);
        match about {
            Some(super::notify::About::Job(job)) => {
                self.notices.open = false;
                self.reveal_job(job, window, cx);
            }
            Some(super::notify::About::File(path)) => self.reveal_path(&path),
            None => cx.notify(),
        }
    }

    /// Brings a run to the front, wherever it lives.
    pub fn reveal_job(&mut self, id: usize, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.workspaces.iter().position(|w| w.job(id).is_some()) else {
            cx.notify();
            return;
        };
        if at != self.active {
            self.select_workspace(at, cx);
        }
        self.page = None;
        self.select_job(id, cx);
    }

    /// Shows a file where it is, in whatever this system calls its file
    /// manager. ntls does not decide what should open your files.
    pub fn reveal_path(&self, path: &std::path::Path) {
        let (program, args): (&str, Vec<String>) = if cfg!(target_os = "macos") {
            ("open", vec!["-R".into(), path.display().to_string()])
        } else if cfg!(target_os = "windows") {
            ("explorer", vec![format!("/select,{}", path.display())])
        } else {
            // Everything else opens the containing directory, the only thing
            // every desktop agrees on.
            let dir = path.parent().unwrap_or(path);
            ("xdg-open", vec![dir.display().to_string()])
        };
        let _ = std::process::Command::new(program).args(args).spawn();
    }

    /// What a run that has just finished has to say for itself.
    fn notify_finished(&mut self, job_id: usize, state: &State) {
        let Some(job) = self.job_anywhere(job_id) else { return };
        let (name, target, digest) = (job.name(), job.target(), job.digest());
        let elsewhere = self.workspaces.get(self.active).is_none_or(|w| w.job(job_id).is_none());
        // "Here" means the user watched it finish. Another workspace, another
        // tab, or a page in front of it all count as somewhere else.
        let here = !elsewhere && self.page.is_none() && self.workspace().selected == Some(job_id);
        let about = Some(super::notify::About::Job(job_id));

        match state {
            // A failure is always worth saying, wherever it happened.
            State::Failed(e) => {
                self.notify(crate::core::Level::Error, format!("{name} failed"), e.clone(), about);
            }
            _ if !self.settings.notify_runs => {}
            // A run you are watching finish does not need to be told to you.
            _ if here => {}
            State::Done => {
                let detail = [target, digest]
                    .into_iter()
                    .filter(|s| !s.is_empty() && s != "\u{2014}")
                    .collect::<Vec<_>>()
                    .join("  \u{b7}  ");
                self.notify(crate::core::Level::Good, format!("{name} finished"), detail, about);
            }
            State::Stopped => {
                self.notify(crate::core::Level::Info, format!("{name} stopped"), "", about);
            }
            _ => {}
        }
    }

    // --- running ----------------------------------------------------------

    /// Reads the form back out of its editors, so validation and the run see
    /// exactly what is on screen.
    pub fn collect_params(&mut self, cx: &mut GpuiApp) {
        let Some(job) = self.selected_job_mut() else { return };
        let fields = job.tool.fields();
        for (i, field) in fields.iter().enumerate() {
            if let Some(Some(input)) = job.inputs.get(i) {
                let value = input.read(cx).value().to_string();
                job.params.set(field.key, &value);
            }
        }
    }

    /// Validates the visible fields, returning the first complaint.
    fn validate(&self) -> Option<(usize, String)> {
        let job = self.selected_job()?;
        job.tool.fields().iter().enumerate().find_map(|(i, field)| {
            if !field.visible(&job.params) {
                return None;
            }
            field.validate?.check(job.params.raw(field.key)).err().map(|e| (i, e))
        })
    }

    /// Starts over: the previous results go and the job runs again from the top.
    ///
    /// This is the one that says so out loud, so it clears the table even for
    /// a tool set to keep what it finds.
    pub fn restart_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_run(true, true, window, cx);
    }

    /// Picks up where an interrupted run stopped, keeping what it already
    /// found and skipping the targets it already covered.
    pub fn resume_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_run(false, false, window, cx);
    }

    pub fn run_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_run(true, false, window, cx);
    }

    /// `clean` overrides a tool's own switch for keeping what it found, which
    /// only Restart does.
    fn start_run(
        &mut self,
        from_scratch: bool,
        clean: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collect_params(cx);
        if let Some(err) = self.validate() {
            if let Some(job) = self.selected_job_mut() {
                job.field_error = Some(err);
                job.show_form = true;
            }
            cx.notify();
            return;
        }

        // Remember the target and ports for the next tool added with an empty
        // box, before the borrow of the job ends.
        let (target, ports) = {
            let Some(job) = self.selected_job() else { return };
            let target = job
                .tool
                .target_key()
                .map(|k| job.params.str(k))
                .filter(|t| is_carryable(t));
            let ports = job.tool.ports_key().map(|k| job.params.str(k)).filter(|p| !p.is_empty());
            (target, ports)
        };
        if target.is_some() {
            self.last_target = target;
        }
        if ports.is_some() {
            self.last_ports = ports;
        }

        let Some(job) = self.selected_job_mut() else { return };
        if let Some(cancel) = job.cancel.take() {
            cancel.cancel();
        }

        // Three ways to start, and the difference between them is what
        // happens to the table that is already there.
        //
        // Restarting throws it away. Resuming keeps it and tells the tool
        // which targets it need not probe again. Keeping is the tool's own
        // switch, for a scan meant to build a picture over time: it keeps it and
        // probes everything anyway, so what is out there now joins what was
        // out there before and does not replace it.
        let keeping = from_scratch
            && !clean
            && job.tool.keep_key().is_some_and(|key| job.params.bool(key));
        let (done, prior) = if keeping {
            job.keep_rows();
            (std::collections::HashSet::new(), job.rows.clone())
        } else if from_scratch {
            job.reset_output();
            (std::collections::HashSet::new(), Vec::new())
        } else {
            job.field_error = None;
            (job.rows.iter().map(|r| r.target.clone()).collect(), job.rows.clone())
        };
        job.state = State::Running;
        job.started = Some(Instant::now());
        job.finished = None;
        job.show_form = false;

        let filter_input = job.filter_input.clone();
        let cancel = Cancel::new();
        job.cancel = Some(cancel.clone());
        let (tool, params, job_id) = (job.tool.clone(), job.params.clone(), job.id);

        // The filter is about the table, so it survives a run that keeps it.
        if from_scratch && !keeping {
            filter_input.update(cx, |input, cx| input.set_value("", cx));
        }

        let (tx, rx) = async_channel::unbounded::<Msg>();
        let emit_tx = tx.clone();
        let emitter = Emitter::new(move |e: Event| {
            let _ = emit_tx.try_send(Msg::Event(e));
        });

        // Built the long way even when there is nothing to carry: a run that
        // is keeping what earlier runs found is keeping it from the first one,
        // whose table is empty. Taking the empty case as "fresh" told the tool
        // it was not keeping, and the first scan of a record kept over time
        // came out undated and unidentified.
        let run = crate::core::Run { cancel, params, done, prior, keep: keeping };
        runtime().spawn(async move {
            let result = tool.run(run, emitter).await;
            let _ = tx.send(Msg::Finished(result.map_err(|e| e.to_string()))).await;
        });

        let pump = spawn_pump(job_id, rx, cx);
        if let Some(job) = self.selected_job_mut() {
            job.pump = Some(pump);
        }

        // The form is gone, and with it whichever field had the caret. Focus
        // has to come back to the window or the keyboard would be talking to
        // an element that is no longer on screen.
        window.focus(&self.focus_handle);
        self.save_current();
        cx.notify();
    }

    pub fn stop_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_job_mut().map(|j| j.id) else { return };
        self.stop_job(id, cx);
    }

    /// Stops one run, wherever it lives.
    pub fn stop_job(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(job) = self.job_anywhere_mut(id) else { return };
        if let Some(cancel) = &job.cancel {
            cancel.cancel();
        }
        if job.state.is_running() {
            job.state = State::Stopped;
            job.finished = Some(Instant::now());
        }
        cx.notify();
    }

    /// Applies a batch of messages to the job with this id, wherever it lives.
    fn deliver(&mut self, job_id: usize, batch: Vec<Msg>, cx: &mut Context<Self>) {
        // A run that fails has something to say about why, so the panel it
        // says it in opens itself.
        let mut open_panel = false;
        let mut just_finished = None;
        let follow = self.settings.follow_results;
        let Some(job) = self.job_anywhere_mut(job_id) else { return };
        // Read before the rows arrive: it is where the reader was on the frame
        // they are about to change.
        let end = follow.then(|| job.end_in_view()).flatten();
        for msg in batch {
            match msg {
                Msg::Event(e) => job.apply(e),
                Msg::Finished(result) => {
                    job.finished = Some(Instant::now());
                    job.state = match result {
                        Ok(()) if job.cancel.as_ref().is_some_and(Cancel::is_cancelled) => {
                            just_finished = Some(true);
                            State::Stopped
                        }
                        Ok(()) => {
                            just_finished = Some(false);
                            State::Done
                        }
                        Err(e) => {
                            job.apply(Event::err(e.clone()));
                            open_panel = true;
                            just_finished = Some(true);
                            State::Failed(e)
                        }
                    };
                }
            }
        }
        job.follow(end);
        let finished = !job.state.is_running();
        let state = job.state.clone();
        if job.log_follow {
            job.log_scroll.scroll_to_bottom();
        }
        self.panel_open |= open_panel && self.settings.open_panel_on_failure;
        if just_finished.is_some() {
            self.notify_finished(job_id, &state);
        }
        // A finished run is worth keeping; one still going would mean writing
        // the whole table on every batch.
        if finished {
            let (ws_index, id) = (
                self.workspaces.iter().position(|w| w.job(job_id).is_some()),
                job_id,
            );
            if let Some(i) = ws_index {
                self.save_job(i, id);
                self.save_workspace(i);
            }
        }
        cx.notify();

        // A workflow waiting on this run carries on, or stops if it failed.
        if let Some(failed) = just_finished {
            self.finished_for_flow(job_id, failed, cx);
        }
    }

    /// Notes that a run a workflow was waiting on has finished.
    ///
    /// It is not acted on here: starting the next run needs a window, and a
    /// message from a background task does not carry one. The next repaint
    /// picks it up.
    fn finished_for_flow(&mut self, job: usize, failed: bool, _cx: &mut Context<Self>) {
        let waiting = self
            .workspaces
            .iter()
            .any(|w| w.flows.iter().any(|f| f.busy.as_ref().and_then(super::flows::Busy::job) == Some(job)));
        if waiting {
            self.flow_pending = Some((job, failed));
        }
    }

    /// Carries on any workflow whose run has finished. Called from the repaint,
    /// where a window is to hand.
    pub fn resume_flows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((job, failed)) = self.flow_pending.take() else { return };
        self.flow_finished(job, failed, window, cx);
    }

    // --- handoff ----------------------------------------------------------

    /// Sends the selected result to another tool.
    ///
    /// It lands in this workspace, not a new one: chasing a host from a sweep
    /// into a port scan is the same investigation, and keeping them together
    /// is most of the reason a workspace exists.
    pub fn handoff(&mut self, tool_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.selected_job_mut() else { return };
        let Some(row) = job.selected_row() else { return };
        let target = row.target.clone();
        if target.is_empty() {
            return;
        }
        let Some(tool) = self.registry.get(tool_id) else { return };

        // A row that names a port carries it into a port field, when the tool
        // has one.
        let (host, port) = match target.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
                (h.to_string(), Some(p.to_string()))
            }
            _ => (target.clone(), None),
        };
        let carry_port = tool.ports_key().is_some();
        let host = if carry_port { host } else { target };

        self.add_tool(tool, Some(host), port.filter(|_| carry_port), window, cx);
    }

    /// The tools a selected result could be sent to: everything with a target
    /// field, other than the tool the result came from.
    pub fn handoff_targets(&self, from: &str) -> Vec<Arc<dyn Tool>> {
        self.registry
            .all()
            .iter()
            .filter(|t| t.id() != from && t.target_key().is_some())
            .cloned()
            .collect()
    }

    // --- the palette ------------------------------------------------------

    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        let palette = self.palette.clone();
        palette.update(cx, |p, cx| p.open(window, cx));
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        self.palette_open = false;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Tab in the palette: fills in whatever it is pointing at.
    ///
    /// Completing, not choosing, is what makes the command line worth
    /// typing at: the tool's name, then a setting's name, then one of its
    /// values, each one press.
    pub fn complete_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (query, cursor) =
            (self.palette.read(cx).query().to_string(), self.palette.read(cx).cursor);
        let hits = self.hits(&query);
        let Some(filled) = super::palette::complete(&query, cursor, &self.registry, hits) else {
            return;
        };
        let input = self.palette.read(cx).input.clone();
        input.update(cx, |input, cx| input.set_value(&filled, cx));
        self.palette.update(cx, |p, _| p.cursor = 0);
        window.focus(&input.read(cx).focus_handle.clone());
        cx.notify();
    }

    /// Everything matching what is in the palette.
    pub fn hits(&self, query: &str) -> Vec<super::search::Hit> {
        super::search::search(query, &self.registry, &self.workspaces, self.active, &self.ifaces)
    }

    /// What the palette is offering for what is typed in it.
    pub fn suggestion(&self, cx: &GpuiApp) -> super::palette::Suggest {
        let query = self.palette.read(cx).query().to_string();
        let hits = self.hits(&query);
        super::palette::suggest(&query, &self.registry, hits)
    }

    /// Takes whatever the palette is pointing at.
    ///
    /// A tool name with arguments is a command and is run. Anything else is a
    /// place, and the window goes to it.
    pub fn confirm_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.palette.read(cx).cursor;
        let (target, named, run_it) = {
            let command = self.palette.read(cx).command();
            (
                Some(command.target()).filter(|t| !t.is_empty()),
                command
                    .named
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect::<Vec<_>>(),
                command.has_arguments(),
            )
        };

        // A settled tool means the list is showing settings, not places, so
        // the command line is what to act on.
        let chosen = match self.suggestion(cx) {
            super::palette::Suggest::Search(hits) => hits.into_iter().nth(cursor),
            _ => {
                let query = self.palette.read(cx).query().to_string();
                super::palette::tool_of(&query, &self.registry).map(super::search::Hit::Tool)
            }
        };
        self.close_palette(window, cx);

        let Some(hit) = chosen else { return };
        let tool = match hit {
            super::search::Hit::Tool(tool) => tool,
            elsewhere => return self.reveal(elsewhere, window, cx),
        };

        let id = self.add_tool(tool, target, None, window, cx);
        if named.is_empty() && !run_it {
            return;
        }

        // A setting the tool does not have is worth saying so about rather
        // than silently ignoring: a typo in a key would otherwise look like
        // the setting having no effect.
        let unknown = self.apply_settings(id, &named, cx);
        if let Some(job) = self.job_anywhere_mut(id) {
            for key in &unknown {
                job.apply(Event::warn(format!("No setting called \u{201c}{key}\u{201d}")));
            }
        }
        if run_it && unknown.is_empty() {
            self.run_selected(window, cx);
        } else if !unknown.is_empty() {
            self.panel_open = true;
        }
        cx.notify();
    }

    /// Goes to something the search found.
    pub fn reveal(
        &mut self,
        hit: super::search::Hit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use super::search::Hit;
        match hit {
            Hit::Tool(tool) => {
                self.add_tool(tool, None, None, window, cx);
            }
            Hit::Workspace { index, .. } => {
                self.select_workspace(index, cx);
                self.view = View::Workspaces;
            }
            Hit::Job { workspace, job, .. } => {
                self.select_workspace(workspace, cx);
                self.select_job(job, cx);
                self.view = View::Tools;
            }
            Hit::Row { workspace, job, row, .. } => {
                self.select_workspace(workspace, cx);
                self.select_job(job, cx);
                self.view = View::Tools;
                // The row is found by where it sits in the table as it is
                // filtered and sorted now, not where it sat when the
                // search read it.
                if let Some(open) = self.selected_job_mut() {
                    let at = open.view().iter().position(|&raw| raw == row);
                    match at {
                        Some(at) => open.select_index(at),
                        None => {
                            // A filter is hiding it; clearing it is less
                            // surprising than going somewhere and showing
                            // nothing.
                            open.set_filter(String::new());
                            let at = open.view().iter().position(|&raw| raw == row);
                            if let Some(at) = at {
                                open.select_index(at);
                            }
                        }
                    }
                }
            }
            Hit::Doc { workspace, doc, .. } => {
                self.select_workspace(workspace, cx);
                self.select_doc(doc, cx);
                self.view = View::Tools;
            }
            Hit::Flow { workspace, flow, .. } => {
                self.select_workspace(workspace, cx);
                self.select_flow(flow, cx);
                self.view = View::Tools;
            }
            Hit::Iface { name, .. } => {
                self.page = Some(Page::Interfaces);
                let _ = name;
            }
        }
        cx.notify();
    }

    /// Writes `key=value` settings into a job's form, returning the keys the
    /// tool does not have.
    fn apply_settings(
        &mut self,
        id: usize,
        settings: &[(String, String)],
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let Some(job) = self.job_anywhere_mut(id) else { return Vec::new() };
        let fields = job.tool.fields();
        let mut unknown = Vec::new();

        for (key, value) in settings {
            // A field can be named by its key or by its label, since one is
            // what the tool calls it and the other is what the screen does.
            let found = fields.iter().position(|f| {
                f.key.eq_ignore_ascii_case(key)
                    || f.label.eq_ignore_ascii_case(key)
                    || f.label.replace(' ', "-").eq_ignore_ascii_case(key)
            });
            let Some(at) = found else {
                unknown.push(key.clone());
                continue;
            };
            job.params.set(fields[at].key, value);
            if let Some(Some(input)) = job.inputs.get(at).cloned() {
                input.update(cx, |input, cx| input.set_value(value, cx));
            }
        }
        unknown
    }

    // --- fields -----------------------------------------------------------

    /// Cycles a select or toggles a switch.
    pub fn cycle_field(&mut self, field_index: usize, delta: isize, cx: &mut Context<Self>) {
        let Some(job) = self.selected_job_mut() else { return };
        let fields = job.tool.fields();
        let Some(field) = fields.get(field_index) else { return };

        match field.kind {
            FieldKind::Bool => {
                let now = job.params.bool(field.key);
                job.params.set(field.key, if now { "false" } else { "true" });
            }
            FieldKind::Select => {
                if field.options.is_empty() {
                    return;
                }
                let current = job.params.str(field.key);
                let at = field.options.iter().position(|o| o.value == current).unwrap_or(0);
                let n = field.options.len() as isize;
                let next = (at as isize + delta).rem_euclid(n) as usize;
                job.params.set(field.key, &field.options[next].value);
            }
            FieldKind::Text => {}
        }
        job.field_error = None;
        cx.notify();
    }

    pub fn set_field(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        if let Some(job) = self.selected_job_mut() {
            job.params.set(key, value);
            job.field_error = None;
            cx.notify();
        }
    }

    /// Resolves shorthand in the focused field, one step at a time, so the
    /// user can see exactly what is about to happen and edit it before it
    /// does. When there is nothing left to expand, focus moves on.
    pub fn expand_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.collect_params(cx);

        let Some(job) = self.selected_job() else { return };
        let fields = job.tool.fields();
        let focused = fields.iter().enumerate().find(|(i, _)| {
            job.inputs
                .get(*i)
                .and_then(Option::as_ref)
                .is_some_and(|e| e.read(cx).focus_handle.is_focused(window))
        });

        if let Some((i, field)) = focused
            && let Some(expand) = field.expand
        {
            let value = job.params.raw(field.key).to_string();
            if let Some(next) = expand.apply(&value, &job.params) {
                let input = job.inputs.get(i).and_then(Option::as_ref).cloned();
                let key = field.key;
                if let Some(input) = input {
                    input.update(cx, |input, cx| input.set_value(&next, cx));
                }
                if let Some(job) = self.selected_job_mut() {
                    job.params.set(key, &next);
                    job.field_error = None;
                }
                cx.notify();
                return;
            }
        }

        self.focus_next_field(window, cx);
    }

    /// Moves focus to the next visible text field, wrapping around.
    pub fn focus_next_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.selected_job() else { return };
        if !job.show_form {
            return;
        }
        let fields = job.tool.fields();
        let editable: Vec<usize> = fields
            .iter()
            .enumerate()
            .filter(|(i, f)| {
                f.visible(&job.params) && job.inputs.get(*i).is_some_and(Option::is_some)
            })
            .map(|(i, _)| i)
            .collect();
        if editable.is_empty() {
            return;
        }

        let current = editable.iter().position(|i| {
            job.inputs[*i].as_ref().is_some_and(|e| e.read(cx).focus_handle.is_focused(window))
        });
        let next = editable[current.map_or(0, |p| (p + 1) % editable.len())];

        if let Some(Some(input)) = job.inputs.get(next) {
            let handle = input.read(cx).focus_handle.clone();
            window.focus(&handle);
            input.update(cx, |input, cx| input.select_all_now(cx));
        }
    }

    /// Puts the caret in the first field of an open form, and back in the
    /// window when there is no form to type into.
    pub fn focus_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.selected_job().is_some_and(|j| j.show_form);
        if !open {
            window.focus(&self.focus_handle);
            cx.notify();
            return;
        }
        let Some(job) = self.selected_job() else { return };
        let fields = job.tool.fields();
        let first = fields.iter().enumerate().find(|(i, f)| {
            f.visible(&job.params) && job.inputs.get(*i).is_some_and(Option::is_some)
        });
        if let Some((i, _)) = first
            && let Some(Some(input)) = job.inputs.get(i)
        {
            let handle = input.read(cx).focus_handle.clone();
            window.focus(&handle);
            input.update(cx, |input, cx| input.select_all_now(cx));
        }
    }

    /// Restores every field to the tool's own default.
    pub fn reset_fields(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.selected_job_mut() else { return };
        let fields = job.tool.fields();
        job.params = Params::defaults(&fields);
        job.field_error = None;

        let editors: Vec<(Option<Entity<TextInput>>, String)> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| (job.inputs.get(i).and_then(Option::as_ref).cloned(), f.default.clone()))
            .collect();
        for (input, default) in editors {
            if let Some(input) = input {
                input.update(cx, |input, cx| input.set_value(&default, cx));
            }
        }
        cx.notify();
    }

    /// Picks up a filter typed into the results box.
    pub fn sync_filter(&mut self, cx: &mut GpuiApp) {
        let Some(job) = self.selected_job_mut() else { return };
        let (revision, value) = {
            let input = job.filter_input.read(cx);
            (input.revision, input.value().to_string())
        };
        if revision != job.filter_revision {
            job.filter_revision = revision;
            let changed = job.filter != value;
            job.set_filter(value);
            if changed {
                self.end_note_edit();
            }
        }
    }

    /// Opens the run's name for editing, in place.
    pub fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.selected_job() else { return };
        let current = job.title.clone().unwrap_or_else(|| job.tool.title().to_string());
        self.renaming = true;
        self.rename_for = self.workspace().selected;

        let input = self.rename_input.clone();
        input.update(cx, |input, cx| {
            input.set_value(&current, cx);
            input.select_all_now(cx);
        });
        self.rename_revision = self.rename_input.read(cx).revision;
        let handle = self.rename_input.read(cx).focus_handle.clone();
        window.focus(&handle);
        cx.notify();
    }

    /// Opens a document's or a workflow's name for editing, in its pane.
    pub fn begin_item_rename(
        &mut self,
        item: super::workspace::Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use super::workspace::Item;
        let current = match item {
            Item::Doc(id) => {
                let data = super::notes::Data::of(self.workspace());
                self.workspace().doc(id).map(|d| d.title(&data))
            }
            Item::Flow(id) => self.workspace().flow(id).map(|f| f.title()),
            Item::Tool(_) => None,
        };
        let Some(current) = current else { return };

        self.renaming = false;
        self.renaming_item = Some(item);
        let input = self.rename_input.clone();
        input.update(cx, |input, cx| {
            input.set_value(&current, cx);
            input.select_all_now(cx);
        });
        self.item_rename_revision = self.rename_input.read(cx).revision;
        let handle = self.rename_input.read(cx).focus_handle.clone();
        window.focus(&handle);
        cx.notify();
    }

    pub fn end_item_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming_item.take().is_none() {
            return;
        }
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Takes what has been typed into a document's or workflow's name box.
    pub fn commit_item_rename(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.renaming_item else { return };
        let (revision, value) = {
            let input = self.rename_input.read(cx);
            (input.revision, input.value().to_string())
        };
        if revision == self.item_rename_revision {
            return;
        }
        self.item_rename_revision = revision;
        self.rename_item(item, &value, cx);
    }

    pub fn end_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = false;
        self.rename_for = None;
        window.focus(&self.focus_handle);
        self.save_current();
        cx.notify();
    }

    /// Opens the workspace's name for editing, in the rail.
    pub fn begin_workspace_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.workspace().name();
        self.renaming_workspace = true;
        self.ws_rename_for = Some(self.workspace().id);

        let input = self.ws_rename_input.clone();
        input.update(cx, |input, cx| {
            input.set_value(&current, cx);
            input.select_all_now(cx);
        });
        self.ws_rename_revision = self.ws_rename_input.read(cx).revision;
        let handle = self.ws_rename_input.read(cx).focus_handle.clone();
        window.focus(&handle);
        cx.notify();
    }

    pub fn end_workspace_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming_workspace = false;
        self.ws_rename_for = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Points the workspace naming box at the active workspace, and reads it
    /// back when it changes. Renaming moves the directory with it.
    pub fn sync_workspace_rename(&mut self, cx: &mut GpuiApp) -> Option<String> {
        if !self.renaming_workspace {
            return None;
        }
        let (revision, value) = {
            let input = self.ws_rename_input.read(cx);
            (input.revision, input.value().to_string())
        };
        if revision == self.ws_rename_revision {
            return None;
        }
        self.ws_rename_revision = revision;
        Some(value)
    }

    /// Reads the naming box back, when it has changed.
    pub fn commit_rename(&mut self, cx: &mut GpuiApp) -> bool {
        if !self.renaming {
            return false;
        }
        let (revision, value) = {
            let input = self.rename_input.read(cx);
            (input.revision, input.value().to_string())
        };
        if revision == self.rename_revision || self.rename_for != self.workspace().selected {
            return false;
        }
        self.rename_revision = revision;
        let name = value.trim().to_string();
        if let Some(job) = self.selected_job_mut() {
            job.title = (!name.is_empty()).then_some(name);
        }
        true
    }

    /// Points the note box at whichever result row is selected, and reads it back.
    ///
    /// What is being typed into is a line on screen, and the line on screen is
    /// not the row in the table: sorting and filtering put them in a different
    /// order, so the note has to be written through the view and not
    /// straight into `rows`. Otherwise a remark about the host you are looking
    /// at lands on whichever host happens to occupy that position.
    pub fn sync_note(&mut self, cx: &mut GpuiApp) -> bool {
        if self.editing_note.is_none() {
            return false;
        }
        let Some((at, identity)) = self.editing_note_row.clone() else { return false };
        let (revision, value) = {
            let input = self.note_input.read(cx);
            (input.revision, input.value().to_string())
        };
        if revision == self.note_revision {
            return false;
        }
        self.note_revision = revision;
        let note = (!value.trim().is_empty()).then(|| value.trim().to_string());
        let Some(job) = self.selected_job_mut() else { return false };
        let Some(idx) = locate_row(&job.rows, at, &identity) else { return false };
        job.rows[idx].note = note;
        let job_id = job.id;
        self.editing_note_row = Some((idx, identity));
        self.save_job(self.active, job_id);
        true
    }

    /// Starts dragging a column edge.
    pub fn begin_resize(&mut self, column: usize, at_x: f32, width: f32) {
        self.resizing = Some(Resize { column: Some(column), from_x: at_x, from_width: width });
    }

    /// Starts dragging the side bar's edge. It is the same gesture as a column
    /// drag and the same drag state, so the window only has to watch the
    /// pointer in one place.
    pub fn begin_sidebar_resize(&mut self, at_x: f32) {
        self.resizing = Some(Resize { column: None, from_x: at_x, from_width: self.sidebar_width });
    }

    /// Follows the pointer while an edge is being dragged.
    pub fn drag_resize(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(resize) = self.resizing else { return };
        let travelled = x - resize.from_x;
        let Some(column) = resize.column else {
            self.sidebar_width = (resize.from_width + travelled).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
            self.sidebar_open = true;
            cx.notify();
            return;
        };
        let wanted = (resize.from_width + travelled).clamp(36., 900.);
        if let Some(job) = self.selected_job_mut()
            && let Some(slot) = job.column_override.get_mut(column)
        {
            *slot = Some(wanted);
            cx.notify();
        }
    }

    pub fn end_resize(&mut self) {
        if self.resizing.take().is_some_and(|r| r.column.is_none()) {
            self.remember_layout();
        }
    }

    /// Puts a column back to sizing itself from what is in it.
    pub fn reset_column(&mut self, column: usize, cx: &mut Context<Self>) {
        if let Some(job) = self.selected_job_mut()
            && let Some(slot) = job.column_override.get_mut(column)
        {
            *slot = None;
            cx.notify();
        }
    }

    /// Shows a side bar view, or hides the side bar when its own icon is
    /// clicked again, the way an activity bar behaves everywhere else.
    pub fn show_view(&mut self, view: View, _window: &mut Window, cx: &mut Context<Self>) {
        // Coming back from a page always lands on a side bar, whichever icon
        // was clicked: hiding it would look like the click did nothing.
        let from_page = self.page.take().is_some();
        if !from_page && self.view == view && self.sidebar_open {
            self.sidebar_open = false;
        } else {
            self.view = view;
            self.sidebar_open = true;
        }
        cx.notify();
    }

    /// Re-reads the machine's interfaces. They are polled anyway, but a
    /// cable just plugged in is worth not waiting for.
    /// Opens an IP scan pointed at a network, on the interface it belongs to.
    ///
    /// Reading which networks this machine is on and sweeping one of them are
    /// the same errand, so the answer carries the button.
    pub fn scan_network(
        &mut self,
        network: &str,
        iface: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tool) = self.registry.get("ipscan") else { return };
        self.page = None;
        let id = self.add_tool(tool, Some(network.to_string()), None, window, cx);
        if let Some(job) = self.workspace_mut().job_mut(id) {
            job.params.set("iface", iface);
            if let Some(Some(input)) = job.inputs.iter().enumerate().find_map(|(i, slot)| {
                job.tool.fields().get(i).filter(|f| f.key == "iface").map(|_| slot.clone())
            }) {
                input.update(cx, |input, cx| input.set_value(iface, cx));
            }
        }
        self.save_current();
        cx.notify();
    }

    pub fn refresh_interfaces(&mut self, cx: &mut Context<Self>) {
        iface::forget_snapshot();
        self.ifaces = iface::interfaces();
        cx.notify();
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        // A page has no side bar. Asking for one is asking to be back where
        // there is one, instead of asking for nothing to happen.
        let want = if self.page.take().is_some() { true } else { !self.sidebar_open };
        if want != self.sidebar_open {
            self.sidebar_open = want;
            self.sidebar_moved = Some((std::time::Instant::now(), want));
        }
        self.remember_layout();
        cx.notify();
    }

    /// How wide the side bar is being drawn right now: its full width, none of
    /// it, or somewhere in between while it is on the move.
    pub fn sidebar_at(&self) -> f32 {
        let full = self.sidebar_width;
        let shut = if self.sidebar_open { full } else { 0. };
        if !super::widgets::motion_on() {
            return shut;
        }
        let Some((since, opening)) = self.sidebar_moved else { return shut };
        let d = super::widgets::motion::SETTLE.as_secs_f32();
        let t = (since.elapsed().as_secs_f32() / d).clamp(0., 1.);
        // The same easing the animated elements use, worked out here because
        // this one drives a layout instead of one element's own style.
        let eased = if t < 0.5 { 2. * t * t } else { 1. - (-2. * t + 2.).powi(2) / 2. };
        let fraction = if opening { eased } else { 1. - eased };
        full * fraction
    }

    /// Whether the side bar is still on the move, and should be repainted.
    pub fn sidebar_moving(&self) -> bool {
        self.sidebar_moved.is_some_and(|(since, _)| {
            since.elapsed() < super::widgets::motion::SETTLE
                && super::widgets::motion_on()
        })
    }

    pub fn toggle_panel(&mut self, cx: &mut Context<Self>) {
        self.panel_open = !self.panel_open;
        cx.notify();
    }

    // --- the context menu -------------------------------------------------

    /// Opens a menu at a point. Opening one closes whatever was open, since
    /// there is only ever one.
    pub fn open_menu(
        &mut self,
        at: gpui::Point<gpui::Pixels>,
        items: Vec<super::menu::Item>,
        cx: &mut Context<Self>,
    ) {
        self.clearing = None;
        if items.is_empty() {
            return;
        }
        self.menu = Some(Menu::new(at, items));
        cx.notify();
    }

    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    /// Carries out whatever was chosen, and closes the menu.
    pub fn choose(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        match act {
            Act::RenameWorkspace(i) => {
                self.select_workspace(i, cx);
                self.begin_workspace_rename(window, cx);
            }
            Act::TagWorkspace(i, tag) => self.tag_workspace(i, tag, cx),
            Act::CloseWorkspace(i) => self.close_workspace(i, cx),
            Act::DeleteWorkspace(i) => self.delete_workspace(i, window, cx),
            Act::RevealWorkspace(i) => {
                if let Some(ws) = self.workspaces.get(i) {
                    cx.reveal_path(&ws.dir);
                }
            }
            Act::NewWorkspace => self.new_workspace(cx),
            Act::OpenFolder => self.open_folder(cx),

            Act::RunJob(id) => {
                self.select_job(id, cx);
                self.run_selected(window, cx);
            }
            Act::StopJob(id) => {
                self.select_job(id, cx);
                self.stop_selected(cx);
            }
            Act::RenameJob(id) => {
                self.select_job(id, cx);
                self.begin_rename(window, cx);
            }
            Act::TagJob(id, tag) => {
                self.select_job(id, cx);
                self.tag_job(tag, cx);
            }
            Act::FavouriteJob(id) => {
                self.select_job(id, cx);
                self.toggle_favorite(cx);
            }
            Act::DuplicateJob(id) => {
                self.duplicate_job(id, window, cx);
            }
            Act::DuplicateRunJob(id) => self.duplicate_and_run_job(id, window, cx),
            Act::ExportJob(id) => self.export_job(id, cx),
            Act::AskCompare(id) => self.ask_compare(id, window, cx),
            Act::StopComparing(id) => self.compare_job(id, None, cx),
            Act::AskMove(id) => self.ask_move(id, window, cx),
            Act::AskMoveDoc(id) => self.ask_move_doc(id, window, cx),
            Act::AskMoveFlow(id) => self.ask_move_flow(id, window, cx),
            Act::MoveJobUp(id) => self.move_job_by(id, -1, cx),
            Act::MoveJobDown(id) => self.move_job_by(id, 1, cx),
            Act::MoveDocUp(id) => self.move_doc_by(id, -1, cx),
            Act::MoveDocDown(id) => self.move_doc_by(id, 1, cx),
            Act::MoveFlowUp(id) => self.move_flow_by(id, -1, cx),
            Act::MoveFlowDown(id) => self.move_flow_by(id, 1, cx),
            Act::AskCreateFolder => self.ask_create_folder(window, cx),
            Act::RevealPath(path) => cx.reveal_path(std::path::Path::new(&path)),
            Act::RevealFolder(name) => {
                let dir = self.workspace().dir.join(store::slugify(&name));
                cx.reveal_path(&dir);
            }
            Act::RevealJob(id) => {
                let path = self.workspaces.iter().find_map(|w| {
                    w.job(id).map(|j| {
                        super::store::job_path_in(&w.dir, j.folder.as_deref(), &j.stem)
                    })
                });
                if let Some(path) = path {
                    cx.reveal_path(&path);
                }
            }
            Act::CloseTab(id) => self.close_tab(id, cx),
            Act::CloseJob(id) => self.close_job(id, cx),
            Act::CloseGroup(group, folder) => self.close_group_now(group, folder, cx),

            Act::OpenFlow(id) => self.select_flow(id, cx),
            Act::RunFlow(id) => self.start_flow(id, window, cx),
            Act::StopFlow(id) => self.stop_flow(id, cx),
            Act::EditFlow(id) => self.edit_flow(id, cx),
            Act::CloseFlowTab(id) => self.close_flow_tab(id, cx),

            Act::ChangeStep(flow, spot, change) => self.change_step(flow, &spot, &change, cx),
            Act::AddStepAfter(flow, spot, shape) => {
                self.change_step(flow, &spot, &Change::Add(shape), cx)
            }
            Act::AddStepInside(flow, spot, block, shape) => {
                self.change_step(flow, &spot, &Change::AddInside(shape, block), cx)
            }
            Act::AddStepAtEnd(flow, shape) => self.add_step(flow, shape, cx),
            Act::AskStepRun(flow, spot) => self.ask_step_run(flow, spot, window, cx),

            Act::NewDoc => self.new_document(window, cx),
            Act::NewFlow => self.new_workflow(window, cx),

            Act::OpenDoc(id) => self.select_doc(id, cx),
            Act::EditDoc(id) => self.edit_doc(id, cx),
            Act::CloseDocTab(id) => self.close_doc_tab(id, cx),
            Act::DeleteDoc(id) => self.delete_doc(id, cx),
            Act::RenameDoc(id) => {
                self.select_doc(id, cx);
                self.begin_item_rename(super::workspace::Item::Doc(id), window, cx);
            }
            Act::TagDoc(id, tag) => self.tag_item(super::workspace::Item::Doc(id), tag, cx),
            Act::FavouriteDoc(id) => self.favourite_item(super::workspace::Item::Doc(id), cx),
            Act::RenameFlow(id) => {
                self.select_flow(id, cx);
                self.begin_item_rename(super::workspace::Item::Flow(id), window, cx);
            }
            Act::TagFlow(id, tag) => self.tag_item(super::workspace::Item::Flow(id), tag, cx),
            Act::FavouriteFlow(id) => self.favourite_item(super::workspace::Item::Flow(id), cx),
            Act::DeleteFlow(id) => self.delete_flow(id, cx),
            Act::RevealFlow(id) => {
                if let Some(flow) = self.workspace().flow(id) {
                    cx.reveal_path(&flow.path.clone());
                }
            }
            Act::RevealDoc(id) => {
                if let Some(doc) = self.workspace().doc(id) {
                    cx.reveal_path(&doc.path.clone());
                }
            }

            Act::CopyText(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
            Act::SendTo(tool) => self.handoff(tool, window, cx),

            Act::ToggleSidebar => self.toggle_sidebar(cx),
            Act::TogglePanel => self.toggle_panel(cx),
            Act::ToggleTheme => self.toggle_theme(cx),
            Act::OpenPalette => self.open_palette(window, cx),
        }
        cx.notify();
    }

    // --- opening things ----------------------------------------------------

    /// Asks for a folder and opens it as a workspace.
    ///
    /// A workspace is a directory of tool files, so any directory can be one:
    /// an empty folder becomes a new workspace that saves itself there, and a
    /// folder ntls wrote earlier comes back with everything in it.
    pub fn open_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |app, cx| {
            let Ok(Ok(Some(chosen))) = paths.await else { return };
            app.update(cx, |app: &mut App, cx| {
                for dir in chosen {
                    app.open_workspace_dir(&dir, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Opens one directory as a workspace, selecting it if it is already open.
    pub fn open_workspace_dir(&mut self, dir: &std::path::Path, cx: &mut Context<Self>) -> usize {
        let dir = dir.to_path_buf();
        if let Some(at) = self.workspaces.iter().position(|w| w.dir == dir) {
            self.select_workspace(at, cx);
            return at;
        }

        // Opening a folder is the way back in for one that was closed.
        let key = dir.display().to_string();
        if self.settings.closed.contains(&key) {
            self.settings.closed.retain(|c| *c != key);
            self.save_settings();
        }

        let mut ws = Workspace::new(self.next_ws_id, dir.clone());
        self.next_ws_id += 1;
        if let Some(record) = store::read_workspace(&dir) {
            ws.restore(&record);
        } else {
            // A folder that ntls did not write is still a fine place to work;
            // it takes its name from the folder.
            ws.name_override = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty());
        }
        for (folder, stem, record) in store::load_jobs(&dir) {
            if let Some(mut job) = self.restore_job(&stem, &record, cx) {
                job.folder = folder;
                ws.observe_stem(&stem);
                ws.jobs.push(job);
            }
        }
        ws.docs = super::notes::load(&dir, &mut self.next_job_id);
        ws.flows = super::flows::load(&dir, &mut self.next_job_id);
        ws.selected = ws.jobs.first().map(|j| j.id);

        self.workspaces.push(ws);
        self.active = self.workspaces.len() - 1;
        self.view = View::Tools;
        self.save_workspace(self.active);
        cx.notify();
        self.active
    }

    /// Opens whatever was dropped on the window: a folder as a workspace, a
    /// tool file into the current one.
    pub fn open_paths(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        for path in paths {
            if path.is_dir() {
                self.open_workspace_dir(path, cx);
            } else {
                self.open_job_file(path, cx);
            }
        }
    }

    /// Reads one saved tool file into the current workspace.
    pub fn open_job_file(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(record) = store::read_job(path) else { return };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "imported".into());
        let Some(job) = self.restore_job(&stem, &record, cx) else { return };

        // It is a copy, not the original: it gets a file of its own in this
        // workspace, not back to wherever it was dragged from.
        let id = job.id;
        let stem = self.workspace_mut().next_stem(&record.tool);
        let mut job = job;
        job.stem = stem;
        self.workspace_mut().add(job);
        self.select_job(id, cx);
        self.save_current();
        self.save_workspace(self.active);
        cx.notify();
    }

    /// Copies a tool, its settings and its results into a second entry.
    ///
    /// Returns the copy, so the caller can start it straight away.
    pub fn duplicate_job(
        &mut self,
        id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let source = self.workspaces[self.active].job(id)?;
        let record = source.record();
        let stem = self.workspace_mut().next_stem(&record.tool);
        let mut job = self.restore_job(&stem, &record, cx)?;
        job.title = Some(match &record.title {
            Some(t) => format!("{t} copy"),
            None => format!("{} copy", job.tool.title()),
        });
        let new_id = job.id;
        self.workspace_mut().add(job);
        self.select_job(new_id, cx);
        self.save_current();
        self.save_workspace(self.active);
        let _ = window;
        cx.notify();
        Some(new_id)
    }

    /// Copies a tool and starts the copy at once.
    ///
    /// Running a scan again while keeping the one you already have is the
    /// common case (the settings are right, the results are worth keeping)
    /// and doing it in one go is what stops the second half being forgotten.
    pub fn duplicate_and_run_job(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(copy) = self.duplicate_job(id, window, cx) else { return };
        // The copy carries the original's results; the run it is about to do
        // is its own, so it starts from an empty table.
        if let Some(job) = self.workspace_mut().job_mut(copy) {
            job.reset_output();
            job.state = State::Setup;
        }
        self.run_selected(window, cx);
    }

    /// Writes a set of results out as CSV, wherever the user says.
    ///
    /// What is written is what is on screen: the tool's own columns, the rows
    /// in the order the table is sorted and filtered into, and the notes
    /// attached to them.
    pub fn export_job(&mut self, id: usize, cx: &mut Context<Self>) {
        let notes = self.workspace().notes.clone();
        let Some(job) = self.workspace_mut().job_mut(id) else { return };
        let (columns, tool, title) = (job.tool.columns(), job.tool.id(), job.name());
        let view: Vec<usize> = job.view().to_vec();
        let rows: Vec<crate::core::Row> =
            view.iter().filter_map(|i| job.rows.get(*i).cloned()).collect();
        if rows.is_empty() {
            return;
        }

        let text = super::export::csv(&columns, &rows.iter().collect::<Vec<_>>(), &notes);
        let suggested = super::export::suggested_name(tool, &title);
        let directory = crate::sys::downloads();

        let chosen = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn(async move |app, cx| {
            let Ok(Ok(Some(path))) = chosen.await else { return };
            let wrote = std::fs::write(&path, text);
            app.update(cx, |app: &mut App, cx| {
                match wrote {
                    Ok(()) => {
                        if let Some(job) = app.job_anywhere_mut(id) {
                            job.apply(Event::good(format!("Exported to {}", path.display())));
                        }
                    }
                    Err(e) => {
                        if let Some(job) = app.job_anywhere_mut(id) {
                            job.apply(Event::err(format!("Cannot write {}: {e}", path.display())));
                        }
                        app.panel_open = true;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // --- documents ---------------------------------------------------------

    /// Adds a document to this workspace and opens it here to be written.
    ///
    /// It is a file like any other and something else can open it, but not
    /// unasked: making a document puts the caret in it in this window, which
    /// is where it was made.
    pub fn new_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (dir, name) = (self.workspace().dir.clone(), self.workspace().name());
        let path = match super::notes::create(&dir, &name) {
            Ok(path) => path,
            Err(e) => {
                self.notify_error("Cannot make a document", format!("{}: {e}", dir.display()));
                cx.notify();
                return;
            }
        };

        let mut id = self.next_job_id;
        let docs = super::notes::load(&dir, &mut id);
        self.next_job_id = id;
        let opened = docs.iter().find(|d| d.path == path).map(|d| d.id);
        self.workspace_mut().docs = docs;
        if let Some(id) = opened {
            self.workspace_mut().select_doc(id);
            // A new document is empty, so there is nothing to read and every
            // reason to open the editor on it.
            self.toggle_editing(super::workspace::Item::Doc(id), window, cx);
        }
        cx.notify();
    }

    /// Shows a document.
    pub fn select_doc(&mut self, id: usize, cx: &mut Context<Self>) {
        self.clearing = None;
        if let Some(doc) = self.workspace_mut().doc_mut(id)
            && doc.stale()
        {
            doc.reload();
        }
        self.workspace_mut().select_doc(id);
        cx.notify();
    }

    pub fn close_doc_tab(&mut self, id: usize, cx: &mut Context<Self>) {
        self.workspace_mut().close_doc_tab(id);
        cx.notify();
    }

    /// Hands a document to whatever else the system writes markdown with,
    /// only ever done when it is asked for by name.
    pub fn edit_doc(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(doc) = self.workspace().doc(id) {
            cx.open_with_system(&doc.path);
        }
    }

    /// The colour and star of a document or a workflow.
    pub fn mark_of(&self, item: super::workspace::Item) -> (Tag, bool) {
        let mark = self.workspace().mark(item);
        (mark.tag, mark.favorite)
    }

    pub fn tag_item(&mut self, item: super::workspace::Item, tag: Tag, cx: &mut Context<Self>) {
        let mut mark = self.workspace().mark(item);
        mark.tag = tag;
        self.workspace_mut().set_mark(item, mark);
        self.save_workspace(self.active);
        cx.notify();
    }

    pub fn favourite_item(&mut self, item: super::workspace::Item, cx: &mut Context<Self>) {
        let mut mark = self.workspace().mark(item);
        mark.favorite = !mark.favorite;
        self.workspace_mut().set_mark(item, mark);
        self.save_workspace(self.active);
        cx.notify();
    }

    /// Renames a document or a workflow.
    ///
    /// What both are called is the first line of the file: a document's
    /// heading, a workflow's opening comment. That line is what a rename
    /// writes. The file keeps its name on disk, so nothing that points at it
    /// stops pointing at it.
    pub fn rename_item(
        &mut self,
        item: super::workspace::Item,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        use super::workspace::Item;
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        match item {
            Item::Doc(id) => {
                let Some(doc) = self.workspace().doc(id) else { return };
                let source = Self::retitled(&doc.source, name);
                let path = doc.path.clone();
                if std::fs::write(&path, &source).is_ok()
                    && let Some(doc) = self.workspace_mut().doc_mut(id)
                {
                    doc.source = source;
                    doc.seen = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                }
            }
            Item::Flow(id) => {
                let Some(sheet) = self.workspace().flow(id) else { return };
                let source = Self::retitled(&sheet.source, name);
                let path = sheet.path.clone();
                if std::fs::write(&path, &source).is_ok()
                    && let Some(sheet) = self.workspace_mut().flow_mut(id)
                {
                    sheet.source = source;
                    sheet.seen = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());
                }
                if let Some(editor) = self.editor_for(item).cloned() {
                    let text = self.workspace().flow(id).map(|f| f.source.clone());
                    if let Some(text) = text {
                        editor.update(cx, |editor, cx| editor.set_text(&text, cx));
                    }
                }
            }
            Item::Tool(_) => {}
        }
        cx.notify();
    }

    /// Rewrites the line a file is named by, the first heading, or writes
    /// one where there is none.
    fn retitled(source: &str, name: &str) -> String {
        let mut out = String::new();
        let mut named = false;
        for line in source.lines() {
            if !named && line.trim_start().starts_with("# ") {
                out.push_str("# ");
                out.push_str(name);
                out.push('\n');
                named = true;
                continue;
            }
            // Something that is not a heading, before any heading: the file
            // has none, and one goes in front of it.
            if !named && !line.trim().is_empty() {
                out.push_str("# ");
                out.push_str(name);
                out.push_str("\n\n");
                named = true;
            }
            out.push_str(line);
            out.push('\n');
        }
        if !named {
            out.push_str("# ");
            out.push_str(name);
            out.push('\n');
        }
        out
    }

    /// Takes a document out of the workspace.
    ///
    /// The same rule as a tool: the file is moved into the workspace's own
    /// `.closed` folder, not destroyed.
    pub fn delete_doc(&mut self, id: usize, cx: &mut Context<Self>) {
        let dir = self.workspace().dir.clone();
        let Some(path) = self.workspace().doc(id).map(|d| d.path.clone()) else { return };
        if self.editor_for(super::workspace::Item::Doc(id)).is_some() {
            self.stop_editing(cx);
        }
        store::remove_file(&path, &dir);
        self.workspace_mut().close_doc_tab(id);
        let ws = self.workspace_mut();
        ws.docs.retain(|d| d.id != id);
        if ws.selected_doc == Some(id) {
            ws.selected_doc = None;
        }
        self.save_workspace(self.active);
        cx.notify();
    }

    // --- workflows ---------------------------------------------------------

    /// Adds a workflow to this workspace and shows it, ready to have its
    /// first step added.
    pub fn new_workflow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (dir, name) = (self.workspace().dir.clone(), self.workspace().name());
        let path = match super::flows::create(&dir, &name) {
            Ok(path) => path,
            Err(e) => {
                self.notify_error("Cannot make a workflow", format!("{}: {e}", dir.display()));
                cx.notify();
                return;
            }
        };

        let mut id = self.next_job_id;
        let flows = super::flows::load(&dir, &mut id);
        self.next_job_id = id;
        let opened = flows.iter().find(|f| f.path == path).map(|f| f.id);
        self.workspace_mut().flows = flows;
        if let Some(id) = opened {
            self.workspace_mut().select_flow(id);
        }
        let _ = window;
        cx.notify();
    }

    pub fn select_flow(&mut self, id: usize, cx: &mut Context<Self>) {
        self.clearing = None;
        if let Some(flow) = self.workspace_mut().flow_mut(id)
            && flow.stale()
        {
            flow.reload();
        }
        self.workspace_mut().select_flow(id);
        cx.notify();
    }

    pub fn close_flow_tab(&mut self, id: usize, cx: &mut Context<Self>) {
        self.workspace_mut().close_flow_tab(id);
        cx.notify();
    }

    pub fn edit_flow(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(flow) = self.workspace().flow(id) {
            cx.open_with_system(&flow.path);
        }
    }

    /// Takes a workflow out of the workspace, file and all, into `.closed`,
    /// like everything else that is removed.
    pub fn delete_flow(&mut self, id: usize, cx: &mut Context<Self>) {
        let dir = self.workspace().dir.clone();
        let Some(path) = self.workspace().flow(id).map(|f| f.path.clone()) else { return };
        if self.editor_for(super::workspace::Item::Flow(id)).is_some() {
            self.stop_editing(cx);
        }
        self.stop_flow(id, cx);
        store::remove_file(&path, &dir);
        self.workspace_mut().close_flow_tab(id);
        let ws = self.workspace_mut();
        ws.flows.retain(|f| f.id != id);
        if ws.selected_flow == Some(id) {
            ws.selected_flow = None;
        }
        self.save_workspace(self.active);
        cx.notify();
    }

    // --- building a workflow -----------------------------------------------
    //
    // A workflow is edited by changing its tree and writing the file from it,
    // never by editing the text, which keeps the same workflow the
    // same thing whether it was built here or typed into the file by hand.

    /// Puts the editor's cursor on a step, which brings its controls onto
    /// screen.
    pub fn select_step(&mut self, flow: usize, spot: Option<Spot>, cx: &mut Context<Self>) {
        self.select_flow(flow, cx);
        if let Some(sheet) = self.workspace_mut().flow_mut(flow) {
            sheet.cursor = spot;
        }
        cx.notify();
    }

    /// Applies one change to one step, and writes the file.
    pub fn change_step(
        &mut self,
        flow: usize,
        spot: &Spot,
        change: &Change,
        cx: &mut Context<Self>,
    ) {
        self.edit_flow_tree(flow, cx, |steps| crate::flow::edit::apply(steps, spot, change));
    }

    /// Adds a step at the end of the workflow, and selects it so its controls
    /// are on screen to be filled in.
    pub fn add_step(&mut self, flow: usize, shape: Shape, cx: &mut Context<Self>) {
        self.edit_flow_tree(flow, cx, |steps| {
            steps.push(shape.blank());
            Some(Spot::top(steps.len() - 1))
        });
    }

    /// Changes the tree, moves the cursor to wherever the change says, and
    /// writes the workflow out.
    fn edit_flow_tree(
        &mut self,
        flow: usize,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Vec<Step>) -> Option<Spot>,
    ) {
        let Some(sheet) = self.workspace().flow(flow) else { return };
        let mut parsed = sheet.flow();
        if parsed.lossy {
            return;
        }
        let spot = edit(&mut parsed.steps);

        let Some(sheet) = self.workspace_mut().flow_mut(flow) else { return };
        sheet.cursor = spot;
        let failure = sheet.put(&parsed).err().map(|e| (sheet.path.clone(), e));
        let source = sheet.source.clone();
        if let Some((path, e)) = failure {
            self.cannot_write("the workflow", &path, &e);
        }

        // The text is the same workflow, so it says the same thing the moment
        // the controls change it. (The other direction is free: the steps are
        // read back out of the source on every repaint, so typing shows up in
        // the controls as it is typed.)
        if let Some(editor) = self.editor_for(super::workspace::Item::Flow(flow)).cloned() {
            editor.update(cx, |editor, cx| editor.set_text(&source, cx));
        }
        cx.notify();
    }

    /// Asks which run a step should start, in the same searchable list the
    /// command bar uses.
    pub fn ask_step_run(
        &mut self,
        flow: usize,
        spot: Spot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_step(flow, Some(spot), cx);
        let choices: Vec<super::picker::Choice> = self
            .workspace()
            .jobs
            .iter()
            .map(|job| super::picker::Choice {
                id: job.id,
                icon: job.tool.icon(),
                label: job.name(),
                detail: format!("{} \u{b7} {}", job.tool.title(), job.target()),
            })
            .collect();

        self.picker_open = true;
        let picker = self.picker.clone();
        picker.update(cx, |picker, cx| {
            picker.open(
                "Run which tool",
                super::picker::Purpose::StepRun { flow },
                choices,
                "Search the runs in this workspace",
                window,
                cx,
            );
            // A workflow may name a run that is not there yet, so a name that
            // matches nothing is still a name.
            picker.new_label = Some("Name it".into());
        });
        cx.notify();
    }

    /// Points the selected step at a run.
    pub fn set_step_run(&mut self, flow: usize, choice: usize, cx: &mut Context<Self>) {
        let name = if choice == super::picker::NEW {
            self.picker.read(cx).query().trim().to_string()
        } else {
            self.workspace().job(choice).map(|j| j.name()).unwrap_or_default()
        };
        if name.is_empty() {
            return;
        }
        let Some(spot) = self.workspace().flow(flow).and_then(|s| s.cursor.clone()) else { return };
        self.change_step(flow, &spot, &Change::SetRun(name), cx);
    }

    /// Keeps the box for a condition's value pointed at the selected step, and
    /// takes what is typed into it.
    ///
    /// The other three parts of a condition are chosen from what exists and
    /// are set the moment they are chosen; only the value has to be typed, so
    /// only the value needs this.
    pub fn sync_step_value(&mut self, cx: &mut Context<Self>) {
        use crate::flow::edit::{Guide, condition_of, takes_a_condition};
        use crate::flow::Step;

        let showing = match self.workspace().showing() {
            Some(super::workspace::Item::Flow(id)) => Some(id),
            _ => None,
        };
        let selected = showing.and_then(|flow| {
            let (spot, step) = self.workspace().flow(flow)?.selected()?;
            Some((flow, spot, step))
        });

        // The name of a `set` step is the one thing with a box of its own.
        match &selected {
            Some((flow, spot, Step::Set { name, .. })) => {
                self.sync_step_name(*flow, spot.clone(), name.clone(), cx)
            }
            _ => self.step_name_for = None,
        }

        let here = selected.and_then(|(flow, spot, step)| match &step {
            Step::Set { value, .. } => Some((flow, spot, value.clone())),
            step if takes_a_condition(step) => {
                let guide = Guide::read_partial(condition_of(step)).unwrap_or_default();
                Some((flow, spot, guide.value))
            }
            _ => None,
        });

        let Some((flow, spot, value)) = here else {
            self.step_value_for = None;
            return;
        };
        let is_set = self
            .workspace()
            .flow(flow)
            .and_then(|f| f.selected())
            .is_some_and(|(_, step)| matches!(step, Step::Set { .. }));

        // A different step means the box is about something else, so it is
        // filled in from that step, not read as a change to it.
        if self.step_value_for.as_ref() != Some(&(flow, spot.clone())) {
            self.step_value_for = Some((flow, spot));
            let input = self.step_input.clone();
            input.update(cx, |input, cx| input.set_value(&value, cx));
            self.step_value_revision = self.step_input.read(cx).revision;
            return;
        }

        let (revision, typed) = {
            let input = self.step_input.read(cx);
            (input.revision, input.value().to_string())
        };
        if revision == self.step_value_revision {
            return;
        }
        self.step_value_revision = revision;
        if typed != value {
            let change =
                if is_set { Change::SetVarValue(typed) } else { Change::SetValue(typed) };
            self.change_step(flow, &spot.clone(), &change, cx);
        }
    }

    /// The same, for the name half of a `set` step.
    fn sync_step_name(
        &mut self,
        flow: usize,
        spot: Spot,
        name: String,
        cx: &mut Context<Self>,
    ) {
        if self.step_name_for.as_ref() != Some(&(flow, spot.clone())) {
            self.step_name_for = Some((flow, spot));
            let input = self.step_name_input.clone();
            input.update(cx, |input, cx| input.set_value(&name, cx));
            self.step_name_revision = self.step_name_input.read(cx).revision;
            return;
        }

        let (revision, typed) = {
            let input = self.step_name_input.read(cx);
            (input.revision, input.value().to_string())
        };
        if revision == self.step_name_revision {
            return;
        }
        self.step_name_revision = revision;
        if typed != name {
            self.change_step(flow, &spot, &Change::SetVarName(typed), cx);
        }
    }

    /// Starts a workflow from its first step.
    pub fn start_flow(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sheet) = self.workspace_mut().flow_mut(id) else { return };
        let steps = crate::flow::parse(&sheet.source).steps;
        sheet.trail.clear();
        sheet.busy = None;
        sheet.machine = Some(crate::flow::compile::Machine::new(&steps));
        self.select_flow(id, cx);
        self.advance_flow(id, window, cx);
    }

    /// Stops a workflow where it stands, and whatever it started.
    pub fn stop_flow(&mut self, id: usize, cx: &mut Context<Self>) {
        let busy = self.workspace().flow(id).and_then(|f| f.busy.clone());
        if let Some(job) = busy.as_ref().and_then(super::flows::Busy::job) {
            self.stop_job(job, cx);
        }
        if let Some(flow) = self.workspace_mut().flow_mut(id) {
            let step = busy.as_ref().map_or(0, super::flows::Busy::step);
            flow.busy = None;
            flow.machine = None;
            flow.trail.push(super::flows::Mark {
                step,
                outcome: super::flows::Outcome::Failed("stopped".into()),
            });
        }
        cx.notify();
    }

    /// Works through a workflow until it has to wait for something.
    ///
    /// Every condition is decided when it is reached, not when the workflow
    /// starts, so a step sees what the steps before it
    /// found.
    fn advance_flow(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        use crate::flow::compile::Event;
        use super::flows::{Busy, Mark, Outcome};

        loop {
            let data = super::notes::Snapshot::of(self.workspace());
            let mut truth = |condition: &str| crate::flow::holds(condition, &data);

            let Some(sheet) = self.workspace_mut().flow_mut(id) else { return };
            let Some(machine) = sheet.machine.as_mut() else { return };

            match machine.next(&mut truth) {
                Event::Skipped { step, why } => {
                    sheet.trail.push(Mark { step, outcome: Outcome::Skipped(why) });
                }
                // The expression is worked out here, against what the steps
                // before it found, and kept for everything after it: the
                // later steps, the conditions, and every document in the
                // workspace.
                Event::Set { step, name, value } => {
                    let worked_out = crate::expr::run(&value, &data)
                        .map(|v| v.show())
                        .map_err(|e| e.to_string());
                    match worked_out {
                        Ok(text) => {
                            // The trail says what it set, not merely that it
                            // did, and the variable remembers which workflow
                            // wrote it and when.
                            sheet.trail.push(Mark {
                                step,
                                outcome: Outcome::Set(format!("{name} = {text}")),
                            });
                            let by = sheet.title();
                            let about = self
                                .workspace()
                                .var(&name)
                                .map(|v| v.about.clone())
                                .unwrap_or_default();
                            self.workspace_mut().put_var(
                                &name,
                                crate::ui::store::Var {
                                    value: text,
                                    formula: false,
                                    set_by: Some(by),
                                    at: Some(std::time::SystemTime::now()),
                                    about,
                                },
                            );
                            self.save_workspace(self.active);
                        }
                        Err(why) => {
                            sheet.trail.push(Mark { step, outcome: Outcome::Skipped(why) });
                        }
                    }
                    cx.notify();
                }
                Event::Wait { step, seconds } => {
                    let at = std::time::Instant::now()
                        + std::time::Duration::from_secs_f64(seconds.clamp(0., 3600.));
                    sheet.busy = Some(Busy::Until { step, at });
                    sheet.trail.push(Mark { step, outcome: Outcome::Running });
                    self.wake_flow(id, seconds, window, cx);
                    cx.notify();
                    return;
                }
                Event::Stopped { step } => {
                    sheet.trail.push(Mark { step, outcome: Outcome::Done });
                    sheet.machine = None;
                    sheet.busy = None;
                    cx.notify();
                    return;
                }
                Event::Finished => {
                    sheet.machine = None;
                    sheet.busy = None;
                    cx.notify();
                    return;
                }
                Event::Run { step, name } => {
                    let Some(job) = self.workspace().job_named(&name).map(|j| j.id) else {
                        let Some(sheet) = self.workspace_mut().flow_mut(id) else { return };
                        sheet.trail.push(Mark {
                            step,
                            outcome: Outcome::Failed(format!("nothing here is called {name}")),
                        });
                        // A step naming a tool that is not here is a mistake in
                        // the workflow, not a result: stop, do not carry on
                        // as if it had happened.
                        sheet.machine = None;
                        sheet.busy = None;
                        cx.notify();
                        return;
                    };
                    // The run is started and the workflow waits: `deliver`
                    // brings it back here when the run finishes.
                    if let Some(sheet) = self.workspace_mut().flow_mut(id) {
                        sheet.trail.push(Mark { step, outcome: Outcome::Running });
                        sheet.busy = Some(Busy::Run { step, job });
                    }
                    self.select_job(job, cx);
                    self.run_selected(window, cx);
                    self.workspace_mut().select_flow(id);
                    cx.notify();
                    return;
                }
            }
        }
    }

    /// Comes back to a workflow once its pause is over.
    fn wake_flow(&mut self, id: usize, seconds: f64, window: &mut Window, cx: &mut Context<Self>) {
        let delay = std::time::Duration::from_secs_f64(seconds.clamp(0., 3600.));
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| {
                // Only if it is still waiting on that same pause: the user may
                // have stopped it, or started it again.
                let waiting = this
                    .workspace()
                    .flow(id)
                    .is_some_and(|f| matches!(f.busy, Some(super::flows::Busy::Until { .. })));
                if !waiting {
                    return;
                }
                if let Some(sheet) = this.workspace_mut().flow_mut(id) {
                    if let Some(last) = sheet.trail.last_mut() {
                        last.outcome = super::flows::Outcome::Done;
                    }
                    sheet.busy = None;
                }
                this.advance_flow(id, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Called when a run finishes, to carry on any workflow waiting on it.
    fn flow_finished(&mut self, job: usize, failed: bool, window: &mut Window, cx: &mut Context<Self>) {
        use super::flows::Outcome;
        let waiting = self
            .workspace()
            .flows
            .iter()
            .find(|f| f.busy.as_ref().and_then(super::flows::Busy::job) == Some(job))
            .map(|f| f.id);
        let Some(id) = waiting else { return };

        if let Some(flow) = self.workspace_mut().flow_mut(id) {
            if let Some(last) = flow.trail.last_mut() {
                last.outcome =
                    if failed { Outcome::Failed("the run failed".into()) } else { Outcome::Done };
            }
            flow.busy = None;
            if let Some(machine) = flow.machine.as_mut() {
                machine.resume(!failed);
            }
        }
        self.advance_flow(id, window, cx);
    }

    /// Re-reads every document whose file has changed. Called on each repaint,
    /// often enough to notice a save in another editor, and cheap
    /// enough that it costs one `stat` per document.
    pub fn refresh_docs(&mut self) -> bool {
        let mut changed = false;
        for ws in &mut self.workspaces {
            for doc in &mut ws.docs {
                if doc.stale() {
                    doc.reload();
                    changed = true;
                }
            }
            for flow in &mut ws.flows {
                // A workflow that is running is reading from the copy it
                // started with; changing the file underneath it would move the
                // steps it has already taken.
                if !flow.running() && flow.stale() {
                    flow.reload();
                    changed = true;
                }
            }
        }
        changed
    }

    // --- comparing ---------------------------------------------------------

    /// The other runs of the same tool in this workspace, which are the ones
    /// a run can be read against.
    pub fn comparable(&self, id: usize) -> Vec<(usize, String)> {
        let Some(job) = self.workspace().job(id) else { return Vec::new() };
        let tool = job.tool.id();
        self.workspace()
            .jobs
            .iter()
            .filter(|other| other.id != id && other.tool.id() == tool && !other.rows.is_empty())
            .map(|other| (other.id, other.name()))
            .collect()
    }

    // --- folders -----------------------------------------------------------

    /// The folders this workspace has, plus the workspace itself.
    ///
    /// The root is index 0 so that "no folder" is a choice like any other; a
    /// folder is index 1 upwards, matching [`store::folders`].
    pub fn folders(&self) -> Vec<String> {
        store::folders(&self.workspace().dir)
    }

    /// Asks which folder to move a tool into. Typing a name that is not there
    /// offers to make it.
    pub fn ask_move(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let here = self.workspace().job(id).and_then(|j| j.folder.clone());
        let mut choices = vec![super::picker::Choice {
            id: 0,
            icon: "explorer",
            label: self.workspace().name(),
            detail: if here.is_none() { "current".into() } else { "the workspace itself".into() },
        }];
        for (at, name) in self.folders().into_iter().enumerate() {
            let current = here.as_deref() == Some(name.as_str());
            choices.push(super::picker::Choice {
                id: at + 1,
                icon: "folder-open",
                detail: if current { "current".into() } else { String::new() },
                label: name,
            });
        }

        let name = self.workspace().job(id).map(|j| j.name()).unwrap_or_default();
        self.picker_open = true;
        let picker = self.picker.clone();
        picker.update(cx, |picker, cx| {
            picker.open(
                format!("Move {name} to"),
                super::picker::Purpose::Move { job: id },
                choices,
                "Search folders, or type a new name",
                window,
                cx,
            );
            picker.new_label = Some("Create folder".into());
        });
        cx.notify();
    }

    /// Moves a tool into a folder, or back out to the workspace itself.
    ///
    /// `choice` is 0 for the workspace and 1 upwards for a folder;
    /// [`super::picker::NEW`] means the name typed into the box.
    pub fn move_job(&mut self, id: usize, choice: usize, cx: &mut Context<Self>) {
        let folders = self.folders();
        let wanted = if choice == super::picker::NEW {
            let typed = self.picker.read(cx).query().trim().to_string();
            let name = store::slugify(&typed);
            if name.is_empty() {
                return;
            }
            Some(name)
        } else if choice == 0 {
            None
        } else {
            folders.get(choice - 1).cloned()
        };

        let dir = self.workspace().dir.clone();
        let Some(job) = self.workspace_mut().job_mut(id) else { return };
        if job.folder == wanted {
            return;
        }
        let (was, stem) = (job.folder.clone(), job.stem.clone());
        job.folder = wanted;

        // The file moves with it, so the workspace on disk stays the workspace
        // on screen.
        store::remove_job_in(&dir, was.as_deref(), &stem);
        self.save_job(self.active, id);
        self.save_workspace(self.active);
        cx.notify();
    }

    /// Asks which folder to move a document into.
    pub fn ask_move_doc(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let here = self.workspace().doc(id).and_then(|d| d.folder.clone());
        let mut choices = vec![super::picker::Choice {
            id: 0,
            icon: "explorer",
            label: self.workspace().name(),
            detail: if here.is_none() { "current".into() } else { "the workspace root".into() },
        }];
        for (at, name) in self.folders().into_iter().enumerate() {
            let current = here.as_deref() == Some(name.as_str());
            choices.push(super::picker::Choice {
                id: at + 1,
                icon: "folder-open",
                detail: if current { "current".into() } else { String::new() },
                label: name,
            });
        }

        let name = self.workspace().doc(id).map(|d| d.stem.clone()).unwrap_or_default();
        self.picker_open = true;
        let picker = self.picker.clone();
        picker.update(cx, |picker, cx| {
            picker.open(
                format!("Move {name} to"),
                super::picker::Purpose::MoveDoc { doc: id },
                choices,
                "Search folders, or type a new name",
                window,
                cx,
            );
            picker.new_label = Some("Create folder".into());
        });
        cx.notify();
    }

    /// Moves a document into a folder, or back out to the workspace root.
    pub fn move_doc(&mut self, id: usize, choice: usize, cx: &mut Context<Self>) {
        let Some(wanted) = self.folder_chosen(choice, cx) else { return };
        self.move_doc_to_folder(id, wanted, cx);
    }

    /// Which folder a picker choice means: the workspace itself, one that
    /// exists, or one to be made from what was typed.
    fn folder_chosen(&self, choice: usize, cx: &Context<Self>) -> Option<Option<String>> {
        if choice == super::picker::NEW {
            let typed = self.picker.read(cx).query().trim().to_string();
            let name = store::slugify(&typed);
            return (!name.is_empty()).then_some(Some(name));
        }
        if choice == 0 {
            return Some(None);
        }
        self.folders().get(choice - 1).cloned().map(Some)
    }

    pub fn move_doc_to_folder(
        &mut self,
        id: usize,
        wanted: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let dir = self.workspace().dir.clone();
        let Some(doc) = self.workspace_mut().doc_mut(id) else { return };
        if doc.folder == wanted {
            return;
        }

        let target_dir = match &wanted {
            Some(f) => dir.join(f),
            None => dir.clone(),
        };
        if std::fs::create_dir_all(&target_dir).is_err() {
            return;
        }
        let new_path = target_dir.join(format!("{}.md", doc.stem));
        if std::fs::rename(&doc.path, &new_path).is_ok() {
            doc.path = new_path;
            doc.folder = wanted;
            doc.seen = std::fs::metadata(&doc.path).ok().and_then(|m| m.modified().ok());
        }
        cx.notify();
    }

    /// Asks which folder to move a workflow into.
    pub fn ask_move_flow(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let here = self.workspace().flow(id).and_then(|f| f.folder.clone());
        let mut choices = vec![super::picker::Choice {
            id: 0,
            icon: "explorer",
            label: self.workspace().name(),
            detail: if here.is_none() { "current".into() } else { "the workspace root".into() },
        }];
        for (at, name) in self.folders().into_iter().enumerate() {
            let current = here.as_deref() == Some(name.as_str());
            choices.push(super::picker::Choice {
                id: at + 1,
                icon: "folder-open",
                detail: if current { "current".into() } else { String::new() },
                label: name,
            });
        }

        let name = self.workspace().flow(id).map(|f| f.stem.clone()).unwrap_or_default();
        self.picker_open = true;
        let picker = self.picker.clone();
        picker.update(cx, |picker, cx| {
            picker.open(
                format!("Move {name} to"),
                super::picker::Purpose::MoveFlow { flow: id },
                choices,
                "Search folders, or type a new name",
                window,
                cx,
            );
            picker.new_label = Some("Create folder".into());
        });
        cx.notify();
    }

    /// Moves a workflow into a folder, or back out to the workspace root.
    pub fn move_flow(&mut self, id: usize, choice: usize, cx: &mut Context<Self>) {
        let Some(wanted) = self.folder_chosen(choice, cx) else { return };
        self.move_flow_to_folder(id, wanted, cx);
    }

    pub fn move_flow_to_folder(
        &mut self,
        id: usize,
        wanted: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let dir = self.workspace().dir.clone();
        let Some(flow) = self.workspace_mut().flow_mut(id) else { return };
        if flow.folder == wanted {
            return;
        }

        let target_dir = match &wanted {
            Some(f) => dir.join(f),
            None => dir.clone(),
        };
        if std::fs::create_dir_all(&target_dir).is_err() {
            return;
        }
        let new_path = target_dir.join(format!("{}.flow", flow.stem));
        if std::fs::rename(&flow.path, &new_path).is_ok() {
            flow.path = new_path;
            flow.folder = wanted;
            flow.seen = std::fs::metadata(&flow.path).ok().and_then(|m| m.modified().ok());
        }
        cx.notify();
    }

    /// Asks to create a new folder in this workspace.
    pub fn ask_create_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choices: Vec<super::picker::Choice> = self
            .folders()
            .into_iter()
            .enumerate()
            .map(|(at, name)| super::picker::Choice {
                id: at + 1,
                icon: "folder-open",
                label: name,
                detail: "existing folder".into(),
            })
            .collect();

        self.picker_open = true;
        let picker = self.picker.clone();
        picker.update(cx, |picker, cx| {
            picker.open(
                "Create new folder".to_string(),
                super::picker::Purpose::CreateFolder,
                choices,
                "Type folder name and press Enter",
                window,
                cx,
            );
            picker.new_label = Some("Create folder".into());
        });
        cx.notify();
    }

    /// Creates a new directory inside the active workspace.
    pub fn create_folder(&mut self, name: &str, cx: &mut Context<Self>) {
        let slug = store::slugify(name);
        if slug.is_empty() {
            return;
        }
        let folder_path = self.workspace().dir.join(&slug);
        let _ = std::fs::create_dir_all(&folder_path);
        cx.notify();
    }

    /// Reorders a job in the workspace.
    pub fn move_job_by(&mut self, id: usize, delta: isize, cx: &mut Context<Self>) {
        if self.workspace_mut().move_job(id, delta) {
            self.save_workspace(self.active);
            cx.notify();
        }
    }

    /// Reorders a document in the workspace.
    pub fn move_doc_by(&mut self, id: usize, delta: isize, cx: &mut Context<Self>) {
        if self.workspace_mut().move_doc(id, delta) {
            cx.notify();
        }
    }

    /// Reorders a workflow in the workspace.
    pub fn move_flow_by(&mut self, id: usize, delta: isize, cx: &mut Context<Self>) {
        if self.workspace_mut().move_flow(id, delta) {
            cx.notify();
        }
    }

    /// Saves the active open file (doc or workflow) when Cmd+S is pressed.
    pub fn save_active_editor(&mut self, cx: &mut Context<Self>) {
        if let Some((item, editor)) = self.editing.clone() {
            let (saved, text) = editor.update(cx, |e, cx| (e.save(cx), e.text().to_string()));
            if saved {
                self.take_edit(item, &text);
                cx.notify();
            }
        }
    }

    /// Asks which run to read this one against.
    pub fn ask_compare(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let others = self.comparable(id);
        if others.is_empty() {
            return;
        }
        let choices: Vec<super::picker::Choice> = others
            .into_iter()
            .map(|(other, label)| {
                let job = self.workspace().job(other);
                super::picker::Choice {
                    id: other,
                    icon: job.map(|j| j.tool.icon()).unwrap_or("compare"),
                    detail: job
                        .map(|j| {
                            let n = j.rows.len();
                            format!("{} · {n} row{}", j.target(), if n == 1 { "" } else { "s" })
                        })
                        .unwrap_or_default(),
                    label,
                }
            })
            .collect();

        let name = self.workspace().job(id).map(|j| j.name()).unwrap_or_default();
        self.picker_open = true;
        let picker = self.picker.clone();
        picker.update(cx, |picker, cx| {
            picker.open(
                format!("Compare {name} with"),
                super::picker::Purpose::Compare { job: id },
                choices,
                "Search runs of the same tool",
                window,
                cx,
            )
        });
        cx.notify();
    }

    pub fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.picker_open {
            return;
        }
        self.picker_open = false;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Takes whatever the picker is pointing at.
    pub fn confirm_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (purpose, chosen) = {
            let picker = self.picker.read(cx);
            (picker.purpose, picker.chosen())
        };
        self.close_picker(window, cx);
        let Some(chosen) = chosen else { return };

        match purpose {
            super::picker::Purpose::Compare { job } => self.compare_job(job, Some(chosen), cx),
            super::picker::Purpose::Move { job } => self.move_job(job, chosen, cx),
            super::picker::Purpose::MoveDoc { doc } => self.move_doc(doc, chosen, cx),
            super::picker::Purpose::MoveFlow { flow } => self.move_flow(flow, chosen, cx),
            super::picker::Purpose::StepRun { flow } => self.set_step_run(flow, chosen, cx),
            super::picker::Purpose::CreateFolder => {
                if chosen == super::picker::NEW {
                    let typed = self.picker.read(cx).query().trim().to_string();
                    self.create_folder(&typed, cx);
                }
            }
        }
    }

    /// Reads one run against another, or stops.
    pub fn compare_job(&mut self, id: usize, other: Option<usize>, cx: &mut Context<Self>) {
        self.select_job(id, cx);
        if let Some(job) = self.workspace_mut().job_mut(id) {
            job.compare_with = other;
        }
        cx.notify();
    }

    // --- collapsing & sidebar folding --------------------------------------

    pub fn is_collapsed(&self, key: &str) -> bool {
        self.collapsed.contains(key)
    }

    pub fn toggle_collapsed(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.collapsed.contains(key) {
            self.collapsed.remove(key);
        } else {
            self.collapsed.insert(key.to_string());
        }
        cx.notify();
    }

    // --- drag and drop in tree ---------------------------------------------

    pub fn move_job_to_folder(&mut self, id: usize, folder: Option<String>, cx: &mut Context<Self>) {
        let dir = self.workspace().dir.clone();
        let Some(job) = self.workspace_mut().job_mut(id) else { return };
        if job.folder == folder {
            return;
        }
        let (was, stem) = (job.folder.clone(), job.stem.clone());
        job.folder = folder;
        store::remove_job_in(&dir, was.as_deref(), &stem);
        self.save_job(self.active, id);
        self.save_workspace(self.active);
        cx.notify();
    }

    /// Puts whatever was dragged into a folder, or back out into the
    /// workspace, for `None`.
    ///
    /// A tool, a document and a workflow are all files in the same directory,
    /// so all three move the same way and the side bar does not have to know
    /// which of them it is carrying.
    pub fn drop_in_folder(
        &mut self,
        item: super::workspace::Item,
        folder: Option<String>,
        cx: &mut Context<Self>,
    ) {
        use super::workspace::Item;
        match item {
            Item::Tool(id) => self.move_job_to_folder(id, folder, cx),
            Item::Doc(id) => self.move_doc_to_folder(id, folder, cx),
            Item::Flow(id) => self.move_flow_to_folder(id, folder, cx),
        }
    }

    /// Dropping one row onto another: it joins that row's folder, and where
    /// the two are the same kind of thing it lands after it.
    pub fn drop_on_item(
        &mut self,
        dragged: super::workspace::Item,
        target: super::workspace::Item,
        cx: &mut Context<Self>,
    ) {
        use super::workspace::Item;
        if dragged == target {
            return;
        }
        let folder = match target {
            Item::Tool(id) => self.workspace().job(id).and_then(|j| j.folder.clone()),
            Item::Doc(id) => self.workspace().doc(id).and_then(|d| d.folder.clone()),
            Item::Flow(id) => self.workspace().flow(id).and_then(|f| f.folder.clone()),
        };
        self.drop_in_folder(dragged, folder, cx);

        if let (Item::Tool(source), Item::Tool(over)) = (dragged, target) {
            self.workspace_mut().reorder_job_after(source, over);
        }
        self.save_workspace(self.active);
        cx.notify();
    }

    // --- notes -------------------------------------------------------------

    /// Starts typing into the note of the line at `line` on screen.
    ///
    /// The index is a position in the table as displayed, the same as every
    /// other selection in the table. The row it stands for is found
    /// through the view, so a sorted or filtered table annotates what you
    /// clicked on.
    pub fn begin_note(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.selected_job_mut() else { return };
        let Some(row_index) = job.view().get(line).copied() else { return };
        let Some(row) = job.rows.get(row_index) else { return };
        let (existing, identity) =
            (row.note.clone().unwrap_or_default(), row.identity().to_string());
        // A note is edited on the line it is read on, so the selection follows
        // the caret there.
        job.select_index(line);
        self.editing_note = Some(line);
        self.editing_note_row = Some((row_index, identity));
        let handle = self.note_input.read(cx).focus_handle.clone();
        window.focus(&handle);
        let input = self.note_input.clone();
        input.update(cx, |input, cx| {
            input.set_value(&existing, cx);
            input.select_all_now(cx);
        });
        self.note_revision = self.note_input.read(cx).revision;
        cx.notify();
    }

    /// Stops typing into it. `sync_note` writes on every keystroke, so what
    /// was typed is already saved and this only puts the caret back.
    pub fn end_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.end_note_edit() {
            return;
        }
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Puts the note editor away without touching the focus, for the places
    /// that are already moving it: leaving the workspace, reordering the
    /// table. Answers whether anything was being edited.
    ///
    /// Anything that reorders the table has to end the edit, because the
    /// editor is drawn on a line and the line would then be a different row.
    pub fn end_note_edit(&mut self) -> bool {
        self.editing_note_row = None;
        self.editing_note.take().is_some()
    }

    /// Sorts the results on a column. This also ends any note being typed:
    /// the line the editor sits on is about to be a different row.
    pub fn sort_results(&mut self, column: usize, cx: &mut Context<Self>) {
        self.end_note_edit();
        if let Some(job) = self.selected_job_mut() {
            job.set_sort(column);
        }
        cx.notify();
    }

    /// Puts the results back into the order they arrived in.
    pub fn clear_sort(&mut self, cx: &mut Context<Self>) {
        self.end_note_edit();
        if let Some(job) = self.selected_job_mut() {
            job.sort = None;
            job.set_filter(job.filter.clone());
        }
        cx.notify();
    }

    // --- editing a file in the pane -----------------------------------------

    /// The editor for what the pane is showing, if it is being edited.
    pub fn editor_for(&self, item: super::workspace::Item) -> Option<&Entity<super::editor::Editor>> {
        self.editing.as_ref().filter(|(open, _)| *open == item).map(|(_, editor)| editor)
    }

    /// Opens the file the pane is showing for editing, or closes the editor
    /// and goes back to reading it.
    pub fn toggle_editing(
        &mut self,
        item: super::workspace::Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor_for(item).is_some() {
            self.stop_editing(cx);
            return;
        }

        use super::workspace::Item;
        let (text, path, language) = match item {
            Item::Doc(id) => {
                let Some(doc) = self.workspace().doc(id) else { return };
                (doc.source.clone(), doc.path.clone(), super::syntax::Language::Markdown)
            }
            Item::Flow(id) => {
                let Some(sheet) = self.workspace().flow(id) else { return };
                (sheet.source.clone(), sheet.path.clone(), super::syntax::Language::Flow)
            }
            Item::Tool(_) => return,
        };

        let editor = cx.new(|cx| super::editor::Editor::new(cx, &text, path, language));
        let handle = editor.read(cx).focus_handle.clone();
        self.editing = Some((item, editor));
        window.focus(&handle);
        cx.notify();
    }

    /// Closes the editor, saving anything unsaved.
    pub fn stop_editing(&mut self, cx: &mut Context<Self>) {
        let Some((item, editor)) = self.editing.take() else { return };
        let text = editor.update(cx, |editor, cx| {
            if editor.dirty {
                editor.save(cx);
            }
            editor.text().to_string()
        });
        self.take_edit(item, &text);
        cx.notify();
    }

    /// Keeps what is on screen in step with what was typed, without waiting
    /// for the file to be read back.
    fn take_edit(&mut self, item: super::workspace::Item, text: &str) {
        use super::workspace::Item;
        match item {
            Item::Doc(id) => {
                if let Some(doc) = self.workspace_mut().doc_mut(id) {
                    doc.source = text.to_string();
                    doc.seen = std::fs::metadata(&doc.path).ok().and_then(|m| m.modified().ok());
                }
            }
            Item::Flow(id) => {
                if let Some(sheet) = self.workspace_mut().flow_mut(id) {
                    sheet.source = text.to_string();
                    sheet.seen =
                        std::fs::metadata(&sheet.path).ok().and_then(|m| m.modified().ok());
                }
            }
            Item::Tool(_) => {}
        }
    }

    /// Keeps the editor pointed at the current theme and the current runs, and
    /// picks up what has been typed. Called from the repaint.
    pub fn sync_editor(&mut self, theme: &Theme, cx: &mut Context<Self>) {
        let Some((item, editor)) = self.editing.clone() else { return };
        let colours = super::editor::Colours::of(theme);
        // Completion offers names, columns and figures and never reads a row,
        // so it is given the shape of each run and not everything in it.
        // This runs on every frame the editor is open.
        let tables = super::notes::shapes(self.workspace());

        let vars: Vec<String> = self.workspace().vars.keys().cloned().collect();
        let (revision, text) = editor.update(cx, |editor, _| {
            editor.colours = colours;
            editor.tables = tables;
            editor.vars = vars;
            (editor.revision, editor.text().to_string())
        });
        // The rendered half of the pane follows the typing, so a document
        // shows its own figures as it is written.
        if self.last_edit_revision != revision {
            self.last_edit_revision = revision;
            self.take_edit(item, &text);
        }
    }

    /// Chooses one of the two cuts of the palette, or hands the choice back to
    /// the desktop.
    pub fn set_theme_choice(
        &mut self,
        choice: store::ThemeChoice,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.theme = choice;
        self.save_settings();
        self.apply_theme(window, cx);
    }

    /// Puts the chosen palette in place. With `System` chosen this follows the
    /// desktop, so it runs on every repaint and not only when something is
    /// clicked.
    pub fn apply_theme(&self, window: &Window, cx: &mut Context<Self>) {
        let wanted = match self.settings.theme {
            store::ThemeChoice::System => Mode::from_appearance(window.appearance()),
            store::ThemeChoice::Light => Mode::Light,
            store::ThemeChoice::Dark => Mode::Dark,
        };
        if super::theme::theme(cx).mode != wanted {
            cx.set_global(Theme::of(wanted));
            cx.notify();
        }
    }

    /// The theme button and ⌘D: flip to the other cut, and mean it.
    ///
    /// Flipping is a choice, so it stops following the desktop. Otherwise the
    /// next repaint would put it straight back.
    pub fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        let next = self.theme(cx).mode.flipped();
        self.settings.theme = match next {
            Mode::Light => store::ThemeChoice::Light,
            Mode::Dark => store::ThemeChoice::Dark,
        };
        self.save_settings();
        cx.set_global(Theme::of(next));
        cx.notify();
    }

    /// Remembers how the side bar was left, when it was asked to.
    pub fn remember_layout(&mut self) {
        if !self.settings.remember_layout {
            return;
        }
        let (open, width) = (self.sidebar_open, self.sidebar_width);
        if self.settings.sidebar_open == Some(open) && self.settings.sidebar_width == Some(width) {
            return;
        }
        self.settings.sidebar_open = Some(open);
        self.settings.sidebar_width = Some(width);
        self.save_settings();
    }

    /// Whether a text field currently has the caret, which decides what Enter
    /// and the arrow keys mean.
    pub fn typing_in_form(&self, window: &Window, cx: &GpuiApp) -> bool {
        let Some(job) = self.selected_job() else { return false };
        job.show_form
            && job
                .inputs
                .iter()
                .flatten()
                .any(|e| e.read(cx).focus_handle.is_focused(window))
    }

    /// Whether the caret is in one of a workflow step's boxes.
    pub fn typing_step_value(&self, window: &Window, cx: &GpuiApp) -> bool {
        self.step_input.read(cx).focus_handle.is_focused(window)
            || self.step_name_input.read(cx).focus_handle.is_focused(window)
    }

    pub fn typing_in_filter(&self, window: &Window, cx: &GpuiApp) -> bool {
        self.selected_job()
            .is_some_and(|j| j.filter_input.read(cx).focus_handle.is_focused(window))
            || self.note_input.read(cx).focus_handle.is_focused(window)
    }
}

impl Focusable for App {
    fn focus_handle(&self, _: &GpuiApp) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Drains the event channel in batches.
///
/// A wide port scan produces tens of thousands of events; applying them one at
/// a time with a repaint each would spend all its time in the renderer. This
/// takes everything waiting, applies it in one go, and then yields for a frame
/// so the window stays responsive without repainting more often than a screen
/// can show.
fn spawn_pump(job_id: usize, rx: async_channel::Receiver<Msg>, cx: &mut Context<App>) -> gpui::Task<()> {
    cx.spawn(async move |this, cx| {
        const MAX_BATCH: usize = 4096;
        loop {
            let Ok(first) = rx.recv().await else { return };
            let mut batch = Vec::with_capacity(64);
            batch.push(first);
            while batch.len() < MAX_BATCH {
                match rx.try_recv() {
                    Ok(msg) => batch.push(msg),
                    Err(_) => break,
                }
            }
            let finished = batch.iter().any(|m| matches!(m, Msg::Finished(_)));

            if this.update(cx, |app, cx| app.deliver(job_id, batch, cx)).is_err() {
                return;
            }
            if finished {
                return;
            }
            // Long enough that a wide scan does not repaint per packet,
            // short enough not to be the thing capping the frame rate on a
            // fast display.
            cx.background_executor().timer(Duration::from_millis(8)).await;
        }
    })
}

/// Re-reads the interface list periodically: plugging in a cable or joining a
/// network should show up without a restart.
fn spawn_iface_poll(cx: &mut Context<App>) -> gpui::Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let fresh = cx.background_executor().spawn(async { iface::interfaces() }).await;
            if this
                .update(cx, |app, cx| {
                    let changed = app.ifaces.len() != fresh.len()
                        || app.ifaces.iter().zip(&fresh).any(|(a, b)| {
                            a.name != b.name
                                || a.addrs.len() != b.addrs.len()
                                || a.default != b.default
                        });
                    if changed {
                        app.ifaces = fresh;
                        // Whatever else is asking what this machine's own
                        // addresses are should stop answering from before.
                        iface::forget_snapshot();
                        cx.notify();
                    }
                })
                .is_err()
            {
                return;
            }
        }
    })
}

/// Keeps elapsed times and rates moving while anything is running.
fn spawn_tick(cx: &mut Context<App>) -> gpui::Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            if this
                .update(cx, |app, cx| {
                    if app.workspaces.iter().any(Workspace::is_busy) || app.notices.any_fading()
                    {
                        cx.notify();
                    }
                })
                .is_err()
            {
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{is_carryable, locate_row};
    use crate::core::{Row, Status};

    fn row(target: &str) -> Row {
        Row {
            cells: vec![target.to_string()],
            status: Status::Up,
            target: target.into(),
            note: None,
            key: None,
        }
    }

    #[test]
    fn a_run_that_keeps_what_was_found_says_so_from_the_first_one() {
        // The first kept run has an empty table to add to, and an empty table
        // must not be mistaken for "not keeping": the rows it writes have to
        // be dated and identified like every kept row after them.
        use crate::core::{Cancel, Params, Run};
        let keeping = Run {
            cancel: Cancel::new(),
            params: Params::new(),
            done: std::collections::HashSet::new(),
            prior: Vec::new(),
            keep: true,
        };
        assert!(keeping.keep);
        assert!(!keeping.resuming(), "keeping is not resuming");
    }

    #[test]
    fn a_note_follows_its_row_rather_than_its_position() {
        let rows = vec![row("10.0.0.1"), row("10.0.0.2"), row("10.0.0.3")];
        assert_eq!(locate_row(&rows, 1, "10.0.0.2"), Some(1));

        // A row inserted above it does not move the note onto its neighbour.
        // Indexing by position alone would.
        let mut grown = rows.clone();
        grown.insert(0, row("10.0.0.9"));
        assert_eq!(locate_row(&grown, 1, "10.0.0.2"), Some(2));

        // A row that has gone leaves nothing to write to, and does not write
        // to whatever took its place.
        let gone: Vec<Row> = rows.iter().filter(|r| r.target != "10.0.0.2").cloned().collect();
        assert_eq!(locate_row(&gone, 1, "10.0.0.2"), None);
    }

    #[test]
    fn a_host_is_carried_into_the_next_tool_and_a_link_is_not() {
        assert!(is_carryable("192.168.1.0/24"));
        assert!(is_carryable("host.example.com"));

        // Placeholders mean "work it out for me", so carrying them forward
        // would freeze an answer that should be worked out again.
        assert!(!is_carryable("auto"));
        assert!(!is_carryable(""));

        // A downloader's target is a URL, or several.
        assert!(!is_carryable("https://www.mediafire.com/file/abc/thing.rar/file"));
        assert!(!is_carryable("https://a.example/1 https://b.example/2"));
    }
}
