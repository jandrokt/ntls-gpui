//! One open job: its inputs, its output, and its lifecycle.
//!
//! Every job runs in its own tab and keeps running whether or not you are
//! looking at it, so a slow subnet sweep can carry on while you port-scan
//! something it already found.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Entity, ScrollHandle, Task, UniformListScrollHandle};

use crate::core::{Cancel, Event, Kv, Level, Params, Role, Row, Status, Tool};

use super::store::{JobRecord, SavedLog, SavedSeries, Tag};
use super::text_input::TextInput;

/// What a job is doing right now.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum State {
    /// Not started: the form is showing and waiting for a Run.
    Setup,
    Running,
    Done,
    Stopped,
    Failed(String),
}

impl State {
    pub fn label(&self) -> &'static str {
        match self {
            State::Setup => "ready",
            State::Running => "running",
            State::Done => "done",
            State::Stopped => "stopped",
            State::Failed(_) => "failed",
        }
    }

    pub fn status(&self) -> Status {
        match self {
            State::Setup => Status::Info,
            State::Running => Status::Info,
            State::Done => Status::Up,
            State::Stopped => Status::Warn,
            State::Failed(_) => Status::Down,
        }
    }

    pub fn is_running(&self) -> bool {
        *self == State::Running
    }
}

/// A line in the log pane.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub level: Level,
    pub text: String,
    pub at: Duration,
}

/// A live chart: one named series of observations.
#[derive(Clone, Debug)]
pub struct Series {
    pub name: String,
    pub unit: String,
    pub values: Vec<f64>,
}

/// How many samples a chart keeps. Past this the oldest fall off the left,
/// the way a live graph should.
const MAX_SAMPLES: usize = 1200;

/// How many log lines are kept. A wide scan can log thousands and only the
/// recent ones are ever read.
const MAX_LOG: usize = 2000;

/// One end of a table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum End {
    Top,
    Bottom,
}

/// A message from the running tool, or the news that it has stopped.
pub enum Msg {
    Event(Event),
    Finished(Result<(), String>),
}

pub struct Job {
    pub id: usize,
    pub tool: Arc<dyn Tool>,
    pub params: Params,
    pub state: State,

    /// The name the user gave this run, so two port scans in one workspace can
    /// be told apart by what they are for instead of by what they are.
    pub title: Option<String>,
    /// A Finder-style colour, for the ones worth spotting at a glance.
    pub tag: Tag,
    pub favorite: bool,
    /// The file this job is written to, inside its workspace's directory.
    pub stem: String,

    pub rows: Vec<Row>,
    pub log: Vec<LogLine>,
    pub stats: Vec<Kv>,
    pub progress: Option<(usize, usize, Option<String>)>,
    pub charts: Vec<Series>,
    /// The last whole answer the tool received, for the tools that get one.
    /// The response pane reads it, and so does any document that asks about
    /// a part of it.
    pub answer: Option<crate::core::Answer>,
    /// What that answer looks like to an expression: `json`, `body`,
    /// `headers` and `response`, worked out once when the answer lands.
    ///
    /// Derived from `answer` and never written to disk. It is read wherever
    /// an expression names this run — a document being drawn, the completion
    /// list, a workflow deciding its next step — and those all happen on the
    /// thread that draws the window, often every frame.
    pub extras: Vec<(String, crate::expr::Value)>,

    /// The widest cell seen in each column, in characters. Column widths are
    /// derived from this so a column never reserves room for text that is
    /// never there. Once the user drags one, their width wins.
    pub content_width: Vec<usize>,
    /// Widths the user set by dragging, in pixels.
    pub column_override: Vec<Option<f32>>,

    pub started: Option<Instant>,
    pub finished: Option<Instant>,

