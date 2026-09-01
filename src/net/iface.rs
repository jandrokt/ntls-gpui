//! What this machine is attached to: interfaces, their networks, and the
//! address the kernel would send from.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};

use super::target::Prefix;

/// The value meaning "let the routing table decide".
pub const AUTO_INTERFACE: &str = "auto";

/// A summary of one network interface, as shown on the main screen.
#[derive(Clone, Debug)]
pub struct Iface {
    pub name: String,
    /// The IPv4 networks assigned to the interface.
    pub addrs: Vec<Prefix>,
    /// The hardware address, empty for interfaces that have none (VPN
    /// tunnels) and for those where the OS declined to reveal it.
    pub mac: String,
    /// Marks the interface carrying the default route.
    pub default: bool,
    /// Marks an interface with no hardware address, which in practice means a
    /// tunnel instead of a physical port.
    pub virtual_: bool,
}

/// Lists the interfaces actually carrying IPv4 traffic: up, not loopback, and
/// holding at least one address. The interface with the default route sorts
/// first.
///
/// Interfaces that are up but unaddressed are left out deliberately. A Mac
/// reports a dozen of them and none tell you anything about your connectivity.
pub fn interfaces() -> Vec<Iface> {
    let Ok(list) = if_addrs::get_if_addrs() else { return Vec::new() };
    let outbound = outbound_addr().ok();

    let mut by_name: Vec<Iface> = Vec::new();
    for i in list {
        if i.is_loopback() {
            continue;
        }
        let if_addrs::IfAddr::V4(v4) = &i.addr else { continue };

        let bits = mask_bits(v4.netmask);
        let prefix = Prefix { addr: IpAddr::V4(v4.ip), bits };
        let is_default = outbound == Some(v4.ip);

        if let Some(existing) = by_name.iter_mut().find(|e| e.name == i.name) {
            existing.addrs.push(prefix);
            existing.default |= is_default;
            continue;
        }

        // An interface with no hardware address at all is a tunnel. One whose
        // address the OS withholds is still a real port, so it must not be
        // mistaken for a virtual one just because we cannot see its MAC.
        let reported = mac_address::mac_address_by_name(&i.name)
            .ok()
            .flatten()
            .map(|m| m.to_string().to_lowercase());
        let physical = reported.is_some();
        let mac = reported.filter(|m| !super::oui::withheld(m)).unwrap_or_default();

        by_name.push(Iface {
            name: i.name.clone(),
            addrs: vec![prefix],
            virtual_: !physical,
            mac,
            default: is_default,
        });
    }

    by_name.sort_by(|a, b| {
        b.default
            .cmp(&a.default)
            .then(a.virtual_.cmp(&b.virtual_))
            .then(a.name.cmp(&b.name))
    });
    by_name
}

fn mask_bits(mask: Ipv4Addr) -> u8 {
    u32::from(mask).count_ones() as u8
}

/// Reports whether a prefix contains other hosts to look for. A /31 or /32
/// does not, the usual shape of a VPN tunnel.
fn scannable(p: &Prefix) -> bool {
    p.addr.is_ipv4() && p.bits <= 30
}

/// The local IPv4 address the kernel would use to reach the internet. No
/// packet is sent: connecting a UDP socket only sets up routing state.
pub fn outbound_addr() -> Result<Ipv4Addr, String> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("no usable IPv4 interface: {e}"))?;
    sock.connect("198.51.100.1:9")
        .map_err(|e| format!("no usable IPv4 interface: {e}"))?;
    match sock.local_addr().map_err(|e| e.to_string())? {
        SocketAddr::V4(a) => Ok(*a.ip()),
        _ => Err("no IPv4 address on the outbound interface".into()),
    }
}

/// The IPv4 subnet "auto" expands to.
///
/// That is normally the network of the interface carrying the default route,
/// but a VPN tunnel is often a /32 with nothing else in it. When the default
/// route leads somewhere unscannable, the first real attached network is a far
/// more useful answer than an empty one.
pub fn local_prefix() -> Result<Prefix, String> {
    let outbound = outbound_addr();

    if let Ok(addr) = outbound {
        for i in interfaces() {
            for p in &i.addrs {
                if p.addr == IpAddr::V4(addr) && scannable(p) {
                    return Ok(p.masked());
                }
            }
        }
    }

    // Fall back to an attached network with room for other hosts, preferring
    // interfaces with real hardware behind them.
    let mut fallback: Option<Prefix> = None;
    for i in interfaces() {
        for p in &i.addrs {
            if !scannable(p) {
                continue;
            }
            if !i.virtual_ {
                return Ok(p.masked());
            }
            fallback.get_or_insert(p.masked());
        }
    }
    if let Some(p) = fallback {
        return Ok(p);
    }

    // We know our own address but nothing about its mask; assume a /24, which
    // is right on essentially every home and office LAN.
    let addr = outbound?;
    Ok(Prefix::new(IpAddr::V4(addr), 24))
}

