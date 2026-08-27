//! An ICMP echo engine that shares one socket across every probe in flight.
//!
//! Any number of tasks may call [`Pinger::ping`] concurrently: each request
//! carries a unique token in its payload and a background reader dispatches
//! replies to the waiting caller, so a full subnet sweep needs only one
//! socket, and a traceroute's thirty hops overlap on it too.
//!
//! It opens unprivileged datagram ICMP sockets when the OS allows them (macOS
//! always, Linux subject to `net.ipv4.ping_group_range`) and falls back to raw
//! sockets, which need root.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
#[cfg(unix)]
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};
#[cfg(unix)]
use tokio::io::unix::AsyncFd;
use tokio::sync::oneshot;

/// The kinds of answer an ICMP probe can draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplyKind {
    /// An echo reply: the target itself answered.
    Echo,
    /// A router reporting that the TTL ran out. It is what makes traceroute
    /// possible.
    TimeExceeded,
    /// A destination-unreachable report.
    Unreachable,
}

/// A response to an echo request.
#[derive(Clone, Copy, Debug)]
pub struct Reply {
    pub kind: ReplyKind,
    /// Whoever answered: the target for an echo reply, an intermediate router
    /// for a time-exceeded.
    pub from: IpAddr,
    pub rtt: Duration,
    pub ttl: u8,
    /// The ICMP code, for unreachable replies.
    pub code: u8,
}

/// Why a probe produced no usable answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PingError {
    Timeout,
    Cancelled,
    Closed,
    NoSocket,
}

impl std::fmt::Display for PingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PingError::Timeout => "no reply",
            PingError::Cancelled => "stopped",
            PingError::Closed => "socket closed",
            PingError::NoSocket => "no ICMP socket for this address family",
        })
    }
}

/// Tags our payloads so replies can be matched to requests even when the
/// kernel rewrites the ICMP identifier, which it does for unprivileged
/// datagram sockets.
const MAGIC: [u8; 4] = *b"ntls";
const PAYLOAD_HEADER: usize = 4 + 4 + 8; // magic + token + timestamp

/// A second handle on the same socket for the reader to wait on.
///
/// On Unix it is registered with the reactor, so the reader sleeps until the
/// socket is readable. Windows has no such registration for a socket the
/// runtime did not create, so there the reader is a blocking thread with a
/// read timeout — the same shape, one thread rather than none.
#[cfg(unix)]
fn reader_handle(conn: &Conn) -> Option<AsyncFd<OwnedFd>> {
    let dup = conn.socket.try_clone().ok()?;
    AsyncFd::new(OwnedFd::from(dup)).ok()
}

#[cfg(not(unix))]
fn reader_handle(conn: &Conn) -> Option<Socket> {
    let dup = conn.socket.try_clone().ok()?;
    dup.set_nonblocking(false).ok()?;
    // Short enough that a dropped pinger stops its reader promptly, long
    // enough that an idle socket costs nothing.
    dup.set_read_timeout(Some(Duration::from_millis(200))).ok()?;
    Some(dup)
}

struct Pending {
    seq: u16,
    sent: Instant,
    tx: Option<oneshot::Sender<Result<Reply, PingError>>>,
}

#[derive(Default)]
struct Waiters {
    by_token: HashMap<u32, Pending>,
    /// The same requests indexed by their 16-bit sequence number. Error
    /// replies quote only the first eight bytes of the original datagram, so
    /// the sequence is all there is left to match on.
    by_seq: HashMap<u16, u32>,
}

impl Waiters {
    fn take_by_token(&mut self, token: u32) -> Option<Pending> {
        let p = self.by_token.remove(&token)?;
        self.by_seq.remove(&p.seq);
        Some(p)
    }

    fn take_by_seq(&mut self, seq: u16) -> Option<Pending> {
        let token = self.by_seq.remove(&seq)?;
        self.by_token.remove(&token)
    }
}

/// One socket for one address family.
struct Conn {
    socket: Socket,
    /// Serialises the TTL setting with the write that depends on it, since the
    /// TTL is a property of the socket rather than the packet.
    send: Mutex<u8>,
    raw: bool,
    v6: bool,
}

