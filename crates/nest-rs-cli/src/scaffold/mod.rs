//! File scaffolding: the transactional commit engine ([`transaction`]),
//! templated rendering ([`render`]), and idempotent source edits ([`wiring`]).

mod render;
mod transaction;
mod wiring;

pub use render::Renderer;
pub use transaction::{Scaffold, rustfmt};
pub use wiring::{Transform, ensure_decl, ensure_lines, ensure_module_import};