    // --- interaction ------------------------------------------------------
    /// The form's editors, one per field, in field order.
    pub inputs: Vec<Option<Entity<TextInput>>>,
    /// The form's body editors, one per field in field order, and `None` for
    /// every field that is not a body. Kept apart from `inputs` because a
    /// body is a different widget: several lines, coloured, and with no file
    /// behind it.
    pub bodies: Vec<Option<Entity<super::editor::Editor>>>,
    pub filter_input: Entity<TextInput>,
    pub filter_revision: usize,
    pub filter: String,
    /// The column to sort on and whether it is descending. `None` keeps
    /// arrival order, usually what you want while a scan is running.
    pub sort: Option<(usize, bool)>,
    pub selected: Option<usize>,
    /// Whether it has a tab in the editor. Closing the tab clears this; it is
    /// still in the workspace, and clicking it in the side bar brings it back.
    pub open: bool,
    /// The folder inside the workspace its file sits in, if any. A folder is
    /// a way of grouping; the stage the tool is at still orders it, inside the
    /// folder and not across the workspace.
    pub folder: Option<String>,
    /// Another run of the same tool this one is being read against. Runtime
    /// only: a comparison is a way of looking, not part of the result.
    pub compare_with: Option<usize>,
    pub show_form: bool,
    /// Whether the answer takes the pane. Only the tools that receive a whole
    /// answer have one to show, and it starts hidden: the table is what a run
    /// is normally read from, and the body is what you open when the table
    /// has told you something is wrong.
    pub show_response: bool,
    /// When the answer was last copied, so the button can say it did
    /// something. A control that gives no sign of having worked reads as a
    /// control that did not.
    pub copied_at: Option<std::time::Instant>,
    /// Which half of the answer the response pane is showing, and whether
    /// the body is laid out or left exactly as it arrived.
    pub show_headers: bool,
    pub raw_body: bool,
    /// Whether the settings most runs never touch are unfolded.
    pub more_open: bool,
    /// Whether the graph takes the whole pane. It is always drawn when there
    /// is anything to draw; the button decides how much room it gets.
    pub chart_expanded: bool,
    pub field_error: Option<(usize, String)>,

    pub table_scroll: UniformListScrollHandle,
    /// How many lines the table had when it was last painted, so a run that
    /// has just added some can be followed.
    shown: usize,
    pub log_scroll: ScrollHandle,
    /// Whether the log should stick to the newest line.
    pub log_follow: bool,

    /// The row indices that survive the filter, in display order. Rebuilt when
    /// rows, filter or sort change.
    view: Vec<usize>,
    view_dirty: bool,

    pub cancel: Option<Cancel>,
    /// The pump feeding events into this job. Dropping it stops the pump.
    pub pump: Option<Task<()>>,
    /// How long the run took, for a job read back from disk.
    restored_elapsed: Option<Duration>,
}

impl Job {
    pub fn new(
        id: usize,
        stem: String,
        tool: Arc<dyn Tool>,
        params: Params,
        inputs: Vec<Option<Entity<TextInput>>>,
        bodies: Vec<Option<Entity<super::editor::Editor>>>,
        filter_input: Entity<TextInput>,
    ) -> Job {
        let columns = tool.columns().len();
        Job {
            id,
            tool,
            params,
            state: State::Setup,
            title: None,
            tag: Tag::default(),
            favorite: false,
            stem,
            rows: Vec::new(),
            log: Vec::new(),
            stats: Vec::new(),
            progress: None,
            charts: Vec::new(),
            answer: None,
            more_open: false,
            copied_at: None,
            show_headers: false,
            raw_body: false,
            extras: Vec::new(),
            content_width: vec![0; columns],
            column_override: vec![None; columns],
            started: None,
            finished: None,
            inputs,
            bodies,
            filter_input,
            filter_revision: 0,
            filter: String::new(),
            sort: None,
            selected: None,
            open: true,
            folder: None,
            compare_with: None,
            show_form: true,
            show_response: false,
            chart_expanded: false,
            field_error: None,
            table_scroll: UniformListScrollHandle::new(),
            shown: 0,
            log_scroll: ScrollHandle::new(),
            log_follow: true,
            view: Vec::new(),
            view_dirty: true,
            cancel: None,
            pump: None,
            restored_elapsed: None,
        }
    }

    /// What this job is called: the user's name for it, or the tool's.
    pub fn name(&self) -> String {
        self.title.clone().unwrap_or_else(|| self.tool.title().to_string())
    }

