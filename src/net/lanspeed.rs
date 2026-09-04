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

/// The only test lengths the two ends can agree on. The greeting carries whole
/// seconds, so anything shorter than one has no representation on the wire at
/// all, and the ceiling keeps a mistyped "10m" from holding a link and a
/// socket for the rest of the afternoon.
const MIN_DURATION: Duration = Duration::from_secs(1);
const MAX_DURATION: Duration = Duration::from_secs(300);

/// Rounds a requested test length to one both ends will actually run.
///
/// Every caller clamps before it announces a length or sets a deadline, and the
/// server clamps what it is handed the same way. That agreement is the whole
/// point: a length one end silently replaces with a different one leaves the
/// two measuring different windows, with nothing on the wire to say so.
pub fn clamp_duration(d: Duration) -> Duration {
    // Through seconds as a float rather than whole seconds, so half a second
    // becomes the one second that can be sent instead of the zero that
    // truncation gave. The cast saturates, so an absurd number lands on the
    // ceiling.
    let secs =
        (d.as_secs_f64().round() as u64).clamp(MIN_DURATION.as_secs(), MAX_DURATION.as_secs());
    Duration::from_secs(secs)
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

/// How long the accept loop waits after a failure before looking for the next
/// connection, and how many failures with nothing accepted in between mean the
/// listening socket itself is gone rather than one arrival having gone wrong.
const ACCEPT_RETRY_WAIT: Duration = Duration::from_millis(100);
const ACCEPT_GIVE_UP: u32 = 20;

/// Whether a failed accept leaves a listening socket worth going back to.
///
/// Connection-level failures say nothing about the listener: the connection
/// that caused them is already gone and the next accept takes the next one, so
/// however many of them arrive they never end the server. Anything else is
/// retried too, because nearly every accept error is momentary, but only for a
/// run of tries; a socket that has genuinely stopped accepting has to be
/// reported rather than retried for the rest of the afternoon.
fn accept_again(e: &std::io::Error, consecutive: u32) -> bool {
    use std::io::ErrorKind;
    matches!(
        e.kind(),
        ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::Interrupted
    ) || consecutive < ACCEPT_GIVE_UP
}

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

    let discovery = tokio::spawn(answer_discovery(
        cancel.clone(),
        port,
        DEFAULT_DISCOVERY_PORT,
        name.clone(),
        on_event.clone(),
    ));

    let mut failures = 0u32;
    loop {
        let accepted = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            a = listener.accept() => a,
        };
        let (conn, remote) = match accepted {
            Ok(pair) => {
                failures = 0;
                pair
            }
            Err(e) => {
                // A failed accept is nearly always about the arrival rather
                // than the listener: a client that reset between the handshake
                // and the accept, a signal, a moment without a spare
                // descriptor. Leaving the loop on the first of them stopped
                // this machine answering for good, and serve() then returned
                // Ok, so the window that had said "listening" simply went
                // quiet while every peer got connection refused, with nothing
                // anywhere saying why.
                failures += 1;
                on_event(format!("could not accept a connection: {e}"));
                if !accept_again(&e, failures) {
                    discovery.abort();
                    return Err(format!("stopped listening on port {port}: {e}"));
                }
                // A listener that fails instantly and endlessly would spin a
                // core here, so wait a little first. Stop is still answered
                // while we do.
                if !cancel.sleep(ACCEPT_RETRY_WAIT).await {
                    break;
                }
                continue;
            }
        };
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

    // A greeting asking for no time at all, or for hours, gets the nearest
    // length this end is willing to run. It used to substitute an unrelated
    // ten seconds, so a client asking for half a second measured half a second
    // while this end blasted for ten, and only this end's own log ever
    // mentioned it.
    let duration = clamp_duration(Duration::from_secs(seconds as u64));
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
async fn answer_discovery(
    cancel: Cancel,
    data_port: u16,
    disc_port: u16,
    name: String,
    on_event: Arc<dyn Fn(String) + Send + Sync>,
) {
    let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), disc_port);
    // Losing the discovery port is the difference between a listener anybody
    // can find and one that has to be typed in by address, so it has to be
    // said out loud. Anything else holding the port, another ntls or a
    // previous one still winding down, used to end this task without a word:
    // the server said it was listening, was perfectly reachable, and answered
    // nobody's search, with nothing on screen to connect the two.
    let sock = match UdpSocket::bind(bind).await {
        Ok(sock) => sock,
        Err(e) => {
            on_event(format!(
                "cannot answer discovery on UDP port {disc_port}: {e}; \
                 peers will have to be given this machine's address"
            ));
            return;
        }
    };
    let _ = sock.set_broadcast(true);
    let self_id = instance_id();

    let mut buf = [0u8; 256];
    loop {
        let received = tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            r = sock.recv_from(&mut buf) => r,
        };
        // The same goes for a socket that stops receiving: the server carries
        // on taking connections, so the only sign of it is that searches
        // suddenly go unanswered.
        let (n, from) = match received {
            Ok(got) => got,
            Err(e) => {
                on_event(format!("no longer answering discovery: {e}"));
                return;
            }
        };
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

    // Clamped once, here, before the length is both sent and kept: the header
    // and this end's own deadline have to be the same number, or the far end
    // stops while this one is still counting.
    let duration = clamp_duration(duration);
    let header = marshal_header(dir, duration.as_secs() as u16);
    conn.write_all(&header).await.map_err(|e| e.to_string())?;

    let counted = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + duration;
    let ticker = spawn_ticker(cancel.clone(), counted.clone(), sample, deadline);

    let start = Instant::now();
    let mut moved = if dir == Direction::ToServer {
        let n = blast(cancel, &mut conn, deadline, &counted).await;
        // The server reports what it actually received, and this end's own
        // count stands in when that report never arrives.
        match read_tally(cancel, &mut conn, TALLY_WAIT).await {
            Some(received) if received > 0 => received,
            _ => n,
        }
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

    // The write has to be able to end. A peer that stops reading without
    // closing the connection, a laptop that went to sleep or wireless that
    // dropped, fills the send window and then leaves the write nothing to
    // complete and no error to fail on. Since the deadline and Stop are only
    // consulted between writes, a bare write parked in the middle of the loop
    // for as long as the kernel kept retransmitting: the test ran on well past
    // the length it had announced, Stop did nothing to it at all, and the
    // socket outlived the window that had given up on it. Racing each write
    // against the time that is left, and against cancellation, is what makes
    // those two checks reachable again. A write that loses the race has
    // written nothing, so no byte this end counted is lost with it.
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match cancel.run(tokio::time::timeout(remaining, conn.write(&block))).await {
            Some(Ok(Ok(n))) if n > 0 => {
                counted.fetch_add(n as u64, Ordering::Relaxed);
            }
            // Out of time, stopped, or the far end is gone: either way there
            // is no more sending to do.
            _ => break,
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

/// How long the client waits for the server's byte count once its upload is
/// over. The far end stops draining two seconds past the same deadline and
/// answers immediately after, so this is only a generous outer edge.
const TALLY_WAIT: Duration = Duration::from_secs(10);

/// Reads the count the server sends at the end of an upload, if it sends one.
///
/// The wait has to be able to end. A peer that goes away mid-test without
/// closing the connection, a sleeping laptop or wireless that dropped, leaves
/// this read nothing to receive and no error to fail on, and so does anything
/// that happens to be listening on the port without being ntls. A bare read
/// of the eight bytes then waited for a number that was never coming: the
/// test never finished, Stop did nothing to it, and the task and its socket
/// stayed alive behind a window that had long since given up on them.
async fn read_tally(cancel: &Cancel, conn: &mut TcpStream, wait: Duration) -> Option<u64> {
    let mut reply = [0u8; 8];
    let read = tokio::time::timeout(wait, conn.read_exact(&mut reply));
    let arrived = matches!(cancel.run(read).await, Some(Ok(Ok(_))));
    if arrived { Some(u64::from_be_bytes(reply)) } else { None }
}

fn hostname() -> String {
    crate::sys::hostname()
}

#[cfg(test)]
mod clamp_tests {
    use super::{MAX_DURATION, MIN_DURATION, clamp_duration};
    use std::time::Duration;

    #[test]
    fn a_length_the_wire_cannot_carry_becomes_one_it_can() {
        // The greeting carries whole seconds, so half a second used to arrive
        // as no seconds at all, and the far end quietly substituted ten.
        assert_eq!(clamp_duration(Duration::from_millis(500)), MIN_DURATION);
        assert_eq!(clamp_duration(Duration::from_millis(1)), MIN_DURATION);
        assert_eq!(clamp_duration(Duration::ZERO), MIN_DURATION);
        // And an afternoon's worth comes back as the longest test on offer.
        assert_eq!(clamp_duration(Duration::from_secs(600)), MAX_DURATION);
        assert_eq!(clamp_duration(Duration::from_secs(u64::MAX)), MAX_DURATION);
    }

    #[test]
    fn an_ordinary_length_is_left_alone() {
        assert_eq!(clamp_duration(Duration::from_secs(8)), Duration::from_secs(8));
        assert_eq!(clamp_duration(MAX_DURATION), MAX_DURATION);
        // Rounded to the nearest second rather than truncated towards zero.
        assert_eq!(clamp_duration(Duration::from_millis(7600)), Duration::from_secs(8));
    }
}

#[cfg(test)]
mod tally_tests {
    use super::{TALLY_WAIT, read_tally};
    use crate::core::Cancel;
    use std::time::{Duration, Instant};
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap()
    }

    /// A connected pair on the loopback: the client end an upload would read
    /// its tally on, and the server end, which the caller keeps alive so the
    /// connection stays open however the far end behaves.
    async fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a loopback listener");
        let addr = listener.local_addr().expect("the port it was given");
        let client = TcpStream::connect(addr).await.expect("a loopback connection");
        let (server, _) = listener.accept().await.expect("the other end of it");
        (client, server)
    }

    #[test]
    fn a_peer_that_never_reports_its_count_ends_the_test_rather_than_holding_it_open() {
        runtime().block_on(async {
            // The far end is connected and silent, which is what a machine
            // that dropped off the network mid-test looks like from here:
            // nothing to read, and no error saying so either.
            let (mut client, _far_end) = pair().await;
            let cancel = Cancel::new();
            let outcome = tokio::time::timeout(
                Duration::from_secs(5),
                read_tally(&cancel, &mut client, Duration::from_millis(100)),
            )
            .await;
            assert_eq!(outcome, Ok(None), "the wait for a tally has to end on its own");
        });
    }

    #[test]
    fn stopping_the_run_gives_up_on_the_count_without_waiting_it_out() {
        runtime().block_on(async {
            let (mut client, _far_end) = pair().await;
            let cancel = Cancel::new();
            cancel.cancel();
            let began = Instant::now();
            let tally = read_tally(&cancel, &mut client, Duration::from_secs(30)).await;
            assert_eq!(tally, None);
            assert!(
                began.elapsed() < Duration::from_secs(1),
                "Stop was answered only after {:?}",
                began.elapsed()
            );
        });
    }

    #[test]
    fn the_count_the_server_reports_is_the_one_the_test_believes() {
        runtime().block_on(async {
            let (mut client, mut far_end) = pair().await;
            far_end.write_all(&7_654_321u64.to_be_bytes()).await.expect("the tally to be sent");
            let cancel = Cancel::new();
            assert_eq!(read_tally(&cancel, &mut client, TALLY_WAIT).await, Some(7_654_321));
        });
    }
}

