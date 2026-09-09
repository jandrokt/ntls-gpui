//! Workflows in a workspace: the `.flow` files, and running one.
//!
//! Like documents, a workflow is a file the workspace holds and something else
//! edits. Unlike a document, it does something: it starts runs in order, and
//! decides between them from what the ones before found.

use std::path::{Path, PathBuf};

use crate::flow::edit::Spot;
use crate::flow::{Flow, Step, compile::Machine};

/// One workflow, as it was last read off disk.
#[derive(Clone, Debug)]
pub struct Sheet {
    pub id: usize,
    pub stem: String,
    pub path: PathBuf,
    pub source: String,
    pub open: bool,
    pub folder: Option<String>,
    pub seen: Option<std::time::SystemTime>,
    /// What happened the last time it was run, in the order it happened. A
    /// step inside a repeat appears once per pass.
    pub trail: Vec<Mark>,
    /// The workflow part-way through, while it is running.
    pub machine: Option<Machine>,
    /// Everything the run said, oldest first.
    ///
    /// The trail says what became of each step and keeps only the latest
    /// pass of it, which is the wrong shape for a walk: a `print` inside one
    /// says something different every time round, and all but the last of
    /// them would be gone. This keeps the lot, in the order it happened.
    pub printed: Vec<Printed>,
    /// Whether the log is open.
    pub log_open: bool,
    /// What the walks it is inside have their items under, innermost last.
    ///
    /// Not a workspace variable: an item may be a whole object out of an
    /// answer, and it belongs to this run of this workflow rather than to
    /// the workspace, which should not be left holding a `slide` afterwards.
    pub bindings: Vec<(String, crate::expr::Value)>,
    /// What it is waiting on, if anything.
    pub busy: Option<Busy>,
    pub scroll: gpui::ScrollHandle,
    /// The step the editor has selected, and where in the tree it is.
    ///
    /// Selecting a step is what puts its controls on screen, so this is the
    /// whole of the editor's state: everything else it knows is in the file.
    pub cursor: Option<Spot>,
}

/// One line of a run's log: when, which step, and what it said.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Printed {
    pub at: std::time::SystemTime,
    pub step: Option<usize>,
    pub text: String,
    /// Whether it is something going wrong, so the log can show it as such.
    pub bad: bool,
}

/// One thing that happened, against the step it happened to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mark {
    pub step: usize,
    pub outcome: Outcome,
}

/// What a running workflow is waiting for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Busy {
    /// A run it started, and the step that started it.
    Run { step: usize, job: usize },
    /// A pause, and when it is over.
    Until { step: usize, at: std::time::Instant },
}

impl Busy {
    pub fn step(&self) -> usize {
        match self {
            Busy::Run { step, .. } | Busy::Until { step, .. } => *step,
        }
    }

    pub fn job(&self) -> Option<usize> {
        match self {
            Busy::Run { job, .. } => Some(*job),
            Busy::Until { .. } => None,
        }
    }
}

/// What became of one step.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Its condition was false.
    Skipped(String),
    Running,
    Done,
    /// It worked something out and kept it, and this is what it kept.
    Set(String),
    /// It said something into the trail and changed nothing.
    Said(String),
    /// It could not be carried out, and why.
    Failed(String),
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Skipped(_) => "skipped",
            Outcome::Running => "running",
            Outcome::Done => "done",
            Outcome::Set(_) => "set",
            Outcome::Said(_) => "printed",
            Outcome::Failed(_) => "failed",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Outcome::Skipped(why)
            | Outcome::Failed(why)
            | Outcome::Set(why)
            | Outcome::Said(why) => why.clone(),
            _ => String::new(),
        }
    }

    pub fn status(&self) -> crate::core::Status {
        match self {
            Outcome::Skipped(_) => crate::core::Status::Info,
            Outcome::Running => crate::core::Status::Warn,
            Outcome::Done | Outcome::Set(_) => crate::core::Status::Up,
            Outcome::Said(_) => crate::core::Status::Info,
            Outcome::Failed(_) => crate::core::Status::Down,
        }
    }
}

impl Sheet {
    /// Adds a line to the run log.
    ///
    /// Bounded: a `print` inside a walk over a thousand hosts is a thousand
    /// lines, and a workflow left running all night should not grow without
    /// end. The oldest go first, which is the right end to lose.
    pub fn say(&mut self, step: Option<usize>, text: String, bad: bool) {
        /// How many lines of one run are kept.
        const MOST: usize = 2_000;

        self.printed.push(Printed { at: std::time::SystemTime::now(), step, text, bad });
        if self.printed.len() > MOST {
            let over = self.printed.len() - MOST;
            self.printed.drain(..over);
        }
    }

