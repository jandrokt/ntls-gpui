//! Context menus.
//!
//! One menu at a time, opened at a point with a list of items, closed by
//! choosing something or by clicking away. Each item carries an [`Act`] rather
//! than a closure, so a menu is built where the click happened, the only
//! place that knows what it was about, and performed on the application.

use gpui::{Pixels, Point, SharedString};

use crate::flow::edit::{Change, Shape, Spot};

use super::store::Tag;
use super::workspace::Group;

/// What choosing an item does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    // Workspaces.
    RenameWorkspace(usize),
    TagWorkspace(usize, Tag),
    CloseWorkspace(usize),
    DeleteWorkspace(usize),
    RevealWorkspace(usize),
    NewWorkspace,
    OpenFolder,

    // Tools.
    RunJob(usize),
    CloseTab(usize),
    StopJob(usize),
    RenameJob(usize),
    TagJob(usize, Tag),
    FavouriteJob(usize),
    DuplicateJob(usize),
    /// Copy a tool and start the copy at once.
    DuplicateRunJob(usize),
    ExportJob(usize),
    AskCompare(usize),
    StopComparing(usize),
    AskMove(usize),
    AskMoveDoc(usize),
    AskMoveFlow(usize),
    MoveJobUp(usize),
    MoveJobDown(usize),
    MoveDocUp(usize),
    MoveDocDown(usize),
    MoveFlowUp(usize),
    MoveFlowDown(usize),
    AskCreateFolder,
    RevealFolder(String),
    /// Any path at all, for a file the workspace does not hold.
    RevealPath(String),
    RevealJob(usize),
    CloseJob(usize),
    CloseGroup(Group, Option<String>),

    // Documents.
    OpenDoc(usize),
    EditDoc(usize),
    CloseDocTab(usize),
    RevealDoc(usize),
    DeleteDoc(usize),
    RenameDoc(usize),
    TagDoc(usize, Tag),
    FavouriteDoc(usize),

    // Workflows.
    OpenFlow(usize),
    RunFlow(usize),
    StopFlow(usize),
    EditFlow(usize),
    CloseFlowTab(usize),
    RevealFlow(usize),
    DeleteFlow(usize),
    RenameFlow(usize),
    TagFlow(usize, Tag),
    FavouriteFlow(usize),

    // Workflows, step by step. The editor changes the tree and the file is
    // written from it, so every one of these is one change at one spot.
    ChangeStep(usize, Spot, Change),
    AddStepAfter(usize, Spot, Shape),
    AddStepInside(usize, Spot, usize, Shape),
    AddStepAtEnd(usize, Shape),
    AskStepRun(usize, Spot),

    // Results.
    CopyText(String),
    SendTo(&'static str),

    // Making things.
    NewDoc,
    NewFlow,

    // The frame.
    ToggleSidebar,
    TogglePanel,
    ToggleTheme,
    OpenPalette,
}

/// One line of a menu.
#[derive(Clone, Debug)]
pub enum Item {
    Choice {
        label: SharedString,
        icon: Option<&'static str>,
        act: Act,
        /// Destructive, so it is drawn as such and put at the bottom.
        danger: bool,
    },
    /// A row of colour swatches, a faster way to tag something than a
    /// submenu of colour names.
    Colours {
        current: Tag,
        act: fn(usize, Tag) -> Act,
        subject: usize,
    },
    Separator,
    Heading(SharedString),
}

impl Item {
    pub fn choice(label: impl Into<SharedString>, icon: &'static str, act: Act) -> Item {
        Item::Choice { label: label.into(), icon: Some(icon), act, danger: false }
    }

    pub fn plain(label: impl Into<SharedString>, act: Act) -> Item {
        Item::Choice { label: label.into(), icon: None, act, danger: false }
    }

    pub fn danger(label: impl Into<SharedString>, icon: &'static str, act: Act) -> Item {
        Item::Choice { label: label.into(), icon: Some(icon), act, danger: true }
    }
}

