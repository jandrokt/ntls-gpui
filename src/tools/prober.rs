//! The shared vocabulary of "is this host alive?".
//!
//! Keeping the reachability method here means a new tool gets the same
//! options, and a new method becomes available everywhere at once.

use std::net::IpAddr;
use std::time::Duration;

use crate::core::{Cancel, Emitter, Event, Field, Level, Opt};
use crate::net::{arp, iface, icmp, oui};

pub const METHOD_ICMP: &str = "icmp";
pub const METHOD_ARP: &str = "arp";

/// The standard interface selector. Its options are built from the machine's
/// current interfaces, so a tool picks up new ones without any change of its
/// own.
pub fn iface_field() -> Field {
    let mut opts = vec![Opt::new(
        iface::AUTO_INTERFACE,
        "auto",
        "whichever interface the routing table picks for the target",
    )];
    for i in iface::interfaces() {
        let mut desc =
            i.addrs.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" ");
        if i.default {
            desc += " · default route";
        }
        opts.push(Opt { value: i.name.clone(), label: i.name, desc });
    }
    Field::select(
        "iface",
        "Interface",
        "Which interface to send from. Pick one to scan a network you are multi-homed onto",
        iface::AUTO_INTERFACE,
        opts,
    )
}

/// The standard reachability-method selector.
pub fn method_field() -> Field {
    Field::select(
        "method",
        "Method",
        "ICMP works anywhere; ARP only on your local network, but finds hosts that ignore pings",
        METHOD_ICMP,
        vec![
            Opt::new(METHOD_ICMP, "ping (ICMP)", "echo request / echo reply"),
            Opt::new(METHOD_ARP, "arping (ARP)", "local link only, sees hosts that drop ICMP"),
        ],
    )
}

/// A target field with the usual expansion and validation.
pub fn target_field(label: &'static str, placeholder: &str, help: &'static str) -> Field {
    Field::text("target", label, help)
        .placeholder(placeholder)
        .role(crate::core::Role::Target)
        .validate(crate::core::Validator::Required)
        .expand(crate::core::Expand::Target)
}

/// The outcome of one reachability check.
#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub rtt: Duration,
    pub from: Option<IpAddr>,
    /// The TTL an echo reply carried, zero when the method does not report
    /// one. It gets a column of its own: it is a number, and it is the number
    /// that tells you what kind of thing answered.
    pub ttl: u8,
    /// Anything else worth saying that is not already a column.
    pub info: String,
    /// The hardware behind the address, when it could be determined. It comes
    /// free with ARP, and is looked up separately for hosts found over ICMP.
    pub mac: String,
    pub vendor: String,
    /// Marks an answer that is true but weaker than it looks, such as an ARP
    /// entry that was already in the cache.
    pub suspect: bool,
}

impl Probe {
    /// The MAC and its vendor for display, empty when neither is known.
    pub fn hardware(&self) -> String {
        match (self.mac.as_str(), self.vendor.as_str()) {
            ("", _) => String::new(),
            (mac, "") => mac.to_string(),
            (mac, vendor) => format!("{mac}  {vendor}"),
        }
    }

    /// The TTL for a table cell, empty when there is none to report.
    pub fn ttl_text(&self) -> String {
        if self.ttl == 0 { String::new() } else { self.ttl.to_string() }
    }

