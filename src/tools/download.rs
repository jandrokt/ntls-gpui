//! Fetching things off the web.
//!
//! Paste in one link or twenty and this works out what each is, queues them,
//! and fetches them several at a time with a row per file that rewrites itself
//! as the transfer moves. Which sites are supported is not a question this
//! file answers — see [`crate::dl`], which asks the extractors other people
//! maintain and otherwise reads the page.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::cells;
use crate::core::tool::BoxFuture;
use crate::core::{
    Column, Emitter, Event, Expand, Field, Level, Opt, Role, Run, Status, Tool,
    Validator, bar, col,
};
use crate::dl::external::{self, Extractor};
use crate::dl::fetch::{self, Opts, Progress};
use crate::dl::names;
use crate::dl::resolve::{self, Plan, Prefer};

pub struct Download;

/// How often a transfer in flight redraws. Fast enough to look live, slow
/// enough that a queue of twenty does not flood the interface.
const TICK: Duration = Duration::from_millis(400);

impl Tool for Download {
    fn id(&self) -> &'static str {
        "download"
    }

    fn title(&self) -> &'static str {
        "Download"
    }

    fn desc(&self) -> &'static str {
        "Fetch files, media and galleries from a URL"
    }

    fn icon(&self) -> &'static str {
        "download"
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            Field::text("urls", "Links", "One or more URLs, separated by spaces or newlines")
                .role(Role::Target)
                .placeholder("https://www.mediafire.com/file/…")
                .validate(Validator::Required),
            Field::text("dest", "Save to", "Destination directory")
                .default("~/Downloads/ntls")
                .expand(Expand::Downloads),
            Field::select(
                "engine",
                "Route",
                "How to resolve the URL",
                "auto",
                vec![
                    Opt::new("auto", "Automatic", "Probe the URL, query the extractors, then parse the page"),
                    Opt::new("direct", "Direct", "Treat the URL as the file"),
                    Opt::new("ytdlp", "yt-dlp", "Media sites, via yt-dlp"),
                    Opt::new("gallerydl", "gallery-dl", "Image galleries, via gallery-dl"),
                    Opt::new("page", "Read the page", "Parse the page markup only"),
                ],
            ),
            Field::select(
                "quality",
                "Quality",
                "Which stream to take when several are offered",
                "best",
                vec![
                    Opt::new("best", "Best", "Highest available"),
                    Opt::new("1080", "1080p", "Capped at 1080p"),
                    Opt::new("720", "720p", "Capped at 720p"),
                    Opt::new("audio", "Audio only", "Extract audio as MP3"),
                ],
            ),
            Field::select(
                "connections",
                "Connections",
                "Ranged requests per file",
                "4",
                vec![
                    Opt::new("1", "1", "Single stream"),
                    Opt::new("2", "2", ""),
                    Opt::new("4", "4", "Default"),
                    Opt::new("8", "8", ""),
                    Opt::new("16", "16", "Maximum; some hosts refuse"),
                ],
            ),
            Field::select(
                "parallel",
                "At once",
                "Files fetched concurrently",
                "2",
                vec![
                    Opt::new("1", "1", "Sequential"),
                    Opt::new("2", "2", "Default"),
                    Opt::new("4", "4", ""),
                    Opt::new("8", "8", ""),
                ],
            ),
            Field::text(
                "limit",
                "Speed limit",
                "Ceiling for the whole queue: 2MB, 500KB, or blank for none",
            )
            .placeholder("no limit")
            .validate(Validator::Rate),
            Field::select(
                "subdir",
                "Put in a folder",
                "Group downloads into subfolders",
                "none",
                vec![
                    Opt::new("none", "No", "All files in the destination folder"),
                    Opt::new("host", "By site", "One folder per source site"),
                    Opt::new("date", "By date", "One folder per day, named YYYY-MM-DD"),
                ],
            ),
            Field::boolean("overwrite", "Overwrite", "Replace an existing file of the same name", false),
            Field::boolean(
                "keep_partial",
                "Keep part files",
                "Keep stopped transfers on disk so they can be resumed",
                true,
            ),
        ]
    }

    fn columns(&self) -> Vec<Column> {
        vec![
            col("File", 34),
            col("Host", 20),
            col("Size", 10),
            col("Rate", 10),
            bar("Progress", 0),
        ]
    }

    /// A queue picks up where it left off: the files already on disk are
    /// skipped and the rest are fetched.
    fn resumable(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, run: Run, emit: Emitter) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { download(run, emit).await })
    }
}

