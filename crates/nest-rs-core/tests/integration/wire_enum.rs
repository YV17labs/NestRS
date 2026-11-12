//! `#[wire_enum]` (`src/wire_enum.rs` in the macro crate): the enum both decoded
//! and sent. Part of the assertion is the compile: this file names neither
//! `serde` nor `schemars` anywhere a derive could reach.

use nest_rs_core::wire_enum;

#[wire_enum]
#[serde(rename_all = "lowercase")]
pub(crate) enum Tier {
    Free,
    Pro,
}

#[test]
fn a_wire_enum_round_trips_through_serde_under_the_authors_own_rename() {
    assert_eq!(serde_json::to_value(Tier::Pro).expect("serialize"), "pro");
    let parsed: Tier = serde_json::from_value(serde_json::json!("free")).expect("deserialize");
    assert_eq!(parsed, Tier::Free);
}

#[test]
fn a_wire_enum_carries_the_value_shape_the_derives_require() {
    // `async_graphql::Enum` and SeaORM's `DeriveActiveEnum` both require these.
    fn assert_shape<T: Copy + Clone + Eq + std::fmt::Debug>(_: T) {}
    assert_shape(Tier::Free);
}

#[test]
fn a_wire_enum_describes_itself_to_openapi() {
    let schema = nest_rs_core::__private::schemars::schema_for!(Tier);
    let json = serde_json::to_value(&schema).expect("the schema serializes");
    assert_eq!(
        json.get("enum"),
        Some(&serde_json::json!(["free", "pro"])),
        "the schema carries the renamed variants, not the Rust idents: {json}",
    );
}
