//! The third op-specific key: `paginate` configures the `list` op, so writing it
//! beside an `ops` that leaves `list` out is two declarations disagreeing — and
//! `paginate = none`, the documented opt-out, was the value dropped without a
//! word.

use nest_rs::http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(service = svc, entity = OrgEntity, output = Org, ops = [get], paginate = none)]
impl OrgsController {}

fn main() {}
