//! Documents in a workspace: the `.md` files beside the tool files.
//!
//! A document is a file. ntls lists them, renders them with the tools' figures
//! filled in, and opens them in whatever you write with. ntls is not a text
//! editor; a workspace is a real directory, so handing the file over costs
//! nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::expr::{Source, Table};

use super::job::Job;
use super::workspace::Workspace;

/// One document, as it was last read off disk.
#[derive(Clone, Debug)]
pub struct Doc {
    /// Runtime identity, so a tab can refer to it.
    pub id: usize,
    /// The file name, without the extension.
    pub stem: String,
    pub path: PathBuf,
    /// What the file said when it was last read.
    pub source: String,
    /// Whether it has a tab in the editor.
    pub open: bool,
    /// Which folder it sits in, if any.
    pub folder: Option<String>,
    /// When the file was last modified, so a change on disk is noticed.
    pub seen: Option<std::time::SystemTime>,
    pub scroll: gpui::ScrollHandle,
    /// How much of the pane the preview takes while the source is open, as a
    /// fraction. Zero hides it.
    ///
    /// A document that is mostly prose wants the room to write in; one that
    /// is mostly figures wants to watch them. Neither is the default for the
    /// other, so it is dragged.
    pub preview: f32,
}

/// What the preview starts at, and the least and most it can be dragged to.
///
/// A preview narrower than this cannot show a line of prose, and one wider
/// leaves nowhere to type; below the floor it snaps shut instead, which is
/// what the drag is for as well as the button.
pub const PREVIEW: f32 = 0.5;
pub const PREVIEW_LEAST: f32 = 0.15;
pub const PREVIEW_MOST: f32 = 0.85;

impl Doc {
    /// What the document is called: its first heading, or its file name.
    ///
    /// A heading may itself be computed (`# Link health, {{ Router.target }}`),
    /// so the source is given the figures before the name is taken from it.
    pub fn title(&self, data: &dyn Source) -> String {
        for line in self.source.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("# ") {
                return crate::doc::expand(rest.trim(), data);
            }
            if !line.is_empty() {
                break;
            }
        }
        self.stem.clone()
    }

    /// The first line worth showing under the title in a list.
    pub fn summary(&self, data: &dyn Source) -> String {
        let mut lines = self.source.lines().map(str::trim).filter(|l| !l.is_empty());
        // Skip the title, since it is already the label.
        if self.source.trim_start().starts_with("# ") {
            lines.next();
        }
        let line = lines.next().unwrap_or_default();
        let plain = crate::doc::expand(line, data);
        // Emphasis marks are for the document, not for a one-line summary.
        plain
            .replace("**", "")
            .replace('`', "")
            .trim_start_matches(['#', '>', '-', '*', ' '])
            .chars()
            .take(90)
            .collect()
    }

    /// Whether the file has changed since it was read.
    pub fn stale(&self) -> bool {
        modified(&self.path) != self.seen
    }

    /// Reads the file again.
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

/// Every markdown file in a workspace directory and its folders, in name order.
pub fn load(dir: &Path, next_id: &mut usize) -> Vec<Doc> {
    let mut docs = Vec::new();
    scan_docs(dir, None, next_id, &mut docs);
    docs
}

fn scan_docs(dir: &Path, rel: Option<String>, next_id: &mut usize, docs: &mut Vec<Doc>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                dirs.push(path);
            }
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
            files.push(path);
        }
    }
    files.sort();
    for path in files {
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else { continue };
        let Ok(source) = std::fs::read_to_string(&path) else { continue };
        let id = *next_id;
        *next_id += 1;
        docs.push(Doc {
            id,
            stem,
            seen: modified(&path),
            path,
            source,
            open: false,
            folder: rel.clone(),
            scroll: gpui::ScrollHandle::new(),
            preview: crate::ui::notes::PREVIEW,
        });
    }
    dirs.sort();
    for sub in dirs {
        let name = sub.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let next_rel = match &rel {
            Some(parent) => format!("{parent}/{name}"),
            None => name,
        };
        scan_docs(&sub, Some(next_rel), next_id, docs);
    }
}

/// Writes a new document, without overwriting one that is there.
pub fn create(dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    create_in(dir, None, name)
}

