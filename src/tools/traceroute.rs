//! Maps the routers between here and a host, hop by hop.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cells;
use crate::core::{Cancel, Column, Emitter, Field, Status, Tool, Validator, col};
use crate::net::{iface, icmp};
use crate::tools::prober::{iface_field, resolve_targets, target_field};
use crate::tools::stats::{elapsed, ms};

pub struct Traceroute;

impl Tool for Traceroute {
    fn id(&self) -> &'static str {
        "traceroute"
    }
    fn title(&self) -> &'static str {
        "Traceroute"
    }
    fn desc(&self) -> &'static str {
        "Map the route to a host, hop by hop"
    }
    fn icon(&self) -> &'static str {
        "traceroute"
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            target_field("Target", "1.1.1.1, example.com", "The far end of the path you want to see"),
            iface_field(),
            Field::text("maxhops", "Max hops", "Give up after this many routers")
                .default("30")
                .validate(Validator::IntRange(1, 64)),
            Field::text(
                "probes",
                "Probes per hop",
                "More probes give a better idea of a hop's latency and loss",
            )
            .default("3")
            .validate(Validator::IntRange(1, 10)),
            Field::text("timeout", "Timeout", "How long to wait for each probe")
                .default("2s")
                .validate(Validator::Duration),
            Field::boolean(
                "resolve",
                "Resolve names",
                "Look up the reverse DNS name of every router that answers",
                true,
            ),
        ]
    }

    fn columns(&self) -> Vec<Column> {
        vec![col("HOP", 4), col("ADDRESS", 17), col("RTT", 26), col("HOST", 0)]
    }

    fn run<'a>(
        &'a self,
        r: crate::core::Run,
        emit: Emitter,
    ) -> crate::core::tool::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { run(r, emit).await })
    }
}

/// Every probe sent at one TTL.
#[derive(Default)]
struct Hop {
    ttl: usize,
    addrs: Vec<IpAddr>,
    rtts: Vec<Duration>,
    lost: usize,
    /// Marks the hop the trace ends at: the target answered, or something on
    /// the way said the probes are going no further.
    final_: bool,
    /// Whether it was the target answering for itself that ended the trace,
    /// rather than something short of it turning the probes back.
    is_target: bool,
    /// An unreachable explanation, when there is one.
    note: String,
}

impl Hop {
    /// Folds one answer into the hop.
    fn record(&mut self, reply: &icmp::Reply, target: IpAddr) {
        self.rtts.push(reply.rtt);
        if !self.addrs.contains(&reply.from) {
            self.addrs.push(reply.from);
        }
        match reply.kind {
            icmp::ReplyKind::Echo => {
                self.final_ = true;
                self.is_target = true;
            }
            // An unreachable report ends the trace wherever it comes from,
            // since nothing is getting past whatever sent it. But it is
            // usually a firewall short of the target refusing to pass the
            // probes on, not the target answering for itself, and every one
            // of them was being counted as an arrival: a trace that a
            // middlebox stopped six hops out finished by announcing it had
            // reached a host that had never answered anything, with the
            // middlebox's address in the last row.
            icmp::ReplyKind::Unreachable => {
                self.final_ = true;
                self.is_target |= reply.from == target;
                self.note = icmp::unreachable_note(reply.code).into();
            }
            icmp::ReplyKind::TimeExceeded => {}
        }
    }
}

/// How many TTLs are probed at once. The TTL is a socket setting, but it is
/// applied under the same lock as the write that uses it, so probes at
/// different TTLs can safely overlap, and overlapping them is the difference
/// between a trace that takes two seconds and one that takes a minute.
const PARALLEL_HOPS: usize = 8;

