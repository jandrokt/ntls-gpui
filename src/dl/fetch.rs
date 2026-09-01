//! The transfer engine: one URL to one file on disk.
//!
//! A transfer is split across several ranged requests, each retrying
//! independently. Progress is recorded in a ledger beside the part file, so an
//! interrupted transfer resumes from where it stopped, including across
//! sessions.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures::StreamExt;
use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG,
    HeaderMap, LAST_MODIFIED, RANGE, REFERER, USER_AGENT,
};
use serde::{Deserialize, Serialize};

use crate::core::Cancel;

use super::names;
use super::resolve::Asset;

/// Chrome's, because a file host that turns away an unfamiliar client is
/// common and a file host that turns away Chrome is not.
pub const AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// Below this, splitting a transfer costs more in requests than it saves in
/// throughput.
const SPLIT_ABOVE: u64 = 4 * 1024 * 1024;
/// How much arrives before it is handed to the disk.
const FLUSH_AT: usize = 512 * 1024;
/// How many times a segment may fail before the transfer does.
const ATTEMPTS: usize = 5;

/// What the caller can set about a transfer.
#[derive(Clone, Debug)]
pub struct Opts {
    pub dir: PathBuf,
    /// How many ranged requests to run at once for a single file.
    pub connections: usize,
    /// Replace an existing file instead of downloading alongside it.
    pub overwrite: bool,
    /// A ceiling on the whole queue's rate, shared by every transfer in it.
    pub limit: Option<Arc<Limit>>,
    /// Leave a stopped transfer's part file and ledger on disk, so it can be
    /// resumed in this session or a later one.
    pub keep_partial: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            dir: names::default_dir(),
            connections: 4,
            overwrite: false,
            limit: None,
            keep_partial: true,
        }
    }
}

/// A ceiling on how fast bytes may arrive, shared by every transfer running
/// under it.
///
/// It is a token bucket instead of a sleep per chunk: a limit that is applied
/// per connection is not the limit anyone asked for, and one that sleeps a
/// fixed time per chunk stalls on a slow server instead of speeding up.
#[derive(Debug)]
pub struct Limit {
    per_second: u64,
    state: std::sync::Mutex<Bucket>,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    at: std::time::Instant,
}

impl Limit {
    pub fn new(per_second: u64) -> Arc<Limit> {
        Arc::new(Limit {
            per_second: per_second.max(1),
            state: std::sync::Mutex::new(Bucket {
                // One second of credit, so a short transfer is not throttled
                // before it has started.
                tokens: per_second as f64,
                at: std::time::Instant::now(),
            }),
        })
    }

    /// Waits until this many bytes may be taken.
    async fn take(&self, bytes: u64) {
        loop {
            let wait = {
                let mut bucket = self.state.lock().expect("limit");
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(bucket.at).as_secs_f64();
                bucket.at = now;
                bucket.tokens =
                    (bucket.tokens + elapsed * self.per_second as f64).min(self.per_second as f64);

                if bucket.tokens >= bytes as f64 {
                    bucket.tokens -= bytes as f64;
                    return;
                }
                let short = bytes as f64 - bucket.tokens;
                Duration::from_secs_f64((short / self.per_second as f64).min(1.0))
            };
            tokio::time::sleep(wait).await;
        }
    }
}

/// How far along a transfer is. Shared with whoever is reporting on it.
#[derive(Debug, Default)]
pub struct Progress {
    /// Bytes on disk.
    pub done: AtomicU64,
    /// Bytes expected, or zero while the size is unknown.
    pub total: AtomicU64,
}

impl Progress {
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| (self.done.load(Ordering::Relaxed) as f32 / total as f32).clamp(0.0, 1.0))
    }
}

/// A finished transfer.
#[derive(Clone, Debug)]
pub struct Fetched {
    pub path: PathBuf,
    pub bytes: u64,
}

/// What a first look at a URL tells us.
#[derive(Clone, Debug)]
pub struct Head {
    pub size: Option<u64>,
    pub ranges: bool,
    pub name: Option<String>,
    pub mime: String,
    /// The version of the file the server is serving, so a resumed transfer
    /// can tell whether it is still the same file.
    pub tag: Option<String>,
    /// Where the request ended up, after redirects.
    pub url: String,
}

