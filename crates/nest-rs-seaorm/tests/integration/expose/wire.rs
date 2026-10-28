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

// Hidden secret columns, bare and optional: a secret never crosses the wire, so
// the rebuild fills each with its value's own placeholder.
mod vault {
    use super::*;
    use nest_rs_core::Secret;
    use serde::{Deserialize, Serialize};

    #[expose(name = "Vault")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "vaults")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub name: String,
        pub api_key_hash: Secret<String>,
        pub recovery_hash: Option<Secret<String>>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

// Hidden byte-like secret columns: outside the narrow set, so each is filled
// from the value its `#[wire_default]` holds, or `null` where it is optional.
mod keyring {
    use super::*;
    use nest_rs_core::Secret;
    use serde::{Deserialize, Serialize};

    #[expose(name = "Keyring")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "keyrings")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[wire_default]
        pub seed: Secret<Vec<u8>>,
        #[wire_default(Secret::new(vec![7]))]
        pub pepper: Secret<Vec<u8>>,
        pub backup: Option<Secret<Vec<u8>>>,
        #[wire_default(Some(Secret::new(vec![9])))]
        pub spare: Option<Secret<Vec<u8>>>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

// A hidden byte-like secret with no `#[wire_default]`: no placeholder exists.
mod sealed {
    use super::*;
    use nest_rs_core::Secret;
    use serde::{Deserialize, Serialize};

    #[expose(name = "Sealed")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "sealeds")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        pub seed: Secret<Vec<u8>>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// What the mask's rebuild does with a wire object: fill, then deserialize.
fn rebuilt<E>(wire: serde_json::Value) -> (serde_json::Value, Result<E::Model, serde_json::Error>)
where
    E: nest_rs_authz::WireModelDefaults,
    E::Model: serde::de::DeserializeOwned,
{
    let serde_json::Value::Object(mut row) = wire else {
        panic!("a row is an object");
    };
    E::fill_wire_defaults(&mut row);
    let filled = serde_json::Value::Object(row);
    let model = serde_json::from_value(filled.clone());
    (filled, model)
}

/// A secret column is never exposed, so a masked reply rebuilds it: bare, it
/// takes its value's placeholder, and optional, `null` — never a 500 for want
/// of one.
#[test]
fn a_hidden_secret_column_is_rebuilt_from_its_value_s_placeholder() {
    use nest_rs_authz::WireModelDefaults;
    use vault::{Column, Entity, Model};

    let mut row = serde_json::Map::new();
    row.insert("id".into(), serde_json::json!(Uuid::nil()));
    row.insert("name".into(), serde_json::json!("ops"));
    Entity::fill_wire_defaults(&mut row);
    assert_eq!(row.get("api_key_hash"), Some(&serde_json::json!("")));
    assert_eq!(row.get("recovery_hash"), Some(&serde_json::Value::Null));
    let rebuilt: Model =
        serde_json::from_value(serde_json::Value::Object(row)).expect("the row rebuilds");
    assert_eq!(rebuilt.api_key_hash.expose_secret(), "");
    assert!(Entity::is_placeholdered(Column::ApiKeyHash));
    assert!(Entity::is_placeholdered(Column::RecoveryHash));
    assert_eq!(Entity::wire_key(Column::ApiKeyHash), None);
}

/// A byte-like secret's `#[wire_default]` fills the value the column holds —
/// its type's `Default` or the expression's — never the `"<redacted>"` a
/// `Secret` serializes as, which its value type does not read back.
#[test]
fn a_hidden_byte_like_secret_is_rebuilt_from_its_wire_default_s_value() {
    use keyring::{Column, Entity};
    use nest_rs_authz::WireModelDefaults;

    let (filled, model) = rebuilt::<Entity>(serde_json::json!({ "id": Uuid::nil() }));
    assert_eq!(
        filled,
        serde_json::json!({
            "id": Uuid::nil(),
            "seed": [],
            "pepper": [7],
            "backup": null,
            "spare": [9],
        })
    );
    let model = model.expect("the row rebuilds");
    assert!(model.seed.expose_secret().is_empty());
    assert_eq!(model.pepper.expose_secret(), &[7]);
    assert!(model.backup.is_none());
    assert_eq!(
        model
            .spare
            .as_ref()
            .map(|spare| spare.expose_secret().as_slice()),
        Some([9].as_slice())
    );
    for column in [Column::Seed, Column::Pepper, Column::Backup, Column::Spare] {
        assert!(Entity::is_placeholdered(column), "{column:?}");
    }
}

/// A handler returning the raw `Model` sends each secret as `"<redacted>"`:
/// the fill writes over it, so the row still rebuilds, whatever the value type.
#[test]
fn a_raw_model_s_redacted_secrets_are_written_over_before_the_rebuild() {
    let keys = keyring::Model {
        id: Uuid::nil(),
        seed: vec![1, 2, 3].into(),
        pepper: vec![4].into(),
        backup: Some(vec![5].into()),
        spare: None,
    };
    let (filled, model) =
        rebuilt::<keyring::Entity>(serde_json::to_value(&keys).expect("the row serializes"));
    assert_eq!(
        filled,
        serde_json::json!({
            "id": Uuid::nil(),
            "seed": [],
            "pepper": [7],
            "backup": null,
            "spare": [9],
        })
    );
    model.expect("the row rebuilds");

    let vault = vault::Model {
        id: Uuid::nil(),
        name: "ops".into(),
        api_key_hash: "$argon2id$v=19$key".into(),
        recovery_hash: Some("$argon2id$v=19$recovery".into()),
    };
    let (filled, model) =
        rebuilt::<vault::Entity>(serde_json::to_value(&vault).expect("the row serializes"));
    assert_eq!(filled["api_key_hash"], serde_json::json!(""));
    assert_eq!(filled["recovery_hash"], serde_json::Value::Null);
    assert_eq!(filled["name"], serde_json::json!("ops"));
    model.expect("the row rebuilds");
}

/// A byte-like secret has no placeholder of its own: with no `#[wire_default]`
/// the fill invents nothing, and the rebuild fails closed.
#[test]
fn a_hidden_byte_like_secret_without_a_wire_default_fails_closed() {
    use nest_rs_authz::WireModelDefaults;
    use sealed::{Column, Entity};

    let (filled, model) = rebuilt::<Entity>(serde_json::json!({ "id": Uuid::nil() }));
    assert_eq!(filled, serde_json::json!({ "id": Uuid::nil() }));
    let refused = model.expect_err("a missing column fails the rebuild");
    assert!(
        refused.to_string().contains("missing field `seed`"),
        "{refused}"
    );
    assert!(!Entity::is_placeholdered(Column::Seed));
}

/// The row's own derives say nothing of a secret column: `Debug` and its
/// serialization print the placeholder.
#[test]
fn a_secret_column_prints_and_serializes_as_its_placeholder() {
    let model = vault::Model {
        id: Uuid::nil(),
        name: "ops".into(),
        api_key_hash: "$argon2id$v=19$key".into(),
        recovery_hash: Some("$argon2id$v=19$recovery".into()),
    };
    let active = vault::ActiveModel::from(model.clone());
    for printed in [
        format!("{model:?}"),
        format!("{active:?}"),
        serde_json::to_string(&model).expect("the row serializes"),
    ] {
        assert!(!printed.contains("argon2id"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(printed.contains("ops"), "the other columns stay: {printed}");
    }
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
