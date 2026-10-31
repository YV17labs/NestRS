//! `service` names the service's type, which `#[crud]` reaches through the
//! host's `#[inject]` field of that type: a field's name, the key's earlier
//! spelling, is refused naming the type to write.

use std::sync::Arc;

use nest_rs::core::injectable;
use nest_rs::http::{controller, crud};

#[injectable]
#[derive(Default)]
struct NotesService;

#[controller(path = "/notes")]
struct NotesController {
    #[inject]
    svc: Arc<NotesService>,
}

#[crud(service = svc, ops = [get])]
impl NotesController {}

fn main() {}
