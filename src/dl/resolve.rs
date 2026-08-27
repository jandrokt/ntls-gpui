//! Resolving a pasted URL into files a `GET` can fetch.
//!
//! Three strategies, tried in order of how much they know:
//!
//! 1. **The server.** A ranged probe says whether the URL is a file or a page.
//!    A direct link goes no further.
//! 2. **The extractors.** `yt-dlp` and `gallery-dl` maintain the per-site
//!    knowledge, and both answer questions about a URL without downloading it.
//!    If one recognises the link, it takes precedence.
//! 3. **The page.** Otherwise the markup is parsed for the elements file hosts
//!    publish so that other sites can link to them: a download anchor, a
//!    `<video>` element, Open Graph tags, a JSON-LD `contentUrl`, a meta
//!    refresh, a URL held in a `data-` attribute. These are rules about HTML,
//!    not about any site.
//!
//! Every candidate from step three is checked against step one before it is
//! offered, so a candidate that turns out to be another page is discarded.

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use reqwest::header::USER_AGENT;
use scraper::{Html, Selector};
use url::Url;

use crate::core::{Cancel, Emitter};

use super::external::{self, Extractor};
use super::fetch::AGENT;
use super::names;

/// One file to fetch, already resolved to something a plain `GET` can take.
#[derive(Clone, Debug)]
pub struct Asset {
    pub url: String,
    /// The name the source suggested, if it suggested one. The server has the
    /// last word.
    pub name: Option<String>,
    /// The page the link was found on, sent back to hosts that check.
    pub referer: Option<String>,
}

impl Asset {
    pub fn new(url: impl Into<String>) -> Asset {
        Asset { url: url.into(), name: None, referer: None }
    }

    pub fn from(mut self, page: &str) -> Asset {
        self.referer = Some(page.to_string());
        self
    }
}

/// What a link turned out to be.
#[derive(Clone, Debug)]
pub enum Plan {
    /// URLs to fetch with our own engine.
    Direct(Vec<Asset>),
    /// A link an extractor claimed. It knows how to reassemble whatever is
    /// behind it — streams in fragments, separate audio and video — so it does
    /// the downloading too.
    Handled { by: Extractor, url: String, title: String },
}

/// Which route to take, when the user would rather not leave it to the
/// program.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Prefer {
    #[default]
    Auto,
    /// Treat the link as a file and fetch it.
    Direct,
    /// Insist on the media extractor.
    YtDlp,
    /// Insist on the gallery extractor.
    GalleryDl,
    /// Skip the extractors and read the page.
    Page,
}

impl Prefer {
    pub fn parse(s: &str) -> Prefer {
        match s {
            "direct" => Prefer::Direct,
            "ytdlp" => Prefer::YtDlp,
            "gallerydl" => Prefer::GalleryDl,
            "page" => Prefer::Page,
            _ => Prefer::Auto,
        }
    }
}

/// Works out how to fetch one link.
pub async fn resolve(
    client: &reqwest::Client,
    raw: &str,
    prefer: Prefer,
    cancel: &Cancel,
    emit: &Emitter,
) -> Result<Plan> {
    let url = normalize(raw)?;

    if prefer == Prefer::Direct {
        return Ok(Plan::Direct(vec![Asset::new(url)]));
    }

    // 1. What does the server say it is?
    if matches!(prefer, Prefer::Auto) {
        let asset = Asset::new(url.clone());
        match super::fetch::head(client, &asset, cancel).await {
            Ok(head) if !names::is_markup(&head.mime) => {
                emit.info(format!("{} — direct file, {}", short(&url), head.mime));
                return Ok(Plan::Direct(vec![asset]));
            }
            Ok(_) => {}
            // A server that refuses a bare range request may still serve the
            // page, so this is a reason to keep looking rather than to stop.
            Err(e) => emit.info(format!("{}: probe failed, {e}", short(&url))),
        }
    }

    // 2. Does an extractor recognise it?
    for extractor in Extractor::ALL {
        if !extractor.wanted_by(prefer) {
            continue;
        }
        match external::claim(extractor, &url, cancel).await {
            Ok(Some(claim)) => {
                emit.good(format!("{}: recognised {}", extractor.binary(), short(&url)));
                return match extractor {
                    // The media extractor downloads what it claims, because
                    // what it claims is often not one file.
                    Extractor::YtDlp => {
                        Ok(Plan::Handled { by: extractor, url: url.clone(), title: claim.title })
                    }
                    // The gallery extractor hands back plain URLs, which our
                    // engine fetches faster than it would.
                    Extractor::GalleryDl => Ok(Plan::Direct(
                        claim
                            .urls
                            .into_iter()
                            .map(|u| Asset::new(u).from(&url))
                            .collect::<Vec<_>>(),
                    )),
                };
            }
            Ok(None) => {}
            Err(e) => emit.info(format!("{}: {e}", extractor.binary())),
        }
    }

    if prefer == Prefer::YtDlp || prefer == Prefer::GalleryDl {
        bail!("{} does not handle {}", "the selected extractor", short(&url));
    }

    // 3. Read the page.
    let assets = scrape(client, &url, cancel, emit).await?;
    if assets.is_empty() {
        bail!(
            "No downloadable content found at {}. Installing yt-dlp or gallery-dl \
             adds support for several thousand more sites.",
            short(&url)
        );
    }
    Ok(Plan::Direct(assets))
}

