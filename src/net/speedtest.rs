//! Throughput against an HTTP endpoint.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::core::Cancel;

/// The outcome of a throughput measurement.
#[derive(Clone, Copy, Debug, Default)]
pub struct Transfer {
    pub bytes: u64,
    pub elapsed: Duration,
}

impl Transfer {
    /// The headline figure: throughput in bits per second, the unit
    /// connections are actually sold in.
    pub fn bits_per_second(&self) -> f64 {
        if self.elapsed.is_zero() {
            return 0.0;
        }
        self.bytes as f64 * 8.0 / self.elapsed.as_secs_f64()
    }
}

/// How often throughput is reported while a test runs.
pub const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

/// Called with the bytes moved since the previous call and the interval they
/// were moved in. That feeds the live chart.
pub type Sampler = Arc<dyn Fn(u64, Duration) + Send + Sync>;

/// Measures throughput against an HTTP endpoint.
pub struct SpeedClient {
    client: reqwest::Client,
}

impl SpeedClient {
    /// Builds a client bound to a source address when one is given.
    pub fn new(src: Option<std::net::Ipv4Addr>, timeout: Duration) -> Result<SpeedClient, String> {
        let mut b = reqwest::Client::builder()
            .user_agent("ntls")
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(15))
            // Compression would measure the compressor, not the link.
            .no_gzip()
            .no_brotli()
            .pool_max_idle_per_host(64);
        if let Some(src) = src {
            b = b.local_address(std::net::IpAddr::V4(src));
        }
        let client = b.build().map_err(|e| e.to_string())?;
        Ok(SpeedClient { client })
    }

    /// Pulls from `url` with the given number of parallel streams until
    /// `duration` elapses.
    pub async fn download(
        &self,
        cancel: &Cancel,
        url: &str,
        streams: usize,
        duration: Duration,
        sample: Option<Sampler>,
    ) -> Result<Transfer, String> {
        use futures::StreamExt;

        let counted = Arc::new(AtomicU64::new(0));
        let deadline = Instant::now() + duration;
        let start = Instant::now();
        let sampler = spawn_sampler(cancel.clone(), counted.clone(), sample, deadline);

        let mut set = Vec::new();
        for _ in 0..streams.max(1) {
            let (client, url, counted, cancel) =
                (self.client.clone(), url.to_string(), counted.clone(), cancel.clone());
            set.push(tokio::spawn(async move {
                let mut first_err: Option<String> = None;
                while Instant::now() < deadline && !cancel.is_cancelled() {
                    let resp = match client.get(&url).send().await {
                        Ok(r) => r,
                        Err(e) => {
                            first_err.get_or_insert(e.to_string());
                            break;
                        }
                    };
                    if !resp.status().is_success() {
                        first_err.get_or_insert(format!("server returned {}", resp.status()));
                        break;
                    }
                    let mut body = resp.bytes_stream();
                    loop {
                        if Instant::now() >= deadline || cancel.is_cancelled() {
                            return first_err;
                        }
                        match body.next().await {
                            // A cancelled read is the normal way this ends.
                            Some(Ok(chunk)) => {
                                counted.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                            }
                            Some(Err(_)) | None => break,
                        }
                    }
                }
                first_err
            }));
        }

        let mut first_err = None;
        for h in set {
            if let Ok(Some(e)) = h.await {
                first_err.get_or_insert(e);
            }
        }
        sampler.abort();

        let total = counted.load(Ordering::Relaxed);
        match (total, first_err) {
            (0, Some(e)) => Err(e),
            _ => Ok(Transfer { bytes: total, elapsed: start.elapsed() }),
        }
    }

    /// Pushes generated data to `url` until `duration` elapses.
    pub async fn upload(
        &self,
        cancel: &Cancel,
        url: &str,
        streams: usize,
        duration: Duration,
        sample: Option<Sampler>,
    ) -> Result<Transfer, String> {
        let counted = Arc::new(AtomicU64::new(0));
        let deadline = Instant::now() + duration;
        let start = Instant::now();
        let sampler = spawn_sampler(cancel.clone(), counted.clone(), sample, deadline);

        let mut set = Vec::new();
        for _ in 0..streams.max(1) {
            let (client, url, counted, cancel) =
                (self.client.clone(), url.to_string(), counted.clone(), cancel.clone());
            set.push(tokio::spawn(async move {
                let mut first_err: Option<String> = None;
                while Instant::now() < deadline && !cancel.is_cancelled() {
                    let body = reqwest::Body::wrap_stream(upload_stream(
                        counted.clone(),
                        deadline,
                        cancel.clone(),
                    ));
                    match client
                        .post(&url)
                        .header("Content-Type", "application/octet-stream")
                        .body(body)
                        .send()
                        .await
                    {
                        // Cancellation ends the run.
                        Ok(resp) => {
                            let _ = resp.bytes().await;
                        }
                        Err(e) => {
                            first_err.get_or_insert(e.to_string());
                            break;
                        }
                    }
                }
                first_err
            }));
        }

        let mut first_err = None;
        for h in set {
            if let Ok(Some(e)) = h.await {
                first_err.get_or_insert(e);
            }
        }
        sampler.abort();

        let total = counted.load(Ordering::Relaxed);
        match (total, first_err) {
            (0, Some(e)) => Err(e),
            _ => Ok(Transfer { bytes: total, elapsed: start.elapsed() }),
        }
    }

    /// Measures request round trips, the lag you notice even on a
    /// fast link.
    pub async fn latency(
        &self,
        cancel: &Cancel,
        url: &str,
        samples: usize,
    ) -> Result<Vec<Duration>, String> {
        let mut out = Vec::new();
        let mut first_err = None;
        for _ in 0..samples {
            if cancel.is_cancelled() {
                break;
            }
            let start = Instant::now();
            match self.client.get(url).send().await {
                Ok(resp) => {
                    let _ = resp.bytes().await;
                    out.push(start.elapsed());
                }
                Err(e) => {
                    first_err.get_or_insert(e.to_string());
                }
            }
        }
        if out.is_empty() {
            return Err(first_err.unwrap_or_else(|| "no samples".into()));
        }
        Ok(out)
    }
}

