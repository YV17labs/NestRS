//! The `paginate` vocabulary is closed too, and its refusal used to name
//! neither the decorator nor the key — a bare `expected `cursor` or `none``
//! left the reader to guess which of seven keys the compiler meant.

use nest_rs::http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(service = OrgsService, paginate = pages)]
impl OrgsController {}

fn main() {}