    /// What it is called: the first comment line, or the file name.
    pub fn title(&self) -> String {
        for line in self.source.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("# ") {
                let rest = rest.trim();
                if !rest.is_empty() {
                    return rest.to_string();
                }
            }
            if !line.is_empty() && !line.starts_with('#') {
                break;
            }
        }
        self.stem.clone()
    }

    pub fn flow(&self) -> Flow {
        crate::flow::parse(&self.source)
    }

    /// Replaces what the workflow says and writes it to disk.
    ///
    /// The editor changes the tree, not the text; this is the one place the
    /// two are reconciled, so a workflow built with the mouse and one typed
    /// into a file are the same thing afterwards.
    pub fn put(&mut self, flow: &Flow) -> std::io::Result<()> {
        self.source = crate::flow::write(flow);
        // Write beside the workflow and rename over it. Writing in place
        // empties the file first, so a save that ran out of disk, hit a
        // disconnected volume or was killed part-way left the workflow with
        // nothing or half a workflow in it; the repaint after that saw a file
        // whose time had changed, read the wreckage back in, and the good copy
        // in memory went with it. There is no undo for that.
        let tmp = self.path.with_extension("flow.tmp");
        let saved =
            std::fs::write(&tmp, &self.source).and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = saved {
            // Nothing reads a leftover `.flow.tmp`, but a workspace is a
            // directory people open in Finder, so do not leave one sitting in
            // it.
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        self.seen = modified(&self.path);
        Ok(())
    }

    /// The step the editor has selected, if the tree still has one there.
    ///
    /// A workflow is re-read from its text on every repaint, so a cursor left
    /// over from before a step was deleted has to be checked and not
    /// trusted.
    pub fn selected(&self) -> Option<(Spot, Step)> {
        let spot = self.cursor.clone()?;
        let flow = self.flow();
        let step = crate::flow::edit::at(&flow.steps, &spot)?.clone();
        Some((spot, step))
    }

    /// The last thing that happened to a step, for the mark on its card.
    pub fn mark(&self, step: usize) -> Option<&Outcome> {
        self.trail.iter().rev().find(|m| m.step == step).map(|m| &m.outcome)
    }

    /// How many passes of a repeated step have been done, so a card can say
    /// `2 of 3` and does not flicker.
    pub fn passes(&self, step: usize) -> usize {
        self.trail.iter().filter(|m| m.step == step).count()
    }

    /// A one-line description for the side bar.
    pub fn summary(&self) -> String {
        let flow = self.flow();
        let runs = flow.runs();
        match (runs, flow.problems.len()) {
            (_, problems) if problems > 0 => {
                format!("{runs} step(s) · {problems} problem(s)")
            }
            (1, _) => "1 step".into(),
            (n, _) => format!("{n} steps"),
        }
    }

    pub fn running(&self) -> bool {
        self.machine.is_some()
    }

    pub fn stale(&self) -> bool {
        modified(&self.path) != self.seen
    }

    pub fn reload(&mut self) {
        if let Ok(source) = std::fs::read_to_string(&self.path) {
            self.source = source;
            self.seen = modified(&self.path);
        }
    }
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Every workflow file in a workspace directory and its folders, in name order.
pub fn load(dir: &Path, next_id: &mut usize) -> Vec<Sheet> {
    let mut flows = Vec::new();
    scan_flows(dir, None, next_id, &mut flows);
    flows
}

fn scan_flows(dir: &Path, rel: Option<String>, next_id: &mut usize, flows: &mut Vec<Sheet>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                dirs.push(path);
            }
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("flow")) {
            files.push(path);
        }
    }
    files.sort();
    for path in files {
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else { continue };
        let Ok(source) = std::fs::read_to_string(&path) else { continue };
        let id = *next_id;
        *next_id += 1;
        flows.push(Sheet {
            id,
            stem,
            seen: modified(&path),
            path,
            source,
            open: false,
            folder: rel.clone(),
            trail: Vec::new(),
            machine: None,
            printed: Vec::new(),
            log_open: false,
            bindings: Vec::new(),
            busy: None,
            scroll: gpui::ScrollHandle::new(),
            cursor: None,
        });
    }
    dirs.sort();
    for sub in dirs {
        let name = sub.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let next_rel = match &rel {
            Some(parent) => format!("{parent}/{name}"),
            None => name,
        };
        scan_flows(&sub, Some(next_rel), next_id, flows);
    }
}

/// Writes a new workflow, without overwriting one that is there.
pub fn create(dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    create_in(dir, None, name)
}