/// A menu that is open.
#[derive(Clone, Debug)]
pub struct Menu {
    /// Where the pointer was. The menu is placed here unless that would put it
    /// off the window, which the renderer works out because only it knows how
    /// big the window is.
    pub at: Point<Pixels>,
    pub items: Vec<Item>,
}

impl Menu {
    pub fn new(at: Point<Pixels>, items: Vec<Item>) -> Menu {
        Menu { at, items }
    }

    /// How tall the menu will be, which decides whether it opens
    /// downwards from the pointer or upwards from it.
    pub fn height(&self) -> Pixels {
        let rows: f32 = self
            .items
            .iter()
            .map(|i| match i {
                Item::Choice { .. } | Item::Heading(_) => 24.,
                Item::Colours { .. } => 24.,
                Item::Separator => 9.,
            })
            .sum();
        gpui::px(rows + 8.)
    }
}

/// The menu for a workspace in the side bar.
pub fn for_workspace(index: usize, tag: Tag, closable: bool) -> Vec<Item> {
    let mut items = vec![
        Item::choice("Rename…", "gear", Act::RenameWorkspace(index)),
        Item::choice("Duplicate", "plus", Act::NewWorkspace),
        Item::choice("Open folder…", "folder-open", Act::OpenFolder),
        Item::choice("Reveal in file manager", "link", Act::RevealWorkspace(index)),
        Item::Separator,
        Item::Colours { current: tag, act: Act::TagWorkspace, subject: index },
        Item::Separator,
    ];
    if closable {
        items.push(Item::choice("Close", "close", Act::CloseWorkspace(index)));
    }
    // A single workspace cannot be closed, but one the user wants to get
    // rid of is not one you should have to keep another beside.
    items.push(Item::danger("Delete\u{2026}", "close", Act::DeleteWorkspace(index)));
    items
}

/// Which way a thing can be moved in the side bar, if either.
///
/// Asked before the menu is built so that a move which would not move
/// anything is left out of it. An item that does nothing when it is chosen is
/// worse than no item at all: it says the thing is possible.
#[derive(Clone, Copy, Debug, Default)]
pub struct CanMove {
    pub up: bool,
    pub down: bool,
}

/// What a tool's menu needs to know about the tool.
///
/// A struct rather than a row of booleans. There were seven of them and
/// nothing at the call site said which was which.
#[derive(Clone, Copy, Debug, Default)]
pub struct JobMenu {
    pub id: usize,
    pub tag: Tag,
    pub running: bool,
    pub favourite: bool,
    /// Whether it has a tab in the editor.
    pub open: bool,
    /// Whether there is another run of the same tool to read this one
    /// against, and whether it already is being read against one.
    pub comparable: bool,
    pub comparing: bool,
    pub can_move: CanMove,
}

/// The move items, and only the ones that would move something.
fn move_items(can: CanMove, up: Act, down: Act) -> Vec<Item> {
    let mut items = Vec::with_capacity(2);
    if can.up {
        items.push(Item::choice("Move up", "chevron-up", up));
    }
    if can.down {
        items.push(Item::choice("Move down", "chevron-down", down));
    }
    items
}

