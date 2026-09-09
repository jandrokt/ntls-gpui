//! Turning a workflow into something that can be stepped through.
//!
//! The tree says what the workflow *is*; running it needs somewhere to be in
//! the middle of, and a branch or a loop has no obvious "next" in a tree. So
//! the tree is compiled once, when the workflow starts, into a flat list of
//! operations with a program counter, for the same reason a language has a
//! bytecode and does not walk its syntax.
//!
//! A repeat becomes a counter on a stack instead of a hundred copies of its
//! body, so nesting two of them costs two operations and not ten thousand.
//!
//! Nothing here knows what a run is or how to evaluate a condition. The caller
//! answers both, so the whole machine can be tested without a window.

use super::Step;

#[derive(Clone, PartialEq, Debug)]
pub enum Op {
    /// Start this run, and wait for it, having first set these of its fields.
    Run { name: String, with: Vec<(String, String)> },
    /// If the condition is false, jump; otherwise carry on.
    Check { condition: String, otherwise: usize },
    Jump(usize),
    /// Begin a loop: push a counter, or jump past it if there is nothing to do.
    Enter { times: usize, end: usize },
    /// End of a loop: go round again, or drop the counter and carry on.
    Again { start: usize },
    /// Begin a walk over a list: work the expression out, and either jump
    /// past the body or take the first item.
    Each { name: String, over: String, end: usize },
    /// End of a walk: take the next item, or drop the list and carry on.
    Step { start: usize },
    Wait(f64),
    /// Work out an expression and keep the answer under a name.
    Set { name: String, value: String },
    /// Work out an expression and say what it came to, changing nothing.
    Log(String),
    Halt,
}

/// One operation, and the step of the tree it came from, so what happens can
/// be shown against what was written.
#[derive(Clone, PartialEq, Debug)]
pub struct Line {
    pub op: Op,
    pub step: usize,
}

/// What the machine needs the caller to do, or what it just did.
#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    /// Start this run and call [`Machine::resume`] when it finishes. `with`
    /// names the fields to set first, each to whatever its expression comes
    /// to; the caller works those out, the same way it answers a condition.
    Run { step: usize, name: String, with: Vec<(String, String)> },
    /// A walk over a list reached its next item. Keep it under this name,
    /// then carry on. `at` and `of` are which one it is, for the trail.
    Each { step: usize, name: String, value: String, at: usize, of: usize },
    /// Work this out and put it in the trail.
    Logged { step: usize, value: String },
    /// Pause, then carry on.
    Wait { step: usize, seconds: f64 },
    /// Work this out and keep it under this name, then carry on. The caller
    /// answers it, the same way it answers a condition.
    Set { step: usize, name: String, value: String },
    /// A condition was false, or could not be answered.
    Skipped { step: usize, why: String },
    /// A `stop` was reached.
    Stopped { step: usize },
    /// It was doing far too much to be doing anything useful, and was given up
    /// on where it stood. `why` says so in words, for the trail to show.
    Failed { step: usize, why: String },
    /// There is nothing left.
    Finished,
}

/// How many operations one run of a workflow may do before it is given up on.
///
/// Reading a file clamps one repeat to a hundred passes, and that is as far as
/// clamping can go: nesting multiplies, so four repeats of a hundred inside
/// one another are a hundred million passes over their body. A million
/// operations is a few tens of milliseconds, which nobody sees, and further
/// than any workflow that stops to run something ever gets.
const MOST_OPERATIONS: usize = 1_000_000;

/// What the machine cannot work out for itself.
///
/// Nothing in this module knows what a run is or how to read an expression,
/// so the caller answers both and the whole machine can be tested without a
/// window.
pub trait Ask {
    /// Whether a condition holds.
    fn truth(&mut self, condition: &str) -> Result<bool, String>;
    /// What a list comes to, one item per entry.
    fn list(&mut self, expression: &str) -> Result<Vec<String>, String>;
}

