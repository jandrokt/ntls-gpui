//! Workflows: what to run, in what order, and on what conditions.
//!
//! A question about a network is rarely one tool, and rarely the same tools
//! every time. You sweep, and *then* scan the ports of whatever answered, or
//! give up if nothing did. A workflow writes that down as a tree of steps.
//!
//! The file is text so that it can be read, diffed and edited anywhere:
//!
//! ```text
//! # Nightly check
//! run "Sweep"
//! if Sweep.up > 0 {
//!   run "Port scan"
//!   wait 30s
//! } else {
//!   run "Ping gateway"
//! }
//! repeat 3 {
//!   run "Ping gateway"
//! }
//! stop
//! ```
//!
//! It is normally built in the editor and not typed, which is why the
//! writer is as much a part of this module as the reader: what the editor
//! changes is the tree, and the file is written back from it.

pub mod compile;
pub mod edit;

use std::fmt::Write as _;

/// One step, which may hold others.
#[derive(Clone, PartialEq, Debug)]
pub enum Step {
    /// Start a run in this workspace and wait for it to finish.
    Run { name: String, condition: Option<String> },
    /// Take one branch or the other.
    If { condition: String, then: Vec<Step>, otherwise: Vec<Step> },
    /// Do the same thing a fixed number of times.
    Repeat { times: usize, body: Vec<Step> },
    /// Pause.
    Wait { seconds: f64 },
    /// Work something out and keep it under a name, for every document,
    /// condition and later step in the workspace to read.
    Set { name: String, value: String },
    /// End the workflow.
    Stop { condition: Option<String> },
}

impl Step {
    /// The icon it is drawn with.
    pub fn icon(&self) -> &'static str {
        match self {
            Step::Run { .. } => "play",
            Step::If { .. } => "compare",
            Step::Repeat { .. } => "refresh",
            Step::Wait { .. } => "gear",
            Step::Set { .. } => "note",
            Step::Stop { .. } => "stop",
        }
    }

    /// The steps inside it, if it holds any.
    pub fn blocks(&self) -> Vec<&Vec<Step>> {
        match self {
            Step::If { then, otherwise, .. } => vec![then, otherwise],
            Step::Repeat { body, .. } => vec![body],
            _ => Vec::new(),
        }
    }
}

/// A duration, written the way it is typed.
pub fn seconds_text(seconds: f64) -> String {
    if seconds >= 60.0 && seconds % 60.0 == 0.0 {
        return format!("{}m", (seconds / 60.0) as i64);
    }
    if seconds == seconds.trunc() {
        return format!("{}s", seconds as i64);
    }
    format!("{seconds}s")
}

/// A parsed workflow, and whatever could not be parsed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flow {
    /// The comment lines at the top, kept so writing the file back does not
    /// throw away what it says about itself.
    pub heading: Vec<String>,
    pub steps: Vec<Step>,
    /// Lines that are not steps, with why, so a typo is visible and not
    /// silently doing nothing.
    pub problems: Vec<(usize, String)>,
    /// Whether reading it dropped something, so writing it back would lose
    /// what the file says.
    ///
    /// Not every problem is one of these. A branch with no condition yet is a
    /// step that is half-built: it is read, written and read again
    /// unchanged, and it is exactly what the editor produces the moment you
    /// add a branch. An unreadable line is different: it is in the file
    /// and not in the tree, and writing the tree back would take it out. The
    /// editor puts its controls away for the second and not for the first.
    pub lossy: bool,
}

impl Flow {
    /// How many runs it holds, however deeply.
    pub fn runs(&self) -> usize {
        fn count(steps: &[Step]) -> usize {
            steps
                .iter()
                .map(|step| {
                    usize::from(matches!(step, Step::Run { .. }))
                        + step.blocks().iter().map(|b| count(b)).sum::<usize>()
                })
                .sum()
        }
        count(&self.steps)
    }
}

