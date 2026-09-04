//! Working out what an expression comes to.

use std::sync::Arc;

use super::parse::Expr;
use super::value::{Table, Value, format_number};

/// Where an expression finds the results it refers to.
pub trait Source {
    /// One tool's results, by the name it is called. Matching is on the run's
    /// name first and the tool's title second.
    fn table(&self, name: &str) -> Option<Arc<Table>>;
    /// Every tool in scope, for `tools()` and for a name that names a tool
    /// type instead of a run.
    fn tables(&self) -> Vec<Arc<Table>>;
    /// A variable of the workspace, by name.
    ///
    /// A source that has none (a document evaluated on its own, a test)
    /// says so by saying nothing, and every name goes on being a run.
    fn var(&self, _name: &str) -> Option<String> {
        None
    }
}

/// A source with nothing in it, for a document evaluated on its own.
#[cfg(test)]
pub struct Empty;

#[cfg(test)]

impl Source for Empty {
    fn table(&self, _: &str) -> Option<Arc<Table>> {
        None
    }
    fn tables(&self) -> Vec<Arc<Table>> {
        Vec::new()
    }
}

pub fn eval(expr: &Expr, source: &dyn Source) -> Result<Value, String> {
    match expr {
        Expr::Number(n) => Ok(Value::Number(*n)),
        Expr::Text(s) => Ok(Value::Text(s.clone())),
        Expr::Name(name) => name_value(name, source),
        Expr::Field(subject, field) => {
            let subject = subject_value(subject, source)?;
            field_value(&subject, field)
        }
        Expr::Index(subject, key) => {
            let (subject, key) = (subject_value(subject, source)?, eval(key, source)?);
            index(&subject, &key)
        }
        Expr::Call(name, args) => {
            let args: Result<Vec<Value>, String> =
                args.iter().map(|a| eval(a, source)).collect();
            call(name, &args?, source)
        }
        // The subject of a method is a subject like any other, so
        // `"IP scan".count()` goes through the same door as `"IP scan".rows`.
        // Worked out as a plain argument it stayed the text of the name, and
        // `count` then answered about that text: one, for one piece of text,
        // where the run it named had three rows, and nothing said so.
        Expr::Method(subject, name, args) => {
            let mut values = vec![subject_value(subject, source)?];
            for arg in args {
                values.push(eval(arg, source)?);
            }
            call(name, &values, source)
        }
        Expr::Unary(op, a) => {
            let a = eval(a, source)?;
            match *op {
                "-" => Ok(Value::Number(-number(&a, "-")?)),
                _ => Ok(Value::Bool(!a.truth())),
            }
        }
        Expr::Binary(op, a, b) => binary(op, a, b, source),
        Expr::If(condition, then, otherwise) => {
            if eval(condition, source)?.truth() {
                eval(then, source)
            } else {
                eval(otherwise, source)
            }
        }
    }
}

/// What is being asked about, where something is being asked about it.
///
/// A run is normally named as a bare word, `Router.rtt`, but a run called
/// `IP scan` cannot be written that way, and most runs are called what their
/// tool is called. So a quoted name in front of a dot names the run too, and
/// `"IP scan".up` reads as well as `tool("IP scan").up` did.
fn subject_value(subject: &Expr, source: &dyn Source) -> Result<Value, String> {
    let Expr::Text(name) = subject else { return eval(subject, source) };
    match source.table(name) {
        Some(table) => Ok(Value::Table(table)),
        // Nothing else in the language answers to a field of a piece of text,
        // so this is what was meant however it turned out.
        None => Err(format!("nothing here is called {name}")),
    }
}

fn name_value(name: &str, source: &dyn Source) -> Result<Value, String> {
    match name {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        "nothing" => return Ok(Value::Nothing),
        _ => {}
    }
    // A bare name is a run, so `Router.rtt` reads as well as `tool("Router")`.
    if let Some(table) = source.table(name) {
        return Ok(Value::Table(table));
    }
    // Then a variable of the workspace. Runs come first because a document
    // that names one means the run; a variable that shadows a run is named
    // after it by mistake, and this way the mistake is the one that gives.
    if let Some(value) = source.var(name) {
        return Ok(Value::Text(value));
    }
    Err(format!("nothing here is called {name}"))
}

