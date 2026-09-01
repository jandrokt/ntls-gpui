//! Changing a workflow: what the editor does to the tree.
//!
//! The editor never asks the user to type a workflow. Every change it can make
//! is one [`Change`] applied at one [`Spot`], which makes the whole of
//! it testable here, without a window, and makes undo, later, a matter of
//! keeping the trees instead of the keystrokes.

use super::Step;

/// Where a step is.
///
/// `inside` names the blocks to descend through (for each, which step holds
/// the block and which of that step's blocks it is) and `index` is the step's
/// place in the innermost one. An empty `inside` means the top level.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Spot {
    pub inside: Vec<(usize, usize)>,
    pub index: usize,
}

impl Spot {
    pub fn top(index: usize) -> Spot {
        Spot { inside: Vec::new(), index }
    }

    /// The spot of a step inside one of this one's blocks.
    pub fn child(&self, block: usize, index: usize) -> Spot {
        let mut inside = self.inside.clone();
        inside.push((self.index, block));
        Spot { inside, index }
    }

    /// The block this spot sits in.
    pub fn block(&self) -> &[(usize, usize)] {
        &self.inside
    }
}

/// A step the editor can add.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Run,
    If,
    Repeat,
    Wait,
    Set,
    Stop,
}

impl Shape {
    pub fn label(self) -> &'static str {
        match self {
            Shape::Run => "Run a tool",
            Shape::If => "Branch",
            Shape::Repeat => "Repeat",
            Shape::Wait => "Wait",
            Shape::Set => "Set a variable",
            Shape::Stop => "Stop",
        }
    }

    pub fn icon(self) -> &'static str {
        self.blank().icon()
    }

    /// The step it makes, ready to be filled in.
    pub fn blank(self) -> Step {
        match self {
            Shape::Run => Step::Run { name: String::new(), condition: None },
            Shape::If => {
                Step::If { condition: String::new(), then: Vec::new(), otherwise: Vec::new() }
            }
            Shape::Repeat => Step::Repeat { times: 2, body: Vec::new() },
            Shape::Wait => Step::Wait { seconds: 30.0 },
            Shape::Set => Step::Set { name: String::new(), value: String::new() },
            Shape::Stop => Step::Stop { condition: None },
        }
    }

    pub const ALL: [Shape; 6] =
        [Shape::Run, Shape::If, Shape::Repeat, Shape::Wait, Shape::Set, Shape::Stop];
}

/// One thing the editor can do to one step.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    /// Put a new step after this one.
    Add(Shape),
    /// Put a new step at the end of one of this step's blocks.
    AddInside(Shape, usize),
    Remove,
    /// Move it up (`false`) or down (`true`) among its neighbours.
    Move(bool),
    /// Put it inside a new branch, so an existing step can be made conditional
    /// without rebuilding it.
    Wrap,
    SetRun(String),
    /// The name a `set` step keeps its answer under.
    SetVarName(String),
    /// The expression a `set` step works out.
    SetVarValue(String),
    /// One part of a value that is being built, not typed: which run
    /// or variable, which of its figures, and how to summarise it.
    SetValueSubject(String),
    SetValueField(String),
    SetValueSummary(String),
    SetTimes(usize),
    SetWait(u32),
    /// Set one part of a condition that is being built, not typed.
    SetSubject(String),
    SetField(String),
    SetTest(Test),
    SetValue(String),
    ClearCondition,
}

