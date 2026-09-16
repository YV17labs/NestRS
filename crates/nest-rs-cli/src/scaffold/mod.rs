//! File scaffolding: the transactional commit engine ([`transaction`]),
//! templated rendering ([`render`]), and idempotent source edits ([`wiring`]).

mod render;
mod transaction;
mod wiring;

pub(crate) use render::Renderer;
pub(crate) use transaction::{Scaffold, rustfmt};
pub(crate) use wiring::{
    Transform, ensure_decl, ensure_expose_graphql, ensure_lines, ensure_module_imports,
};
