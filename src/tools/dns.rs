//! Resolves a name or an address against any server, any record type.

use std::net::IpAddr;
use std::time::Duration;

use crate::cells;
use crate::core::{
    Column, Emitter, Expand, Field, Opt, Role, Status, Tool, Validator, col, kv,
};
use crate::net::dns;
use crate::tools::stats::ms;

pub struct Dns;

/// Asks for the record that fits the query, so the user does not have to choose:
/// a PTR for an address, A and AAAA for a name.
const AUTO_TYPE: &str = "auto";

impl Tool for Dns {
    fn id(&self) -> &'static str {
        "dns"
    }
    fn title(&self) -> &'static str {
        "DNS lookup"
    }
    fn desc(&self) -> &'static str {
        "Resolve names and addresses against any server or record type"
    }
    fn icon(&self) -> &'static str {
        "dns"
    }

    fn fields(&self) -> Vec<Field> {
        let mut opts =
            vec![Opt::new(AUTO_TYPE, "auto", "A and AAAA for a name, PTR for an address")];
        for t in dns::TYPES {
            opts.push(Opt { value: t.to_string(), label: t.to_string(), desc: record_desc(t).into() });
        }

        vec![
            Field::text(
                "target",
                "Name or IP",
                "A hostname to look up, or an address to look up backwards",
            )
            .placeholder("example.com, 1.1.1.1")
            .role(Role::Target)
            .validate(Validator::Required),
            Field::select("type", "Record", "Which record type to ask for", AUTO_TYPE, opts),
            Field::text(
                "server",
                "Server",
                "Which resolver to ask; empty uses the system one. Compare two to spot stale or filtered answers",
            )
            .placeholder(&system_resolver_hint())
            .expand(Expand::Resolver),
            Field::text("timeout", "Timeout", "How long to wait for the server to answer")
                .default("4s")
                .validate(Validator::Duration).advanced(),
            Field::boolean(
                "extra",
                "Show all sections",
                "Include the authority and additional sections, not just the answer",
                false,
            ),
            Field::boolean(
                "keep",
                "Keep earlier answers",
                "Add to what previous lookups returned instead of replacing it: an answer that has changed is shown beside the one it replaced, and a record that has gone keeps its row",
                false,
            )
            .role(Role::Keep),
        ]
    }

    fn columns(&self) -> Vec<Column> {
        vec![col("NAME", 34), col("TYPE", 6), col("TTL", 8), col("SECTION", 10), col("VALUE", 0)]
    }

    fn run<'a>(
        &'a self,
        r: crate::core::Run,
        emit: Emitter,
    ) -> crate::core::tool::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { run(r, emit).await })
    }
}

fn record_desc(t: &str) -> &'static str {
    match t {
        "A" => "IPv4 address",
        "AAAA" => "IPv6 address",
        "CNAME" => "canonical name this one aliases",
        "MX" => "mail exchangers",
        "NS" => "authoritative nameservers",
        "TXT" => "text records, where SPF and verification tokens live",
        "SOA" => "zone authority and serial",
        "SRV" => "service location",
        "PTR" => "the name an address points back to",
        _ => "",
    }
}

fn system_resolver_hint() -> String {
    let r = dns::system_resolvers();
    if r.is_empty() { "1.1.1.1".into() } else { format!("system: {}", r.join(", ")) }
}

struct Planned {
    name: String,
    display: String,
    record_type: String,
}

/// Turns the user's input into the queries to send, treating an address as a
/// reverse lookup without being told.
fn plan_queries(target: &str, record_type: &str) -> Result<Vec<Planned>, String> {
    let target = target.trim();
    if target.is_empty() {
        return Err("nothing to look up".into());
    }

    if let Ok(addr) = target.parse::<IpAddr>() {
        let arpa = dns::reverse_arpa_name(addr);
        // An explicit type against an address still asks about the .arpa name.
        let rt = if record_type == AUTO_TYPE { "PTR" } else { record_type };
        return Ok(vec![Planned {
            name: arpa,
            display: target.into(),
            record_type: rt.into(),
        }]);
    }

    if record_type == AUTO_TYPE {
        return Ok(["A", "AAAA"]
            .into_iter()
            .map(|rt| Planned {
                name: target.into(),
                display: target.into(),
                record_type: rt.into(),
            })
            .collect());
    }
    Ok(vec![Planned {
        name: target.into(),
        display: target.into(),
        record_type: record_type.into(),
    }])
}

