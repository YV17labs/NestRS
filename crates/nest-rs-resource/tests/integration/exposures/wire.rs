//! Wire-only `#[expose]` (`src/exposures/wire.rs`): the emitted surface must
//! not pull in `async_graphql`, and [`WireModelDefaults`] reconstruction must
//! fill — and masking must strip — an unexposed column's audited placeholder.

use nest_rs_resource::expose;
use nest_rs_seaorm::{Creatable, CrudService, Deletable, Updatable};
use sea_orm::entity::prelude::*;

mod thing {
    use super::*;

    #[expose(name = "Thing", service = ThingsService)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "things")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create, update), validate(length(min = 1)))]
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub(super) struct ThingsService;

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

// A read-only resource: no `input(...)` column, so no `Create`/`Update` type.
mod reading {
    use super::*;

    #[expose(name = "Reading", service = ReadingsService)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "readings")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub label: String,
        #[expose]
        pub taken_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub(super) struct ReadingsService;

    impl CrudService for ReadingsService {
        type Entity = Entity;
    }
}

// An unexposed custom-enum column: only `#[wire_default]` can default it for
// masking reconstruction.
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
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create, update), validate(length(min = 1)))]
        pub name: String,
        #[wire_default]
        pub tier: Tier,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub(super) struct AccountsService;

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
    // The guard is that this file compiles with no `async_graphql` in scope.
    let _svc = thing::ThingsService;
    let _read = reading::ReadingsService;
    let _acct = account::AccountsService;
}

#[test]
fn wire_default_reconstructs_and_strips_an_unexposed_custom_enum() {
    use account::{Entity, Tier};
    use nest_rs_authz::WireModelDefaults;

    let mut body: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    Entity::fill_wire_defaults(&mut body);
    assert_eq!(
        body.get("tier"),
        Some(&serde_json::to_value(Tier::default()).expect("serialize default")),
        "bare #[wire_default] fills the column type's Default",
    );
    assert_eq!(body["tier"], serde_json::json!("free"));

    let mut present = serde_json::Map::new();
    present.insert("tier".into(), serde_json::json!("pro"));
    Entity::fill_wire_defaults(&mut present);
    assert_eq!(present["tier"], serde_json::json!("pro"));

    let keys = Entity::wire_keys().expect("an #[expose]d entity yields a key set");
    assert!(keys.contains(&"id") && keys.contains(&"name"));
    assert!(!keys.contains(&"tier"), "the placeholder is not a wire key");
}

#[test]
fn an_id_and_a_timestamp_keep_their_format_on_the_wire_string() {
    let schema = nest_rs_resource::__private::schemars::schema_for!(reading::Reading);
    let json = serde_json::to_value(&schema).expect("the schema serializes");
    let properties = &json["properties"];
    assert_eq!(properties["id"]["type"], "string", "{json}");
    assert_eq!(properties["id"]["format"], "uuid", "{json}");
    assert_eq!(properties["taken_at"]["type"], "string", "{json}");
    assert_eq!(properties["taken_at"]["format"], "date-time", "{json}");
    assert!(
        properties["label"].get("format").is_none(),
        "a plain String claims no format: {json}",
    );
}

#[test]
fn a_timestamp_reads_as_the_model_itself_serializes_it() {
    use sea_orm::prelude::DateTimeWithTimeZone;

    for raw in ["2026-10-08T12:30:00.250+00:00", "2026-10-08T14:30:00+02:00"] {
        let taken_at = DateTimeWithTimeZone::parse_from_rfc3339(raw).expect("a timestamp");
        let model = reading::Model {
            id: Uuid::nil(),
            label: "x".into(),
            taken_at,
        };
        let wire = reading::Reading::from(&model);
        assert_eq!(
            serde_json::Value::String(wire.taken_at),
            serde_json::to_value(taken_at).expect("serialize"),
            "a masked reply is re-serialized from the model: both paths spell {raw} alike",
        );
    }
    let utc = reading::Reading::from(&reading::Model {
        id: Uuid::nil(),
        label: "x".into(),
        taken_at: DateTimeWithTimeZone::parse_from_rfc3339("2026-10-08T12:30:00+00:00")
            .expect("a timestamp"),
    });
    assert_eq!(utc.taken_at, "2026-10-08T12:30:00Z");
}
