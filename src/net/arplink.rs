//! Real ARP, over a raw link-layer socket.
//!
//! Recent macOS withholds the neighbour table and every hardware address the
//! ordinary APIs would report: `arp -an` comes back empty and `getifaddrs`
//! hands out `02:00:00:00:00:00`. None of that applies on the wire: an ARP
//! request broadcast over BPF is answered by the host itself, with its real
//! address in the reply.
//!
//! BPF needs either root or membership of `access_bpf`, which installing
//! Wireshark's ChmodBPF grants. When neither is available the
//! caller falls back to reading the neighbour table.

#![cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]

use std::collections::HashMap;
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

// The BPF ioctls, spelled out and not pulled from a binding crate: there
// are five of them and they have not changed in thirty years.
const BIOCSBLEN: libc::c_ulong = 0xc004_4266;
const BIOCSETF: libc::c_ulong = 0x8010_4267;
const BIOCSETIF: libc::c_ulong = 0x8020_426c;
const BIOCIMMEDIATE: libc::c_ulong = 0x8004_4270;
const BIOCSHDRCMPLT: libc::c_ulong = 0x8004_4275;
const BIOCGBLEN: libc::c_ulong = 0x4004_4266;
const BIOCSRTIMEOUT: libc::c_ulong = 0x8010_426d;
const BIOCSSEESENT: libc::c_ulong = 0x8004_4277;

const ETHERTYPE_ARP: u16 = 0x0806;
const ARP_REQUEST: u16 = 1;
const ARP_REPLY: u16 = 2;
/// Ethernet header plus a full ARP payload.
const ARP_FRAME: usize = 14 + 28;
/// The shortest frame Ethernet will carry.
const MIN_FRAME: usize = 60;
/// How often a request is repeated while an answer is being waited for.
const RETRY: Duration = Duration::from_millis(180);

#[repr(C)]
#[derive(Clone, Copy)]
struct BpfInsn {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct BpfProgram {
    len: libc::c_uint,
    insns: *const BpfInsn,
}

/// Everything that is not an ARP frame is dropped in the kernel, so a busy
/// link cannot fill the buffer with traffic we do not want.
const ARP_FILTER: [BpfInsn; 4] = [
    // ldh [12]: the EtherType
    BpfInsn { code: 0x28, jt: 0, jf: 0, k: 12 },
    // jeq #0x0806: ARP?
    BpfInsn { code: 0x15, jt: 0, jf: 1, k: ETHERTYPE_ARP as u32 },
    // ret #262144: take the whole frame
    BpfInsn { code: 0x06, jt: 0, jf: 0, k: 262_144 },
    // ret #0: drop
    BpfInsn { code: 0x06, jt: 0, jf: 0, k: 0 },
];

/// The BPF header in front of every captured frame. `hdrlen` is read back
/// and not assumed, since the padding differs between platforms.
#[repr(C)]
#[derive(Clone, Copy)]
struct BpfHdr {
    tv_sec: i32,
    tv_usec: i32,
    caplen: u32,
    datalen: u32,
    hdrlen: u16,
}

/// One resolved hardware address, and whether the wire said so just now.
#[derive(Clone, Copy, Debug)]
pub struct Answer {
    pub mac: [u8; 6],
    pub rtt: Duration,
    /// False for an address remembered from an earlier run and not
    /// answered for now. It is still the right address as far as anything
    /// knows; it is just not proof the host is still there.
    pub fresh: bool,
}

struct Waiter {
    /// Which waiter this is, so a caller that gives up takes its own
    /// registration away and not everybody else's.
    id: u64,
    tx: oneshot::Sender<[u8; 6]>,
}

static NEXT_WAITER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Which link an address was heard on.
///
/// An address on its own does not identify a host: 192.168.1.1 is the router
/// here and a different router in the next building along, and the handful of
/// prefixes consumer routers hand out are the same everywhere. The interface
/// name alone will not separate them either, because joining another Wi-Fi
/// network keeps `en0`. What does separate them is the address this machine
/// holds on the link, which came from that network's own DHCP server, together
/// with the interface it came in on, which separates two networks attached at
/// the same time.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct LinkId {
    interface: String,
    src_ip: Ipv4Addr,
}