// --- reading -----------------------------------------------------------------

/// Reads a workflow file.
pub fn parse(source: &str) -> Flow {
    let lines: Vec<&str> = source.lines().collect();
    let mut flow = Flow::default();

    // The comments before the first step are what the workflow says about
    // itself.
    let mut at = 0usize;
    while at < lines.len() {
        let line = lines[at].trim();
        if line.is_empty() {
            at += 1;
            continue;
        }
        let Some(rest) = line.strip_prefix('#') else { break };
        flow.heading.push(rest.trim().to_string());
        at += 1;
    }

    let mut lost = false;
    flow.steps = block(&lines, &mut at, &mut flow.problems, &mut lost, 0);
    flow.lossy = lost;
    flow
}

/// Reads steps until the end, or until the `}` that closes this block.
fn block(
    lines: &[&str],
    at: &mut usize,
    problems: &mut Vec<(usize, String)>,
    lost: &mut bool,
    depth: usize,
) -> Vec<Step> {
    let mut steps = Vec::new();

    while *at < lines.len() {
        let line = *at;
        let text = strip_comment(lines[line]);
        *at += 1;

        if text.is_empty() {
            continue;
        }
        if text == "}" || text.starts_with("} else") || text.starts_with("}else") {
            // The caller opened this block and reads the closer.
            *at -= 1;
            if depth == 0 {
                *at += 1;
                problems.push((line + 1, "a closing brace with nothing open".into()));
                *lost = true;
                continue;
            }
            return steps;
        }

        match step(lines, at, text, line, problems, lost, depth) {
            Some(step) => steps.push(step),
            None => {
                problems.push((line + 1, format!("cannot read {text:?}")));
                *lost = true;
            }
        }
    }
    steps
}

