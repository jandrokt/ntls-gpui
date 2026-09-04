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
use tokio::io::Interest;
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
    /// The request never left the machine: the kernel refused the write, and
    /// this is the reason it gave. It is kept because the reasons call for
    /// quite different things from whoever is reading the table, and because
    /// none of them is the socket having closed.
    Unsent(&'static str),
}

impl std::fmt::Display for PingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PingError::Timeout => "no reply",
            PingError::Cancelled => "stopped",
            PingError::Closed => "socket closed",
            PingError::NoSocket => "no ICMP socket for this address family",
            PingError::Unsent(why) => why,
        })
    }
}

/// Names a write the kernel would not perform.
///
/// Every one of these used to be reported as "socket closed", which is a
/// thing that had not happened and told the reader nothing: a sweep of a
/// subnet this machine has no route to filled all two hundred and fifty-four
/// rows with a closed socket, and the one fact that explained the whole
/// table -- that there is no route out of here -- was thrown away at the
/// point it was learnt. A full send queue is worse than useless there, since
/// it is a fact about our own burst rather than about the address probed, and
/// it reads as if the tool had broken.
fn why_unsent(e: &io::Error) -> PingError {
    // The one a burst of our own probes actually draws on the BSDs has no
    // `ErrorKind` of its own, so it is read off the errno instead of being
    // left to fall through to the vague answer below.
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::ENOBUFS) {
        return PingError::Unsent("send queue full");
    }
    PingError::Unsent(match e.kind() {
        io::ErrorKind::HostUnreachable => "no route to host",
        io::ErrorKind::NetworkUnreachable => "network unreachable",
        io::ErrorKind::NetworkDown => "network is down",
        io::ErrorKind::PermissionDenied => "not allowed to send",
        io::ErrorKind::WouldBlock | io::ErrorKind::OutOfMemory => "send queue full",
        _ => "cannot send",
    })
}

/// Tags our payloads so replies can be matched to requests even when the
/// kernel rewrites the ICMP identifier, which it does for unprivileged
/// datagram sockets.
const MAGIC: [u8; 4] = *b"ntls";
const PAYLOAD_HEADER: usize = 4 + 4 + 8; // magic + token + timestamp

/// Whether this machine will let us speak ICMP at all.
///
/// Opening a socket is the only reliable way to ask, and unlike `Pinger` this
/// needs no runtime, so the tests can decide whether there is anything to
/// test before they build one.
#[cfg(test)]
pub fn available() -> bool {
    Conn::open(false, None).is_ok()
}

/// What to do about a refused ICMP socket, on this system.
///
/// The advice differs enough between the three that one sentence for all of
/// them would be wrong on two.
fn permission_hint() -> &'static str {
    if cfg!(windows) {
        "run ntls as Administrator"
    } else if cfg!(target_os = "linux") {
        // The sysctl is the better answer of the two: it grants exactly this
        // and nothing else, and it does not need the whole program to be root.
        "sudo sysctl -w net.ipv4.ping_group_range=\"0 2147483647\", or run ntls with sudo"
    } else {
        "run ntls with sudo"
    }
}

