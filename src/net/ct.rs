//! Subdomains from certificate transparency logs.
//!
//! Every certificate a public CA issues is published to append-only logs, and
//! a certificate names the hosts it covers. That makes the logs a passive
//! inventory of a domain's subdomains: no scanning, no wordlist, and it turns
//! up names that never resolve publicly or that DNS will not let you walk.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use serde::Deserialize;

/// Caps how many distinct names one query keeps. A large estate can have tens
/// of thousands, and past this the listing stops being readable long before it
/// stops being expensive.
pub const MAX_NAMES: usize = 20_000;

/// Bounds the response we are willing to read. crt.sh answers for a busy
/// domain run to tens of megabytes; this stops a pathological one from eating
/// the process.
const MAX_BODY: usize = 192 << 20;

/// A calendar date, all the precision these listings need.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Date {
    /// Days since the Unix epoch. Zero means "not known".
    pub days: i64,
}

impl Date {
    pub fn is_zero(&self) -> bool {
        self.days == 0
    }

    pub fn today() -> Date {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Date { days: secs / 86_400 }
    }

    /// Renders as `YYYY-MM-DD`, or "-" when unknown.
    pub fn format(&self) -> String {
        if self.is_zero() {
            return "-".into();
        }
        let (y, m, d) = civil_from_days(self.days);
        format!("{y:04}-{m:02}-{d:02}")
    }
}

/// Howard Hinnant's days-to-calendar algorithm: exact and branch-free
/// over the whole proleptic Gregorian range.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// One hostname seen in the logs, aggregated over every certificate that
/// named it.
#[derive(Clone, Debug, Default)]
pub struct Name {
    pub name: String,
    pub certs: usize,
    /// The earliest and latest issuance dates across those certificates.
    pub first_seen: Date,
    pub last_seen: Date,
    /// The latest expiry among them, the one that decides whether the
    /// name still has a certificate covering it today.
    pub expires: Date,
    /// The CA behind the most recently issued of them.
    pub issuer: String,
    /// Reports a name of the form `*.example.com`.
    pub wildcard: bool,
}

impl Name {
    /// Reports whether every certificate naming this host has lapsed.
    pub fn expired(&self) -> bool {
        !self.expires.is_zero() && self.expires < Date::today()
    }

    /// The name to hand to another tool. A wildcard is not a host you can
    /// resolve, so it stands in for the parent it covers.
    pub fn host(&self) -> &str {
        self.name.strip_prefix("*.").unwrap_or(&self.name)
    }
}

/// Names a certificate transparency aggregator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// Tries crt.sh and falls back to certspotter, which matters because
    /// crt.sh is regularly overloaded.
    Auto,
    CrtSh,
    CertSpotter,
}

impl Source {
    pub fn parse(s: &str) -> Source {
        match s {
            "crt.sh" => Source::CrtSh,
            "certspotter" => Source::CertSpotter,
            _ => Source::Auto,
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Source::Auto => "auto",
            Source::CrtSh => "crt.sh",
            Source::CertSpotter => "certspotter",
        }
    }

    pub fn desc(&self) -> &'static str {
        match self {
            Source::Auto => "crt.sh, falling back to certspotter if it is down",
            Source::CrtSh => "crt.sh; the most complete, and the most often overloaded",
            Source::CertSpotter => "certspotter; paged and rate limited, but reliable",
        }
    }
}

pub const SOURCES: &[Source] = &[Source::Auto, Source::CrtSh, Source::CertSpotter];

/// Describes a subdomain lookup.
pub struct Query {
    /// The registrable name to enumerate under. The domain itself is included
    /// in the results when the logs know it.
    pub domain: String,
    /// Keeps names whose certificates have all lapsed. Those are often the
    /// interesting ones: decommissioned hosts that still answer.
    pub include_expired: bool,
    /// Keeps `*.example.com` entries.
    pub include_wildcards: bool,
    pub timeout: Duration,
}

