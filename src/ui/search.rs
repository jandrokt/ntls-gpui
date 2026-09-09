//! Search across everything open.
//!
//! One query, matched against tools by name, workspaces by name, open tools by
//! name or target, interfaces by name, address or hardware address, and every
//! row of every set of results. Each hit carries enough to navigate straight
//! to it.

use std::sync::Arc;

use crate::core::{Registry, Status, Tool};
use crate::net::iface::Iface;

use super::workspace::Workspace;

/// How many answers are worth showing. Past this the list is a haystack of its
/// own.
const LIMIT: usize = 40;
/// How many rows one set of results may contribute, so a 65,000-port scan
/// cannot crowd out everything else.
const PER_JOB: usize = 6;

/// One thing the search found.
#[derive(Clone)]
pub enum Hit {
    /// A tool to open.
    Tool(Arc<dyn Tool>),
    /// A workspace to switch to.
    Workspace { index: usize, label: String, context: String },
    /// A tool that is already open, matched on its name or its target.
    Job { workspace: usize, job: usize, icon: &'static str, label: String, context: String },
    /// A row in a set of results.
    Row {
        workspace: usize,
        job: usize,
        row: usize,
        status: Status,
        label: String,
        context: String,
    },
    /// An interface on this machine.
    Iface { name: String, context: String },
    /// A document in a workspace.
    Doc { workspace: usize, doc: usize, label: String, context: String },
    /// A workflow in a workspace.
    Flow { workspace: usize, flow: usize, label: String, context: String },
    /// Something to do rather than somewhere to go.
    Command(Box<crate::ui::command::Command>),
    /// Whatever this was last time, offered again under its own heading.
    ///
    /// A wrapper and not a kind of its own: it is the same answer, and
    /// everything about it but which heading it sits under is the answer's.
    Recent(Box<Hit>),
    /// One of the pages that are not about a single tool.
    ///
    /// They hang off one small icon in the rail and nowhere else, so a
    /// workspace's variables could not be reached by name from the one box
    /// that is supposed to reach everything.
    Page { page: crate::ui::app::Page, context: String },
}

impl Hit {
    /// What the row reads as, for the icon column.
    pub fn icon(&self) -> &'static str {
        match self {
            Hit::Tool(t) => t.icon(),
            Hit::Workspace { .. } => "folder-open",
            Hit::Job { icon, .. } => icon,
            Hit::Row { .. } => "chevron-right",
            Hit::Iface { .. } => "globe",
            Hit::Doc { .. } => "note",
            Hit::Flow { .. } => "play",
            Hit::Page { page, .. } => page.icon(),
            Hit::Command(c) => c.icon,
            Hit::Recent(inner) => inner.icon(),
        }
    }

    /// What this is, for as long as the workspace holds it.
    ///
    /// Used to remember what was chosen last, so the box opens on what you
    /// actually use rather than on the tool list in registry order.
    pub fn key(&self) -> String {
        match self {
            Hit::Tool(t) => format!("tool:{}", t.id()),
            Hit::Workspace { label, .. } => format!("ws:{label}"),
            Hit::Job { workspace, job, .. } => format!("job:{workspace}:{job}"),
            Hit::Row { workspace, job, row, .. } => format!("row:{workspace}:{job}:{row}"),
            Hit::Iface { name, .. } => format!("iface:{name}"),
            Hit::Doc { workspace, doc, .. } => format!("doc:{workspace}:{doc}"),
            Hit::Flow { workspace, flow, .. } => format!("flow:{workspace}:{flow}"),
            Hit::Page { page, .. } => format!("page:{}", page.title()),
            Hit::Command(c) => format!("cmd:{}", c.title),
            Hit::Recent(inner) => inner.key(),
        }
    }