/// One loop that is open, and how far through it the machine is.
#[derive(Clone, Debug)]
enum Frame {
    /// A `repeat`, and how many passes are left.
    Times(usize),
    /// A `for each`, the items it is walking, and which one it is on.
    Each { items: Vec<String>, at: usize },
}

/// A workflow part-way through.
#[derive(Clone, Debug)]
pub struct Machine {
    program: Vec<Line>,
    pc: usize,
    /// The open loops, innermost last.
    frames: Vec<Frame>,
    /// How many operations are left before it is given up on. For the whole
    /// run and not for one call, since a loop going round for ever goes round
    /// across as many calls as the caller cares to make.
    operations_left: usize,
    done: bool,
}

impl Machine {
    pub fn new(steps: &[Step]) -> Machine {
        Machine {
            program: compile(steps),
            pc: 0,
            frames: Vec::new(),
            operations_left: MOST_OPERATIONS,
            done: false,
        }
    }

    /// Works forward until something has to happen outside the machine.
    ///
    /// `ask` answers the questions this cannot: whether a condition holds,
    /// and what a list comes to. Both may fail, since either can name a run
    /// that has not been done. A condition that cannot be answered is treated
    /// as false rather than as a reason to stop, since the step it guards is
    /// exactly the step that should not run; a list that cannot be worked out
    /// is treated as empty, for the same reason.
    pub fn next(&mut self, ask: &mut impl Ask) -> Event {
        loop {
            if self.done {
                return Event::Finished;
            }
            let Some(line) = self.program.get(self.pc).cloned() else {
                self.done = true;
                return Event::Finished;
            };
            self.pc += 1;

            // Clamping one repeat's count does not bound the product of nested
            // ones, and a body of nothing but loops and untaken branches asks
            // the caller for nothing, so all hundred million passes happened
            // inside this one call, which is made from the click that started
            // the workflow, on the thread that draws the window. Nothing
            // repainted, Stop could not be reached, and the application had to
            // be killed. A workflow this far in is not going to finish, so it
            // ends here and the trail says why rather than showing it as done.
            if self.operations_left == 0 {
                self.done = true;
                let why = format!(
                    "gave up after {MOST_OPERATIONS} operations: the repeats in it ask for \
                     more passes than one run of a workflow may do"
                );
                return Event::Failed { step: line.step, why };
            }
            self.operations_left -= 1;

            match line.op {
                Op::Run { name, with } => return Event::Run { step: line.step, name, with },
                Op::Log(value) => return Event::Logged { step: line.step, value },
                Op::Wait(seconds) => return Event::Wait { step: line.step, seconds },
                Op::Set { name, value } => return Event::Set { step: line.step, name, value },
                Op::Halt => {
                    self.done = true;
                    return Event::Stopped { step: line.step };
                }
                Op::Jump(to) => self.pc = to,
                Op::Check { condition, otherwise } => {
                    // A check that guards a step of its own stands directly in
                    // front of that step's own work, which is a run or a stop.
                    // Sharing the step number was not enough to tell the two
                    // apart: a branch whose `then` half is empty puts its own
                    // jump over the `else` half where that half would have
                    // been, and the jump carries the branch's step number, so
                    // `if up {} else { run B }` reported the branch as skipped
                    // and then ran B. The trail said a step had not happened
                    // while the workflow was doing it.
                    let guards = self.program.get(self.pc).is_some_and(|l| {
                        l.step == line.step && matches!(l.op, Op::Run { .. } | Op::Halt)
                    });
                    match ask.truth(&condition) {
                        Ok(true) => {}
                        Ok(false) => {
                            self.pc = otherwise;
                            // Only a step of its own is worth reporting as skipped;
                            // the untaken half of a branch is not a step.
                            if guards {
                                return Event::Skipped { step: line.step, why: condition };
                            }
                        }
                        Err(why) => {
                            self.pc = otherwise;
                            if guards {
                                return Event::Skipped { step: line.step, why };
                            }
                        }
                    }
                }
                Op::Enter { times, end } => {
                    if times == 0 {
                        self.pc = end;
                    } else {
                        self.frames.push(Frame::Times(times));
                    }
                }
                Op::Again { start } => match self.frames.last_mut() {
                    Some(Frame::Times(1)) | None => {
                        self.frames.pop();
                    }
                    Some(Frame::Times(left)) => {
                        *left -= 1;
                        self.pc = start + 1;
                    }
                    // A `for each` closes with its own operation, so a frame
                    // of the other kind here is a compiler fault, not a
                    // workflow's. Dropping it is what keeps the stack honest.
                    Some(Frame::Each { .. }) => {
                        self.frames.pop();
                    }
                },
                Op::Each { name, over, end } => {
                    // A list that cannot be worked out is an empty one, for
                    // the same reason an unanswerable condition is false: the
                    // body is exactly what should not happen.
                    let items = ask.list(&over).unwrap_or_default();
                    let of = items.len();
                    let Some(first) = items.first().cloned() else {
                        self.pc = end;
                        continue;
                    };
                    self.frames.push(Frame::Each { items, at: 0 });
                    return Event::Each { step: line.step, name, value: first, at: 1, of };
                }
                Op::Step { start } => {
                    let Some(Frame::Each { items, at }) = self.frames.last_mut() else {
                        self.frames.pop();
                        continue;
                    };
                    *at += 1;
                    let Some(value) = items.get(*at).cloned() else {
                        self.frames.pop();
                        continue;
                    };
                    let (at, of) = (*at + 1, items.len());
                    // The name is on the operation that opened the loop.
                    let name = match &self.program[start].op {
                        Op::Each { name, .. } => name.clone(),
                        _ => String::new(),
                    };
                    self.pc = start + 1;
                    return Event::Each { step: line.step, name, value, at, of };
                }
            }
        }
    }

