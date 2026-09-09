//! A small expression language, used by documents and workflows.
//!
//! It is deliberately not general: there are no loops, no assignment and no
//! definitions. An expression reads what the tools in a workspace found, does
//! arithmetic on it, and chooses between answers. That is all a document needs
//! and all a workflow's conditions need.

pub mod eval;
pub mod lex;
pub mod parse;
pub mod value;

pub use eval::{Source, run};
pub use value::{Table, Value};