/// One step, which may open a block the reader carries on into.
fn step(
    lines: &[&str],
    at: &mut usize,
    text: &str,
    line: usize,
    problems: &mut Vec<(usize, String)>,
    lost: &mut bool,
    depth: usize,
) -> Option<Step> {
    // Reading a block reads what is inside it, so a file of branches inside
    // branches was read as deeply as it was nested and nothing said stop. A
    // few thousand `if` lines ran the reader out of stack, and a stack that
    // runs out is not a workflow that failed to load: it is the application
    // gone, with every other workspace and whatever was unsaved in it. The
    // whole file is read again on every repaint of the workflow, so it went
    // on the thread that draws the window. Everything downstream walks the
    // tree the same way it was read, so bounding the read is what keeps the
    // machine and the editor off the same cliff.
    if depth >= MOST_NESTING {
        problems.push((line + 1, format!("nested more than {MOST_NESTING} deep")));
        *lost = true;
        return None;
    }

    if let Some(rest) = keyword(text, "if") {
        let condition = rest.trim_end_matches('{').trim().to_string();
        if opens_a_block(&condition) {
            // A branch is read a line at a time, so a whole branch written on
            // one line was read as none of it: everything after the brace
            // became part of the condition, the body was gone, and the lines
            // that followed were taken as the inside of a block that never
            // closed, which put the rest of the file in the branch. Nothing
            // was recorded as lost, so the next edit wrote that back over the
            // file. It is a line the reader does not understand, and saying so
            // is what leaves it alone.
            return None;
        }
        if condition.is_empty() {
            problems.push((line + 1, "if needs a condition".into()));
        }
        let then = block(lines, at, problems, lost, depth + 1);

        // `}`, `} else {`, or a chained `} else if ... {`.
        let mut otherwise = Vec::new();
        if *at < lines.len() {
            let closer_at = *at;
            let closer = strip_comment(lines[closer_at]);
            *at += 1;
            if closer.starts_with("} else") || closer.starts_with("}else") {
                let tail = else_tail(closer);
                if keyword(tail, "if").is_some() {
                    // A chained branch is a branch inside the else, and is
                    // read as one. The line used to be recognised by its
                    // `} else` prefix and read no further, so the second
                    // condition was neither run nor reported: the file said
                    // one thing, the tree said another, and because nothing
                    // was recorded as lost the next edit wrote the tree back
                    // over the line that held it. The `}` ending the chain
                    // closes both branches, so the nested read takes it and
                    // this one must not.
                    match step(lines, at, tail, closer_at, problems, lost, depth + 1) {
                        Some(chained) => otherwise.push(chained),
                        // A chained branch the reader cannot make sense of is
                        // still a branch somebody wrote. Dropping it without a
                        // word would leave the tree saying less than the file
                        // and let the next edit write over it.
                        None => {
                            problems.push((closer_at + 1, format!("cannot read {tail:?}")));
                            *lost = true;
                        }
                    }
                } else {
                    if !tail.is_empty() {
                        // In the file and not in the tree. Saying so is what
                        // stops the editor writing over it.
                        problems.push((closer_at + 1, format!("cannot read {tail:?} after else")));
                        *lost = true;
                    }
                    otherwise = block(lines, at, problems, lost, depth + 1);
                    if *at < lines.len() {
                        *at += 1;
                    }
                }
            }
        }
        return Some(Step::If { condition, then, otherwise });
    }

    if let Some(rest) = keyword(text, "repeat") {
        let count = rest.trim_end_matches('{').trim();
        let times = count.parse().unwrap_or_else(|_| {
            problems.push((line + 1, format!("{count:?} is not a number of times")));
            // Writing it back would put a 1 where their word was.
            *lost = true;
            1
        });
        let body = block(lines, at, problems, lost, depth + 1);
        if *at < lines.len() {
            *at += 1;
        }
        return Some(Step::Repeat { times: times_in_range(times), body });
    }

    // `set gateway = Sweep.up`: the name, then the expression worked out
    // when the step is reached.
    if let Some(rest) = keyword(text, "set") {
        let rest = rest.trim();
        let (name, value) = match rest.split_once('=') {
            Some((name, value)) => (name.trim().to_string(), value.trim().to_string()),
            None => {
                problems.push((line + 1, "set needs a name, an = and a value".into()));
                (rest.to_string(), String::new())
            }
        };
        if name.is_empty() {
            problems.push((line + 1, "set needs a name".into()));
        }
        return Some(Step::Set { name, value });
    }

    if let Some(rest) = keyword(text, "wait") {
        let seconds = parse_seconds(rest.trim()).unwrap_or_else(|| {
            problems.push((line + 1, format!("{:?} is not a length of time", rest.trim())));
            *lost = true;
            1.0
        });
        return Some(Step::Wait { seconds });
    }

    let (head, condition) = split_condition(text);
    if head.trim().eq_ignore_ascii_case("stop") {
        return Some(Step::Stop { condition });
    }
    if let Some(rest) = keyword(head.trim(), "run") {
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }
        let name = unquote(rest);
        return Some(Step::Run { name, condition });
    }
    None
}

/// A repeat has to stop: an unbounded one would be a way to hang the
/// application from a text file.
///
/// This bounds one repeat and nothing further. Nesting multiplies, so what
/// stops a file of repeats inside repeats is the budget the machine runs on,
/// `compile::MOST_OPERATIONS`, and not this.
fn times_in_range(times: usize) -> usize {
    times.clamp(1, 100)
}

/// How deep a workflow may be nested.
///
/// A branch inside a branch is read by reading the inner one while the outer
/// one waits, so the depth of the file is the depth of the reader, and a file
/// deeper than the stack takes the application with it rather than failing to
/// load. Everything after the reader walks the tree the same way, so this is
/// also what keeps the machine, the editor and dropping the tree inside their
/// own stacks. The editor indents two spaces a level, so this is far past any
/// workflow anybody can read.
const MOST_NESTING: usize = 64;