async fn download(run: Run, emit: Emitter) -> anyhow::Result<()> {
    let params = &run.params;
    let links = resolve::split_links(params.raw("urls"));
    if links.is_empty() {
        anyhow::bail!("no links to fetch");
    }

    let root = names::expand_home(params.raw("dest"));
    let dir = match params.str("subdir").as_str() {
        // Sorting by day is what turns a downloads folder used every week into
        // one you can still find things in.
        "date" => root.join(names::today()),
        _ => root.clone(),
    };
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("cannot use {}: {e}", dir.display()))?;

    let limit = crate::core::field::parse_rate(params.raw("limit")).map(fetch::Limit::new);
    let opts = Opts {
        dir: dir.clone(),
        connections: params.usize("connections", 4),
        overwrite: params.bool("overwrite"),
        limit: limit.clone(),
        keep_partial: params.bool("keep_partial"),
    };
    let by_host = params.str("subdir") == "host";
    let prefer = Prefer::parse(&params.str("engine"));
    let quality = params.str("quality");
    let at_once = params.usize("parallel", 2).clamp(1, 8);

    emit.info(format!("Destination: {}", dir.display()));
    emit.info(external::availability());
    if let Some(rate) = crate::core::field::parse_rate(params.raw("limit")) {
        emit.info(format!("Rate limit: {}", names::rate(rate as f64)));
    }
    if run.resuming() {
        emit.info(format!("Skipping {} URL(s) already fetched", run.done.len()));
    }

    let client = fetch::client()?;
    let tally = Arc::new(Tally::default());
    let queue = Arc::new(tokio::sync::Semaphore::new(at_once));
    let pending: Vec<String> = links.into_iter().filter(|l| !run.done.contains(l)).collect();
    let total = pending.len();

    tally.total.store(total, Ordering::Relaxed);
    emit.progress(0, total);

    let reporting = watch_queue(&emit, tally.clone(), total);

    let mut running = futures::stream::FuturesUnordered::new();
    for link in pending {
        let permit = queue.clone();
        let (client, emit, opts, quality, cancel, tally) = (
            client.clone(),
            emit.clone(),
            opts.clone(),
            quality.clone(),
            run.cancel.clone(),
            tally.clone(),
        );
        running.push(async move {
            let _slot = permit.acquire_owned().await;
            if cancel.is_cancelled() {
                return;
            }
            // A folder per site is decided per link, since that is where the
            // site's name comes from.
            let opts = if by_host {
                let host = host_of(&link);
                Opts { dir: opts.dir.join(names::sanitize(&host).unwrap_or(host)), ..opts }
            } else {
                opts
            };
            one(&client, &link, prefer, &quality, &opts, &cancel, &emit, &tally).await;
        });
    }

    use futures::StreamExt as _;
    while running.next().await.is_some() {
        emit.progress(tally.finished.load(Ordering::Relaxed), total);
    }
    reporting.abort();

    tally.rate.store(0, Ordering::Relaxed);
    emit.stats(tally.stats());
    emit.progress(tally.finished.load(Ordering::Relaxed), total);
    let (ok, failed) = (tally.ok.load(Ordering::Relaxed), tally.failed.load(Ordering::Relaxed));
    if run.cancel.is_cancelled() {
        emit.warn(format!("Stopped after {ok} file(s)"));
        return Ok(());
    }
    if failed > 0 {
        emit.warn(format!("{ok} succeeded, {failed} failed"));
    } else {
        emit.good(format!("{ok} file(s) saved to {}", dir.display()));
    }
    Ok(())
}

