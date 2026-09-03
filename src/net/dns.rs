//! A small DNS client: build a query, send it, and read the answer back.
//!
//! Speaking the wire format directly is what lets the tool ask any server for
//! any record type and show every section of the response, which a system
//! resolver will not do.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

/// One answer from a DNS query, rendered for display.
#[derive(Clone, Debug)]
pub struct Record {
    pub name: String,
    pub rtype: String,
    pub ttl: u32,
    pub value: String,
    /// "answer", "authority" or "additional".
    pub section: &'static str,
}

/// A complete response.
#[derive(Clone, Debug)]
pub struct Response {
    pub server: String,
    pub rcode: String,
    pub elapsed: Duration,
    pub records: Vec<Record>,
    /// Reports that the UDP answer did not fit and was re-fetched over TCP.
    pub truncated: bool,
}

/// The record types ntls can ask for, in menu order.
pub const TYPES: &[&str] = &["A", "AAAA", "CNAME", "MX", "NS", "TXT", "SOA", "SRV", "PTR"];

fn type_code(name: &str) -> Result<u16, String> {
    Ok(match name.to_ascii_uppercase().as_str() {
        "A" => 1,
        "NS" => 2,
        "CNAME" => 5,
        "SOA" => 6,
        "PTR" => 12,
        "MX" => 15,
        "TXT" => 16,
        "AAAA" => 28,
        "SRV" => 33,
        _ => return Err(format!("unsupported record type {name:?}")),
    })
}

fn type_name(code: u16) -> String {
    match code {
        1 => "A".into(),
        2 => "NS".into(),
        5 => "CNAME".into(),
        6 => "SOA".into(),
        12 => "PTR".into(),
        15 => "MX".into(),
        16 => "TXT".into(),
        28 => "AAAA".into(),
        33 => "SRV".into(),
        41 => "OPT".into(),
        other => format!("TYPE{other}"),
    }
}

fn rcode_name(c: u8) -> String {
    match c {
        0 => "NOERROR".into(),
        1 => "FORMERR".into(),
        2 => "SERVFAIL".into(),
        3 => "NXDOMAIN".into(),
        4 => "NOTIMP".into(),
        5 => "REFUSED".into(),
        other => format!("RCODE{other}"),
    }
}

/// The nameservers this machine is configured to use.
///
/// Unix keeps them in a file; Windows keeps them in the configuration store
/// and prints them with `ipconfig`. A sweep asks for these once per host, so
/// the answer is held briefly: long enough that a 254-host scan does not run
/// 254 subprocesses, short enough that joining another network is noticed.
pub fn system_resolvers() -> Vec<String> {
    const FRESH: Duration = Duration::from_secs(20);
    static CACHE: std::sync::Mutex<Option<(Instant, Vec<String>)>> = std::sync::Mutex::new(None);

    if let Ok(cache) = CACHE.lock()
        && let Some((read, resolvers)) = cache.as_ref()
        && read.elapsed() < FRESH
    {
        return resolvers.clone();
    }

    let resolvers = read_resolvers();
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((Instant::now(), resolvers.clone()));
    }
    resolvers
}

fn read_resolvers() -> Vec<String> {
    #[cfg(not(windows))]
    {
        std::fs::read_to_string("/etc/resolv.conf").map(|t| parse_resolv_conf(&t)).unwrap_or_default()
    }
    #[cfg(windows)]
    {
        crate::sys::quietly(&mut std::process::Command::new("ipconfig"))
            .arg("/all")
            .output()
            .ok()
            .map(|out| parse_ipconfig(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or_default()
    }
}

/// `nameserver 1.1.1.1`, one per line, with comments.
#[cfg_attr(windows, allow(dead_code))]
fn parse_resolv_conf(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with(';'))
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            (f.next() == Some("nameserver")).then(|| f.next())?
        })
        .filter(|s| s.parse::<IpAddr>().is_ok())
        .map(str::to_string)
        .collect()
}

