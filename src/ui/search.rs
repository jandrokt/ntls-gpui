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
            | Hit::Flow { context, .. } => context.clone(),
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
) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return registry.all().iter().cloned().map(Hit::Tool).collect();
    }

    let mut scored: Vec<(u8, usize, Hit)> = Vec::new();
    let mut order = 0usize;
    let mut push = |rank: u8, hit: Hit, order: &mut usize| {
        scored.push((rank, *order, hit));
        *order += 1;
    };

    // Tools, by what they are called and what they say they do.
    for tool in registry.all() {
        let (title, id, desc) =
            (tool.title().to_lowercase(), tool.id().to_lowercase(), tool.desc().to_lowercase());
        let rank = if title.starts_with(&needle) || id.starts_with(&needle) {
            0
        } else if title.contains(&needle) || id.contains(&needle) {
            2
        } else if desc.contains(&needle) {
            5
        } else {
            continue;
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

    scored.sort_by_key(|(rank, order, _)| (*rank, *order));
    scored.truncate(LIMIT);
    scored.into_iter().map(|(_, _, hit)| hit).collect()
}

/// How well one piece of text answers the query: exactly, at the start, or
/// somewhere in the middle.
fn rank_of(haystack: &str, needle: &str) -> Option<u8> {
    if haystack == needle {
        Some(0)
    } else if haystack.starts_with(needle) {
        Some(1)
    } else if haystack.contains(needle) {
        Some(3)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exact_match_beats_a_prefix_beats_a_mention() {
        assert!(rank_of("192.168.1.1", "192.168.1.1") < rank_of("192.168.1.10", "192.168.1.1"));
        assert!(rank_of("192.168.1.10", "192.168.1.1") < rank_of("a-192.168.1.1", "192.168.1.1"));
        assert_eq!(rank_of("nothing", "192.168.1.1"), None);
    }

    #[test]
    fn an_empty_query_lists_the_tools() {
        let hits = search("", &crate::tools::all(), &[], 0, &[]);
        assert_eq!(hits.len(), crate::tools::all().all().len());
        assert!(hits.iter().all(|h| matches!(h, Hit::Tool(_))));
    }

    #[test]
    fn a_tool_is_found_by_name_and_by_what_it_does() {
        let registry = crate::tools::all();
        let by_name = search("portscan", &registry, &[], 0, &[]);
        assert!(matches!(&by_name[0], Hit::Tool(t) if t.id() == "portscan"));

        // A word only its description uses still finds it, further down.
        let by_desc = search("subdomains", &registry, &[], 0, &[]);
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
            busy: None,
            scroll: gpui::ScrollHandle::new(),
            cursor: None,
        });

        let registry = crate::tools::all();
        let spaces = [ws];
        let by_title = search("Cabling", &registry, &spaces, 0, &[]);
        assert!(
            by_title.iter().any(|h| matches!(h, Hit::Doc { doc: 7, .. })),
            "a document is found by what it is called"
        );

        let by_step = search("Sweep", &registry, &spaces, 0, &[]);
        assert!(
            by_step.iter().any(|h| matches!(h, Hit::Flow { flow: 8, .. })),
            "a workflow is found by the run it starts"
        );

        let by_name = search("nightly", &registry, &spaces, 0, &[]);
        assert!(by_name.iter().any(|h| matches!(h, Hit::Flow { flow: 8, .. })));
    }

    #[test]
    fn a_workspace_is_found_by_name() {
        let registry = crate::tools::all();
        let spaces = [named("Office LAN"), named("Home")];

        let hits = search("office", &registry, &spaces, 0, &[]);
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
        assert!(search("qqqqzzzz", &registry, &[], 0, &[]).is_empty());
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
            let hits = search(query, &registry, &[], 0, std::slice::from_ref(&iface));
            assert!(
                hits.iter().any(|h| matches!(h, Hit::Iface { name, .. } if name == "en0")),
                "{query} did not find the interface"
            );
        }
    }
}
