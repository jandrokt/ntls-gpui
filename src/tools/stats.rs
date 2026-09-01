//! Round-trip statistics and the number formatting the tools share.

use std::time::Duration;

/// How many recent samples the summary keeps: as many as a small
/// graph has room to draw.
pub const RECENT: usize = 48;

/// Accumulates round-trip times and renders the summary that sits above the
/// ping results.
#[derive(Clone, Debug, Default)]
pub struct RttStats {
    pub sent: usize,
    pub recv: usize,
    pub min: Duration,
    pub max: Duration,
    sum: Duration,
    sum_sq: f64,
    pub recent: Vec<Duration>,
}

impl RttStats {
    pub fn send(&mut self) {
        self.sent += 1;
    }

    pub fn add(&mut self, rtt: Duration) {
        self.recv += 1;
        if self.min.is_zero() || rtt < self.min {
            self.min = rtt;
        }
        if rtt > self.max {
            self.max = rtt;
        }
        self.sum += rtt;
        let ms = rtt.as_secs_f64() * 1000.0;
        self.sum_sq += ms * ms;

        self.recent.push(rtt);
        if self.recent.len() > RECENT {
            self.recent.remove(0);
        }
    }

    pub fn avg(&self) -> Duration {
        if self.recv == 0 {
            return Duration::ZERO;
        }
        self.sum / self.recv as u32
    }

    /// The population standard deviation, the "mdev" figure ping
    /// traditionally reports.
    pub fn stddev(&self) -> Duration {
        if self.recv < 2 {
            return Duration::ZERO;
        }
        let mean = self.sum.as_secs_f64() * 1000.0 / self.recv as f64;
        let variance = (self.sum_sq / self.recv as f64 - mean * mean).max(0.0);
        Duration::from_secs_f64(variance.sqrt() / 1000.0)
    }

    pub fn loss_pct(&self) -> f64 {
        if self.sent == 0 {
            return 0.0;
        }
        (self.sent - self.recv) as f64 / self.sent as f64 * 100.0
    }

}

/// Formats a duration the way a network tool should: enough precision to be
/// useful, not enough to be noise.
pub fn ms(d: Duration) -> String {
    if d.is_zero() {
        return "-".into();
    }
    let v = d.as_secs_f64() * 1000.0;
    match v {
        v if v < 10.0 => format!("{v:.2} ms"),
        v if v < 1000.0 => format!("{v:.1} ms"),
        v => format!("{:.2} s", v / 1000.0),
    }
}

/// Reads back what [`ms`] wrote, so a resumed run can count what the first
/// one measured. A cell it cannot read is a probe that got no reply.
pub fn parse_ms(cell: &str) -> Option<Duration> {
    let cell = cell.trim();
    let (value, scale) = match cell.strip_suffix(" ms") {
        Some(v) => (v, 1e-3),
        None => (cell.strip_suffix(" s")?, 1.0),
    };
    let seconds = value.trim().parse::<f64>().ok()? * scale;
    (seconds > 0.0).then(|| Duration::from_secs_f64(seconds))
}

/// Formats a percentage without trailing noise.
pub fn pct(v: f64) -> String {
    if v == v.trunc() { format!("{v:.0}%") } else { format!("{v:.1}%") }
}

/// Rounds an elapsed time to a tenth of a second, all the precision a
/// summary line can honestly claim.
pub fn elapsed(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{}ms", (secs * 1000.0).round() as u64)
    } else if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        format!("{}m{:02}s", (secs / 60.0) as u64, (secs % 60.0).round() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::{ms, parse_ms};
    use std::time::Duration;

    #[test]
    fn a_round_trip_reads_back_the_way_it_was_written() {
        // Resuming a ping means reading its own table back, so the formatter
        // and the parser have to agree.
        for original in [
            Duration::from_micros(1500),
            Duration::from_millis(250),
            Duration::from_millis(2400),
        ] {
            let read = parse_ms(&ms(original)).expect("a duration");
            let drift = read.as_secs_f64() - original.as_secs_f64();
            assert!(drift.abs() < 0.001, "{original:?} came back as {read:?}");
        }
        // A probe that got no reply.
        assert_eq!(parse_ms("-"), None);
        assert_eq!(parse_ms(""), None);
    }
}
