//! What the command bar can *do*, as opposed to what it can find.
//!
//! Everything else the palette offers is a noun: a tool, a run, a document, a
//! row of results. This is the verbs. They live in menus and on keys already,
//! which is exactly the problem: a command bar that cannot save, run, stop,
//! close, or switch a workspace is a search box, and typing the name of a
//! thing you want to *do* found nothing at all.
//!
//! A command is only offered when it would do something. Stop is not there
//! while nothing is running, and Toggle Response is not there for a tool that
//! never received an answer, because a list of things that will silently do
//! nothing is worse than a shorter list.

use super::menu::Act;
use super::workspace::Item;

/// One thing the palette can do.
#[derive(Clone, Debug)]
pub struct Command {
    /// What it is called, as it reads in the list.
    pub title: String,
    /// The keys that also do it, shown on the right so the palette teaches
    /// them rather than replacing them.
    pub keys: Option<&'static str>,
    pub icon: &'static str,
    pub act: Act,
}

fn cmd(
    title: impl Into<String>,
    keys: Option<&'static str>,
    icon: &'static str,
    act: Act,
) -> Command {
    Command { title: title.into(), keys, icon, act }
}

/// What is true of the window right now, which decides what is worth
/// offering.
#[derive(Clone, Debug, Default)]
pub struct Now {
    /// The tool the pane is showing, if it is showing one.
    pub job: Option<usize>,
    /// What that tool is called, so a command can name it.
    pub job_name: String,
    pub job_running: bool,
    pub job_has_answer: bool,
    pub job_has_chart: bool,
    /// The document or workflow on screen, if that is what it is.
    pub showing: Option<Item>,
    pub doc_name: String,
    pub flow_running: bool,
    /// Which workspace is open, and how many there are: moving between them
    /// only means something when there is somewhere to move to.
    pub workspace: usize,
    pub workspaces: usize,
    pub workspace_name: String,
    pub sidebar_open: bool,
    pub panel_open: bool,
    pub notices: usize,
}

