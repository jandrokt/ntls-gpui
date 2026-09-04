//! Checks TCP and UDP ports on a host, by range, list, or all.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::cells;
use crate::core::{
    Column, Emitter, Expand, Field, Opt, Role, Status, Tool, Validator, VisibleIf,
    col, kv,
};
use crate::net::{iface, ports, portscan};
use crate::tools::prober::{iface_field, resolve_targets, target_field};
use crate::tools::stats::{elapsed, ms};

pub struct PortScan;

const PROTO_TCP: &str = "tcp";
const PROTO_UDP: &str = "udp";
const PROTO_BOTH: &str = "both";

impl Tool for PortScan {
    fn id(&self) -> &'static str {
        "portscan"
    }
    fn title(&self) -> &'static str {
        "Port scan"
    }
    fn desc(&self) -> &'static str {
        "Check TCP and UDP ports on a host, by range, list, or all"
    }
    fn icon(&self) -> &'static str {
        "portscan"
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            target_field(
                "Target",
                "192.168.1.10, example.com, 10.0.0.0/28",
                "One host, or a network to scan every host in. Tab expands a network into its range",
            ),
            Field::select(
                "proto",
                "Protocol",
                "TCP gives definite answers; UDP is slower and often ambiguous",
                PROTO_TCP,
                vec![
                    Opt::new(PROTO_TCP, "TCP", "Full connect scan"),
                    Opt::new(PROTO_UDP, "UDP", "Protocol-aware probes; silence is ambiguous"),
                    Opt::new(PROTO_BOTH, "TCP + UDP", "Every port over both, twice the probes"),
                ],
            ),
            Field::text(
                "ports",
                "Ports",
                "all = 1-65535, top = ~90 popular ports, common = the usual suspects. Tab expands to explicit numbers",
            )
            .placeholder("all, top, common, 22, 1-1024, 80,443,8000-8100")
            .default("all")
            .role(Role::Ports)
            .validate(Validator::Ports)
            .expand(Expand::Ports),
            Field::text(
                "timeout",
                "Timeout",
                "How long to wait per port; raise it for hosts across the internet",
            )
            .default("700ms")
            .validate(Validator::Duration),
            iface_field(),
            Field::text(
                "concurrency",
                "Concurrency",
                "Ports probed at the same time. The main speed knob; lower it if your network drops probes",
            )
            .default("1024")
            .validate(Validator::IntRange(1, 8192)),
            Field::boolean(
                "banner",
                "Grab banners",
                "Read the greeting an open service sends, to identify what it is",
                true,
            )
            .visible_if(VisibleIf::NotEquals("proto", PROTO_UDP)),
            Field::boolean(
                "showclosed",
                "List closed ports",
                "Also add a row for ports that are closed or filtered",
                false,
            ),
        ]
    }

    fn resumable(&self) -> bool {
        true
    }

    fn columns(&self) -> Vec<Column> {
        vec![
            col("HOST", 16),
            col("PORT", 11),
            col("SERVICE", 12),
            col("STATE", 13),
            col("TIME", 9),
            col("BANNER", 0),
        ]
    }

    fn run<'a>(
        &'a self,
        r: crate::core::Run,
        emit: Emitter,
    ) -> crate::core::tool::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { run(r, emit).await })
    }
}

/// The protocols to probe for the chosen setting.
fn protocols(proto: &str) -> Vec<&'static str> {
    match proto {
        PROTO_UDP => vec![PROTO_UDP],
        PROTO_BOTH => vec![PROTO_TCP, PROTO_UDP],
        _ => vec![PROTO_TCP],
    }
}

