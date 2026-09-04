//! Turning a human target specification into concrete addresses.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::iface;

/// Caps expansion so a typo like 10.0.0.0/8 fails fast with a clear message
/// instead of eating all available memory.
pub const MAX_HOSTS: usize = 65536;

/// An IPv4 or IPv6 network.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Prefix {
    pub addr: IpAddr,
    pub bits: u8,
}

impl Prefix {
    pub fn new(addr: IpAddr, bits: u8) -> Prefix {
        Prefix { addr, bits }.masked()
    }

    pub fn bit_len(&self) -> u8 {
        match self.addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        }
    }

    /// The prefix with every host bit cleared.
    pub fn masked(self) -> Prefix {
        let bits = self.bits.min(self.bit_len());
        let addr = match self.addr {
            IpAddr::V4(v4) => {
                let n = u32::from(v4);
                let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
                IpAddr::V4(Ipv4Addr::from(n & mask))
            }
            IpAddr::V6(v6) => {
                let n = u128::from(v6);
                let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
                IpAddr::V6(Ipv6Addr::from(n & mask))
            }
        };
        Prefix { addr, bits }
    }

    pub fn contains(&self, other: IpAddr) -> bool {
        match (self.masked().addr, other) {
            (IpAddr::V4(net), IpAddr::V4(a)) => {
                let mask = if self.bits == 0 { 0 } else { u32::MAX << (32 - self.bits) };
                u32::from(a) & mask == u32::from(net)
            }
            (IpAddr::V6(net), IpAddr::V6(a)) => {
                let mask = if self.bits == 0 { 0 } else { u128::MAX << (128 - self.bits) };
                u128::from(a) & mask == u128::from(net)
            }
            _ => false,
        }
    }

    /// The broadcast address of an IPv4 prefix, if it has one.
    pub fn broadcast(&self) -> Option<Ipv4Addr> {
        let IpAddr::V4(net) = self.masked().addr else { return None };
        if self.bits >= 31 {
            return None;
        }
        // A /0 gets this far whenever the OS declines to report an
        // interface's netmask: `if-addrs` hands back 0.0.0.0 for the mask and
        // the bit count comes out zero. Raising a one by 32 places to build
        // the host part was then a shift off the end of a u32, which panics
        // while the interfaces screen is drawing in a debug build and quietly
        // calls the broadcast 0.0.0.0 in a release one.
        Some(Ipv4Addr::from(u32::from(net) | (u32::MAX >> self.bits)))
    }

    /// Every host address in the prefix.
    ///
    /// For ordinary IPv4 subnets the network and broadcast addresses are not
    /// hosts, so they are skipped. A /31 and a /32 have no such reservation.
    pub fn hosts(&self) -> Result<Vec<IpAddr>, String> {
        let p = self.masked();
        let host_bits = p.bit_len() - p.bits;
        if host_bits > 20 {
            return Err(format!("{p} is too large to scan ({host_bits} host bits)"));
        }

        let skip_edges = matches!(p.addr, IpAddr::V4(_)) && host_bits >= 2;
        let count: u64 = 1u64 << host_bits;
        let mut out = Vec::with_capacity(count as usize);

        match p.addr {
            IpAddr::V4(base) => {
                let base = u32::from(base);
                for i in 0..count {
                    if skip_edges && (i == 0 || i == count - 1) {
                        continue;
                    }
                    out.push(IpAddr::V4(Ipv4Addr::from(base + i as u32)));
                }
            }
            IpAddr::V6(base) => {
                let base = u128::from(base);
                for i in 0..count {
                    out.push(IpAddr::V6(Ipv6Addr::from(base + i as u128)));
                }
            }
        }

        if out.len() > MAX_HOSTS {
            return Err(format!("{p} expands to {} hosts, past the {MAX_HOSTS} limit", out.len()));
        }
        Ok(out)
    }
}

impl std::fmt::Display for Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.addr, self.bits)
    }
}

pub fn parse_prefix(s: &str) -> Result<Prefix, String> {
    let (addr, bits) = s.split_once('/').ok_or_else(|| format!("bad CIDR {s:?}"))?;
    let addr: IpAddr = addr.trim().parse().map_err(|_| format!("bad CIDR {s:?}"))?;
    let bits: u8 = bits.trim().parse().map_err(|_| format!("bad CIDR {s:?}"))?;
    let max = if addr.is_ipv4() { 32 } else { 128 };
    if bits > max {
        return Err(format!("bad CIDR {s:?}: /{bits} is out of range"));
    }
    Ok(Prefix { addr, bits })
}