    /// The word for what kind of thing it is.
    pub fn kind(&self) -> &'static str {
        match self {
            Hit::Tool(_) => "Tool",
            Hit::Workspace { .. } => "Workspace",
            Hit::Job { .. } => "Open",
            Hit::Row { .. } => "Result",
            Hit::Iface { .. } => "Interface",
            Hit::Doc { .. } => "Document",
            Hit::Flow { .. } => "Workflow",
            Hit::Page { .. } => "Page",
            Hit::Command(_) => "Command",
            Hit::Recent(_) => "Recent",
        }
    }

    pub fn label(&self) -> String {
        match self {
            Hit::Tool(t) => t.title().to_string(),
            Hit::Workspace { label, .. }
            | Hit::Job { label, .. }
            | Hit::Row { label, .. }
            | Hit::Doc { label, .. }
            | Hit::Flow { label, .. } => label.clone(),
            Hit::Iface { name, .. } => name.clone(),
            Hit::Page { page, .. } => page.title().to_string(),
            Hit::Command(c) => c.title.clone(),
            Hit::Recent(inner) => inner.label(),
        }
    }

    pub fn context(&self) -> String {
        match self {
            Hit::Tool(t) => t.desc().to_string(),
            Hit::Workspace { context, .. }
            | Hit::Job { context, .. }
            | Hit::Row { context, .. }
            | Hit::Iface { context, .. }
            | Hit::Doc { context, .. }
            | Hit::Flow { context, .. }
            | Hit::Page { context, .. } => context.clone(),
            // Nothing: what a command does is its name, and the keys that
            // also do it are drawn as keys at the end of the row.
            Hit::Command(_) => String::new(),
            Hit::Recent(inner) => inner.context(),
        }
    }
}