/// A second handle on the same socket for the reader to wait on.
///
/// On Unix it is registered with the reactor, so the reader sleeps until the
/// socket is readable. Windows has no such registration for a socket the
/// runtime did not create, so there the reader is a blocking thread with a
/// read timeout: the same shape, with one thread instead of none.
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
        // A sequence number is sixteen bits wide, so a long enough run of
        // probes comes back round to one whose earlier owner is still waiting,
        // and the later probe takes the entry over. Removing it on the strength
        // of the sequence alone then cost that later probe the only thing an
        // error report can be matched by: it could still be handed an echo
        // reply, which carries the token, but a time-exceeded or an unreachable
        // quotes the sequence and nothing else, so a traceroute hop that had
        // answered turned into a star.
        if self.by_seq.get(&p.seq) == Some(&token) {
            self.by_seq.remove(&p.seq);
        }
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
    /// TTL is a property of the socket instead of the packet.
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
        // ICMP socket at all, so there it is raw or nothing, and raw needs
        // Administrator. The caller reports that when this fails.
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
        // platform will not report it we leave the column blank and do not
        // failing the ping. That happens on Windows, where a
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

        // Ask Linux for the ICMP errors as well. A datagram ICMP socket there
        // is handed echo replies and nothing else: a time-exceeded or an
        // unreachable report drawn by one of our own probes is never queued as
        // a readable datagram. It goes on the socket's error queue, and only
        // if the socket asked for it. Without this a traceroute printed a star
        // for every hop on any system that allows these sockets, which is
        // every systemd one by default, and a sweep of a subnet whose router
        // says "host unreachable" heard nothing back at all. A raw socket is
        // given the whole ICMP message the ordinary way, so it is left alone.
        #[cfg(target_os = "linux")]
        if !raw {
            unsafe {
                let on: libc::c_int = 1;
                let (level, opt) = if v6 {
                    (libc::IPPROTO_IPV6, libc::IPV6_RECVERR)
                } else {
                    (libc::IPPROTO_IP, libc::IP_RECVERR)
                };
                libc::setsockopt(
                    socket.as_raw_fd(),
                    level,
                    opt,
                    &on as *const _ as *const libc::c_void,
                    std::mem::size_of_val(&on) as libc::socklen_t,
                );
            }
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
            return Err(format!("cannot open an ICMP socket: {e} (try: {})", permission_hint()));
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
            Err(e) => Err(why_unsent(&e)),
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

/// What the reader waits for on its socket.
///
/// An error queue is not readable: putting something on it wakes a poll with
/// an error and never with a read, so a reader that waits only for readability
/// sleeps through every ICMP error Linux reports that way, which is all of
/// them. Nowhere else has such a queue, and there the interest stays as narrow
/// as it was.
#[cfg(target_os = "linux")]
const READER_WANTS: Interest = Interest::READABLE.add(Interest::ERROR);
#[cfg(all(unix, not(target_os = "linux")))]
const READER_WANTS: Interest = Interest::READABLE;

/// Pumps one socket, matching replies to waiters until the pinger is dropped.
#[cfg(unix)]
fn spawn_reader(inner: Weak<Inner>, v6: bool, fd: AsyncFd<OwnedFd>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        let mut failures = 0usize;
        loop {
            let Ok(mut guard) = fd.ready(READER_WANTS).await else { return };

            let Some(inner) = inner.upgrade() else { return };
            if inner.closed.load(Ordering::SeqCst) {
                return;
            }
            // The socket this reader belongs to must still be there; if the
            // pinger dropped its half, so does this task.
            if (if v6 { inner.v6.as_ref() } else { inner.v4.as_ref() }).is_none() {
                return;
            }
            let raw_fd = guard.get_inner().as_raw_fd();
            // The error queue goes first: emptying it is also what clears the
            // pending error the kernel would otherwise hand to the ordinary
            // read below.
            let mut got = drain_error_queue(&inner, raw_fd);
            let mut failed = false;
            match recv_with_ttl(raw_fd, &mut buf) {
                Ok(Some((n, from, ttl))) => {
                    got = true;
                    dispatch(&inner, &buf[..n], from, ttl);
                }
                Ok(None) => {}
                // One failed read means one datagram lost, not a socket to
                // abandon. A socket with an error pending reports it to the
                // next ordinary read even when the error queue already carried
                // the same news, so giving up here left a Linux traceroute
                // deaf from its first hop onwards, with every later probe
                // timing out and nothing anywhere to say why.
                Err(_) => failed = true,
            }
            if got {
                failures = 0;
            } else if failed {
                failures += 1;
                if failures > MAX_READ_FAILURES {
                    return;
                }
            } else {
                guard.clear_ready();
            }
        }
    })
}

/// How many reads may fail in a row before the reader gives up on its socket.
///
/// High enough that no run of ordinary failures ends the listening, low enough
/// that a socket which has genuinely gone does not spin.
const MAX_READ_FAILURES: usize = 64;