fn strip_comment(line: &str) -> &str {
    let mut quote: Option<u8> = None;
    for (at, byte) in line.bytes().enumerate() {
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'#' => return line[..at].trim(),
            None => {}
        }
    }
    line.trim()
}

/// Whether what is left of an `if` line still holds a `{` that opens a block,
/// which is what a branch written all on one line leaves behind.
///
/// A brace inside quotes is part of what is being compared and not one of
/// these, so a condition may still say `Router.name == "{"`.
fn opens_a_block(condition: &str) -> bool {
    let mut quote: Option<char> = None;
    for ch in condition.chars() {
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch == '{' => return true,
            None => {}
        }
    }
    false
}

/// What a `} else ...` line says after the `else`, with the `{` that opens the
/// block taken off: nothing at all for a plain `} else {`, and `if b > 0 {`
/// for a chain.
fn else_tail(closer: &str) -> &str {
    let rest = closer.trim_start_matches('}').trim_start();
    let rest = keyword(rest, "else").unwrap_or(rest).trim();
    rest.strip_prefix('{').map_or(rest, str::trim)
}

fn keyword<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    text.get(..word.len()).filter(|head| head.eq_ignore_ascii_case(word))?;
    let rest = &text[word.len()..];
    if rest.is_empty() {
        return Some(rest);
    }
    (rest.starts_with(char::is_whitespace) || rest.starts_with('{')).then(|| rest.trim_start())
}

/// Splits a line at its `if`, ignoring one inside quotes so a run may be
/// called "Check if reachable".
fn split_condition(text: &str) -> (&str, Option<String>) {
    // Walked a character at a time, not a byte at a time. Stepping by bytes
    // meant the index landed inside a multi-byte character, and slicing there
    // is not something a `str` allows: one accented letter anywhere in a
    // workflow took the program down with it.
    let mut quote: Option<char> = None;
    let mut after_space = false;

    for (at, ch) in text.char_indices() {
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if after_space
                && text[at..].starts_with("if")
                && text[at + 2..].chars().next().is_none_or(char::is_whitespace) =>
            {
                let condition = text[at + 2..].trim();
                return (
                    &text[..at],
                    (!condition.is_empty()).then(|| condition.to_string()),
                );
            }
            None => {}
        }
        after_space = ch.is_whitespace();
    }
    (text, None)
}

pub(crate) fn unquote(text: &str) -> String {
    let trimmed = text.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = trimmed.strip_prefix(quote).and_then(|t| t.strip_suffix(quote)) {
            return inner.to_string();
        }
    }
    trimmed.to_string()
}

/// `30s`, `2m`, or a bare number of seconds.
pub fn parse_seconds(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (number, unit) = match text.find(|c: char| c.is_alphabetic()) {
        Some(at) => (&text[..at], text[at..].trim()),
        None => (text, ""),
    };
    let value: f64 = number.trim().parse().ok()?;
    if value < 0.0 {
        return None;
    }
    Some(match unit {
        "" | "s" | "sec" | "secs" => value,
        "m" | "min" | "mins" => value * 60.0,
        "h" => value * 3600.0,
        _ => return None,
    })
}

// --- writing -----------------------------------------------------------------

/// Writes a workflow back out.
///
/// The editor changes the tree; this is what puts it on disk, and what makes
/// the file the same thing whether it was typed or built.
pub fn write(flow: &Flow) -> String {
    let mut out = String::new();
    for line in &flow.heading {
        let _ = writeln!(out, "# {line}");
    }
    if !flow.heading.is_empty() && !flow.steps.is_empty() {
        out.push('\n');
    }
    write_block(&mut out, &flow.steps, 0);
    out
}