/// Everything matching a query, best first.
///
/// Ranking is by how directly the match answers the question: something whose
/// name starts with what was typed before something that merely contains it,
/// and a thing you can open before a row inside one.
pub fn search(
    query: &str,
    registry: &Registry,
    workspaces: &[Workspace],
    active: usize,
    ifaces: &[Iface],
    commands: &[crate::ui::command::Command],
    recent: &[String],
) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    // A prefix that says "only the things that do something". With commands
    // in the list, `run` finds the tool, the runs called Run, and the command
    // that runs one; sometimes you already know which you meant.
    let (needle, only_commands) = match needle.strip_prefix('>') {
        Some(rest) => (rest.trim().to_string(), true),
        None => (needle, false),
    };

    if needle.is_empty() {
        // Nothing typed. What you last used, then the verbs, then the tools:
        // a list of every tool in registry order is the one thing nobody was
        // looking for.
        let mut out: Vec<Hit> = Vec::new();
        if !only_commands {
            out.extend(
                recent_hits(recent, registry, workspaces, active)
                    .into_iter()
                    .map(|hit| Hit::Recent(Box::new(hit))),
            );
            // The tools before the verbs: opening the box with nothing typed
            // is nearly always about starting something, and there are more
            // commands than would fit if they went first. `>` is what asks
            // for the verbs, and the line of keys along the bottom says so.
            out.extend(registry.all().iter().cloned().map(Hit::Tool));
        }
        out.extend(commands.iter().cloned().map(|c| Hit::Command(Box::new(c))));
        out.truncate(LIMIT);
        return out;
    }

    let mut scored: Vec<(u8, usize, Hit)> = Vec::new();
    let mut order = 0usize;
    // Something chosen a moment ago is very likely what is wanted again, so
    // it moves up a place. One place: enough to break a tie, not enough to
    // put a worse match above a better one.
    let bonus = |hit: &Hit| u8::from(!recent.iter().any(|k| *k == hit.key()));
    let mut push = |rank: u8, hit: Hit, order: &mut usize| {
        let rank = rank.saturating_add(bonus(&hit));
        scored.push((rank, *order, hit));
        *order += 1;
    };

    // The verbs, which is what a command bar is for.
    for command in commands {
        let title = command.title.to_lowercase();
        let Some(found) = hit_of(&title, &needle) else { continue };
        push(found.rank, Hit::Command(Box::new(command.clone())), &mut order);
    }
    if only_commands {
        scored.sort_by_key(|(rank, order, _)| (*rank, *order));
        scored.truncate(LIMIT);
        return scored.into_iter().map(|(_, _, hit)| hit).collect();
    }

    // Tools, by what they are called and what they say they do.
    //
    // Ranked the same way as everything else in the list. It used to have a
    // matcher of its own, so `psc` found the IP scan — whose id happens to
    // hold those three letters together — and not the port scan, which is
    // the one it reads as.
    for tool in registry.all() {
        let (title, id, desc) =
            (tool.title().to_lowercase(), tool.id().to_lowercase(), tool.desc().to_lowercase());
        let named = rank_of(&title, &needle).into_iter().chain(rank_of(&id, &needle)).min();
        // What it says it does is a weaker answer than what it is called.
        let rank = match named {
            Some(rank) => rank,
            None => match rank_of(&desc, &needle) {
                Some(rank) => rank.saturating_add(3),
                None => continue,
            },
        };
        push(rank, Hit::Tool(tool.clone()), &mut order);
    }

    // Workspaces, and what is inside them.
    for (index, ws) in workspaces.iter().enumerate() {
        let name = ws.name();
        let rank = match rank_of(&name.to_lowercase(), &needle) {
            Some(r) => r,
            None => continue,
        };
        let tools = ws.jobs.len();
        push(
            rank + 1,
            Hit::Workspace {
                index,
                label: name,
                context: format!(
                    "{tools} tool{}{}",
                    if tools == 1 { "" } else { "s" },
                    if index == active { " · open" } else { "" }
                ),
            },
            &mut order,
        );
    }

    for (index, ws) in workspaces.iter().enumerate() {
        // Naming the workspace is only worth the room when it is not the one
        // you are already in.
        let elsewhere = if index == active { String::new() } else { format!(" · {}", ws.name()) };

        for job in &ws.jobs {
            let (name, target) = (job.name(), job.target());
            // A tool matched by what it is pointed at is usually what was
            // being looked for, so it ranks with a name match and not
            // below it.
            if let Some(rank) =
                rank_of(&name.to_lowercase(), &needle).or(rank_of(&target.to_lowercase(), &needle))
            {
                push(
                    rank + 1,
                    Hit::Job {
                        workspace: index,
                        job: job.id,
                        icon: job.tool.icon(),
                        label: if target == "—" { name } else { format!("{name} · {target}") },
                        context: format!("{}{elsewhere}", job.state.label()),
                    },
                    &mut order,
                );
            }

            // And the rows themselves, so an address becomes
            // findable long after the scan that found it.
            let mut taken = 0;
            for (row_index, row) in job.rows.iter().enumerate() {
                if taken >= PER_JOB {
                    break;
                }
                let target_match = rank_of(&row.target.to_lowercase(), &needle);
                let cell_match = row
                    .cells
                    .iter()
                    .filter_map(|c| rank_of(&c.to_lowercase(), &needle))
                    .min();
                let Some(rank) = target_match.into_iter().chain(cell_match).min() else {
                    continue;
                };
                taken += 1;
                push(
                    rank + 3,
                    Hit::Row {
                        workspace: index,
                        job: job.id,
                        row: row_index,
                        status: row.status,
                        label: row.cells.join("  "),
                        context: format!("{}{elsewhere}", job.name()),
                    },
                    &mut order,
                );
            }
        }
    }

    // The documents and workflows a workspace holds, which are as much a part
    // of an investigation as the runs are: by what they are called, by their
    // file name, and by the line they open with.
    for (index, ws) in workspaces.iter().enumerate() {
        let elsewhere = if index == active { String::new() } else { format!(" · {}", ws.name()) };
        let data = crate::ui::notes::Data::of(ws);

        for doc in &ws.docs {
            let (title, summary) = (doc.title(&data), doc.summary(&data));
            let rank = rank_of(&title.to_lowercase(), &needle)
                .or_else(|| rank_of(&doc.stem.to_lowercase(), &needle))
                .or_else(|| summary.to_lowercase().contains(&needle).then_some(4))
                .or_else(|| doc.source.to_lowercase().contains(&needle).then_some(5));
            let Some(rank) = rank else { continue };
            push(
                rank + 1,
                Hit::Doc {
                    workspace: index,
                    doc: doc.id,
                    label: title,
                    context: if summary.is_empty() {
                        format!("document{elsewhere}")
                    } else {
                        format!("{summary}{elsewhere}")
                    },
                },
                &mut order,
            );
        }

        for flow in &ws.flows {
            let title = flow.title();
            let rank = rank_of(&title.to_lowercase(), &needle)
                .or_else(|| rank_of(&flow.stem.to_lowercase(), &needle))
                // The steps too: a workflow is worth finding by the run it
                // starts, the same way a scan is worth finding by its rows.
                .or_else(|| flow.source.to_lowercase().contains(&needle).then_some(4));
            let Some(rank) = rank else { continue };
            push(
                rank + 1,
                Hit::Flow {
                    workspace: index,
                    flow: flow.id,
                    label: title,
                    context: format!("{}{elsewhere}", flow.summary()),
                },
                &mut order,
            );
        }
    }

    // This machine.
    for iface in ifaces {
        let addresses: Vec<String> = iface.addrs.iter().map(|a| a.to_string()).collect();
        let rank = rank_of(&iface.name.to_lowercase(), &needle)
            .or_else(|| addresses.iter().filter_map(|a| rank_of(a, &needle)).min())
            .or_else(|| rank_of(&iface.mac.to_lowercase(), &needle));
        let Some(rank) = rank else { continue };
        push(
            rank + 1,
            Hit::Iface {
                name: iface.name.clone(),
                context: if addresses.is_empty() {
                    "no address".into()
                } else {
                    addresses.join(" · ")
                },
            },
            &mut order,
        );
    }

    // The pages that are not about one tool.
    for page in crate::ui::app::Page::ALL {
        let Some(rank) = rank_of(&page.title().to_lowercase(), &needle) else { continue };
        push(rank, Hit::Page { page, context: page.about().into() }, &mut order);
    }

    scored.sort_by_key(|(rank, order, _)| (*rank, *order));
    scored.truncate(LIMIT);
    scored.into_iter().map(|(_, _, hit)| hit).collect()
}

