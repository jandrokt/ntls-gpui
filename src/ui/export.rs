//! Writing a set of results out as CSV.
//!
//! The table on screen is the tool's own columns plus whatever notes have been
//! attached to the rows, so that is what is written: one header line, one line
//! per row, in the order the table is currently sorted and filtered into.

use std::collections::BTreeMap;

use crate::core::{Column, Row};

/// Renders rows as CSV, in RFC 4180 form.
pub fn csv(columns: &[Column], rows: &[&Row], notes: &BTreeMap<String, String>) -> String {
    let mut out = String::new();

    let mut header: Vec<&str> = columns.iter().map(|c| c.title).collect();
    header.push("Target");
    header.push("Status");
    header.push("Note");
    write_line(&mut out, header.into_iter().map(field));

    for row in rows {
        let mut cells: Vec<String> = (0..columns.len())
            .map(|i| row.cells.get(i).cloned().unwrap_or_default())
            .collect();
        cells.push(row.target.clone());
        cells.push(status_of(row.status).to_string());
        // The row's own note first: the workspace map is only what an older
        // version of ntls left behind.
        let note = row
            .note
            .clone()
            .or_else(|| notes.get(&row.target).cloned())
            .unwrap_or_default();
        cells.push(note);
        write_line(&mut out, cells.iter().map(|c| field(c.as_str())));
    }
    out
}

fn write_line(out: &mut String, mut fields: impl Iterator<Item = String>) {
    if let Some(first) = fields.next() {
        out.push_str(&first);
    }
    for field in fields {
        out.push(',');
        out.push_str(&field);
    }
    // CRLF, because that is what the format says and what spreadsheets on
    // every platform accept.
    out.push_str("\r\n");
}

/// Quotes a field if it needs it, doubles any quotes inside it, and keeps a
/// cell that reads as a formula from being one.
fn field(value: &str) -> String {
    let needs_quoting =
        value.contains([',', '"', '\n', '\r']) || value.starts_with(' ') || value.ends_with(' ');
    // Much of a cell is whatever the far end said about itself: a TXT record,
    // a Server: header, a service banner. Quoting keeps such text inside its
    // field but does nothing about the spreadsheet, which reads a cell
    // opening with = + - @ as a formula and will happily run DDE or
    // WEBSERVICE() out of it, so scanning a hostile host and exporting the
    // table was enough to have the host's text execute when the file was
    // opened. The apostrophe is the marker that says the cell is text.
    let guard = if reads_as_formula(value) { "'" } else { "" };
    if !needs_quoting {
        return format!("{guard}{value}");
    }
    format!("\"{guard}{}\"", value.replace('"', "\"\""))
}

/// Whether a spreadsheet would take this cell for a formula rather than text.
fn reads_as_formula(value: &str) -> bool {
    match value.chars().next() {
        // A leading tab or carriage return is thrown away before the rest of
        // the cell is looked at, so it hides a formula instead of stopping
        // one.
        Some('=' | '+' | '@' | '\t' | '\r') => true,
        // A lone dash is what a tool writes for "no answer" and a negative
        // number is a measurement. Neither can carry a payload, and marking
        // them as text would stop the column being added up.
        Some('-') => value.len() > 1 && value.parse::<f64>().is_err(),
        _ => false,
    }
}

fn status_of(status: crate::core::Status) -> &'static str {
    match status {
        crate::core::Status::Up => "up",
        crate::core::Status::Down => "down",
        crate::core::Status::Warn => "warn",
        crate::core::Status::Info => "info",
    }
}