/// Every hardware address this process has heard on the wire, and when, kept
/// per link.
///
/// A BPF handle lives as long as the run that opened it, so without this every
/// scan starts knowing nothing and has to ask the link again for every host,
/// and a host that answers slowly, which over Wi-Fi is most of them, is missed
/// as often as not. Remembering what has already been established makes the
/// second scan of a network at least as complete as the first.
///
/// Per link, and not per address, because it used to be per address: then
/// unplugging at home and joining a network handing out the same prefix
/// reported the old gateway's hardware address, and its vendor, against the
/// new one, and an address simply vacant on the new network came back as a
/// host that was there. Two interfaces on overlapping networks at once had the
/// same problem without any unplugging at all. Time alone cannot tell those
/// apart: the entry is not old, it is about somewhere else.
static REMEMBERED: Mutex<Option<HashMap<LinkId, HashMap<Ipv4Addr, ([u8; 6], Instant)>>>> =
    Mutex::new(None);

/// How long a remembered address is still worth offering. Long enough to cover
/// a session of scanning the same network, short enough that a device swapped
/// out this morning is not still being reported this afternoon.
const REMEMBER_FOR: Duration = Duration::from_secs(600);

fn remember(link: &LinkId, ip: Ipv4Addr, mac: [u8; 6]) {
    let mut held = REMEMBERED.lock().unwrap();
    let table = held.get_or_insert_with(HashMap::new);
    table.entry(link.clone()).or_default().insert(ip, (mac, Instant::now()));
}

/// What this link last heard from this address, if it was recent enough to
/// repeat.
fn remembered(link: &LinkId, ip: Ipv4Addr) -> Option<[u8; 6]> {
    let mut held = REMEMBERED.lock().unwrap();
    let table = held.as_mut()?;
    for seen in table.values_mut() {
        seen.retain(|_, (_, at)| at.elapsed() < REMEMBER_FOR);
    }
    // A link nobody is on any more, whose addresses have all expired, is not
    // worth a row of its own.
    table.retain(|_, seen| !seen.is_empty());
    table.get(link)?.get(&ip).map(|(mac, _)| *mac)
}

struct Inner {
    fd: RawFd,
    src_mac: [u8; 6],
    src_ip: Ipv4Addr,
    /// Which link this is, so what the reader overhears is filed under the
    /// network it was overheard on and not offered to a different one.
    link: LinkId,
    /// Every sender seen on the wire, not just the ones we asked about. A
    /// sweep provokes a great deal of ARP traffic and listening to all of it
    /// means most hosts are already known by the time we ask.
    learned: Mutex<HashMap<Ipv4Addr, [u8; 6]>>,
    waiters: Mutex<HashMap<Ipv4Addr, Vec<Waiter>>>,
    closed: AtomicBool,
}

/// Sends ARP requests and matches the replies, over one shared BPF handle.
pub struct ArpLink {
    inner: Arc<Inner>,
    /// Kept so the descriptor outlives the reader.
    _owned: Arc<OwnedFd>,
    pub interface: String,
}

