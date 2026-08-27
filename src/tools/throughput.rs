//! Measures real speed to another machine running ntls.
//!
//! An internet speed test tells you what your line does. This tells you what
//! your own cabling, switches and wireless do, which is usually the thing
//! actually limiting a file copy.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::cells;
use crate::core::{
    Cancel, Column, Emitter, Event, Field, Opt, Params, Role, Status, Tool, Validator, VisibleIf,
    col, kv,
};
use crate::net::iface;
use crate::net::lanspeed::{self, Direction};
use crate::net::speedtest::{Transfer, format_bits, format_bytes};
use crate::tools::prober::iface_field;
use crate::tools::stats::elapsed;

pub struct Throughput;

const ROLE_CLIENT: &str = "client";
const ROLE_SERVER: &str = "server";
/// The target value that means "find one on this network".
const DISCOVER: &str = "discover";

impl Tool for Throughput {
    fn id(&self) -> &'static str {
        "throughput"
    }
    fn title(&self) -> &'static str {
        "LAN throughput"
    }
    fn desc(&self) -> &'static str {
        "Measure throughput to another machine running ntls"
    }
    fn icon(&self) -> &'static str {
        "throughput"
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            Field::select(
                "role",
                "Role",
                "One machine listens, the other measures against it",
                ROLE_CLIENT,
                vec![
                    Opt::new(ROLE_CLIENT, "Measure", "Test against a machine that is listening"),
                    Opt::new(ROLE_SERVER, "Listen", "Wait for another machine to test against this one"),
                ],
            ),
            Field::text(
                "peer",
                "Peer",
                "discover broadcasts for a listening machine; or name one directly",
            )
            .placeholder("discover, 192.168.1.20, 192.168.1.20:5333")
            .default(DISCOVER)
            .role(Role::Target)
            .visible_if(VisibleIf::Equals("role", ROLE_CLIENT)),
            Field::select(
                "direction",
                "Direction",
                "Which way to move the data",
                "both",
                vec![
                    Opt::new("both", "Down + up", "Both directions in turn"),
                    Opt::new("down", "Download", "From the peer to here"),
                    Opt::new("up", "Upload", "From here to the peer"),
                ],
            )
            .visible_if(VisibleIf::Equals("role", ROLE_CLIENT)),
            Field::text("duration", "Duration", "How long to run each direction")
                .default("8s")
                .validate(Validator::Duration)
                .visible_if(VisibleIf::Equals("role", ROLE_CLIENT)),
            Field::text("port", "Port", "The port the listening side uses; both machines must agree")
                .default(&lanspeed::DEFAULT_PORT.to_string())
                .validate(Validator::IntRange(1, 65535)),
            iface_field(),
        ]
    }

    fn columns(&self) -> Vec<Column> {
        vec![
            col("PEER", 22),
            col("TEST", 10),
            col("RESULT", 14),
            col("TRANSFERRED", 13),
            col("DETAIL", 0),
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

async fn run(r: crate::core::Run, emit: Emitter) -> anyhow::Result<()> {
    let (cancel, p) = (r.cancel.clone(), r.params.clone());
    let src = iface::resolve_interface(&p.str("iface")).map_err(anyhow::Error::msg)?;
    let port = p.int("port", lanspeed::DEFAULT_PORT as i64).clamp(1, 65535) as u16;

    if p.str("role") == ROLE_SERVER {
        serve(cancel, emit, port, src).await
    } else {
        measure(cancel, emit, p, port, src).await
    }
}

/// Waits for other machines to test against this one.
async fn serve(
    cancel: Cancel,
    emit: Emitter,
    port: u16,
    src: Option<std::net::Ipv4Addr>,
) -> anyhow::Result<()> {
    let tests = Arc::new(AtomicUsize::new(0));

    let on_event: Arc<dyn Fn(String) + Send + Sync> = {
        let emit = emit.clone();
        Arc::new(move |msg| emit.info(msg))
    };
    let on_result: Arc<dyn Fn(IpAddr, Direction, u64, Duration) + Send + Sync> = {
        let (emit, tests) = (emit.clone(), tests.clone());
        Arc::new(move |remote, dir, bytes, el| {
            let n = tests.fetch_add(1, Ordering::Relaxed) + 1;
            let t = Transfer { bytes, elapsed: el };
            // The direction is named from the client's point of view, so
            // invert it: their upload is our download.
            let label = if dir == Direction::ToServer { "received" } else { "sent" };
            emit.row(
                Status::Up,
                remote.to_string(),
                cells![
                    remote,
                    label,
                    format_bits(t.bits_per_second()),
                    format_bytes(bytes),
                    format!("over {}", elapsed(el))
                ],
            );
            emit.stats(vec![
                kv("tests served", n.to_string()),
                kv("last", format_bits(t.bits_per_second())),
            ]);
        })
    };

    emit.info("run ntls on the other machine, pick LAN throughput, and leave the peer on discover");
    emit.stats(vec![kv("state", "listening")]);

    match lanspeed::serve(cancel.clone(), port, src, on_event, on_result).await {
        Ok(()) => Ok(()),
        Err(e) if cancel.is_cancelled() => {
            let _ = e;
            Ok(())
        }
        Err(e) => Err(anyhow::Error::msg(e)),
    }
}

/// Runs the test against a peer, finding one first if asked.
async fn measure(
    cancel: Cancel,
    emit: Emitter,
    p: Params,
    port: u16,
    src: Option<std::net::Ipv4Addr>,
) -> anyhow::Result<()> {
    let mut peer_spec = p.str("peer");
    let duration = p.dur("duration", Duration::from_secs(8));

    if peer_spec.is_empty() || peer_spec.eq_ignore_ascii_case(DISCOVER) {
        let discovery = lanspeed::discover_peers(
            src,
            lanspeed::DEFAULT_DISCOVERY_PORT,
            Duration::from_millis(1500),
        );
        let Some(peers) = cancel.run(discovery).await else { return Ok(()) };
        let peers = peers.map_err(|e| anyhow::anyhow!("discovery failed: {e}"))?;

        match peers.len() {
            0 => {
                emit.warn("no listening ntls found on this network");
                emit.info(
                    "start ntls on the other machine, choose LAN throughput, and set Role to Listen",
                );
                return Ok(());
            }
            1 => {
                emit.good(format!("found {} at {}", peers[0].name, peers[0].addr));
                peer_spec = peers[0].addr_port();
            }
            n => {
                emit.info(format!("found {n} peers — select one and press enter to test against it"));
                for peer in peers {
                    emit.row(
                        Status::Info,
                        peer.addr_port(),
                        cells![peer.addr, "found", "-", "-", peer.name],
                    );
                }
                return Ok(());
            }
        }
    }

    let target =
        if peer_spec.contains(':') { peer_spec.clone() } else { format!("{peer_spec}:{port}") };

    let directions: Vec<Direction> = match p.str("direction").as_str() {
        "down" => vec![Direction::ToClient],
        "up" => vec![Direction::ToServer],
        _ => vec![Direction::ToClient, Direction::ToServer],
    };

    for dir in directions {
        if cancel.is_cancelled() {
            return Ok(());
        }
        emit.info(format!("{dir} against {target} for {}…", elapsed(duration)));

        let series = format!("{dir} throughput");
        let sampler: crate::net::speedtest::Sampler = {
            let emit = emit.clone();
            Arc::new(move |bytes, interval| {
                if interval.is_zero() {
                    return;
                }
                let mbps = bytes as f64 * 8.0 / interval.as_secs_f64() / 1e6;
                emit.emit(Event::sample(series.clone(), " Mbps", mbps));
                emit.stats(vec![kv(dir.to_string(), format_bits(mbps * 1e6))]);
            })
        };

        match lanspeed::run_test(&cancel, &target, dir, duration, src, Some(sampler)).await {
            Err(e) => {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                emit.row(Status::Down, &target, cells![target, dir, "failed", "-", e]);
            }
            Ok(t) => emit.row(
                Status::Up,
                &target,
                cells![
                    target,
                    dir,
                    format_bits(t.bits_per_second()),
                    format_bytes(t.bytes),
                    format!("over {}", elapsed(t.elapsed))
                ],
            ),
        }
    }

    if !cancel.is_cancelled() {
        emit.good("throughput test complete");
    }
    Ok(())
}