/// What `ipconfig /all` prints:
///
/// ```text
///    DNS Servers . . . . . . . . . . . : 192.168.1.1
///                                        8.8.8.8
/// ```
///
/// The first is labelled and the rest are not, so a line that is nothing but
/// an address counts as long as the last labelled line was this one. The
/// address may carry a zone (`fe80::1%12`) which is not part of it.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_ipconfig(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_list = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            in_list = false;
            continue;
        }
        let candidate = if let Some((label, value)) = trimmed.split_once(':') {
            // A label ends in a run of dots; anything else with a colon is an
            // address on a continuation line.
            let labelled = label.contains('.') || label.chars().any(|c| c.is_alphabetic());
            if labelled {
                in_list = label.to_lowercase().contains("dns servers");
                value.trim()
            } else {
                trimmed
            }
        } else {
            trimmed
        };

        if !in_list {
            continue;
        }
        let address = candidate.split('%').next().unwrap_or(candidate).trim();
        if address.parse::<IpAddr>().is_ok() && !out.iter().any(|a| a == address) {
            out.push(address.to_string());
        }
    }
    out
}

/// Builds the `in-addr.arpa` or `ip6.arpa` name for an address, which is the
/// name a PTR lookup actually asks for.
pub fn reverse_arpa_name(addr: IpAddr) -> String {
    match addr {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.{}.in-addr.arpa.", o[3], o[2], o[1], o[0])
        }
        IpAddr::V6(v6) => {
            let mut s = String::with_capacity(72);
            for b in v6.octets().iter().rev() {
                s += &format!("{:x}.{:x}.", b & 0x0f, b >> 4);
            }
            s + "ip6.arpa."
        }
    }
}

/// Looks up the PTR record for an address, returning "" when there is none.
/// The short timeout keeps a scan from stalling on a slow resolver.
pub async fn reverse_name(addr: IpAddr, timeout: Duration) -> String {
    let Some(server) = system_resolvers().into_iter().next() else { return String::new() };
    let Ok(res) = query(&server, &reverse_arpa_name(addr), "PTR", timeout).await else {
        return String::new();
    };
    res.records
        .into_iter()
        .find(|r| r.section == "answer" && r.rtype == "PTR")
        .map(|r| r.value)
        .unwrap_or_default()
}

/// Asks `server` for a record. An empty server uses the first system resolver.
/// The name is taken literally, so it must already be a fully qualified domain
/// or an `.arpa` name.
pub async fn query(
    server: &str,
    name: &str,
    record_type: &str,
    timeout: Duration,
) -> Result<Response, String> {
    let qtype = type_code(record_type)?;

    let server = if server.trim().is_empty() {
        system_resolvers()
            .into_iter()
            .next()
            .ok_or("no system resolver configured; name a server explicitly")?
    } else {
        server.trim().to_string()
    };
    let addr = with_default_port(&server)?;

    let name = if name.ends_with('.') { name.to_string() } else { format!("{name}.") };
    let id: u16 = rand::random();
    let query = encode_query(id, &name, qtype)?;

    let start = Instant::now();
    let (reply, truncated) = exchange(addr, &query, timeout).await?;
    let mut result = decode(&reply)?;
    result.server = server;
    result.elapsed = start.elapsed();
    result.truncated = truncated;
    Ok(result)
}

/// Sends the query over UDP, falling back to TCP when the answer is too large
/// to fit in a datagram.
async fn exchange(server: SocketAddr, query: &[u8], timeout: Duration) -> Result<(Vec<u8>, bool), String> {
    let bind: SocketAddr = if server.is_ipv4() { "0.0.0.0:0".parse() } else { "[::]:0".parse() }
        .map_err(|_| "bad bind address".to_string())?;

    let sock = UdpSocket::bind(bind).await.map_err(|e| e.to_string())?;
    sock.connect(server).await.map_err(|e| e.to_string())?;
    sock.send(query).await.map_err(|e| e.to_string())?;

    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(timeout, sock.recv(&mut buf))
        .await
        .map_err(|_| format!("no answer from {server}"))?
        .map_err(|e| format!("no answer from {server}: {e}"))?;
    buf.truncate(n);

    // Bit 0x02 of the flags byte is TC, the truncation flag.
    if n >= 3 && buf[2] & 0x02 != 0
        && let Ok(reply) = exchange_tcp(server, query, timeout).await
    {
        return Ok((reply, true));
    }
    Ok((buf, false))
}