/// Reads a page and returns whatever on it turns out to be a file.
///
/// Every candidate is checked before it is offered, which is what lets the
/// scoring be generous: a wrong guess costs one request, and the guesses come
/// from markup that exists to be read.
async fn scrape(
    client: &reqwest::Client,
    page: &str,
    cancel: &Cancel,
    emit: &Emitter,
) -> Result<Vec<Asset>> {
    let mut here = page.to_string();

    // A landing page that only redirects is not the page we want to read.
    for _ in 0..3 {
        let body = get_text(client, &here, cancel).await?;
        let candidates = candidates(&body, &here);
        emit.info(format!("{}: parsed, {} candidate(s)", short(&here), candidates.len()));

        if let Some(next) = meta_refresh(&body, &here)
            && next != here
        {
            emit.info(format!("Redirected to {}", short(&next)));
            here = next;
            continue;
        }

        let mut found = Vec::new();
        for candidate in candidates.into_iter().take(8) {
            if cancel.is_cancelled() {
                bail!("cancelled");
            }
            let asset = Asset { url: candidate.url, name: candidate.name, referer: Some(here.clone()) };
            match super::fetch::head(client, &asset, cancel).await {
                Ok(head) if !names::is_markup(&head.mime) => {
                    emit.good(format!(
                        "Resolved via {} → {} ({})",
                        candidate.why,
                        short(&asset.url),
                        head.size.map(names::bytes).unwrap_or_else(|| head.mime.clone())
                    ));
                    found.push(asset);
                    // One good file per page is the usual case; a gallery is
                    // what the gallery extractor is for.
                    break;
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        return Ok(found);
    }
    Ok(Vec::new())
}

/// A link the page offered, and why we believe in it.
#[derive(Clone, Debug)]
struct Candidate {
    url: String,
    name: Option<String>,
    why: &'static str,
    score: u32,
}

/// Every link on a page that could be a file, best first.
///
/// The rules are about HTML, not about hosts: an anchor that calls itself a
/// download, the media elements, the metadata a page publishes so that other
/// sites can embed it, and the attributes a page stashes a URL in when it
/// would rather not put it in an `href`.
fn candidates(body: &str, page: &str) -> Vec<Candidate> {
    let document = Html::parse_document(body);
    let base = base_url(&document, page);
    let mut out: Vec<Candidate> = Vec::new();

    let mut push = |raw: &str, why: &'static str, score: u32, name: Option<String>| {
        if let Some(url) = absolute(&base, raw)
            && !uninteresting(&url, page)
        {
            out.push(Candidate { url, name, why, score });
        }
    };

    // An anchor that says it is a download. The `download` attribute is the
    // standard way to say so; naming it is what everyone did before that.
    for element in select(&document, "a[href]") {
        let Some(href) = element.value().attr("href") else { continue };
        let identity = format!(
            "{} {} {} {}",
            element.value().attr("id").unwrap_or(""),
            element.value().attr("class").unwrap_or(""),
            element.value().attr("rel").unwrap_or(""),
            element.value().attr("aria-label").unwrap_or(""),
        )
        .to_ascii_lowercase();
        let named = element.value().attr("download").filter(|n| !n.is_empty()).map(str::to_string);

        if element.value().attr("download").is_some() {
            push(href, "the download attribute", 100, named);
        } else if identity.contains("download") {
            push(href, "a link named download", 96, None);
        } else if element.text().collect::<String>().to_ascii_lowercase().contains("download") {
            push(href, "a link labelled download", 88, None);
        } else if looks_like_a_file(href) {
            push(href, "a link to a file", 46, None);
        }
    }

    // A URL a page stashed somewhere other than an href, which is what a host
    // does when it would rather a scraper did not find it. Both the plain and
    // the base64 form are worth a look.
    const STASHES: [&str; 7] =
        ["data-scrambled-url", "data-url", "data-href", "data-src", "data-file", "data-download", "data-link"];
    for stash in STASHES {
        for element in select(&document, &format!("[{stash}]")) {
            let Some(value) = element.value().attr(stash) else { continue };
            push(value, "a data attribute", 92, None);
            if let Some(decoded) = unbase64(value) {
                push(&decoded, "a base64 data attribute", 94, None);
            }
        }
    }

    // The media the page is showing.
    for (selector, why, score) in [
        ("video source[src]", "a video source", 86),
        ("video[src]", "a video element", 85),
        ("audio source[src]", "an audio source", 84),
        ("audio[src]", "an audio element", 83),
    ] {
        for element in select(&document, selector) {
            if let Some(src) = element.value().attr("src") {
                push(src, why, score, None);
            }
        }
    }

    // What the page publishes about itself so other sites can embed it.
    for (property, why, score) in [
        ("og:video:secure_url", "an Open Graph video tag", 82),
        ("og:video:url", "an Open Graph video tag", 81),
        ("og:video", "an Open Graph video tag", 80),
        ("og:audio", "an Open Graph audio tag", 78),
        ("twitter:player:stream", "a player stream tag", 76),
        ("og:image", "an Open Graph image tag", 40),
    ] {
        for element in select(&document, &format!(r#"meta[property="{property}"], meta[name="{property}"]"#)) {
            if let Some(content) = element.value().attr("content") {
                push(content, why, score, None);
            }
        }
    }

    // Structured data, where a page states outright where its content is.
    for element in select(&document, r#"script[type="application/ld+json"]"#) {
        let text = element.text().collect::<String>();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            for url in json_content_urls(&value) {
                push(&url, "a JSON-LD content URL", 74, None);
            }
        }
    }

    // A stated alternative to the page: a feed, a media file, a torrent.
    for element in select(&document, "link[rel][href]") {
        let kind = element.value().attr("type").unwrap_or("");
        if !kind.is_empty() && !names::is_markup(kind) {
            if let Some(href) = element.value().attr("href") {
                push(href, "a declared alternative", 60, None);
            }
        }
    }

    // Anything in the page's scripts that looks like a file. This is the last
    // resort, and the cheapest one to be wrong about.
    for element in select(&document, "script") {
        for url in urls_in(&element.text().collect::<String>()) {
            if looks_like_a_file(&url) {
                push(&url, "a URL in an inline script", 30, None);
            }
        }
    }

    out.sort_by_key(|c| std::cmp::Reverse(c.score));
    out.dedup_by(|a, b| a.url == b.url);
    out
}

/// The URL a `<meta http-equiv="refresh">` sends a browser to.
fn meta_refresh(body: &str, page: &str) -> Option<String> {
    let document = Html::parse_document(body);
    let base = base_url(&document, page);
    let element = select(&document, r#"meta[http-equiv]"#)
        .into_iter()
        .find(|e| e.value().attr("http-equiv").is_some_and(|v| v.eq_ignore_ascii_case("refresh")))?;
    let content = element.value().attr("content")?;
    let (_, target) = content.split_once(';')?;
    let target = target.trim().strip_prefix("url").or(Some(target.trim()))?;
    let target = target.trim_start_matches(['=', ' ', '"', '\'']).trim_end_matches(['"', '\'']);
    absolute(&base, target)
}

fn base_url(document: &Html, page: &str) -> Url {
    let declared = select(document, "base[href]")
        .into_iter()
        .next()
        .and_then(|e| e.value().attr("href").map(str::to_string))
        .and_then(|href| Url::parse(&href).ok());
    declared.or_else(|| Url::parse(page).ok()).unwrap_or_else(|| {
        Url::parse("https://invalid.invalid/").expect("a constant URL")
    })
}

fn select<'a>(document: &'a Html, selector: &str) -> Vec<scraper::ElementRef<'a>> {
    match Selector::parse(selector) {
        Ok(s) => document.select(&s).collect(),
        Err(_) => Vec::new(),
    }
}

/// Resolves a possibly relative link against the page it was found on.
fn absolute(base: &Url, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('#') {
        return None;
    }
    let joined = base.join(raw).ok()?;
    matches!(joined.scheme(), "http" | "https").then(|| joined.to_string())
}

/// Links that are on every page and are never the file: the page itself, the
/// navigation, the share buttons.
fn uninteresting(url: &str, page: &str) -> bool {
    if url == page {
        return true;
    }
    let Ok(parsed) = Url::parse(url) else { return true };
    let host = parsed.host_str().unwrap_or("");
    const ELSEWHERE: [&str; 12] = [
        "facebook.com",
        "twitter.com",
        "x.com",
        "instagram.com",
        "linkedin.com",
        "pinterest.com",
        "reddit.com",
        "youtube.com/channel",
        "google.com",
        "apple.com",
        "microsoft.com",
        "doubleclick.net",
    ];
    ELSEWHERE.iter().any(|bad| host.ends_with(bad) || url.contains(bad))
}

/// Extensions worth a request. Deliberately not a list of file types the
/// program understands — it never opens what it downloads — but of the ones
/// that distinguish a file from a page.
const FILE_EXTENSIONS: [&str; 34] = [
    "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "dmg", "pkg", "exe", "msi", "apk", "deb",
    "rpm", "mp4", "mkv", "webm", "mov", "avi", "m4v", "mp3", "flac", "wav", "m4a", "opus", "ogg",
    "pdf", "epub", "jpg", "jpeg", "png", "gif", "webp",
];

fn looks_like_a_file(raw: &str) -> bool {
    let path = raw.split(['?', '#']).next().unwrap_or(raw).to_ascii_lowercase();
    let Some((_, ext)) = path.rsplit_once('.') else { return false };
    FILE_EXTENSIONS.contains(&ext)
}

/// The base64 hiding place, when the value decodes to a URL and not to noise.
fn unbase64(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 16 || value.len() > 4096 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(value))
        .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    (text.starts_with("http://") || text.starts_with("https://")).then_some(text)
}

