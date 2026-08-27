//! Colouring the source of a document or a workflow.
//!
//! A tokeniser per language, run over one line at a time and producing spans a
//! line of text can be shaped from. It is deliberately shallow — enough to see
//! the shape of what you are writing, not a parser — because the parsers that
//! matter already exist: [`crate::doc::render`] reads the markdown and
//! [`crate::flow`] reads the workflow.

use std::ops::Range;

/// What is being edited.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Language {
    Markdown,
    Flow,
}

/// What a run of characters is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    /// A heading, and the `#` marks that make it one.
    Heading,
    Strong,
    Emphasis,
    /// Inline code, and fenced code.
    Code,
    Link,
    /// A bullet, a number, a quote mark, a rule.
    Marker,
    /// The `{{` and `}}` around an expression.
    Delim,
    /// Inside an expression: a name, a call, a literal.
    Name,
    Function,
    Keyword,
    Str,
    Number,
    Comment,
}

/// One coloured run, as a byte range within its line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: Kind,
}

fn span(range: Range<usize>, kind: Kind) -> Span {
    Span { range, kind }
}

/// The functions the expression language offers, which are worth colouring
/// apart from the names of runs.
pub const FUNCTIONS: [&str; 30] = [
    "avg", "mean", "min", "max", "sum", "count", "median", "p95", "percentile", "round", "floor",
    "ceil", "abs", "fixed", "percent", "first", "last", "join", "sort", "text", "upper", "lower",
    "number", "exists", "tool", "tools", "len", "length", "contains", "col",
];

/// The words a workflow is written with.
pub const FLOW_WORDS: [&str; 6] = ["run", "stop", "if", "else", "repeat", "wait"];

/// The words an expression is written with.
const EXPR_WORDS: [&str; 10] = [
    "if", "then", "else", "true", "false", "nothing", "and", "or", "not", "in",
];

/// Colours a whole document, one list of spans per line.
///
/// The spans of a line are line-relative, in order, and **cover it exactly** —
/// the gaps between the interesting parts come back as [`Kind::Text`] — so the
/// renderer shapes a line by walking its spans once and never has to work out
/// what it missed.
pub fn highlight(language: Language, source: &str) -> Vec<Vec<Span>> {
    let mut out = Vec::new();
    let mut in_fence = false;

    for line in source.lines() {
        let spans = match language {
            Language::Flow => flow_line(line),
            Language::Markdown => {
                let is_fence = line.trim_start().starts_with("```");
                if is_fence {
                    in_fence = !in_fence;
                    vec![span(0..line.len(), Kind::Code)]
                } else if in_fence {
                    with_expressions(line, Kind::Code)
                } else {
                    markdown_line(line)
                }
            }
        };
        out.push(cover(line, spans));
    }
    out
}

/// Fills the gaps between spans with plain text, so the line is covered end to
/// end.
fn cover(line: &str, spans: Vec<Span>) -> Vec<Span> {
    let mut out = Vec::with_capacity(spans.len() * 2 + 1);
    let mut at = 0usize;
    for s in spans {
        // A tokeniser that overlapped itself would silently drop text; taking
        // the later span and skipping the overlap keeps the line intact.
        if s.range.start > at {
            out.push(span(at..s.range.start, Kind::Text));
        }
        if s.range.end > at {
            let start = s.range.start.max(at);
            out.push(span(start..s.range.end, s.kind));
            at = s.range.end;
        }
    }
    if at < line.len() {
        out.push(span(at..line.len(), Kind::Text));
    }
    out
}