/// Applies a change, and says where the cursor should be afterwards.
///
/// Returns `None` when the spot does not name a step, which happens when a
/// stale click arrives after the tree has changed under it.
pub fn apply(steps: &mut Vec<Step>, spot: &Spot, change: &Change) -> Option<Spot> {
    match change {
        Change::Add(shape) => {
            let block = block_mut(steps, spot.block())?;
            let at = (spot.index + 1).min(block.len());
            block.insert(at, shape.blank());
            Some(Spot { inside: spot.inside.clone(), index: at })
        }
        Change::AddInside(shape, which) => {
            let step = at_mut(steps, spot)?;
            let block = inner_mut(step, *which)?;
            block.push(shape.blank());
            Some(spot.child(*which, block.len() - 1))
        }
        Change::Remove => {
            let block = block_mut(steps, spot.block())?;
            if spot.index >= block.len() {
                return None;
            }
            block.remove(spot.index);
            // The cursor lands on whatever took its place, or on the one
            // before if it was the last.
            let index = spot.index.min(block.len().saturating_sub(1));
            (!block.is_empty()).then(|| Spot { inside: spot.inside.clone(), index })
        }
        Change::Move(down) => {
            let block = block_mut(steps, spot.block())?;
            let to = if *down { spot.index + 1 } else { spot.index.checked_sub(1)? };
            if to >= block.len() {
                return None;
            }
            block.swap(spot.index, to);
            Some(Spot { inside: spot.inside.clone(), index: to })
        }
        Change::Wrap => {
            let block = block_mut(steps, spot.block())?;
            let step = block.get(spot.index)?.clone();
            block[spot.index] =
                Step::If { condition: String::new(), then: vec![step], otherwise: Vec::new() };
            Some(spot.clone())
        }
        Change::SetRun(name) => {
            if let Step::Run { name: it, .. } = at_mut(steps, spot)? {
                *it = name.clone();
            }
            Some(spot.clone())
        }
        Change::SetVarName(name) => {
            if let Step::Set { name: it, .. } = at_mut(steps, spot)? {
                *it = name.trim().to_string();
            }
            Some(spot.clone())
        }
        Change::SetVarValue(value) => {
            if let Step::Set { value: it, .. } = at_mut(steps, spot)? {
                *it = value.trim().to_string();
            }
            Some(spot.clone())
        }
        Change::SetValueSubject(_) | Change::SetValueField(_) | Change::SetValueSummary(_) => {
            let step = at_mut(steps, spot)?;
            let Step::Set { value, .. } = step else { return Some(spot.clone()) };
            let mut recipe = Recipe::read(value).unwrap_or_default();
            match change {
                Change::SetValueSubject(it) => {
                    // A different subject has different figures, so the one
                    // chosen for the last subject does not carry over.
                    if !recipe.subject.eq_ignore_ascii_case(it) {
                        recipe.field.clear();
                        recipe.summary.clear();
                    }
                    recipe.subject = it.clone();
                }
                Change::SetValueField(it) => recipe.field = it.clone(),
                Change::SetValueSummary(it) => recipe.summary = it.clone(),
                _ => unreachable!("only the three parts of a value reach here"),
            }
            *value = recipe.write();
            Some(spot.clone())
        }
        Change::SetTimes(times) => {
            if let Step::Repeat { times: it, .. } = at_mut(steps, spot)? {
                *it = (*times).clamp(1, 100);
            }
            Some(spot.clone())
        }
        Change::SetWait(seconds) => {
            if let Step::Wait { seconds: it } = at_mut(steps, spot)? {
                *it = f64::from(*seconds).clamp(1., 3600.);
            }
            Some(spot.clone())
        }
        Change::ClearCondition => {
            set_condition(at_mut(steps, spot)?, String::new());
            Some(spot.clone())
        }
        Change::SetSubject(_)
        | Change::SetField(_)
        | Change::SetTest(_)
        | Change::SetValue(_) => {
            let step = at_mut(steps, spot)?;
            let mut guide = Guide::read_partial(condition_of(step)).unwrap_or_default();
            match change {
                Change::SetSubject(it) => guide.subject = it.clone(),
                Change::SetField(it) => guide.field = it.clone(),
                Change::SetTest(it) => guide.test = *it,
                Change::SetValue(it) => guide.value = it.clone(),
                _ => unreachable!("only the four parts of a condition reach here"),
            }
            set_condition(step, guide.write());
            Some(spot.clone())
        }
    }
}