#[cfg(test)]
mod accept_tests {
    use super::{ACCEPT_GIVE_UP, accept_again};
    use std::io::{Error, ErrorKind};

    #[test]
    fn a_connection_lost_before_it_could_be_accepted_never_stops_the_server() {
        // A client that resets between the handshake and the accept costs that
        // one connection and nothing else; the listening socket is untouched.
        // It used to take the whole server down with it, however many well
        // behaved peers were still waiting to be served.
        let aborted = Error::from(ErrorKind::ConnectionAborted);
        assert!(accept_again(&aborted, 1));
        assert!(accept_again(&aborted, ACCEPT_GIVE_UP + 1));
        assert!(accept_again(&Error::from(ErrorKind::ConnectionReset), ACCEPT_GIVE_UP + 1));
        assert!(accept_again(&Error::from(ErrorKind::Interrupted), ACCEPT_GIVE_UP + 1));
    }

    #[test]
    fn a_listener_that_fails_every_single_time_is_reported_rather_than_retried_forever() {
        let broken = Error::from(ErrorKind::PermissionDenied);
        assert!(accept_again(&broken, 1), "one odd failure is still worth another try");
        assert!(!accept_again(&broken, ACCEPT_GIVE_UP));
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::{DEFAULT_PORT, answer_discovery};
    use crate::core::Cancel;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::net::UdpSocket;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap()
    }

