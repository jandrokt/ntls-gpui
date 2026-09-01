//! Finding the expressions in a document and working them out.

use crate::expr::{Source, run};

/// A document, split into what is written and what is computed.
#[derive(Clone, PartialEq, Debug)]
pub enum Piece {
    Text(String),
    /// The source of one expression, kept so an error can quote it.
    Expr(String),
}

/// Splits a document on `{{ … }}`.
///
/// An unclosed `{{` is text, not an error: a document is edited a character at
/// a time, and half a placeholder should not blank the page.
pub fn pieces(source: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut rest = source;

    while let Some(open) = rest.find("{{") {
        let Some(close) = rest[open + 2..].find("}}") else { break };
        if open > 0 {
            out.push(Piece::Text(rest[..open].to_string()));
        }
        out.push(Piece::Expr(rest[open + 2..open + 2 + close].trim().to_string()));
        rest = &rest[open + 2 + close + 2..];
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest.to_string()));
    }
    out
}

/// The document with every expression replaced by what it comes to.
///
/// An expression that cannot be worked out is left in place as `⟨why⟩`, so the
/// mistake is visible where it was made and not swallowed.
pub fn expand(source: &str, data: &dyn Source) -> String {
    let mut out = String::new();
    for piece in pieces(source) {
        match piece {
            Piece::Text(text) => out.push_str(&text),
            Piece::Expr(source_text) if source_text.is_empty() => {}
            Piece::Expr(source_text) => match run(&source_text, data) {
                Ok(value) => out.push_str(&value.show()),
                Err(why) => out.push_str(&format!("\u{27e8}{why}\u{27e9}")),
            },
        }
    }
    out
}

/// Every expression in a document, for checking one without rendering it.
#[cfg(test)]
pub fn expressions(source: &str) -> Vec<String> {
    pieces(source)
        .into_iter()
        .filter_map(|p| match p {
            Piece::Expr(source_text) if !source_text.is_empty() => Some(source_text),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::eval::Empty;

    #[test]
    fn a_document_splits_into_writing_and_arithmetic() {
        let out = pieces("Average is {{ 1 + 1 }} ms.");
        assert_eq!(
            out,
            vec![
                Piece::Text("Average is ".into()),
                Piece::Expr("1 + 1".into()),
                Piece::Text(" ms.".into()),
            ]
        );
    }

    #[test]
    fn an_expression_is_replaced_by_what_it_comes_to() {
        assert_eq!(expand("{{ 2 * 21 }}", &Empty), "42");
        assert_eq!(expand("a {{ 1 }} b {{ 2 }} c", &Empty), "a 1 b 2 c");
    }

    #[test]
    fn a_half_written_placeholder_is_left_as_writing() {
        // A document is edited a character at a time; `{{` on its own must not
        // blank the page.
        assert_eq!(expand("what about {{ this", &Empty), "what about {{ this");
        assert_eq!(pieces("{{").len(), 1);
    }

    #[test]
    fn a_mistake_is_shown_where_it_was_made() {
        let out = expand("value: {{ 1 + }}", &Empty);
        assert!(out.starts_with("value: \u{27e8}"), "{out}");
        assert!(out.ends_with('\u{27e9}'));
    }

    #[test]
    fn an_empty_placeholder_writes_nothing() {
        assert_eq!(expand("a{{}}b", &Empty), "ab");
    }

    #[test]
    fn the_expressions_can_be_listed_without_running_them() {
        assert_eq!(expressions("{{ a }} and {{ b }}"), vec!["a", "b"]);
    }
}