/// One pasted link, from working out what it is to the last byte of it.
#[allow(clippy::too_many_arguments)]
async fn one(
    client: &reqwest::Client,
    link: &str,
    prefer: Prefer,
    quality: &str,
    opts: &Opts,
    cancel: &crate::core::Cancel,
    emit: &Emitter,
    tally: &Arc<Tally>,
) {
    let label = guess_name(link);
    row(
        emit,
        Line {
            key: link,
            status: Status::Info,
            name: &label,
            host: host_of(link),
            size: "—".into(),
            rate: "—".into(),
            fraction: 0.0,
        },
    );

    let plan = match resolve::resolve(client, link, prefer, cancel, emit).await {
        Ok(plan) => plan,
        Err(e) => return fail(emit, link, &label, &e.to_string(), tally),
    };

    match plan {
        Plan::Direct(assets) if assets.is_empty() => {
            fail(emit, link, &label, "nothing to fetch", tally)
        }
        Plan::Direct(assets) => {
            let many = assets.len() > 1;
            if many {
                emit.info(format!("{label}: {} files", assets.len()));
            }
            for (index, asset) in assets.iter().enumerate() {
                if cancel.is_cancelled() {
                    return;
                }
                // A gallery is many files behind one link, so each gets its
                // own row rather than sharing the pasted link's.
                let key = if many { format!("{link}#{index}") } else { link.to_string() };
                transfer(client, &key, asset, opts, cancel, emit, tally).await;
            }
        }
        Plan::Handled { by, url, title } => {
            handled(by, &url, &title, quality, opts, cancel, emit, tally, link).await
        }
    }
}

/// A transfer this program performs itself.
async fn transfer(
    client: &reqwest::Client,
    key: &str,
    asset: &crate::dl::Asset,
    opts: &Opts,
    cancel: &crate::core::Cancel,
    emit: &Emitter,
    tally: &Arc<Tally>,
) {
    let progress = Arc::new(Progress::default());
    let label = asset
        .name
        .clone()
        .or_else(|| names::from_url(&asset.url))
        .unwrap_or_else(|| resolve::short(&asset.url));
    tally.begin(key, &progress);
    let watching = watch(emit, key, &label, host_of(&asset.url), progress.clone());

    let outcome = fetch::fetch(client, asset, opts, cancel, &progress).await;
    watching.abort();
    tally.end(key);

    match outcome {
        Ok(done) => {
            let name = done
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| label.clone());
            row(
                emit,
                Line {
                    key,
                    status: Status::Up,
                    name: &name,
                    host: host_of(&asset.url),
                    size: names::bytes(done.bytes),
                    rate: "done".into(),
                    fraction: 1.0,
                },
            );
            emit.good(format!("{name} — {}", names::bytes(done.bytes)));
            tally.ok.fetch_add(1, Ordering::Relaxed);
            tally.bytes.fetch_add(done.bytes, Ordering::Relaxed);
        }
        Err(e) if cancel.is_cancelled() => {
            row(
                emit,
                Line {
                    key,
                    status: Status::Warn,
                    name: &label,
                    host: host_of(&asset.url),
                    size: "—".into(),
                    rate: "stopped".into(),
                    fraction: partial(&progress),
                },
            );
            let _ = e;
        }
        Err(e) => fail(emit, key, &label, &e.to_string(), tally),
    }
    tally.finished.fetch_add(1, Ordering::Relaxed);
    emit.stats(tally.stats());
}

/// A transfer the media extractor performs, because what is behind the link is
/// often several streams that have to be put back together.
#[allow(clippy::too_many_arguments)]
async fn handled(
    by: Extractor,
    url: &str,
    title: &str,
    quality: &str,
    opts: &Opts,
    cancel: &crate::core::Cancel,
    emit: &Emitter,
    tally: &Arc<Tally>,
    key: &str,
) {
    let label = if title.is_empty() { resolve::short(url) } else { title.to_string() };
    let progress = Arc::new(Progress::default());
    tally.begin(key, &progress);
    let watching = watch(emit, key, &label, host_of(url), progress.clone());

    let relay = emit.clone();
    let outcome = match by {
        Extractor::YtDlp => {
            external::ytdlp_download(url, &opts.dir, quality, cancel, &progress, move |line| {
                relay.emit(Event::log(Level::Info, line));
            })
            .await
        }
        // The gallery extractor never gets here: it hands over plain URLs.
        Extractor::GalleryDl => Err(anyhow::anyhow!("unsupported route")),
    };
    watching.abort();
    tally.end(key);

    match outcome {
        Ok(files) => {
            let size: u64 = files.iter().filter_map(|p| p.metadata().ok()).map(|m| m.len()).sum();
            let name = files
                .first()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| label.clone());
            let shown = if files.len() > 1 { format!("{name} +{}", files.len() - 1) } else { name };
            row(
                emit,
                Line {
                    key,
                    status: Status::Up,
                    name: &shown,
                    host: host_of(url),
                    size: names::bytes(size),
                    rate: "done".into(),
                    fraction: 1.0,
                },
            );
            emit.good(format!("{shown} — {}", names::bytes(size)));
            tally.ok.fetch_add(1, Ordering::Relaxed);
            tally.bytes.fetch_add(size, Ordering::Relaxed);
        }
        Err(e) if cancel.is_cancelled() => {
            row(
                emit,
                Line {
                    key,
                    status: Status::Warn,
                    name: &label,
                    host: host_of(url),
                    size: "—".into(),
                    rate: "stopped".into(),
                    fraction: partial(&progress),
                },
            );
            let _ = e;
        }
        Err(e) => fail(emit, key, &label, &e.to_string(), tally),
    }
    tally.finished.fetch_add(1, Ordering::Relaxed);
    emit.stats(tally.stats());
}

