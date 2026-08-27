//! The command bar: search, and the command line behind it.
//!
//! Anything typed is first a search across everything open — see
//! [`super::search`]. Once the first word names a tool and is followed by a
//! space, it is a command instead: `ping 1.1.1.1`, `portscan 10.0.0.1
//! ports=top`, `download https://… parallel=4`. The first word picks the tool,
//! bare words after it fill the field the tool marks as its target, and
//! `key=value` fills any other field by name.

use gpui::{AppContext, Context, Entity, Window};

use crate::core::{FieldKind, Registry, Tool};
use std::sync::Arc;

use super::text_input::TextInput;

pub struct Palette {
    pub input: Entity<TextInput>,
    /// Which row the keyboard is on, as a position in the filtered list.
    pub cursor: usize,
    revision: usize,
    query: String,
}

impl Palette {
    pub fn new(cx: &mut Context<Self>) -> Palette {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "Search, or run a command");
            input.mono = false;
            input
        });
        Palette {
            input,
            cursor: 0,
            revision: 0,
            query: String::new(),
        }
    }

    /// Clears the box and puts the caret in it, which is what opening should
    /// always do — a palette that remembers your last search is a palette you
    /// have to clear first.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor = 0;
        self.query.clear();
        self.input.update(cx, |input, cx| {
            input.set_value("", cx);
        });
        let handle = self.input.read(cx).focus_handle.clone();
        window.focus(&handle);
    }

    /// Picks up what has been typed. Returns true when it changed, so the
    /// caller knows to repaint.
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

    /// What has been typed, as a command: the first word, and the rest.
    pub fn command(&self) -> Command<'_> {
        Command::parse(self.query.trim())
    }

    /// The text in the box, as typed.
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn move_cursor(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor as isize + delta).rem_euclid(len as isize) as usize;
    }

}



/// The token the caret is in: the last word, or nothing when the query ends in
/// a space and a new word is about to start.
fn tail(query: &str) -> &str {
    if query.ends_with(char::is_whitespace) {
        return "";
    }
    query.split_whitespace().next_back().unwrap_or("")
}

/// The tool the query names, once the first word has been finished — either by
/// a space after it or by naming a tool exactly.
pub fn tool_of(query: &str, registry: &Registry) -> Option<Arc<dyn Tool>> {
    tool_settled(query, registry)
}

fn tool_settled(query: &str, registry: &Registry) -> Option<Arc<dyn Tool>> {
    let mut words = query.split_whitespace();
    let first = words.next()?;
    let settled = words.next().is_some() || query.ends_with(char::is_whitespace);
    settled.then(|| registry.get(&first.to_lowercase())).flatten()
}

/// What to offer for what has been typed so far.
///
/// Three things, in the order a command is written: which tool, which of its
/// settings, and — once a setting is named — which values it takes.
pub fn suggest(query: &str, registry: &Registry, hits: Vec<super::search::Hit>) -> Suggest {
    let Some(tool) = tool_settled(query, registry) else {
        return Suggest::Search(hits);
    };

    let tail = tail(query);
    let fields = tool.fields();

        // `key=` — the values that setting accepts.
        if let Some((key, typed)) = tail.split_once('=')
            && let Some(field) = fields.iter().find(|f| f.key.eq_ignore_ascii_case(key))
        {
            let typed = typed.to_lowercase();
            let values: Vec<Value> = field
                .options
                .iter()
                .filter(|o| {
                    o.value.to_lowercase().starts_with(&typed)
                        || o.label.to_lowercase().contains(&typed)
                })
                .map(|o| Value {
                    value: o.value.clone(),
                    label: o.label.clone(),
                    desc: o.desc.clone(),
                })
                .collect();
            if !values.is_empty() {
                return Suggest::Values { key: field.key, items: values };
            }
            return Suggest::Values { key: field.key, items: Vec::new() };
        }

        // Otherwise: the settings this tool has, minus the ones already given.
        let already: Vec<String> =
            Command::parse(query.trim()).named.iter().map(|(k, _)| k.to_lowercase()).collect();
    let needle = tail.to_lowercase();
    // What you typed is the start of a key before it is anything else:
    // "inter" means `interval`, not the `iface` field whose label happens to
    // read "Interface".
    let mut scored: Vec<(u8, usize, Param)> = fields
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            !already.contains(&f.key.to_lowercase()) || f.key.eq_ignore_ascii_case(tail)
        })
        .filter_map(|(i, f)| {
            let (key, label) = (f.key.to_lowercase(), f.label.to_lowercase());
            let rank = if needle.is_empty() || key.starts_with(&needle) {
                0
            } else if label.starts_with(&needle) {
                1
            } else if key.contains(&needle) || label.contains(&needle) {
                2
            } else {
                return None;
            };
            Some((
                rank,
                i,
                Param {
                    key: f.key,
                    label: f.label,
                    help: f.help,
                    default: f.default.clone(),
                    values: f.options.iter().map(|o| o.value.clone()).collect(),
                    free: f.kind == FieldKind::Text,
                },
            ))
        })
        .collect();
    scored.sort_by_key(|(rank, i, _)| (*rank, *i));
    let items = scored.into_iter().map(|(_, _, p)| p).collect();
    Suggest::Params { tool, items }
}