/// The things last chosen, in the order they were chosen, as hits again.
///
/// A key that no longer names anything is dropped: a tool that has been
/// closed, or a workspace that has gone.
fn recent_hits(
    recent: &[String],
    registry: &Registry,
    workspaces: &[Workspace],
    active: usize,
) -> Vec<Hit> {
    let mut out = Vec::new();
    for key in recent {
        if let Some(id) = key.strip_prefix("tool:") {
            if let Some(tool) = registry.get(id) {
                out.push(Hit::Tool(tool));
            }
            continue;
        }
        let Some(rest) = key.strip_prefix("job:") else { continue };
        let Some((ws, job)) = rest.split_once(':') else { continue };
        let (Ok(ws), Ok(job)) = (ws.parse::<usize>(), job.parse::<usize>()) else { continue };
        let Some(workspace) = workspaces.get(ws) else { continue };
        let Some(held) = workspace.jobs.iter().find(|j| j.id == job) else { continue };
        let (name, target) = (held.name(), held.target());
        let elsewhere =
            if ws == active { String::new() } else { format!(" \u{b7} {}", workspace.name()) };
        out.push(Hit::Job {
            workspace: ws,
            job,
            icon: held.tool.icon(),
            label: if target == "\u{2014}" { name } else { format!("{name} \u{b7} {target}") },
            context: format!("{}{elsewhere}", held.state.label()),
        });
    }
    out
}

