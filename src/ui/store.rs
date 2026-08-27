//! Workspaces on disk.
//!
//! A workspace is a directory and every tool in it is a file, so what the
//! application shows is a view of something you can also open in Finder, back
//! up, or delete by hand. Closing a tool deletes its file; closing a workspace
//! deletes its directory. Everything else — the results, the notes, the names
//! and colours — is written as it changes and read back at startup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::core::{Kv, Level, Params, Row};

/// A colour a workspace or a tool can be tagged with, the way a Finder folder
/// can.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Tag {
    #[default]
    None,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Grey,
}

impl Tag {
    pub const ALL: [Tag; 8] =
        [Tag::None, Tag::Red, Tag::Orange, Tag::Yellow, Tag::Green, Tag::Blue, Tag::Purple, Tag::Grey];

    pub fn label(self) -> &'static str {
        match self {
            Tag::None => "None",
            Tag::Red => "Red",
            Tag::Orange => "Orange",
            Tag::Yellow => "Yellow",
            Tag::Green => "Green",
            Tag::Blue => "Blue",
            Tag::Purple => "Purple",
            Tag::Grey => "Grey",
        }
    }

    /// The swatch, which is also what tints a row in the rail.
    pub fn color(self) -> Option<gpui::Hsla> {
        let rgb = match self {
            Tag::None => return None,
            Tag::Red => 0xff5f57,
            Tag::Orange => 0xff9f43,
            Tag::Yellow => 0xf5c518,
            Tag::Green => 0x3fb950,
            Tag::Blue => 0x5eb3ff,
            Tag::Purple => 0xa970ff,
            Tag::Grey => 0x8b98a8,
        };
        Some(gpui::rgb(rgb).into())
    }
}

/// One tool as it is written to disk.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRecord {
    pub tool: String,
    /// The name the user gave it, when they gave it one.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub tag: Tag,
    #[serde(default)]
    pub favorite: bool,
    /// Whether it had a tab in the editor. A tool with no tab is still in the
    /// workspace and still on disk — it is just not in front of you.
    #[serde(default = "yes")]
    pub open: bool,
    pub params: Params,
    #[serde(default)]
    pub state: SavedState,
    #[serde(default)]
    pub elapsed: Option<Duration>,
    #[serde(default)]
    pub stats: Vec<Kv>,
    #[serde(default)]
    pub rows: Vec<Row>,
    #[serde(default)]
    pub log: Vec<SavedLog>,
    #[serde(default)]
    pub charts: Vec<SavedSeries>,
}

/// What a run had come to when it was last written.
///
/// There is no saved `Running`: a run cannot survive the process that was
/// doing it, so one that was in flight is recorded as interrupted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SavedState {
    #[default]
    NotRun,
    Done,
    Stopped,
    Failed(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedLog {
    pub level: Level,
    pub text: String,
    pub at: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedSeries {
    pub name: String,
    pub unit: String,
    pub values: Vec<f64>,
}

/// A workspace's own file: everything about it that is not a tool.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    /// The name the user typed, empty when the workspace is still named after
    /// whatever it contains. Writing the derived name here would freeze it,
    /// so a workspace that later points somewhere else could never say so.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tag: Tag,
    /// Notes keyed by the thing they are about — an address, a hostname, a
    /// `host:port`. A note follows its subject between tools, so a remark made
    /// while reading a sweep is still there in the port scan.
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
    /// The order tools appear in, by file stem.
    #[serde(default)]
    pub order: Vec<String>,
    /// The workspace's variables: what somebody wrote down, what a workflow
    /// worked out and kept, and what is worked out afresh every time it is
    /// read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, StoredVar>,
    /// What has been said about the documents and workflows, by file stem —
    /// a colour and whether it is a favourite.
    ///
    /// A tool keeps this in its own file; a document is a markdown file and a
    /// workflow is a text file, and neither has anywhere to put it. So the
    /// workspace keeps it, the way it keeps the notes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub marks: BTreeMap<String, Mark>,
}

/// One variable of a workspace.
///
/// Either a piece of text, or a formula — an expression worked out against the
/// runs and the other variables every time it is read. The difference is the
/// difference between a fact somebody wrote down and one that stays current:
/// `set` in a workflow freezes what was true at that step, and a formula does
/// not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Var {
    /// The text, or the expression when this is a formula.
    pub value: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub formula: bool,
    /// What last wrote it, when that was not a person: the workflow's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_by: Option<String>,
    /// And when, so the side bar can say how old an answer is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<std::time::SystemTime>,
    /// What it is for, in the words of whoever made it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
}