impl ArpLink {
    /// Opens a link-layer handle on the interface serving `src_ip`.
    pub fn open(interface: &str, src_ip: Ipv4Addr) -> io::Result<ArpLink> {
        let src_mac = real_mac(interface).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("no hardware address for {interface}"))
        })?;

        let fd = open_bpf(interface)?;
        let owned = Arc::new(unsafe { OwnedFd::from_raw_fd(fd) });

        let inner = Arc::new(Inner {
            fd,
            src_mac,
            src_ip,
            link: LinkId { interface: interface.to_string(), src_ip },
            learned: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        });

        // A BPF device cannot be registered with kqueue for readiness, so the
        // reader is a plain thread on a blocking descriptor. The read timeout
        // is what lets it notice that the link has been dropped. There is at
        // most one of these per scan.
        spawn_reader(Arc::downgrade(&inner), Arc::clone(&owned), buffer_len(fd));

        Ok(ArpLink { inner, _owned: owned, interface: interface.to_string() })
    }

    pub fn src_mac(&self) -> [u8; 6] {
        self.inner.src_mac
    }

    /// Asks `dst` for its hardware address and waits for the answer.
    ///
    /// A host that has already answered somebody else's request is returned
    /// straight from what we overheard, which is why a subnet sweep gets
    /// steadily faster as it goes.
    pub async fn resolve(&self, dst: Ipv4Addr, timeout: Duration) -> Option<Answer> {
        let start = Instant::now();
        if let Some(mac) = self.inner.learned.lock().unwrap().get(&dst).copied() {
            return Some(Answer { mac, rtt: Duration::ZERO, fresh: true });
        }

        let id = NEXT_WAITER.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.waiters.lock().unwrap().entry(dst).or_default().push(Waiter { id, tx });

        // The request is repeated for as long as we are waiting, not
        // once or twice: a broadcast that nothing answered may simply have
        // been lost, and a reply over Wi-Fi can take a couple of hundred
        // milliseconds to arrive.
        let asking = async {
            loop {
                let _ = self.send_request(dst);
                tokio::time::sleep(RETRY).await;
            }
        };
        let waiting = async {
            tokio::select! {
                got = rx => got.ok(),
                // Never finishes; it is here to run alongside the wait.
                _ = asking => None,
            }
        };

        if let Ok(Some(mac)) = tokio::time::timeout(timeout, waiting).await {
            remember(&self.inner.link, dst, mac);
            return Some(Answer { mac, rtt: start.elapsed(), fresh: true });
        }

        self.forget_waiter(dst, id);
        if let Some(mac) = self.inner.learned.lock().unwrap().get(&dst).copied() {
            remember(&self.inner.link, dst, mac);
            return Some(Answer { mac, rtt: start.elapsed(), fresh: true });
        }
        // Nothing answered now. What this address was last seen to be on this
        // link is better than an empty column, as long as it is offered as
        // history instead of as an observation. What another link heard is not
        // history about this one, so it is not asked for.
        remembered(&self.inner.link, dst)
            .map(|mac| Answer { mac, rtt: start.elapsed(), fresh: false })
    }

    /// Takes one caller's registration away, leaving anybody else waiting on
    /// the same address still waiting.
    fn forget_waiter(&self, dst: Ipv4Addr, id: u64) {
        let mut waiters = self.inner.waiters.lock().unwrap();
        if let Some(list) = waiters.get_mut(&dst) {
            list.retain(|w| w.id != id);
            if list.is_empty() {
                waiters.remove(&dst);
            }
        }
    }

    fn send_request(&self, dst: Ipv4Addr) -> io::Result<()> {
        let mut frame = [0u8; MIN_FRAME];
        // Ethernet: broadcast, from us, ARP.
        frame[0..6].copy_from_slice(&[0xff; 6]);
        frame[6..12].copy_from_slice(&self.inner.src_mac);
        frame[12..14].copy_from_slice(&ETHERTYPE_ARP.to_be_bytes());
        // ARP: Ethernet over IPv4, a request from us for them.
        frame[14..16].copy_from_slice(&1u16.to_be_bytes());
        frame[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[18] = 6;
        frame[19] = 4;
        frame[20..22].copy_from_slice(&ARP_REQUEST.to_be_bytes());
        frame[22..28].copy_from_slice(&self.inner.src_mac);
        frame[28..32].copy_from_slice(&self.inner.src_ip.octets());
        frame[32..38].copy_from_slice(&[0u8; 6]);
        frame[38..42].copy_from_slice(&dst.octets());

        let n = unsafe {
            libc::write(self.inner.fd, frame.as_ptr() as *const libc::c_void, frame.len())
        };
        if n < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
}

impl Drop for ArpLink {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::Relaxed);
    }
}