/// The step at a spot.
pub fn at<'a>(steps: &'a [Step], spot: &Spot) -> Option<&'a Step> {
    let mut here = steps;
    for (index, which) in &spot.inside {
        here = inner(here.get(*index)?, *which)?;
    }
    here.get(spot.index)
}

fn at_mut<'a>(steps: &'a mut Vec<Step>, spot: &Spot) -> Option<&'a mut Step> {
    let block = block_mut(steps, spot.block())?;
    block.get_mut(spot.index)
}

fn block_mut<'a>(steps: &'a mut Vec<Step>, inside: &[(usize, usize)]) -> Option<&'a mut Vec<Step>> {
    let mut here = steps;
    for (index, which) in inside {
        here = inner_mut(here.get_mut(*index)?, *which)?;
    }
    Some(here)
}

fn inner(step: &Step, which: usize) -> Option<&Vec<Step>> {
    match (step, which) {
        (Step::If { then, .. }, 0) => Some(then),
        (Step::If { otherwise, .. }, 1) => Some(otherwise),
        (Step::Repeat { body, .. }, 0) => Some(body),
        _ => None,
    }
}

fn inner_mut(step: &mut Step, which: usize) -> Option<&mut Vec<Step>> {
    match (step, which) {
        (Step::If { then, .. }, 0) => Some(then),
        (Step::If { otherwise, .. }, 1) => Some(otherwise),
        (Step::Repeat { body, .. }, 0) => Some(body),
        _ => None,
    }
}

/// The condition on a step, whatever kind it is.
pub fn condition_of(step: &Step) -> &str {
    match step {
        Step::Run { condition, .. } | Step::Stop { condition } => {
            condition.as_deref().unwrap_or("")
        }
        Step::If { condition, .. } => condition,
        Step::Repeat { .. } | Step::Wait { .. } | Step::Set { .. } => "",
    }
}

fn set_condition(step: &mut Step, text: String) {
    let text = text.trim().to_string();
    match step {
        Step::Run { condition, .. } | Step::Stop { condition } => {
            *condition = (!text.is_empty()).then_some(text);
        }
        Step::If { condition, .. } => *condition = text,
        Step::Repeat { .. } | Step::Wait { .. } | Step::Set { .. } => {}
    }
}

/// Whether a step can carry a condition at all. A wait and a repeat cannot,
/// so the editor does not offer them one.
pub fn takes_a_condition(step: &Step) -> bool {
    matches!(step, Step::Run { .. } | Step::Stop { .. } | Step::If { .. })
}

// --- conditions, built not typed ---------------------------------------------

/// How two things are compared.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Test {
    #[default]
    Is,
    IsNot,
    More,
    Less,
    AtLeast,
    AtMost,
}

impl Test {
    pub fn symbol(self) -> &'static str {
        match self {
            Test::Is => "==",
            Test::IsNot => "!=",
            Test::More => ">",
            Test::Less => "<",
            Test::AtLeast => ">=",
            Test::AtMost => "<=",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Test::Is => "is",
            Test::IsNot => "is not",
            Test::More => "is more than",
            Test::Less => "is less than",
            Test::AtLeast => "is at least",
            Test::AtMost => "is at most",
        }
    }

    /// Longest first, so `>=` is not read as `>`.
    pub const ALL: [Test; 6] =
        [Test::AtLeast, Test::AtMost, Test::IsNot, Test::Is, Test::More, Test::Less];
}

/// A condition in the shape the editor can build: one field of one run,
/// compared with one value.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Guide {
    pub subject: String,
    pub field: String,
    pub test: Test,
    pub value: String,
}