    /// Tells the machine how the run it was waiting on went. A run that failed
    /// ends the workflow: carrying on would mean acting on results that are
    /// not there.
    pub fn resume(&mut self, ok: bool) {
        if !ok {
            self.done = true;
        }
    }
}

fn compile(steps: &[Step]) -> Vec<Line> {
    let mut out = Vec::new();
    emit(steps, &mut out, &mut 0);
    out
}

/// Writes out one block. `id` numbers the steps depth-first, the same order
/// the editor draws them in, so an operation can point back at the card it
/// came from.
fn emit(steps: &[Step], out: &mut Vec<Line>, id: &mut usize) {
    for step in steps {
        let me = *id;
        *id += 1;

        match step {
            Step::Run { name, condition, with } => {
                let check = guard(out, me, condition);
                out.push(Line {
                    op: Op::Run { name: name.clone(), with: with.clone() },
                    step: me,
                });
                land(out, check);
            }
            Step::Stop { condition } => {
                let check = guard(out, me, condition);
                out.push(Line { op: Op::Halt, step: me });
                land(out, check);
            }
            Step::Wait { seconds } => out.push(Line { op: Op::Wait(*seconds), step: me }),
            Step::Log { value } => {
                out.push(Line { op: Op::Log(value.clone()), step: me });
            }
            Step::Set { name, value } => out.push(Line {
                op: Op::Set { name: name.clone(), value: value.clone() },
                step: me,
            }),
            Step::If { condition, then, otherwise } => {
                let check = out.len();
                out.push(Line {
                    op: Op::Check { condition: condition.clone(), otherwise: 0 },
                    step: me,
                });
                emit(then, out, id);

                if otherwise.is_empty() {
                    land(out, Some(check));
                } else {
                    let jump = out.len();
                    out.push(Line { op: Op::Jump(0), step: me });
                    land(out, Some(check));
                    emit(otherwise, out, id);
                    let end = out.len();
                    out[jump].op = Op::Jump(end);
                }
            }
            Step::Repeat { times, body } => {
                let enter = out.len();
                out.push(Line { op: Op::Enter { times: *times, end: 0 }, step: me });
                emit(body, out, id);
                out.push(Line { op: Op::Again { start: enter }, step: me });
                let end = out.len();
                out[enter].op = Op::Enter { times: *times, end };
            }
            Step::ForEach { name, over, body } => {
                let each = out.len();
                out.push(Line {
                    op: Op::Each { name: name.clone(), over: over.clone(), end: 0 },
                    step: me,
                });
                emit(body, out, id);
                out.push(Line { op: Op::Step { start: each }, step: me });
                let end = out.len();
                out[each].op =
                    Op::Each { name: name.clone(), over: over.clone(), end };
            }
            // A `while` needs no frame: the check at the top and the jump
            // back at the bottom are the whole of it. What stops one that
            // never comes false is the operation budget, the same thing that
            // stops a nest of repeats.
            Step::While { condition, body } => {
                let check = out.len();
                out.push(Line {
                    op: Op::Check { condition: condition.clone(), otherwise: 0 },
                    step: me,
                });
                emit(body, out, id);
                out.push(Line { op: Op::Jump(check), step: me });
                let end = out.len();
                out[check].op = Op::Check { condition: condition.clone(), otherwise: end };
            }
        }
    }
}

