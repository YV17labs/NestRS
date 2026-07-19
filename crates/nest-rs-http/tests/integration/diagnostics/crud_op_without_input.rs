//! `ops = [create]` without `create = <InputType>` is a hard compile error
//! naming the op and the capability trait — never a silently dropped op.

use nest_rs_http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(service = svc, entity = OrgEntity, output = Org, ops = [create])]
impl OrgsController {}

fn main() {}