/// Every command worth offering right now, in the order a menu would put
/// them.
/// They are pushed in the order a menu would put them — what you can do to
/// the thing in front of you, then making things, then the workspace, then
/// the window, then the application — and that push order is the order they
/// are offered in.
pub fn all(now: &Now) -> Vec<Command> {
    let mut out = Vec::new();

    // What you can do to the tool in front of you. Named, so the list says
    // which tool it means rather than making you remember.
    if let Some(id) = now.job {
        let about = |verb: &str| format!("{verb} {}", now.job_name);
        if now.job_running {
            out.push(cmd(about("Stop"), Some("⌘."), "stop", Act::StopJob(id)));
        } else {
            out.push(cmd(about("Run"), Some("⌘R"), "play", Act::RunJob(id)));
        }
        out.push(cmd(about("Settings for"), Some("⌘E"), "gear", Act::Global(Global::ShowForm)));
        out.push(cmd(about("Name and colour of"), Some("⌘I"), "note", Act::Global(Global::Properties)));
        out.push(cmd(about("Duplicate"), None, "plus", Act::DuplicateJob(id)));
        out.push(cmd(about("Export"), None, "download", Act::ExportJob(id)));
        out.push(cmd(about("Compare"), None, "compare", Act::AskCompare(id)));
        out.push(cmd(about("Reveal the file of"), None, "link", Act::RevealJob(id)));
        out.push(cmd(about("Close"), Some("⌘W"), "close", Act::CloseJob(id)));
    }

    // A document or a workflow, when that is what is on screen.
    match now.showing {
        Some(Item::Doc(id)) => {
            let about = |verb: &str| format!("{verb} {}", now.doc_name);
            out.push(cmd(about("Edit the source of"), Some("⌘E"), "note", Act::Global(Global::ShowForm)));
            out.push(cmd(about("Open elsewhere:"), None, "link", Act::EditDoc(id)));
            out.push(cmd(about("Reveal the file of"), None, "link", Act::RevealDoc(id)));
            out.push(cmd(about("Close"), Some("⌘W"), "close", Act::CloseDocTab(id)));
        }
        Some(Item::Flow(id)) => {
            let about = |verb: &str| format!("{verb} {}", now.doc_name);
            if now.flow_running {
                out.push(cmd(about("Stop"), Some("⌘."), "stop", Act::StopFlow(id)));
            } else {
                out.push(cmd(about("Run"), Some("⌘R"), "play", Act::RunFlow(id)));
            }
            out.push(cmd(about("Edit the source of"), Some("⌘E"), "note", Act::Global(Global::ShowForm)));
            out.push(cmd(about("Reveal the file of"), None, "link", Act::RevealFlow(id)));
            out.push(cmd(about("Close"), Some("⌘W"), "close", Act::CloseFlowTab(id)));
        }
        _ => {}
    }

    // Making things.
    out.push(cmd("New tool\u{2026}", Some("⌘N"), "plus", Act::Global(Global::AddTool)));
    out.push(cmd("New document", None, "note", Act::NewDoc));
    out.push(cmd("New workflow", None, "play", Act::NewFlow));
    out.push(cmd("New folder\u{2026}", None, "folder", Act::AskCreateFolder));
    out.push(cmd("New workspace", Some("⌘T"), "folder-open", Act::NewWorkspace));
    out.push(cmd("Open a folder as a workspace\u{2026}", None, "folder-open", Act::OpenFolder));

    // The workspace itself.
    out.push(cmd(format!("Rename {}", now.workspace_name), None, "gear", Act::RenameWorkspace(now.workspace)));
    out.push(cmd(format!("Reveal {} in the file manager", now.workspace_name), None, "link", Act::RevealWorkspace(now.workspace)));
    out.push(cmd("Save everything", Some("⌘S"), "check", Act::Global(Global::Save)));
    if now.workspaces > 1 {
        out.push(cmd("Next workspace", Some("⌘⇧]"), "chevron-right", Act::Global(Global::NextWorkspace)));
        out.push(cmd("Previous workspace", Some("⌘⇧["), "chevron-right", Act::Global(Global::PrevWorkspace)));
    }
    out.push(cmd(format!("Close {}", now.workspace_name), Some("⌘⇧W"), "close", Act::CloseWorkspace(now.workspace)));

    // What the window shows.
    out.push(cmd(
        if now.sidebar_open { "Hide the side bar" } else { "Show the side bar" },
        Some("⌘B"),
        "sidebar", Act::ToggleSidebar,
    ));
    if now.job.is_some() {
        out.push(cmd(
            if now.panel_open { "Hide the output panel" } else { "Show the output panel" },
            Some("⌘J"),
            "panel", Act::TogglePanel,
        ));
        out.push(cmd("Filter the results", Some("⌘F"), "filter", Act::Global(Global::FocusFilter)));
        if now.job_has_chart {
            out.push(cmd("Toggle the graph", Some("⌘G"), "chart", Act::Global(Global::ToggleChart)));
        }
        if now.job_has_answer {
            out.push(cmd("Toggle the response", Some("⌘⇧R"), "globe", Act::Global(Global::ToggleResponse)));
        }
    }
    out.push(cmd("Workspaces", Some("⌘⇧O"), "folder-open", Act::Global(Global::ShowWorkspaces)));
    out.push(cmd("Tools", Some("⌘⇧E"), "explorer", Act::Global(Global::ShowTools)));
    out.push(cmd("Variables", None, "variables", Act::Global(Global::ShowVariables)));
    out.push(cmd("This machine", None, "globe", Act::Global(Global::ShowInterfaces)));
    out.push(cmd(
        if now.notices == 0 {
            "Notifications".to_string()
        } else {
            format!("Notifications ({})", now.notices)
        },
        Some("⌘⇧M"),
        "bell", Act::Global(Global::ShowNotices),
    ));

    // The application.
    out.push(cmd("Switch between light and dark", Some("⌘D"), "moon", Act::ToggleTheme));
    out.push(cmd("Settings\u{2026}", Some("⌘,"), "gear", Act::Global(Global::Preferences)));
    out.push(cmd("Quit ntls", Some("⌘Q"), "close", Act::Global(Global::Quit)));

    out
}

/// A command that acts on whatever is in front of you rather than on a thing
/// named by number.
///
/// These already exist as keys and menu items. Rather than a second copy of
/// each, the palette names the same one and the application does what the key
/// would have done.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Global {
    Save,
    AddTool,
    ShowForm,
    Properties,
    FocusFilter,
    ToggleChart,
    ToggleResponse,
    ShowWorkspaces,
    ShowTools,
    ShowVariables,
    ShowInterfaces,
    ShowNotices,
    NextWorkspace,
    PrevWorkspace,
    Preferences,
    Quit,
}
