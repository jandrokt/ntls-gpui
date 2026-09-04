//! Sweeps a subnet or range and lists the hosts that are up.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use std::collections::HashMap;

use crate::cells;
use crate::core::{Cancel, Column, Emitter, Field, Role, Status, Tool, Validator, col, kv};
use crate::net::dns;
use crate::tools::prober::{
    METHOD_ARP, Probe, ProbeError, Prober, failure_text, iface_field, method_field, off_link_count,
    resolve_targets, target_field,
};
use crate::tools::stats::{elapsed, ms};

pub struct IpScan;

impl Tool for IpScan {
    fn id(&self) -> &'static str {
        "ipscan"
    }
    fn title(&self) -> &'static str {
        "IP scan"
    }
    fn desc(&self) -> &'static str {
        "Sweep a subnet or range and list responding hosts"
    }
    fn icon(&self) -> &'static str {
        "ipscan"
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            target_field(
                "Network",
                "auto, 192.168.1.0/24, 10.0.0.1-50",
                "auto uses your own subnet. Press tab to expand it into the exact range, then edit it",
            )
            .default("auto"),
            method_field(),
            iface_field(),
            Field::text("timeout", "Timeout", "How long to wait for each host to answer")
                .default("1s")
                .validate(Validator::Duration),
            Field::text(
                "retries",
                "Retries",
                "Extra attempts before calling a host down; raise this on lossy networks",
            )
            .default("1")
            .validate(Validator::IntRange(0, 10)),
            Field::text("concurrency", "Concurrency", "Hosts probed at the same time")
                .default("64")
                .validate(Validator::IntRange(1, 4096)),
            Field::boolean(
                "resolve",
                "Resolve names",
                "Look up the reverse DNS name of every host that answers",
                true,
            ),
            Field::boolean(
                "vendors",
                "Identify hardware",
                "Resolve the MAC and vendor of hosts on this link. ICMP alone cannot tell you what a device is",
                true,
            ),
            Field::boolean(
                "showdown",
                "List silent hosts",
                "Also add a row for every host that did not answer",
                false,
            ),
            Field::boolean(
                "keep",
                "Keep earlier results",
                "Add to what earlier runs found. Every row says which scan last saw it, hosts that have gone quiet are kept and marked, and an address that has changed hands gets a second row",
                false,
            )
            .role(Role::Keep),
        ]
    }

    fn resumable(&self) -> bool {
        true
    }

    fn columns(&self) -> Vec<Column> {
        vec![
            col("HOST", 16),
            col("RTT", 9),
            col("TTL", 4),
            col("NAME", 24),
            col("MAC", 18),
            col("VENDOR", 0),
            // Only a scan that keeps what earlier ones found fills this in.
            // a single scan is all one moment, and every row of it would say
            // the same thing. The heading is short because a column is never
            // narrower than its own title, and this one is empty most of the
            // time: an unused column must not take its room from the vendor.
            col("SEEN", 12),
        ]
    }

    fn run<'a>(
        &'a self,
        r: crate::core::Run,
        emit: Emitter,
    ) -> crate::core::tool::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { run(r, emit).await })
    }
}