/// The network "auto" means for a given interface selection. An empty or
/// "auto" name defers to [`local_prefix`].
pub fn prefix_for_interface(name: &str) -> Result<Prefix, String> {
    let name = name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case(AUTO_INTERFACE) {
        return local_prefix();
    }
    let i = interfaces()
        .into_iter()
        .find(|i| i.name == name)
        .ok_or_else(|| format!("no interface named {name:?}"))?;
    i.addrs
        .first()
        .map(|p| p.masked())
        .ok_or_else(|| format!("interface {name} has no IPv4 network"))
}

/// Turns an interface name into the source address to send from. The name
/// "auto" (or an empty string) leaves the choice to the routing table.
pub fn resolve_interface(name: &str) -> Result<Option<Ipv4Addr>, String> {
    let name = name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case(AUTO_INTERFACE) {
        return Ok(None);
    }
    let i = interfaces()
        .into_iter()
        .find(|i| i.name == name)
        .ok_or_else(|| format!("no interface named {name:?}"))?;
    match i.addrs.first().map(|p| p.addr) {
        Some(IpAddr::V4(a)) => Ok(Some(a)),
        _ => Err(format!("interface {name} has no IPv4 address")),
    }
}

/// What this machine's own addressing looks like: the networks it is attached
/// to, and the addresses it holds on them.
#[derive(Clone, Default)]
struct Own {
    prefixes: Vec<Prefix>,
    addrs: Vec<IpAddr>,
}

/// How long that answer stands before it is worked out again.
///
/// [`interfaces`] asks the kernel for the interface list, every hardware
/// address on it and the outbound route, every time it is called. The results
/// table asks which addresses are this machine's on every frame it draws, and
/// a subnet sweep asks whether each of 254 targets is on this link. Neither
/// is worth that, and an interface that came up half a second ago is not worth
/// paying it for either.
const OWN_FOR: std::time::Duration = std::time::Duration::from_millis(1000);

static OWN: std::sync::Mutex<Option<(std::time::Instant, Own)>> = std::sync::Mutex::new(None);

fn own() -> Own {
    let mut held = OWN.lock().expect("the interface snapshot");
    if let Some((at, snapshot)) = held.as_ref()
        && at.elapsed() < OWN_FOR
    {
        return snapshot.clone();
    }
    let list = interfaces();
    let snapshot = Own {
        prefixes: list.iter().flat_map(|i| i.addrs.iter().map(|p| p.masked())).collect(),
        addrs: list.iter().flat_map(|i| i.addrs.iter().map(|p| p.addr)).collect(),
    };
    *held = Some((std::time::Instant::now(), snapshot.clone()));
    snapshot
}

/// Forgets the snapshot, for the places that have just been told the machine's
/// addressing has changed.
pub fn forget_snapshot() {
    *OWN.lock().expect("the interface snapshot") = None;
}

/// Every IPv4 network this host is directly attached to, skipping loopback.
/// It is how the ARP prober decides whether a target is on the local link.
pub fn local_prefixes() -> Vec<Prefix> {
    own().prefixes
}

/// Reports whether `addr` sits in one of this host's directly attached
/// networks, and so can be reached with ARP.
pub fn is_local_link(addr: IpAddr) -> bool {
    addr.is_ipv4() && own().prefixes.iter().any(|p| p.contains(addr))
}

/// Every IPv4 address assigned to this host.
pub fn local_addrs() -> Vec<IpAddr> {
    own().addrs
}

/// Reports whether `addr` belongs to this host.
pub fn is_local_addr(addr: IpAddr) -> bool {
    own().addrs.contains(&addr)
}

/// This host's hardware address on the interface serving `addr`, empty when
/// the OS will not say.
pub fn self_mac_for(addr: IpAddr) -> String {
    let Some(i) = interfaces().into_iter().find(|i| i.addrs.iter().any(|p| p.contains(addr)))
    else {
        return String::new();
    };
    if !i.mac.is_empty() {
        return i.mac;
    }
    // The ordinary APIs withhold it on recent macOS; the configuration
    // database still knows.
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    if let Some(mac) = super::arplink::real_mac(&i.name) {
        return super::arplink::format_mac(mac);
    }
    String::new()
}

/// Resolves a hostname to addresses, preferring IPv4 so that the scanning
/// tools behave predictably on dual-stack networks.
///
/// This blocks; call it from a blocking task.
pub fn resolve_host(host: &str) -> Result<Vec<IpAddr>, String> {
    if let Ok(a) = host.parse::<IpAddr>() {
        return Ok(vec![a]);
    }
    let mut addrs: Vec<IpAddr> = (host, 0u16)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .map(|sa| sa.ip())
        .collect();
    if addrs.is_empty() {
        return Err("no addresses returned".into());
    }
    addrs.sort_by_key(|a| !a.is_ipv4());
    addrs.dedup();
    Ok(addrs)
}
