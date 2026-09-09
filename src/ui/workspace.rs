//! A workspace is one investigation.
//!
//! A question about a network is rarely one tool: you sweep the subnet, pick a
//! host out of it, scan its ports, look up its name. A workspace holds all of
//! that together (every tool you have pointed at the same thing) and the
//! side bar lists the investigations, while the editor's tabs list the tools
//! inside whichever one is open.

use std::collections::BTreeMap;
use std::path::PathBuf;
#[cfg(test)]
use std::time::Duration;

use super::job::{Job, State};
use super::store::{self, Tag, WorkspaceRecord};

/// Where a tool sits in its own life. The side bar groups by this, so the
/// three questions: what have I set up, what is working, what did I find.
/// each have their own place.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Stage {
    /// Configured but not started.
    Draft,
    Running,
    /// Finished, stopped or failed: something to read.
    Results,
}

impl Stage {
    pub fn of(state: &State) -> Stage {
        match state {
            State::Setup => Stage::Draft,
            State::Running => Stage::Running,
            State::Done | State::Stopped | State::Failed(_) => Stage::Results,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Stage::Draft => "Not run",
            Stage::Running => "Running",
            Stage::Results => "Results",
        }
    }

    pub const ALL: [Stage; 3] = [Stage::Draft, Stage::Running, Stage::Results];
}

/// A group in the side bar: the things listed under one heading.
///
/// The tools are grouped by the stage they are at; documents and workflows are
/// groups of their own. Anything done to a whole heading (empty it, ask before
/// emptying it) goes through one of these, so all three headings
/// behave the same way instead of the tools having the only one that works.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Stage(Stage),
    Documents,
    Workflows,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Stage(stage) => stage.title(),
            Group::Documents => "Documents",
            Group::Workflows => "Workflows",
        }
    }

    /// What emptying it does, in the word the heading shows.
    ///
    /// Running tools are stopped, not thrown away; everything else in a
    /// group is a file, and emptying it retires those files.
    pub fn is_stop(self) -> bool {
        matches!(self, Group::Stage(Stage::Running))
    }
}

/// What the editor is showing: a tool, or a document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Tool(usize),
    Doc(usize),
    Flow(usize),
}

pub struct Workspace {
    pub id: usize,
    pub jobs: Vec<Job>,
    /// The markdown files in the directory. They are read off disk, not held
    /// as state, so editing one in any editor is the same as editing it here.
    pub docs: Vec<super::notes::Doc>,
    /// The document the editor is showing, when it is showing one.
    pub selected_doc: Option<usize>,
    /// The workflow files in the directory.
    pub flows: Vec<super::flows::Sheet>,
    pub selected_flow: Option<usize>,
    /// The job the main pane is showing, by job id. A workspace with jobs
    /// always has one selected.
    pub selected: Option<usize>,
    /// Set when the user renames the tab; otherwise the name is read off what
    /// the workspace contains.
    pub name_override: Option<String>,
    pub tag: Tag,
    /// Notes keyed by what they are about, so a remark made while reading a
    /// sweep is still there in the port scan that follows it.
    pub notes: BTreeMap<String, String>,
    /// The colour and star of each document and workflow, which are files with
    /// nowhere of their own to keep them.
    pub marks: BTreeMap<String, store::Mark>,
    /// The workspace's variables.
    ///
    /// One name to one piece of text, or to a formula that works one out,
    /// belonging to the investigation instead of to any run in it: written by
    /// hand in the side bar, or set by a workflow as it goes, and read by
    /// every document, condition and workflow in the workspace.
    pub vars: BTreeMap<String, store::Var>,
    /// The directory this workspace is, on disk.
    pub dir: PathBuf,
    /// The folder names last read off disk, and when. Behind a cell because
    /// the side bar asks for them while holding the workspace by reference.
    disk_folders: std::cell::RefCell<Option<(std::time::Instant, Vec<String>)>>,
    /// The number the next tool's file is named with.
    next_index: usize,
}

impl Workspace {
    pub fn new(id: usize, dir: PathBuf) -> Workspace {
        Workspace {
            id,
            jobs: Vec::new(),
            docs: Vec::new(),
            selected_doc: None,
            flows: Vec::new(),
            selected_flow: None,
            selected: None,
            name_override: None,
            tag: Tag::default(),
            notes: BTreeMap::new(),
            marks: BTreeMap::new(),
            vars: BTreeMap::new(),
            dir,
            next_index: 1,
            disk_folders: std::cell::RefCell::new(None),
        }
    }

    /// Claims the next file name for a tool.
    pub fn next_stem(&mut self, tool: &str) -> String {
        let stem = store::job_stem(self.next_index, tool);
        self.next_index += 1;
        stem
    }

    /// Everything about the workspace that is not a tool.
    pub fn record(&self) -> WorkspaceRecord {
        WorkspaceRecord {
            name: self.name_override.clone().unwrap_or_default(),
            tag: self.tag,
            notes: self.notes.clone(),
            order: self.jobs.iter().map(|j| j.stem.clone()).collect(),
            marks: self.marks.clone(),
            vars: self.vars.iter().map(|(k, v)| (k.clone(), v.clone().into())).collect(),
        }
    }