impl Conn {
    fn open(v6: bool, bind: Option<IpAddr>) -> io::Result<Conn> {
        let domain = if v6 { Domain::IPV6 } else { Domain::IPV4 };
        let proto = if v6 { Protocol::ICMPV6 } else { Protocol::ICMPV4 };

        // Try the unprivileged datagram socket first; only fall back to raw,
        // which needs root, when the kernel refuses. Windows has no datagram
        // ICMP socket at all, so there it is raw or nothing — and raw needs
        // Administrator, which is what the caller reports when this fails.
        let (socket, raw) = if cfg!(windows) {
            (Socket::new(domain, Type::RAW, Some(proto))?, true)
        } else {
            match Socket::new(domain, Type::DGRAM, Some(proto)) {
                Ok(s) => (s, false),
                Err(_) => (Socket::new(domain, Type::RAW, Some(proto))?, true),
            }
        };

        // The reader waits on the socket where the runtime can register it and
        // reads it with a timeout where it cannot.
        socket.set_nonblocking(cfg!(unix))?;
        let any: IpAddr = if v6 {
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        };
        let bind_to = match (bind, v6) {
            (Some(b @ IpAddr::V4(_)), false) => b,
            (Some(b @ IpAddr::V6(_)), true) => b,
            _ => any,
        };
        socket.bind(&SocketAddr::new(bind_to, 0).into())?;

        // Ask for the TTL alongside each datagram. Best effort: if the
        // platform will not report it we leave the column blank rather than
        // failing the ping — which is what happens on Windows, where a
        // datagram socket does not carry it.
        #[cfg(unix)]
        unsafe {
            let on: libc::c_int = 1;
            let (level, opt) = if v6 {
                (libc::IPPROTO_IPV6, libc::IPV6_RECVHOPLIMIT)
            } else {
                (libc::IPPROTO_IP, libc::IP_RECVTTL)
            };
            libc::setsockopt(
                socket.as_raw_fd(),
                level,
                opt,
                &on as *const _ as *const libc::c_void,
                std::mem::size_of_val(&on) as libc::socklen_t,
            );
        }

        Ok(Conn { socket, send: Mutex::new(0), raw, v6 })
    }

    fn set_ttl(&self, ttl: u8) -> io::Result<()> {
        if self.v6 {
            self.socket.set_unicast_hops_v6(ttl as u32)
        } else {
            self.socket.set_ttl_v4(ttl as u32)
        }
    }
}

/// Sends ICMP echo requests over a shared socket per address family.
pub struct Pinger {
    inner: Arc<Inner>,
    reader_v4: Option<tokio::task::JoinHandle<()>>,
    reader_v6: Option<tokio::task::JoinHandle<()>>,
}

struct Inner {
    waiters: Mutex<Waiters>,
    token: AtomicU32,
    seq: AtomicU32,
    v4: Option<Conn>,
    v6: Option<Conn>,
    closed: AtomicBool,
    /// Reports whether raw sockets were needed. Surfaced in the UI so an
    /// unexpected permission error is self-explanatory.
    pub privileged: bool,
}

impl Pinger {
    /// Opens the sockets needed for the given families. `src`, when set, binds
    /// the sockets to one interface's address.
    pub fn new(need_v4: bool, need_v6: bool, src: Option<Ipv4Addr>) -> Result<Pinger, String> {
        let bind = src.map(IpAddr::V4);
        let mut first_err: Option<io::Error> = None;

        let v4 = if need_v4 {
            match Conn::open(false, bind) {
                Ok(c) => Some(c),
                Err(e) => {
                    first_err = Some(e);
                    None
                }
            }
        } else {
            None
        };
        let v6 = if need_v6 {
            match Conn::open(true, None) {
                Ok(c) => Some(c),
                Err(e) => {
                    first_err.get_or_insert(e);
                    None
                }
            }
        } else {
            None
        };

        if v4.is_none() && v6.is_none() {
            let e = first_err.map(|e| e.to_string()).unwrap_or_else(|| "no family requested".into());
            return Err(format!("cannot open an ICMP socket: {e} (try running with sudo)"));
        }

        let privileged = v4.as_ref().is_some_and(|c| c.raw) || v6.as_ref().is_some_and(|c| c.raw);
        let inner = Arc::new(Inner {
            waiters: Mutex::new(Waiters::default()),
            token: AtomicU32::new(rand::random()),
            seq: AtomicU32::new(rand::random()),
            v4,
            v6,
            closed: AtomicBool::new(false),
            privileged,
        });

        let reader_v4 = inner
            .v4
            .as_ref()
            .and_then(reader_handle)
            .map(|fd| spawn_reader(Arc::downgrade(&inner), false, fd));
        let reader_v6 = inner
            .v6
            .as_ref()
            .and_then(reader_handle)
            .map(|fd| spawn_reader(Arc::downgrade(&inner), true, fd));

        Ok(Pinger { inner, reader_v4, reader_v6 })
    }