/// Writes a new document in a specific folder.
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
    let stem = if stem.is_empty() { "notes".to_string() } else { stem };

    let mut path = target_dir.join(format!("{stem}.md"));
    let mut n = 2;
    while path.exists() {
        path = target_dir.join(format!("{stem}-{n}.md"));
        n += 1;
    }
    // Empty. A new document is a blank page: anything written into it here
    // would be something to delete before writing what it is for.
    std::fs::write(&path, "")?;
    Ok(path)
}

/// The tools in a workspace, as expressions see them.
pub struct Data<'a> {
    workspace: &'a Workspace,
    /// The tables built so far, by the run they came from.
    ///
    /// A table carries every row a run found, so building one is not free and
    /// building all of them is what a 65,000-port scan costs. Nothing is built
    /// until an expression asks for it by name, and nothing is built twice:
    /// the side bar renders this on every frame, and most frames ask for
    /// nothing at all.
    built: std::cell::RefCell<std::collections::HashMap<usize, Arc<Table>>>,
    /// The formulas being worked out right now.
    ///
    /// A formula may name another, and two that name each other would go
    /// round for ever; a name already on this list answers with nothing
    /// instead, the same answer every unanswerable expression gives
    /// everywhere else.
    resolving: std::cell::RefCell<Vec<String>>,
}