/// Turns a human target specification into a concrete list of addresses.
///
/// It accepts a comma-separated list whose parts may each be:
///
/// ```text
/// auto | local          the local interface's subnet
/// 192.168.1.0/24        a CIDR block
/// 192.168.1.10-20       a short range (last octet)
/// 192.168.1.10-192.168.1.60   an explicit range
/// 192.168.1.5           a single address
/// host.example.com      a name, resolved
/// ```
///
/// Duplicates are removed and the original ordering is kept. `iface` decides
/// what "auto" means; an empty name lets the routing table choose.
pub fn expand_targets_on(spec: &str, iface: &str) -> Result<Vec<IpAddr>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("empty target".into());
    }

    let mut out: Vec<IpAddr> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        for a in expand_one(part, iface)? {
            if !seen.insert(a) {
                continue;
            }
            if out.len() >= MAX_HOSTS {
                return Err(format!("target expands past {MAX_HOSTS} hosts; narrow the range"));
            }
            out.push(a);
        }
    }

    if out.is_empty() {
        return Err(format!("no hosts in {spec:?}"));
    }
    Ok(out)
}

fn expand_one(part: &str, iface_name: &str) -> Result<Vec<IpAddr>, String> {
    match part.to_ascii_lowercase().as_str() {
        "auto" | "local" | "lan" => {
            return iface::prefix_for_interface(iface_name)?.hosts();
        }
        _ => {}
    }

    if part.contains('/') {
        return parse_prefix(part)?.hosts();
    }

    // An address is an address before it is anything else.
    if let Ok(a) = part.parse::<IpAddr>() {
        return Ok(vec![a]);
    }

    // A dash separates a range only when what is on the left of it is an
    // address. Hostnames have dashes in them too, and far more often: reading
    // the dash first meant `my-server` was taken for a range from `my` to
    // `server`, refused as a bad range start, and never once looked up.
    if let Some(i) = part.find('-').filter(|&i| i > 0) {
        let (lo, hi) = (part[..i].trim(), part[i + 1..].trim());
        if lo.parse::<IpAddr>().is_ok() {
            return expand_range(lo, hi);
        }
    }

    // Fall back to DNS so hostnames work everywhere an address does.
    iface::resolve_host(part).map_err(|e| format!("cannot resolve {part:?}: {e}"))
}

fn expand_range(lo: &str, hi: &str) -> Result<Vec<IpAddr>, String> {
    let (lo, hi) = (lo.trim(), hi.trim());
    let start: IpAddr = lo.parse().map_err(|_| format!("bad range start {lo:?}"))?;

    let end: IpAddr = if hi.contains('.') || hi.contains(':') {
        hi.parse().map_err(|_| format!("bad range end {hi:?}"))?
    } else {
        // Shorthand: 192.168.1.10-20 means .10 through .20.
        let n: u32 = hi.parse().map_err(|_| format!("bad range end {hi:?}"))?;
        let IpAddr::V4(v4) = start else {
            return Err(format!("bad range end {hi:?}"));
        };
        if n > 255 {
            return Err(format!("range end {n} out of bounds"));
        }
        let mut o = v4.octets();
        o[3] = n as u8;
        IpAddr::V4(Ipv4Addr::from(o))
    };

    match (start, end) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let (a, b) = (u32::from(a).min(u32::from(b)), u32::from(a).max(u32::from(b)));
            if (b - a) as usize >= MAX_HOSTS {
                return Err(format!("range {lo}-{hi} is past the {MAX_HOSTS} host limit"));
            }
            Ok((a..=b).map(|n| IpAddr::V4(Ipv4Addr::from(n))).collect())
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let (a, b) = (u128::from(a).min(u128::from(b)), u128::from(a).max(u128::from(b)));
            if b - a >= MAX_HOSTS as u128 {
                return Err(format!("range {lo}-{hi} is past the {MAX_HOSTS} host limit"));
            }
            Ok((a..=b).map(|n| IpAddr::V6(Ipv6Addr::from(n))).collect())
        }
        _ => Err("range endpoints must be the same IP version".into()),
    }
}

