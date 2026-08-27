//! Markdown notes kept in a workspace, with the tools' own figures in them.
//!
//! A workspace already holds what a scan found; what it did not hold is what
//! you concluded from it. A document is a `.md` file beside the tool files,
//! written the way any markdown is, with one addition: anything inside `{{ }}`
//! is an expression evaluated against the tools in the same workspace.
//!
//! ```text
//! Average latency is {{ Router.rtt.avg().fixed(1) }} ms over
//! {{ Router.rows }} probes — {{ if Router.rtt.avg() > 50 then "slow" else "normal" }}.
//! ```
//!
//! The file is the source; the figures are worked out fresh every time it is
//! shown, so a note written last week describes this week's scan.

pub mod render;
pub mod template;

pub use render::{Block, Span, blocks};
pub use template::expand;
