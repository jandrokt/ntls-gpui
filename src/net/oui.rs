//! Resolves MAC address prefixes to hardware vendors.
//!
//! The table is compiled into the binary, so lookups are instant, work
//! offline, and never tell anyone what you are scanning. It is built by
//! `packaging/oui/build-oui.py`, which is also where the reasoning about its
//! two sources lives.
//!
//! The short version, because it decides what a scan can say: the IEEE is
//! authoritative for the names, but it publishes a 24-bit block as "IEEE
//! Registration Authority" once it has subdivided that block into smaller
//! assignments. That is not a vendor, and a device sitting in such a block
//! used to be reported by that name, which told nobody anything. Those
//! placeholders are left out of the table entirely, and several thousand
//! finer-grained assignments that the IEEE does not publish but Wireshark
//! maintains are merged in, so the device is named instead of its
//! registrar.

use std::collections::HashMap;
use std::io::Read;
use std::sync::OnceLock;

static DATA: &[u8] = include_bytes!("../../assets/oui.dat.gz");

/// Prefix lengths in hex digits, longest first: IEEE hands out 36-bit (MA-S),
/// 28-bit (MA-M) and 24-bit (MA-L) blocks, and a longer assignment is the more
/// specific answer.
const PREFIX_LENGTHS: [usize; 3] = [9, 7, 6];

fn table() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = HashMap::with_capacity(60_000);
        let mut raw = Vec::with_capacity(1 << 21);
        if flate2::read::GzDecoder::new(DATA).read_to_end(&mut raw).is_err() {
            return out;
        }
        // The registries are not all UTF-8: a handful of vendor names carry
        // Latin-1 bytes, and one bad byte is no reason to lose 58,000 good
        // prefixes.
        for line in String::from_utf8_lossy(&raw).lines() {
            if let Some((prefix, vendor)) = line.split_once('\t') {
                out.insert(prefix.to_string(), vendor.to_string());
            }
        }
        out
    })
}

/// The vendor that registered a MAC address, or "" if the prefix is unknown.
/// Any common MAC formatting is accepted.
///
/// The longest assignment wins, because that is the specific one: a 36-bit
/// block inside a 24-bit block belongs to whoever the 36 bits were given to.
pub fn lookup(mac: &str) -> &'static str {
    let hex = normalize(mac);
    if hex.len() < 6 {
        return "";
    }
    let t = table();
    for n in PREFIX_LENGTHS {
        if hex.len() < n {
            continue;
        }
        if let Some(vendor) = t.get(&hex[..n]) {
            return vendor.as_str();
        }
    }
    ""
}

/// Reports whether a MAC is locally administered, which means it was chosen by
/// the device and not assigned by a vendor. Phones and laptops use these
/// for per-network privacy addresses, so there is no vendor to find and the
/// absence of one is itself the answer.
///
/// A group address carries the same bit without meaning any of that. The BSD
/// neighbour table keeps `ff:ff:ff:ff:ff:ff` against a subnet's broadcast
/// address permanently, so probing `.255` - a single target, or a range that
/// runs to the end of the subnet - handed the broadcast address back and it
/// was described as a privacy address some device had chosen for itself.
/// Nothing chose it and no device is there.
pub fn is_local(mac: &str) -> bool {
    let hex = normalize(mac);
    // 0x01 of the first octet is the group bit, 0x02 the locally-administered
    // one: only an individual address can be a particular device's own.
    hex.len() >= 2 && u8::from_str_radix(&hex[..2], 16).is_ok_and(|b| b & 0x03 == 0x02)
}

/// Reports an address the operating system declined to give us.
///
/// Recent macOS hands unprivileged processes `02:00:00:00:00:00` for its own
/// interfaces instead of the real hardware address. That has the
/// locally-administered bit set, so without this check it would be reported as
/// a privacy address the device chose, a different and wrong thing
/// to tell someone.
pub fn withheld(mac: &str) -> bool {
    let hex = normalize(mac);
    hex.len() == 12 && hex[2..].bytes().all(|c| c == b'0')
}

/// The best available description of a MAC's origin, empty when there is
/// nothing honest to say.
pub fn describe(mac: &str) -> String {
    if withheld(mac) {
        return String::new();
    }
    let vendor = lookup(mac);
    if !vendor.is_empty() {
        return vendor.to_string();
    }
    if is_local(mac) { "randomised".into() } else { String::new() }
}