/// The hits gathered under one heading each, best group first.
///
/// The group holding the single best match leads, and within a group the best
/// match leads. Relevance decides the order and the headings only say what
/// you are looking at, so a list mixing eight kinds is still scannable.
pub fn grouped(hits: Vec<Hit>) -> Vec<(&'static str, Vec<Hit>)> {
    let mut groups: Vec<(&'static str, Vec<Hit>)> = Vec::new();
    for hit in hits {
        let kind = hit.kind();
        match groups.iter_mut().find(|(name, _)| *name == kind) {
            Some((_, held)) => held.push(hit),
            None => groups.push((kind, vec![hit])),
        }
    }
    groups
}

/// How well one piece of text answers the query: exactly, at the start, or
/// somewhere in the middle.
fn rank_of(haystack: &str, needle: &str) -> Option<u8> {
    hit_of(haystack, needle).map(|m| m.rank)
}

/// How well a piece of text answers the query, and which of its characters
/// answered it.
///
/// The positions are what lets the row show its own reason for being there:
/// with a list mixing tools, addresses and the text of documents, a match
/// three words in is otherwise indistinguishable from no match at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// Lower is better.
    pub rank: u8,
    /// Byte offsets into the haystack of the characters that matched.
    pub at: Vec<usize>,
}

/// Matches `needle` against `haystack`, both already lowercased.
///
/// Five ways, best first: the whole of it, the start of it, the start of a
/// word in it, somewhere in it, and finally its letters in order but not
/// together, which is what lets `psc` find a port scan. The loose one is last
/// on purpose: it matches almost everything, so anything a stricter rule
/// caught should stay above it.
pub fn hit_of(haystack: &str, needle: &str) -> Option<Match> {
    if needle.is_empty() {
        return Some(Match { rank: 0, at: Vec::new() });
    }
    let run = |at: usize| (at..at + needle.len()).collect::<Vec<_>>();

    if haystack == needle {
        return Some(Match { rank: 0, at: run(0) });
    }
    if haystack.starts_with(needle) {
        return Some(Match { rank: 1, at: run(0) });
    }
    // The start of a word inside it: `scan` finding "Port scan", which reads
    // as a much better answer than a match in the middle of a word.
    if let Some(at) = word_starts(haystack).find(|at| haystack[*at..].starts_with(needle)) {
        return Some(Match { rank: 2, at: run(at) });
    }
    if let Some(at) = haystack.find(needle) {
        return Some(Match { rank: 3, at: run(at) });
    }
    scattered(haystack, needle).map(|at| Match { rank: 4, at })
}

/// Where each word of a piece of text begins.
fn word_starts(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.char_indices().filter_map(move |(at, ch)| {
        if at == 0 || !ch.is_alphanumeric() {
            return None;
        }
        let before = text[..at].chars().next_back()?;
        (!before.is_alphanumeric()).then_some(at)
    })
}