impl Var {
    pub fn text(value: impl Into<String>) -> Var {
        Var { value: value.into(), ..Var::default() }
    }
}

/// How a variable is written down.
///
/// The first version of this file wrote a plain string, and a workspace saved
/// by it still opens: a string is read as text with nothing else said about
/// it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StoredVar {
    Text(String),
    Full(Var),
}

impl From<StoredVar> for Var {
    fn from(stored: StoredVar) -> Var {
        match stored {
            StoredVar::Text(value) => Var::text(value),
            StoredVar::Full(var) => var,
        }
    }
}

impl From<Var> for StoredVar {
    fn from(var: Var) -> StoredVar {
        StoredVar::Full(var)
    }
}

/// A colour and a star, for a file that has nowhere to keep them itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    #[serde(default)]
    pub tag: Tag,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub favorite: bool,
}

impl Mark {
    pub fn is_plain(self) -> bool {
        self == Mark::default()
    }
}

/// Rows and log lines are capped so a 65,000-port scan does not write a file
/// nobody can open. What is kept is what fits on a screen many times over.
const MAX_SAVED_ROWS: usize = 50_000;
const MAX_SAVED_LOG: usize = 2_000;

/// Where workspaces live.
///
/// `~/Documents/ntls` rather than a hidden application-support folder,
/// because a workspace being a real directory is only useful if you can find
/// it.
pub fn root() -> PathBuf {
    home().join("Documents").join("ntls")
}

/// This user's home directory, wherever the system keeps it.
pub fn home() -> PathBuf {
    crate::sys::home()
}

pub fn ensure_root() -> std::io::Result<PathBuf> {
    let root = root();
    std::fs::create_dir_all(&root)?;
    Ok(root)
}

/// Turns a name into something a filesystem will accept, without letting it
/// escape the directory it belongs in.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for c in name.chars() {
        let keep = match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' => Some(c),
            ' ' | '-' | '/' | '\\' | ':' => Some('-'),
            _ => None,
        };
        match keep {
            Some('-') if last_dash => {}
            Some(c) => {
                out.push(c);
                last_dash = c == '-';
            }
            None => {}
        }
    }
    // A run of dots is how a path climbs out of its directory. One dot is a
    // legitimate part of a name — `192.168.1.0-24` — so runs are collapsed
    // rather than the character being banned.
    let mut out = out.trim_matches(['-', '.']).to_string();
    while out.contains("..") {
        out = out.replace("..", ".");
    }
    let out = out.trim_matches(['-', '.']).to_string();
    if out.is_empty() { "workspace".into() } else { out.chars().take(64).collect() }
}

/// A directory for this workspace, made unique if the name is taken.
pub fn claim_dir(name: &str, taken: &[PathBuf]) -> PathBuf {
    let root = root();
    let base = slugify(name);
    let mut candidate = root.join(&base);
    let mut n = 2;
    while taken.contains(&candidate) || candidate.exists() {
        candidate = root.join(format!("{base}-{n}"));
        n += 1;
    }
    candidate
}

pub fn write_workspace(dir: &Path, record: &WorkspaceRecord) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    write_json(&dir.join("workspace.json"), record)
}

pub fn read_workspace(dir: &Path) -> Option<WorkspaceRecord> {
    read_json(&dir.join("workspace.json"))
}

/// Where a tool's file lives: in the workspace, or in a folder inside it.
///
/// A folder is one level deep and is only a way of grouping — the stage a tool
/// is at is still what orders it, inside the folder rather than instead of it.
pub fn job_path_in(dir: &Path, folder: Option<&str>, stem: &str) -> PathBuf {
    match folder {
        Some(folder) => {
            let mut target = dir.to_path_buf();
            for part in folder.split('/') {
                let s = slugify(part);
                if !s.is_empty() {
                    target.push(s);
                }
            }
            target.join(format!("{stem}.json"))
        }
        None => job_path(dir, stem),
    }
}

pub fn job_path(dir: &Path, stem: &str) -> PathBuf {
    dir.join(format!("{stem}.json"))
}

pub fn write_job_in(
    dir: &Path,
    folder: Option<&str>,
    stem: &str,
    record: &JobRecord,
) -> std::io::Result<()> {
    let path = job_path_in(dir, folder, stem);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_job_at(&path, record)
}

