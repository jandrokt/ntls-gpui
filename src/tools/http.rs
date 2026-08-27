//! Sends an HTTP request and reads what comes back.
//!
//! The question a network tool is asked about a web service is rarely "is the
//! port open" — it is "what does it answer, how fast, and is it still
//! answering that". So this is a request tool rather than a fetcher: one row
//! per request, the status and the timing in the table, the headers and the
//! start of the body in the log, and a graph of the response time when it is
//! asked more than once.

use std::time::{Duration, Instant};

use crate::cells;
use crate::core::{
    Column, Emitter, Event, Field, Opt, Role, Status, Tool, Validator, VisibleIf, col, kv,
};
use crate::tools::stats::{elapsed, ms};

pub struct Http;

/// How much of the body is worth putting in the log. Past this it is a file,
/// and the download tool is the one that fetches files.
const BODY_PREVIEW: usize = 2048;

/// The methods a person actually sends by hand.
const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

impl Tool for Http {
    fn id(&self) -> &'static str {
        "http"
    }
    fn title(&self) -> &'static str {
        "HTTP request"
    }
    fn desc(&self) -> &'static str {
        "Send a request and read the status, timing and headers that come back"
    }
    fn icon(&self) -> &'static str {
        "globe"
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            Field::text("target", "URL", "What to ask, in full")
                .placeholder("https://example.com/health")
                .role(Role::Target)
                .validate(Validator::Required),
            Field::select(
                "method",
                "Method",
                "Which request to send",
                "GET",
                METHODS.iter().map(|m| Opt::new(m, m, method_desc(m))).collect(),
            ),
            Field::text(
                "headers",
                "Headers",
                "Sent with the request. Separate them with a semicolon: Accept: application/json; X-Token: abc",
            )
            .placeholder("Accept: application/json"),
            Field::text("body", "Body", "Sent as the request body")
                .placeholder("{\"ok\": true}")
                .visible_if(VisibleIf::NotEquals("method", "GET")),
            Field::text("count", "Requests", "How many to send; more than one draws a graph")
                .default("1")
                .validate(Validator::IntRange(1, 100_000)),
            Field::text("interval", "Interval", "How long to wait between them")
                .default("1s")
                .validate(Validator::Duration)
                .visible_if(VisibleIf::NotEquals("count", "1")),
            Field::text("timeout", "Timeout", "How long to wait for an answer")
                .default("10s")
                .validate(Validator::Duration),
            Field::boolean("follow", "Follow redirects", "Ask again wherever it points", true),
            Field::boolean(
                "showheaders",
                "Log the headers",
                "Write the response headers into the output panel",
                true,
            ),
            Field::boolean(
                "showbody",
                "Log the body",
                "Write the start of the response body into the output panel",
                false,
            ),
        ]
    }

    fn columns(&self) -> Vec<Column> {
        vec![
            col("#", 5),
            col("STATUS", 10),
            col("TIME", 9),
            col("SIZE", 9),
            col("TYPE", 22),
            col("DETAIL", 0),
        ]
    }

    fn resumable(&self) -> bool {
        false
    }

    fn run<'a>(
        &'a self,
        r: crate::core::Run,
        emit: Emitter,
    ) -> crate::core::tool::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { run(r, emit).await })
    }
}

fn method_desc(method: &str) -> &'static str {
    match method {
        "GET" => "ask for it",
        "HEAD" => "ask for the headers alone",
        "POST" => "send something",
        "PUT" => "replace it",
        "PATCH" => "change part of it",
        "DELETE" => "remove it",
        _ => "ask what it allows",
    }
}

/// `Accept: application/json; X-Token: abc`, or one per line.
///
/// Both separators, because a header list is as often pasted out of somewhere
/// as it is typed.
fn parse_headers(text: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut headers = Vec::new();
    let mut problems = Vec::new();
    for piece in text.split([';', '\n']) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        match piece.split_once(':') {
            Some((name, value)) if !name.trim().is_empty() => {
                headers.push((name.trim().to_string(), value.trim().to_string()));
            }
            _ => problems.push(piece.to_string()),
        }
    }
    (headers, problems)
}

