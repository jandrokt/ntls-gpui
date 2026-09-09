//! Colouring the source of a document or a workflow.
//!
//! A tokeniser per language, run over one line at a time and producing spans a
//! line of text can be shaped from. It is deliberately shallow: enough to see
//! the shape of what you are writing, not a parser. The parsers that
//! matter already exist: [`crate::doc::render`] reads the markdown and
//! [`crate::flow`] reads the workflow.

use std::ops::Range;

/// What is being edited.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Language {
    Markdown,
    Flow,
    /// A bare expression, with no prose or `{{ }}` around it. What a
    /// variable's formula is.
    Expr,
    /// JSON: what a service answers with, and what gets typed into a request
    /// body.
    Json,
    /// XML, and near enough to HTML to read one.
    Xml,
    /// Anything else. Nothing is coloured, and naming it at all is what lets
    /// the editor and the response view still count lines and still use a
    /// monospaced face for something they cannot read.
    Text,
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
    /// The name of a thing rather than the thing: a field of an object, an
    /// attribute of a tag. Drawn like a name and weighted like a heading,
    /// because in a page of JSON the names are the structure and the values
    /// are the content.
    Key,
}

/// What one line's colouring needs to know about the lines before it.
///
/// Only two things carry: a fenced code block in markdown, and a comment in
/// XML that was not closed. Everything else either lives inside one line or
/// reads the same whether or not it was finished.
#[derive(Default)]
struct Carried {
    in_fence: bool,
    in_comment: bool,
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
pub const FLOW_WORDS: [&str; 13] = [
    "run", "with", "stop", "if", "else", "repeat", "for", "each", "while", "wait", "set",
    "print", "log",
];

/// The words an expression is written with.
const EXPR_WORDS: [&str; 10] = [
    "if", "then", "else", "true", "false", "nothing", "and", "or", "not", "in",
];

/// One line, in whichever language, given whatever the lines before it left
/// open.
fn line_spans(language: Language, line: &str, state: &mut Carried) -> Vec<Span> {
    match language {
        Language::Flow => flow_line(line),
        Language::Expr => inside(line, 0),
        Language::Json => json_line(line),
        Language::Xml => xml_line(line, &mut state.in_comment),
        Language::Text => Vec::new(),
        Language::Markdown => {
            if line.trim_start().starts_with("```") {
                state.in_fence = !state.in_fence;
                vec![span(0..line.len(), Kind::Code)]
            } else if state.in_fence {
                with_expressions(line, Kind::Code)
            } else {
                markdown_line(line)
            }
        }
    }
}

/// Colours a whole document as byte ranges into the whole of it.
///
/// [`highlight`] answers one list per line, relative to that line, which is
/// what an editor drawing a line at a time wants. Something drawn in one
/// piece, like an answer in the response pane, wants the offsets to be into
/// the text it was handed.
pub fn highlight_flat(language: Language, source: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut state = Carried::default();
    let mut at = 0usize;

    // `split` and not `lines`, because these offsets have to land on the text
    // exactly as it was given: `lines` drops a carriage return and the last
    // newline, and every span past the first of those would be adrift.
    for line in source.split('\n') {
        for run in cover(line, line_spans(language, line, &mut state)) {
            out.push(span(at + run.range.start..at + run.range.end, run.kind));
        }
        at += line.len() + 1;
    }
    out
}

/// Colours a whole document, one list of spans per line.
///
/// The spans of a line are line-relative, in order, and **cover it exactly**:
/// the gaps between the interesting parts come back as [`Kind::Text`], so the
/// renderer shapes a line by walking its spans once and never has to work out
/// what it missed.
pub fn highlight(language: Language, source: &str) -> Vec<Vec<Span>> {
    let mut out = Vec::new();
    let mut state = Carried::default();

    for line in source.lines() {
        out.push(cover(line, line_spans(language, line, &mut state)));
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

// --- JSON -------------------------------------------------------------------

/// One line of JSON.
///
/// A line is a complete unit here, and that is not an approximation: JSON
/// forbids a raw newline inside a string, so nothing can be left open at the
/// end of one.
fn json_line(line: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut at = 0usize;

    while at < line.len() {
        let rest = &line[at..];
        let ch = rest.chars().next().unwrap_or(' ');

        if ch.is_whitespace() {
            at += ch.len_utf8();
            continue;
        }
        if ch == '"' {
            let end = at + quoted_end(rest, '"');
            // A string with a colon after it is naming the thing that
            // follows, not being it.
            let kind =
                if line[end..].trim_start().starts_with(':') { Kind::Key } else { Kind::Str };
            spans.push(span(at..end, kind));
            at = end;
            continue;
        }
        if ch == '-' || ch.is_ascii_digit() {
            let end = at
                + rest
                    .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')))
                    .unwrap_or(rest.len());
            spans.push(span(at..end, Kind::Number));
            at = end;
            continue;
        }
        if ch.is_ascii_alphabetic() {
            let end = at + rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
            // `true`, `false` and `null` are the only bare words JSON has. A
            // word that is not one of them is a mistake, and colouring it as
            // a name is how it stands out from the words that are.
            let kind = match &line[at..end] {
                "true" | "false" | "null" => Kind::Keyword,
                _ => Kind::Name,
            };
            spans.push(span(at..end, kind));
            at = end;
            continue;
        }
        if matches!(ch, '{' | '}' | '[' | ']' | ':' | ',') {
            spans.push(span(at..at + 1, Kind::Delim));
            at += 1;
            continue;
        }
        at += ch.len_utf8();
    }
    spans
}

/// How long the quoted run at the front of `rest` is, both quotes included.
///
/// One that is never closed runs to the end of the line, which is what it is
/// while it is still being typed.
fn quoted_end(rest: &str, quote: char) -> usize {
    let mut escaped = false;
    for (at, ch) in rest.char_indices().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            c if c == quote => return at + c.len_utf8(),
            _ => {}
        }
    }
    rest.len()
}

// --- XML --------------------------------------------------------------------

/// One line of XML, given whether the line before it left a comment open.
///
/// Only comments are carried between lines. Everything else XML has is either
/// inside one tag or is the text between two, and both of those read the same
/// whether or not they are finished.
fn xml_line(line: &str, in_comment: &mut bool) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut at = 0usize;

