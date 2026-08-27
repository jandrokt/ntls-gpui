//! Probing one TCP or UDP port and drawing a conclusion from what comes back.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, UdpSocket};

/// The conclusion drawn about a single port.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PortState {
    /// Something accepted the connection or answered the probe.
    Open,
    /// The host actively refused: a TCP reset, or an ICMP port-unreachable
    /// for UDP.
    Closed,
    /// Nothing came back at all, so a firewall is dropping the traffic.
    Filtered,
    /// The honest UDP answer: silence means the port is either open and not
    /// replying, or filtered. There is no way to tell.
    OpenFiltered,
}

impl std::fmt::Display for PortState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PortState::Open => "open",
            PortState::Closed => "closed",
            PortState::Filtered => "filtered",
            PortState::OpenFiltered => "open|filtered",
        })
    }
}

/// One probed port.
#[derive(Clone, Debug)]
pub struct PortResult {
    pub state: PortState,
    pub latency: Duration,
    /// Whatever the service volunteered, trimmed to one line. Empty when
    /// nothing was said or banner grabbing was off.
    pub banner: String,
}

/// Attempts a full connection. A completed handshake means open, a refusal
/// means closed, and silence means filtered.
pub async fn probe_tcp(
    addr: IpAddr,
    port: u16,
    timeout: Duration,
    banner: bool,
    src: Option<Ipv4Addr>,
) -> PortResult {
    let start = Instant::now();
    let target = SocketAddr::new(addr, port);

    let connect = async {
        let sock = match addr {
            IpAddr::V4(_) => TcpSocket::new_v4()?,
            IpAddr::V6(_) => TcpSocket::new_v6()?,
        };
        if let (Some(src), IpAddr::V4(_)) = (src, addr) {
            sock.bind(SocketAddr::new(IpAddr::V4(src), 0))?;
        }
        sock.connect(target).await
    };

    let outcome = tokio::time::timeout(timeout, connect).await;
    let latency = start.elapsed();

    match outcome {
        Ok(Ok(mut stream)) => {
            let banner = if banner { grab_banner(&mut stream, addr, port).await } else { String::new() };
            PortResult { state: PortState::Open, latency, banner }
        }
        Ok(Err(e)) => {
            PortResult { state: classify_dial_error(&e), latency, banner: String::new() }
        }
        Err(_) => PortResult { state: PortState::Filtered, latency, banner: String::new() },
    }
}

fn classify_dial_error(e: &io::Error) -> PortState {
    // Reset by peer and "connection aborted" are also active refusals.
    match e.kind() {
        io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted => PortState::Closed,
        _ => PortState::Filtered,
    }
}

/// HTTP servers say nothing until spoken to, so these ports get an
/// unsolicited request.
fn is_http_port(port: u16) -> bool {
    matches!(port, 80 | 591 | 3000 | 5000 | 8000 | 8008 | 8080 | 8081 | 8088 | 8888 | 9090 | 9200)
}

