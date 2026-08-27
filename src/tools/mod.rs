//! The built-in ntls tools.
//!
//! To add one: implement [`crate::core::Tool`] in a new file here and append
//! it to the list in [`register`]. The picker, the form, validation, the
//! result table, the log, the chart and the run lifecycle all follow from the
//! trait, so there is nothing to wire up in the interface.

pub mod dns;
pub mod download;
pub mod http;
pub mod ipscan;
pub mod ping;
pub mod portscan;
pub mod prober;
pub mod speedtest;
pub mod stats;
pub mod subdomains;
pub mod throughput;
pub mod traceroute;

use std::sync::Arc;

use crate::core::{Registry, Tool};

/// Populates a registry with the built-in tools, in picker order.
pub fn register(r: &mut Registry) {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ping::Ping),
        Arc::new(traceroute::Traceroute),
        Arc::new(ipscan::IpScan),
        Arc::new(portscan::PortScan),
        Arc::new(dns::Dns),
        Arc::new(http::Http),
        Arc::new(subdomains::Subdomains),
        Arc::new(speedtest::SpeedTest),
        Arc::new(throughput::Throughput),
        Arc::new(download::Download),
    ];
    for t in tools {
        r.add(t);
    }
}

/// A registry containing every built-in tool.
pub fn all() -> Registry {
    let mut r = Registry::new();
    register(&mut r);
    r
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::core::{Cancel, Emitter, Event, FieldKind, Params, Role, Status};

    /// The conventions the shared machinery relies on. A tool that breaks one
    /// of these does not fail loudly at runtime — it just renders wrong — so
    /// they are checked here instead.
    #[test]
    fn every_tool_keeps_the_contract() {
        for tool in super::all().all() {
            let id = tool.id();
            let fields = tool.fields();
            let columns = tool.columns();

            assert!(!tool.title().is_empty(), "{id}: no title");
            assert!(!tool.desc().is_empty(), "{id}: no description");
            assert!(!tool.icon().is_empty(), "{id}: no icon");
            assert!(!fields.is_empty(), "{id}: no fields");
            assert!(!columns.is_empty(), "{id}: no columns");

            // Exactly one field carries the target. That is the field a result
            // handed off from another tool lands in.
            let targets = fields.iter().filter(|f| f.role == Role::Target).count();
            assert_eq!(targets, 1, "{id}: {targets} target fields, want exactly 1");

            // At most one port field, and it must be a text field.
            let ports: Vec<_> = fields.iter().filter(|f| f.role == Role::Ports).collect();
            assert!(ports.len() <= 1, "{id}: more than one ports field");
            for f in ports {
                assert_eq!(f.kind, FieldKind::Text, "{id}: ports field {} is not text", f.key);
            }

            // Exactly one column flexes to fill the space left over.
            let flexible = columns.iter().filter(|c| c.width == 0).count();
            assert_eq!(flexible, 1, "{id}: {flexible} flexible columns, want exactly 1");

            let defaults = Params::defaults(&fields);
            let mut keys = std::collections::HashSet::new();
            for f in &fields {
                assert!(keys.insert(f.key), "{id}: duplicate field key {}", f.key);
                assert!(!f.label.is_empty(), "{id}.{}: no label", f.key);
                assert!(!f.help.is_empty(), "{id}.{}: no help line", f.key);

                // Every default passes its own validator, so a form opens
                // valid and Run works without editing anything. The one
                // exception is an empty target: that is the field the user is
                // there to fill in.
                let user_supplies_it = f.role == Role::Target && f.default.is_empty();
                if let Some(v) = f.validate
                    && f.visible(&defaults)
                    && !user_supplies_it
                {
                    assert!(
                        v.check(&f.default).is_ok(),
                        "{id}.{}: default {:?} fails its own validator: {:?}",
                        f.key,
                        f.default,
                        v.check(&f.default)
                    );
                }

                match f.kind {
                    FieldKind::Select => {
                        assert!(!f.options.is_empty(), "{id}.{}: select with no options", f.key);
                        assert!(
                            f.options.iter().any(|o| o.value == f.default),
                            "{id}.{}: default {:?} is not one of the options",
                            f.key,
                            f.default
                        );
                        for o in &f.options {
                            assert!(!o.label.is_empty(), "{id}.{}: option with no label", f.key);
                        }
                    }
                    FieldKind::Bool => assert!(
                        f.default == "true" || f.default == "false",
                        "{id}.{}: boolean default {:?}",
                        f.key,
                        f.default
                    ),
                    FieldKind::Text => {}
                }

                // A hidden field is one the tool can also show, so the
                // condition must name a field that exists.
                if let Some(cond) = &f.visible_if {
                    let (crate::core::VisibleIf::Equals(key, _)
                    | crate::core::VisibleIf::NotEquals(key, _)) = cond;
                    assert!(
                        fields.iter().any(|other| other.key == *key),
                        "{id}.{}: visible_if names unknown field {key}",
                        f.key
                    );
                }
            }
        }
    }

    /// Runs a tool for real and collects everything it emits.
    fn drive(tool: &dyn crate::core::Tool, params: Params, run_for: Duration) -> Vec<Event> {
        let collected = Arc::new(Mutex::new(Vec::new()));
        let sink = collected.clone();
        let emit = Emitter::new(move |e| sink.lock().unwrap().push(e));

        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let cancel = Cancel::new();
            let stopper = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(run_for).await;
                stopper.cancel();
            });
            tool.run(crate::core::Run::fresh(cancel, params), emit)
                .await
                .expect("the tool returned an error");
        });

        
        collected.lock().unwrap().clone()
    }

    /// Drives the port scanner against a listener bound here, which exercises
    /// the whole pipeline — expansion, probing, rows, stats, progress —
    /// without touching anything outside this machine.
    #[test]
    fn a_port_scan_finds_a_socket_we_opened() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let tool = super::portscan::PortScan;
        let mut params = Params::defaults(&crate::core::Tool::fields(&tool));
        params.set("target", "127.0.0.1");
        params.set("ports", &format!("{},{}", port, port.wrapping_add(1)));
        params.set("proto", "tcp");
        params.set("banner", "false");
        params.set("showclosed", "true");
        params.set("timeout", "500ms");

        let events = drive(&tool, params, Duration::from_secs(10));

        let columns = crate::core::Tool::columns(&tool).len();
        let rows: Vec<&crate::core::Row> = events
            .iter()
            .filter_map(|e| match e {
                Event::Row(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(rows.len(), 2, "expected one row per probed port");

        // Every row carries exactly as many cells as there are columns, and a
        // target naming the host it refers to.
        for r in &rows {
            assert_eq!(r.cells.len(), columns, "row has the wrong number of cells");
            assert!(r.target.starts_with("127.0.0.1:"), "row target {:?}", r.target);
        }

        // Look up the row for the socket this test opened. The neighbouring
        // port belongs to the machine, not to us, and is occasionally in use
        // by something else — so nothing is claimed about it.
        let ours = rows
            .iter()
            .find(|r| r.target == format!("127.0.0.1:{port}"))
            .expect("no row for the port we opened");
        assert_eq!(ours.cells[1], format!("{port}/tcp"));
        assert_eq!(ours.cells[3], "open", "a socket we are listening on read as closed");
        assert_eq!(ours.status, Status::Up);

        // The bar closes at the end, and the summary is a log line.
        let last_progress = events.iter().rev().find_map(|e| match e {
            Event::Progress { done, total, .. } => Some((*done, *total)),
            _ => None,
        });
        assert_eq!(last_progress, Some((2, 2)));
        assert!(events.iter().any(|e| matches!(e, Event::Stats(s) if !s.is_empty())));
    }

    /// A resumed scan must skip what the interrupted one already covered, or
    /// resuming would duplicate every row it re-probed.
    #[test]
    fn a_resumed_scan_skips_what_is_already_done() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let tool = super::portscan::PortScan;
        assert!(crate::core::Tool::resumable(&tool), "the port scanner should be resumable");

        let mut params = Params::defaults(&crate::core::Tool::fields(&tool));
        params.set("target", "127.0.0.1");
        params.set("ports", &format!("{}-{}", port.saturating_sub(2), port));
        params.set("banner", "false");
        params.set("showclosed", "true");
        params.set("timeout", "400ms");

        // Told that everything up to the open port is done, only nothing is
        // left — the scan reports no rows and closes the bar.
        let collected = Arc::new(Mutex::new(Vec::new()));
        let sink = collected.clone();
        let emit = Emitter::new(move |e| sink.lock().unwrap().push(e));
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let run = crate::core::Run {
                cancel: Cancel::new(),
                params: params.clone(),
                done: std::iter::once(format!("127.0.0.1:{port}")).collect(),
                prior: Vec::new(),
                keep: false,
            };
            crate::core::Tool::run(&tool, run, emit).await.unwrap();
        });

        let events = collected.lock().unwrap().clone();
        assert!(
            events.iter().all(|e| !matches!(e, Event::Row(_))),
            "a fully covered range should probe nothing"
        );
        assert!(
            events.iter().any(|e| matches!(e, Event::Log { text, .. } if text.contains("resuming"))),
            "a resumed run should say so"
        );
    }

    /// The one tool that needs nothing but a loopback socket to prove the
    /// ICMP engine round-trips a real packet.
    #[test]
    fn ping_reaches_the_loopback() {
        // Whether ICMP can be spoken at all is the operating system's
        // decision, not ours: Linux only allows it unprivileged when
        // `net.ipv4.ping_group_range` covers this user's group, and Windows
        // wants Administrator. Where it is refused there is nothing to test,
        // so say so and stop rather than reporting a bug that is not there.
        if !crate::net::icmp::available() {
            eprintln!(
                "skipped: this machine will not open an ICMP socket \
                 (on Linux: sysctl -w net.ipv4.ping_group_range=\"0 2147483647\")"
            );
            return;
        }

        let tool = super::ping::Ping;
        let mut params = Params::defaults(&crate::core::Tool::fields(&tool));
        params.set("target", "127.0.0.1");
        params.set("count", "2");
        params.set("interval", "50ms");
        params.set("timeout", "2s");

        let events = drive(&tool, params, Duration::from_secs(15));
        let rows: Vec<&crate::core::Row> = events
            .iter()
            .filter_map(|e| match e {
                Event::Row(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(rows.len(), 2, "expected one row per probe");

        // The loopback answers its own pings, so anything else is a bug in
        // the engine rather than a network condition.
        for r in &rows {
            assert_eq!(r.status, Status::Up, "the loopback did not answer: {:?}", r.cells);
            // The TTL has a column of its own, and the loopback reports one
            // wherever the socket carries it. A Windows datagram socket does
            // not, and the column is blank there by design.
            #[cfg(unix)]
            assert!(
                r.cells[3].parse::<u8>().is_ok_and(|t| t > 0),
                "no TTL reported: {:?}",
                r.cells
            );
        }
        assert!(events.iter().any(|e| matches!(e, Event::Sample { .. })), "no chart samples");
        // A run that received everything it sent reports no loss.
        let stats = events
            .iter()
            .rev()
            .find_map(|e| match e {
                Event::Stats(s) => Some(s.clone()),
                _ => None,
            })
            .expect("no stat bar");
        let loss = stats.iter().find(|kv| kv.k == "loss").expect("no loss figure");
        assert_eq!(loss.v, "0%");
    }
}