/// One line of a workflow.
fn flow_line(line: &str) -> Vec<Span> {
    let mut spans = Vec::new();

    // A comment runs to the end, and everything before it is still code.
    let (code, comment) = match line.find('#') {
        Some(at) => (&line[..at], Some(at..line.len())),
        None => (line, None),
    };

    let mut at = 0usize;
    while at < code.len() {
        let rest = &code[at..];
        let ch = rest.chars().next().unwrap_or(' ');

        if ch.is_whitespace() {
            at += ch.len_utf8();
            continue;
        }
        if ch == '"' || ch == '\'' {
            let end = rest[1..].find(ch).map(|e| at + 1 + e + 1).unwrap_or(code.len());
            spans.push(span(at..end, Kind::Str));
            at = end;
            continue;
        }
        if ch.is_ascii_digit() {
            let end = at + rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
            spans.push(span(at..end, Kind::Number));
            at = end;
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let end = at
                + rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
            let word = &code[at..end];
            let called = code[end..].trim_start().starts_with('(');
            let kind = if FLOW_WORDS.iter().any(|w| w.eq_ignore_ascii_case(word))
                || EXPR_WORDS.contains(&word)
            {
                Kind::Keyword
            } else if called || FUNCTIONS.contains(&word) {
                Kind::Function
            } else {
                Kind::Name
            };
            spans.push(span(at..end, kind));
            at = end;
            continue;
        }
        at += ch.len_utf8();
    }

    spans.extend(comment.map(|range| span(range, Kind::Comment)));
    spans
}

/// One line of markdown, outside a fence.
fn markdown_line(line: &str) -> Vec<Span> {
    let bare = line.trim_start();
    let indent = line.len() - bare.len();

    // A heading is the whole line, and so is a rule.
    if let Some(hashes) = bare.split_whitespace().next()
        && hashes.chars().all(|c| c == '#')
        && (1..=6).contains(&hashes.len())
    {
        return with_expressions(line, Kind::Heading);
    }
    if bare.len() >= 3 && ['-', '*', '_'].iter().any(|m| bare.chars().all(|c| c == *m)) {
        return vec![span(0..line.len(), Kind::Marker)];
    }

    let mut spans = Vec::new();
    let mut body = indent;

    // The marker at the start of a list item or a quote.
    for marker in ["- ", "* ", "+ ", "> "] {
        if bare.starts_with(marker) {
            spans.push(span(indent..indent + marker.len(), Kind::Marker));
            body = indent + marker.len();
        }
    }
    if body == indent {
        let digits: usize = bare.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && (bare[digits..].starts_with(". ") || bare[digits..].starts_with(") ")) {
            spans.push(span(indent..indent + digits + 2, Kind::Marker));
            body = indent + digits + 2;
        }
    }

    spans.extend(inline(&line[body..], body));
    spans
}

/// Emphasis, code, links and expressions inside a line.
fn inline(text: &str, offset: usize) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut at = 0usize;

    while at < text.len() {
        let rest = &text[at..];

        if rest.starts_with("{{") {
            let end = rest.find("}}").map(|e| at + e + 2).unwrap_or(text.len());
            spans.extend(expression(&text[at..end], offset + at));
            at = end;
            continue;
        }
        if rest.starts_with('`')
            && let Some(e) = rest[1..].find('`')
        {
            let code_content = &text[at..at + 1 + e + 1];
            if code_content.contains("{{") {
                spans.extend(with_expressions(code_content, Kind::Code).into_iter().map(|mut s| {
                    s.range.start += offset + at;
                    s.range.end += offset + at;
                    s
                }));
            } else {
                spans.push(span(offset + at..offset + at + 1 + e + 1, Kind::Code));
            }
            at += 1 + e + 1;
            continue;
        }
        if let Some((mark, kind)) = starts_emphasis(rest)
            && let Some(e) = rest[mark.len()..].find(mark)
            && e > 0
        {
            let end = at + mark.len() * 2 + e;
            spans.push(span(offset + at..offset + end, kind));
            at = end;
            continue;
        }
        if rest.starts_with('[')
            && let Some(close) = rest.find("](")
            && let Some(e) = rest[close..].find(')')
        {
            let end = at + close + e + 1;
            spans.push(span(offset + at..offset + end, Kind::Link));
            at = end;
            continue;
        }

        at += rest.chars().next().map_or(1, char::len_utf8);
    }
    spans
}