    while at < line.len() {
        if *in_comment {
            match line[at..].find("-->") {
                Some(end) => {
                    spans.push(span(at..at + end + 3, Kind::Comment));
                    at += end + 3;
                    *in_comment = false;
                }
                None => {
                    spans.push(span(at..line.len(), Kind::Comment));
                    at = line.len();
                }
            }
            continue;
        }

        // Whatever sits before the next tag is the text of the document, and
        // `cover` fills it in.
        let Some(open) = line[at..].find('<') else { break };
        let open = at + open;

        if line[open..].starts_with("<!--") {
            *in_comment = true;
            at = open;
            continue;
        }
        let close = line[open..].find('>').map_or(line.len(), |e| open + e + 1);
        spans.extend(xml_tag(&line[open..close], open));
        at = close;
    }
    spans
}

/// One tag, from its opening bracket to its closing one.
///
/// `offset` is where the tag starts in its line, since the spans belong to the
/// line and not to the tag.
fn xml_tag(tag: &str, offset: usize) -> Vec<Span> {
    let mut spans = Vec::new();

    // The bracket, with whatever punctuation follows it: the `/` of a closing
    // tag, the `?` of a declaration, the `!` of a doctype.
    let name_at = tag
        .char_indices()
        .skip(1)
        .find(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == ':')
        .map_or(tag.len(), |(i, _)| i);
    spans.push(span(offset..offset + name_at, Kind::Delim));

    let mut at = name_at;
    let end_of_name =
        at + tag[at..].find(|c: char| !is_name_char(c)).unwrap_or(tag.len() - at);
    if end_of_name > at {
        spans.push(span(offset + at..offset + end_of_name, Kind::Name));
        at = end_of_name;
    }

    while at < tag.len() {
        let rest = &tag[at..];
        let ch = rest.chars().next().unwrap_or(' ');

        if ch.is_whitespace() {
            at += ch.len_utf8();
            continue;
        }
        if ch == '"' || ch == '\'' {
            let end = at + quoted_end(rest, ch);
            spans.push(span(offset + at..offset + end, Kind::Str));
            at = end;
            continue;
        }
        if is_name_char(ch) {
            let end = at + rest.find(|c: char| !is_name_char(c)).unwrap_or(rest.len());
            spans.push(span(offset + at..offset + end, Kind::Key));
            at = end;
            continue;
        }
        // `=`, and the `/` and `>` that close it.
        spans.push(span(offset + at..offset + at + ch.len_utf8(), Kind::Delim));
        at += ch.len_utf8();
    }
    spans
}

/// Whether a character can appear in a tag or attribute name.
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '-' | '.')
}

