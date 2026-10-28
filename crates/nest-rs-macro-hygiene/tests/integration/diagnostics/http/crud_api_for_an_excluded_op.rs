//! `api(…)` documents one generated operation, so naming one that `ops` leaves
//! out is two declarations disagreeing — refused as `paginate` is, never a doc
//! dropped without a word.

use nest_rs::http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(
    service = svc,
    entity = OrgEntity,
    output = Org,
    ops = [list],
    api(get(summary = "Get an org by id")),
)]
impl OrgsController {}

fn main() {}