/// The same, where the socket cannot be registered with the runtime.
///
/// One blocking thread per socket, woken by its own read timeout, which is
/// what lets the reply dispatch above stay exactly the same.
#[cfg(not(unix))]
fn spawn_reader(inner: Weak<Inner>, v6: bool, socket: Socket) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 2048];
        // How many reads in a row have failed. One failure means nothing: a
        // datagram wider than the buffer, an interface that went away for a
        // moment, a refusal arriving on the socket. Giving up on the first one
        // is what turned a single oversized reply into every later probe
        // timing out, with nothing anywhere to say why.
        let mut failures = 0usize;
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
                    failures = 0;
                    let Some(inner) = inner.upgrade() else { return };
                    let from = from.as_socket().map(|s| s.ip()).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
                    // No ancillary data here, so no TTL: the column is left
                    // blank, not filled in with a guess.
                    dispatch(&inner, &buf[..n], from, 0);
                }
                Err(e) if e.kind() == io::ErrorKind::TimedOut => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                // Windows answers a datagram too big for the buffer with an
                // error rather than a truncated read, and that is one reply
                // to skip, not a reason to stop listening for the rest.
                Err(_) => {
                    failures += 1;
                    if failures > MAX_READ_FAILURES {
                        return;
                    }
                    continue;
                }
            }
        }
    })
}

/// Whether a piece of ancillary data is the hop count we asked for.
///
/// What we ask for and what comes back are not the same name: Linux answers
/// `IP_RECVTTL` with a cmsg tagged `IP_TTL`, while macOS and the BSDs tag it
/// `IP_RECVTTL`. Accepting only one of the two is how the TTL column ends up
/// empty on the other system.
#[cfg(unix)]
fn is_hop_cmsg(level: libc::c_int, kind: libc::c_int) -> bool {
    (level == libc::IPPROTO_IP && (kind == libc::IP_TTL || kind == libc::IP_RECVTTL))
        || (level == libc::IPPROTO_IPV6 && kind == libc::IPV6_HOPLIMIT)
}

/// Reads a hop count out of ancillary data.
///
/// Its width is not the same everywhere either: macOS sends the IPv4 TTL as a
/// single byte, while Linux sends it as an `int`, and the IPv6 hop limit is an
/// `int` on both. Reading the first byte of an `int` happens to work on a
/// little-endian machine and is wrong everywhere else, so read what is there.
#[cfg(unix)]
unsafe fn cmsg_hop(cmsg: *const libc::cmsghdr, len: usize) -> u8 {
    unsafe {
        let data = libc::CMSG_DATA(cmsg);
        let payload = len.saturating_sub(data as usize - cmsg as usize);
        match payload {
            1 => *data,
            n if n >= 4 => {
                let mut value = 0i32;
                std::ptr::copy_nonoverlapping(data, &raw mut value as *mut u8, 4);
                value.clamp(0, u8::MAX as i32) as u8
            }
            _ => 0,
        }
    }
}

