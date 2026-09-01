//! Fetching content from the web, with no per-site rules.
//!
//! [`resolve`] turns a URL into a list of plain HTTP URLs; [`fetch`] transfers
//! them. Neither contains a rule about a particular site. Resolution defers to
//! the extractors that already exist (`yt-dlp` and `gallery-dl`, several
//! thousand sites between them) and otherwise parses the page the way a
//! browser would, following markup that file hosts publish anyway. Support for
//! a new site therefore comes from updating the extractors, not from changes
//! here.

pub mod external;
pub mod fetch;
pub mod resolve;
pub mod names;

pub use resolve::Asset;