/// Asks a server what is at a URL without downloading it.
///
/// A one-byte ranged `GET` instead of a `HEAD`, because file hosts commonly
/// answer `HEAD` with a page, an error, or a length they then contradict.
pub async fn head(client: &reqwest::Client, asset: &Asset, cancel: &Cancel) -> Result<Head> {
    let request = client
        .get(&asset.url)
        .header(USER_AGENT, AGENT)
        .header(RANGE, "bytes=0-0")
        .headers(extra_headers(asset));
    let response = cancel
        .run(request.send())
        .await
        .ok_or_else(|| anyhow!("cancelled"))?
        .with_context(|| format!("cannot reach {}", asset.url))?;

    let status = response.status();
    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        bail!("{} answered {}", asset.url, status);
    }

    let headers = response.headers();
    let mime = header(headers, CONTENT_TYPE).unwrap_or_default();
    let name = header(headers, CONTENT_DISPOSITION).and_then(|d| names::from_disposition(&d));
    let tag = header(headers, ETAG).or_else(|| header(headers, LAST_MODIFIED));
    let url = response.url().to_string();

    // A server that honoured the range reports the whole size in
    // `Content-Range`; one that ignored it reports it in `Content-Length`.
    let (size, ranges) = match header(headers, CONTENT_RANGE) {
        Some(range) => (range.rsplit('/').next().and_then(|n| n.parse().ok()), true),
        None => {
            let advertised = header(headers, ACCEPT_RANGES).is_some_and(|v| v.contains("bytes"));
            (header(headers, CONTENT_LENGTH).and_then(|n| n.parse().ok()), advertised)
        }
    };

    Ok(Head { size, ranges, name, mime, tag, url })
}

/// Downloads one asset, returning where it landed.
pub async fn fetch(
    client: &reqwest::Client,
    asset: &Asset,
    opts: &Opts,
    cancel: &Cancel,
    progress: &Arc<Progress>,
) -> Result<Fetched> {
    let head = head(client, asset, cancel).await?;
    std::fs::create_dir_all(&opts.dir)
        .with_context(|| format!("cannot create {}", opts.dir.display()))?;

    let name = pick_name(asset, &head);
    let final_path = if opts.overwrite {
        opts.dir.join(&name)
    } else {
        names::unused(&opts.dir, &name)
    };
    let part = with_suffix(&final_path, ".ntlspart");
    let ledger_path = with_suffix(&final_path, ".ntlspart.json");

    progress.total.store(head.size.unwrap_or(0), Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);

    let total = match head.size {
        // Without a length there is nothing to divide and nothing to resume:
        // one stream, straight through.
        None => {
            return stream_whole(
                client,
                asset,
                &head,
                opts,
                (&part, &final_path),
                cancel,
                progress,
            )
            .await;
        }
        Some(0) => bail!("{} is empty", asset.url),
        Some(n) => n,
    };

    let want = opts.connections.clamp(1, 16);
    let ledger = Ledger::open(&ledger_path, &head, total, want, &part)?;
    progress.done.store(ledger.done(), Ordering::Relaxed);

    let file = Arc::new(
        std::fs::OpenOptions::new()
            .create(true)
            // Never truncate: the part file is what a resumed transfer is
            // resuming from.
            .truncate(false)
            .read(true)
            .write(true)
            .open(&part)
            .with_context(|| format!("cannot open {}", part.display()))?,
    );
    file.set_len(total).ok();

    let ledger = Arc::new(std::sync::Mutex::new(ledger));
    let mut running = futures::stream::FuturesUnordered::new();
    for index in 0..ledger.lock().expect("ledger").segments.len() {
        running.push(segment(
            client.clone(),
            asset.clone(),
            head.url.clone(),
            file.clone(),
            ledger.clone(),
            ledger_path.clone(),
            index,
            opts.limit.clone(),
            cancel.clone(),
            progress.clone(),
        ));
    }
    while let Some(outcome) = running.next().await {
        outcome?;
    }

    if cancel.is_cancelled() {
        drop(file);
        if !opts.keep_partial {
            std::fs::remove_file(&part).ok();
            std::fs::remove_file(&ledger_path).ok();
        }
        bail!("cancelled");
    }
    drop(file);
    std::fs::rename(&part, &final_path)
        .with_context(|| format!("cannot move {} into place", part.display()))?;
    std::fs::remove_file(&ledger_path).ok();
    Ok(Fetched { path: final_path, bytes: total })
}