/// A URL typed the way it is spoken: `example.com` means `https://example.com`.
fn normalize_url(raw: &str) -> String {
    let raw = raw.trim();
    if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") }
}

/// What the status line says, and how to colour the row it is on.
fn status_of(code: u16) -> Status {
    match code {
        200..=299 => Status::Up,
        300..=399 => Status::Info,
        400..=499 => Status::Warn,
        _ => Status::Down,
    }
}

async fn run(r: crate::core::Run, emit: Emitter) -> anyhow::Result<()> {
    let (cancel, p) = (r.cancel.clone(), r.params.clone());
    let url = normalize_url(&p.str("target"));
    let method = p.str("method");
    let timeout = p.dur("timeout", Duration::from_secs(10));
    let interval = p.dur("interval", Duration::from_secs(1));
    let count = p.usize("count", 1).max(1);
    let follow = p.bool("follow");
    let (headers, unreadable) = parse_headers(&p.str("headers"));
    for bad in &unreadable {
        emit.warn(format!("{bad:?} is not a header: they read Name: value"));
    }

    let verb = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| anyhow::anyhow!("{method} is not a method"))?;
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(if follow {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        })
        .user_agent(concat!("ntls/", env!("CARGO_PKG_VERSION")))
        .build()?;

    // A resumed run carries on the numbering rather than starting again.
    let first = r.prior.len();
    let start = Instant::now();
    let mut sent = 0usize;
    let mut ok = 0usize;
    let mut times: Vec<f64> = Vec::new();

    for n in 0..count {
        if cancel.is_cancelled() {
            break;
        }
        if n > 0 {
            // The interval is between requests, so a stopped run does not
            // wait out the last one.
            if !cancel.sleep(interval).await {
                break;
            }
        }

        let mut request = client.request(verb.clone(), &url);
        for (name, value) in &headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let body = p.str("body");
        if !body.is_empty() && verb != reqwest::Method::GET && verb != reqwest::Method::HEAD {
            request = request.body(body);
        }

        let index = first + n + 1;
        let began = Instant::now();
        let sending = request.send();
        let answered = match cancel.run(sending).await {
            None => break,
            Some(answered) => answered,
        };
        sent += 1;

        match answered {
            Err(e) => {
                emit.row(
                    Status::Down,
                    url.clone(),
                    cells![index, "—", ms(began.elapsed()), "—", "—", why(&e)],
                );
                emit.err(format!("{method} {url}: {}", why(&e)));
            }
            Ok(response) => {
                let code = response.status();
                let landed = response.url().to_string();
                let kind = header(&response, reqwest::header::CONTENT_TYPE);
                let server = header(&response, reqwest::header::SERVER);
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);

                if n == 0 && p.bool("showheaders") {
                    emit.info(format!("{} {}", code.as_u16(), code.canonical_reason().unwrap_or("")));
                    for (name, value) in response.headers() {
                        emit.info(format!("{name}: {}", value.to_str().unwrap_or("—")));
                    }
                }

                let show_body = n == 0 && p.bool("showbody");
                let body = match cancel.run(response.bytes()).await {
                    None => break,
                    Some(Ok(bytes)) => bytes,
                    Some(Err(e)) => {
                        emit.warn(format!("the body did not arrive whole: {}", why(&e)));
                        Default::default()
                    }
                };
                let took = began.elapsed();
                times.push(took.as_secs_f64() * 1000.);
                if code.is_success() {
                    ok += 1;
                }

                // What is worth saying about the answer beyond its status:
                // where it ended up, or where it is pointing.
                let detail = match (&location, landed != url) {
                    (Some(to), _) if !follow => format!("→ {to}"),
                    (_, true) => format!("→ {landed}"),
                    _ => server.clone(),
                };

                emit.row(
                    status_of(code.as_u16()),
                    url.clone(),
                    cells![
                        index,
                        format!("{}", code.as_u16()),
                        ms(took),
                        crate::dl::names::bytes(body.len() as u64),
                        short(&kind),
                        detail
                    ],
                );
                emit.emit(Event::sample("response", "ms", took.as_secs_f64() * 1000.));

                if show_body {
                    preview(&body, &emit);
                }
            }
        }

        emit.progress(n + 1, count);
        publish(&emit, sent, ok, &times, start);
    }

    publish(&emit, sent, ok, &times, start);
    let failed = sent - ok;
    let message = format!(
        "{sent} request(s) in {}, {ok} answered with success",
        elapsed(start.elapsed())
    );
    if sent == 0 {
        emit.warn("nothing was sent");
    } else if failed > 0 {
        emit.warn(message);
    } else {
        emit.good(message);
    }
    Ok(())
}

