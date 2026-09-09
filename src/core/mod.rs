//! The contract between the tools and the interface: fields, events, and the
//! registry that lists what is available.

pub mod event;
pub mod field;
pub mod tool;

pub use event::{Answer, Emitter, Event, Kv, Level, Row, Status, kv};
pub use field::{Expand, Field, FieldKind, Opt, Params, Role, Validator, VisibleIf};
pub use tool::{Cancel, Cells, Column, Registry, Run, Tool, bar, col};
