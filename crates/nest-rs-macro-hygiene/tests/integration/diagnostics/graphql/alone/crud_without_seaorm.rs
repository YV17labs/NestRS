//! `#[crud]` in a build without `seaorm` names the features its expansion
//! needs, `seaorm` and the `authz` its operations carry — once for itself and
//! once for the `#[operations]` it re-emits — where rustc said `E0433`. It
//! deliberately fails to resolve: the expansion names `nest_rs::seaorm` and
//! `nest_rs::authz`, which the build does not enable.

use std::sync::Arc;

use nest_rs::graphql::async_graphql::SimpleObject;
use nest_rs::graphql::{crud, resolver};

struct Note;
struct NotesService;

#[derive(SimpleObject)]
#[graphql(crate = "::nest_rs::graphql::async_graphql")]
struct NoteView {
    id: i32,
}

#[resolver]
struct NotesResolver {
    #[inject]
    svc: Arc<NotesService>,
}

#[crud(service = svc, entity = Note, output = NoteView, ops = [get])]
impl NotesResolver {}

fn main() {}