    /// The colour and star of a document or a workflow.
    pub fn mark(&self, item: Item) -> store::Mark {
        self.mark_key(item).and_then(|k| self.marks.get(&k).copied()).unwrap_or_default()
    }

    pub fn set_mark(&mut self, item: Item, mark: store::Mark) {
        let Some(key) = self.mark_key(item) else { return };
        if mark.is_plain() {
            self.marks.remove(&key);
        } else {
            self.marks.insert(key, mark);
        }
    }

    /// What a mark is filed under: the kind of thing and its file name, so it
    /// survives being renamed in the side bar and being moved into a folder.
    fn mark_key(&self, item: Item) -> Option<String> {
        match item {
            Item::Doc(id) => self.doc(id).map(|d| format!("doc:{}", d.stem)),
            Item::Flow(id) => self.flow(id).map(|f| format!("flow:{}", f.stem)),
            Item::Tool(_) => None,
        }
    }

    /// Restores the parts of a workspace that live in its own file.
    pub fn restore(&mut self, record: &WorkspaceRecord) {
        self.name_override = Some(record.name.clone()).filter(|n| !n.trim().is_empty());
        self.tag = record.tag;
        self.notes = record.notes.clone();
        self.marks = record.marks.clone();
        self.vars =
            record.vars.iter().map(|(k, v)| (k.clone(), v.clone().into())).collect();
    }

    /// Keeps file numbering ahead of what is already on disk.
    pub fn observe_stem(&mut self, stem: &str) {
        if let Some(n) = stem.split('-').next().and_then(|n| n.parse::<usize>().ok()) {
            self.next_index = self.next_index.max(n + 1);
        }
    }