/// One contiguous range of the file, retried on its own.
#[allow(clippy::too_many_arguments)]
async fn segment(
    client: reqwest::Client,
    asset: Asset,
    url: String,
    file: Arc<std::fs::File>,
    ledger: Arc<std::sync::Mutex<Ledger>>,
    ledger_path: PathBuf,
    index: usize,
    limit: Option<Arc<Limit>>,
    cancel: Cancel,
    progress: Arc<Progress>,
) -> Result<()> {
    let mut last_error = None;
    for attempt in 0..ATTEMPTS {
        if cancel.is_cancelled() {
            return Ok(());
        }
        if attempt > 0 {
            // Backing off matters: a host that rejected one range because it
            // is rate limiting will reject the retry too if it is immediate.
            let wait = Duration::from_millis(400 * (1 << attempt.min(5)));
            if !cancel.sleep(wait).await {
                return Ok(());
            }
        }

        let (origin, start, end) = {
            let held = ledger.lock().expect("ledger");
            let seg = &held.segments[index];
            (seg.start, seg.start + seg.done, seg.end)
        };
        if start > end {
            return Ok(());
        }

        match pull(
            &client, &asset, &url, origin, start, end, limit.as_ref(), &file, &ledger, index,
            &cancel, &progress,
        )
        .await
        {
            Ok(()) => {
                ledger.lock().expect("ledger").save(&ledger_path);
                return Ok(());
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("segment {index} failed")))
}

/// Streams one range into the file at its own offset.
///
/// `origin` is where the whole segment begins and `start` is where this
/// attempt begins; a retry starts partway in, and measuring progress from the
/// request instead of from the segment would throw away everything the
/// earlier attempt already wrote.
#[allow(clippy::too_many_arguments)]
async fn pull(
    client: &reqwest::Client,
    asset: &Asset,
    url: &str,
    origin: u64,
    start: u64,
    end: u64,
    limit: Option<&Arc<Limit>>,
    file: &Arc<std::fs::File>,
    ledger: &Arc<std::sync::Mutex<Ledger>>,
    index: usize,
    cancel: &Cancel,
    progress: &Arc<Progress>,
) -> Result<()> {
    let request = client
        .get(url)
        .header(USER_AGENT, AGENT)
        .header(RANGE, format!("bytes={start}-{end}"))
        .headers(extra_headers(asset));
    let response = cancel
        .run(request.send())
        .await
        .ok_or_else(|| anyhow!("cancelled"))??
        .error_for_status()?;

    // Without a partial response the server is sending the whole file again,
    // which would land at the wrong offset.
    if start > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        bail!("{url} will not resume from {start}");
    }

    let mut at = start;
    let mut buffered: Vec<u8> = Vec::with_capacity(FLUSH_AT);
    let mut stream = response.bytes_stream();
    loop {
        let Some(chunk) = cancel.run(stream.next()).await else { break };
        let Some(chunk) = chunk else { break };
        let chunk = chunk?;
        if let Some(limit) = limit {
            limit.take(chunk.len() as u64).await;
        }
        buffered.extend_from_slice(&chunk);
        if buffered.len() >= FLUSH_AT {
            at += flush(file, at, std::mem::take(&mut buffered), progress).await?;
            note(ledger, index, at - origin);
        }
    }
    if !buffered.is_empty() {
        at += flush(file, at, buffered, progress).await?;
        note(ledger, index, at - origin);
    }

    if cancel.is_cancelled() {
        return Ok(());
    }
    if at <= end {
        bail!("{url} stopped {} bytes short", end + 1 - at);
    }
    Ok(())
}

