//! Sends an HTTP request and reads what comes back.
//!
//! The question a network tool is asked about a web service is rarely "is the
//! port open". It is "what does it answer, how fast, is that what it should
//! answer, and is it still answering it". So this is a request tool rather
//! than a fetcher: one row per request, the status and the timing in the
//! table, the headers and the start of the body in the log, a graph of the
//! response time, and two things that turn a request into a check:
//!
//! - **Expect**, which says what a good answer looks like, so a row that is
//!   not one is coloured as a failure and not left to be read.
//! - **Capture**, which pulls one value out of every response (a field of the
//!   JSON, a header, the body itself) into its own column. A number captured
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

/// How much of the body is held on to: enough for the log, and for a value
/// captured out of the JSON of any answer a service is meant to give.
///
/// The whole of it used to be kept, which is fine for an answer and not for
/// what a URL sometimes turns out to point at: a disk image, or an endpoint
/// that streams for as long as it is read. That grew in memory for as long as
/// the timeout allowed and was then thrown away entirely, on a machine that
/// is also running everything else the person is diagnosing.
const BODY_KEEP: usize = 8 * 1024 * 1024;

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

    /// A `curl` line pasted into the URL box becomes the whole request.
    ///
    /// Which is how an HTTP request is handed round: in an issue, in a
    /// service's own documentation, off a browser's "copy as cURL". Reading
    /// it here saves retyping five fields out of one line.
    fn absorb(&self, pasted: &str) -> Option<Vec<(&'static str, String)>> {
        let asked = crate::tools::curl::parse(pasted)?;
        let mut out = vec![("target", asked.url)];
        if let Some(method) = asked.method {
            // Only the methods the form offers; anything else is left as it
            // was rather than putting a word in the box that is not a choice.
            if METHODS.contains(&method.as_str()) {
                out.push(("method", method));
            }
        }
        if !asked.headers.is_empty() {
            out.push(("headers", asked.headers.join("\n")));
        }
        if let Some(body) = asked.body {
            out.push(("body", body));
        }
        if let Some(kind) = asked.body_kind {
            out.push(("bodytype", kind.to_string()));
        }
        if let Some((kind, value)) = asked.auth {
            out.push(("authkind", kind.to_string()));
            out.push(("auth", value));
        }
        // Only when it says so: a switch curl was not given should not turn
        // one off that the form already has on.
        if asked.insecure {
            out.push(("insecure", "true".into()));
        }
        if asked.follow {
            out.push(("follow", "true".into()));
        }
        if asked.http1 {
            out.push(("http1", "true".into()));
        }
        Some(out)
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            Field::text(
                "target",
                "URL",
                "What to ask, in full. Paste a curl line here and the rest of the form fills itself",
            )
                .placeholder("https://example.com/health")
                .role(Role::Target)
                .validate(Validator::Required),
            // Seven choices, so a row of its own: in the grid's column each
            // one gets thirty pixels and reads as a stub.
            Field::select(
                "method",
                "Method",
                "Which request to send",
                "GET",
                METHODS.iter().map(|m| Opt::new(m, m, method_desc(m))).collect(),
            )
            .wide(),
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
                "What the body is, and the content type it is sent with",
                "text",
                vec![
                    Opt::new("text", "text", "sent as written, with no content type added"),
                    Opt::new("json", "JSON", "application/json"),
                    Opt::new("xml", "XML", "application/xml"),
                    Opt::new("form", "form", "application/x-www-form-urlencoded"),
                ],
            )
            .visible_if(VisibleIf::NoneOf("method", &["GET", "HEAD"])),
            // A payload is several lines of JSON or XML far more often than
            // it is one line of anything, so it gets an editor and not a box:
            // room to type in, line breaks that stay, and colouring that
            // follows whatever the body type above it is set to.
            Field::code("body", "Body", "Sent as the request body", "bodytype")
                .placeholder("{\"ok\": true}")
                .visible_if(VisibleIf::NoneOf("method", &["GET", "HEAD"])),
            Field::text(
                "attach",
                "Attachments",
                "Files to send with it, one per line: a path, or name=path to choose what the part is called. Sending any makes the request multipart/form-data",
            )
            .placeholder("/tmp/report.pdf")
            .wide()
            .visible_if(VisibleIf::NoneOf("method", &["GET", "HEAD"])),
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
                .validate(Validator::Duration).advanced(),
            Field::text("proxy", "Proxy", "Ask through this instead; empty uses the system's")
                .placeholder("http://proxy.example.com:8080").advanced(),
            Field::boolean("follow", "Follow redirects", "Ask again wherever it points", true),
            Field::boolean(
                "insecure",
                "Accept any certificate",
                "Do not check the certificate, for an appliance with a self-signed one",
                false,
            ).advanced(),
            Field::boolean(
                "http1",
                "HTTP/1.1 only",
                "Do not offer HTTP/2, for a server that answers one badly",
                false,
            ).advanced(),
            Field::boolean(
                "showheaders",
                "Log the headers",
                "Write the response headers into the output panel",
                true,
            ).advanced(),
            Field::boolean(
                "showbody",
                "Log the body",
                "Write the start of the response body into the output panel",
                false,
            ).advanced(),
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
/// A name means `https://`, which the web now is. An address does
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
/// `any`, a family such as `2xx`, a number, a range, or a list of those.
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
/// the form is submitted, instead of silently passing everything.
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
        // Each index is read from its opening bracket, because whatever
        // follows an index has to be another index. Looking for the next `]`
        // by itself and reading the number from one byte in went wrong the
        // moment the brackets did not pair: `items[0]]` sliced `1..0` of a
        // lone `]` and `items[0]é]` cut a character in half, and either one
        // panicked. A path is typed by hand into a box, so a path like that
        // arrives, and the panic killed the request without the run ever
        // reporting that it had finished.
        let mut rest = rest;
        while !rest.is_empty() {
            let inside = rest.strip_prefix('[')?;
            let close = inside.find(']')?;
            let index: usize = inside[..close].trim().parse().ok()?;
            at = at.get(index)?;
            rest = &inside[close + 1..];
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
    // Read once, and re-read from disk on every request: a file that changes
    // between two of them is meant to.
    let attachments = parse_attachments(&p.str("attach"));
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
        .user_agent(format!("ntls/{}", crate::sys::build_tag()));
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

    // A resumed run carries on the numbering and does not start again.
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
        let carries = verb != reqwest::Method::GET && verb != reqwest::Method::HEAD;

        if carries && !attachments.is_empty() {
            // Files make it a form upload, and the form sets its own content
            // type with the boundary in it, so nothing else may.
            match multipart_form(&body, &p.str("bodytype"), &attachments) {
                Ok(form) => request = request.multipart(form),
                Err(e) => {
                    emit.err(format!("{method} {url}: {e}"));
                    // The files are the request. Sending it without them
                    // would be asking a different question, and doing so
                    // sixty times over would be worse.
                    break;
                }
            }
        } else if carries && !body.is_empty() {
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
            Ok(mut response) => {
                // `send` comes back when the headers do, so this is
                // everything up to the first byte: name resolution, the
                // connection, the handshake and the far end's own thinking.
                let waited = began.elapsed();
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
                let body = match cancel.run(read_body(&mut response)).await {
                    None => break,
                    Some(Ok(body)) => body,
                    Some(Err(e)) => {
                        emit.warn(format!("the body did not arrive whole: {}", why(&e)));
                        Body::default()
                    }
                };
                // Said once, because an empty captured value out of an answer
                // this big is the cap and not the path.
                if n == 0 && !body.whole() {
                    emit.warn(format!(
                        "the body is {}: only the first {} of it was kept",
                        crate::dl::names::bytes(body.len),
                        crate::dl::names::bytes(BODY_KEEP as u64)
                    ));
                }
                let took = began.elapsed();
                times.push(took.as_secs_f64() * 1000.);

                let as_asked = expected(code.as_u16(), &want);
                if as_asked {
                    met += 1;
                }
                let value = capture(&capturing, code.as_u16(), &headers_back, &body.kept);
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
                        crate::dl::names::bytes(body.len),
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

                // The whole answer is kept, separately from the row, so that
                // it can be read in the response pane and so that a document
                // or a workflow can reach any part of it rather than only the
                // one value this run was told to capture.
                emit.emit(Event::Answered(crate::core::Answer {
                    status: code.as_u16(),
                    reason: code.canonical_reason().unwrap_or_default().to_string(),
                    content_type: short(&kind),
                    headers: headers_back
                        .iter()
                        .map(|(name, value)| {
                            (name.to_string(), value.to_str().unwrap_or_default().to_string())
                        })
                        .collect(),
                    // Lossily on purpose: an answer that is not text is
                    // shown as the bytes that could be read as text, which
                    // is more use than showing nothing at all.
                    body: String::from_utf8_lossy(&body.kept).into_owned(),
                    truncated: !body.whole(),
                    waited: waited.as_secs_f64() * 1000.,
                    read: took.saturating_sub(waited).as_secs_f64() * 1000.,
                    method: method.to_string(),
                    url: url.clone(),
                    landed: if landed == url { String::new() } else { landed.clone() },
                }));
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

/// How much of an upload is held in memory at once.
///
/// Every attachment is read whole before the request is sent, because a part
/// has to know its own length. Something larger than this is not an
/// attachment on a diagnostic request, it is a transfer, and the download
/// tool is the one that moves files.
const MOST_ATTACHED: u64 = 64 * 1024 * 1024;

/// One attachment: what the part is called, and the file it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub path: std::path::PathBuf,
}

/// The files to send with a request.
///
/// One per line, or separated by semicolons, because a list of paths is as
/// often pasted as typed. A bare path takes its part name from the file, and
/// `name=path` says what to call it instead, which is what an API asking for
/// a field called `avatar` needs.
///
/// A Windows path has a colon in it and a name may not, so the split is on
/// the first `=` and nothing else.
pub fn parse_attachments(spec: &str) -> Vec<Attachment> {
    spec.split(['\n', ';'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| match line.split_once('=') {
            Some((name, path)) if !name.trim().is_empty() && !path.trim().is_empty() => {
                Attachment { name: name.trim().to_string(), path: path.trim().into() }
            }
            _ => Attachment { name: part_name(line), path: std::path::PathBuf::from(line) },
        })
        .collect()
}

/// What to call the part holding a file, when nobody said.
///
/// The file's own name without its extension. Both separators are cut on,
/// not just this machine's: a Windows path typed on Windows is what the
/// person is sending, and `std::path` on a Unix machine does not know that
/// `\` divides one, so the whole path became the name of the part.
fn part_name(path: &str) -> String {
    let last = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = last.rsplit_once('.').map_or(last, |(head, _)| head);
    let stem = stem.trim();
    if stem.is_empty() { "file".to_string() } else { stem.to_string() }
}

/// The multipart body for a request that carries files.
///
/// The attachments become one part each. What else goes in depends on the
/// body type: a form body is already a list of names and values, so its pairs
/// become text parts beside the files, which is what an API expects of a form
/// upload. A JSON or XML body has no such shape, so the whole of it goes in as
/// one part called `body`, carrying its own content type.
fn multipart_form(
    body: &str,
    kind: &str,
    attachments: &[Attachment],
) -> Result<reqwest::multipart::Form, String> {
    let mut form = reqwest::multipart::Form::new();

    if !body.is_empty() {
        if kind == "form" {
            for pair in body.split('&').filter(|p| !p.is_empty()) {
                let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                form = form.text(name.to_string(), value.to_string());
            }
        } else {
            let mut part = reqwest::multipart::Part::text(body.to_string());
            if let Some(content) = body_type(kind) {
                part = part.mime_str(content).map_err(|e| e.to_string())?;
            }
            form = form.part("body", part);
        }
    }

    let mut total = 0u64;
    for attachment in attachments {
        let shown = attachment.path.display();
        let size = std::fs::metadata(&attachment.path)
            .map_err(|e| format!("cannot read {shown}: {e}"))?
            .len();
        total += size;
        if total > MOST_ATTACHED {
            return Err(format!(
                "the attachments come to more than {}, which is a transfer and not a request",
                crate::dl::names::bytes(MOST_ATTACHED)
            ));
        }
        let bytes = std::fs::read(&attachment.path)
            .map_err(|e| format!("cannot read {shown}: {e}"))?;
        let filename = attachment
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| attachment.name.clone());
        let guessed = mime_guess::from_path(&attachment.path).first_or_octet_stream();

        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename)
            .mime_str(guessed.essence_str())
            .map_err(|e| e.to_string())?;
        form = form.part(attachment.name.clone(), part);
    }
    Ok(form)
}

/// What a body is sent as, unless a header already says.
fn body_type(kind: &str) -> Option<&'static str> {
    match kind {
        "json" => Some("application/json"),
        "xml" => Some("application/xml"),
        "form" => Some("application/x-www-form-urlencoded"),
        // `text` deliberately sends no content type at all, so that a header
        // of your own is the only thing that decides. Adding one here would
        // quietly overrule the answer for anybody relying on that.
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

/// A content type without its parameters: `text/html`, not
/// `text/html; charset=utf-8`.
fn short(kind: &str) -> String {
    kind.split(';').next().unwrap_or(kind).trim().to_string()
}

/// What came back, and how much of it there was.
///
/// The two are not the same number: the row says the size the server sent,
/// and only the start of it is held in memory.
#[derive(Default)]
struct Body {
    /// The first [`BODY_KEEP`] of the body, which is what the log shows and
    /// what a value is captured out of.
    kept: Vec<u8>,
    /// How long the whole body was, kept or not.
    len: u64,
}

impl Body {
    fn take(&mut self, chunk: &[u8]) {
        self.len += chunk.len() as u64;
        let room = BODY_KEEP.saturating_sub(self.kept.len());
        if room > 0 {
            self.kept.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
    }

    /// Whether what is kept is all of it.
    fn whole(&self) -> bool {
        self.len <= BODY_KEEP as u64
    }
}

/// Reads the body to the end, holding on to the start of it.
///
/// It is read to the end rather than dropped at the cap so that the size in
/// the row, and the timing beside it, are still about the whole answer.
async fn read_body(response: &mut reqwest::Response) -> reqwest::Result<Body> {
    let mut body = Body::default();
    while let Some(chunk) = response.chunk().await? {
        body.take(&chunk);
    }
    Ok(body)
}

/// The start of the body, as lines in the log.
fn preview(body: &Body, emit: &Emitter) {
    let text = String::from_utf8_lossy(&body.kept[..body.kept.len().min(BODY_PREVIEW)]);
    if text.trim().is_empty() {
        emit.info("the body is empty");
        return;
    }
    for line in text.lines().take(40) {
        emit.info(line.to_string());
    }
    // Counted off the whole body and not off what was kept of it, so the line
    // is about how much there is left to read rather than how much of it this
    // tool happens to be holding.
    if body.len > BODY_PREVIEW as u64 {
        emit.info(format!(
            "… and {} more",
            crate::dl::names::bytes(body.len - BODY_PREVIEW as u64)
        ));
    }
}

/// What went wrong, in a few words instead of a chain of causes.
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
    fn a_curl_line_pasted_in_becomes_the_whole_request() {
        let filled = Http
            .absorb(
                "curl -X POST 'https://api.example.com/v1/things' \
                 -H 'Accept: application/json' \
                 -H 'Authorization: Bearer tok' \
                 -k -L --data-raw '{\"a\": 1}'",
            )
            .expect("a curl line");
        let of = |key: &str| {
            filled.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
        };

        assert_eq!(of("target"), Some("https://api.example.com/v1/things"));
        assert_eq!(of("method"), Some("POST"));
        assert_eq!(of("headers"), Some("Accept: application/json"));
        assert_eq!(of("body"), Some("{\"a\": 1}"));
        assert_eq!(of("bodytype"), Some("json"));
        // The bearer token became the credential rather than staying a
        // header nobody would think to look in.
        assert_eq!(of("authkind"), Some("bearer"));
        assert_eq!(of("auth"), Some("tok"));
        assert_eq!(of("insecure"), Some("true"));
        assert_eq!(of("follow"), Some("true"));

        // Every key it names has to be a field this tool actually has, or
        // the paste would quietly fill nothing.
        let keys: Vec<&str> = Http.fields().iter().map(|f| f.key).collect();
        for (key, _) in &filled {
            assert!(keys.contains(key), "{key} is not a field of the http tool");
        }
    }

    #[test]
    fn a_switch_the_line_does_not_give_is_left_as_it_was() {
        // Curl without `-k` says nothing about certificates; it must not
        // turn off a switch the form already has on.
        let filled = Http.absorb("curl https://example.com").expect("a curl line");
        assert!(!filled.iter().any(|(k, _)| *k == "insecure"), "{filled:?}");
        assert!(!filled.iter().any(|(k, _)| *k == "follow"), "{filled:?}");
        // And a method the form does not offer is not written into the box.
        let filled = Http.absorb("curl -X PROPFIND https://example.com").expect("a curl line");
        assert!(!filled.iter().any(|(k, _)| *k == "method"), "{filled:?}");
    }

    #[test]
    fn anything_that_is_not_a_curl_line_is_not_absorbed() {
        assert!(Http.absorb("https://example.com").is_none());
        assert!(Http.absorb("").is_none());
    }

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
    fn a_capture_path_whose_brackets_do_not_pair_is_nothing_rather_than_a_panic() {
        let body = br#"{"items": [{"id": "a"}], "data": {"queue": 12}}"#;
        let headers = reqwest::header::HeaderMap::new();

        // A path is typed by hand, so it arrives half-finished and it arrives
        // wrong. None of these is a path, and none of them may take the run
        // down with it.
        assert_eq!(capture("items[0]]", 200, &headers, body), "");
        assert_eq!(capture("items[0]]x", 200, &headers, body), "");
        assert_eq!(capture("items[0]é]", 200, &headers, body), "");
        assert_eq!(capture("items[]", 200, &headers, body), "");
        assert_eq!(capture("items[0", 200, &headers, body), "");
        assert_eq!(capture("items]", 200, &headers, body), "");

        // And one that is one still reads.
        assert_eq!(capture("items[0].id", 200, &headers, body), "a");
        assert_eq!(capture("data.queue", 200, &headers, body), "12");
    }

    #[test]
    fn a_body_past_what_is_kept_is_counted_in_full_and_held_in_part() {
        let mut body = Body::default();
        body.take(b"{\"queue\": 12}");
        assert!(body.whole());
        assert_eq!(body.len, 13);
        assert_eq!(body.kept, b"{\"queue\": 12}");

        // What a URL sometimes turns out to point at. The size is the size
        // that arrived; the memory is not.
        let mut body = Body::default();
        let chunk = vec![b'x'; BODY_KEEP / 2 + 1];
        for _ in 0..4 {
            body.take(&chunk);
        }
        assert_eq!(body.len, 4 * (BODY_KEEP / 2 + 1) as u64);
        assert_eq!(body.kept.len(), BODY_KEEP);
        assert!(!body.whole());
    }

    #[test]
    fn the_log_says_how_much_of_the_body_it_is_not_showing() {
        // The start of something far larger, which is what a capped read
        // leaves behind.
        let body = Body { kept: b"first line\nsecond line\n".to_vec(), len: 5_000_000_000 };
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = std::sync::Arc::clone(&lines);
        preview(
            &body,
            &Emitter::new(move |e: Event| {
                if let Event::Log { text, .. } = e {
                    sink.lock().unwrap().push(text);
                }
            }),
        );
        let logged = lines.lock().unwrap();
        assert_eq!(logged[0], "first line");
        assert_eq!(logged[1], "second line");
        // The remainder is what is left of the answer, not what is left of
        // the 23 bytes that were kept of it.
        assert_eq!(logged[2], "… and 4.7 GB more");
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
    fn attachments_are_read_one_per_line_and_named_after_their_file() {
        use std::path::PathBuf;
        let found = parse_attachments("/tmp/report.pdf\n avatar=/tmp/me.png ;/tmp/a.txt\n\n");
        assert_eq!(
            found,
            vec![
                Attachment { name: "report".into(), path: PathBuf::from("/tmp/report.pdf") },
                Attachment { name: "avatar".into(), path: PathBuf::from("/tmp/me.png") },
                Attachment { name: "a".into(), path: PathBuf::from("/tmp/a.txt") },
            ]
        );
        assert!(parse_attachments("   \n \n").is_empty());
    }

    #[test]
    fn a_windows_path_is_not_split_on_its_drive_letter() {
        use std::path::PathBuf;
        // The split is on `=` and nothing else, so a colon in a path is a
        // colon in a path.
        let found = parse_attachments(r"C:\reports\q3.pdf");
        assert_eq!(
            found,
            vec![Attachment { name: "q3".into(), path: PathBuf::from(r"C:\reports\q3.pdf") }]
        );
        // And a name given explicitly still wins.
        let named = parse_attachments(r"doc=C:\reports\q3.pdf");
        assert_eq!(named[0].name, "doc");
    }

    #[test]
    fn an_attachment_that_is_not_there_is_reported_and_not_sent_empty() {
        let missing =
            vec![Attachment { name: "a".into(), path: "/nowhere/at/all.bin".into() }];
        let failed = multipart_form("", "json", &missing).expect_err("it to refuse");
        assert!(failed.contains("cannot read"), "{failed}");
    }

    #[test]
    fn a_form_body_beside_files_becomes_the_fields_of_the_upload() {
        // A form body is already a list of names and values, so its pairs
        // belong beside the files rather than inside a part of their own.
        let dir = std::env::temp_dir().join("ntls-attach-test");
        std::fs::create_dir_all(&dir).expect("the directory");
        let file = dir.join("note.txt");
        std::fs::write(&file, b"hello").expect("a file");

        let attached = vec![Attachment { name: "note".into(), path: file }];
        assert!(multipart_form("a=1&b=2", "form", &attached).is_ok());
        // And a JSON body, which has no such shape, goes in whole.
        assert!(multipart_form(r#"{"a":1}"#, "json", &attached).is_ok());
        // With nothing to send beside them, the files are the whole of it.
        assert!(multipart_form("", "json", &attached).is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_body_is_sent_as_what_it_is() {
        assert_eq!(body_type("json"), Some("application/json"));
        assert_eq!(body_type("form"), Some("application/x-www-form-urlencoded"));
        assert_eq!(body_type("text"), None);
    }
}