/// A file name for a set of results that says what it is and when it was
/// taken.
pub fn suggested_name(tool: &str, title: &str) -> String {
    let stem: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' { c } else { '-' })
        .collect();
    let stem = stem.trim_matches('-').to_lowercase();
    let stem = if stem.is_empty() { tool.to_string() } else { stem };
    format!("{stem}-{}.csv", crate::dl::names::today())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Status, col};

    fn row(cells: &[&str], target: &str, status: Status) -> Row {
        Row {
            cells: cells.iter().map(|c| c.to_string()).collect(),
            status,
            target: target.into(),
            note: None,
            key: None,
        }
    }

    #[test]
    fn the_header_names_the_columns_and_what_the_table_adds() {
        let columns = [col("HOST", 20), col("RTT", 10)];
        let out = csv(&columns, &[], &BTreeMap::new());
        assert_eq!(out, "HOST,RTT,Target,Status,Note\r\n");
    }

    #[test]
    fn a_row_carries_its_status_and_its_note() {
        let columns = [col("HOST", 20), col("RTT", 10)];
        let rows = [row(&["10.0.0.1", "2.4 ms"], "10.0.0.1", Status::Up)];
        let mut notes = BTreeMap::new();
        notes.insert("10.0.0.1".to_string(), "the router".to_string());

        let out = csv(&columns, &rows.iter().collect::<Vec<_>>(), &notes);
        assert!(out.ends_with("10.0.0.1,2.4 ms,10.0.0.1,up,the router\r\n"), "{out}");
    }

    #[test]
    fn anything_that_would_break_the_format_is_quoted() {
        // A note is free text, so it is the field most likely to contain a
        // comma, a quote or a newline.
        assert_eq!(field("plain"), "plain");
        assert_eq!(field("a,b"), "\"a,b\"");
        assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(field("two\nlines"), "\"two\nlines\"");
        assert_eq!(field(" padded "), "\" padded \"");
    }

    #[test]
    fn a_cell_the_scanned_host_wrote_cannot_arrive_as_a_live_formula() {
        // A TXT record, a Server: header and a service banner all land in a
        // cell exactly as they came off the wire.
        assert_eq!(field("=cmd|'/c calc'!A0"), "'=cmd|'/c calc'!A0");
        assert_eq!(field("+1+cmd|'/c calc'!A0"), "'+1+cmd|'/c calc'!A0");
        assert_eq!(field("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(field("-2+3+cmd|'/c calc'!A0"), "'-2+3+cmd|'/c calc'!A0");
        // A tab in front is dropped by the spreadsheet, so it is a way of
        // smuggling the same thing past a check on the first character.
        assert_eq!(field("\t=1+1"), "'\t=1+1");
    }

    #[test]
    fn a_guarded_cell_is_still_quoted_when_the_format_needs_it() {
        assert_eq!(field("=A1,B1"), "\"'=A1,B1\"");
        assert_eq!(field("=HYPERLINK(\"http://x\")"), "\"'=HYPERLINK(\"\"http://x\"\")\"");
    }

    #[test]
    fn a_measurement_and_the_dash_that_means_no_answer_are_written_as_they_are() {
        // Guarding these would turn a numeric column into text and leave an
        // apostrophe in front of every failed row.
        assert_eq!(field("-"), "-");
        assert_eq!(field("-12.5"), "-12.5");
        assert_eq!(field("2.4 ms"), "2.4 ms");
    }

    #[test]
    fn the_guard_reaches_the_line_the_export_writes() {
        let columns = [col("NAME", 20), col("VALUE", 40)];
        let rows = [row(&["evil.example", "=cmd|'/c calc'!A0"], "evil.example", Status::Info)];
        let out = csv(&columns, &rows.iter().collect::<Vec<_>>(), &BTreeMap::new());
        assert!(out.contains("evil.example,'=cmd|'/c calc'!A0,"), "{out}");
    }

    #[test]
    fn a_short_row_is_padded_to_the_columns() {
        // A tool that emits fewer cells than it declared would otherwise
        // shift every later field into the wrong column.
        let columns = [col("A", 5), col("B", 5), col("C", 5)];
        let rows = [row(&["one"], "x", Status::Info)];
        let out = csv(&columns, &rows.iter().collect::<Vec<_>>(), &BTreeMap::new());
        assert!(out.ends_with("one,,,x,info,\r\n"), "{out}");
    }

    #[test]
    fn the_file_is_named_after_what_it_holds() {
        let name = suggested_name("portscan", "Office gateway");
        assert!(name.starts_with("office-gateway-"), "{name}");
        assert!(name.ends_with(".csv"));
    }
}