/// `subject[key]`: a list by position, a set of results by column name.
fn index(subject: &Value, key: &Value) -> Result<Value, String> {
    match subject {
        Value::List(items) => {
            let at = key.number().ok_or_else(|| "a list is indexed by position".to_string())?;
            let at = if at < 0.0 { items.len() as f64 + at } else { at };
            // Counting back from the end is the point of a negative position,
            // but reaching back past the start is no position at all, and
            // neither is the NaN that `0 / 0` comes to. Both were pulled up to
            // zero, so `rtt[-9]` on a three-row column quietly answered with
            // the first probe, where `rtt[99]` had rightly answered with
            // nothing, and a report showed a figure from the wrong row with
            // nothing anywhere to say so.
            if at.is_nan() || at < 0.0 {
                return Ok(Value::Nothing);
            }
            Ok(items.get(at as usize).cloned().unwrap_or(Value::Nothing))
        }
        Value::Table(_) => field_value(subject, &key.show()),
        Value::Nothing => Ok(Value::Nothing),
        other => Err(format!("{} cannot be indexed", other.kind())),
    }
}

fn field_value(subject: &Value, field: &str) -> Result<Value, String> {
    if let Value::List(items) = subject
        && matches!(field, "length" | "count" | "len")
    {
        return Ok(Value::Number(items.len() as f64));
    }
    let Value::Table(table) = subject else {
        // Reading a field of nothing is nothing, so a document about a tool
        // that has not run yet still renders.
        if matches!(subject, Value::Nothing) {
            return Ok(Value::Nothing);
        }
        return Err(format!("{} has no field {field}", subject.kind()));
    };

    Ok(match field {
        "name" => Value::Text(table.name.clone()),
        "tool" => Value::Text(table.tool.clone()),
        "target" => Value::Text(table.target.clone()),
        "state" => Value::Text(table.state.clone()),
        "rows" => Value::Number(table.rows.len() as f64),
        "columns" => {
            Value::List(table.columns.iter().cloned().map(Value::Text).collect())
        }
        "up" => Value::Number(table.count_status("up") as f64),
        "down" => Value::Number(table.count_status("down") as f64),
        "warn" => Value::Number(table.count_status("warn") as f64),
        "elapsed" => table.elapsed.map(Value::Number).unwrap_or(Value::Nothing),
        "ok" => Value::Bool(table.state == "done"),
        // Anything else names a column, then a figure from the summary. A
        // document reads `Ping.rtt.avg()` far better than `col("RTT")`.
        other => table
            .column(other)
            .or_else(|| table.stat(other))
            .ok_or_else(|| format!("{} has no {other}", table.name))?,
    })
}

fn binary(op: &str, a: &Expr, b: &Expr, source: &dyn Source) -> Result<Value, String> {
    // The logical operators only evaluate the right side when they must.
    if op == "&&" {
        let left = eval(a, source)?;
        return Ok(Value::Bool(left.truth() && eval(b, source)?.truth()));
    }
    if op == "||" {
        let left = eval(a, source)?;
        return Ok(if left.truth() { left } else { eval(b, source)? });
    }

    // Whether a `+` is spelling out a sentence is settled here, while the
    // expression is still in hand, because the answers no longer say.
    let joining = op == "+" && (joins_text(a) || joins_text(b));

    let (a, b) = (eval(a, source)?, eval(b, source)?);
    Ok(match op {
        "+" => match (&a, &b) {
            // `+` on text joins it, which is how a sentence is built.
            _ if joining => Value::Text(format!("{}{}", a.show(), b.show())),
            (Value::Text(_), _) | (_, Value::Text(_))
                if a.number().is_none() || b.number().is_none() =>
            {
                Value::Text(format!("{}{}", a.show(), b.show()))
            }
            _ => Value::Number(number(&a, "+")? + number(&b, "+")?),
        },
        "-" => Value::Number(number(&a, "-")? - number(&b, "-")?),
        "*" => Value::Number(number(&a, "*")? * number(&b, "*")?),
        "/" => Value::Number(number(&a, "/")? / number(&b, "/")?),
        "%" => Value::Number(number(&a, "%")? % number(&b, "%")?),
        "==" => Value::Bool(same(&a, &b)),
        "!=" => Value::Bool(!same(&a, &b)),
        "<" | "<=" | ">" | ">=" => {
            let (x, y) = (number(&a, op)?, number(&b, op)?);
            Value::Bool(match op {
                "<" => x < y,
                "<=" => x <= y,
                ">" => x > y,
                _ => x >= y,
            })
        }
        _ => return Err(format!("unknown operator {op}")),
    })
}

