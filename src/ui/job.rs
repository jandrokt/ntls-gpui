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
/// which is what a live graph should do.
const MAX_SAMPLES: usize = 1200;

/// How many log lines are kept. A wide scan can log thousands and only the
/// recent ones are ever read.
const MAX_LOG: usize = 2000;

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
    /// be told apart by what they are for rather than by what they are.
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

    /// The widest cell seen in each column, in characters. Column widths are
    /// derived from this so a column never reserves room for text that is
    /// never there — until the user drags one, after which their width wins.
    pub content_width: Vec<usize>,
    /// Widths the user set by dragging, in pixels.
    pub column_override: Vec<Option<f32>>,

    pub started: Option<Instant>,
    pub finished: Option<Instant>,

    // --- interaction ------------------------------------------------------
    /// The form's editors, one per field, in field order.
    pub inputs: Vec<Option<Entity<TextInput>>>,
    pub filter_input: Entity<TextInput>,
    pub filter_revision: usize,
    pub filter: String,
    /// The column to sort on and whether it is descending. `None` keeps
    /// arrival order, which is usually what you want while a scan is running.
    pub sort: Option<(usize, bool)>,
    pub selected: Option<usize>,
    /// Whether it has a tab in the editor. Closing the tab clears this; it is
    /// still in the workspace, and clicking it in the side bar brings it back.
    pub open: bool,
    /// The folder inside the workspace its file sits in, if any. A folder is
    /// a way of grouping; the stage the tool is at still orders it, inside the
    /// folder rather than instead of it.
    pub folder: Option<String>,
    /// Another run of the same tool this one is being read against. Runtime
    /// only: a comparison is a way of looking, not part of the result.
    pub compare_with: Option<usize>,
    pub show_form: bool,
    /// Whether the graph takes the whole pane. It is always drawn when there
    /// is anything to draw; the button decides how much room it gets.
    pub chart_expanded: bool,
    pub field_error: Option<(usize, String)>,

    pub table_scroll: UniformListScrollHandle,
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
            content_width: vec![0; columns],
            column_override: vec![None; columns],
            started: None,
            finished: None,
            inputs,
            filter_input,
            filter_revision: 0,
            filter: String::new(),
            sort: None,
            selected: None,
            open: true,
            folder: None,
            compare_with: None,
            show_form: true,
            chart_expanded: false,
            field_error: None,
            table_scroll: UniformListScrollHandle::new(),
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

        self.content_width = vec![0; self.tool.columns().len()];
        for row in &self.rows {
            for (i, cell) in row.cells.iter().enumerate() {
                if i < self.content_width.len() {
                    self.content_width[i] = self.content_width[i].max(cell.chars().count());
                }
            }
        }
        // A restored run has results to read, so the form starts out of the
        // way rather than covering them.
        self.show_form = self.state == State::Setup;
        self.restored_elapsed = record.elapsed;
        self.view_dirty = true;
    }

    /// What the tab strip and the hub show next to the tool's name.
    ///
    /// A target field the tool is currently hiding does not describe this job
    /// — the speed test against Cloudflare is not a job "about" whatever is in
    /// the custom URL box — so the nearest visible choice stands in instead.
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
    /// it built, for a run that is adding to it rather than replacing it.
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
    /// clean table rather than appending to the old one.
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

    /// Widens the remembered content width to fit a row, which is what the
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
                // What counts as the same row is the row's own idea of its
                // identity, which is its target unless it says otherwise.
                match self.rows.iter_mut().find(|r| r.identity() == row.identity()) {
                    // Rewriting a row in place must not disturb the order the
                    // table is sorted or filtered into, so the view is only
                    // marked stale when a row is genuinely new.
                    Some(existing) => *existing = row,
                    None => {
                        self.rows.push(row);
                        self.view_dirty = true;
                    }
                }
            }
            Event::Stats(stats) => self.stats = stats,
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

    /// How many log lines were good, worrying and wrong, which is what the
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
/// it points at — and a list of links as how many there are.
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

fn cell(rows: &[Row], index: usize, column: usize) -> &str {
    rows.get(index).and_then(|r| r.cells.get(column)).map(String::as_str).unwrap_or("")
}

/// Compares two cells the way a person reads them: numbers numerically, IP
/// addresses by their parts, everything else as text. Sorting "10" after "9"
/// is the whole point of a sortable port column.
fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    if let (Some(x), Some(y)) = (leading_number(a), leading_number(b))
        && x != y
    {
        return x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal);
    }
    if let (Ok(x), Ok(y)) = (a.parse::<std::net::IpAddr>(), b.parse::<std::net::IpAddr>()) {
        return x.cmp(&y);
    }
    a.to_lowercase().cmp(&b.to_lowercase())
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
    use super::shorten;

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
