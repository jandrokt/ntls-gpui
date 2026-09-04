//! Throughput against an HTTP endpoint.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

        // Raised when a stream is told no, so the refusal outlives the task
        // that saw it and can be weighed against the byte count below.
        let refused = Arc::new(AtomicBool::new(false));
        let mut set = Vec::new();
        for _ in 0..streams.max(1) {
            let (client, url, counted, cancel, refused) = (
                self.client.clone(),
                url.to_string(),
                counted.clone(),
                cancel.clone(),
                refused.clone(),
            );
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
                            let status = resp.status();
                            let _ = resp.bytes().await;
                            // An endpoint that will not take the data has
                            // measured nothing, and going straight back round
                            // to post again turned a proxy or a rate limiter
                            // answering 403 or 429 to every request into a
                            // request flood that ran for the whole duration.
                            if !status.is_success() {
                                refused.store(true, Ordering::Relaxed);
                                first_err.get_or_insert(format!("server returned {status}"));
                                break;
                            }
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
        upload_outcome(total, start.elapsed(), refused.load(Ordering::Relaxed), first_err)
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

/// What an upload reports, given the bytes it generated, how long it ran,
/// whether any stream was refused, and the first thing that went wrong.
///
/// A refusal outweighs the byte count. The generator tallies bytes as it hands
/// them to the body stream, not as the far end accepts them, so a POST that
/// came back 403 leaves a healthy-looking total behind it all the same, and a
/// run against an endpoint refusing every request was reported as a perfectly
/// good upload speed.
fn upload_outcome(
    bytes: u64,
    elapsed: Duration,
    refused: bool,
    first_err: Option<String>,
) -> Result<Transfer, String> {
    match first_err {
        Some(e) if refused || bytes == 0 => Err(e),
        _ => Ok(Transfer { bytes, elapsed }),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An endpoint that refuses the POST has taken delivery of nothing,
    /// however much the generator handed to the body stream on the way out.
    /// Those bytes divided by the time they took looks exactly like a
    /// throughput figure, which is how a proxy answering 403 to every upload
    /// came back as a completed test instead of as the error it was.
    #[test]
    fn an_upload_the_server_refused_is_an_error_however_many_bytes_were_generated() {
        let out = upload_outcome(
            64 << 20,
            Duration::from_secs(8),
            true,
            Some("server returned 403 Forbidden".into()),
        );
        assert_eq!(out.err(), Some("server returned 403 Forbidden".to_string()));
    }

    /// A stream that dropped part way through had already moved real bytes
    /// that a real server accepted, and that is a measurement, so one broken
    /// connection must not throw away what the run managed.
    #[test]
    fn an_upload_that_moved_bytes_before_a_connection_broke_still_reports_a_transfer() {
        let out = upload_outcome(
            1 << 20,
            Duration::from_secs(8),
            false,
            Some("connection reset by peer".into()),
        );
        assert_eq!(out.expect("a transfer").bytes, 1 << 20);
    }

    /// Nothing moved at all and something went wrong, so there is no figure to
    /// report, only the reason.
    #[test]
    fn an_upload_that_moved_nothing_reports_why_instead() {
        let out = upload_outcome(0, Duration::from_secs(8), false, Some("dns failure".into()));
        assert_eq!(out.err(), Some("dns failure".to_string()));
    }

    /// A clean run reports what it moved.
    #[test]
    fn an_upload_with_nothing_wrong_reports_what_it_moved() {
        let out = upload_outcome(4 << 20, Duration::from_secs(2), false, None);
        let t = out.expect("a transfer");
        assert_eq!(t.bytes, 4 << 20);
        assert_eq!(t.bits_per_second(), (4 << 20) as f64 * 8.0 / 2.0);
    }
}
