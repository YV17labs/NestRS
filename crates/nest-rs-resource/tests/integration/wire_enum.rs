//! `#[wire_enum]` (`src/wire_enum.rs` in the macro crate): the enum mode of
//! `#[expose]`.
//!
//! The class this closes: an `#[expose]`d column of a custom enum passes
//! through to the wire DTO verbatim, so the *enum* had to carry `Serialize`,
//! `Deserialize`, `JsonSchema` and `async_graphql::Enum` — four derives whose
//! expansions target the call site's prelude, i.e. two crates in the entity
//! crate's manifest for code it never wrote. The assertion below is partly the
//! compile itself: this file names neither `schemars` nor `async_graphql`
//! anywhere a derive could reach.

use nest_rs_resource::wire_enum;

#[wire_enum]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Free,
    Pro,
}

#[test]
fn a_wire_enum_round_trips_through_serde_under_the_authors_own_rename() {
    // `#[serde(rename_all = …)]` is the developer's, and it still binds: the
    // decorator emits `#[serde(crate = …)]` *before* the item, so the two land
    // on one derive rather than the helper preceding the derive that claims it.
    assert_eq!(serde_json::to_value(Tier::Pro).expect("serialize"), "pro");
    let parsed: Tier = serde_json::from_value(serde_json::json!("free")).expect("deserialize");
    assert_eq!(parsed, Tier::Free);
}

#[test]
fn a_wire_enum_carries_the_value_shape_the_derives_require() {
    // `Copy` + `Eq` are not decoration: `async_graphql::Enum` and SeaORM's
    // `DeriveActiveEnum` both require them, and a developer who had to supply
    // them by hand met the bound failure inside someone else's expansion.
    fn assert_shape<T: Copy + Clone + Eq + std::fmt::Debug>(_: T) {}
    assert_shape(Tier::Free);
}

#[test]
fn a_wire_enum_describes_itself_to_openapi() {
    // `JsonSchema` reached through the surface crate's re-export — the entity
    // crate declares no `schemars`, which is the whole point of the override.
    let schema = nest_rs_resource::schemars::schema_for!(Tier);
    let json = serde_json::to_value(&schema).expect("the schema serializes");
    assert_eq!(
        json.get("enum"),
        Some(&serde_json::json!(["free", "pro"])),
        "the schema carries the renamed variants, not the Rust idents: {json}",
    );
}

#[cfg(feature = "graphql")]
mod graphql {
    use nest_rs_resource::graphql::async_graphql;
    use nest_rs_resource::{expose, wire_enum};
    use nest_rs_seaorm::CrudService;
    use sea_orm::entity::prelude::*;

    #[wire_enum(graphql)]
    #[derive(EnumIter, DeriveActiveEnum)]
    #[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
    #[serde(rename_all = "lowercase")]
    pub enum Stage {
        #[sea_orm(string_value = "draft")]
        Draft,
        #[sea_orm(string_value = "shipped")]
        Shipped,
    }

    #[expose(name = "Release", service = ReleasesService, graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    #[sea_orm(table_name = "releases")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        // The column the whole decorator exists for: a custom enum crossing
        // the wire on both transports, declared once.
        #[expose(input(create, update))]
        pub stage: Stage,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub struct ReleasesService;

    impl CrudService for ReleasesService {
        type Entity = Entity;
    }

    #[test]
    fn a_graphql_wire_enum_is_a_schema_enum_on_both_sides_of_an_operation() {
        // `Enum` is emitted through the surface crate's `crate = ` override, so
        // this file's crate declares no `async-graphql` — and the type is
        // usable as both an argument and a field, which is what an entity
        // column needs.
        assert_eq!(
            <Stage as async_graphql::InputType>::type_name(),
            "Stage",
            "the enum names itself in the SDL",
        );
        assert_eq!(
            <Stage as async_graphql::OutputType>::type_name(),
            <Stage as async_graphql::InputType>::type_name(),
        );
    }

    #[test]
    fn the_enum_column_reaches_the_wire_dto_verbatim() {
        let id = Uuid::now_v7();
        let wire = Release::from(&Model {
            id,
            stage: Stage::Shipped,
        });
        assert_eq!(wire.stage, Stage::Shipped);
        assert_eq!(
            serde_json::to_value(&wire).expect("the wire DTO serializes")["stage"],
            serde_json::json!("shipped"),
        );
    }

    #[test]
    fn the_enum_column_reaches_the_generated_input_too() {
        let create = CreateRelease {
            stage: Stage::Draft,
        };
        assert_eq!(create.stage, Stage::Draft);
    }
}
