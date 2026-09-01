//! The speed of the local network itself, between two machines running ntls.
//!
//! An internet speed test tells you what your line does. This tells you what
//! your own cabling, switches and wireless do, usually the thing
//! actually limiting a file copy. The two ends speak a deliberately tiny
//! protocol: enough to agree on a direction and a duration, and nothing else.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket};

use super::iface;
use super::speedtest::{SAMPLE_INTERVAL, Sampler, Transfer};
use crate::core::Cancel;

/// Carries the data connection.
pub const DEFAULT_PORT: u16 = 5333;
/// Carries peer discovery broadcasts.
pub const DEFAULT_DISCOVERY_PORT: u16 = 5334;

const VERSION: u8 = 1;
const MAGIC: &[u8; 8] = b"NTLSTHRU";
const DISCOVERY_QUERY: &[u8] = b"NTLSDISC?1";
const DISCOVERY_REPLY: &[u8] = b"NTLSDISC!1";

/// Identifies this process among everything that might answer a broadcast.
/// Discovery filters on it instead of on the source address, so a server
/// never answers its own search but two instances on one machine can still
/// find each other.
pub fn instance_id() -> [u8; 8] {
    use std::sync::OnceLock;
    static ID: OnceLock<[u8; 8]> = OnceLock::new();
    *ID.get_or_init(|| {
        let mut id = [0u8; 8];
        rand::fill(&mut id);
        id
    })
}

/// Which way the data flows during a test.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// Measures the client's upload.
    ToServer = 1,
    /// Measures the client's download.
    ToClient = 2,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Direction::ToServer => "upload",
            Direction::ToClient => "download",
        })
    }
}

/// The fixed 16-byte greeting.
fn marshal_header(dir: Direction, seconds: u16) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..8].copy_from_slice(MAGIC);
    b[8] = VERSION;
    b[9] = dir as u8;
    b[10..12].copy_from_slice(&seconds.to_be_bytes());
    b
}

fn parse_header(b: &[u8]) -> Result<(Direction, u16), String> {
    if b.len() < 16 || &b[0..8] != MAGIC {
        return Err("not an ntls throughput client".into());
    }
    if b[8] != VERSION {
        return Err(format!("unsupported protocol version {}", b[8]));
    }
    let dir = match b[9] {
        1 => Direction::ToServer,
        2 => Direction::ToClient,
        other => return Err(format!("unknown direction {other}")),
    };
    Ok((dir, u16::from_be_bytes([b[10], b[11]])))
}

/// Another ntls instance offering throughput tests.
#[derive(Clone, Debug)]
pub struct Peer {
    pub addr: IpAddr,
    pub port: u16,
    pub name: String,
}

impl Peer {
    /// The peer's data endpoint.
    pub fn addr_port(&self) -> String {
        format!("{}:{}", self.addr, self.port)
    }
}

/// The unit of work for a throughput test: big enough to keep the kernel busy,
/// small enough to respect the deadline closely.
const BLOCK: usize = 256 << 10;

/// Accepts throughput tests and answers discovery broadcasts, running until
/// cancelled.
pub async fn serve(
    cancel: Cancel,
    port: u16,
    src: Option<Ipv4Addr>,
    on_event: Arc<dyn Fn(String) + Send + Sync>,
    on_result: Arc<dyn Fn(IpAddr, Direction, u64, Duration) + Send + Sync>,
) -> Result<(), String> {
    let bind = SocketAddr::new(src.map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), IpAddr::V4), port);
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| format!("cannot listen on port {port}: {e}"))?;

    let name = hostname();
    on_event(format!("listening on {bind} as {name:?}"));

    let discovery = tokio::spawn(answer_discovery(cancel.clone(), port, name.clone()));

    loop {
        let accepted = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            a = listener.accept() => a,
        };
        let Ok((conn, remote)) = accepted else { break };
        let (on_event, on_result, cancel) = (on_event.clone(), on_result.clone(), cancel.clone());
        tokio::spawn(async move {
            if let Err(e) = handle(cancel, conn, remote.ip(), &on_event, &on_result).await {
                on_event(format!("rejected {}: {e}", remote.ip()));
            }
        });
    }

    discovery.abort();
    Ok(())
}

