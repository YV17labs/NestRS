//! An enum column on a GraphQL `#[expose]`d entity: `#[wire_enum(graphql)]`'s enum
//! reaches both sides of an operation, the wire type and the generated input.

use nest_rs_core::wire_enum;
use nest_rs_seaorm::__private::async_graphql;
use nest_rs_seaorm::{CrudService, expose};
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
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "releases")]
pub(super) struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose(input(create, update))]
    pub stage: Stage,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub(super) enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

pub(super) struct ReleasesService;

impl CrudService for ReleasesService {
    type Entity = Entity;
}

#[test]
fn a_graphql_wire_enum_is_a_schema_enum_on_both_sides_of_an_operation() {
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