fn guard(out: &mut Vec<Line>, step: usize, condition: &Option<String>) -> Option<usize> {
    let condition = condition.as_ref()?;
    out.push(Line { op: Op::Check { condition: condition.clone(), otherwise: 0 }, step });
    Some(out.len() - 1)
}

/// Points a check at wherever the block it guards turned out to end.
fn land(out: &mut Vec<Line>, check: Option<usize>) {
    let Some(check) = check else { return };
    let end = out.len();
    if let Op::Check { condition, .. } = &out[check].op {
        out[check].op = Op::Check { condition: condition.clone(), otherwise: end };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::parse;

    #[test]
    fn a_walk_over_a_list_runs_its_body_once_for_each_item() {
        let seen = traced(
            "for each host in Sweep.host {\n  run \"Port scan\"\n}\nrun \"Report\"\n",
            &[],
            &[("Sweep.host", &["10.0.0.1", "10.0.0.2", "10.0.0.3"])],
        );
        assert_eq!(
            seen,
            vec![
                "each host = 10.0.0.1 (1/3)",
                "run Port scan",
                "each host = 10.0.0.2 (2/3)",
                "run Port scan",
                "each host = 10.0.0.3 (3/3)",
                "run Port scan",
                "run Report",
            ]
        );
    }

    #[test]
    fn a_walk_over_nothing_does_nothing_and_carries_on() {
        // An empty list is not a reason to stop: the sweep found nothing, so
        // there is nothing to scan, and the step after it still happens.
        let seen = traced(
            "for each host in Sweep.host {\n  run \"Port scan\"\n}\nrun \"Report\"\n",
            &[],
            &[("Sweep.host", &[])],
        );
        assert_eq!(seen, vec!["run Report"]);

        // And a list that cannot be worked out at all is an empty one, the
        // same way a condition that cannot be answered is false.
        let seen = traced(
            "for each host in Nowhere.host {\n  run \"Port scan\"\n}\nrun \"Report\"\n",
            &[],
            &[],
        );
        assert_eq!(seen, vec!["run Report"]);
    }

    #[test]
    fn walks_inside_walks_keep_their_own_places() {
        let seen = traced(
            "for each a in As {\n  for each b in Bs {\n    run \"X\"\n  }\n}\n",
            &[],
            &[("As", &["1", "2"]), ("Bs", &["x", "y"])],
        );
        assert_eq!(
            seen,
            vec![
                "each a = 1 (1/2)",
                "each b = x (1/2)",
                "run X",
                "each b = y (2/2)",
                "run X",
                "each a = 2 (2/2)",
                "each b = x (1/2)",
                "run X",
                "each b = y (2/2)",
                "run X",
            ]
        );
    }

    #[test]
    fn a_while_goes_round_until_its_condition_comes_false() {
        // Answered true four times and then false, which is the shape of
        // waiting for something to come up.
        let flow = parse("while Queue.busy {\n  run \"Drain\"\n}\nrun \"Done\"\n");
        let mut machine = Machine::new(&flow.steps);
        let mut left = 3;
        struct Countdown<'a>(&'a mut i32);
        impl Ask for Countdown<'_> {
            fn truth(&mut self, _: &str) -> Result<bool, String> {
                *self.0 -= 1;
                Ok(*self.0 >= 0)
            }
            fn list(&mut self, _: &str) -> Result<Vec<String>, String> {
                Ok(Vec::new())
            }
        }
        let mut seen = Vec::new();
        for _ in 0..50 {
            match machine.next(&mut Countdown(&mut left)) {
                Event::Run { name, .. } => {
                    seen.push(name);
                    machine.resume(true);
                }
                Event::Finished => break,
                _ => {}
            }
        }
        assert_eq!(seen, vec!["Drain", "Drain", "Drain", "Done"]);
    }

    #[test]
    fn a_while_that_never_comes_false_is_given_up_on_rather_than_hanging() {
        // The one thing a condition-driven loop can do that a counted one
        // cannot. A body that asks the caller for nothing spins entirely
        // inside one call to `next`, which is made from the click that
        // started the workflow, on the thread that draws the window: without
        // a budget the application is simply gone. It has to end somewhere,
        // and the trail has to say so rather than showing it as done.
        let flow = parse("while always {\n}\n");
        assert!(flow.problems.is_empty(), "{:?}", flow.problems);
        let mut machine = Machine::new(&flow.steps);
        let event = machine.next(&mut Table { truth: &[("always", Ok(true))], lists: &[] });
        let Event::Failed { why, .. } = event else { panic!("expected a failure, got {event:?}") };
        assert!(why.contains("gave up"), "{why}");
        // And it is over: it does not carry on from where it was given up on.
        assert_eq!(
            machine.next(&mut Table { truth: &[("always", Ok(true))], lists: &[] }),
            Event::Finished
        );
    }

    #[test]
    fn a_run_carries_the_fields_the_step_set() {
        let seen = trace(
            "run \"Port scan\" with target = host, ports = \"1-1024\"\n",
            &[],
        );
        assert_eq!(seen, vec![r#"run Port scan with target=host,ports="1-1024""#]);
    }

    #[test]
    fn a_note_says_something_and_changes_nothing() {
        let seen = trace("print \"starting\"\nrun \"A\"\n", &[]);
        assert_eq!(seen, vec![r#"log "starting""#, "run A"]);
    }

    /// Answers a machine's questions from two lookup tables.
    struct Table<'a> {
        truth: &'a [(&'a str, Result<bool, &'a str>)],
        lists: &'a [(&'a str, &'a [&'a str])],
    }

    impl Ask for Table<'_> {
        fn truth(&mut self, condition: &str) -> Result<bool, String> {
            match self.truth.iter().find(|(c, _)| *c == condition) {
                Some((_, Ok(yes))) => Ok(*yes),
                Some((_, Err(why))) => Err((*why).to_string()),
                None => Err(format!("no answer for {condition:?}")),
            }
        }

        fn list(&mut self, expression: &str) -> Result<Vec<String>, String> {
            match self.lists.iter().find(|(e, _)| *e == expression) {
                Some((_, items)) => Ok(items.iter().map(|s| (*s).to_string()).collect()),
                None => Err(format!("no list for {expression:?}")),
            }
        }
    }

    /// A machine that is never asked anything, for the workflows that ask
    /// nothing.
    struct Nothing;

    impl Ask for Nothing {
        fn truth(&mut self, _: &str) -> Result<bool, String> {
            Ok(true)
        }
        fn list(&mut self, _: &str) -> Result<Vec<String>, String> {
            Ok(Vec::new())
        }
    }

    /// Runs a workflow to the end, answering conditions from a table, and
    /// reports what happened as short strings.
    fn trace(source: &str, truth: &[(&str, Result<bool, &str>)]) -> Vec<String> {
        traced(source, truth, &[])
    }

    /// The same, for a workflow that walks a list.
    fn traced(
        source: &str,
        truth: &[(&str, Result<bool, &str>)],
        lists: &[(&str, &[&str])],
    ) -> Vec<String> {
        let flow = parse(source);
        assert!(flow.problems.is_empty(), "{:?}", flow.problems);
        let mut machine = Machine::new(&flow.steps);
        let mut answer = Table { truth, lists };

        let mut seen = Vec::new();
        for _ in 0..500 {
            match machine.next(&mut answer) {
                Event::Run { name, with, .. } => {
                    let settings: Vec<String> =
                        with.iter().map(|(f, v)| format!("{f}={v}")).collect();
                    if settings.is_empty() {
                        seen.push(format!("run {name}"));
                    } else {
                        seen.push(format!("run {name} with {}", settings.join(",")));
                    }
                    machine.resume(true);
                }
                Event::Each { name, value, at, of, .. } => {
                    seen.push(format!("each {name} = {value} ({at}/{of})"));
                }
                Event::Logged { value, .. } => seen.push(format!("log {value}")),
                Event::Wait { seconds, .. } => seen.push(format!("wait {seconds}")),
                Event::Set { name, value, .. } => seen.push(format!("set {name} = {value}")),
                Event::Skipped { why, .. } => seen.push(format!("skip {why}")),
                Event::Stopped { .. } => {
                    seen.push("stop".into());
                    break;
                }
                Event::Failed { why, .. } => {
                    seen.push(format!("gave up: {why}"));
                    break;
                }
                Event::Finished => break,
            }
        }
        seen
    }

    #[test]
    fn steps_happen_in_order() {
        assert_eq!(trace("run A\nrun B\nwait 5s", &[]), ["run A", "run B", "wait 5"]);
    }

    #[test]
    fn a_condition_decides_whether_a_step_happens() {
        assert_eq!(
            trace("run A if yes\nrun B if no", &[("yes", Ok(true)), ("no", Ok(false))]),
            ["run A", "skip no"]
        );
    }

    #[test]
    fn a_condition_that_cannot_be_answered_skips_rather_than_stops() {
        // A condition naming a run that has not happened is the common case,
        // and the step it guards is exactly the one that should not run.
        let seen = trace("run A if unknowable\nrun B", &[("unknowable", Err("no results yet"))]);
        assert_eq!(seen, ["skip no results yet", "run B"]);
    }

    #[test]
    fn a_branch_takes_one_side_or_the_other() {
        let source = "if up {\n  run A\n  run B\n} else {\n  run C\n}\nrun D";
        assert_eq!(trace(source, &[("up", Ok(true))]), ["run A", "run B", "run D"]);
        assert_eq!(trace(source, &[("up", Ok(false))]), ["run C", "run D"]);
    }

    #[test]
    fn a_branch_with_no_else_carries_on_past_it() {
        let source = "if up {\n  run A\n}\nrun B";
        assert_eq!(trace(source, &[("up", Ok(false))]), ["run B"]);
        assert_eq!(trace(source, &[("up", Ok(true))]), ["run A", "run B"]);
    }

    #[test]
    fn a_branch_with_an_empty_then_half_is_not_called_skipped_while_its_else_half_runs() {
        // A branch is added in the editor with both halves empty, and filling
        // in only the else half is an ordinary thing to do. Such a branch has
        // nothing between its check and the jump over the else half, and that
        // jump belongs to the branch, so the trail marked the branch skipped
        // in the same breath as running B.
        let source = "if up {\n} else {\n  run B\n}\nrun C";
        assert_eq!(trace(source, &[("up", Ok(false))]), ["run B", "run C"]);
        assert_eq!(trace(source, &[("up", Ok(true))]), ["run C"]);
        // A condition that cannot be answered takes the else half too, and
        // says no more about it than a false one does.
        assert_eq!(trace(source, &[("up", Err("no results yet"))]), ["run B", "run C"]);
    }

    #[test]
    fn a_repeat_does_its_body_that_many_times() {
        assert_eq!(
            trace("repeat 3 {\n  run A\n}\nrun B", &[]),
            ["run A", "run A", "run A", "run B"]
        );
    }

    #[test]
    fn a_repeat_costs_the_same_however_many_passes_it_asks_for() {
        // Unrolling would make a nested pair of hundred-pass loops ten
        // thousand operations long.
        let few = Machine::new(&parse("repeat 2 {\n run A\n}").steps).program.len();
        let many = Machine::new(&parse("repeat 100 {\n run A\n}").steps).program.len();
        assert_eq!(few, many);
    }

    #[test]
    fn repeats_nest() {
        assert_eq!(
            trace("repeat 2 {\n  run A\n  repeat 2 {\n    run B\n  }\n}", &[]),
            ["run A", "run B", "run B", "run A", "run B", "run B"]
        );
    }

    #[test]
    fn a_variable_is_set_where_the_step_is_reached() {
        // The machine does not work the expression out; the caller does,
        // against what the steps before it found.
        assert_eq!(
            trace("run A\nset gateway = Sweep.up\nrun B", &[]),
            ["run A", "set gateway = Sweep.up", "run B"]
        );
    }

    #[test]
    fn a_stop_ends_it_wherever_it_is() {
        assert_eq!(trace("run A\nrepeat 5 {\n  stop\n}\nrun B", &[]), ["run A", "stop"]);
    }

    #[test]
    fn a_conditional_stop_only_stops_when_it_should() {
        assert_eq!(trace("stop if bad\nrun A", &[("bad", Ok(false))]), ["skip bad", "run A"]);
        assert_eq!(trace("stop if bad\nrun A", &[("bad", Ok(true))]), ["stop"]);
    }

    #[test]
    fn a_run_that_fails_ends_the_workflow() {
        let flow = parse("run A\nrun B");
        let mut machine = Machine::new(&flow.steps);
        assert!(matches!(machine.next(&mut Nothing), Event::Run { .. }));
        machine.resume(false);
        assert_eq!(machine.next(&mut Nothing), Event::Finished);
        assert!(machine.done);
    }

    #[test]
    fn a_chained_branch_only_runs_its_side_when_its_own_condition_holds() {
        // The second condition used to be dropped as the file was read, so
        // this ran B whenever `a` was false, whatever `b` said.
        let source = "if a {\n  run A\n} else if b {\n  run B\n} else {\n  run C\n}\nrun D";
        assert_eq!(trace(source, &[("a", Ok(true)), ("b", Ok(true))]), ["run A", "run D"]);
        assert_eq!(trace(source, &[("a", Ok(false)), ("b", Ok(true))]), ["run B", "run D"]);
        assert_eq!(trace(source, &[("a", Ok(false)), ("b", Ok(false))]), ["run C", "run D"]);
    }

    #[test]
    fn a_file_of_repeats_inside_repeats_gives_up_instead_of_hanging() {
        // Clamping one repeat to a hundred passes does not bound the product
        // of nested ones. Four of them are a hundred million passes over a
        // body that asks the caller for nothing, so every one of those passes
        // happened inside a single call, made from the click that started the
        // workflow, on the thread that draws the window.
        let source = "repeat 100 {\n repeat 100 {\n  repeat 100 {\n   repeat 100 {\n    if a {\n    }\n   }\n  }\n }\n}";
        let seen = trace(source, &[("a", Ok(false))]);
        assert!(
            seen.iter().any(|line| line.starts_with("gave up")),
            "it has to end rather than spin: {seen:?}"
        );

        // A repeat somebody actually meant still does every pass of it.
        let runs = trace("repeat 100 {\n  run A\n}", &[]);
        assert_eq!(runs.iter().filter(|line| *line == "run A").count(), 100);
    }

    #[test]
    fn an_operation_points_back_at_the_step_it_came_from() {
        // The editor numbers its cards depth-first; this is what lets a card be
        // marked with what happened to it.
        let flow = parse("run A\nif c {\n  run B\n}\nrun D");
        let mut machine = Machine::new(&flow.steps);
        let steps: Vec<usize> = std::iter::from_fn(|| match machine.next(&mut Nothing) {
            Event::Run { step, .. } => Some(step),
            _ => None,
        })
        .collect();
        // A is 0, the branch is 1, B inside it is 2, D is 3.
        assert_eq!(steps, [0, 2, 3]);
    }
}