/// Whether a `+` is spelling out a sentence rather than adding figures.
///
/// Quoted text anywhere in the chain settles it, because quoting is what
/// somebody does to write prose. `Router.loss + Router.avg` names two cells
/// and nothing else, and goes on being addition.
///
/// The whole chain is looked at and not just the two sides of one `+`, because
/// `+` is left-associative and a join leaves text behind. In
/// `Router.up + "/" + Router.rows` the outer `+` sees only "2/3" and 3, and
/// "2/3" reads back as the figure it starts with, so judging that `+` by its
/// answers added them and put "5" in the middle of a report with nothing
/// anywhere to show that it had.
fn joins_text(expr: &Expr) -> bool {
    match expr {
        Expr::Text(_) => true,
        // `text(...)` is how somebody says they mean the writing and not the
        // figure, so it joins even with no sentence quoted around it.
        Expr::Call(name, _) => name == "text",
        Expr::Binary(op, a, b) => *op == "+" && (joins_text(a) || joins_text(b)),
        _ => false,
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.number(), b.number()) {
        (Some(x), Some(y)) => x == y,
        _ => a.show() == b.show(),
    }
}

fn number(value: &Value, op: &str) -> Result<f64, String> {
    value.number().ok_or_else(|| format!("{op} needs a number, not {}", value.kind()))
}

