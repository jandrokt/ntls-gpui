//! What an expression works on, and what a tool's results look like to it.

use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum Value {
    Number(f64),
    Text(String),
    Bool(bool),
    List(Vec<Value>),
    /// One tool's results.
    Table(Arc<Table>),
    /// A missing thing. Reading a field of it is not an error, so a document
    /// written against a tool that has not run yet still renders.
    Nothing,
}

impl Value {
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Number(_) => "a number",
            Value::Text(_) => "text",
            Value::Bool(_) => "true or false",
            Value::List(_) => "a list",
            Value::Table(_) => "a set of results",
            Value::Nothing => "nothing",
        }
    }

    /// The number this is, if it is one. Text that reads as a number counts,
    /// since a tool's cells are text: `"12.4 ms"` is 12.4.
    pub fn number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Text(s) => leading_number(s),
            _ => None,
        }
    }

    /// Whether this counts as true. Nothing, zero, empty text and an empty
    /// list are false; everything else is true.
    pub fn truth(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::Text(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Table(t) => !t.rows.is_empty(),
            Value::Nothing => false,
        }
    }

    /// Every number in this, for the functions that summarise.
    pub fn numbers(&self) -> Vec<f64> {
        match self {
            Value::List(items) => items.iter().filter_map(Value::number).collect(),
            other => other.number().into_iter().collect(),
        }
    }

    /// How it is written into a document.
    pub fn show(&self) -> String {
        match self {
            Value::Number(n) => format_number(*n),
            Value::Text(s) => s.clone(),
            Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Value::List(items) => {
                items.iter().map(Value::show).collect::<Vec<_>>().join(", ")
            }
            Value::Table(t) => format!("{} ({} rows)", t.name, t.rows.len()),
            Value::Nothing => String::new(),
        }
    }
}

/// Trims the noise off a computed number: whole numbers stay whole, and the
/// rest keep enough digits to be worth reading.
pub fn format_number(n: f64) -> String {
    if !n.is_finite() {
        return "—".into();
    }
    if n == n.trunc() && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    let text = format!("{n:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The number a cell starts with, so `"12.4 ms"`, `"87%"` and `"443/tcp"` all
/// yield a figure to work with.
pub fn leading_number(s: &str) -> Option<f64> {
    let s = s.trim();
    let end = s
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && *c == '-')))
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

/// One tool's results, as an expression sees them.
#[derive(Clone, Debug, Default)]
pub struct Table {
    /// What the run is called.
    pub name: String,
    /// The tool's id, so a document can tell a ping from a port scan.
    pub tool: String,
    /// What it was pointed at.
    pub target: String,
    /// Where it got to: ready, running, done, stopped, failed.
    pub state: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// One per row: up, down, warn or info.
    pub statuses: Vec<String>,
    /// The summary the tool published, as it appears above the table.
    pub stats: Vec<(String, String)>,
    /// How long the run took, in seconds.
    pub elapsed: Option<f64>,
}

impl Table {
    /// One column, by name, case-insensitively.
    pub fn column(&self, name: &str) -> Option<Value> {
        let at = self.columns.iter().position(|c| c.eq_ignore_ascii_case(name))?;
        Some(Value::List(
            self.rows
                .iter()
                .map(|r| Value::Text(r.get(at).cloned().unwrap_or_default()))
                .collect(),
        ))
    }

    /// One figure from the summary, by name.
    pub fn stat(&self, name: &str) -> Option<Value> {
        let (_, value) = self.stats.iter().find(|(k, _)| k.eq_ignore_ascii_case(name))?;
        Some(Value::Text(value.clone()))
    }

    pub fn count_status(&self, want: &str) -> usize {
        self.statuses.iter().filter(|s| s.eq_ignore_ascii_case(want)).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_yields_the_figure_it_starts_with() {
        assert_eq!(leading_number("12.4 ms"), Some(12.4));
        assert_eq!(leading_number("87%"), Some(87.0));
        assert_eq!(leading_number("443/tcp"), Some(443.0));
        assert_eq!(leading_number("-3 dB"), Some(-3.0));
        // An address is not a number.
        assert_eq!(leading_number("10.0.0.1"), None);
        assert_eq!(leading_number("ssh"), None);
        assert_eq!(leading_number("-"), None);
    }

    #[test]
    fn a_computed_number_is_written_without_noise() {
        assert_eq!(format_number(4.0), "4");
        assert_eq!(format_number(4.5), "4.5");
        assert_eq!(format_number(4.567), "4.57");
        assert_eq!(format_number(f64::NAN), "—");
    }

    #[test]
    fn emptiness_is_false_and_everything_else_is_true() {
        assert!(!Value::Nothing.truth());
        assert!(!Value::Number(0.0).truth());
        assert!(!Value::Text(String::new()).truth());
        assert!(!Value::List(Vec::new()).truth());
        assert!(Value::Number(1.0).truth());
        assert!(Value::Text("x".into()).truth());
    }

    #[test]
    fn a_column_is_found_however_it_is_capitalised() {
        let table = Table {
            columns: vec!["#".into(), "RTT".into()],
            rows: vec![vec!["1".into(), "10 ms".into()], vec!["2".into(), "20 ms".into()]],
            ..Table::default()
        };
        let column = table.column("rtt").expect("the column");
        assert_eq!(column.numbers(), vec![10.0, 20.0]);
        assert!(table.column("nope").is_none());
    }
}