/// Produces incompressible data for upload tests, counting what it hands over
/// and stopping when the deadline passes.
fn upload_stream(
    counted: Arc<AtomicU64>,
    deadline: Instant,
    cancel: Cancel,
) -> impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> {
    // Random data so no link or proxy can compress the test away.
    let mut block = vec![0u8; 128 << 10];
    rand::fill(&mut block[..]);
    let block = bytes::Bytes::from(block);

    // 64 MiB per request, which is enough that the request setup cost
    // disappears into the transfer.
    let mut remaining: i64 = 64 << 20;
    futures::stream::poll_fn(move |_| {
        if remaining <= 0 || Instant::now() >= deadline || cancel.is_cancelled() {
            return std::task::Poll::Ready(None);
        }
        remaining -= block.len() as i64;
        counted.fetch_add(block.len() as u64, Ordering::Relaxed);
        std::task::Poll::Ready(Some(Ok(block.clone())))
    })
}

/// Reports throughput at a steady cadence while a test runs.
fn spawn_sampler(
    cancel: Cancel,
    counted: Arc<AtomicU64>,
    sample: Option<Sampler>,
    deadline: Instant,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(sample) = sample else { return };
        let (mut last, mut last_at) = (0u64, Instant::now());
        loop {
            tokio::time::sleep(SAMPLE_INTERVAL).await;
            if cancel.is_cancelled() || Instant::now() > deadline + Duration::from_secs(1) {
                return;
            }
            let now = Instant::now();
            let current = counted.load(Ordering::Relaxed);
            sample(current.saturating_sub(last), now.duration_since(last_at));
            last = current;
            last_at = now;
        }
    })
}

/// Renders a bits-per-second figure in the unit a person would use.
pub fn format_bits(bps: f64) -> String {
    match bps {
        b if b >= 1e9 => format!("{:.2} Gbps", b / 1e9),
        b if b >= 1e6 => format!("{:.1} Mbps", b / 1e6),
        b if b >= 1e3 => format!("{:.1} kbps", b / 1e3),
        b => format!("{b:.0} bps"),
    }
}

/// Renders a byte count in binary units.
pub fn format_bytes(n: u64) -> String {
    const UNIT: u64 = 1024;
    if n < UNIT {
        return format!("{n} B");
    }
    let (mut div, mut exp) = (UNIT, 0usize);
    let mut v = n / UNIT;
    while v >= UNIT && exp < 4 {
        div *= UNIT;
        v /= UNIT;
        exp += 1;
    }
    format!("{:.1} {}iB", n as f64 / div as f64, ["K", "M", "G", "T", "P"][exp])
}
