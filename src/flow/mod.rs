//! Workflows: what to run, in what order, and on what conditions.
//!
//! A question about a network is rarely one tool, and rarely the same tools
//! every time — you sweep, and *then* scan the ports of whatever answered, or
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
//! It is normally built in the editor rather than typed, which is why the
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
    /// Lines that are not steps, with why, so a typo is visible rather than
    /// silently doing nothing.
    pub problems: Vec<(usize, String)>,
    /// Whether reading it dropped something, so writing it back would lose
    /// what the file says.
    ///
    /// Not every problem is one of these. A branch with no condition yet is a
    /// step that is half-built — it is read, written and read again
    /// unchanged, and it is exactly what the editor produces the moment you
    /// add a branch. A line nobody can read is different: it is in the file
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

    /// What it is called: its first comment line.
    pub fn title(&self) -> Option<&str> {
        self.heading.first().map(String::as_str).filter(|t| !t.is_empty())
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
    if let Some(rest) = keyword(text, "if") {
        let condition = rest.trim_end_matches('{').trim().to_string();
        if condition.is_empty() {
            problems.push((line + 1, "if needs a condition".into()));
        }
        let then = block(lines, at, problems, lost, depth + 1);

        // `}` or `} else {`
        let mut otherwise = Vec::new();
        if *at < lines.len() {
            let closer = strip_comment(lines[*at]);
            *at += 1;
            if closer.starts_with("} else") || closer.starts_with("}else") {
                otherwise = block(lines, at, problems, lost, depth + 1);
                if *at < lines.len() {
                    *at += 1;
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

    // `set gateway = Sweep.up` — the name, then the expression worked out
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
fn times_in_range(times: usize) -> usize {
    times.clamp(1, 100)
}

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
    let bytes = text.as_bytes();
    let mut quote: Option<u8> = None;

    for at in 0..bytes.len() {
        let byte = bytes[at];
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if text[at..].starts_with("if")
                && at > 0
                && bytes[at - 1].is_ascii_whitespace()
                && bytes.get(at + 2).is_none_or(u8::is_ascii_whitespace) =>
            {
                let condition = text[at + 2..].trim();
                return (
                    &text[..at],
                    (!condition.is_empty()).then(|| condition.to_string()),
                );
            }
            None => {}
        }
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
            // half is not chosen yet is how an editor loses work — and every
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
/// A condition that cannot be answered — one naming a run that has not been
/// done — is a failure rather than a `false`, so the caller can say which of
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
    use super::*;

    fn run(name: &str) -> Step {
        Step::Run { name: name.into(), condition: None }
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
        assert_eq!(flow.title(), Some("Nightly check"));
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
        // writes and reads again unchanged — so it is a thing to finish, not
        // a reason to stop offering the controls that would finish it.
        let flow = parse("if  {\n  run A\n}");
        assert!(!flow.problems.is_empty(), "it still says the condition is missing");
        assert!(!flow.lossy);
        assert_eq!(parse(&write(&flow)).steps, flow.steps);
    }

    #[test]
    fn how_many_runs_it_holds_counts_the_nested_ones() {
        let flow = parse("run A\nif x {\n  run B\n  repeat 2 {\n    run C\n  }\n}");
        assert_eq!(flow.runs(), 3);
    }
}