/// Reads one datagram along with the TTL of the packet that carried it.
///
/// `recvmsg` is what makes the TTL reachable: it arrives as ancillary data
/// instead of in the payload, and on a datagram ICMP socket there is no IP
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
            if is_hop_cmsg(hdr.cmsg_level, hdr.cmsg_type) {
                ttl = cmsg_hop(cmsg, hdr.cmsg_len as usize);
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

/// Empties the socket's error queue, handing each report to its probe.
///
/// Nothing outside Linux has such a queue: an ICMP error arrives there as an
/// ordinary datagram and is read by the loop above.
#[cfg(all(unix, not(target_os = "linux")))]
fn drain_error_queue(_inner: &Inner, _fd: RawFd) -> bool {
    false
}

/// The same, on the one system that reports ICMP errors out of band.
///
/// Returns whether there was anything on the queue. Every entry is taken, not
/// just the first: several probes overlap on this socket, so several hops can
/// answer between two wakeups, and an entry left behind would sit there until
/// the next one arrived to wake the reader again.
#[cfg(target_os = "linux")]
fn drain_error_queue(inner: &Inner, fd: RawFd) -> bool {
    let mut any = false;
    // Only the first eight bytes are ever read: the quoted ICMP header, which
    // is all a router has to send back and all that is needed to know whose
    // probe this answers. The rest is room for the routers that quote more.
    let mut quoted = [0u8; 64];
    loop {
        unsafe {
            let mut iov = libc::iovec {
                iov_base: quoted.as_mut_ptr() as *mut libc::c_void,
                iov_len: quoted.len(),
            };
            let mut control = [0u8; 256];
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
            msg.msg_controllen = control.len() as _;

            let n = libc::recvmsg(fd, &mut msg, libc::MSG_ERRQUEUE | libc::MSG_DONTWAIT);
            if n < 0 {
                return any;
            }
            any = true;
            let Some(err) = parse_error_cmsg(&msg) else { continue };
            // What the queue hands back begins at the quoted ICMP header: the
            // kernel has already taken the quoted IP header off the front.
            let n = (n as usize).min(quoted.len());
            let Some(seq) = echo_seq(&quoted[..n], err.from.is_ipv6()) else { continue };
            // No hop count. It came off a queue instead of off the wire, so
            // the column stays blank rather than holding a guess.
            deliver_error(inner, err.icmp_type, err.code, err.from, 0, seq);
        }
    }
}

/// What one error-queue entry says: which ICMP error arrived, and from whom.
#[cfg(target_os = "linux")]
struct QueuedError {
    from: IpAddr,
    icmp_type: u8,
    code: u8,
}

/// Reads an error-queue entry's ancillary data.
///
/// The error itself is described there and nowhere else: a `sock_extended_err`
/// carrying the ICMP type and code, followed by the address of the node that
/// sent them. The datagram's own name is the address we were probing, so the
/// router that answered has to come from here or a traceroute would attribute
/// every hop to the target.
#[cfg(target_os = "linux")]
unsafe fn parse_error_cmsg(msg: &libc::msghdr) -> Option<QueuedError> {
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(msg);
        while !cmsg.is_null() {
            let hdr = &*cmsg;
            let is_err = (hdr.cmsg_level == libc::IPPROTO_IP
                && hdr.cmsg_type == libc::IP_RECVERR)
                || (hdr.cmsg_level == libc::IPPROTO_IPV6 && hdr.cmsg_type == libc::IPV6_RECVERR);
            if is_err {
                let data = libc::CMSG_DATA(cmsg);
                let len = (hdr.cmsg_len as usize).saturating_sub(data as usize - cmsg as usize);
                let ee_size = std::mem::size_of::<libc::sock_extended_err>();
                if len >= ee_size {
                    // Copied out byte by byte: the queue is not obliged to
                    // align its ancillary data the way the struct wants it.
                    let mut ee: libc::sock_extended_err = std::mem::zeroed();
                    std::ptr::copy_nonoverlapping(data, &raw mut ee as *mut u8, ee_size);
                    let icmp = ee.ee_origin == libc::SO_EE_ORIGIN_ICMP
                        || ee.ee_origin == libc::SO_EE_ORIGIN_ICMP6;
                    if icmp {
                        let mut storage: libc::sockaddr_storage = std::mem::zeroed();
                        let addr = (len - ee_size).min(std::mem::size_of_val(&storage));
                        std::ptr::copy_nonoverlapping(
                            data.add(ee_size),
                            &raw mut storage as *mut u8,
                            addr,
                        );
                        return Some(QueuedError {
                            from: sockaddr_to_ip(&storage)?,
                            icmp_type: ee.ee_type,
                            code: ee.ee_code,
                        });
                    }
                }
            }
            cmsg = libc::CMSG_NXTHDR(msg, cmsg);
        }
        None
    }
}

/// Resolves one received ICMP message back to whoever is waiting for it.
fn dispatch(inner: &Inner, packet: &[u8], from: IpAddr, ttl: u8) {
    let Some(msg) = icmp_body(packet) else { return };

    let (icmp_type, code) = (msg[0], msg[1]);
    let echo_reply = if from.is_ipv4() { 0 } else { 129 };

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

    // The quoted packet holds only the original IP header plus the first eight
    // bytes of the datagram, which is the ICMP header and no payload. The
    // token is therefore out of reach and the sequence number is what
    // identifies the request.
    let Some(seq) = quoted_seq(&msg[8..]) else { return };
    deliver_error(inner, icmp_type, code, from, ttl, seq);
}