    pub fn privileged(&self) -> bool {
        self.inner.privileged
    }

    /// Sends one echo request and waits for the matching reply. `payload` is
    /// the number of data bytes to append beyond the header ntls uses for
    /// matching.
    pub async fn ping(&self, dst: IpAddr, payload: usize, timeout: Duration) -> Result<Reply, PingError> {
        self.ping_ttl(dst, 0, payload, timeout).await
    }

    /// Sends an echo request with an explicit IP TTL. A `ttl` of 0 leaves the
    /// system default in place. A small TTL makes routers along the path
    /// answer with time-exceeded instead, which is how the route is
    /// discovered.
    pub async fn ping_ttl(
        &self,
        dst: IpAddr,
        ttl: u8,
        payload: usize,
        timeout: Duration,
    ) -> Result<Reply, PingError> {
        let conn = match dst {
            IpAddr::V4(_) => self.inner.v4.as_ref(),
            IpAddr::V6(_) => self.inner.v6.as_ref(),
        }
        .ok_or(PingError::NoSocket)?;

        let token = self.inner.token.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let seq = self.inner.seq.fetch_add(1, Ordering::Relaxed) as u16;
        let sent = Instant::now();
        let (tx, rx) = oneshot::channel();

        {
            let mut w = self.inner.waiters.lock().unwrap();
            w.by_token.insert(token, Pending { seq, sent, tx: Some(tx) });
            w.by_seq.insert(seq, token);
        }

        let result = match send(conn, dst, token, seq, ttl, payload, sent) {
            Ok(()) => match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(r)) => r,
                Ok(Err(_)) => Err(PingError::Closed),
                Err(_) => Err(PingError::Timeout),
            },
            Err(_) => Err(PingError::Closed),
        };

        let mut w = self.inner.waiters.lock().unwrap();
        w.take_by_token(token);
        result
    }
}

impl Drop for Pinger {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        if let Some(h) = self.reader_v4.take() {
            h.abort();
        }
        if let Some(h) = self.reader_v6.take() {
            h.abort();
        }
        let mut w = self.inner.waiters.lock().unwrap();
        let tokens: Vec<u32> = w.by_token.keys().copied().collect();
        for t in tokens {
            if let Some(mut p) = w.take_by_token(t)
                && let Some(tx) = p.tx.take()
            {
                let _ = tx.send(Err(PingError::Closed));
            }
        }
    }
}

fn send(
    conn: &Conn,
    dst: IpAddr,
    token: u32,
    seq: u16,
    ttl: u8,
    payload: usize,
    sent: Instant,
) -> io::Result<()> {
    let mut body = vec![0u8; PAYLOAD_HEADER + payload.min(65_000)];
    body[0..4].copy_from_slice(&MAGIC);
    body[4..8].copy_from_slice(&token.to_be_bytes());
    // The remaining header bytes are filler: the round trip is measured from
    // the Instant held by the waiter, not from anything on the wire.
    let _ = sent;
    for (i, b) in body.iter_mut().enumerate().skip(PAYLOAD_HEADER) {
        *b = i as u8;
    }

    // type(1) code(1) checksum(2) id(2) seq(2) then the payload.
    let echo_type: u8 = if dst.is_ipv4() { 8 } else { 128 };
    let mut packet = Vec::with_capacity(8 + body.len());
    packet.extend_from_slice(&[echo_type, 0, 0, 0]);
    packet.extend_from_slice(&(token as u16).to_be_bytes());
    packet.extend_from_slice(&seq.to_be_bytes());
    packet.extend_from_slice(&body);

    // IPv6 checksums cover a pseudo-header the kernel fills in for us.
    if dst.is_ipv4() {
        let sum = checksum(&packet);
        packet[2..4].copy_from_slice(&sum.to_be_bytes());
    }

    let mut current = conn.send.lock().unwrap();
    if ttl > 0 && ttl != *current {
        conn.set_ttl(ttl)?;
        *current = ttl;
    }
    conn.socket.send_to(&packet, &SocketAddr::new(dst, 0).into())?;
    Ok(())
}

