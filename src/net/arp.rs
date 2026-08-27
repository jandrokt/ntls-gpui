//! "Is this host on my link, and what is its MAC?"
//!
//! ARP only works on your own link, but it finds hosts that ignore pings.
//! Sending genuine ARP requests needs a raw link-layer socket and therefore
//! root, so without that privilege this nudges the kernel into resolving the
//! address and reads the neighbour table, marking any answer that came out of
//! the cache — those can be minutes old.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::iface;
#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
use super::arplink::{ArpLink, format_mac};

/// The outcome of a successful ARP probe.
#[derive(Clone, Debug, Default)]
pub struct ArpResult {
    pub mac: String,
    /// True when the answer came from an entry this run created rather than
    /// from a pre-existing cache entry.
    pub fresh: bool,
    pub rtt: Duration,
    /// Anything else worth saying about the answer, kept separate from the
    /// address so a vendor lookup still sees a clean MAC.
    pub note: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArpError {
    NotLocalLink,
    Timeout,
    Unsupported,
    Cancelled,
}

impl std::fmt::Display for ArpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ArpError::NotLocalLink => "not on local link",
            ArpError::Timeout => "no reply",
            ArpError::Unsupported => "ARP is not available on this platform",
            ArpError::Cancelled => "stopped",
        })
    }
}

struct Cache {
    table: HashMap<IpAddr, String>,
    read: Instant,
}

/// How the prober is asking.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Genuine ARP requests over a raw link-layer socket. The answer came off
    /// the wire just now and carries the host's real hardware address.
    Raw,
    /// The kernel's neighbour table, read after provoking a resolution. Needs
    /// no privilege, but the entries can be stale and recent macOS does not
    /// expose the table at all.
    Neighbour,
}

pub struct ArpProber {
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    link: Option<ArpLink>,
    /// Why the raw socket was unavailable, when it was.
    raw_error: Option<String>,
    cache: Mutex<Cache>,
    baseline: HashMap<IpAddr, String>,
    /// How long a neighbour-table read is reused. A scan probes many hosts at
    /// once and this keeps it to a handful of reads.
    ttl: Duration,
    /// Records that the table could not be seen at all, which on recent macOS
    /// means the cache is simply not readable.
    empty_table: bool,
}

impl ArpProber {
    /// Prepares a prober for the network reached through `interface`, whose
    /// address on it is `src`.
    ///
    /// Only the BSD family can open a raw link socket here, so `interface` and
    /// `src` are only read there. Everywhere else the neighbour table is the
    /// whole of what is available.
    pub fn new(
        #[allow(unused_variables)] interface: Option<&str>,
        #[allow(unused_variables)] src: Option<Ipv4Addr>,
    ) -> Result<ArpProber, String> {
        if !supported() {
            return Err("ARP probing is not supported on this platform".into());
        }

        #[allow(unused_mut)]
        let mut raw_error = None;
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        let link = match link_for(interface, src) {
            Ok(link) => Some(link),
            Err(e) => {
                raw_error = Some(e);
                None
            }
        };

        // The neighbour table is still worth reading: it costs nothing and on
        // the platforms that expose it, it answers for hosts that have been
        // spoken to recently without a single packet of ours.
        let baseline = read_neighbours().unwrap_or_default();

        Ok(ArpProber {
            #[cfg(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            link,
            raw_error,
            empty_table: baseline.is_empty(),
            cache: Mutex::new(Cache { table: baseline.clone(), read: Instant::now() }),
            baseline,
            ttl: Duration::from_millis(250),
        })
    }

    pub fn mode(&self) -> Mode {
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        if self.link.is_some() {
            return Mode::Raw;
        }
        Mode::Neighbour
    }

    /// Reports that hardware addresses will be missing: no raw socket, and no
    /// readable neighbour table either.
    pub fn blind(&self) -> bool {
        self.mode() == Mode::Neighbour && self.empty_table
    }