async fn run(r: crate::core::Run, emit: Emitter) -> anyhow::Result<()> {
    let (cancel, p) = (r.cancel.clone(), r.params.clone());
    let addrs = resolve_targets(&p.str("target"), &p.str("iface"), &emit).map_err(anyhow::Error::msg)?;
    let target = addrs[0];
    if addrs.len() > 1 {
        emit.warn(format!("target resolves to {} addresses; tracing to {target}", addrs.len()));
    }

    let max_hops = p.usize("maxhops", 30).clamp(1, 64);
    let probes = p.usize("probes", 3).clamp(1, 10);
    let timeout = p.dur("timeout", Duration::from_secs(2));
    let resolve = p.bool("resolve");

    let src = iface::resolve_interface(&p.str("iface")).map_err(anyhow::Error::msg)?;
    let pinger = Arc::new(
        icmp::Pinger::new(target.is_ipv4(), target.is_ipv6(), src).map_err(anyhow::Error::msg)?,
    );
    if pinger.privileged() {
        emit.warn("using a raw ICMP socket (running privileged)");
    }

    let start = Instant::now();
    let sem = Arc::new(tokio::sync::Semaphore::new(PARALLEL_HOPS));
    let state = Arc::new(Mutex::new(State {
        results: Vec::new(),
        emitted: 1,
        reached: 0,
        by_target: false,
    }));
    state.lock().unwrap().results.resize_with(max_hops + 1, || None);

    let mut tasks = Vec::new();
    for ttl in 1..=max_hops {
        if cancel.is_cancelled() {
            break;
        }
        {
            let s = state.lock().unwrap();
            if s.reached > 0 && ttl > s.reached {
                break;
            }
        }

        let (pinger, sem, state, emit, cancel_hop) =
            (pinger.clone(), sem.clone(), state.clone(), emit.clone(), cancel.clone());
        tasks.push(tokio::spawn(async move {
            let Ok(_permit) = sem.acquire().await else { return };
            if cancel_hop.is_cancelled() {
                return;
            }

            let hop = probe_hop(&cancel_hop, &pinger, target, ttl, probes, timeout).await;

            // Rows appear in path order even though the probes finish out of
            // order: each completed hop is held until every earlier one is in.
            let ready = {
                let mut s = state.lock().unwrap();
                if hop.final_ && (s.reached == 0 || ttl < s.reached) {
                    s.reached = ttl;
                    s.by_target = hop.is_target;
                }
                s.results[ttl] = Some(hop);
                s.drain_ready(max_hops)
            };
            for hop in ready {
                let done = hop.ttl;
                emit_row(&emit, hop, target, resolve).await;
                emit.progress(done, max_hops);
            }
        }));
    }

    for t in tasks {
        let _ = t.await;
    }

    let ready = state.lock().unwrap().drain_ready(max_hops);
    for hop in ready {
        emit_row(&emit, hop, target, resolve).await;
    }

    if cancel.is_cancelled() {
        return Ok(());
    }

    let (done, by_target) = {
        let s = state.lock().unwrap();
        (s.reached, s.by_target)
    };
    emit_summary(&emit, target, done, by_target, max_hops, start.elapsed());
    Ok(())
}

/// Says how the trace ended and closes the progress bar.
fn emit_summary(
    emit: &Emitter,
    target: IpAddr,
    done: usize,
    by_target: bool,
    max_hops: usize,
    took: Duration,
) {
    if done == 0 {
        emit.progress(max_hops, max_hops);
        emit.warn(format!("gave up after {max_hops} hops without reaching {target}"));
        return;
    }

    // Ending the path early makes the hop budget irrelevant, so close the bar
    // against what the path actually turned out to be.
    emit.progress(done, done);
    if by_target {
        emit.good(format!("reached {target} in {done} hops, {}", elapsed(took)));
    } else {
        // Something on the way refused to carry the probes any further, which
        // is not the same as arriving. The last row names what answered and
        // why, so all this has to say is that the path stops there.
        emit.warn(format!("path to {target} ends at hop {done}, {}", elapsed(took)));
    }
}

struct State {
    results: Vec<Option<Hop>>,
    emitted: usize,
    /// The TTL the path ended at, 0 while unknown.
    reached: usize,
    /// Whether the hop at `reached` was the target itself.
    by_target: bool,
}

impl State {
    fn drain_ready(&mut self, max_hops: usize) -> Vec<Hop> {
        let mut out = Vec::new();
        while self.emitted <= max_hops {
            if self.reached > 0 && self.emitted > self.reached {
                break;
            }
            let Some(hop) = self.results[self.emitted].take() else { break };
            out.push(hop);
            self.emitted += 1;
        }
        out
    }
}

/// Sends the probes for one TTL and summarises what came back.
async fn probe_hop(
    cancel: &Cancel,
    pinger: &icmp::Pinger,
    target: IpAddr,
    ttl: usize,
    probes: usize,
    timeout: Duration,
) -> Hop {
    let mut hop = Hop { ttl, ..Default::default() };

    for _ in 0..probes {
        if cancel.is_cancelled() {
            break;
        }
        let Some(result) = cancel.run(pinger.ping_ttl(target, ttl as u8, 0, timeout)).await else {
            break;
        };
        let Ok(reply) = result else {
            hop.lost += 1;
            continue;
        };

        hop.record(&reply, target);
    }
    hop
}