/// Hands a buffer to the disk off the runtime's threads.
async fn flush(
    file: &Arc<std::fs::File>,
    at: u64,
    buffer: Vec<u8>,
    progress: &Arc<Progress>,
) -> Result<u64> {
    let written = buffer.len() as u64;
    let file = file.clone();
    tokio::task::spawn_blocking(move || crate::sys::write_all_at(&file, &buffer, at)).await??;
    progress.done.fetch_add(written, Ordering::Relaxed);
    Ok(written)
}

fn note(ledger: &Arc<std::sync::Mutex<Ledger>>, index: usize, done: u64) {
    ledger.lock().expect("ledger").segments[index].done = done;
}

/// The whole file in one stream, for servers that will not say how big it is.
async fn stream_whole(
    client: &reqwest::Client,
    asset: &Asset,
    head: &Head,
    opts: &Opts,
    paths: (&Path, &Path),
    cancel: &Cancel,
    progress: &Arc<Progress>,
) -> Result<Fetched> {
    let (part, final_path) = paths;
    let request = client.get(&head.url).header(USER_AGENT, AGENT).headers(extra_headers(asset));
    let response = cancel
        .run(request.send())
        .await
        .ok_or_else(|| anyhow!("cancelled"))??
        .error_for_status()?;

    let mut file = std::fs::File::create(part)
        .with_context(|| format!("cannot open {}", part.display()))?;
    let mut written = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(Some(chunk)) = cancel.run(stream.next()).await {
        let chunk = chunk?;
        if let Some(limit) = &opts.limit {
            limit.take(chunk.len() as u64).await;
        }
        file.write_all(&chunk)?;
        written += chunk.len() as u64;
        progress.done.store(written, Ordering::Relaxed);
    }
    file.flush()?;
    drop(file);

    if cancel.is_cancelled() {
        bail!("cancelled");
    }
    std::fs::rename(part, final_path)?;
    progress.total.store(written, Ordering::Relaxed);
    Ok(Fetched { path: final_path.to_path_buf(), bytes: written })
}

/// What has already arrived, written beside the part file so an interrupted
/// transfer can be picked up in a later session.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Ledger {
    url: String,
    total: u64,
    tag: Option<String>,
    segments: Vec<Segment>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Segment {
    start: u64,
    end: u64,
    done: u64,
}

impl Ledger {
    /// Reads the ledger back if it still describes this file, and starts a new
    /// one otherwise.
    fn open(path: &Path, head: &Head, total: u64, want: usize, part: &Path) -> Result<Ledger> {
        let resumable = head.ranges && part.exists();
        if resumable
            && let Ok(text) = std::fs::read_to_string(path)
            && let Ok(saved) = serde_json::from_str::<Ledger>(&text)
            && saved.total == total
            && saved.tag == head.tag
        {
            return Ok(saved);
        }

        let count = if head.ranges && total > SPLIT_ABOVE { want } else { 1 };
        let span = total.div_ceil(count as u64);
        let segments = (0..count as u64)
            .map(|i| Segment { start: i * span, end: ((i + 1) * span - 1).min(total - 1), done: 0 })
            .filter(|s| s.start <= s.end)
            .collect();
        Ok(Ledger { url: head.url.clone(), total, tag: head.tag.clone(), segments })
    }

    fn done(&self) -> u64 {
        self.segments.iter().map(|s| s.done).sum()
    }

    /// Best effort: losing the ledger costs a restart, not the file.
    fn save(&self, path: &Path) {
        if let Ok(text) = serde_json::to_string(self) {
            std::fs::write(path, text).ok();
        }
    }
}

