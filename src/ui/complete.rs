//! What the editor offers to finish for you.
//!
//! A document's expressions and a workflow's steps both name things that exist
//! the runs in the workspace, their columns, their summary figures, and the
//! functions the expression language has. All of those are known, so none of
//! them should have to be typed out or remembered.

use crate::expr::Table;
use crate::ui::syntax::{FLOW_WORDS, FUNCTIONS, Language};

/// One thing that can be filled in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// What gets inserted.
    pub text: String,
    /// What it is, for the row: a run, a column, a function.
    pub kind: &'static str,
    /// A word or two about it.
    pub detail: String,
}

fn candidate(text: &str, kind: &'static str, detail: impl Into<String>) -> Candidate {
    Candidate { text: text.to_string(), kind, detail: detail.into() }
}

/// What a list can be asked. A column is a list.
const METHODS: [(&str, &str); 14] = [
    ("avg", "the mean"),
    ("min", "the smallest"),
    ("max", "the largest"),
    ("sum", "added up"),
    ("count", "how many"),
    ("median", "the middle one"),
    ("p95", "the 95th percentile"),
    ("percentile", "any percentile"),
    ("first", "the first"),
    ("last", "the last"),
    ("sort", "in order"),
    ("contains", "whether it holds a value"),
    ("join", "as one line of text"),
    ("fixed", "to a number of places"),
];

/// A column name that can be written after a dot. `#` cannot, and is reached
/// with `col("#")` instead.
fn as_name(column: &str) -> Option<String> {
    let lower = column.to_lowercase();
    let usable = lower.chars().all(|c| c.is_alphanumeric() || c == '_')
        && lower.chars().next().is_some_and(|c| c.is_alphabetic());
    usable.then_some(lower)
}

/// The fields every run answers to, whatever tool it is.
pub const FIELDS: [(&str, &str); 9] = [
    ("rows", "how many results"),
    ("up", "results that answered"),
    ("down", "results that did not"),
    ("warn", "ambiguous results"),
    ("target", "what it was pointed at"),
    ("state", "ready, running, done, stopped, failed"),
    ("ok", "whether it finished"),
    ("elapsed", "how long it took, in seconds"),
    ("name", "what the run is called"),
];

/// Where the caret is, as far as completion is concerned.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Context {
    /// Not somewhere anything can be offered.
    Nowhere,
    /// Inside `{{ }}`, or anywhere in a workflow line, typing a bare word.
    Word(String),
    /// After `subject.`, typing the part after the dot.
    Field { subject: String, typed: String },
    /// After `run ` in a workflow, naming a tool. `quoted` records whether the
    /// name was opened with a quote, which decides whether accepting closes
    /// one.
    Run { typed: String, quoted: bool },
}

/// Works out what the caret is in the middle of.
///
/// `line` is the text of the line and `at` the byte offset of the caret in it.
pub fn context(language: Language, line: &str, at: usize) -> Context {
    let before = &line[..at.min(line.len())];

    if language == Language::Markdown {
        // Only inside an expression: the prose around it is prose.
        let Some(open) = before.rfind("{{") else { return Context::Nowhere };
        if before[open..].contains("}}") {
            return Context::Nowhere;
        }
    } else {
        // A comment is not a place for suggestions.
        if before.contains('#') {
            return Context::Nowhere;
        }
        // `run "` names a tool, and the name may have spaces in it.
        if let Some(rest) = after_keyword(before, "run") {
            let opened = rest.strip_prefix('"').or_else(|| rest.strip_prefix('\''));
            return Context::Run {
                quoted: opened.is_some(),
                typed: opened.unwrap_or(rest).to_string(),
            };
        }
    }

    // A run whose name has a space in it is named in quotes, so the thing
    // being asked about may be a quoted name instead of a bare word.
    if let Some((subject, typed)) = quoted_subject(before) {
        return Context::Field { subject, typed };
    }

    let word_start = before
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .map_or(0, |i| i + 1);
    let word = &before[word_start..];

    match word.rsplit_once('.') {
        Some((subject, typed)) if !subject.is_empty() => Context::Field {
            subject: subject.rsplit('.').next().unwrap_or(subject).to_string(),
            typed: typed.to_string(),
        },
        _ => Context::Word(word.to_string()),
    }
}

/// `"IP scan".ro`: the name in the quotes, and what has been typed after the
/// dot that follows them.
fn quoted_subject(before: &str) -> Option<(String, String)> {
    let (head, typed) = before.rsplit_once('.')?;
    if !typed.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let quote = head.chars().last().filter(|c| *c == '"' || *c == '\'')?;
    let head = &head[..head.len() - quote.len_utf8()];
    let open = head.rfind(quote)?;
    Some((head[open + quote.len_utf8()..].to_string(), typed.to_string()))
}