/// The menu for a tool, wherever it is clicked: the side bar or its tab.
pub fn for_job(job: JobMenu) -> Vec<Item> {
    let JobMenu { id, tag, running, favourite, open, comparable, comparing, can_move } = job;
    let mut items = vec![
        if running {
            Item::choice("Stop", "stop", Act::StopJob(id))
        } else {
            Item::choice("Run", "play", Act::RunJob(id))
        },
        Item::choice("Rename…", "gear", Act::RenameJob(id)),
        Item::plain(
            if favourite { "Remove from favourites" } else { "Add to favourites" },
            Act::FavouriteJob(id),
        ),
        Item::Separator,
        Item::Colours { current: tag, act: Act::TagJob, subject: id },
        Item::Separator,
    ];
    items.extend(move_items(can_move, Act::MoveJobUp(id), Act::MoveJobDown(id)));
    items.extend([
        Item::choice("Move to folder\u{2026}", "folder-open", Act::AskMove(id)),
        Item::Separator,
        Item::choice("Duplicate", "plus", Act::DuplicateJob(id)),
        Item::choice("Duplicate and run", "play", Act::DuplicateRunJob(id)),
        Item::choice("Export as CSV\u{2026}", "download", Act::ExportJob(id)),
        Item::choice("Reveal in file manager", "link", Act::RevealJob(id)),
        Item::Separator,
    ]);
    // Comparing is only offered where there is something to compare with:
    // another run of the same tool, in the same workspace. Which one is asked
    // in a searchable list instead of in a submenu that a workspace of thirty
    // runs would make unusable.
    if comparing {
        items.push(Item::choice("Stop comparing", "compare", Act::StopComparing(id)));
        items.push(Item::Separator);
    } else if comparable {
        items.push(Item::choice("Compare with\u{2026}", "compare", Act::AskCompare(id)));
        items.push(Item::Separator);
    }

    // Closing the tab and taking the tool out of the workspace are different
    // things, and only one of them touches the file, so they are two items,
    // and the one that does is the one drawn in red.
    if open {
        items.push(Item::choice("Close tab", "close", Act::CloseTab(id)));
    }
    items.push(Item::danger("Delete tool", "close", Act::CloseJob(id)));
    items
}

/// The menu for a document.
pub fn for_doc(id: usize, tag: Tag, favourite: bool, can_move: CanMove) -> Vec<Item> {
    let mut items = vec![
        Item::choice("Open", "note", Act::OpenDoc(id)),
        Item::choice("Rename\u{2026}", "gear", Act::RenameDoc(id)),
        Item::plain(
            if favourite { "Remove from favourites" } else { "Add to favourites" },
            Act::FavouriteDoc(id),
        ),
        Item::Separator,
        Item::Colours { current: tag, act: Act::TagDoc, subject: id },
        Item::Separator,
        Item::choice("Open in another editor\u{2026}", "link", Act::EditDoc(id)),
    ];
    items.extend(move_items(can_move, Act::MoveDocUp(id), Act::MoveDocDown(id)));
    items.extend([
        Item::choice("Move to folder\u{2026}", "folder-open", Act::AskMoveDoc(id)),
        Item::choice("Reveal in file manager", "link", Act::RevealDoc(id)),
        Item::Separator,
        Item::choice("Close tab", "close", Act::CloseDocTab(id)),
        // The same rule as a tool: this moves the file into the workspace's
        // own `.closed` folder and does not destroy it.
        Item::danger("Delete document", "close", Act::DeleteDoc(id)),
    ]);
    items
}

/// The menu for a workflow.
pub fn for_flow(
    id: usize,
    running: bool,
    tag: Tag,
    favourite: bool,
    can_move: CanMove,
) -> Vec<Item> {
    let mut items = vec![
        if running {
            Item::choice("Stop", "stop", Act::StopFlow(id))
        } else {
            Item::choice("Run", "play", Act::RunFlow(id))
        },
        Item::choice("Open", "note", Act::OpenFlow(id)),
        Item::choice("Rename\u{2026}", "gear", Act::RenameFlow(id)),
        Item::plain(
            if favourite { "Remove from favourites" } else { "Add to favourites" },
            Act::FavouriteFlow(id),
        ),
        Item::Separator,
        Item::Colours { current: tag, act: Act::TagFlow, subject: id },
        Item::Separator,
        Item::choice("Open in another editor\u{2026}", "link", Act::EditFlow(id)),
    ];
    items.extend(move_items(can_move, Act::MoveFlowUp(id), Act::MoveFlowDown(id)));
    items.extend([
        Item::choice("Move to folder\u{2026}", "folder-open", Act::AskMoveFlow(id)),
        Item::choice("Reveal in file manager", "link", Act::RevealFlow(id)),
        Item::Separator,
        Item::choice("Close tab", "close", Act::CloseFlowTab(id)),
        Item::danger("Delete workflow", "close", Act::DeleteFlow(id)),
    ]);
    items
}

