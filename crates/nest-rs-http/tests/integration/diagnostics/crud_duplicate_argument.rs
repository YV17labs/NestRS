//! A `#[crud]` key written twice is refused rather than resolved by source
//! order.
//!
//! Accepting the repeat drops one of two declarations, and here the dropped one
//! decides which entity the resource exposes.

use nest_rs_http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(service = svc, entity = OrgEntity, entity = OtherEntity, output = Org)]
impl OrgsController {}

fn main() {}