/// Redraws one row while its transfer is in flight, working the rate out from
/// what arrived between two ticks.
fn watch(
    emit: &Emitter,
    key: &str,
    label: &str,
    host: String,
    progress: Arc<Progress>,
) -> tokio::task::JoinHandle<()> {
    let (emit, key, label) = (emit.clone(), key.to_string(), label.to_string());
    tokio::spawn(async move {
        let mut last = (Instant::now(), 0u64);
        loop {
            tokio::time::sleep(TICK).await;
            let done = progress.done.load(Ordering::Relaxed);
            let total = progress.total.load(Ordering::Relaxed);
            let now = Instant::now();
            let elapsed = now.duration_since(last.0).as_secs_f64();
            let rate =
                if elapsed > 0.0 { done.saturating_sub(last.1) as f64 / elapsed } else { 0.0 };
            last = (now, done);

            row(
                &emit,
                Line {
                    key: &key,
                    status: Status::Info,
                    name: &label,
                    host: host.clone(),
                    size: if total > 0 { names::bytes(total) } else { names::bytes(done) },
                    rate: names::rate(rate),
                    fraction: if total > 0 { done as f32 / total as f32 } else { 0.0 },
                },
            );
        }
    })
}

/// Reports on the queue as a whole: one chart of the combined rate, and the
/// summary strip.
///
/// It is one task rather than one per transfer because a chart of eight
/// overlapping series is not a chart of how fast the queue is going, which is
/// the only rate anyone asks about.
fn watch_queue(emit: &Emitter, tally: Arc<Tally>, total: usize) -> tokio::task::JoinHandle<()> {
    let emit = emit.clone();
    tokio::spawn(async move {
        let mut last = (Instant::now(), 0u64);
        loop {
            tokio::time::sleep(TICK).await;
            let moved = tally.moved();
            let now = Instant::now();
            let elapsed = now.duration_since(last.0).as_secs_f64();
            let rate =
                if elapsed > 0.0 { moved.saturating_sub(last.1) as f64 / elapsed } else { 0.0 };
            last = (now, moved);

            tally.rate.store(rate as u64, Ordering::Relaxed);
            emit.emit(Event::sample("Rate", " B/s", rate));
            emit.stats(tally.stats());
            emit.progress(tally.finished.load(Ordering::Relaxed), total);
        }
    })
}

/// One line of the transfer table. It is rewritten in place as the transfer
/// moves, so a queue of twenty files is twenty rows rather than a log of every
/// state each of them passed through.
struct Line<'a> {
    key: &'a str,
    status: Status,
    name: &'a str,
    host: String,
    size: String,
    rate: String,
    fraction: f32,
}

fn row(emit: &Emitter, line: Line<'_>) {
    emit.upsert(
        line.status,
        line.key,
        cells![line.name, line.host, line.size, line.rate, format!("{:.4}", line.fraction)],
    );
}

fn fail(emit: &Emitter, key: &str, label: &str, why: &str, tally: &Arc<Tally>) {
    emit.err(format!("{label}: {why}"));
    emit.upsert(Status::Down, key, cells![label, "—", "—", "failed", "0"]);
    tally.failed.fetch_add(1, Ordering::Relaxed);
    tally.finished.fetch_add(1, Ordering::Relaxed);
}

fn partial(progress: &Arc<Progress>) -> f32 {
    progress.fraction().unwrap_or(0.0)
}