/// Writes a tool's file, trimming what is too big to be worth keeping.
fn write_job_at(path: &Path, record: &JobRecord) -> std::io::Result<()> {
    let mut trimmed = record.clone();
    if trimmed.rows.len() > MAX_SAVED_ROWS {
        trimmed.rows.drain(..trimmed.rows.len() - MAX_SAVED_ROWS);
    }
    if trimmed.log.len() > MAX_SAVED_LOG {
        trimmed.log.drain(..trimmed.log.len() - MAX_SAVED_LOG);
    }
    write_json(path, &trimmed)
}

/// Retires a tool: its file is moved into the workspace's own `.closed`
/// folder rather than deleted.
///
/// A tool is a file, and a file is not something a click should destroy. It is
/// moved rather than left in place so that reopening the workspace does not
/// bring back everything ever removed from it.
pub fn remove_job_in(dir: &Path, folder: Option<&str>, stem: &str) {
    let path = job_path_in(dir, folder, stem);
    if !path.exists() {
        return;
    }
    let closed = dir.join(".closed");
    if std::fs::create_dir_all(&closed).is_err() {
        return;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut destination = closed.join(format!("{stem}-{stamp}.json"));
    let mut n = 2;
    while destination.exists() {
        destination = closed.join(format!("{stem}-{stamp}-{n}.json"));
        n += 1;
    }
    let _ = std::fs::rename(&path, &destination);
}

/// Retires any other file a workspace holds — a document, a workflow — the
/// same way, and to the same place.
///
/// The same rule as a tool: it is moved into `.closed` rather than deleted,
/// because a file is not something one click should destroy, and reopening the
/// workspace must not bring back everything ever removed from it.
pub fn remove_file(path: &Path, dir: &Path) {
    if !path.exists() {
        return;
    }
    let closed = dir.join(".closed");
    if std::fs::create_dir_all(&closed).is_err() {
        return;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = path.extension().map(|e| e.to_string_lossy().into_owned());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = |suffix: String| match &extension {
        Some(extension) => format!("{stem}-{stamp}{suffix}.{extension}"),
        None => format!("{stem}-{stamp}{suffix}"),
    };
    let mut destination = closed.join(name(String::new()));
    let mut n = 2;
    while destination.exists() {
        destination = closed.join(name(format!("-{n}")));
        n += 1;
    }
    let _ = std::fs::rename(path, &destination);
}

/// A tool written before tabs could be closed was, by definition, open.
fn yes() -> bool {
    true
}

/// Settings that belong to the application rather than to any one workspace.
///
/// It lives in the workspace root as a hidden file, so a workspace directory
/// stays a directory of tools and nothing else.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppState {
    /// Workspace directories that have been closed. They are still on disk and
    /// untouched; they are simply not opened at startup. Opening the folder
    /// again removes it from this list.
    #[serde(default)]
    pub closed: Vec<String>,
    /// The interface new tools start on, when one has been chosen.
    #[serde(default)]
    pub iface: Option<String>,
}

fn state_path() -> PathBuf {
    root().join(".ntls.json")
}

pub fn read_state() -> AppState {
    read_json(&state_path()).unwrap_or_default()
}

pub fn write_state(state: &AppState) {
    let _ = ensure_root();
    let _ = write_json(&state_path(), state);
}

/// Deletes a workspace's directory and everything in it. There is no undo, so
/// the caller is expected to have asked.
pub fn delete_workspace(dir: &Path) -> std::io::Result<()> {
    // Only ever inside our own root, however the name was mangled.
    if !dir.starts_with(root()) || dir == root() {
        return Err(std::io::Error::other("outside the workspace root"));
    }
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(dir)
}

/// A workspace as it was found on disk: where it is, what it is, and the
/// tools inside it keyed by file stem.
pub type Loaded = (PathBuf, WorkspaceRecord, Vec<(Option<String>, String, JobRecord)>);

/// Every workspace directory on disk, oldest first, with its tools.
pub fn load_all() -> Vec<Loaded> {
    let Ok(entries) = std::fs::read_dir(root()) else { return Vec::new() };
    let closed = read_state().closed;

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("workspace.json").exists())
        // A closed workspace is still on disk and untouched; it is simply not
        // opened until it is asked for again.
        .filter(|p| !closed.iter().any(|c| Path::new(c) == p))
        .collect();
    dirs.sort();

    dirs.into_iter()
        .filter_map(|dir| {
            let record = read_workspace(&dir)?;
            let jobs = load_jobs(&dir);
            Some((dir, record, jobs))
        })
        .collect()
}

