//! Sends an HTTP request and reads what comes back.
//!
//! The question a network tool is asked about a web service is rarely "is the
//! port open" — it is "what does it answer, how fast, is that what it should
//! answer, and is it still answering it". So this is a request tool rather
//! than a fetcher: one row per request, the status and the timing in the
//! table, the headers and the start of the body in the log, a graph of the
//! response time, and two things that turn it from a request into a check —
//!
//! - **Expect**, which says what a good answer looks like, so a row that is
//!   not one is coloured as a failure rather than left to be read.
//! - **Capture**, which pulls one value out of every response — a field of the
//!   JSON, a header, the body itself — into its own column. A number captured
//!   that way is graphed beside the response time, and a workflow can keep it:
//!   `set queue = "Status page".value.last()`.

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
        "Send a request, check what comes back, and pull a value out of it"
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
                "query",
                "Query",
                "Added to the URL, and escaped for you: name=value, separated by &",
            )
            .placeholder("since=1h&format=json"),
            Field::text(
                "headers",
                "Headers",
                "Sent with the request. Separate them with a semicolon: Accept: application/json; X-Token: abc",
            )
            .placeholder("Accept: application/json"),
            Field::select(
                "authkind",
                "Authentication",
                "How to prove who is asking",
                "none",
                vec![
                    Opt::new("none", "none", "no credentials"),
                    Opt::new("basic", "Basic", "a user and a password"),
                    Opt::new("bearer", "Bearer", "a token"),
                ],
            ),
            Field::text("auth", "Credentials", "user:password for Basic, or the token for Bearer")
                .placeholder("user:password")
                .visible_if(VisibleIf::NotEquals("authkind", "none")),
            Field::select(
                "bodytype",
                "Body type",
                "What the body is, which is also the content type it is sent with",
                "text",
                vec![
                    Opt::new("text", "text", "sent as it is written"),
                    Opt::new("json", "JSON", "application/json"),
                    Opt::new("form", "form", "application/x-www-form-urlencoded"),
                ],
            )
            .visible_if(VisibleIf::NotEquals("method", "GET")),
            Field::text("body", "Body", "Sent as the request body")
                .placeholder("{\"ok\": true}")
                .visible_if(VisibleIf::NotEquals("method", "GET")),
            Field::text(
                "expect",
                "Expect",
                "What a good answer is: any, 2xx, 404, 200-204, or a list. Anything else is a failure",
            )
            .default("2xx")
            .validate(Validator::Expect),
            Field::text(
                "capture",
                "Capture",
                "One value out of every answer, into its own column: a field of the JSON (data.queue, items[0].id), header:Name, status, or body",
            )
            .placeholder("data.queue"),
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
            Field::text("proxy", "Proxy", "Ask through this instead; empty uses the system's")
                .placeholder("http://proxy.example.com:8080"),
            Field::boolean("follow", "Follow redirects", "Ask again wherever it points", true),
            Field::boolean(
                "insecure",
                "Accept any certificate",
                "Do not check the certificate — for an appliance with a self-signed one",
                false,
            ),
            Field::boolean(
                "http1",
                "HTTP/1.1 only",
                "Do not offer HTTP/2, for a server that answers one badly",
                false,
            ),
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
            col("TYPE", 20),
            col("VALUE", 16),
            col("DETAIL", 0),
        ]
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

/// A URL typed the way it is spoken.
///
/// A name means `https://` — that is what the web is now. An address does
/// not: the machine at `10.0.0.1:8080` is on this network, and a box on this
/// network with a certificate anyone trusts is the exception rather than the
/// rule. Either way, anything that says its own scheme keeps it.
fn normalize_url(raw: &str) -> String {
    let raw = raw.trim();
    if raw.contains("://") {
        return raw.to_string();
    }
    let host = raw.split(['/', '?', '#']).next().unwrap_or(raw);
    // `[::1]:8080`, `10.0.0.1:8080`, `example.com:8080`.
    let bare = match host.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
        Some((inside, _)) => inside,
        None => match host.rsplit_once(':') {
            Some((before, port)) if port.chars().all(|c| c.is_ascii_digit()) => before,
            _ => host,
        },
    };
    let literal =
        bare.parse::<std::net::IpAddr>().is_ok() || bare.eq_ignore_ascii_case("localhost");
    if literal { format!("http://{raw}") } else { format!("https://{raw}") }
}

/// Adds `name=value&…` to a URL, escaping what needs it and keeping whatever
/// query the URL already had.
fn with_query(url: &str, query: &str) -> Result<String, String> {
    let mut parsed = url::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
    let query = query.trim();
    if !query.is_empty() {
        let mut pairs = parsed.query_pairs_mut();
        for piece in query.split('&') {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            match piece.split_once('=') {
                Some((name, value)) => pairs.append_pair(name.trim(), value.trim()),
                None => pairs.append_key_only(piece),
            };
        }
    }
    Ok(parsed.to_string())
}