/// Every `contentUrl` in a JSON-LD document, however deeply it is nested.
fn json_content_urls(value: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    match value {
        serde_json::Value::Object(map) => {
            for key in ["contentUrl", "embedUrl", "url"] {
                if let Some(serde_json::Value::String(s)) = map.get(key)
                    && (looks_like_a_file(s) || key == "contentUrl")
                {
                    out.push(s.clone());
                }
            }
            for nested in map.values() {
                out.extend(json_content_urls(nested));
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                out.extend(json_content_urls(item));
            }
        }
        _ => {}
    }
    out
}

/// Absolute URLs appearing in a block of script, unescaped the two ways a
/// script commonly escapes them.
fn urls_in(text: &str) -> Vec<String> {
    let text = text.replace("\\/", "/").replace("\\u002F", "/").replace("\\u002f", "/");
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("http") {
        rest = &rest[at..];
        if !(rest.starts_with("http://") || rest.starts_with("https://")) {
            rest = &rest[4..];
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '\\' | ')'))
            .unwrap_or(rest.len());
        out.push(rest[..end].to_string());
        rest = &rest[end.max(1)..];
    }
    out
}

async fn get_text(client: &reqwest::Client, url: &str, cancel: &Cancel) -> Result<String> {
    let request = client.get(url).header(USER_AGENT, AGENT);
    let response = cancel
        .run(request.send())
        .await
        .ok_or_else(|| anyhow!("cancelled"))?
        .with_context(|| format!("cannot reach {}", short(url)))?
        .error_for_status()?;
    cancel.run(response.text()).await.ok_or_else(|| anyhow!("cancelled"))?.context("unreadable page")
}

