//! `#[crud]` has one required key, the service its operations delegate to and
//! read their types off, refused through the one
//! `nest_rs::codegen::missing_argument` sentence.

use nest_rs::http::{controller, crud};

#[controller(path = "/widgets")]
pub struct WidgetsController;

#[crud(ops = [list])]
impl WidgetsController {}

fn main() {}