impl Guide {
    /// Reads a condition back into its parts, so a workflow that was typed can
    /// still be edited with the controls.
    ///
    /// Anything more involved than `subject.field test value` does not fit, and
    /// the editor shows it as written instead of pretending it does.
    pub fn read(condition: &str) -> Option<Guide> {
        let condition = condition.trim();
        if condition.is_empty() {
            return None;
        }
        let (test, at) = Test::ALL
            .iter()
            .filter_map(|test| condition.find(test.symbol()).map(|at| (*test, at)))
            .min_by_key(|(test, at)| (*at, std::cmp::Reverse(test.symbol().len())))?;

        let left = condition[..at].trim();
        let value = condition[at + test.symbol().len()..].trim();
        if value.is_empty() {
            return None;
        }
        let is_simple_value = (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
            || (value.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
                && !value.contains(char::is_whitespace));
        if !is_simple_value {
            return None;
        }

        // `Sweep.up > 0` names a figure of a run; `hosts_up > 0` names a
        // variable, which has no figures and needs no dot.
        let (subject_part, field) = match left.split_once('.') {
            Some((subject, field)) => (subject.trim(), field.trim()),
            None => (left, ""),
        };
        if !field.is_empty() && !field.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }

        let subject = parse_subject(subject_part)?;
        Some(Guide { subject, field: field.to_string(), test, value: value.to_string() })
    }

    pub(crate) fn read_partial(condition: &str) -> Option<Guide> {
        let condition = condition.trim();
        if condition.is_empty() {
            return None;
        }
        if let Some(guide) = Self::read(condition) {
            return Some(guide);
        }

        if let Some((test, at)) = Test::ALL
            .iter()
            .filter_map(|test| condition.find(test.symbol()).map(|at| (*test, at)))
            .min_by_key(|(test, at)| (*at, std::cmp::Reverse(test.symbol().len())))
        {
            let left = condition[..at].trim();
            let value = condition[at + test.symbol().len()..].trim();
            if !value.is_empty() {
                let is_simple_value = (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
                    || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
                    || (value.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
                        && !value.contains(char::is_whitespace));
                if !is_simple_value {
                    return None;
                }
            }
            let (subject_part, field) = match left.split_once('.') {
                Some((subject, field)) => (subject.trim(), field.trim()),
                None => (left, ""),
            };
            if !field.is_empty() && !field.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return None;
            }
            if let Some(subject) = parse_subject(subject_part) {
                return Some(Guide {
                    subject,
                    field: field.to_string(),
                    test,
                    value: value.to_string(),
                });
            }
        }

        if let Some((subject_part, field)) = condition.split_once('.') {
            let field = field.trim();
            if !field.is_empty() && !field.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return None;
            }
            let subject = parse_subject(subject_part.trim())?;
            return Some(Guide {
                subject,
                field: field.to_string(),
                test: Test::Is,
                value: String::new(),
            });
        }

        let subject = parse_subject(condition)?;
        Some(Guide {
            subject,
            field: String::new(),
            test: Test::Is,
            value: String::new(),
        })
    }

    /// Writes the parts back out, including a condition that is only part
    /// chosen.
    ///
    /// Half of a condition is not nothing: it is what somebody has chosen so
    /// far, and it is written down for the same reason the rest of the
    /// workflow is. Every partial form still reads back into the same parts,
    /// so choosing the subject, coming back tomorrow and choosing the value
    /// works. A condition that is not finished is worked out when the step is
    /// reached, fails there, and skips the step it guards, which an
    /// unfinished condition should do.
    pub fn write(&self) -> String {
        let (subject, field, value) =
            (self.subject.trim(), self.field.trim(), self.value.trim());
        if subject.is_empty() {
            return String::new();
        }
        let subj = if subject.contains(' ') {
            format!("tool({subject:?})")
        } else {
            subject.to_string()
        };
        if field.is_empty() {
            // A variable is the whole of its own left-hand side.
            if value.is_empty() {
                if self.test != Test::Is {
                    return format!("{subj} {}", self.test.symbol());
                }
                return subj;
            }
            return format!("{subj} {} {}", self.test.symbol(), quoted_value(value));
        }
        if value.is_empty() {
            if self.test != Test::Is {
                return format!("{subj}.{field} {}", self.test.symbol());
            }
            return format!("{subj}.{field}");
        }
        format!("{subj}.{field} {} {}", self.test.symbol(), quoted_value(value))
    }
}