/// Reads the tools in a workspace directory, including any folders inside it.
///
/// It is separate from [`load_all`] because a directory can be opened as a
/// workspace on its own — including one ntls did not write, which has tool
/// files and no workspace file.
pub fn load_jobs(dir: &Path) -> Vec<(Option<String>, String, JobRecord)> {
    let mut jobs = Vec::new();
    scan_jobs(dir, None, &mut jobs);

    // The workspace file remembers the order; anything it does not mention was
    // added by hand — or dropped in — and goes at the end.
    let order = read_workspace(dir).map(|r| r.order).unwrap_or_default();
    jobs.sort_by_key(|(_, stem, _)| order.iter().position(|s| s == stem).unwrap_or(usize::MAX));
    jobs
}

fn scan_jobs(dir: &Path, rel: Option<String>, jobs: &mut Vec<(Option<String>, String, JobRecord)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                dirs.push(path);
            }
        } else if path.extension().is_some_and(|e| e == "json")
            && path.file_stem().is_some_and(|s| s != "workspace")
        {
            if let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) {
                if let Some(record) = read_json::<JobRecord>(&path) {
                    jobs.push((rel.clone(), stem, record));
                }
            }
        }
    }
    dirs.sort();
    for sub in dirs {
        let name = sub.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let next_rel = match &rel {
            Some(parent) => format!("{parent}/{name}"),
            None => name,
        };
        scan_jobs(&sub, Some(next_rel), jobs);
    }
}

/// The folders a workspace has, in name order.
pub fn folders(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    scan_folder_names(dir, None, &mut names);
    names.sort();
    names
}

fn scan_folder_names(dir: &Path, rel: Option<String>, names: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .collect();
    dirs.sort();
    for sub in dirs {
        let name = sub.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let next_rel = match &rel {
            Some(parent) => format!("{parent}/{name}"),
            None => name,
        };
        names.push(next_rel.clone());
        scan_folder_names(&sub, Some(next_rel), names);
    }
}