/// What to call a link before anything is known about it.
///
/// The row exists from the moment the link is queued, and a column of
/// identical URL prefixes says nothing; the file name in the path usually
/// does. The server has the last word once the transfer starts.
fn guess_name(link: &str) -> String {
    let path = link.split(['?', '#']).next().unwrap_or(link);
    let named = path
        .rsplit('/')
        .filter(|segment| !segment.is_empty())
        .find(|segment| segment.contains('.') && !segment.ends_with('.'));
    match named {
        Some(name) => names::sanitize(name).unwrap_or_else(|| host_of(link)),
        None => host_of(link),
    }
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_else(|| "—".into())
}

/// What the summary strip counts, and what the whole queue has moved.
#[derive(Default)]
struct Tally {
    total: AtomicUsize,
    finished: AtomicUsize,
    ok: AtomicUsize,
    failed: AtomicUsize,
    /// Bytes belonging to transfers that have finished.
    bytes: AtomicU64,
    rate: AtomicU64,
    /// The transfers still in flight, so the queue's rate counts the bytes
    /// arriving now and not only the files that have landed.
    live: std::sync::Mutex<Vec<(String, Arc<Progress>)>>,
}

impl Tally {
    fn begin(&self, key: &str, progress: &Arc<Progress>) {
        self.live.lock().expect("live").push((key.to_string(), progress.clone()));
    }

    fn end(&self, key: &str) {
        self.live.lock().expect("live").retain(|(k, _)| k != key);
    }

    /// Every byte this queue has written: the finished files, plus however
    /// far the ones still going have got.
    fn moved(&self) -> u64 {
        let in_flight: u64 = self
            .live
            .lock()
            .expect("live")
            .iter()
            .map(|(_, p)| p.done.load(Ordering::Relaxed))
            .sum();
        self.bytes.load(Ordering::Relaxed) + in_flight
    }

    fn stats(&self) -> Vec<crate::core::Kv> {
        vec![
            crate::core::kv("queued", self.total.load(Ordering::Relaxed).to_string()),
            crate::core::kv("done", self.ok.load(Ordering::Relaxed).to_string()),
            crate::core::kv("failed", self.failed.load(Ordering::Relaxed).to_string()),
            crate::core::kv("fetched", names::bytes(self.moved())),
            crate::core::kv("rate", names::rate(self.rate.load(Ordering::Relaxed) as f64)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queued_link_is_named_after_the_file_in_its_path() {
        assert_eq!(
            guess_name("https://www.mediafire.com/file/abc/holiday.rar/file"),
            "holiday.rar"
        );
        assert_eq!(guess_name("https://cdn.example/a/b/clip.mp4?token=1"), "clip.mp4");
        // Nothing in the path to go on, so the site will have to do.
        assert_eq!(guess_name("https://example.com/watch"), "example.com");
    }

    #[test]
    fn the_host_column_reads_like_a_site_name() {
        assert_eq!(host_of("https://www.mediafire.com/file/abc"), "mediafire.com");
        assert_eq!(host_of("not a url"), "—");
    }

    /// A real transfer, end to end: resolve a link, split it, write it, and
    /// check what landed. Ignored by default because it needs the network.
    #[test]
    #[ignore = "needs the network"]
    fn a_direct_link_is_fetched_and_lands_whole() {
        let dir = std::env::temp_dir().join("ntls-download-test");
        std::fs::remove_dir_all(&dir).ok();
        let runtime = tokio::runtime::Runtime::new().expect("a runtime");
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = events.clone();
        let emit = Emitter::new(move |e| sink.lock().expect("events").push(format!("{e:?}")));

        let mut params = crate::core::Params::new();
        params.set("urls", "https://httpbin.org/bytes/65536");
        params.set("dest", dir.to_str().expect("a path"));
        params.set("engine", "direct");
        params.set("connections", "4");
        params.set("parallel", "1");

        runtime
            .block_on(download(Run::fresh(crate::core::Cancel::new(), params), emit))
            .expect("the download to succeed");

        let written: Vec<_> = std::fs::read_dir(&dir)
            .expect("the directory")
            .filter_map(Result::ok)
            .filter(|e| !e.file_name().to_string_lossy().contains("ntlspart"))
            .collect();
        assert_eq!(written.len(), 1, "expected one file, got {written:?}");
        assert_eq!(written[0].metadata().expect("metadata").len(), 65536);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_progress_cell_is_a_fraction_the_table_can_draw() {
        let cells = cells!["a.zip", "h", "1 MB", "—", format!("{:.4}", 0.5f32)];
        assert_eq!(cells[4], "0.5000");
        assert_eq!(cells[4].parse::<f32>().expect("a fraction"), 0.5);
    }
}
