//! Probes one host repeatedly and watches latency and loss.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::cells;
use crate::core::{Column, Emitter, Event, Field, Status, Tool, Validator, VisibleIf, col, kv};
use crate::tools::prober::{METHOD_ICMP, Prober, failure_text, iface_field, method_field, resolve_targets, target_field};
use crate::tools::stats::{RttStats, ms, pct};

pub struct Ping;

impl Tool for Ping {
    fn id(&self) -> &'static str {
        "ping"
    }
    fn title(&self) -> &'static str {
        "Ping"
    }
    fn desc(&self) -> &'static str {
        "Probe a host repeatedly and report latency and loss"
    }
    fn icon(&self) -> &'static str {
        "ping"
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            target_field(
                "Target",
                "1.1.1.1, example.com, 192.168.1.1",
                "Hostname or IP address to probe",
            ),
            method_field(),
            iface_field(),
            Field::text("count", "Count", "Number of probes; 0 runs until stopped")
                .default("0")
                .validate(Validator::IntRange(0, 1_000_000)),
            Field::text("interval", "Interval", "Delay between probes")
                .default("1s")
                .validate(Validator::Duration),
            Field::text("timeout", "Timeout", "Time to wait for each reply")
                .default("2s")
                .validate(Validator::Duration),
            Field::text("size", "Payload", "Payload bytes per echo request")
                .default("56")
                .validate(Validator::IntRange(0, 65000))
                .visible_if(VisibleIf::Equals("method", METHOD_ICMP)),
        ]
    }

    fn columns(&self) -> Vec<Column> {
        vec![col("#", 5), col("FROM", 26), col("RTT", 11), col("TTL", 5), col("DETAIL", 0)]
    }

    /// A ping that was stopped can be picked up: the sequence carries on from
    /// where it left off and the summary counts every probe, not just the ones
    /// since you pressed the button.
    fn resumable(&self) -> bool {
        true
    }

    fn run<'a>(
        &'a self,
        r: crate::core::Run,
        emit: Emitter,
    ) -> crate::core::tool::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { run(r, emit).await })
    }
}

/// Rebuilds the statistics from the table a stopped run left behind.
///
/// The table is the record: it is what was saved to disk and read back, so a
/// resumed run reads its own output and does not keep a second copy of it
/// somewhere. The RTT column is the one that matters; a row without a readable
/// one is a probe that got no reply.
fn seed(prior: &[crate::core::Row]) -> (usize, RttStats) {
    let mut stats = RttStats::default();
    let mut highest = 0usize;
    for row in prior {
        if let Some(seq) = row.cells.first().and_then(|c| c.trim().parse::<usize>().ok()) {
            highest = highest.max(seq);
        }
        stats.send();
        if let Some(rtt) = row.cells.get(2).and_then(|c| crate::tools::stats::parse_ms(c)) {
            stats.add(rtt);
        }
    }
    // Without a readable sequence column the row count is the next best thing.
    (highest.max(prior.len()), stats)
}

/// Lets go of the probes that have already answered.
///
/// The list exists only so the run can wait for the last few probes before it
/// writes its summary. But a ping with no count runs until somebody presses
/// Stop, and a task whose handle is never awaited keeps its allocation until
/// that handle is dropped, so the list was growing by one entry for every
/// probe ever sent and giving none of them back: an overnight ping at the
/// default second apart held tens of thousands of them, and one ten
/// milliseconds apart got there in minutes. Only the probes still waiting for
/// a reply are worth keeping.
fn reap(inflight: &mut Vec<tokio::task::JoinHandle<()>>) {
    inflight.retain(|h| !h.is_finished());
}

