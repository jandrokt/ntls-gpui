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
                "auto uses your own subnet — press tab to expand it into the exact range, then edit it",
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
                "Resolve the MAC and vendor of hosts on this link — ICMP alone cannot tell you what a device is",
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
                "Add to what previous runs found instead of replacing it: a host that has since gone quiet keeps its row, and an address a different device has taken over gets a second one",
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
                            let cells = cells![addr, "-", "", "", "", failure_text(&e)];
                            if keep {
                                // One row per address for the silent ones, so
                                // a nightly sweep does not grow a row a night
                                // for everything that is not there.
                                emit.upsert_as(
                                    seen.silent_key(&addr.to_string()),
                                    Status::Down,
                                    addr.to_string(),
                                    cells,
                                );
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
                        let cells =
                            cells![addr, ms(r.rtt), r.ttl_text(), name, r.mac, r.vendor];
                        if keep {
                            answered.lock().await.insert(addr.to_string());
                            match seen.compare(&addr.to_string(), &r.mac) {
                                Was::Nothing => {}
                                Was::New => emit.good(format!("{addr} is new since the last run")),
                                Was::Same => {}
                                Was::Different(before) => emit.warn(format!(
                                    "{addr} now answers from {} — the row for {before} is kept",
                                    hardware(&r.mac)
                                )),
                            }
                            emit.upsert_as(
                                seen.key_for(&addr.to_string(), &r.mac),
                                status,
                                addr.to_string(),
                                cells,
                            );
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

    if keep && !seen.is_empty() {
        let answered = answered.lock().await;
        let quiet = seen.missing(&answered);
        if quiet > 0 {
            emit.info(format!(
                "{quiet} address(es) that answered before did not answer this time; their rows are left as they were"
            ));
        }
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

/// What was in the table when this run started, for a run that is adding to
/// it rather than replacing it.
///
/// Only the hosts that answered are worth remembering: a row saying an address
/// was silent says nothing about what lives there.
struct Seen {
    devices: HashMap<String, Vec<Device>>,
    /// The rows saying an address did not answer, which are about the address
    /// rather than about anything living on it.
    silent: HashMap<String, String>,
}

/// One device an address has answered from, and the row that says so.
struct Device {
    /// Its hardware address, lowercased, or empty where none was learned.
    mac: String,
    /// What that row calls itself, so answering again rewrites that row rather
    /// than adding a second one beside it — including for the rows of runs
    /// made before this switch existed, which name themselves by their target.
    row: String,
}

/// What an address was, the last time anything answered on it.
enum Was {
    /// Nothing was there to compare with — a first run with this switch on.
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
            let mac = identity(row.cells.get(4).map(String::as_str).unwrap_or_default());
            let entry = devices.entry(row.target.clone()).or_default();
            if !entry.iter().any(|d| d.mac == mac) {
                entry.push(Device { mac, row: row.identity().to_string() });
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
            // tell the two apart with, so they are treated as one device.
            if now.is_empty() || before.iter().all(|d| d.mac.is_empty()) {
                return before[0].row.clone();
            }
        } else if let Some(row) = self.silent.get(addr) {
            return row.clone();
        }
        format!("up:{addr}|{now}")
    }

    /// What the row saying this address did not answer should call itself.
    fn silent_key(&self, addr: &str) -> String {
        self.silent.get(addr).cloned().unwrap_or_else(|| format!("down:{addr}"))
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

    /// How many of the addresses that answered before did not answer now.
    fn missing(&self, answered: &std::collections::HashSet<String>) -> usize {
        self.devices.keys().filter(|addr| !answered.contains(*addr)).count()
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
            cells: crate::cells![target, "1 ms", "64", "", mac, ""],
            status,
            target: target.into(),
            note: None,
            key: key.map(str::to_string),
        }
    }

    #[test]
    fn a_host_that_answers_again_rewrites_the_row_it_already_had() {
        // Including the rows of runs made before any of this existed, which
        // name themselves by their target — otherwise the first kept run
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
    fn what_did_not_answer_this_time_is_counted_rather_than_removed() {
        let seen = Seen::of(&[
            row("10.0.0.1", "aa:bb:cc:dd:ee:ff", Status::Up, None),
            row("10.0.0.2", "11:22:33:44:55:66", Status::Up, None),
        ]);
        let answered = std::collections::HashSet::from(["10.0.0.1".to_string()]);
        assert_eq!(seen.missing(&answered), 1);
        assert!(matches!(seen.compare("10.0.0.7", ""), Was::New));
    }
}