    /// A variable of this workspace, however it is capitalised.
    pub fn var(&self, name: &str) -> Option<&store::Var> {
        self.vars.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, var)| var)
    }

    /// The name it is filed under, whatever capitalisation was asked for.
    pub fn var_named(&self, name: &str) -> Option<String> {
        self.vars.keys().find(|key| key.eq_ignore_ascii_case(name)).cloned()
    }

    /// Sets one, replacing whatever it was called before under a different
    /// capitalisation so there is only ever one of each.
    pub fn put_var(&mut self, name: &str, var: store::Var) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Some(existing) = self.var_named(name)
            && existing != name
        {
            self.vars.remove(&existing);
        }
        self.vars.insert(name.to_string(), var);
    }

    /// Writes a plain piece of text into one, keeping whatever else was said
    /// about it.
    pub fn set_var(&mut self, name: &str, value: &str) {
        let mut var = self.var(name).cloned().unwrap_or_default();
        var.value = value.to_string();
        var.formula = false;
        var.set_by = None;
        var.at = Some(std::time::SystemTime::now());
        self.put_var(name, var);
    }

    /// Renames one, keeping everything said about it.
    pub fn rename_var(&mut self, from: &str, to: &str) {
        let to = to.trim();
        if to.is_empty() || from == to {
            return;
        }
        if let Some(var) = self.vars.remove(from) {
            self.put_var(to, var);
        }
    }

    pub fn remove_var(&mut self, name: &str) {
        self.vars.remove(name);
    }

    /// A note an older version of ntls attached to a target.
    ///
    /// Notes belong to the row now, not to the workspace, since the same
    /// address can be two different machines in two different runs. This still
    /// answers so that notes written before that change are not lost; nothing
    /// writes here any more.
    pub fn note(&self, target: &str) -> Option<&str> {
        self.notes.get(target).map(String::as_str).filter(|n| !n.is_empty())
    }

    /// What the tab is called.
    ///
    /// A workspace is about a thing, not about a tool, so it takes its name
    /// from whatever its tools are pointed at. Placeholders that mean "work it
    /// out for me" are not names.
    pub fn name(&self) -> String {
        if let Some(name) = &self.name_override {
            return name.clone();
        }

        let mut counts: Vec<(String, usize)> = Vec::new();
        for job in &self.jobs {
            let target = job.target();
            if matches!(target.as_str(), "—" | "auto" | "local" | "lan" | "discover" | "Cloudflare") {
                continue;
            }
            match counts.iter_mut().find(|(t, _)| *t == target) {
                Some((_, n)) => *n += 1,
                None => counts.push((target, 1)),
            }
        }
        most_used(counts).unwrap_or_else(|| match self.jobs.len() {
            0 => "New workspace".into(),
            1 => self.jobs[0].name(),
            n => format!("{n} tools"),
        })
    }

    pub fn job(&self, id: usize) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn job_mut(&mut self, id: usize) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn selected_job(&self) -> Option<&Job> {
        self.job(self.selected?)
    }

    pub fn selected_job_mut(&mut self) -> Option<&mut Job> {
        self.job_mut(self.selected?)
    }

    /// The jobs in one stage: the ones marked worth keeping first, then the
    /// rest in the order they were added.
    pub fn stage(&self, stage: Stage) -> Vec<&Job> {
        let mut jobs: Vec<&Job> =
            self.jobs.iter().filter(|j| Stage::of(&j.state) == stage).collect();
        jobs.sort_by_key(|j| !j.favorite);
        jobs
    }

    /// The same, inside one folder, or in the workspace itself for `None`.
    pub fn stage_in(&self, folder: Option<&str>, stage: Stage) -> Vec<&Job> {
        let mut jobs: Vec<&Job> = self
            .jobs
            .iter()
            .filter(|j| j.folder.as_deref() == folder && Stage::of(&j.state) == stage)
            .collect();
        jobs.sort_by_key(|j| !j.favorite);
        jobs
    }

    /// The documents in one folder, or in the workspace root for `None`.
    pub fn docs_in(&self, folder: Option<&str>) -> Vec<&super::notes::Doc> {
        self.docs
            .iter()
            .filter(|d| d.folder.as_deref() == folder)
            .collect()
    }

    /// The workflows in one folder, or in the workspace root for `None`.
    pub fn flows_in(&self, folder: Option<&str>) -> Vec<&super::flows::Sheet> {
        self.flows
            .iter()
            .filter(|f| f.folder.as_deref() == folder)
            .collect()
    }

    /// The folders the open tools sit in, in name order. A folder that exists
    /// on disk but holds nothing is still listed, since it is somewhere to
    /// move a tool to.
    pub fn folders(&self) -> Vec<String> {
        let mut names: Vec<String> =
            self.jobs.iter().filter_map(|j| j.folder.clone()).collect();
        names.extend(self.docs.iter().filter_map(|d| d.folder.clone()));
        names.extend(self.flows.iter().filter_map(|f| f.folder.clone()));
        names.extend(self.disk_folders());
        names.sort();
        names.dedup();
        names
    }

    /// The folder names on disk, remembered for a moment.
    ///
    /// Finding them is a walk of the workspace directory and everything
    /// inside it, and the side bar asks every time it draws. So the answer is
    /// kept briefly rather than the disk being walked afresh for every frame,
    /// which on a workspace of any size is the difference between a side bar
    /// that scrolls and one that stutters.
    fn disk_folders(&self) -> Vec<String> {
        const FRESH: std::time::Duration = std::time::Duration::from_millis(750);

        let mut cached = self.disk_folders.borrow_mut();
        if let Some((looked, names)) = cached.as_ref()
            && looked.elapsed() < FRESH
        {
            return names.clone();
        }
        let names = store::folders(&self.dir);
        *cached = Some((std::time::Instant::now(), names.clone()));
        names
    }

    /// Forgets the folder list, so the next look is a fresh one.
    ///
    /// For when ntls has just made a folder itself: waiting out the moment
    /// above would mean clicking "New folder" and watching nothing happen.
    pub fn forget_folders(&self) {
        *self.disk_folders.borrow_mut() = None;
    }

    /// Puts a tool down just after the tool it was dropped on, which is what
    /// dropping one row onto another in the side bar means.
    pub fn reorder_job_after(&mut self, source_id: usize, target_id: usize) -> bool {
        let Some(from) = self.jobs.iter().position(|j| j.id == source_id) else { return false };
        move_after(&mut self.jobs, from, |j| j.id == target_id);
        true
    }

    /// Moves a tool up or down within its group.
    ///
    /// Up and down mean what the side bar shows, and it lists a group with the
    /// starred tools first. Stepping through the job vector instead moved a
    /// tool past whatever happened to be next to it in the file order, which
    /// in a group holding both starred and plain tools is not the row above or
    /// below it on screen: the favourites sort put the pair straight back the
    /// way they were, so the list looked untouched while the new order was
    /// written to the workspace file all the same.
    pub fn move_job(&mut self, id: usize, delta: isize) -> bool {
        let Some((pos, group)) = self.job_group(id) else { return false };
        let Some((a, b)) = move_within_group(&group, pos, delta) else { return false };
        self.jobs.swap(a, b);
        true
    }

    /// Where a tool sits in the job vector, and the group the side bar draws
    /// it in: the places its members hold and whether each is starred.
    fn job_group(&self, id: usize) -> Option<(usize, Vec<(usize, bool)>)> {
        let pos = self.jobs.iter().position(|j| j.id == id)?;
        let folder = self.jobs[pos].folder.clone();
        let stage = Stage::of(&self.jobs[pos].state);
        let group = self
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| j.folder == folder && Stage::of(&j.state) == stage)
            .map(|(i, j)| (i, j.favorite))
            .collect();
        Some((pos, group))
    }

    /// Whether moving this tool would move it.
    ///
    /// Asked before the menu is built, so an item that cannot act is left out
    /// of it. A tool at the top of its group has nowhere above it, and so has
    /// the first unstarred tool, because the row above that one is the last
    /// starred one and the favourites sort would put the pair straight back.
    pub fn can_move_job(&self, id: usize, delta: isize) -> bool {
        self.job_group(id)
            .is_some_and(|(pos, group)| move_within_group(&group, pos, delta).is_some())
    }

    /// Moves a document up or down within its folder.
    pub fn move_doc(&mut self, id: usize, delta: isize) -> bool {
        let Some((pos, group)) = self.doc_group(id) else { return false };
        let Some((a, b)) = move_within_group(&group, pos, delta) else { return false };
        self.docs.swap(a, b);
        true
    }

    /// The same for a document as [`Self::job_group`] is for a tool.
    ///
    /// A document's star is not on the document, which is a file on disk with
    /// nowhere to keep one, so it is read out of the workspace's marks. That
    /// is where the side bar reads it from too, and the side bar sorts by it,
    /// which this used to ignore: stepping through the vector moved a document
    /// past whichever one happened to be next to it in the file order, and the
    /// favourites sort put the pair back, so the list did not change.
    fn doc_group(&self, id: usize) -> Option<(usize, Vec<(usize, bool)>)> {
        let pos = self.docs.iter().position(|d| d.id == id)?;
        let folder = self.docs[pos].folder.clone();
        let group = self
            .docs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.folder == folder)
            .map(|(i, d)| (i, self.mark(Item::Doc(d.id)).favorite))
            .collect();
        Some((pos, group))
    }

    /// Whether moving this document would move it.
    pub fn can_move_doc(&self, id: usize, delta: isize) -> bool {
        self.doc_group(id)
            .is_some_and(|(pos, group)| move_within_group(&group, pos, delta).is_some())
    }

    /// Moves a workflow up or down within its folder.
    pub fn move_flow(&mut self, id: usize, delta: isize) -> bool {
        let Some((pos, group)) = self.flow_group(id) else { return false };
        let Some((a, b)) = move_within_group(&group, pos, delta) else { return false };
        self.flows.swap(a, b);
        true
    }

    /// The same for a workflow, whose star is kept in the marks as a
    /// document's is.
    fn flow_group(&self, id: usize) -> Option<(usize, Vec<(usize, bool)>)> {
        let pos = self.flows.iter().position(|f| f.id == id)?;
        let folder = self.flows[pos].folder.clone();
        let group = self
            .flows
            .iter()
            .enumerate()
            .filter(|(_, f)| f.folder == folder)
            .map(|(i, f)| (i, self.mark(Item::Flow(f.id)).favorite))
            .collect();
        Some((pos, group))
    }

    /// Whether moving this workflow would move it.
    pub fn can_move_flow(&self, id: usize, delta: isize) -> bool {
        self.flow_group(id)
            .is_some_and(|(pos, group)| move_within_group(&group, pos, delta).is_some())
    }

    /// Adds a job and selects it, since adding one is always to look at it.
    pub fn add(&mut self, mut job: Job) -> usize {
        let id = job.id;
        job.open = true;
        self.jobs.push(job);
        self.selected = Some(id);
        self.selected_doc = None;
        self.selected_flow = None;
        id
    }

    /// Closes one job, moving the selection to its neighbour so the pane never
    /// goes blank while there is still something to show.
    pub fn close(&mut self, id: usize) {
        let Some(at) = self.jobs.iter().position(|j| j.id == id) else { return };
        if let Some(cancel) = &self.jobs[at].cancel {
            cancel.cancel();
        }
        self.jobs.remove(at);

        if self.selected == Some(id) {
            self.selected = self
                .jobs
                .get(at)
                .or_else(|| self.jobs.get(at.wrapping_sub(1)))
                .or_else(|| self.jobs.last())
                .map(|j| j.id);
        }
    }

    /// The tools with a tab, in the order the tabs appear.
    pub fn open_jobs(&self) -> Vec<usize> {
        self.walk_order().into_iter().filter(|id| self.job(*id).is_some_and(|j| j.open)).collect()
    }

    /// Everything with a tab: the documents first, then the tools, because a
    /// document is usually what the tools are for.
    pub fn open_items(&self) -> Vec<Item> {
        let docs = self.docs.iter().filter(|d| d.open).map(|d| Item::Doc(d.id));
        let flows = self.flows.iter().filter(|f| f.open).map(|f| Item::Flow(f.id));
        docs.chain(flows).chain(self.open_jobs().into_iter().map(Item::Tool)).collect()
    }

    pub fn any_open(&self) -> bool {
        self.jobs.iter().any(|j| j.open)
            || self.docs.iter().any(|d| d.open)
            || self.flows.iter().any(|f| f.open)
    }

    /// What the editor is showing.
    pub fn showing(&self) -> Option<Item> {
        if let Some(id) = self.selected_doc
            && self.doc(id).is_some_and(|d| d.open)
        {
            return Some(Item::Doc(id));
        }
        if let Some(id) = self.selected_flow
            && self.flow(id).is_some_and(|f| f.open)
        {
            return Some(Item::Flow(id));
        }
        if let Some(id) = self.selected
            && self.job(id).is_some_and(|j| j.open)
        {
            return Some(Item::Tool(id));
        }
        self.open_items().first().copied()
    }

    pub fn flow(&self, id: usize) -> Option<&super::flows::Sheet> {
        self.flows.iter().find(|f| f.id == id)
    }

    pub fn flow_mut(&mut self, id: usize) -> Option<&mut super::flows::Sheet> {
        self.flows.iter_mut().find(|f| f.id == id)
    }

    pub fn select_job(&mut self, id: usize) {
        if let Some(job) = self.job_mut(id) {
            job.open = true;
        }
        self.selected = Some(id);
        self.selected_doc = None;
        self.selected_flow = None;
    }

    pub fn select_flow(&mut self, id: usize) {
        if let Some(flow) = self.flow_mut(id) {
            flow.open = true;
        }
        self.selected_flow = Some(id);
        self.selected_doc = None;
        self.selected = None;
    }

    pub fn close_flow_tab(&mut self, id: usize) {
        let open: Vec<usize> = self.flows.iter().filter(|f| f.open).map(|f| f.id).collect();
        if let Some(flow) = self.flow_mut(id) {
            flow.open = false;
        }
        if self.selected_flow == Some(id) {
            self.selected_flow = neighbour(&open, id);
        }
    }

    /// A run by the name a workflow calls it, matching what it is called first
    /// and what tool it is second.
    pub fn job_named(&self, name: &str) -> Option<&Job> {
        self.jobs
            .iter()
            .find(|j| j.name().eq_ignore_ascii_case(name))
            .or_else(|| self.jobs.iter().find(|j| j.tool.title().eq_ignore_ascii_case(name)))
            .or_else(|| self.jobs.iter().find(|j| j.tool.id().eq_ignore_ascii_case(name)))
    }

    pub fn doc(&self, id: usize) -> Option<&super::notes::Doc> {
        self.docs.iter().find(|d| d.id == id)
    }

    pub fn doc_mut(&mut self, id: usize) -> Option<&mut super::notes::Doc> {
        self.docs.iter_mut().find(|d| d.id == id)
    }

    /// Shows a document, which also means it has a tab.
    pub fn select_doc(&mut self, id: usize) {
        if let Some(doc) = self.doc_mut(id) {
            doc.open = true;
        }
        self.selected_doc = Some(id);
        self.selected_flow = None;
        self.selected = None;
    }

    /// Closes a document's tab. The file is untouched.
    pub fn close_doc_tab(&mut self, id: usize) {
        let open: Vec<usize> = self.docs.iter().filter(|d| d.open).map(|d| d.id).collect();
        if let Some(doc) = self.doc_mut(id) {
            doc.open = false;
        }
        if self.selected_doc == Some(id) {
            self.selected_doc = neighbour(&open, id);
        }
    }

    /// Closes a tool's tab without touching the tool. The selection moves to
    /// a neighbouring tab, so the editor never goes blank while something is
    /// still open.
    pub fn close_tab(&mut self, id: usize) {
        let open = self.open_jobs();
        let Some(job) = self.job_mut(id) else { return };
        job.open = false;

        if self.selected == Some(id) {
            self.selected = neighbour(&open, id);
        }
    }

    /// The order the keyboard walks: by stage, then by age.
    pub fn walk_order(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.jobs.len());
        for stage in Stage::ALL {
            out.extend(self.stage(stage).iter().map(|j| j.id));
        }
        out
    }

    pub fn select_neighbour(&mut self, delta: isize) {
        let order = self.open_jobs();
        if order.is_empty() {
            self.selected = None;
            return;
        }
        let at = self.selected.and_then(|id| order.iter().position(|&x| x == id));
        let next = match at {
            Some(i) => (i as isize + delta).rem_euclid(order.len() as isize) as usize,
            None if delta < 0 => order.len() - 1,
            None => 0,
        };
        self.selected = Some(order[next]);
    }

    pub fn is_busy(&self) -> bool {
        self.jobs.iter().any(|j| j.state.is_running())
    }

    /// Stops everything, for when the whole workspace is closed.
    pub fn cancel_all(&self) {
        for job in &self.jobs {
            if let Some(cancel) = &job.cancel {
                cancel.cancel();
            }
        }
    }
}

