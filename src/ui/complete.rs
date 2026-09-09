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
    ///
    /// `path` is every name walked through to get here, outermost first: the
    /// run, then each field of its answer. One segment was enough while the
    /// only thing after a dot was a column, but an answer nests, and
    /// `"Health".json.queue.` can only be answered by something that knows
    /// all three.
    Field { path: Vec<String>, typed: String },
    /// After `run ` in a workflow, naming a tool. `quoted` records whether the
    /// name was opened with a quote, which decides whether accepting closes
    /// one.
    Run { typed: String, quoted: bool },
    /// After `with` on a `run` line, naming one of that tool's own fields.
    ///
    /// `run` is the only step whose settings are the tool's and not the
    /// language's, so the names offered here come from the tool the line
    /// already names.
    With { run: String, typed: String },
}

/// Works out what the caret is in the middle of.
///
/// `line` is the text of the line and `at` the byte offset of the caret in it.
pub fn context(language: Language, line: &str, at: usize) -> Context {
    let before = &line[..at.min(line.len())];

    match language {
        Language::Markdown => {
            // Only inside an expression: the prose around it is prose.
            let Some(open) = before.rfind("{{") else { return Context::Nowhere };
            if before[open..].contains("}}") {
                return Context::Nowhere;
            }
        }
        Language::Flow => {
            // A comment is not a place for suggestions.
            if before.contains('#') {
                return Context::Nowhere;
            }
            // `run "` names a tool, and the name may have spaces in it.
            if let Some(rest) = after_keyword(before, "run") {
                match closed_run(rest) {
                    // Once the name is closed, `with` may follow it, and
                    // what comes after that is a field of that tool.
                    //
                    // Past the `=` there is nothing to return: the caret is
                    // in an expression, and what belongs there is everything
                    // the workspace holds. That is the whole point of a
                    // setting — to point a tool at something worked out a
                    // moment ago — so it falls through to the ordinary path.
                    Some((run, Some(settings))) => {
                        if let Some(typed) = field_typed(&settings) {
                            return Context::With { run, typed };
                        }
                    }
                    // The name is finished and nothing follows it, so there
                    // is nothing to finish.
                    Some((_, None)) => return Context::Nowhere,
                    // Still naming the tool.
                    None => {
                        let opened =
                            rest.strip_prefix('"').or_else(|| rest.strip_prefix('\''));
                        return Context::Run {
                            quoted: opened.is_some(),
                            typed: opened.unwrap_or(rest).to_string(),
                        };
                    }
                }
            }
        }
        // The whole of it is the expression, so there is nothing to be
        // outside of and nothing to open first.
        Language::Expr => {}
        // A request body is data, not a place that names runs and variables.
        // Offering `Router.rtt` inside a piece of JSON would be offering to
        // write something the server will never read.
        Language::Json | Language::Xml | Language::Text => return Context::Nowhere,
    }

    // Stepping one byte past the delimiter assumed every delimiter is one
    // byte wide. An em dash, a curly quote or a degree sign is not, so the
    // word began inside a character and slicing there ended the program: in
    // the formula box that meant a panic on every frame, from one keystroke.
    let typed_start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let typed = before[typed_start..].to_string();

    let head = &before[..typed_start];
    let Some(head) = head.strip_suffix('.') else { return Context::Word(typed) };

    let path = dotted_path(head);
    if path.is_empty() { Context::Word(typed) } else { Context::Field { path, typed } }
}

