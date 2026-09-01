//! Lists a domain's subdomains from certificate transparency logs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::cells;
use crate::core::{
    Column, Emitter, Expand, Field, Opt, Role, Status, Tool, Validator, col, kv,
};
use crate::net::ct;
use crate::net::speedtest::format_bytes;

pub struct Subdomains;

impl Tool for Subdomains {
    fn id(&self) -> &'static str {
        "subdomains"
    }
    fn title(&self) -> &'static str {
        "Subdomains"
    }
    fn desc(&self) -> &'static str {
        "List subdomains from certificate transparency logs"
    }
    fn icon(&self) -> &'static str {
        "subdomains"
    }

    fn fields(&self) -> Vec<Field> {
        let sources: Vec<Opt> =
            ct::SOURCES.iter().map(|s| Opt::new(s.id(), s.id(), s.desc())).collect();

        vec![
            Field::text(
                "target",
                "Domain",
                "The domain to enumerate under. A pasted URL is reduced to its domain",
            )
            .placeholder("example.com")
            .role(Role::Target)
            .validate(Validator::CtDomain)
            .expand(Expand::CtDomain),
            Field::select("source", "Source", "Which log aggregator to ask", ct::Source::Auto.id(), sources),
            Field::boolean(
                "expired",
                "Include expired",
                "Keep names whose certificates have all lapsed; these are often forgotten hosts still answering",
                true,
            ),
            Field::boolean(
                "wildcards",
                "Include wildcards",
                "Keep *.example.com entries, which show where a wildcard certificate is in use",
                true,
            ),
            Field::text(
                "timeout",
                "Timeout",
                "How long to wait; a busy domain returns a lot of certificates and the logs are slow",
            )
            .default("60s")
            .validate(Validator::Duration),
        ]
    }

    fn columns(&self) -> Vec<Column> {
        vec![
            col("SUBDOMAIN", 40),
            col("CERTS", 5),
            col("FIRST SEEN", 10),
            col("LAST SEEN", 10),
            col("EXPIRES", 10),
            col("ISSUER", 0),
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
    let source = ct::Source::parse(&p.str("source"));

    // A pasted URL is reduced to the domain that will actually be queried, so
    // the log and the summary say what was asked instead of what was typed.
    let domain = ct::normalize_domain(&p.str("target")).map_err(anyhow::Error::msg)?;
    emit.info(format!("asking {} for certificates naming {domain}", source.id()));

    let query = ct::Query {
        domain: domain.clone(),
        include_expired: p.bool("expired"),
        include_wildcards: p.bool("wildcards"),
        timeout: p.dur("timeout", Duration::from_secs(60)),
    };

    // The fetch reports as it streams in. Repainting on every certificate
    // would be pure noise, so it is sampled.
    let last_mb = AtomicUsize::new(0);
    let last_certs = AtomicUsize::new(0);
    let progress = {
        let emit = emit.clone();
        move |bytes: u64, certs: usize| {
            // Report on a whole megabyte or every few hundred certificates,
            // whichever the source is producing.
            let mb = (bytes >> 20) as usize;
            let moved = bytes > 0 && mb != last_mb.swap(mb, Ordering::Relaxed);
            let counted = certs > 0 && certs >= last_certs.load(Ordering::Relaxed) + 250;
            if !moved && !counted {
                return;
            }
            if counted {
                last_certs.store(certs, Ordering::Relaxed);
            }
            // The total is unknown until the response ends, so this is the
            // stat bar instead of the progress bar: a rising count says more
            // than a bar stuck at zero.
            let mut line = vec![kv("downloaded", format_bytes(bytes))];
            let seen = last_certs.load(Ordering::Relaxed);
            if seen > 0 {
                line.push(kv("certs read", seen.to_string()));
            }
            emit.stats(line);
        }
    };

    let Some(result) = cancel.run(ct::subdomains(source, &query, &progress)).await else {
        // Stopping the run is not a failure worth reporting as one.
        return Ok(());
    };
    let res = result.map_err(anyhow::Error::msg)?;

    if res.names.is_empty() {
        emit.progress(1, 1);
        emit.warn(format!("no certificates found for {domain}"));
        return Ok(());
    }

    let (mut live, mut expired, mut wildcards) = (0usize, 0usize, 0usize);
    for (i, n) in res.names.iter().enumerate() {
        if cancel.is_cancelled() {
            return Ok(());
        }

        let status = if n.expired() {
            expired += 1;
            Status::Warn
        } else {
            live += 1;
            Status::Up
        };
        if n.wildcard {
            wildcards += 1;
        }

        // A wildcard is not a host, so what gets handed to the next tool is
        // the name it stands for.
        emit.row(
            status,
            n.host(),
            cells![
                n.name,
                n.certs,
                n.first_seen.format(),
                n.last_seen.format(),
                n.expires.format(),
                n.issuer
            ],
        );

        if i % 200 == 0 {
            emit.progress(i + 1, res.names.len());
        }
    }
    emit.progress(res.names.len(), res.names.len());

    emit.stats(vec![
        kv("names", res.names.len().to_string()),
        kv("live", live.to_string()),
        kv("expired", expired.to_string()),
        kv("wildcard", wildcards.to_string()),
        kv("certs", res.certs.to_string()),
        kv("source", res.source),
    ]);

    if res.truncated {
        emit.warn(format!("stopped at {} names; the domain has more", ct::MAX_NAMES));
    }
    if source == ct::Source::Auto && res.source != ct::Source::CrtSh.id() {
        emit.warn(format!("crt.sh did not answer; these results are from {}", res.source));
    }
    emit.good(format!("{} name(s) across {} certificate(s)", res.names.len(), res.certs));
    Ok(())
}