/// What a lookup found.
#[derive(Default)]
pub struct Found {
    pub names: Vec<Name>,
    /// The aggregator that actually answered, which differs from the one asked
    /// for when `Auto` falls back.
    pub source: &'static str,
    /// How many certificates were examined to produce `names`.
    pub certs: usize,
    /// Reports that `MAX_NAMES` was reached and names were dropped.
    pub truncated: bool,
}

/// Lists the subdomains of a domain from certificate transparency logs.
///
/// `progress` is called with the running byte and certificate counts while the
/// response streams in: a big domain takes a while and the caller wants
/// something to show for it.
pub async fn subdomains(
    source: Source,
    q: &Query,
    progress: &(dyn Fn(u64, usize) + Sync),
) -> Result<Found, String> {
    let client = reqwest::Client::builder()
        .user_agent("ntls")
        .timeout(q.timeout)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    match source {
        Source::CrtSh => query_crtsh(&client, q, progress).await,
        Source::CertSpotter => query_certspotter(&client, q, progress).await,
        Source::Auto => match query_crtsh(&client, q, progress).await {
            Ok(res) => Ok(res),
            Err(first) => match query_certspotter(&client, q, progress).await {
                Ok(res) => Ok(res),
                // The first source is the one the user expected to work, so
                // its error leads.
                Err(second) => Err(format!("crt.sh: {first} (certspotter also failed: {second})")),
            },
        },
    }
}

async fn get_body(
    client: &reqwest::Client,
    url: &str,
    progress: &(dyn Fn(u64, usize) + Sync),
) -> Result<Vec<u8>, String> {
    use futures::StreamExt;

    let resp = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(status_error(resp).await);
    }

    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            break;
        }
        progress(body.len() as u64, 0);
    }
    Ok(body)
}

/// Turns a bad response into a message that says what to do about it, since
/// these two services fail in predictable ways.
async fn status_error(resp: reqwest::Response) -> String {
    let code = resp.status().as_u16();
    match code {
        429 => return "rate limited (429); wait a minute or switch source".into(),
        502..=504 => {
            return format!("service unavailable ({code}); it is overloaded, try again");
        }
        _ => {}
    }
    // A short snippet of the body usually carries the real reason.
    let body = resp.text().await.unwrap_or_default();
    let msg: String = body.split_whitespace().take(24).collect::<Vec<_>>().join(" ");
    if msg.is_empty() { format!("http {code}") } else { format!("http {code}: {msg}") }
}

// --- crt.sh ---------------------------------------------------------------

/// The subset of crt.sh's JSON we use. `name_value` holds every name on the
/// certificate, newline separated.
#[derive(Deserialize)]
struct CrtEntry {
    #[serde(default)]
    name_value: String,
    #[serde(default)]
    common_name: String,
    #[serde(default)]
    issuer_name: String,
    #[serde(default)]
    not_before: String,
    #[serde(default)]
    not_after: String,
}

async fn query_crtsh(
    client: &reqwest::Client,
    q: &Query,
    progress: &(dyn Fn(u64, usize) + Sync),
) -> Result<Found, String> {
    // The leading % is crt.sh's wildcard: it matches every name ending in the
    // domain. Certificates found this way carry the apex in their SAN list
    // too, so the domain itself turns up without a second query.
    let mut url = format!("https://crt.sh/?q=%25.{}&output=json", q.domain);
    if !q.include_expired {
        url += "&exclude=expired";
    }

    let body = get_body(client, &url, progress).await?;
    let entries: Vec<CrtEntry> = serde_json::from_slice(&body)
        .map_err(|e| format!("decoding crt.sh response: {e}"))?;

    let mut agg = Agg::new(q);
    for e in entries {
        let mut names: Vec<&str> = e.name_value.split('\n').collect();
        if !e.common_name.is_empty() {
            names.push(&e.common_name);
        }
        agg.add(&names, short_issuer(&e.issuer_name), parse_date(&e.not_before), parse_date(&e.not_after));
        progress(body.len() as u64, agg.certs);
    }

    let mut res = agg.finish();
    res.source = "crt.sh";
    Ok(res)
}

// --- certspotter ----------------------------------------------------------