/// Every probe a run has to make — each port on each host over each protocol
/// — handed out one at a time.
///
/// This whole list used to be built before the first packet went out: a
/// `Vec` of one tuple per probe, with room for all of them reserved up
/// front. Nothing needed the list to exist, since a worker only ever asks
/// for the next probe, and the sizes involved are not modest. The default
/// port setting is `all`, and a target of a /16 — which is what `auto`
/// expands to on plenty of corporate networks — is four thousand million
/// probes, tens of gigabytes of tuples for a TCP scan and double that for
/// TCP + UDP. An allocation that size does not come back as an error the
/// scan could report; it kills the process before a single port has been
/// checked, and the same reservation makes an ordinary /24 over every port
/// cost hundreds of megabytes for nothing. So each probe is computed from
/// its position instead, and a scan of any size starts immediately holding
/// only the host and port lists it was given.
struct Jobs {
    hosts: Vec<IpAddr>,
    /// Ascending, as `ports::parse_ports` returns it. That is what lets a
    /// resume mark be found by bisection instead of by walking the ports.
    ports: Vec<u16>,
    protos: Vec<&'static str>,
    /// The highest port a previous, interrupted run reported on each host.
    reached: std::collections::HashMap<IpAddr, u16>,
    /// Which host is being worked through, and how far into its own probes.
    host: usize,
    at: usize,
}

impl Jobs {
    fn new(
        hosts: Vec<IpAddr>,
        ports: Vec<u16>,
        protos: Vec<&'static str>,
        reached: std::collections::HashMap<IpAddr, u16>,
    ) -> Jobs {
        Jobs { hosts, ports, protos, reached, host: 0, at: 0 }
    }

    /// How many probes one host takes.
    fn per_host(&self) -> usize {
        self.ports.len() * self.protos.len()
    }

    /// How many of a host's probes a previous run already reported on. A host
    /// with no row of its own has no mark, so it is probed from the beginning
    /// rather than assumed to have been covered by another.
    fn skip_for(&self, host: &IpAddr) -> usize {
        match self.reached.get(host) {
            Some(&mark) => self.ports.partition_point(|p| *p <= mark) * self.protos.len(),
            None => 0,
        }
    }

    /// How many probes are still to be handed out. Counting them arithmetically
    /// is the point: asking a list how long it is means having built the list,
    /// which is exactly what a large scan cannot afford.
    fn remaining(&self) -> usize {
        let per_host = self.per_host();
        let mut left = 0;
        for (i, host) in self.hosts.iter().enumerate().skip(self.host) {
            let from =
                if i == self.host { self.at.max(self.skip_for(host)) } else { self.skip_for(host) };
            left += per_host.saturating_sub(from);
        }
        left
    }
}

