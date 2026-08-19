//! `ops = []` declares nothing, and is refused rather than generating nothing.
//!
//! `version = []` was already refused by the same reasoning — "declares nothing
//! — drop the argument instead" — while `ops = []` produced a `#[crud]` block
//! with zero operations and no word. One question, two answers, both worded in
//! `nest-rs-codegen`, ten files apart.

use nest_rs_http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(service = svc, entity = OrgEntity, output = Org, ops = [])]
impl OrgsController {}

fn main() {}