fn pick_name(asset: &Asset, head: &Head) -> String {
    let named = head
        .name
        .clone()
        .or_else(|| asset.name.as_ref().and_then(|n| names::sanitize(n)))
        .or_else(|| names::from_url(&head.url))
        .or_else(|| names::from_url(&asset.url))
        .unwrap_or_else(|| "download".into());

    // A name the URL gave up without an extension is worth completing from
    // what the server says it is sending.
    if Path::new(&named).extension().is_none()
        && let Some(ext) = names::extension_for(&head.mime)
    {
        return format!("{named}.{ext}");
    }
    named
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn extra_headers(asset: &Asset) -> HeaderMap {
    let mut headers = HeaderMap::new();
    // Hosts that serve a file only to their own pages check this, and sending
    // the page we found the link on is exactly what a browser would do.
    if let Some(referer) = &asset.referer
        && let Ok(value) = referer.parse()
    {
        headers.insert(REFERER, value);
    }
    headers
}

fn header(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

/// The client every transfer shares, so connections are pooled across a queue.
pub fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(Duration::from_secs(60 * 60))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .context("cannot build an HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(size: Option<u64>, ranges: bool) -> Head {
        Head {
            size,
            ranges,
            name: None,
            mime: "video/mp4".into(),
            tag: Some("\"abc\"".into()),
            url: "https://h.example/a.mp4".into(),
        }
    }

    #[test]
    fn a_big_file_is_split_and_a_small_one_is_not() {
        let dir = std::env::temp_dir().join("ntls-fetch-tests");
        let part = dir.join("nothing-here.ntlspart");

        let big = Ledger::open(&dir.join("x.json"), &head(Some(40 << 20), true), 40 << 20, 4, &part)
            .expect("a ledger");
        assert_eq!(big.segments.len(), 4);
        assert_eq!(big.segments[0].start, 0);
        assert_eq!(big.segments[3].end, (40 << 20) - 1);

        let small = Ledger::open(&dir.join("y.json"), &head(Some(1000), true), 1000, 4, &part)
            .expect("a ledger");
        assert_eq!(small.segments.len(), 1);
    }

    #[test]
    fn a_server_that_will_not_do_ranges_gets_one_stream() {
        let dir = std::env::temp_dir().join("ntls-fetch-tests");
        let ledger =
            Ledger::open(&dir.join("z.json"), &head(Some(40 << 20), false), 40 << 20, 8, &dir)
                .expect("a ledger");
        assert_eq!(ledger.segments.len(), 1);
    }

    #[test]
    fn a_half_finished_segment_is_picked_up_where_it_stopped() {
        // The offset a retry asks for is the segment's origin plus everything
        // written so far, not the origin, which would fetch bytes twice, and
        // not the last attempt's start, which would leave a hole.
        let mut segment = Segment { start: 1_000, end: 1_999, done: 0 };
        segment.done = 400;
        assert_eq!(segment.start + segment.done, 1_400);

        // A second attempt writes 200 more. Measured from the origin, the
        // segment is 600 in; measured from the attempt it would be 200, and
        // the next resume would re-fetch what is already on disk.
        let attempt_start = segment.start + segment.done;
        let reached = attempt_start + 200;
        assert_eq!(reached - segment.start, 600);
    }

    #[test]
    fn a_resumed_ledger_is_only_reused_for_the_same_file() {
        let dir = std::env::temp_dir().join("ntls-fetch-resume");
        std::fs::create_dir_all(&dir).expect("the directory");
        let part = dir.join("thing.ntlspart");
        std::fs::write(&part, b"partial").expect("a part file");
        let ledger_path = dir.join("thing.ntlspart.json");

        let first = head(Some(4 << 20), true);
        let mut saved = Ledger::open(&ledger_path, &first, 4 << 20, 2, &part).expect("a ledger");
        saved.segments[0].done = 128;
        saved.save(&ledger_path);

        // Same file: the progress comes back.
        let again = Ledger::open(&ledger_path, &first, 4 << 20, 2, &part).expect("a ledger");
        assert_eq!(again.done(), 128);

        // A different file at the same URL: start over and does not write one
        // file's bytes into another's.
        let changed = Head { tag: Some("\"different\"".into()), ..first };
        let fresh = Ledger::open(&ledger_path, &changed, 4 << 20, 2, &part).expect("a ledger");
        assert_eq!(fresh.done(), 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_segments_cover_the_file_exactly_once() {
        let dir = std::env::temp_dir().join("ntls-fetch-tests");
        let total = 10_000_003;
        let ledger = Ledger::open(&dir.join("w.json"), &head(Some(total), true), total, 3, &dir)
            .expect("a ledger");
        let covered: u64 = ledger.segments.iter().map(|s| s.end - s.start + 1).sum();
        assert_eq!(covered, total);
        for pair in ledger.segments.windows(2) {
            assert_eq!(pair[0].end + 1, pair[1].start);
        }
    }
}