fn checksum(b: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = b.chunks_exact(2);
    for c in &mut chunks {
        sum += u16::from_be_bytes([c[0], c[1]]) as u32;
    }
    if let [last] = chunks.remainder() {
        sum += (*last as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Pumps one socket, matching replies to waiters until the pinger is dropped.
#[cfg(unix)]
fn spawn_reader(inner: Weak<Inner>, v6: bool, fd: AsyncFd<OwnedFd>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            let Ok(mut guard) = fd.readable().await else { return };

            let Some(inner) = inner.upgrade() else { return };
            if inner.closed.load(Ordering::SeqCst) {
                return;
            }
            // The socket this reader belongs to must still be there; if the
            // pinger dropped its half, so does this task.
            if (if v6 { inner.v6.as_ref() } else { inner.v4.as_ref() }).is_none() {
                return;
            }
            match recv_with_ttl(guard.get_inner().as_raw_fd(), &mut buf) {
                Ok(Some((n, from, ttl))) => dispatch(&inner, &buf[..n], from, ttl),
                Ok(None) => guard.clear_ready(),
                Err(_) => return,
            }
        }
    })
}

/// The same, where the socket cannot be registered with the runtime.
///
/// One blocking thread per socket, woken by its own read timeout, which is
/// what lets the reply dispatch above stay exactly the same.
#[cfg(not(unix))]
fn spawn_reader(inner: Weak<Inner>, v6: bool, socket: Socket) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 2048];
        loop {
            let Some(alive) = inner.upgrade() else { return };
            if alive.closed.load(Ordering::SeqCst) {
                return;
            }
            if (if v6 { alive.v6.as_ref() } else { alive.v4.as_ref() }).is_none() {
                return;
            }
            drop(alive);

            let read = {
                // Reading into uninitialised memory is what socket2 asks for;
                // only the bytes it reports are ever looked at.
                let raw = unsafe {
                    &mut *(&mut buf[..] as *mut [u8] as *mut [std::mem::MaybeUninit<u8>])
                };
                socket.recv_from(raw)
            };
            match read {
                Ok((n, from)) => {
                    let Some(inner) = inner.upgrade() else { return };
                    let from = from.as_socket().map(|s| s.ip()).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
                    // No ancillary data here, so no TTL: the column is left
                    // blank rather than filled in with a guess.
                    dispatch(&inner, &buf[..n], from, 0);
                }
                Err(e) if e.kind() == io::ErrorKind::TimedOut => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            }
        }
    })
}

/// Reads one datagram along with the TTL of the packet that carried it.
///
/// `recvmsg` is what makes the TTL reachable: it arrives as ancillary data
/// rather than in the payload, and on a datagram ICMP socket there is no IP
/// header to read it out of.
#[cfg(unix)]
fn recv_with_ttl(fd: RawFd, buf: &mut [u8]) -> io::Result<Option<(usize, IpAddr, u8)>> {
    unsafe {
        let mut addr: libc::sockaddr_storage = std::mem::zeroed();
        let mut iov = libc::iovec { iov_base: buf.as_mut_ptr() as *mut libc::c_void, iov_len: buf.len() };
        let mut control = [0u8; 256];
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_name = &mut addr as *mut _ as *mut libc::c_void;
        msg.msg_namelen = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = control.len() as _;

        let n = libc::recvmsg(fd, &mut msg, 0);
        if n < 0 {
            let err = io::Error::last_os_error();
            return match err.kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Ok(None),
                _ => Err(err),
            };
        }

        let from = sockaddr_to_ip(&addr).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

        let mut ttl = 0u8;
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            let hdr = &*cmsg;
            let is_ttl = (hdr.cmsg_level == libc::IPPROTO_IP && hdr.cmsg_type == libc::IP_RECVTTL)
                || (hdr.cmsg_level == libc::IPPROTO_IPV6 && hdr.cmsg_type == libc::IPV6_HOPLIMIT);
            if is_ttl {
                ttl = *(libc::CMSG_DATA(cmsg) as *const u8);
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }

        Ok(Some((n as usize, from, ttl)))
    }
}