/// Whether a status is the kind of answer that was asked for.
///
/// `any`, a family — `2xx` — a number, a range, or a list of those.
fn expected(code: u16, spec: &str) -> bool {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("any") {
        return true;
    }
    spec.split([',', ' ']).filter(|p| !p.trim().is_empty()).any(|piece| {
        let piece = piece.trim();
        if let Some(family) = piece.strip_suffix("xx").or_else(|| piece.strip_suffix("XX")) {
            return family.parse::<u16>().map(|f| code / 100 == f).unwrap_or(false);
        }
        if let Some((low, high)) = piece.split_once('-') {
            return match (low.trim().parse::<u16>(), high.trim().parse::<u16>()) {
                (Ok(low), Ok(high)) => code >= low && code <= high,
                _ => false,
            };
        }
        piece.parse::<u16>().map(|want| want == code).unwrap_or(false)
    })
}

/// Reads what somebody typed into the Expect box, so a typo is refused when
/// the form is submitted rather than silently passing everything.
pub fn valid_expect(spec: &str) -> Result<(), String> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("any") {
        return Ok(());
    }
    for piece in spec.split([',', ' ']).filter(|p| !p.trim().is_empty()) {
        let piece = piece.trim();
        let ok = if let Some(family) = piece.strip_suffix("xx").or_else(|| piece.strip_suffix("XX"))
        {
            matches!(family.parse::<u16>(), Ok(1..=5))
        } else if let Some((low, high)) = piece.split_once('-') {
            matches!(
                (low.trim().parse::<u16>(), high.trim().parse::<u16>()),
                (Ok(low), Ok(high)) if low <= high
            )
        } else {
            piece.parse::<u16>().is_ok()
        };
        if !ok {
            return Err(format!("{piece:?} is not a status: use any, 2xx, 404 or 200-204"));
        }
    }
    Ok(())
}

/// One value out of a response.
///
/// `header:Name` reads a header, `status` the code, `body` the whole of it,
/// and anything else is a path into the JSON: `data.queue`, `items[0].id`.
fn capture(what: &str, code: u16, headers: &reqwest::header::HeaderMap, body: &[u8]) -> String {
    let what = what.trim();
    if what.is_empty() {
        return String::new();
    }
    if let Some(name) = what.strip_prefix("header:") {
        return headers
            .get(name.trim())
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
    }
    if what.eq_ignore_ascii_case("status") {
        return code.to_string();
    }
    if what.eq_ignore_ascii_case("body") {
        return String::from_utf8_lossy(body).trim().chars().take(200).collect();
    }
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) else { return String::new() };
    json_path(&json, what).map(show_json).unwrap_or_default()
}

/// Walks `data.items[0].name` through parsed JSON.
fn json_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut at = value;
    for step in path.split('.') {
        let step = step.trim();
        if step.is_empty() {
            continue;
        }
        // A name, then any number of [n] after it.
        let (name, rest) = match step.find('[') {
            Some(i) => (&step[..i], &step[i..]),
            None => (step, ""),
        };
        if !name.is_empty() {
            at = at.get(name)?;
        }
        let mut rest = rest;
        while let Some(close) = rest.find(']') {
            let index: usize = rest[1..close].trim().parse().ok()?;
            at = at.get(index)?;
            rest = &rest[close + 1..];
        }
    }
    Some(at)
}

/// A JSON value as a person would read it: text without its quotes, anything
/// else as it is written.
fn show_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// What the status line says, and how to colour the row it is on.
fn status_of(code: u16, met: bool) -> Status {
    if !met {
        return Status::Down;
    }
    match code {
        200..=299 => Status::Up,
        300..=399 => Status::Info,
        400..=499 => Status::Warn,
        _ => Status::Down,
    }
}