fn call(name: &str, args: &[Value], source: &dyn Source) -> Result<Value, String> {
    let first = args.first();
    let numbers = || first.map(Value::numbers).unwrap_or_default();

    Ok(match name {
        "tool" => {
            let wanted = args.first().map(Value::show).unwrap_or_default();
            match source.table(&wanted) {
                Some(table) => Value::Table(table),
                None => Value::Nothing,
            }
        }
        "tools" => Value::Number(source.tables().len() as f64),
        "exists" => Value::Bool(!matches!(first, None | Some(Value::Nothing))),

        "count" => match first {
            Some(Value::List(items)) => Value::Number(items.len() as f64),
            Some(Value::Table(t)) => Value::Number(t.rows.len() as f64),
            Some(Value::Nothing) | None => Value::Number(0.0),
            Some(_) => Value::Number(1.0),
        },
        "sum" => Value::Number(numbers().iter().sum()),
        "avg" | "mean" => {
            let ns = numbers();
            if ns.is_empty() {
                Value::Nothing
            } else {
                Value::Number(ns.iter().sum::<f64>() / ns.len() as f64)
            }
        }
        "min" => pick(numbers(), args, f64::min),
        "max" => pick(numbers(), args, f64::max),
        "median" => percentile(numbers(), 50.0),
        "p95" => percentile(numbers(), 95.0),
        "percentile" => {
            let want = args.get(1).and_then(Value::number).unwrap_or(50.0);
            percentile(numbers(), want)
        }

        // Shaping a list shapes what is in it, so `rtt.fixed(1)`
        // has to mean if it is to mean anything.
        "round" => shape(first, "round", |n| Value::Number(n.round()))?,
        "floor" => shape(first, "floor", |n| Value::Number(n.floor()))?,
        "ceil" => shape(first, "ceil", |n| Value::Number(n.ceil()))?,
        "abs" => shape(first, "abs", |n| Value::Number(n.abs()))?,
        "number" => first.and_then(Value::number).map(Value::Number).unwrap_or(Value::Nothing),
        "len" | "length" => match first {
            Some(Value::List(items)) => Value::Number(items.len() as f64),
            Some(Value::Table(t)) => Value::Number(t.rows.len() as f64),
            Some(Value::Text(s)) => Value::Number(s.chars().count() as f64),
            _ => Value::Number(0.0),
        },
        "sort" => match first {
            Some(Value::List(items)) => {
                let mut items = items.clone();
                // Two figures were held against each other by value while a
                // figure and a cell that is no figure were held against each
                // other by their writing, and those two orders disagree, so
                // together they were no order at all: 9 comes under 20 by
                // value, "20" comes under "3.4.5" by writing, and "3.4.5"
                // comes under "9" by writing. A column holding those three ran
                // in a circle, and the sort was free to leave it in whatever
                // order the merging happened to end at, or to give up and
                // panic in the middle of drawing a document. So a cell that is
                // no figure — the "-" of a probe that got no reply, an address
                // — is ordered by its writing and kept ahead of the figures,
                // which are ordered among themselves.
                items.sort_by(|a, b| match (a.number(), b.number()) {
                    (Some(x), Some(y)) => x.total_cmp(&y),
                    (None, None) => a.show().cmp(&b.show()),
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                });
                Value::List(items)
            }
            Some(other) => other.clone(),
            None => Value::Nothing,
        },
        "contains" => {
            let needle = args.get(1).map(Value::show).unwrap_or_default();
            match first {
                Some(Value::List(items)) => {
                    Value::Bool(items.iter().any(|i| i.show() == needle))
                }
                Some(Value::Table(t)) => Value::Bool(
                    t.rows.iter().any(|r| r.iter().any(|c| c.contains(&needle))),
                ),
                Some(other) => Value::Bool(other.show().contains(&needle)),
                None => Value::Bool(false),
            }
        }
        // The explicit forms, for a column or a figure whose name is not
        // something that can be written after a dot, `#` say.
        "col" | "column" => match (first, args.get(1)) {
            (Some(Value::Table(t)), Some(name)) => {
                t.column(&name.show()).unwrap_or(Value::Nothing)
            }
            _ => Value::Nothing,
        },
        "stat" => match (first, args.get(1)) {
            (Some(Value::Table(t)), Some(name)) => t.stat(&name.show()).unwrap_or(Value::Nothing),
            _ => Value::Nothing,
        },

        "first" => list_item(first, 0),
        "last" => match first {
            Some(Value::List(items)) if !items.is_empty() => items[items.len() - 1].clone(),
            _ => Value::Nothing,
        },
        "join" => {
            let glue = args.get(1).map(Value::show).unwrap_or_else(|| ", ".into());
            match first {
                Some(Value::List(items)) => Value::Text(
                    items.iter().map(Value::show).collect::<Vec<_>>().join(&glue),
                ),
                Some(other) => Value::Text(other.show()),
                None => Value::Text(String::new()),
            }
        }
        "text" => Value::Text(first.map(Value::show).unwrap_or_default()),
        "upper" => Value::Text(first.map(Value::show).unwrap_or_default().to_uppercase()),
        "lower" => Value::Text(first.map(Value::show).unwrap_or_default().to_lowercase()),
        "fixed" => {
            let places = args.get(1).and_then(Value::number).unwrap_or(2.0).clamp(0.0, 10.0);
            shape(first, "fixed", move |n| Value::Text(format!("{:.*}", places as usize, n)))?
        }
        "percent" => {
            let whole = args.get(1).and_then(Value::number).unwrap_or(1.0);
            shape(first, "percent", move |n| {
                Value::Text(format!("{}%", format_number(n / whole * 100.0)))
            })?
        }

        other => return Err(format!("there is no function called {other}")),
    })
}