/// Reads one tool file, wherever it is.
pub fn read_job(path: &Path) -> Option<JobRecord> {
    read_json::<JobRecord>(path)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    // Write beside the target and rename, so a crash mid-write cannot leave a
    // half-written workspace behind.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Maps a live run state onto what gets written.
pub fn saved_state(state: &crate::ui::job::State) -> SavedState {
    use crate::ui::job::State;
    match state {
        State::Setup => SavedState::NotRun,
        // A run does not outlive the process doing it.
        State::Running | State::Stopped => SavedState::Stopped,
        State::Done => SavedState::Done,
        State::Failed(e) => SavedState::Failed(e.clone()),
    }
}

pub fn live_state(saved: &SavedState) -> crate::ui::job::State {
    use crate::ui::job::State;
    match saved {
        SavedState::NotRun => State::Setup,
        SavedState::Done => State::Done,
        SavedState::Stopped => State::Stopped,
        SavedState::Failed(e) => State::Failed(e.clone()),
    }
}

/// A stable, readable file stem for a tool: its position, then its id.
pub fn job_stem(index: usize, tool: &str) -> String {
    format!("{index:03}-{}", slugify(tool).to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Status;

    #[test]
    fn names_become_safe_directory_names() {
        assert_eq!(slugify("192.168.1.0/24"), "192.168.1.0-24");
        assert_eq!(slugify("My Network!"), "My-Network");
        assert_eq!(slugify("  "), "workspace");
        assert_eq!(slugify("../../etc"), "etc");
        assert_eq!(slugify("a///b"), "a-b");
    }

    #[test]
    fn a_closed_workspace_is_left_on_disk_and_not_opened_again() {
        // Closing takes a workspace out of the application. The directory is
        // untouched, and simply not loaded until it is asked for again.
        let dir = super::root().join("ntls-closed-state-test");
        std::fs::create_dir_all(&dir).expect("the directory");
        std::fs::write(dir.join("workspace.json"), "{}").expect("a file");

        let before = super::read_state();
        let mut state = before.clone();
        state.closed.push(dir.display().to_string());
        super::write_state(&state);

        assert!(dir.exists(), "closing must not touch the directory");
        assert!(
            !super::load_all().iter().any(|(d, _, _)| *d == dir),
            "a closed workspace must not be opened at startup"
        );

        super::write_state(&before);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_removed_tool_is_moved_aside_and_not_destroyed() {
        // A tool is a file, and a click that destroys a file is a click too
        // powerful. Removing one puts it in the workspace's own `.closed`
        // folder, where it is out of the way and still there.
        let dir = super::root().join("ntls-remove-test");
        std::fs::create_dir_all(&dir).expect("the directory");
        std::fs::write(super::job_path(&dir, "001-ping"), "{}").expect("a file");

        super::remove_job_in(&dir, None, "001-ping");
        assert!(!super::job_path(&dir, "001-ping").exists(), "it should not still be open");

        let kept: Vec<_> = std::fs::read_dir(dir.join(".closed"))
            .expect("the closed folder")
            .flatten()
            .collect();
        assert_eq!(kept.len(), 1, "the file should have been kept, not deleted");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_closed_folder_is_not_read_back_as_tools() {
        // Otherwise every tool ever removed would come back with the
        // workspace.
        let dir = super::root().join("ntls-closed-test");
        std::fs::create_dir_all(dir.join(".closed")).expect("the directories");
        std::fs::write(dir.join("workspace.json"), "{}").ok();
        std::fs::write(dir.join(".closed/001-ping-123.json"), "{}").ok();

        assert!(super::load_jobs(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_slug_cannot_escape_its_directory() {
        for hostile in ["../../../etc/passwd", "..", "/absolute", "a/../../b"] {
            let slug = slugify(hostile);
            assert!(!slug.contains('/'), "{hostile} -> {slug}");
            assert!(!slug.contains(".."), "{hostile} -> {slug}");
        }
    }

    #[test]
    fn a_run_in_flight_is_saved_as_interrupted() {
        use crate::ui::job::State;
        assert_eq!(saved_state(&State::Running), SavedState::Stopped);
        assert_eq!(saved_state(&State::Setup), SavedState::NotRun);
        assert_eq!(live_state(&SavedState::NotRun), State::Setup);
        assert_eq!(
            live_state(&SavedState::Failed("boom".into())),
            State::Failed("boom".into())
        );
    }

    #[test]
    fn job_stems_sort_in_the_order_they_were_added() {
        let mut stems = vec![job_stem(10, "ping"), job_stem(2, "dns"), job_stem(1, "ipscan")];
        stems.sort();
        assert_eq!(stems, ["001-ipscan", "002-dns", "010-ping"]);
    }

    #[test]
    fn a_record_survives_a_round_trip() {
        let record = JobRecord {
            tool: "ping".into(),
            title: Some("the router".into()),
            tag: Tag::Blue,
            favorite: true,
            open: true,
            params: Params::new(),
            state: SavedState::Done,
            elapsed: Some(Duration::from_millis(1500)),
            stats: vec![crate::core::kv("sent", "4")],
            rows: vec![Row { cells: vec!["1".into()], status: Status::Up, target: "1.1.1.1".into(), note: None, key: None }],
            log: vec![SavedLog { level: Level::Good, text: "done".into(), at: Duration::ZERO }],
            charts: vec![SavedSeries { name: "rtt".into(), unit: " ms".into(), values: vec![1.0] }],
        };
        let json = serde_json::to_vec(&record).unwrap();
        let back: JobRecord = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.tool, "ping");
        assert_eq!(back.title.as_deref(), Some("the router"));
        assert_eq!(back.tag, Tag::Blue);
        assert!(back.favorite);
        assert_eq!(back.rows.len(), 1);
        assert_eq!(back.rows[0].target, "1.1.1.1");
        assert_eq!(back.charts[0].values, vec![1.0]);
    }

    #[test]
    fn an_old_file_missing_new_fields_still_loads() {
        // Everything added after the first release is `#[serde(default)]`, so
        // a workspace written by an earlier build keeps working.
        let json = br#"{"tool":"dns","params":{"target":"example.com"}}"#;
        let record: JobRecord = serde_json::from_slice(json).unwrap();
        assert_eq!(record.tool, "dns");
        assert_eq!(record.state, SavedState::NotRun);
        assert!(!record.favorite);
        assert_eq!(record.tag, Tag::None);
    }
}
