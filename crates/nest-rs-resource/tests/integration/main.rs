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

// An entity with an **unexposed custom-enum** column: the built-in
// `default_value_tokens` type match refuses a custom enum (it can't tell the
// placeholder from a real value), so the audited `#[wire_default]` opt-in is the
// only way its masking reconstruction (`fill_wire_defaults`) can default the
// hidden column. This is the exact shape the demo's `user.role` uses.
mod account {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(
        Clone,
        Copy,
        Debug,
        PartialEq,
        Eq,
        Default,
        Serialize,
        Deserialize,
        EnumIter,
        DeriveActiveEnum,
    )]
    #[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
    #[serde(rename_all = "lowercase")]
    pub enum Tier {
        #[default]
        #[sea_orm(string_value = "free")]
        Free,
        #[sea_orm(string_value = "pro")]
        Pro,
    }

    #[expose(name = "Account", service = AccountsService)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "accounts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create, update), validate(length(min = 1)))]
        pub name: String,
        // Unexposed, non-defaultable custom enum — the placeholder comes from
        // `#[wire_default]` (bare ⇒ `Tier::default()`), stripped before the wire.
        #[wire_default]
        pub tier: Tier,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub struct AccountsService;

    impl CrudService for AccountsService {
        type Entity = Entity;
    }

    impl Creatable for AccountsService {
        type Create = CreateAccount;
    }

    impl Updatable for AccountsService {
        type Update = UpdateAccount;
    }

    impl Deletable for AccountsService {}
}

#[test]
fn wire_only_expose_compiles_without_graphql_tokens() {
    // The regression guard is that this file compiles and the `CrudService`
    // impls above resolve with no `async_graphql` tokens in scope.
    // Constructing the services exercises those impls at runtime.
    let _svc = thing::ThingsService;
    let _read = reading::ReadingsService;
    let _acct = account::AccountsService;
}

#[test]
fn wire_default_reconstructs_and_strips_an_unexposed_custom_enum() {
    use account::{Entity, Tier};
    use nest_rs_resource::WireModelDefaults;

    // Reconstruction: a wire body (which omits the hidden column) gets the
    // audited placeholder so the `Model` can deserialize for masking.
    let mut body: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    Entity::fill_wire_defaults(&mut body);
    assert_eq!(
        body.get("tier"),
        Some(&serde_json::to_value(Tier::default()).expect("serialize default")),
        "bare #[wire_default] fills the column type's Default",
    );
    assert_eq!(body["tier"], serde_json::json!("free"));

    // Never overwrites a value the body already carried.
    let mut present = serde_json::Map::new();
    present.insert("tier".into(), serde_json::json!("pro"));
    Entity::fill_wire_defaults(&mut present);
    assert_eq!(present["tier"], serde_json::json!("pro"));

    // Masking strips it: the static exposed key-set that `retain_static_keys`
    // keys on never lists the unexposed column, so the placeholder can't leak.
    let keys = Entity::wire_keys().expect("an #[expose]d entity yields a key set");
    assert!(keys.contains(&"id") && keys.contains(&"name"));
    assert!(!keys.contains(&"tier"), "the placeholder is not a wire key");
}