/// Accepts what a person pastes, which is often missing its scheme.
pub fn normalize(raw: &str) -> Result<String> {
    let raw = raw.trim().trim_matches(['<', '>', '"', '\'']);
    if raw.is_empty() {
        bail!("empty link");
    }
    let with_scheme =
        if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
    let parsed = Url::parse(&with_scheme).with_context(|| format!("{raw} is not a link"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("{} is not something to download over the web", parsed.scheme());
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        bail!("{raw} has no host");
    }
    Ok(parsed.to_string())
}

/// Splits whatever was pasted into the links inside it. People paste lists
/// separated by newlines, by commas, or by nothing but a space.
pub fn split_links(raw: &str) -> Vec<String> {
    raw.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// A URL short enough to read in a log line.
pub fn short(url: &str) -> String {
    let trimmed = url.split(['?', '#']).next().unwrap_or(url);
    if trimmed.chars().count() <= 72 {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(44).collect();
    let tail: String = trimmed.chars().rev().take(24).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
    <html><head>
      <meta property="og:video" content="//cdn.example/clip.mp4">
      <script type="application/ld+json">{"@type":"VideoObject","contentUrl":"https://cdn.example/ld.mp4"}</script>
    </head><body>
      <a href="/help">Help</a>
      <a href="/files/manual.pdf">The manual</a>
      <a id="downloadButton" href="https://download123.example/f/thing.zip"
         data-scrambled-url="aHR0cHM6Ly9kb3dubG9hZDk5OS5leGFtcGxlL2YvdGhpbmcuemlw">Download (4MB)</a>
      <a href="https://facebook.com/share">Share</a>
      <script>var src = "https:\/\/cdn.example\/inline.mp4";</script>
    </body></html>"#;

    #[test]
    fn a_download_link_outranks_everything_else_on_the_page() {
        let found = candidates(PAGE, "https://host.example/file/abc");
        let best = &found[0];
        assert!(best.url.contains("download"), "got {}", best.url);
        assert!(best.score >= 92);
    }

    #[test]
    fn a_url_hidden_in_an_attribute_is_decoded() {
        let found = candidates(PAGE, "https://host.example/file/abc");
        assert!(
            found.iter().any(|c| c.url == "https://download999.example/f/thing.zip"),
            "the base64 attribute was not decoded"
        );
    }

    #[test]
    fn relative_and_protocol_relative_links_are_made_absolute() {
        let found = candidates(PAGE, "https://host.example/file/abc");
        // A protocol-relative URL and a rooted path both resolve against the
        // page they were found on.
        assert!(found.iter().any(|c| c.url == "https://cdn.example/clip.mp4"));
        assert!(found.iter().any(|c| c.url == "https://host.example/files/manual.pdf"));
        // A link that is neither named nor shaped like a file is not one.
        assert!(!found.iter().any(|c| c.url.ends_with("/help")));
    }

    #[test]
    fn the_links_everyone_puts_on_every_page_are_left_alone() {
        let found = candidates(PAGE, "https://host.example/file/abc");
        assert!(!found.iter().any(|c| c.url.contains("facebook")));
    }

    #[test]
    fn structured_data_and_inline_scripts_are_read() {
        let found = candidates(PAGE, "https://host.example/file/abc");
        assert!(found.iter().any(|c| c.url == "https://cdn.example/ld.mp4"));
        assert!(found.iter().any(|c| c.url == "https://cdn.example/inline.mp4"));
    }

    #[test]
    fn a_refresh_is_followed() {
        let body = r#"<meta http-equiv="refresh" content="0; url=/next/page">"#;
        assert_eq!(
            meta_refresh(body, "https://host.example/a").as_deref(),
            Some("https://host.example/next/page")
        );
    }

    /// The generic path against a real file host, which is the claim worth
    /// checking: the page reader finds the file without a rule about the
    /// site. Ignored by default because it needs the network and a link that
    /// still exists.
    #[test]
    #[ignore = "needs the network"]
    fn a_file_host_page_gives_up_its_file() {
        let page = "https://www.mediafire.com/file/987dewn745uosgl/\
                    -MissTiikeri-_AG_3t2_Shirt_Around_Waist_Accessory.rar/file";
        let runtime = tokio::runtime::Runtime::new().expect("a runtime");
        let emit = Emitter::new(|e| println!("{e:?}"));
        let client = super::super::fetch::client().expect("a client");

        let plan = runtime
            .block_on(resolve(&client, page, Prefer::Page, &Cancel::new(), &emit))
            .expect("the page to resolve");
        match plan {
            Plan::Direct(assets) => {
                assert!(!assets.is_empty());
                println!("resolved to {}", assets[0].url);
            }
            other => panic!("expected a direct link, got {other:?}"),
        }
    }

    #[test]
    fn a_pasted_list_is_split_however_it_was_written() {
        let pasted = "https://a.example/1\n https://b.example/2 , https://c.example/3";
        assert_eq!(split_links(pasted).len(), 3);
    }

    #[test]
    fn a_link_missing_its_scheme_still_works() {
        assert_eq!(normalize("example.com/a.zip").unwrap(), "https://example.com/a.zip");
        assert!(normalize("ftp://example.com/a.zip").is_err());
        assert!(normalize("   ").is_err());
    }
}