async fn handle(
    cancel: Cancel,
    mut conn: TcpStream,
    remote: IpAddr,
    on_event: &Arc<dyn Fn(String) + Send + Sync>,
    on_result: &Arc<dyn Fn(IpAddr, Direction, u64, Duration) + Send + Sync>,
) -> Result<(), String> {
    let mut head = [0u8; 16];
    tokio::time::timeout(Duration::from_secs(10), conn.read_exact(&mut head))
        .await
        .map_err(|_| "no greeting".to_string())?
        .map_err(|e| e.to_string())?;
    let (dir, seconds) = parse_header(&head)?;

    let mut duration = Duration::from_secs(seconds as u64);
    if duration.is_zero() || duration > Duration::from_secs(300) {
        duration = Duration::from_secs(10);
    }
    on_event(format!("{remote} is running a {dir} test for {duration:?}"));

    let deadline = Instant::now() + duration;
    let start = Instant::now();
    let counted = Arc::new(AtomicU64::new(0));

    let moved = if dir == Direction::ToServer {
        let n = drain(&mut conn, deadline, &counted).await;
        // Tell the client how much actually arrived, which is the honest
        // number: its own count includes whatever is still in flight.
        let _ = conn.write_all(&n.to_be_bytes()).await;
        n
    } else {
        blast(&cancel, &mut conn, deadline, &counted).await
    };

    on_result(remote, dir, moved, start.elapsed());
    Ok(())
}

/// Replies to broadcast searches so a peer can be found without anyone typing
/// an address.
async fn answer_discovery(cancel: Cancel, data_port: u16, name: String) {
    let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_DISCOVERY_PORT);
    let Ok(sock) = UdpSocket::bind(bind).await else { return };
    let _ = sock.set_broadcast(true);
    let self_id = instance_id();

    let mut buf = [0u8; 256];
    loop {
        let received = tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            r = sock.recv_from(&mut buf) => r,
        };
        let Ok((n, from)) = received else { return };
        if n < DISCOVERY_QUERY.len() || &buf[..DISCOVERY_QUERY.len()] != DISCOVERY_QUERY {
            continue;
        }
        // Never answer our own search.
        let id_at = DISCOVERY_QUERY.len();
        if n >= id_at + 8 && buf[id_at..id_at + 8] == self_id {
            continue;
        }

        let mut reply = DISCOVERY_REPLY.to_vec();
        reply.extend_from_slice(&data_port.to_be_bytes());
        reply.extend_from_slice(name.as_bytes());
        let _ = sock.send_to(&reply, from).await;
    }
}

/// Broadcasts a search and collects whoever answers.
pub async fn discover_peers(
    src: Option<Ipv4Addr>,
    port: u16,
    wait: Duration,
) -> Result<Vec<Peer>, String> {
    let bind = SocketAddr::new(src.map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), IpAddr::V4), 0);
    let sock = UdpSocket::bind(bind).await.map_err(|e| e.to_string())?;
    sock.set_broadcast(true).map_err(|e| e.to_string())?;

    // Broadcast to the global address and to each attached network, since some
    // stacks only deliver the directed form.
    let mut targets = vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), port)];
    for prefix in iface::local_prefixes() {
        if let Some(b) = prefix.broadcast() {
            targets.push(SocketAddr::new(IpAddr::V4(b), port));
        }
    }

    let mut query = DISCOVERY_QUERY.to_vec();
    query.extend_from_slice(&instance_id());
    for t in targets {
        let _ = sock.send_to(&query, t).await;
    }

    let deadline = Instant::now() + wait;
    let mut peers: Vec<Peer> = Vec::new();
    let mut buf = [0u8; 512];

    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let Ok(Ok((n, from))) = tokio::time::timeout(remaining, sock.recv_from(&mut buf)).await
        else {
            break;
        };
        let head = DISCOVERY_REPLY.len();
        if n < head + 2 || &buf[..head] != DISCOVERY_REPLY {
            continue;
        }
        let peer = Peer {
            addr: from.ip(),
            port: u16::from_be_bytes([buf[head], buf[head + 1]]),
            name: String::from_utf8_lossy(&buf[head + 2..n]).to_string(),
        };
        if peers.iter().any(|p| p.addr_port() == peer.addr_port()) {
            continue;
        }
        peers.push(peer);
    }
    Ok(peers)
}