/// What pressing tab would leave in the box, if anything.
pub fn complete(
    query: &str,
    cursor: usize,
    registry: &Registry,
    hits: Vec<super::search::Hit>,
) -> Option<String> {
    let head = &query[..query.len() - tail(query).len()];
    match suggest(query, registry, hits) {
        Suggest::Search(hits) => match hits.into_iter().nth(cursor)? {
            // Completing a tool writes its name and a space, ready for its
            // settings. Everything else is somewhere to go, not something to
            // type, so tab leaves it alone.
            super::search::Hit::Tool(tool) => Some(format!("{} ", tool.id())),
            _ => None,
        },
        Suggest::Params { items, .. } => {
            let param = items.into_iter().nth(cursor)?;
            Some(format!("{head}{}=", param.key))
        }
        Suggest::Values { key, items } => {
            let value = items.into_iter().nth(cursor)?;
            Some(format!("{head}{key}={} ", value.value))
        }
    }
}

/// What the palette is offering for what has been typed.
pub enum Suggest {
    /// Anything matching what has been typed, since nothing has settled into a
    /// command yet. Built by [`super::search`], which sees the workspaces.
    Search(Vec<super::search::Hit>),
    /// Which of the chosen tool's settings to name next.
    Params { tool: Arc<dyn Tool>, items: Vec<Param> },
    /// Which value a named setting takes.
    Values { key: &'static str, items: Vec<Value> },
}

impl Suggest {
    /// How many rows there are to walk.
    pub fn len(&self) -> usize {
        match self {
            Suggest::Search(hits) => hits.len(),
            Suggest::Params { items, .. } => items.len(),
            Suggest::Values { items, .. } => items.len(),
        }
    }

}

/// One setting a tool has, as the palette describes it.
#[derive(Clone, Debug)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub default: String,
    /// The values it accepts, for a setting that has a fixed set.
    pub values: Vec<String>,
    /// Whether anything can be typed into it.
    pub free: bool,
}

/// One value a setting accepts.
#[derive(Clone, Debug)]
pub struct Value {
    pub value: String,
    pub label: String,
    pub desc: String,
}

/// A line typed at the palette, split into the tool and its arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command<'a> {
    /// The first word: which tool.
    pub name: &'a str,
    /// The bare words after it, which fill the target field in order.
    pub bare: Vec<&'a str>,
    /// `key=value` pairs, which fill fields by name.
    pub named: Vec<(&'a str, &'a str)>,
    /// A setting named but not yet given a value.
    pub pending: Option<&'a str>,
}