async fn run(r: crate::core::Run, emit: Emitter) -> anyhow::Result<()> {
    let (cancel, p) = (r.cancel.clone(), r.params.clone());
    let server = p.str("server");
    let timeout = p.dur("timeout", Duration::from_secs(4));
    let show_all = p.bool("extra");
    let queries = plan_queries(&p.str("target"), &p.str("type")).map_err(anyhow::Error::msg)?;

    // Keeping means the answers of earlier lookups are still in the table. An
    // answer that comes back the same rewrites its own row, one that has
    // changed joins it, and a record that has stopped being returned stays:
    // which is the whole of what a document says about a zone over time.
    let keep = r.keep;
    let mut before = Answered::of(&r.prior);
    if keep && !before.is_empty() {
        emit.info(format!(
            "keeping {} answer(s) from earlier lookups; nothing already in the table is removed",
            before.len()
        ));
    }
    let mut fresh = 0usize;
    let mut changed = 0usize;

    let mut answers = 0usize;
    for (i, q) in queries.iter().enumerate() {
        if cancel.is_cancelled() {
            return Ok(());
        }
        emit.progress(i, queries.len());

        let res = match cancel.run(dns::query(&server, &q.name, &q.record_type, timeout)).await {
            None => return Ok(()),
            Some(Err(e)) => {
                emit.err(format!("{} {}: {e}", q.record_type, q.display));
                continue;
            }
            Some(Ok(res)) => res,
        };

        emit.stats(vec![
            kv("server", &res.server),
            kv("status", &res.rcode),
            kv("time", ms(res.elapsed)),
        ]);
        if res.truncated {
            emit.info("the answer did not fit in a datagram and was re-fetched over TCP");
        }

        let mut found = 0usize;
        for r in &res.records {
            let is_answer = r.section == "answer";
            if !is_answer && !show_all {
                continue;
            }
            if is_answer {
                found += 1;
            }
            let status = if is_answer { Status::Up } else { Status::Info };
            let cells = cells![r.name, r.rtype, format_ttl(r.ttl), r.section, r.value];
            if keep {
                let key = record_key(&cells);
                // An answer whose row cannot be rewritten is still an answer
                // that was already there, so whether it is new is asked of
                // the table and not of whether a row was found for it.
                let known = before.has(&key);
                let claimed = before.claim_row(&key);
                match claimed {
                    Some(row) => emit.upsert_as(row, status, handoff_target(r), cells),
                    None => {
                        if !before.is_empty() && !known {
                            fresh += 1;
                            if before.knows(&r.name, &r.rtype) {
                                changed += 1;
                            }
                        }
                        emit.upsert_as(key, status, handoff_target(r), cells);
                    }
                }
            } else {
                emit.row(status, handoff_target(r), cells);
            }
        }
        answers += found;

        match res.rcode.as_str() {
            "NXDOMAIN" => emit.warn(format!("{} does not exist", q.display)),
            "NOERROR" if found == 0 => {
                emit.info(format!("no {} record for {}", q.record_type, q.display))
            }
            "NOERROR" => {}
            other => emit.warn(format!("{} {} returned {other}", q.record_type, q.display)),
        }
    }

    emit.progress(queries.len(), queries.len());
    if keep && !before.is_empty() {
        if changed > 0 {
            emit.warn(format!(
                "{changed} answer(s) differ from what was returned before; both are in the table"
            ));
        }
        emit.info(format!(
            "{fresh} answer(s) not seen before, {} kept from earlier lookups",
            before.len()
        ));
    }
    if answers == 0 {
        emit.warn("no answers");
    } else {
        emit.good(format!("{answers} answer(s)"));
    }
    Ok(())
}