/// A value with a space in it is text, and text in an expression is quoted.
///
/// Typing one into the box would otherwise write a condition the controls can
/// no longer read, and they would vanish from under the words being typed.
fn quoted_value(value: &str) -> String {
    let quoted = (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
        || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2);
    if quoted || !value.contains(char::is_whitespace) {
        return value.to_string();
    }
    format!("{value:?}")
}

/// What a `set` step keeps, in the shape the controls can build: one figure of
/// one run, optionally summarised, or one variable, which is a value already.
///
/// The same idea as [`Guide`], for the other side of the language. Anything
/// more involved than this does not fit, and the editor shows it as written
/// and does not pretend the controls describe it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Recipe {
    pub subject: String,
    pub field: String,
    /// `avg`, `max`, `count`: how a column of many rows becomes one value.
    pub summary: String,
}

/// The ways a column can be turned into a single value, and what each says.
pub const SUMMARIES: [(&str, &str); 10] = [
    ("", "every value"),
    ("avg", "the mean"),
    ("min", "the smallest"),
    ("max", "the largest"),
    ("sum", "added up"),
    ("count", "how many"),
    ("median", "the middle one"),
    ("p95", "the 95th percentile"),
    ("first", "the first"),
    ("last", "the last"),
];

impl Recipe {
    /// Reads an expression back into its parts, so a value that was typed can
    /// still be edited with the controls.
    pub fn read(expression: &str) -> Option<Recipe> {
        let text = expression.trim();
        if text.is_empty() {
            return Some(Recipe::default());
        }

        // `Sweep.rtt.avg()`: the summary is the call on the end.
        let (rest, summary) = match text.strip_suffix("()") {
            Some(head) => match head.rsplit_once('.') {
                Some((head, name)) if SUMMARIES.iter().any(|(s, _)| *s == name) => {
                    (head.trim(), name.to_string())
                }
                _ => return None,
            },
            None => (text, String::new()),
        };

        // The subject may be `tool("Port scan")`, which has a dot-free name of
        // its own; split after the closing bracket where there is one.
        let (subject, field) = match rest.strip_prefix("tool(") {
            Some(_) => match rest.find(')') {
                Some(at) => {
                    let (subject, tail) = rest.split_at(at + 1);
                    let field = tail.strip_prefix('.').unwrap_or(tail).trim();
                    (subject, field)
                }
                None => return None,
            },
            None => match rest.split_once('.') {
                Some((subject, field)) => (subject, field.trim()),
                None => (rest, ""),
            },
        };

        if !field.is_empty() && !field.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }
        let subject = parse_subject(subject.trim())?;
        Some(Recipe { subject, field: field.to_string(), summary })
    }

    /// Writes the parts back out, as far as they go.
    pub fn write(&self) -> String {
        let (subject, field, summary) =
            (self.subject.trim(), self.field.trim(), self.summary.trim());
        if subject.is_empty() {
            return String::new();
        }
        let subject =
            if subject.contains(' ') { format!("tool({subject:?})") } else { subject.to_string() };
        if field.is_empty() {
            return subject;
        }
        if summary.is_empty() {
            return format!("{subject}.{field}");
        }
        format!("{subject}.{field}.{summary}()")
    }
}