fn open_bpf(interface: &str) -> io::Result<RawFd> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no BPF device available");
    for i in 0..256 {
        let path = format!("/dev/bpf{i}\0");
        let fd = unsafe { libc::open(path.as_ptr() as *const libc::c_char, libc::O_RDWR) };
        if fd < 0 {
            last = io::Error::last_os_error();
            // EBUSY simply means somebody else has that one.
            continue;
        }

        if let Err(e) = configure(fd, interface) {
            unsafe { libc::close(fd) };
            return Err(e);
        }
        return Ok(fd);
    }
    Err(last)
}

fn configure(fd: RawFd, interface: &str) -> io::Result<()> {
    unsafe {
        // A bigger buffer, set before the interface is bound because that is
        // when it is allocated.
        let size: libc::c_uint = 32 * 1024;
        libc::ioctl(fd, BIOCSBLEN, &size);

        let mut ifreq: [u8; 32] = [0; 32];
        let name = interface.as_bytes();
        if name.len() >= 16 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "interface name too long"));
        }
        ifreq[..name.len()].copy_from_slice(name);
        if libc::ioctl(fd, BIOCSETIF, ifreq.as_ptr()) < 0 {
            return Err(io::Error::last_os_error());
        }

        // Hand packets over as they arrive, not when the buffer fills.
        let on: libc::c_uint = 1;
        if libc::ioctl(fd, BIOCIMMEDIATE, &on) < 0 {
            return Err(io::Error::last_os_error());
        }
        // We write complete link-layer headers ourselves.
        if libc::ioctl(fd, BIOCSHDRCMPLT, &on) < 0 {
            return Err(io::Error::last_os_error());
        }

        let program = BpfProgram { len: ARP_FILTER.len() as libc::c_uint, insns: ARP_FILTER.as_ptr() };
        libc::ioctl(fd, BIOCSETF, &program);

        // Do not hand our own frames back to us. A sweep sends one broadcast
        // per address, and capturing all of them fills the buffer with
        // questions instead of answers, and the replies we are waiting
        // for are then dropped, and a scan comes back with no hardware
        // addresses at all.
        let off: libc::c_uint = 0;
        libc::ioctl(fd, BIOCSSEESENT, &off);

        // A read timeout, so a reader blocked on a quiet link still comes back
        // often enough to notice it should stop.
        let timeout = libc::timeval { tv_sec: 0, tv_usec: 200_000 };
        libc::ioctl(fd, BIOCSRTIMEOUT, &timeout);
    }
    Ok(())
}

fn buffer_len(fd: RawFd) -> usize {
    let mut len: libc::c_uint = 0;
    unsafe {
        if libc::ioctl(fd, BIOCGBLEN, &mut len) < 0 {
            return 32 * 1024;
        }
    }
    (len as usize).max(4096)
}

fn spawn_reader(inner: Weak<Inner>, owned: Arc<OwnedFd>, buffer: usize) {
    std::thread::Builder::new()
        .name("ntls-arp".into())
        .spawn(move || {
            let mut buf = vec![0u8; buffer];
            loop {
                let Some(inner) = inner.upgrade() else { return };
                if inner.closed.load(Ordering::Relaxed) {
                    return;
                }

                let n = unsafe {
                    libc::read(
                        owned.as_raw_fd(),
                        buf.as_mut_ptr() as *mut libc::c_void,
                        buf.len(),
                    )
                };
                if n > 0 {
                    dispatch(&inner, &buf[..n as usize]);
                    continue;
                }
                if n == 0 {
                    continue; // the read timed out on a quiet link
                }
                match io::Error::last_os_error().kind() {
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted | io::ErrorKind::TimedOut => {
                        continue;
                    }
                    _ => return,
                }
            }
        })
        .ok();
}

