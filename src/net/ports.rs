//! Port specifications and the names of well-known services.

/// The short list used by the "top" keyword: the ports worth checking first
/// when you have no idea what a host runs.
pub const TOP_PORTS: &[u16] = &[
    21, 22, 23, 25, 53, 67, 68, 69, 80, 110, 111, 123, 135, 137, 138, 139, 143, 161, 389, 443, 445,
    465, 500, 514, 515, 587, 631, 636, 873, 990, 993, 995, 1080, 1194, 1433, 1521, 1723, 1883,
    2049, 2082, 2181, 2375, 2376, 3000, 3128, 3268, 3306, 3389, 4000, 4369, 4444, 5000, 5060, 5061,
    5222, 5353, 5432, 5555, 5601, 5672, 5900, 5984, 6000, 6379, 6443, 7000, 7001, 8000, 8008, 8080,
    8081, 8086, 8088, 8123, 8443, 8500, 8888, 9000, 9042, 9090, 9092, 9100, 9200, 9300, 10000,
    11211, 15672, 27017, 32400, 50000,
];

/// The "common" keyword: the handful you check by reflex.
pub const COMMON_PORTS: &[u16] = &[
    21, 22, 23, 25, 53, 80, 110, 143, 443, 445, 587, 993, 995, 3306, 3389, 5432, 6379, 8080, 8443,
];

/// Turns a port specification into a sorted, de-duplicated list.
///
/// It accepts a comma-separated list whose parts may each be:
///
/// ```text
/// all | *      1-65535
/// top          the TOP_PORTS list
/// common       the COMMON_PORTS list
/// priv         1-1024
/// 80           a single port
/// 8000-8100    an inclusive range
/// ```
pub fn parse_ports(spec: &str) -> Result<Vec<u16>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("empty port specification".into());
    }

    let mut seen = vec![false; 65536];
    let mut out: Vec<u16> = Vec::new();
    let add = |p: i64, out: &mut Vec<u16>, seen: &mut Vec<bool>| -> Result<(), String> {
        if !(1..=65535).contains(&p) {
            return Err(format!("port {p} out of range (1-65535)"));
        }
        if !seen[p as usize] {
            seen[p as usize] = true;
            out.push(p as u16);
        }
        Ok(())
    };

    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let keyword: Option<Vec<u16>> = match part.to_ascii_lowercase().as_str() {
            "all" | "*" | "any" => Some((1..=65535).collect()),
            "top" | "top100" => Some(TOP_PORTS.to_vec()),
            "common" => Some(COMMON_PORTS.to_vec()),
            "priv" | "privileged" | "system" => Some((1..=1024).collect()),
            _ => None,
        };
        if let Some(ports) = keyword {
            for p in ports {
                add(p as i64, &mut out, &mut seen)?;
            }
            continue;
        }

        if let Some(i) = part.find('-') {
            let (lo_s, hi_s) = (part[..i].trim(), part[i + 1..].trim());
            let lo: i64 = if lo_s.is_empty() {
                1
            } else {
                lo_s.parse().map_err(|_| format!("bad port {lo_s:?}"))?
            };
            let hi: i64 = if hi_s.is_empty() {
                65535
            } else {
                hi_s.parse().map_err(|_| format!("bad port {hi_s:?}"))?
            };
            let (lo, hi) = (lo.min(hi).max(1), hi.max(lo).min(65535));
            for p in lo..=hi {
                add(p, &mut out, &mut seen)?;
            }
            continue;
        }

        let p: i64 = part.parse().map_err(|_| format!("bad port {part:?}"))?;
        add(p, &mut out, &mut seen)?;
    }

    if out.is_empty() {
        return Err(format!("no ports in {spec:?}"));
    }
    out.sort_unstable();
    Ok(out)
}

/// Renders a port list back into the compact specification syntax, collapsing
/// consecutive runs into ranges. It is the inverse of [`parse_ports`] and is
/// what turns a keyword like "all" into something editable.
pub fn format_ports(ports: &[u16]) -> String {
    if ports.is_empty() {
        return String::new();
    }
    let mut sorted = ports.to_vec();
    sorted.sort_unstable();

    let mut parts: Vec<String> = Vec::new();
    let (mut start, mut prev) = (sorted[0], sorted[0]);
    let flush = |start: u16, prev: u16, parts: &mut Vec<String>| {
        if start == prev {
            parts.push(start.to_string());
        } else if prev == start + 1 {
            parts.push(start.to_string());
            parts.push(prev.to_string());
        } else {
            parts.push(format!("{start}-{prev}"));
        }
    };

    for &p in &sorted[1..] {
        if p == prev + 1 {
            prev = p;
            continue;
        }
        flush(start, prev, &mut parts);
        start = p;
        prev = p;
    }
    flush(start, prev, &mut parts);
    parts.join(",")
}

