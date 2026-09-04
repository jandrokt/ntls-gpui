//! Turning tokens into an expression tree.
//!
//! A Pratt parser, so precedence is a number per operator and not a
//! grammar rule per level.

use super::lex::{Tok, lex};

#[derive(Clone, Debug)]
pub enum Expr {
    Number(f64),
    Text(String),
    /// A bare name: a variable, or `true` / `false`.
    Name(String),
    /// `subject.field`
    Field(Box<Expr>, String),
    /// `subject[key]`: a list by position, a set of results by column name.
    Index(Box<Expr>, Box<Expr>),
    /// `name(args...)`.
    Call(String, Vec<Expr>),
    /// `subject.name(args...)`, with the subject kept apart from the
    /// arguments.
    ///
    /// Folding the subject in as the first argument threw away the one thing
    /// that tells a receiver from an argument, and the two are not read the
    /// same way: a quoted receiver names a run, where a quoted argument is
    /// only ever text. That is what made `"IP scan".count()` answer about the
    /// name instead of about the run.
    Method(Box<Expr>, String, Vec<Expr>),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    /// `if condition then a else b`
    If(Box<Expr>, Box<Expr>, Box<Expr>),
}

/// How tightly each operator binds.
fn power(sym: &str) -> Option<u8> {
    Some(match sym {
        "||" => 1,
        "&&" => 2,
        "==" | "!=" | "<" | "<=" | ">" | ">=" => 3,
        "+" | "-" => 4,
        "*" | "/" | "%" => 5,
        _ => return None,
    })
}