async fn emit_row(emit: &Emitter, hop: Hop, target: IpAddr, resolve: bool) {
    if hop.addrs.is_empty() {
        emit.row(Status::Down, target.to_string(), cells![hop.ttl, "*", "no reply", ""]);
        return;
    }

    // A hop that load-balances answers from more than one router.
    let mut addr_text = hop.addrs[0].to_string();
    if hop.addrs.len() > 1 {
        addr_text += &format!(" +{}", hop.addrs.len() - 1);
    }

    let mut name = if resolve {
        crate::net::dns::reverse_name(hop.addrs[0], Duration::from_millis(900)).await
    } else {
        String::new()
    };
    if !hop.note.is_empty() {
        if !name.is_empty() {
            name += " · ";
        }
        name += &hop.note;
    }

    let status = if hop.lost > 0 && !hop.final_ { Status::Warn } else { Status::Up };
    emit.row(status, hop.addrs[0].to_string(), cells![hop.ttl, addr_text, format_rtts(&hop), name]);
}

/// Renders each probe's round trip followed by a star for every one that never
/// came back, as traceroute has always shown loss.
fn format_rtts(hop: &Hop) -> String {
    let mut parts: Vec<String> =
        hop.rtts.iter().map(|r| ms(*r).trim_end_matches(" ms").to_string()).collect();
    if !hop.rtts.is_empty() {
        parts.push("ms".into());
    }
    parts.extend(std::iter::repeat_n("*".to_string(), hop.lost));
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Event, Level};

    const TARGET: &str = "1.1.1.1";

    fn target() -> IpAddr {
        TARGET.parse().expect("an address")
    }

    fn reply(kind: icmp::ReplyKind, from: &str, code: u8) -> icmp::Reply {
        icmp::Reply {
            kind,
            from: from.parse().expect("an address"),
            rtt: Duration::from_millis(12),
            ttl: 64,
            code,
        }
    }

    /// Collects the log lines a summary produces.
    fn logged(f: impl FnOnce(&Emitter)) -> Vec<(Level, String)> {
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let held = events.clone();
        f(&Emitter::new(move |e| held.lock().expect("events").push(e)));
        events
            .lock()
            .expect("events")
            .iter()
            .filter_map(|e| match e {
                Event::Log { level, text } => Some((*level, text.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_unreachable_report_from_a_middlebox_is_not_the_target_answering() {
        let mut hop = Hop { ttl: 6, ..Default::default() };
        hop.record(&reply(icmp::ReplyKind::Unreachable, "10.0.0.1", 13), target());

        // The trace still stops here, since nothing is getting past it.
        assert!(hop.final_, "an unreachable report has to end the trace");
        assert!(!hop.is_target, "a middlebox's report was credited to the target");
        assert_eq!(hop.note, "administratively prohibited");
    }

    #[test]
    fn an_unreachable_report_from_the_target_itself_counts_as_reaching_it() {
        let mut hop = Hop { ttl: 9, ..Default::default() };
        hop.record(&reply(icmp::ReplyKind::Unreachable, TARGET, 3), target());
        assert!(hop.final_ && hop.is_target, "the target's own answer went uncredited");
    }

    #[test]
    fn a_time_exceeded_answer_leaves_the_trace_running() {
        let mut hop = Hop { ttl: 3, ..Default::default() };
        hop.record(&reply(icmp::ReplyKind::TimeExceeded, "10.0.0.1", 0), target());
        assert!(!hop.final_ && !hop.is_target);
        assert_eq!(hop.addrs, vec!["10.0.0.1".parse::<IpAddr>().expect("an address")]);
    }

    #[test]
    fn a_trace_stopped_short_of_the_target_does_not_claim_to_have_reached_it() {
        let logs = logged(|emit| {
            emit_summary(emit, target(), 6, false, 30, Duration::from_millis(400));
        });
        assert!(
            logs.iter().all(|(_, text)| !text.contains("reached 1.1.1.1")),
            "a trace a middlebox stopped announced an arrival: {logs:?}"
        );
        assert!(
            logs.iter().any(|(level, text)| *level == Level::Warn && text.contains("hop 6")),
            "nothing said where the path stopped: {logs:?}"
        );
    }

    #[test]
    fn a_trace_the_target_answered_reports_reaching_it() {
        let logs = logged(|emit| {
            emit_summary(emit, target(), 9, true, 30, Duration::from_millis(400));
        });
        assert!(
            logs.iter().any(|(level, text)| {
                *level == Level::Good && text.contains("reached 1.1.1.1 in 9 hops")
            }),
            "reaching the target went unreported: {logs:?}"
        );
    }
}