async fn exchange_tcp(server: SocketAddr, query: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
    let work = async {
        let mut conn = TcpStream::connect(server).await.map_err(|e| e.to_string())?;
        // DNS over TCP prefixes each message with its length.
        let mut framed = Vec::with_capacity(2 + query.len());
        framed.extend_from_slice(&(query.len() as u16).to_be_bytes());
        framed.extend_from_slice(query);
        conn.write_all(&framed).await.map_err(|e| e.to_string())?;

        let mut len = [0u8; 2];
        conn.read_exact(&mut len).await.map_err(|e| e.to_string())?;
        let mut reply = vec![0u8; u16::from_be_bytes(len) as usize];
        conn.read_exact(&mut reply).await.map_err(|e| e.to_string())?;
        Ok::<_, String>(reply)
    };
    tokio::time::timeout(timeout, work).await.map_err(|_| "TCP retry timed out".to_string())?
}

fn encode_query(id: u16, name: &str, qtype: u16) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&0x0100u16.to_be_bytes()); // recursion desired
    out.extend_from_slice(&1u16.to_be_bytes()); // one question
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // no answers, authorities, additionals

    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() {
            continue;
        }
        if label.len() > 63 {
            return Err(format!("label {label:?} is too long"));
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&qtype.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // class IN
    Ok(out)
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes([self.u8()?, self.u8()?, self.u8()?, self.u8()?]))
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }

    /// Reads a name, following compression pointers. The hop limit stops a
    /// malicious answer that points at itself from spinning forever.
    fn name(&mut self) -> Option<String> {
        let mut out = String::new();
        let mut pos = self.pos;
        let mut jumped = false;
        let mut hops = 0;

        loop {
            let len = *self.b.get(pos)?;
            if len & 0xc0 == 0xc0 {
                let target = ((len as usize & 0x3f) << 8) | *self.b.get(pos + 1)? as usize;
                if !jumped {
                    self.pos = pos + 2;
                    jumped = true;
                }
                hops += 1;
                if hops > 32 {
                    return None;
                }
                pos = target;
                continue;
            }
            pos += 1;
            if len == 0 {
                break;
            }
            let label = self.b.get(pos..pos + len as usize)?;
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(&String::from_utf8_lossy(label));
            pos += len as usize;
        }

        if !jumped {
            self.pos = pos;
        }
        Some(if out.is_empty() { ".".into() } else { out })
    }
}

fn decode(reply: &[u8]) -> Result<Response, String> {
    let mut c = Cursor { b: reply, pos: 0 };
    let malformed = || "malformed answer".to_string();

    let _id = c.u16().ok_or_else(malformed)?;
    let flags = c.u16().ok_or_else(malformed)?;
    let counts: Vec<u16> = (0..4).map(|_| c.u16().unwrap_or(0)).collect();

    for _ in 0..counts[0] {
        c.name().ok_or_else(malformed)?;
        c.u16().ok_or_else(malformed)?;
        c.u16().ok_or_else(malformed)?;
    }

    let mut records = Vec::new();
    for (section, count) in [("answer", counts[1]), ("authority", counts[2]), ("additional", counts[3])] {
        for _ in 0..count {
            let Some(name) = c.name() else { break };
            let (Some(rtype), Some(_class), Some(ttl), Some(len)) = (c.u16(), c.u16(), c.u32(), c.u16())
            else {
                break;
            };
            let start = c.pos;
            let Some(_) = c.take(len as usize) else { break };
            records.push(Record {
                name: trim_root(&name),
                rtype: type_name(rtype),
                ttl,
                value: record_value(reply, rtype, start, len as usize),
                section,
            });
        }
    }

    Ok(Response {
        server: String::new(),
        rcode: rcode_name((flags & 0x0f) as u8),
        elapsed: Duration::ZERO,
        records,
        truncated: false,
    })
}

/// Drops the trailing dot but keeps the root zone visible as ".".
fn trim_root(name: &str) -> String {
    if name == "." { ".".into() } else { name.trim_end_matches('.').to_string() }
}

