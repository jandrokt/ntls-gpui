//! Comparing two runs of the same tool.
//!
//! A scan is most useful against another scan: what is on the network now that
//! was not last week, what has stopped answering, what changed its banner.
//! Rows are matched on the target they refer to, the one part of a row that
//! names the same thing across two runs, then compared cell by cell.

use crate::core::{Column, Row, Status};

/// What became of one target between two runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    /// Present now, absent before.
    Added,
    /// Present before, absent now.
    Removed,
    /// Present in both, with at least one cell different.
    Changed,
    /// Present in both and identical.
    Same,
}

impl Change {
    pub fn label(self) -> &'static str {
        match self {
            Change::Added => "added",
            Change::Removed => "removed",
            Change::Changed => "changed",
            Change::Same => "same",
        }
    }

    /// Read as a result status, so the table colours a diff without knowing
    /// it is one.
    pub fn status(self) -> Status {
        match self {
            Change::Added => Status::Up,
            Change::Removed => Status::Down,
            Change::Changed => Status::Warn,
            Change::Same => Status::Info,
        }
    }
}

/// One target, as it stands across the two runs.
#[derive(Clone, Debug)]
pub struct Entry {
    pub target: String,
    pub change: Change,
    /// The cells to show: the current run's, or the earlier run's for a target
    /// that has gone.
    pub cells: Vec<String>,
    /// For a changed row, the earlier value of each cell that differs, by
    /// column index.
    pub was: Vec<(usize, String)>,
}

/// A comparison, and what it adds up to.
#[derive(Clone, Debug, Default)]
pub struct Diff {
    pub entries: Vec<Entry>,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub same: usize,
}

impl Diff {
    /// Whether anything is different at all.
    pub fn any(&self) -> bool {
        self.added + self.removed + self.changed > 0
    }

    /// The one-line summary that sits above the table.
    pub fn summary(&self) -> String {
        if !self.any() {
            return format!("No differences across {} row(s)", self.same);
        }
        let mut parts = Vec::new();
        if self.added > 0 {
            parts.push(format!("{} added", self.added));
        }
        if self.removed > 0 {
            parts.push(format!("{} removed", self.removed));
        }
        if self.changed > 0 {
            parts.push(format!("{} changed", self.changed));
        }
        if self.same > 0 {
            parts.push(format!("{} unchanged", self.same));
        }
        parts.join(" · ")
    }
}

/// Compares an earlier run against the current one.
///
/// `columns` decides which cells are worth comparing: a column the tool
/// declared but a row did not fill is not a difference, and a cell past the
/// declared columns is not shown, so it is not compared either.
pub fn compare(before: &[Row], after: &[Row], columns: &[Column]) -> Diff {
    let mut diff = Diff::default();
    let width = columns.len();

    for row in after {
        match before.iter().find(|b| b.target == row.target) {
            None => {
                diff.added += 1;
                diff.entries.push(Entry {
                    target: row.target.clone(),
                    change: Change::Added,
                    cells: padded(row, width),
                    was: Vec::new(),
                });
            }
            Some(earlier) => {
                let (now, then) = (padded(row, width), padded(earlier, width));
                let was: Vec<(usize, String)> = now
                    .iter()
                    .zip(&then)
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .map(|(i, (_, b))| (i, b.clone()))
                    .collect();

                let change = if was.is_empty() { Change::Same } else { Change::Changed };
                if was.is_empty() {
                    diff.same += 1;
                } else {
                    diff.changed += 1;
                }
                diff.entries.push(Entry { target: row.target.clone(), change, cells: now, was });
            }
        }
    }

    for row in before {
        if after.iter().any(|a| a.target == row.target) {
            continue;
        }
        diff.removed += 1;
        diff.entries.push(Entry {
            target: row.target.clone(),
            change: Change::Removed,
            cells: padded(row, width),
            was: Vec::new(),
        });
    }

    // What differs is what you opened the comparison for, so it goes first.
    diff.entries.sort_by_key(|e| match e.change {
        Change::Added => 0,
        Change::Removed => 1,
        Change::Changed => 2,
        Change::Same => 3,
    });
    diff
}

fn padded(row: &Row, width: usize) -> Vec<String> {
    (0..width).map(|i| row.cells.get(i).cloned().unwrap_or_default()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::col;

    fn row(target: &str, cells: &[&str]) -> Row {
        Row {
            cells: cells.iter().map(|c| c.to_string()).collect(),
            status: Status::Up,
            target: target.into(),
            note: None,
            key: None,
        }
    }

    fn columns() -> Vec<Column> {
        vec![col("HOST", 20), col("PORT", 8), col("BANNER", 0)]
    }

    #[test]
    fn a_host_that_appeared_is_added_and_one_that_went_is_removed() {
        let before = [row("10.0.0.1", &["10.0.0.1", "22", "ssh"])];
        let after = [row("10.0.0.2", &["10.0.0.2", "80", "http"])];

        let diff = compare(&before, &after, &columns());
        assert_eq!((diff.added, diff.removed, diff.changed), (1, 1, 0));
        // What differs comes first, and additions before removals.
        assert_eq!(diff.entries[0].change, Change::Added);
        assert_eq!(diff.entries[0].target, "10.0.0.2");
        assert_eq!(diff.entries[1].change, Change::Removed);
    }

    #[test]
    fn a_changed_cell_is_reported_with_what_it_was() {
        let before = [row("10.0.0.1", &["10.0.0.1", "22", "OpenSSH 8.4"])];
        let after = [row("10.0.0.1", &["10.0.0.1", "22", "OpenSSH 9.6"])];

        let diff = compare(&before, &after, &columns());
        assert_eq!(diff.changed, 1);
        let entry = &diff.entries[0];
        assert_eq!(entry.change, Change::Changed);
        assert_eq!(entry.was, vec![(2, "OpenSSH 8.4".to_string())]);
        assert_eq!(entry.cells[2], "OpenSSH 9.6");
    }

    #[test]
    fn an_identical_run_has_nothing_to_report() {
        let rows = [row("10.0.0.1", &["10.0.0.1", "22", "ssh"])];
        let diff = compare(&rows, &rows, &columns());

        assert!(!diff.any());
        assert_eq!(diff.same, 1);
        assert_eq!(diff.summary(), "No differences across 1 row(s)");
    }

    #[test]
    fn only_the_declared_columns_are_compared() {
        // An extra cell beyond the declared columns is not shown, so it
        // cannot be reported as a difference either.
        let before = [row("10.0.0.1", &["10.0.0.1", "22", "ssh", "hidden-a"])];
        let after = [row("10.0.0.1", &["10.0.0.1", "22", "ssh", "hidden-b"])];

        assert_eq!(compare(&before, &after, &columns()).changed, 0);
    }

    #[test]
    fn a_row_missing_a_cell_matches_one_with_an_empty_cell() {
        let before = [row("10.0.0.1", &["10.0.0.1", "22"])];
        let after = [row("10.0.0.1", &["10.0.0.1", "22", ""])];

        assert_eq!(compare(&before, &after, &columns()).changed, 0);
    }

    #[test]
    fn the_summary_reads_as_a_count_of_what_changed() {
        let before = [row("a", &["a"]), row("b", &["b"])];
        let after = [row("a", &["a"]), row("c", &["c"])];

        let diff = compare(&before, &after, &[col("HOST", 20)]);
        assert_eq!(diff.summary(), "1 added · 1 removed · 1 unchanged");
    }
}