/// Resolves keywords in a port specification into explicit numbers, returning
/// `None` when nothing changed.
pub fn expand_port_spec(spec: &str) -> Option<String> {
    let ports = parse_ports(spec).ok()?;
    let expanded = format_ports(&ports);
    (expanded != spec.trim()).then_some(expanded)
}

/// The well-known service for a port, or "" if unknown.
pub fn service_name(port: u16) -> &'static str {
    match port {
        21 => "ftp",
        22 => "ssh",
        23 => "telnet",
        25 => "smtp",
        53 => "dns",
        67 | 68 => "dhcp",
        69 => "tftp",
        80 => "http",
        110 => "pop3",
        111 => "rpcbind",
        123 => "ntp",
        135 => "msrpc",
        137 => "netbios-ns",
        138 => "netbios-dgm",
        139 => "netbios-ssn",
        143 => "imap",
        161 => "snmp",
        162 => "snmptrap",
        389 => "ldap",
        443 => "https",
        445 => "smb",
        465 => "smtps",
        500 => "isakmp",
        514 => "syslog",
        515 => "printer",
        587 => "submission",
        631 => "ipp",
        636 => "ldaps",
        873 => "rsync",
        990 => "ftps",
        993 => "imaps",
        995 => "pop3s",
        1080 => "socks",
        1194 => "openvpn",
        1433 => "mssql",
        1521 => "oracle",
        1723 => "pptp",
        1883 => "mqtt",
        1900 => "ssdp",
        2049 => "nfs",
        2181 => "zookeeper",
        2375 => "docker",
        2376 => "docker-tls",
        3000 => "dev-http",
        3128 => "squid",
        3268 => "globalcat",
        3306 => "mysql",
        3389 => "rdp",
        4369 => "epmd",
        5000 => "upnp",
        5060 => "sip",
        5061 => "sips",
        5222 => "xmpp",
        5353 => "mdns",
        5432 => "postgres",
        5601 => "kibana",
        5672 => "amqp",
        5900 => "vnc",
        5984 => "couchdb",
        6000 => "x11",
        6379 => "redis",
        6443 => "kube-api",
        8000 | 8008 | 8081 | 8088 | 8888 | 9000 => "http-alt",
        8080 => "http-proxy",
        8086 => "influxdb",
        8123 => "hass",
        8443 => "https-alt",
        8500 => "consul",
        9042 => "cassandra",
        9090 => "prometheus",
        9092 => "kafka",
        9100 => "jetdirect",
        9200 | 9300 => "elastic",
        10000 => "webmin",
        11211 => "memcached",
        15672 => "rabbitmq",
        27017 => "mongodb",
        32400 => "plex",
        50000 => "sap",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_expand() {
        assert_eq!(parse_ports("all").unwrap().len(), 65535);
        assert_eq!(parse_ports("priv").unwrap().len(), 1024);
        assert_eq!(parse_ports("common").unwrap(), COMMON_PORTS.to_vec());
    }

    #[test]
    fn a_list_is_sorted_and_de_duplicated() {
        assert_eq!(parse_ports("443,22,443,80").unwrap(), vec![22, 80, 443]);
    }

    #[test]
    fn ranges_are_inclusive_and_may_be_reversed() {
        assert_eq!(parse_ports("8000-8002").unwrap(), vec![8000, 8001, 8002]);
        assert_eq!(parse_ports("8002-8000").unwrap(), vec![8000, 8001, 8002]);
    }

    #[test]
    fn an_open_ended_range_takes_the_limit() {
        assert_eq!(*parse_ports("65533-").unwrap().last().unwrap(), 65535);
        assert_eq!(*parse_ports("-3").unwrap().first().unwrap(), 1);
    }

    #[test]
    fn nonsense_is_refused() {
        assert!(parse_ports("").is_err());
        assert!(parse_ports("http").is_err());
        assert!(parse_ports("70000").is_err());
        assert!(parse_ports("0").is_err());
    }

    #[test]
    fn formatting_is_the_inverse_of_parsing() {
        let spec = "22,80,443,8000-8100";
        let ports = parse_ports(spec).unwrap();
        assert_eq!(format_ports(&ports), spec);
        assert_eq!(parse_ports(&format_ports(&ports)).unwrap(), ports);
    }

    #[test]
    fn two_consecutive_ports_stay_a_list() {
        assert_eq!(format_ports(&[80, 81]), "80,81");
        assert_eq!(format_ports(&[80, 81, 82]), "80-82");
    }

    #[test]
    fn expansion_reports_only_real_changes() {
        assert_eq!(expand_port_spec("priv"), Some("1-1024".to_string()));
        assert_eq!(expand_port_spec("1-1024"), None);
        assert_eq!(expand_port_spec("nonsense"), None);
    }
}