/// Walks one BPF read, which holds as many frames as fitted in the buffer.
fn dispatch(inner: &Inner, data: &[u8]) {
    let mut off = 0usize;
    while off + std::mem::size_of::<BpfHdr>() <= data.len() {
        let hdr: BpfHdr = unsafe { std::ptr::read_unaligned(data[off..].as_ptr() as *const BpfHdr) };
        let (hdrlen, caplen) = (hdr.hdrlen as usize, hdr.caplen as usize);
        if hdrlen == 0 || off + hdrlen + caplen > data.len() {
            return;
        }

        if let Some((ip, mac)) = parse_reply(&data[off + hdrlen..off + hdrlen + caplen]) {
            inner.learned.lock().unwrap().insert(ip, mac);
            remember(&inner.link, ip, mac);
            if let Some(waiters) = inner.waiters.lock().unwrap().remove(&ip) {
                for w in waiters {
                    let _ = w.tx.send(mac);
                }
            }
        }

        // Frames are aligned to a four-byte boundary.
        let step = (hdrlen + caplen).div_ceil(4) * 4;
        if step == 0 {
            return;
        }
        off += step;
    }
}

/// Reads the sender out of an ARP reply, ignoring anything else on the wire.
fn parse_reply(frame: &[u8]) -> Option<(Ipv4Addr, [u8; 6])> {
    if frame.len() < ARP_FRAME || u16::from_be_bytes([frame[12], frame[13]]) != ETHERTYPE_ARP {
        return None;
    }
    // Only Ethernet-over-IPv4 replies carry what we are after.
    if u16::from_be_bytes([frame[14], frame[15]]) != 1
        || u16::from_be_bytes([frame[16], frame[17]]) != 0x0800
        || frame[18] != 6
        || frame[19] != 4
        || u16::from_be_bytes([frame[20], frame[21]]) != ARP_REPLY
    {
        return None;
    }

    let mut mac = [0u8; 6];
    mac.copy_from_slice(&frame[22..28]);
    if mac == [0u8; 6] {
        return None;
    }
    let ip = Ipv4Addr::new(frame[28], frame[29], frame[30], frame[31]);
    Some((ip, mac))
}

/// What each interface was last found to hold, and when it was asked.
static OWN_MACS: Mutex<Option<HashMap<String, (Option<[u8; 6]>, Instant)>>> = Mutex::new(None);

/// How long an answer about this machine's own hardware stands before the
/// system is asked again.
///
/// A burned-in address does not change while the application runs, so this
/// could nearly be forever, but an interface name can end up on different
/// hardware: unplug a USB Ethernet adapter and the next one is `en5` as well.
/// Long enough that drawing never pays for it twice, short enough that a
/// swapped adapter is not reported as the old one for the rest of the session.
const OWN_MAC_FOR: Duration = Duration::from_secs(60);

/// This machine's real hardware address on an interface.
///
/// `getifaddrs` reports `02:00:00:00:00:00` on recent macOS, which is useless
/// as the sender of an ARP request, so the configuration database is asked
/// instead, which still tells the truth.
///
/// The answer is kept, because asking is not cheap. `networksetup` is a
/// separate program, and forking and executing it costs some tens of
/// milliseconds whether it comes back with an address or with an error. The
/// interfaces page reads this machine's hardware address while it is building
/// a frame, and a frame is built on every hover, resize and notification, so
/// asking each time meant a process forked per interface on the thread that
/// draws and a window that stopped answering the mouse for as long as that
/// took.
pub fn real_mac(interface: &str) -> Option<[u8; 6]> {
    if let Some(known) = recently_asked(interface) {
        return known;
    }
    // Deliberately not under the lock: a scan opening a link and a frame being
    // drawn ask about the same interface at the same time, and making one wait
    // on the other's fork is the stall this is here to avoid. Two answers to
    // the same question cost one extra fork and agree with each other.
    let found = ask_the_system(interface);
    OWN_MACS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(interface.to_string(), (found, Instant::now()));
    found
}