/// Hands an ICMP error report to the probe whose sequence number it quotes.
fn deliver_error(inner: &Inner, icmp_type: u8, code: u8, from: IpAddr, ttl: u8, seq: u16) {
    let time_exceeded = if from.is_ipv4() { 11 } else { 3 };
    let unreachable = if from.is_ipv4() { 3 } else { 1 };
    let kind = if icmp_type == time_exceeded {
        ReplyKind::TimeExceeded
    } else if icmp_type == unreachable {
        ReplyKind::Unreachable
    } else {
        return;
    };

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
///
/// Most of the quotes that arrive are nobody's business here. A raw ICMP
/// socket, and a datagram one on macOS, is handed every ICMP message the host
/// receives, which includes the errors drawn by every other program's traffic:
/// a port-unreachable quoting a DNS query, a time-exceeded quoting the system
/// traceroute's UDP probes. Two bytes at a fixed offset into one of those is a
/// UDP checksum or the middle of a TCP sequence number, and one collision in
/// sixty-five thousand was enough to tell a sweep that a live host was
/// unreachable, or to end a trace at a hop it had never reached. So the quote
/// has to prove it is an echo request before its sequence number is believed.
fn quoted_seq(b: &[u8]) -> Option<u16> {
    if b.len() < 20 {
        return None;
    }
    let (offset, v6) = match b[0] >> 4 {
        4 => (((b[0] & 0x0f) as usize) * 4, false),
        6 => (40, true),
        _ => return None,
    };
    // A header shorter than the minimum is a malformed quote, and its stated
    // length would put the search for the ICMP header inside the IP header.
    if offset < 20 {
        return None;
    }
    // What the quoted packet carried: byte nine of an IPv4 header, the next
    // header of an IPv6 one. Anything else is somebody else's conversation.
    // An IPv6 extension header counts as anything else, since it leaves the
    // ICMP header somewhere this cannot find rather than where it looks.
    let proto = if v6 { b[6] } else { b[9] };
    if proto != if v6 { 58 } else { 1 } {
        return None;
    }
    echo_seq(b.get(offset..)?, v6)
}

/// The sequence number of a quoted echo request.
///
/// Eight bytes: type(1) code(1) checksum(2) id(2) seq(2). The identifier is no
/// help in matching, since an unprivileged datagram socket has the kernel's
/// there and not ours, but the type says whether this was an echo request at
/// all, and only an echo request can be one of ours.
fn echo_seq(icmp: &[u8], v6: bool) -> Option<u16> {
    if icmp.len() < 8 || icmp[0] != if v6 { 128 } else { 8 } {
        return None;
    }
    Some(u16::from_be_bytes([icmp[6], icmp[7]]))
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

    /// An IPv4 header quoting an ICMP echo request, as an error report
    /// carries it: twenty bytes of header with protocol 1 at byte nine, then
    /// type, code, checksum, id, sequence.
    fn quoted_echo(seq: u16) -> Vec<u8> {
        let mut quoted = vec![0x45, 0x00, 0x00, 0x54];
        quoted.extend_from_slice(&[0u8; 16]);
        quoted[9] = 1;
        quoted.extend_from_slice(&[0x08, 0x00, 0x00, 0x00, 0xaa, 0xbb]);
        quoted.extend_from_slice(&seq.to_be_bytes());
        quoted
    }

    #[test]
    fn an_error_report_is_matched_by_the_sequence_it_quotes() {
        assert_eq!(quoted_seq(&quoted_echo(1234)), Some(1234));

        assert_eq!(quoted_seq(&[0u8; 8]), None);
    }

    /// An error report about somebody else's packet is not one of our probes.
    ///
    /// The socket is handed every ICMP error the host receives, and the two
    /// bytes where an echo request keeps its sequence number are, in a UDP
    /// header, the checksum: whatever a passing DNS query happened to hash to
    /// used to be matched against the probes in flight.
    #[test]
    fn a_report_quoting_another_protocol_matches_nothing() {
        // A port-unreachable quoting a UDP datagram whose checksum is 1234.
        let mut udp = quoted_echo(0);
        udp[9] = 17;
        udp[26] = 0x04;
        udp[27] = 0xd2;
        assert_eq!(quoted_seq(&udp), None, "a quoted UDP datagram is not our probe");

        // And one quoting TCP, where those bytes are inside the sequence.
        let mut tcp = quoted_echo(1234);
        tcp[9] = 6;
        assert_eq!(quoted_seq(&tcp), None);

        // ICMP, but not an echo request: an error quoting an error.
        let mut other = quoted_echo(1234);
        other[20] = 11;
        assert_eq!(quoted_seq(&other), None, "we never sent a time-exceeded");

        // An IPv6 quote whose next header is a routing extension, not ICMPv6.
        // The ICMP header is not forty bytes in, so there is nothing to read.
        let mut v6 = vec![0x60u8; 48];
        v6[6] = 43;
        assert_eq!(quoted_seq(&v6), None);
        v6[6] = 58;
        v6[40] = 128;
        v6[46] = 0x04;
        v6[47] = 0xd2;
        assert_eq!(quoted_seq(&v6), Some(1234), "a quoted ICMPv6 echo is ours");
    }

    /// The Linux error queue quotes the bare ICMP header, with no IP header in
    /// front of it, which is the one shape the reader has to read differently.
    #[test]
    fn a_bare_quoted_echo_request_still_yields_its_sequence() {
        let icmp = [0x08u8, 0x00, 0x00, 0x00, 0xaa, 0xbb, 0x04, 0xd2];
        assert_eq!(echo_seq(&icmp, false), Some(1234));
        // The same bytes are not an ICMPv6 echo request: the type differs.
        assert_eq!(echo_seq(&icmp, true), None);
        assert_eq!(echo_seq(&icmp[..7], false), None);
    }

    /// The router that sent an error is named in the ancillary data.
    ///
    /// Linux does not deliver ICMP errors to a datagram ICMP socket as
    /// datagrams; they go on the error queue, where the type, the code and the
    /// sender all arrive as ancillary data. The datagram's own name is the
    /// address that was being probed, so a traceroute that read that would
    /// report the target at every hop. Only Linux has this, and only Linux can
    /// build one to read, so the queue's own message is assembled here.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_error_queue_entry_names_the_router_that_sent_the_report() {
        unsafe {
            let ee_size = std::mem::size_of::<libc::sock_extended_err>();
            let sin_size = std::mem::size_of::<libc::sockaddr_in>();
            let mut control = [0u8; 256];
            let cmsg = control.as_mut_ptr() as *mut libc::cmsghdr;
            let len = libc::CMSG_LEN((ee_size + sin_size) as u32) as usize;
            (*cmsg).cmsg_len = len as _;
            (*cmsg).cmsg_level = libc::IPPROTO_IP;
            (*cmsg).cmsg_type = libc::IP_RECVERR;

            let mut ee: libc::sock_extended_err = std::mem::zeroed();
            ee.ee_origin = libc::SO_EE_ORIGIN_ICMP;
            ee.ee_type = 11;
            ee.ee_code = 0;
            let data = libc::CMSG_DATA(cmsg);
            std::ptr::copy_nonoverlapping(&raw const ee as *const u8, data, ee_size);

            let mut sin: libc::sockaddr_in = std::mem::zeroed();
            sin.sin_family = libc::AF_INET as libc::sa_family_t;
            sin.sin_addr.s_addr = u32::from(Ipv4Addr::new(10, 0, 0, 1)).to_be();
            std::ptr::copy_nonoverlapping(
                &raw const sin as *const u8,
                data.add(ee_size),
                sin_size,
            );

            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
            msg.msg_controllen = len as _;

            let err = parse_error_cmsg(&msg).expect("the report was not read");
            assert_eq!(err.from, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
            assert_eq!(err.icmp_type, 11, "a time-exceeded, which is what a hop sends");
            assert_eq!(err.code, 0);

            // A local error -- one the kernel raised itself, with no router
            // behind it -- has no sender to report and is not a hop.
            ee.ee_origin = libc::SO_EE_ORIGIN_LOCAL;
            std::ptr::copy_nonoverlapping(&raw const ee as *const u8, data, ee_size);
            assert!(parse_error_cmsg(&msg).is_none());
        }
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

    /// A hop count is read whichever way the system reports it.
    ///
    /// The two systems disagree twice over: on what the ancillary data is
    /// called, and on how wide it is. Only one of the two shapes can be
    /// produced by the machine running the test, so both are built by hand.
    #[cfg(unix)]
    #[test]
    fn a_hop_count_is_read_however_the_system_words_it() {
        // Linux tags the IPv4 TTL IP_TTL and sends an int; macOS and the BSDs
        // tag it IP_RECVTTL and send a byte; IPv6 is an int on both.
        assert!(is_hop_cmsg(libc::IPPROTO_IP, libc::IP_TTL));
        assert!(is_hop_cmsg(libc::IPPROTO_IP, libc::IP_RECVTTL));
        assert!(is_hop_cmsg(libc::IPPROTO_IPV6, libc::IPV6_HOPLIMIT));
        assert!(!is_hop_cmsg(libc::IPPROTO_IP, libc::IP_TOS));

        // A cmsg built the way the kernel builds one, so the reading is
        // tested against the real layout and not against an assumption.
        fn hop(payload: &[u8]) -> u8 {
            unsafe {
                let mut buf = [0u8; 64];
                let cmsg = buf.as_mut_ptr() as *mut libc::cmsghdr;
                let len = libc::CMSG_LEN(payload.len() as u32) as usize;
                (*cmsg).cmsg_len = len as _;
                (*cmsg).cmsg_level = libc::IPPROTO_IP;
                (*cmsg).cmsg_type = libc::IP_TTL;
                std::ptr::copy_nonoverlapping(
                    payload.as_ptr(),
                    libc::CMSG_DATA(cmsg),
                    payload.len(),
                );
                cmsg_hop(cmsg, len)
            }
        }

        assert_eq!(hop(&[64u8]), 64, "the one-byte form");
        assert_eq!(hop(&64i32.to_ne_bytes()), 64, "the int form");
        // 255 is a real TTL and must survive the widening intact.
        assert_eq!(hop(&255i32.to_ne_bytes()), 255);
        assert_eq!(hop(&[]), 0, "nothing to read is no TTL, not a panic");
    }

    /// A sequence number reused while its first probe still waits stays with
    /// the probe that took it over.
    ///
    /// Sixteen bits is all there is, so a wide sweep gets back round to a
    /// sequence whose earlier owner has not finished. Only the newer probe can
    /// be matched by it from then on, and the older one finishing used to
    /// delete the mapping anyway, leaving an error report for the newer probe
    /// with nowhere to go.
    #[test]
    fn an_older_probe_finishing_leaves_a_reused_sequence_with_its_new_owner() {
        let mut w = Waiters::default();
        let sent = Instant::now();
        w.by_token.insert(1, Pending { seq: 7, sent, tx: None });
        w.by_seq.insert(7, 1);
        // The counter has wrapped: probe 2 goes out on the same sequence while
        // probe 1 is still waiting, and owns it from here on.
        w.by_token.insert(2, Pending { seq: 7, sent, tx: None });
        w.by_seq.insert(7, 2);

        assert!(w.take_by_token(1).is_some(), "probe 1 was answered by its token");
        assert_eq!(w.by_seq.get(&7), Some(&2), "probe 2 still owns the sequence");

        let p = w.take_by_seq(7).expect("a time-exceeded must still reach probe 2");
        assert_eq!(p.seq, 7);
        assert!(w.by_token.is_empty(), "nothing left waiting");
        assert!(w.by_seq.is_empty(), "and no mapping left behind");
    }

    /// A request the kernel refused says why, rather than blaming the socket.
    #[test]
    fn a_refused_send_is_reported_as_the_reason_the_kernel_gave() {
        let no_route = why_unsent(&io::Error::from(io::ErrorKind::HostUnreachable));
        assert_eq!(no_route.to_string(), "no route to host");
        assert_ne!(no_route, PingError::Closed, "the socket is open; the route is not there");

        assert_eq!(
            why_unsent(&io::Error::from(io::ErrorKind::NetworkUnreachable)).to_string(),
            "network unreachable"
        );
        // Our own burst filling the send queue is not a fact about the target.
        assert_eq!(
            why_unsent(&io::Error::from(io::ErrorKind::WouldBlock)).to_string(),
            "send queue full"
        );
        #[cfg(unix)]
        assert_eq!(
            why_unsent(&io::Error::from_raw_os_error(libc::ENOBUFS)).to_string(),
            "send queue full",
            "ENOBUFS is the BSD spelling and has no ErrorKind of its own"
        );

        // Something with no better name still must not claim the socket closed.
        let odd = why_unsent(&io::Error::from(io::ErrorKind::InvalidInput));
        assert_eq!(odd.to_string(), "cannot send");
        assert_ne!(odd, PingError::Closed);
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
            // Unix reports the hop count as ancillary data. Windows has no
            // equivalent for a socket the runtime did not create, so there the
            // column is blank by design and there is nothing to assert.
            #[cfg(unix)]
            assert!(reply.ttl > 0, "no TTL came back with the reply");
        });
    }
}
