//! A value of the wrong kind names the decorator and the key, as every value
//! refusal does — `#[crud]`'s parser used to hand these to syn, whose `expected
//! identifier` named neither. One item per shape the parser reads: a name, a
//! list, a list's element, and a closed vocabulary.

use nest_rs::http::{controller, crud};

#[controller(path = "/a")]
struct AController;

#[crud(service = "svc", entity = OrgEntity, output = Org)]
impl AController {}

#[controller(path = "/b")]
struct BController;

#[crud(service = svc, entity = OrgEntity, output = Org, ops = list)]
impl BController {}

#[controller(path = "/c")]
struct CController;

#[crud(service = svc, entity = OrgEntity, output = Org, ops = ["list"])]
impl CController {}

#[controller(path = "/d")]
struct DController;

#[crud(service = svc, entity = OrgEntity, output = Org, paginate = "cursor")]
impl DController {}

fn main() {}