/// What an interface was last found to hold, if it was asked recently enough
/// not to ask again.
///
/// Having no address is remembered too. Establishing that a tunnel has no
/// hardware costs the same fork and exec as reading a real port's address, and
/// a Mac carries several tunnels, so forgetting the empty answers would leave
/// most of the cost in place.
fn recently_asked(interface: &str) -> Option<Option<[u8; 6]>> {
    let mut held = OWN_MACS.lock().unwrap();
    let table = held.as_mut()?;
    table.retain(|_, (_, at)| at.elapsed() < OWN_MAC_FOR);
    table.get(interface).map(|(mac, _)| *mac)
}

fn ask_the_system(interface: &str) -> Option<[u8; 6]> {
    #[cfg(target_os = "macos")]
    if let Some(mac) = mac_from_networksetup(interface) {
        return Some(mac);
    }
    mac_from_getifaddrs(interface)
}

#[cfg(target_os = "macos")]
fn mac_from_networksetup(interface: &str) -> Option<[u8; 6]> {
    let out = std::process::Command::new("/usr/sbin/networksetup")
        .args(["-getmacaddress", interface])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let hex: String = text
        .split_whitespace()
        .find(|w| w.matches(':').count() == 5)?
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    parse_hex_mac(&hex)
}

fn mac_from_getifaddrs(interface: &str) -> Option<[u8; 6]> {
    let mac = mac_address::mac_address_by_name(interface).ok().flatten()?;
    let bytes = mac.bytes();
    (bytes != [0u8; 6] && bytes[1..] != [0u8; 5]).then_some(bytes)
}

fn parse_hex_mac(hex: &str) -> Option<[u8; 6]> {
    if hex.len() != 12 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, b) in mac.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    (mac != [0u8; 6]).then_some(mac)
}

