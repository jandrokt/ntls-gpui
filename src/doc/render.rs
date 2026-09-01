//! Reading markdown into the blocks a pane can draw.
//!
//! Enough of the language to write a note in: headings, paragraphs, lists,
//! quotes, fenced and inline code, rules, and bold, italic and links inside a
//! line. Not a full implementation: tables and reference links are missing,
//! because what is wanted is a readable note, not a typesetter.

/// One block of a document.
#[derive(Clone, PartialEq, Debug)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Paragraph(Vec<Span>),
    /// A list item, with how deeply it is nested and its marker if numbered.
    Item { depth: usize, marker: Option<String>, spans: Vec<Span> },
    Quote(Vec<Span>),
    Code { language: String, text: String },
    Rule,
}

/// A run of text inside a line.
#[derive(Clone, PartialEq, Debug)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// Where a link points, for the runs that are one.
    pub link: Option<String>,
}

impl Span {
    fn plain(text: &str) -> Span {
        Span { text: text.to_string(), bold: false, italic: false, code: false, link: None }
    }
}

/// Splits a document into blocks.
pub fn blocks(source: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut lines = source.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim_end();

        // A fence runs until it is closed or the document ends.
        if let Some(rest) = trimmed.trim_start().strip_prefix("```") {
            flush(&mut paragraph, &mut out);
            let language = rest.trim().to_string();
            let mut text = String::new();
            for line in lines.by_ref() {
                if line.trim_start().starts_with("```") {
                    break;
                }
                text.push_str(line);
                text.push('\n');
            }
            out.push(Block::Code { language, text: text.trim_end().to_string() });
            continue;
        }

        if trimmed.trim().is_empty() {
            flush(&mut paragraph, &mut out);
            continue;
        }

        let bare = trimmed.trim_start();
        let indent = trimmed.len() - bare.len();

        if is_rule(bare) {
            flush(&mut paragraph, &mut out);
            out.push(Block::Rule);
            continue;
        }

        if let Some(hashes) = bare.split_whitespace().next()
            && hashes.chars().all(|c| c == '#')
            && (1..=6).contains(&hashes.len())
        {
            flush(&mut paragraph, &mut out);
            let level = hashes.len() as u8;
            out.push(Block::Heading(level, spans(bare[hashes.len()..].trim())));
            continue;
        }

        if let Some(rest) = bare.strip_prefix("> ").or_else(|| bare.strip_prefix(">")) {
            flush(&mut paragraph, &mut out);
            out.push(Block::Quote(spans(rest.trim())));
            continue;
        }

        if let Some((marker, rest)) = list_item(bare) {
            flush(&mut paragraph, &mut out);
            out.push(Block::Item { depth: indent / 2, marker, spans: spans(rest) });
            continue;
        }

        paragraph.push(bare.to_string());
    }

    flush(&mut paragraph, &mut out);
    out
}

fn flush(paragraph: &mut Vec<String>, out: &mut Vec<Block>) {
    for line in paragraph.drain(..) {
        out.push(Block::Paragraph(spans(&line)));
    }
}

fn is_rule(line: &str) -> bool {
    let marks = ['-', '*', '_'];
    line.len() >= 3
        && marks.iter().any(|m| line.chars().all(|c| c == *m))
}

/// A bullet or a number, and what follows it.
fn list_item(line: &str) -> Option<(Option<String>, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            return Some((None, rest));
        }
    }
    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &line[digits.len()..];
    let rest = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))?;
    Some((Some(digits), rest))
}

/// Splits a line into its emphasised, code and linked runs.
pub fn spans(line: &str) -> Vec<Span> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<Span> = Vec::new();
    let mut plain = String::new();
    let mut at = 0usize;

    let push = |plain: &mut String, out: &mut Vec<Span>| {
        if !plain.is_empty() {
            out.push(Span::plain(plain));
            plain.clear();
        }
    };

    while at < chars.len() {
        let rest: String = chars[at..].iter().collect();

        if rest.starts_with('`')
            && let Some(end) = rest[1..].find('`')
        {
            push(&mut plain, &mut out);
            out.push(Span { code: true, ..Span::plain(&rest[1..1 + end]) });
            at += 1 + end + 1;
            continue;
        }

        for (mark, bold) in [("**", true), ("__", true), ("*", false), ("_", false)] {
            if rest.starts_with(mark)
                && let Some(end) = rest[mark.len()..].find(mark)
                && end > 0
            {
                push(&mut plain, &mut out);
                let inner = &rest[mark.len()..mark.len() + end];
                out.push(Span { bold, italic: !bold, ..Span::plain(inner) });
                at += mark.len() * 2 + end;
                break;
            }
        }
        if at >= chars.len() {
            break;
        }
        let rest: String = chars[at..].iter().collect();

        // `[text](target)`
        if rest.starts_with('[')
            && let Some(close) = rest.find("](")
            && let Some(end) = rest[close..].find(')')
        {
            push(&mut plain, &mut out);
            let text = &rest[1..close];
            let target = &rest[close + 2..close + end];
            out.push(Span { link: Some(target.to_string()), ..Span::plain(text) });
            at += close + end + 1;
            continue;
        }

        plain.push(chars[at]);
        at += 1;
    }

    push(&mut plain, &mut out);
    if out.is_empty() {
        out.push(Span::plain(""));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.clone()).collect()
    }

    #[test]
    fn headings_carry_their_level() {
        let out = blocks("# One\n\n### Three");
        assert!(matches!(&out[0], Block::Heading(1, s) if text_of(s) == "One"));
        assert!(matches!(&out[1], Block::Heading(3, s) if text_of(s) == "Three"));
    }

    #[test]
    fn lines_become_paragraphs_and_a_blank_line_ends_it() {
        let out = blocks("one\ntwo\n\nthree");
        assert_eq!(out.len(), 3);
        assert!(matches!(&out[0], Block::Paragraph(s) if text_of(s) == "one"));
        assert!(matches!(&out[1], Block::Paragraph(s) if text_of(s) == "two"));
        assert!(matches!(&out[2], Block::Paragraph(s) if text_of(s) == "three"));
    }

    #[test]
    fn lists_keep_their_marker_and_their_depth() {
        let out = blocks("- one\n  - nested\n1. first");
        assert!(matches!(&out[0], Block::Item { depth: 0, marker: None, .. }));
        assert!(matches!(&out[1], Block::Item { depth: 1, .. }));
        assert!(matches!(&out[2], Block::Item { marker: Some(m), .. } if m == "1"));
    }

    #[test]
    fn a_fence_is_kept_verbatim() {
        let out = blocks("```rust\nlet x = 1;\n# not a heading\n```\nafter");
        assert!(
            matches!(&out[0], Block::Code { language, text }
                if language == "rust" && text == "let x = 1;\n# not a heading")
        );
        assert!(matches!(&out[1], Block::Paragraph(_)));
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end_rather_than_swallowing_the_parser() {
        let out = blocks("```\nstill code");
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], Block::Code { .. }));
    }

    #[test]
    fn emphasis_code_and_links_come_out_as_runs() {
        let out = spans("plain **bold** `code` [text](https://example.com)");
        assert!(out.iter().any(|s| s.bold && s.text == "bold"));
        assert!(out.iter().any(|s| s.code && s.text == "code"));
        assert!(
            out.iter()
                .any(|s| s.link.as_deref() == Some("https://example.com") && s.text == "text")
        );
    }

    #[test]
    fn a_rule_is_a_rule_and_a_dash_is_not() {
        assert!(matches!(blocks("---")[0], Block::Rule));
        assert!(matches!(blocks("- item")[0], Block::Item { .. }));
    }
}
