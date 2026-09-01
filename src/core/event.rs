//! Events streamed by a running tool.
//!
//! A tool describes what it found by emitting these; nothing in the UI knows
//! what a ping or a port scan is, so a new tool gets a table, a log, a stat
//! bar, a progress bar and a chart without asking for any of them.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Status colours a result row and drives the summary counters.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Status {
    #[default]
    Info,
    /// Reachable, open, success.
    Up,
    /// Unreachable, closed, failure.
    Down,
    /// Ambiguous: filtered, open|filtered, partial loss.
    Warn,
}

/// Level is the severity of a log line.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Level {
    #[default]
    Info,
    Good,
    Warn,
    Error,
}

/// One line of the result table.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Row {
    pub cells: Vec<String>,
    pub status: Status,
    /// The host (or host:port) this row refers to. It is what gets handed off
    /// when the user pipes a result into another tool.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// What makes this row the same row as an earlier one, where its target
    /// is not enough to say.
    ///
    /// A scan that keeps what earlier runs found has two rows about one
    /// address the moment the device behind it changes, and both are about
    /// that address, so an upsert has to be told what identity means before
    /// it rewrites one of them. Empty means the target is the identity, which
    /// is what it is for every tool that does not think about this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl Row {
    /// What this row is about, for the purpose of rewriting it in place.
    pub fn identity(&self) -> &str {
        self.key.as_deref().unwrap_or(&self.target)
    }
}

/// One entry in the stat bar.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Kv {
    pub k: String,
    pub v: String,
}

pub fn kv(k: impl Into<String>, v: impl Into<String>) -> Kv {
    Kv { k: k.into(), v: v.into() }
}

/// A single update from a running tool.
#[derive(Clone, Debug)]
pub enum Event {
    /// Appends a line to the log pane.
    Log { level: Level, text: String },
    /// Appends a row to the result table.
    Row(Row),
    /// Replaces the row with the same `target`, appending it when there is
    /// none. A tool that watches one thing change (a transfer, a lease, a
    /// link) keeps a row per thing instead of a log of every state it passed
    /// through.
    Upsert(Row),
    /// Replaces the whole stat bar.
    Stats(Vec<Kv>),
    /// Updates the progress bar. `label` replaces the "done/total" text for
    /// tools whose progress is not a count of things.
    Progress { done: usize, total: usize, label: Option<String> },
    /// Records a numeric observation for the live chart. A tool that emits
    /// samples gets a graph without asking for one.
    Sample { series: String, unit: String, value: f64 },
}

impl Event {
    pub fn log(level: Level, text: impl Into<String>) -> Event {
        Event::Log { level, text: text.into() }
    }
    pub fn info(text: impl Into<String>) -> Event {
        Event::log(Level::Info, text)
    }
    pub fn good(text: impl Into<String>) -> Event {
        Event::log(Level::Good, text)
    }
    pub fn warn(text: impl Into<String>) -> Event {
        Event::log(Level::Warn, text)
    }
    pub fn err(text: impl Into<String>) -> Event {
        Event::log(Level::Error, text)
    }

    pub fn row(status: Status, target: impl Into<String>, cells: Vec<String>) -> Event {
        Event::Row(Row { cells, status, target: target.into(), note: None, key: None })
    }

    /// A row that stands for a thing, not for a moment: emitting it
    /// again with the same target rewrites it in place.
    pub fn upsert(status: Status, target: impl Into<String>, cells: Vec<String>) -> Event {
        Event::Upsert(Row { cells, status, target: target.into(), note: None, key: None })
    }

    /// An upsert whose identity is something other than its target, for a
    /// table that holds more than one row about the same thing: the two
    /// devices that have answered on one address, the two answers a name has
    /// given.
    pub fn upsert_as(
        key: impl Into<String>,
        status: Status,
        target: impl Into<String>,
        cells: Vec<String>,
    ) -> Event {
        Event::Upsert(Row {
            cells,
            status,
            target: target.into(),
            note: None,
            key: Some(key.into()),
        })
    }

    pub fn stats(stats: Vec<Kv>) -> Event {
        Event::Stats(stats)
    }

    pub fn progress(done: usize, total: usize) -> Event {
        Event::Progress { done, total, label: None }
    }

    /// Progress with the counter replaced by text, for progress measured in
    /// something other than items, elapsed time say.
    pub fn progress_label(done: usize, total: usize, label: impl Into<String>) -> Event {
        Event::Progress { done, total, label: Some(label.into()) }
    }

    /// Samples with the same series name accumulate into one line; a tool may
    /// emit several series and each gets its own chart.
    pub fn sample(series: impl Into<String>, unit: impl Into<String>, value: f64) -> Event {
        Event::Sample { series: series.into(), unit: unit.into(), value }
    }
}

/// Receives events from a running tool. Cloneable and safe to use from any
/// task; sending after the job is gone is a no-op.
#[derive(Clone)]
pub struct Emitter(Arc<dyn Fn(Event) + Send + Sync>);

impl Emitter {
    pub fn new(f: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Emitter(Arc::new(f))
    }

    pub fn emit(&self, e: Event) {
        (self.0)(e)
    }

    pub fn info(&self, text: impl Into<String>) {
        self.emit(Event::info(text))
    }
    pub fn good(&self, text: impl Into<String>) {
        self.emit(Event::good(text))
    }
    pub fn warn(&self, text: impl Into<String>) {
        self.emit(Event::warn(text))
    }
    pub fn err(&self, text: impl Into<String>) {
        self.emit(Event::err(text))
    }
    pub fn row(&self, status: Status, target: impl Into<String>, cells: Vec<String>) {
        self.emit(Event::row(status, target, cells))
    }
    pub fn upsert(&self, status: Status, target: impl Into<String>, cells: Vec<String>) {
        self.emit(Event::upsert(status, target, cells))
    }
    pub fn upsert_as(
        &self,
        key: impl Into<String>,
        status: Status,
        target: impl Into<String>,
        cells: Vec<String>,
    ) {
        self.emit(Event::upsert_as(key, status, target, cells))
    }
    pub fn stats(&self, stats: Vec<Kv>) {
        self.emit(Event::stats(stats))
    }
    pub fn progress(&self, done: usize, total: usize) {
        self.emit(Event::progress(done, total))
    }
}

/// Builds the `Vec<String>` a row needs without a pile of `.to_string()`.
#[macro_export]
macro_rules! cells {
    ($($cell:expr),* $(,)?) => {
        vec![$(::std::string::ToString::to_string(&$cell)),*]
    };
}
