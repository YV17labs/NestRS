//! File scaffolding: the transactional commit engine ([`transaction`]),
//! templated rendering ([`render`]), idempotent source edits ([`wiring`]) and
//! manifest edits ([`manifest`]).

mod manifest;
mod render;
mod transaction;
mod wiring;

pub(crate) use manifest::ManifestEdit;
pub(crate) use render::Renderer;
pub(crate) use transaction::{Scaffold, rel, rustfmt, rustfmt_source};
pub(crate) use wiring::{
    Transform, declares_expose, ensure_decl, ensure_expose_graphql, ensure_lines,
    ensure_module_imports,
};