    /// Everything worth writing to disk.
    pub fn record(&self) -> JobRecord {
        JobRecord {
            tool: self.tool.id().to_string(),
            title: self.title.clone(),
            tag: self.tag,
            favorite: self.favorite,
            open: self.open,
            params: self.params.clone(),
            state: super::store::saved_state(&self.state),
            elapsed: self.elapsed(),
            stats: self.stats.clone(),
            rows: self.rows.clone(),
            log: self
                .log
                .iter()
                .map(|l| SavedLog { level: l.level, text: l.text.clone(), at: l.at })
                .collect(),
            charts: self
                .charts
                .iter()
                .map(|s| SavedSeries {
                    name: s.name.clone(),
                    unit: s.unit.clone(),
                    values: s.values.clone(),
                })
                .collect(),
            answer: self.answer.clone(),
        }
    }

    /// Puts a saved run back the way it was left.
    pub fn restore(&mut self, record: &JobRecord) {
        self.title = record.title.clone();
        self.tag = record.tag;
        self.favorite = record.favorite;
        self.open = record.open;
        self.state = super::store::live_state(&record.state);
        self.stats = record.stats.clone();
        self.rows = record.rows.clone();
        self.log = record
            .log
            .iter()
            .map(|l| LogLine { level: l.level, text: l.text.clone(), at: l.at })
            .collect();
        self.charts = record
            .charts
            .iter()
            .map(|s| Series { name: s.name.clone(), unit: s.unit.clone(), values: s.values.clone() })
            .collect();
        self.answer = record.answer.clone();
        self.extras =
            self.answer.as_ref().map(crate::ui::notes::extras_for).unwrap_or_default();

        self.content_width = vec![0; self.tool.columns().len()];
        for row in &self.rows {
            for (i, cell) in row.cells.iter().enumerate() {
                if i < self.content_width.len() {
                    self.content_width[i] = self.content_width[i].max(cell.chars().count());
                }
            }
        }
        // A restored run has results to read, so the form starts out of the
        // way, not covering them.
        self.show_form = self.state == State::Setup;
        self.restored_elapsed = record.elapsed;
        self.view_dirty = true;
    }