/// The text after `run`, if the line is one and nothing has closed it.
fn after_keyword<'a>(before: &'a str, word: &str) -> Option<&'a str> {
    let trimmed = before.trim_start();
    let rest = trimmed.get(..word.len()).filter(|h| h.eq_ignore_ascii_case(word))?;
    let _ = rest;
    let rest = &trimmed[word.len()..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    // Once the name is closed, or an `if` has started, this is no longer it.
    let closed = rest.starts_with('"') && rest[1..].contains('"')
        || rest.starts_with('\'') && rest[1..].contains('\'');
    if closed {
        return None;
    }
    Some(rest)
}

/// What to offer, given where the caret is and what the workspace holds.
pub fn candidates(context: &Context, tables: &[Table], vars: &[String]) -> Vec<Candidate> {
    let mut out = Vec::new();

    let typed = match context {
        Context::Nowhere => return out,

        Context::Run { typed, .. } => {
            for table in tables {
                out.push(candidate(&table.name, "run", format!("{} · {}", table.tool, table.target)));
            }
            typed
        }

        Context::Word(typed) => {
            for table in tables {
                out.push(candidate(&table.name, "run", format!("{} · {}", table.tool, table.target)));
            }
            // What the workspace has written down, which reads the same way a
            // run does and is offered beside them.
            for name in vars {
                out.push(candidate(name, "variable", "a variable of this workspace"));
            }
            for name in FUNCTIONS {
                out.push(candidate(name, "function", ""));
            }
            for word in ["if", "then", "else", "true", "false", "nothing"] {
                out.push(candidate(word, "word", ""));
            }
            for word in FLOW_WORDS {
                out.push(candidate(word, "word", ""));
            }
            typed
        }

        Context::Field { subject, typed } => {
            let table = tables
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(subject))
                .or_else(|| tables.iter().find(|t| t.tool.eq_ignore_ascii_case(subject)));

            // A run answers to its fields, its columns and its figures. A
            // column is a list, and a list only answers to methods. Offering
            // `up` after `Router.rtt.` would be nonsense.
            if let Some(table) = table {
                for (name, detail) in FIELDS {
                    out.push(candidate(name, "field", detail));
                }
                for column in &table.columns {
                    if let Some(name) = as_name(column) {
                        out.push(candidate(&name, "column", "a column of results"));
                    }
                }
                for (key, value) in &table.stats {
                    out.push(candidate(key, "figure", value.clone()));
                }
                out.push(candidate("col", "method", "a column by name"));
                out.push(candidate("stat", "method", "a figure by name"));
            } else {
                for (name, detail) in METHODS {
                    out.push(candidate(name, "method", detail));
                }
            }
            typed
        }
    };

    narrow(out, typed)
}

/// Which kinds are worth offering first when they match equally well.
///
/// What is in this workspace beats what is in the language: a document about a
/// run called Router means the run, not the `round` function.
fn priority(kind: &str) -> u8 {
    match kind {
        "run" => 0,
        "column" | "figure" => 1,
        "field" => 2,
        "method" | "function" => 3,
        _ => 4,
    }
}

/// Keeps what matches, best first, and drops duplicates.
fn narrow(all: Vec<Candidate>, typed: &str) -> Vec<Candidate> {
    let needle = typed.to_lowercase();
    let mut scored: Vec<(u8, usize, Candidate)> = all
        .into_iter()
        .enumerate()
        .filter_map(|(i, c)| {
            if needle.is_empty() {
                return Some((1, i, c));
            }
            let lower = c.text.to_lowercase();
            let rank = if lower == needle {
                0
            } else if lower.starts_with(&needle) {
                1
            } else if lower.split(['_', '-', ' ']).any(|part| part.starts_with(&needle)) {
                2
            } else if lower.contains(&needle) {
                3
            } else {
                return None;
            };
            Some((rank, i, c))
        })
        .collect();

    // With something typed, the shortest match is the one most likely meant.
    // With nothing typed, the order they were offered in is the useful one:
    // this run's own columns before the methods that work on anything.
    scored.sort_by(|a, b| {
        let by_length = if needle.is_empty() {
            std::cmp::Ordering::Equal
        } else {
            a.2.text.len().cmp(&b.2.text.len())
        };
        a.0
            .cmp(&b.0)
            .then(priority(a.2.kind).cmp(&priority(b.2.kind)))
            .then(by_length)
            .then(a.1.cmp(&b.1))
    });
    let mut seen = std::collections::HashSet::new();
    scored
        .into_iter()
        .map(|(_, _, c)| c)
        .filter(|c| seen.insert((c.text.clone(), c.kind)))
        .take(14)
        .collect()
}