/// The inside of a `{{ … }}`, so that a name reads differently from a call.
fn expression(text: &str, offset: usize) -> Vec<Span> {
    let open = 2.min(text.len());
    let closed = text.len() >= 4 && text.ends_with("}}");
    let inner_end = if closed { text.len() - 2 } else { text.len() };

    let mut spans = vec![span(offset..offset + open, Kind::Delim)];
    spans.extend(inside(&text[open..inner_end], offset + open));
    if closed {
        spans.push(span(offset + inner_end..offset + text.len(), Kind::Delim));
    }
    spans
}

/// The inside of an expression: names, calls, keywords and literals.
///
/// `offset` is where `inner` starts in the line the spans are for, so the same
/// tokeniser serves an expression buried in a paragraph and one that is the
/// whole of what is being typed.
fn inside(inner: &str, offset: usize) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut at = 0usize;

    while at < inner.len() {
        let rest = &inner[at..];
        let ch = rest.chars().next().unwrap_or(' ');
        let here = offset + at;

        if ch.is_whitespace() {
            at += ch.len_utf8();
            continue;
        }
        if ch == '"' || ch == '\'' {
            let end = rest[1..].find(ch).map(|e| at + 1 + e + 1).unwrap_or(inner.len());
            spans.push(span(here..offset + end, Kind::Str));
            at = end;
            continue;
        }
        if ch.is_ascii_digit() {
            let end =
                at + rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
            spans.push(span(here..offset + end, Kind::Number));
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
            spans.push(span(here..offset + end, kind));
            at = end;
            continue;
        }
        at += ch.len_utf8();
    }
    spans
}

/// Colours a formula, which is an expression and nothing else.
///
/// The spans cover the whole of it, the way [`highlight`] covers a line, so
/// the caller can shape it in one pass.
pub fn expr_line(source: &str) -> Vec<Span> {
    cover(source, inside(source, 0))
}

