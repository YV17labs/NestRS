//! Wire-only `#[expose]` (`src/wire.rs` in the macro crate): the emitted
//! surface must not pull in `async_graphql`, and [`WireModelDefaults`]
//! reconstruction must fill — and masking must strip — an unexposed column's
//! audited placeholder.

use nest_rs_seaorm::{Creatable, CrudService, Deletable, Updatable, expose};
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

// Every shape a hidden column takes, and the two spellings that move its
// `Column` variant or its key away from its ident.
mod ledger {
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

    #[expose(name = "Ledger")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "ledgers")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        #[expose]
        pub name: String,
        #[expose]
        pub r#type: String,
        #[sea_orm(column_name = "display_title")]
        #[expose]
        pub headline: Option<String>,
        pub note: String,
        pub archived_at: Option<DateTimeWithTimeZone>,
        pub locked: bool,
        pub revision: i64,
        #[wire_default]
        pub tier: Tier,
        pub owner_id: Uuid,
        #[sea_orm(enum_name = "Kind")]
        pub category: String,
        pub r#ref: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// A rule reading a column the rebuilt row invented cannot be decided there, so
/// the derive names exactly the columns its fill may invent.
#[test]
fn a_placeholdered_column_is_exactly_one_the_fill_may_invent() {
    use ledger::{Column, Entity};
    use nest_rs_authz::WireModelDefaults;
    use sea_orm::Iterable;

    let mut filled = serde_json::Map::new();
    Entity::fill_wire_defaults(&mut filled);
    let mut invented: Vec<&str> = filled.keys().map(String::as_str).collect();
    invented.sort_unstable();
    assert_eq!(
        invented,
        [
            "archived_at",
            "category",
            "locked",
            "note",
            "ref",
            "revision",
            "tier"
        ],
        "a primary key, an exposed column and a type with no placeholder are never invented",
    );

    let placeholdered: Vec<String> = Column::iter()
        .filter(|column| Entity::is_placeholdered(*column))
        .map(|column| format!("{column:?}"))
        .collect();
    assert_eq!(
        placeholdered,
        [
            "Note",
            "ArchivedAt",
            "Locked",
            "Revision",
            "Tier",
            "Kind",
            "Ref"
        ],
        "the variant of each invented column, `enum_name` and `r#` read as sea-orm reads them",
    );
}

/// An exposed column is keyed as its field serializes, whatever name the
/// database knows it by; a hidden one never crosses the wire.
#[test]
fn an_exposed_column_is_keyed_by_its_field_and_a_hidden_one_is_not() {
    use ledger::{Column, Entity};
    use nest_rs_authz::WireModelDefaults;

    assert_eq!(Entity::wire_key(Column::Name), Some("name"));
    assert_eq!(Entity::wire_key(Column::Type), Some("type"));
    assert_eq!(
        Entity::wire_key(Column::Headline),
        Some("headline"),
        "`column_name` moves the database's name, not the wire's",
    );
    for hidden in [Column::Id, Column::Note, Column::Kind, Column::Ref] {
        assert_eq!(Entity::wire_key(hidden), None, "{hidden:?}");
    }
}

/// A raw identifier serializes without its `r#`, so its key does too.
#[test]
fn a_raw_identifier_is_keyed_as_it_serializes() {
    use ledger::Entity;
    use nest_rs_authz::WireModelDefaults;

    let keys = Entity::wire_keys().expect("an #[expose]d entity yields a key set");
    assert!(keys.contains(&"type"), "{keys:?}");
    assert!(!keys.iter().any(|key| key.starts_with("r#")), "{keys:?}");
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
    let schema = nest_rs_seaorm::__private::schemars::schema_for!(reading::Reading);
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