    /// What the tab strip and the hub show next to the tool's name.
    ///
    /// A target field the tool is currently hiding does not describe this job
    /// The speed test against Cloudflare is not a job "about" whatever is in
    /// the custom URL box, so the nearest visible choice stands in instead.
    pub fn target(&self) -> String {
        let fields = self.tool.fields();
        if let Some(f) = fields.iter().find(|f| f.role == Role::Target)
            && f.visible(&self.params)
        {
            let value = self.params.str(f.key);
            if !value.is_empty() {
                return shorten(&value);
            }
        }
        // Fall back to whichever select says most about what this job is:
        // the speed test's server, the throughput test's role.
        fields
            .iter()
            .find(|f| f.kind == crate::core::FieldKind::Select && f.visible(&self.params))
            .map(|f| {
                let current = self.params.str(f.key);
                f.options
                    .iter()
                    .find(|o| o.value == current)
                    .map(|o| o.label.clone())
                    .unwrap_or(current)
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "—".into())
    }

    pub fn elapsed(&self) -> Option<Duration> {
        match self.started {
            Some(start) => Some(self.finished.unwrap_or_else(Instant::now).duration_since(start)),
            // A run loaded from disk has no Instant to measure from, but it
            // still knows how long it took.
            None => self.restored_elapsed,
        }
    }

    /// Clears what a previous run said about itself while leaving the table
    /// it built, for a run that is adding to it, not replacing it.
    ///
    /// The log, the figures and the graph all describe one run and would read
    /// as nonsense spliced together; the rows are a record of what is out
    /// there, and that is exactly what is worth keeping.
    pub fn keep_rows(&mut self) {
        self.log.clear();
        self.stats.clear();
        self.charts.clear();
        self.progress = None;
        self.field_error = None;
    }

    /// Clears everything a previous run produced, so a re-run starts from a
    /// clean table and does not append to the old one.
    pub fn reset_output(&mut self) {
        self.rows.clear();
        self.log.clear();
        self.stats.clear();
        self.charts.clear();
        self.progress = None;
        self.selected = None;
        self.content_width = vec![0; self.tool.columns().len()];
        self.field_error = None;
        self.set_filter(String::new());
    }

    /// Widens the remembered content width to fit a row, which the
    /// table sizes its columns from.
    fn widen(&mut self, row: &Row) {
        for (i, cell) in row.cells.iter().enumerate() {
            if i < self.content_width.len() {
                self.content_width[i] = self.content_width[i].max(cell.chars().count());
            }
        }
    }

    /// Folds one event into the job's state.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Log { level, text } => {
                let at = self.started.map(|s| s.elapsed()).unwrap_or_default();
                self.log.push(LogLine { level, text, at });
                if self.log.len() > MAX_LOG {
                    self.log.drain(..self.log.len() - MAX_LOG);
                }
            }
            Event::Row(row) => {
                self.widen(&row);
                self.rows.push(row);
                self.view_dirty = true;
            }
            Event::Upsert(row) => {
                self.widen(&row);
                if upsert(&mut self.rows, row) {
                    self.view_dirty = true;
                }
            }
            Event::Stats(stats) => self.stats = stats,
            // The newest answer replaces the one before it: a run that asks
            // the same thing sixty times is asking about the last reply, and
            // keeping all sixty would be keeping sixty bodies in memory.
            Event::Answered(answer) => {
                self.extras = crate::ui::notes::extras_for(&answer);
                self.answer = Some(answer);
            }
            Event::Progress { done, total, label } => self.progress = Some((done, total, label)),
            Event::Sample { series, unit, value } => {
                let s = match self.charts.iter_mut().find(|s| s.name == series) {
                    Some(s) => s,
                    None => {
                        self.charts.push(Series { name: series, unit, values: Vec::new() });
                        self.charts.last_mut().expect("just pushed")
                    }
                };
                s.values.push(value);
                if s.values.len() > MAX_SAMPLES {
                    s.values.remove(0);
                }
            }
        }
    }

    pub fn set_filter(&mut self, filter: String) {
        if self.filter != filter {
            self.filter = filter;
            self.view_dirty = true;
            self.selected = None;
        }
    }

    pub fn set_sort(&mut self, column: usize) {
        self.sort = match self.sort {
            Some((c, false)) if c == column => Some((c, true)),
            Some((c, true)) if c == column => None,
            _ => Some((column, false)),
        };
        self.view_dirty = true;
    }

    /// The row indices to show, filtered and sorted.
    pub fn view(&mut self) -> &[usize] {
        if self.view_dirty {
            self.rebuild_view();
            self.view_dirty = false;
        }
        &self.view
    }

    pub fn view_len(&mut self) -> usize {
        self.view().len()
    }

    fn rebuild_view(&mut self) {
        let needle = self.filter.trim().to_lowercase();
        self.view = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                needle.is_empty()
                    || r.cells.iter().any(|c| c.to_lowercase().contains(&needle))
                    || r.target.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect();

        if let Some((col, desc)) = self.sort {
            let rows = &self.rows;
            self.view.sort_by(|&a, &b| {
                let (x, y) = (cell(rows, a, col), cell(rows, b, col));
                let ord = compare(x, y);
                if desc { ord.reverse() } else { ord }
            });
        }
    }

    /// Which end of the table is being read, when it is one of them.
    ///
    /// A scan that is still going adds lines while you are reading, and where
    /// they appear depends on how the table is sorted. Staying put at either
    /// end is what makes a live table readable: at the bottom you watch what
    /// arrives, at the top you watch what arrives, and anywhere in between you
    /// are reading something and nothing should move.
    pub fn end_in_view(&self) -> Option<End> {
        let state = self.table_scroll.0.borrow();
        let offset = state.base_handle.offset().y;
        let max = state.base_handle.max_offset().height;
        // Nothing to scroll: both ends are in view, and the bottom is the one
        // worth following.
        if max <= gpui::px(1.) {
            return Some(End::Bottom);
        }
        let from_top = -offset;
        if from_top <= gpui::px(2.) {
            return Some(End::Top);
        }
        if from_top >= max - gpui::px(2.) {
            return Some(End::Bottom);
        }
        None
    }

    /// Follows the end that was in view, if the table has grown since the last
    /// frame.
    pub fn follow(&mut self, end: Option<End>) {
        let lines = self.view_len();
        let grew = lines > self.shown;
        self.shown = lines;
        if !grew || lines == 0 {
            return;
        }
        match end {
            Some(End::Bottom) => {
                self.table_scroll.scroll_to_item(lines - 1, gpui::ScrollStrategy::Bottom)
            }
            Some(End::Top) => self.table_scroll.scroll_to_item(0, gpui::ScrollStrategy::Top),
            None => {}
        }
    }

    /// The row the selection points at, if any.
    pub fn selected_row(&mut self) -> Option<&Row> {
        let selected = self.selected?;
        let index = *self.view().get(selected)?;
        self.rows.get(index)
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.view_len();
        if len == 0 {
            self.selected = None;
            return;
        }
        let next = match self.selected {
            None if delta > 0 => 0,
            None => len - 1,
            Some(i) => (i as isize + delta).clamp(0, len as isize - 1) as usize,
        };
        self.selected = Some(next);
        self.table_scroll.scroll_to_item(next, gpui::ScrollStrategy::Center);
    }

    pub fn select_index(&mut self, index: usize) {
        if index < self.view_len() {
            self.selected = Some(index);
        }
    }

    /// A one-line digest of what this job is doing, for the tab strip and the
    /// hub. Built from what the tool already emits, so a new tool gets one
    /// without doing anything for it.
    pub fn digest(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some((done, total, label)) = &self.progress
            && *total > 0
        {
            parts.push(match label {
                Some(l) => l.clone(),
                None => format!("{}%", (*done * 100 / (*total).max(1)).min(100)),
            });
        }
        for kv in self.stats.iter().filter(|kv| !kv.k.is_empty()).take(2) {
            parts.push(format!("{} {}", kv.k, kv.v));
        }
        if parts.is_empty() && !self.rows.is_empty() {
            parts.push(format!("{} result(s)", self.rows.len()));
        }
        parts.join("  ·  ")
    }

    /// How many log lines were good, worrying and wrong, which the
    /// output panel puts in its header.
    pub fn log_counts(&self) -> (usize, usize, usize) {
        let count = |want: Level| self.log.iter().filter(|l| l.level == want).count();
        (count(Level::Good), count(Level::Warn), count(Level::Error))
    }

    /// The most recent chart samples, for the sparkline in the side bar.
    pub fn spark(&self, width: usize) -> Option<&[f64]> {
        let s = self.charts.first()?;
        let n = s.values.len().min(width);
        (n > 0).then(|| &s.values[s.values.len() - n..])
    }
}