/// Which tab to show once this one closes: the one after it, or the one
/// before if it was last, or nothing if it was the only one.
fn neighbour(open: &[usize], closing: usize) -> Option<usize> {
    let at = open.iter().position(|&x| x == closing)?;
    open.get(at + 1).or_else(|| at.checked_sub(1).and_then(|p| open.get(p))).copied()
}

/// Which two places in the job vector a move up or down exchanges.
///
/// The group arrives as the places its members hold in the vector and whether
/// each is starred, and is walked in the order the side bar draws it: the
/// starred ones first, then the rest as they come. The pair to exchange is
/// therefore rarely the pair that sits together in the vector.
///
/// A step that would take a tool across the line the favourites sort draws is
/// no move at all, since the sort hands the two back in the order they were
/// already in, so it is refused instead of rewriting the saved order for a
/// list that does not change.
fn move_within_group(group: &[(usize, bool)], pos: usize, delta: isize) -> Option<(usize, usize)> {
    let mut shown: Vec<(usize, bool)> = group.to_vec();
    shown.sort_by_key(|&(_, favourite)| !favourite);
    let at = shown.iter().position(|&(place, _)| place == pos)?;
    let &(other, favourite) = shown.get(at.checked_add_signed(delta)?)?;
    (favourite == shown[at].1).then_some((pos, other))
}