fn starts_emphasis(rest: &str) -> Option<(&'static str, Kind)> {
    for (mark, kind) in [("**", Kind::Strong), ("__", Kind::Strong), ("*", Kind::Emphasis), ("_", Kind::Emphasis)] {
        if rest.starts_with(mark) {
            return Some((mark, kind));
        }
    }
    None
}

/// The inside of a `{{ … }}`, so that a name reads differently from a call.
fn expression(text: &str, offset: usize) -> Vec<Span> {
    let mut spans = vec![span(offset..offset + 2.min(text.len()), Kind::Delim)];
    let inner_end = text.len().saturating_sub(if text.ends_with("}}") { 2 } else { 0 });
    let inner = &text[2.min(text.len())..inner_end];

    let mut at = 0usize;
    while at < inner.len() {
        let rest = &inner[at..];
        let ch = rest.chars().next().unwrap_or(' ');
        let here = offset + 2 + at;

        if ch.is_whitespace() {
            at += ch.len_utf8();
            continue;
        }
        if ch == '"' || ch == '\'' {
            let end = rest[1..].find(ch).map(|e| at + 1 + e + 1).unwrap_or(inner.len());
            spans.push(span(here..offset + 2 + end, Kind::Str));
            at = end;
            continue;
        }
        if ch.is_ascii_digit() {
            let end =
                at + rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
            spans.push(span(here..offset + 2 + end, Kind::Number));
            at = end;
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let end = at
                + rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
            let word = &inner[at..end];
            // A name followed by `(` is being called.
            let called = inner[end..].trim_start().starts_with('(');
            let kind = if EXPR_WORDS.contains(&word) {
                Kind::Keyword
            } else if called || FUNCTIONS.contains(&word) {
                Kind::Function
            } else {
                Kind::Name
            };
            spans.push(span(here..offset + 2 + end, kind));
            at = end;
            continue;
        }
        at += ch.len_utf8();
    }

    if text.ends_with("}}") {
        spans.push(span(offset + inner_end..offset + text.len(), Kind::Delim));
    }
    spans
}

