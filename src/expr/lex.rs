//! Turning expression source into tokens.

use std::fmt;

#[derive(Clone, PartialEq, Debug)]
pub enum Tok {
    Number(f64),
    Text(String),
    Name(String),
    /// One of `+ - * / % ( ) , . < <= > >= == != && || ! ?`
    Sym(&'static str),
    End,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Number(n) => write!(f, "{n}"),
            Tok::Text(s) => write!(f, "\"{s}\""),
            Tok::Name(s) => write!(f, "{s}"),
            Tok::Sym(s) => write!(f, "{s}"),
            Tok::End => write!(f, "end of expression"),
        }
    }
}

/// The two-character symbols, tried before the one-character ones so that `<=`
/// is never read as `<` followed by `=`.
const PAIRS: [&str; 6] = ["<=", ">=", "==", "!=", "&&", "||"];
const SINGLES: [&str; 14] =
    ["+", "-", "*", "/", "%", "(", ")", "[", "]", ",", ".", "<", ">", "!"];

pub fn lex(source: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut at = 0usize;

    while at < chars.len() {
        let c = chars[at];
        if c.is_whitespace() {
            at += 1;
            continue;
        }

        if c.is_ascii_digit() || (c == '.' && chars.get(at + 1).is_some_and(char::is_ascii_digit)) {
            let start = at;
            while chars.get(at).is_some_and(|c| c.is_ascii_digit() || *c == '.') {
                at += 1;
            }
            let text: String = chars[start..at].iter().collect();
            let value = text.parse().map_err(|_| format!("{text} is not a number"))?;
            out.push(Tok::Number(value));
            continue;
        }

        if c == '"' || c == '\'' {
            let quote = c;
            at += 1;
            let mut text = String::new();
            loop {
                match chars.get(at) {
                    None => return Err("unclosed string".into()),
                    Some(&ch) if ch == quote => {
                        at += 1;
                        break;
                    }
                    Some('\\') => {
                        at += 1;
                        match chars.get(at) {
                            Some('n') => text.push('\n'),
                            Some('t') => text.push('\t'),
                            Some(&ch) => text.push(ch),
                            None => return Err("unclosed string".into()),
                        }
                        at += 1;
                    }
                    Some(&ch) => {
                        text.push(ch);
                        at += 1;
                    }
                }
            }
            out.push(Tok::Text(text));
            continue;
        }

        if c.is_alphabetic() || c == '_' {
            let start = at;
            while chars.get(at).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
                at += 1;
            }
            out.push(Tok::Name(chars[start..at].iter().collect()));
            continue;
        }

        let rest: String = chars[at..].iter().take(2).collect();
        if let Some(pair) = PAIRS.iter().find(|p| rest.starts_with(**p)) {
            out.push(Tok::Sym(pair));
            at += 2;
            continue;
        }
        if let Some(single) = SINGLES.iter().find(|s| c.to_string() == **s) {
            out.push(Tok::Sym(single));
            at += 1;
            continue;
        }
        return Err(format!("unexpected character {c:?}"));
    }

    out.push(Tok::End);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_names_and_strings_come_out_whole() {
        let toks = lex(r#"avg(12.5, "a b", x_1)"#).expect("it to lex");
        assert_eq!(
            toks,
            vec![
                Tok::Name("avg".into()),
                Tok::Sym("("),
                Tok::Number(12.5),
                Tok::Sym(","),
                Tok::Text("a b".into()),
                Tok::Sym(","),
                Tok::Name("x_1".into()),
                Tok::Sym(")"),
                Tok::End,
            ]
        );
    }

    #[test]
    fn two_character_symbols_are_not_split() {
        let toks = lex("a <= b && c != d").expect("it to lex");
        assert!(toks.contains(&Tok::Sym("<=")));
        assert!(toks.contains(&Tok::Sym("&&")));
        assert!(toks.contains(&Tok::Sym("!=")));
        assert!(!toks.contains(&Tok::Sym("<")));
    }

    #[test]
    fn a_string_keeps_its_escapes_and_must_be_closed() {
        assert_eq!(lex(r#""a\"b""#).expect("it to lex")[0], Tok::Text("a\"b".into()));
        assert!(lex("\"unfinished").is_err());
    }

    #[test]
    fn a_leading_dot_is_part_of_a_number_and_otherwise_is_access() {
        assert_eq!(lex(".5").expect("it to lex")[0], Tok::Number(0.5));
        assert_eq!(lex("a.b").expect("it to lex")[1], Tok::Sym("."));
    }
}