fn record_value(msg: &[u8], rtype: u16, start: usize, len: usize) -> String {
    let Some(data) = msg.get(start..start + len) else { return String::new() };
    let mut c = Cursor { b: msg, pos: start };

    match rtype {
        1 if len == 4 => Ipv4Addr::new(data[0], data[1], data[2], data[3]).to_string(),
        28 if len == 16 => {
            let mut o = [0u8; 16];
            o.copy_from_slice(data);
            Ipv6Addr::from(o).to_string()
        }
        2 | 5 | 12 => c.name().map(|n| trim_root(&n)).unwrap_or_default(),
        15 => {
            let pref = c.u16().unwrap_or(0);
            let host = c.name().map(|n| trim_root(&n)).unwrap_or_default();
            format!("{pref} {host}")
        }
        16 => {
            let mut parts = Vec::new();
            let mut i = 0;
            while i < data.len() {
                let n = data[i] as usize;
                i += 1;
                if i + n > data.len() {
                    break;
                }
                parts.push(String::from_utf8_lossy(&data[i..i + n]).to_string());
                i += n;
            }
            parts.join(" ")
        }
        33 => {
            let (p, w, port) = (c.u16().unwrap_or(0), c.u16().unwrap_or(0), c.u16().unwrap_or(0));
            let target = c.name().map(|n| trim_root(&n)).unwrap_or_default();
            format!("{p} {w} {port} {target}")
        }
        6 => {
            let ns = c.name().map(|n| trim_root(&n)).unwrap_or_default();
            let mbox = c.name().map(|n| trim_root(&n)).unwrap_or_default();
            let nums: Vec<u32> = (0..5).map(|_| c.u32().unwrap_or(0)).collect();
            format!(
                "{ns} {mbox} serial {} refresh {} retry {} expire {} min {}",
                nums[0], nums[1], nums[2], nums[3], nums[4]
            )
        }
        41 => "EDNS0".into(),
        _ => String::new(),
    }
}

/// Appends port 53 when the address does not carry one.
fn with_default_port(addr: &str) -> Result<SocketAddr, String> {
    if let Ok(sa) = addr.parse::<SocketAddr>() {
        return Ok(sa);
    }
    if let Ok(ip) = addr.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, 53));
    }
    // A name for a resolver is unusual but valid; resolve it with the system.
    let ips = super::iface::resolve_host(addr.split(':').next().unwrap_or(addr))?;
    let port = addr.rsplit_once(':').and_then(|(_, p)| p.parse().ok()).unwrap_or(53);
    ips.first()
        .map(|ip| SocketAddr::new(*ip, port))
        .ok_or_else(|| format!("cannot resolve {addr:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_resolvers_are_read_out_of_a_resolv_conf() {
        let text = "# comment\nsearch lan\nnameserver 192.168.1.1\nnameserver 8.8.8.8\nnameserver nonsense\n";
        assert_eq!(parse_resolv_conf(text), vec!["192.168.1.1", "8.8.8.8"]);
        assert!(parse_resolv_conf("").is_empty());
    }

    #[test]
    fn the_resolvers_are_read_out_of_what_ipconfig_prints() {
        // The second server is on a line of its own with no label, which is
        // the whole difficulty of this format.
        let text = "\r\nWindows IP Configuration\r\n\r\n   Host Name . . . . . . . . . . . . : desktop\r\n\r\nEthernet adapter Ethernet:\r\n\r\n   IPv4 Address. . . . . . . . . . . : 192.168.1.23(Preferred)\r\n   DNS Servers . . . . . . . . . . . : 192.168.1.1\r\n                                       8.8.8.8\r\n   NetBIOS over Tcpip. . . . . . . . : Enabled\r\n";
        assert_eq!(parse_ipconfig(text), vec!["192.168.1.1", "8.8.8.8"]);
    }

    #[test]
    fn an_address_with_a_zone_on_it_is_still_an_address() {
        let text = "   DNS Servers . . . . . . . . . . . : fe80::1%12\r\n                                       192.168.0.1\r\n";
        assert_eq!(parse_ipconfig(text), vec!["fe80::1", "192.168.0.1"]);
    }

    #[test]
    fn nothing_but_the_dns_servers_is_taken_for_one() {
        // Every other line in that output holds an address too.
        let text = "   IPv4 Address. . . . . . . . . . . : 192.168.1.23(Preferred)\r\n   Subnet Mask . . . . . . . . . . . : 255.255.255.0\r\n   Default Gateway . . . . . . . . . : 192.168.1.1\r\n";
        assert!(parse_ipconfig(text).is_empty());
    }
}
