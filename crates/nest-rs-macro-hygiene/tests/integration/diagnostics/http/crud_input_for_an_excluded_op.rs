//! The mirror of `crud_op_without_input`: an input type declared for an op
//! `ops` leaves out is refused at the type, never dropped — two declarations
//! disagreeing, one of them silently ignored.

use nest_rs::http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(service = svc, entity = OrgEntity, output = Org, create = CreateOrg, ops = [list, get])]
impl OrgsController {}

fn main() {}