#[derive(Deserialize)]
struct SpotterEntry {
    id: String,
    #[serde(default)]
    dns_names: Vec<String>,
    #[serde(default)]
    not_before: String,
    #[serde(default)]
    not_after: String,
    #[serde(default)]
    issuer: SpotterIssuer,
}

#[derive(Deserialize, Default)]
struct SpotterIssuer {
    #[serde(default)]
    name: String,
}

async fn query_certspotter(
    client: &reqwest::Client,
    q: &Query,
    progress: &(dyn Fn(u64, usize) + Sync),
) -> Result<Found, String> {
    let mut agg = Agg::new(q);

    // certspotter answers in pages keyed by the last id seen. Without an API
    // key the pages are small, so cap the walk and do not let a large domain
    // spend the whole timeout collecting them.
    const MAX_PAGES: usize = 200;
    let mut after = String::new();
    let today = Date::today();

    for _ in 0..MAX_PAGES {
        let mut url = format!(
            "https://api.certspotter.com/v1/issuances?domain={}&include_subdomains=true&expand=dns_names&expand=issuer",
            q.domain
        );
        if !after.is_empty() {
            url += &format!("&after={after}");
        }

        let body = get_body(client, &url, progress).await?;
        let entries: Vec<SpotterEntry> = serde_json::from_slice(&body)
            .map_err(|e| format!("decoding certspotter response: {e}"))?;
        if entries.is_empty() {
            break;
        }

        after = entries[entries.len() - 1].id.clone();
        for e in entries {
            let not_after = parse_date(&e.not_after);
            if !q.include_expired && !not_after.is_zero() && not_after < today {
                continue;
            }
            let names: Vec<&str> = e.dns_names.iter().map(String::as_str).collect();
            agg.add(&names, short_issuer(&e.issuer.name), parse_date(&e.not_before), not_after);
        }
        progress(0, agg.certs);
    }

    let mut res = agg.finish();
    res.source = "certspotter";
    Ok(res)
}

// --- aggregation ----------------------------------------------------------

/// Folds a stream of certificates into one row per distinct name.
struct Agg<'a> {
    q: &'a Query,
    /// "." + domain, the test for "is a subdomain of".
    suffix: String,
    by_name: HashMap<String, Name>,
    certs: usize,
    full: bool,
}