pub fn parse(source: &str) -> Result<Expr, String> {
    let toks = lex(source)?;
    let mut p = Parser { toks, at: 0 };
    let expr = p.expr(0)?;
    match p.peek() {
        Tok::End => Ok(expr),
        // Two names in a row is almost always one name written without its
        // quotes, and saying so beats saying what the parser noticed.
        Tok::Name(_) if matches!(expr, Expr::Name(_) | Expr::Field(..)) => Err(format!(
            "unexpected {} after the expression. A name with a space in it goes in quotes, like \"IP scan\".rows",
            p.peek()
        )),
        other => Err(format!("unexpected {other} after the expression")),
    }
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Tok {
        self.toks.get(self.at).cloned().unwrap_or(Tok::End)
    }

    fn next(&mut self) -> Tok {
        let tok = self.peek();
        self.at += 1;
        tok
    }

    fn eat(&mut self, sym: &str) -> bool {
        if matches!(self.peek(), Tok::Sym(s) if s == sym) {
            self.at += 1;
            return true;
        }
        false
    }

    fn expect(&mut self, sym: &str) -> Result<(), String> {
        if self.eat(sym) { Ok(()) } else { Err(format!("expected {sym}, found {}", self.peek())) }
    }

    fn expect_word(&mut self, word: &str) -> Result<(), String> {
        match self.peek() {
            Tok::Name(n) if n == word => {
                self.at += 1;
                Ok(())
            }
            other => Err(format!("expected {word}, found {other}")),
        }
    }

    /// One expression, taking operators that bind at least this tightly.
    fn expr(&mut self, least: u8) -> Result<Expr, String> {
        let mut left = self.prefix()?;
        while let Tok::Sym(sym) = self.peek() {
            let Some(power) = power(sym) else { break };
            if power < least {
                break;
            }
            self.at += 1;
            // Every operator here is left-associative, so the right side takes
            // only what binds more tightly than this.
            let right = self.expr(power + 1)?;
            left = Expr::Binary(sym, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn prefix(&mut self) -> Result<Expr, String> {
        let expr = match self.next() {
            Tok::Number(n) => Expr::Number(n),
            Tok::Text(s) => Expr::Text(s),
            Tok::Name(name) if name == "if" => {
                let condition = self.expr(0)?;
                self.expect_word("then")?;
                let then = self.expr(0)?;
                self.expect_word("else")?;
                let otherwise = self.expr(0)?;
                Expr::If(Box::new(condition), Box::new(then), Box::new(otherwise))
            }
            // `not x` reads better than `!x` in a document, and costs one
            // line to allow.
            Tok::Name(name) if name == "not" => Expr::Unary("!", Box::new(self.prefix()?)),
            Tok::Name(name) => {
                if self.eat("(") {
                    Expr::Call(name, self.arguments()?)
                } else {
                    Expr::Name(name)
                }
            }
            Tok::Sym("-") => Expr::Unary("-", Box::new(self.prefix()?)),
            Tok::Sym("!") => Expr::Unary("!", Box::new(self.prefix()?)),
            Tok::Sym("(") => {
                let inner = self.expr(0)?;
                self.expect(")")?;
                inner
            }
            other => return Err(format!("expected a value, found {other}")),
        };
        self.suffix(expr)
    }

    /// Field access, indexing and method calls, which chain: `a.b[0].c(1).d`.
    fn suffix(&mut self, mut expr: Expr) -> Result<Expr, String> {
        loop {
            if self.eat(".") {
                let Tok::Name(name) = self.next() else {
                    return Err("expected a name after '.'".into());
                };
                expr = if self.eat("(") {
                    // A method still means the function with the subject
                    // first, but which of the two was written has to survive
                    // parsing: the evaluator reads a quoted subject as the
                    // name of a run, and it can only do that while it can
                    // still tell the subject from an argument.
                    Expr::Method(Box::new(expr), name, self.arguments()?)
                } else {
                    Expr::Field(Box::new(expr), name)
                };
                continue;
            }
            if self.eat("[") {
                let key = self.expr(0)?;
                self.expect("]")?;
                expr = Expr::Index(Box::new(expr), Box::new(key));
                continue;
            }
            return Ok(expr);
        }
    }

    fn arguments(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        if self.eat(")") {
            return Ok(args);
        }
        loop {
            args.push(self.expr(0)?);
            if self.eat(",") {
                continue;
            }
            self.expect(")")?;
            return Ok(args);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tree, written back out, so a test can say what it expects without
    /// matching on nested boxes.
    fn show(expr: &Expr) -> String {
        match expr {
            Expr::Number(n) => format!("{n}"),
            Expr::Text(s) => format!("{s:?}"),
            Expr::Name(n) => n.clone(),
            Expr::Field(s, f) => format!("({}.{f})", show(s)),
            Expr::Index(s, k) => format!("({}[{}])", show(s), show(k)),
            Expr::Call(name, args) => {
                let args: Vec<String> = args.iter().map(show).collect();
                format!("{name}({})", args.join(" "))
            }
            // Written the way the call it means is written, since a method is
            // that function with the subject first.
            Expr::Method(subject, name, args) => {
                let args: Vec<String> =
                    std::iter::once(show(subject)).chain(args.iter().map(show)).collect();
                format!("{name}({})", args.join(" "))
            }
            Expr::Unary(op, a) => format!("({op}{})", show(a)),
            Expr::Binary(op, a, b) => format!("({} {op} {})", show(a), show(b)),
            Expr::If(c, a, b) => format!("(if {} {} {})", show(c), show(a), show(b)),
        }
    }

    fn parsed(source: &str) -> String {
        show(&parse(source).expect("it to parse"))
    }

    #[test]
    fn arithmetic_binds_the_way_arithmetic_does() {
        assert_eq!(parsed("1 + 2 * 3"), "(1 + (2 * 3))");
        assert_eq!(parsed("(1 + 2) * 3"), "((1 + 2) * 3)");
        assert_eq!(parsed("1 - 2 - 3"), "((1 - 2) - 3)");
    }

    #[test]
    fn comparison_binds_looser_than_arithmetic_and_tighter_than_and() {
        assert_eq!(parsed("a + 1 > b && c"), "(((a + 1) > b) && c)");
        assert_eq!(parsed("a || b && c"), "(a || (b && c))");
    }

    #[test]
    fn a_method_is_a_function_with_the_subject_first() {
        assert_eq!(parsed("rows.avg(\"RTT\")"), "avg(rows \"RTT\")");
        assert_eq!(parsed("tool(\"Ping\").rows.count()"), "count((tool(\"Ping\").rows))");
    }

    #[test]
    fn a_conditional_reads_as_one() {
        assert_eq!(parsed("if a > 1 then \"yes\" else \"no\""), "(if (a > 1) \"yes\" \"no\")");
    }

    #[test]
    fn a_subject_can_be_indexed_as_well_as_named() {
        assert_eq!(parsed("a[0]"), "(a[0])");
        assert_eq!(parsed("a[\"RTT\"].avg()"), "avg((a[\"RTT\"]))");
        assert_eq!(parsed("a.b[1].c"), "(((a.b)[1]).c)");
    }

    #[test]
    fn not_is_another_way_of_writing_it() {
        assert_eq!(parsed("not a"), "(!a)");
        assert_eq!(parsed("not a && b"), "((!a) && b)");
    }

    #[test]
    fn a_name_written_without_its_quotes_says_what_to_do_about_it() {
        let complaint = parse("IP scan.rows").expect_err("it not to parse");
        assert!(complaint.contains("quotes"), "{complaint}");
        assert!(parse(r#""IP scan".rows"#).is_ok());
    }

    #[test]
    fn what_cannot_be_parsed_says_so() {
        assert!(parse("1 +").is_err());
        assert!(parse("(1").is_err());
        assert!(parse("if a then b").is_err());
        assert!(parse("1 2").is_err());
        assert!(parse("a[1").is_err());
    }
}
