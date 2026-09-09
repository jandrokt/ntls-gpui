//! Reading a `curl` command line.
//!
//! An HTTP request is nearly always handed round as a `curl` line: in an
//! issue, in a service's own documentation, on the clipboard from a browser's
//! "copy as cURL". Retyping one into a form a field at a time is the tedium
//! the form was supposed to remove, so the form reads it instead.
//!
//! Only the flags that say something about the request itself. `curl` has
//! well over two hundred; the ones anybody pastes are the method, the
//! headers, the body, the credentials and a handful of switches. An unknown
//! flag is skipped rather than refused, because a line that is 90% readable
//! is still worth reading.

/// What a `curl` line asks for, in this tool's own field names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub method: Option<String>,
    /// One `Name: value` per line, which is how the headers field is written.
    pub headers: Vec<String>,
    pub body: Option<String>,
    pub body_kind: Option<&'static str>,
    /// `user:password` for Basic, or the token for Bearer.
    pub auth: Option<(&'static str, String)>,
    pub insecure: bool,
    pub follow: bool,
    pub http1: bool,
}

/// Reads a `curl` line, or returns `None` when it is not one.
pub fn parse(line: &str) -> Option<Request> {
    let words = split(line)?;
    let mut words = words.into_iter();
    // The first word has to be the program, or this is not a curl line at
    // all and the text is whatever the user meant it to be.
    let first = words.next()?;
    if !first.eq_ignore_ascii_case("curl") && !first.to_lowercase().ends_with("/curl") {
        return None;
    }

    let mut out = Request::default();
    let mut form_fields: Vec<String> = Vec::new();
    // `-fsSL` is four flags, and pasted lines are full of such bundles.
    // Split only when every letter is one that takes no value, so `-XPOST`
    // and `-Hx` are still one flag and its argument.
    let spread: Vec<String> = words
        .flat_map(|word| match bundle(&word) {
            Some(flags) => flags,
            None => vec![word],
        })
        .collect();
    let mut words = spread.into_iter().peekable();

    while let Some(word) = words.next() {
        let mut value = |held: &str| -> Option<String> {
            // `-H x`, `-Hx` and `--header=x` are all the same thing.
            if let Some(rest) = word.strip_prefix(held).filter(|r| !r.is_empty()) {
                return Some(rest.strip_prefix('=').unwrap_or(rest).to_string());
            }
            words.next()
        };

        match word.as_str() {
            w if w == "-X" || w == "--request" || w.starts_with("-X") || w.starts_with("--request=") => {
                out.method = value(if w.starts_with("--request") { "--request" } else { "-X" })
                    .map(|m| m.to_uppercase());
            }
            w if w == "-H" || w == "--header" || w.starts_with("-H") || w.starts_with("--header=") => {
                if let Some(header) = value(if w.starts_with("--header") { "--header" } else { "-H" })
                {
                    out.headers.push(header);
                }
            }
            w if w == "-u" || w == "--user" || w.starts_with("--user=") => {
                if let Some(user) = value(if w.starts_with("--user") { "--user" } else { "-u" }) {
                    out.auth = Some(("basic", user));
                }
            }
            w if w == "-d"
                || w == "--data"
                || w == "--data-raw"
                || w == "--data-binary"
                || w == "--data-ascii"
                || w.starts_with("--data") =>
            {
                let flag = ["--data-raw", "--data-binary", "--data-ascii", "--data", "-d"]
                    .into_iter()
                    .find(|f| w.starts_with(f))
                    .unwrap_or("-d");
                if let Some(body) = value(flag) {
                    // Several `-d` are joined with `&`, the way curl joins
                    // them.
                    match &mut out.body {
                        Some(held) => {
                            held.push('&');
                            held.push_str(&body);
                        }
                        None => out.body = Some(body),
                    }
                }
            }
            w if w == "-F" || w == "--form" || w.starts_with("--form=") => {
                if let Some(field) = value(if w.starts_with("--form") { "--form" } else { "-F" }) {
                    form_fields.push(field);
                }
            }
            "-k" | "--insecure" => out.insecure = true,
            "-L" | "--location" => out.follow = true,
            "--http1.1" | "--http1.0" | "-0" => out.http1 = true,
            // A HEAD request is spelt as a switch rather than a method.
            "-I" | "--head" => out.method = Some("HEAD".into()),
            // Flags that say nothing about the request: how loud curl is,
            // where it writes, how long it waits at the shell.
            "-s" | "--silent" | "-v" | "--verbose" | "-i" | "--include" | "-S" | "--show-error"
            | "--compressed" | "-g" | "--globoff" | "-f" | "--fail" | "-N" | "--no-buffer" => {}
            // Something with an argument this does not act on: skip both, so
            // its argument is not mistaken for the URL.
            w if w.starts_with('-') => {
                if takes_an_argument(w) && !w.contains('=') {
                    words.next();
                }
            }
            // The first bare word is the URL. A second is another URL, which
            // one request cannot be.
            bare => {
                if out.url.is_empty() {
                    out.url = bare.to_string();
                }
            }
        }
    }

    if out.url.is_empty() {
        return None;
    }
    // Attachments are a form, so anything sent with `-F` becomes one whether
    // or not a body was also given.
    if !form_fields.is_empty() {
        out.body = Some(form_fields.join("\n"));
        out.body_kind = Some("form");
    } else if out.body.is_some() {
        out.body_kind = Some(kind_of(&out.headers, out.body.as_deref().unwrap_or("")));
    }
    // A body with no method named is a POST, the way curl treats it.
    if out.body.is_some() && out.method.is_none() {
        out.method = Some("POST".into());
    }
    // Bearer is a header rather than a flag, so it is read back out of one.
    if out.auth.is_none()
        && let Some(token) = bearer(&out.headers)
    {
        out.auth = Some(("bearer", token));
        out.headers.retain(|h| !is_authorization(h));
    }
    Some(out)
}