impl<'a> Agg<'a> {
    fn new(q: &'a Query) -> Agg<'a> {
        Agg { suffix: format!(".{}", q.domain), q, by_name: HashMap::new(), certs: 0, full: false }
    }

    /// Folds one certificate in. Names outside the queried domain are dropped:
    /// a certificate can cover unrelated hosts, and those are not what was
    /// asked for.
    fn add(&mut self, names: &[&str], issuer: String, not_before: Date, not_after: Date) {
        self.certs += 1;

        let mut seen = std::collections::HashSet::new();
        for raw in names {
            let Some(name) = clean_name(raw) else { continue };
            if !seen.insert(name.clone()) {
                continue;
            }

            let bare = name.strip_prefix("*.").unwrap_or(&name);
            if bare != self.q.domain && !bare.ends_with(&self.suffix) {
                continue;
            }
            let wildcard = name.starts_with("*.");
            if wildcard && !self.q.include_wildcards {
                continue;
            }

            if !self.by_name.contains_key(&name) {
                if self.by_name.len() >= MAX_NAMES {
                    self.full = true;
                    continue;
                }
                self.by_name
                    .insert(name.clone(), Name { name: name.clone(), wildcard, ..Default::default() });
            }
            let entry = self.by_name.get_mut(&name).expect("just inserted");

            entry.certs += 1;
            if !not_before.is_zero() {
                if entry.first_seen.is_zero() || not_before < entry.first_seen {
                    entry.first_seen = not_before;
                }
                // The newest certificate is the one whose issuer is worth
                // showing.
                if not_before > entry.last_seen {
                    entry.last_seen = not_before;
                    entry.issuer = issuer.clone();
                }
            }
            if not_after > entry.expires {
                entry.expires = not_after;
            }
            if entry.issuer.is_empty() {
                entry.issuer = issuer.clone();
            }
        }
    }

    fn finish(self) -> Found {
        let mut names: Vec<Name> = self
            .by_name
            .into_values()
            .filter(|n| self.q.include_expired || !n.expired())
            .collect();

        // Sorting by the name read right-to-left groups a host with its
        // siblings and does not scatter them by first letter, so api.eu and
        // api.us land together.
        names.sort_by(|a, b| {
            reverse_labels(&a.name)
                .cmp(&reverse_labels(&b.name))
                // A wildcard shares its key with the name it hangs off, and
                // belongs just below it, not above.
                .then(a.wildcard.cmp(&b.wildcard))
                .then(a.name.cmp(&b.name))
        });

        Found { names, source: "", certs: self.certs, truncated: self.full }
    }
}

/// Rewrites `a.b.example.com` as `com.example.b.a`, the order that sorts a
/// domain tree sensibly.
fn reverse_labels(name: &str) -> String {
    let bare = name.strip_prefix("*.").unwrap_or(name);
    bare.split('.').rev().collect::<Vec<_>>().join(".")
}

/// Normalises one name from a certificate, rejecting the things that are not
/// hostnames: certificates also carry email addresses and IP addresses, and
/// neither is a subdomain.
fn clean_name(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.trim_end_matches('.');
    if s.is_empty() || s.contains(['@', ' ', '\t']) || !s.contains('.') {
        return None;
    }
    if s.parse::<IpAddr>().is_ok() {
        return None;
    }
    Some(s.to_string())
}

/// Accepts what people paste (a URL, a wildcard, a trailing dot) and reduces
/// it to the bare domain to query.
pub fn normalize_domain(s: &str) -> Result<String, String> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err("no domain given".into());
    }
    let s = s.split_once("://").map(|(_, rest)| rest).unwrap_or(&s);
    let s = s.strip_prefix("*.").unwrap_or(s).trim_end_matches('.');
    let s = s.split(['/', '?', '#']).next().unwrap_or(s);
    let s = s.rsplit_once(':').map_or(s, |(host, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() { host } else { s }
    });

    if s.is_empty() || !s.contains('.') {
        return Err("expected a domain such as example.com".into());
    }
    if s.parse::<IpAddr>().is_ok() {
        return Err("certificate logs are indexed by name, not by address".into());
    }
    if s.contains([' ', '\t', '@', '_']) {
        return Err(format!("{s:?} is not a domain"));
    }
    Ok(s.to_string())
}

/// Reduces a pasted URL to the domain that will actually be queried, so the
/// form shows what is about to happen.
pub fn expand_domain(s: &str) -> Option<String> {
    let d = normalize_domain(s).ok()?;
    (d != s.trim()).then_some(d)
}

/// Reads the timestamps these APIs use. crt.sh omits the zone and means UTC;
/// certspotter sends RFC 3339. Only the date matters here.
fn parse_date(s: &str) -> Date {
    let s = s.trim();
    if s.len() < 10 {
        return Date::default();
    }
    let d = &s[..10];
    let mut parts = d.split('-');
    let (Some(y), Some(m), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return Date::default();
    };
    let (Ok(y), Ok(m), Ok(day)) = (y.parse::<i64>(), m.parse::<u32>(), day.parse::<u32>()) else {
        return Date::default();
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return Date::default();
    }
    Date { days: days_from_civil(y, m, day) }
}

