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

    let mut jobs: Vec<(IpAddr, u16, &'static str)> = Vec::with_capacity(total);
    for host in &hosts {
        for port in &port_list {
            for proto in &protos {
                // A resumed run skips the ports the interrupted one reached.
                // Only open ports leave a row behind, so this resumes from the
                // furthest port already reported instead of from a set.
                jobs.push((*host, *port, *proto));
            }
        }
    }
    let resume_from = r
        .done
        .iter()
        .filter_map(|t| t.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()))
        .max();
    if let Some(from) = resume_from {
        jobs.retain(|(_, port, _)| *port > from);
        emit.info(format!("resuming after port {from}"));
    }
    let skipped = total - jobs.len();
    if jobs.is_empty() {
        emit.progress(total, total);
        emit.good("nothing left to probe");
        return Ok(());
    }
    let workers = p.usize("concurrency", 1024).clamp(1, 8192).min(jobs.len());

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

    let queue = Arc::new(tokio::sync::Mutex::new(jobs.into_iter()));

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
