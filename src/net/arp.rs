//! "Is this host on my link, and what is its MAC?"
//!
//! ARP only works on your own link, but it finds hosts that ignore pings.
//! Sending genuine ARP requests needs a raw link-layer socket and therefore
//! root, so without that privilege this nudges the kernel into resolving the
//! address and reads the neighbour table, marking any answer that came out of
//! the cache, since those can be minutes old.

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
    /// True when the wire, or this run's own resolution, answered for it just
    /// now. False for an address taken from a cache or remembered from an
    /// earlier run: still the best answer anything has, but not proof that
    /// what holds it is still there.
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

impl Cache {
    /// Whether what we are holding is old enough to be worth reading again.
    fn stale(&self, ttl: Duration) -> bool {
        self.read.elapsed() > ttl
    }

    /// Takes the outcome of one neighbour-table read.
    ///
    /// The stamp moves whether or not the read produced anything, because a
    /// read that failed is still a read that was attempted. It used to move
    /// only on success, so a system where the read can never succeed - no
    /// `/proc/net/arp`, no `arp` on the path - was permanently overdue for a
    /// refresh, and the probe loop, which asks again every forty milliseconds
    /// and has as many loops running as there are hosts in the sweep, tried
    /// the read again on every single poll for the whole length of the scan.
    ///
    /// A read that said nothing is also no reason to forget what was already
    /// known: the table stays as it was until something replaces it.
    fn adopt(&mut self, read: Result<HashMap<IpAddr, String>, String>) {
        if let Ok(table) = read {
            self.table = table;
        }
        self.read = Instant::now();
    }
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
    /// Held for the length of a neighbour-table read, so that a sweep's many
    /// probes make one read between them instead of one apiece.
    refreshing: tokio::sync::Mutex<()>,
    baseline: HashMap<IpAddr, String>,
    /// How long a neighbour-table read is reused. A scan probes many hosts at
    /// once and this keeps it to a handful of reads.
    ttl: Duration,
    /// The address the nudge below goes out from, so that resolution happens
    /// on the interface the scan was told to use.
    src: Option<Ipv4Addr>,
    /// Records that no hardware address is ever going to come out of the
    /// neighbour table, so the empty column can be explained before the
    /// results arrive.
    withheld: bool,
    /// Why the table could not be read, when that is the reason it will not
    /// answer. Worth quoting: somebody whose `arp` is not on the path is not
    /// helped by being told about privileges.
    table_error: Option<String>,
}