/// Pulls the readable part out of an X.500 issuer string, otherwise
/// too wide for a column.
fn short_issuer(dn: &str) -> String {
    let dn = dn.trim();
    if dn.is_empty() {
        return String::new();
    }
    let (mut cn, mut org) = (String::new(), String::new());
    for part in split_dn(dn) {
        let Some((key, value)) = part.split_once('=') else { continue };
        let value = value.trim().to_string();
        match key.trim().to_ascii_uppercase().as_str() {
            "CN" => cn = value,
            "O" => org = value,
            _ => {}
        }
    }
    // The organisation is the name people recognise; the CN is the specific
    // intermediate, worth keeping when it adds something.
    match (org.is_empty(), cn.is_empty()) {
        (false, false) if !org.eq_ignore_ascii_case(&cn) => format!("{org} {cn}"),
        (false, _) => org,
        (true, false) => cn,
        _ => dn.split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

/// Splits on commas that separate attributes, leaving escaped ones alone.
fn split_dn(dn: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut chars = dn.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            ',' => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_url_reduces_to_its_domain() {
        assert_eq!(normalize_domain("https://Example.com/path?q=1").unwrap(), "example.com");
        assert_eq!(normalize_domain("*.example.com.").unwrap(), "example.com");
        assert_eq!(normalize_domain("example.com:443").unwrap(), "example.com");
    }

    #[test]
    fn things_that_are_not_domains_are_refused() {
        assert!(normalize_domain("").is_err());
        assert!(normalize_domain("localhost").is_err());
        assert!(normalize_domain("1.1.1.1").is_err());
        assert!(normalize_domain("a b.com").is_err());
    }

    #[test]
    fn expansion_reports_only_real_changes() {
        assert_eq!(expand_domain("https://example.com/"), Some("example.com".into()));
        assert_eq!(expand_domain("example.com"), None);
    }

    #[test]
    fn certificate_names_are_filtered_to_hostnames() {
        assert_eq!(clean_name(" API.Example.COM. "), Some("api.example.com".into()));
        assert_eq!(clean_name("admin@example.com"), None);
        assert_eq!(clean_name("192.0.2.1"), None);
        assert_eq!(clean_name("localhost"), None);
    }

    #[test]
    fn names_sort_right_to_left_by_label() {
        assert_eq!(reverse_labels("internal.api.example.com"), "com.example.api.internal");
        // That keeps a host next to its siblings instead of
        // scattering them by first letter.
        let mut names = ["api.eu.example.com", "zeta.example.com", "api.us.example.com"];
        names.sort_by_key(|n| reverse_labels(n));
        assert_eq!(names, ["api.eu.example.com", "api.us.example.com", "zeta.example.com"]);
    }

    #[test]
    fn dates_round_trip_through_the_day_count() {
        for s in ["2024-02-29", "1999-12-31", "2026-08-23"] {
            assert_eq!(parse_date(s).format(), s);
        }
        assert_eq!(parse_date("2024-06-01T12:00:00Z").format(), "2024-06-01");
        assert!(parse_date("").is_zero());
        assert_eq!(parse_date("").format(), "-");
    }

    #[test]
    fn an_issuer_reduces_to_the_name_people_recognise() {
        assert_eq!(short_issuer("C=US, O=Let's Encrypt, CN=R11"), "Let's Encrypt R11");
        assert_eq!(short_issuer("O=Acme, CN=Acme"), "Acme");
        assert_eq!(short_issuer("CN=Some Intermediate"), "Some Intermediate");
    }

    #[test]
    fn aggregation_keeps_one_row_per_name() {
        let q = Query {
            domain: "example.com".into(),
            include_expired: true,
            include_wildcards: true,
            timeout: Duration::from_secs(1),
        };
        let mut agg = Agg::new(&q);
        agg.add(&["a.example.com", "example.com"], "CA One".into(), parse_date("2024-01-01"), parse_date("2024-04-01"));
        agg.add(&["a.example.com", "elsewhere.net"], "CA Two".into(), parse_date("2024-03-01"), parse_date("2025-06-01"));

        let res = agg.finish();
        assert_eq!(res.certs, 2);
        let names: Vec<&str> = res.names.iter().map(|n| n.name.as_str()).collect();
        // Names outside the queried domain are dropped.
        assert_eq!(names, ["example.com", "a.example.com"]);

        let a = res.names.iter().find(|n| n.name == "a.example.com").unwrap();
        assert_eq!(a.certs, 2);
        assert_eq!(a.first_seen.format(), "2024-01-01");
        // The newest certificate is the one whose issuer is shown.
        assert_eq!(a.issuer, "CA Two");
        assert_eq!(a.expires.format(), "2025-06-01");
    }

    #[test]
    fn a_wildcard_hands_off_the_name_it_covers() {
        let n = Name { name: "*.example.com".into(), wildcard: true, ..Default::default() };
        assert_eq!(n.host(), "example.com");
    }
}