async fn grab_banner(stream: &mut tokio::net::TcpStream, addr: IpAddr, port: u16) -> String {
    let read = async {
        if is_http_port(port) {
            let req = format!("HEAD / HTTP/1.0\r\nHost: {addr}\r\nUser-Agent: ntls\r\n\r\n");
            stream.write_all(req.as_bytes()).await.ok()?;
        }
        let mut buf = [0u8; 512];
        let n = stream.read(&mut buf).await.ok()?;
        (n > 0).then(|| sanitize(&buf[..n], 90))
    };
    tokio::time::timeout(Duration::from_millis(700), read)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Collapses a raw byte response into one printable ASCII line. Anything
/// outside printable ASCII is dropped rather than rendered, because a binary
/// payload printed verbatim is noise that also corrupts the table.
fn sanitize(b: &[u8], max: usize) -> String {
    let mut out = String::with_capacity(max);
    for &c in b {
        match c {
            b'\r' | b'\n' | b'\t'
                if !out.is_empty() && !out.ends_with(' ') => {
                    out.push(' ');
                }
            0x20..=0x7e => out.push(c as char),
            _ => {}
        }
        if out.len() >= max {
            break;
        }
    }
    out.trim().to_string()
}

/// Describes a response payload. Text protocols are shown as text; binary
/// ones are reported by size, which is the only honest thing to say about a
/// DNS or NTP answer in a table cell.
fn summarize(b: &[u8]) -> String {
    if b.is_empty() {
        return String::new();
    }
    let printable = b
        .iter()
        .filter(|&&c| c == b'\t' || c == b'\r' || c == b'\n' || (0x20..=0x7e).contains(&c))
        .count();
    if printable as f64 / b.len() as f64 > 0.85 {
        let s = sanitize(b, 60);
        if !s.is_empty() {
            return s;
        }
    }
    format!("{} bytes", b.len())
}

/// Sends a protocol-appropriate payload and interprets the answer. A reply
/// means open and an ICMP port-unreachable (surfaced as a refusal) means
/// closed; anything else is genuinely ambiguous.
pub async fn probe_udp(
    addr: IpAddr,
    port: u16,
    timeout: Duration,
    src: Option<Ipv4Addr>,
) -> PortResult {
    let start = Instant::now();
    let filtered = |latency| PortResult { state: PortState::Filtered,
        latency,
        banner: String::new(),
    };

    let bind: SocketAddr = match (addr, src) {
        (IpAddr::V4(_), Some(s)) => SocketAddr::new(IpAddr::V4(s), 0),
        (IpAddr::V4(_), None) => "0.0.0.0:0".parse().unwrap(),
        (IpAddr::V6(_), _) => "[::]:0".parse().unwrap(),
    };
    let Ok(sock) = UdpSocket::bind(bind).await else { return filtered(start.elapsed()) };
    if sock.connect(SocketAddr::new(addr, port)).await.is_err() {
        return filtered(start.elapsed());
    }
    if let Err(e) = sock.send(&udp_probe(port)).await {
        return PortResult { state: classify_udp_error(&e),
            latency: start.elapsed(),
            banner: String::new(),
        };
    }

    let mut buf = [0u8; 1500];
    let outcome = tokio::time::timeout(timeout, sock.recv(&mut buf)).await;
    let latency = start.elapsed();

    match outcome {
        Ok(Ok(n)) if n > 0 => {
            PortResult { state: PortState::Open, latency, banner: summarize(&buf[..n]) }
        }
        Ok(Ok(_)) => {
            PortResult { state: PortState::OpenFiltered, latency, banner: String::new() }
        }
        Ok(Err(e)) => {
            PortResult { state: classify_udp_error(&e), latency, banner: String::new() }
        }
        Err(_) => {
            PortResult { state: PortState::OpenFiltered, latency, banner: String::new() }
        }
    }
}

fn classify_udp_error(e: &io::Error) -> PortState {
    // An ICMP port-unreachable is reported back on the connected socket as a
    // refusal, which is the one unambiguous UDP answer available.
    match e.kind() {
        io::ErrorKind::ConnectionRefused => PortState::Closed,
        io::ErrorKind::HostUnreachable | io::ErrorKind::NetworkUnreachable => PortState::Filtered,
        _ => PortState::OpenFiltered,
    }
}

/// A payload likely to make the service on this port answer. Sending the right
/// thing is the difference between a useful UDP scan and a page of
/// "open|filtered".
fn udp_probe(port: u16) -> Vec<u8> {
    match port {
        // DNS: query for the root NS record.
        53 => vec![
            0x2b, 0x1c, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x02, 0x00, 0x01,
        ],
        // NTP: client request, version 3.
        123 => {
            let mut p = vec![0u8; 48];
            p[0] = 0x1b;
            p
        }
        // NetBIOS node status for the wildcard name "*".
        137 => {
            let mut p = vec![
                0x82, 0x28, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20,
            ];
            p.extend_from_slice(b"CKAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
            p.extend_from_slice(&[0x00, 0x00, 0x21, 0x00, 0x01]);
            p
        }
        // SNMP v2c GetRequest for 1.3.6.1.2.1 with community "public".
        161 => vec![
            0x30, 0x26, 0x02, 0x01, 0x01, 0x04, 0x06, b'p', b'u', b'b', b'l', b'i', b'c', 0xa0,
            0x19, 0x02, 0x04, 0x71, 0x4d, 0x61, 0x1e, 0x02, 0x01, 0x00, 0x02, 0x01, 0x00, 0x30,
            0x0b, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x06, 0x01, 0x02, 0x01, 0x05, 0x00,
        ],
        // IKEv1 main-mode security association proposal header.
        500 => {
            let mut p = vec![0u8; 8];
            p.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x01, 0x10, 0x02, 0x00]);
            p
        }
        // SSDP discovery.
        1900 => b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: ssdp:all\r\n\r\n".to_vec(),
        // mDNS: PTR for _services._dns-sd._udp.local
        5353 => {
            let mut p = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
            for label in ["_services", "_dns-sd", "_udp", "local"] {
                p.push(label.len() as u8);
                p.extend_from_slice(label.as_bytes());
            }
            p.extend_from_slice(&[0x00, 0x00, 0x0c, 0x00, 0x01]);
            p
        }
        // Nothing to say: a short payload is still enough to draw an ICMP
        // port-unreachable from a closed port.
        _ => b"\r\n\r\n".to_vec(),
    }
}

/// Raises the open-file limit as far as the hard limit allows.
///
/// A wide connect scan needs far more descriptors than the default soft limit
/// gives, and failing to raise it is not worth refusing to start over: the
/// scan still runs, just less concurrently.
pub fn raise_file_limit() -> u64 {
    #[cfg(unix)]
    unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return 0;
        }
        if lim.rlim_cur < lim.rlim_max {
            lim.rlim_cur = lim.rlim_max;
            libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
        }
        lim.rlim_cur as u64
    }
    // Windows has no such limit to raise: sockets are handles and the ceiling
    // is memory. Zero means "no figure to report", which is what the caller
    // does with an unraisable limit anyway.
    #[cfg(not(unix))]
    0
}