    /// Caveats worth showing the user before results arrive.
    pub fn notes(&self) -> Vec<String> {
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        if let Some(link) = &self.link {
            return vec![format!(
                "sending ARP requests on {} as {}",
                link.interface,
                format_mac(link.src_mac())
            )];
        }

        let mut out = Vec::new();
        if let Some(e) = &self.raw_error {
            out.push(format!(
                "no raw link-layer socket ({e}); reading the kernel neighbour table instead, whose entries may be minutes old"
            ));
        }
        if self.empty_table {
            out.push(
                "this system will not show hardware addresses to an unprivileged process — join the access_bpf group (Wireshark's ChmodBPF installs it) or run as root, and ntls will ask the wire directly"
                    .to_string(),
            );
        }
        out
    }

    /// Resolves the link-layer address of `dst`, waiting up to `timeout`.
    pub async fn probe(&self, dst: IpAddr, timeout: Duration) -> Result<ArpResult, ArpError> {
        if !dst.is_ipv4() {
            return Err(ArpError::Unsupported);
        }
        if !iface::is_local_link(dst) {
            return Err(ArpError::NotLocalLink);
        }
        // A host never ARPs for its own address, so answer for ourselves
        // rather than leaving a gap in the middle of a subnet sweep.
        if iface::is_local_addr(dst) {
            return Ok(ArpResult {
                mac: iface::self_mac_for(dst),
                fresh: true,
                ..Default::default()
            });
        }

        let start = Instant::now();

        // A real request, when we have a socket to send one on. The answer is
        // current by construction, which is the whole difference between this
        // and reading a cache.
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        if let Some(link) = &self.link {
            let IpAddr::V4(v4) = dst else { return Err(ArpError::Unsupported) };
            return match link.resolve(v4, timeout).await {
                Some((mac, rtt)) => Ok(ArpResult {
                    mac: format_mac(mac),
                    fresh: true,
                    rtt,
                    note: String::new(),
                }),
                None => Err(ArpError::Timeout),
            };
        }

        let was_cached = self.baseline.contains_key(&dst);
        nudge(dst);

        loop {
            if let Some(mac) = self.lookup(dst) {
                return Ok(ArpResult { mac, fresh: !was_cached, rtt: start.elapsed(), note: String::new() });
            }
            if start.elapsed() >= timeout {
                return Err(ArpError::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    fn lookup(&self, dst: IpAddr) -> Option<String> {
        let mut c = self.cache.lock().unwrap();
        if c.read.elapsed() > self.ttl
            && let Ok(t) = read_neighbours()
        {
            c.table = t;
            c.read = Instant::now();
        }
        c.table.get(&dst).cloned()
    }
}

/// Provokes link-layer resolution without sending anything meaningful. Port 9
/// is discard; the payload is irrelevant and a closed port is fine, because
/// the ARP exchange happens before the UDP datagram leaves the host.
fn nudge(dst: IpAddr) {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return };
    let _ = sock.send_to(&[0], SocketAddr::new(dst, 9));
}

/// Opens a link-layer handle on the right interface.
///
/// A named interface is taken at its word; otherwise the one carrying the
/// default route is the one the scan will be using.
#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn link_for(interface: Option<&str>, src: Option<Ipv4Addr>) -> Result<ArpLink, String> {
    let chosen = match interface.map(str::trim).filter(|n| {
        !n.is_empty() && !n.eq_ignore_ascii_case(iface::AUTO_INTERFACE)
    }) {
        Some(name) => iface::interfaces().into_iter().find(|i| i.name == name),
        None => match src {
            Some(addr) => iface::interfaces()
                .into_iter()
                .find(|i| i.addrs.iter().any(|p| p.addr == IpAddr::V4(addr))),
            None => iface::interfaces().into_iter().find(|i| i.default),
        },
    }
    .ok_or_else(|| "no interface to send ARP from".to_string())?;

    let addr = chosen
        .addrs
        .iter()
        .find_map(|p| match p.addr {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        })
        .ok_or_else(|| format!("{} has no IPv4 address", chosen.name))?;

    ArpLink::open(&chosen.name, addr).map_err(|e| e.to_string())
}

fn supported() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))
}

/// The current IPv4 neighbour table, keyed by address. Incomplete entries are
/// omitted.
fn read_neighbours() -> Result<HashMap<IpAddr, String>, String> {
    if cfg!(target_os = "linux") {
        read_proc_net_arp()
    } else if cfg!(windows) {
        read_arp_windows()
    } else {
        read_arp_command()
    }
}