fn publish(emit: &Emitter, sent: usize, ok: usize, times: &[f64], start: Instant) {
    let mut stats = vec![
        kv("sent", sent.to_string()),
        kv("ok", ok.to_string()),
        kv("elapsed", elapsed(start.elapsed())),
    ];
    if !times.is_empty() {
        let average = times.iter().sum::<f64>() / times.len() as f64;
        let slowest = times.iter().cloned().fold(f64::MIN, f64::max);
        stats.push(kv("avg", format!("{average:.1} ms")));
        stats.push(kv("max", format!("{slowest:.1} ms")));
    }
    emit.stats(stats);
}

fn header(response: &reqwest::Response, name: reqwest::header::HeaderName) -> String {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// A content type without its parameters: `text/html` rather than
/// `text/html; charset=utf-8`.
fn short(kind: &str) -> String {
    kind.split(';').next().unwrap_or(kind).trim().to_string()
}

/// The start of the body, as lines in the log.
fn preview(body: &[u8], emit: &Emitter) {
    let text = String::from_utf8_lossy(&body[..body.len().min(BODY_PREVIEW)]);
    if text.trim().is_empty() {
        emit.info("the body is empty");
        return;
    }
    for line in text.lines().take(40) {
        emit.info(line.to_string());
    }
    if body.len() > BODY_PREVIEW {
        emit.info(format!("… and {} more", crate::dl::names::bytes((body.len() - BODY_PREVIEW) as u64)));
    }
}

/// What went wrong, in a few words rather than a chain of causes.
fn why(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "timed out".into();
    }
    if e.is_connect() {
        return "cannot connect".into();
    }
    if e.is_redirect() {
        return "too many redirects".into();
    }
    let mut source: &dyn std::error::Error = e;
    while let Some(next) = source.source() {
        source = next;
    }
    source.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_are_read_however_they_are_separated() {
        let (headers, bad) = parse_headers("Accept: application/json; X-Token: abc");
        assert_eq!(
            headers,
            vec![
                ("Accept".to_string(), "application/json".to_string()),
                ("X-Token".to_string(), "abc".to_string()),
            ]
        );
        assert!(bad.is_empty());

        // Pasted out of somewhere, one per line, with a value that has its own
        // colon in it.
        let (headers, _) = parse_headers("Host: example.com\nReferer: https://example.com/a");
        assert_eq!(headers[1].1, "https://example.com/a");
    }

    #[test]
    fn something_that_is_not_a_header_is_reported_rather_than_sent() {
        let (headers, bad) = parse_headers("Accept: text/html; nonsense");
        assert_eq!(headers.len(), 1);
        assert_eq!(bad, vec!["nonsense".to_string()]);
    }

    #[test]
    fn a_url_is_taken_the_way_it_is_spoken() {
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url(" example.com/a "), "https://example.com/a");
        // Anything that says its own scheme keeps it.
        assert_eq!(normalize_url("http://example.com"), "http://example.com");
    }

    #[test]
    fn the_status_says_how_the_row_is_read() {
        assert_eq!(status_of(200), Status::Up);
        assert_eq!(status_of(301), Status::Info);
        assert_eq!(status_of(404), Status::Warn);
        assert_eq!(status_of(500), Status::Down);
    }

    #[test]
    fn a_content_type_is_shown_without_its_parameters() {
        assert_eq!(short("text/html; charset=utf-8"), "text/html");
        assert_eq!(short("application/json"), "application/json");
    }
}
