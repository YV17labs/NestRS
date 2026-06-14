//! Compile-time guard: wire-only `#[expose]` must not pull in `async_graphql`.
//! Run with `cargo test -p nest-rs-resource --no-default-features`.

use nest_rs_resource::expose;
use nest_rs_seaorm::{Creatable, CrudService, Deletable, Updatable};
use sea_orm::entity::prelude::*;

mod thing {
    use super::*;

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

    pub struct ThingsService;

    impl CrudService for ThingsService {
        type Entity = Entity;
    }

    impl Creatable for ThingsService {
        type Create = CreateThing;
    }

    impl Updatable for ThingsService {
        type Update = UpdateThing;
    }

    impl Deletable for ThingsService {}
}

// A read-only resource: it has **no** `input(create/update)` column, so the
// `#[expose]` macro emits no `CreateReading`/`UpdateReading` type — and with the
// segregated traits the service implements only `CrudService`, declaring no
// `Create`/`Update` placeholder. This is the case that previously forced a
// `struct … { _unused }` stub.
mod reading {
    use super::*;

    #[expose(name = "Reading", service = ReadingsService)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "readings")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub label: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub struct ReadingsService;

    // No `Creatable`/`Updatable`/`Deletable` — and no placeholder write type.
    impl CrudService for ReadingsService {
        type Entity = Entity;
    }
}

#[test]
fn wire_only_expose_compiles_without_graphql_tokens() {
    // The regression guard is that this file compiles and the `CrudService`
    // impls above resolve with no `async_graphql` tokens in scope.
    // Constructing the services exercises those impls at runtime.
    let _svc = thing::ThingsService;
    let _read = reading::ReadingsService;
}