/// A heading — or anything else that is one colour — with the expressions in
/// it picked out.
fn with_expressions(line: &str, around: Kind) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut at = 0usize;

    while let Some(open) = line[at..].find("{{") {
        let open = at + open;
        if open > at {
            spans.push(span(at..open, around));
        }
        let end = line[open..].find("}}").map(|e| open + e + 2).unwrap_or(line.len());
        spans.extend(expression(&line[open..end], open));
        at = end;
    }
    if at < line.len() {
        spans.push(span(at..line.len(), around));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(spans: &[Span]) -> Vec<Kind> {
        spans.iter().map(|s| s.kind).collect()
    }

    /// The coloured runs of a line, ignoring the plain text between them,
    /// which is what these tests are about.
    fn marked<'a>(line: &'a str, spans: &'a [Span]) -> Vec<(Kind, &'a str)> {
        spans
            .iter()
            .filter(|s| s.kind != Kind::Text)
            .map(|s| (s.kind, &line[s.range.clone()]))
            .collect()
    }

    #[test]
    fn a_heading_is_one_colour_with_its_expressions_picked_out() {
        let line = "# Link to {{ Router.target }}";
        let spans = &highlight(Language::Markdown, line)[0];
        assert_eq!(spans[0].kind, Kind::Heading);
        assert!(kinds(spans).contains(&Kind::Delim));
        assert!(kinds(spans).contains(&Kind::Name));
    }

    #[test]
    fn emphasis_code_and_links_are_told_apart() {
        let line = "a **bold** `code` [text](url)";
        let spans = &highlight(Language::Markdown, line)[0];
        assert_eq!(
            marked(line, spans),
            vec![
                (Kind::Strong, "**bold**"),
                (Kind::Code, "`code`"),
                (Kind::Link, "[text](url)"),
            ]
        );
    }

    #[test]
    fn a_list_marker_is_not_part_of_the_sentence() {
        let line = "  - a point";
        let spans = &highlight(Language::Markdown, line)[0];
        assert_eq!(marked(line, spans), vec![(Kind::Marker, "- ")]);
    }

    #[test]
    fn a_fence_colours_everything_inside_it() {
        let source = "before\n```rust\n# not a heading\n```\nafter";
        let lines = highlight(Language::Markdown, source);
        assert_eq!(lines[1][0].kind, Kind::Code, "the opening fence");
        assert_eq!(lines[2][0].kind, Kind::Code, "and what is inside it");
        assert_eq!(lines[3][0].kind, Kind::Code, "and the closing fence");
        assert_eq!(lines[4][0].kind, Kind::Text, "but not what follows");
    }

    #[test]
    fn a_call_reads_differently_from_a_name() {
        let line = "{{ avg(Router.rtt) }}";
        let spans = &highlight(Language::Markdown, line)[0];
        let pairs = marked(line, spans);
        assert!(pairs.contains(&(Kind::Function, "avg")));
        assert!(pairs.contains(&(Kind::Name, "Router")));
        assert!(pairs.contains(&(Kind::Delim, "{{")));
        assert!(pairs.contains(&(Kind::Delim, "}}")));
    }

    #[test]
    fn a_workflow_line_is_keywords_names_and_a_comment() {
        let line = "run \"Port scan\" if Sweep.up > 0  # only if anything answered";
        let spans = &highlight(Language::Flow, line)[0];
        let pairs = marked(line, spans);
        assert_eq!(pairs[0], (Kind::Keyword, "run"));
        assert_eq!(pairs[1], (Kind::Str, "\"Port scan\""));
        assert!(pairs.contains(&(Kind::Keyword, "if")));
        assert!(pairs.contains(&(Kind::Name, "Sweep")));
        assert!(pairs.contains(&(Kind::Number, "0")));
        assert_eq!(pairs.last().expect("a comment").0, Kind::Comment);
    }

    #[test]
    fn an_unclosed_expression_still_colours_what_is_there() {
        // A document is edited a character at a time, so half of anything has
        // to be drawable.
        let spans = &highlight(Language::Markdown, "text {{ Router.")[0];
        assert!(kinds(spans).contains(&Kind::Delim));
        assert!(!spans.is_empty());
    }

    #[test]
    fn the_spans_of_a_line_cover_it_exactly() {
        // The renderer walks them once and shapes what it is given, so a gap
        // would drop text off the screen and an overlap would double it.
        let source = "# H {{ a }}\n\nplain text\n- **b** `c` [d](e)\n```\nfenced\n```\nafter";
        let flow = "run \"A\" if x > 1  # note\nstop\n\nwibble";

        for (language, text) in
            [(Language::Markdown, source), (Language::Flow, flow)]
        {
            for (at, spans) in highlight(language, text).iter().enumerate() {
                let line = text.lines().nth(at).expect("the line");
                let mut end = 0;
                for span in spans {
                    assert_eq!(span.range.start, end, "line {at}: a gap or an overlap");
                    end = span.range.end;
                }
                assert_eq!(end, line.len(), "line {at:?} of {language:?} is not covered");
            }
        }
    }

    #[test]
    fn a_line_with_no_markup_is_one_run_of_plain_text() {
        let spans = &highlight(Language::Markdown, "just some words")[0];
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, Kind::Text);
    }

    #[test]
    fn expressions_inside_code_blocks_are_highlighted() {
        let source = "```\nlet x = {{ Router.target }};\n```";
        let lines = highlight(Language::Markdown, source);
        let line = &lines[1];
        assert!(kinds(line).contains(&Kind::Code));
        assert!(kinds(line).contains(&Kind::Delim));
        assert!(kinds(line).contains(&Kind::Name));
    }
}