fn parse_subject(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(inner) = text.strip_prefix("tool(").and_then(|s| s.strip_suffix(')')) {
        let s = super::unquote(inner);
        return (!s.is_empty()).then_some(s);
    }
    if text.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ' ') {
        return Some(text.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::{parse, write};

    fn tree(source: &str) -> Vec<Step> {
        let flow = parse(source);
        assert!(flow.problems.is_empty(), "{:?}", flow.problems);
        flow.steps
    }

    fn shown(steps: &[Step]) -> String {
        write(&crate::flow::Flow { steps: steps.to_vec(), ..Default::default() })
    }

    #[test]
    fn a_step_is_added_after_the_one_that_is_selected() {
        let mut steps = tree("run A\nrun B");
        let spot = apply(&mut steps, &Spot::top(0), &Change::Add(Shape::Wait)).unwrap();
        assert_eq!(spot, Spot::top(1));
        assert_eq!(shown(&steps), "run A\nwait 30s\nrun B\n");
    }

    #[test]
    fn a_step_is_added_inside_a_branch() {
        let mut steps = tree("if c {\n  run A\n}");
        let spot = apply(&mut steps, &Spot::top(0), &Change::AddInside(Shape::Run, 0)).unwrap();
        assert_eq!(spot, Spot { inside: vec![(0, 0)], index: 1 });
        // Into the else half, which starts empty.
        apply(&mut steps, &Spot::top(0), &Change::AddInside(Shape::Stop, 1)).unwrap();
        assert_eq!(shown(&steps), "if c {\n  run A\n  run \"\"\n} else {\n  stop\n}\n");
    }

    #[test]
    fn a_step_deep_inside_is_reached_and_changed() {
        let mut steps = tree("repeat 2 {\n  if c {\n    run A\n  }\n}");
        let spot = Spot { inside: vec![(0, 0), (0, 0)], index: 0 };
        assert_eq!(at(&steps, &spot), Some(&Step::Run { name: "A".into(), condition: None }));
        apply(&mut steps, &spot, &Change::SetRun("Sweep".into())).unwrap();
        assert_eq!(shown(&steps), "repeat 2 {\n  if c {\n    run Sweep\n  }\n}\n");
    }

    #[test]
    fn steps_move_among_their_neighbours_and_no_further() {
        let mut steps = tree("run A\nrun B\nrun C");
        assert_eq!(apply(&mut steps, &Spot::top(2), &Change::Move(false)), Some(Spot::top(1)));
        assert_eq!(shown(&steps), "run A\nrun C\nrun B\n");
        // The first cannot go up and the last cannot go down.
        assert_eq!(apply(&mut steps, &Spot::top(0), &Change::Move(false)), None);
        assert_eq!(apply(&mut steps, &Spot::top(2), &Change::Move(true)), None);
        assert_eq!(shown(&steps), "run A\nrun C\nrun B\n");
    }

    #[test]
    fn removing_a_step_leaves_the_cursor_somewhere_real() {
        let mut steps = tree("run A\nrun B");
        assert_eq!(apply(&mut steps, &Spot::top(1), &Change::Remove), Some(Spot::top(0)));
        assert_eq!(apply(&mut steps, &Spot::top(0), &Change::Remove), None);
        assert!(steps.is_empty());
    }

    #[test]
    fn wrapping_makes_an_existing_step_conditional_without_rebuilding_it() {
        let mut steps = tree("run A");
        apply(&mut steps, &Spot::top(0), &Change::Wrap).unwrap();
        apply(&mut steps, &Spot::top(0), &Change::SetSubject("Sweep".into())).unwrap();
        apply(&mut steps, &Spot::top(0), &Change::SetField("up".into())).unwrap();
        assert_eq!(shown(&steps), "if Sweep.up {\n  run A\n}\n");
    }

    #[test]
    fn a_repeat_cannot_be_set_to_run_forever() {
        let mut steps = tree("repeat 2 {\n}");
        apply(&mut steps, &Spot::top(0), &Change::SetTimes(0)).unwrap();
        assert_eq!(shown(&steps), "repeat 1 {\n}\n");
        apply(&mut steps, &Spot::top(0), &Change::SetTimes(9999)).unwrap();
        assert_eq!(shown(&steps), "repeat 100 {\n}\n");
    }

    #[test]
    fn a_condition_is_built_one_control_at_a_time() {
        let mut steps = tree("run A");
        let spot = Spot::top(0);
        // Half-built, it is written down as far as it goes: what has been
        // chosen is not thrown away for what has not.
        apply(&mut steps, &spot, &Change::SetSubject("Sweep".into())).unwrap();
        assert_eq!(shown(&steps), "run A if Sweep\n");
        apply(&mut steps, &spot, &Change::SetField("up".into())).unwrap();
        assert_eq!(shown(&steps), "run A if Sweep.up\n");
        apply(&mut steps, &spot, &Change::SetTest(Test::More)).unwrap();
        apply(&mut steps, &spot, &Change::SetValue("0".into())).unwrap();
        assert_eq!(shown(&steps), "run A if Sweep.up > 0\n");
        // And changing one part keeps the rest.
        apply(&mut steps, &spot, &Change::SetTest(Test::Is)).unwrap();
        assert_eq!(shown(&steps), "run A if Sweep.up == 0\n");
        apply(&mut steps, &spot, &Change::ClearCondition).unwrap();
        assert_eq!(shown(&steps), "run A\n");
    }

    #[test]
    fn a_run_whose_name_has_a_space_is_named_the_long_way() {
        let mut steps = tree("run A");
        apply(&mut steps, &Spot::top(0), &Change::SetSubject("Port scan".into())).unwrap();
        apply(&mut steps, &Spot::top(0), &Change::SetField("open".into())).unwrap();
        apply(&mut steps, &Spot::top(0), &Change::SetValue("1".into())).unwrap();
        assert_eq!(shown(&steps), "run A if tool(\"Port scan\").open == 1\n");
    }

    #[test]
    fn a_condition_being_built_survives_the_file_it_is_written_to() {
        // The editor writes on every change, so anything it cannot write down
        // is lost the moment it is chosen. Each stage has to read back as the
        // stage it was.
        let mut steps = tree("run A");
        let spot = Spot::top(0);
        for (change, expected) in [
            (Change::SetSubject("Sweep".into()), "Sweep"),
            (Change::SetField("up".into()), "Sweep.up"),
            (Change::SetTest(Test::More), "Sweep.up >"),
            (Change::SetValue("0".into()), "Sweep.up > 0"),
        ] {
            apply(&mut steps, &spot, &change).unwrap();
            let written = shown(&steps);
            let back = parse(&written);
            assert!(back.problems.is_empty(), "{written:?}: {:?}", back.problems);
            assert_eq!(back.steps, steps, "{written:?} did not read back as itself");
            assert_eq!(
                Guide::read_partial(condition_of(&back.steps[0])).map(|g| g.write()),
                Some(expected.to_string()),
                "{written:?}"
            );
        }
    }

    #[test]
    fn a_value_with_a_space_in_it_is_written_as_the_text_it_is() {
        let mut steps = tree("run A");
        let spot = Spot::top(0);
        apply(&mut steps, &spot, &Change::SetSubject("Sweep".into())).unwrap();
        apply(&mut steps, &spot, &Change::SetField("state".into())).unwrap();
        apply(&mut steps, &spot, &Change::SetValue("not there".into())).unwrap();
        assert_eq!(shown(&steps), "run A if Sweep.state == \"not there\"\n");
        // And it is still a condition the controls describe, so they stay on
        // screen instead of giving up on what was just typed.
        let guide = Guide::read(condition_of(&steps[0])).expect("the controls to read it");
        assert_eq!(guide.value, "\"not there\"");
        assert_eq!(guide.write(), "Sweep.state == \"not there\"");
    }

    #[test]
    fn a_value_is_built_the_way_a_condition_is() {
        let mut steps = tree("set x = ");
        let spot = Spot::top(0);
        apply(&mut steps, &spot, &Change::SetValueSubject("Sweep".into())).unwrap();
        assert_eq!(shown(&steps), "set x = Sweep\n");
        apply(&mut steps, &spot, &Change::SetValueField("rtt".into())).unwrap();
        assert_eq!(shown(&steps), "set x = Sweep.rtt\n");
        apply(&mut steps, &spot, &Change::SetValueSummary("avg".into())).unwrap();
        assert_eq!(shown(&steps), "set x = Sweep.rtt.avg()\n");

        // Choosing another run drops the figure that belonged to the old one.
        apply(&mut steps, &spot, &Change::SetValueSubject("Ping".into())).unwrap();
        assert_eq!(shown(&steps), "set x = Ping\n");
    }

    #[test]
    fn a_value_that_was_typed_reads_back_into_the_controls() {
        for (written, subject, field, summary) in [
            ("Sweep.up", "Sweep", "up", ""),
            ("Sweep.rtt.avg()", "Sweep", "rtt", "avg"),
            ("hosts_up", "hosts_up", "", ""),
            ("tool(\"Port scan\").open", "Port scan", "open", ""),
        ] {
            let recipe = Recipe::read(written).unwrap_or_else(|| panic!("{written}"));
            assert_eq!(
                recipe,
                Recipe {
                    subject: subject.into(),
                    field: field.into(),
                    summary: summary.into()
                },
                "{written}"
            );
            assert_eq!(recipe.write(), written);
        }
    }

    #[test]
    fn a_value_too_involved_for_the_controls_is_left_as_written() {
        for written in ["Sweep.up * 2", "if Sweep.up > 0 then 1 else 0", "join(Sweep.host)"] {
            assert_eq!(Recipe::read(written), None, "{written}");
        }
    }

    #[test]
    fn a_condition_can_be_about_a_variable_rather_than_a_run() {
        // A run has figures and needs a dot to name one; a variable is a
        // value already, and the controls have nothing to ask after it.
        let guide = Guide::read("hosts_up > 0").expect("the controls to read it");
        assert_eq!(guide.subject, "hosts_up");
        assert!(guide.field.is_empty());
        assert_eq!(guide.test, Test::More);
        assert_eq!(guide.value, "0");
        assert_eq!(guide.write(), "hosts_up > 0");

        // And it survives being written into a workflow and read back.
        let mut steps = tree("run A");
        let spot = Spot::top(0);
        apply(&mut steps, &spot, &Change::SetSubject("hosts_up".into())).unwrap();
        apply(&mut steps, &spot, &Change::SetTest(Test::AtLeast)).unwrap();
        apply(&mut steps, &spot, &Change::SetValue("2".into())).unwrap();
        assert_eq!(shown(&steps), "run A if hosts_up >= 2\n");
        assert_eq!(parse(&shown(&steps)).steps, steps);
    }

    #[test]
    fn a_typed_condition_can_still_be_edited_with_the_controls() {
        let guide = Guide::read("Sweep.up >= 2").unwrap();
        assert_eq!(
            guide,
            Guide {
                subject: "Sweep".into(),
                field: "up".into(),
                test: Test::AtLeast,
                value: "2".into(),
            }
        );
        assert_eq!(guide.write(), "Sweep.up >= 2");
    }

    #[test]
    fn a_condition_too_involved_for_the_controls_is_left_as_written() {
        // The editor shows these as text and does not pretend the controls
        // describe them.
        for condition in
            ["Sweep.up > 0 and Ping.ok", "exists(tool(\"X\"))", "Sweep.rtt.max() > 5", "Sweep.ok"]
        {
            assert_eq!(Guide::read(condition), None, "{condition}");
        }
    }

    #[test]
    fn every_shape_makes_a_step_that_reads_and_writes_again() {
        for shape in Shape::ALL {
            let steps = vec![shape.blank()];
            let text = shown(&steps);
            let back = parse(&text);
            assert_eq!(back.steps, steps, "{shape:?} wrote as {text:?}");
        }
    }
}
