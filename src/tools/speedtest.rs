//! Measures download, upload and latency against a public server.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::cells;
use crate::core::{
    Cancel, Column, Emitter, Event, Field, Opt, Role, Status, Tool, Validator, VisibleIf,
    col, kv,
};
use crate::net::iface;
use crate::net::speedtest::{SpeedClient, Transfer, format_bits, format_bytes};
use crate::tools::prober::iface_field;
use crate::tools::stats::{RttStats, elapsed, ms};

pub struct SpeedTest;

/// Cloudflare runs an open, unauthenticated speed-test endpoint, so this
/// works without an account or an API key.
const CF_UP: &str = "https://speed.cloudflare.com/__up";
/// The size asked for per request. Cloudflare rejects requests much above
/// 50 MB outright, and each stream simply asks again when a chunk runs out, so
/// this stays comfortably under the limit.
const DOWN_CHUNK: usize = 25 << 20;

fn cf_down(bytes: usize) -> String {
    format!("https://speed.cloudflare.com/__down?bytes={bytes}")
}

impl Tool for SpeedTest {
    fn id(&self) -> &'static str {
        "speedtest"
    }
    fn title(&self) -> &'static str {
        "Internet speed"
    }
    fn desc(&self) -> &'static str {
        "Measure download, upload and latency against a public server"
    }
    fn icon(&self) -> &'static str {
        "speedtest"
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            Field::select(
                "server",
                "Server",
                "Where to test against",
                "cloudflare",
                vec![
                    Opt::new("cloudflare", "Cloudflare", "speed.cloudflare.com, no account required"),
                    Opt::new("custom", "Custom URL", "Any URL that serves a download"),
                ],
            ),
            Field::text(
                "url",
                "Download URL",
                "A URL large enough to saturate the link for the test duration",
            )
            .placeholder("https://example.com/large-file.bin")
            .role(Role::Target)
            .visible_if(VisibleIf::Equals("server", "custom")),
            Field::select(
                "direction",
                "Direction",
                "Which way to measure",
                "both",
                vec![
                    Opt::new("both", "Down + up", "Both directions"),
                    Opt::new("down", "Download", "Download only"),
                    Opt::new("up", "Upload", "Upload only"),
                ],
            ),
            Field::text("duration", "Duration", "How long to run each direction; longer is steadier")
                .default("8s")
                .validate(Validator::Duration),
            Field::text(
                "streams",
                "Streams",
                "Parallel connections; one stream rarely fills a fast link",
            )
            .default("4")
            .validate(Validator::IntRange(1, 32)),
            iface_field(),
        ]
    }

    fn columns(&self) -> Vec<Column> {
        vec![col("TEST", 12), col("RESULT", 14), col("TRANSFERRED", 13), col("DETAIL", 0)]
    }

    fn run<'a>(
        &'a self,
        r: crate::core::Run,
        emit: Emitter,
    ) -> crate::core::tool::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { run(r, emit).await })
    }
}

/// Accumulates the figures so the stat bar keeps showing the download result
/// while the upload is still running.
#[derive(Default)]
struct Summary {
    down: String,
    up: String,
}

impl Summary {
    fn publish(&mut self, emit: &Emitter, name: &str, value: String) {
        if name == "upload" {
            self.up = value;
        } else {
            self.down = value;
        }
        let mut stats = Vec::new();
        if !self.down.is_empty() {
            stats.push(kv("download", &self.down));
        }
        if !self.up.is_empty() {
            stats.push(kv("upload", &self.up));
        }
        emit.stats(stats);
    }
}