/// Parses `/proc/net/arp`, avoiding a subprocess on Linux.
fn read_proc_net_arp() -> Result<HashMap<IpAddr, String>, String> {
    let text = std::fs::read_to_string("/proc/net/arp")
        .map_err(|e| format!("read neighbour table: {e}"))?;

    let mut out = HashMap::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        // Flag 0x2 (ATF_COM) means the entry is complete.
        let Ok(flags) = u32::from_str_radix(f[2].trim_start_matches("0x"), 16) else { continue };
        if flags & 0x2 == 0 {
            continue;
        }
        let (Ok(addr), Some(mac)) = (f[0].parse::<IpAddr>(), normalize_mac(f[3])) else { continue };
        out.insert(addr, mac);
    }
    Ok(out)
}

/// Parses `arp -an` on the BSD family, including macOS, where the neighbour
/// table is only reachable through the routing socket.
fn read_arp_command() -> Result<HashMap<IpAddr, String>, String> {
    let out = std::process::Command::new("arp")
        .arg("-an")
        .output()
        .map_err(|e| format!("read neighbour table: {e}"))?;

    let text = String::from_utf8_lossy(&out.stdout);
    let mut table = HashMap::new();
    for line in text.lines() {
        // ? (192.168.1.1) at a4:83:e7:1:2:3 on en0 ifscope [ethernet]
        let (Some(open), Some(close)) = (line.find('('), line.find(')')) else { continue };
        if close < open {
            continue;
        }
        let Ok(addr) = line[open + 1..close].parse::<IpAddr>() else { continue };
        let rest: Vec<&str> = line[close + 1..].split_whitespace().collect();
        if rest.len() < 2 || rest[0] != "at" {
            continue;
        }
        let Some(mac) = normalize_mac(rest[1]) else { continue }; // "(incomplete)"
        table.insert(addr, mac);
    }
    Ok(table)
}

/// Parses `arp -a` on Windows, which prints a table rather than a line per
/// entry and separates the octets with dashes.
///
/// ```text
/// Interface: 192.168.1.23 --- 0xa
///   Internet Address      Physical Address      Type
///   192.168.1.1           a4-83-e7-01-02-03     dynamic
/// ```
fn read_arp_windows() -> Result<HashMap<IpAddr, String>, String> {
    let out = std::process::Command::new("arp")
        .arg("-a")
        .output()
        .map_err(|e| format!("read neighbour table: {e}"))?;

    let text = String::from_utf8_lossy(&out.stdout);
    let mut table = HashMap::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 2 {
            continue;
        }
        // The heading rows and the "Interface:" line have no address in the
        // first column, so parsing it is the whole of the filtering.
        let Ok(addr) = fields[0].parse::<IpAddr>() else { continue };
        let Some(mac) = normalize_mac(fields[1]) else { continue };
        table.insert(addr, mac);
    }
    Ok(table)
}

/// Pads single-digit octets, which BSD's arp prints unpadded, accepts the
/// dashes Windows prints instead of colons, and rejects the all-zero address
/// used for unresolved entries.
fn normalize_mac(s: &str) -> Option<String> {
    let s = s.trim().replace('-', ":");
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut zero = true;
    let mut out = Vec::with_capacity(6);
    for p in parts {
        if p.is_empty() || p.len() > 2 {
            return None;
        }
        let v = u8::from_str_radix(p, 16).ok()?;
        zero &= v == 0;
        out.push(format!("{v:02x}"));
    }
    (!zero).then(|| out.join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hardware_address_is_read_however_the_system_prints_it() {
        // BSD pads nothing, Linux pads everything, Windows uses dashes.
        assert_eq!(normalize_mac("a4:83:e7:1:2:3").as_deref(), Some("a4:83:e7:01:02:03"));
        assert_eq!(normalize_mac("A4-83-E7-01-02-03").as_deref(), Some("a4:83:e7:01:02:03"));
        assert_eq!(normalize_mac("a4:83:e7:01:02:03").as_deref(), Some("a4:83:e7:01:02:03"));
        // An unresolved entry says nothing about what is there.
        assert_eq!(normalize_mac("00:00:00:00:00:00"), None);
        assert_eq!(normalize_mac("(incomplete)"), None);
    }
}