/// The needle's characters in order but not necessarily together.
///
/// Greedy from the left, which is wrong in the general case and right in this
/// one: names are short, and the leftmost run is the one a reader expects to
/// see picked out.
fn scattered(haystack: &str, needle: &str) -> Option<Vec<usize>> {
    let mut at = Vec::with_capacity(needle.len());
    let mut hay = haystack.char_indices();
    for want in needle.chars() {
        let found = hay.find(|(_, ch)| *ch == want)?;
        at.push(found.0);
    }
    Some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_is_found_by_the_letters_of_its_name_in_order() {
        // `psc` reads as "port scan". It used to find the IP scan instead,
        // whose id `ipscan` happens to hold those three letters together,
        // and miss the port scan entirely.
        let registry = crate::tools::all();
        let hits = search("psc", &registry, &[], 0, &[], &[], &[]);
        let tools: Vec<String> = hits
            .iter()
            .filter(|h| matches!(h, Hit::Tool(_)))
            .map(Hit::label)
            .collect();
        assert!(tools.iter().any(|t| t == "Port scan"), "{tools:?}");
    }

    #[test]
    fn the_five_ways_of_matching_are_ranked_best_first() {
        let rank = |hay: &str, needle: &str| hit_of(hay, needle).map(|m| m.rank);
        assert_eq!(rank("port scan", "port scan"), Some(0), "the whole of it");
        assert_eq!(rank("port scan", "port"), Some(1), "the start of it");
        assert_eq!(rank("port scan", "scan"), Some(2), "the start of a word");
        assert_eq!(rank("port scan", "rt s"), Some(3), "somewhere in it");
        assert_eq!(rank("port scan", "psc"), Some(4), "its letters in order");
        assert_eq!(rank("port scan", "zx"), None);

        // Strictest wins: a word-start match must not be reported as the
        // scattered one, or every search would rank everything the same.
        assert!(rank("port scan", "scan") < rank("port scan", "psc"));
    }

    #[test]
    fn a_match_says_which_characters_answered() {
        let found = hit_of("port scan", "scan").expect("a match");
        assert_eq!(found.at, vec![5, 6, 7, 8]);

        // Scattered, so the positions are not a run.
        let found = hit_of("port scan", "psc").expect("a match");
        assert_eq!(found.at, vec![0, 5, 6]);

        // And every position is the start of a character, so slicing there
        // is safe however the label is spelt.
        let label = "r\u{e9}seau local";
        let found = hit_of(label, "rl").expect("a match");
        for at in found.at {
            assert!(label.is_char_boundary(at), "{at} in {label:?}");
        }
    }

    #[test]
    fn the_letters_of_a_fuzzy_match_are_found_in_order() {
        // Not just "all present": `nacs` must not match by finding the `s`
        // before the `c`.
        assert!(hit_of("ip scan", "isc").is_some());
        assert!(hit_of("ip scan", "nsi").is_none());
    }

    #[test]
    fn answers_are_gathered_under_one_heading_each_best_group_first() {
        let registry = crate::tools::all();
        let ping = registry.get("ping").expect("ping");
        let hits = vec![
            Hit::Tool(ping.clone()),
            Hit::Iface { name: "en0".into(), context: String::new() },
            Hit::Tool(registry.get("ipscan").expect("ipscan")),
        ];
        let groups = grouped(hits);
        // Two groups, not three: the second tool joins the first one's.
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "Tool");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "Interface");
    }

    #[test]
    fn a_command_is_offered_by_name_and_carries_what_it_does() {
        use crate::ui::command::Now;

        let commands = crate::ui::command::all(&Now {
            workspaces: 2,
            workspace_name: "Duo".into(),
            ..Now::default()
        });
        let registry = crate::tools::all();
        let hits = search("light and dark", &registry, &[], 0, &[], &commands, &[]);
        let first = hits.first().expect("something to have matched");
        assert_eq!(first.kind(), "Command");
        assert!(first.label().contains("light and dark"), "{}", first.label());
        let keys = match first {
            Hit::Command(c) => c.keys,
            _ => None,
        };
        assert_eq!(keys, Some("\u{2318}D"), "the keys that also do it");
    }

    #[test]
    fn a_command_that_would_do_nothing_is_not_offered() {
        use crate::ui::command::Now;

        // Nothing is running and no tool is selected, so there is nothing to
        // stop and nothing to run: a list of entries that quietly do nothing
        // is worse than a shorter list.
        let idle = crate::ui::command::all(&Now::default());
        assert!(!idle.iter().any(|c| c.title.starts_with("Stop")), "{:?}", titles(&idle));
        assert!(!idle.iter().any(|c| c.title.starts_with("Run")), "{:?}", titles(&idle));

        let busy = crate::ui::command::all(&Now {
            job: Some(7),
            job_name: "Ping".into(),
            job_running: true,
            ..Now::default()
        });
        assert!(busy.iter().any(|c| c.title == "Stop Ping"), "{:?}", titles(&busy));
        assert!(!busy.iter().any(|c| c.title == "Run Ping"), "not while it runs");
    }

    #[test]
    fn only_one_workspace_means_nowhere_to_move_to() {
        use crate::ui::command::Now;

        let alone = crate::ui::command::all(&Now { workspaces: 1, ..Now::default() });
        assert!(!alone.iter().any(|c| c.title == "Next workspace"));
        let several = crate::ui::command::all(&Now { workspaces: 3, ..Now::default() });
        assert!(several.iter().any(|c| c.title == "Next workspace"));
    }

    #[test]
    fn the_prefix_narrows_the_list_to_things_that_do_something() {
        use crate::ui::command::Now;

        let commands = crate::ui::command::all(&Now::default());
        let registry = crate::tools::all();
        // "new" names a command and nothing else here, but with a workspace
        // full of runs it would name plenty; the prefix says which was meant.
        let scoped = search("> new", &registry, &[], 0, &[], &commands, &[]);
        assert!(!scoped.is_empty());
        assert!(scoped.iter().all(|h| h.kind() == "Command"), "{:?}", kinds(&scoped));

        // And with nothing after it, every command.
        let all = search(">", &registry, &[], 0, &[], &commands, &[]);
        assert_eq!(all.len(), commands.len().min(LIMIT));
        assert!(all.iter().all(|h| h.kind() == "Command"));
    }

    #[test]
    fn an_empty_box_offers_what_was_last_used_before_the_whole_tool_list() {
        use crate::ui::command::Now;

        let registry = crate::tools::all();
        let commands = crate::ui::command::all(&Now::default());
        let recent = vec!["tool:portscan".to_string()];
        let hits = search("", &registry, &[], 0, &[], &commands, &recent);

        let first = hits.first().expect("something to be offered");
        assert_eq!(first.kind(), "Recent");
        assert_eq!(first.label(), "Port scan");
        // A key that names nothing any more is dropped rather than drawn as
        // a row that cannot be taken.
        let gone = search("", &registry, &[], 0, &[], &commands, &["tool:nope".to_string()]);
        assert!(gone.first().is_some_and(|h| h.kind() != "Recent"));
    }

    #[test]
    fn something_used_a_moment_ago_comes_up_before_an_equal_match() {
        use crate::ui::command::Now;

        let registry = crate::tools::all();
        let commands = crate::ui::command::all(&Now::default());
        let plain = search("scan", &registry, &[], 0, &[], &commands, &[]);
        let first = plain.first().expect("a match").label();

        // Whichever came second, having just been used, comes first.
        let second = plain.get(1).expect("two matches").clone();
        let hits = search("scan", &registry, &[], 0, &[], &commands, &[second.key()]);
        assert_eq!(hits.first().expect("a match").label(), second.label());
        assert_ne!(second.label(), first, "the fixture needs two different answers");
    }

    fn titles(commands: &[crate::ui::command::Command]) -> Vec<&str> {
        commands.iter().map(|c| c.title.as_str()).collect()
    }

    fn kinds(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(Hit::kind).collect()
    }

    #[test]
    fn an_exact_match_beats_a_prefix_beats_a_mention() {
        assert!(rank_of("192.168.1.1", "192.168.1.1") < rank_of("192.168.1.10", "192.168.1.1"));
        assert!(rank_of("192.168.1.10", "192.168.1.1") < rank_of("a-192.168.1.1", "192.168.1.1"));
        assert_eq!(rank_of("nothing", "192.168.1.1"), None);
    }

    #[test]
    fn an_empty_query_lists_the_tools() {
        let hits = search("", &crate::tools::all(), &[], 0, &[], &[], &[]);
        assert_eq!(hits.len(), crate::tools::all().all().len());
        assert!(hits.iter().all(|h| matches!(h, Hit::Tool(_))));
    }

    #[test]
    fn a_tool_is_found_by_name_and_by_what_it_does() {
        let registry = crate::tools::all();
        let by_name = search("portscan", &registry, &[], 0, &[], &[], &[]);
        assert!(matches!(&by_name[0], Hit::Tool(t) if t.id() == "portscan"));

        // A word only its description uses still finds it, further down.
        let by_desc = search("subdomains", &registry, &[], 0, &[], &[], &[]);
        assert!(by_desc.iter().any(|h| matches!(h, Hit::Tool(t) if t.id() == "subdomains")));
    }

    /// An empty workspace with a name. Rows and tools need a GPUI context to
    /// build, so what they contribute is covered by [`rank_of`] and by the
    /// live search in the running program.
    fn named(name: &str) -> Workspace {
        let mut ws = Workspace::new(1, std::path::PathBuf::from("/tmp/ntls-search-test"));
        ws.name_override = Some(name.to_string());
        ws
    }

    #[test]
    fn a_document_and_a_workflow_are_found_like_anything_else() {
        // They are as much a part of an investigation as the runs are, and
        // were the one thing the search could not reach.
        let mut ws = named("Office LAN");
        ws.docs.push(crate::ui::notes::Doc {
            id: 7,
            stem: "report".into(),
            path: std::path::PathBuf::from("/tmp/ntls-search-test/report.md"),
            source: "# Cabling report\n\nWhat the survey found.".into(),
            open: false,
            folder: None,
            seen: None,
            scroll: gpui::ScrollHandle::new(),
            preview: crate::ui::notes::PREVIEW,
        });
        ws.flows.push(crate::ui::flows::Sheet {
            id: 8,
            stem: "nightly".into(),
            path: std::path::PathBuf::from("/tmp/ntls-search-test/nightly.flow"),
            source: "# Nightly check\nrun \"Sweep\"\n".into(),
            open: false,
            folder: None,
            seen: None,
            trail: Vec::new(),
            machine: None,
            printed: Vec::new(),
            log_open: false,
            bindings: Vec::new(),
            busy: None,
            scroll: gpui::ScrollHandle::new(),
            cursor: None,
        });

        let registry = crate::tools::all();
        let spaces = [ws];
        let by_title = search("Cabling", &registry, &spaces, 0, &[], &[], &[]);
        assert!(
            by_title.iter().any(|h| matches!(h, Hit::Doc { doc: 7, .. })),
            "a document is found by what it is called"
        );

        let by_step = search("Sweep", &registry, &spaces, 0, &[], &[], &[]);
        assert!(
            by_step.iter().any(|h| matches!(h, Hit::Flow { flow: 8, .. })),
            "a workflow is found by the run it starts"
        );

        let by_name = search("nightly", &registry, &spaces, 0, &[], &[], &[]);
        assert!(by_name.iter().any(|h| matches!(h, Hit::Flow { flow: 8, .. })));
    }

    #[test]
    fn a_workspace_is_found_by_name() {
        let registry = crate::tools::all();
        let spaces = [named("Office LAN"), named("Home")];

        let hits = search("office", &registry, &spaces, 0, &[], &[], &[]);
        assert!(
            matches!(&hits[0], Hit::Workspace { label, .. } if label == "Office LAN"),
            "the workspace should be the first answer"
        );

        // And the one you are already in says so.
        assert!(hits[0].context().contains("open"));
    }

    #[test]
    fn a_query_that_matches_nothing_finds_nothing() {
        let registry = crate::tools::all();
        assert!(search("qqqqzzzz", &registry, &[], 0, &[], &[], &[]).is_empty());
    }

    #[test]
    fn an_interface_is_found_by_name_or_by_address() {
        let iface = Iface {
            name: "en0".into(),
            addrs: vec![crate::net::target::parse_prefix("192.168.1.5/24").expect("a prefix")],
            mac: "aa:bb:cc:dd:ee:ff".into(),
            default: true,
            virtual_: false,
        };
        let registry = crate::tools::all();

        for query in ["en0", "192.168.1.5", "aa:bb"] {
            let hits = search(query, &registry, &[], 0, std::slice::from_ref(&iface), &[], &[]);
            assert!(
                hits.iter().any(|h| matches!(h, Hit::Iface { name, .. } if name == "en0")),
                "{query} did not find the interface"
            );
        }
    }
}
