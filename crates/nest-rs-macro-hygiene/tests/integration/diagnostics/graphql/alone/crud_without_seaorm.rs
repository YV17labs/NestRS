//! `#[crud]` in a build without `seaorm` names, in one error, every feature
//! its expansion needs — `seaorm` and the `authz` its operations carry — and
//! the attribute the developer wrote, never the `#[authorize]` it emits, nor
//! again for the `#[operations]` it re-emits; rustc said `E0433`. It
//! deliberately fails to resolve: the expansion names `nest_rs::seaorm` and
//! `nest_rs::authz`, which the build does not enable.

use std::sync::Arc;

use nest_rs::graphql::{crud, resolver};

struct NotesService;

#[resolver]
struct NotesResolver {
    #[inject]
    svc: Arc<NotesService>,
}

#[crud(service = NotesService, ops = [get])]
impl NotesResolver {}

fn main() {}