/// Applies a function to a number, or to each number in a list, which is
/// what a column is.
fn shape(
    value: Option<&Value>,
    name: &str,
    f: impl Fn(f64) -> Value,
) -> Result<Value, String> {
    match value {
        Some(Value::List(items)) => {
            Ok(Value::List(items.iter().filter_map(Value::number).map(&f).collect()))
        }
        Some(Value::Nothing) | None => Ok(Value::Nothing),
        Some(other) => Ok(f(one(Some(other), name)?)),
    }
}

fn one(value: Option<&Value>, name: &str) -> Result<f64, String> {
    value
        .and_then(Value::number)
        .ok_or_else(|| format!("{name} needs a number"))
}

fn list_item(value: Option<&Value>, at: usize) -> Value {
    match value {
        Some(Value::List(items)) => items.get(at).cloned().unwrap_or(Value::Nothing),
        Some(other) if at == 0 => other.clone(),
        _ => Value::Nothing,
    }
}

fn pick(numbers: Vec<f64>, args: &[Value], choose: fn(f64, f64) -> f64) -> Value {
    // `min(a, b)` compares its arguments; `min(list)` summarises one.
    let numbers = if args.len() > 1 {
        args.iter().filter_map(Value::number).collect()
    } else {
        numbers
    };
    match numbers.into_iter().reduce(choose) {
        Some(n) => Value::Number(n),
        None => Value::Nothing,
    }
}

/// The percentile by linear interpolation between the two ranks it falls
/// between: the definition a spreadsheet uses, so the median of an even
/// number of samples is the midpoint of the middle two instead of one of
/// them.
fn percentile(mut numbers: Vec<f64>, want: f64) -> Value {
    if numbers.is_empty() {
        return Value::Nothing;
    }
    numbers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = want.clamp(0.0, 100.0) / 100.0 * (numbers.len() - 1) as f64;
    let (below, above) = (rank.floor() as usize, rank.ceil() as usize);
    let between = rank - below as f64;
    Value::Number(numbers[below] + (numbers[above] - numbers[below]) * between)
}