/// The chain of names a dot was typed after, outermost first.
///
/// Read from the right, because that is the end the caret is at, and turned
/// round at the end. A segment is a bare word or a quoted name, since a run
/// called `IP scan` can only be written in quotes. Anything else — a bracket,
/// a call, an operator — ends the chain: what it evaluates to is not
/// something this can know without running it.
fn dotted_path(mut head: &str) -> Vec<String> {
    let mut parts = Vec::new();
    loop {
        head = head.trim_end();
        let quote = head.chars().last().filter(|c| *c == '"' || *c == '\'');
        if let Some(quote) = quote {
            let inner = &head[..head.len() - quote.len_utf8()];
            let Some(open) = inner.rfind(quote) else { return Vec::new() };
            parts.push(inner[open + quote.len_utf8()..].to_string());
            head = &inner[..open];
        } else {
            let start = head
                .char_indices()
                .rev()
                .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
                .map_or(0, |(i, c)| i + c.len_utf8());
            if start == head.len() {
                // Nothing name-shaped here: a `)` or a `]`, so stop.
                return Vec::new();
            }
            parts.push(head[start..].to_string());
            head = &head[..start];
        }
        match head.strip_suffix('.') {
            Some(rest) => head = rest,
            None => break,
        }
    }
    parts.reverse();
    parts
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
    Some(rest.trim_start())
}

/// A `run` line whose name is finished: the name, and whatever follows the
/// `with` after it.
///
/// `None` if the name is still being typed, which is the other thing the text
/// after `run` can be.
fn closed_run(rest: &str) -> Option<(String, Option<String>)> {
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let body = &rest[quote.len_utf8()..];
    let close = body.find(quote)?;
    let name = body[..close].to_string();
    let after = body[close + quote.len_utf8()..].trim_start();
    // An `if` after the name guards the step and is a condition, not a
    // setting, so only a `with` opens the settings.
    let Some(settings) = after_keyword(after, "with") else {
        return Some((name, None));
    };
    Some((name, Some(settings.to_string())))
}

/// Which field name is being typed in `target = x, por`.
///
/// `None` once the caret is past an `=`, where what is being written is the
/// value and not the name of a field.
fn field_typed(settings: &str) -> Option<String> {
    let last = settings.rsplit(',').next().unwrap_or(settings);
    if last.contains('=') {
        return None;
    }
    Some(last.trim().to_string())
}

/// What to offer, given where the caret is and what the workspace holds.
pub fn candidates(
    context: &Context,
    language: Language,
    tables: &[Table],
    vars: &[String],
) -> Vec<Candidate> {
    let mut out = Vec::new();

    let typed = match context {
        Context::Nowhere => return out,

        Context::Run { typed, .. } => {
            for table in tables {
                out.push(candidate(&table.name, "run", format!("{} · {}", table.tool, table.target)));
            }
            typed
        }

        // The fields of whichever tool the line names, which are the only
        // names a `with` can hold.
        Context::With { run, typed } => {
            let tool = tables
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(run))
                .or_else(|| tables.iter().find(|t| t.tool.eq_ignore_ascii_case(run)))
                .and_then(|t| crate::tools::all().get(&t.tool));
            if let Some(tool) = tool {
                for field in tool.fields() {
                    out.push(candidate(field.key, "setting", field.label));
                }
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
            // Only a workflow has steps. `repeat` in a formula would be a
            // name that resolves to nothing.
            if language == Language::Flow {
                for word in FLOW_WORDS {
                    out.push(candidate(word, "word", ""));
                }
            }
            typed
        }

        Context::Field { path, typed } => {
            let (subject, rest) = path.split_first().map_or(("", &[][..]), |(s, r)| (s.as_str(), r));
            let table = tables
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(subject))
                .or_else(|| tables.iter().find(|t| t.tool.eq_ignore_ascii_case(subject)));

            match (table, rest.is_empty()) {
                // A run answers to its fields, its columns, its figures and
                // whatever it came back with. A column is a list, and a list
                // only answers to methods: offering `up` after `Router.rtt.`
                // would be nonsense.
                (Some(table), true) => {
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
                    for (key, value) in &table.extras {
                        out.push(candidate(key, "answer", preview(value)));
                    }
                    out.push(candidate("col", "method", "a column by name"));
                    out.push(candidate("stat", "method", "a figure by name"));
                }
                // Further in: whatever the path arrived at decides what can
                // follow it.
                (Some(table), false) => match walk(table, rest) {
                    Some(crate::expr::Value::Object(fields)) => {
                        for (key, value) in fields.iter() {
                            out.push(candidate(key, "answer", preview(value)));
                        }
                        out.push(candidate("keys", "answer", "the names it has"));
                    }
                    // A list, or a value the path did not reach. Both answer
                    // to the methods and to nothing else.
                    _ => {
                        for (name, detail) in METHODS {
                            out.push(candidate(name, "method", detail));
                        }
                    }
                },
                _ => {
                    for (name, detail) in METHODS {
                        out.push(candidate(name, "method", detail));
                    }
                }
            }
            typed
        }
    };

    narrow(out, typed)
}