/// Resolves a target specification one step at a time, so the user can see and
/// edit what will actually be scanned:
///
/// ```text
/// auto  ->  192.168.1.0/24  ->  192.168.1.1-192.168.1.254
/// ```
///
/// It returns `None` when nothing could be expanded further, so pressing Tab
/// again is safe.
pub fn expand_target_spec(spec: &str, iface_name: &str) -> Option<String> {
    let mut parts: Vec<String> = spec.split(',').map(str::to_string).collect();
    let mut changed = false;

    for part in parts.iter_mut() {
        let trimmed = part.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }

        if matches!(trimmed.to_ascii_lowercase().as_str(), "auto" | "local" | "lan") {
            if let Ok(p) = iface::prefix_for_interface(iface_name) {
                *part = p.to_string();
                changed = true;
            }
            continue;
        }

        if trimmed.contains('/')
            && let Ok(p) = parse_prefix(&trimmed)
            && let Ok(hosts) = p.hosts()
            && !hosts.is_empty()
        {
            *part = if hosts.len() == 1 {
                hosts[0].to_string()
            } else {
                format!("{}-{}", hosts[0], hosts[hosts.len() - 1])
            };
            changed = true;
        }
    }

    changed.then(|| parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn cidr_skips_network_and_broadcast() {
        let hosts = parse_prefix("192.168.1.0/29").unwrap().hosts().unwrap();
        assert_eq!(hosts.first(), Some(&v4("192.168.1.1")));
        assert_eq!(hosts.last(), Some(&v4("192.168.1.6")));
        assert_eq!(hosts.len(), 6);
    }

    #[test]
    fn a_thirty_two_is_its_own_host() {
        let hosts = parse_prefix("10.0.0.7/32").unwrap().hosts().unwrap();
        assert_eq!(hosts, vec![v4("10.0.0.7")]);
    }

    #[test]
    fn short_range_takes_the_last_octet() {
        let hosts = expand_targets_on("192.168.1.10-12", "").unwrap();
        assert_eq!(hosts, vec![v4("192.168.1.10"), v4("192.168.1.11"), v4("192.168.1.12")]);
    }

    #[test]
    fn explicit_range_crosses_octets() {
        let hosts = expand_targets_on("10.0.0.254-10.0.1.1", "").unwrap();
        assert_eq!(hosts.len(), 4);
        assert_eq!(hosts.last(), Some(&v4("10.0.1.1")));
    }

    #[test]
    fn a_hostname_with_a_dash_in_it_is_a_hostname() {
        // A dash means a range between two addresses. It also appears in most
        // hostnames anyone actually types, and reading it as a range first
        // meant every one of those was refused before it could be looked up.
        let refused = expand_targets_on("my-server.invalid", "").unwrap_err();
        assert!(refused.contains("cannot resolve"), "{refused}");
        assert!(!refused.contains("bad range"), "{refused}");
    }

    #[test]
    fn a_range_between_addresses_is_still_a_range() {
        let hosts = expand_targets_on("192.168.1.10-12", "").unwrap();
        assert_eq!(hosts.len(), 3);
        let hosts = expand_targets_on("10.0.0.254-10.0.1.1", "").unwrap();
        assert_eq!(hosts.len(), 4);
    }

    #[test]
    fn a_list_de_duplicates_and_keeps_order() {
        let hosts = expand_targets_on("10.0.0.2, 10.0.0.1, 10.0.0.2", "").unwrap();
        assert_eq!(hosts, vec![v4("10.0.0.2"), v4("10.0.0.1")]);
    }

    #[test]
    fn a_huge_prefix_is_refused_rather_than_expanded() {
        assert!(expand_targets_on("10.0.0.0/8", "").is_err());
    }

    #[test]
    fn expansion_walks_one_step_at_a_time() {
        let step = expand_target_spec("192.168.1.0/30", "").unwrap();
        assert_eq!(step, "192.168.1.1-192.168.1.2");
        // Nothing left to resolve, so Tab should move on instead.
        assert_eq!(expand_target_spec(&step, ""), None);
        assert_eq!(expand_target_spec("1.1.1.1", ""), None);
    }

    #[test]
    fn broadcast_of_a_prefix() {
        assert_eq!(
            parse_prefix("192.168.4.0/22").unwrap().broadcast().map(IpAddr::V4),
            Some(v4("192.168.7.255"))
        );
        assert_eq!(parse_prefix("10.0.0.0/31").unwrap().broadcast(), None);
    }

    #[test]
    fn a_prefix_with_no_mask_bits_broadcasts_to_all_ones() {
        // An interface whose netmask the OS withholds arrives as a /0, and
        // asking that prefix for its broadcast address used to shift a one
        // past the top of the word rather than answer.
        assert_eq!(
            parse_prefix("0.0.0.0/0").unwrap().broadcast().map(IpAddr::V4),
            Some(v4("255.255.255.255"))
        );
        assert_eq!(
            Prefix::new(v4("10.4.5.6"), 0).broadcast().map(IpAddr::V4),
            Some(v4("255.255.255.255"))
        );
    }

    #[test]
    fn contains_respects_the_mask() {
        let p = parse_prefix("192.168.1.0/24").unwrap();
        assert!(p.contains(v4("192.168.1.99")));
        assert!(!p.contains(v4("192.168.2.1")));
    }
}