/// The menu for one step of a workflow, wherever it is clicked.
pub fn for_step(flow: usize, spot: Spot, step: &crate::flow::Step) -> Vec<Item> {
    use crate::flow::edit::{condition_of, takes_a_condition};

    let mut items = Vec::new();
    if matches!(step, crate::flow::Step::Run { .. }) {
        items.push(Item::choice(
            "Run a different tool\u{2026}",
            "play",
            Act::AskStepRun(flow, spot.clone()),
        ));
        items.push(Item::Separator);
    }

    items.push(Item::Heading("Add after this".into()));
    for shape in Shape::ALL {
        items.push(Item::choice(
            shape.label(),
            shape.icon(),
            Act::AddStepAfter(flow, spot.clone(), shape),
        ));
    }

    items.push(Item::Separator);
    items.push(Item::choice(
        "Move up",
        "chevron-up",
        Act::ChangeStep(flow, spot.clone(), Change::Move(false)),
    ));
    items.push(Item::choice(
        "Move down",
        "chevron-down",
        Act::ChangeStep(flow, spot.clone(), Change::Move(true)),
    ));
    // A step that can carry a condition of its own says so; one that cannot
    // is made conditional by being put inside a branch, the same
    // thing said the long way.
    if takes_a_condition(step) && !condition_of(step).is_empty() {
        items.push(Item::choice(
            "Do this always",
            "close",
            Act::ChangeStep(flow, spot.clone(), Change::ClearCondition),
        ));
    } else if !matches!(step, crate::flow::Step::If { .. }) {
        items.push(Item::choice(
            "Put inside a branch",
            "compare",
            Act::ChangeStep(flow, spot.clone(), Change::Wrap),
        ));
    }

    items.push(Item::Separator);
    items.push(Item::danger("Delete step", "close", Act::ChangeStep(flow, spot, Change::Remove)));
    items
}

/// The menu that offers the kinds of step there are, for the end of a
/// workflow.
pub fn for_new_step(flow: usize) -> Vec<Item> {
    let mut items = vec![Item::Heading("Add a step".into())];
    for shape in Shape::ALL {
        items.push(Item::choice(shape.label(), shape.icon(), Act::AddStepAtEnd(flow, shape)));
    }
    items
}

/// The same, for the end of a block inside a step.
pub fn for_new_step_inside(flow: usize, spot: Spot, block: usize) -> Vec<Item> {
    let mut items = vec![Item::Heading("Add a step".into())];
    for shape in Shape::ALL {
        items.push(Item::choice(
            shape.label(),
            shape.icon(),
            Act::AddStepInside(flow, spot.clone(), block, shape),
        ));
    }
    items
}

/// The menu for a folder in the side bar.
pub fn for_folder(name: String) -> Vec<Item> {
    vec![
        Item::choice("Reveal in file manager", "link", Act::RevealFolder(name)),
        Item::Separator,
        Item::choice("New Folder…", "folder-open", Act::AskCreateFolder),
    ]
}

/// The menu for a group heading in the side bar.
///
/// Every heading has one, and every heading can be emptied: the documents and
/// the workflows are groups like the tools are, not lists with no
/// handle on them.
pub fn for_group(group: Group, folder: Option<String>) -> Vec<Item> {
    let add = match group {
        Group::Stage(_) => Item::choice("Add a tool…", "plus", Act::OpenPalette),
        Group::Documents => Item::choice("New document", "note", Act::NewDoc),
        Group::Workflows => Item::choice("New workflow", "play", Act::NewFlow),
    };
    vec![
        add,
        Item::Separator,
        Item::danger(
            if group.is_stop() { "Stop everything here" } else { "Delete everything here" },
            "close",
            Act::CloseGroup(group, folder),
        ),
    ]
}

