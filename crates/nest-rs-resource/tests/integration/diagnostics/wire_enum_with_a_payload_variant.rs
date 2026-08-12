//! A GraphQL enum and a SeaORM `DeriveActiveEnum` column both require unit
//! variants; the refusal says what to do with the payload instead of leaving
//! two foreign derives to complain about it in their own words.

use nest_rs_resource::wire_enum;

#[wire_enum]
pub enum Tier {
    Free,
    Pro(u8),
}

fn main() {}