/// Writes a new workflow in a specific folder.
pub fn create_in(dir: &Path, folder: Option<&str>, name: &str) -> std::io::Result<PathBuf> {
    let mut target_dir = dir.to_path_buf();
    if let Some(folder) = folder {
        for part in folder.split('/') {
            let s = super::store::slugify(part);
            if !s.is_empty() {
                target_dir.push(s);
            }
        }
    }
    std::fs::create_dir_all(&target_dir)?;
    let stem = super::store::slugify(name);
    let stem = if stem.is_empty() { "workflow".to_string() } else { stem };

    let mut path = target_dir.join(format!("{stem}.flow"));
    let mut n = 2;
    while path.exists() {
        path = target_dir.join(format!("{stem}-{n}.flow"));
        n += 1;
    }
    std::fs::write(&path, crate::flow::write(&crate::flow::starter(name)))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(source: &str) -> Sheet {
        Sheet {
            id: 1,
            stem: "nightly".into(),
            path: PathBuf::from("/tmp/ntls-flow-test/nightly.flow"),
            source: source.into(),
            open: true,
            folder: None,
            seen: None,
            trail: Vec::new(),
            machine: None,
            printed: Vec::new(),
            log_open: false,
            bindings: Vec::new(),
            busy: None,
            scroll: gpui::ScrollHandle::new(),
            cursor: None,
        }
    }

    #[test]
    fn a_workflow_is_named_by_its_first_comment() {
        assert_eq!(sheet("# Nightly check\nrun A").title(), "Nightly check");
        // Failing that, by its file name.
        assert_eq!(sheet("run A").title(), "nightly");
        assert_eq!(sheet("").title(), "nightly");
    }

    #[test]
    fn the_summary_counts_the_steps_and_the_mistakes() {
        assert_eq!(sheet("run A").summary(), "1 step");
        assert_eq!(sheet("run A\nrun B").summary(), "2 steps");
        assert!(sheet("run A\nwibble").summary().contains("1 problem"));
    }

    #[test]
    fn an_outcome_reads_as_a_result_status() {
        use crate::core::Status;
        assert_eq!(Outcome::Done.status(), Status::Up);
        assert_eq!(Outcome::Failed("x".into()).status(), Status::Down);
        assert_eq!(Outcome::Skipped("x".into()).detail(), "x");
    }

    #[test]
    fn a_card_is_marked_with_the_last_thing_that_happened_to_it() {
        let mut sheet = sheet("repeat 2 {\n  run A\n}");
        sheet.trail = vec![
            Mark { step: 1, outcome: Outcome::Done },
            Mark { step: 1, outcome: Outcome::Running },
        ];
        assert_eq!(sheet.mark(1), Some(&Outcome::Running));
        assert_eq!(sheet.passes(1), 2);
        assert_eq!(sheet.mark(0), None);
    }

    #[test]
    fn writing_a_workflow_back_keeps_what_it_says() {
        let dir = std::env::temp_dir().join("ntls-flow-put");
        std::fs::create_dir_all(&dir).unwrap();
        let mut sheet = sheet("# Nightly\n\nrun A\n");
        sheet.path = dir.join("nightly.flow");

        let mut flow = sheet.flow();
        flow.steps.push(crate::flow::Step::Wait { seconds: 30.0 });
        sheet.put(&flow).unwrap();

        assert_eq!(sheet.title(), "Nightly");
        assert_eq!(std::fs::read_to_string(&sheet.path).unwrap(), sheet.source);
        assert_eq!(sheet.flow(), flow);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What this is really asking is whether the save ever emptied the file it
    /// found, because that is the file an interrupted save would leave behind.
    /// A second link to it, made beforehand, still holds the old text if the
    /// new text went somewhere else and was renamed over the top. Extra links
    /// are a Unix idea, hence the gate.
    #[cfg(unix)]
    #[test]
    fn saving_a_workflow_puts_a_whole_file_in_place_of_the_old_one() {
        let dir = std::env::temp_dir().join("ntls-flow-put-whole");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let before = "# Nightly\n\nrun A\n";
        let mut sheet = sheet(before);
        sheet.path = dir.join("nightly.flow");
        std::fs::write(&sheet.path, before).unwrap();
        let witness = dir.join("nightly.before");
        std::fs::hard_link(&sheet.path, &witness).unwrap();

        let mut flow = sheet.flow();
        flow.steps.push(crate::flow::Step::Wait { seconds: 30.0 });
        sheet.put(&flow).unwrap();

        assert_eq!(std::fs::read_to_string(&sheet.path).unwrap(), sheet.source);
        assert_eq!(std::fs::read_to_string(&witness).unwrap(), before);
        assert!(!dir.join("nightly.flow.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