/// What accepting a candidate actually inserts.
///
/// A function is being called, so it gets its bracket; a name in a workflow's
/// `run "…"` gets its closing quote. Finishing the token is the point of
/// completing it.
pub fn insertion(context: &Context, candidate: &Candidate) -> String {
    match (context, candidate.kind) {
        // A name with a space in it has to be quoted; one that was opened with
        // a quote has to be closed.
        (Context::Run { quoted: true, .. }, _) => format!("{}\"", candidate.text),
        (Context::Run { quoted: false, .. }, _)
            if candidate.text.contains(char::is_whitespace) =>
        {
            format!("\"{}\"", candidate.text)
        }
        (Context::Run { .. }, _) => candidate.text.clone(),
        // Everywhere else a run is named, it is named inside an expression,
        // where a name with a space in it has to be quoted to be read at all.
        (_, "run") if candidate.text.contains(char::is_whitespace) => {
            format!("{:?}", candidate.text)
        }
        (_, "function" | "method") if takes_no_argument(&candidate.text) => {
            format!("{}()", candidate.text)
        }
        (_, "function" | "method") => format!("{}(", candidate.text),
        _ => candidate.text.clone(),
    }
}

/// The summarising functions take only the thing they are called on, so the
/// brackets can be closed for you.
fn takes_no_argument(name: &str) -> bool {
    matches!(
        name,
        "avg" | "mean" | "min" | "max" | "sum" | "count" | "median" | "p95" | "first" | "last"
            | "sort" | "round" | "floor" | "ceil" | "abs" | "number" | "text" | "upper" | "lower"
            | "len" | "length" | "tools"
    )
}

