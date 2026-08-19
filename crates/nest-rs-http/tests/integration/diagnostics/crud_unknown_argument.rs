//! An unknown `#[crud]` key is refused by name, listing the ones it takes.
//!
//! One wording for every decorator whose arguments are `key = value`
//! (`nest_rs_codegen::unknown_argument`), because the sentence used to be
//! written per decorator and had drifted into two forms — one of which did not
//! name the offending key at all.

use nest_rs_http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(service = svc, entity = OrgEntity, output = Org, cached = true)]
impl OrgsController {}

fn main() {}