impl<'a> Command<'a> {
    pub fn parse(line: &'a str) -> Command<'a> {
        let mut words = line.split_whitespace();
        let name = words.next().unwrap_or("");
        let mut command = Command { name, bare: Vec::new(), named: Vec::new(), pending: None };
        for word in words {
            match word.split_once('=') {
                // A bare `=` on either side is not a pair; a URL with a query
                // string is full of them and is a bare word.
                Some((key, value)) if is_key(key) && !value.is_empty() => {
                    command.named.push((key, value))
                }
                // `key=` with nothing after it is a setting halfway typed. It
                // is not a target, and running on it would put the word
                // "method=" in the host box.
                Some((key, "")) if is_key(key) => command.pending = Some(key),
                _ => command.bare.push(word),
            }
        }
        command
    }

    /// Whether anything followed the tool's name that is finished enough to
    /// act on. A setting halfway typed is not.
    pub fn has_arguments(&self) -> bool {
        !self.bare.is_empty() || !self.named.is_empty()
    }

    /// The bare words joined back up, which is what a target field wants: one
    /// host, or a list of links.
    pub fn target(&self) -> String {
        self.bare.join(" ")
    }
}

/// A field key is a bare identifier. Anything else — a host, a URL, a port
/// range — is a value that happens to contain an equals sign.
fn is_key(word: &str) -> bool {
    !word.is_empty()
        && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && word.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::{Command, Suggest};
    use crate::core::Registry;

    fn registry() -> Registry {
        crate::tools::all()
    }

    /// The search results a query would produce with nothing open, which is
    /// all these tests need: they are about the command line, not the search.
    fn hits(query: &str, registry: &Registry) -> Vec<crate::ui::search::Hit> {
        crate::ui::search::search(query, registry, &[], 0, &[])
    }

    fn suggest(query: &str, registry: &Registry) -> Suggest {
        super::suggest(query, registry, hits(query, registry))
    }

    fn complete(query: &str, cursor: usize, registry: &Registry) -> Option<String> {
        super::complete(query, cursor, registry, hits(query, registry))
    }

    #[test]
    fn the_first_word_is_completed_to_a_tool_and_a_space() {
        let r = registry();
        assert_eq!(complete("por", 0, &r).as_deref(), Some("portscan "));
        // With a space after it, the tool has settled and the settings come
        // next — so tab no longer rewrites the name.
        assert!(complete("portscan ", 0, &r).is_some_and(|c| c.starts_with("portscan ")));
    }

    #[test]
    fn a_settled_tool_offers_its_settings() {
        let r = registry();
        let Suggest::Params { tool, items } = suggest("ping ", &r) else {
            panic!("expected the settings of a settled tool");
        };
        assert_eq!(tool.id(), "ping");
        assert!(items.iter().any(|p| p.key == "count"));
        assert!(items.iter().any(|p| p.key == "method" && !p.values.is_empty()));
    }

    #[test]
    fn a_half_typed_setting_narrows_and_completes_to_an_equals() {
        let r = registry();
        let Suggest::Params { items, .. } = suggest("ping cou", &r) else {
            panic!("expected settings");
        };
        assert_eq!(items[0].key, "count", "the key you started typing comes first");
        assert_eq!(complete("ping cou", 0, &r).as_deref(), Some("ping count="));
    }

    #[test]
    fn a_named_setting_offers_the_values_it_takes() {
        let r = registry();
        let Suggest::Values { key, items } = suggest("ping method=", &r) else {
            panic!("expected values");
        };
        assert_eq!(key, "method");
        assert!(items.len() >= 2, "ping speaks more than one protocol");
        // Completing a value leaves a space, ready for the next setting.
        let filled = complete("ping method=", 0, &r).expect("a completion");
        assert!(filled.starts_with("ping method="));
        assert!(filled.ends_with(' '));
    }

    #[test]
    fn a_setting_already_given_is_not_offered_again() {
        let r = registry();
        let Suggest::Params { items, .. } = suggest("ping count=5 ", &r) else {
            panic!("expected settings");
        };
        assert!(!items.iter().any(|p| p.key == "count"));
    }

    #[test]
    fn completing_keeps_everything_already_typed() {
        let r = registry();
        // The target, and an earlier setting, both survive.
        assert_eq!(
            complete("ping 1.1.1.1 count=5 inter", 0, &r).as_deref(),
            Some("ping 1.1.1.1 count=5 interval=")
        );
    }

    #[test]
    fn a_bare_tool_name_is_just_a_search() {
        let command = Command::parse("ping");
        assert_eq!(command.name, "ping");
        assert!(!command.has_arguments());
    }

    #[test]
    fn a_target_follows_the_tool() {
        let command = Command::parse("ping 1.1.1.1");
        assert_eq!(command.name, "ping");
        assert_eq!(command.target(), "1.1.1.1");
        assert!(command.has_arguments());
    }

    #[test]
    fn settings_are_named_and_the_rest_is_the_target() {
        let command = Command::parse("portscan 10.0.0.1 ports=top timeout=500ms");
        assert_eq!(command.name, "portscan");
        assert_eq!(command.target(), "10.0.0.1");
        assert_eq!(command.named, vec![("ports", "top"), ("timeout", "500ms")]);
    }

    #[test]
    fn several_bare_words_are_one_target() {
        // Which is what a downloader wants: a list of links.
        let command = Command::parse("download https://a.example/1 https://b.example/2");
        assert_eq!(command.target(), "https://a.example/1 https://b.example/2");
    }

    #[test]
    fn a_setting_halfway_typed_is_not_a_target() {
        // Otherwise `ping method=` would ping a host called "method=".
        let command = Command::parse("ping method=");
        assert_eq!(command.pending, Some("method"));
        assert_eq!(command.target(), "");
        assert!(!command.has_arguments());
    }

    #[test]
    fn a_url_with_a_query_string_is_not_a_setting() {
        // `v=abc` inside a URL is part of the link, not a field named `v`.
        let command = Command::parse("download https://x.example/watch?v=abc&t=3");
        assert!(command.named.is_empty());
        assert_eq!(command.target(), "https://x.example/watch?v=abc&t=3");
    }

    #[test]
    fn the_search_only_ever_uses_the_first_word() {
        // Otherwise typing an address would filter every tool away.
        assert_eq!(Command::parse("ping 1.1.1.1").name, "ping");
        assert_eq!(Command::parse("   ").name, "");
    }
}