/// What kind of body it is: what the headers say, or what it looks like.
fn kind_of(headers: &[String], body: &str) -> &'static str {
    for header in headers {
        let Some((name, value)) = header.split_once(':') else { continue };
        if !name.trim().eq_ignore_ascii_case("content-type") {
            continue;
        }
        let value = value.trim().to_lowercase();
        if value.contains("json") {
            return "json";
        }
        if value.contains("xml") {
            return "xml";
        }
        if value.contains("x-www-form-urlencoded") {
            return "form";
        }
        return "text";
    }
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return "json";
    }
    if trimmed.starts_with('<') {
        return "xml";
    }
    if body.contains('=') && !body.contains('\n') {
        return "form";
    }
    "text"
}

fn is_authorization(header: &str) -> bool {
    header.split_once(':').is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("authorization"))
}

fn bearer(headers: &[String]) -> Option<String> {
    headers.iter().filter(|h| is_authorization(h)).find_map(|h| {
        let (_, value) = h.split_once(':')?;
        let value = value.trim();
        let rest = value.get(..7).filter(|p| p.eq_ignore_ascii_case("bearer "))?;
        let _ = rest;
        Some(value[7..].trim().to_string())
    })
}

/// `-fsSL` as the flags it stands for, when every letter is one that takes
/// no value of its own.
fn bundle(word: &str) -> Option<Vec<String>> {
    /// The short flags with nothing after them. A letter outside this set
    /// means the word is a flag and its argument, not a bundle.
    const ALONE: &str = "sSvikLIfNg0";

    let letters = word.strip_prefix('-').filter(|rest| !rest.starts_with('-'))?;
    if letters.len() < 2 || !letters.chars().all(|c| ALONE.contains(c)) {
        return None;
    }
    Some(letters.chars().map(|c| format!("-{c}")).collect())
}

/// Whether a flag this does not act on still swallows the word after it.
fn takes_an_argument(flag: &str) -> bool {
    const WITH: [&str; 18] = [
        "-o", "--output", "-A", "--user-agent", "-e", "--referer", "-b", "--cookie", "-c",
        "--cookie-jar", "-m", "--max-time", "--connect-timeout", "-x", "--proxy", "--cert",
        "--key", "--cacert",
    ];
    WITH.contains(&flag)
}

