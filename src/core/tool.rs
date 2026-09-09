//! The contract every ntls tool implements.
//!
//! A tool describes itself, declares the inputs it needs as a list of fields,
//! and streams events while it runs. The UI knows nothing about pinging or
//! port scanning: it renders forms from fields and tables from columns, so
//! adding a tool means implementing this trait and registering it.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::event::Emitter;
use super::field::{Field, Params, Role};

/// How a column's cells are drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Cells {
    /// The cell is its text.
    #[default]
    Text,
    /// The cell is a fraction between 0 and 1, drawn as a filled bar with the
    /// percentage beside it. A tool that tracks work in flight (a transfer,
    /// an upload) gets a bar per row without the interface knowing what the
    /// work is.
    Bar,
}

/// One column of the result table.
#[derive(Clone, Debug)]
pub struct Column {
    pub title: &'static str,
    /// The preferred display width in characters. Zero means the column
    /// flexes to fill whatever horizontal space is left over.
    pub width: usize,
    pub cells: Cells,
}

pub fn col(title: &'static str, width: usize) -> Column {
    Column { title, width, cells: Cells::Text }
}

/// A column whose cells are a fraction of the way done.
pub fn bar(title: &'static str, width: usize) -> Column {
    Column { title, width, cells: Cells::Bar }
}

/// Cancellation for a running tool. Every long loop should consult it, and
/// every await should race against [`Cancel::cancelled`].
#[derive(Clone, Default)]
pub struct Cancel {
    flag: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolves when cancellation is requested, and immediately if it already
    /// has been.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let waiter = self.notify.notified();
        if self.is_cancelled() {
            return;
        }
        waiter.await;
    }

    /// Runs a future, giving up as soon as the job is cancelled.
    pub async fn run<T>(&self, fut: impl Future<Output = T>) -> Option<T> {
        tokio::select! {
            biased;
            _ = self.cancelled() => None,
            v = fut => Some(v),
        }
    }

    /// Sleeps, returning false if the job was cancelled first.
    pub async fn sleep(&self, d: std::time::Duration) -> bool {
        self.run(tokio::time::sleep(d)).await.is_some()
    }
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Everything a run needs to know about itself.
pub struct Run {
    pub cancel: Cancel,
    pub params: Params,
    /// The targets a previous, interrupted run of this same job already
    /// covered. A resumable tool skips them; every other tool can ignore this
    /// and will simply start again.
    pub done: std::collections::HashSet<String>,
    /// The rows that run already produced, in the order it produced them.
    ///
    /// A tool that works through a list only needs [`Run::done`]; a tool that
    /// measures one thing over time needs to know what it measured, so that a
    /// resumed run continues the sequence and the summary counts everything
    /// and does not start over halfway down the table.
    pub prior: Vec<super::event::Row>,
    /// This run is adding to a table earlier runs of the same job left behind,
    /// and is not carrying on from one that was interrupted.
    ///
    /// Everything in [`Run::prior`] is still on screen and stays there. The
    /// whole target list is probed again, nothing is skipped, and what is
    /// found either rewrites the row it is about or joins it, which is the
    /// difference between a scan and a record of a network over time.
    pub keep: bool,
}

impl Run {
    /// A run with nothing behind it. The interface always says what it means
    /// A run that keeps what earlier ones found is keeping from the first one,
    /// whose table is empty, so this is for the tests, which drive a
    /// tool on its own.
    #[cfg(test)]
    pub fn fresh(cancel: Cancel, params: Params) -> Run {
        Run {
            cancel,
            params,
            done: std::collections::HashSet::new(),
            prior: Vec::new(),
            keep: false,
        }
    }

    /// Whether this run is carrying on from an earlier one.
    ///
    /// A run that is keeping what earlier runs found is not resuming: it has
    /// their rows, but nothing has been probed yet and nothing is skipped.
    pub fn resuming(&self) -> bool {
        !self.keep && (!self.done.is_empty() || !self.prior.is_empty())
    }
}

/// The unit of functionality in ntls.
pub trait Tool: Send + Sync + 'static {
    /// The stable, lowercase identifier used for handoffs and config.
    fn id(&self) -> &'static str;
    /// The short display name shown in the picker.
    fn title(&self) -> &'static str;
    /// A one-line summary shown next to the title.
    fn desc(&self) -> &'static str;
    /// The name of the icon that stands in for the tool in dense places.
    fn icon(&self) -> &'static str;
    /// Declares the inputs. The form is generated from these in order.
    fn fields(&self) -> Vec<Field>;
    /// Declares the result table layout. Rows emitted by `run` must carry
    /// exactly this many cells.
    fn columns(&self) -> Vec<Column>;
    /// Whether an interrupted run can be picked up where it stopped.
    ///
    /// True for the tools that work through a list, a subnet or a port range,
    /// and can be told which entries are already accounted for. A tool that
    /// measures one thing over time has nothing to resume.
    fn resumable(&self) -> bool {
        false
    }

    /// Executes the tool, emitting an event for everything it learns. It must
    /// respect `cancel` and return promptly once cancelled.
    fn run<'a>(&'a self, run: Run, emit: Emitter) -> BoxFuture<'a, anyhow::Result<()>>;

    /// A whole request pasted into the target field, spread across the form.
    ///
    /// Some tools have a text form the rest of the world already writes them
    /// in. An HTTP request is nearly always handed round as a `curl` line,
    /// and retyping one field at a time out of it is the tedium the form was
    /// supposed to remove. A tool that recognises what was pasted says which
    /// of its fields it fills; anything not named is left alone.
    ///
    /// Returns `None` when the text is nothing it knows, which is the common
    /// case and must stay cheap: this is asked on every edit of the field.
    fn absorb(&self, _pasted: &str) -> Option<Vec<(&'static str, String)>> {
        None
    }

    /// The key of the field carrying the target, if there is one.
    fn target_key(&self) -> Option<&'static str> {
        self.fields().into_iter().find(|f| f.role == Role::Target).map(|f| f.key)
    }

    /// The key of the field carrying a port specification, if there is one.
    fn ports_key(&self) -> Option<&'static str> {
        self.fields().into_iter().find(|f| f.role == Role::Ports).map(|f| f.key)
    }

    /// The key of the switch that says a run should add to what earlier runs
    /// of the same job found, if the tool has one.
    ///
    /// The interface reads this and does not know which tools accumulate:
    /// a tool declares the switch and gets the behaviour, the same way it
    /// declares a target and gets the hand-offs.
    fn keep_key(&self) -> Option<&'static str> {
        self.fields().into_iter().find(|f| f.role == Role::Keep).map(|f| f.key)
    }
}

/// Holds the tools available to the UI, in the order the picker shows them.
pub struct Registry {
    order: Vec<Arc<dyn Tool>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry { order: Vec::new() }
    }

    /// Appends a tool. A duplicate id replaces the earlier registration in
    /// place, keeping the original position.
    pub fn add(&mut self, t: Arc<dyn Tool>) {
        if let Some(slot) = self.order.iter_mut().find(|e| e.id() == t.id()) {
            *slot = t;
        } else {
            self.order.push(t);
        }
    }

    /// The tools in display order.
    pub fn all(&self) -> &[Arc<dyn Tool>] {
        &self.order
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Tool>> {
        self.order.iter().find(|t| t.id() == id).cloned()
    }

}

impl Default for Registry {
    fn default() -> Self {
        Registry::new()
    }
}