/// How much of what is typed the completion replaces.
pub fn replacing(context: &Context) -> usize {
    match context {
        Context::Nowhere => 0,
        Context::Word(typed) | Context::Run { typed, .. } => typed.len(),
        Context::Field { typed, .. } => typed.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> Vec<Table> {
        vec![Table {
            name: "Router".into(),
            tool: "ping".into(),
            target: "192.168.1.1".into(),
            columns: vec!["#".into(), "FROM".into(), "RTT".into()],
            stats: vec![("loss".into(), "0%".into())],
            ..Table::default()
        }]
    }

    #[test]
    fn prose_is_prose_and_an_expression_is_not() {
        assert_eq!(context(Language::Markdown, "just writing", 12), Context::Nowhere);
        assert_eq!(
            context(Language::Markdown, "the average is {{ Rou", 21),
            Context::Word("Rou".into())
        );
        // Once the expression is closed, it is prose again.
        assert_eq!(context(Language::Markdown, "{{ a }} and", 11), Context::Nowhere);
    }

    #[test]
    fn a_dot_asks_about_the_thing_before_it() {
        let found = context(Language::Markdown, "{{ Router.rt", 12);
        assert_eq!(found, Context::Field { subject: "Router".into(), typed: "rt".into() });

        let offered = candidates(&found, &tables(), &[]);
        assert_eq!(offered[0].text, "rtt", "the column it is a prefix of");
    }

    #[test]
    fn a_run_is_offered_where_a_workflow_names_one() {
        assert_eq!(
            context(Language::Flow, "run \"Rou", 8),
            Context::Run { typed: "Rou".into(), quoted: true }
        );
        let offered = candidates(&Context::Run { typed: "Rou".into(), quoted: true }, &tables(), &[]);
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].text, "Router");

        // Without a quote it is still a name being typed, and accepting adds
        // the quotes if the name needs them.
        assert_eq!(
            context(Language::Flow, "run Rou", 7),
            Context::Run { typed: "Rou".into(), quoted: false }
        );

        // Once the name is closed it is no longer being typed.
        assert!(!matches!(
            context(Language::Flow, "run \"Router\" if ", 16),
            Context::Run { .. }
        ));
    }

    #[test]
    fn a_run_whose_name_has_a_space_is_offered_with_the_quotes_it_needs() {
        let spaced = vec![Table {
            name: "IP scan".into(),
            tool: "ipscan".into(),
            columns: vec!["HOST".into(), "RTT".into()],
            ..Table::default()
        }];

        // Offered in a document, it comes with its quotes, because
        // `{{ IP scan.rows }}` is not an expression.
        let word = Context::Word("IP".into());
        let offered = candidates(&word, &spaced, &[]);
        assert_eq!(offered[0].text, "IP scan");
        assert_eq!(insertion(&word, &offered[0]), "\"IP scan\"");

        // And once it is written, the dot after it asks about that run.
        let after = context(Language::Markdown, "{{ \"IP scan\".ro", 17);
        assert_eq!(after, Context::Field { subject: "IP scan".into(), typed: "ro".into() });
        let offered = candidates(&after, &spaced, &[]);
        assert_eq!(offered[0].text, "rows");
        // What is replaced is only what was typed after the dot.
        assert_eq!(replacing(&after), 2);
    }

    #[test]
    fn a_run_in_this_workspace_beats_a_function_of_the_same_shape() {
        // "Rou" matches both the run called Router and the `round` function;
        // the document is about the run.
        let offered = candidates(&Context::Word("Rou".into()), &tables(), &[]);
        assert_eq!(offered[0].text, "Router");
        assert!(offered.iter().any(|c| c.text == "round"));
    }

    #[test]
    fn a_comment_is_not_a_place_for_suggestions() {
        assert_eq!(context(Language::Flow, "run A  # about Rou", 18), Context::Nowhere);
    }

    #[test]
    fn the_fields_every_run_has_are_offered_whatever_the_tool() {
        let found = Context::Field { subject: "Router".into(), typed: String::new() };
        let offered = candidates(&found, &tables(), &[]);
        let texts: Vec<&str> = offered.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"up"));
        assert!(texts.contains(&"rows"));
        // And the figures the tool itself published.
        assert!(texts.contains(&"loss"));
    }

    #[test]
    fn nothing_is_offered_for_a_subject_that_is_not_there() {
        let found = Context::Field { subject: "Nowhere".into(), typed: "rt".into() };
        let offered = candidates(&found, &tables(), &[]);
        // The fields every run has still apply; the columns of a run that does
        // not exist do not.
        assert!(!offered.iter().any(|c| c.text == "rtt"));
    }

    #[test]
    fn a_column_is_a_list_and_only_answers_to_methods() {
        // `Router.rtt.` is a list; offering `up` after it would be nonsense.
        let found = Context::Field { subject: "rtt".into(), typed: String::new() };
        let offered = candidates(&found, &tables(), &[]);
        let texts: Vec<&str> = offered.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"avg"));
        assert!(!texts.contains(&"up"));
        assert!(!texts.contains(&"rows"));
    }

    #[test]
    fn a_column_that_cannot_follow_a_dot_is_not_offered_as_one() {
        // `#` is a column of the ping table, and `Router.#` is not writable.
        let found = Context::Field { subject: "Router".into(), typed: String::new() };
        let offered = candidates(&found, &tables(), &[]);
        assert!(!offered.iter().any(|c| c.text == "#"));
        // It is reached through `col` instead, which is.
        assert!(offered.iter().any(|c| c.text == "col"));
    }

    #[test]
    fn accepting_a_candidate_finishes_the_token() {
        let word = Context::Word(String::new());
        let avg = candidate("avg", "function", "");
        let percentile = candidate("percentile", "function", "");
        // A summary takes only what it is called on, so it is closed for you.
        assert_eq!(insertion(&word, &avg), "avg()");
        // One that takes more is left open, with the caret inside.
        assert_eq!(insertion(&word, &percentile), "percentile(");
        // A name is just a name.
        assert_eq!(insertion(&word, &candidate("Router", "run", "")), "Router");
        // And a workflow's run name closes its quote.
        assert_eq!(
            insertion(&Context::Run { typed: "Rou".into(), quoted: true }, &candidate("Router", "run", "")),
            "Router\""
        );
        assert_eq!(
            insertion(
                &Context::Run { typed: "Po".into(), quoted: false },
                &candidate("Port scan", "run", "")
            ),
            "\"Port scan\""
        );
    }

    #[test]
    fn what_is_replaced_is_what_has_been_typed() {
        assert_eq!(replacing(&Context::Word("Rou".into())), 3);
        assert_eq!(replacing(&Context::Run { typed: "Rou".into(), quoted: true }), 3);
        assert_eq!(replacing(&Context::Field { subject: "a".into(), typed: "rt".into() }), 2);
        assert_eq!(replacing(&Context::Nowhere), 0);
    }

    #[test]
    fn exact_matches_and_prefixes_rank_first() {
        let offered = candidates(&Context::Word("Rou".into()), &tables(), &[]);
        assert_eq!(offered[0].text, "Router");
        // Exact match or prefix match is offered first
        let offered_field = candidates(&Context::Field { subject: "Router".into(), typed: "rtt".into() }, &tables(), &[]);
        assert_eq!(offered_field[0].text, "rtt");
    }
}