async fn run(r: crate::core::Run, emit: Emitter) -> anyhow::Result<()> {
    let (cancel, p) = (r.cancel.clone(), r.params.clone());
    let addrs = resolve_targets(&p.str("target"), &p.str("iface"), &emit).map_err(anyhow::Error::msg)?;
    let target = addrs[0];
    if addrs.len() > 1 {
        emit.warn(format!("target resolves to {} addresses; pinging {target}", addrs.len()));
    }

    let timeout = p.dur("timeout", Duration::from_secs(2));
    let interval = p.dur("interval", Duration::from_secs(1)).max(Duration::from_millis(10));
    let count = p.usize("count", 0);

    // Everything the earlier run measured, so the sequence continues and the
    // summary above the table describes the whole session and not the
    // latest slice of it.
    let (already, seeded) = seed(&r.prior);
    if already > 0 {
        emit.info(format!("Resuming from probe {already}"));
    }
    if count > 0 && already >= count {
        emit.good(format!("All {count} probes already sent"));
        return Ok(());
    }

    let prober = Arc::new(Prober::new(&p, &addrs[..1], timeout).map_err(anyhow::Error::msg)?);
    prober.emit_notes(&emit);

    let st = Arc::new(Mutex::new(seeded));
    let publish = |st: &RttStats| {
        emit.stats(vec![
            kv("sent", st.sent.to_string()),
            kv("recv", st.recv.to_string()),
            kv("loss", pct(st.loss_pct())),
            kv("min", ms(st.min)),
            kv("avg", ms(st.avg())),
            kv("max", ms(st.max)),
            kv("mdev", ms(st.stddev())),
        ]);
    };

    let mut inflight: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut seq = already;

    loop {
        seq += 1;
        let sent = {
            let mut s = st.lock().unwrap();
            s.send();
            s.sent
        };
        if count > 0 {
            emit.progress(sent, count);
        }

        // What is left after this is the probes still out on the wire, which
        // is a handful at most: a run that never ends would otherwise carry
        // every probe it has ever sent for as long as it lasts.
        reap(&mut inflight);

        let (prober, emit, st, cancel_probe) =
            (prober.clone(), emit.clone(), st.clone(), cancel.clone());
        inflight.push(tokio::spawn(async move {
            let res = prober.probe(&cancel_probe, target).await;

            match &res {
                Ok(r) => {
                    st.lock().unwrap().add(r.rtt);
                    emit.emit(Event::sample(
                        "round-trip time",
                        " ms",
                        r.rtt.as_secs_f64() * 1000.0,
                    ));
                    let from = r.from.unwrap_or(target);
                    let status = if r.suspect { Status::Warn } else { Status::Up };
                    emit.row(
                        status,
                        from.to_string(),
                        cells![seq, from, ms(r.rtt), r.ttl_text(), r.detail_line()],
                    );
                }
                Err(e) => emit.row(
                    Status::Down,
                    target.to_string(),
                    cells![seq, target, "-", "", failure_text(e)],
                ),
            }

            let s = st.lock().unwrap();
            emit.stats(vec![
                kv("sent", s.sent.to_string()),
                kv("recv", s.recv.to_string()),
                kv("loss", pct(s.loss_pct())),
                kv("min", ms(s.min)),
                kv("avg", ms(s.avg())),
                kv("max", ms(s.max)),
                kv("mdev", ms(s.stddev())),
            ]);
        }));

        if count > 0 && seq >= count {
            break;
        }
        if !cancel.sleep(interval).await {
            break;
        }
    }

    for h in inflight {
        let _ = h.await;
    }

    let s = st.lock().unwrap().clone();
    publish(&s);
    let summary = format!(
        "{} sent, {} received, {} loss, avg {}",
        s.sent,
        s.recv,
        pct(s.loss_pct()),
        ms(s.avg())
    );
    if s.recv == 0 {
        emit.err(summary);
    } else {
        emit.good(summary);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::core::{Row, Status};

    fn row(seq: usize, rtt: &str) -> Row {
        Row {
            cells: vec![seq.to_string(), "1.1.1.1".into(), rtt.into(), "55".into(), String::new()],
            status: Status::Up,
            target: "1.1.1.1".into(),
            note: None,
            key: None,
        }
    }

    #[test]
    fn a_resumed_ping_carries_on_from_where_it_stopped() {
        let prior = vec![row(1, "10.0 ms"), row(2, "-"), row(3, "20.0 ms")];
        let (already, stats) = super::seed(&prior);

        // The next probe is number four, not number one.
        assert_eq!(already, 3);
        // And the summary counts all three, including the one that got no
        // reply, or the loss figure would reset every time.
        assert_eq!(stats.sent, 3);
        assert_eq!(stats.recv, 2);
        assert_eq!(stats.avg().as_millis(), 15);
    }

    #[test]
    fn a_fresh_ping_starts_at_one() {
        let (already, stats) = super::seed(&[]);
        assert_eq!(already, 0);
        assert_eq!(stats.sent, 0);
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a runtime")
    }

    /// A run with no count sends probes for as long as it is left alone, so
    /// the list it keeps them in has to stay the size of what is actually
    /// outstanding. Nothing here touches the network: the two tasks that
    /// finish stand in for probes that have answered, and the one that sleeps
    /// for a probe still waiting.
    #[test]
    fn a_ping_that_runs_until_stopped_lets_go_of_the_probes_that_have_answered() {
        rt().block_on(async {
            let mut inflight = vec![
                tokio::spawn(async {}),
                tokio::spawn(async { tokio::time::sleep(Duration::from_secs(60)).await }),
                tokio::spawn(async {}),
            ];

            // Spawning does not run anything by itself, so give the two short
            // tasks the chance to be over before asking which of them are.
            for _ in 0..500 {
                if inflight[0].is_finished() && inflight[2].is_finished() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            super::reap(&mut inflight);

            assert_eq!(inflight.len(), 1, "only the probe still waiting for a reply is kept");
            assert!(!inflight[0].is_finished(), "and it is the one that has not finished");
        });
    }
}