/// The menu for a result row.
pub fn for_row(cell: String, target: String, sends: &[(&'static str, &'static str, &'static str)]) -> Vec<Item> {
    let mut items = Vec::new();
    if !cell.is_empty() && cell != target {
        items.push(Item::choice(
            format!("Copy \u{201c}{cell}\u{201d}"),
            "link",
            Act::CopyText(cell),
        ));
    }
    if !target.is_empty() {
        items.push(Item::choice(format!("Copy {target}"), "link", Act::CopyText(target.clone())));
    }
    if !sends.is_empty() {
        items.push(Item::Separator);
        items.push(Item::Heading(format!("Send {target} to").into()));
        // The registry's own titles and icons, so the menu names a tool the
        // way the rest of the application does.
        for (id, title, icon) in sends {
            items.push(Item::choice(*title, icon, Act::SendTo(id)));
        }
    }
    items
}

/// The menu for the empty parts of the frame.
pub fn for_background() -> Vec<Item> {
    vec![
        Item::choice("Add a tool…", "plus", Act::OpenPalette),
        Item::Separator,
        Item::choice("Toggle side bar", "sidebar", Act::ToggleSidebar),
        Item::choice("Toggle output panel", "panel", Act::TogglePanel),
        Item::choice("Toggle theme", "sun", Act::ToggleTheme),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_workspace_cannot_be_closed_from_its_menu() {
        // Closing the only workspace would leave nothing to work in, so the
        // menu does not offer it, and does not offer it only to refuse.
        let only = for_workspace(0, Tag::None, false);
        assert!(!only.iter().any(|i| matches!(i, Item::Choice { act: Act::CloseWorkspace(_), .. })));

        let one_of_several = for_workspace(0, Tag::None, true);
        assert!(
            one_of_several
                .iter()
                .any(|i| matches!(i, Item::Choice { act: Act::CloseWorkspace(_), .. }))
        );
    }

    #[test]
    fn closing_and_deleting_a_workspace_are_different_items() {
        // One takes it out of the program; the other takes it off the disk.
        // Only the second is red, and it is offered even for the last one.
        let items = for_workspace(0, Tag::None, false);
        assert!(
            items
                .iter()
                .any(|i| matches!(i, Item::Choice { act: Act::DeleteWorkspace(0), danger: true, .. }))
        );
    }

    #[test]
    fn a_running_tool_offers_stop_and_a_stopped_one_offers_run() {
        let running = for_job(JobMenu { id: 1, running: true, open: true, ..Default::default() });
        assert!(matches!(&running[0], Item::Choice { act: Act::StopJob(1), .. }));

        let idle = for_job(JobMenu { id: 1, open: true, ..Default::default() });
        assert!(matches!(&idle[0], Item::Choice { act: Act::RunJob(1), .. }));
    }

    #[test]
    fn closing_a_tab_and_removing_a_tool_are_offered_separately() {
        // Only one of them touches the file, so only one of them is red.
        let open = for_job(JobMenu { id: 1, open: true, ..Default::default() });
        assert!(open.iter().any(|i| matches!(i, Item::Choice { act: Act::CloseTab(1), .. })));
        assert!(
            open.iter()
                .any(|i| matches!(i, Item::Choice { act: Act::CloseJob(1), danger: true, .. }))
        );

        // A tool with no tab has no tab to close.
        let shut = for_job(JobMenu { id: 1, ..Default::default() });
        assert!(!shut.iter().any(|i| matches!(i, Item::Choice { act: Act::CloseTab(_), .. })));
    }

    #[test]
    fn a_menu_knows_how_tall_it_is_before_it_is_drawn() {
        // The renderer needs this to decide whether to open downwards from the
        // pointer or upwards from it.
        let short = Menu::new(Default::default(), vec![Item::plain("One", Act::NewWorkspace)]);
        let long =
            Menu::new(Default::default(), for_job(JobMenu { id: 1, open: true, ..Default::default() }));
        assert!(long.height() > short.height());
    }

    #[test]
    fn comparing_is_offered_only_where_there_is_something_to_compare_with() {
        let alone = for_job(JobMenu { id: 1, open: true, ..Default::default() });
        assert!(!alone.iter().any(|i| matches!(i, Item::Choice { act: Act::AskCompare(_), .. })));

        let paired = for_job(JobMenu { id: 1, open: true, comparable: true, ..Default::default() });
        assert!(paired.iter().any(|i| matches!(i, Item::Choice { act: Act::AskCompare(1), .. })));

        // While comparing, the offer is to stop.
        let comparing = for_job(JobMenu { id: 1, open: true, comparable: true, comparing: true, ..Default::default() });
        assert!(
            comparing.iter().any(|i| matches!(i, Item::Choice { act: Act::StopComparing(1), .. }))
        );
        assert!(
            !comparing.iter().any(|i| matches!(i, Item::Choice { act: Act::AskCompare(_), .. }))
        );
    }

    #[test]
    fn a_move_that_would_not_move_anything_is_not_offered() {
        // A menu item that does nothing when it is chosen is worse than no
        // item: it says the thing is possible. The top of a group has nowhere
        // above it, and neither has the first unstarred tool in one, because
        // the row above that is the last starred one and the favourites sort
        // would put the pair straight back.
        let up = |items: &[Item]| {
            items.iter().any(|i| matches!(i, Item::Choice { act: Act::MoveJobUp(_), .. }))
        };
        let down = |items: &[Item]| {
            items.iter().any(|i| matches!(i, Item::Choice { act: Act::MoveJobDown(_), .. }))
        };

        let stuck = for_job(JobMenu { id: 1, open: true, ..Default::default() });
        assert!(!up(&stuck) && !down(&stuck), "neither way is offered");

        let both = for_job(JobMenu {
            id: 1,
            open: true,
            can_move: CanMove { up: true, down: true },
            ..Default::default()
        });
        assert!(up(&both) && down(&both));

        let only_down = for_job(JobMenu {
            id: 1,
            open: true,
            can_move: CanMove { up: false, down: true },
            ..Default::default()
        });
        assert!(!up(&only_down) && down(&only_down));

        // And the same for a document and a workflow, whose stars live in the
        // workspace's marks rather than on the thing itself.
        let doc = for_doc(1, Tag::None, false, CanMove::default());
        assert!(!doc.iter().any(|i| matches!(i, Item::Choice { act: Act::MoveDocUp(_), .. })));
        let flow = for_flow(1, false, Tag::None, false, CanMove { up: true, down: false });
        assert!(flow.iter().any(|i| matches!(i, Item::Choice { act: Act::MoveFlowUp(_), .. })));
        assert!(!flow.iter().any(|i| matches!(i, Item::Choice { act: Act::MoveFlowDown(_), .. })));

        // Taking the items out does not disturb what sits around them.
        assert!(stuck.iter().any(|i| matches!(i, Item::Choice { act: Act::AskMove(1), .. })));
    }

    #[test]
    fn a_tool_can_be_copied_and_the_copy_started_in_one_go() {
        // Re-running a scan without losing the one already there is two
        // clicks otherwise, and the second is easy to forget.
        let items = for_job(JobMenu { id: 1, open: true, ..Default::default() });
        let at = |want: &Act| items.iter().position(|i| matches!(i, Item::Choice { act, .. } if act == want));
        let copy = at(&Act::DuplicateJob(1)).expect("duplicate");
        let copy_run = at(&Act::DuplicateRunJob(1)).expect("duplicate and run");
        assert_eq!(copy_run, copy + 1, "it belongs next to the plain copy");
    }

    #[test]
    fn a_result_menu_names_the_tools_it_can_send_to() {
        let sends = [("portscan", "Port scan", "portscan")];
        let items = for_row("443".into(), "10.0.0.1".into(), &sends);
        assert!(
            items.iter().any(|i| matches!(
                i,
                Item::Choice { label, act: Act::SendTo("portscan"), .. } if label == "Port scan"
            )),
            "the menu should use the tool's own title"
        );
    }

    #[test]
    fn a_cell_that_is_the_target_is_not_offered_twice() {
        let items = for_row("10.0.0.1".into(), "10.0.0.1".into(), &[]);
        let copies = items
            .iter()
            .filter(|i| matches!(i, Item::Choice { act: Act::CopyText(_), .. }))
            .count();
        assert_eq!(copies, 1);
    }
}
