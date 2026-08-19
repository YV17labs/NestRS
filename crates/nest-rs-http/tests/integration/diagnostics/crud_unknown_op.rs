//! A value outside a closed vocabulary is refused the same way an unknown key
//! is — naming the decorator, the position and the offender
//! (`nest_rs_codegen::unknown_value`).

use nest_rs_http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(service = svc, entity = OrgEntity, output = Org, ops = [list, upsert])]
impl OrgsController {}

fn main() {}
