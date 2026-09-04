//! Comparing two runs of the same tool.
//!
//! A scan is most useful against another scan: what is on the network now that
//! was not last week, what has stopped answering, what changed its banner.
//! Rows are matched on the identity a row gives itself, which is its target
//! unless the tool says otherwise, and on the target alone where that leaves
//! rows unaccounted for, then compared cell by cell.

use crate::core::{Column, Row, Status};
use std::collections::{HashMap, VecDeque};

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

    // Which earlier row each current row is about, settled before anything is
    // reported, because an earlier row may be spoken for only once.
    //
    // Searching the earlier run for a matching target paired a row with the
    // first earlier row that merely mentioned the same host, and more than one
    // row can. A traceroute names every silent hop after the destination it is
    // tracing to, so a run's rows share a target; read against an identical
    // traceroute, every silent hop matched the first one and all but that one
    // came out changed, their TTL cells being different. A kept scan holds two
    // rows for one address once the device answering there changes, and there
    // the row for the device that has gone was matched by the row for the
    // device that replaced it, reported as a change, and never reported as
    // removed, its address still being present. So the identity a row gives
    // itself decides it, the same thing the table uses to know when one row
    // replaces another.
    //
    // Identity alone is not enough, though. It is the target for a scan that
    // is not keeping what earlier runs found and something longer for one that
    // is, so two runs with that switch flipped between them share no
    // identities at all and would read as every host having gone and an
    // identical one arrived. Whatever no identity accounts for is matched on
    // its target afterwards, against the earlier rows still unclaimed.
    //
    // Indexing the earlier run also spares it a pass per row of the current
    // one. The comparison is rebuilt from scratch on every frame it is on
    // screen, and on the tens of thousands of rows a port scan listing closed
    // ports leaves behind, a pass per row is more work than a frame has room
    // for.
    let mut by_identity: HashMap<&str, VecDeque<usize>> = HashMap::with_capacity(before.len());
    for (at, row) in before.iter().enumerate() {
        by_identity.entry(row.identity()).or_default().push_back(at);
    }
    let mut claimed = vec![false; before.len()];
    let mut paired: Vec<Option<usize>> = Vec::with_capacity(after.len());
    for row in after {
        let at = by_identity.get_mut(row.identity()).and_then(|rows| rows.pop_front());
        if let Some(at) = at {
            claimed[at] = true;
        }
        paired.push(at);
    }
    if paired.iter().any(|pair| pair.is_none()) {
        let mut by_target: HashMap<&str, VecDeque<usize>> = HashMap::new();
        for (at, row) in before.iter().enumerate() {
            if !claimed[at] {
                by_target.entry(row.target.as_str()).or_default().push_back(at);
            }
        }
        for (row, pair) in after.iter().zip(&mut paired) {
            if pair.is_some() {
                continue;
            }
            if let Some(at) =
                by_target.get_mut(row.target.as_str()).and_then(|rows| rows.pop_front())
            {
                claimed[at] = true;
                *pair = Some(at);
            }
        }
    }

    for (row, pair) in after.iter().zip(&paired) {
        match *pair {
            None => {
                diff.added += 1;
                diff.entries.push(Entry {
                    target: row.target.clone(),
                    change: Change::Added,
                    cells: padded(row, width),
                    was: Vec::new(),
                });
            }
            Some(at) => {
                let earlier = &before[at];
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

    for (at, row) in before.iter().enumerate() {
        if claimed[at] {
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

    /// A row that says which row it is, for the tools that put more than one
    /// row about the same host in a table.
    fn keyed(target: &str, key: &str, cells: &[&str]) -> Row {
        Row { key: Some(key.into()), ..row(target, cells) }
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

    #[test]
    fn a_traceroute_read_against_itself_reports_no_changed_hops() {
        // Every silent hop of a traceroute is named after the destination, so
        // several rows of one run share a target. Each has to be read against
        // the hop at the same place in the earlier run, not against the first
        // row that happens to name the same host.
        let hops = [
            row("1.1.1.1", &["1", "192.168.1.1"]),
            row("1.1.1.1", &["2", "*"]),
            row("1.1.1.1", &["3", "*"]),
            row("1.1.1.1", &["4", "1.1.1.1"]),
        ];
        let path = [col("TTL", 6), col("HOST", 20)];

        let diff = compare(&hops, &hops, &path);
        assert!(!diff.any(), "{}", diff.summary());
        assert_eq!(diff.same, 4);
    }

    #[test]
    fn a_device_that_has_gone_is_removed_even_where_another_row_shares_its_address() {
        // A kept scan keeps the row for the device that used to answer on an
        // address alongside the row for the one that answers there now. The
        // old device is gone, however busy its address still is.
        let before = [
            keyed("10.0.0.5", "10.0.0.5/aa:aa", &["10.0.0.5", "22", "aa:aa"]),
            keyed("10.0.0.5", "10.0.0.5/bb:bb", &["10.0.0.5", "22", "bb:bb"]),
        ];
        let after = [keyed("10.0.0.5", "10.0.0.5/bb:bb", &["10.0.0.5", "22", "bb:bb"])];

        let diff = compare(&before, &after, &columns());
        assert_eq!((diff.added, diff.removed, diff.changed, diff.same), (0, 1, 0, 1));
        let gone = diff.entries.iter().find(|e| e.change == Change::Removed).unwrap();
        assert_eq!(gone.cells[2], "aa:aa");
    }

    #[test]
    fn two_runs_that_name_their_rows_differently_still_match_on_the_target() {
        // A scan keeping what earlier runs found gives its rows keys; the same
        // scan with that switch off does not. The two runs are still about the
        // same hosts, so they must not read as though every host had gone and
        // an identical one arrived.
        let before = [keyed("10.0.0.5", "up:10.0.0.5|aa:aa", &["10.0.0.5", "22", "ssh"])];
        let after = [row("10.0.0.5", &["10.0.0.5", "22", "ssh"])];

        let diff = compare(&before, &after, &columns());
        assert!(!diff.any(), "{}", diff.summary());
        assert_eq!(diff.same, 1);
    }

    #[test]
    fn more_rows_about_one_target_than_before_are_the_extra_ones_added() {
        let before = [row("1.1.1.1", &["1", "*"])];
        let after =
            [row("1.1.1.1", &["1", "*"]), row("1.1.1.1", &["2", "*"]), row("1.1.1.1", &["3", "*"])];
        let path = [col("TTL", 6), col("HOST", 20)];

        let diff = compare(&before, &after, &path);
        assert_eq!((diff.added, diff.removed, diff.changed, diff.same), (2, 0, 0, 1));
    }
}
