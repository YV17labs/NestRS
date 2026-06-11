//! Compile-time guard: wire-only `#[expose]` must not pull in `async_graphql`.
//! Run with `cargo test -p nest-rs-resource --no-default-features`.

use nest_rs_resource::expose;
use nest_rs_seaorm::CrudService;
use sea_orm::entity::prelude::*;

#[expose(name = "Thing", service = ThingsService)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "things")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub name: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

struct ThingsService;

impl CrudService for ThingsService {
    type Entity = Entity;
    type Create = CreateThingInput;
    type Update = UpdateThingInput;
}

#[test]
fn wire_only_expose_compiles_without_graphql_tokens() {
    assert!(true, "this test file compiling is the regression guard");
}
