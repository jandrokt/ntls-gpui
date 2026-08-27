//! Real ARP, over a raw link-layer socket.
//!
//! Recent macOS withholds the neighbour table and every hardware address the
//! ordinary APIs would report — `arp -an` comes back empty and `getifaddrs`
//! hands out `02:00:00:00:00:00`. None of that applies on the wire: an ARP
//! request broadcast over BPF is answered by the host itself, with its real
//! address in the reply.
//!
//! BPF needs either root or membership of `access_bpf`, which is what
//! installing Wireshark's ChmodBPF grants. When neither is available the
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

// The BPF ioctls, spelled out rather than pulled from a binding crate: there
// are five of them and they have not changed in thirty years.
const BIOCSBLEN: libc::c_ulong = 0xc004_4266;
const BIOCSETF: libc::c_ulong = 0x8010_4267;
const BIOCSETIF: libc::c_ulong = 0x8020_426c;
const BIOCIMMEDIATE: libc::c_ulong = 0x8004_4270;
const BIOCSHDRCMPLT: libc::c_ulong = 0x8004_4275;
const BIOCGBLEN: libc::c_ulong = 0x4004_4266;
const BIOCSRTIMEOUT: libc::c_ulong = 0x8010_426d;

const ETHERTYPE_ARP: u16 = 0x0806;
const ARP_REQUEST: u16 = 1;
const ARP_REPLY: u16 = 2;
/// Ethernet header plus a full ARP payload.
const ARP_FRAME: usize = 14 + 28;
/// The shortest frame Ethernet will carry.
const MIN_FRAME: usize = 60;

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

/// Everything that is not an ARP frame is dropped in the kernel, which is what
/// keeps a busy link from filling the buffer with traffic we do not want.
const ARP_FILTER: [BpfInsn; 4] = [
    // ldh [12] — the EtherType
    BpfInsn { code: 0x28, jt: 0, jf: 0, k: 12 },
    // jeq #0x0806 — ARP?
    BpfInsn { code: 0x15, jt: 0, jf: 1, k: ETHERTYPE_ARP as u32 },
    // ret #262144 — take the whole frame
    BpfInsn { code: 0x06, jt: 0, jf: 0, k: 262_144 },
    // ret #0 — drop
    BpfInsn { code: 0x06, jt: 0, jf: 0, k: 0 },
];

/// The BPF header in front of every captured frame. `hdrlen` is read back
/// rather than assumed, since the padding differs between platforms.
#[repr(C)]
#[derive(Clone, Copy)]
struct BpfHdr {
    tv_sec: i32,
    tv_usec: i32,
    caplen: u32,
    datalen: u32,
    hdrlen: u16,
}

struct Waiter {
    tx: oneshot::Sender<[u8; 6]>,
}

struct Inner {
    fd: RawFd,
    src_mac: [u8; 6],
    src_ip: Ipv4Addr,
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
    pub async fn resolve(&self, dst: Ipv4Addr, timeout: Duration) -> Option<([u8; 6], Duration)> {
        let start = Instant::now();
        if let Some(mac) = self.inner.learned.lock().unwrap().get(&dst).copied() {
            return Some((mac, Duration::ZERO));
        }

        let (tx, rx) = oneshot::channel();
        self.inner.waiters.lock().unwrap().entry(dst).or_default().push(Waiter { tx });

        // Two requests: the first often arrives while the far end is still
        // waking its own stack up, and a lost broadcast is not worth a whole
        // timeout.
        let _ = self.send_request(dst);
        let half = timeout / 2;

        if let Ok(Ok(mac)) = tokio::time::timeout(half, rx).await {
            return Some((mac, start.elapsed()));
        }
        if let Some(mac) = self.inner.learned.lock().unwrap().get(&dst).copied() {
            return Some((mac, start.elapsed()));
        }

        let (tx, rx) = oneshot::channel();
        self.inner.waiters.lock().unwrap().entry(dst).or_default().push(Waiter { tx });
        let _ = self.send_request(dst);

        match tokio::time::timeout(timeout - half, rx).await {
            Ok(Ok(mac)) => Some((mac, start.elapsed())),
            _ => {
                self.inner.waiters.lock().unwrap().remove(&dst);
                self.inner.learned.lock().unwrap().get(&dst).copied().map(|m| (m, start.elapsed()))
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

        // Hand packets over as they arrive rather than when the buffer fills.
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

/// This machine's real hardware address on an interface.
///
/// `getifaddrs` reports `02:00:00:00:00:00` on recent macOS, which is useless
/// as the sender of an ARP request, so the configuration database is asked
/// instead — it still tells the truth.
pub fn real_mac(interface: &str) -> Option<[u8; 6]> {
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
    fn macs_render_the_way_everything_else_prints_them() {
        assert_eq!(format_mac([0x84, 0x2f, 0x57, 0x42, 0x58, 0xf4]), "84:2f:57:42:58:f4");
        assert_eq!(parse_hex_mac("842f574258f4"), Some([0x84, 0x2f, 0x57, 0x42, 0x58, 0xf4]));
        assert_eq!(parse_hex_mac("00"), None);
        // The all-zero address is the absence of one.
        assert_eq!(parse_hex_mac("000000000000"), None);
    }
}