#[cfg(unix)]
unsafe fn sockaddr_to_ip(storage: &libc::sockaddr_storage) -> Option<IpAddr> {
    unsafe {
        match storage.ss_family as libc::c_int {
            libc::AF_INET => {
                let sin = &*(storage as *const _ as *const libc::sockaddr_in);
                Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))))
            }
            libc::AF_INET6 => {
                let sin6 = &*(storage as *const _ as *const libc::sockaddr_in6);
                Some(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr)))
            }
            _ => None,
        }
    }
}

/// Resolves one received ICMP message back to whoever is waiting for it.
fn dispatch(inner: &Inner, packet: &[u8], from: IpAddr, ttl: u8) {
    let Some(msg) = icmp_body(packet) else { return };

    let (icmp_type, code) = (msg[0], msg[1]);
    let echo_reply = if from.is_ipv4() { 0 } else { 129 };
    let time_exceeded = if from.is_ipv4() { 11 } else { 3 };
    let unreachable = if from.is_ipv4() { 3 } else { 1 };

    if icmp_type == echo_reply {
        let Some(token) = parse_token(&msg[8..]) else { return };
        let Some(mut p) = inner.waiters.lock().unwrap().take_by_token(token) else { return };
        if let Some(tx) = p.tx.take() {
            let _ = tx.send(Ok(Reply {
                kind: ReplyKind::Echo,
                from,
                rtt: p.sent.elapsed(),
                ttl,
                code: 0,
            }));
        }
        return;
    }

    let kind = if icmp_type == time_exceeded {
        ReplyKind::TimeExceeded
    } else if icmp_type == unreachable {
        ReplyKind::Unreachable
    } else {
        return;
    };

    // The quoted packet holds only the original IP header plus the first eight
    // bytes of the datagram, which is the ICMP header and no payload. The
    // token is therefore out of reach and the sequence number is what
    // identifies the request.
    let Some(seq) = quoted_seq(&msg[8..]) else { return };
    let Some(mut p) = inner.waiters.lock().unwrap().take_by_seq(seq) else { return };
    if let Some(tx) = p.tx.take() {
        let _ = tx.send(Ok(Reply { kind, from, rtt: p.sent.elapsed(), ttl, code }));
    }
}

/// Finds the ICMP message inside what the socket handed back.
///
/// Whether the IP header comes with it is not something the caller gets to
/// know: macOS prepends it even on an unprivileged datagram socket, Linux does
/// not, and a raw socket does on both. An IPv4 header always starts with a
/// version nibble of 4, and no ICMP type reaches 0x40, so the first byte says
/// which one this is.
fn icmp_body(packet: &[u8]) -> Option<&[u8]> {
    let first = *packet.first()?;
    let body = if first >> 4 == 4 {
        let ihl = ((first & 0x0f) as usize) * 4;
        if ihl < 20 {
            return None;
        }
        packet.get(ihl..)?
    } else {
        packet
    };
    (body.len() >= 8).then_some(body)
}

fn parse_token(data: &[u8]) -> Option<u32> {
    if data.len() < PAYLOAD_HEADER || data[0..4] != MAGIC {
        return None;
    }
    Some(u32::from_be_bytes([data[4], data[5], data[6], data[7]]))
}