/// Measures throughput against a peer in one direction.
pub async fn run_test(
    cancel: &Cancel,
    peer: &str,
    dir: Direction,
    duration: Duration,
    src: Option<Ipv4Addr>,
    sample: Option<Sampler>,
) -> Result<Transfer, String> {
    let target: SocketAddr = resolve_peer(peer)?;

    let sock = if target.is_ipv4() { TcpSocket::new_v4() } else { TcpSocket::new_v6() }
        .map_err(|e| e.to_string())?;
    if let (Some(src), true) = (src, target.is_ipv4()) {
        sock.bind(SocketAddr::new(IpAddr::V4(src), 0)).map_err(|e| e.to_string())?;
    }
    let mut conn = tokio::time::timeout(Duration::from_secs(10), sock.connect(target))
        .await
        .map_err(|_| format!("cannot reach {peer}: timed out"))?
        .map_err(|e| format!("cannot reach {peer}: {e}"))?;

    let header = marshal_header(dir, duration.as_secs().min(u16::MAX as u64) as u16);
    conn.write_all(&header).await.map_err(|e| e.to_string())?;

    let counted = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + duration;
    let ticker = spawn_ticker(cancel.clone(), counted.clone(), sample, deadline);

    let start = Instant::now();
    let mut moved = if dir == Direction::ToServer {
        let n = blast(cancel, &mut conn, deadline, &counted).await;
        // The server reports what it actually received.
        let mut reply = [0u8; 8];
        if conn.read_exact(&mut reply).await.is_ok() {
            let received = u64::from_be_bytes(reply);
            if received > 0 {
                return finish(ticker, received, start.elapsed());
            }
        }
        n
    } else {
        drain(&mut conn, deadline, &counted).await
    };
    if moved == 0 {
        moved = counted.load(Ordering::Relaxed);
    }

    if moved == 0 {
        ticker.abort();
        return Err("no data moved".into());
    }
    finish(ticker, moved, start.elapsed())
}

fn finish(
    ticker: tokio::task::JoinHandle<()>,
    bytes: u64,
    elapsed: Duration,
) -> Result<Transfer, String> {
    ticker.abort();
    Ok(Transfer { bytes, elapsed })
}

fn resolve_peer(peer: &str) -> Result<SocketAddr, String> {
    if let Ok(sa) = peer.parse::<SocketAddr>() {
        return Ok(sa);
    }
    let (host, port) = peer
        .rsplit_once(':')
        .map(|(h, p)| (h, p.parse().unwrap_or(DEFAULT_PORT)))
        .unwrap_or((peer, DEFAULT_PORT));
    let ip = *iface::resolve_host(host)?
        .first()
        .ok_or_else(|| format!("cannot resolve {host:?}"))?;
    Ok(SocketAddr::new(ip, port))
}

fn spawn_ticker(
    cancel: Cancel,
    counted: Arc<AtomicU64>,
    sample: Option<Sampler>,
    deadline: Instant,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(sample) = sample else { return };
        let (mut last, mut last_at) = (0u64, Instant::now());
        loop {
            tokio::time::sleep(SAMPLE_INTERVAL).await;
            if cancel.is_cancelled() || Instant::now() > deadline + Duration::from_secs(1) {
                return;
            }
            let now = Instant::now();
            let current = counted.load(Ordering::Relaxed);
            sample(current.saturating_sub(last), now.duration_since(last_at));
            last = current;
            last_at = now;
        }
    })
}

async fn blast(
    cancel: &Cancel,
    conn: &mut TcpStream,
    deadline: Instant,
    counted: &Arc<AtomicU64>,
) -> u64 {
    let mut block = vec![0u8; BLOCK];
    rand::fill(&mut block[..]);

    while Instant::now() < deadline && !cancel.is_cancelled() {
        match conn.write(&block).await {
            Ok(0) => break,
            Ok(n) => {
                counted.fetch_add(n as u64, Ordering::Relaxed);
            }
            Err(_) => break,
        }
    }
    // Half-closing tells the far end the stream is finished without tearing
    // down the connection it still needs to answer on.
    let _ = conn.shutdown().await;
    counted.load(Ordering::Relaxed)
}

async fn drain(conn: &mut TcpStream, deadline: Instant, counted: &Arc<AtomicU64>) -> u64 {
    let mut buf = vec![0u8; BLOCK];
    let hard_stop = deadline + Duration::from_secs(2);
    while let Some(remaining) = hard_stop.checked_duration_since(Instant::now()) {
        match tokio::time::timeout(remaining, conn.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(n)) => {
                counted.fetch_add(n as u64, Ordering::Relaxed);
            }
        }
    }
    counted.load(Ordering::Relaxed)
}

fn hostname() -> String {
    crate::sys::hostname()
}