fn write_block(out: &mut String, steps: &[Step], depth: usize) {
    let pad = "  ".repeat(depth);
    for step in steps {
        match step {
            Step::Run { name, condition } => {
                let _ = writeln!(out, "{pad}run {}{}", quoted(name), suffix(condition));
            }
            Step::Stop { condition } => {
                let _ = writeln!(out, "{pad}stop{}", suffix(condition));
            }
            Step::Wait { seconds } => {
                let _ = writeln!(out, "{pad}wait {}", seconds_text(*seconds));
            }
            Step::Set { name, value } => {
                let _ = writeln!(out, "{pad}set {name} = {value}");
            }
            Step::If { condition, then, otherwise } => {
                let _ = writeln!(out, "{pad}if {condition} {{");
                write_block(out, then, depth + 1);
                if otherwise.is_empty() {
                    let _ = writeln!(out, "{pad}}}");
                } else {
                    let _ = writeln!(out, "{pad}}} else {{");
                    write_block(out, otherwise, depth + 1);
                    let _ = writeln!(out, "{pad}}}");
                }
            }
            Step::Repeat { times, body } => {
                let _ = writeln!(out, "{pad}repeat {times} {{");
                write_block(out, body, depth + 1);
                let _ = writeln!(out, "{pad}}}");
            }
        }
    }
}

fn quoted(name: &str) -> String {
    if name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') && !name.is_empty() {
        name.to_string()
    } else {
        format!("{name:?}")
    }
}

fn suffix(condition: &Option<String>) -> String {
    match condition {
        Some(condition) => {
            // Whatever it says, including a condition that is still being
            // built. Dropping half of what somebody chose because the other
            // half is not chosen yet is how an editor loses work, and every
            // partial form reads back into the parts it was built from.
            let text = match edit::Guide::read_partial(condition) {
                Some(guide) => guide.write(),
                None => condition.trim().to_string(),
            };
            if text.is_empty() { String::new() } else { format!(" if {text}") }
        }
        None => String::new(),
    }
}

/// Whether a condition holds, against what the workspace has found.
///
/// A condition that cannot be answered, one naming a run that has not been
/// done, is a failure and not a `false`, so the caller can say which of
/// the two happened.
pub fn holds(condition: &str, source: &dyn crate::expr::Source) -> Result<bool, String> {
    crate::expr::run(condition, source).map(|value| value.truth())
}