/// What earlier lookups of this job put in the table, for a lookup that is
/// adding to it, not replacing it.
struct Answered {
    /// Every answer already there, by what makes it that answer, against what
    /// the row holding it calls itself, which for the rows of lookups made
    /// before this switch existed is their target.
    rows: std::collections::HashMap<String, String>,
    /// The name and type of every answer already there, so an answer that is
    /// new can be told from one that has changed.
    subjects: std::collections::HashSet<(String, String)>,
    /// Which answer has taken each of those rows over during this lookup,
    /// because a row can only be rewritten by one of them.
    claimed: std::collections::HashMap<String, String>,
}

impl Answered {
    fn of(rows: &[crate::core::Row]) -> Answered {
        let mut out = Answered {
            rows: std::collections::HashMap::new(),
            subjects: std::collections::HashSet::new(),
            claimed: std::collections::HashMap::new(),
        };
        for row in rows {
            out.rows.insert(record_key(&row.cells), row.identity().to_string());
            let (name, rtype) = (row.cells.first(), row.cells.get(1));
            if let (Some(name), Some(rtype)) = (name, rtype) {
                out.subjects.insert((name.to_lowercase(), rtype.to_uppercase()));
            }
        }
        out
    }

    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether this exact answer is already in the table, whether or not
    /// there is a row of it this lookup can address.
    fn has(&self, key: &str) -> bool {
        self.rows.contains_key(key)
    }

    /// The row this answer should rewrite, or none, meaning it takes a row of
    /// its own.
    ///
    /// A row is only ever found by the identity it goes by, and the rows of
    /// lookups made before this switch existed go by their hand-off target,
    /// which several records of one name share: every TXT record of a name
    /// hands off that name, and two mail exchangers on one host hand off that
    /// host. Giving a second answer the identity a first had just taken sent
    /// it to the same row, so a name with three TXT records came back as its
    /// last record three times over with the other two nowhere in the table.
    /// An identity is therefore taken once, and the answers behind it get
    /// rows of their own; the same answer arriving twice in one lookup, as a
    /// CNAME does when auto asks both A and AAAA, still lands on the row it
    /// took the first time.
    fn claim_row(&mut self, key: &str) -> Option<String> {
        let row = self.rows.get(key)?.clone();
        let owner = self.claimed.entry(row.clone()).or_insert_with(|| key.to_string());
        if owner.as_str() == key { Some(row) } else { None }
    }

    /// Whether this name was already answering for this type, which makes a
    /// new row a change instead of an addition.
    fn knows(&self, name: &str, rtype: &str) -> bool {
        self.subjects.contains(&(name.to_lowercase(), rtype.to_uppercase()))
    }
}

/// What makes two answers the same answer: everything about them except how
/// long they have left to live.
fn record_key(cells: &[String]) -> String {
    let at = |i: usize| cells.get(i).map(String::as_str).unwrap_or_default();
    format!("{}|{}|{}|{}", at(0).to_lowercase(), at(1).to_uppercase(), at(3), at(4))
}

/// What pressing enter on a row should carry to the next tool: an address
/// where the record names one, otherwise the name itself.
fn handoff_target(r: &dns::Record) -> String {
    match r.rtype.as_str() {
        "A" | "AAAA" | "CNAME" | "NS" | "PTR" => r.value.clone(),
        "MX" => r.value.split_once(' ').map(|(_, h)| h.to_string()).unwrap_or_else(|| r.name.clone()),
        _ => r.name.clone(),
    }
}

