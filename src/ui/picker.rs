//! A searchable list, shown over the window.
//!
//! The command bar is the right shape for choosing one thing out of many, so
//! anything else that has to (which run to compare against, which folder to
//! move a tool into) borrows it without growing a submenu that becomes
//! unusable at twenty entries.

use gpui::{AppContext, Context, Entity, Window};

use super::text_input::TextInput;

/// One thing that can be chosen.
#[derive(Clone, Debug)]
pub struct Choice {
    /// What choosing it means, for the caller to act on.
    pub id: usize,
    pub icon: &'static str,
    pub label: String,
    pub detail: String,
}

/// What a picker is for, and what the caller does with the answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    /// Choose another run to read the given one against.
    Compare { job: usize },
    /// Choose the folder to move the given tool into.
    Move { job: usize },
    /// Choose the folder to move the given document into.
    MoveDoc { doc: usize },
    /// Choose the folder to move the given workflow into.
    MoveFlow { flow: usize },
    /// Create a new folder.
    CreateFolder,
    /// Choose the run a workflow step should start.
    StepRun { flow: usize },
}

/// The id of the row that means "what I typed", for a picker that can make a
/// new thing as well as choose an existing one.
pub const NEW: usize = usize::MAX;

pub struct Picker {
    pub input: Entity<TextInput>,
    pub title: String,
    pub purpose: Purpose,
    pub choices: Vec<Choice>,
    pub cursor: usize,
    /// Set when typing a name that is not in the list offers to create it, and
    /// says what creating it is called.
    pub new_label: Option<String>,
    revision: usize,
    query: String,
}

impl Picker {
    pub fn new(cx: &mut Context<Self>) -> Picker {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "Search");
            input.mono = false;
            input
        });
        Picker {
            input,
            title: String::new(),
            purpose: Purpose::Compare { job: 0 },
            choices: Vec::new(),
            cursor: 0,
            new_label: None,
            revision: 0,
            query: String::new(),
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether the box would make something new, and what it would be called.
    pub fn offering_new(&self) -> Option<String> {
        let label = self.new_label.as_ref()?;
        let typed = self.query.trim();
        if typed.is_empty() {
            return None;
        }
        if self.choices.iter().any(|c| c.label.eq_ignore_ascii_case(typed)) {
            return None;
        }
        Some(format!("{label} \u{201c}{typed}\u{201d}"))
    }

    /// Points it at a new question and puts the caret in the box.
    pub fn open(
        &mut self,
        title: impl Into<String>,
        purpose: Purpose,
        choices: Vec<Choice>,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title = title.into();
        self.purpose = purpose;
        self.choices = choices;
        self.cursor = 0;
        self.new_label = None;
        self.query.clear();

        let placeholder = placeholder.to_string();
        let input = self.input.clone();
        input.update(cx, |input, cx| {
            input.placeholder = placeholder.into();
            input.set_value("", cx);
        });
        self.revision = self.input.read(cx).revision;
        let handle = self.input.read(cx).focus_handle.clone();
        window.focus(&handle);
    }

    /// Picks up what has been typed, returning true when it changed.
    pub fn sync(&mut self, cx: &Context<Self>) -> bool {
        let input = self.input.read(cx);
        if input.revision == self.revision {
            return false;
        }
        self.revision = input.revision;
        self.query = input.value().to_string();
        self.cursor = 0;
        true
    }

    /// The choices matching what has been typed, best first.
    pub fn matches(&self) -> Vec<&Choice> {
        matching(&self.choices, &self.query)
    }

    /// How many rows there are, counting the one that makes something new.
    pub fn len(&self) -> usize {
        self.matches().len() + usize::from(self.offering_new().is_some())
    }

    pub fn chosen(&self) -> Option<usize> {
        let matches = self.matches();
        if self.cursor < matches.len() {
            return Some(matches[self.cursor].id);
        }
        self.offering_new().map(|_| NEW)
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let len = self.len();
        if len == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor as isize + delta).rem_euclid(len as isize) as usize;
    }
}

/// The choices matching a query, best first: the start of a name before a
/// mention of it, and a mention of the name before one in the detail.
pub fn matching<'a>(choices: &'a [Choice], query: &str) -> Vec<&'a Choice> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return choices.iter().collect();
    }
    let mut scored: Vec<(u8, usize, &Choice)> = choices
        .iter()
        .enumerate()
        .filter_map(|(i, choice)| {
            let (label, detail) = (choice.label.to_lowercase(), choice.detail.to_lowercase());
            let rank = if label.starts_with(&needle) {
                0
            } else if label.contains(&needle) {
                1
            } else if detail.contains(&needle) {
                2
            } else {
                return None;
            };
            Some((rank, i, choice))
        })
        .collect();
    scored.sort_by_key(|(rank, i, _)| (*rank, *i));
    scored.into_iter().map(|(_, _, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices() -> Vec<Choice> {
        vec![
            Choice { id: 1, icon: "ping", label: "Last week".into(), detail: "10.0.0.0/24".into() },
            Choice { id: 2, icon: "ping", label: "Today".into(), detail: "10.0.0.0/24".into() },
            Choice {
                id: 3,
                icon: "ping",
                label: "Yesterday".into(),
                detail: "192.168.1.0/24".into(),
            },
        ]
    }

    #[test]
    fn an_empty_query_offers_everything_in_order() {
        let all = choices();
        let ids: Vec<usize> = matching(&all, "  ").iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn the_start_of_a_name_beats_a_mention_of_it() {
        let all = choices();
        let ids: Vec<usize> = matching(&all, "y").iter().map(|c| c.id).collect();
        // Yesterday starts with it; Today only contains it.
        assert_eq!(ids, vec![3, 2]);
    }

    #[test]
    fn the_detail_is_searched_after_the_name() {
        let all = choices();
        let ids: Vec<usize> = matching(&all, "192.168").iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![3]);
    }

    #[test]
    fn a_query_that_matches_nothing_offers_nothing() {
        assert!(matching(&choices(), "qqqq").is_empty());
    }
}