impl<'a> Data<'a> {
    pub fn of(workspace: &'a Workspace) -> Data<'a> {
        Data {
            workspace,
            built: std::cell::RefCell::new(std::collections::HashMap::new()),
            resolving: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn build(&self, job: &Job) -> Arc<Table> {
        if let Some(table) = self.built.borrow().get(&job.id) {
            return table.clone();
        }
        let table = Arc::new(table_of(job));
        self.built.borrow_mut().insert(job.id, table.clone());
        table
    }
}

impl Source for Data<'_> {
    fn table(&self, name: &str) -> Option<Arc<Table>> {
        // A run's own name first, then the tool it is: `Router` before `ping`.
        let job = self
            .workspace
            .jobs
            .iter()
            .find(|j| j.name().eq_ignore_ascii_case(name))
            .or_else(|| self.workspace.jobs.iter().find(|j| j.tool.id().eq_ignore_ascii_case(name)))?;
        Some(self.build(job))
    }

    fn tables(&self) -> Vec<Arc<Table>> {
        self.workspace.jobs.iter().map(|j| self.build(j)).collect()
    }

    fn var(&self, name: &str) -> Option<String> {
        let var = self.workspace.var(name)?.clone();
        resolve(&var, name, self, &self.resolving)
    }
}

/// What a variable comes to: the text it holds, or the answer to the formula
/// it holds.
pub fn resolve(
    var: &crate::ui::store::Var,
    name: &str,
    source: &dyn Source,
    resolving: &std::cell::RefCell<Vec<String>>,
) -> Option<String> {
    if !var.formula {
        return Some(var.value.clone());
    }
    if resolving.borrow().iter().any(|held| held.eq_ignore_ascii_case(name)) {
        return Some(String::new());
    }
    resolving.borrow_mut().push(name.to_string());
    let answer = crate::expr::run(&var.value, source).map(|value| value.show()).ok();
    resolving.borrow_mut().pop();
    answer
}

/// Every run in a workspace, read once and owned.
///
/// A running workflow decides its next step while it is holding the workspace
/// open to change it, so it answers its conditions from a copy and not
/// from a borrow. It is read once per step, not once per frame, so the copy
/// is affordable.
pub struct Snapshot {
    tables: Vec<Arc<Table>>,
    vars: std::collections::BTreeMap<String, crate::ui::store::Var>,
    /// What the walks a running workflow is inside have their items under,
    /// innermost last, so an inner walk may reuse an outer one's name.
    bindings: Vec<(String, crate::expr::Value)>,
    resolving: std::cell::RefCell<Vec<String>>,
}

impl Snapshot {
    pub fn of(workspace: &Workspace) -> Snapshot {
        Snapshot {
            tables: workspace.jobs.iter().map(|j| Arc::new(table_of(j))).collect(),
            vars: workspace.vars.clone(),
            bindings: Vec::new(),
            resolving: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// The same, for a workflow part-way through a walk.
    pub fn inside(
        workspace: &Workspace,
        bindings: &[(String, crate::expr::Value)],
    ) -> Snapshot {
        Snapshot { bindings: bindings.to_vec(), ..Snapshot::of(workspace) }
    }
}

impl Source for Snapshot {
    fn table(&self, name: &str) -> Option<Arc<Table>> {
        self.tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .or_else(|| self.tables.iter().find(|t| t.tool.eq_ignore_ascii_case(name)))
            .cloned()
    }

    fn tables(&self) -> Vec<Arc<Table>> {
        self.tables.clone()
    }

    fn var(&self, name: &str) -> Option<String> {
        let (_, var) = self.vars.iter().find(|(key, _)| key.eq_ignore_ascii_case(name))?;
        resolve(var, name, self, &self.resolving)
    }

    fn bound(&self, name: &str) -> Option<crate::expr::Value> {
        // Innermost first: a walk inside a walk may use the same name, and
        // the one being walked now is the one meant.
        self.bindings
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }
}

/// What a run looks like to something that only needs to know what it could be
/// asked: its name, its columns, the figures it reported.
///
/// Completion and the workflow's condition controls offer those and never read
/// a single row, so they take this instead of the table, and a scan with tens
/// of thousands of rows costs them nothing.
///
/// What a run answered *is* here, though. Leaving it out was why nothing an
/// HTTP request came back with could be found: completion listed the six
/// columns of the request table and none of the answer, and the workflow's
/// condition controls offered the same six, so the only way to reach a field
/// of the JSON was to already know it was there and type the whole path
/// blind. It costs a clone of a handful of `Arc`s, since the run worked it
/// out when the answer arrived.
pub fn shapes(workspace: &Workspace) -> Vec<Table> {
    workspace
        .jobs
        .iter()
        .map(|job| Table {
            name: job.name(),
            tool: job.tool.id().to_string(),
            target: job.target(),
            state: job.state.label().to_lowercase(),
            columns: job.tool.columns().iter().map(|c| c.title.to_string()).collect(),
            stats: job.stats.iter().map(|kv| (kv.k.clone(), kv.v.clone())).collect(),
            elapsed: job.elapsed().map(|d| d.as_secs_f64()),
            extras: extras_of(job),
            ..Table::default()
        })
        .collect()
}

fn table_of(job: &Job) -> Table {
    Table {
        name: job.name(),
        tool: job.tool.id().to_string(),
        target: job.target(),
        state: job.state.label().to_lowercase(),
        columns: job.tool.columns().iter().map(|c| c.title.to_string()).collect(),
        rows: job.rows.iter().map(|r| r.cells.clone()).collect(),
        statuses: job
            .rows
            .iter()
            .map(|r| {
                match r.status {
                    crate::core::Status::Up => "up",
                    crate::core::Status::Down => "down",
                    crate::core::Status::Warn => "warn",
                    crate::core::Status::Info => "info",
                }
                .to_string()
            })
            .collect(),
        stats: job.stats.iter().map(|kv| (kv.k.clone(), kv.v.clone())).collect(),
        elapsed: job.elapsed().map(|d| d.as_secs_f64()),
        extras: extras_of(job),
    }
}

/// The structured values a run publishes beside its table.
///
/// Only the tools that receive a whole answer have any, and today that is the
/// HTTP request. `status` is deliberately not among them: the table already
/// has a STATUS column, and a name that means one thing at the top level and
/// another inside `response` would be worse than a longer name.
fn extras_of(job: &Job) -> Vec<(String, crate::expr::Value)> {
    job.extras.clone()
}

/// The same, worked out from the answer itself.
///
/// Called once when an answer arrives and kept on the run, not worked out
/// where it is read. The body is parsed here, and the side bar, completion
/// and a running workflow all read these on every frame: parsing a megabyte
/// of JSON that often is the difference between a window that draws and one
/// that does not. Every value below an `Object` is behind an `Arc`, so
/// handing out a copy costs a pointer.
pub fn extras_for(answer: &crate::core::Answer) -> Vec<(String, crate::expr::Value)> {
    use crate::expr::Value;

    let headers = Value::Object(std::sync::Arc::new(
        answer.headers.iter().map(|(k, v)| (k.clone(), Value::Text(v.clone()))).collect(),
    ));
    // Parsed once, here, rather than every time a document is drawn.
    let json = Value::from_json(&answer.body).unwrap_or(Value::Nothing);

    let response = Value::Object(std::sync::Arc::new(vec![
        ("status".to_string(), Value::Number(f64::from(answer.status))),
        ("reason".to_string(), Value::Text(answer.reason.clone())),
        ("type".to_string(), Value::Text(answer.content_type.clone())),
        ("headers".to_string(), headers.clone()),
        ("body".to_string(), Value::Text(answer.body.clone())),
        ("json".to_string(), json.clone()),
        ("truncated".to_string(), Value::Bool(answer.truncated)),
    ]));

    vec![
        ("json".to_string(), json),
        ("body".to_string(), Value::Text(answer.body.clone())),
        ("headers".to_string(), headers),
        ("response".to_string(), response),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(source: &str) -> Doc {
        Doc {
            id: 1,
            stem: "notes".into(),
            path: PathBuf::from("/tmp/ntls-doc-test/notes.md"),
            source: source.into(),
            open: true,
            folder: None,
            seen: None,
            scroll: gpui::ScrollHandle::new(),
            preview: crate::ui::notes::PREVIEW,
        }
    }

    #[test]
    fn a_formula_is_worked_out_when_it_is_read() {
        let mut ws = Workspace::new(1, PathBuf::from("/tmp/ntls-var-test"));
        ws.put_var("subnet", crate::ui::store::Var::text("10.0.0.0/24"));
        ws.put_var(
            "label",
            crate::ui::store::Var { value: "\"net \" + subnet".into(), formula: true, ..Default::default() },
        );
        let data = Data::of(&ws);
        assert_eq!(Source::var(&data, "subnet").as_deref(), Some("10.0.0.0/24"));
        assert_eq!(Source::var(&data, "label").as_deref(), Some("net 10.0.0.0/24"));
        // However it is capitalised.
        assert_eq!(Source::var(&data, "LABEL").as_deref(), Some("net 10.0.0.0/24"));
    }

    #[test]
    fn a_formula_that_names_itself_answers_rather_than_going_round_for_ever() {
        let mut ws = Workspace::new(1, PathBuf::from("/tmp/ntls-var-test"));
        ws.put_var(
            "loop",
            crate::ui::store::Var { value: "loop + 1".into(), formula: true, ..Default::default() },
        );
        ws.put_var(
            "there",
            crate::ui::store::Var { value: "back".into(), formula: true, ..Default::default() },
        );
        ws.put_var(
            "back",
            crate::ui::store::Var { value: "there".into(), formula: true, ..Default::default() },
        );
        let data = Data::of(&ws);
        // Nothing, the same as every other unanswerable expression.
        assert_eq!(Source::var(&data, "loop").as_deref(), Some("1"));
        assert_eq!(Source::var(&data, "there").as_deref(), Some(""));
    }

    #[test]
    fn a_document_is_named_by_its_first_heading() {
        let none = crate::expr::eval::Empty;
        assert_eq!(doc("# Office survey\n\nbody").title(&none), "Office survey");
        // Failing that, by its file name.
        assert_eq!(doc("no heading here").title(&none), "notes");
        assert_eq!(doc("").title(&none), "notes");
        // A computed heading is worked out before it is used as a name.
        assert_eq!(doc("# Link to {{ 1 + 1 }}").title(&none), "Link to 2");
    }

    #[test]
    fn the_summary_is_the_first_line_that_is_not_the_title() {
        let none = crate::expr::eval::Empty;
        assert_eq!(doc("# Title\n\nThe first line.").summary(&none), "The first line.");
        assert_eq!(doc("Just a line").summary(&none), "Just a line");
        // Markers and emphasis are not part of a one-line summary.
        assert_eq!(doc("# T\n\n- a **point**").summary(&none), "a point");
    }

    #[test]
    fn a_new_document_does_not_overwrite_one_that_is_there() {
        let dir = std::env::temp_dir().join("ntls-doc-create-test");
        std::fs::remove_dir_all(&dir).ok();

        let first = create(&dir, "Survey").expect("the first file");
        let second = create(&dir, "Survey").expect("the second file");
        assert_ne!(first, second);
        assert!(first.exists() && second.exists());
        assert_eq!(load(&dir, &mut 1).len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_new_document_is_a_blank_page() {
        let dir = std::env::temp_dir().join("ntls-doc-blank-test");
        std::fs::remove_dir_all(&dir).ok();
        let path = create(&dir, "Survey").expect("the file");
        assert_eq!(std::fs::read_to_string(&path).expect("it to be readable"), "");
        std::fs::remove_dir_all(&dir).ok();
    }
}
