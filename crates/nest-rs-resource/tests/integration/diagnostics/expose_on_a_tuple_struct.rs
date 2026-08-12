//! The wire DTO's fields are named after the entity's columns, so there is
//! nothing to build one from without named fields.

use nest_rs_resource::expose;

#[expose(name = "Thing", service = ThingsService)]
#[derive(Clone, Debug, PartialEq)]
pub struct Model(pub sea_orm::prelude::Uuid);

fn main() {}