/// What a target looks like in a tab, a breadcrumb or a workspace name.
///
/// A host or a subnet is already short. A pasted link is not, and a whole URL
/// in a tab pushes everything else off the strip, so it is shown as the site
/// it points at, and a list of links as how many there are.
fn shorten(target: &str) -> String {
    let links: Vec<&str> = target.split_whitespace().filter(|s| !s.is_empty()).collect();
    if links.len() > 1 && links.iter().all(|l| l.contains("://") || l.contains('.')) {
        return format!("{} links", links.len());
    }
    let Some(rest) = target.split_once("://").map(|(_, rest)| rest) else {
        return target.to_string();
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    match host.strip_prefix("www.") {
        Some(bare) if !bare.is_empty() => bare.to_string(),
        _ if host.is_empty() => target.to_string(),
        _ => host.to_string(),
    }
}

/// Where a new row about `target` belongs.
///
/// A second row about something the table already has a row about goes next to
/// it instead of at the bottom: the two devices that have answered on one
/// address are only worth keeping both of if they can be read against each
/// other. Anything genuinely new goes at the end, in arrival order.
fn place(rows: &[Row], target: &str) -> usize {
    match rows.iter().rposition(|r| r.target == target) {
        Some(beside) => beside + 1,
        None => rows.len(),
    }
}

/// Folds an upserted row into the table, saying whether the filtered, sorted
/// view has to be built again.
///
/// What counts as the same row is the row's own idea of its identity: its
/// target, unless it says otherwise.
///
/// A rewrite in place counts as much as an arrival. The view is built from the
/// cells and the target and from nothing else, so rewriting either of them
/// changes which rows belong in it and what order they come in; treating a
/// rewrite as invisible left a filtered table showing rows whose new contents
/// no longer match, counting them in the header, and never showing the ones
/// that have only just come to match. A transfer that failed under a "failed"
/// filter never appeared, and a sweep's closing pass that dates every row it
/// did not hear from this time reached nobody who had typed in the box, because
/// in both of those tables nothing else ever happens to make the view stale.
fn upsert(rows: &mut Vec<Row>, row: Row) -> bool {
    match rows.iter_mut().find(|r| r.identity() == row.identity()) {
        Some(existing) => {
            let changed = existing.cells != row.cells || existing.target != row.target;
            // The note on a row belongs to whoever wrote it, not to the tool.
            // Every upsert arrives with none, so replacing the row wholesale
            // threw away what somebody had typed about that host the moment
            // the scan reported it again.
            let note = existing.note.take();
            *existing = row;
            if existing.note.is_none() {
                existing.note = note;
            }
            changed
        }
        None => {
            let at = place(rows, &row.target);
            rows.insert(at, row);
            true
        }
    }
}

fn cell(rows: &[Row], index: usize, column: usize) -> &str {
    rows.get(index).and_then(|r| r.cells.get(column)).map(String::as_str).unwrap_or("")
}

/// Where one cell sits in the order.
///
/// Worked out once for the cell, and not afresh for each pair it is compared
/// with. That distinction is the whole point: choosing how to compare from
/// what the two cells happened to be meant the answer depended on the pair,
/// and an order that depends on the pair is not an order at all. A DNS column
/// holding `5 alt1.aspmx.l.google.com.`, `10 mail.example.com.` and
/// `3.4.5.6` gave the first below the second by their figures, the second
/// below the third as text, and the third below the first as text. Sorting a
/// cycle is what the standard library aborts the program over, so one click
/// on that column header took the window and every run in it.
enum Key {
    /// A cell that begins with a figure, by that figure.
    Num(f64),
    /// An address, by its parts.
    Ip(std::net::IpAddr),
    /// Anything else, as text.
    Text(String),
}

impl Key {
    /// Which kind of cell this is, so that unlike kinds still have an order.
    fn rank(&self) -> u8 {
        match self {
            Key::Num(_) => 0,
            Key::Ip(_) => 1,
            Key::Text(_) => 2,
        }
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Key) -> std::cmp::Ordering {
        match (self, other) {
            // `total_cmp` and not `partial_cmp`: it is an order over every
            // `f64` there is, which is what makes this whole thing total.
            (Key::Num(x), Key::Num(y)) => x.total_cmp(y),
            (Key::Ip(x), Key::Ip(y)) => x.cmp(y),
            (Key::Text(x), Key::Text(y)) => x.cmp(y),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Key) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for Key {}

/// Reads a cell the way a person reads it: a number as a number, an address
/// by its parts, anything else as text.
fn sort_key(s: &str) -> Key {
    if let Some(n) = leading_number(s) {
        return Key::Num(n);
    }
    if let Ok(ip) = s.parse::<std::net::IpAddr>() {
        return Key::Ip(ip);
    }
    Key::Text(s.to_lowercase())
}

/// Compares two cells. Sorting "10" after "9" is the whole point of a
/// sortable port column.
fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    sort_key(a).cmp(&sort_key(b))
}

/// The number a cell starts with, so "18.12 ms" and "443/tcp" both sort by
/// their leading figure.
fn leading_number(s: &str) -> Option<f64> {
    let end = s
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == '-'))
        .map_or(s.len(), |(i, _)| i);
    let head = &s[..end];
    if head.is_empty() || head == "-" || head == "." {
        return None;
    }
    // An address is not a number, however much its first label looks like one.
    if head.matches('.').count() > 1 {
        return None;
    }
    head.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{Row, compare, place, shorten, upsert};

    #[test]
    fn a_column_of_mixed_cells_has_an_order_and_does_not_abort() {
        // These three used to make a cycle: the first below the second by
        // their figures, the second below the third as text, the third below
        // the first as text. Sorting a cycle is what the standard library
        // ends the program over.
        let mut values = vec![
            "5 alt1.aspmx.l.google.com.",
            "3.4.5.6",
            "10 mail.example.com.",
            "1.2.3.4",
            "alpha",
            "",
            "-",
            "9",
            "10",
            "192.168.1.20",
        ];
        // Every pair agrees with itself and with the reverse of its opposite.
        for a in &values {
            for b in &values {
                assert_eq!(compare(a, b), compare(b, a).reverse(), "{a:?} vs {b:?}");
            }
        }
        // And the order is transitive, which is what was actually broken.
        for a in &values {
            for b in &values {
                for c in &values {
                    if compare(a, b).is_lt() && compare(b, c).is_lt() {
                        assert!(compare(a, c).is_lt(), "{a:?} < {b:?} < {c:?}");
                    }
                }
            }
        }
        // Sorting it is what would previously abort.
        values.sort_by(|a, b| compare(a, b));
        // A figure still sorts as a figure, which is the point of the column.
        let ports = &mut ["10", "9", "100", "2"];
        ports.sort_by(|a, b| compare(a, b));
        assert_eq!(ports, &["2", "9", "10", "100"]);
    }


    fn row(target: &str) -> Row {
        Row { target: target.into(), ..Row::default() }
    }

    fn row_with(target: &str, cells: &[&str]) -> Row {
        Row {
            target: target.into(),
            cells: cells.iter().map(|c| c.to_string()).collect(),
            ..Row::default()
        }
    }

    #[test]
    fn a_row_rewritten_in_place_says_the_filtered_view_has_to_be_built_again() {
        let mut rows = vec![row_with("10.0.0.1", &["10.0.0.1", "queued"])];
        // The same row said again, word for word: there is nothing here for a
        // filter or a sort to change its mind about.
        assert!(!upsert(&mut rows, row_with("10.0.0.1", &["10.0.0.1", "queued"])));
        // The same row with a new cell. A filter reads cells, so whatever the
        // view said about this row before it was rewritten it no longer says.
        assert!(upsert(&mut rows, row_with("10.0.0.1", &["10.0.0.1", "failed"])));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cells[1], "failed");
        // A row nobody has seen before, which always did rebuild the view.
        assert!(upsert(&mut rows, row_with("10.0.0.2", &["10.0.0.2", "queued"])));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn rewriting_a_row_keeps_the_note_somebody_typed_on_it() {
        let mut rows = vec![row_with("10.0.0.1", &["10.0.0.1", "queued"])];
        rows[0].note = Some("the noisy printer".into());
        upsert(&mut rows, row_with("10.0.0.1", &["10.0.0.1", "done"]));
        assert_eq!(rows[0].note.as_deref(), Some("the noisy printer"));
    }

    #[test]
    fn a_second_device_on_one_address_lands_beside_the_first() {
        let rows = vec![row("10.0.0.1"), row("10.0.0.2"), row("10.0.0.3")];
        assert_eq!(place(&rows, "10.0.0.1"), 1);
        // Behind every row already about it, not in the middle of them.
        let two = vec![row("10.0.0.1"), row("10.0.0.1"), row("10.0.0.2")];
        assert_eq!(place(&two, "10.0.0.1"), 2);
        // And something new goes where new things go.
        assert_eq!(place(&rows, "10.0.0.9"), 3);
    }

    #[test]
    fn a_pasted_link_is_shown_as_the_site_it_points_at() {
        assert_eq!(shorten("https://www.mediafire.com/file/abc/thing.rar/file"), "mediafire.com");
        assert_eq!(shorten("http://example.org/a/b"), "example.org");
    }

    #[test]
    fn a_list_of_links_is_shown_as_a_count() {
        assert_eq!(shorten("https://a.example/1 https://b.example/2"), "2 links");
    }

    #[test]
    fn everything_that_is_already_short_is_left_alone() {
        assert_eq!(shorten("192.168.1.0/24"), "192.168.1.0/24");
        assert_eq!(shorten("example.com"), "example.com");
        assert_eq!(shorten("auto"), "auto");
    }
}
