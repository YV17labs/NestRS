//! Wire-only `#[expose]` (`src/wire.rs` in the macro crate): the emitted
//! surface pulls in no `async_graphql`, and the round trip a masked reply runs
//! on a row writes the exposed columns as the wire type does and rebuilds every
//! hidden one from its placeholder — the `Model` itself derives no serde.

use nest_rs_authz::__private::WireModel;
use nest_rs_authz::{AbilityBuilder, Action};
use nest_rs_seaorm::{Creatable, CrudService, Deletable, Updatable, expose};
use sea_orm::entity::prelude::*;
use serde_json::{Map, Value, json};

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
// masking reconstruction, and it needs no serde of its own.
mod account {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default, EnumIter, DeriveActiveEnum)]
    #[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
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

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default, EnumIter, DeriveActiveEnum)]
    #[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
    pub enum Tier {
        #[default]
        #[sea_orm(string_value = "free")]
        Free,
        #[sea_orm(string_value = "pro")]
        Pro,
    }

    #[expose(name = "Ledger")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "ledgers")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
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
        #[wire_default]
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

    #[expose(name = "Vault")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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
// from the value its `#[wire_default]` holds, or `None` where it is optional.
mod keyring {
    use super::*;
    use nest_rs_core::Secret;

    #[expose(name = "Keyring")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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

/// A wire object as a masked reply carries one.
fn object(wire: Value) -> Map<String, Value> {
    match wire {
        Value::Object(row) => row,
        other => panic!("a row is an object, not {other}"),
    }
}

/// A secret column is never exposed, so a masked reply rebuilds it: bare, it
/// takes its value's placeholder, and optional, `None` — never a 500 for want
/// of one.
#[test]
fn a_hidden_secret_column_is_rebuilt_from_its_value_s_placeholder() {
    use vault::{Column, Entity};

    let rebuilt = Entity::from_wire(&object(json!({ "id": Uuid::nil(), "name": "ops" })))
        .expect("the row rebuilds");
    assert_eq!(rebuilt.api_key_hash.expose_secret(), "");
    assert!(rebuilt.recovery_hash.is_none());
    assert_eq!(rebuilt.name, "ops");
    assert!(Entity::is_placeholdered(Column::ApiKeyHash));
    assert!(Entity::is_placeholdered(Column::RecoveryHash));
    assert_eq!(Entity::wire_key(Column::ApiKeyHash), None);
}

/// A byte-like secret's `#[wire_default]` gives the value the column holds —
/// its type's `Default` or the expression's.
#[test]
fn a_hidden_byte_like_secret_is_rebuilt_from_its_wire_default_s_value() {
    use keyring::{Column, Entity};

    let model = Entity::from_wire(&object(json!({ "id": Uuid::nil() }))).expect("the row rebuilds");
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

/// A hidden column is never read off the wire, whatever an object carries
/// under its key — a `"<redacted>"` a byte-like secret could not read back
/// included: the rebuild gives it its placeholder.
#[test]
fn a_hidden_column_an_object_carries_is_rebuilt_from_its_placeholder() {
    let keys = keyring::Entity::from_wire(&object(json!({
        "id": Uuid::nil(),
        "seed": "<redacted>",
        "pepper": [1, 2, 3],
    })))
    .expect("the row rebuilds");
    assert!(keys.seed.expose_secret().is_empty());
    assert_eq!(keys.pepper.expose_secret(), &[7]);

    let ledger = ledger::Entity::from_wire(&object(json!({
        "id": Uuid::nil(),
        "name": "books",
        "type": "cash",
        "note": "the wire's",
        "tier": "pro",
    })))
    .expect("the row rebuilds");
    assert_eq!(ledger.note, "");
    assert_eq!(ledger.tier, ledger::Tier::Free);
}

/// The row's own `Debug` says nothing of a secret column, and the round trip
/// writes none: a secret is never exposed.
#[test]
fn a_secret_column_prints_as_its_placeholder_and_never_reaches_the_wire() {
    let model = vault::Model {
        id: Uuid::nil(),
        name: "ops".into(),
        api_key_hash: "$argon2id$v=19$key".into(),
        recovery_hash: Some("$argon2id$v=19$recovery".into()),
    };
    let active = vault::ActiveModel::from(model.clone());
    for printed in [format!("{model:?}"), format!("{active:?}")] {
        assert!(!printed.contains("argon2id"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(printed.contains("ops"), "the other columns stay: {printed}");
    }
    let wire = vault::Entity::to_wire(&model).expect("the row writes");
    assert_eq!(
        Value::Object(wire),
        json!({ "id": Uuid::nil(), "name": "ops" })
    );
}

/// A rule reading a column the rebuilt row invented cannot be decided there, so
/// the derive names exactly the columns its rebuild invents.
#[test]
fn a_placeholdered_column_is_exactly_one_the_rebuild_invents() {
    use ledger::{Column, Entity};
    use sea_orm::Iterable;

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
            "OwnerId",
            "Kind",
            "Ref"
        ],
        "every hidden column, the variant of each read as sea-orm reads `enum_name` and `r#`; \
         an exposed column is never invented",
    );
    let rebuilt = Entity::from_wire(&object(json!({
        "id": Uuid::nil(),
        "name": "books",
        "type": "cash",
    })))
    .expect("the row rebuilds");
    assert_eq!(
        (
            rebuilt.note.as_str(),
            rebuilt.archived_at,
            rebuilt.locked,
            rebuilt.revision
        ),
        ("", None, false, 0),
    );
    assert_eq!(
        (rebuilt.owner_id, rebuilt.r#ref.as_str()),
        (Uuid::nil(), "")
    );
    assert!(
        rebuilt.headline.is_none(),
        "an `Option` the object left out reads `None`"
    );
}

/// An exposed column the object leaves out is no placeholder to invent: the
/// rebuild fails, and the reply with it, closed.
#[test]
fn an_exposed_column_the_object_leaves_out_fails_the_rebuild() {
    let refused = ledger::Entity::from_wire(&object(json!({ "id": Uuid::nil(), "type": "cash" })))
        .expect_err("`name` is exposed and absent");
    assert_eq!(refused.to_string(), "missing field `name`");
}

/// An exposed column is keyed as its field serializes, whatever name the
/// database knows it by; a hidden one never crosses the wire.
#[test]
fn an_exposed_column_is_keyed_by_its_field_and_a_hidden_one_is_not() {
    use ledger::{Column, Entity};

    assert_eq!(Entity::wire_key(Column::Id), Some("id"));
    assert_eq!(Entity::wire_key(Column::Name), Some("name"));
    assert_eq!(Entity::wire_key(Column::Type), Some("type"));
    assert_eq!(
        Entity::wire_key(Column::Headline),
        Some("headline"),
        "`column_name` moves the database's name, not the wire's",
    );
    for hidden in [Column::Note, Column::Kind, Column::Ref, Column::OwnerId] {
        assert_eq!(Entity::wire_key(hidden), None, "{hidden:?}");
    }
}

/// `.fields` names a column, not a spelling of it: `column_name` moves the
/// database's name, and the grant still keeps what the wire calls that column.
#[test]
fn a_field_grant_on_a_renamed_column_keeps_its_wire_key() {
    let mut grants = AbilityBuilder::new();
    grants
        .can(Action::Read, ledger::Entity)
        .fields([ledger::Column::Id, ledger::Column::Headline]);
    let ability = grants.build().expect("a valid ability");
    let row = ledger::Model {
        id: Uuid::nil(),
        name: "ledger".to_owned(),
        r#type: "cash".to_owned(),
        headline: Some("Q3".to_owned()),
        note: String::new(),
        archived_at: None,
        locked: false,
        revision: 0,
        tier: ledger::Tier::Free,
        owner_id: Uuid::nil(),
        category: String::new(),
        r#ref: String::new(),
    };

    assert_eq!(
        ability.mask::<ledger::Entity>(Action::Read, &row),
        json!({ "id": Uuid::nil(), "headline": "Q3" }),
    );
    assert_eq!(
        ability.mask_many::<ledger::Entity>(Action::Read, [&row]),
        [json!({ "id": Uuid::nil(), "headline": "Q3" })],
    );
}

/// The way back from a key to the column a field grant names, for each exposed
/// column and no hidden one.
#[test]
fn a_wire_key_reads_back_to_the_column_as_the_table_names_it() {
    use ledger::Entity;

    assert_eq!(Entity::key_column("id"), Some("id"));
    assert_eq!(Entity::key_column("headline"), Some("display_title"));
    assert_eq!(Entity::key_column("type"), Some("type"));
    assert_eq!(
        Entity::key_column("display_title"),
        None,
        "the database's name is no key"
    );
    assert_eq!(Entity::key_column("note"), None, "a hidden column");
}

/// A raw identifier serializes without its `r#`, so its key does too.
#[test]
fn a_raw_identifier_is_keyed_as_it_serializes() {
    let keys = ledger::Entity::WIRE_KEYS;
    assert!(keys.contains(&"type"), "{keys:?}");
    assert!(!keys.iter().any(|key| key.starts_with("r#")), "{keys:?}");
}

/// The round trip writes a row as its exposed type does, so a masked reply and
/// an unmasked one spell every column alike, an id and a timestamp included.
#[test]
fn the_round_trip_writes_a_row_as_its_exposed_type_does() {
    for raw in ["2026-10-08T12:30:00.250+00:00", "2026-10-08T14:30:00+02:00"] {
        let model = reading::Model {
            id: Uuid::now_v7(),
            label: "x".into(),
            taken_at: DateTimeWithTimeZone::parse_from_rfc3339(raw).expect("a timestamp"),
        };
        let wire = reading::Entity::to_wire(&model).expect("the row writes");
        assert_eq!(
            Value::Object(wire.clone()),
            serde_json::to_value(reading::Reading::from(&model)).expect("the wire type writes"),
            "{raw}",
        );
        let back = reading::Entity::from_wire(&wire).expect("the row reads back");
        assert_eq!(back, model, "{raw}");
    }
    let utc = reading::Model {
        id: Uuid::nil(),
        label: "x".into(),
        taken_at: DateTimeWithTimeZone::parse_from_rfc3339("2026-10-08T12:30:00+00:00")
            .expect("a timestamp"),
    };
    let wire = reading::Entity::to_wire(&utc).expect("the row writes");
    assert_eq!(wire["taken_at"], json!("2026-10-08T12:30:00Z"));
}

#[test]
fn wire_only_expose_compiles_without_graphql_tokens() {
    // The guard is that this file compiles with no `async_graphql` in scope.
    let _svc = thing::ThingsService;
    let _read = reading::ReadingsService;
    let _acct = account::AccountsService;
}

#[test]
fn wire_default_rebuilds_and_strips_an_unexposed_custom_enum() {
    use account::{Entity, Tier};

    let rebuilt = Entity::from_wire(&object(json!({ "id": Uuid::nil(), "name": "Ada" })))
        .expect("the row rebuilds");
    assert_eq!(
        rebuilt.tier,
        Tier::default(),
        "bare #[wire_default] is the type's Default"
    );

    let wire = Entity::to_wire(&account::Model {
        tier: Tier::Pro,
        ..rebuilt
    })
    .expect("the row writes");
    assert!(
        !wire.contains_key("tier"),
        "the placeholder is not a wire key"
    );
    assert!(Entity::WIRE_KEYS.contains(&"id") && Entity::WIRE_KEYS.contains(&"name"));
    assert!(!Entity::WIRE_KEYS.contains(&"tier"));
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

/// The wire-only arm: `#[wire_enum]`'s enum as a column of an entity with no
/// GraphQL object.
mod enum_column {
    use nest_rs_core::wire_enum;
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[wire_enum]
    #[derive(EnumIter, DeriveActiveEnum)]
    #[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
    #[serde(rename_all = "lowercase")]
    pub enum Stage {
        #[sea_orm(string_value = "draft")]
        Draft,
        #[sea_orm(string_value = "shipped")]
        Shipped,
    }

    #[expose(name = "Release")]
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
}