async fn run(r: crate::core::Run, emit: Emitter) -> anyhow::Result<()> {
    let (cancel, p) = (r.cancel.clone(), r.params.clone());
    let url = with_query(&normalize_url(&p.str("target")), &p.str("query"))
        .map_err(anyhow::Error::msg)?;
    let method = p.str("method");
    let timeout = p.dur("timeout", Duration::from_secs(10));
    let interval = p.dur("interval", Duration::from_secs(1));
    let count = p.usize("count", 1).max(1);
    let follow = p.bool("follow");
    let want = p.str("expect");
    let capturing = p.str("capture");
    let (headers, unreadable) = parse_headers(&p.str("headers"));
    for bad in &unreadable {
        emit.warn(format!("{bad:?} is not a header: they read Name: value"));
    }

    let verb = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| anyhow::anyhow!("{method} is not a method"))?;

    let mut client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(if follow {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        })
        .user_agent(concat!("ntls/", env!("CARGO_PKG_VERSION")));
    if p.bool("insecure") {
        emit.warn("certificates are not being checked");
        client = client.danger_accept_invalid_certs(true);
    }
    if p.bool("http1") {
        client = client.http1_only();
    }
    let proxy = p.str("proxy");
    if !proxy.is_empty() {
        client = client.proxy(reqwest::Proxy::all(normalize_url(&proxy))?);
    }
    let client = client.build()?;

    // A resumed run carries on the numbering rather than starting again.
    let first = r.prior.len();
    let start = Instant::now();
    let (mut sent, mut met) = (0usize, 0usize);
    let mut times: Vec<f64> = Vec::new();
    let mut last_value = String::new();

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
        let body = p.str("body");
        let sends_body =
            !body.is_empty() && verb != reqwest::Method::GET && verb != reqwest::Method::HEAD;
        if sends_body {
            if let Some(kind) = body_type(&p.str("bodytype"))
                && !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            {
                request = request.header(reqwest::header::CONTENT_TYPE, kind);
            }
            request = request.body(body);
        }
        for (name, value) in &headers {
            request = request.header(name.as_str(), value.as_str());
        }
        request = match p.str("authkind").as_str() {
            "basic" => {
                let credentials = p.str("auth");
                let (user, password) = credentials
                    .split_once(':')
                    .map(|(u, p)| (u.to_string(), p.to_string()))
                    .unwrap_or((credentials, String::new()));
                request.basic_auth(user, Some(password))
            }
            "bearer" => request.bearer_auth(p.str("auth")),
            _ => request,
        };

        let index = first + n + 1;
        let began = Instant::now();
        let answered = match cancel.run(request.send()).await {
            None => break,
            Some(answered) => answered,
        };
        sent += 1;

        match answered {
            Err(e) => {
                emit.row(
                    Status::Down,
                    url.clone(),
                    cells![index, "—", ms(began.elapsed()), "—", "—", "", why(&e)],
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
                let headers_back = response.headers().clone();

                if n == 0 && p.bool("showheaders") {
                    emit.info(format!(
                        "{} {} \u{b7} {:?}",
                        code.as_u16(),
                        code.canonical_reason().unwrap_or(""),
                        response.version()
                    ));
                    for (name, value) in &headers_back {
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

                let as_asked = expected(code.as_u16(), &want);
                if as_asked {
                    met += 1;
                }
                let value = capture(&capturing, code.as_u16(), &headers_back, &body);
                if !value.is_empty() {
                    last_value = value.clone();
                }

                // What is worth saying about the answer beyond its status:
                // what was expected instead, where it ended up, or where it is
                // pointing.
                let detail = if !as_asked {
                    format!("expected {want}")
                } else {
                    match (&location, landed != url) {
                        (Some(to), _) if !follow => format!("→ {to}"),
                        (_, true) => format!("→ {landed}"),
                        _ => server.clone(),
                    }
                };

                emit.row(
                    status_of(code.as_u16(), as_asked),
                    url.clone(),
                    cells![
                        index,
                        format!("{}", code.as_u16()),
                        ms(took),
                        crate::dl::names::bytes(body.len() as u64),
                        short(&kind),
                        value.clone(),
                        detail
                    ],
                );
                emit.emit(Event::sample("response", "ms", took.as_secs_f64() * 1000.));
                // A captured number is worth a graph of its own: an endpoint
                // asked every minute for its queue length is a queue graph.
                if let Some(number) = crate::expr::value::leading_number(&value) {
                    emit.emit(Event::sample(capturing.clone(), "", number));
                }

                if show_body {
                    preview(&body, &emit);
                }
            }
        }

        emit.progress(n + 1, count);
        publish(&emit, sent, met, &times, &capturing, &last_value, start);
    }

    publish(&emit, sent, met, &times, &capturing, &last_value, start);
    let missed = sent - met;
    if sent == 0 {
        emit.warn("nothing was sent");
    } else if missed > 0 {
        emit.warn(format!(
            "{missed} of {sent} did not answer {want}, in {}",
            elapsed(start.elapsed())
        ));
    } else {
        emit.good(format!(
            "{sent} request(s) in {}, all answering {want}",
            elapsed(start.elapsed())
        ));
    }
    Ok(())
}

/// What a body is sent as, unless a header already says.
fn body_type(kind: &str) -> Option<&'static str> {
    match kind {
        "json" => Some("application/json"),
        "form" => Some("application/x-www-form-urlencoded"),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn publish(
    emit: &Emitter,
    sent: usize,
    met: usize,
    times: &[f64],
    capturing: &str,
    last_value: &str,
    start: Instant,
) {
    let mut stats = vec![
        kv("sent", sent.to_string()),
        kv("ok", met.to_string()),
        kv("elapsed", elapsed(start.elapsed())),
    ];
    if !times.is_empty() {
        let average = times.iter().sum::<f64>() / times.len() as f64;
        let slowest = times.iter().cloned().fold(f64::MIN, f64::max);
        let fastest = times.iter().cloned().fold(f64::MAX, f64::min);
        stats.push(kv("min", format!("{fastest:.1} ms")));
        stats.push(kv("avg", format!("{average:.1} ms")));
        stats.push(kv("max", format!("{slowest:.1} ms")));
    }
    // The captured value is a figure of this run like any other, so a document
    // can read it by the name it was captured under.
    if !capturing.trim().is_empty() && !last_value.is_empty() {
        stats.push(kv(capturing.trim(), last_value));
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
        emit.info(format!(
            "… and {} more",
            crate::dl::names::bytes((body.len() - BODY_PREVIEW) as u64)
        ));
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
        assert_eq!(normalize_url("example.com:8080/a"), "https://example.com:8080/a");
        // A machine on this network is asked in plain HTTP, because that is
        // what a machine on this network almost always answers.
        assert_eq!(normalize_url("10.0.0.1"), "http://10.0.0.1");
        assert_eq!(normalize_url("127.0.0.1:8099/health"), "http://127.0.0.1:8099/health");
        assert_eq!(normalize_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url("[::1]:8080/a"), "http://[::1]:8080/a");
        // Anything that says its own scheme keeps it.
        assert_eq!(normalize_url("http://example.com"), "http://example.com");
        assert_eq!(normalize_url("https://10.0.0.1"), "https://10.0.0.1");
    }

    #[test]
    fn a_query_is_added_and_escaped_without_losing_what_was_there() {
        assert_eq!(
            with_query("https://example.com/a?x=1", "q=two words&flag").unwrap(),
            "https://example.com/a?x=1&q=two+words&flag"
        );
        assert_eq!(with_query("https://example.com", "").unwrap(), "https://example.com/");
        assert!(with_query("not a url", "").is_err());
    }

    #[test]
    fn what_counts_as_a_good_answer_is_said_the_way_people_say_it() {
        assert!(expected(200, "2xx"));
        assert!(expected(204, "2xx"));
        assert!(!expected(500, "2xx"));
        assert!(expected(404, "404"));
        assert!(expected(201, "200-204"));
        assert!(!expected(205, "200-204"));
        assert!(expected(301, "2xx, 301"));
        assert!(expected(500, "any"));
        assert!(expected(500, ""));

        assert!(valid_expect("2xx, 301").is_ok());
        assert!(valid_expect("200-204").is_ok());
        assert!(valid_expect("nonsense").is_err());
        assert!(valid_expect("9xx").is_err());
    }

    #[test]
    fn one_value_is_pulled_out_of_the_answer() {
        let body = br#"{"data": {"queue": 12, "name": "north"}, "items": [{"id": "a"}, {"id": "b"}]}"#;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-request-id", "abc123".parse().unwrap());

        assert_eq!(capture("data.queue", 200, &headers, body), "12");
        // Text comes back without its quotes, so it reads as a value.
        assert_eq!(capture("data.name", 200, &headers, body), "north");
        assert_eq!(capture("items[1].id", 200, &headers, body), "b");
        assert_eq!(capture("header:x-request-id", 200, &headers, body), "abc123");
        assert_eq!(capture("status", 503, &headers, body), "503");
        // A path that is not there is nothing, not an error: the request
        // itself was still an answer.
        assert_eq!(capture("data.missing", 200, &headers, body), "");
        assert_eq!(capture("", 200, &headers, body), "");
    }

    #[test]
    fn a_row_is_a_failure_when_it_is_not_what_was_asked_for() {
        // The status alone does not decide it: a 404 that was expected is the
        // answer, and a 200 that was not is a failure.
        assert_eq!(status_of(200, true), Status::Up);
        assert_eq!(status_of(404, true), Status::Warn);
        assert_eq!(status_of(200, false), Status::Down);
        assert_eq!(status_of(301, true), Status::Info);
    }

    #[test]
    fn a_content_type_is_shown_without_its_parameters() {
        assert_eq!(short("text/html; charset=utf-8"), "text/html");
        assert_eq!(short("application/json"), "application/json");
    }

    #[test]
    fn a_body_is_sent_as_what_it_is() {
        assert_eq!(body_type("json"), Some("application/json"));
        assert_eq!(body_type("form"), Some("application/x-www-form-urlencoded"));
        assert_eq!(body_type("text"), None);
    }
}