impl Iterator for Jobs {
    type Item = (IpAddr, u16, &'static str);

    fn next(&mut self) -> Option<(IpAddr, u16, &'static str)> {
        let per_host = self.per_host();
        loop {
            let host = *self.hosts.get(self.host)?;
            // The ports are ascending, so everything at or below this host's
            // resume mark sits in one block at the front and is stepped over
            // whole rather than examined probe by probe.
            if self.at == 0 {
                self.at = self.skip_for(&host);
            }
            if self.at >= per_host {
                self.host += 1;
                self.at = 0;
                continue;
            }
            let port = self.ports[self.at / self.protos.len()];
            let proto = self.protos[self.at % self.protos.len()];
            self.at += 1;
            return Some((host, port, proto));
        }
    }
}

async fn run(r: crate::core::Run, emit: Emitter) -> anyhow::Result<()> {
    let (cancel, p) = (r.cancel.clone(), r.params.clone());
    let hosts = resolve_targets(&p.str("target"), &p.str("iface"), &emit).map_err(anyhow::Error::msg)?;
    let port_list = ports::parse_ports(&p.str("ports")).map_err(anyhow::Error::msg)?;

    let src = iface::resolve_interface(&p.str("iface")).map_err(anyhow::Error::msg)?;
    if let Some(src) = src {
        emit.info(format!("sending from {src}"));
    }

    let protos = protocols(&p.str("proto"));
    let timeout = p.dur("timeout", Duration::from_millis(700));
    let banner = p.bool("banner");
    let show_closed = p.bool("showclosed");

    let total = hosts.len() * port_list.len() * protos.len();

    emit.info(format!(
        "{} port(s) × {} host(s) × {} = {total} probes",
        port_list.len(),
        hosts.len(),
        if protos.len() == 1 { protos[0] } else { "tcp+udp" }
    ));
    if protos.contains(&PROTO_UDP) {
        emit.info(
            "UDP: a silent port cannot be told apart from a filtered one, so it is reported as open|filtered",
        );
    }

    // A resumed run skips what the interrupted one had already reached. Only
    // an open port leaves a row behind, so a row proves that its own host was
    // probed at least that far, and proves nothing whatever about any other
    // host.
    //
    // This used to take the highest port over every row and apply that one
    // number to every host. So a sweep of a /28 in which the first host
    // answered on 443 dropped ports 1 to 443 on all fifteen others, and then
    // reported itself finished. Each host now carries its own mark.
    //
    // What this still cannot know is that a probe left in flight when the run
    // stopped reported nothing either, so a port below the highest open one
    // may never have been answered. Telling those apart needs the run to
    // record how far it got rather than inferring it from the ports that
    // happened to be open.
    let workers = p.usize("concurrency", 1024).clamp(1, 8192);
    let mut reached: std::collections::HashMap<IpAddr, u16> = std::collections::HashMap::new();
    for target in &r.done {
        let Some((host, port)) = target.rsplit_once(':') else { continue };
        let (Ok(host), Ok(port)) = (host.parse::<IpAddr>(), port.parse::<u16>()) else { continue };
        let mark = reached.entry(host).or_insert(0);
        *mark = (*mark).max(port);
    }
    let resuming = !reached.is_empty();

    let jobs = Jobs::new(hosts, port_list, protos, reached);
    let left = jobs.remaining();
    let skipped = total - left;
    if resuming {
        emit.info(format!("resuming: {skipped} of {total} probes already reported"));
    }
    if left == 0 {
        emit.progress(total, total);
        emit.good("nothing left to probe");
        return Ok(());
    }
    let workers = workers.min(left);

    let done = Arc::new(AtomicUsize::new(0));
    let open = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();

    let publish = {
        let (done, open, emit) = (done.clone(), open.clone(), emit.clone());
        move || {
            let d = done.load(Ordering::Relaxed) + skipped;
            emit.progress(d, total);
            let el = start.elapsed();
            let secs = el.as_secs_f64().max(0.001);
            let mut stats = vec![
                kv("probed", format!("{d}/{total}")),
                kv("open", open.load(Ordering::Relaxed).to_string()),
                kv("elapsed", elapsed(el)),
                kv("rate", format!("{:.0}/s", d as f64 / secs)),
            ];
            if d > 0 && d < total {
                let remaining = Duration::from_secs_f64((total - d) as f64 * secs / d as f64);
                stats.push(kv("eta", elapsed(remaining)));
            }
            emit.stats(stats);
        }
    };

    let ticker = {
        let (publish, cancel) = (publish.clone(), cancel.clone());
        tokio::spawn(async move {
            while cancel.sleep(Duration::from_millis(120)).await {
                publish();
            }
        })
    };

    let queue = Arc::new(tokio::sync::Mutex::new(jobs));

    let mut handles = Vec::new();
    for _ in 0..workers.max(1) {
        let (queue, emit, done, open, cancel) =
            (queue.clone(), emit.clone(), done.clone(), open.clone(), cancel.clone());
        handles.push(tokio::spawn(async move {
            loop {
                let Some((addr, port, proto)) = queue.lock().await.next() else { break };
                if cancel.is_cancelled() {
                    break;
                }

                let probe = async {
                    if proto == PROTO_UDP {
                        portscan::probe_udp(addr, port, timeout, src).await
                    } else {
                        portscan::probe_tcp(addr, port, timeout, banner, src).await
                    }
                };
                let Some(res) = cancel.run(probe).await else { break };
                done.fetch_add(1, Ordering::Relaxed);

                let is_open = res.state == portscan::PortState::Open;
                if is_open {
                    open.fetch_add(1, Ordering::Relaxed);
                }
                if (!is_open && !show_closed) || cancel.is_cancelled() {
                    continue;
                }
                emit.row(
                    port_status(res.state),
                    format!("{addr}:{port}"),
                    cells![
                        addr,
                        format!("{port}/{proto}"),
                        ports::service_name(port),
                        res.state,
                        ms(res.latency),
                        res.banner
                    ],
                );
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }
    ticker.abort();
    publish();

    let found = open.load(Ordering::Relaxed);
    let msg = format!(
        "{found} open, {} probed in {}",
        done.load(Ordering::Relaxed),
        elapsed(start.elapsed())
    );
    if found == 0 {
        emit.warn(msg);
    } else {
        emit.good(msg);
    }
    Ok(())
}

fn port_status(s: portscan::PortState) -> Status {
    match s {
        portscan::PortState::Open => Status::Up,
        portscan::PortState::Closed => Status::Down,
        _ => Status::Warn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn marks(pairs: &[(&str, u16)]) -> std::collections::HashMap<IpAddr, u16> {
        pairs.iter().map(|(h, p)| (host(h), *p)).collect()
    }

    #[test]
    fn every_host_gets_every_port_over_every_protocol_in_that_order() {
        let got: Vec<_> = Jobs::new(
            vec![host("10.0.0.1"), host("10.0.0.2")],
            vec![22, 80],
            vec![PROTO_TCP, PROTO_UDP],
            marks(&[]),
        )
        .collect();
        assert_eq!(
            got,
            vec![
                (host("10.0.0.1"), 22, PROTO_TCP),
                (host("10.0.0.1"), 22, PROTO_UDP),
                (host("10.0.0.1"), 80, PROTO_TCP),
                (host("10.0.0.1"), 80, PROTO_UDP),
                (host("10.0.0.2"), 22, PROTO_TCP),
                (host("10.0.0.2"), 22, PROTO_UDP),
                (host("10.0.0.2"), 80, PROTO_TCP),
                (host("10.0.0.2"), 80, PROTO_UDP),
            ]
        );
    }

    #[test]
    fn a_resume_mark_skips_ports_on_its_own_host_and_on_no_other() {
        // Only an open port leaves a row behind, so one host answering high up
        // says nothing about how far any other host got.
        let got: Vec<_> = Jobs::new(
            vec![host("10.0.0.1"), host("10.0.0.2")],
            vec![22, 443, 8080],
            vec![PROTO_TCP],
            marks(&[("10.0.0.1", 443)]),
        )
        .collect();
        assert_eq!(
            got,
            vec![
                (host("10.0.0.1"), 8080, PROTO_TCP),
                (host("10.0.0.2"), 22, PROTO_TCP),
                (host("10.0.0.2"), 443, PROTO_TCP),
                (host("10.0.0.2"), 8080, PROTO_TCP),
            ]
        );
    }

    #[test]
    fn the_count_of_probes_left_matches_what_the_cursor_hands_out() {
        // The progress bar, the "already reported" line and the worker count
        // are all sized from this number, and it is now arrived at by
        // arithmetic rather than by measuring a list.
        let jobs = Jobs::new(
            vec![host("10.0.0.1"), host("10.0.0.2")],
            vec![22, 80, 443],
            vec![PROTO_TCP],
            marks(&[("10.0.0.1", 80)]),
        );
        let left = jobs.remaining();
        assert_eq!(left, 4);
        assert_eq!(jobs.count(), left);
    }

    #[test]
    fn a_host_whose_every_port_was_already_reported_is_stepped_over_entirely() {
        let mut jobs = Jobs::new(
            vec![host("10.0.0.1"), host("10.0.0.2")],
            vec![22, 80],
            vec![PROTO_TCP],
            marks(&[("10.0.0.1", 65535), ("10.0.0.2", 65535)]),
        );
        assert_eq!(jobs.remaining(), 0);
        assert_eq!(jobs.next(), None);
    }

    #[test]
    fn a_scan_far_too_large_to_ever_be_listed_still_starts_on_its_first_probe() {
        // A /16 over the default port setting of `all` is four thousand
        // million probes. Building that list, which is what used to happen
        // before the first packet went out, asks for tens of gigabytes and
        // takes the process down with it. Nothing here may cost anything per
        // probe: the count is arithmetic and the first probe is immediate.
        let hosts: Vec<IpAddr> =
            (0..65534u32).map(|i| IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i))).collect();
        let mut jobs =
            Jobs::new(hosts, (1..=65535).collect(), vec![PROTO_TCP], marks(&[("10.0.0.0", 1)]));
        assert_eq!(jobs.remaining(), 65_534usize * 65_535 - 1);
        assert_eq!(jobs.next(), Some((host("10.0.0.0"), 2, PROTO_TCP)));
    }
}