/// Which target the workspace is named after, out of the ones its tools are
/// pointed at and how often each is used.
///
/// The most-used one wins, and a tie goes to whichever was counted first,
/// since the tools are counted in the order they were opened and the first is
/// the one the investigation started from. Asking for the greatest count
/// instead took the last of the tied ones, because that is the one
/// `max_by_key` hands back: with a tool each on two addresses, which is most
/// of the time, the workspace was named after whichever tool had just been
/// added and renamed itself out from under the user every time another
/// address was pointed at. Reversing the count and asking for the least
/// keeps the first of the tie.
fn most_used(counts: Vec<(String, usize)>) -> Option<String> {
    counts.into_iter().min_by_key(|(_, n)| std::cmp::Reverse(*n)).map(|(target, _)| target)
}

/// Lifts the thing at `from` out of a list and puts it back down just after
/// the one `is_target` picks out, or on the end if that one has gone.
///
/// The target's place is read after the removal, so it is a place in the
/// shortened list, and the thing dropped belongs one past it. Putting it at
/// the target's own place instead left it above the row the pointer was over:
/// dropping a tool onto the row below it then changed nothing at all, so the
/// drop read as ignored, and every other drop landed the tool on the wrong
/// side of the row it was aimed at and wrote that order to the workspace
/// file, where it outlasted a restart.
fn move_after<T>(items: &mut Vec<T>, from: usize, is_target: impl Fn(&T) -> bool) {
    let item = items.remove(from);
    let at = items.iter().position(is_target).map_or(items.len(), |place| place + 1);
    items.insert(at, item);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::store::{JobRecord, SavedState, Tag};

    fn workspace() -> Workspace {
        Workspace::new(1, PathBuf::from("/tmp/ntls-test"))
    }


    #[test]
    fn a_workspace_is_named_after_what_it_is_pointed_at() {
        let mut ws = workspace();
        assert_eq!(ws.name(), "New workspace");

        // A name the user typed always wins.
        ws.name_override = Some("Home LAN".into());
        assert_eq!(ws.name(), "Home LAN");
    }

    #[test]
    fn a_tie_between_two_targets_keeps_the_one_the_investigation_started_from() {
        // The targets are counted in the order their tools were opened, so
        // the first of a tie is the address the workspace was opened on.
        // Asking for the greatest count hands back the last of the tied ones,
        // which named a workspace after whichever tool had just been added.
        fn counted(pairs: &[(&str, usize)]) -> Option<String> {
            super::most_used(pairs.iter().map(|&(t, n)| (t.to_string(), n)).collect())
        }
        assert_eq!(
            counted(&[("192.168.1.1", 1), ("1.1.1.1", 1)]).as_deref(),
            Some("192.168.1.1")
        );

        // A clear winner wins wherever in the list it sits.
        assert_eq!(counted(&[("192.168.1.1", 1), ("1.1.1.1", 3)]).as_deref(), Some("1.1.1.1"));
        assert_eq!(counted(&[("1.1.1.1", 3), ("192.168.1.1", 1)]).as_deref(), Some("1.1.1.1"));

        // Nothing pointed anywhere is nothing to be named after.
        assert_eq!(counted(&[]), None);
    }

    #[test]
    fn a_typed_name_is_the_only_one_written_down() {
        // Writing the derived name would freeze it, so a workspace that later
        // points somewhere else could never say so.
        let mut ws = workspace();
        assert_eq!(ws.record().name, "");

        ws.name_override = Some("Home LAN".into());
        assert_eq!(ws.record().name, "Home LAN");

        let mut restored = workspace();
        restored.restore(&ws.record());
        assert_eq!(restored.name_override.as_deref(), Some("Home LAN"));

        // An empty name means "work it out from the contents".
        let blank = WorkspaceRecord { name: "  ".into(), ..Default::default() };
        restored.restore(&blank);
        assert_eq!(restored.name_override, None);
    }

    #[test]
    fn notes_written_by_an_older_version_are_still_readable() {
        let mut ws = workspace();
        ws.notes.insert("192.168.1.1".into(), "the router".into());
        ws.notes.insert("192.168.1.2".into(), String::new());
        assert_eq!(ws.note("192.168.1.1"), Some("the router"));
        // An empty one is not a note.
        assert_eq!(ws.note("192.168.1.2"), None);
    }

    #[test]
    fn file_names_stay_ahead_of_what_is_already_on_disk() {
        let mut ws = workspace();
        ws.observe_stem("007-ping");
        assert_eq!(ws.next_stem("dns"), "008-dns");
        assert_eq!(ws.next_stem("dns"), "009-dns");
    }

    #[test]
    fn closing_a_tab_moves_to_the_next_one() {
        // The editor never goes blank while something is still open: the tab
        // after it, or the one before if it was last.
        let open = [7, 8, 9];
        assert_eq!(super::neighbour(&open, 8), Some(9));
        assert_eq!(super::neighbour(&open, 9), Some(8), "the last falls back to its left");
        assert_eq!(super::neighbour(&open, 7), Some(8));

        // The only tab leaves nothing behind, and one that is not open is not
        // something to move away from.
        assert_eq!(super::neighbour(&[7], 7), None);
        assert_eq!(super::neighbour(&open, 42), None);
    }

    #[test]
    fn a_saved_run_is_grouped_by_what_became_of_it() {
        assert_eq!(Stage::of(&State::Setup), Stage::Draft);
        assert_eq!(Stage::of(&State::Running), Stage::Running);
        for finished in [State::Done, State::Stopped, State::Failed("x".into())] {
            assert_eq!(Stage::of(&finished), Stage::Results);
        }
    }

    #[test]
    fn a_restored_run_comes_back_with_its_results() {
        let record = JobRecord {
            tool: "ping".into(),
            title: Some("the router".into()),
            tag: Tag::Green,
            favorite: true,
            open: true,
            params: crate::core::Params::new(),
            state: SavedState::Done,
            elapsed: Some(Duration::from_secs(3)),
            stats: vec![crate::core::kv("sent", "4")],
            rows: vec![crate::core::Row {
                cells: vec!["1".into(), "1.1.1.1".into()],
                status: crate::core::Status::Up,
                target: "1.1.1.1".into(),
                note: None,
                key: None,
            }],
            log: Vec::new(),
            charts: Vec::new(),
            answer: None,
        };

        // The pieces a restored job must bring back with it.
        assert_eq!(crate::ui::store::live_state(&record.state), State::Done);
        assert_eq!(record.rows.len(), 1);
        assert!(record.favorite);
        assert_eq!(record.elapsed, Some(Duration::from_secs(3)));
    }

    #[test]
    fn a_starred_document_is_moved_against_the_order_the_side_bar_draws() {
        // The side bar draws a folder with the starred documents first, and
        // this used to step through the vector instead: a document was moved
        // past whichever one happened to be next to it in the file order, the
        // favourites sort put the pair straight back, and the list did not
        // change while the new order was saved anyway.
        let mut ws = workspace();
        for id in 1..=3 {
            ws.docs.push(crate::ui::notes::Doc {
                id,
                stem: format!("doc{id}"),
                path: PathBuf::from(format!("/tmp/doc{id}.md")),
                source: String::new(),
                open: true,
                folder: None,
                seen: None,
                scroll: gpui::ScrollHandle::new(),
            preview: crate::ui::notes::PREVIEW,
            });
        }
        // Star the last one, so the drawn order is 3, 1, 2.
        ws.set_mark(Item::Doc(3), store::Mark { favorite: true, ..Default::default() });
        let drawn = |ws: &Workspace| {
            let mut ids: Vec<usize> = ws.docs_in(None).iter().map(|d| d.id).collect();
            ids.sort_by_key(|id| !ws.mark(Item::Doc(*id)).favorite);
            ids
        };
        assert_eq!(drawn(&ws), vec![3, 1, 2]);

        // Document 1 is drawn second, and moving it down swaps it with 2.
        assert!(ws.can_move_doc(1, 1));
        assert!(ws.move_doc(1, 1));
        assert_eq!(drawn(&ws), vec![3, 2, 1]);

        // It is now last, so there is nowhere below it, and the menu is asked
        // before it offers the move.
        assert!(!ws.can_move_doc(1, 1));
        assert!(!ws.move_doc(1, 1));

        // And nothing crosses the line the favourites sort draws: the topmost
        // unstarred document has the starred one above it and cannot pass it.
        assert!(!ws.can_move_doc(2, -1));
        assert!(!ws.can_move_doc(3, -1), "the starred one is already at the top");
    }

    #[test]
    fn documents_and_workflows_can_be_grouped_in_folders_and_reordered() {
        let mut ws = workspace();
        let doc1 = crate::ui::notes::Doc {
            id: 1,
            stem: "doc1".into(),
            path: PathBuf::from("/tmp/doc1.md"),
            source: "# Doc 1".into(),
            open: true,
            folder: Some("survey".into()),
            seen: None,
            scroll: gpui::ScrollHandle::new(),
            preview: crate::ui::notes::PREVIEW,
        };
        let doc2 = crate::ui::notes::Doc {
            id: 2,
            stem: "doc2".into(),
            path: PathBuf::from("/tmp/doc2.md"),
            source: "# Doc 2".into(),
            open: true,
            folder: Some("survey".into()),
            seen: None,
            scroll: gpui::ScrollHandle::new(),
            preview: crate::ui::notes::PREVIEW,
        };
        ws.docs.push(doc1);
        ws.docs.push(doc2);

        assert_eq!(ws.docs_in(Some("survey")).len(), 2);
        assert_eq!(ws.docs_in(None).len(), 0);
        assert!(ws.folders().contains(&"survey".to_string()));

        // Reorder doc 1 down
        assert!(ws.move_doc(1, 1));
        assert_eq!(ws.docs[0].id, 2);
        assert_eq!(ws.docs[1].id, 1);
    }

    #[test]
    fn moving_a_tool_exchanges_it_with_the_row_the_side_bar_draws_beside_it() {
        // A group of plain tools reads the same on screen as in the file, so
        // neighbours there are neighbours here.
        let plain = [(0usize, false), (1usize, false), (2usize, false)];
        assert_eq!(super::move_within_group(&plain, 1, 1), Some((1, 2)));
        assert_eq!(super::move_within_group(&plain, 1, -1), Some((1, 0)));
        assert_eq!(super::move_within_group(&plain, 0, -1), None, "nothing above the first");
        assert_eq!(super::move_within_group(&plain, 2, 1), None, "nothing below the last");

        // With a plain tool sitting between two starred ones in the file, the
        // side bar draws the starred pair together, and moving one down has to
        // reach past the plain one to the other star. Taking the next place in
        // the file instead swapped a star with the plain tool, which the
        // favourites sort undid on the way back out.
        let mixed = [(0usize, true), (1usize, false), (2usize, true)];
        assert_eq!(super::move_within_group(&mixed, 0, 1), Some((0, 2)));
        assert_eq!(super::move_within_group(&mixed, 2, -1), Some((2, 0)));

        // A place that is not in the group is nothing to move.
        assert_eq!(super::move_within_group(&mixed, 7, 1), None);
    }

    #[test]
    fn a_tool_is_not_moved_across_the_line_the_favourites_sort_draws() {
        // The file holds the plain tool first and the starred one second; the
        // side bar shows them the other way round. Exchanging the two leaves
        // the side bar exactly as it was, since the sort puts the star back on
        // top, and the workspace file is rewritten for a list nobody saw
        // change. There is nowhere for either of them to go.
        let group = [(0usize, false), (1usize, true)];
        assert_eq!(super::move_within_group(&group, 0, -1), None);
        assert_eq!(super::move_within_group(&group, 0, 1), None);
        assert_eq!(super::move_within_group(&group, 1, 1), None);
        assert_eq!(super::move_within_group(&group, 1, -1), None);
    }

    #[test]
    fn a_tool_dropped_onto_another_comes_to_rest_just_after_it() {
        // Dragging the first tool onto the second puts it below the second.
        // Landing it at the second's own place instead left the list exactly
        // as it was, so the drop looked like nothing had happened.
        let mut order = vec![1usize, 2, 3];
        super::move_after(&mut order, 0, |&id| id == 2);
        assert_eq!(order, vec![2, 1, 3]);

        // Dragging up the list is the same rule read the other way: the tool
        // follows the row it was dropped on.
        super::move_after(&mut order, 2, |&id| id == 2);
        assert_eq!(order, vec![2, 3, 1]);

        // A row that went away while the drag was in the air is no anchor, so
        // the tool goes back on the end rather than being lost.
        super::move_after(&mut order, 0, |&id| id == 99);
        assert_eq!(order, vec![3, 1, 2]);
    }

    #[test]
    fn row_notes_are_independent_per_entry() {
        let row1 = crate::core::Row {
            cells: vec!["1".into(), "1.1.1.1".into()],
            status: crate::core::Status::Up,
            target: "1.1.1.1".into(),
            note: Some("first ping probe".into()),
            key: None,
        };
        let row2 = crate::core::Row {
            cells: vec!["2".into(), "1.1.1.1".into()],
            status: crate::core::Status::Up,
            target: "1.1.1.1".into(),
            note: Some("second ping probe".into()),
            key: None,
        };
        assert_ne!(row1.note, row2.note);
        assert_eq!(row1.note.as_deref(), Some("first ping probe"));
        assert_eq!(row2.note.as_deref(), Some("second ping probe"));
    }

    #[test]
    fn nested_folders_can_be_grouped_and_listed() {
        let mut ws = workspace();
        let doc1 = crate::ui::notes::Doc {
            id: 1,
            stem: "child_doc".into(),
            path: PathBuf::from("/tmp/parent/child/child_doc.md"),
            source: "# Child Doc".into(),
            open: true,
            folder: Some("parent/child".into()),
            seen: None,
            scroll: gpui::ScrollHandle::new(),
            preview: crate::ui::notes::PREVIEW,
        };
        ws.docs.push(doc1);
        assert_eq!(ws.docs_in(Some("parent/child")).len(), 1);
        assert_eq!(ws.docs_in(Some("parent")).len(), 0);
        assert!(ws.folders().contains(&"parent/child".to_string()));
    }
}