    #[test]
    fn a_taken_discovery_port_is_reported_instead_of_leaving_the_listener_unfindable() {
        runtime().block_on(async {
            // Something else already holds the port searches are answered on,
            // which on a machine that has just restarted ntls is the previous
            // process on its way out.
            let squatter =
                UdpSocket::bind("0.0.0.0:0").await.expect("a udp port of our own to sit on");
            let taken = squatter.local_addr().expect("the port it was given").port();

            let said: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let on_event: Arc<dyn Fn(String) + Send + Sync> = {
                let said = said.clone();
                Arc::new(move |msg| said.lock().expect("the event log").push(msg))
            };

            tokio::time::timeout(
                Duration::from_secs(5),
                answer_discovery(Cancel::new(), DEFAULT_PORT, taken, "here".to_string(), on_event),
            )
            .await
            .expect("the responder to give up rather than hang");

            let said = said.lock().expect("the event log");
            assert!(
                said.iter().any(|m| m.contains(&taken.to_string())),
                "the listener is undiscoverable and said nothing about it: {said:?}"
            );
        });
    }
}

#[cfg(test)]
mod blast_tests {
    use super::blast;
    use crate::core::Cancel;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, Instant};
    use tokio::net::{TcpListener, TcpStream};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap()
    }

    /// A connected pair on the loopback whose far end is never read from,
    /// which is what a peer that stopped taking data mid-test looks like from
    /// this end: the connection is open, the window fills, and nothing drains
    /// it again.
    async fn unread_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a loopback listener");
        let addr = listener.local_addr().expect("the port it was given");
        let client = TcpStream::connect(addr).await.expect("a loopback connection");
        let (server, _) = listener.accept().await.expect("the other end of it");
        (client, server)
    }

    #[test]
    fn a_peer_that_stops_reading_does_not_hold_the_send_past_its_deadline() {
        runtime().block_on(async {
            let (mut client, _far_end) = unread_pair().await;
            let cancel = Cancel::new();
            let counted = Arc::new(AtomicU64::new(0));
            let deadline = Instant::now() + Duration::from_millis(300);
            let ended = tokio::time::timeout(
                Duration::from_secs(10),
                blast(&cancel, &mut client, deadline, &counted),
            )
            .await;
            assert!(ended.is_ok(), "the send has to end when the test it belongs to does");
        });
    }

    #[test]
    fn stopping_the_run_ends_the_send_even_with_the_far_end_taking_nothing() {
        runtime().block_on(async {
            let (mut client, _far_end) = unread_pair().await;
            let cancel = Cancel::new();
            let counted = Arc::new(AtomicU64::new(0));
            // Far enough off that only Stop can end this one.
            let deadline = Instant::now() + Duration::from_secs(600);
            {
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    cancel.cancel();
                });
            }
            let began = Instant::now();
            tokio::time::timeout(
                Duration::from_secs(10),
                blast(&cancel, &mut client, deadline, &counted),
            )
            .await
            .expect("Stop to be answered while a write is stuck");
            assert!(
                began.elapsed() < Duration::from_secs(5),
                "Stop was answered only after {:?}",
                began.elapsed()
            );
        });
    }
}