/// Follows a path of names into what a run answered.
///
/// The first name is one of its extras; the rest walk down through the
/// objects inside. Anything that is not an object stops the walk, which is
/// the right answer as well as the only one: there is nothing below a number.
fn walk(table: &Table, path: &[String]) -> Option<crate::expr::Value> {
    let (first, rest) = path.split_first()?;
    let mut value = table
        .extras
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(first))
        .map(|(_, value)| value.clone())?;
    for name in rest {
        value = value.field(name)?;
    }
    Some(value)
}

/// A word or two about a value, for the row it is offered on.
///
/// What is actually there, kept short. Seeing `depth 12` beside a name is
/// what tells you the path you are typing is the right one, without leaving
/// the line to go and look at the answer.
pub fn preview(value: &crate::expr::Value) -> String {
    use crate::expr::Value;
    const MOST: usize = 40;
    let text = match value {
        Value::Object(fields) => {
            let names: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).take(4).collect();
            if names.is_empty() { "{}".to_string() } else { format!("{{ {} }}", names.join(", ")) }
        }
        Value::List(items) => format!("{} of them", items.len()),
        Value::Nothing => "nothing".to_string(),
        other => other.show(),
    };
    let text = text.replace('\n', " ");
    match text.char_indices().nth(MOST) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