/// A heading, or anything else that is one colour, with the expressions in
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

    /// The coloured runs of a line, ignoring the plain text between them.
    /// That is what these tests check.
    fn marked<'a>(line: &'a str, spans: &'a [Span]) -> Vec<(Kind, &'a str)> {
        spans
            .iter()
            .filter(|s| s.kind != Kind::Text)
            .map(|s| (s.kind, &line[s.range.clone()]))
            .collect()
    }

    #[test]
    fn a_formula_is_coloured_without_any_braces_around_it() {
        let line = "round(Router.rtt.avg) + 1";
        let spans = expr_line(line);
        assert_eq!(
            marked(line, &spans),
            vec![
                (Kind::Function, "round"),
                (Kind::Name, "Router"),
                (Kind::Name, "rtt"),
                (Kind::Function, "avg"),
                (Kind::Number, "1"),
            ]
        );
        // Covered end to end, so a caller can shape it in one pass.
        let mut at = 0;
        for span in &spans {
            assert_eq!(span.range.start, at);
            at = span.range.end;
        }
        assert_eq!(at, line.len());
    }

    #[test]
    fn a_formula_keeps_its_keywords_and_its_strings() {
        let line = "if Router.ok then \"up\" else \"down\"";
        assert_eq!(
            marked(line, &expr_line(line)),
            vec![
                (Kind::Keyword, "if"),
                (Kind::Name, "Router"),
                (Kind::Name, "ok"),
                (Kind::Keyword, "then"),
                (Kind::Str, "\"up\""),
                (Kind::Keyword, "else"),
                (Kind::Str, "\"down\""),
            ]
        );
    }

    #[test]
    fn flat_offsets_land_on_the_text_they_were_given() {
        // The offsets are into the whole document, and every span after the
        // first newline depends on that arithmetic being right.
        let source = "{\n  \"a\": 1\n}";
        for run in highlight_flat(Language::Json, source) {
            // Every span has to be a real slice of the source, and the whole
            // has to be covered end to end with nothing overlapping.
            assert!(source.get(run.range.clone()).is_some(), "{:?}", run.range);
        }
        let flat = highlight_flat(Language::Json, source);
        // In order, never overlapping, and what falls between two of them is
        // only ever the newline that separates their lines: a span covers a
        // line and nothing covers the break after it.
        let mut at = 0;
        let mut rebuilt = String::new();
        for run in &flat {
            assert!(run.range.start >= at, "out of order at {at}");
            rebuilt.push_str(&source[at..run.range.start]);
            rebuilt.push_str(&source[run.range.clone()]);
            at = run.range.end;
        }
        rebuilt.push_str(&source[at..]);
        assert_eq!(rebuilt, source, "the spans and the gaps have to be the whole of it");

        // The name on the second line is found where it actually is.
        let name = flat
            .iter()
            .find(|r| r.kind == Kind::Key)
            .map(|r| &source[r.range.clone()])
            .expect("a name");
        assert_eq!(name, "\"a\"");
    }

    #[test]
    fn a_carriage_return_does_not_shift_every_later_span() {
        // `lines` drops the carriage return, so counting offsets from it
        // would put every span after the first line one byte early.
        let source = "{\r\n  \"a\": 1\r\n}";
        for run in highlight_flat(Language::Json, source) {
            assert!(source.get(run.range.clone()).is_some(), "{:?}", run.range);
        }
        let found: Vec<&str> = highlight_flat(Language::Json, source)
            .iter()
            .filter(|r| r.kind == Kind::Key)
            .map(|r| &source[r.range.clone()])
            .collect();
        assert_eq!(found, vec!["\"a\""]);
    }

    #[test]
    fn json_reads_as_names_and_values_and_not_as_one_colour() {
        let line = r#"  {"name": "ok", "count": 12, "live": true, "spare": null}"#;
        assert_eq!(
            marked(line, &highlight(Language::Json, line)[0]),
            vec![
                (Kind::Delim, "{"),
                (Kind::Key, "\"name\""),
                (Kind::Delim, ":"),
                (Kind::Str, "\"ok\""),
                (Kind::Delim, ","),
                (Kind::Key, "\"count\""),
                (Kind::Delim, ":"),
                (Kind::Number, "12"),
                (Kind::Delim, ","),
                (Kind::Key, "\"live\""),
                (Kind::Delim, ":"),
                (Kind::Keyword, "true"),
                (Kind::Delim, ","),
                (Kind::Key, "\"spare\""),
                (Kind::Delim, ":"),
                (Kind::Keyword, "null"),
                (Kind::Delim, "}"),
            ]
        );
    }

    #[test]
    fn a_quote_inside_a_json_string_does_not_end_it() {
        let line = r#"{"say": "a \" and more", "n": 1}"#;
        let spans = highlight(Language::Json, line);
        let strings: Vec<&str> =
            marked(line, &spans[0]).into_iter().filter(|(k, _)| *k == Kind::Str).map(|(_, s)| s).collect();
        assert_eq!(strings, vec![r#""a \" and more""#]);
        // And the name after it is still read as a name.
        assert!(marked(line, &spans[0]).contains(&(Kind::Key, "\"n\"")));
    }

    #[test]
    fn xml_picks_out_the_tags_the_attributes_and_their_values() {
        let line = r#"<item id="7" name="a">text</item>"#;
        assert_eq!(
            marked(line, &highlight(Language::Xml, line)[0]),
            vec![
                (Kind::Delim, "<"),
                (Kind::Name, "item"),
                (Kind::Key, "id"),
                (Kind::Delim, "="),
                (Kind::Str, "\"7\""),
                (Kind::Key, "name"),
                (Kind::Delim, "="),
                (Kind::Str, "\"a\""),
                (Kind::Delim, ">"),
                (Kind::Delim, "</"),
                (Kind::Name, "item"),
                (Kind::Delim, ">"),
            ]
        );
        // The text between the tags is the text of the document.
        let spans = &highlight(Language::Xml, line)[0];
        assert!(spans.iter().any(|s| s.kind == Kind::Text && &line[s.range.clone()] == "text"));
    }

    #[test]
    fn an_xml_comment_runs_on_until_it_is_closed() {
        let source = "<a>\n<!-- one\ntwo -->\n<b/>";
        let spans = highlight(Language::Xml, source);
        let line = |n: usize| {
            let text = source.lines().nth(n).unwrap();
            marked(text, &spans[n])
        };
        // The middle two lines are comment from end to end.
        assert!(line(1).iter().all(|(k, _)| *k == Kind::Comment), "{:?}", line(1));
        assert!(line(2).iter().all(|(k, _)| *k == Kind::Comment), "{:?}", line(2));
        // And the tag after it is a tag again.
        assert!(line(3).contains(&(Kind::Name, "b")));
    }

    #[test]
    fn plain_text_is_left_alone_but_still_covered() {
        let source = "just words\nand more";
        let spans = highlight(Language::Text, source);
        assert_eq!(spans.len(), 2);
        // Covered end to end in one run, so the renderer still shapes it in
        // one pass.
        assert_eq!(spans[0], vec![Span { range: 0..10, kind: Kind::Text }]);
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