/// Splits a command line the way a shell would: on spaces, respecting quotes
/// and the backslash-newline that a pasted multi-line command is full of.
fn split(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut has_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            // Inside single quotes a backslash is an ordinary character.
            Some('\'') if ch == '\'' => quote = None,
            Some('\'') => word.push(ch),
            Some('"') if ch == '"' => quote = None,
            Some('"') if ch == '\\' => match chars.next() {
                // A line continuation inside a quoted string is still one.
                Some('\n') => {}
                Some(next) => {
                    // Only these mean something after a backslash in double
                    // quotes; anything else keeps the backslash.
                    if !matches!(next, '"' | '\\' | '$' | '`') {
                        word.push('\\');
                    }
                    word.push(next);
                }
                None => word.push('\\'),
            },
            Some('"') => word.push(ch),
            Some(_) => word.push(ch),
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    has_word = true;
                }
                '\\' => match chars.next() {
                    // The continuation a multi-line curl line is broken on.
                    Some('\n') | None => {}
                    Some(next) => {
                        word.push(next);
                        has_word = true;
                    }
                },
                ch if ch.is_whitespace() => {
                    if has_word {
                        words.push(std::mem::take(&mut word));
                        has_word = false;
                    }
                }
                ch => {
                    word.push(ch);
                    has_word = true;
                }
            },
        }
    }
    // An unclosed quote is a line still being typed, not one to act on.
    if quote.is_some() {
        return None;
    }
    if has_word {
        words.push(word);
    }
    (!words.is_empty()).then_some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_line_is_a_get_of_its_one_bare_word() {
        let out = parse("curl https://example.com/health").expect("a curl line");
        assert_eq!(out.url, "https://example.com/health");
        assert_eq!(out.method, None);
        assert!(out.headers.is_empty());
        assert_eq!(out.body, None);
    }

    #[test]
    fn anything_that_is_not_a_curl_line_is_left_alone() {
        // The common case, and the one that must stay cheap: this is asked
        // on every edit of the URL field.
        assert!(parse("https://example.com").is_none());
        assert!(parse("").is_none());
        assert!(parse("curl").is_none(), "no URL");
        assert!(parse("wget https://example.com").is_none());
        // Still being typed: the quote is open, so there is nothing to act
        // on yet.
        assert!(parse("curl -H 'Accept: application/json").is_none());
    }

    #[test]
    fn the_method_headers_and_body_are_read_out_of_it() {
        let out = parse(
            r#"curl -X PUT 'https://api.example.com/v1/things?since=1h' \
                 -H 'Accept: application/json' \
                 -H "X-Trace: abc def" \
                 --data-raw '{"name": "one"}'"#,
        )
        .expect("a curl line");

        assert_eq!(out.url, "https://api.example.com/v1/things?since=1h");
        assert_eq!(out.method.as_deref(), Some("PUT"));
        assert_eq!(out.headers, vec!["Accept: application/json", "X-Trace: abc def"]);
        assert_eq!(out.body.as_deref(), Some(r#"{"name": "one"}"#));
        assert_eq!(out.body_kind, Some("json"));
    }

    #[test]
    fn a_body_with_no_method_is_a_post() {
        let out = parse("curl https://example.com -d 'a=1'").expect("a curl line");
        assert_eq!(out.method.as_deref(), Some("POST"));
        assert_eq!(out.body_kind, Some("form"));

        // And several bodies are joined the way curl joins them.
        let out = parse("curl https://example.com -d a=1 -d b=2").expect("a curl line");
        assert_eq!(out.body.as_deref(), Some("a=1&b=2"));
    }

    #[test]
    fn credentials_are_read_from_the_flag_or_from_the_header() {
        let out = parse("curl -u alice:secret https://example.com").expect("a curl line");
        assert_eq!(out.auth, Some(("basic", "alice:secret".into())));

        // A bearer token arrives as a header, and becomes the credential
        // rather than staying a header nobody would think to look in.
        let out = parse("curl -H 'Authorization: Bearer abc123' https://example.com")
            .expect("a curl line");
        assert_eq!(out.auth, Some(("bearer", "abc123".into())));
        assert!(out.headers.is_empty(), "{:?}", out.headers);
    }

    #[test]
    fn the_switches_that_change_the_request_are_kept_and_the_rest_dropped() {
        let out = parse("curl -sSL -k --compressed https://example.com").expect("a curl line");
        assert!(out.follow);
        assert!(out.insecure);
        assert_eq!(out.url, "https://example.com");

        // `-I` is a method spelt as a switch.
        assert_eq!(parse("curl -I https://example.com").unwrap().method.as_deref(), Some("HEAD"));
    }

    #[test]
    fn a_flag_this_ignores_does_not_swallow_the_url() {
        // `-o out.json` takes an argument. Without knowing that, the file
        // name would have been read as the URL and the real URL dropped.
        let out = parse("curl -o out.json https://example.com").expect("a curl line");
        assert_eq!(out.url, "https://example.com");

        // And one that takes no argument must not eat the URL either.
        let out = parse("curl --fail https://example.com").expect("a curl line");
        assert_eq!(out.url, "https://example.com");
    }

    #[test]
    fn a_flag_written_against_its_value_reads_the_same() {
        for line in [
            "curl -XPOST https://example.com",
            "curl -X POST https://example.com",
            "curl --request=POST https://example.com",
        ] {
            assert_eq!(parse(line).unwrap().method.as_deref(), Some("POST"), "{line}");
        }
    }

    #[test]
    fn attachments_make_it_a_form() {
        let out = parse("curl -F file=@/tmp/a.pdf -F name=one https://example.com")
            .expect("a curl line");
        assert_eq!(out.body.as_deref(), Some("file=@/tmp/a.pdf\nname=one"));
        assert_eq!(out.body_kind, Some("form"));
        assert_eq!(out.method.as_deref(), Some("POST"));
    }

    #[test]
    fn quoting_survives_the_way_a_shell_would_leave_it() {
        let words = split(r#"curl 'a b' "c\"d" e\ f"#).expect("words");
        assert_eq!(words, vec!["curl", "a b", r#"c"d"#, "e f"]);
        // An empty argument is a word, not nothing.
        assert_eq!(split(r#"curl -d '' x"#).expect("words"), vec!["curl", "-d", "", "x"]);
    }
}
