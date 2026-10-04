//! `#[crud]` has three required keys, all through the one
//! `nest_rs::codegen::missing_argument` sentence — the crate that owns the
//! wording was itself three of the family's eight hand-written copies.

use nest_rs::http::{controller, crud};

#[controller(path = "/widgets")]
pub struct WidgetsController;

#[crud(entity = widgets::Entity, output = Widget)]
impl WidgetsController {}

fn main() {}
