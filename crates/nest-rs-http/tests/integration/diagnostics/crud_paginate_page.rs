//! `paginate = page` (offset) is not implemented on the REST list op — asking
//! for it names the supported modes instead of silently falling back.

use nest_rs_http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(service = svc, entity = OrgEntity, output = Org, paginate = page)]
impl OrgsController {}

fn main() {}