fn format_ttl(ttl: u32) -> String {
    match ttl {
        0 => "0".into(),
        t if t < 60 => format!("{t}s"),
        t if t < 3600 => format!("{}m", t / 60),
        t if t < 172_800 => format!("{}h", t / 3600),
        t => format!("{}d", t / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells;
    use crate::core::Row;

    fn row(name: &str, rtype: &str, ttl: &str, value: &str, key: Option<&str>) -> Row {
        Row {
            cells: cells![name, rtype, ttl, "answer", value],
            status: Status::Up,
            target: value.into(),
            note: None,
            key: key.map(str::to_string),
        }
    }

    #[test]
    fn the_same_answer_returned_again_rewrites_the_row_it_already_had() {
        // Only its time to live has moved, and a table with the same A record
        // in it forty times is not a record of anything.
        let mut before = Answered::of(&[row("example.com", "A", "5m", "1.2.3.4", None)]);
        let again = cells!["example.com", "A", "1m", "answer", "1.2.3.4"];
        assert_eq!(before.claim_row(&record_key(&again)).as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn an_answer_that_has_changed_joins_the_one_it_replaced() {
        let mut before = Answered::of(&[row("example.com", "A", "5m", "1.2.3.4", None)]);
        let moved = cells!["example.com", "A", "5m", "answer", "5.6.7.8"];
        assert_eq!(before.claim_row(&record_key(&moved)), None, "it is a row of its own");
        // And it is a change instead of an addition, because the name was
        // already answering for this type.
        assert!(before.knows("example.com", "A"));
        assert!(!before.knows("example.com", "MX"));
    }

    #[test]
    fn what_makes_two_answers_the_same_answer_ignores_how_it_is_written() {
        let a = cells!["Example.com", "a", "5m", "answer", "1.2.3.4"];
        let b = cells!["example.com", "A", "30s", "answer", "1.2.3.4"];
        assert_eq!(record_key(&a), record_key(&b));
        // The section is part of it: an authority record is not an answer.
        let authority = cells!["example.com", "A", "5m", "authority", "1.2.3.4"];
        assert_ne!(record_key(&a), record_key(&authority));
    }

    /// A row of a lookup made with the switch off, which goes by its hand-off
    /// target and not by which answer it holds.
    fn untagged(rtype: &str, value: &str, target: &str) -> Row {
        Row {
            cells: cells!["example.com", rtype, "5m", "answer", value],
            status: Status::Up,
            target: target.into(),
            note: None,
            key: None,
        }
    }

    #[test]
    fn two_answers_that_hand_off_the_same_thing_do_not_take_one_another_s_row() {
        // Every TXT record of a name hands off that name, so the rows of a
        // lookup made before the switch was on all go by the same identity.
        // The second answer used to be sent to the row the first had just
        // taken, which left the table holding one record twice over and the
        // other not at all.
        let mut before = Answered::of(&[
            untagged("TXT", "v=spf1 -all", "example.com"),
            untagged("TXT", "verify=x", "example.com"),
        ]);
        let spf = record_key(&["example.com".into(), "TXT".into(), "1m".into(), "answer".into(), "v=spf1 -all".into()]);
        let token = record_key(&["example.com".into(), "TXT".into(), "1m".into(), "answer".into(), "verify=x".into()]);
        assert_eq!(before.claim_row(&spf).as_deref(), Some("example.com"));
        assert_eq!(before.claim_row(&token), None, "it gets a row of its own instead");
        // And it is neither new nor changed: it was in the table all along.
        assert!(before.has(&token));
    }

    #[test]
    fn the_same_answer_reaching_the_table_twice_in_one_lookup_keeps_to_one_row() {
        // Auto asks for A and AAAA, and an alias answers both with the same
        // CNAME record, so that record is written twice in one lookup and
        // must go to the row it took the first time rather than to a second
        // row beside it.
        let mut before =
            Answered::of(&[untagged("CNAME", "target.example.net", "target.example.net")]);
        let cname =
            record_key(&["example.com".into(), "CNAME".into(), "1m".into(), "answer".into(), "target.example.net".into()]);
        assert_eq!(before.claim_row(&cname).as_deref(), Some("target.example.net"));
        assert_eq!(before.claim_row(&cname).as_deref(), Some("target.example.net"));
    }
}