/// Pulls the ICMP sequence number out of the packet quoted back by an error
/// message, skipping the original IP header.
fn quoted_seq(b: &[u8]) -> Option<u16> {
    if b.len() < 20 {
        return None;
    }
    let offset = match b[0] >> 4 {
        4 => ((b[0] & 0x0f) as usize) * 4,
        6 => 40,
        _ => return None,
    };
    // The quoted ICMP header is type(1) code(1) checksum(2) id(2) seq(2).
    if b.len() < offset + 8 {
        return None;
    }
    Some(u16::from_be_bytes([b[offset + 6], b[offset + 7]]))
}

/// The human name for an ICMP destination-unreachable code.
pub fn unreachable_note(code: u8) -> &'static str {
    match code {
        0 => "network unreachable",
        1 => "host unreachable",
        2 => "protocol unreachable",
        3 => "port unreachable",
        9 | 10 | 13 => "administratively prohibited",
        _ => "unreachable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ip_header_is_recognised_and_skipped() {
        // macOS hands back the IP header even on a datagram socket; Linux
        // does not. Both must reach the same eight-byte ICMP header.
        let mut with_header = vec![0x45, 0x00, 0x00, 0x1c];
        with_header.extend_from_slice(&[0u8; 16]);
        with_header.extend_from_slice(&[0x00, 0x00, 0x7c, 0x33, 0x12, 0x34, 0x00, 0x07]);
        let body = icmp_body(&with_header).expect("header not skipped");
        assert_eq!(body.len(), 8);
        assert_eq!(body[0], 0x00, "should start at the ICMP type");

        let bare = [0x00u8, 0x00, 0x7c, 0x33, 0x12, 0x34, 0x00, 0x07];
        assert_eq!(icmp_body(&bare).unwrap(), &bare);

        assert!(icmp_body(&[0x00, 0x00]).is_none(), "a runt is not a message");
        assert!(icmp_body(&[]).is_none());
    }

    #[test]
    fn a_payload_carries_a_matchable_token() {
        let mut body = Vec::from(MAGIC);
        body.extend_from_slice(&0xdead_beefu32.to_be_bytes());
        body.extend_from_slice(&[0u8; 8]);
        assert_eq!(parse_token(&body), Some(0xdead_beef));

        // Somebody else's ping must not be mistaken for ours.
        assert_eq!(parse_token(&[0u8; 16]), None);
        assert_eq!(parse_token(&body[..4]), None);
    }

    #[test]
    fn an_error_report_is_matched_by_the_sequence_it_quotes() {
        // The quoted packet is the original IP header plus the first eight
        // bytes of the datagram: type, code, checksum, id, sequence.
        let mut quoted = vec![0x45, 0x00, 0x00, 0x54];
        quoted.extend_from_slice(&[0u8; 16]);
        quoted.extend_from_slice(&[0x08, 0x00, 0x00, 0x00, 0xaa, 0xbb, 0x04, 0xd2]);
        assert_eq!(quoted_seq(&quoted), Some(1234));

        assert_eq!(quoted_seq(&[0u8; 8]), None);
    }

    #[test]
    fn the_checksum_matches_a_known_echo_request() {
        // An eight-byte echo request with id 0 and sequence 0 sums to 0xf7ff.
        let packet = [0x08u8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(checksum(&packet), 0xf7ff);
        // A packet with its checksum in place sums to zero, which is how a
        // receiver validates one.
        let mut fixed = packet;
        fixed[2..4].copy_from_slice(&0xf7ffu16.to_be_bytes());
        assert_eq!(checksum(&fixed), 0);
    }

    /// The loopback answers its own pings, which exercises the socket, the
    /// reader task and the token matching together.
    #[test]
    fn the_loopback_answers() {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let Ok(pinger) = Pinger::new(true, false, None) else {
                // No ICMP socket at all is a permission problem, not a bug.
                return;
            };
            let reply = pinger
                .ping("127.0.0.1".parse().unwrap(), 56, Duration::from_secs(3))
                .await
                .expect("the loopback did not answer its own ping");
            assert_eq!(reply.kind, ReplyKind::Echo);
            assert_eq!(reply.from, "127.0.0.1".parse::<IpAddr>().unwrap());
            assert!(reply.ttl > 0, "no TTL came back with the reply");
        });
    }
}
