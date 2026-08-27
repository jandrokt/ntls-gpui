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
    /// Marks the hop that answered with an echo reply, meaning the target
    /// itself.
    final_: bool,
    /// An unreachable explanation, when there is one.
    note: String,
}

/// How many TTLs are probed at once. The TTL is a socket setting, but it is
/// applied under the same lock as the write that uses it, so probes at
/// different TTLs can safely overlap — and overlapping them is the difference
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
    let state = Arc::new(Mutex::new(State { results: Vec::new(), emitted: 1, reached: 0 }));
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

    // Reaching the target early makes the hop budget irrelevant, so close the
    // bar against what the path actually turned out to be.
    let done = state.lock().unwrap().reached;
    if done > 0 {
        emit.progress(done, done);
        emit.good(format!("reached {target} in {done} hops, {}", elapsed(start.elapsed())));
    } else {
        emit.progress(max_hops, max_hops);
        emit.warn(format!("gave up after {max_hops} hops without reaching {target}"));
    }
    Ok(())
}

struct State {
    results: Vec<Option<Hop>>,
    emitted: usize,
    /// The TTL at which the target answered, 0 while unknown.
    reached: usize,
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

        hop.rtts.push(reply.rtt);
        if !hop.addrs.contains(&reply.from) {
            hop.addrs.push(reply.from);
        }
        match reply.kind {
            icmp::ReplyKind::Echo => hop.final_ = true,
            icmp::ReplyKind::Unreachable => {
                hop.final_ = true;
                hop.note = icmp::unreachable_note(reply.code).into();
            }
            icmp::ReplyKind::TimeExceeded => {}
        }
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
/// came back, which is how traceroute has always shown loss.
fn format_rtts(hop: &Hop) -> String {
    let mut parts: Vec<String> =
        hop.rtts.iter().map(|r| ms(*r).trim_end_matches(" ms").to_string()).collect();
    if !hop.rtts.is_empty() {
        parts.push("ms".into());
    }
    parts.extend(std::iter::repeat_n("*".to_string(), hop.lost));
    parts.join(" ")
}