/// Renders a hardware address the way everything else prints one.
pub fn format_mac(mac: [u8; 6]) -> String {
    mac.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply_frame(ip: [u8; 4], mac: [u8; 6]) -> Vec<u8> {
        let mut f = vec![0u8; ARP_FRAME];
        f[12..14].copy_from_slice(&ETHERTYPE_ARP.to_be_bytes());
        f[14..16].copy_from_slice(&1u16.to_be_bytes());
        f[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
        f[18] = 6;
        f[19] = 4;
        f[20..22].copy_from_slice(&ARP_REPLY.to_be_bytes());
        f[22..28].copy_from_slice(&mac);
        f[28..32].copy_from_slice(&ip);
        f
    }

    #[test]
    fn a_reply_yields_its_sender() {
        let mac = [0x84, 0x2f, 0x57, 0x42, 0x58, 0xf4];
        let got = parse_reply(&reply_frame([192, 168, 1, 5], mac)).expect("not parsed");
        assert_eq!(got, (Ipv4Addr::new(192, 168, 1, 5), mac));
    }

    #[test]
    fn requests_and_other_traffic_are_ignored() {
        // A request, not a reply.
        let mut f = reply_frame([192, 168, 1, 5], [1, 2, 3, 4, 5, 6]);
        f[20..22].copy_from_slice(&ARP_REQUEST.to_be_bytes());
        assert!(parse_reply(&f).is_none());

        // Not ARP at all.
        let mut f = reply_frame([192, 168, 1, 5], [1, 2, 3, 4, 5, 6]);
        f[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        assert!(parse_reply(&f).is_none());

        // An all-zero sender is an unresolved entry, not an answer.
        assert!(parse_reply(&reply_frame([192, 168, 1, 5], [0; 6])).is_none());

        assert!(parse_reply(&[0u8; 20]).is_none());
    }

    #[test]
    fn what_the_wire_said_is_remembered_between_runs() {
        // A BPF handle lives as long as one scan, so without this the second
        // scan of a network starts knowing nothing and has to ask again for
        // every host, which left the hardware column half empty.
        let link = LinkId { interface: "en0".into(), src_ip: Ipv4Addr::new(198, 51, 100, 20) };
        let addr = Ipv4Addr::new(198, 51, 100, 7);
        assert_eq!(remembered(&link, addr), None);
        remember(&link, addr, [1, 2, 3, 4, 5, 6]);
        assert_eq!(remembered(&link, addr), Some([1, 2, 3, 4, 5, 6]));

        // And forgotten once it is old enough to be about a different device.
        REMEMBERED
            .lock()
            .unwrap()
            .as_mut()
            .expect("the table")
            .get_mut(&link)
            .expect("what this link heard")
            .insert(
                addr,
                ([1, 2, 3, 4, 5, 6], Instant::now() - REMEMBER_FOR - Duration::from_secs(1)),
            );
        assert_eq!(remembered(&link, addr), None);
    }

    #[test]
    fn what_one_network_said_is_not_offered_to_another() {
        // The same address on two networks is two different hosts, and the
        // prefixes consumer routers hand out are the same everywhere. Keyed on
        // the address alone, unplugging at home and joining a network with the
        // same prefix reported the old gateway's hardware address, and its
        // vendor, against the new one.
        let home = LinkId { interface: "en0".into(), src_ip: Ipv4Addr::new(192, 168, 1, 40) };
        let away = LinkId { interface: "en0".into(), src_ip: Ipv4Addr::new(192, 168, 1, 77) };
        let gateway = Ipv4Addr::new(192, 168, 1, 1);

        remember(&home, gateway, [0xaa; 6]);
        assert_eq!(remembered(&home, gateway), Some([0xaa; 6]));
        // A different link has heard nothing about it, so it says nothing.
        assert_eq!(remembered(&away, gateway), None);

        // And each link keeps its own answer.
        remember(&away, gateway, [0xbb; 6]);
        assert_eq!(remembered(&home, gateway), Some([0xaa; 6]));
        assert_eq!(remembered(&away, gateway), Some([0xbb; 6]));
    }

    #[test]
    fn the_system_is_not_asked_for_our_own_hardware_address_once_per_frame() {
        // The interfaces page reads this machine's hardware address while it
        // is building a frame, and asking means forking and executing
        // networksetup, tens of milliseconds per interface on the thread that
        // draws. Planting an answer the system could not possibly give and
        // seeing it come back is how we know it is asked once and not again.
        let planted = [0x84, 0x2f, 0x57, 0x42, 0x58, 0xf4];
        OWN_MACS
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert("no-such-if0".to_string(), (Some(planted), Instant::now()));
        assert_eq!(real_mac("no-such-if0"), Some(planted));

        // An interface with no address to give has to be remembered as well:
        // coming back empty costs the same fork as coming back with an
        // address, and it is the tunnels that come back empty.
        assert_eq!(real_mac("no-such-if1"), None);
        assert!(
            OWN_MACS.lock().unwrap().as_ref().expect("the table").contains_key("no-such-if1"),
            "an interface with no hardware address was not remembered as having none"
        );

        // And once it is old enough the system is asked again, so a name given
        // to different hardware is not answered for out of date for ever.
        OWN_MACS.lock().unwrap().as_mut().expect("the table").insert(
            "no-such-if0".to_string(),
            (Some(planted), Instant::now() - OWN_MAC_FOR - Duration::from_secs(1)),
        );
        assert_eq!(real_mac("no-such-if0"), None);
    }

    #[test]
    fn macs_render_the_way_everything_else_prints_them() {
        assert_eq!(format_mac([0x84, 0x2f, 0x57, 0x42, 0x58, 0xf4]), "84:2f:57:42:58:f4");
        assert_eq!(parse_hex_mac("842f574258f4"), Some([0x84, 0x2f, 0x57, 0x42, 0x58, 0xf4]));
        assert_eq!(parse_hex_mac("00"), None);
        // The all-zero address is the absence of one.
        assert_eq!(parse_hex_mac("000000000000"), None);
    }
}