    /// Whatever is left over once the columns have taken their share.
    pub fn detail_line(&self) -> String {
        [self.hardware(), self.info.clone()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// Why a probe failed, in the words the table cell wants.
pub fn failure_text(e: &ProbeError) -> String {
    e.to_string()
}

#[derive(Clone, Debug)]
pub enum ProbeError {
    Icmp(icmp::PingError),
    Arp(arp::ArpError),
    Other(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::Icmp(icmp::PingError::Timeout) => f.write_str("timeout"),
            ProbeError::Icmp(e) => write!(f, "{e}"),
            ProbeError::Arp(arp::ArpError::Timeout) => f.write_str("timeout"),
            ProbeError::Arp(e) => write!(f, "{e}"),
            ProbeError::Other(s) => f.write_str(s),
        }
    }
}

/// Hides the difference between the reachability methods behind one call.
/// Both backends share their sockets and caches across the whole run.
pub struct Prober {
    kind: String,
    pinger: Option<icmp::Pinger>,
    arp: Option<arp::ArpProber>,
    payload: usize,
    timeout: Duration,
    /// Resolves hardware addresses for hosts found over ICMP, which does not
    /// reveal them on its own. `None` when the lookup is off or unavailable.
    macs: Option<arp::ArpProber>,
    notes: Vec<(Level, String)>,
}

/// How long a hardware lookup is given, and the range the scan's own timeout
/// can move it inside.
///
/// It used to be a flat 400ms, on the reasoning that the host had already
/// answered a ping. But an ARP reply on a wireless link routinely takes longer
/// than that, so the column came back filled in on one scan and empty on the
/// next for no reason anybody could see. The scan's own timeout is the better
/// guide: somebody who raised it was asking for more patience everywhere.
const MAC_LOOKUP_MIN: Duration = Duration::from_millis(900);
const MAC_LOOKUP_MAX: Duration = Duration::from_millis(2500);

impl Prober {
    /// Builds the backend named by `method`, preparing it for the given
    /// targets.
    pub fn new(
        p: &crate::core::Params,
        targets: &[IpAddr],
        timeout: Duration,
    ) -> Result<Prober, String> {
        let mut kind = p.str("method");
        if kind.is_empty() {
            kind = METHOD_ICMP.into();
        }
        let src = iface::resolve_interface(&p.str("iface"))?;
        let iface_name = p.str("iface");

        let mut pr = Prober {
            kind,
            pinger: None,
            arp: None,
            payload: p.usize("size", 56),
            timeout,
            macs: None,
            notes: Vec::new(),
        };

        if let Some(src) = src {
            pr.notes.push((Level::Info, format!("sending from {src}")));
        }

        match pr.kind.as_str() {
            METHOD_ICMP => {
                let v4 = targets.iter().any(IpAddr::is_ipv4);
                let v6 = targets.iter().any(IpAddr::is_ipv6);
                let pinger = icmp::Pinger::new(v4 || targets.is_empty(), v6, src)?;
                if pinger.privileged() {
                    pr.notes
                        .push((Level::Warn, "using a raw ICMP socket (running privileged)".into()));
                }
                pr.pinger = Some(pinger);
            }
            METHOD_ARP => {
                let arp = arp::ArpProber::new(Some(&iface_name), src)?;
                for n in arp.notes() {
                    pr.notes.push((Level::Warn, format!("ARP: {n}")));
                }
                pr.arp = Some(arp);
            }
            other => return Err(format!("unknown method {other:?}")),
        }

        // ICMP tells you a host is there but nothing about what it is. When
        // the targets are on this link, a second ARP lookup fills that in, and
        // it only runs for hosts that actually answered.
        if pr.kind == METHOD_ICMP
            && p.bool("vendors")
            && targets.iter().any(|t| iface::is_local_link(*t))
        {
            pr.macs = arp::ArpProber::new(Some(&iface_name), src).ok();
            // An empty hardware column is otherwise unexplained, and the
            // reason is worth saying once.
            match pr.macs.as_ref() {
                Some(macs) if macs.blind() => {
                    for n in macs.notes() {
                        pr.notes.push((Level::Warn, n));
                    }
                }
                Some(macs) if macs.mode() == arp::Mode::Raw => {
                    pr.notes.push((
                        Level::Info,
                        "resolving hardware addresses with real ARP requests".into(),
                    ));
                }
                _ => {}
            }
        }

        Ok(pr)
    }

    /// How long to wait for a hardware address, given how patient the scan
    /// itself was told to be.
    fn mac_timeout(&self) -> Duration {
        self.timeout.clamp(MAC_LOOKUP_MIN, MAC_LOOKUP_MAX)
    }

    /// Caveats worth showing the user before results start arriving.
    pub fn emit_notes(&self, emit: &Emitter) {
        for (level, text) in &self.notes {
            emit.emit(Event::log(*level, text.clone()));
        }
    }

    pub async fn probe(&self, cancel: &Cancel, addr: IpAddr) -> Result<Probe, ProbeError> {
        if cancel.is_cancelled() {
            return Err(ProbeError::Icmp(icmp::PingError::Cancelled));
        }

        if let Some(arp) = &self.arp {
            let res = cancel
                .run(arp.probe(addr, self.timeout))
                .await
                .ok_or(ProbeError::Arp(arp::ArpError::Cancelled))?
                .map_err(ProbeError::Arp)?;
            let info = if res.fresh {
                res.note.clone()
            } else {
                format!("{} cached", res.note).trim().to_string()
            };
            return Ok(Probe {
                rtt: res.rtt,
                from: Some(addr),
                ttl: 0,
                info,
                vendor: oui::describe(&res.mac),
                mac: res.mac,
                suspect: !res.fresh,
            });
        }

        let pinger = self.pinger.as_ref().ok_or(ProbeError::Icmp(icmp::PingError::NoSocket))?;
        let rep = cancel
            .run(pinger.ping(addr, self.payload, self.timeout))
            .await
            .ok_or(ProbeError::Icmp(icmp::PingError::Cancelled))?
            .map_err(ProbeError::Icmp)?;

        let mut out = Probe {
            rtt: rep.rtt,
            from: Some(rep.from),
            ttl: rep.ttl,
            ..Default::default()
        };

        if let Some(macs) = &self.macs
            && iface::is_local_link(addr)
            && let Some(Ok(mac)) = cancel.run(macs.probe(addr, self.mac_timeout())).await
        {
            out.vendor = oui::describe(&mac.mac);
            out.mac = mac.mac;
            if !mac.note.is_empty() {
                out.info = mac.note.clone();
            }
        }
        Ok(out)
    }
}

/// Expands a target specification and reports what it found, so the log makes
/// the scope of a run obvious before it starts.
pub fn resolve_targets(spec: &str, iface_name: &str, emit: &Emitter) -> Result<Vec<IpAddr>, String> {
    let addrs = crate::net::target::expand_targets_on(spec, iface_name)?;
    if addrs.len() == 1 {
        emit.info(format!("target {}", addrs[0]));
    } else {
        emit.info(format!(
            "{spec} expands to {} hosts ({} … {})",
            addrs.len(),
            addrs[0],
            addrs[addrs.len() - 1]
        ));
    }
    Ok(addrs)
}

/// How many targets are somewhere ARP cannot reach.
pub fn off_link_count(targets: &[IpAddr]) -> usize {
    targets.iter().filter(|t| !iface::is_local_link(**t)).count()
}