async fn run(r: crate::core::Run, emit: Emitter) -> anyhow::Result<()> {
    let (cancel, p) = (r.cancel.clone(), r.params.clone());
    let all = resolve_targets(&p.str("target"), &p.str("iface"), &emit).map_err(anyhow::Error::msg)?;
    let total = all.len();

    // A resumed run skips whatever the interrupted one already reported, so
    // the table it is adding to stays consistent.
    let targets: Vec<IpAddr> =
        all.into_iter().filter(|a| !r.done.contains(&a.to_string())).collect();
    let skipped = total - targets.len();
    if r.resuming() {
        emit.info(format!("resuming: {skipped} of {total} already probed"));
    }
    if targets.is_empty() {
        emit.progress(total, total);
        emit.good("nothing left to probe");
        return Ok(());
    }

    let timeout = p.dur("timeout", Duration::from_secs(1));
    let retries = p.usize("retries", 1);
    let workers = p.usize("concurrency", 64).clamp(1, 4096).min(targets.len());
    let resolve = p.bool("resolve");
    let show_down = p.bool("showdown");

    // Keeping means the table earlier runs built is still on screen. What is
    // found now rewrites the row it is about, joins it where the device has
    // changed, or leaves it alone where the host has gone quiet.
    let keep = r.keep;
    let seen = Arc::new(Seen::of(&r.prior));
    let answered = Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new()));
    if keep && !seen.is_empty() {
        emit.info(format!(
            "keeping what earlier runs found about {} address(es); nothing already in the table is removed",
            seen.len()
        ));
    }

    // What this run asks about, so that a row about an address outside the
    // range is left alone and not aged. A scan of one host says nothing
    // whatever about the rest of the subnet.
    let probed: std::collections::HashSet<String> =
        targets.iter().map(|a| a.to_string()).collect();

    let prober = Arc::new(Prober::new(&p, &targets, timeout).map_err(anyhow::Error::msg)?);
    prober.emit_notes(&emit);

    if p.str("method") == METHOD_ARP {
        let n = off_link_count(&targets);
        if n > 0 {
            emit.warn(format!(
                "{n} of {total} targets are not on a directly attached network and cannot answer ARP"
            ));
        }
    }

    let done = Arc::new(AtomicUsize::new(0));
    let up = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();

    // A single publisher keeps the stat bar coherent while workers finish in
    // whatever order the network decides.
    let publish = {
        let (done, up, emit) = (done.clone(), up.clone(), emit.clone());
        move || {
            let d = done.load(Ordering::Relaxed) + skipped;
            emit.progress(d, total);
            let secs = start.elapsed().as_secs_f64().max(0.001);
            emit.stats(vec![
                kv("scanned", format!("{d}/{total}")),
                kv("up", up.load(Ordering::Relaxed).to_string()),
                kv("elapsed", elapsed(start.elapsed())),
                kv("rate", format!("{:.0}/s", d as f64 / secs)),
            ]);
        }
    };

    let ticker = {
        let (publish, cancel) = (publish.clone(), cancel.clone());
        tokio::spawn(async move {
            while cancel.sleep(Duration::from_millis(120)).await {
                publish();
            }
        })
    };

    let queue = Arc::new(tokio::sync::Mutex::new(targets.into_iter()));
    let mut handles = Vec::new();
    for _ in 0..workers.max(1) {
        let (queue, prober, emit, done, up, cancel) =
            (queue.clone(), prober.clone(), emit.clone(), done.clone(), up.clone(), cancel.clone());
        let (seen, answered) = (seen.clone(), answered.clone());
        handles.push(tokio::spawn(async move {
            loop {
                let Some(addr) = queue.lock().await.next() else { break };
                if cancel.is_cancelled() {
                    break;
                }

                let res = probe_with_retries(&cancel, &prober, addr, retries).await;
                done.fetch_add(1, Ordering::Relaxed);

                match res {
                    Err(e) => {
                        if show_down && !cancel.is_cancelled() {
                            // Nothing answered, so there is nothing this row
                            // has ever been seen to be.
                            let cells = cells![addr, "-", "", "", "", failure_text(&e), ""];
                            if keep {
                                // One row per address for the silent ones, so
                                // a nightly sweep does not grow a row a night
                                // for everything that is not there. And none
                                // at all for an address that already has a row
                                // about something that answered on it: the
                                // ageing pass below marks and dates that row,
                                // and a second row saying only that the
                                // address timed out doubled every host that
                                // comes and goes, then sat in the table
                                // insisting the address was silent for as long
                                // as it went on answering.
                                if seen.silence_worth_a_row(&addr.to_string()) {
                                    emit.upsert_as(
                                        seen.silent_key(&addr.to_string()),
                                        Status::Down,
                                        addr.to_string(),
                                        cells,
                                    );
                                }
                            } else {
                                emit.row(Status::Down, addr.to_string(), cells);
                            }
                        }
                    }
                    Ok(r) => {
                        up.fetch_add(1, Ordering::Relaxed);
                        let name = if resolve {
                            dns::reverse_name(addr, Duration::from_millis(900)).await
                        } else {
                            String::new()
                        };
                        let status = if r.suspect { Status::Warn } else { Status::Up };
                        let mut cells = cells![
                            addr,
                            ms(r.rtt),
                            r.ttl_text(),
                            name,
                            r.mac,
                            r.vendor,
                            if keep { SEEN_NOW } else { "" }
                        ];
                        if keep {
                            let key = seen.key_for(&addr.to_string(), &r.mac);
                            answered.lock().await.insert(key.clone());
                            match seen.compare(&addr.to_string(), &r.mac) {
                                Was::Nothing => {}
                                Was::New => emit.good(format!("{addr} is new since the last run")),
                                Was::Same => {}
                                Was::Different(before) => emit.warn(format!(
                                    "{addr} now answers from {}; the row for {before} is kept",
                                    hardware(&r.mac)
                                )),
                            }
                            seen.carry(&addr.to_string(), &key, &mut cells);
                            emit.upsert_as(key, status, addr.to_string(), cells);
                        } else {
                            emit.row(status, addr.to_string(), cells);
                        }
                    }
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }
    ticker.abort();
    publish();

    // Every row this scan asked about and did not get an answer from gets a
    // scan older and stops claiming to be current. Without this the table
    // reads as though everything in it were still there, the
    // thing a record kept over time must not do.
    if keep && !cancel.is_cancelled() {
        let answered = answered.lock().await;
        let quiet = stale(&r.prior, &probed, &answered);
        for row in &quiet {
            let mut cells = row.cells.clone();
            age(&mut cells);
            emit.upsert_as(row.identity(), Status::Warn, row.target.clone(), cells);
        }
        if !quiet.is_empty() {
            emit.info(format!(
                "{} row(s) that answered before did not answer this time; they are kept, marked, and dated to the scan that last found them",
                quiet.len()
            ));
        }
        // What is on screen is no longer just what this scan found, so the
        // figures say how much of it is history.
        emit.stats(vec![
            kv("scanned", format!("{}/{total}", done.load(Ordering::Relaxed) + skipped)),
            kv("up", up.load(Ordering::Relaxed).to_string()),
            kv("kept", quiet.len().to_string()),
            kv("elapsed", elapsed(start.elapsed())),
        ]);
    }

    let found = up.load(Ordering::Relaxed);
    let msg = format!(
        "{found} host(s) up out of {} probed in {}",
        done.load(Ordering::Relaxed),
        elapsed(start.elapsed())
    );
    if found == 0 {
        emit.warn(msg);
    } else {
        emit.good(msg);
    }
    Ok(())
}

/// Which column carries what a kept run has to read back out of the rows
/// earlier runs wrote: the name, the hardware address and its vendor, none of
/// which a later probe is certain to learn again, and the date.
///
/// They are read back out of rows written by earlier runs, including runs of
/// earlier versions whose rows are one column short, so the positions are
/// named, not counted out at each use.
const NAME_COL: usize = 3;
const MAC_COL: usize = 4;
const VENDOR_COL: usize = 5;
const SEEN_COL: usize = 6;

/// What the date column says about a row this scan has just confirmed.
const SEEN_NOW: &str = "this scan";

/// What it says about a row there is nothing to date: one written by a
/// version of ntls that had no such column.
const UNDATED: &str = "earlier";

/// The rows a kept run should put a scan into the past: the ones it asked
/// about and did not hear from.
///
/// A row about an address this run never probed is not evidence of anything;
/// scanning one host says nothing about the rest of the subnet. A row
/// that only ever recorded silence has no date to move.
fn stale<'a>(
    prior: &'a [crate::core::Row],
    probed: &std::collections::HashSet<String>,
    answered: &std::collections::HashSet<String>,
) -> Vec<&'a crate::core::Row> {
    prior
        .iter()
        .filter(|row| row.status != Status::Down)
        .filter(|row| probed.contains(&row.target))
        .filter(|row| !answered.contains(row.identity()))
        .collect()
}

/// Puts a row one scan further into the past.
///
/// The count is kept in the cell and nowhere else, because the cell
/// is the only part of a row written to disk and read back, and
/// what lets a table survive being closed and reopened and still know how old
/// each line in it is.
fn age(cells: &mut Vec<String>) {
    // Read before the row is padded out. A row too short to have a date
    // column at all and a row whose date column is empty are two different
    // things, and padding first made them the same one.
    let dated = older(cells.get(SEEN_COL).map(String::as_str));
    if cells.len() <= SEEN_COL {
        cells.resize(SEEN_COL + 1, String::new());
    }
    cells[SEEN_COL] = dated;
}

fn older(current: Option<&str>) -> String {
    // A row with no date column comes from a version that had none, and
    // nothing in it says when it was written.
    let Some(current) = current else { return UNDATED.into() };
    let current = current.trim();
    let scans = if current.is_empty() {
        // A scan that keeps nothing dates nothing: it is all one moment, so
        // every row of it would say the same thing. The blank still means the
        // scan just before this one, and reading it as "no idea" is what the
        // first kept run did to every host that had gone quiet: it wrote
        // "earlier" over the lot of them, where it then stayed for good. That
        // run is the one a user makes first, its table having come from a scan
        // that was not keeping anything.
        0
    } else if let Some(n) = scans_ago(current) {
        n
    } else {
        // Anything else no version of this wrote, and guessing a number for it
        // would be inventing one.
        return UNDATED.into();
    };
    match scans + 1 {
        1 => "1 scan ago".into(),
        n => format!("{n} scans ago"),
    }
}

/// How many scans ago a date column says its row was last seen, where it says
/// anything countable at all.
fn scans_ago(seen: &str) -> Option<u32> {
    let seen = seen.trim();
    if seen == SEEN_NOW {
        return Some(0);
    }
    if seen == "1 scan ago" {
        return Some(1);
    }
    seen.strip_suffix(" scans ago").and_then(|n| n.parse::<u32>().ok())
}

/// What was in the table when this run started, for a run that is adding to
/// it and does not replace it.
///
/// Only the hosts that answered are worth remembering: a row saying an address
/// was silent says nothing about what lives there.
struct Seen {
    devices: HashMap<String, Vec<Device>>,
    /// The rows saying an address did not answer, which are about the address
    /// and not about anything living on it.
    silent: HashMap<String, String>,
}

/// One device an address has answered from, and the row that says so.
struct Device {
    /// Its hardware address, lowercased, or empty where none was learned.
    mac: String,
    /// What that row calls itself, so answering again rewrites that row rather
    /// than adding a second one beside it, including for the rows of runs
    /// made before this switch existed, which name themselves by their target.
    row: String,
    /// The cells of that row, so what an earlier run learned about the device
    /// can be written again by a run whose lookups came back with nothing.
    cells: Vec<String>,
}

/// What an address was, the last time anything answered on it.
enum Was {
    /// Nothing was there to compare with: a first run with this switch on.
    Nothing,
    /// The address had never answered before.
    New,
    /// The same device, as far as its hardware address says.
    Same,
    /// Somebody else, and who it was.
    Different(String),
}

impl Seen {
    fn of(rows: &[crate::core::Row]) -> Seen {
        let mut devices: HashMap<String, Vec<Device>> = HashMap::new();
        let mut silent: HashMap<String, String> = HashMap::new();
        for row in rows {
            if row.target.is_empty() {
                continue;
            }
            if row.status == Status::Down {
                silent.insert(row.target.clone(), row.identity().to_string());
                continue;
            }
            let mac = identity(row.cells.get(MAC_COL).map(String::as_str).unwrap_or_default());
            let entry = devices.entry(row.target.clone()).or_default();
            if !entry.iter().any(|d| d.mac == mac) {
                entry.push(Device {
                    mac,
                    row: row.identity().to_string(),
                    cells: row.cells.clone(),
                });
            }
        }
        Seen { devices, silent }
    }

    /// What the row for this answer should call itself.
    ///
    /// The device that was already there keeps its row; one that has taken
    /// over an address gets a row of its own beside it; and an address that
    /// was only ever recorded as silent gives that row up, since a row saying
    /// nothing answered is worth nothing once something has.
    fn key_for(&self, addr: &str, mac: &str) -> String {
        let now = identity(mac);
        if let Some(before) = self.devices.get(addr) {
            if let Some(same) = before.iter().find(|d| d.mac == now) {
                return same.row.clone();
            }
            // Where either side has no hardware address there is nothing to
            // tell the two apart with, so they are treated as one device: the
            // one the table saw last. Taking the first row instead took the
            // oldest, so a sweep that learned no MAC over an address that had
            // changed hands wrote the answer into the row of the device that
            // had been displaced, said that device was found by this scan, and
            // left the one actually living there to be dated a scan further
            // into the past.
            if now.is_empty() || before.iter().all(|d| d.mac.is_empty()) {
                if let Some(latest) = last_seen(before) {
                    return latest.row.clone();
                }
            }
        } else if let Some(row) = self.silent.get(addr) {
            return row.clone();
        }
        format!("up:{addr}|{now}")
    }

    /// Writes back into a fresh answer whatever the row it is about already
    /// said and this probe did not find out.
    ///
    /// A host answering says only that it is there. Its name comes from a
    /// reverse lookup that times out, is switched off, or was never asked
    /// for; its hardware from an ARP request that goes unanswered, is
    /// switched off, or cannot reach an address off this link. So a second
    /// scan of the same network routinely answers with those three columns
    /// empty, and the row it rewrote lost the name, the MAC and the vendor
    /// the run before had established. The MAC was the worse loss: it is the
    /// only record of what answered on that address, and with it blanked
    /// nothing could ever again notice the address changing hands.
    ///
    /// Only the row of this same device lends anything, which is what keeps
    /// a device that has just taken an address over from inheriting the name
    /// of the one it displaced.
    fn carry(&self, addr: &str, key: &str, cells: &mut Vec<String>) {
        let Some(before) = self.devices.get(addr).and_then(|ds| ds.iter().find(|d| d.row == key))
        else {
            return;
        };
        for col in [NAME_COL, MAC_COL, VENDOR_COL] {
            if cells.get(col).is_some_and(|c| !c.trim().is_empty()) {
                continue;
            }
            let Some(learned) = before.cells.get(col).filter(|c| !c.trim().is_empty()) else {
                continue;
            };
            if cells.len() <= col {
                cells.resize(col + 1, String::new());
            }
            cells[col] = learned.clone();
        }
    }

    /// What the row saying this address did not answer should call itself.
    fn silent_key(&self, addr: &str) -> String {
        self.silent.get(addr).cloned().unwrap_or_else(|| format!("down:{addr}"))
    }

    /// Whether a row about this address staying silent is worth having in a
    /// kept table.
    ///
    /// An address nothing has ever answered on needs one, or there is no line
    /// for it at all. An address that already has a row about a device does
    /// not: that row is the one the ageing pass marks and dates, and a row
    /// beside it saying only that the address timed out is the same news with
    /// the device's name, hardware and history left out. A row of that kind
    /// already in the table is still worth rewriting, since one left saying
    /// what an earlier run found is worse than one kept current.
    fn silence_worth_a_row(&self, addr: &str) -> bool {
        self.silent.contains_key(addr) || !self.devices.contains_key(addr)
    }

    fn is_empty(&self) -> bool {
        self.devices.is_empty() && self.silent.is_empty()
    }

    /// How many addresses it holds anything about.
    fn len(&self) -> usize {
        self.devices
            .keys()
            .chain(self.silent.keys())
            .collect::<std::collections::HashSet<_>>()
            .len()
    }

    fn compare(&self, addr: &str, mac: &str) -> Was {
        if self.is_empty() {
            return Was::Nothing;
        }
        let Some(before) = self.devices.get(addr) else { return Was::New };
        let now = identity(mac);
        if before.iter().any(|d| d.mac == now) {
            return Was::Same;
        }
        // An address whose hardware was never known cannot be said to have
        // changed hands, however different the two runs look.
        if now.is_empty() || before.iter().all(|d| d.mac.is_empty()) {
            return Was::Same;
        }
        Was::Different(
            before
                .iter()
                .filter(|d| !d.mac.is_empty())
                .map(|d| d.mac.clone())
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

/// Which of the devices recorded on one address the table saw most recently.
///
/// When an answer arrives with no hardware address in it, the dates the rows
/// carry are the only evidence there is about which of them it came from, and
/// the likeliest is the one that was there last. A row nothing ever dated is
/// taken as the older of the two: the dated one was dated by a run that was
/// keeping results, which is later than whatever left the other blank.
fn last_seen(devices: &[Device]) -> Option<&Device> {
    devices
        .iter()
        .min_by_key(|d| d.cells.get(SEEN_COL).and_then(|s| scans_ago(s)).unwrap_or(u32::MAX))
}

/// What makes two answers from one address the same device: its hardware
/// address, where we have one.
fn identity(mac: &str) -> String {
    mac.trim().to_lowercase()
}

fn hardware(mac: &str) -> String {
    let mac = mac.trim();
    if mac.is_empty() { "an unknown device".into() } else { mac.to_string() }
}

/// Gives a host a second chance before calling it down, which matters on
/// wireless links where a single lost packet is routine.
async fn probe_with_retries(
    cancel: &Cancel,
    prober: &Prober,
    addr: IpAddr,
    retries: usize,
) -> Result<Probe, ProbeError> {
    let mut last = ProbeError::Other("not attempted".into());
    for _ in 0..=retries {
        if cancel.is_cancelled() {
            return Err(ProbeError::Other("stopped".into()));
        }
        match prober.probe(cancel, addr).await {
            Ok(r) => return Ok(r),
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Row;

    fn row(target: &str, mac: &str, status: Status, key: Option<&str>) -> Row {
        Row {
            cells: crate::cells![target, "1 ms", "64", "", mac, "", SEEN_NOW],
            status,
            target: target.into(),
            note: None,
            key: key.map(str::to_string),
        }
    }

    #[test]
    fn a_host_that_answers_again_rewrites_the_row_it_already_had() {
        // Including the rows of runs made before any of this existed, which
        // name themselves by their target. Otherwise the first kept run
        // would double every line in the table.
        let seen = Seen::of(&[row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None)]);
        assert_eq!(seen.key_for("10.0.0.1", "AA:BB:CC:DD:EE:FF"), "10.0.0.1");
        assert!(matches!(seen.compare("10.0.0.1", "aa:bb:cc:dd:ee:ff"), Was::Same));
    }

    #[test]
    fn an_address_a_different_device_has_taken_over_gets_a_row_of_its_own() {
        let seen = Seen::of(&[row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None)]);
        let key = seen.key_for("10.0.0.1", "11:22:33:44:55:66");
        assert_ne!(key, "10.0.0.1", "the row for the device that was there is kept");
        assert!(matches!(seen.compare("10.0.0.1", "11:22:33:44:55:66"), Was::Different(_)));
        // And that second device keeps its own row on the run after that.
        let both = Seen::of(&[
            row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None),
            row("10.0.0.1", "11:22:33:44:55:66", Status::Up, Some(&key)),
        ]);
        assert_eq!(both.key_for("10.0.0.1", "11:22:33:44:55:66"), key);
        assert_eq!(both.key_for("10.0.0.1", "aa:bb:cc:dd:ee:ff"), "10.0.0.1");
    }

    #[test]
    fn hardware_nobody_knows_is_not_a_reason_to_claim_a_device_changed() {
        // ICMP alone learns no MAC, and two silent answers are not two
        // devices.
        let seen = Seen::of(&[row("10.0.0.1", "", Status::Up, None)]);
        assert!(matches!(seen.compare("10.0.0.1", ""), Was::Same));
        assert!(matches!(seen.compare("10.0.0.1", "aa:bb:cc:dd:ee:ff"), Was::Same));
        assert_eq!(seen.key_for("10.0.0.1", "aa:bb:cc:dd:ee:ff"), "10.0.0.1");
    }

    #[test]
    fn an_address_that_has_answered_is_not_described_by_the_row_saying_it_did_not() {
        // A row recording silence is worth nothing once something answers, so
        // it is the one row a kept run replaces.
        let seen = Seen::of(&[row("10.0.0.9", "", Status::Down, None)]);
        assert_eq!(seen.key_for("10.0.0.9", "aa:bb:cc:dd:ee:ff"), "10.0.0.9");
        assert_eq!(seen.silent_key("10.0.0.9"), "10.0.0.9");
        // And an address that was up keeps that row when it goes quiet.
        let up = Seen::of(&[row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None)]);
        assert_eq!(up.silent_key("10.0.0.1"), "down:10.0.0.1");
    }

    #[test]
    fn an_address_never_recorded_is_new_rather_than_changed() {
        let seen = Seen::of(&[row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None)]);
        assert!(matches!(seen.compare("10.0.0.7", ""), Was::New));
    }

    #[test]
    fn only_what_was_asked_about_and_stayed_quiet_is_put_into_the_past() {
        let prior = vec![
            row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None),
            row("10.0.0.2", "11:22:33:44:55:66", Status::Up, None),
            row("10.0.0.3", "22:33:44:55:66:77", Status::Up, None),
            row("10.0.0.9", "", Status::Down, None),
        ];
        // This run swept .1 to .3; .9 was in the table from a wider one.
        let probed =
            ["10.0.0.1", "10.0.0.2", "10.0.0.3"].into_iter().map(str::to_string).collect();
        let answered = std::collections::HashSet::from(["10.0.0.1".to_string()]);

        let quiet: Vec<&str> =
            stale(&prior, &probed, &answered).iter().map(|r| r.target.as_str()).collect();
        // .1 answered, .9 was never asked, and a row recording silence has no
        // date to move.
        assert_eq!(quiet, ["10.0.0.2", "10.0.0.3"]);
    }

    #[test]
    fn a_row_that_did_not_answer_gets_a_scan_older_rather_than_staying_current() {
        // The whole point of keeping earlier results is being able to tell
        // what is still there from what merely was, so the row has to say
        // which scan last found it.
        let mut cells = crate::cells!["10.0.0.1", "1 ms", "64", "", "", "", SEEN_NOW];
        age(&mut cells);
        assert_eq!(cells[SEEN_COL], "1 scan ago");
        age(&mut cells);
        assert_eq!(cells[SEEN_COL], "2 scans ago");
        for _ in 0..8 {
            age(&mut cells);
        }
        assert_eq!(cells[SEEN_COL], "10 scans ago");
    }

    #[test]
    fn a_row_left_undated_by_a_scan_that_kept_nothing_is_dated_from_that_scan() {
        // Seven cells with the date blank, which is every row on screen when
        // the switch is turned on: a scan that keeps nothing dates nothing,
        // because it is all one moment. The blank still means the scan just
        // before this one, and treating it as undatable marked every host that
        // had gone quiet by the first kept run "earlier" and kept it there.
        let mut cells = crate::cells!["10.0.0.1", "1 ms", "64", "host", "aa:bb", "Acme", ""];
        age(&mut cells);
        assert_eq!(cells[SEEN_COL], "1 scan ago");
        age(&mut cells);
        assert_eq!(cells[SEEN_COL], "2 scans ago");
    }

    #[test]
    fn a_row_written_before_there_was_a_date_column_gets_one() {
        // Six cells, from a version of ntls that had six columns. Ageing it
        // must not put the date in the vendor column, and must not claim a
        // number nothing recorded.
        let mut cells = crate::cells!["10.0.0.1", "1 ms", "64", "host", "aa:bb", "Acme"];
        age(&mut cells);
        assert_eq!(cells.len(), SEEN_COL + 1);
        assert_eq!(cells[5], "Acme");
        assert_eq!(cells[SEEN_COL], "earlier");
        // And stays there, since there is nothing to count from.
        age(&mut cells);
        assert_eq!(cells[SEEN_COL], "earlier");
    }

    /// The row a host already has is the only place what was learned about it
    /// is written down, so a scan that answers with less than the one before
    /// must not overwrite it with the difference.
    #[test]
    fn a_probe_that_learned_no_hardware_or_name_keeps_the_ones_an_earlier_run_did() {
        let mut before = row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None);
        before.cells[NAME_COL] = "printer.lan".into();
        before.cells[VENDOR_COL] = "Acme".into();
        let seen = Seen::of(&[before]);

        // A second scan over ICMP with the hardware lookup off and the
        // reverse lookup timed out: it knows the host is up and nothing else.
        let key = seen.key_for("10.0.0.1", "");
        let mut cells = crate::cells!["10.0.0.1", "1 ms", "64", "", "", "", SEEN_NOW];
        seen.carry("10.0.0.1", &key, &mut cells);
        assert_eq!(cells[NAME_COL], "printer.lan");
        assert_eq!(cells[MAC_COL], "aa:bb:cc:dd:ee:ff");
        assert_eq!(cells[VENDOR_COL], "Acme");
        // And still says this scan is the one that found it.
        assert_eq!(cells[SEEN_COL], SEEN_NOW);
    }

    #[test]
    fn the_hardware_a_scan_failed_to_learn_again_is_still_there_to_compare_against() {
        // The MAC in the row is the whole of the change-detection baseline.
        // Blanking it did not merely empty a column: every later scan of that
        // address then had nothing to tell a swapped device from the one that
        // had been there, and said Same forever.
        let seen = Seen::of(&[row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None)]);
        let key = seen.key_for("10.0.0.1", "");
        let mut cells = crate::cells!["10.0.0.1", "1 ms", "64", "", "", "", SEEN_NOW];
        seen.carry("10.0.0.1", &key, &mut cells);

        let after = Seen::of(&[Row {
            cells,
            status: Status::Up,
            target: "10.0.0.1".into(),
            note: None,
            key: Some(key),
        }]);
        assert!(matches!(after.compare("10.0.0.1", "11:22:33:44:55:66"), Was::Different(_)));
    }

    #[test]
    fn the_device_that_has_taken_an_address_over_inherits_nothing_from_the_one_before_it() {
        let mut before = row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None);
        before.cells[NAME_COL] = "printer.lan".into();
        before.cells[VENDOR_COL] = "Acme".into();
        let seen = Seen::of(&[before]);

        let key = seen.key_for("10.0.0.1", "11:22:33:44:55:66");
        let mut cells =
            crate::cells!["10.0.0.1", "1 ms", "64", "", "11:22:33:44:55:66", "", SEEN_NOW];
        seen.carry("10.0.0.1", &key, &mut cells);
        assert_eq!(cells[NAME_COL], "", "the name belonged to the device that was there");
        assert_eq!(cells[MAC_COL], "11:22:33:44:55:66");
        assert_eq!(cells[VENDOR_COL], "");
    }

    #[test]
    fn an_answer_that_names_no_device_goes_to_the_one_the_table_saw_last() {
        // An address that has changed hands has a row per device. A later
        // sweep with the hardware lookup off, or blind, or over ICMP to an
        // address off this link, learns no MAC and cannot say which of them
        // answered. Going to the first row went to the device that had been
        // displaced: the table then said the old device was back, dated the
        // one actually living there a scan further into the past, and handed
        // the answer the displaced device's hardware to be compared against
        // next time.
        let mut displaced = row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None);
        displaced.cells[SEEN_COL] = "3 scans ago".into();
        let current = row(
            "10.0.0.1",
            "11:22:33:44:55:66",
            Status::Up,
            Some("up:10.0.0.1|11:22:33:44:55:66"),
        );
        let seen = Seen::of(&[displaced, current]);

        let key = seen.key_for("10.0.0.1", "");
        assert_eq!(key, "up:10.0.0.1|11:22:33:44:55:66");
        let mut cells = crate::cells!["10.0.0.1", "1 ms", "64", "", "", "", SEEN_NOW];
        seen.carry("10.0.0.1", &key, &mut cells);
        assert_eq!(cells[MAC_COL], "11:22:33:44:55:66");
    }

    #[test]
    fn a_host_that_has_gone_quiet_is_marked_in_the_row_it_has_and_not_given_a_second_one() {
        // The row about the device is what the ageing pass marks and dates. A
        // row beside it saying only that the address timed out doubled every
        // host that comes and goes, and stayed in the table calling the
        // address silent for as long as it went on answering.
        let seen = Seen::of(&[row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None)]);
        assert!(!seen.silence_worth_a_row("10.0.0.1"));
        // An address nothing has ever answered on has no other row to be
        // marked in, so it still gets one.
        assert!(seen.silence_worth_a_row("10.0.0.2"));
        // And one already in the table is kept current rather than left
        // saying what an earlier run found.
        let both = Seen::of(&[
            row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None),
            row("10.0.0.1", "", Status::Down, Some("down:10.0.0.1")),
        ]);
        assert!(both.silence_worth_a_row("10.0.0.1"));
    }

    #[test]
    fn a_row_recording_silence_lends_nothing_to_the_answer_that_replaces_it() {
        // Why a host did not answer is written in the vendor column of its
        // row, so a host that has just answered must take nothing out of the
        // row it is replacing, or it would be labelled "timeout".
        let mut silent = row("10.0.0.9", "", Status::Down, None);
        silent.cells[VENDOR_COL] = "timeout".into();
        let seen = Seen::of(&[silent]);

        let key = seen.key_for("10.0.0.9", "aa:bb:cc:dd:ee:ff");
        let mut cells =
            crate::cells!["10.0.0.9", "1 ms", "64", "", "aa:bb:cc:dd:ee:ff", "", SEEN_NOW];
        seen.carry("10.0.0.9", &key, &mut cells);
        assert_eq!(cells[VENDOR_COL], "");
    }
}