/// Strips separators and upper-cases, accepting `aa:bb:cc:dd:ee:ff`,
/// `aa-bb-cc-dd-ee-ff`, `aabb.ccdd.eeff` and bare hex alike.
fn normalize(mac: &str) -> String {
    mac.chars().filter(|c| c.is_ascii_hexdigit()).map(|c| c.to_ascii_uppercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_registry_loads() {
        assert!(table().len() > 30_000, "only {} prefixes loaded", table().len());
    }

    #[test]
    fn a_small_vendors_own_block_is_named_and_not_its_registrar() {
        // The IEEE publishes a 24-bit block as "IEEE Registration Authority"
        // once it has cut that block into smaller assignments, and a device in
        // one used to be reported by that name: a registrar, in the column
        // meant for whoever made the thing. There are several thousand such
        // devices and other scanners name them, because the finer assignments
        // exist even where the IEEE does not publish them.
        assert_eq!(lookup("00:1b:c5:00:00:01"), "Converging Systems Inc.");
        assert_eq!(lookup("00:50:c2:00:00:01"), "T.L.S. Corp.");
        // The registrar's own name is not in the table at all, so a block of
        // its that nobody has claimed says nothing rather than saying that.
        assert!(!table().values().any(|v| v.contains("Registration Authority")));
    }

    #[test]
    fn the_longest_assignment_is_the_one_that_answers() {
        // A 36-bit block inside a 24-bit block belongs to whoever the 36 bits
        // were given to, so the specific answer has to win over the general
        // one. Apple holds a whole 24-bit block, and nothing finer inside it,
        // so that one is answered by its own prefix.
        assert_eq!(lookup("a4:83:e7:00:00:01"), "Apple, Inc.");
        assert_ne!(lookup("00:1b:c5:00:00:01"), lookup("00:1b:c5:f0:00:01"));
    }

    #[test]
    fn any_common_formatting_is_accepted() {
        let apple = lookup("a4:83:e7:00:00:01");
        assert!(!apple.is_empty());
        for form in ["A4-83-E7-00-00-01", "a483.e700.0001", "a483e7000001"] {
            assert_eq!(lookup(form), apple, "{form}");
        }
    }

    #[test]
    fn a_device_chosen_address_is_reported_as_such() {
        // Bit 0x02 of the first octet marks a locally administered address.
        assert!(is_local("02:00:00:00:00:01"));
        assert!(!is_local("a4:83:e7:00:00:01"));
        assert_eq!(describe("02:00:00:00:00:01"), "randomised");
    }

    #[test]
    fn an_address_the_os_withheld_is_not_mistaken_for_a_chosen_one() {
        // Recent macOS reports this for its own interfaces.
        assert!(withheld("02:00:00:00:00:00"));
        assert!(withheld("00:00:00:00:00:00"));
        assert!(!withheld("02:11:00:00:00:00"));
        assert!(!withheld("a4:83:e7:00:00:01"));
        // It has the locally-administered bit, but saying "randomised" would
        // claim something we do not know.
        assert_eq!(describe("02:00:00:00:00:00"), "");
    }

    #[test]
    fn a_broadcast_or_multicast_address_is_not_a_device_chosen_one() {
        // The broadcast address has the locally-administered bit set, but a
        // subnet's broadcast IP is not a device with a privacy address.
        assert!(!is_local("ff:ff:ff:ff:ff:ff"));
        assert_eq!(describe("ff:ff:ff:ff:ff:ff"), "");
        // The IPv6 multicast prefix has it too, and is no more a device.
        assert!(!is_local("33:33:00:00:00:01"));
        assert_eq!(describe("33:33:00:00:00:01"), "");
        // Every other locally-administered first octet still counts, so the
        // group bit is the only thing being ruled out here.
        assert!(is_local("06:00:00:00:00:01"));
        assert!(is_local("aa:bb:cc:dd:ee:ff"));
    }

    #[test]
    fn an_unknown_vendor_is_empty_rather_than_guessed() {
        assert_eq!(lookup(""), "");
        assert_eq!(lookup("zz"), "");
    }
}