impl ArpProber {
    /// Prepares a prober for the network reached through `interface`, whose
    /// address on it is `src`.
    ///
    /// Only the BSD family can open a raw link socket here, so `interface` is
    /// only read there. Everywhere else the neighbour table is the whole of
    /// what is available, and `src` is what keeps that path on the chosen
    /// interface.
    pub fn new(
        #[allow(unused_variables)] interface: Option<&str>,
        src: Option<Ipv4Addr>,
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
        let read = read_neighbours();
        let withheld = table_withheld(&read, EMPTY_TABLE_MEANS_WITHHELD);
        let table_error = read.as_ref().err().cloned();
        let baseline = read.unwrap_or_default();

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
            withheld,
            table_error,
            cache: Mutex::new(Cache { table: baseline.clone(), read: Instant::now() }),
            refreshing: tokio::sync::Mutex::new(()),
            baseline,
            ttl: Duration::from_millis(250),
            src,
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
    /// neighbour table that is going to answer either.
    pub fn blind(&self) -> bool {
        self.mode() == Mode::Neighbour && self.withheld
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
        if self.withheld {
            // The privilege advice belongs only to the system that answered
            // the read and said nothing, which is macOS withholding the cache
            // from an unprivileged process. A read that failed outright has a
            // reason of its own, and naming it beats sending somebody after a
            // group and an installer that only exist on another platform.
            out.push(match &self.table_error {
                Some(e) => format!(
                    "the kernel neighbour table cannot be read ({e}); hardware addresses will be missing"
                ),
                None => "this system will not show hardware addresses to an unprivileged process. Join the access_bpf group (Wireshark's ChmodBPF installs it) or run as root, and ntls will ask the wire directly"
                    .to_string(),
            });
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
        // so a subnet sweep has no gap in the middle of it.
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
                Some(answer) => Ok(ArpResult {
                    mac: format_mac(answer.mac),
                    fresh: answer.fresh,
                    rtt: answer.rtt,
                    // An address nothing answered for just now, offered from
                    // what it was last seen to be, has to say so.
                    note: if answer.fresh { String::new() } else { "last seen".into() },
                }),
                None => Err(ArpError::Timeout),
            };
        }

        let was_cached = self.baseline.contains_key(&dst);
        nudge(dst, self.src);

        loop {
            if let Some(mac) = self.lookup(dst).await {
                return Ok(ArpResult { mac, fresh: !was_cached, rtt: start.elapsed(), note: String::new() });
            }
            if start.elapsed() >= timeout {
                return Err(ArpError::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    /// The hardware address the neighbour table has for `dst`, reading the
    /// table again first if what we are holding has gone stale.
    ///
    /// Everywhere but Linux that read means forking `arp` and waiting for it,
    /// which has no business happening on a runtime thread: one pool serves
    /// every open job, and a sweep polling here from dozens of tasks at once
    /// stopped all of them - the progress bar and whatever else was running
    /// included - for as long as the subprocess took, which on Windows is
    /// tens of milliseconds a time. The read goes to the blocking pool, and
    /// only one goes at a time so that the tasks which arrive together do not
    /// each fork an `arp` of their own.
    async fn lookup(&self, dst: IpAddr) -> Option<String> {
        if self.stale() {
            let _one_at_a_time = self.refreshing.lock().await;
            // Whoever waited for that is very likely to find the read someone
            // else was making already done.
            if self.stale() {
                let read = match tokio::task::spawn_blocking(read_neighbours).await {
                    Ok(read) => read,
                    Err(e) => Err(format!("read neighbour table: {e}")),
                };
                self.cache.lock().unwrap().adopt(read);
            }
        }
        self.cached(dst)
    }

    fn stale(&self) -> bool {
        self.cache.lock().unwrap().stale(self.ttl)
    }

    /// What the table says right now, with nothing read on the way.
    fn cached(&self, dst: IpAddr) -> Option<String> {
        self.cache.lock().unwrap().table.get(&dst).cloned()
    }
}

/// Provokes link-layer resolution without sending anything meaningful. Port 9
/// is discard; the payload is irrelevant and a closed port is fine, because
/// the ARP exchange happens before the UDP datagram leaves the host.
///
/// It goes out of the chosen interface first, and out of whatever the routing
/// table prefers only if that interface cannot carry it: an explicit range
/// need not lie on the chosen interface's own network, and a host that some
/// other interface can reach must not be timed out for that.
fn nudge(dst: IpAddr, src: Option<Ipv4Addr>) {
    let to = SocketAddr::new(dst, 9);
    if let Some(sock) = nudge_socket(src)
        && sock.send_to(&[0], to).is_ok()
    {
        return;
    }
    if src.is_none() {
        return;
    }
    if let Some(sock) = nudge_socket(None) {
        let _ = sock.send_to(&[0], to);
    }
}

/// The socket a nudge goes out of, bound to the address of the interface the
/// scan was told to use.
///
/// Leaving it unbound left the choice to the routing table, which is not the
/// interface the user picked whenever the two disagree. On a machine
/// multi-homed onto networks that overlap - a wired LAN and a tunnel both
/// numbered 192.168.1.0/24, say - the resolution then happened on the other
/// link, and the address that came back over it belonged to a different host
/// altogether. A bind that fails, for an address that has just gone away with
/// its interface, is no reason not to ask at all.
fn nudge_socket(src: Option<Ipv4Addr>) -> Option<UdpSocket> {
    UdpSocket::bind(SocketAddr::new(IpAddr::V4(src.unwrap_or(Ipv4Addr::UNSPECIFIED)), 0)).ok()
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

/// Whether the neighbour table can be read here at all.
///
/// This has to agree with [`read_neighbours`], which is what actually does the
/// reading: Linux has the file, Windows has `arp -a`, and the rest of the Unix
/// family has `arp -an`. Saying no to a system that in fact has a reader is
/// how the Windows parser below came to be written, shipped, and never once
/// called: probing refused before it got that far, so a scan there resolved no
/// hardware address at all and the MAC and vendor columns were always empty.
fn supported() -> bool {
    cfg!(any(
        target_os = "linux",
        windows,
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))
}

/// Whether a neighbour table that reads as empty is a system refusing to show
/// it. Recent macOS, and the BSDs that share its routing socket, answer the
/// read and say nothing whatever has just been spoken to. Linux and Windows
/// hand over what they hold, so an empty table there is only a cold one and
/// the nudge fills it.
const EMPTY_TABLE_MEANS_WITHHELD: bool = cfg!(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
));

/// Whether one neighbour-table read means no hardware address is ever going to
/// come out of the table, as against a table that has nothing in it yet.
///
/// The two used to be one thing, and the warning that followed told whoever
/// saw it to join `access_bpf` and install Wireshark's ChmodBPF - a group and
/// an installer that exist only on macOS, along with the raw-socket path they
/// unlock. On Linux and Windows an empty table at the start of a scan is
/// merely a cold cache, so anyone scanning from a machine that had not spoken
/// to a neighbour yet was sent after a privilege that would have changed
/// nothing, for a column that filled in by itself a moment later.
fn table_withheld(
    read: &Result<HashMap<IpAddr, String>, String>,
    empty_means_withheld: bool,
) -> bool {
    match read {
        // A read that failed outright means the same everywhere: nothing is
        // going to come out of a table we cannot open.
        Err(_) => true,
        Ok(table) => empty_means_withheld && table.is_empty(),
    }
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

/// Parses `arp -a` on Windows, which prints a table instead of a line per
/// entry and separates the octets with dashes.
///
/// ```text
/// Interface: 192.168.1.23 --- 0xa
///   Internet Address      Physical Address      Type
///   192.168.1.1           a4-83-e7-01-02-03     dynamic
/// ```
fn read_arp_windows() -> Result<HashMap<IpAddr, String>, String> {
    let out = crate::sys::quietly(&mut std::process::Command::new("arp"))
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

    fn table(addr: &str, mac: &str) -> HashMap<IpAddr, String> {
        HashMap::from([(addr.parse::<IpAddr>().expect("an address"), mac.to_string())])
    }

    #[test]
    fn a_neighbour_table_read_that_failed_still_puts_the_next_one_off() {
        let known = table("10.0.0.1", "a4:83:e7:01:02:03");
        let mut c = Cache { table: known.clone(), read: Instant::now() };
        std::thread::sleep(Duration::from_millis(5));
        assert!(c.stale(Duration::from_millis(1)), "the table has aged past its lifetime");

        c.adopt(Err("read neighbour table: No such file or directory".into()));

        // Otherwise the probe loop, which polls every forty milliseconds and
        // runs once per host in a sweep, reads the table again on every poll
        // for the whole scan, on exactly the systems where reading it cannot
        // work.
        assert!(!c.stale(Duration::from_millis(500)), "a read that failed was still a read");
        // And a read that said nothing says nothing about what was known.
        assert_eq!(c.table, known);
    }

    #[test]
    fn a_neighbour_table_read_that_worked_replaces_what_was_there() {
        let mut c = Cache { table: table("10.0.0.1", "a4:83:e7:01:02:03"), read: Instant::now() };
        let now = table("10.0.0.2", "b8:27:eb:04:05:06");

        c.adopt(Ok(now.clone()));

        assert_eq!(c.table, now);
        assert!(!c.stale(Duration::from_millis(500)));
    }

    #[test]
    fn a_cold_neighbour_table_is_not_a_system_withholding_it() {
        // Linux and Windows: an empty table at the start of a scan is a cache
        // nothing has been resolved into yet, and the nudge fills it.
        assert!(!table_withheld(&Ok(HashMap::new()), false));
        // macOS answers the read and says nothing however busy the link is.
        assert!(table_withheld(&Ok(HashMap::new()), true));
        // A table that cannot be opened will not answer anywhere.
        assert!(table_withheld(&Err("read neighbour table: not found".into()), false));
        // And a table with entries in it is answering, whatever the platform.
        assert!(!table_withheld(&Ok(table("10.0.0.1", "a4:83:e7:01:02:03")), true));
    }

    /// A prober that got no raw socket, as every Linux and Windows one does,
    /// holding the given verdict about the neighbour table.
    fn neighbour_prober(withheld: bool, table_error: Option<&str>) -> ArpProber {
        ArpProber {
            #[cfg(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            link: None,
            raw_error: None,
            withheld,
            table_error: table_error.map(str::to_string),
            cache: Mutex::new(Cache { table: HashMap::new(), read: Instant::now() }),
            refreshing: tokio::sync::Mutex::new(()),
            baseline: HashMap::new(),
            ttl: Duration::from_millis(250),
            src: None,
        }
    }

    #[test]
    fn a_table_that_cannot_be_read_says_why_rather_than_naming_a_group_the_system_has_not_got() {
        let notes = neighbour_prober(true, Some("read neighbour table: No such file")).notes();

        assert_eq!(notes.len(), 1, "one caveat, not two: {notes:?}");
        assert!(notes[0].contains("No such file"), "the reason it will not answer: {}", notes[0]);
        assert!(!notes[0].contains("access_bpf"), "no such group off macOS: {}", notes[0]);
        assert!(!notes[0].contains("ChmodBPF"), "a macOS installer: {}", notes[0]);
    }

    #[test]
    fn a_table_the_kernel_answers_and_leaves_empty_is_the_one_worth_asking_for_privilege_over() {
        let notes = neighbour_prober(true, None).notes();

        assert_eq!(notes.len(), 1, "one caveat: {notes:?}");
        assert!(notes[0].contains("access_bpf"), "{}", notes[0]);
    }

    #[test]
    fn a_table_that_will_answer_is_no_caveat_at_all() {
        assert!(neighbour_prober(false, None).notes().is_empty());
    }

    #[test]
    fn the_nudge_leaves_from_the_address_the_scan_was_told_to_send_from() {
        // Unbound, the routing table chooses, and on a machine multi-homed
        // onto overlapping networks its choice is not the interface the user
        // picked: the resolution goes out on the wrong link.
        let sock = nudge_socket(Some(Ipv4Addr::LOCALHOST)).expect("a socket");
        assert_eq!(sock.local_addr().expect("its address").ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));

        // "auto" still means whatever the routing table prefers.
        let any = nudge_socket(None).expect("a socket");
        assert_eq!(any.local_addr().expect("its address").ip(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));

        // An address this host does not hold cannot be bound, and the caller
        // falls back to sending unbound rather than not asking at all.
        assert!(nudge_socket(Some(Ipv4Addr::new(192, 0, 2, 1))).is_none());
    }
}