async fn run(r: crate::core::Run, emit: Emitter) -> anyhow::Result<()> {
    let (cancel, p) = (r.cancel.clone(), r.params.clone());
    let duration = p.dur("duration", Duration::from_secs(8));
    let streams = p.usize("streams", 4).clamp(1, 32);
    let direction = p.str("direction");

    let src = iface::resolve_interface(&p.str("iface")).map_err(anyhow::Error::msg)?;
    let custom = p.str("server") == "custom";

    let (down_url, up_url) = if custom {
        let url = p.str("url");
        if url.is_empty() {
            anyhow::bail!("a custom test needs a download URL");
        }
        (url, String::new())
    } else {
        (cf_down(DOWN_CHUNK), CF_UP.to_string())
    };

    let client = SpeedClient::new(src, duration + Duration::from_secs(30))
        .map_err(anyhow::Error::msg)?;

    // Latency first: it is quick, and a link that cannot be reached at all
    // should say so before spending the duration on a transfer.
    if !custom {
        emit.info("measuring latency…");
        match cancel.run(client.latency(&cancel, &cf_down(0), 6)).await {
            None => return Ok(()),
            Some(Err(e)) => anyhow::bail!("cannot reach the speed test server: {e}"),
            Some(Ok(latencies)) => report_latency(&emit, &latencies),
        }
    }

    let summary = Arc::new(std::sync::Mutex::new(Summary::default()));
    if direction == "both" || direction == "down" {
        transfer(&cancel, &emit, &client, &summary, "download", &down_url, streams, duration, false)
            .await;
    }
    if direction == "both" || direction == "up" {
        if up_url.is_empty() {
            emit.info("a custom server has no upload endpoint, so only download was measured");
        } else {
            transfer(&cancel, &emit, &client, &summary, "upload", &up_url, streams, duration, true)
                .await;
        }
    }

    if !cancel.is_cancelled() {
        emit.good("speed test complete");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn transfer(
    cancel: &Cancel,
    emit: &Emitter,
    client: &SpeedClient,
    summary: &Arc<std::sync::Mutex<Summary>>,
    name: &str,
    url: &str,
    streams: usize,
    duration: Duration,
    upload: bool,
) {
    emit.info(format!("{name}: {streams} stream(s) for {}…", elapsed(duration)));

    let series = format!("{name} throughput");
    let sampler: crate::net::speedtest::Sampler = {
        let (emit, summary, name) = (emit.clone(), summary.clone(), name.to_string());
        Arc::new(move |bytes, interval| {
            if interval.is_zero() {
                return;
            }
            let mbps = bytes as f64 * 8.0 / interval.as_secs_f64() / 1e6;
            emit.emit(Event::sample(series.clone(), " Mbps", mbps));
            summary.lock().unwrap().publish(&emit, &name, format_bits(mbps * 1e6));
        })
    };

    // A steady progress bar while a transfer runs: the tool knows how long it
    // means to take, so the bar tracks time and not bytes.
    let running = Arc::new(AtomicBool::new(true));
    let bar = {
        let (emit, running, cancel) = (emit.clone(), running.clone(), cancel.clone());
        let start = Instant::now();
        tokio::spawn(async move {
            while running.load(Ordering::Relaxed) && cancel.sleep(Duration::from_millis(200)).await {
                let el = start.elapsed().min(duration);
                emit.emit(Event::progress_label(
                    el.as_millis() as usize,
                    duration.as_millis() as usize,
                    format!("{} of {}", elapsed(el), elapsed(duration)),
                ));
            }
        })
    };

    let result = if upload {
        client.upload(cancel, url, streams, duration, Some(sampler)).await
    } else {
        client.download(cancel, url, streams, duration, Some(sampler)).await
    };
    running.store(false, Ordering::Relaxed);
    bar.abort();

    match result {
        Err(e) => {
            if !cancel.is_cancelled() {
                emit.row(Status::Down, url, cells![name, "failed", "-", e]);
            }
        }
        Ok(t) => {
            emit.emit(Event::progress_label(1, 1, format!("{} of {}", elapsed(duration), elapsed(duration))));
            report(emit, name, url, streams, t);
        }
    }
}

fn report(emit: &Emitter, name: &str, url: &str, streams: usize, t: Transfer) {
    emit.row(
        Status::Up,
        url,
        cells![
            name,
            format_bits(t.bits_per_second()),
            format_bytes(t.bytes),
            format!("{streams} stream(s) over {}", elapsed(t.elapsed))
        ],
    );
}

fn report_latency(emit: &Emitter, latencies: &[Duration]) {
    let mut st = RttStats::default();
    for l in latencies {
        st.send();
        st.add(*l);
    }
    emit.row(
        Status::Info,
        "",
        cells![
            "latency",
            ms(st.avg()),
            "-",
            format!(
                "min {} · max {} · jitter {} over {} requests",
                ms(st.min),
                ms(st.max),
                ms(st.stddev()),
                st.recv
            )
        ],
    );
}