/// Works out one expression, from its source text.
pub fn run(source_text: &str, source: &dyn Source) -> Result<Value, String> {
    eval(&super::parse::parse(source_text)?, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct One(Arc<Table>);

    /// A run called what its tool is called, which most runs are.
    struct Spaced(Arc<Table>);

    impl Source for Spaced {
        fn table(&self, name: &str) -> Option<Arc<Table>> {
            (self.0.name.eq_ignore_ascii_case(name) || self.0.tool.eq_ignore_ascii_case(name))
                .then(|| self.0.clone())
        }
        fn tables(&self) -> Vec<Arc<Table>> {
            vec![self.0.clone()]
        }
    }

    impl Source for One {
        fn table(&self, name: &str) -> Option<Arc<Table>> {
            (self.0.name.eq_ignore_ascii_case(name) || self.0.tool.eq_ignore_ascii_case(name))
                .then(|| self.0.clone())
        }
        fn tables(&self) -> Vec<Arc<Table>> {
            vec![self.0.clone()]
        }
    }

    fn source() -> One {
        One(Arc::new(Table {
            name: "Router".into(),
            tool: "ping".into(),
            target: "192.168.1.1".into(),
            state: "done".into(),
            columns: vec!["#".into(), "FROM".into(), "RTT".into()],
            rows: vec![
                vec!["1".into(), "192.168.1.1".into(), "10.0 ms".into()],
                vec!["2".into(), "192.168.1.1".into(), "30.0 ms".into()],
                vec!["3".into(), "192.168.1.1".into(), "-".into()],
            ],
            statuses: vec!["up".into(), "up".into(), "down".into()],
            stats: vec![("loss".into(), "33%".into()), ("avg".into(), "20.0 ms".into())],
            elapsed: Some(3.0),
        }))
    }

    fn value(text: &str) -> Value {
        run(text, &source()).expect("it to evaluate")
    }

    #[test]
    fn a_run_whose_name_has_a_space_is_named_in_quotes() {
        // Most runs are called what their tool is called, and half the tools
        // are two words, so this is the common case, not the awkward one.
        let source = Spaced(Arc::new(Table {
            name: "IP scan".into(),
            tool: "ipscan".into(),
            columns: vec!["HOST".into(), "RTT".into()],
            rows: vec![
                vec!["10.0.0.1".into(), "1.0 ms".into()],
                vec!["10.0.0.2".into(), "3.0 ms".into()],
            ],
            statuses: vec!["up".into(), "up".into()],
            stats: vec![("up".into(), "2".into())],
            ..Table::default()
        }));
        let value = |text: &str| run(text, &source).expect("it to evaluate").show();
        assert_eq!(value(r#""IP scan".rows"#), "2");
        assert_eq!(value(r#""IP scan".rtt.avg()"#), "2");
        assert_eq!(value(r#""ip scan".up"#), "2", "however it is capitalised");
        // The long way round still works, and means the same thing.
        assert_eq!(value(r#"tool("IP scan").rows"#), "2");
        // A name that is not there says so, and does not complain about
        // text having no fields.
        let missing = run(r#""Nowhere".rows"#, &source).expect_err("an error");
        assert!(missing.contains("Nowhere"), "{missing}");
    }

    #[test]
    fn a_column_summarises_to_a_figure() {
        // The RTT column is text with units; the maths reads the figures out
        // of it and skips the probe that got no reply.
        assert_eq!(value("Router.rtt.avg()").show(), "20");
        assert_eq!(value("Router.rtt.max()").show(), "30");
        assert_eq!(value("count(Router.rtt)").show(), "3");
    }

    #[test]
    fn a_run_reports_on_itself() {
        assert_eq!(value("Router.rows").show(), "3");
        assert_eq!(value("Router.up").show(), "2");
        assert_eq!(value("Router.down").show(), "1");
        assert_eq!(value("Router.target").show(), "192.168.1.1");
        assert!(value("Router.ok").truth());
    }

    #[test]
    fn the_tools_own_summary_is_readable_by_name() {
        assert_eq!(value("Router.loss").show(), "33%");
        // And reads as a number where one is wanted.
        assert!(value("Router.loss > 30").truth());
    }

    #[test]
    fn a_conditional_chooses_between_two_answers() {
        assert_eq!(
            value(r#"if Router.rtt.avg() > 20 then "Degraded" else "Normal""#).show(),
            "Normal"
        );
        assert_eq!(
            value(r#"if Router.down > 0 then "Loss seen" else "Clean""#).show(),
            "Loss seen"
        );
    }

    #[test]
    fn text_and_numbers_join_the_way_a_sentence_does() {
        assert_eq!(value(r#""avg " + Router.rtt.avg() + " ms""#).show(), "avg 20 ms");
        assert_eq!(value("1 + 2").show(), "3");
    }

    #[test]
    fn a_join_that_has_already_happened_is_not_added_up_again() {
        // `+` is left-associative and a join leaves text behind, so the second
        // `+` here used to see "2/3" and 3. Text starting with a digit reads
        // back as a figure, so it added them and rendered "5" in the middle of
        // a report, with nothing to show anything had gone wrong.
        assert_eq!(value(r#"Router.up + "/" + Router.rows"#).show(), "2/3");
        assert_eq!(value(r#""" + 1 + 2"#).show(), "12");
        // Figures with nothing quoted anywhere are still added.
        assert_eq!(value("Router.up + Router.rows").show(), "5");
        assert_eq!(value("1 + 2 + 3").show(), "6");
    }

    #[test]
    fn a_method_on_a_quoted_run_name_answers_about_the_run() {
        // A quoted name in front of a dot names the run. That was true of
        // `"IP scan".rows` and not of `"IP scan".count()`, because parsing
        // folded the subject in with the arguments and nothing afterwards
        // could tell which it had been. So the method answered about the
        // piece of text it was handed rather than about the run, and nothing
        // anywhere said so.
        let source = Spaced(Arc::new(Table {
            name: "IP scan".into(),
            tool: "ipscan".into(),
            columns: vec!["HOST".into(), "RTT".into()],
            rows: vec![
                vec!["10.0.0.1".into(), "1.0 ms".into()],
                vec!["10.0.0.2".into(), "3.0 ms".into()],
            ],
            statuses: vec!["up".into(), "up".into()],
            stats: vec![("up".into(), "2".into())],
            ..Table::default()
        }));
        // However it is written, it is the same question about the same run.
        let both = |quoted: &str, long: &str| {
            let a = run(quoted, &source).map(|v| v.show());
            let b = run(long, &source).map(|v| v.show());
            assert_eq!(a, b, "{quoted} and {long} have to agree");
        };
        both(r#""IP scan".count()"#, r#"tool("IP scan").count()"#);
        both(r#""IP scan".col("HOST")"#, r#"tool("IP scan").col("HOST")"#);
        both(r#""IP scan".rows"#, r#"tool("IP scan").rows"#);

        // And a quoted argument is still only ever text.
        assert_eq!(run(r#"upper("ip scan")"#, &source).unwrap().show(), "IP SCAN");
    }

    #[test]
    fn a_tool_that_is_not_there_is_nothing_rather_than_an_error() {
        // A document written ahead of the scan still renders.
        assert_eq!(value(r#"tool("Missing")"#).show(), "");
        assert!(!value(r#"exists(tool("Missing"))"#).truth());
        assert!(value(r#"exists(tool("Router"))"#).truth());
        assert_eq!(value(r#"tool("Missing").rtt"#).show(), "");
    }

    /// A sweep of the forms someone would reasonably write, so that a method
    /// that does not work is found here instead of in a document.
    #[test]
    fn the_ways_a_figure_gets_written_all_work() {
        let source = source();
        let cases: [(&str, &str); 22] = [
            ("Router.rtt.avg()", "20"),
            ("avg(Router.rtt)", "20"),
            ("Router.rtt.avg().round()", "20"),
            ("Router.rtt.avg().fixed(1)", "20.0"),
            ("Router.rtt.min()", "10"),
            ("Router.rtt.max()", "30"),
            ("Router.rtt.sum()", "40"),
            ("Router.rtt.count()", "3"),
            ("Router.rtt.median()", "20"),
            ("Router.rtt.p95()", "29"),
            ("Router.rtt.percentile(50)", "20"),
            ("Router.rtt.first()", "10.0 ms"),
            ("Router.rtt.last()", "-"),
            ("Router.rows", "3"),
            ("Router.up", "2"),
            ("Router.elapsed.fixed(1)", "3.0"),
            ("Router.loss", "33%"),
            ("Router.name.upper()", "ROUTER"),
            ("Router.state", "done"),
            ("percent(Router.up, Router.rows)", "66.67%"),
            ("min(2, 9)", "2"),
            ("round(Router.rtt.avg() / 3)", "7"),
        ];
        for (text, want) in cases {
            let got = run(text, &source);
            assert_eq!(got.as_ref().map(Value::show).as_deref(), Ok(want), "{text}");
        }
    }

    #[test]
    fn a_list_answers_for_itself() {
        // A column is a list, and the things you want to know about one.
        assert_eq!(value("Router.rtt.length").show(), "3");
        assert_eq!(value("Router.rtt.len()").show(), "3");
        assert_eq!(value("Router.rtt.sort().first()").show(), "-");
        assert!(value("Router.rtt.contains(\"30.0 ms\")").truth());
        assert!(!value("Router.rtt.contains(\"99 ms\")").truth());
    }

    #[test]
    fn a_column_of_figures_and_addresses_sorts_the_same_way_whatever_order_it_arrives_in() {
        // Figures were compared by value and a figure against a cell that is
        // no figure by its writing, and the two orders disagree: 9 is under 20
        // by value, "20" is under "3.4.5" by writing, "3.4.5" is under "9" by
        // writing. Those three ran in a circle, so the answer depended on
        // which pairs the sort happened to look at, and the same column came
        // back in a different order depending on the order it went in.
        let cells = ["9", "3.4.5", "20", "-", "10.0.0.1"];
        let sorted = |order: &[&str]| {
            let list = Value::List(order.iter().map(|s| Value::Text((*s).into())).collect());
            call("sort", &[list], &Empty).expect("it to sort").show()
        };
        let mut backwards = cells.to_vec();
        backwards.reverse();
        assert_eq!(sorted(&cells), "-, 10.0.0.1, 3.4.5, 9, 20");
        assert_eq!(sorted(&backwards), sorted(&cells), "however the column arrives");
        // A column with a probe that got no reply in it still reads low to
        // high, with the missing one at the front, as it did before.
        assert_eq!(value("Router.rtt.sort()").show(), "-, 10.0 ms, 30.0 ms");
    }

    #[test]
    fn shaping_a_list_shapes_what_is_in_it() {
        // `rtt.fixed(1)` has to mean something, and this is the only thing it
        // can mean.
        assert_eq!(value("Router.rtt.fixed(1)").show(), "10.0, 30.0");
        assert_eq!(value("Router.rtt.round()").show(), "10, 30");
        // And a single figure still shapes as one.
        assert_eq!(value("Router.rtt.avg().fixed(1)").show(), "20.0");
    }

    #[test]
    fn a_subject_can_be_indexed_as_well_as_named() {
        // The only way to reach a column whose name cannot follow a dot.
        assert_eq!(value("Router[\"RTT\"].avg()").show(), "20");
        assert_eq!(value("Router.col(\"#\").first()").show(), "1");
        assert_eq!(value("Router.stat(\"loss\")").show(), "33%");
        // A list by position, counting back from the end for a negative one.
        assert_eq!(value("Router.rtt[0]").show(), "10.0 ms");
        assert_eq!(value("Router.rtt[-1]").show(), "-");
        assert_eq!(value("Router.rtt[99]").show(), "");
    }

    #[test]
    fn a_position_before_the_start_of_a_list_is_nothing_and_not_the_first_item() {
        // Counting back from the end is what a negative position is for, and
        // reaching back past the start is a mistake. It was pulled up to zero,
        // so `rtt[-9]` read as `rtt[0]` and answered with the first probe,
        // where `rtt[99]` rightly answered with nothing.
        assert_eq!(value("Router.rtt[-1]").show(), "-", "the last row, as before");
        assert_eq!(value("Router.rtt[-3]").show(), "10.0 ms", "and back to the first");
        assert_eq!(value("Router.rtt[-4]").show(), "");
        assert_eq!(value("Router.rtt[-99]").show(), "");
        // `0 / 0` is a number that is no position either, and it was pulled up
        // to zero the same way.
        assert_eq!(value("Router.rtt[0 / 0]").show(), "");
    }

    #[test]
    fn not_reads_as_well_as_the_symbol() {
        assert!(!value("not Router.ok").truth());
        assert!(value("not (Router.down == 0)").truth());
    }

    #[test]
    fn a_mistake_says_what_is_wrong() {
        let source = source();
        assert!(run("Router.nonsense", &source).unwrap_err().contains("no nonsense"));
        assert!(run("Nowhere.rows", &source).unwrap_err().contains("Nowhere"));
        assert!(run("wibble(1)", &source).unwrap_err().contains("no function"));
        assert!(run(r#""a" - 1"#, &source).unwrap_err().contains("needs a number"));
    }

    #[test]
    fn the_shaping_functions_do_what_they_say() {
        assert_eq!(value("fixed(1.239, 2)").show(), "1.24");
        assert_eq!(value("round(1.6)").show(), "2");
        assert_eq!(value("percent(1, 4)").show(), "25%");
        assert_eq!(value("upper(Router.name)").show(), "ROUTER");
        assert_eq!(value(r#"join(Router.columns, " | ")"#).show(), "# | FROM | RTT");
        // The median of two samples is the midpoint of them, as a
        // spreadsheet would give it.
        assert_eq!(value("median(Router.rtt)").show(), "20");
        assert_eq!(value("p95(Router.rtt)").show(), "29");
    }
}
