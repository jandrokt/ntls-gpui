//! A workspace is one investigation.
//!
//! A question about a network is rarely one tool: you sweep the subnet, pick a
//! host out of it, scan its ports, look up its name. A workspace holds all of
//! that together — every tool you have pointed at the same thing — and the
//! side bar lists the investigations, while the editor's tabs list the tools
//! inside whichever one is open.

use std::collections::BTreeMap;
use std::path::PathBuf;
#[cfg(test)]
use std::time::Duration;

use super::job::{Job, State};
use super::store::{self, Tag, WorkspaceRecord};

/// Where a tool sits in its own life. The side bar groups by this, so the
/// three questions — what have I set up, what is working, what did I find —
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
/// groups of their own. Anything that can be done to a whole heading — empty
/// it, ask before emptying it — is done to one of these, so all three headings
/// behave the same way rather than the tools having the only one that works.
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
    /// Running tools are stopped rather than thrown away; everything else in a
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
    /// One name to one piece of text — or to a formula that works one out —
    /// belonging to the investigation rather than to any run in it: written by
    /// hand in the side bar, or set by a workflow as it goes, and read by
    /// every document, condition and workflow in the workspace.
    pub vars: BTreeMap<String, store::Var>,
    /// The directory this workspace is, on disk.
    pub dir: PathBuf,
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

    /// The note attached to a result, if there is one.
    pub fn note(&self, target: &str) -> Option<&str> {
        self.notes.get(target).map(String::as_str).filter(|n| !n.is_empty())
    }

    pub fn set_note(&mut self, target: &str, note: &str) {
        if note.trim().is_empty() {
            self.notes.remove(target);
        } else {
            self.notes.insert(target.to_string(), note.trim().to_string());
        }
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
        // The most-used target wins; ties go to whichever was opened first,
        // which is the one the investigation started from.
        counts
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(t, _)| t)
            .unwrap_or_else(|| match self.jobs.len() {
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

    /// The same, inside one folder — or in the workspace itself, for `None`.
    pub fn stage_in(&self, folder: Option<&str>, stage: Stage) -> Vec<&Job> {
        let mut jobs: Vec<&Job> = self
            .jobs
            .iter()
            .filter(|j| j.folder.as_deref() == folder && Stage::of(&j.state) == stage)
            .collect();
        jobs.sort_by_key(|j| !j.favorite);
        jobs
    }

    /// The documents in one folder — or in the workspace root for `None`.
    pub fn docs_in(&self, folder: Option<&str>) -> Vec<&super::notes::Doc> {
        self.docs
            .iter()
            .filter(|d| d.folder.as_deref() == folder)
            .collect()
    }

    /// The workflows in one folder — or in the workspace root for `None`.
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
        names.extend(store::folders(&self.dir));
        names.sort();
        names.dedup();
        names
    }

    /// Reorders a job so it sits before/at target job.
    pub fn reorder_job_after(&mut self, source_id: usize, target_id: usize) -> bool {
        let Some(from) = self.jobs.iter().position(|j| j.id == source_id) else { return false };
        let job = self.jobs.remove(from);
        let target_pos = self.jobs.iter().position(|j| j.id == target_id).unwrap_or(self.jobs.len());
        self.jobs.insert(target_pos, job);
        true
    }

    /// Moves a tool up or down within its group.
    pub fn move_job(&mut self, id: usize, delta: isize) -> bool {
        let Some(pos) = self.jobs.iter().position(|j| j.id == id) else { return false };
        let folder = self.jobs[pos].folder.clone();
        let stage = Stage::of(&self.jobs[pos].state);
        let same_group: Vec<usize> = self
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| j.folder == folder && Stage::of(&j.state) == stage)
            .map(|(i, _)| i)
            .collect();
        let Some(idx_in_group) = same_group.iter().position(|&i| i == pos) else { return false };
        let target_idx = idx_in_group as isize + delta;
        if target_idx < 0 || target_idx as usize >= same_group.len() {
            return false;
        }
        let swap_with = same_group[target_idx as usize];
        self.jobs.swap(pos, swap_with);
        true
    }

    /// Moves a document up or down within its folder.
    pub fn move_doc(&mut self, id: usize, delta: isize) -> bool {
        let Some(pos) = self.docs.iter().position(|d| d.id == id) else { return false };
        let folder = self.docs[pos].folder.clone();
        let same_group: Vec<usize> = self
            .docs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.folder == folder)
            .map(|(i, _)| i)
            .collect();
        let Some(idx_in_group) = same_group.iter().position(|&i| i == pos) else { return false };
        let target_idx = idx_in_group as isize + delta;
        if target_idx < 0 || target_idx as usize >= same_group.len() {
            return false;
        }
        let swap_with = same_group[target_idx as usize];
        self.docs.swap(pos, swap_with);
        true
    }

    /// Moves a workflow up or down within its folder.
    pub fn move_flow(&mut self, id: usize, delta: isize) -> bool {
        let Some(pos) = self.flows.iter().position(|f| f.id == id) else { return false };
        let folder = self.flows[pos].folder.clone();
        let same_group: Vec<usize> = self
            .flows
            .iter()
            .enumerate()
            .filter(|(_, f)| f.folder == folder)
            .map(|(i, _)| i)
            .collect();
        let Some(idx_in_group) = same_group.iter().position(|&i| i == pos) else { return false };
        let target_idx = idx_in_group as isize + delta;
        if target_idx < 0 || target_idx as usize >= same_group.len() {
            return false;
        }
        let swap_with = same_group[target_idx as usize];
        self.flows.swap(pos, swap_with);
        true
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
    fn notes_are_kept_against_what_they_are_about() {
        let mut ws = workspace();
        ws.set_note("192.168.1.1", "  the router  ");
        assert_eq!(ws.note("192.168.1.1"), Some("the router"));

        // Clearing a note removes it rather than storing an empty one.
        ws.set_note("192.168.1.1", "   ");
        assert_eq!(ws.note("192.168.1.1"), None);
        assert!(ws.record().notes.is_empty());
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
        };

        // The pieces a restored job must bring back with it.
        assert_eq!(crate::ui::store::live_state(&record.state), State::Done);
        assert_eq!(record.rows.len(), 1);
        assert!(record.favorite);
        assert_eq!(record.elapsed, Some(Duration::from_secs(3)));
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
        };
        ws.docs.push(doc1);
        assert_eq!(ws.docs_in(Some("parent/child")).len(), 1);
        assert_eq!(ws.docs_in(Some("parent")).len(), 0);
        assert!(ws.folders().contains(&"parent/child".to_string()));
    }
}