/// Which kinds are worth offering first when they match equally well.
///
/// What is in this workspace beats what is in the language: a document about a
/// run called Router means the run, not the `round` function.
fn priority(kind: &str) -> u8 {
    match kind {
        "run" => 0,
        // What this particular run came back with, which is the most
        // specific thing there is to offer and was being ranked below the
        // nine fields every run has. With the list capped, an answer's own
        // names fell off the end of it: the run had them, the language knew
        // about them, and they could not be found.
        "answer" | "setting" => 1,
        "column" | "figure" | "variable" => 2,
        "field" => 3,
        "method" | "function" => 4,
        _ => 5,
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
        Context::Word(typed) | Context::Run { typed, .. } | Context::With { typed, .. } => {
            typed.len()
        }
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

    /// A run that came back with an answer, the way an HTTP request does.
    fn answered() -> Vec<Table> {
        use crate::expr::Value;
        use std::sync::Arc;

        let queue = Value::Object(Arc::new(vec![
            ("depth".into(), Value::Number(12.0)),
            ("name".into(), Value::Text("mail".into())),
        ]));
        let json = Value::Object(Arc::new(vec![
            ("queue".into(), queue),
            ("hosts".into(), Value::List(vec![Value::Text("a".into())])),
        ]));
        let response = Value::Object(Arc::new(vec![
            ("status".into(), Value::Number(200.0)),
            ("reason".into(), Value::Text("OK".into())),
        ]));
        vec![Table {
            name: "HTTP request".into(),
            tool: "http".into(),
            target: "example.com".into(),
            columns: vec!["#".into(), "STATUS".into()],
            stats: vec![("sent".into(), "1".into())],
            extras: vec![("json".into(), json), ("response".into(), response)],
            ..Table::default()
        }]
    }

    /// Everything offered at the end of this line.
    fn offered(line: &str) -> Vec<Candidate> {
        let found = context(Language::Markdown, line, line.len());
        candidates(&found, Language::Markdown, &answered(), &[])
    }

    /// Just the names, for asserting on what is and is not there.
    fn names(line: &str) -> Vec<String> {
        offered(line).into_iter().map(|c| c.text).collect()
    }

    #[test]
    fn what_a_run_answered_is_offered_beside_its_columns() {
        // The columns of the request table were all that was offered, so
        // nothing the server actually said could be found without knowing it
        // was there and typing the whole path from memory.
        let offered = names(r#"{{ "HTTP request"."#);
        let has = |name: &str| offered.iter().any(|n| n == name);
        assert!(has("json"), "{offered:?}");
        assert!(has("response"), "{offered:?}");
        // Beside, not instead of.
        assert!(has("status"), "{offered:?}");
        assert!(has("sent"), "{offered:?}");
    }

    #[test]
    fn a_dot_after_an_answer_offers_what_is_inside_it() {
        let offered = names(r#"{{ "HTTP request".json."#);
        let has = |name: &str| offered.iter().any(|n| n == name);
        assert!(has("queue"), "{offered:?}");
        assert!(has("hosts"), "{offered:?}");
        // And not the run's own columns, which have nothing to do with what
        // is inside the answer.
        assert!(!has("status"), "{offered:?}");
    }

    #[test]
    fn the_walk_goes_as_deep_as_the_answer_does() {
        let deep = offered(r#"{{ "HTTP request".json.queue."#);
        let has = |name: &str| deep.iter().any(|c| c.text == name);
        assert!(has("depth") && has("name"), "{deep:?}");
        // What is actually there is shown beside the name, so the path can
        // be checked without leaving the line to go and look at the answer.
        let depth = deep.iter().find(|c| c.text == "depth").expect("depth");
        assert_eq!(depth.detail, "12");
    }

    #[test]
    fn a_value_with_nothing_below_it_offers_the_methods_and_not_a_guess() {
        let list = names(r#"{{ "HTTP request".json.hosts."#);
        assert!(list.iter().any(|n| n == "count"), "a list answers to methods: {list:?}");

        let number = names(r#"{{ "HTTP request".json.queue.depth."#);
        assert!(number.iter().any(|n| n == "fixed"), "{number:?}");
        assert!(!number.iter().any(|n| n == "depth"), "nothing is below a number: {number:?}");
    }

    #[test]
    fn a_path_is_read_whole_however_it_is_written() {
        assert_eq!(
            context(Language::Expr, "Router.json.queue.de", 20),
            Context::Field {
                path: vec!["Router".into(), "json".into(), "queue".into()],
                typed: "de".into()
            }
        );
        // A name in quotes is one segment, spaces and all.
        assert_eq!(
            context(Language::Expr, r#""IP scan".json.up"#, 17),
            Context::Field { path: vec!["IP scan".into(), "json".into()], typed: "up".into() }
        );
        // Something that is not a name ends the chain rather than being
        // guessed at: what a call returns is not knowable from the text.
        assert_eq!(context(Language::Expr, "count(x).", 9), Context::Word(String::new()));
    }

    #[test]
    fn a_with_on_a_run_line_offers_that_tools_own_fields() {
        // The names a `with` can hold belong to the tool, not to the
        // language, so nothing else in the workspace knows them.
        let tables = vec![Table {
            name: "Scan".into(),
            tool: "portscan".into(),
            target: "10.0.0.1".into(),
            ..Table::default()
        }];
        let line = r#"run "Scan" with ta"#;
        let found = context(Language::Flow, line, line.len());
        assert_eq!(found, Context::With { run: "Scan".into(), typed: "ta".into() });

        let offered = candidates(&found, Language::Flow, &tables, &[]);
        assert!(
            offered.iter().any(|c| c.text == "target" && c.kind == "setting"),
            "{offered:?}"
        );
    }

    #[test]
    fn the_second_setting_is_offered_the_same_way_as_the_first() {
        let tables = vec![Table {
            name: "Scan".into(),
            tool: "portscan".into(),
            ..Table::default()
        }];
        let line = r#"run "Scan" with target = host, po"#;
        let found = context(Language::Flow, line, line.len());
        assert_eq!(found, Context::With { run: "Scan".into(), typed: "po".into() });
        let offered = candidates(&found, Language::Flow, &tables, &[]);
        assert!(offered.iter().any(|c| c.text == "ports"), "{offered:?}");

        // Past the `=` it is a value, and a value is an expression: what the
        // workspace holds, not what the tool is called.
        let line = r#"run "Scan" with target = Sw"#;
        assert_eq!(context(Language::Flow, line, line.len()), Context::Word("Sw".into()));
    }

    #[test]
    fn a_run_whose_name_is_still_being_typed_is_still_naming_a_run() {
        let line = r#"run "Sca"#;
        assert_eq!(
            context(Language::Flow, line, line.len()),
            Context::Run { typed: "Sca".into(), quoted: true }
        );
        // And a finished name with nothing after it has nothing to finish.
        let line = r#"run "Scan""#;
        assert_eq!(context(Language::Flow, line, line.len()), Context::Nowhere);
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
    fn a_character_wider_than_one_byte_is_not_sliced_through() {
        // The start of the word was found by stepping one byte past the
        // delimiter, so a delimiter wider than a byte put it inside a
        // character. Reading a formula happens on every frame, so one
        // degree sign typed into one meant a panic on every frame after.
        for line in ["a\u{b0}", "x \u{2019}", "{{ a\u{2014}b", "run x\u{2014}y", "Router.rtt\u{2014}"] {
            for language in [Language::Expr, Language::Flow, Language::Markdown] {
                for caret in 0..=line.len() {
                    if line.is_char_boundary(caret) {
                        let _ = context(language, line, caret);
                    }
                }
            }
        }
        // And the word under the caret is still the word.
        assert_eq!(context(Language::Expr, "a\u{b0}Rou", 6), Context::Word("Rou".into()));
    }

    #[test]
    fn a_formula_is_all_expression_with_nothing_to_open_first() {
        // A document needs `{{` before anything is offered. A formula is the
        // expression, so the first character typed into one is already in it.
        assert_eq!(context(Language::Markdown, "Rou", 3), Context::Nowhere);
        assert_eq!(context(Language::Expr, "Rou", 3), Context::Word("Rou".into()));
        assert_eq!(
            context(Language::Expr, "Router.rt", 9),
            Context::Field { path: vec!["Router".into()], typed: "rt".into() }
        );

        let offered = candidates(&Context::Word("Rou".into()), Language::Expr, &tables(), &[]);
        assert_eq!(offered[0].text, "Router");
    }

    #[test]
    fn what_the_workspace_holds_is_offered_before_what_the_language_has() {
        // `text` and `tool` are functions and `threshold` is a variable of
        // this workspace. The one that was written here is the one meant.
        let vars = vec!["threshold".to_string()];
        let offered = candidates(&Context::Word("t".into()), Language::Expr, &[], &vars);
        assert_eq!(offered[0].text, "threshold");
    }

    #[test]
    fn a_formula_is_not_offered_the_words_a_workflow_is_written_with() {
        // `repeat` is a step. A formula that said it would be naming
        // something that does not exist.
        let offered = candidates(&Context::Word("rep".into()), Language::Expr, &[], &[]);
        assert!(offered.iter().all(|c| c.text != "repeat"), "{offered:?}");

        let in_a_flow = candidates(&Context::Word("rep".into()), Language::Flow, &[], &[]);
        assert!(in_a_flow.iter().any(|c| c.text == "repeat"));

        // The words an expression does have are still offered to one.
        let words = candidates(&Context::Word("if".into()), Language::Expr, &[], &[]);
        assert_eq!(words[0].text, "if");
    }

    #[test]
    fn a_dot_asks_about_the_thing_before_it() {
        let found = context(Language::Markdown, "{{ Router.rt", 12);
        assert_eq!(found, Context::Field { path: vec!["Router".into()], typed: "rt".into() });

        let offered = candidates(&found, Language::Markdown, &tables(), &[]);
        assert_eq!(offered[0].text, "rtt", "the column it is a prefix of");
    }

    #[test]
    fn a_run_is_offered_where_a_workflow_names_one() {
        assert_eq!(
            context(Language::Flow, "run \"Rou", 8),
            Context::Run { typed: "Rou".into(), quoted: true }
        );
        let offered = candidates(&Context::Run { typed: "Rou".into(), quoted: true }, Language::Flow, &tables(), &[]);
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
        let offered = candidates(&word, Language::Markdown, &spaced, &[]);
        assert_eq!(offered[0].text, "IP scan");
        assert_eq!(insertion(&word, &offered[0]), "\"IP scan\"");

        // And once it is written, the dot after it asks about that run.
        let after = context(Language::Markdown, "{{ \"IP scan\".ro", 17);
        assert_eq!(after, Context::Field { path: vec!["IP scan".into()], typed: "ro".into() });
        let offered = candidates(&after, Language::Markdown, &spaced, &[]);
        assert_eq!(offered[0].text, "rows");
        // What is replaced is only what was typed after the dot.
        assert_eq!(replacing(&after), 2);
    }

    #[test]
    fn a_run_in_this_workspace_beats_a_function_of_the_same_shape() {
        // "Rou" matches both the run called Router and the `round` function;
        // the document is about the run.
        let offered = candidates(&Context::Word("Rou".into()), Language::Markdown, &tables(), &[]);
        assert_eq!(offered[0].text, "Router");
        assert!(offered.iter().any(|c| c.text == "round"));
    }

    #[test]
    fn a_comment_is_not_a_place_for_suggestions() {
        assert_eq!(context(Language::Flow, "run A  # about Rou", 18), Context::Nowhere);
    }

    #[test]
    fn the_fields_every_run_has_are_offered_whatever_the_tool() {
        let found = Context::Field { path: vec!["Router".into()], typed: String::new() };
        let offered = candidates(&found, Language::Markdown, &tables(), &[]);
        let texts: Vec<&str> = offered.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"up"));
        assert!(texts.contains(&"rows"));
        // And the figures the tool itself published.
        assert!(texts.contains(&"loss"));
    }

    #[test]
    fn nothing_is_offered_for_a_subject_that_is_not_there() {
        let found = Context::Field { path: vec!["Nowhere".into()], typed: "rt".into() };
        let offered = candidates(&found, Language::Markdown, &tables(), &[]);
        // The fields every run has still apply; the columns of a run that does
        // not exist do not.
        assert!(!offered.iter().any(|c| c.text == "rtt"));
    }

    #[test]
    fn a_column_is_a_list_and_only_answers_to_methods() {
        // `Router.rtt.` is a list; offering `up` after it would be nonsense.
        let found = Context::Field { path: vec!["rtt".into()], typed: String::new() };
        let offered = candidates(&found, Language::Markdown, &tables(), &[]);
        let texts: Vec<&str> = offered.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"avg"));
        assert!(!texts.contains(&"up"));
        assert!(!texts.contains(&"rows"));
    }

    #[test]
    fn a_column_that_cannot_follow_a_dot_is_not_offered_as_one() {
        // `#` is a column of the ping table, and `Router.#` is not writable.
        let found = Context::Field { path: vec!["Router".into()], typed: String::new() };
        let offered = candidates(&found, Language::Markdown, &tables(), &[]);
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
        assert_eq!(replacing(&Context::Field { path: vec!["a".into()], typed: "rt".into() }), 2);
        assert_eq!(replacing(&Context::Nowhere), 0);
    }

    #[test]
    fn exact_matches_and_prefixes_rank_first() {
        let offered = candidates(&Context::Word("Rou".into()), Language::Markdown, &tables(), &[]);
        assert_eq!(offered[0].text, "Router");
        // Exact match or prefix match is offered first
        let offered_field = candidates(&Context::Field { path: vec!["Router".into()], typed: "rtt".into() }, Language::Markdown, &tables(), &[]);
        assert_eq!(offered_field[0].text, "rtt");
    }
}