/// What a new workflow holds.
pub fn starter(name: &str) -> Flow {
    Flow {
        heading: vec![
            name.to_string(),
            String::new(),
            "Built in the editor, kept as text. Add a step to begin.".to_string(),
        ],
        ..Flow::default()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_line_with_an_accent_in_it_is_read_and_not_fatal() {
        // The condition split used to step through the line a byte at a time,
        // so the first multi-byte character put the index inside a character
        // and slicing there ended the program.
        assert_eq!(super::split_condition("run \"caf\u{e9}\""), ("run \"caf\u{e9}\"", None));
        let (before, cond) = super::split_condition("run \"caf\u{e9}\" if Router.ok");
        assert_eq!(before.trim(), "run \"caf\u{e9}\"");
        assert_eq!(cond.as_deref(), Some("Router.ok"));
        // An `if` inside a quoted name is still part of the name.
        assert_eq!(super::split_condition("run \"Check if reach\u{e9}ble\"").1, None);
    }
    use super::*;

    fn run(name: &str) -> Step {
        Step::Run { name: name.into(), condition: None }
    }

    /// How deep the blocks inside blocks go.
    fn nesting(steps: &[Step]) -> usize {
        steps
            .iter()
            .map(|step| 1 + step.blocks().iter().map(|b| nesting(b)).max().unwrap_or(0))
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn a_step_is_a_run_and_maybe_a_condition() {
        let flow = parse("run \"Sweep\"\nrun \"Port scan\" if Sweep.up > 0");
        assert_eq!(flow.steps.len(), 2);
        assert_eq!(flow.steps[0], run("Sweep"));
        assert_eq!(
            flow.steps[1],
            Step::Run { name: "Port scan".into(), condition: Some("Sweep.up > 0".into()) }
        );
        assert!(flow.problems.is_empty(), "{:?}", flow.problems);
    }

    #[test]
    fn a_branch_holds_the_steps_inside_it() {
        let flow = parse(
            "if Sweep.up > 0 {\n  run \"Port scan\"\n  wait 30s\n} else {\n  run \"Ping\"\n}",
        );
        assert!(flow.problems.is_empty(), "{:?}", flow.problems);
        let Step::If { condition, then, otherwise } = &flow.steps[0] else {
            panic!("expected a branch, got {:?}", flow.steps);
        };
        assert_eq!(condition, "Sweep.up > 0");
        assert_eq!(then, &vec![run("Port scan"), Step::Wait { seconds: 30.0 }]);
        assert_eq!(otherwise, &vec![run("Ping")]);
    }

    #[test]
    fn a_branch_without_an_else_has_an_empty_one() {
        let flow = parse("if a > 1 {\n  run X\n}\nrun Y");
        assert!(flow.problems.is_empty(), "{:?}", flow.problems);
        assert_eq!(flow.steps.len(), 2, "{:?}", flow.steps);
        assert!(matches!(&flow.steps[0], Step::If { otherwise, .. } if otherwise.is_empty()));
        assert_eq!(flow.steps[1], run("Y"));
    }

    #[test]
    fn a_repeat_holds_a_body_and_cannot_be_unbounded() {
        let flow = parse("repeat 3 {\n  run Ping\n}");
        assert_eq!(flow.steps[0], Step::Repeat { times: 3, body: vec![run("Ping")] });
        // A file that asked for a million passes would be a way to hang the
        // application.
        assert!(matches!(parse("repeat 99999 {\n}").steps[0], Step::Repeat { times: 100, .. }));
    }

    #[test]
    fn a_step_can_keep_what_it_worked_out_under_a_name() {
        let flow = parse("set gateway = Sweep.up\nrun \"Ping\" if gateway > 0");
        assert!(flow.problems.is_empty(), "{:?}", flow.problems);
        assert_eq!(
            flow.steps[0],
            Step::Set { name: "gateway".into(), value: "Sweep.up".into() }
        );
        // And it is written back the way it was read.
        assert_eq!(write(&flow), "set gateway = Sweep.up\nrun Ping if gateway > 0\n");
    }

    #[test]
    fn a_set_without_a_value_says_so_rather_than_being_dropped() {
        let flow = parse("set gateway");
        assert_eq!(flow.steps.len(), 1);
        assert!(!flow.problems.is_empty());
        // It is half-built, not unreadable: the controls stay available to
        // finish it.
        assert!(!flow.lossy);
    }

    #[test]
    fn a_workflow_survives_being_written_and_read_again() {
        // The editor changes the tree and the file is written from it, so this
        // is the property everything else rests on.
        let source = "# Nightly check\n# \n# what it is for\n\nrun \"Sweep\"\nif Sweep.up > 0 {\n  run \"Port scan\" if Sweep.down == 0\n  wait 2m\n} else {\n  repeat 2 {\n    run Ping\n  }\n}\nstop if Sweep.up > 10\n";
        let flow = parse(source);
        assert!(flow.problems.is_empty(), "{:?}", flow.problems);

        let written = write(&flow);
        let again = parse(&written);
        assert_eq!(flow, again, "written as:\n{written}");
        assert_eq!(written, write(&again), "writing is stable");
    }

    #[test]
    fn the_heading_is_kept_and_names_the_workflow() {
        let flow = parse("# Nightly check\n# and why\n\nrun A");
        // The first line names it; the rest is whatever else it wanted to say.
        assert_eq!(flow.heading, vec!["Nightly check", "and why"]);
        assert!(write(&flow).starts_with("# Nightly check\n# and why\n"));
    }

    #[test]
    fn a_time_is_read_and_written_the_way_it_is_typed() {
        assert_eq!(parse_seconds("30s"), Some(30.0));
        assert_eq!(parse_seconds("2m"), Some(120.0));
        assert_eq!(parse_seconds("90"), Some(90.0));
        assert_eq!(parse_seconds("soon"), None);
        assert_eq!(seconds_text(120.0), "2m");
        assert_eq!(seconds_text(30.0), "30s");
    }

    #[test]
    fn an_if_inside_a_name_is_part_of_the_name() {
        let flow = parse("run \"Check if reachable\"");
        assert_eq!(flow.steps[0], run("Check if reachable"));
    }

    #[test]
    fn a_line_that_is_not_a_step_is_reported_rather_than_ignored() {
        let flow = parse("wibble\nrun\n}");
        assert_eq!(flow.problems.len(), 3, "{:?}", flow.problems);
        assert_eq!(flow.problems[0].0, 1);
        // Those lines are in the file and not in the tree, so writing the
        // tree back would take them out.
        assert!(flow.lossy);
    }

    #[test]
    fn a_step_that_is_only_half_built_is_not_a_line_at_risk() {
        // Adding a branch with the editor makes exactly this, and it reads,
        // writes and reads again unchanged, so it is a thing to finish, not
        // a reason to stop offering the controls that would finish it.
        let flow = parse("if  {\n  run A\n}");
        assert!(!flow.problems.is_empty(), "it still says the condition is missing");
        assert!(!flow.lossy);
        assert_eq!(parse(&write(&flow)).steps, flow.steps);
    }

    #[test]
    fn a_chained_else_if_keeps_the_condition_it_was_written_with() {
        // The closer used to be recognised by its `} else` prefix and read no
        // further, so this became a plain else: `b > 0` was neither run nor
        // reported, and the next edit wrote the tree back over the line that
        // held it.
        let flow = parse("if a > 0 {\n  run A\n} else if b > 0 {\n  run B\n}\nrun C\n");
        assert!(flow.problems.is_empty(), "{:?}", flow.problems);
        assert!(!flow.lossy, "nothing was dropped");
        assert_eq!(
            flow.steps,
            vec![
                Step::If {
                    condition: "a > 0".into(),
                    then: vec![run("A")],
                    otherwise: vec![Step::If {
                        condition: "b > 0".into(),
                        then: vec![run("B")],
                        otherwise: Vec::new(),
                    }],
                },
                run("C"),
            ]
        );
        // And it is the same workflow written back out and read again.
        assert_eq!(parse(&write(&flow)), flow, "written as:\n{}", write(&flow));
    }

    #[test]
    fn a_chain_of_branches_ends_in_the_last_else() {
        let source =
            "if a > 0 {\n  run A\n} else if b > 0 {\n  run B\n} else {\n  run C\n}\nrun D\n";
        let flow = parse(source);
        assert!(flow.problems.is_empty(), "{:?}", flow.problems);
        assert!(!flow.lossy);
        assert_eq!(flow.steps.len(), 2, "{:?}", flow.steps);
        assert_eq!(flow.steps[1], run("D"));
        let Step::If { otherwise, .. } = &flow.steps[0] else {
            panic!("expected a branch, got {:?}", flow.steps);
        };
        assert_eq!(
            otherwise,
            &vec![Step::If {
                condition: "b > 0".into(),
                then: vec![run("B")],
                otherwise: vec![run("C")],
            }]
        );
        assert_eq!(parse(&write(&flow)), flow, "written as:\n{}", write(&flow));
    }

    #[test]
    fn words_after_an_else_that_are_not_a_branch_are_reported_rather_than_dropped() {
        let flow = parse("if a > 0 {\n  run A\n} elsewhere {\n  run B\n}");
        assert!(!flow.problems.is_empty(), "the words are said to be unreadable");
        // They are in the file and not in the tree, so writing the tree back
        // would take them out.
        assert!(flow.lossy);
    }

    #[test]
    fn a_branch_written_on_one_line_is_reported_rather_than_swallowing_the_file() {
        // A block is read a line at a time, so none of this line was read as
        // what it says: the body went into the condition, and every line after
        // it was taken as the inside of a block that never closed, which put
        // the rest of the file in the branch. Nothing was said to be lost, so
        // the next edit wrote that back out as the file.
        let flow = parse("if c { run A }\nrun B\n");
        assert!(!flow.problems.is_empty(), "the line is said to be unreadable");
        assert!(flow.lossy, "it is in the file and not in the tree");
        assert_eq!(flow.steps, vec![run("B")], "{:?}", flow.steps);

        // And the same line after an `else`, where a branch the reader cannot
        // make sense of used to be dropped without a word.
        let chained = parse("if a > 0 {\n  run A\n} else if c { run B }\n");
        assert!(!chained.problems.is_empty(), "{:?}", chained.problems);
        assert!(chained.lossy);

        // A brace inside quotes is part of what is being compared, so a
        // condition may still hold one and the branch is read as written.
        let quoted = parse("if a == \"{\" {\n  run A\n}\n");
        assert!(quoted.problems.is_empty(), "{:?}", quoted.problems);
        assert!(!quoted.lossy);
        assert_eq!(
            quoted.steps,
            vec![Step::If {
                condition: "a == \"{\"".into(),
                then: vec![run("A")],
                otherwise: Vec::new(),
            }]
        );
        assert_eq!(parse(&write(&quoted)), quoted, "written as:\n{}", write(&quoted));
    }

    #[test]
    fn a_file_nested_deeper_than_the_reader_reads_is_reported_and_not_fatal() {
        // Reading a branch reads what is inside it while the outer one waits,
        // so the depth of the file was the depth of the reader: a few thousand
        // `if` lines ran it out of stack, and that is not a workflow that
        // failed to load but the application gone, on the thread that draws
        // the window, since the file is read again on every repaint.
        let deep = "if x {\n".repeat(MOST_NESTING + 8);
        let flow = parse(&deep);
        assert!(!flow.problems.is_empty(), "the depth is said to be too much");
        assert!(flow.lossy, "the lines it would not read are in the file");
        let deepest = nesting(&flow.steps);
        assert!(deepest <= MOST_NESTING, "read {deepest} blocks deep");
    }

    #[test]
    fn a_negated_condition_survives_being_written_back_out() {
        // `not Sweep` looked like a two-word run name, so the condition was
        // rewritten as a lookup of a run called "not Sweep". A run that is
        // not there is nothing rather than an error, so the condition became
        // a silent permanent false and the step it guarded stopped running.
        // Nothing had to touch that line: the file is written out from the
        // tree after any other edit.
        let flow = parse("run A if not Sweep.ok\n");
        assert_eq!(
            flow.steps,
            vec![Step::Run { name: "A".into(), condition: Some("not Sweep.ok".into()) }]
        );
        let written = write(&flow);
        assert!(written.contains("not Sweep.ok"), "written as:\n{written}");
        assert!(!written.contains("tool("), "written as:\n{written}");
        assert_eq!(parse(&written), flow);
    }

    #[test]
    fn a_run_whose_name_merely_starts_with_those_letters_is_still_a_name() {
        // It is the first whole word that decides, so a run called `notes` is
        // read and written as the run it is.
        let flow = parse("run A if notes.ok\n");
        let written = write(&flow);
        assert!(written.contains("notes.ok"), "written as:\n{written}");
        assert_eq!(parse(&written), flow);
    }

    #[test]
    fn how_many_runs_it_holds_counts_the_nested_ones() {
        let flow = parse("run A\nif x {\n  run B\n  repeat 2 {\n    run C\n  }\n}");
        assert_eq!(flow.runs(), 3);
    }
}
